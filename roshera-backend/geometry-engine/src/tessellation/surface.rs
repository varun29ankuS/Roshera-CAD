//! Surface tessellation algorithms
//!
//! Indexed access into UV-grid sample arrays and triangle-strip vertex
//! indices is the canonical idiom for parametric tessellation — all `arr[i]`
//! and `grid[u][v]` sites are bounds-guaranteed by the (samples_u × samples_v)
//! grid dimensions established at the top of each tessellator. Matches the
//! numerical-kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::adaptive::compute_plane_axes;
use super::{AdaptiveTessellator, MeshVertex, TessellationParams, TriangleMesh};
use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::face::Face;
use crate::primitives::surface::Surface;
use crate::primitives::topology_builder::BRepModel;
use std::collections::HashMap;
use tracing;

/// Tessellate a face into triangles
pub fn tessellate_face(
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) {
    // Get surface
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return,
    };

    match surface.type_name() {
        "Plane" => tessellate_planar_face(face, model, mesh),
        "Cylinder" => tessellate_cylindrical_face(face, model, params, mesh),
        "Sphere" => tessellate_spherical_face(face, model, params, mesh),
        "Cone" => tessellate_conical_face(face, model, params, mesh),
        "Torus" => tessellate_toroidal_face(face, model, params, mesh),
        "NURBS" => tessellate_nurbs_face(face, model, params, mesh),
        _ => tessellate_generic_face(face, model, params, mesh),
    }
}

/// Tessellate a planar face using constrained Delaunay triangulation
fn tessellate_planar_face(face: &Face, model: &BRepModel, mesh: &mut TriangleMesh) {
    // Get surface and compute normal
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return,
    };

    let (u_range, v_range) = surface.parameter_bounds();
    let u_mid = (u_range.0 + u_range.1) / 2.0;
    let v_mid = (v_range.0 + v_range.1) / 2.0;

    let normal = face
        .normal_at(u_mid, v_mid, &model.surfaces)
        .unwrap_or(Vector3::Z);

    // Collect all vertices from outer loop and holes
    let mut all_vertices = Vec::new();
    let mut loop_boundaries = Vec::new();

    // Process outer loop
    if let Some(outer_loop) = model.loops.get(face.outer_loop) {
        let start_idx = all_vertices.len();
        sample_loop_3d_polygon(outer_loop, model, &mut all_vertices);
        let end_idx = all_vertices.len();
        if end_idx > start_idx {
            loop_boundaries.push((start_idx, end_idx, true)); // true = outer loop
        }
    }

    // Process inner loops (holes)
    for &inner_loop_id in &face.inner_loops {
        if let Some(inner_loop) = model.loops.get(inner_loop_id) {
            let start_idx = all_vertices.len();
            sample_loop_3d_polygon(inner_loop, model, &mut all_vertices);
            let end_idx = all_vertices.len();
            if end_idx > start_idx {
                loop_boundaries.push((start_idx, end_idx, false)); // false = inner loop (hole)
            }
        }
    }

    if all_vertices.len() < 3 {
        return;
    }

    // Triangulate. We unify the hole-free and holed cases on a single
    // bridged-ear-clipping algorithm:
    //
    //   * Project the loops to the face's tangent plane (2D).
    //   * Force outer CCW, every hole CW (shoelace-signed-area test).
    //   * For each hole, find a visible bridge target on the outer
    //     polygon and splice the hole into outer as a thin notch
    //     (Hertel 1985, also used by mapbox/earcut).
    //   * Ear-clip the resulting simple polygon.
    //
    // This replaced the previous Bowyer-Watson + constraint-enforcement
    // path, whose `enforce_edge_constraint` step silently corrupted the
    // triangulation by discarding the cavity-boundary edges (the
    // `_boundary_edges` collection at the old surface.rs:472 was
    // computed but never used) and falling back to a naive
    // sort-vertices-by-angle scheme that only worked for fan-shaped
    // cavities. On axis-aligned quads (every box face, extrude/revolve
    // caps) this produced a triangulation whose triangles fell outside
    // the polygon, the retain filter then dropped them all, and the
    // face emitted zero triangles.
    let triangles = triangulate_planar_polygon(&all_vertices, &loop_boundaries, &normal);

    // Add vertices to mesh and build index mapping
    let mut vertex_map = Vec::new();
    for vertex in &all_vertices {
        let index = mesh.add_vertex(MeshVertex {
            position: *vertex,
            normal,
            uv: None,
        });
        vertex_map.push(index);
    }

    // Add triangles to mesh.
    //
    // No additional orientation flip is needed here: the triangulator
    // was passed the already-flipped face normal (`face.normal_at`
    // applies `orientation.sign()`), and `compute_plane_axes` builds
    // a right-handed basis where `u_axis × v_axis = normal`. The
    // triangulator forces 2D CCW in that basis, so every emitted
    // triangle's geometric normal `(b - a) × (c - a)` aligns with
    // the stored vertex normal. A previous `if Forward { (a,b,c) }
    // else { (a,c,b) }` branch was a double-flip that wound 8/12
    // box-face triangles backwards relative to their stored normals
    // — the bug `box_tessellation_winding_agrees_with_vertex_normals`
    // catches.
    for triangle in triangles {
        mesh.add_triangle(
            vertex_map[triangle[0]],
            vertex_map[triangle[1]],
            vertex_map[triangle[2]],
        );
    }
}

/// Sample a loop's edges into a dense 3D polygon for the planar
/// tessellator.
///
/// # Why dense sampling is required
/// `Loop::vertices(...)` returns one B-Rep vertex per edge (start or
/// end depending on orientation). For a planar face with a single
/// closed-edge loop — e.g. a cylinder cap whose only edge is a full
/// circle whose `start_vertex == end_vertex` — that yields a
/// **single** vertex, not enough to triangulate. The previous code
/// then hit `all_vertices.len() < 3` and returned, emitting zero
/// triangles for every cap. Cylinders therefore looked hollow.
///
/// # Strategy
/// For each edge:
/// * If the curve is a straight line (cross product of mid-vs-endpoint
///   vectors below tolerance) emit a single sample at `t_start`. This
///   matches the previous one-vertex-per-edge behaviour for box faces
///   and keeps the resulting ear-clipping cheap.
/// * If the edge is closed (start == end vertex) it is necessarily
///   curved (a circle, ellipse, or NURBS loop). Sample 32 points so the
///   resulting polygon approximates the curve well.
/// * Otherwise (curved arc with distinct endpoints) sample 16 points.
///
/// Sampling uses the loop's recorded edge orientation so the polygon
/// winds consistently — `triangulate_planar_polygon` then forces outer
/// CCW / inner CW via the shoelace test, so absolute winding here is
/// not load-bearing, but per-edge orientation must be respected to
/// keep the polygon simple.
fn sample_loop_3d_polygon(
    loop_data: &crate::primitives::r#loop::Loop,
    model: &BRepModel,
    out: &mut Vec<Point3>,
) {
    const SAMPLES_CLOSED_EDGE: usize = 32;
    const SAMPLES_CURVED_EDGE: usize = 16;
    const COLLINEAR_TOL: f64 = 1e-9;

    for (i, &edge_id) in loop_data.edges.iter().enumerate() {
        let forward = loop_data.orientations.get(i).copied().unwrap_or(true);
        let edge = match model.edges.get(edge_id) {
            Some(e) => e,
            None => continue,
        };
        let curve = match model.curves.get(edge.curve_id) {
            Some(c) => c,
            None => continue,
        };

        let (t_start, t_end) = if forward {
            (edge.param_range.start, edge.param_range.end)
        } else {
            (edge.param_range.end, edge.param_range.start)
        };

        // Decide sampling density. Closed edges are always curved; for
        // open edges, a 3-point collinearity check decides.
        let is_closed_edge = edge.start_vertex == edge.end_vertex;
        let n = if is_closed_edge {
            SAMPLES_CLOSED_EDGE
        } else {
            let mid = (t_start + t_end) * 0.5;
            match (
                curve.point_at(t_start),
                curve.point_at(mid),
                curve.point_at(t_end),
            ) {
                (Ok(p_start), Ok(p_mid), Ok(p_end)) => {
                    let v1 = p_mid - p_start;
                    let v2 = p_end - p_start;
                    if v1.cross(&v2).magnitude() < COLLINEAR_TOL {
                        1
                    } else {
                        SAMPLES_CURVED_EDGE
                    }
                }
                _ => 1,
            }
        };

        for j in 0..n {
            let t = t_start + (j as f64) * (t_end - t_start) / (n as f64);
            if let Ok(p) = curve.point_at(t) {
                out.push(p);
            }
        }
    }
}

/// Triangulate a planar face's outer + (optional) inner loops in the
/// face's tangent plane.
///
/// Algorithm: bridged ear-clipping (Hertel 1985, also used by
/// mapbox/earcut). Bullet-proof for any simple polygon, with or without
/// holes, runs in O((n+h)²) where n = total vertex count, h = hole count.
///
/// Steps:
///
///   1. Project all vertices to 2D using `compute_plane_axes(normal)`.
///   2. Force the outer loop CCW and every hole CW (shoelace signed-
///      area test). This convention matches `ear_clip_2d`'s positive-
///      cross-product ear test.
///   3. Sort holes by max-x descending and bridge each into the running
///      outer polygon by:
///        a. Choosing M = the hole's rightmost vertex.
///        b. Casting a ray from M in +x; finding the closest outer edge
///           it pierces.
///        c. Picking the edge endpoint with larger x as the bridge
///           target P, then refining: if any reflex outer vertex lies
///           strictly inside triangle (M, ray-hit, P), the closest such
///           vertex (smallest |angle from +x|, ties broken by squared
///           distance) becomes P instead. This guarantees segment MP
///           does not cross the polygon boundary (Eberly 2008,
///           "Triangulation by Ear Clipping").
///        d. Splicing the hole's CW walk into the outer at position P
///           using two synthetic duplicate vertices for M and P; the
///           result is a simple polygon with a thin notch.
///   4. Ear-clip the bridged polygon.
///   5. Remap synthetic duplicates back to their original 3D indices so
///      the output mesh has no orphan vertices.
///
/// This unifies the previously-separate hole-free and holed paths on a
/// single algorithm. The previous implementation routed holed faces
/// through a Bowyer-Watson + constraint-enforcement chain whose
/// `enforce_edge_constraint` step silently corrupted the triangulation
/// (cavity boundary edges were collected but never used; a naïve
/// sort-by-angle fallback only worked for fan-shaped cavities). On
/// axis-aligned quads it produced triangles outside the polygon,
/// the retain filter dropped them all, and the face emitted zero
/// triangles. Bridged ear-clipping has no super-triangle, no cavity
/// retriangulation, and no retain filter — there is nowhere for
/// triangles to silently disappear.
fn triangulate_planar_polygon(
    vertices: &[Point3],
    loop_boundaries: &[(usize, usize, bool)],
    normal: &Vector3,
) -> Vec<[usize; 3]> {
    let outer_range = match loop_boundaries.iter().find(|(_, _, is_outer)| *is_outer) {
        Some(&(s, e, _)) if e - s >= 3 => (s, e),
        _ => return Vec::new(),
    };
    let inner_ranges: Vec<(usize, usize)> = loop_boundaries
        .iter()
        .filter(|(_, _, is_outer)| !*is_outer)
        .filter_map(|&(s, e, _)| if e - s >= 3 { Some((s, e)) } else { None })
        .collect();

    // Project to 2D in the face's tangent plane.
    let (u_axis, v_axis) = compute_plane_axes(normal);
    let origin = vertices[outer_range.0];
    let mut vertices_2d: Vec<(f64, f64)> = vertices
        .iter()
        .map(|p| {
            let r = *p - origin;
            (r.dot(&u_axis), r.dot(&v_axis))
        })
        .collect();
    // Track the original 3D index for every (possibly synthetic) 2D
    // vertex. Synthetic duplicates introduced by hole-bridging carry the
    // 3D index of the vertex they shadow.
    let mut index_remap: Vec<usize> = (0..vertices_2d.len()).collect();

    // Outer (force CCW).
    let mut outer: Vec<usize> = (outer_range.0..outer_range.1).collect();
    if polygon_signed_area_2d(&vertices_2d, &outer) < 0.0 {
        outer.reverse();
    }

    if inner_ranges.is_empty() {
        let mut tris = Vec::new();
        ear_clip_2d(&vertices_2d, &outer, &mut tris);
        return tris;
    }

    // Holes (force each CW), sorted by max-x descending so we bridge
    // the rightmost hole first.
    let mut holes: Vec<Vec<usize>> = inner_ranges
        .iter()
        .map(|&(s, e)| {
            let mut h: Vec<usize> = (s..e).collect();
            if polygon_signed_area_2d(&vertices_2d, &h) > 0.0 {
                h.reverse();
            }
            h
        })
        .collect();
    holes.sort_by(|a, b| {
        let amx = polygon_max_x(a, &vertices_2d);
        let bmx = polygon_max_x(b, &vertices_2d);
        bmx.partial_cmp(&amx).unwrap_or(std::cmp::Ordering::Equal)
    });

    for hole in holes {
        if !bridge_hole_into_outer(&mut outer, &hole, &mut vertices_2d, &mut index_remap) {
            tracing::warn!(
                "triangulate_planar_polygon: failed to bridge hole into outer; \
                 face will be tessellated without this hole"
            );
        }
    }

    let mut bridged_tris: Vec<[usize; 3]> = Vec::new();
    ear_clip_2d(&vertices_2d, &outer, &mut bridged_tris);

    // Collapse synthetic-duplicate indices back to their original 3D
    // indices. After this remap, two indices in the same triangle may
    // refer to the same 3D vertex only on the bridge degenerate
    // (zero-area) triangles; those are filtered out below.
    bridged_tris
        .into_iter()
        .filter_map(|[a, b, c]| {
            let ra = index_remap[a];
            let rb = index_remap[b];
            let rc = index_remap[c];
            if ra == rb || rb == rc || ra == rc {
                None
            } else {
                Some([ra, rb, rc])
            }
        })
        .collect()
}

