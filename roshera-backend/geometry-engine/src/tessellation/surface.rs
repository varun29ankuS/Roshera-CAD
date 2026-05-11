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

/// Number of subdivisions across an angular `span` on a circle of given
/// `radius` to satisfy every quality bound in `params`. Returns the max
/// of three step counts so the strictest constraint always wins:
///
/// - **Chord-height (sagitta)** — `θ ≤ 2·acos(1 − chord_tolerance/radius)`.
///   The perpendicular deviation from the true arc stays below
///   `chord_tolerance`. This is size-invariant in the quality-per-pixel
///   sense (segments per arc grow as √radius, not radius), which is why
///   it's the primary driver here. Falls back to `min_segments` if
///   `chord_tolerance ≥ radius` (degenerate over-coarse setting).
/// - **Chord length** — `θ ≤ 2·asin(max_edge_length / (2·radius))`.
///   Caps the *geometric* edge length of mesh triangles. Useful for
///   shaders and downstream consumers that care about absolute size.
/// - **Angle deviation** — `θ ≤ max_angle_deviation`. Caps the parametric
///   step regardless of curvature. Becomes the binding constraint on
///   small radii where chord-height would otherwise demand very large θ.
///
/// Final result is clamped to `[params.min_segments, params.max_segments]`.
fn arc_steps_for_quality(span: f64, radius: f64, params: &TessellationParams) -> usize {
    if span <= 0.0 || radius <= 0.0 {
        return params.min_segments;
    }

    let from_sagitta = if params.chord_tolerance > 0.0 && params.chord_tolerance < radius {
        // cos(θ/2) = 1 − h/r, with h = chord_tolerance. The guard above
        // keeps the argument strictly in (0, 1) so acos is real-valued.
        let cos_half = 1.0 - params.chord_tolerance / radius;
        // cos_half is in (0, 1) by the guard above, so acos is in (0, π/2).
        let theta = 2.0 * cos_half.acos();
        if theta > 0.0 {
            (span / theta).ceil() as usize
        } else {
            params.min_segments
        }
    } else {
        params.min_segments
    };

    let from_chord_length = if params.max_edge_length > 0.0 {
        // half_chord clamped to 1.0 so asin stays in [0, π/2] for
        // degenerate cases where max_edge_length ≥ 2·radius.
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

    from_sagitta
        .max(from_chord_length)
        .max(from_angle)
        .max(params.min_segments)
        .min(params.max_segments)
}

/// Number of subdivisions across a linear span of given `length` to
/// satisfy `params.max_edge_length`. Linear axes have zero curvature
/// (a cylinder's height, a cone's slant) so chord-height and
/// angle-deviation never bind — only the absolute edge-length cap matters.
/// Result is clamped to `[params.min_segments.max(1), params.max_segments]`.
fn linear_steps_for_quality(length: f64, params: &TessellationParams) -> usize {
    if length <= 0.0 {
        return params.min_segments.max(1);
    }
    let from_chord = if params.max_edge_length > 0.0 {
        ((length / params.max_edge_length).ceil() as usize).max(1)
    } else {
        1
    };
    from_chord
        .max(params.min_segments.max(1))
        .min(params.max_segments)
}

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
        "Plane" => tessellate_planar_face(face, model, params, mesh),
        "Cylinder" => tessellate_cylindrical_face(face, model, params, mesh),
        "Sphere" => tessellate_spherical_face(face, model, params, mesh),
        "Cone" => tessellate_conical_face(face, model, params, mesh),
        "Torus" => tessellate_toroidal_face(face, model, params, mesh),
        "NURBS" => tessellate_nurbs_face(face, model, params, mesh),
        "CylindricalFillet" | "ToroidalFillet" | "SphericalFillet" | "VariableRadiusFillet" => {
            tessellate_fillet_face(face, model, params, mesh)
        }
        _ => {
            // RuledSurface (extruded straight-line side faces, prismatic
            // sweeps, etc.) is geometrically planar whenever its two
            // boundary curves keep parallel tangents along the rail —
            // which is the dominant case for extrude. Routing those
            // through `tessellate_planar_face` is mandatory for
            // watertightness: the planar caps that share the same B-Rep
            // edges sample those edges via `sample_loop_3d_polygon`,
            // which emits exactly one sample per straight segment. A
            // grid sampler instead emits N+1 samples along every
            // boundary parametric direction, so the side face's
            // interior boundary samples have no twin on the cap for
            // `weld_mesh_watertight_range` to collapse — leaving the
            // seam open and visible as a crack on the rendered solid.
            // Routing planar generics through the polygon path makes
            // both faces agree at every shared edge.
            //
            // Non-planar generic surfaces (extrude/sweep of a curved
            // profile, RuledSurface with non-parallel rails, foreign
            // surface implementations) go through the curvature-adaptive
            // quadtree — the same path NURBS uses. This replaces the
            // legacy uniform UV-grid sampler, which had no curvature
            // awareness and either under-tessellated tight curvature
            // (visible faceting) or over-tessellated low curvature
            // (wasted triangles) depending on `max_edge_length`.
            let planar_tolerance =
                Tolerance::new(params.chord_tolerance, params.max_angle_deviation);
            if surface.is_planar(planar_tolerance) {
                tessellate_planar_face(face, model, params, mesh);
            } else {
                let (u_min, u_max, v_min, v_max) = get_face_parameter_bounds(face, model);
                tessellate_curved_adaptive(
                    surface, face, model, params, mesh, u_min, u_max, v_min, v_max,
                );
            }
        }
    }
}

