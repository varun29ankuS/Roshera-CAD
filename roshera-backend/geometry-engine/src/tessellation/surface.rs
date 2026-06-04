//! Surface tessellation algorithms
//!
//! Indexed access into UV-grid sample arrays and triangle-strip vertex
//! indices is the canonical idiom for parametric tessellation — all `arr[i]`
//! and `grid[u][v]` sites are bounds-guaranteed by the (samples_u × samples_v)
//! grid dimensions established at the top of each tessellator. Matches the
//! numerical-kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::adaptive::compute_plane_axes;
use super::edge_cache::{compute_curve_sample_count, EdgeSampleCache};
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
    cache: &EdgeSampleCache,
    mesh: &mut TriangleMesh,
) {
    // Get surface
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return,
    };

    match surface.type_name() {
        "Plane" => tessellate_planar_face(face, model, params, cache, mesh),
        // CDT-γ.3: route the cylinder lateral face through the constraint-
        // aware curved-CDT path (the same one NURBS uses). The grid
        // tessellator sampled its boundary independently of the
        // EdgeSampleCache, so the lateral and the planar caps disagreed on
        // the shared circular seam — leaving T-junctions the vertex-weld
        // cannot repair (a closed cylinder came out non-watertight). The CDT
        // path consumes the cache for boundary 3D, so lateral and caps share
        // the seam samples bit-exactly. (Requires the seam to coincide with
        // the circles' t=0 — see create_cylinder_topology.) Empty-mesh-on-Err
        // contract, as for NURBS / generic curved faces.
        "Cylinder" => {
            if let Err(e) =
                super::curved_cdt::tessellate_curved_cdt(surface, face, model, params, cache, mesh)
            {
                // curved-CDT can fail on a transformed (e.g. rotated) cylinder:
                // once the lateral seam no longer coincides with the cap
                // circles' t=0, a projected boundary sample can land exactly on
                // a constraint edge (`CdtFailed(PointOnFixedEdge)`). Emitting an
                // empty mesh there silently drops the whole lateral wall,
                // leaving a non-watertight caps-only shell whose divergence-
                // theorem volume collapses to ~1/3 of the truth (cone value) —
                // a silent mass-property/export corruption for any non-axis-
                // aligned cylinder. Fall back to the analytic grid, exactly as
                // the cone-frustum path does, so the lateral is present and
                // mass properties stay correct (at the cost of possible seam
                // T-junctions on export, far less harmful than a 3× volume
                // error).
                tracing::warn!(
                    "curved_cdt failed for cylinder face {:?}: {:?}; falling back to grid",
                    face.id,
                    e
                );
                // Grid over the cylinder's INTRINSIC parameter domain (full
                // angular sweep + its own height limits) rather than
                // `get_face_parameter_bounds`: once the seam is desynced the
                // face-edge-derived u-range collapses, which would grid an
                // empty wall. `evaluate_full(u, v)` traces the correct lateral
                // from the surface's stored (rotated) frame regardless of seam.
                let cyl = surface
                    .as_any()
                    .downcast_ref::<crate::primitives::surface::Cylinder>();
                let (radius, (u_min, u_max), (v_min, v_max)) = match cyl {
                    Some(c) => {
                        let (u0, u1) = c
                            .angle_limits
                            .map_or((0.0, std::f64::consts::TAU), |[a, b]| (a, b));
                        // height_limits are the v-domain; fall back to the
                        // face-derived v-range only if the surface is infinite.
                        let (v0, v1) = c.height_limits.map_or_else(
                            || {
                                let (_, _, vlo, vhi) = get_face_parameter_bounds(face, model);
                                (vlo, vhi)
                            },
                            |[a, b]| (a, b),
                        );
                        (c.radius, (u0, u1), (v0, v1))
                    }
                    None => {
                        let (u0, u1, v0, v1) = get_face_parameter_bounds(face, model);
                        (1.0, (u0, u1), (v0, v1))
                    }
                };
                let u_steps =
                    arc_steps_for_quality(u_max - u_min, radius, params).max(params.min_segments);
                let v_steps = linear_steps_for_quality((v_max - v_min).abs(), params).max(3);
                tessellate_surface_grid_untrimmed(
                    face, model, surface, u_min, u_max, v_min, v_max, u_steps, v_steps, mesh,
                );
            }
        }
        "Sphere" => tessellate_spherical_face(face, model, params, cache, mesh),
        "Cone" => tessellate_conical_face(face, model, params, cache, mesh),
        "Torus" => tessellate_toroidal_face(face, model, params, cache, mesh),
        "NURBS" => tessellate_nurbs_face(face, model, params, cache, mesh),
        // Revolve emits each curved wall as thin per-segment SurfaceOfRevolution
        // wedge faces; the generic curved-CDT path fails on these high-aspect
        // slivers at fine chord (REVOLVE-ROBUSTNESS #47). A wedge is a tensor-
        // product quad, so tessellate it as a structured grid with cache-sampled
        // boundaries (watertight, no cdt). Fall back to curved-CDT for the rare
        // non-rectangular wedge (apex triangle / unequal-radius slanted patch).
        "SurfaceOfRevolution" => {
            // Try the structured wedge grid first: it only succeeds for a clean
            // rectangular wedge (equal-count opposite boundaries), which is
            // exactly the curved constant-radius wall the generic curved-CDT
            // path chokes on at fine chord. A thin curved wedge can read as
            // "nearly planar", so a planarity test FIRST would wrongly route it
            // to the planar cdt path (same failure) — hence grid-first.
            //
            // Anything the grid declines (a flat radial rim sector, an apex
            // triangle, a slanted unequal-radius wedge) falls back to the
            // generic planar-or-curved-CDT routing the `_` arm uses.
            if !tessellate_revolution_wedge(face, model, cache, mesh) {
                let planar_tol = Tolerance::new(params.chord_tolerance, params.max_angle_deviation);
                if surface.is_planar(planar_tol)
                    || is_face_loop_planar_in_3d(face, model, cache, planar_tol)
                {
                    tessellate_planar_face(face, model, params, cache, mesh);
                } else if let Err(e) = super::curved_cdt::tessellate_curved_cdt(
                    surface, face, model, params, cache, mesh,
                ) {
                    tracing::warn!(
                        "revolution wedge: grid declined and curved_cdt failed for face {:?}: {:?}",
                        face.id,
                        e
                    );
                }
            }
        }
        "CylindricalFillet" | "ToroidalFillet" | "SphericalFillet" | "VariableRadiusFillet" => {
            tessellate_fillet_face(face, model, params, cache, mesh)
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
            // Two-stage planarity classification:
            //
            // 1. **Surface-global** (`surface.is_planar`): tests the
            //    FULL parameter bounds. Fast path that catches
            //    genuinely-flat ruled surfaces (extruded straight
            //    segments, prismatic sweeps with parallel rails).
            //
            // 2. **Face-restricted** (`is_face_uv_range_planar`):
            //    samples normals over the face's actual UV window
            //    `[u_min, u_max] × [v_min, v_max]` and accepts when
            //    every normal agrees with the centre's within the
            //    tolerance angle.
            //
            // The second stage exists because a single B-Rep face can
            // cover a flat patch of an otherwise non-flat surface. The
            // canonical case is the polyline-extruded side wall: the
            // host `RuledSurface(Polyline, Polyline)` carries N kinks
            // in its u-direction (one per polyline vertex), so
            // `surface.is_planar` over u ∈ [0, 1] correctly returns
            // false. But each side FACE is bounded to a single
            // polyline segment (`u ∈ [i/N, (i+1)/N]`), within which
            // the rails are colinear and the ruled patch is exactly
            // planar. Historical: before CDT-β.2 the curved-surface
            // fallback was the legacy quadtree
            // (`tessellate_curved_adaptive`, MAX_DEPTH = 12, up to
            // 4^12 ≈ 16M recursive calls per face), which produced
            // an apparent 15+ s "hang" on a polyline-extruded
            // hex/L-shape. The face-restricted planar check still
            // matters post-β.2 because `tessellate_planar_face`
            // ear-clips a 4-vertex polygon in microseconds, skipping
            // the full CDT pipeline for trivially planar side walls.
            if surface.is_planar(planar_tolerance)
                || is_face_loop_planar_in_3d(face, model, cache, planar_tolerance)
            {
                tessellate_planar_face(face, model, params, cache, mesh);
            } else {
                // CDT-β.2: the legacy `tessellate_curved_adaptive`
                // quadtree fallback has been retired. The empirical
                // signal that motivated retirement: zero
                // `[tess] curved_cdt fallback` lines on the full
                // workspace test corpus (geometry-engine lib + all
                // integration suites + export-engine + api-server)
                // under `ROSHERA_TESS_TRACE=1`. On `Err(_)` the
                // contract is "this face emits zero triangles"; the
                // shell-level `tessellate_shell` continues with the
                // rest of the shell.
                match super::curved_cdt::tessellate_curved_cdt(
                    surface, face, model, params, cache, mesh,
                ) {
                    Ok(()) => return,
                    Err(e) => {
                        tracing::warn!(
                            "curved_cdt failed for face {:?}: {:?}; emitting empty \
                             mesh (CDT-β.2: legacy quadtree fallback retired)",
                            face.id,
                            e
                        );
                    }
                }
            }
        }
    }
}