/// Maximum x-coordinate among the indexed 2D points.
fn polygon_max_x(polygon: &[usize], vertices_2d: &[(f64, f64)]) -> f64 {
    polygon
        .iter()
        .map(|&i| vertices_2d[i].0)
        .fold(f64::NEG_INFINITY, f64::max)
}

/// Bridge a single hole into `outer`. Returns false if no visible bridge
/// target could be found (degenerate input — caller emits a warning and
/// skips this hole).
///
/// Mutates `vertices_2d` and `index_remap` to add two synthetic duplicate
/// vertices (one for the outer-bridge target, one for the hole's
/// rightmost vertex). Synthetic duplicates carry the same 2D coords and
/// the same `index_remap[i]` as their originals — `ear_clip_2d` treats
/// them as independent vertices for its index-equality "same vertex"
/// check, but the final `index_remap` collapse undoes the duplication
/// in the emitted triangles.
fn bridge_hole_into_outer(
    outer: &mut Vec<usize>,
    hole: &[usize],
    vertices_2d: &mut Vec<(f64, f64)>,
    index_remap: &mut Vec<usize>,
) -> bool {
    if hole.len() < 3 {
        return false;
    }

    // 1. M = rightmost hole vertex (break x ties by larger y).
    let m_in_hole = (0..hole.len())
        .max_by(|&a, &b| {
            let pa = vertices_2d[hole[a]];
            let pb = vertices_2d[hole[b]];
            pa.0.partial_cmp(&pb.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| pa.1.partial_cmp(&pb.1).unwrap_or(std::cmp::Ordering::Equal))
        })
        .unwrap_or(0);
    let m_idx = hole[m_in_hole];
    let m = vertices_2d[m_idx];

    // 2. Ray from M in +x. Find outer edge with min x-intersection > M.x.
    let n_outer = outer.len();
    let mut best_hit: Option<(f64, usize)> = None;
    for i in 0..n_outer {
        let a = vertices_2d[outer[i]];
        let b = vertices_2d[outer[(i + 1) % n_outer]];
        // Half-open span check avoids double-counting at shared y.
        let spans = (a.1 <= m.1 && m.1 < b.1) || (b.1 <= m.1 && m.1 < a.1);
        if !spans {
            continue;
        }
        let dy = b.1 - a.1;
        if dy.abs() < 1e-14 {
            continue;
        }
        let t = (m.1 - a.1) / dy;
        let x = a.0 + t * (b.0 - a.0);
        if x > m.0 - 1e-14 && best_hit.map_or(true, |(bx, _)| x < bx) {
            best_hit = Some((x, i));
        }
    }
    let (hit_x, hit_edge) = match best_hit {
        Some(h) => h,
        None => return false,
    };

    // 3. Initial P = endpoint of (outer[hit_edge], outer[hit_edge+1]) with larger x.
    let a_pos = hit_edge;
    let b_pos = (hit_edge + 1) % n_outer;
    let a_pt = vertices_2d[outer[a_pos]];
    let b_pt = vertices_2d[outer[b_pos]];
    let mut p_pos = if a_pt.0 >= b_pt.0 { a_pos } else { b_pos };
    let mut p_pt = vertices_2d[outer[p_pos]];

    // 4. Refine: if any reflex outer vertex lies strictly inside triangle
    //    (M, hit_point, P), the bridge MP crosses the boundary. Pick the
    //    closest such reflex vertex (smallest |angle from M's +x axis|,
    //    ties broken by squared distance) as P instead.
    let hit_pt = (hit_x, m.1);
    let mut best_angle = (p_pt.1 - m.1).atan2(p_pt.0 - m.0).abs();
    let mut best_dist2 = (p_pt.0 - m.0).powi(2) + (p_pt.1 - m.1).powi(2);
    for i in 0..n_outer {
        if i == p_pos {
            continue;
        }
        let v = vertices_2d[outer[i]];
        let prev = vertices_2d[outer[(i + n_outer - 1) % n_outer]];
        let next = vertices_2d[outer[(i + 1) % n_outer]];
        // CCW ⇒ reflex iff (curr - prev) × (next - prev) ≤ 0.
        let cross = (v.0 - prev.0) * (next.1 - prev.1) - (v.1 - prev.1) * (next.0 - prev.0);
        if cross > 0.0 {
            continue;
        }
        if !point_in_triangle_2d(&v, &m, &hit_pt, &p_pt) {
            continue;
        }
        let angle = (v.1 - m.1).atan2(v.0 - m.0).abs();
        let dist2 = (v.0 - m.0).powi(2) + (v.1 - m.1).powi(2);
        if angle < best_angle - 1e-14
            || ((angle - best_angle).abs() <= 1e-14 && dist2 < best_dist2)
        {
            p_pos = i;
            p_pt = v;
            best_angle = angle;
            best_dist2 = dist2;
        }
    }

    // 5. Splice. Insert into outer immediately after position p_pos:
    //    [M_dup, hole walked CW from m_in_hole+1 wrapping to m_in_hole-1,
    //     M (original), P_dup]
    //
    // Synthetic duplicates carry the original 3D index via `index_remap`
    // so the final triangle remap collapses them back.
    let m_dup = vertices_2d.len();
    vertices_2d.push(m);
    index_remap.push(index_remap[m_idx]);
    let p_orig_idx = outer[p_pos];
    let p_dup = vertices_2d.len();
    vertices_2d.push(p_pt);
    index_remap.push(index_remap[p_orig_idx]);

    let mut spliced = Vec::with_capacity(outer.len() + hole.len() + 2);
    spliced.extend_from_slice(&outer[..=p_pos]);
    spliced.push(m_dup);
    let h_len = hole.len();
    for k in 1..h_len {
        spliced.push(hole[(m_in_hole + k) % h_len]);
    }
    spliced.push(m_idx);
    spliced.push(p_dup);
    spliced.extend_from_slice(&outer[p_pos + 1..]);
    *outer = spliced;
    true
}

/// Signed area of a polygon described by indices into `vertices_2d`.
/// Positive ⇒ CCW, negative ⇒ CW. Uses the shoelace formula.
fn polygon_signed_area_2d(vertices_2d: &[(f64, f64)], polygon: &[usize]) -> f64 {
    let n = polygon.len();
    if n < 3 {
        return 0.0;
    }
    let mut area = 0.0;
    for i in 0..n {
        let (x1, y1) = vertices_2d[polygon[i]];
        let (x2, y2) = vertices_2d[polygon[(i + 1) % n]];
        area += x1 * y2 - x2 * y1;
    }
    area * 0.5
}

/// Triangulate a simple polygon in 2D using ear clipping.
/// Appends resulting triangles to `output`.
fn ear_clip_2d(vertices: &[(f64, f64)], polygon: &[usize], output: &mut Vec<[usize; 3]>) {
    if polygon.len() < 3 {
        return;
    }
    if polygon.len() == 3 {
        output.push([polygon[0], polygon[1], polygon[2]]);
        return;
    }

    let mut remaining: Vec<usize> = polygon.to_vec();

    let mut max_iterations = remaining.len() * remaining.len();
    let mut i = 0;

    while remaining.len() > 3 && max_iterations > 0 {
        max_iterations -= 1;
        let n = remaining.len();
        let prev = remaining[(i + n - 1) % n];
        let curr = remaining[i % n];
        let next = remaining[(i + 1) % n];

        let p0 = vertices[prev];
        let p1 = vertices[curr];
        let p2 = vertices[next];

        // Check if this is a convex (ear) vertex
        let cross = (p1.0 - p0.0) * (p2.1 - p0.1) - (p1.1 - p0.1) * (p2.0 - p0.0);
        if cross <= 1e-14 {
            // Not convex, skip
            i = (i + 1) % remaining.len();
            continue;
        }

        // Check that no other vertex lies inside this ear triangle
        let mut is_ear = true;
        for &vi in &remaining {
            if vi == prev || vi == curr || vi == next {
                continue;
            }
            if point_in_triangle_2d(&vertices[vi], &p0, &p1, &p2) {
                is_ear = false;
                break;
            }
        }

        if is_ear {
            output.push([prev, curr, next]);
            remaining.remove(i % remaining.len());
            if i >= remaining.len() && !remaining.is_empty() {
                i = 0;
            }
        } else {
            i = (i + 1) % remaining.len();
        }
    }

    // Emit final triangle
    if remaining.len() == 3 {
        output.push([remaining[0], remaining[1], remaining[2]]);
    }
}

/// Check if point p is inside triangle (a, b, c) using barycentric coordinates
fn point_in_triangle_2d(p: &(f64, f64), a: &(f64, f64), b: &(f64, f64), c: &(f64, f64)) -> bool {
    let v0 = (c.0 - a.0, c.1 - a.1);
    let v1 = (b.0 - a.0, b.1 - a.1);
    let v2 = (p.0 - a.0, p.1 - a.1);

    let dot00 = v0.0 * v0.0 + v0.1 * v0.1;
    let dot01 = v0.0 * v1.0 + v0.1 * v1.1;
    let dot02 = v0.0 * v2.0 + v0.1 * v2.1;
    let dot11 = v1.0 * v1.0 + v1.1 * v1.1;
    let dot12 = v1.0 * v2.0 + v1.1 * v2.1;

    let inv_denom = 1.0 / (dot00 * dot11 - dot01 * dot01);
    let u = (dot11 * dot02 - dot01 * dot12) * inv_denom;
    let v = (dot00 * dot12 - dot01 * dot02) * inv_denom;

    // Point is inside if u >= 0, v >= 0, u + v <= 1 (with tolerance)
    u > 1e-10 && v > 1e-10 && (u + v) < 1.0 - 1e-10
}