/// Tessellate a planar face using constrained Delaunay triangulation
fn tessellate_planar_face(
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) {
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
        sample_loop_3d_polygon(outer_loop, model, params, &mut all_vertices);
        let end_idx = all_vertices.len();
        if end_idx > start_idx {
            loop_boundaries.push((start_idx, end_idx, true)); // true = outer loop
        }
    }

    // Process inner loops (holes)
    for &inner_loop_id in &face.inner_loops {
        if let Some(inner_loop) = model.loops.get(inner_loop_id) {
            let start_idx = all_vertices.len();
            sample_loop_3d_polygon(inner_loop, model, params, &mut all_vertices);
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
/// # Why sample density is chord-tolerance driven
/// The primitive tessellators (cylindrical, spherical, conical,
/// toroidal) derive their UV-grid step counts from
/// `params.max_edge_length` via the chord-length-to-arc relationship
/// `n = ceil(arc_length / max_edge_length)`. For shared edges between
/// a primitive's curved face and an adjacent planar cap (e.g. cylinder
/// bottom edge: shared by the bottom cap and the lateral face),
/// `weld_mesh_watertight_range` can only collapse the seam if BOTH
/// faces emit the same number of boundary samples at the same curve
/// parameters. Hardcoding `32` for closed edges and `16` for arcs
/// breaks that invariant the moment the chord-tolerance asks for any
/// other count. Instead we derive `n` from the same chord-tolerance
/// rule the primitive tessellators use, so the boundary always lines
/// up regardless of tolerance.
///
/// # Strategy
/// For each edge:
/// * If the curve is a straight line (cross product of mid-vs-endpoint
///   vectors below tolerance) emit a single sample at `t_start`. This
///   matches the previous one-vertex-per-edge behaviour for box faces
///   and keeps the resulting ear-clipping cheap.
/// * Otherwise sample `compute_curve_sample_count(...)` points — a
///   chord-tolerance-driven count that matches the primitive grid
///   density.
///
/// Sampling uses the loop's recorded edge orientation so the polygon
/// winds consistently — `triangulate_planar_polygon` then forces outer
/// CCW / inner CW via the shoelace test, so absolute winding here is
/// not load-bearing, but per-edge orientation must be respected to
/// keep the polygon simple.
fn sample_loop_3d_polygon(
    loop_data: &crate::primitives::r#loop::Loop,
    model: &BRepModel,
    params: &TessellationParams,
    out: &mut Vec<Point3>,
) {
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
        // open edges, a 3-point collinearity check decides whether to
        // collapse to a single sample.
        let is_closed_edge = edge.start_vertex == edge.end_vertex;
        let n = if is_closed_edge {
            compute_curve_sample_count(curve, t_start, t_end, params)
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
                        compute_curve_sample_count(curve, t_start, t_end, params)
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

/// Compute the chord-tolerance-driven sample count for a curve segment.
///
/// Estimates arc length via a 16-point polyline probe, then returns
/// `ceil(arc_length / max_edge_length)` clamped to
/// `[min_segments, max_segments]`. The sample count is identical to
/// what the cylindrical / spherical / conical / toroidal tessellators
/// derive from the same chord tolerance applied to their parametric
/// span — so boundary samples land at the same curve parameters as
/// the curved-surface grid samples, and `weld_mesh_watertight_range`
/// collapses the shared edge cleanly. This is the invariant that lets
/// a primitive cylinder render watertight: cap and lateral face
/// agree on every closed-circle boundary point.
fn compute_curve_sample_count(
    curve: &dyn crate::primitives::curve::Curve,
    t_start: f64,
    t_end: f64,
    params: &TessellationParams,
) -> usize {
    const PROBE: usize = 16;
    let mut total_length = 0.0_f64;
    let mut prev = curve.point_at(t_start).ok();
    for i in 1..=PROBE {
        let t = t_start + (i as f64) * (t_end - t_start) / (PROBE as f64);
        let cur = curve.point_at(t).ok();
        if let (Some(a), Some(b)) = (prev.as_ref(), cur.as_ref()) {
            total_length += (*b - *a).magnitude();
        }
        prev = cur;
    }
    let n = if params.max_edge_length > 0.0 {
        (total_length / params.max_edge_length).ceil() as usize
    } else {
        params.min_segments
    };
    n.max(params.min_segments.max(3)).min(params.max_segments)
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
        if angle < best_angle - 1e-14 || ((angle - best_angle).abs() <= 1e-14 && dist2 < best_dist2)
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

    // Radial subdivision is driven by curvature: `arc_steps_for_quality`
    // combines chord-height (sagitta), chord-length, and angle-deviation
    // and picks the strictest. Chord-height is the size-invariant quality
    // driver — segments grow as √radius instead of radius, so a 100 mm
    // cylinder doesn't get 10× the triangles of a 10 mm one for the same
    // visual quality. Axial subdivision uses chord-length only because a
    // cylinder has zero curvature along its axis.
    let u_steps = arc_steps_for_quality(u_span, radius, params);
    let v_steps = linear_steps_for_quality(v_span, params);

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

    // Both axes on a sphere trace circles of the same radius, so both
    // use `arc_steps_for_quality` (chord-height + chord-length + angle).
    // The sphere's principal curvature is 1/radius in both directions,
    // so this is exact — not a conservative approximation.
    let radius = estimate_sphere_radius(surface).max(crate::math::constants::EPSILON);
    let u_steps = arc_steps_for_quality(u_span, radius, params);
    let v_steps = arc_steps_for_quality(v_span, radius, params);

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

    // Radial subdivision uses the MAXIMUM cross-section radius (at the
    // wide end) because chord-height demands more steps as radius grows.
    // Picking the max is conservative — every other v-level meets the
    // tolerance with slack. For a `Cone`, r(v) = v · sin(half_angle).
    // Falls back to 1.0 if the surface is not a `Cone` (generic-grid
    // path), which keeps the angular metric as the safe lower bound.
    let u_span = u_max - u_min;
    let base_radius = surface
        .as_any()
        .downcast_ref::<crate::primitives::surface::Cone>()
        .map(|cone| (v_max.abs()).max(v_min.abs()) * cone.half_angle.sin())
        .unwrap_or(1.0);
    let u_steps = arc_steps_for_quality(u_span, base_radius, params)
        // Apex-singular cones need at least 8 radial divisions to avoid
        // a visually triangular cross-section near the tip.
        .max(params.min_segments.max(8));

    // Linear resolution along the cone's slant (zero curvature in v).
    let cone_height = estimate_cone_height(surface, v_min, v_max);
    let v_steps = linear_steps_for_quality(cone_height, params).max(3);

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

    // First row is the apex.
    //
    // `Cone::evaluate_full(u, 0)` returns `Err(DivisionByZero)` because at
    // `v = 0` the radius is zero, `du` is the zero vector, and the surface
    // normal `du.cross(&dv).normalize()` fails. Falling through to the
    // `surface.point_at` / `face.normal_at` path therefore drops the apex
    // vertex entirely — every fan triangle then evaluates `vertex_grid[0][0]`
    // as `None` and emits nothing, leaving a visible hole at the cone tip.
    //
    // Synthesize the apex directly from the `Cone` primitive: the position
    // is `cone.apex`, and the limit normal averaged over `u` is `-axis`
    // (each (u, v=ε) sample's outward normal direction is
    // `(cos u · cos α, sin u · cos α, -sin α)`; integrating over `u`
    // cancels the radial components and leaves `(0, 0, -sin α)`, which
    // unit-normalizes to `-axis`). Multiply by the face orientation sign
    // so a backward face flips the normal to match the rest of its lateral
    // ring. This function is only reached from `tessellate_conical_face`
    // when `includes_apex` is true, so the downcast is sound by
    // construction; the fallback to surface evaluation covers any future
    // dispatcher that routes a non-`Cone` apex-singular surface here.
    if v_min.abs() < 1e-6 {
        let u = (u_min + u_max) / 2.0; // Any u value at apex
        let v = v_min;

        let apex_synth = surface
            .as_any()
            .downcast_ref::<crate::primitives::surface::Cone>()
            .map(|cone| (cone.apex, -cone.axis * face.orientation.sign()));

        let apex_vertex = match apex_synth {
            Some((position, normal)) => Some((position, normal)),
            None => match (
                surface.point_at(u, v),
                face.normal_at(u, v, &model.surfaces),
            ) {
                (Ok(p), Ok(n)) => Some((p, n)),
                _ => None,
            },
        };

        if let Some((position, normal)) = apex_vertex {
            let index = mesh.add_vertex(MeshVertex {
                position,
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

    // U sweeps the major circle; the radius of the 3D circle traced by a
    // fixed-v latitude is `R + r·cos(v)`, which peaks at `R + r` (v = 0).
    // Use that worst case so the chord-height bound holds across the
    // entire (u_min..u_max, v_min..v_max) patch — at any other v, the
    // chord error is strictly less than tolerance with slack.
    //
    // V sweeps the minor circle of constant radius `r`, so the chord
    // metric on v uses `minor_radius` directly. Cap v at half
    // `max_segments` so the total triangle count for a full torus stays
    // within max_segments² rather than 2·max_segments².
    let u_radius = major_radius + minor_radius;
    let u_steps = arc_steps_for_quality(u_span, u_radius, params);
    let v_cap_params = TessellationParams {
        max_segments: params.max_segments.max(2) / 2,
        ..params.clone()
    };
    let v_steps = arc_steps_for_quality(v_span, minor_radius, &v_cap_params);

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

/// Tessellate a NURBS face with curvature-driven adaptive refinement
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

    // For NURBS surfaces, we need adaptive tessellation based on curvature.
    // The adaptive path is generic over `&dyn Surface` — it's also used
    // for any other curved generic surface (see the `_ =>` arm in
    // `tessellate_face`).
    tessellate_curved_adaptive(
        surface, face, model, params, mesh, u_min, u_max, v_min, v_max,
    );
}

/// Tessellate a fillet face (CylindricalFillet, ToroidalFillet,
/// SphericalFillet, VariableRadiusFillet).
///
/// Fillet surfaces are parameterized over a full `[0,1] × [0,1]` UV
/// domain whose four boundaries correspond exactly to the four-sided
/// blend loop produced by `create_trimmed_fillet_face`:
///
/// * `v = 0` → contact-1 curve (= trim1 in 3D, sampled by face1's
///   planar tessellator via `sample_loop_3d_polygon`)
/// * `v = 1` → contact-2 curve (= trim2 in 3D, sampled by face2)
/// * `u = 0` → cap_v0 (a Line in 3D between trim2_first and trim1_first)
/// * `u = 1` → cap_v1 (a Line in 3D between trim1_last and trim2_last)
///
/// Because the loop tightly wraps the surface's parameter domain, no
/// inside-loop filter is needed — every grid sample is on the face.
///
/// **Watertightness contract**: the U-direction sample count is
/// derived from `compute_curve_sample_count` of the longest non-line
/// loop edge (trim1 or trim2) so it matches the count the adjacent
/// planar face uses when sampling the same trim curve via
/// `sample_loop_3d_polygon`. With matching U counts and matching
/// `point_at(u, 0) == trim1.point_at(u)` / `point_at(u, 1) == trim2(u)`
/// (an invariant of `CylindricalFillet::evaluate_full` after the
/// frame-storage fix), the boundary vertices on both sides of the
/// shared edge land at the same 3D positions and
/// `weld_mesh_watertight_range` collapses the seam.
///
/// V-direction count is chord-tolerance-driven on the actual arc
/// (probed by sampling `point_at(u_mid, v)` so we don't depend on
/// a per-fillet-type radius accessor).
fn tessellate_fillet_face(
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) {
    let Some(surface) = model.surfaces.get(face.surface_id) else {
        return;
    };

    // U-direction sample count: take the maximum of compute_curve_sample_count
    // over every non-degenerate loop edge whose 3D length exceeds the
    // maximum cap-edge length. The loop has 2 trim edges (long, curved
    // along the spine) and 2 cap edges (short, straight). The trim edges
    // dominate; using the max is robust if the loop is partially
    // collapsed (3-sided degenerate case).
    let mut u_steps = params.min_segments.max(3);
    if let Some(outer_loop) = model.loops.get(face.outer_loop) {
        let mut longest_edge_len = 0.0_f64;
        let mut longest_edge_n = 0usize;
        for &edge_id in &outer_loop.edges {
            let Some(edge) = model.edges.get(edge_id) else {
                continue;
            };
            let Some(curve) = model.curves.get(edge.curve_id) else {
                continue;
            };
            let t_start = edge.param_range.start;
            let t_end = edge.param_range.end;
            // Chord-length probe: 16 sample sum, same as compute_curve_sample_count.
            let mut len = 0.0_f64;
            let mut prev = curve.point_at(t_start).ok();
            for i in 1..=16 {
                let t = t_start + (i as f64) * (t_end - t_start) / 16.0;
                let cur = curve.point_at(t).ok();
                if let (Some(a), Some(b)) = (prev.as_ref(), cur.as_ref()) {
                    len += (*b - *a).magnitude();
                }
                prev = cur;
            }
            if len > longest_edge_len {
                longest_edge_len = len;
                longest_edge_n = compute_curve_sample_count(
                    curve,
                    t_start,
                    t_end,
                    params,
                );
            }
        }
        if longest_edge_n > u_steps {
            u_steps = longest_edge_n;
        }
    }

    // V-direction sample count: chord-length probe along the arc at u=0.5,
    // which traces the fillet's cross-section through the spine midpoint.
    let v_steps = {
        let mut arc_length = 0.0_f64;
        let mut prev = surface.point_at(0.5, 0.0).ok();
        const PROBE: usize = 16;
        for i in 1..=PROBE {
            let v = (i as f64) / (PROBE as f64);
            let cur = surface.point_at(0.5, v).ok();
            if let (Some(a), Some(b)) = (prev.as_ref(), cur.as_ref()) {
                arc_length += (*b - *a).magnitude();
            }
            prev = cur;
        }
        let n = if params.max_edge_length > 0.0 && arc_length > 0.0 {
            (arc_length / params.max_edge_length).ceil() as usize
        } else {
            params.min_segments
        };
        n.max(params.min_segments.max(3))
            .min(params.max_segments)
    };

    // Generate the full UV grid. No inside-loop filter is needed — the
    // parameter domain is exactly the face's interior plus boundary.
    let mut vertex_grid: Vec<Vec<Option<u32>>> = Vec::with_capacity(v_steps + 1);
    for v_idx in 0..=v_steps {
        let v = (v_idx as f64) / (v_steps as f64);
        let mut row = Vec::with_capacity(u_steps + 1);
        for u_idx in 0..=u_steps {
            let u = (u_idx as f64) / (u_steps as f64);
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

    // Triangulate the grid. Winding follows `face.orientation` so
    // emitted geometric normals match the stored vertex normals
    // (which `face.normal_at` already flips for backward faces).
    let forward = face.orientation.is_forward();
    for v_idx in 0..v_steps {
        for u_idx in 0..u_steps {
            let v0 = vertex_grid[v_idx][u_idx];
            let v1 = vertex_grid[v_idx][u_idx + 1];
            let v2 = vertex_grid[v_idx + 1][u_idx];
            let v3 = vertex_grid[v_idx + 1][u_idx + 1];
            if let (Some(v0), Some(v1), Some(v2), Some(v3)) = (v0, v1, v2, v3) {
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

    // Full-period collapse: when the unwrapped loop spans the surface's
    // full u- or v-period, snap to the canonical surface bounds instead
    // of clamping the lifted polygon's `[u_min, u_max]` against
    // `[u_range.0, u_range.1]`. The clamp loses the **angular offset**
    // between the boundary curve's local x-axis and the surface's
    // `ref_dir`. Concrete failure (cone): the wide-end `Circle` is
    // built from `Circle::new(center, axis = +Z, …)` whose canonical
    // x-axis for `+Z` is `+X`, while `Cone::ref_dir` is computed via
    // `axis.perpendicular()` which for `+Z` returns `-Y`. The two
    // frames are 90° apart, so `closest_point` lifts the circle into
    // u-space as `[π/2, 5π/2]` — a full 2π span, but offset. Clamping
    // that to `[0, 2π]` truncates to `[π/2, 2π]` and the grid
    // tessellator sees only 270° = **75% of the lateral surface**.
    // The torus (full + partial-V) and any other periodic surface
    // where the boundary edge frame disagrees with `ref_dir` exhibit
    // the same shear; snapping to surface bounds whenever the lifted
    // span covers the full period is the only correct response.
    const PERIOD_TOL: f64 = 1e-6;
    if let Some(period) = surface.period_u() {
        if (u_max - u_min) >= period - PERIOD_TOL {
            u_min = u_range.0;
            u_max = u_range.1;
        }
    }
    if let Some(period) = surface.period_v() {
        if (v_max - v_min) >= period - PERIOD_TOL {
            v_min = v_range.0;
            v_max = v_range.1;
        }
    }

    // Use the loop's UV bounds directly, clamped to the surface's own
    // parameter domain. A previous `±1% margin` expansion was meant to
    // give "numerical stability" room but instead pushed the outermost
    // grid samples (`u_idx = 0` and `u_idx = u_steps`) **strictly
    // outside** the loop polygon, where `inside_face` then rejected
    // them. The result was a ~9 % un-tessellated strip around every
    // face boundary — visible as the "cracks on side faces" symptom
    // for any RuledSurface / NURBS face routed through the generic
    // grid tessellator (the planar fast-path uses ear-clipping and
    // is unaffected). Sample exactly at the loop bounds; the
    // `inside_face` boundary-tolerance branch handles atan2 noise at
    // axis-aligned polygon corners.
    (
        u_min.max(u_range.0),
        u_max.min(u_range.1),
        v_min.max(v_range.0),
        v_max.min(v_range.1),
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
/// Adaptive curvature-driven tessellation for any curved surface
/// (NURBS, RuledSurface with non-linear profile, generic
/// non-planar `&dyn Surface` implementations).
///
/// Initialises a UV quadtree over the face's parametric bounds and
/// recursively subdivides whenever the chord-height, normal-deviation,
/// or edge-length guards are violated (see `should_subdivide_curved`).
/// Leaf quads are stamped into the mesh; samples outside the face's
/// trim loops are skipped via `is_point_inside_face`.
///
/// **Watertightness**: relies on `weld_mesh_watertight_range` at the
/// shell level to collapse coincident vertices. Adjacent faces sharing
/// a B-Rep edge will produce 3D-coincident corner samples whenever the
/// edge endpoint parameters align, which holds for the common case of
/// untrimmed parametric boundaries. T-junctions between adjacent leaves
/// at different subdivision levels are not currently healed; the welder
/// tolerates them within `weld_tolerance` but they remain a known
/// limitation of the adaptive path.
fn tessellate_curved_adaptive(
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
    subdivide_curved_quad(
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
fn subdivide_curved_quad(
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
        || !should_subdivide_curved(surface, face, model, params, u_min, u_max, v_min, v_max)
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

    subdivide_curved_quad(
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
    subdivide_curved_quad(
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
    subdivide_curved_quad(
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
    subdivide_curved_quad(
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
fn should_subdivide_curved(
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

    let mut spatial_hash: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::with_capacity(n - v_start);
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
        let (verts, loops) =
            build_planar_loops(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)], &[]);
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
        let (verts, loops) =
            build_planar_loops(&[(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0)], &[]);
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

    // === T-1: arc_steps_for_quality / linear_steps_for_quality tests ===

    /// Default params at radius 1 with full 2π sweep: sagitta=0.001 wins
    /// over chord-length=0.1 (sagitta gives ≈71 steps, chord-length ≈63,
    /// angle ≈63), so we expect at least 70 steps and within max_segments.
    #[test]
    fn arc_steps_default_unit_radius_full_sweep() {
        let params = TessellationParams::default();
        let n = arc_steps_for_quality(2.0 * std::f64::consts::PI, 1.0, &params);
        assert!(n >= 70, "expected ≥70 steps at default quality, got {n}");
        assert!(
            n <= params.max_segments,
            "expected ≤max_segments, got {n}"
        );
    }

    /// Chord-height is the primary driver: tightening `chord_tolerance`
    /// must monotonically increase the step count (until max_segments cap).
    #[test]
    fn arc_steps_monotonic_in_chord_tolerance() {
        let mk = |tol: f64| TessellationParams {
            chord_tolerance: tol,
            max_edge_length: 0.0,   // disable chord-length cap
            max_angle_deviation: 0.0, // disable angle cap
            min_segments: 3,
            max_segments: 10_000, // raise cap so monotonicity is observable
        };
        let span = 2.0 * std::f64::consts::PI;
        let n_coarse = arc_steps_for_quality(span, 1.0, &mk(0.1));
        let n_medium = arc_steps_for_quality(span, 1.0, &mk(0.01));
        let n_fine = arc_steps_for_quality(span, 1.0, &mk(0.001));
        let n_ultra = arc_steps_for_quality(span, 1.0, &mk(0.0001));
        assert!(
            n_coarse < n_medium && n_medium < n_fine && n_fine < n_ultra,
            "expected strict monotonic step growth, got {n_coarse}, {n_medium}, {n_fine}, {n_ultra}"
        );
    }

    /// Size-invariance test: a 100× larger radius needs only √100 = 10×
    /// more segments for the same chord tolerance (not 100× as
    /// chord-length sampling would give). Verifies n ∝ √r scaling.
    #[test]
    fn arc_steps_chord_height_scales_with_sqrt_radius() {
        let params = TessellationParams {
            chord_tolerance: 0.001,
            max_edge_length: 0.0,
            max_angle_deviation: 0.0,
            min_segments: 3,
            max_segments: 100_000,
        };
        let span = 2.0 * std::f64::consts::PI;
        let n_small = arc_steps_for_quality(span, 1.0, &params) as f64;
        let n_big = arc_steps_for_quality(span, 100.0, &params) as f64;
        let ratio = n_big / n_small;
        // Expected ratio ≈ √100 = 10. Allow ±15% slack for ceil rounding.
        assert!(
            ratio > 8.5 && ratio < 11.5,
            "expected ≈10× growth (√r law), got ratio {ratio} (n_small={n_small}, n_big={n_big})"
        );
    }

    /// Chord-length cap dominates when set tighter than chord-height.
    /// At max_edge_length=0.01 on r=1 full sweep: θ ≈ 0.01 rad → ~628 steps.
    /// Chord-height of 0.1 gives only ~7 steps. The strictest (628) must win.
    #[test]
    fn arc_steps_strictest_constraint_wins() {
        let params = TessellationParams {
            chord_tolerance: 0.1,  // loose
            max_edge_length: 0.01, // tight
            max_angle_deviation: 0.0,
            min_segments: 3,
            max_segments: 10_000,
        };
        let n = arc_steps_for_quality(2.0 * std::f64::consts::PI, 1.0, &params);
        assert!(n >= 620, "chord-length cap should dominate, got {n}");
    }

    /// Result is clamped to [min_segments, max_segments].
    #[test]
    fn arc_steps_respects_segment_clamps() {
        let params = TessellationParams {
            chord_tolerance: 1e-6,    // would request enormous step count
            max_edge_length: 1e-6,
            max_angle_deviation: 1e-6,
            min_segments: 3,
            max_segments: 50,
        };
        let n = arc_steps_for_quality(2.0 * std::f64::consts::PI, 1.0, &params);
        assert_eq!(n, 50, "result must clamp to max_segments");

        let params_min = TessellationParams {
            chord_tolerance: 100.0, // way larger than radius → fallback
            max_edge_length: 100.0,
            max_angle_deviation: 100.0,
            min_segments: 12,
            max_segments: 200,
        };
        // span small enough that all metrics request 1 step → floor at min
        let n_min = arc_steps_for_quality(0.01, 1.0, &params_min);
        assert_eq!(n_min, 12, "result must floor at min_segments");
    }

    /// Degenerate inputs return min_segments without panicking.
    #[test]
    fn arc_steps_degenerate_inputs() {
        let params = TessellationParams::default();
        assert_eq!(arc_steps_for_quality(0.0, 1.0, &params), params.min_segments);
        assert_eq!(arc_steps_for_quality(-1.0, 1.0, &params), params.min_segments);
        assert_eq!(arc_steps_for_quality(1.0, 0.0, &params), params.min_segments);
        assert_eq!(arc_steps_for_quality(1.0, -1.0, &params), params.min_segments);
    }

    /// linear_steps: zero-curvature axis only uses chord-length.
    #[test]
    fn linear_steps_basic_chord_length() {
        let params = TessellationParams {
            chord_tolerance: 0.001,    // ignored on linear axis
            max_edge_length: 0.1,
            max_angle_deviation: 0.01, // ignored on linear axis
            min_segments: 1,
            max_segments: 100,
        };
        // length 1.0 / chord 0.1 → 10 segments
        assert_eq!(linear_steps_for_quality(1.0, &params), 10);
        // length 0.5 / chord 0.1 → 5 segments
        assert_eq!(linear_steps_for_quality(0.5, &params), 5);
    }

    /// linear_steps clamps to [min, max] and handles degenerate inputs.
    #[test]
    fn linear_steps_clamps() {
        let params = TessellationParams {
            chord_tolerance: 0.0,
            max_edge_length: 0.001, // tight
            max_angle_deviation: 0.0,
            min_segments: 1,
            max_segments: 50,
        };
        assert_eq!(linear_steps_for_quality(10.0, &params), 50);
        assert_eq!(linear_steps_for_quality(0.0, &params), 1);
    }

    /// End-to-end integration test: tightening `chord_tolerance` on a
    /// cylinder must produce strictly more triangles than a looser one
    /// (with all other quality knobs disabled). This verifies that the
    /// chord-height path is actually wired into `tessellate_cylindrical_face`,
    /// not just available as a helper.
    #[test]
    fn cylinder_tessellation_density_grows_with_chord_tolerance() {
        use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
        use crate::tessellation::tessellate_solid;

        fn tri_count(chord_tol: f64) -> usize {
            let mut model = BRepModel::new();
            let solid_id = {
                let mut b = TopologyBuilder::new(&mut model);
                match b
                    .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 1.0, 2.0)
                    .expect("create_cylinder_3d")
                {
                    GeometryId::Solid(id) => id,
                    other => panic!("expected Solid, got {other:?}"),
                }
            };
            let solid = model.solids.get(solid_id).expect("solid").clone();
            let params = TessellationParams {
                chord_tolerance: chord_tol,
                // Disable the other quality knobs so chord-height is the
                // sole driver of step count for this assertion.
                max_edge_length: 0.0,
                max_angle_deviation: 0.0,
                min_segments: 3,
                max_segments: 10_000,
            };
            tessellate_solid(&solid, &model, &params).triangles.len()
        }

        let coarse = tri_count(0.1);
        let medium = tri_count(0.01);
        let fine = tri_count(0.001);
        assert!(
            coarse < medium && medium < fine,
            "tightening chord_tolerance must strictly increase tri count, got \
             coarse={coarse}, medium={medium}, fine={fine}"
        );
    }

    /// Sphere tessellation density also grows with tightening tolerance —
    /// proves T-1's primary curvature path is wired for spheres too.
    #[test]
    fn sphere_tessellation_density_grows_with_chord_tolerance() {
        use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
        use crate::tessellation::tessellate_solid;

        fn tri_count(chord_tol: f64) -> usize {
            let mut model = BRepModel::new();
            let solid_id = {
                let mut b = TopologyBuilder::new(&mut model);
                match b
                    .create_sphere_3d(Point3::new(0.0, 0.0, 0.0), 1.0)
                    .expect("create_sphere_3d")
                {
                    GeometryId::Solid(id) => id,
                    other => panic!("expected Solid, got {other:?}"),
                }
            };
            let solid = model.solids.get(solid_id).expect("solid").clone();
            let params = TessellationParams {
                chord_tolerance: chord_tol,
                max_edge_length: 0.0,
                max_angle_deviation: 0.0,
                min_segments: 3,
                max_segments: 10_000,
            };
            tessellate_solid(&solid, &model, &params).triangles.len()
        }

        let coarse = tri_count(0.1);
        let fine = tri_count(0.001);
        assert!(
            coarse < fine,
            "tightening chord_tolerance must increase sphere tri count, \
             got coarse={coarse}, fine={fine}"
        );
    }
}