/// 3D-loop planarity test: fit a plane to the face's boundary samples
/// and accept when every sample lies within `tolerance.distance()` of
/// it.
///
/// This bypasses surface parameter space entirely, which is critical
/// for the canonical case this helper exists to catch — a polyline-
/// extruded `RuledSurface(Polyline, Polyline)` whose host curve is C0
/// at vertices between segments. Two failure modes in UV space:
///
/// 1. **Wraparound corner**: at the polyline's closing vertex
///    (`u = 0 ≡ u = 1` in 3D), `closest_point` may project to either
///    end of the parameter range nondeterministically. The face's
///    `get_face_parameter_bounds` then reports a `u` range that spans
///    almost the whole polyline (e.g. `[0, 0.97]`) instead of the
///    single segment the face actually occupies (e.g. `[0.75, 1.0]`).
///    Any UV-space planarity probe over the bogus range hits every
///    polyline kink and rejects the face as non-planar.
///
/// 2. **C0 derivative discontinuity at kink**: `RuledSurface::evaluate
///    _full` uses central-difference for du; at a polyline vertex the
///    derivative averages the incoming and outgoing segment tangents,
///    producing a hybrid `normal_at` that disagrees with both adjacent
///    segments' true normals by up to 45°. UV probes sitting on the
///    boundary trip on this.
///
/// The 3D-loop probe sidesteps both. It uses `cache.get_or_compute`
/// (the same canonical edge sample cache that `tessellate_planar_face`
/// consumes downstream), Newell's best-fit plane (the same primitive
/// `create_planar_surface_from_edges` uses for actual face construction
/// in `extrude.rs`), and a max-deviation check.
///
/// Cost is dominated by the cache fetches, which are already amortised
/// across every face bounding each edge — at worst one curve evaluation
/// per edge per tessellation pass, regardless of how many faces probe
/// it.
fn is_face_loop_planar_in_3d(
    face: &Face,
    model: &BRepModel,
    cache: &EdgeSampleCache,
    tolerance: Tolerance,
) -> bool {
    // Sample the outer loop only. Inner loops (holes) cannot make a
    // face non-planar — if the outer ring fits a plane, every hole
    // sits inside it on the same plane by topological construction
    // (a face has exactly one supporting surface).
    let outer = match model.loops.get(face.outer_loop) {
        Some(l) => l,
        None => return false,
    };
    let mut samples = Vec::new();
    sample_loop_3d_polygon(outer, model, cache, &mut samples);
    if samples.len() < 3 {
        // Degenerate loop — let the curved path handle it (or fail
        // downstream uniformly). Not our call to make here.
        return false;
    }

    let n = match newell_normal(&samples) {
        Some(v) => v,
        None => return false,
    };
    let p0 = samples[0];
    let d_plane = n.dot(&Vector3::new(p0.x, p0.y, p0.z));

    let dist_tol = tolerance.distance();
    for p in &samples {
        let signed = n.dot(&Vector3::new(p.x, p.y, p.z)) - d_plane;
        if signed.abs() > dist_tol {
            return false;
        }
    }
    true
}

/// Newell's best-fit-plane normal for a 3D polygon.
///
/// Sums signed-area projections onto the three coordinate planes
/// (Sutherland-Hodgman-style decomposition; see Goldman, "Area of
/// Planar Polygons and Volume of Polyhedra", Graphics Gems II 1991).
/// The result is robust for non-convex polygons, oblique projections,
/// and slightly non-planar samples — the dominant fitting plane
/// "wins" because each face contributes signed area proportional to
/// its projection onto that coordinate plane.
///
/// Returns `None` iff the polygon is degenerate (all samples
/// collinear or coincident). This is the same fit
/// `create_planar_surface_from_edges` uses in `operations/extrude.rs`,
/// so the planar tessellator's projection normal agrees with the
/// construction-time plane normal by construction.
pub(crate) fn newell_normal(samples: &[Point3]) -> Option<Vector3> {
    let n = samples.len();
    if n < 3 {
        return None;
    }
    let mut nx = 0.0_f64;
    let mut ny = 0.0_f64;
    let mut nz = 0.0_f64;
    for i in 0..n {
        let a = samples[i];
        let b = samples[(i + 1) % n];
        nx += (a.y - b.y) * (a.z + b.z);
        ny += (a.z - b.z) * (a.x + b.x);
        nz += (a.x - b.x) * (a.y + b.y);
    }
    let mag = (nx * nx + ny * ny + nz * nz).sqrt();
    if mag < 1e-12 {
        None
    } else {
        let inv = 1.0 / mag;
        Some(Vector3::new(nx * inv, ny * inv, nz * inv))
    }
}

/// Tessellate a planar face using constrained Delaunay triangulation
fn tessellate_planar_face(
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    cache: &EdgeSampleCache,
    mesh: &mut TriangleMesh,
) {
    // Get surface
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return,
    };

    // Collect all vertices from outer loop and holes
    let mut all_vertices = Vec::new();
    let mut loop_boundaries = Vec::new();

    // Process outer loop
    let outer_start;
    let outer_end;
    if let Some(outer_loop) = model.loops.get(face.outer_loop) {
        let start_idx = all_vertices.len();
        sample_loop_3d_polygon(outer_loop, model, cache, &mut all_vertices);
        let end_idx = all_vertices.len();
        outer_start = start_idx;
        outer_end = end_idx;
        if end_idx > start_idx {
            loop_boundaries.push((start_idx, end_idx, true)); // true = outer loop
        }
    } else {
        outer_start = 0;
        outer_end = 0;
    }

    // Process inner loops (holes)
    for &inner_loop_id in &face.inner_loops {
        if let Some(inner_loop) = model.loops.get(inner_loop_id) {
            let start_idx = all_vertices.len();
            sample_loop_3d_polygon(inner_loop, model, cache, &mut all_vertices);
            let end_idx = all_vertices.len();
            if end_idx > start_idx {
                loop_boundaries.push((start_idx, end_idx, false)); // false = inner loop (hole)
            }
        }
    }

    if all_vertices.len() < 3 {
        return;
    }

    // Compute the projection / outward normal.
    //
    // Strategy: prefer Newell's best-fit normal of the **outer loop's
    // actual 3D samples** over the surface's analytical normal at a
    // parameter midpoint. The surface-midpoint normal is wrong for
    // any face whose UV bounds come back skewed from
    // `get_face_parameter_bounds` — most notably the polyline-extruded
    // side wall whose `RuledSurface(Polyline, Polyline)` covers a
    // single segment but whose `closest_point` may project the
    // wraparound vertex onto the opposite end of the parameter range,
    // sending `u_mid` into a different segment of the polyline. The
    // resulting `surface.normal_at(u_mid, v_mid)` is rotated by
    // up-to-360°/N relative to the actual face, and the downstream
    // ear-clip projection collapses the loop polygon to a degenerate
    // sliver — emitting zero triangles. Newell's normal is a property
    // of the loop's 3D vertices alone, independent of surface
    // parameterisation, so it is correct by construction whenever the
    // loop genuinely is planar (which the dispatch already verified
    // for us via `is_face_loop_planar_in_3d`).
    //
    // B-Rep convention: the outer loop is CCW viewed from the
    // surface's positive side. Newell's normal on a CCW polygon
    // points "out of the page" — i.e. along the surface positive
    // direction. The face's outward-pointing normal is then
    // `surface_normal × face.orientation.sign()`, exactly the pattern
    // `Face::normal_at` implements analytically. We apply the same
    // sign multiplier here.
    //
    // Fallback: if Newell degenerates (zero-magnitude on a collinear
    // sample sequence), reach for `face.normal_at` at the surface
    // midpoint — at that point the loop is degenerate so any normal
    // is acceptable; the ear-clipper will reject the polygon anyway.
    let newell_n = newell_normal(&all_vertices[outer_start..outer_end]);
    let normal = if let Some(n) = newell_n {
        n * face.orientation.sign()
    } else {
        let (u_range, v_range) = surface.parameter_bounds();
        let u_mid = (u_range.0 + u_range.1) / 2.0;
        let v_mid = (v_range.0 + v_range.1) / 2.0;
        face.normal_at(u_mid, v_mid, &model.surfaces)
            .unwrap_or(Vector3::Z)
    };

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
    cache: &EdgeSampleCache,
    out: &mut Vec<Point3>,
) {
    // Each edge contributes samples drawn from the canonical
    // `EdgeSampleCache`. The cache returns `n + 1` points in the
    // forward curve-param direction (both endpoints inclusive). To
    // form a closed polygon we emit `n` of those per edge: the
    // omitted point is supplied by the next edge's first sample,
    // which by construction shares the vertex.
    for (i, &edge_id) in loop_data.edges.iter().enumerate() {
        let forward = loop_data.orientations.get(i).copied().unwrap_or(true);
        let samples = cache.get_or_compute(edge_id, model);
        let n = samples.len();
        if n < 2 {
            // Degenerate or unfetchable curve: cache returns 0 or 1
            // sample. Skip — the loop continues with the next edge.
            continue;
        }

        if forward {
            // Emit samples[0 ..= n-2]; samples[n-1] (end vertex) is
            // supplied by the next edge's start sample.
            out.extend_from_slice(&samples[..n - 1]);
        } else {
            // Walk the canonical sample sequence backwards. Emit
            // samples[n-1 ..= 1]; samples[0] (canonical start, which
            // is the reversed-edge's end) is supplied by the next
            // edge's first sample.
            for j in (1..n).rev() {
                out.push(samples[j]);
            }
        }
    }
}