/// Tessellate a cylindrical face
fn tessellate_cylindrical_face(
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) {
    // Tessellation is void-return; if the face's surface has gone missing
    // from the model (invariant violation), we skip silently rather than
    // panicking the entire tessellation pass.
    let Some(surface) = model.surfaces.get(face.surface_id) else {
        return;
    };

    // Get parameter bounds from face loops
    let (u_min, u_max, v_min, v_max) = get_face_parameter_bounds(face, model);

    // Extract actual cylinder radius from surface
    let radius = surface
        .as_any()
        .downcast_ref::<crate::primitives::surface::Cylinder>()
        .map(|c| c.radius)
        .unwrap_or(1.0);
    let u_span = u_max - u_min;
    let v_span = v_max - v_min;

    // Angular subdivision capped by max_segments — mirrors the spherical
    // path. The previous formula `(radius * u_span) / max_edge_length`
    // never consulted max_segments and produced ~1M steps per face for
    // r=15, h=80, max_edge_length=0.1; the floor+cap below keeps it
    // bounded while still respecting both chord-length and angle
    // tolerances.
    let u_from_chord = if params.max_edge_length > 0.0 && radius > 0.0 {
        let half_chord = (params.max_edge_length / (2.0 * radius)).min(1.0);
        let theta = 2.0 * half_chord.asin();
        if theta > 0.0 {
            (u_span / theta).ceil() as usize
        } else {
            params.min_segments
        }
    } else {
        params.min_segments
    };
    let u_from_angle = if params.max_angle_deviation > 0.0 {
        (u_span / params.max_angle_deviation).ceil() as usize
    } else {
        params.min_segments
    };
    let u_steps = u_from_chord
        .max(u_from_angle)
        .max(params.min_segments)
        .min(params.max_segments);

    let v_steps_raw = if params.max_edge_length > 0.0 {
        ((v_span / params.max_edge_length).ceil() as usize).max(1)
    } else {
        1
    };
    let v_steps = v_steps_raw.min(params.max_segments);

    // Generate vertices
    let mut vertex_grid = Vec::new();
    for v_idx in 0..=v_steps {
        let v = v_min + (v_idx as f64) * (v_max - v_min) / (v_steps as f64);
        let mut row = Vec::new();

        for u_idx in 0..=u_steps {
            let u = u_min + (u_idx as f64) * (u_max - u_min) / (u_steps as f64);

            if let (Ok(point), Ok(normal)) = (
                surface.point_at(u, v),
                face.normal_at(u, v, &model.surfaces),
            ) {
                let index = mesh.add_vertex(MeshVertex {
                    position: point,
                    normal,
                    uv: Some((u, v)),
                });
                row.push(index);
            }
        }
        vertex_grid.push(row);
    }

    // Generate triangles. Winding follows `face.orientation` so the
    // emitted geometric normal (CCW cross product) agrees with the
    // stored vertex normal (which `Face::normal_at` already flips for
    // backward faces); without this, downstream back-face culling
    // would invert reversed faces.
    let forward = face.orientation.is_forward();
    for v_idx in 0..v_steps {
        for u_idx in 0..u_steps {
            if vertex_grid[v_idx].len() > u_idx + 1 && vertex_grid[v_idx + 1].len() > u_idx + 1 {
                let v0 = vertex_grid[v_idx][u_idx];
                let v1 = vertex_grid[v_idx][u_idx + 1];
                let v2 = vertex_grid[v_idx + 1][u_idx];
                let v3 = vertex_grid[v_idx + 1][u_idx + 1];

                if forward {
                    mesh.add_triangle(v0, v1, v2);
                    mesh.add_triangle(v1, v3, v2);
                } else {
                    mesh.add_triangle(v0, v2, v1);
                    mesh.add_triangle(v1, v2, v3);
                }
            }
        }
    }
}

/// Tessellate a spherical face with adaptive refinement
fn tessellate_spherical_face(
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) {
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return,
    };

    // Get parameter bounds from face loops
    let (u_min, u_max, v_min, v_max) = get_face_parameter_bounds(face, model);

    // Sphere-specific: detect if we're near poles
    let near_north_pole = v_max > std::f64::consts::PI * 0.9;
    let near_south_pole = v_min < std::f64::consts::PI * 0.1;

    // Adaptive tessellation based on angular span
    let u_span = u_max - u_min;
    let v_span = v_max - v_min;

    // Calculate resolution using the same triple-guard pattern as the
    // cylinder and cone tessellators: chord tolerance, angular tolerance,
    // and an explicit `max_segments` cap. Without the cap, an aggressive
    // `max_edge_length` (e.g. 0.1 mm on a 30 mm sphere) and a tiny
    // `max_angle_deviation` would request millions of triangles per face.
    let radius = estimate_sphere_radius(surface).max(crate::math::constants::EPSILON);

    let u_from_chord = if params.max_edge_length > 0.0 {
        let half_chord = (params.max_edge_length / (2.0 * radius)).min(1.0);
        let theta = 2.0 * half_chord.asin();
        if theta > 0.0 {
            (u_span / theta).ceil() as usize
        } else {
            params.min_segments
        }
    } else {
        params.min_segments
    };
    let u_from_angle = if params.max_angle_deviation > 0.0 {
        (u_span / params.max_angle_deviation).ceil() as usize
    } else {
        params.min_segments
    };
    let u_steps = u_from_chord
        .max(u_from_angle)
        .max(params.min_segments.max(3))
        .min(params.max_segments);

    let v_from_chord = if params.max_edge_length > 0.0 {
        let half_chord = (params.max_edge_length / (2.0 * radius)).min(1.0);
        let theta = 2.0 * half_chord.asin();
        if theta > 0.0 {
            (v_span / theta).ceil() as usize
        } else {
            params.min_segments
        }
    } else {
        params.min_segments
    };
    let v_from_angle = if params.max_angle_deviation > 0.0 {
        (v_span / params.max_angle_deviation).ceil() as usize
    } else {
        params.min_segments
    };
    let v_steps = v_from_chord
        .max(v_from_angle)
        .max(params.min_segments.max(3))
        .min(params.max_segments);

    // Special handling for poles to avoid degeneracies
    if near_north_pole || near_south_pole {
        tessellate_spherical_with_poles(
            face,
            model,
            surface,
            u_min,
            u_max,
            v_min,
            v_max,
            u_steps,
            v_steps,
            near_north_pole,
            near_south_pole,
            mesh,
        );
    } else {
        // Regular grid tessellation for non-polar regions
        tessellate_spherical_regular(
            face, model, surface, u_min, u_max, v_min, v_max, u_steps, v_steps, mesh,
        );
    }
}

/// Tessellate spherical surface with pole handling
#[allow(clippy::expect_used)] // pole vertex presence verified by is_some() guard above expect
fn tessellate_spherical_with_poles(
    face: &Face,
    model: &BRepModel,
    surface: &dyn Surface,
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
    u_steps: usize,
    v_steps: usize,
    near_north_pole: bool,
    near_south_pole: bool,
    mesh: &mut TriangleMesh,
) {
    let mut vertex_grid = Vec::new();

    // Generate vertices with special pole handling
    for v_idx in 0..=v_steps {
        let v = v_min + (v_idx as f64) * (v_max - v_min) / (v_steps as f64);
        let mut row = Vec::new();

        // Check if we're at a pole
        let at_pole = (near_north_pole && v_idx == v_steps) || (near_south_pole && v_idx == 0);

        if at_pole {
            // Single vertex at pole
            let u = (u_min + u_max) / 2.0; // Any u value works at pole
            if let (Ok(point), Ok(normal)) = (
                surface.point_at(u, v),
                face.normal_at(u, v, &model.surfaces),
            ) {
                if is_point_inside_face(u, v, face, model) {
                    let index = mesh.add_vertex(MeshVertex {
                        position: point,
                        normal,
                        uv: Some((u, v)),
                    });
                    row.push(Some(index));
                }
            }
        } else {
            // Regular row of vertices
            for u_idx in 0..=u_steps {
                let u = u_min + (u_idx as f64) * (u_max - u_min) / (u_steps as f64);

                let inside = is_point_inside_face(u, v, face, model);
                if inside {
                    if let (Ok(point), Ok(normal)) = (
                        surface.point_at(u, v),
                        face.normal_at(u, v, &model.surfaces),
                    ) {
                        let index = mesh.add_vertex(MeshVertex {
                            position: point,
                            normal,
                            uv: Some((u, v)),
                        });
                        row.push(Some(index));
                    } else {
                        row.push(None);
                    }
                } else {
                    row.push(None);
                }
            }
        }
        vertex_grid.push(row);
    }

    // Generate triangles with special handling for poles. Winding
    // follows `face.orientation` for the same reason as the
    // cylindrical path — a backward face must emit reversed CCW so
    // the geometric normal agrees with the stored vertex normal.
    let forward = face.orientation.is_forward();
    for v_idx in 0..v_steps {
        let at_south_pole = near_south_pole && v_idx == 0;
        let at_north_pole = near_north_pole && v_idx == v_steps - 1;

        if at_south_pole && vertex_grid[0].len() == 1 && vertex_grid[0][0].is_some() {
            // Triangles from south pole
            let pole_vertex = vertex_grid[0][0]
                .expect("south pole vertex presence verified by is_some() guard above");
            for u_idx in 0..u_steps {
                if let (Some(v1), Some(v2)) = (
                    vertex_grid[1].get(u_idx).and_then(|&v| v),
                    vertex_grid[1].get(u_idx + 1).and_then(|&v| v),
                ) {
                    if forward {
                        mesh.add_triangle(pole_vertex, v1, v2);
                    } else {
                        mesh.add_triangle(pole_vertex, v2, v1);
                    }
                }
            }
        } else if at_north_pole
            && vertex_grid[v_steps].len() == 1
            && vertex_grid[v_steps][0].is_some()
        {
            // Triangles to north pole
            let pole_vertex = vertex_grid[v_steps][0]
                .expect("north pole vertex presence verified by is_some() guard above");
            for u_idx in 0..u_steps {
                if let (Some(v1), Some(v2)) = (
                    vertex_grid[v_idx].get(u_idx).and_then(|&v| v),
                    vertex_grid[v_idx].get(u_idx + 1).and_then(|&v| v),
                ) {
                    if forward {
                        mesh.add_triangle(v1, v2, pole_vertex);
                    } else {
                        mesh.add_triangle(v2, v1, pole_vertex);
                    }
                }
            }
        } else {
            // Regular quad tessellation
            for u_idx in 0..u_steps {
                let v0 = vertex_grid[v_idx].get(u_idx).and_then(|&v| v);
                let v1 = vertex_grid[v_idx].get(u_idx + 1).and_then(|&v| v);
                let v2 = vertex_grid[v_idx + 1].get(u_idx).and_then(|&v| v);
                let v3 = vertex_grid[v_idx + 1].get(u_idx + 1).and_then(|&v| v);

                match (v0, v1, v2, v3) {
                    (Some(a), Some(b), Some(c), Some(d)) => {
                        if forward {
                            mesh.add_triangle(a, b, c);
                            mesh.add_triangle(b, d, c);
                        } else {
                            mesh.add_triangle(a, c, b);
                            mesh.add_triangle(b, c, d);
                        }
                    }
                    // Handle degenerate cases
                    (Some(a), Some(b), Some(c), None) => {
                        if forward {
                            mesh.add_triangle(a, b, c);
                        } else {
                            mesh.add_triangle(a, c, b);
                        }
                    }
                    (Some(a), Some(b), None, Some(d)) => {
                        if forward {
                            mesh.add_triangle(a, b, d);
                        } else {
                            mesh.add_triangle(a, d, b);
                        }
                    }
                    (Some(a), None, Some(c), Some(d)) => {
                        if forward {
                            mesh.add_triangle(a, d, c);
                        } else {
                            mesh.add_triangle(a, c, d);
                        }
                    }
                    (None, Some(b), Some(c), Some(d)) => {
                        if forward {
                            mesh.add_triangle(b, d, c);
                        } else {
                            mesh.add_triangle(b, c, d);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Regular spherical tessellation for non-polar regions
fn tessellate_spherical_regular(
    face: &Face,
    model: &BRepModel,
    surface: &dyn Surface,
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
    _u_steps: usize,
    _v_steps: usize,
    mesh: &mut TriangleMesh,
) {
    // Use adaptive tessellation for better quality
    let tessellator = AdaptiveTessellator::new(TessellationParams::default());
    let temp_mesh = tessellator.tessellate_patch(surface, (u_min, u_max), (v_min, v_max));

    // Convert to ThreeJS mesh with face normal
    let _normal = face
        .normal_at(
            (u_min + u_max) / 2.0,
            (v_min + v_max) / 2.0,
            &model.surfaces,
        )
        .unwrap_or(Vector3::Z);

    let mut vertex_map = Vec::new();
    for vertex in &temp_mesh.vertices {
        // Check if vertex is inside face boundaries
        if let Some((u, v)) = vertex.uv {
            if is_point_inside_face(u, v, face, model) {
                let index = mesh.add_vertex(MeshVertex {
                    position: vertex.position,
                    normal: vertex.normal,
                    uv: Some((u, v)),
                });
                vertex_map.push(Some(index));
            } else {
                vertex_map.push(None);
            }
        } else {
            vertex_map.push(None);
        }
    }

    // Add triangles with mapping. Winding follows `face.orientation`
    // so the geometric normal agrees with the stored vertex normal
    // (`Face::normal_at` already flips for backward faces).
    let forward = face.orientation.is_forward();
    for triangle in &temp_mesh.triangles {
        if let (Some(v0), Some(v1), Some(v2)) = (
            vertex_map.get(triangle[0] as usize).and_then(|&v| v),
            vertex_map.get(triangle[1] as usize).and_then(|&v| v),
            vertex_map.get(triangle[2] as usize).and_then(|&v| v),
        ) {
            if forward {
                mesh.add_triangle(v0, v1, v2);
            } else {
                mesh.add_triangle(v0, v2, v1);
            }
        }
    }
}

/// Estimate sphere radius from surface
fn estimate_sphere_radius(surface: &dyn Surface) -> f64 {
    // Sample center point and estimate radius
    let (u_range, v_range) = surface.parameter_bounds();
    let u_mid = (u_range.0 + u_range.1) / 2.0;
    let v_mid = (v_range.0 + v_range.1) / 2.0;

    if let Ok(center_point) = surface.point_at(u_mid, v_mid) {
        // Sample another point to estimate radius
        if let Ok(edge_point) = surface.point_at(u_mid + 0.1, v_mid) {
            center_point.distance(&edge_point) / 0.1 // Approximate radius
        } else {
            1.0 // Default radius
        }
    } else {
        1.0
    }
}

/// Tessellate a conical face with special handling for apex
fn tessellate_conical_face(
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) {
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return,
    };

    // Get parameter bounds from face loops
    let (u_min, u_max, v_min, v_max) = get_face_parameter_bounds(face, model);

    // Detect if we include the apex (v = 0 for typical cone parameterization)
    let includes_apex = v_min.abs() < 1e-6;

    // Calculate tessellation resolution. Both axes use a triple guard
    // (chord-tolerance, angle-tolerance, hard `max_segments` cap) so a
    // user-supplied `max_angle_deviation` near zero or `max_edge_length`
    // near zero cannot blow the step count to millions — the same
    // failure mode the cylinder path used to suffer.
    let u_span = u_max - u_min;

    // Maximum cross-section radius (at v_max) for the chord metric. For
    // a Cone, r(v) = v · sin(half_angle). Falls back to a unit radius if
    // the surface is not a Cone (generic-grid path), which keeps the
    // angular metric as the safe lower bound on step count.
    let base_radius = surface
        .as_any()
        .downcast_ref::<crate::primitives::surface::Cone>()
        .map(|cone| (v_max.abs()).max(v_min.abs()) * cone.half_angle.sin())
        .unwrap_or(1.0);

    let u_from_chord = if params.max_edge_length > 0.0 && base_radius > 0.0 {
        let half_chord = (params.max_edge_length / (2.0 * base_radius)).min(1.0);
        let theta = 2.0 * half_chord.asin();
        if theta > 0.0 {
            (u_span / theta).ceil() as usize
        } else {
            params.min_segments
        }
    } else {
        params.min_segments
    };
    let u_from_angle = if params.max_angle_deviation > 0.0 {
        (u_span / params.max_angle_deviation).ceil() as usize
    } else {
        params.min_segments
    };
    let u_steps = u_from_chord
        .max(u_from_angle)
        .max(params.min_segments.max(8))
        .min(params.max_segments);

    // Linear resolution for v (along slant), capped by max_segments.
    let cone_height = estimate_cone_height(surface, v_min, v_max);
    let v_steps_raw = if params.max_edge_length > 0.0 {
        ((cone_height / params.max_edge_length).ceil() as usize).max(3)
    } else {
        3
    };
    let v_steps = v_steps_raw.min(params.max_segments);

    if includes_apex {
        tessellate_conical_with_apex(
            face, model, surface, u_min, u_max, v_min, v_max, u_steps, v_steps, mesh,
        );
    } else {
        tessellate_conical_regular(
            face, model, surface, u_min, u_max, v_min, v_max, u_steps, v_steps, mesh,
        );
    }
}

/// Tessellate cone with apex handling
fn tessellate_conical_with_apex(
    face: &Face,
    model: &BRepModel,
    surface: &dyn Surface,
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
    u_steps: usize,
    v_steps: usize,
    mesh: &mut TriangleMesh,
) {
    let mut vertex_grid = Vec::new();

    // First row is the apex
    if v_min.abs() < 1e-6 {
        let u = (u_min + u_max) / 2.0; // Any u value at apex
        let v = v_min;

        if let (Ok(point), Ok(normal)) = (
            surface.point_at(u, v),
            face.normal_at(u, v, &model.surfaces),
        ) {
            let index = mesh.add_vertex(MeshVertex {
                position: point,
                normal,
                uv: Some((u, v)),
            });
            vertex_grid.push(vec![Some(index)]); // Single vertex at apex
        }
    }

    // Generate remaining rows. The previous implementation gated each
    // (u, v) sample on `is_point_inside_face`, which fails for the
    // primitive cone topology because its outer loop projects to a
    // single line in (u, v) (the wide-end circle, all at v = height).
    // The (u, v) extent has already been clamped by
    // `get_face_parameter_bounds`, which unions degenerate axes with the
    // surface's own parameter bounds — so every grid point inside that
    // rectangle is, by construction, inside the face. Trimmed cones
    // (e.g. boolean output) carry seam edges that fix the loop
    // projection, and can re-introduce a trim test in a later pass.
    let v_start = if v_min.abs() < 1e-6 { 1 } else { 0 };
    for v_idx in v_start..=v_steps {
        let v = v_min + (v_idx as f64) * (v_max - v_min) / (v_steps as f64);
        let mut row = Vec::new();

        for u_idx in 0..=u_steps {
            let u = u_min + (u_idx as f64) * (u_max - u_min) / (u_steps as f64);

            if let (Ok(point), Ok(normal)) = (
                surface.point_at(u, v),
                face.normal_at(u, v, &model.surfaces),
            ) {
                let index = mesh.add_vertex(MeshVertex {
                    position: point,
                    normal,
                    uv: Some((u, v)),
                });
                row.push(Some(index));
            } else {
                row.push(None);
            }
        }
        vertex_grid.push(row);
    }

    // Generate triangles. Winding follows `face.orientation`
    // (see cylindrical path for rationale).
    let forward = face.orientation.is_forward();
    for v_idx in 0..vertex_grid.len() - 1 {
        if v_idx == 0 && vertex_grid[0].len() == 1 {
            // Triangles from apex
            if let Some(apex) = vertex_grid[0][0] {
                for u_idx in 0..u_steps {
                    if let (Some(v1), Some(v2)) = (
                        vertex_grid[1].get(u_idx).and_then(|&v| v),
                        vertex_grid[1].get(u_idx + 1).and_then(|&v| v),
                    ) {
                        if forward {
                            mesh.add_triangle(apex, v1, v2);
                        } else {
                            mesh.add_triangle(apex, v2, v1);
                        }
                    }
                }
            }
        } else {
            // Regular quads
            for u_idx in 0..u_steps {
                if let (Some(v0), Some(v1), Some(v2), Some(v3)) = (
                    vertex_grid[v_idx].get(u_idx).and_then(|&v| v),
                    vertex_grid[v_idx].get(u_idx + 1).and_then(|&v| v),
                    vertex_grid[v_idx + 1].get(u_idx).and_then(|&v| v),
                    vertex_grid[v_idx + 1].get(u_idx + 1).and_then(|&v| v),
                ) {
                    if forward {
                        mesh.add_triangle(v0, v1, v2);
                        mesh.add_triangle(v1, v3, v2);
                    } else {
                        mesh.add_triangle(v0, v2, v1);
                        mesh.add_triangle(v1, v2, v3);
                    }
                }
            }
        }
    }
}

/// Regular conical tessellation (truncated cone)
fn tessellate_conical_regular(
    face: &Face,
    model: &BRepModel,
    surface: &dyn Surface,
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
    u_steps: usize,
    v_steps: usize,
    mesh: &mut TriangleMesh,
) {
    // Standard grid tessellation
    tessellate_surface_grid(
        face, model, surface, u_min, u_max, v_min, v_max, u_steps, v_steps, mesh,
    );
}

/// Estimate cone height from v parameter range
fn estimate_cone_height(surface: &dyn Surface, v_min: f64, v_max: f64) -> f64 {
    if let (Ok(p1), Ok(p2)) = (surface.point_at(0.0, v_min), surface.point_at(0.0, v_max)) {
        p1.distance(&p2)
    } else {
        v_max - v_min
    }
}

/// Tessellate a toroidal face with proper handling of both parameters
fn tessellate_toroidal_face(
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) {
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return,
    };

    // Get parameter bounds from face loops
    let (u_min, u_max, v_min, v_max) = get_face_parameter_bounds(face, model);
    let u_span = u_max - u_min;
    let v_span = v_max - v_min;

    // Get torus radii: u sweeps the major (R) circle, v sweeps the minor (r) circle.
    let (major_radius, minor_radius) = estimate_torus_radii(surface);

    // Triple guard for both axes (chord, angle, max_segments). The
    // previous implementation handed the patch off to
    // `AdaptiveTessellator::tessellate_patch`, which ignored
    // `max_segments` entirely — a torus with R=30, r=10 generated ≈115k
    // triangles, blowing through the 100k regression cap. The grid path
    // below mirrors `tessellate_cylindrical_face` and always honours the
    // hard segment cap.
    let steps_for = |span: f64, radius: f64, cap: usize| -> usize {
        let from_chord = if params.max_edge_length > 0.0 && radius > 0.0 {
            let half_chord = (params.max_edge_length / (2.0 * radius)).min(1.0);
            let theta = 2.0 * half_chord.asin();
            if theta > 0.0 {
                (span / theta).ceil() as usize
            } else {
                params.min_segments
            }
        } else {
            params.min_segments
        };
        let from_angle = if params.max_angle_deviation > 0.0 {
            (span / params.max_angle_deviation).ceil() as usize
        } else {
            params.min_segments
        };
        from_chord
            .max(from_angle)
            .max(params.min_segments)
            .min(cap)
    };

    let u_steps = steps_for(u_span, major_radius, params.max_segments);
    // Cap v at half max_segments so the grand total tri count stays
    // within max_segments² rather than 2·max_segments² for a full torus.
    let v_steps = steps_for(v_span, minor_radius, params.max_segments.max(2) / 2);

    // Generate vertices on a regular (u, v) grid. As with the cylinder
    // path, the (u, v) extent has been clamped against surface bounds by
    // `get_face_parameter_bounds`, so every grid point lies inside the
    // primitive torus face. Trimmed tori carry seam edges that fix the
    // loop projection and can re-introduce a per-sample trim test later.
    let mut vertex_grid: Vec<Vec<Option<u32>>> = Vec::with_capacity(v_steps + 1);
    for v_idx in 0..=v_steps {
        let v = v_min + (v_idx as f64) * v_span / (v_steps as f64);
        let mut row = Vec::with_capacity(u_steps + 1);
        for u_idx in 0..=u_steps {
            let u = u_min + (u_idx as f64) * u_span / (u_steps as f64);
            if let (Ok(point), Ok(normal)) = (
                surface.point_at(u, v),
                face.normal_at(u, v, &model.surfaces),
            ) {
                let index = mesh.add_vertex(MeshVertex {
                    position: point,
                    normal,
                    uv: Some((u, v)),
                });
                row.push(Some(index));
            } else {
                row.push(None);
            }
        }
        vertex_grid.push(row);
    }

    // Generate triangles
    for v_idx in 0..v_steps {
        for u_idx in 0..u_steps {
            if let (Some(v0), Some(v1), Some(v2), Some(v3)) = (
                vertex_grid[v_idx].get(u_idx).and_then(|&v| v),
                vertex_grid[v_idx].get(u_idx + 1).and_then(|&v| v),
                vertex_grid[v_idx + 1].get(u_idx).and_then(|&v| v),
                vertex_grid[v_idx + 1].get(u_idx + 1).and_then(|&v| v),
            ) {
                if face.orientation == crate::primitives::face::FaceOrientation::Forward {
                    mesh.add_triangle(v0, v1, v2);
                    mesh.add_triangle(v1, v3, v2);
                } else {
                    mesh.add_triangle(v0, v2, v1);
                    mesh.add_triangle(v1, v2, v3);
                }
            }
        }
    }
}

/// Estimate torus radii from surface
fn estimate_torus_radii(surface: &dyn Surface) -> (f64, f64) {
    // Sample points to estimate major and minor radii
    let (u_range, v_range) = surface.parameter_bounds();

    // Points on major circle (v = 0 and v = π)
    if let (Ok(p1), Ok(p2)) = (
        surface.point_at(u_range.0, v_range.0),
        surface.point_at(u_range.0, (v_range.0 + v_range.1) / 2.0),
    ) {
        let minor_radius = p1.distance(&p2) / 2.0;

        // Points around major circle
        if let (Ok(p3), Ok(p4)) = (
            surface.point_at(u_range.0, v_range.0),
            surface.point_at((u_range.0 + u_range.1) / 2.0, v_range.0),
        ) {
            let major_radius = p3.distance(&p4) / std::f64::consts::PI;
            (major_radius, minor_radius)
        } else {
            (1.0, minor_radius)
        }
    } else {
        (1.0, 0.25) // Default radii
    }
}

/// Generic grid tessellation helper
fn tessellate_surface_grid(
    face: &Face,
    model: &BRepModel,
    surface: &dyn Surface,
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
    u_steps: usize,
    v_steps: usize,
    mesh: &mut TriangleMesh,
) {
    let mut vertex_grid = Vec::new();

    // Generate vertices
    for v_idx in 0..=v_steps {
        let v = v_min + (v_idx as f64) * (v_max - v_min) / (v_steps as f64);
        let mut row = Vec::new();

        for u_idx in 0..=u_steps {
            let u = u_min + (u_idx as f64) * (u_max - u_min) / (u_steps as f64);

            if is_point_inside_face(u, v, face, model) {
                if let (Ok(point), Ok(normal)) = (
                    surface.point_at(u, v),
                    face.normal_at(u, v, &model.surfaces),
                ) {
                    let index = mesh.add_vertex(MeshVertex {
                        position: point,
                        normal,
                        uv: Some((u, v)),
                    });
                    row.push(Some(index));
                } else {
                    row.push(None);
                }
            } else {
                row.push(None);
            }
        }
        vertex_grid.push(row);
    }

    // Generate triangles
    for v_idx in 0..v_steps {
        for u_idx in 0..u_steps {
            if let (Some(v0), Some(v1), Some(v2), Some(v3)) = (
                vertex_grid[v_idx].get(u_idx).and_then(|&v| v),
                vertex_grid[v_idx].get(u_idx + 1).and_then(|&v| v),
                vertex_grid[v_idx + 1].get(u_idx).and_then(|&v| v),
                vertex_grid[v_idx + 1].get(u_idx + 1).and_then(|&v| v),
            ) {
                if face.orientation == crate::primitives::face::FaceOrientation::Forward {
                    mesh.add_triangle(v0, v1, v2);
                    mesh.add_triangle(v1, v3, v2);
                } else {
                    mesh.add_triangle(v0, v2, v1);
                    mesh.add_triangle(v1, v2, v3);
                }
            }
        }
    }
}

/// Tessellate a NURBS face with world-class adaptive refinement
fn tessellate_nurbs_face(
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) {
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return,
    };

    // Get parameter bounds for the face
    let (u_min, u_max, v_min, v_max) = get_face_parameter_bounds(face, model);

    // For NURBS surfaces, we need adaptive tessellation based on curvature
    // Use a more sophisticated approach than generic grid
    tessellate_nurbs_adaptive(
        surface, face, model, params, mesh, u_min, u_max, v_min, v_max,
    );
}

/// Generic surface tessellation using uniform grid.
///
/// Builds the face's boundary polygon in (u, v) parameter space once,
/// then point-tests every grid cell against that cached polygon. The
/// previous implementation called `is_point_inside_face` per cell, which
/// reprojected every loop sample through `surface.closest_point` (an
/// iterative Newton solve) for every test — O(grid² · samples · Newton)
/// per face. Caching collapses it to O(grid² + samples · Newton).
fn tessellate_generic_face(
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) {
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return,
    };

    // Get parameter bounds
    let (_u_range, _v_range) = surface.parameter_bounds();
    let (u_min, u_max, v_min, v_max) = get_face_parameter_bounds(face, model);

    // Triple-guard subdivision: parameter-derived chord, max_segments cap,
    // floor for visual smoothness. The previous formula multiplied the
    // parameter-space ratio by 10, saturating at the clamp ceiling for any
    // realistic max_edge_length and torching tessellation throughput.
    let raw_u = ((u_max - u_min) / params.max_edge_length).ceil() as usize + 1;
    let raw_v = ((v_max - v_min) / params.max_edge_length).ceil() as usize + 1;
    let u_steps = raw_u.max(3).min(params.max_segments.max(3));
    let v_steps = raw_v.max(3).min(params.max_segments.max(3));

    // Precompute the outer-loop polygon once and the inner-loop polygons
    // (holes) once. All subsequent point-in-face tests reuse these.
    let outer_polygon = match model.loops.get(face.outer_loop) {
        Some(loop_data) => get_loop_polygon_2d(loop_data, model, surface),
        None => Vec::new(),
    };
    let inner_polygons: Vec<Vec<(f64, f64)>> = face
        .inner_loops
        .iter()
        .filter_map(|&inner_id| {
            model
                .loops
                .get(inner_id)
                .map(|loop_data| get_loop_polygon_2d(loop_data, model, surface))
        })
        .collect();

    let inside_face = |u: f64, v: f64| -> bool {
        if outer_polygon.len() < 3 {
            // Surface has no usable boundary; accept the whole grid so we
            // still emit triangles rather than silently dropping the face.
            return true;
        }
        if calculate_winding_number(&(u, v), &outer_polygon).abs() < 0.5 {
            return false;
        }
        for hole in &inner_polygons {
            if hole.len() >= 3
                && calculate_winding_number(&(u, v), hole).abs() > 0.5
            {
                return false;
            }
        }
        true
    };

    // Generate vertices
    let mut vertex_grid = Vec::new();
    for v_idx in 0..=v_steps {
        let v = v_min + (v_idx as f64) * (v_max - v_min) / (v_steps as f64);
        let mut row = Vec::new();

        for u_idx in 0..=u_steps {
            let u = u_min + (u_idx as f64) * (u_max - u_min) / (u_steps as f64);

            if inside_face(u, v) {
                if let (Ok(point), Ok(normal)) = (
                    surface.point_at(u, v),
                    face.normal_at(u, v, &model.surfaces),
                ) {
                    let index = mesh.add_vertex(MeshVertex {
                        position: point,
                        normal,
                        uv: None,
                    });
                    row.push(Some(index));
                } else {
                    row.push(None);
                }
            } else {
                row.push(None);
            }
        }
        vertex_grid.push(row);
    }

    // Generate triangles. Winding follows `face.orientation`
    // (see cylindrical path for rationale).
    let forward = face.orientation.is_forward();
    for v_idx in 0..v_steps {
        for u_idx in 0..u_steps {
            if let (Some(v0), Some(v1), Some(v2), Some(v3)) = (
                vertex_grid[v_idx].get(u_idx).and_then(|&v| v),
                vertex_grid[v_idx].get(u_idx + 1).and_then(|&v| v),
                vertex_grid[v_idx + 1].get(u_idx).and_then(|&v| v),
                vertex_grid[v_idx + 1].get(u_idx + 1).and_then(|&v| v),
            ) {
                if forward {
                    mesh.add_triangle(v0, v1, v2);
                    mesh.add_triangle(v1, v3, v2);
                } else {
                    mesh.add_triangle(v0, v2, v1);
                    mesh.add_triangle(v1, v2, v3);
                }
            }
        }
    }
}

/// Get parameter bounds for a face from its loops
fn get_face_parameter_bounds(face: &Face, model: &BRepModel) -> (f64, f64, f64, f64) {
    let mut u_min = f64::MAX;
    let mut u_max = f64::MIN;
    let mut v_min = f64::MAX;
    let mut v_max = f64::MIN;

    // Get surface for parameter evaluation. The original `None` arm
    // re-queried the same missing surface and unwrapped it, which would
    // have panicked. Since the surface is genuinely missing, return a
    // neutral zero-extent bound rather than panicking mid-tessellation.
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return (0.0, 0.0, 0.0, 0.0),
    };

    // Process outer loop
    if let Some(outer_loop) = model.loops.get(face.outer_loop) {
        update_bounds_from_loop(
            outer_loop, model, surface, &mut u_min, &mut u_max, &mut v_min, &mut v_max,
        );
    }

    // Process inner loops (holes)
    for &inner_loop_id in &face.inner_loops {
        if let Some(inner_loop) = model.loops.get(inner_loop_id) {
            update_bounds_from_loop(
                inner_loop, model, surface, &mut u_min, &mut u_max, &mut v_min, &mut v_max,
            );
        }
    }

    // Ensure valid bounds
    if u_min > u_max || v_min > v_max {
        // Fallback to surface bounds
        let (u_range, v_range) = surface.parameter_bounds();
        return (u_range.0, u_range.1, v_range.0, v_range.1);
    }

    // Degenerate-axis collapse: a face whose outer loop projects onto a
    // single u- or v-line (e.g. an apex-degenerate cone whose only edge
    // is the wide-end circle, sampled entirely at v = height) yields a
    // zero-span axis here. The face still covers the full surface extent
    // along that axis (the apex is a topological point with no edge);
    // fall back to the surface's parameter bound for any collapsed axis
    // so the grid tessellator has a non-zero region to sample.
    const DEGENERATE_TOL: f64 = 1e-9;
    let (u_range, v_range) = surface.parameter_bounds();
    if (u_max - u_min) < DEGENERATE_TOL {
        u_min = u_range.0;
        u_max = u_range.1;
    }
    if (v_max - v_min) < DEGENERATE_TOL {
        v_min = v_range.0;
        v_max = v_range.1;
    }

    // Add a small margin for numerical stability, but never let the
    // expanded interval escape the surface's own parameter domain. Going
    // below the surface's v_min in particular collides with apex-detection
    // logic (`v_min.abs() < 1e-6`) downstream, and going beyond a surface's
    // u/v limits has no defined evaluation.
    let u_margin = (u_max - u_min) * 0.01;
    let v_margin = (v_max - v_min) * 0.01;
    (
        (u_min - u_margin).max(u_range.0),
        (u_max + u_margin).min(u_range.1),
        (v_min - v_margin).max(v_range.0),
        (v_max + v_margin).min(v_range.1),
    )
}

/// Update parameter bounds from a loop.
///
/// Routes through `project_loop_uv_unwrapped` so the bounds reflect the
/// loop's true span in the lifted parameter domain. Without the unwrap
/// a closed bottom_circle on a cylinder would produce
/// `u_max - u_min ≈ π` (samples `0, π/10, ..., 19π/10` then wrap
/// modulo `2π`) instead of the correct `2π`, causing the grid
/// tessellator to cover only half the cylinder.
fn update_bounds_from_loop(
    loop_data: &crate::primitives::r#loop::Loop,
    model: &BRepModel,
    surface: &dyn Surface,
    u_min: &mut f64,
    u_max: &mut f64,
    v_min: &mut f64,
    v_max: &mut f64,
) {
    // Bounds extremum scan: must include both endpoints of each edge
    // so a sphere's seam-edge sample at t=π hits v=π (otherwise v_max
    // would clamp to 10π/11, missing the north-pole region).
    let polygon = project_loop_uv_unwrapped(loop_data, model, surface, 10, true);
    for (u, v) in polygon {
        *u_min = u_min.min(u);
        *u_max = u_max.max(u);
        *v_min = v_min.min(v);
        *v_max = v_max.max(v);
    }
}

/// Check if a parameter point is inside face boundaries using winding number algorithm
fn is_point_inside_face(u: f64, v: f64, face: &Face, model: &BRepModel) -> bool {
    // First check outer loop - point must be inside
    if !is_point_inside_loop(u, v, face.outer_loop, face, model) {
        return false;
    }

    // Then check inner loops (holes) - point must be outside all holes
    for &inner_loop_id in &face.inner_loops {
        if is_point_inside_loop(u, v, inner_loop_id, face, model) {
            return false;
        }
    }

    true
}

/// Check if a point is inside a loop using winding number algorithm.
///
/// Handles three cases explicitly:
///
/// 1. **Non-degenerate polygon** — winding-number test (Sunday 2001).
///    A non-zero winding number indicates the point is enclosed.
///
/// 2. **Degenerate polygon** (fewer than 3 distinct samples, or
///    near-zero signed area) — the loop is a topological seam, not a
///    meaningful boundary in parameter space. The canonical case is a
///    sphere face whose outer loop is a single seam edge traversed
///    forward then reversed; in `(u, v)` it collapses onto the line
///    `u = 0`. For an **outer** loop this means the face covers the
///    full parametric domain — accept any point. For an **inner** loop
///    (a hole) it means there is effectively no hole — reject any
///    point as not-in-hole.
///
/// 3. **Missing loop / surface** — return `false` for safety.
fn is_point_inside_loop(
    u: f64,
    v: f64,
    loop_id: crate::primitives::r#loop::LoopId,
    face: &Face,
    model: &BRepModel,
) -> bool {
    let loop_data = match model.loops.get(loop_id) {
        Some(l) => l,
        None => return false,
    };

    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return false,
    };

    let polygon = get_loop_polygon_2d(loop_data, model, surface);

    // Degenerate-polygon fallback. Tolerance chosen well below any
    // realistic face area in radians² (a 1-arc-second-square loop has
    // area ≈ 2.3e-11) yet large enough to absorb f64 round-off in
    // `closest_point` projections (~1e-15 per sample × 20 samples per
    // edge × O(1) edges ≈ 2e-14 noise floor).
    const DEGENERATE_AREA_TOL: f64 = 1e-12;
    let is_outer = matches!(
        loop_data.loop_type,
        crate::primitives::r#loop::LoopType::Outer
    );
    if polygon.len() < 3 {
        return is_outer;
    }
    if polygon_signed_area_uv(&polygon).abs() < DEGENERATE_AREA_TOL {
        return is_outer;
    }

    let winding_number = calculate_winding_number(&(u, v), &polygon);
    winding_number.abs() > 0.5
}

/// Get loop as 2D polygon in parameter space.
///
/// Thin wrapper over `project_loop_uv_unwrapped`; kept as a named entry
/// point for the winding-number test in `is_point_inside_loop`.
fn get_loop_polygon_2d(
    loop_data: &crate::primitives::r#loop::Loop,
    model: &BRepModel,
    surface: &dyn Surface,
) -> Vec<(f64, f64)> {
    // Closed loop: drop trailing endpoint of each edge to avoid
    // duplicating the seam vertex with the next edge's start.
    project_loop_uv_unwrapped(loop_data, model, surface, 20, false)
}

/// Project a B-Rep loop into the surface's `(u, v)` parameter space,
/// unwrapping across periodicity discontinuities so consecutive samples
/// form a continuous trace.
///
/// # Why the unwrap is required
/// `Surface::closest_point` returns canonical `(u, v)` in the surface's
/// declared parameter bounds — for a cylinder/sphere/torus this means
/// `u ∈ [0, 2π)`. Without unwrapping, sampling a closed loop edge (e.g.
/// the bottom_circle of a cylinder, parameterised `t ∈ [0, 2π]`)
/// produces u-coordinates that jump from `≈ 2π` back to `0` at the
/// seam. The resulting 2D polygon self-intersects and downstream
/// winding-number / bounding-box logic fails:
///
///   * sphere face's seam-only outer loop projects to all `u = 0`
///     (collapsed seam) — the face covers the entire surface but the
///     winding test classifies every interior sample as "outside";
///   * cylinder lateral's bottom_circle projects to `0 → π → 2π → 0`
///     instead of monotone `0 → π → 2π → 4π`, the winding number is
///     wrong over most of the surface.
///
/// Unwrapping pulls each new sample within `period/2` of the previous
/// one, preserving the topological intent (the trace is the lift of
/// the closed loop into the universal cover of the parameter domain).
///
/// # Arguments
/// * `loop_data`        - The loop whose edges are sampled in order
/// * `model`            - B-Rep model for edge / curve lookup
/// * `surface`          - Owning surface; queried for periodicity
/// * `intervals`        - Number of equal sub-intervals along each
///                        edge's parameter range
/// * `inclusive`        - If `true`, sample at both endpoints (gives
///                        `intervals + 1` samples, used for
///                        bounds-extremum scans). If `false`, sample
///                        `[t_start, t_end)` (gives `intervals`
///                        samples; preferred for closed loops to avoid
///                        duplicating the seam vertex with the next
///                        edge's start).
///
/// # Returns
/// `(u, v)` polygon, possibly empty if no edges produced valid samples.
fn project_loop_uv_unwrapped(
    loop_data: &crate::primitives::r#loop::Loop,
    model: &BRepModel,
    surface: &dyn Surface,
    intervals: usize,
    inclusive: bool,
) -> Vec<(f64, f64)> {
    let u_period = surface.period_u();
    let v_period = surface.period_v();
    let upper = if inclusive { intervals + 1 } else { intervals };
    let mut polygon = Vec::with_capacity(loop_data.edges.len() * upper);
    let mut last: Option<(f64, f64)> = None;

    for (edge_idx, &edge_id) in loop_data.edges.iter().enumerate() {
        let edge = match model.edges.get(edge_id) {
            Some(e) => e,
            None => continue,
        };
        let curve = match model.curves.get(edge.curve_id) {
            Some(c) => c,
            None => continue,
        };
        // Honor the loop's recorded edge orientation: when the loop
        // traverses an edge in reverse (orientations[i] == false), we
        // must sample its parameter range from end → start, otherwise a
        // sphere face's seam-edge-traversed-twice loop projects as
        // *forward + forward* in (u, v) and accumulates a non-zero
        // signed area. The degenerate-loop fallback in
        // `is_point_inside_loop` would then fail to fire and the
        // winding-number test rejects most interior samples.
        let forward = loop_data
            .orientations
            .get(edge_idx)
            .copied()
            .unwrap_or(true);
        let (t_a, t_b) = if forward {
            (edge.param_range.start, edge.param_range.end)
        } else {
            (edge.param_range.end, edge.param_range.start)
        };
        for i in 0..upper {
            let t = t_a + (i as f64) * (t_b - t_a) / (intervals as f64);
            let point_3d = match curve.point_at(t) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let (mut u, mut v) = match surface.closest_point(&point_3d, Tolerance::default()) {
                Ok(uv) => uv,
                Err(_) => continue,
            };
            if let Some((prev_u, prev_v)) = last {
                if let Some(period) = u_period {
                    let half = period * 0.5;
                    while u - prev_u > half {
                        u -= period;
                    }
                    while u - prev_u < -half {
                        u += period;
                    }
                }
                if let Some(period) = v_period {
                    let half = period * 0.5;
                    while v - prev_v > half {
                        v -= period;
                    }
                    while v - prev_v < -half {
                        v += period;
                    }
                }
            }
            polygon.push((u, v));
            last = Some((u, v));
        }
    }

    polygon
}

/// Compute the signed area of a closed `(u, v)` polygon (shoelace).
///
/// Used by the degenerate-loop fallback in `is_point_inside_loop` to
/// detect seam-only outer loops (sphere) whose unwrapped projection
/// still collapses onto a single line in parameter space.
fn polygon_signed_area_uv(polygon: &[(f64, f64)]) -> f64 {
    let n = polygon.len();
    if n < 3 {
        return 0.0;
    }
    let mut sum = 0.0;
    for i in 0..n {
        let (x0, y0) = polygon[i];
        let (x1, y1) = polygon[(i + 1) % n];
        sum += x0 * y1 - x1 * y0;
    }
    sum * 0.5
}

/// Calculate winding number for point-in-polygon test
fn calculate_winding_number(point: &(f64, f64), polygon: &[(f64, f64)]) -> f64 {
    let mut winding_number = 0.0;
    let n = polygon.len();

    for i in 0..n {
        let p1 = polygon[i];
        let p2 = polygon[(i + 1) % n];

        // Calculate angle subtended by edge at the point
        let v1 = (p1.0 - point.0, p1.1 - point.1);
        let v2 = (p2.0 - point.0, p2.1 - point.1);

        // Use atan2 for robust angle calculation
        let angle1 = v1.1.atan2(v1.0);
        let angle2 = v2.1.atan2(v2.0);

        let mut delta = angle2 - angle1;

        // Normalize to [-π, π]
        while delta > std::f64::consts::PI {
            delta -= 2.0 * std::f64::consts::PI;
        }
        while delta < -std::f64::consts::PI {
            delta += 2.0 * std::f64::consts::PI;
        }

        winding_number += delta;
    }

    // Normalize to winding number
    winding_number / (2.0 * std::f64::consts::PI)
}

/// Tessellate a surface patch with adaptive refinement
pub fn tessellate_surface(
    surface: &dyn Surface,
    u_range: (f64, f64),
    v_range: (f64, f64),
    _params: &TessellationParams,
) -> TriangleMesh {
    let mut mesh = TriangleMesh::new();

    // Simple uniform tessellation for now
    let u_steps = 10;
    let v_steps = 10;

    // Generate vertices
    for v_idx in 0..=v_steps {
        let v = v_range.0 + (v_idx as f64) * (v_range.1 - v_range.0) / (v_steps as f64);

        for u_idx in 0..=u_steps {
            let u = u_range.0 + (u_idx as f64) * (u_range.1 - u_range.0) / (u_steps as f64);

            if let Ok(eval) = surface.evaluate_full(u, v) {
                mesh.add_vertex(MeshVertex {
                    position: eval.position,
                    normal: eval.normal,
                    uv: Some((u, v)),
                });
            }
        }
    }

    // Generate triangles
    for v_idx in 0..v_steps {
        for u_idx in 0..u_steps {
            let v0 = (v_idx * (u_steps + 1) + u_idx) as u32;
            let v1 = v0 + 1;
            let v2 = v0 + (u_steps + 1) as u32;
            let v3 = v2 + 1;

            mesh.add_triangle(v0, v1, v2);
            mesh.add_triangle(v1, v3, v2);
        }
    }

    mesh
}

/// Adaptive NURBS tessellation with curvature-based refinement
fn tessellate_nurbs_adaptive(
    surface: &dyn Surface,
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
) {
    // Initial quadtree subdivision based on surface complexity
    let mut quad_tree = QuadTree::new(u_min, u_max, v_min, v_max);

    // Perform adaptive subdivision
    subdivide_nurbs_quad(
        &mut quad_tree,
        surface,
        face,
        model,
        params,
        u_min,
        u_max,
        v_min,
        v_max,
        0,
    );

    // Convert quadtree to triangles. The vertex map is kept locally for
    // QuadTree->mesh stamping; watertight welding is handled at the
    // shell level by `tessellate_shell`, so we don't run it here.
    let _ = quad_tree_to_mesh(&quad_tree, surface, face, model, mesh);
}

/// Quadtree structure for adaptive subdivision
struct QuadTree {
    nodes: Vec<QuadNode>,
}

struct QuadNode {
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
    children: Option<[usize; 4]>,
}

impl QuadTree {
    fn new(u_min: f64, u_max: f64, v_min: f64, v_max: f64) -> Self {
        let root = QuadNode {
            u_min,
            u_max,
            v_min,
            v_max,
            children: None,
        };
        Self { nodes: vec![root] }
    }

    fn subdivide(&mut self, node_idx: usize) -> [usize; 4] {
        // Copy the node data to avoid borrowing issues
        let (u_min, u_max, v_min, v_max) = {
            let node = &self.nodes[node_idx];
            (node.u_min, node.u_max, node.v_min, node.v_max)
        };

        let u_mid = (u_min + u_max) / 2.0;
        let v_mid = (v_min + v_max) / 2.0;

        // Create 4 child nodes
        let children = [
            self.add_node(u_min, u_mid, v_min, v_mid), // SW
            self.add_node(u_mid, u_max, v_min, v_mid), // SE
            self.add_node(u_mid, u_max, v_mid, v_max), // NE
            self.add_node(u_min, u_mid, v_mid, v_max), // NW
        ];

        self.nodes[node_idx].children = Some(children);
        children
    }

    fn add_node(&mut self, u_min: f64, u_max: f64, v_min: f64, v_max: f64) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(QuadNode {
            u_min,
            u_max,
            v_min,
            v_max,
            children: None,
        });
        idx
    }
}