/// Triangulate a planar face's outer + (optional) inner loops in the
/// face's tangent plane.
///
/// Algorithm: Constrained Delaunay Triangulation (CDT) via the
/// [`cdt`](https://crates.io/crates/cdt) crate. Pure-Rust implementation
/// using Shewchuk-style robust adaptive geometric predicates for
/// orient2d / in_circle, so the multi-hole degeneracies that defeat
/// ear-clipping (collinear hole vertices, axis-stacked rectangular
/// holes producing N×collinear bridge targets) are handled by exact
/// arithmetic rather than tolerance-tuning.
///
/// Steps:
///
///   1. Project all vertices to 2D using `compute_plane_axes(normal)`.
///      `compute_plane_axes` builds (u, v) such that `u × v = normal`,
///      so CCW-in-2D corresponds to the +normal outward direction.
///   2. Build closed contours (last index repeats first) — one for
///      the outer loop, one per hole. `cdt::triangulate_contours`
///      flood-fills from the convex hull and erases triangles outside
///      the constraint boundaries, so winding direction does not
///      matter: only the edge set defines what's "inside".
///   3. Run `cdt::triangulate_contours`; map the returned
///      `(usize, usize, usize)` triples to `[usize; 3]`.
///
/// Triangles are emitted CCW in the 2D tangent-plane basis (standard
/// Delaunay convention). Since `u × v = normal`, CCW-in-2D = positive
/// surface normal, satisfying the caller's winding contract in
/// `tessellate_planar_face` without an explicit flip.
///
/// Replaces a prior bridged ear-clipping path (Eberly 2008) that
/// repeatedly broke on N≥4 axis-stacked holes — each new hole introduced
/// new collinearities that defeated the strict/closed point-in-triangle
/// test in a new way. CDT is set-based, not walk-based, so the same
/// failure mode is mathematically impossible.
pub(crate) fn triangulate_planar_polygon(
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

    // Project all 3D vertices to 2D in the face's tangent plane.
    let (u_axis, v_axis) = compute_plane_axes(normal);
    let origin = vertices[outer_range.0];
    let pts2d: Vec<(f64, f64)> = vertices
        .iter()
        .map(|p| {
            let r = *p - origin;
            (r.dot(&u_axis), r.dot(&v_axis))
        })
        .collect();

    // Build closed contours. cdt requires each contour's last index
    // to equal its first.
    let outer_contour: Vec<usize> = (outer_range.0..outer_range.1)
        .chain(std::iter::once(outer_range.0))
        .collect();
    let mut contours: Vec<Vec<usize>> = Vec::with_capacity(1 + inner_ranges.len());
    contours.push(outer_contour);
    for &(s, e) in &inner_ranges {
        let hole_contour: Vec<usize> = (s..e).chain(std::iter::once(s)).collect();
        contours.push(hole_contour);
    }

    // The `cdt` crate `assert!`s on some degenerate inputs (e.g. a contour
    // vertex coincident with another so its deduplicated point index is empty)
    // rather than returning `Err`. Catch the unwind so a third-party panic
    // degrades to a recoverable per-face "emit no triangles" instead of
    // aborting the entire tessellation pass — the same contract the curved-CDT
    // path enforces in `curved_cdt::run_cdt`.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        cdt::triangulate_contours(&pts2d, &contours)
    }));
    let trace = |what: &str| {
        if std::env::var("ROSHERA_TESS_TRACE").is_ok() {
            eprintln!(
                "[tess] cdt {}: outer=[{},{}) holes={}",
                what,
                outer_range.0,
                outer_range.1,
                inner_ranges.len()
            );
            for (i, c) in contours.iter().enumerate() {
                eprintln!("  contour[{}] len={} indices={:?}", i, c.len(), c);
            }
            for (i, p) in pts2d.iter().enumerate() {
                eprintln!("  pt[{}] = ({:.6}, {:.6})", i, p.0, p.1);
            }
        }
    };
    match outcome {
        Ok(Ok(tris)) => tris.into_iter().map(|(a, b, c)| [a, b, c]).collect(),
        Ok(Err(e)) => {
            trace(&format!("failed: {e:?}"));
            tracing::warn!(
                "triangulate_planar_polygon: cdt failed ({:?}); emitting no triangles",
                e
            );
            Vec::new()
        }
        Err(_) => {
            trace("panicked");
            tracing::warn!("triangulate_planar_polygon: cdt panicked; emitting no triangles");
            Vec::new()
        }
    }
}

/// Signed area of a polygon described by indices into `vertices_2d`.
/// Positive ⇒ CCW, negative ⇒ CW. Uses the shoelace formula. Kept as a
/// test-only utility after the CDT migration removed all production
/// callers; the algorithm-level regression tests in `mod tests` below
/// still assert its sign-convention invariant.
#[cfg(test)]
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

/// Tessellate a spherical face with adaptive refinement
/// Spherical interpolation between two unit vectors.
fn slerp_unit(a: Vector3, b: Vector3, t: f64) -> Vector3 {
    let dot = a.dot(&b).clamp(-1.0, 1.0);
    let theta = dot.acos();
    if theta < 1e-9 {
        return a;
    }
    let s = theta.sin();
    a * (((1.0 - t) * theta).sin() / s) + b * ((t * theta).sin() / s)
}

/// Extract the single coplanar circle `(centre, unit axis)` shared by every
/// edge of a loop, or `None` if the loop is not such a circle.
fn loop_cut_circle(
    loop_data: &crate::primitives::r#loop::Loop,
    model: &BRepModel,
) -> Option<(Point3, Vector3)> {
    use crate::primitives::curve::Circle;
    if loop_data.edges.is_empty() {
        return None;
    }
    let mut plane: Option<(Point3, Vector3)> = None;
    for &eid in &loop_data.edges {
        let edge = model.edges.get(eid)?;
        let curve = model.curves.get(edge.curve_id)?;
        let circle = curve.as_any().downcast_ref::<Circle>()?;
        let c = circle.center();
        let n = circle.normal().normalize().ok()?;
        match plane {
            None => plane = Some((c, n)),
            Some((pc, pn)) => {
                if (c - pc).dot(&pn).abs() > 1e-5 || pn.cross(&n).magnitude() > 1e-4 {
                    return None;
                }
            }
        }
    }
    plane
}