/// Recursive subdivision based on curvature
fn subdivide_nurbs_quad(
    quad_tree: &mut QuadTree,
    surface: &dyn Surface,
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
    depth: usize,
) {
    const MAX_DEPTH: usize = 12;

    // Check if we should subdivide based on curvature
    if depth >= MAX_DEPTH
        || !should_subdivide_nurbs(surface, face, model, params, u_min, u_max, v_min, v_max)
    {
        return;
    }

    // Get current node
    let node_idx = quad_tree.nodes.len() - 1;

    // Subdivide into 4 children
    let _children = quad_tree.subdivide(node_idx);

    // Recursively subdivide children
    let u_mid = (u_min + u_max) / 2.0;
    let v_mid = (v_min + v_max) / 2.0;

    subdivide_nurbs_quad(
        quad_tree,
        surface,
        face,
        model,
        params,
        u_min,
        u_mid,
        v_min,
        v_mid,
        depth + 1,
    );
    subdivide_nurbs_quad(
        quad_tree,
        surface,
        face,
        model,
        params,
        u_mid,
        u_max,
        v_min,
        v_mid,
        depth + 1,
    );
    subdivide_nurbs_quad(
        quad_tree,
        surface,
        face,
        model,
        params,
        u_mid,
        u_max,
        v_mid,
        v_max,
        depth + 1,
    );
    subdivide_nurbs_quad(
        quad_tree,
        surface,
        face,
        model,
        params,
        u_min,
        u_mid,
        v_mid,
        v_max,
        depth + 1,
    );
}

/// Check if a quad should be subdivided based on curvature
fn should_subdivide_nurbs(
    surface: &dyn Surface,
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
) -> bool {
    // Sample curvature at multiple points
    let sample_points = [
        (u_min, v_min),
        (u_max, v_min),
        (u_max, v_max),
        (u_min, v_max),
        ((u_min + u_max) / 2.0, (v_min + v_max) / 2.0),
    ];

    let mut max_curvature = 0.0f64;
    let mut max_normal_deviation = 0.0f64;
    let mut normals = Vec::new();

    for &(u, v) in &sample_points {
        if !is_point_inside_face(u, v, face, model) {
            continue;
        }

        if let Ok(eval) = surface.evaluate_full(u, v) {
            // Check curvature
            let k = eval.k1.abs().max(eval.k2.abs());
            max_curvature = max_curvature.max(k);

            // Collect normals for deviation check
            normals.push(eval.normal);
        }
    }

    // Check normal deviation
    for i in 0..normals.len() {
        for j in i + 1..normals.len() {
            if let Ok(angle) = normals[i].angle(&normals[j]) {
                max_normal_deviation = max_normal_deviation.max(angle);
            }
        }
    }

    // Subdivision criteria
    let patch_size = ((u_max - u_min).powi(2) + (v_max - v_min).powi(2)).sqrt();

    // Curvature-based criterion
    if max_curvature > 1e-10 {
        let required_size = (8.0 * params.chord_tolerance / max_curvature).sqrt();
        if patch_size > required_size {
            return true;
        }
    }

    // Normal deviation criterion
    if max_normal_deviation > params.max_angle_deviation {
        return true;
    }

    // Edge length criterion
    let estimated_edge_length = patch_size
        * surface
            .point_at(u_min, v_min)
            .map(|p1| {
                surface
                    .point_at(u_max, v_max)
                    .map(|p2| p1.distance(&p2) / patch_size)
                    .unwrap_or(1.0)
            })
            .unwrap_or(1.0);

    estimated_edge_length > params.max_edge_length
}