/// Boundary-conforming tessellation of a spherical CAP: a sphere region bounded
/// by a single cut circle, filled from the rim to the cap apex.
///
/// The rim vertices are taken VERBATIM from the cut-circle boundary edges'
/// `EdgeSampleCache` samples — identical (same curve id + param ranges) to the
/// samples the adjoining planar face uses for its matching hole, so the seam
/// welds by position. The interior is a structured set of rings slerped from
/// the rim toward the cap apex (the far pole on the circle's axis), so the cap
/// is watertight by construction — unlike the analytic grid, whose
/// membership-gated boundary is an open stair-step.
///
/// Returns `true` when it handled the face (single circular outer loop, no
/// inner loops, on a sphere); `false` to fall through to the grid path.
fn tessellate_spherical_cap(
    face: &Face,
    model: &BRepModel,
    surface: &dyn Surface,
    cache: &EdgeSampleCache,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) -> bool {
    use crate::primitives::surface::Sphere;

    if !face.inner_loops.is_empty() {
        return false;
    }
    let Some(sphere) = surface.as_any().downcast_ref::<Sphere>() else {
        return false;
    };
    let o = sphere.center;
    let r = sphere.radius;
    let Some(outer) = model.loops.get(face.outer_loop) else {
        return false;
    };
    let Some((c_center, c_axis)) = loop_cut_circle(outer, model) else {
        return false;
    };

    // Rim: concatenate the loop edges' cache samples in loop order + orientation,
    // dropping the duplicate vertex shared at each edge join.
    let mut rim: Vec<Point3> = Vec::new();
    for (i, &eid) in outer.edges.iter().enumerate() {
        let samples = cache.get_or_compute(eid, model);
        let fwd = outer.orientations.get(i).copied().unwrap_or(true);
        let ordered: Vec<Point3> = if fwd {
            samples.iter().copied().collect()
        } else {
            samples.iter().rev().copied().collect()
        };
        for p in ordered {
            if rim.last().map_or(true, |&q| (q - p).magnitude() > 1e-9) {
                rim.push(p);
            }
        }
    }
    if rim.len() >= 2 && (rim[0] - *rim.last().unwrap()).magnitude() < 1e-9 {
        rim.pop();
    }
    if rim.len() < 3 {
        return false;
    }
    let m = rim.len();

    // Cap apex = far pole on the circle's axis (the side away from the centre).
    let h = (c_center - o).dot(&c_axis);
    let a_dir = if h >= 0.0 { c_axis } else { -c_axis };
    let apex = o + a_dir * r;

    let dirs: Vec<Vector3> = rim
        .iter()
        .map(|&p| (p - o).normalize().unwrap_or(a_dir))
        .collect();
    // Angular span rim→apex sets the ring count.
    let alpha = dirs[0].dot(&a_dir).clamp(-1.0, 1.0).acos();
    let rings = arc_steps_for_quality(alpha.abs(), r, params).max(1);

    let osign = face.orientation.sign();
    let mut ring_idx: Vec<Vec<u32>> = Vec::with_capacity(rings);
    for s in 0..rings {
        let t = s as f64 / rings as f64;
        let mut row = Vec::with_capacity(m);
        for i in 0..m {
            let pos = if s == 0 {
                rim[i]
            } else {
                o + slerp_unit(dirs[i], a_dir, t) * r
            };
            let normal = (pos - o).normalize().unwrap_or(a_dir) * osign;
            row.push(mesh.add_vertex(MeshVertex {
                position: pos,
                normal,
                uv: None,
            }));
        }
        ring_idx.push(row);
    }
    let apex_id = mesh.add_vertex(MeshVertex {
        position: apex,
        normal: a_dir * osign,
        uv: None,
    });

    let forward = face.orientation.is_forward();
    for s in 0..rings - 1 {
        let a = &ring_idx[s];
        let b = &ring_idx[s + 1];
        for i in 0..m {
            let j = (i + 1) % m;
            if forward {
                mesh.add_triangle(a[i], a[j], b[i]);
                mesh.add_triangle(a[j], b[j], b[i]);
            } else {
                mesh.add_triangle(a[i], b[i], a[j]);
                mesh.add_triangle(a[j], b[i], b[j]);
            }
        }
    }
    let last = &ring_idx[rings - 1];
    for i in 0..m {
        let j = (i + 1) % m;
        if forward {
            mesh.add_triangle(last[i], last[j], apex_id);
        } else {
            mesh.add_triangle(last[j], last[i], apex_id);
        }
    }
    true
}

/// Sample a loop's cut-circle rim verbatim from its boundary edges' cache.
fn loop_rim_samples(
    loop_data: &crate::primitives::r#loop::Loop,
    model: &BRepModel,
    cache: &EdgeSampleCache,
) -> Vec<Point3> {
    let mut rim: Vec<Point3> = Vec::new();
    for (i, &eid) in loop_data.edges.iter().enumerate() {
        let samples = cache.get_or_compute(eid, model);
        let fwd = loop_data.orientations.get(i).copied().unwrap_or(true);
        let ordered: Vec<Point3> = if fwd {
            samples.iter().copied().collect()
        } else {
            samples.iter().rev().copied().collect()
        };
        for p in ordered {
            if rim.last().map_or(true, |&q| (q - p).magnitude() > 1e-9) {
                rim.push(p);
            }
        }
    }
    if rim.len() >= 2 && (rim[0] - *rim.last().unwrap()).magnitude() < 1e-9 {
        rim.pop();
    }
    rim
}

/// Boundary-conforming tessellation of a spherical CENTRAL region (sphere minus
/// N caps). A full lat-long grid is gated by membership; the OPEN boundary of
/// the kept triangles is walked into ordered loops, each loop matched to the
/// hole it surrounds and stitched to that hole's rim ring (cut-circle edge
/// samples, so it welds to the adjoining planar disk by position).
///
/// Returns `true` when handled (a sphere face with inner-loop holes).
fn tessellate_spherical_central(
    face: &Face,
    model: &BRepModel,
    surface: &dyn Surface,
    cache: &EdgeSampleCache,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) -> bool {
    use crate::primitives::surface::Sphere;
    use std::collections::HashMap;

    if face.inner_loops.is_empty() {
        return false;
    }
    let Some(sphere) = surface.as_any().downcast_ref::<Sphere>() else {
        return false;
    };
    let o = sphere.center;
    let r = sphere.radius;
    let osign = face.orientation.sign();
    // The lat-long grid quad `(a,b,c)` = (+u,+v) winds INWARD under the radial
    // normal (same handedness issue as the analytic sphere path), so a Forward
    // (outward) central face needs the reversed winding — hence the `!`. The
    // stitch twins the grid by construction, so this one flip orients the whole
    // central consistently with the adjoining planar disks.
    let forward = !face.orientation.is_forward();

    // Holes: plane (centre, axis), an in-plane frame, and the rim verts already
    // added to the mesh (positions kept for angular sort).
    struct Hole {
        center: Point3,
        axis: Vector3,
        e1: Vector3,
        e2: Vector3,
        rim: Vec<(f64, u32, Point3)>, // (angle, mesh id, pos)
    }
    let mut holes: Vec<Hole> = Vec::new();
    for &lid in &face.inner_loops {
        let Some(lp) = model.loops.get(lid) else {
            return false;
        };
        let Some((c, n)) = loop_cut_circle(lp, model) else {
            return false;
        };
        let rim_pos = loop_rim_samples(lp, model, cache);
        if rim_pos.len() < 3 {
            return false;
        }
        let helper = if n.x.abs() <= n.y.abs() && n.x.abs() <= n.z.abs() {
            Vector3::X
        } else if n.y.abs() <= n.z.abs() {
            Vector3::Y
        } else {
            Vector3::Z
        };
        let e1 = (helper - n * helper.dot(&n))
            .normalize()
            .unwrap_or(Vector3::X);
        let e2 = n.cross(&e1);
        let mut rim: Vec<(f64, u32, Point3)> = rim_pos
            .iter()
            .map(|&p| {
                let d = p - c;
                let ang = d.dot(&e2).atan2(d.dot(&e1));
                let normal = (p - o).normalize().unwrap_or(n) * osign;
                let id = mesh.add_vertex(MeshVertex {
                    position: p,
                    normal,
                    uv: None,
                });
                (ang, id, p)
            })
            .collect();
        rim.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        holes.push(Hole {
            center: c,
            axis: n,
            e1,
            e2,
            rim,
        });
    }

    let in_central = |p: Point3| -> bool {
        holes.iter().all(|h| {
            let pp = (p - h.center).dot(&h.axis);
            let oo = (o - h.center).dot(&h.axis);
            pp * oo >= 0.0
        })
    };

    let tau = std::f64::consts::TAU;
    let pi = std::f64::consts::PI;
    let n_u = arc_steps_for_quality(tau, r, params).max(12);
    let n_v = arc_steps_for_quality(pi, r, params).max(6);
    let key = |i: usize, j: usize| -> u32 { (i * (n_v + 1) + j) as u32 };
    let mut gpos = vec![vec![Point3::ORIGIN; n_v + 1]; n_u];
    let mut gid = vec![vec![None::<u32>; n_v + 1]; n_u];
    let mut gcen = vec![vec![false; n_v + 1]; n_u];
    for i in 0..n_u {
        let u = tau * (i as f64) / (n_u as f64);
        for j in 0..=n_v {
            let v = pi * (j as f64) / (n_v as f64);
            let Ok(p) = surface.point_at(u, v) else {
                continue;
            };
            gpos[i][j] = p;
            if in_central(p) {
                gcen[i][j] = true;
                let normal = (p - o).normalize().unwrap_or(Vector3::Z) * osign;
                gid[i][j] = Some(mesh.add_vertex(MeshVertex {
                    position: p,
                    normal,
                    uv: None,
                }));
            }
        }
    }

    // Emit central grid quads; record directed (i,j)-key edges for boundary
    // extraction. A directed edge with no reverse twin is on the open boundary.
    let mut dir_edges: HashMap<(u32, u32), i32> = HashMap::new();
    let mut tri_keyed = |ka: u32, kb: u32, kc: u32| {
        for &(x, y) in &[(ka, kb), (kb, kc), (kc, ka)] {
            *dir_edges.entry((x, y)).or_insert(0) += 1;
        }
    };
    for i in 0..n_u {
        let i2 = (i + 1) % n_u;
        for j in 0..n_v {
            if let (Some(a), Some(b), Some(c), Some(d)) =
                (gid[i][j], gid[i2][j], gid[i][j + 1], gid[i2][j + 1])
            {
                let (ka, kb, kc, kd) = (key(i, j), key(i2, j), key(i, j + 1), key(i2, j + 1));
                if forward {
                    mesh.add_triangle(a, b, c);
                    mesh.add_triangle(b, d, c);
                    tri_keyed(ka, kb, kc);
                    tri_keyed(kb, kd, kc);
                } else {
                    mesh.add_triangle(a, c, b);
                    mesh.add_triangle(b, c, d);
                    tri_keyed(ka, kc, kb);
                    tri_keyed(kb, kc, kd);
                }
            }
        }
    }

    // Open boundary directed edges (no reverse twin) → next-vertex chain.
    let mut next: HashMap<u32, u32> = HashMap::new();
    for (&(a, b), &cnt) in &dir_edges {
        if cnt > 0 && !dir_edges.contains_key(&(b, a)) {
            next.insert(a, b);
        }
    }
    // Decode a key back to (mesh id, 3D pos).
    let decode = |k: u32| -> (u32, Point3) {
        let i = (k as usize) / (n_v + 1);
        let j = (k as usize) % (n_v + 1);
        (gid[i][j].unwrap_or(0), gpos[i][j])
    };

    // Walk boundary loops.
    let mut visited: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut loops: Vec<Vec<u32>> = Vec::new();
    for &start in next.keys() {
        if visited.contains(&start) {
            continue;
        }
        let mut loop_keys = Vec::new();
        let mut cur = start;
        let mut guard = 0;
        loop {
            if !visited.insert(cur) {
                break;
            }
            loop_keys.push(cur);
            cur = match next.get(&cur) {
                Some(&n) => n,
                None => break,
            };
            guard += 1;
            if guard > n_u * (n_v + 1) + 4 {
                break;
            }
            if cur == start {
                break;
            }
        }
        if loop_keys.len() >= 3 {
            loops.push(loop_keys);
        }
    }

    // Stitch each boundary loop to its matching hole's rim.
    for loop_keys in &loops {
        let pts: Vec<(u32, Point3)> = loop_keys.iter().map(|&k| decode(k)).collect();
        // Match the hole whose plane the loop hugs: min mean |dist to plane|.
        let Some(h) = holes.iter().min_by(|h1, h2| {
            let d = |h: &Hole| {
                pts.iter()
                    .map(|&(_, p)| ((p - h.center).dot(&h.axis)).abs())
                    .sum::<f64>()
            };
            d(h1)
                .partial_cmp(&d(h2))
                .unwrap_or(std::cmp::Ordering::Equal)
        }) else {
            continue;
        };
        // Boundary loop in WALK ORDER (a ring around the hole). Keep order;
        // only normalise winding to CCW about the hole axis (total signed
        // angle ≈ +2π) so it matches the rim's CCW order.
        let angle = |p: Point3| -> f64 {
            let d = p - h.center;
            d.dot(&h.e2).atan2(d.dot(&h.e1))
        };
        let wrap = |mut d: f64| {
            let tau = std::f64::consts::TAU;
            while d > std::f64::consts::PI {
                d -= tau;
            }
            while d < -std::f64::consts::PI {
                d += tau;
            }
            d
        };
        // Keep `b` in GRID open-boundary (chain) order — that order is
        // consistent with the solid's orientation. Align the rim's angular
        // direction to `b` so the greedy stitch walks both the same way.
        let b: Vec<(Point3, u32)> = pts.iter().map(|&(id, p)| (p, id)).collect();
        let signed = |ring: &[(Point3, u32)]| -> f64 {
            (0..ring.len())
                .map(|i| wrap(angle(ring[(i + 1) % ring.len()].0) - angle(ring[i].0)))
                .sum()
        };
        let b_dir = signed(&b);
        let mut rim: Vec<(Point3, u32)> = h.rim.iter().map(|&(_, id, p)| (p, id)).collect();
        if signed(&rim) * b_dir < 0.0 {
            rim.reverse();
        }
        stitch_rings(&b, &rim, forward, mesh);
    }
    true
}