/// Convert quadtree to triangle mesh
fn quad_tree_to_mesh(
    quad_tree: &QuadTree,
    surface: &dyn Surface,
    face: &Face,
    model: &BRepModel,
    mesh: &mut TriangleMesh,
) -> HashMap<(usize, usize), u32> {
    let mut vertex_map = HashMap::new();

    // Process all leaf nodes
    for node in quad_tree.nodes.iter() {
        if node.children.is_none() {
            // This is a leaf node - tessellate it
            let vertices = [
                (node.u_min, node.v_min),
                (node.u_max, node.v_min),
                (node.u_max, node.v_max),
                (node.u_min, node.v_max),
            ];

            let mut indices = Vec::new();

            for &(u, v) in &vertices {
                if is_point_inside_face(u, v, face, model) {
                    let key = discretize_uv(u, v);
                    let vertex_idx = *vertex_map.entry(key).or_insert_with(|| {
                        if let (Ok(point), Ok(normal)) = (
                            surface.point_at(u, v),
                            face.normal_at(u, v, &model.surfaces),
                        ) {
                            mesh.add_vertex(MeshVertex {
                                position: point,
                                normal,
                                uv: Some((u, v)),
                            })
                        } else {
                            0 // Should not happen with proper face boundaries
                        }
                    });
                    indices.push(vertex_idx);
                }
            }

            // Create triangles if we have all 4 vertices. Winding
            // follows `face.orientation` so the geometric normal
            // agrees with the stored vertex normal — see cylindrical
            // path for the full rationale.
            let forward = face.orientation.is_forward();
            if indices.len() == 4 {
                if forward {
                    mesh.add_triangle(indices[0], indices[1], indices[2]);
                    mesh.add_triangle(indices[0], indices[2], indices[3]);
                } else {
                    mesh.add_triangle(indices[0], indices[2], indices[1]);
                    mesh.add_triangle(indices[0], indices[3], indices[2]);
                }
            } else if indices.len() == 3 {
                if forward {
                    mesh.add_triangle(indices[0], indices[1], indices[2]);
                } else {
                    mesh.add_triangle(indices[0], indices[2], indices[1]);
                }
            }
        }
    }

    vertex_map
}

/// Discretize UV coordinates for vertex sharing
fn discretize_uv(u: f64, v: f64) -> (usize, usize) {
    const RESOLUTION: f64 = 1e6;
    (
        (u * RESOLUTION).round() as usize,
        (v * RESOLUTION).round() as usize,
    )
}

/// Weld coincident vertices into a single index, producing a watertight
/// triangle mesh.
///
/// Tessellation emits each face independently — adjacent faces sharing
/// a B-Rep edge sample its curve at the same canonical parameters, so
/// they produce **3D-coincident vertices** along the shared boundary
/// (the per-edge sampling is symmetric: forward face A at {t_start,
/// t_start+Δ, …, t_end-Δ} ∪ {t_end via next edge} and backward face B
/// at {t_end, t_end-Δ, …, t_start+Δ} ∪ {t_start via next edge} contain
/// the same N+1 parameters). What is missing without this pass is the
/// **index unification** — the mesh has two distinct vertex IDs at the
/// same 3D position, so the seam appears as a topological gap to any
/// downstream consumer (STL export, BVH builder, edge-flow analysis).
///
/// Algorithm: voxel-grid spatial hash, O(n) expected, neighbourhood
/// scan over the 27 surrounding cells. Indices ≥ i are never collapsed
/// onto i (we always keep the lower index as canonical). Triangles are
/// rewritten with the remapped indices in place; orphaned vertices in
/// `mesh.vertices` are not garbage-collected (the rendering layer
/// tolerates them, and downstream STL/OBJ exporters apply their own
/// dedup pass — see `export-engine/src/validation.rs`).
///
/// `weld_tolerance` should match the kernel's geometric tolerance for
/// the model — typically `1e-6` for mm-scale parts, looser for
/// metre-scale assemblies. The grid cell size is chosen as
/// `weld_tolerance.max(1e-9) * 1e3` so that a 1×1×1 cell comfortably
/// brackets any pair within tolerance even at the cell edges.
pub(crate) fn weld_mesh_watertight(mesh: &mut TriangleMesh, weld_tolerance: f64) {
    weld_mesh_watertight_range(mesh, weld_tolerance, 0, 0);
}

/// Range-restricted variant of [`weld_mesh_watertight`] used by
/// `tessellate_shell` to weld each shell independently while preserving
/// vertex/triangle indices from earlier shells already in the mesh.
///
/// Welds only vertices at indices `>= v_start` and triangles at indices
/// `>= t_start`. Cross-shell coincidences (e.g. between an outer shell
/// and an inner void shell) are intentionally left un-welded — they
/// represent topologically-distinct boundaries.
pub(crate) fn weld_mesh_watertight_range(
    mesh: &mut TriangleMesh,
    weld_tolerance: f64,
    v_start: usize,
    t_start: usize,
) {
    let n = mesh.vertices.len();
    let m = mesh.triangles.len();
    if v_start >= n || t_start >= m {
        return;
    }

    // Cell size: a few orders of magnitude larger than tolerance so two
    // points within tolerance reliably share a cell or land in adjacent
    // cells. Floor at 1e-9 to avoid pathological 0/negative tolerances
    // collapsing every vertex onto the origin cell.
    let safe_tol = weld_tolerance.max(1e-9);
    let grid_size = safe_tol * 1.0e3;
    let inv_grid = 1.0 / grid_size;
    let tol_sq = safe_tol * safe_tol;

    let to_cell = |p: Point3| -> (i32, i32, i32) {
        // Defensive non-finite handling: treat NaN/inf positions as
        // their own bucket so they don't poison the dedup pass.
        if !p.x.is_finite() || !p.y.is_finite() || !p.z.is_finite() {
            return (i32::MIN, i32::MIN, i32::MIN);
        }
        (
            (p.x * inv_grid).floor() as i32,
            (p.y * inv_grid).floor() as i32,
            (p.z * inv_grid).floor() as i32,
        )
    };

    let mut spatial_hash: HashMap<(i32, i32, i32), Vec<u32>> =
        HashMap::with_capacity(n - v_start);
    for i in v_start..n {
        spatial_hash
            .entry(to_cell(mesh.vertices[i].position))
            .or_default()
            .push(i as u32);
    }

    // remap[i] = canonical index for vertex i, only meaningful for
    // i >= v_start. Earlier vertices are identity-mapped (we don't
    // touch them).
    let mut remap: Vec<u32> = (0..n as u32).collect();

    for i in v_start..n {
        let pos = mesh.vertices[i].position;
        let (cx, cy, cz) = to_cell(pos);

        // Scan the 3×3×3 neighbourhood. Stop at the first vertex with
        // a strictly-smaller original index (still inside the welding
        // range — `cand >= v_start`) that is within tolerance — we
        // keep the lowest index as canonical, which gives a
        // deterministic mapping regardless of insertion order.
        let mut canonical = i as u32;
        'scan: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(bucket) = spatial_hash.get(&(cx + dx, cy + dy, cz + dz)) {
                        for &cand in bucket {
                            if (cand as usize) < v_start || cand >= i as u32 {
                                continue;
                            }
                            let dp = mesh.vertices[cand as usize].position - pos;
                            if dp.dot(&dp) <= tol_sq {
                                canonical = remap[cand as usize];
                                break 'scan;
                            }
                        }
                    }
                }
            }
        }
        remap[i] = canonical;
    }

    let mut welded: u32 = 0;
    for i in v_start..n {
        if remap[i] != i as u32 {
            welded += 1;
        }
    }

    // K14 — G1 normal continuity at smooth seams.
    //
    // Accumulate every welded contributor's normal into its canonical
    // bucket. Then, for canonicals with ≥ 2 contributors, write back
    // the unit-length average **only when contributors agree** — i.e.
    // when |Σnᵢ| / N exceeds `G1_SMOOTHNESS_THRESHOLD`.
    //
    // This is a length-of-mean test: identical normals give |avg| = 1;
    // 18° spread gives |avg| ≈ 0.95; a 90° box corner gives
    // |avg| ≈ 0.71; opposing seam normals collapse to |avg| ≈ 0.
    // The 0.95 threshold accepts smooth cylinder / sphere / NURBS
    // seams (where adjacent faces share the same surface tangent at
    // the seam) and rejects sharp B-Rep edges (where each face's
    // normal is correct as emitted; averaging them would smear the
    // shading discontinuity that the renderer needs).
    //
    // The canonical's own original normal is included in the sum.
    // No vertex is duplicated and the watertight invariant from
    // `weld_mesh_watertight` is preserved — only the canonical's
    // `MeshVertex.normal` is mutated in place.
    const G1_SMOOTHNESS_THRESHOLD: f64 = 0.95;
    let mut normal_accum: HashMap<u32, (Vector3, u32)> = HashMap::with_capacity(n - v_start);
    for i in v_start..n {
        let canon = remap[i];
        let ni = mesh.vertices[i].normal;
        let entry = normal_accum
            .entry(canon)
            .or_insert((Vector3::new(0.0, 0.0, 0.0), 0));
        entry.0 = entry.0 + ni;
        entry.1 += 1;
    }
    let mut g1_smoothed: u32 = 0;
    for (canon, (sum, count)) in normal_accum.iter() {
        if *count <= 1 {
            continue;
        }
        let inv_count = 1.0 / (*count as f64);
        let avg = *sum * inv_count;
        let mag = avg.dot(&avg).sqrt();
        if mag >= G1_SMOOTHNESS_THRESHOLD {
            // Defensive: mag was just verified ≥ 0.95 so 1/mag is finite.
            mesh.vertices[*canon as usize].normal = avg * (1.0 / mag);
            g1_smoothed += 1;
        }
        // else: sharp edge — preserve canonical's first-emitter normal.
    }

    // Rewrite triangle indices in [t_start..]. Drop triangles that
    // collapse to a degenerate sliver (two indices remap to the same
    // canonical) and keep `face_map` consistent with the surviving
    // triangles — both arrays are indexed in lock-step, so a single
    // combined walk is the only way to preserve that invariant.
    let has_face_map = mesh.face_map.len() == m;
    let head_triangles: Vec<[u32; 3]> = mesh.triangles[..t_start].to_vec();
    let head_face_map: Vec<u32> = if has_face_map {
        mesh.face_map[..t_start].to_vec()
    } else {
        Vec::new()
    };
    let mut new_triangles: Vec<[u32; 3]> = Vec::with_capacity(m);
    let mut new_face_map: Vec<u32> = if has_face_map {
        Vec::with_capacity(m)
    } else {
        Vec::new()
    };
    new_triangles.extend(head_triangles);
    if has_face_map {
        new_face_map.extend(head_face_map);
    }
    for idx in t_start..m {
        let tri = mesh.triangles[idx];
        let a = remap[tri[0] as usize];
        let b = remap[tri[1] as usize];
        let c = remap[tri[2] as usize];
        if a == b || b == c || a == c {
            continue;
        }
        new_triangles.push([a, b, c]);
        if has_face_map {
            new_face_map.push(mesh.face_map[idx]);
        }
    }
    mesh.triangles = new_triangles;
    if has_face_map {
        mesh.face_map = new_face_map;
    }

    if welded > 0 || g1_smoothed > 0 {
        tracing::debug!(
            "weld_mesh_watertight_range: collapsed {welded} duplicate vertices, \
             G1-smoothed {g1_smoothed} canonical normals \
             (tol={weld_tolerance:e}, v_start={v_start})"
        );
    }
}

#[cfg(test)]
mod tests {
    //! Direct regression tests for the planar-face triangulation pipeline.
    //!
    //! These exercise the pure 2D entry point (`triangulate_planar_polygon`)
    //! and its helpers without going through `BRepModel`, so they double as
    //! algorithm-level invariants:
    //!
    //!   * Simple square (CCW input)  → ≥ 2 triangles, total signed area == 1.
    //!   * Simple square (CW input)   → ≥ 2 triangles (shoelace correction).
    //!   * Square with square hole    → triangles cover (outer − hole) area,
    //!                                  none has its centroid inside the hole.
    //!
    //! Each test ran red against the prior Bowyer-Watson + constraint-
    //! enforcement implementation (the box demo in `quick_demo` produced
    //! 0 triangles); they pass against the new bridged ear-clipping path.
    use super::*;
    use crate::math::Point3;

    /// Build a Z-up planar polygon: outer + optional CW holes.
    fn build_planar_loops(
        outer: &[(f64, f64)],
        holes: &[&[(f64, f64)]],
    ) -> (Vec<Point3>, Vec<(usize, usize, bool)>) {
        let mut vertices = Vec::new();
        let mut boundaries = Vec::new();
        let start = vertices.len();
        for &(x, y) in outer {
            vertices.push(Point3::new(x, y, 0.0));
        }
        boundaries.push((start, vertices.len(), true));
        for &hole in holes {
            let s = vertices.len();
            for &(x, y) in hole {
                vertices.push(Point3::new(x, y, 0.0));
            }
            boundaries.push((s, vertices.len(), false));
        }
        (vertices, boundaries)
    }

    /// Sum of triangle areas (taken in 2D, ignoring z).
    fn total_tri_area_xy(vertices: &[Point3], tris: &[[usize; 3]]) -> f64 {
        tris.iter()
            .map(|t| {
                let a = vertices[t[0]];
                let b = vertices[t[1]];
                let c = vertices[t[2]];
                ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)).abs() * 0.5
            })
            .sum()
    }

    /// Centroid of a triangle in 2D.
    fn tri_centroid_xy(vertices: &[Point3], tri: [usize; 3]) -> (f64, f64) {
        let a = vertices[tri[0]];
        let b = vertices[tri[1]];
        let c = vertices[tri[2]];
        ((a.x + b.x + c.x) / 3.0, (a.y + b.y + c.y) / 3.0)
    }

    #[test]
    fn signed_area_ccw_is_positive() {
        let v = vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        let poly: Vec<usize> = (0..v.len()).collect();
        assert!(polygon_signed_area_2d(&v, &poly) > 0.0);
    }

    #[test]
    fn signed_area_cw_is_negative() {
        let v = vec![(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0)];
        let poly: Vec<usize> = (0..v.len()).collect();
        assert!(polygon_signed_area_2d(&v, &poly) < 0.0);
    }

    #[test]
    fn planar_face_simple_quad_ccw() {
        // 1x1 unit square, CCW. Must produce ≥ 2 tris totalling area 1.
        let (verts, loops) = build_planar_loops(
            &[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)],
            &[],
        );
        let tris = triangulate_planar_polygon(&verts, &loops, &Vector3::Z);
        assert!(tris.len() >= 2, "expected ≥2 tris, got {}", tris.len());
        let area = total_tri_area_xy(&verts, &tris);
        assert!(
            (area - 1.0).abs() < 1e-9,
            "tri area sum {area} ≠ outer area 1.0"
        );
    }

    #[test]
    fn planar_face_simple_quad_cw_input_is_auto_corrected() {
        // Same square, but CW. Algorithm must shoelace-correct to CCW
        // before ear-clipping rather than return zero triangles.
        let (verts, loops) = build_planar_loops(
            &[(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0)],
            &[],
        );
        let tris = triangulate_planar_polygon(&verts, &loops, &Vector3::Z);
        assert!(tris.len() >= 2, "expected ≥2 tris, got {}", tris.len());
        let area = total_tri_area_xy(&verts, &tris);
        assert!((area - 1.0).abs() < 1e-9, "tri area sum {area} ≠ 1.0");
    }

    #[test]
    fn planar_face_quad_with_square_hole() {
        // 4x4 outer (CCW), 1x1 hole in middle (CW). Expected face area =
        // 16 − 1 = 15. Every triangle's centroid must lie outside the hole.
        let (verts, loops) = build_planar_loops(
            &[(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0)],
            &[&[(1.5, 1.5), (1.5, 2.5), (2.5, 2.5), (2.5, 1.5)]],
        );
        let tris = triangulate_planar_polygon(&verts, &loops, &Vector3::Z);
        assert!(
            tris.len() >= 8,
            "outer-with-hole should produce ≥8 tris, got {}",
            tris.len()
        );
        let area = total_tri_area_xy(&verts, &tris);
        assert!(
            (area - 15.0).abs() < 1e-9,
            "tri area sum {area} ≠ (outer − hole) 15.0"
        );
        for &t in &tris {
            let (cx, cy) = tri_centroid_xy(&verts, t);
            let inside_hole = cx > 1.5 && cx < 2.5 && cy > 1.5 && cy < 2.5;
            assert!(
                !inside_hole,
                "triangle centroid ({cx}, {cy}) lies inside hole — bridging failed"
            );
        }
    }

    #[test]
    fn planar_face_degenerate_loops_return_empty() {
        // Outer with only 2 vertices (degenerate). Must produce no tris,
        // not panic, not produce garbage triangles referencing OOB indices.
        let (verts, loops) = build_planar_loops(&[(0.0, 0.0), (1.0, 0.0)], &[]);
        let tris = triangulate_planar_polygon(&verts, &loops, &Vector3::Z);
        assert!(tris.is_empty());
    }
}