/// Triangulate the band between two closed rings — OUTER `bound` and INNER
/// `rim` — each an ordered `(pos, mesh_id)` loop going the SAME direction (CCW
/// about the hole axis). Greedy shortest-diagonal advance: at each step pick the
/// triangle that adds the shorter new diagonal, so it is robust to a jagged
/// boundary (no reliance on angular monotonicity). Winding is set for the
/// outward normal, flipped by `forward`.
fn stitch_rings(
    bound: &[(Point3, u32)],
    rim: &[(Point3, u32)],
    forward: bool,
    mesh: &mut TriangleMesh,
) {
    let (nb, nr) = (bound.len(), rim.len());
    if nb < 2 || nr < 2 {
        return;
    }
    // Align: rim index closest in 3D to bound[0].
    let k0 = (0..nr)
        .min_by(|&a, &b| {
            (rim[a].0 - bound[0].0)
                .magnitude()
                .partial_cmp(&(rim[b].0 - bound[0].0).magnitude())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(0);

    let mut i = 0usize; // steps taken along bound
    let mut k = 0usize; // steps taken along rim
                        // The bound ring is walked in the grid's open-boundary order, so a stitch
                        // triangle must traverse a shared bound edge as its TWIN — the opposite
                        // winding to the grid quads, hence the inverted branch vs the grid emit.
    let mut emit = |a: u32, b: u32, c: u32, mesh: &mut TriangleMesh| {
        if forward {
            mesh.add_triangle(a, c, b);
        } else {
            mesh.add_triangle(a, b, c);
        }
    };
    while i < nb || k < nr {
        let bi = bound[i % nb];
        let rk = rim[(k0 + k) % nr];
        let advance_bound = if i >= nb {
            false
        } else if k >= nr {
            true
        } else {
            let b_next = bound[(i + 1) % nb].0;
            let r_next = rim[(k0 + k + 1) % nr].0;
            (b_next - rk.0).magnitude() <= (bi.0 - r_next).magnitude()
        };
        if advance_bound {
            let b_next = bound[(i + 1) % nb];
            emit(bi.1, b_next.1, rk.1, mesh);
            i += 1;
        } else {
            let r_next = rim[(k0 + k + 1) % nr];
            emit(bi.1, r_next.1, rk.1, mesh);
            k += 1;
        }
    }
}

fn tessellate_spherical_face(
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    _cache: &EdgeSampleCache,
    mesh: &mut TriangleMesh,
) {
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return,
    };

    // Boundary-conforming cap path: a sphere region bounded by a single cut
    // circle welds to its adjoining planar hole and is watertight, unlike the
    // membership-gated grid below. Falls through for untrimmed / multi-loop
    // sphere faces.
    if tessellate_spherical_cap(face, model, surface, _cache, params, mesh) {
        return;
    }
    // Multi-hole central region: sphere minus N caps, grid + boundary-loop
    // stitch to each hole's rim.
    if tessellate_spherical_central(face, model, surface, _cache, params, mesh) {
        return;
    }

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

    // Generate triangles with special handling for poles. The sphere's
    // (u = longitude, v = colatitude) grid winds the base `(a, b, c)` order
    // INWARD under the surface's outward normal, so a geometrically Forward
    // (outward) face needs the REVERSED branch — hence the `!` here. Without
    // it a Forward sphere tessellates inward (signed volume negative; masked
    // by the `.abs()` in mass-props for a standalone solid) and, fatally, a
    // Backward sphere void (e.g. box − interior sphere) winds OUTWARD and the
    // void is ADDED instead of subtracted (V_box + V_sphere).
    let forward = !face.orientation.is_forward();
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
                    // The first ring (row 1) is the BOTTOM row of the band
                    // directly above this fan, so that band traverses the
                    // ring edge `v1->v2`. For a consistently-oriented closed
                    // mesh the fan must traverse the shared ring edge the
                    // OTHER way (`v2->v1`); hence the fan apex triangle is
                    // `(pole, v2, v1)` under `forward`, the mirror of the
                    // north-pole fan (which uses the ring as its TOP row and
                    // is naturally opposite). Emitting `(pole, v1, v2)` here
                    // duplicates the ring edge direction and leaves
                    // `u_steps` orientation-inconsistent edges around the
                    // south pole — invisible to a signed-volume check but
                    // caught by the manifold oracle.
                    if forward {
                        mesh.add_triangle(pole_vertex, v2, v1);
                    } else {
                        mesh.add_triangle(pole_vertex, v1, v2);
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

    // Add triangles with mapping. The sphere's (longitude, colatitude) grid
    // winds the base order INWARD under the outward normal, so a Forward
    // (outward) face needs the reversed branch — see the with-poles path for
    // the full rationale and the void-subtraction failure it fixes.
    let forward = !face.orientation.is_forward();
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
    cache: &EdgeSampleCache,
    mesh: &mut TriangleMesh,
) {
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return,
    };

    // Get parameter bounds from face loops
    let (u_min, u_max, v_min, v_max) = get_face_parameter_bounds(face, model);

    // Distinguish a true apex cone from a frustum by TOPOLOGY, not a v≈0
    // test (the `Cone` v-origin is the extrapolated apex, so a frustum's
    // v_min can also be ~0). An apex cone's lateral loop is a SINGLE
    // base-circle edge (the apex is a point); a frustum's lateral loop is a
    // seamed rectangle — bottom circle + seam + top circle + seam (4 edge
    // entries) — exactly like the cylinder. So a >1-edge outer loop ⇒ frustum.
    let is_apex_cone = model
        .loops
        .get(face.outer_loop)
        .map(|l| l.edges.len() <= 1)
        .unwrap_or(true);

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

    if is_apex_cone {
        tessellate_conical_with_apex(
            face, model, surface, cache, u_min, u_max, v_min, v_max, u_steps, v_steps, mesh,
        );
    } else {
        // Seamed frustum: a Cone lateral with a single rectangular loop
        // (shared circle edges + seam), structurally identical to the
        // cylinder. Route it through the same curved-CDT path the cylinder
        // uses — the cache-shared circle edges make the lateral↔cap seams
        // bit-exact, and the seam (anchored to the circles' t=0) keeps the
        // u-sweep a clean rectangle. On Err, fall back to the legacy grid so
        // the face degrades to a visible (if non-watertight) mesh.
        if let Err(e) =
            super::curved_cdt::tessellate_curved_cdt(surface, face, model, params, cache, mesh)
        {
            tracing::warn!(
                "curved_cdt failed for cone frustum face {:?}: {:?}; falling back to grid",
                face.id,
                e
            );
            // Grid over the cone's INTRINSIC parameter domain (full angular
            // sweep + its own height limits) through the UNTRIMMED grid —
            // exactly the cylinder fallback. Once the seam desyncs (the usual
            // cause of the curved-CDT failure on a transformed/rotated cone)
            // the face-edge-derived u-range collapses, so a trimmed grid
            // (point-in-face) would drop the entire
            // lateral wall and the divergence-theorem volume loses the cone
            // term — a silent mass-property/export corruption for any
            // non-axis-aligned frustum (observed: 68.06 → 29.86 after a rigid
            // motion). `evaluate_full(u, v)` traces the correct lateral from
            // the surface's stored (rotated) frame regardless of seam state.
            let cone = surface
                .as_any()
                .downcast_ref::<crate::primitives::surface::Cone>();
            let (cu_min, cu_max, cv_min, cv_max) = match cone {
                Some(c) => {
                    let (u0, u1) = c
                        .angle_limits
                        .map_or((0.0, std::f64::consts::TAU), |[a, b]| (a, b));
                    let (v0, v1) = c.height_limits.map_or((v_min, v_max), |[a, b]| (a, b));
                    (u0, u1, v0, v1)
                }
                None => (u_min, u_max, v_min, v_max),
            };
            tessellate_surface_grid_untrimmed(
                face, model, surface, cu_min, cu_max, cv_min, cv_max, u_steps, v_steps, mesh,
            );
        }
    }
}

/// Tessellate cone with apex handling
fn tessellate_conical_with_apex(
    face: &Face,
    model: &BRepModel,
    surface: &dyn Surface,
    cache: &EdgeSampleCache,
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
    // Base-circle row uses the EdgeSampleCache for the lateral's single
    // boundary edge (the wide-end circle) so the lateral and the base cap
    // — which samples the same edge via the cache (`sample_loop_3d_polygon`)
    // — share that seam bit-exactly. Without it the lateral picks its own
    // `u`-resolution (`arc_steps_for_quality`) and the circle T-junctions
    // against the cap: the cone analogue of the pre-fix cylinder. An apex
    // cone's lateral loop is a single degenerate-domain edge (the apex is a
    // point, not an edge), so curved-CDT is N/A and the grid is made
    // cache-coherent instead. (CDT-γ.3)
    let base_circle: Option<Vec<Point3>> = model
        .loops
        .get(face.outer_loop)
        .and_then(|lp| lp.edges.first().copied())
        .map(|eid| (*cache.get_or_compute(eid, model)).clone())
        .filter(|s| s.len() >= 2);

    // Anchor each column to the cone-`u` of its cached base point so the
    // ring sits directly above that point (columns stay vertical — no
    // twist) and the grid width matches the cache (so the quad strips and
    // the u-seam line up). `point_at`/`normal_at` are periodic in `u`, so
    // the branch `closest_point` returns is immaterial — only the position
    // it maps to matters.
    let base_us: Option<Vec<f64>> = base_circle.as_ref().map(|s| {
        s.iter()
            .map(|p| {
                surface
                    .closest_point(p, Tolerance::default())
                    .map(|(u, _)| u)
                    .unwrap_or(u_min)
            })
            .collect()
    });
    let u_steps = match &base_circle {
        Some(s) => s.len() - 1,
        None => u_steps,
    };

    let v_start = if v_min.abs() < 1e-6 { 1 } else { 0 };
    for v_idx in v_start..=v_steps {
        let v = v_min + (v_idx as f64) * (v_max - v_min) / (v_steps as f64);
        let is_base = v_idx == v_steps;
        let mut row = Vec::new();

        for u_idx in 0..=u_steps {
            // Column `u`: the cached base point's cone-`u` when available
            // (keeps the column vertical), else an even sweep.
            let u = match &base_us {
                Some(us) => us.get(u_idx).copied().unwrap_or(u_min),
                None => u_min + (u_idx as f64) * (u_max - u_min) / (u_steps as f64),
            };

            // The base row takes its 3D verbatim from the cache (bit-exact
            // with the cap); interior rows lift through the surface.
            let cached = if is_base {
                base_circle.as_ref().and_then(|s| s.get(u_idx).copied())
            } else {
                None
            };
            let vertex = match cached {
                Some(p) => face.normal_at(u, v, &model.surfaces).ok().map(|n| (p, n)),
                None => match (
                    surface.point_at(u, v),
                    face.normal_at(u, v, &model.surfaces),
                ) {
                    (Ok(p), Ok(n)) => Some((p, n)),
                    _ => None,
                },
            };
            match vertex {
                Some((position, normal)) => {
                    let index = mesh.add_vertex(MeshVertex {
                        position,
                        normal,
                        uv: Some((u, v)),
                    });
                    row.push(Some(index));
                }
                None => row.push(None),
            }
        }
        vertex_grid.push(row);
    }

    // Generate triangles. Winding follows `face.orientation`
    // (see cylindrical path for rationale).
    let forward = face.orientation.is_forward();
    for v_idx in 0..vertex_grid.len() - 1 {
        if v_idx == 0 && vertex_grid[0].len() == 1 {
            // Triangles from apex. Row 1 is the BOTTOM row of the band above
            // this fan, which traverses each ring edge `v1->v2`; for a
            // consistently-oriented mesh the apex fan must traverse the shared
            // edge `v2->v1`, so the apex triangle is `(apex, v2, v1)` under
            // `forward` (the mirror of a top-row fan). Emitting `(apex, v1, v2)`
            // duplicates the ring-edge direction and leaves `u_steps`
            // orientation-inconsistent edges around the apex — invisible to a
            // signed-volume check, caught by the manifold oracle.
            if let Some(apex) = vertex_grid[0][0] {
                for u_idx in 0..u_steps {
                    if let (Some(v1), Some(v2)) = (
                        vertex_grid[1].get(u_idx).and_then(|&v| v),
                        vertex_grid[1].get(u_idx + 1).and_then(|&v| v),
                    ) {
                        if forward {
                            mesh.add_triangle(apex, v2, v1);
                        } else {
                            mesh.add_triangle(apex, v1, v2);
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
    _cache: &EdgeSampleCache,
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

/// Tessellate a single `SurfaceOfRevolution` wedge face as a structured grid
/// whose four boundary edges are sampled from the `EdgeSampleCache` (so shared
/// seams are bit-exact with neighbours) and whose interior is a transfinite
/// (Coons) blend of those boundaries — never invoking `cdt`.
///
/// Revolve builds each curved wall as 32 thin per-segment `SurfaceOfRevolution`
/// faces. Each is a four-sided patch: two profile rails and two angular arcs. At
/// fine chord tolerance the generic curved-CDT path fails on these high-aspect
/// slivers (`PointOnFixedEdge` / `WedgeEscape` from collinear straight rails +
/// dense interior Steiner points), drops the face, and leaves the solid
/// non-watertight — the revolve volume then collapses (REVOLVE-ROBUSTNESS #47).
/// A wedge is a tensor-product patch, so a structured grid is exact and robust.
///
/// The grid indexing is driven by the loop *cycle*: consecutive edges share a
/// corner by construction, so the four chains `A,B,C,D` (each an oriented run of
/// cache samples) tile the patch without any `closest_point` classification or
/// fuzzy corner matching. Opposite chains must have equal sample counts
/// (`|A|=|C|`, `|B|=|D|`); when they don't — a flat radial rim sector, an apex
/// triangle, or a slanted unequal-radius wedge — the function returns `false`
/// having emitted nothing, and the caller falls back to the generic path.
fn tessellate_revolution_wedge(
    face: &Face,
    model: &BRepModel,
    cache: &EdgeSampleCache,
    mesh: &mut TriangleMesh,
) -> bool {
    if !face.inner_loops.is_empty() {
        return false;
    }
    let loop_data = match model.loops.get(face.outer_loop) {
        Some(l) => l,
        None => return false,
    };
    if loop_data.edges.len() != 4 {
        return false;
    }

    // Walk the loop, collecting each edge's cache samples oriented in
    // loop-traversal direction. The chains then form a continuous ring
    // c0→c1→c2→c3→c0, each consecutive pair sharing a corner vertex.
    let mut chains: Vec<Vec<Point3>> = Vec::with_capacity(4);
    for (k, &eid) in loop_data.edges.iter().enumerate() {
        let samples = cache.get_or_compute(eid, model);
        if samples.len() < 2 {
            return false;
        }
        let mut c: Vec<Point3> = samples.iter().copied().collect();
        if !loop_data.orientations.get(k).copied().unwrap_or(true) {
            c.reverse();
        }
        chains.push(c);
    }
    let (a, b, c, d) = (&chains[0], &chains[1], &chains[2], &chains[3]);
    // Opposite chains must tile a rectangular grid.
    if a.len() != c.len() || b.len() != d.len() {
        return false;
    }
    let n = a.len(); // i index: 0..n along A (c0→c1) and C (c2→c3)
    let m = b.len(); // j index: 0..m along B (c1→c2) and D (c3→c0)
    if n < 2 || m < 2 {
        return false;
    }
    // Corner continuity (chains belong to one loop, so these hold exactly for a
    // clean quad; a mismatch means the loop is not a 4-corner patch).
    let close = |p: Point3, q: Point3| (p - q).magnitude() < 1e-6;
    if !close(a[n - 1], b[0])
        || !close(b[m - 1], c[0])
        || !close(c[n - 1], d[0])
        || !close(d[m - 1], a[0])
    {
        return false;
    }

    // Position at grid node (i, j): boundary nodes come verbatim from the cache
    // chains (so shared seams are bit-exact); interior nodes are a bilinear
    // Coons blend of the four boundaries — purely boundary-driven, no surface
    // re-evaluation, and exact for the developable wedge.
    let c0 = a[0];
    let c1 = a[n - 1];
    let c2 = b[m - 1];
    let c3 = c[n - 1];
    let node = |i: usize, j: usize| -> Point3 {
        if j == 0 {
            return a[i]; // c0 → c1
        }
        if j == m - 1 {
            return c[n - 1 - i]; // c3 → c2
        }
        if i == 0 {
            return d[m - 1 - j]; // c0 → c3
        }
        if i == n - 1 {
            return b[j]; // c1 → c2
        }
        let s = i as f64 / (n - 1) as f64;
        let t = j as f64 / (m - 1) as f64;
        let bottom = a[i];
        let top = c[n - 1 - i];
        let left = d[m - 1 - j];
        let right = b[j];
        // Coons bilinear transfinite interpolation.
        left * (1.0 - s) + right * s + bottom * (1.0 - t) + top * t
            - (c0 * ((1.0 - s) * (1.0 - t))
                + c1 * (s * (1.0 - t))
                + c2 * (s * t)
                + c3 * ((1.0 - s) * t))
    };

    // Emit grid vertices. Shading normals are taken from local grid tangents
    // (positions are what matter for watertightness; normals need only be
    // smooth), oriented to agree with the face's outward triangle winding.
    let pos: Vec<Vec<Point3>> = (0..n)
        .map(|i| (0..m).map(|j| node(i, j)).collect())
        .collect();
    let normal_at = |i: usize, j: usize| -> Vector3 {
        let ip = pos[(i + 1).min(n - 1)][j];
        let im = pos[i.saturating_sub(1)][j];
        let jp = pos[i][(j + 1).min(m - 1)];
        let jm = pos[i][j.saturating_sub(1)];
        (ip - im)
            .cross(&(jp - jm))
            .normalize()
            .unwrap_or(Vector3::Z)
    };
    let mut grid: Vec<Vec<u32>> = vec![vec![0u32; m]; n];
    for i in 0..n {
        for j in 0..m {
            grid[i][j] = mesh.add_vertex(MeshVertex {
                position: pos[i][j],
                normal: normal_at(i, j),
                uv: None,
            });
        }
    }

    // Emit two triangles per cell. The loop is walked CCW about the surface's
    // NATURAL normal, and with i along chain A and j along chain D-reversed we
    // have (i+)×(j+) = +natural. The mesh must wind CCW about the OUTWARD normal
    // (= natural · orientation.sign): a Forward face (outward = +natural) needs
    // triangle normal +natural, i.e. winding (v00, v10, v01); a Backward face
    // takes the mirror.
    for i in 0..n - 1 {
        for j in 0..m - 1 {
            let (v00, v10, v01, v11) = (
                grid[i][j],
                grid[i + 1][j],
                grid[i][j + 1],
                grid[i + 1][j + 1],
            );
            if face.orientation == crate::primitives::face::FaceOrientation::Forward {
                mesh.add_triangle(v00, v10, v01);
                mesh.add_triangle(v10, v11, v01);
            } else {
                mesh.add_triangle(v00, v01, v10);
                mesh.add_triangle(v10, v01, v11);
            }
        }
    }
    true
}

/// Grid-tessellate a full surface patch over `[u_min,u_max]×[v_min,v_max]`
/// WITHOUT the per-vertex `is_point_inside_face` trim.
///
/// This is the cylinder-lateral fallback when curved-CDT fails on a
/// transformed (e.g. rotated) cylinder. A closed cylinder's lateral face
/// covers its entire parameter rectangle with no interior holes, so the UV
/// point-in-face trim used by [`tessellate_surface_grid`] is unnecessary —
/// and actively harmful here: once a transform desyncs the seam from the cap
/// circles' `t=0`, the trim's UV classification rejects the whole wall,
/// dropping the lateral and leaving a caps-only shell whose volume collapses
/// to ~1/3 of the truth. Winding, normals and `face_map` match
/// [`tessellate_surface_grid`]; only the trim is removed.
fn tessellate_surface_grid_untrimmed(
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
                row.push(Some(index));
            } else {
                row.push(None);
            }
        }
        vertex_grid.push(row);
    }

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
    cache: &EdgeSampleCache,
    mesh: &mut TriangleMesh,
) {
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return,
    };

    // For NURBS surfaces we route through `curved_cdt`. CDT-β.2
    // retired the legacy `tessellate_curved_adaptive` quadtree
    // fallback after the full workspace test corpus produced zero
    // fallback firings under `ROSHERA_TESS_TRACE=1`. On `Err(_)`
    // the contract is now "this face emits zero triangles" —
    // the shell-level `tessellate_shell` proceeds with the rest
    // of the shell.
    if let Err(e) =
        super::curved_cdt::tessellate_curved_cdt(surface, face, model, params, cache, mesh)
    {
        tracing::warn!(
            "curved_cdt failed for NURBS face {:?}: {:?}; emitting empty \
             mesh (CDT-β.2: legacy quadtree fallback retired)",
            face.id,
            e
        );
    }
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
    cache: &EdgeSampleCache,
    mesh: &mut TriangleMesh,
) {
    // Cache-driven grid. The fillet face's outer loop has a fixed
    // contract (`operations/fillet.rs:2715`):
    //
    //   [trim1 fwd, cap_v1 fwd, trim2 rev, cap_v0 fwd]
    //
    // The four edges define the four sides of a topological rectangle
    // in parameter space:
    //
    //   v=0      row (u_idx 0..=u_steps) ↔ trim1 (canonical fwd)
    //   v=v_max  row (u_idx 0..=u_steps) ↔ trim2 (canonical fwd)
    //   u=0      column (v_idx 0..=v_steps) ↔ cap_v0 reversed
    //   u=u_max  column (v_idx 0..=v_steps) ↔ cap_v1 (canonical fwd)
    //
    // Corner consistency (each derived from BOTH a row and a column):
    //
    //   (0, 0)             = trim1[0]  = cap_v0[len-1] = v_t1_start
    //   (u_max, 0)         = trim1[end]= cap_v1[0]     = v_t1_end
    //   (0, v_max)         = trim2[0]  = cap_v0[0]     = v_t2_start
    //   (u_max, v_max)     = trim2[end]= cap_v1[end]   = v_t2_end
    //
    // Boundary cells take the exact cached samples; the adjacent face's
    // tessellator hits the same cache (via `sample_loop_3d_polygon`) so
    // both sides of every shared edge land on the same 3D points. This
    // is the canonical-edge-sample pattern that eliminates T-junctions
    // between the fillet face and the trimmed planar / cylindrical
    // neighbours.
    //
    // For loops with !=4 edges (zero-radius degenerate fillets etc.)
    // we fall back to the previous UV-grid sampler, which does NOT
    // share its boundary with neighbours — acceptable for geometrically
    // degenerate faces and rare in practice.
    let Some(surface) = model.surfaces.get(face.surface_id) else {
        return;
    };
    let Some(outer_loop) = model.loops.get(face.outer_loop) else {
        return;
    };

    if outer_loop.edges.len() != 4 {
        if outer_loop.edges.len() != 3 {
            tracing::warn!(
                edge_count = outer_loop.edges.len(),
                "fillet face has unexpected loop edge count; using grid fallback"
            );
        }
        tessellate_fillet_face_grid_fallback(face, model, params, mesh);
        return;
    }

    let trim1 = cache.get_or_compute(outer_loop.edges[0], model);
    let cap_v1 = cache.get_or_compute(outer_loop.edges[1], model);
    let trim2 = cache.get_or_compute(outer_loop.edges[2], model);
    let cap_v0 = cache.get_or_compute(outer_loop.edges[3], model);

    if trim1.len() < 2 || trim2.len() < 2 || cap_v0.len() < 2 || cap_v1.len() < 2 {
        tessellate_fillet_face_grid_fallback(face, model, params, mesh);
        return;
    }

    // Grid resolution. Honouring the longer cache on each axis preserves
    // every sample the cache decided was needed. When the trim caches
    // agree in length (the common box-fillet / symmetric-blend case),
    // no resampling occurs and every boundary sample lands on a cached
    // point.
    let u_steps = (trim1.len() - 1).max(trim2.len() - 1);
    let v_steps = (cap_v0.len() - 1).max(cap_v1.len() - 1);

    // Locally resample the shorter sequence by linear interpolation.
    // This does NOT mutate the cache; if a neighbour reads the canonical
    // (un-resampled) cache for the same edge, its boundary samples
    // diverge from ours only on the shorter side. In the common case
    // they agree by construction.
    let trim1_r = resample_polyline_to_n(&trim1, u_steps + 1);
    let trim2_r = resample_polyline_to_n(&trim2, u_steps + 1);
    let cap_v0_r = resample_polyline_to_n(&cap_v0, v_steps + 1);
    let cap_v1_r = resample_polyline_to_n(&cap_v1, v_steps + 1);

    let ((u_min, u_max), (v_min, v_max)) = surface.parameter_bounds();
    let mut vertex_grid: Vec<Vec<Option<u32>>> = Vec::with_capacity(v_steps + 1);

    for v_idx in 0..=v_steps {
        let v_param = if v_steps == 0 {
            v_min
        } else {
            v_min + (v_idx as f64) * (v_max - v_min) / (v_steps as f64)
        };
        let mut row = Vec::with_capacity(u_steps + 1);
        for u_idx in 0..=u_steps {
            let u_param = if u_steps == 0 {
                u_min
            } else {
                u_min + (u_idx as f64) * (u_max - u_min) / (u_steps as f64)
            };

            let position = if v_idx == 0 {
                trim1_r[u_idx]
            } else if v_idx == v_steps {
                trim2_r[u_idx]
            } else if u_idx == 0 {
                // cap_v0 runs v_t2_start → v_t1_start, opposite to the
                // v=0 → v=v_steps walk, so reverse-index.
                cap_v0_r[v_steps - v_idx]
            } else if u_idx == u_steps {
                cap_v1_r[v_idx]
            } else {
                match surface.point_at(u_param, v_param) {
                    Ok(p) => p,
                    Err(_) => {
                        row.push(None);
                        continue;
                    }
                }
            };

            let normal = face
                .normal_at(u_param, v_param, &model.surfaces)
                .unwrap_or(Vector3::Z);

            let index = mesh.add_vertex(MeshVertex {
                position,
                normal,
                uv: Some((u_param, v_param)),
            });
            row.push(Some(index));
        }
        vertex_grid.push(row);
    }

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

/// Linear-interpolate a polyline of `Point3` samples to exactly `n`
/// points. Endpoints are preserved; intermediates are obtained by
/// arclength-parameter-uniform sampling of the cached polyline.
///
/// Used by `tessellate_fillet_face` to bridge an axis where the two
/// boundary caches have different sample counts. Resampled points do
/// NOT enter the cache.
fn resample_polyline_to_n(samples: &[Point3], n: usize) -> Vec<Point3> {
    use crate::math::Interpolate;
    if samples.len() == n {
        return samples.to_vec();
    }
    if n == 0 {
        return Vec::new();
    }
    if samples.is_empty() {
        return Vec::new();
    }
    if samples.len() == 1 || n == 1 {
        return vec![samples[0]; n];
    }
    let src_last = samples.len() - 1;
    let dst_last = n - 1;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let t = (i as f64) * (src_last as f64) / (dst_last as f64);
        let lo = (t.floor() as usize).min(src_last);
        let hi = (lo + 1).min(src_last);
        let frac = t - (lo as f64);
        out.push(samples[lo].lerp(&samples[hi], frac));
    }
    out
}

/// Fallback grid sampler used when the fillet face's outer loop does
/// not have the canonical 4-edge contract. Samples the UV grid
/// directly from `surface.point_at` without consulting the edge cache,
/// so boundary samples may not coincide with neighbours' samples
/// (acceptable for degenerate / unexpected topology).
fn tessellate_fillet_face_grid_fallback(
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) {
    let Some(surface) = model.surfaces.get(face.surface_id) else {
        return;
    };

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
                longest_edge_n = compute_curve_sample_count(curve, t_start, t_end, params);
            }
        }
        if longest_edge_n > u_steps {
            u_steps = longest_edge_n;
        }
    }

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
        n.max(params.min_segments.max(3)).min(params.max_segments)
    };

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
pub(crate) fn get_face_parameter_bounds(face: &Face, model: &BRepModel) -> (f64, f64, f64, f64) {
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

/// Public re-export of the winding-number face-membership test.
///
/// Required by `operations::section` for NURBS / Ruled / Offset /
/// SurfaceOfRevolution face-domain trimming: those surfaces don't have
/// axis-aligned rectangular parameter domains, so the cheap UV-bbox
/// test from `get_face_parameter_bounds` over-includes regions that
/// fall outside the face's trim loops. The point-in-face winding test
/// is the only correct boundary check for general parametric faces.
///
/// Forwards directly to the file-local `is_point_inside_face`; kept as
/// a separate public name so the tessellation module's internal API
/// stays stable.
pub(crate) fn point_inside_face_uv(u: f64, v: f64, face: &Face, model: &BRepModel) -> bool {
    is_point_inside_face(u, v, face, model)
}

/// Check if a parameter point is inside face boundaries using winding number algorithm
fn is_point_inside_face(u: f64, v: f64, face: &Face, model: &BRepModel) -> bool {
    // Robust path for a sphere trimmed by coplanar cut circles: an
    // iso-parametric cut circle has zero `(u, v)` area, so the winding test
    // below degenerates (a cap renders as the whole sphere; a multi-hole
    // central region cannot be expressed). Test the circle-plane half-spaces
    // instead. `None` (non-sphere, or non-circular trims) falls through to the
    // legacy winding test, so existing faces are unaffected.
    if let Some(surface) = model.surfaces.get(face.surface_id) {
        if let Ok(p) = surface.point_at(u, v) {
            if let Some(inside) = crate::operations::boolean::spherical_circular_membership(
                model,
                face,
                surface,
                &p,
                &crate::math::Tolerance::default(),
            ) {
                return inside;
            }
        }
    }

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
pub(crate) fn polygon_signed_area_uv(polygon: &[(f64, f64)]) -> f64 {
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
pub(crate) fn calculate_winding_number(point: &(f64, f64), polygon: &[(f64, f64)]) -> f64 {
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
        assert!(n <= params.max_segments, "expected ≤max_segments, got {n}");
    }

    /// Chord-height is the primary driver: tightening `chord_tolerance`
    /// must monotonically increase the step count (until max_segments cap).
    #[test]
    fn arc_steps_monotonic_in_chord_tolerance() {
        let mk = |tol: f64| TessellationParams {
            chord_tolerance: tol,
            max_edge_length: 0.0,     // disable chord-length cap
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
            chord_tolerance: 1e-6, // would request enormous step count
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
        assert_eq!(
            arc_steps_for_quality(0.0, 1.0, &params),
            params.min_segments
        );
        assert_eq!(
            arc_steps_for_quality(-1.0, 1.0, &params),
            params.min_segments
        );
        assert_eq!(
            arc_steps_for_quality(1.0, 0.0, &params),
            params.min_segments
        );
        assert_eq!(
            arc_steps_for_quality(1.0, -1.0, &params),
            params.min_segments
        );
    }

    /// linear_steps: zero-curvature axis only uses chord-length.
    #[test]
    fn linear_steps_basic_chord_length() {
        let params = TessellationParams {
            chord_tolerance: 0.001, // ignored on linear axis
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
    /// chord-height path actually drives the cylinder's tessellation
    /// (now the cache-based curved-CDT path), not just available as a helper.
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
