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
            // SADDLE-35: the full-period annulus fragments (rim ring + full-period
            // wavy lens boundary) from `split_cylinder_lateral_by_saddle` cannot be
            // meshed by curved-CDT (neither loop closes in the (θ,z) chart). Stitch
            // the two cached rims directly; declined for every other cylinder face.
            if tessellate_cylinder_saddle_annulus(face, model, cache, mesh) {
            } else if let Err(e) =
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
                if std::env::var("ROSHERA_TESS_TRACE").is_ok() {
                    eprintln!(
                        "[tess] FALLBACK cylinder face {:?}: curved_cdt {:?} -> UNTRIMMED grid \
                         (ignores boolean trim/holes; covers the bore — #24)",
                        face.id, e
                    );
                }
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
        "Torus" => {
            // Trimmed torus (boolean rim-poke main body / bump) → conforming
            // grid+stitch mesher; untrimmed full torus falls through to the fast
            // structured grid.
            if tessellate_blend_torus_conforming(face, model, surface, cache, params, mesh) {
            } else if !tessellate_toroidal_trimmed(face, model, surface, cache, params, mesh) {
                tessellate_toroidal_face(face, model, params, cache, mesh);
            }
        }
        // A `GeneralNurbsSurface` reports `type_name() == "NurbsSurface"`. A
        // CLOSED-in-U skin lateral (the `nurbs_loft` wall: a seamed ring surface)
        // routes to the structured cache-driven grid — curved-CDT's NURBS
        // `closest_point` boundary inversion is unreliable at the seam and emits
        // an empty mesh. Open NURBS patches keep the curved-CDT path.
        //
        // The seam test is `is_periodic_u`, NOT `is_closed_u`: a genuinely
        // PERIODIC-U skin (the `skin_surface_periodic_u` wall — wrapped control
        // net, smooth C2 seam) closes onto itself at u=0≡u=1 exactly like the
        // clamped-repeated skin but its first/last control rows differ, so
        // `is_closed_u` misses it. Routed to curved-CDT the periodic wall's
        // boundary→(u,v) inversion zig-zags across the seam, the winding test
        // then rejects EVERY interior Steiner point, and the wall tessellates as
        // a single rim-to-rim band — a geometrically collapsed mesh (a tapered
        // nozzle read as a straight tube). The structured skin-lateral grid
        // samples (u,v) directly and is watertight by construction for both
        // representations.
        "NurbsSurface" => {
            // CF-γ.7 corner-cap apex fan: a small degenerate-column
            // triangular cap (3-rim loop + apex hint) that curved-CDT's
            // UV inversion rejects. Welds via the cached rim; falls
            // through for every other NURBS face.
            if tessellate_corner_cap_apex_fan(face, model, surface, cache, mesh) {
            } else if !(surface.is_periodic_u()
                && tessellate_nurbs_skin_lateral(surface, face, model, params, cache, mesh))
            {
                tessellate_nurbs_face(face, model, params, cache, mesh);
            }
        }
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
                if std::env::var("ROSHERA_WEDGE_TRACE").is_ok() {
                    let ne = model
                        .loops
                        .get(face.outer_loop)
                        .map(|l| l.edges.len())
                        .unwrap_or(0);
                    let pl = surface.is_planar(Tolerance::new(
                        params.chord_tolerance,
                        params.max_angle_deviation,
                    ));
                    eprintln!(
                        "[revfall] face {} edges={} planar={} inner={}",
                        face.id,
                        ne,
                        pl,
                        face.inner_loops.len()
                    );
                }
                let planar_tol = Tolerance::new(params.chord_tolerance, params.max_angle_deviation);
                if surface.is_planar(planar_tol)
                    || is_face_loop_planar_in_3d(face, model, cache, planar_tol)
                {
                    tessellate_planar_face(face, model, params, cache, mesh);
                } else if let Err(e) = super::curved_cdt::tessellate_curved_cdt(
                    surface, face, model, params, cache, mesh,
                ) {
                    if std::env::var("ROSHERA_TESS_TRACE").is_ok() {
                        eprintln!(
                            "[tess] FALLBACK revolution face {:?}: grid declined + curved_cdt {:?} \
                             -> emitted NOTHING (#24)",
                            face.id, e
                        );
                    }
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
        // A shell's collar wall (`offset_solid` on a NURBS / curved lateral)
        // is a RuledSurface between two CLOSED concentric rims that do not lie
        // in a common cap plane — a slanted annular band. Stitch the two cached
        // rim rings directly (the same closed-ring stitch the cylinder/NURBS
        // laterals use), so the collar coincides bit-for-bit with the adjacent
        // exterior / interior laterals at every rim sample. Any other
        // RuledSurface (planar extrude wall, non-annular band) falls through to
        // the generic planar-or-curved routing below.
        "RuledSurface" if tessellate_ruled_annular_band(face, model, cache, mesh) => {}
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
            // Test order matters for latency, not semantics: the `||`
            // accepts either test, but their costs are wildly asymmetric.
            // `is_face_loop_planar_in_3d` reads the loop's cached edge
            // samples (microseconds); the default `Surface::is_planar`
            // scans an 11×11 grid of `normal_at` evaluations over the
            // FULL host-surface parameter bounds. For the dominant case
            // — a polyline-extruded side wall, whose host
            // RuledSurface(Polyline, Polyline) carries every kink of the
            // profile — the surface-global test always FAILS after all
            // 121 evaluations (each a polyline segment search), ~1.9 ms
            // per face, ×N side walls per extrude (profiled 2026-06-12:
            // 120 ms of a 64-gon prism's 163 ms tessellation). Loop-first
            // short-circuits that scan for every flat wall.
            if is_face_loop_planar_in_3d(face, model, cache, planar_tolerance)
                || surface.is_planar(planar_tolerance)
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
    let normal = if let Some(mut n) = newell_n {
        // Newell's normal flips with the outer loop's stored CCW/CW winding.
        // For a face whose entire outer loop is a single CUT circle (a boolean
        // inner-disk fragment), that stored direction is arbitrary, so Newell
        // can point inward — flipping the disk and breaking the weld with the
        // adjoining curved face. A true `Plane` surface has a reliable constant
        // normal, so align Newell to it before applying the orientation sign.
        // (RuledSurface-planar faces keep the Newell-only path — their
        // analytic normal at a UV midpoint is the unreliable one.)
        if surface.type_name() == "Plane" {
            if let Ok(eval) = surface.evaluate_full(0.0, 0.0) {
                if n.dot(&eval.normal) < 0.0 {
                    n = -n;
                }
            }
        }
        n * face.orientation.sign()
    } else {
        let (u_range, v_range) = surface.parameter_bounds();
        let u_mid = (u_range.0 + u_range.1) / 2.0;
        let v_mid = (v_range.0 + v_range.1) / 2.0;
        face.normal_at(u_mid, v_mid, &model.surfaces)
            .unwrap_or(Vector3::Z)
    };

    // ANNULUS FAST PATH. A planar face bounded by an outer circle and a single
    // CONCENTRIC inner circle (the washer caps an analytic revolve emits) is a
    // degenerate case for the general hole-CDT: it produces a chevron/herringbone
    // mesh and — because the `cdt` crate classifies holes by nesting and the two
    // circles confuse it — fills part of the bore with spanning triangles. Detect
    // it and triangulate directly as a clean radial strip between the two rings.
    if let Some(tris) = annulus_radial_strip(&all_vertices, &loop_boundaries, &normal) {
        let mut vertex_map = Vec::with_capacity(all_vertices.len());
        for vertex in &all_vertices {
            vertex_map.push(mesh.add_vertex(MeshVertex {
                position: *vertex,
                normal,
                uv: None,
            }));
        }
        for t in tris {
            mesh.add_triangle(vertex_map[t[0]], vertex_map[t[1]], vertex_map[t[2]]);
        }
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
/// Fan-triangulate `pts2d[range.0..range.1]` if it is a strictly
/// convex simple polygon; `None` sends the caller to the CDT path.
///
/// Strictness is the safety contract: every consecutive edge pair must
/// turn the same way with a cross product decisively above a
/// scale-relative threshold. Collinear vertices are REJECTED rather
/// than skipped — a collinear boundary vertex dropped from this face's
/// triangulation while the neighbouring face keeps it would be a
/// T-junction, which the watertight oracle counts as an open edge.
/// For a polygon that passes, a fan from the first vertex is a valid
/// triangulation by convexity, and emitting it in the polygon's CCW
/// direction satisfies the caller's winding contract (CCW in the
/// `u_axis × v_axis = normal` basis).
fn fan_strictly_convex_polygon(
    pts2d: &[(f64, f64)],
    range: (usize, usize),
) -> Option<Vec<[usize; 3]>> {
    let (s, e) = range;
    let n = e - s;
    if n < 3 || e > pts2d.len() {
        return None;
    }
    // Scale-relative degeneracy threshold: cross products compare
    // against the squared bounding-box extent so the test is invariant
    // under uniform scaling of the model.
    let mut max_extent: f64 = 0.0;
    for &(x, y) in &pts2d[s..e] {
        max_extent = max_extent.max(x.abs()).max(y.abs());
    }
    let eps = 1e-12 * max_extent * max_extent;

    let mut sign = 0.0f64;
    for i in 0..n {
        let (ax, ay) = pts2d[s + i];
        let (bx, by) = pts2d[s + (i + 1) % n];
        let (cx, cy) = pts2d[s + (i + 2) % n];
        let cross = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
        if cross.abs() <= eps {
            return None; // collinear or duplicate vertex — CDT path
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return None; // reflex vertex — CDT path
        }
    }

    let mut tris = Vec::with_capacity(n - 2);
    for i in 1..n - 1 {
        if sign > 0.0 {
            // Polygon is CCW in the projected basis — fan as-is.
            tris.push([s, s + i, s + i + 1]);
        } else {
            // CW polygon — reverse each triangle to emit CCW.
            tris.push([s, s + i + 1, s + i]);
        }
    }
    Some(tris)
}

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

    // FAST PATH: a strictly convex, hole-free polygon needs no
    // Delaunay machinery — a fan from its first vertex is a valid
    // triangulation that keeps every boundary vertex (no T-junctions
    // against neighbouring faces) and costs O(n). This is the planar
    // workhorse case: every side wall of an extruded polygon and every
    // convex cap (e.g. the 64-gon caps of a sketched circle) lands
    // here. Profiled 2026-06-12: routing these through
    // `cdt::triangulate_contours` (exact Shewchuk predicates + full
    // CDT setup) dominated interactive extrude latency. Reflex,
    // collinear-vertex, and holed polygons — boolean scar faces — fall
    // through to the robust CDT path unchanged.
    if inner_ranges.is_empty() {
        if let Some(tris) = fan_strictly_convex_polygon(&pts2d, outer_range) {
            return tris;
        }
    }

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

/// Triangulate a CONCENTRIC-CIRCLE ANNULUS (one outer ring + one inner ring) as a
/// clean radial strip between the two rings — well-conditioned triangles, no
/// chevron seam, the bore left correctly empty. Returns `None` unless the face is
/// exactly such an annulus (the caller then falls back to the general CDT). The
/// rings are sampled CCW from a common seam, so they are stitched by fractional
/// position around the circle (handling unequal sample counts) into `n + m`
/// triangles, each oriented to the face `normal`. Indices are into `vertices`.
fn annulus_radial_strip(
    vertices: &[Point3],
    loop_boundaries: &[(usize, usize, bool)],
    normal: &Vector3,
) -> Option<Vec<[usize; 3]>> {
    // Exactly one outer ring + one inner ring, each with ≥3 samples.
    let mut outer: Option<(usize, usize)> = None;
    let mut inner: Option<(usize, usize)> = None;
    for &(s, e, is_outer) in loop_boundaries {
        if e - s < 3 {
            return None;
        }
        if is_outer {
            if outer.replace((s, e)).is_some() {
                return None;
            }
        } else if inner.replace((s, e)).is_some() {
            return None;
        }
    }
    let (os, oe) = outer?;
    let (is_, ie) = inner?;

    let centroid = |s: usize, e: usize| -> Point3 {
        let mut c = Vector3::ZERO;
        for p in &vertices[s..e] {
            c = c + Vector3::new(p.x, p.y, p.z);
        }
        let c = c * (1.0 / (e - s) as f64);
        Point3::new(c.x, c.y, c.z)
    };
    // Mean radius if the ring is circular (max deviation < 2% of mean), else None.
    let circular = |s: usize, e: usize, c: Point3| -> Option<f64> {
        let pts = &vertices[s..e];
        let rs: Vec<f64> = pts.iter().map(|p| (*p - c).magnitude()).collect();
        let mean = rs.iter().sum::<f64>() / rs.len() as f64;
        if mean < 1e-9 {
            return None;
        }
        let maxdev = rs.iter().map(|r| (r - mean).abs()).fold(0.0_f64, f64::max);
        if maxdev > mean * 0.02 {
            return None;
        }
        // Reject SPARSE polygons whose corners merely happen to be equidistant
        // from the centroid — the classic trap is a rectangle/square cap, whose
        // 4 corners pass the all-equidistant test above yet are NOT a circle.
        // A genuinely circular tessellated ring has every consecutive chord well
        // below its radius (chord = 2·r·sin(π/n) < r for n ≥ 7); a 4-corner
        // square's side (its chord) EXCEEDS its corner-radius (80 > 56.57), so it
        // fails here and falls through to the general CDT — which triangulates a
        // square-outer + circular-hole annulus correctly. Without this guard the
        // radial-strip mis-stitched the 4 square corners to the bore ring and
        // over-covered the cap (area 8320 vs 5948 — the bored-plate-volume bug).
        let n = pts.len();
        let max_chord = (0..n)
            .map(|i| (pts[(i + 1) % n] - pts[i]).magnitude())
            .fold(0.0_f64, f64::max);
        if max_chord > mean {
            return None;
        }
        Some(mean)
    };
    let oc = centroid(os, oe);
    let ic = centroid(is_, ie);
    let or = circular(os, oe, oc)?;
    let ir = circular(is_, ie, ic)?;
    if (oc - ic).magnitude() > or.min(ir) * 0.02 || ir >= or {
        return None; // not concentric, or inner is not the smaller ring
    }

    // Stitch the two rings by ANGLE, not by raw loop index. The previous
    // index-walk assumed both rings were sampled CCW from a COMMON seam — true
    // for a revolve washer (both circles built together) but NOT for a
    // boolean-result annular cap (e.g. a bored boss top), whose outer rim and
    // inner bore have independent seams and can wind oppositely. That mismatch
    // twisted the strip into overlapping spanning triangles (the bore filled →
    // a coaxial bore through a boss rendered solid; mesh area 5484 vs the true
    // annulus 2591). FIX: reorder each ring into canonical CCW-by-angle order
    // about its own centre (kills the winding/seam dependence), then rotate the
    // inner ring so its first point is angularly aligned with the outer's first
    // point. The fractional-advance walk below is then correct for both.
    let (u_axis, v_axis) = compute_plane_axes(normal);
    let angle_of = |vi: usize, c: Point3| -> f64 {
        let d = vertices[vi] - c;
        d.dot(&v_axis).atan2(d.dot(&u_axis))
    };
    let mut o_order: Vec<usize> = (os..oe).collect();
    o_order.sort_by(|&x, &y| angle_of(x, oc).total_cmp(&angle_of(y, oc)));
    let mut i_order: Vec<usize> = (is_..ie).collect();
    i_order.sort_by(|&x, &y| angle_of(x, ic).total_cmp(&angle_of(y, ic)));
    let o0 = angle_of(o_order[0], oc);
    let ang_dist = |a: f64, b: f64| -> f64 {
        let mut d = (a - b).abs();
        if d > std::f64::consts::PI {
            d = std::f64::consts::TAU - d;
        }
        d
    };
    let start_j = (0..i_order.len())
        .min_by(|&x, &y| {
            ang_dist(angle_of(i_order[x], ic), o0)
                .total_cmp(&ang_dist(angle_of(i_order[y], ic), o0))
        })
        .unwrap_or(0);
    i_order.rotate_left(start_j);
    let n = o_order.len();
    let m = i_order.len();
    let oidx = |k: usize| o_order[k % n];
    let iidx = |k: usize| i_order[k % m];
    let mut tris: Vec<[usize; 3]> = Vec::with_capacity(n + m);
    let (mut i, mut j) = (0usize, 0usize);
    for _ in 0..(n + m) {
        let advance_outer = if i == n {
            false
        } else if j == m {
            true
        } else {
            (i as f64 / n as f64) <= (j as f64 / m as f64)
        };
        let (a, b, c) = if advance_outer {
            let t = (oidx(i), oidx(i + 1), iidx(j));
            i += 1;
            t
        } else {
            let t = (oidx(i), iidx(j + 1), iidx(j));
            j += 1;
            t
        };
        let gn = (vertices[b] - vertices[a]).cross(&(vertices[c] - vertices[a]));
        if gn.dot(normal) >= 0.0 {
            tris.push([a, b, c]);
        } else {
            tris.push([a, c, b]);
        }
    }
    Some(tris)
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

    // Cap apex side = which hemisphere this face covers. Prefer the boolean's
    // recorded hint (the kept cap's interior/apex point) — authoritative, and
    // robust where the geometric (c_center-o)·c_axis test is DEGENERATE: a great
    // circle (sphere centre on the cut plane, e.g. a box-face poke) gives h=0, and
    // for the large/near-centre cap the c_center side is the wrong hemisphere.
    // Falls back to the c_center test for non-boolean sphere caps (no hint).
    let a_dir = if let Some(hint) = model.cap_apex_hint.get(&face.id) {
        if (*hint.value() - o).dot(&c_axis) >= 0.0 {
            c_axis
        } else {
            -c_axis
        }
    } else if (c_center - o).dot(&c_axis) >= 0.0 {
        c_axis
    } else {
        -c_axis
    };
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

    // Self-correcting triangle winding: keep the candidate "forward" order iff its
    // geometric (cross-product) normal already agrees with the radial vertex
    // normals (× osign); else flip. Robust for any apex side / orientation — and
    // critically, it CANNOT desync from a_dir the way an h-keyed flip does. (The
    // old `is_forward() ^ (h<0)` flipped on h, but a hint that moves a_dir at a
    // great circle leaves h=0 unchanged, so the cap meshed inward and its flux
    // cancelled the opposing cap to 0.)
    let forward = {
        let p_i = rim[0];
        let p_j = rim[1 % m];
        let p_b = if rings >= 2 {
            o + slerp_unit(dirs[0], a_dir, 1.0 / rings as f64) * r
        } else {
            apex
        };
        let g = (p_j - p_i).cross(&(p_b - p_i));
        let desired = (p_i - o) * osign;
        g.dot(&desired) >= 0.0
    };
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

/// Apex-fan tessellation of a small triangular corner-cap NURBS patch
/// (CF-γ.7, the 1C2F mixed corner cap). The patch is a degenerate-
/// column rational bi-quadratic whose three rims (two fillet arcs + one
/// chamfer chord) meet at a single collapsed apex; curved-CDT's
/// `closest_point` UV inversion is singular at that apex and rejects the
/// patch (`PointOnFixedEdge`). This path sidesteps the inversion:
///
/// * the rim comes VERBATIM from the [`EdgeSampleCache`]
///   ([`loop_rim_samples`]), so it welds bit-exactly to the fillet /
///   bevel faces that share those rim edges;
/// * interior triangles fan each consecutive rim pair to the patch
///   apex (the [`BRepModel::cap_apex_hint`] for this face), lifted onto
///   the cap surface so the mesh follows curvature.
///
/// The cap is star-shaped from its apex (a small convex corner patch),
/// so the fan is crack-free and self-intersection-free. Returns `false`
/// (so the caller falls through to curved-CDT) when the face has no apex
/// hint or its rim cannot be sampled — i.e. for every NURBS face that is
/// not a CF-γ.7 corner cap.
/// Curvature-following radial **ladder** mesh of the 1C2F mixed-corner cap
/// (BUG 1b-5, R1). The now-injective cap patch (the 1b-4 de-fold removed the
/// `a ≡ c` self-fold) is bounded by a straight chamfer chord (the "base",
/// two verbatim edge-cache samples) and two equal-length fillet-arc rims that
/// climb from the two base corners to the shared apex. A single flat fan from
/// the apex chords across ~60° of arc curvature in ONE radial step (~55°
/// facet-normal deviation — 118/125 facets over the 40° mesh gate); this lays
/// down iso-level "rungs" between corresponding samples of the two arcs, so
/// the radial resolution follows the arcs exactly and every facet sits on the
/// surface (measured < ~20°).
///
/// Every boundary vertex is the VERBATIM edge-cache sample (base endpoints,
/// both full arcs, apex), so the cap still welds bit-exactly to its fillet /
/// chamfer neighbours — the whole-solid cert stays watertight. Winding is
/// self-correcting against the outward face normal, and per-vertex normals
/// accumulate the oriented facet normals so the stored-normal oracle agrees.
///
/// Returns `false` (caller falls back to the flat fan, preserving behaviour
/// for every other cap kind) unless the loop is the exact CF-γ.7 shape: one
/// straight 2-sample base edge and two apex-incident arc edges of equal
/// sample count.
fn emit_cap_ladder(
    lp: &crate::primitives::r#loop::Loop,
    model: &BRepModel,
    cache: &EdgeSampleCache,
    apex: Point3,
    n_face: Vector3,
    mesh: &mut TriangleMesh,
) -> bool {
    // Weld tolerance for matching a sample to the apex / a base corner.
    const WELD: f64 = 1e-6;

    // Ordered samples of loop edge `idx` (respecting the loop's directed use),
    // consecutive duplicates removed.
    let edge_samples = |idx: usize| -> Vec<Point3> {
        let eid = lp.edges[idx];
        let s = cache.get_or_compute(eid, model);
        let fwd = lp.orientations.get(idx).copied().unwrap_or(true);
        let mut out: Vec<Point3> = Vec::with_capacity(s.len());
        let mut push = |out: &mut Vec<Point3>, p: Point3| {
            if out.last().map_or(true, |&q| (q - p).magnitude() > 1e-9) {
                out.push(p);
            }
        };
        if fwd {
            for &p in s.iter() {
                push(&mut out, p);
            }
        } else {
            for &p in s.iter().rev() {
                push(&mut out, p);
            }
        }
        out
    };

    let is_apex = |p: Point3| (p - apex).magnitude() < WELD;

    // Classify: exactly one base (neither endpoint the apex) and two arcs
    // (one endpoint the apex).
    let mut base: Option<Vec<Point3>> = None;
    let mut arcs: Vec<Vec<Point3>> = Vec::new();
    for i in 0..lp.edges.len() {
        let s = edge_samples(i);
        if s.len() < 2 {
            return false;
        }
        let touches_apex = is_apex(s[0]) || is_apex(s[s.len() - 1]);
        if touches_apex {
            arcs.push(s);
        } else if base.is_none() {
            base = Some(s);
        } else {
            return false; // two non-apex edges — not the CF-γ.7 shape
        }
    }
    let base = match base {
        Some(b) => b,
        None => return false,
    };
    if arcs.len() != 2 || base.len() != 2 {
        return false;
    }

    // Orient each arc corner -> apex.
    for a in arcs.iter_mut() {
        if is_apex(a[0]) {
            a.reverse();
        }
        match a.last() {
            Some(&last) if is_apex(last) => {}
            _ => return false, // an arc that does not actually reach the apex
        }
    }
    let mut arc_b = arcs.remove(1);
    let mut arc_a = arcs.remove(0);
    // Equal counts give a clean rung pairing (the two equal-radius fillet arcs
    // are cache-sampled identically); anything else falls back.
    if arc_a.len() != arc_b.len() || arc_a.len() < 2 {
        return false;
    }
    // Align arc_a to base corner 0.
    if (arc_a[0] - base[0]).magnitude() > WELD {
        std::mem::swap(&mut arc_a, &mut arc_b);
    }
    if (arc_a[0] - base[0]).magnitude() > WELD || (arc_b[0] - base[1]).magnitude() > WELD {
        return false; // arcs do not meet the base corners as expected
    }

    let r = arc_a.len(); // rings 0..=r-1; ring r-1 is the (shared) apex
                         // Rings 0..=r-2 carry two distinct vertices; the apex is one shared vertex.
    let apex_local = 2 * (r - 1);
    let n_verts = apex_local + 1;
    let mut positions: Vec<Point3> = Vec::with_capacity(n_verts);
    for s in 0..r - 1 {
        positions.push(arc_a[s]);
        positions.push(arc_b[s]);
    }
    positions.push(apex);
    let ia = |s: usize| 2 * s; // arc_a vertex of ring s
    let ib = |s: usize| 2 * s + 1; // arc_b vertex of ring s

    let area_ok = |p0: Point3, p1: Point3, p2: Point3| -> bool {
        (p1 - p0).cross(&(p2 - p0)).magnitude() > 1e-12
    };
    let mut tris: Vec<[usize; 3]> = Vec::new();
    // Quad strips between consecutive non-apex rings.
    for s in 0..r.saturating_sub(2) {
        let (a0, b0, a1, b1) = (ia(s), ib(s), ia(s + 1), ib(s + 1));
        if area_ok(positions[a0], positions[b0], positions[b1]) {
            tris.push([a0, b0, b1]);
        }
        if area_ok(positions[a0], positions[b1], positions[a1]) {
            tris.push([a0, b1, a1]);
        }
    }
    // Apex cap: the last non-apex ring -> apex.
    {
        let s = r - 2;
        let (a0, b0) = (ia(s), ib(s));
        if area_ok(positions[a0], positions[b0], positions[apex_local]) {
            tris.push([a0, b0, apex_local]);
        }
    }
    if tris.is_empty() {
        return false;
    }

    // Orient the whole ladder outward: flip every facet if the summed
    // geometric normal opposes the outward face normal.
    let mut sum = Vector3::ZERO;
    for t in &tris {
        sum = sum + (positions[t[1]] - positions[t[0]]).cross(&(positions[t[2]] - positions[t[0]]));
    }
    if sum.dot(&n_face) < 0.0 {
        for t in tris.iter_mut() {
            t.swap(1, 2);
        }
    }

    // Per-vertex normals: accumulate the oriented facet normals so the
    // stored-normal-agreement oracle sees winding-consistent normals.
    let mut accum: Vec<Vector3> = vec![Vector3::ZERO; n_verts];
    for t in &tris {
        let gn = (positions[t[1]] - positions[t[0]]).cross(&(positions[t[2]] - positions[t[0]]));
        accum[t[0]] = accum[t[0]] + gn;
        accum[t[1]] = accum[t[1]] + gn;
        accum[t[2]] = accum[t[2]] + gn;
    }

    let base_idx = mesh.vertices.len() as u32;
    for i in 0..n_verts {
        mesh.vertices.push(MeshVertex {
            position: positions[i],
            normal: accum[i].normalize().unwrap_or(n_face),
            uv: None,
        });
    }
    for t in tris {
        mesh.triangles.push([
            base_idx + t[0] as u32,
            base_idx + t[1] as u32,
            base_idx + t[2] as u32,
        ]);
    }
    true
}

fn tessellate_corner_cap_apex_fan(
    face: &Face,
    model: &BRepModel,
    surface: &dyn Surface,
    cache: &EdgeSampleCache,
    mesh: &mut TriangleMesh,
) -> bool {
    if !face.inner_loops.is_empty() {
        return false;
    }
    let Some(hint) = model.cap_apex_hint.get(&face.id) else {
        return false;
    };
    let apex = *hint.value();
    let Some(lp) = model.loops.get(face.outer_loop) else {
        return false;
    };
    // Only the 3-rim corner cap routes here (2 fillet arcs + 1 chamfer
    // chord). A larger / different loop is not a CF-γ.7 cap.
    if lp.edges.len() != 3 {
        return false;
    }
    // Outward face normal: the patch's parametric normal at the centre,
    // signed by the face orientation. Used to wind every facet consistently
    // outward.
    let ((u0, u1), (v0, v1)) = surface.parameter_bounds();
    let n_face = surface
        .normal_at(0.5 * (u0 + u1), 0.5 * (v0 + v1))
        .ok()
        .map(|n| n * face.orientation.sign())
        .unwrap_or(Vector3::Z);

    // Preferred path: a curvature-following radial LADDER between the two
    // fillet-arc rims (see `emit_cap_ladder`), which resolves the arc
    // curvature the flat fan below chords across. Falls through to the flat
    // apex fan for any cap that is not the exact CF-γ.7 shape.
    if emit_cap_ladder(lp, model, cache, apex, n_face, mesh) {
        return true;
    }

    // Fallback: flat apex fan.
    let rim = loop_rim_samples(lp, model, cache);
    let m = rim.len();
    if m < 3 {
        return false;
    }

    // Wind ALL fan triangles together by the rim's net orientation (not
    // per-triangle, which can flip individual slivers and desynchronise
    // the shared-edge directed-edge parity). The Newell normal of the
    // rim ring gives the loop's geometric facing; if it opposes the
    // outward face normal, the whole fan is emitted reversed.
    let mut newell = Vector3::ZERO;
    for i in 0..m {
        let a = rim[i];
        let b = rim[(i + 1) % m];
        newell.x += (a.y - b.y) * (a.z + b.z);
        newell.y += (a.z - b.z) * (a.x + b.x);
        newell.z += (a.x - b.x) * (a.y + b.y);
    }
    let forward = newell.dot(&n_face) >= 0.0;

    // First pass: build the triangle index list (winding all triangles
    // consistently by the net rim orientation) and accumulate each
    // triangle's outward geometric normal onto its vertices so the
    // per-vertex stored normals AGREE with the winding (the curved
    // patch's own surface normal near the collapsed apex is unreliable,
    // and the tess-quality oracle requires `normal_agreement == 1`).
    let mut accum: Vec<Vector3> = vec![Vector3::ZERO; m + 1]; // rim[0..m] + apex
    let mut tri_local: Vec<[usize; 3]> = Vec::with_capacity(m);
    for i in 0..m {
        let pa = rim[i];
        let pb = rim[(i + 1) % m];
        // Skip a degenerate fan triangle (rim pair coincident with each
        // other or with the apex) — a zero-area facet fails tess-quality.
        if (pb - pa).magnitude() < 1e-9
            || (apex - pa).magnitude() < 1e-9
            || (pb - pa).cross(&(apex - pa)).magnitude() < 1e-12
        {
            continue;
        }
        let (i0, i1, i2) = if forward {
            (i, (i + 1) % m, m)
        } else {
            ((i + 1) % m, i, m)
        };
        let p0 = if i0 == m { apex } else { rim[i0] };
        let p1 = if i1 == m { apex } else { rim[i1] };
        let p2 = if i2 == m { apex } else { rim[i2] };
        let gn = (p1 - p0).cross(&(p2 - p0));
        accum[i0] = accum[i0] + gn;
        accum[i1] = accum[i1] + gn;
        accum[i2] = accum[i2] + gn;
        tri_local.push([i0, i1, i2]);
    }

    let base = mesh.vertices.len() as u32;
    for i in 0..m {
        mesh.vertices.push(MeshVertex {
            position: rim[i],
            normal: accum[i].normalize().unwrap_or(n_face),
            uv: None,
        });
    }
    mesh.vertices.push(MeshVertex {
        position: apex,
        normal: accum[m].normalize().unwrap_or(n_face),
        uv: None,
    });

    for t in tri_local {
        mesh.triangles
            .push([base + t[0] as u32, base + t[1] as u32, base + t[2] as u32]);
    }
    true
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
        let outward = |c: Point3| (c - o) * osign;
        stitch_rings(&b, &rim, &outward, mesh);
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
    outward_at: &dyn Fn(Point3) -> Vector3,
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

    // Collect the strip triangles in a fixed "natural" vertex order first, then
    // wind them all by ONE band-wide decision. A per-triangle geometric flip
    // (`n·outward`) is numerically unstable when the two rings are very unequal
    // in length (e.g. a torus bump: ~140 grid-boundary points vs ~700 rim
    // points) — the greedy match then emits thin sliver triangles whose normal
    // is tiny and direction-noisy, so the sign of `n·outward` is effectively
    // random and adjacent slivers disagree, leaving inconsistent directed edges.
    // The strip between two consistently-oriented rings has a single correct
    // winding, so we vote: sum the UNNORMALISED `n·outward` over every triangle
    // (slivers carry near-zero weight, well-shaped triangles decide the sign),
    // then apply that one flip uniformly. Adjacent triangles share a diagonal
    // traversed in opposite directions under the common natural order, so a
    // single flip keeps the whole strip consistent — and consistent with the
    // grid bulk and the adjoining planar faces by the manifold-orientation
    // theorem.
    let mut tris: Vec<[(Point3, u32); 3]> = Vec::with_capacity(nb + nr);
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
            tris.push([bi, b_next, rk]);
            i += 1;
        } else {
            let r_next = rim[(k0 + k + 1) % nr];
            tris.push([bi, r_next, rk]);
            k += 1;
        }
    }

    let mut vote = 0.0;
    for t in &tris {
        let centroid = Point3::new(
            (t[0].0.x + t[1].0.x + t[2].0.x) / 3.0,
            (t[0].0.y + t[1].0.y + t[2].0.y) / 3.0,
            (t[0].0.z + t[1].0.z + t[2].0.z) / 3.0,
        );
        let n = (t[1].0 - t[0].0).cross(&(t[2].0 - t[0].0));
        vote += n.dot(&outward_at(centroid));
    }
    let flip = vote < 0.0;
    for t in &tris {
        if flip {
            mesh.add_triangle(t[0].1, t[2].1, t[1].1);
        } else {
            mesh.add_triangle(t[0].1, t[1].1, t[2].1);
        }
    }
}

/// Tessellate a spherical region bounded by an OUTER arc-loop AND ≥1 cut-circle
/// inner-loop holes — the multi-component poke-through face (#88): e.g. a sphere
/// poking two opposite box faces (two small circle holes) AND a box edge (the
/// 2-arc lens whose complement is this region's outer boundary).
///
/// Neither existing path covers it: `tessellate_spherical_central` ignores the
/// outer loop entirely (it would mesh over the lens), and
/// `tessellate_spherical_large_region` requires no holes. This combines them:
/// lat-long grid, membership = (outer-plane half-spaces, signed by the boolean's
/// region-interior hint) ∧ (hole-plane half-spaces, sphere-centre side), then
/// each open grid-boundary ring is matched to its nearest rim (hole rims + the
/// outer rim, verbatim cache samples → bit-exact weld) and stitched.
///
/// The half-space ∧ membership is exact when the region is the INTERSECTION
/// side of its outer planes (true for the poke-through family, where the kept
/// region is the sphere minus per-plane protrusions). The interior hint is
/// verified against the membership before committing; any mismatch falls
/// through to the other paths.
fn tessellate_spherical_holed_region(
    face: &Face,
    model: &BRepModel,
    surface: &dyn Surface,
    cache: &EdgeSampleCache,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) -> bool {
    use crate::primitives::curve::{Arc, Circle};
    use crate::primitives::surface::Sphere;
    use std::collections::HashMap;

    if face.inner_loops.is_empty() {
        return false;
    }
    let Some(sphere) = surface.as_any().downcast_ref::<Sphere>() else {
        return false;
    };
    let Some(outer) = model.loops.get(face.outer_loop) else {
        return false;
    };
    if outer.edges.len() < 2 {
        return false;
    }
    let Some(hint) = model.cap_apex_hint.get(&face.id) else {
        return false;
    };
    let hint = *hint.value();
    let o = sphere.center;
    let r = sphere.radius;
    let osign = face.orientation.sign();

    // Collect cut planes + rim samples from EVERY loop (outer + inners),
    // treated uniformly: the boolean's region assembly may pick ANY of a
    // multi-loop region's boundary cycles as the "outer" (e.g. a z-circle
    // anti-cap as outer with the edge-straddling lens as an inner hole), so
    // the outer/inner distinction carries no geometric meaning here. The
    // face side of each distinct cut plane is the interior hint's side; the
    // region is the intersection of those half-spaces (exact for the
    // poke-through family, where the kept region is the sphere minus
    // per-plane protrusions).
    let mut planes: Vec<(Point3, Vector3, f64)> = Vec::new(); // (cc, n, sign)
    let mut rims: Vec<Vec<Point3>> = Vec::new();
    let mut loop_ids = vec![face.outer_loop];
    loop_ids.extend(face.inner_loops.iter().copied());
    for lid in loop_ids {
        let Some(lp) = model.loops.get(lid) else {
            return false;
        };
        if lp.edges.is_empty() {
            return false;
        }
        for &eid in &lp.edges {
            let Some(edge) = model.edges.get(eid) else {
                return false;
            };
            let Some(curve) = model.curves.get(edge.curve_id) else {
                return false;
            };
            let (cc, n_raw) = if let Some(ci) = curve.as_any().downcast_ref::<Circle>() {
                (ci.center(), ci.normal())
            } else if let Some(ar) = curve.as_any().downcast_ref::<Arc>() {
                (ar.center, ar.normal)
            } else {
                return false;
            };
            let Ok(n) = n_raw.normalize() else {
                return false;
            };
            let dup = planes.iter().any(|&(pc, pn, _)| {
                n.dot(&pn).abs() > 1.0 - 1e-9 && (cc - pc).dot(&pn).abs() < 1e-9
            });
            if !dup {
                let side = (hint - cc).dot(&n);
                if side.abs() < 1e-12 {
                    return false; // hint on a cut plane — ambiguous
                }
                planes.push((cc, n, side.signum()));
            }
        }
        let rim = loop_rim_samples(lp, model, cache);
        if rim.len() < 3 {
            return false;
        }
        rims.push(rim);
    }
    // Single-plane faces belong to the cap/central/large paths.
    if planes.len() < 2 {
        return false;
    }

    let in_region =
        |p: Point3| -> bool { planes.iter().all(|&(cc, n, s)| (p - cc).dot(&n) * s >= 0.0) };
    if !in_region(hint) {
        return false; // membership model contradicts the interior hint
    }

    // Lat-long grid, kept where in_region (same spine as central/large_region).
    let forward = !face.orientation.is_forward();
    let pi = std::f64::consts::PI;
    let tau = std::f64::consts::TAU;
    let n_u = arc_steps_for_quality(tau, r, params).max(12);
    let n_v = arc_steps_for_quality(pi, r, params).max(6);
    let key = |i: usize, j: usize| -> u32 { (i * (n_v + 1) + j) as u32 };
    let mut gpos = vec![vec![Point3::ORIGIN; n_v + 1]; n_u];
    let mut gid = vec![vec![None::<u32>; n_v + 1]; n_u];
    for i in 0..n_u {
        let u = tau * (i as f64) / (n_u as f64);
        for j in 0..=n_v {
            let v = pi * (j as f64) / (n_v as f64);
            let Ok(p) = surface.point_at(u, v) else {
                continue;
            };
            gpos[i][j] = p;
            if in_region(p) {
                let normal = (p - o).normalize().unwrap_or(Vector3::Z) * osign;
                gid[i][j] = Some(mesh.add_vertex(MeshVertex {
                    position: p,
                    normal,
                    uv: None,
                }));
            }
        }
    }

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

    let mut next: HashMap<u32, u32> = HashMap::new();
    for (&(a, b), &cnt) in &dir_edges {
        if cnt > 0 && !dir_edges.contains_key(&(b, a)) {
            next.insert(a, b);
        }
    }
    let decode = |k: u32| -> (u32, Point3) {
        let i = (k as usize) / (n_v + 1);
        let j = (k as usize) % (n_v + 1);
        (gid[i][j].unwrap_or(0), gpos[i][j])
    };
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

    // Match each walked grid-boundary ring to its nearest rim (mean nearest-
    // sample distance — plane-free, works for the multi-plane outer rim), and
    // stitch with the rim's centroid-direction angular frame. Each rim is
    // stitched at most once (best-matching ring wins); spurious pole-row rings
    // have huge mean distance to every rim and lose the match.
    let mean_dist = |ring: &[u32], rim: &[Point3]| -> f64 {
        let mut total = 0.0;
        for &k in ring {
            let p = decode(k).1;
            let d = rim
                .iter()
                .map(|&q| (q - p).magnitude())
                .fold(f64::INFINITY, f64::min);
            total += d;
        }
        total / (ring.len() as f64)
    };
    let wrap = |mut d: f64| {
        while d > pi {
            d -= tau;
        }
        while d < -pi {
            d += tau;
        }
        d
    };
    let mut used: Vec<bool> = vec![false; loops.len()];
    for rim_pos in &rims {
        let Some(bi) = (0..loops.len()).filter(|&i| !used[i]).min_by(|&a, &b| {
            mean_dist(&loops[a], rim_pos)
                .partial_cmp(&mean_dist(&loops[b], rim_pos))
                .unwrap_or(std::cmp::Ordering::Equal)
        }) else {
            continue;
        };
        used[bi] = true;
        let best = &loops[bi];
        let b: Vec<(Point3, u32)> = best
            .iter()
            .map(|&k| decode(k))
            .map(|(id, p)| (p, id))
            .collect();
        let rim_dirs: Vec<Vector3> = rim_pos
            .iter()
            .map(|&p| (p - o).normalize().unwrap_or(Vector3::Z))
            .collect();
        let rc = rim_dirs
            .iter()
            .fold(Vector3::ZERO, |acc, &d| acc + d)
            .normalize()
            .unwrap_or(Vector3::Z);
        let seed = if rc.x.abs() < 0.9 {
            Vector3::X
        } else {
            Vector3::Y
        };
        let e1 = (seed - rc * seed.dot(&rc))
            .normalize()
            .unwrap_or(Vector3::X);
        let e2 = rc.cross(&e1);
        let angle = |p: Point3| -> f64 {
            let d = (p - o).normalize().unwrap_or(rc);
            d.dot(&e2).atan2(d.dot(&e1))
        };
        let signed = |ring: &[(Point3, u32)]| -> f64 {
            (0..ring.len())
                .map(|i| wrap(angle(ring[(i + 1) % ring.len()].0) - angle(ring[i].0)))
                .sum()
        };
        let mut rim: Vec<(Point3, u32)> = rim_pos
            .iter()
            .zip(rim_dirs.iter())
            .map(|(&p, &d)| {
                let id = mesh.add_vertex(MeshVertex {
                    position: p,
                    normal: d * osign,
                    uv: None,
                });
                (p, id)
            })
            .collect();
        if signed(&rim) * signed(&b) < 0.0 {
            rim.reverse();
        }
        let outward = |c: Point3| (c - o) * osign;
        stitch_rings(&b, &rim, &outward, mesh);
    }
    true
}

/// Tessellate a LARGE spherical region bounded by a single outer arc-loop whose
/// rim sits near-antipodal to the region's own centroid — the "sphere minus a
/// small lens/cap" case. The driving example is box∪sphere where the sphere pokes
/// a box EDGE: the kept (outside-the-box) region is most of the sphere, ringed by
/// the two cut arcs that bound the small poked-in lens. `tessellate_spherical_-
/// polygon`'s centroid fan correctly REJECTS this — the rim is ~π from the region
/// centroid, so the geodesics from the centroid to the rim graze the hole and the
/// region is NOT star-shaped from the centroid (the centroid's antipode lies
/// inside the hole). Here we instead grid the whole sphere, keep the grid points
/// on the region side via a spherical winding-number test against the rim, and
/// stitch the grid's open boundary to the verbatim rim samples (bit-exact weld to
/// the adjoining planar box faces). Same grid+stitch spine as
/// `tessellate_spherical_central`, but the hole is a single OUTER arc-loop and the
/// membership is plane-free, so it covers non-coplanar (multi-cut) rims that
/// `loop_cut_circle` cannot describe. Returns `false` (falls through) for non-
/// sphere / holed / small / hint-less faces, so it never contends with the
/// centroid-fan or central paths that run before it.
fn tessellate_spherical_large_region(
    face: &Face,
    model: &BRepModel,
    surface: &dyn Surface,
    cache: &EdgeSampleCache,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) -> bool {
    use crate::primitives::surface::Sphere;
    use std::collections::HashMap;

    if !face.inner_loops.is_empty() {
        return false;
    }
    let Some(sphere) = surface.as_any().downcast_ref::<Sphere>() else {
        return false;
    };
    // The boolean's region-interior point both proves this is an arrangement face
    // and anchors the winding membership. Absent ⇒ not our case.
    let Some(hint) = model.cap_apex_hint.get(&face.id) else {
        return false;
    };
    let Some(lp) = model.loops.get(face.outer_loop) else {
        return false;
    };
    if lp.edges.is_empty() {
        return false;
    }
    let o = sphere.center;
    let r = sphere.radius;
    let osign = face.orientation.sign();

    let rim_pos = loop_rim_samples(lp, model, cache);
    if rim_pos.len() < 3 {
        return false;
    }
    let rim_dirs: Vec<Vector3> = rim_pos
        .iter()
        .map(|&p| (p - o).normalize().unwrap_or(Vector3::Z))
        .collect();
    let Ok(apex) = (*hint.value() - o).normalize() else {
        return false;
    };
    // Only the LARGE region: the rim must reach near-antipodal to the region
    // centroid. Otherwise the centroid fan (`tessellate_spherical_polygon`, which
    // runs first) already handles it correctly — don't duplicate or contend.
    let max_ang = rim_dirs
        .iter()
        .map(|&d| d.dot(&apex).clamp(-1.0, 1.0).acos())
        .fold(0.0_f64, f64::max);
    if max_ang < std::f64::consts::PI * 0.95 {
        return false;
    }
    // Rim centroid direction `rc` — points toward the SMALL cap (lens) the rim
    // bounds, ≈ antipodal to the region apex. The lens is a small cap around `rc`;
    // the large region is everything else.
    let rc = rim_dirs
        .iter()
        .fold(Vector3::ZERO, |acc, &d| acc + d)
        .normalize()
        .unwrap_or(-apex);

    // Spherical winding number of the rim polygon as seen from a unit direction
    // `p`: the sum of signed angles ∠(R_i, p, R_{i+1}) measured about `p`. Its
    // magnitude is ≈ 2π when the loop encircles `p` OR its antipode `−p` (the
    // tangent-plane projection cannot tell `p` from `−p`), and ≈ 0 otherwise. So
    // `|winding| > π` flags BOTH the small lens cap (around `rc`) and the antipodal
    // cap (around `−rc ≈ apex`). We disambiguate with the `rc` hemisphere: a point
    // is in the LENS iff the loop encircles it AND it sits on the rim-centroid
    // side. The large region is the complement of the lens — this is robust even
    // though the large region spans more than a hemisphere (the per-point winding
    // alone cannot classify a >hemisphere region, but the small lens it can).
    let winding = |p: Vector3| -> f64 {
        let n = rim_dirs.len();
        let mut sum = 0.0;
        for i in 0..n {
            let a = rim_dirs[i];
            let b = rim_dirs[(i + 1) % n];
            let ta = (a - p * a.dot(&p)).normalize();
            let tb = (b - p * b.dot(&p)).normalize();
            if let (Ok(ta), Ok(tb)) = (ta, tb) {
                let cross = ta.cross(&tb).dot(&p);
                let dot = ta.dot(&tb).clamp(-1.0, 1.0);
                sum += cross.atan2(dot);
            }
        }
        sum
    };
    let pi = std::f64::consts::PI;
    let in_region = |p: Vector3| -> bool { !(winding(p).abs() > pi && p.dot(&rc) > 0.0) };

    // Same lat-long grid + open-boundary extraction + rim stitch as
    // `tessellate_spherical_central`. The (+u,+v) quad winds INWARD under the
    // radial normal, so a Forward (outward) face needs the reversed winding.
    let forward = !face.orientation.is_forward();
    let tau = std::f64::consts::TAU;
    let n_u = arc_steps_for_quality(tau, r, params).max(12);
    let n_v = arc_steps_for_quality(pi, r, params).max(6);
    let key = |i: usize, j: usize| -> u32 { (i * (n_v + 1) + j) as u32 };
    let mut gpos = vec![vec![Point3::ORIGIN; n_v + 1]; n_u];
    let mut gid = vec![vec![None::<u32>; n_v + 1]; n_u];
    for i in 0..n_u {
        let u = tau * (i as f64) / (n_u as f64);
        for j in 0..=n_v {
            let v = pi * (j as f64) / (n_v as f64);
            let Ok(p) = surface.point_at(u, v) else {
                continue;
            };
            gpos[i][j] = p;
            let dir = (p - o).normalize().unwrap_or(Vector3::Z);
            if in_region(dir) {
                let normal = dir * osign;
                gid[i][j] = Some(mesh.add_vertex(MeshVertex {
                    position: p,
                    normal,
                    uv: None,
                }));
            }
        }
    }

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
    let decode = |k: u32| -> (u32, Point3) {
        let i = (k as usize) / (n_v + 1);
        let j = (k as usize) % (n_v + 1);
        (gid[i][j].unwrap_or(0), gpos[i][j])
    };
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
    // A single outer-loop hole yields ONE real boundary ring; stitch the largest
    // walked loop to the rim (a degenerate pole-row aliasing artefact, if any,
    // is a tiny loop and is dropped). Stitching only the largest avoids double-
    // covering the rim.
    // The lat-long grid collapses both poles to a point, so each pole row emits a
    // spurious zero-extent `n_u`-cycle of degenerate edges that the open-boundary
    // walk reports as a "loop". The genuine boundary is the lens hole — pick the
    // loop with the largest spatial extent (bbox diagonal); pole loops have ≈0.
    let extent = |l: &[u32]| -> f64 {
        let ps: Vec<Point3> = l.iter().map(|&k| decode(k).1).collect();
        let (mut lo, mut hi) = (ps[0], ps[0]);
        for p in &ps {
            lo = Point3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
            hi = Point3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
        }
        (hi - lo).magnitude()
    };
    let Some(best) = loops.iter().max_by(|a, b| {
        extent(a)
            .partial_cmp(&extent(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    }) else {
        // No open boundary: the region wrapped the whole sphere — let the caller
        // fall through (should not happen given max_ang ≥ 0.95π, but stay safe).
        return false;
    };
    let b: Vec<(Point3, u32)> = best
        .iter()
        .map(|&k| decode(k))
        .map(|(id, p)| (p, id))
        .collect();

    // Rim vertices (verbatim cache positions → bit-exact seam weld). Orient the
    // rim ring the SAME rotational direction as the grid boundary so `stitch_rings`
    // walks both consistently. Centre the angular frame on the rim centroid `rc`.
    let seed = if rc.x.abs() < 0.9 {
        Vector3::X
    } else {
        Vector3::Y
    };
    let e1 = (seed - rc * seed.dot(&rc))
        .normalize()
        .unwrap_or(Vector3::X);
    let e2 = rc.cross(&e1);
    let angle = |p: Point3| -> f64 {
        let d = (p - o).normalize().unwrap_or(rc);
        d.dot(&e2).atan2(d.dot(&e1))
    };
    let wrap = |mut d: f64| {
        while d > pi {
            d -= tau;
        }
        while d < -pi {
            d += tau;
        }
        d
    };
    let signed = |ring: &[(Point3, u32)]| -> f64 {
        (0..ring.len())
            .map(|i| wrap(angle(ring[(i + 1) % ring.len()].0) - angle(ring[i].0)))
            .sum()
    };
    let mut rim: Vec<(Point3, u32)> = rim_pos
        .iter()
        .zip(rim_dirs.iter())
        .map(|(&p, &d)| {
            let id = mesh.add_vertex(MeshVertex {
                position: p,
                normal: d * osign,
                uv: None,
            });
            (p, id)
        })
        .collect();
    if signed(&rim) * signed(&b) < 0.0 {
        rim.reverse();
    }
    let outward = |c: Point3| (c - o) * osign;
    stitch_rings(&b, &rim, &outward, mesh);
    true
}

/// Tessellate an arc-bounded spherical POLYGON (a face of a mutually-intersecting
/// cut-circle arrangement — the corner-poke case, where the region is a spherical
/// triangle/quad bounded by circle arcs with no inner holes).
///
/// The boundary is sampled VERBATIM from the `EdgeSampleCache` (so it welds by
/// position to the adjoining planar box faces that share those arc edges), then
/// the interior is filled by a concentric-ring fan from the region's spherical
/// centroid: ring 0 is the rim, successive rings slerp inward to the centroid,
/// and quads between rings are split into triangles wound CCW under the outward
/// radial normal. Crack-free (every ring carries the same point count until the
/// final collapse to the centroid) and curvature-following (ring vertices lie on
/// the sphere). Returns `false` for non-sphere / holed / seam-loop faces so the
/// caller falls through.
fn tessellate_spherical_polygon(
    face: &Face,
    model: &BRepModel,
    surface: &dyn Surface,
    cache: &EdgeSampleCache,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) -> bool {
    use crate::primitives::surface::Sphere;
    let Some(sphere) = surface.as_any().downcast_ref::<Sphere>() else {
        return false;
    };
    if !face.inner_loops.is_empty() {
        return false;
    }
    let Some(lp) = model.loops.get(face.outer_loop) else {
        return false;
    };
    if lp.edges.is_empty() {
        return false;
    }
    let o = sphere.center;
    let r = sphere.radius;
    let osign = face.orientation.sign();

    let rim = loop_rim_samples(lp, model, cache);
    let m = rim.len();
    if m < 3 {
        return false;
    }
    // Unit radial directions of the rim, and the spherical centroid direction.
    let mut rim = rim;
    let mut dirs: Vec<Vector3> = rim
        .iter()
        .map(|&p| (p - o).normalize().unwrap_or(Vector3::Z))
        .collect();
    let mut cdir = dirs.iter().fold(Vector3::ZERO, |a, &d| a + d);
    cdir = match cdir.normalize() {
        Ok(v) => v,
        // Degenerate (rim wraps a great circle / antipodal spread): not a simple
        // polygon — let another path handle it.
        Err(_) => return false,
    };
    // A region and its COMPLEMENT share the same rim, so the rim centroid sits on
    // the SMALL side for BOTH — fanning from it would fill the small triangle even
    // for the large complement (the 7/8 petal of a sphere-corner union). When the
    // boolean recorded the face's own interior point (its region centroid), fan
    // from THAT instead: for the complement it is the antipode −cdir, from which
    // the 7/8 region is star-shaped.
    if let Some(hint) = model.cap_apex_hint.get(&face.id) {
        if let Ok(d) = (*hint.value() - o).normalize() {
            cdir = d;
        }
    }

    // Order the rim by azimuth around the centroid so the fan always sees a
    // SIMPLE (non-self-crossing) boundary, independent of the order/handedness
    // in which the boundary arcs were concatenated into the loop. The faces
    // this path handles — boolean cut-circle arrangement cells and fillet
    // corner-octant patches — are convex / star-shaped from their centroid, so
    // azimuthal order IS their true boundary order. Without this, a corner
    // octant assembled from three cap arcs of mixed parametric handedness (the
    // `CylindricalFillet` frame_y sign flip) yielded a vertex-connected but
    // geometrically self-crossing rim that the centroid fan double-covered —
    // rendering a quarter-sphere (2× area) instead of an octant, the count of
    // affected corners varying run-to-run with HashMap order
    // (FILLET-MULTIEDGE-VOLUME). Sorting also makes the triangulation
    // deterministic. Rim 3D positions are unchanged (only reordered), so the
    // bit-exact seam weld to the neighbouring faces is preserved.
    {
        let seed = if cdir.x.abs() < 0.9 {
            Vector3::X
        } else {
            Vector3::Y
        };
        let e1 = (seed - cdir * seed.dot(&cdir))
            .normalize()
            .unwrap_or(Vector3::X);
        let e2 = cdir.cross(&e1);
        let mut order: Vec<usize> = (0..m).collect();
        order.sort_by(|&i, &j| {
            let ai = dirs[i].dot(&e2).atan2(dirs[i].dot(&e1));
            let aj = dirs[j].dot(&e2).atan2(dirs[j].dot(&e1));
            ai.partial_cmp(&aj).unwrap_or(std::cmp::Ordering::Equal)
        });
        rim = order.iter().map(|&i| rim[i]).collect();
        dirs = order.iter().map(|&i| dirs[i]).collect();
    }
    // The concentric-ring fan slerps every rim point toward the centroid, so it
    // fills any star-shaped (from the centroid) spherical polygon whose rim sits
    // strictly within the open hemisphere antipodal to NONE of it — i.e. every
    // rim point is < π from the centroid. The arrangement faces are convex
    // circle-arc polygons (hence star-shaped from their centroid), so the only
    // genuine failure is a rim point near-antipodal to the centroid (a face
    // wrapping more than a hemisphere, where the centroid direction degenerates).
    // Reject only that. The earlier π/2 cap wrongly bounced the large sphere-
    // OUTSIDE union faces to a non-conforming fallback mesher, so their arc rims
    // didn't weld to the box-corner caps and the corner detached in the mesh.
    let max_ang = dirs
        .iter()
        .map(|&d| d.dot(&cdir).clamp(-1.0, 1.0).acos())
        .fold(0.0_f64, f64::max);
    if max_ang >= std::f64::consts::PI * 0.95 {
        return false;
    }

    // slerp on the sphere from rim direction `d` toward the centroid `cdir`.
    let slerp = |d: Vector3, t: f64| -> Vector3 {
        let dot = d.dot(&cdir).clamp(-1.0, 1.0);
        let w = dot.acos();
        if w < 1e-9 {
            return cdir;
        }
        let s = w.sin();
        (d * ((1.0 - t) * w).sin() / s + cdir * (t * w).sin() / s)
            .normalize()
            .unwrap_or(cdir)
    };

    let mut n_rings = arc_steps_for_quality(max_ang, r, params).max(2);

    // NON-TERMINATION GUARD (NO-HANGS pillar). This fan emits ~`2·n_rings·m`
    // triangles. `n_rings` is chord-bounded (≤ `params.max_segments`), but `m`
    // (the rim sample count) comes from `loop_rim_samples` over the face's
    // boundary arcs and is NOT bounded here: a Boolean cut-circle arrangement
    // cell on a sphere can present a rim of THOUSANDS of samples (e.g. a sphere
    // fragment from a chained curved boolean sampled at `fine()` produced
    // m≈3450), which multiplied by `n_rings=200` rings is ~1.4M triangles for a
    // SINGLE face — the divergence-theorem mass-properties / watertight check
    // (which tessellates at `fine()`) then takes minutes, presenting to the
    // caller as a HANG. Cap the fan's total triangle yield: when `2·n_rings·m`
    // would exceed the budget, thin only the INTERIOR radial rings (`n_rings`),
    // never the rim (`ring0` is emitted verbatim from the edge cache, so the
    // weld to the adjoining faces stays bit-exact and the boundary is never
    // truncated). The interior of a star-shaped spherical cell is smooth, so
    // fewer radial rings only coarsens interior facets — geometry-preserving for
    // the rim/boundary that matters. The budget is an order of magnitude above
    // any legitimate single-face tessellation, so it fires only on the
    // pathological over-sampled-rim case, never on a real fine mesh — and the
    // poke_matrix gate (chord 0.08, `max_segments` 100) is far below it.
    //
    // Budget = 150k for DISPLAY/EXPORT quality (`default()`/`fine()`, segments
    // ≥100): a FULL `fine()` sphere is ~80k triangles and a single arrangement-
    // cell fragment is a fraction of one sphere, so 150k is ~2× the most any
    // single legitimate sphere face can need — it cannot truncate a real fine
    // mesh, yet hard-bounds the pathological m·n_rings product. This constant is
    // PRESERVED exactly for the production tessellation paths (no regression).
    //
    // For the internal coarse AUDIT path (`TessellationParams::audit()`, max
    // segments 24 — used by `mesh_self_intersects` to bound its O(n²) pair scan)
    // the budget tightens proportionally (~18k): the audit rim already samples
    // far fewer points, so the smaller cap cannot truncate a legitimate audit
    // mesh, only the pathological over-sampled-rim fragment that makes the
    // quadratic self-intersection scan explode. Gating on `max_segments >= 100`
    // keeps `default()`/`fine()` at the original 150k and only the deliberately-
    // coarse presets get the tighter cap.
    let fan_tri_budget: usize = if params.max_segments >= 100 {
        150_000
    } else {
        (750 * params.max_segments).max(12_000)
    };
    if m >= 2 && 2 * n_rings * m > fan_tri_budget {
        let capped = (fan_tri_budget / (2 * m)).max(2);
        n_rings = n_rings.min(capped);
    }
    let mk = |dir: Vector3, mesh: &mut TriangleMesh| -> u32 {
        let p = o + dir * r;
        mesh.add_vertex(MeshVertex {
            position: p,
            normal: dir * osign,
            uv: None,
        })
    };

    // Ring 0 = rim (verbatim cache positions so the weld is bit-exact).
    let ring0: Vec<u32> = rim
        .iter()
        .zip(dirs.iter())
        .map(|(&p, &d)| {
            mesh.add_vertex(MeshVertex {
                position: p,
                normal: d * osign,
                uv: None,
            })
        })
        .collect();
    let mut prev = ring0;

    // Wind each triangle by its GEOMETRIC normal rather than `osign` alone.
    // The fan winding also depends on the rim's chirality (the loop edge order
    // / orientations), which is NOT guaranteed consistent across faces: a
    // fillet corner-octant loop is assembled from three cap arcs in an order
    // that varies, so winding on `osign` alone flipped ~half the corner patches
    // inward and made the all-edges fillet's spherical volume cancel to ~0
    // (non-deterministically, via sub-ulp drift near the orientation threshold).
    // The sphere's outward normal at any point is radial (`pos − centre`), so
    // emit each triangle with its geometric normal aligned to `radial · osign`
    // (the face's oriented outward normal) — robust to loop chirality.
    let tri = |a: u32, b: u32, c: u32, mesh: &mut TriangleMesh| {
        let pa = mesh.vertices[a as usize].position;
        let pb = mesh.vertices[b as usize].position;
        let pc = mesh.vertices[c as usize].position;
        let gnormal = (pb - pa).cross(&(pc - pa));
        let radial = (pa + pb + pc) / 3.0 - o;
        if gnormal.dot(&radial) * osign >= 0.0 {
            mesh.add_triangle(a, b, c);
        } else {
            mesh.add_triangle(a, c, b);
        }
    };

    for j in 1..n_rings {
        let t = j as f64 / n_rings as f64;
        let cur_dirs: Vec<Vector3> = dirs.iter().map(|&d| slerp(d, t)).collect();
        let cur: Vec<u32> = cur_dirs.iter().map(|&d| mk(d, mesh)).collect();
        for i in 0..m {
            let i2 = (i + 1) % m;
            // Quad (prev[i], prev[i2], cur[i2], cur[i]) → two triangles, outward.
            tri(prev[i], prev[i2], cur[i], mesh);
            tri(prev[i2], cur[i2], cur[i], mesh);
        }
        prev = cur;
    }
    // Cap the innermost ring to the centroid.
    let apex = mk(cdir, mesh);
    for i in 0..m {
        let i2 = (i + 1) % m;
        tri(prev[i], prev[i2], apex, mesh);
    }
    true
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
    // Outer arc-loop + cut-circle holes (#88 multi-component poke-through):
    // must run before `central`, which ignores the outer loop and would mesh
    // over the lens region the outer boundary excludes.
    if tessellate_spherical_holed_region(face, model, surface, _cache, params, mesh) {
        return;
    }
    // Multi-hole central region: sphere minus N caps, grid + boundary-loop
    // stitch to each hole's rim.
    if tessellate_spherical_central(face, model, surface, _cache, params, mesh) {
        return;
    }
    // Arc-bounded spherical polygon (a face of a mutually-intersecting cut-circle
    // arrangement — e.g. a corner-poke spherical triangle).
    if tessellate_spherical_polygon(face, model, surface, _cache, params, mesh) {
        return;
    }
    // Large region bounded by a single small outer arc-loop (sphere minus a small
    // lens — the edge-poke union complement). The centroid fan above rejects it
    // (rim near-antipodal to the centroid); grid + winding membership + rim stitch.
    if tessellate_spherical_large_region(face, model, surface, _cache, params, mesh) {
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
/// Boundary-conforming tessellation of a CONE lateral band produced by a
/// curved Boolean. One circular boundary loop ⇒ an apex tip cap (fan
/// apex→rim); two ⇒ a frustum band (the cone is ruled, so a direct stitch of
/// the two rim circles is exact). Rims are the cut-circle `EdgeSampleCache`
/// samples, so they weld to the adjoining planar box caps. Returns `true` when
/// handled (a cone face whose every boundary loop is a single circle).
fn tessellate_conical_cut(
    face: &Face,
    model: &BRepModel,
    surface: &dyn Surface,
    cache: &EdgeSampleCache,
    mesh: &mut TriangleMesh,
) -> bool {
    use crate::primitives::surface::Cone;
    let Some(cone) = surface.as_any().downcast_ref::<Cone>() else {
        return false;
    };
    let apex = cone.apex;
    let axis = cone.axis;
    let osign = face.orientation.sign();
    let outward = |c: Point3| {
        let w = c - apex;
        (w - axis * w.dot(&axis)) * osign
    };
    let vnorm = |p: Point3| outward(p).normalize().unwrap_or(axis);

    let mut rims: Vec<Vec<Point3>> = Vec::new();
    if let Some(l) = model.loops.get(face.outer_loop) {
        if !l.edges.is_empty() {
            if loop_cut_circle(l, model).is_none() {
                return false;
            }
            let r = loop_rim_samples(l, model, cache);
            if r.len() >= 3 {
                rims.push(r);
            }
        }
    }
    for &il in &face.inner_loops {
        let Some(l) = model.loops.get(il) else {
            return false;
        };
        if loop_cut_circle(l, model).is_none() {
            return false;
        }
        let r = loop_rim_samples(l, model, cache);
        if r.len() >= 3 {
            rims.push(r);
        }
    }

    match rims.len() {
        1 => {
            let rim = &rims[0];
            let apex_id = mesh.add_vertex(MeshVertex {
                position: apex,
                normal: axis * -osign,
                uv: None,
            });
            let ids: Vec<u32> = rim
                .iter()
                .map(|&p| {
                    mesh.add_vertex(MeshVertex {
                        position: p,
                        normal: vnorm(p),
                        uv: None,
                    })
                })
                .collect();
            let m = rim.len();
            for i in 0..m {
                let j = (i + 1) % m;
                let cen = Point3::new(
                    (apex.x + rim[i].x + rim[j].x) / 3.0,
                    (apex.y + rim[i].y + rim[j].y) / 3.0,
                    (apex.z + rim[i].z + rim[j].z) / 3.0,
                );
                let n = (rim[i] - apex).cross(&(rim[j] - apex));
                if n.dot(&outward(cen)) >= 0.0 {
                    mesh.add_triangle(apex_id, ids[i], ids[j]);
                } else {
                    mesh.add_triangle(apex_id, ids[j], ids[i]);
                }
            }
            true
        }
        2 => {
            // Align the two rim circles to the same angular direction about the
            // axis so the greedy stitch walks them coherently.
            let helper = if axis.x.abs() <= axis.y.abs() && axis.x.abs() <= axis.z.abs() {
                Vector3::X
            } else if axis.y.abs() <= axis.z.abs() {
                Vector3::Y
            } else {
                Vector3::Z
            };
            let e1 = (helper - axis * helper.dot(&axis))
                .normalize()
                .unwrap_or(Vector3::X);
            let e2 = axis.cross(&e1);
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
            let signed = |r: &[Point3]| -> f64 {
                let ang = |p: Point3| {
                    let w = p - apex;
                    w.dot(&e2).atan2(w.dot(&e1))
                };
                (0..r.len())
                    .map(|i| wrap(ang(r[(i + 1) % r.len()]) - ang(r[i])))
                    .sum::<f64>()
            };
            let mut r1 = rims[1].clone();
            if signed(&rims[0]) * signed(&r1) < 0.0 {
                r1.reverse();
            }
            let r0v: Vec<(Point3, u32)> = rims[0]
                .iter()
                .map(|&p| {
                    (
                        p,
                        mesh.add_vertex(MeshVertex {
                            position: p,
                            normal: vnorm(p),
                            uv: None,
                        }),
                    )
                })
                .collect();
            let r1v: Vec<(Point3, u32)> = r1
                .iter()
                .map(|&p| {
                    (
                        p,
                        mesh.add_vertex(MeshVertex {
                            position: p,
                            normal: vnorm(p),
                            uv: None,
                        }),
                    )
                })
                .collect();
            stitch_rings(&r0v, &r1v, &outward, mesh);
            true
        }
        _ => false,
    }
}

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

    // Boundary-conforming path for a CHAMFER-blend cone frustum (the truncated
    // cone created by `create_closed_edge_chamfer` on a cylinder rim). Takes both
    // trim circles VERBATIM from the cache and stitches the ruled band, so the
    // blend welds bit-exactly to its cap (plane) and lateral (cylinder)
    // neighbours — the direct analogue of `tessellate_blend_torus_conforming` for
    // the fillet. Returns false for any non-frustum / boolean-cut cone, which
    // falls through to the paths below.
    if tessellate_blend_cone_conforming(face, model, surface, cache, mesh) {
        return;
    }

    // Boundary-conforming path for cone faces cut into bands by a curved
    // Boolean (apex tip cap / frustum band). Falls through for the analytic
    // grid otherwise.
    if tessellate_conical_cut(face, model, surface, cache, mesh) {
        return;
    }

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
            if std::env::var("ROSHERA_TESS_TRACE").is_ok() {
                eprintln!(
                    "[tess] FALLBACK cone face {:?}: curved_cdt {:?} -> UNTRIMMED grid \
                     (ignores boolean trim/holes; covers the bore — #24)",
                    face.id, e
                );
            }
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
/// Conforming tessellation of a TRIMMED torus face produced by a boolean rim-
/// poke — the main body (torus minus the bumps, carrying the cut ovals as inner
/// holes) or a single bump (bounded by one oval). Mirrors
/// [`tessellate_spherical_central`]: a grid over the DOUBLY-periodic (u, v)
/// domain gated by [`is_point_inside_face`] (so the oval holes are excluded),
/// periodic quads wound CCW under the torus's true outward normal, then each
/// open grid-boundary loop is stitched to the matching oval rim sampled VERBATIM
/// from the `EdgeSampleCache` — so the torus rim is bit-identical to the box-
/// wall hole's rim and welds. Returns `false` (→ caller falls back to the naive
/// grid) when the face carries no cut oval to stitch (the untrimmed full torus).
fn tessellate_toroidal_trimmed(
    face: &Face,
    model: &BRepModel,
    surface: &dyn Surface,
    cache: &EdgeSampleCache,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) -> bool {
    use crate::primitives::surface::Torus;
    use std::collections::{HashMap, HashSet};

    let Some(torus) = surface.as_any().downcast_ref::<Torus>() else {
        return false;
    };
    // Only the boolean split path sets a non-default param window / inner loops;
    // an untrimmed full torus (commutator outer loop, no inner loops, default
    // domain) has no oval to stitch — let the fast grid handle it.
    if torus.param_limits.is_some() {
        return false;
    }
    let center = torus.center;
    let axis = torus.axis;
    let big_r = torus.major_radius;
    let small_r = torus.minor_radius;
    let ref_dir = torus.ref_dir;
    let osign = face.orientation.sign();

    // Rim loops to STITCH: a bump (no inner loops) stitches its single outer
    // oval; the main body stitches its inner-loop ovals. The torus's own
    // commutator seam (the main body's OUTER loop) is internal to the periodic
    // grid — never a rim.
    let rim_loop_ids: Vec<crate::primitives::r#loop::LoopId> = if face.inner_loops.is_empty() {
        vec![face.outer_loop]
    } else {
        face.inner_loops.clone()
    };

    // True outward normal at p (away from the tube-centre circle), flipped by
    // the face orientation so a Difference cavity winds inward.
    let outward_at = move |p: Point3| -> Vector3 {
        let rel = p - center;
        let equ = rel - axis * rel.dot(&axis);
        let major_pt = center + equ.normalize().unwrap_or(ref_dir) * big_r;
        (p - major_pt).normalize().unwrap_or(axis) * osign
    };

    // Cache-sampled rim vertices added to the mesh, one ring per oval.
    let mut rims: Vec<Vec<(Point3, u32)>> = Vec::new();
    for &lid in &rim_loop_ids {
        let Some(lp) = model.loops.get(lid) else {
            return false;
        };
        let rim_pos = loop_rim_samples(lp, model, cache);
        if rim_pos.len() < 3 {
            return false;
        }
        // Reject the full-torus commutator masquerading as an "oval": its rim
        // wraps the whole tube (spans the major circle), so its xy-extent is the
        // full 2(R+r). A real oval is local (extent ≲ 2(s_max)).
        let span = |f: &dyn Fn(&Point3) -> f64| -> f64 {
            let mut lo = f64::INFINITY;
            let mut hi = f64::NEG_INFINITY;
            for p in &rim_pos {
                let x = f(p);
                lo = lo.min(x);
                hi = hi.max(x);
            }
            hi - lo
        };
        let ex = span(&|p| p.x).max(span(&|p| p.y));
        if ex > 1.8 * big_r {
            return false;
        }
        let pts: Vec<(Point3, u32)> = rim_pos
            .iter()
            .map(|&p| {
                let normal = outward_at(p);
                let id = mesh.add_vertex(MeshVertex {
                    position: p,
                    normal,
                    uv: None,
                });
                (p, id)
            })
            .collect();
        rims.push(pts);
    }
    if rims.is_empty() {
        return false;
    }

    // Grid over [0, 2π]² (both u and v periodic), gated by membership.
    let tau = std::f64::consts::TAU;
    let n_u = arc_steps_for_quality(tau, big_r + small_r, params).max(24);
    let n_v = arc_steps_for_quality(tau, small_r, params).max(12);
    // Precompute the loop UV polygons ONCE — `is_point_inside_face` reprojects
    // every loop per call, which over the grid is O(n_u·n_v · loops · samples ·
    // closest_point) and dominated tessellation time (~80 s). The membership
    // logic is identical (inside the outer loop, outside every inner hole;
    // a degenerate loop = whole domain), just hoisted.
    // Plane-side membership — exact and period-free.
    //
    // Each oval is the intersection of the torus with a FLAT cutting face, so it
    // lies in a plane that cleanly separates the small "bump" cap (on the side
    // away from the torus centre) from the body. Critically, the torus surface
    // only ever crosses one of these planes WITHIN that oval's cap, so a global
    // half-space test needs no in-polygon qualifier. Being purely 3-D it
    // sidesteps the doubly-periodic (u, v) seam/branch wrapping that made a
    // winding test either merge the four holes (single-branch miss) or punch a
    // phantom hole into the body (a multi-translate grazing a neighbour's
    // corner). The rim loops carried in `rims` are exactly these ovals: fit a
    // plane to each, orient its normal away from the torus centre (toward the
    // bump), then a BUMP face (no inner loops) keeps the cap PAST its single oval
    // wall, while the BODY face keeps points on the body side (NOT past) of EVERY
    // oval wall.
    let oval_planes: Vec<(Point3, Vector3)> = rims
        .iter()
        .filter_map(|r| {
            let pts: Vec<Point3> = r.iter().map(|&(p, _)| p).collect();
            if pts.len() < 3 {
                return None;
            }
            let np = pts.len() as f64;
            let sum = pts.iter().fold(Point3::ORIGIN, |a, &p| {
                Point3::new(a.x + p.x, a.y + p.y, a.z + p.z)
            });
            let c = Point3::new(sum.x / np, sum.y / np, sum.z / np);
            let mut n = newell_normal(&pts)?;
            if (c - center).dot(&n) < 0.0 {
                n = n * -1.0;
            }
            Some((c, n))
        })
        .collect();
    if oval_planes.is_empty() {
        return false;
    }
    let is_body = !face.inner_loops.is_empty();
    let inside = move |p: Point3| -> bool {
        if is_body {
            oval_planes.iter().all(|&(c, n)| (p - c).dot(&n) < 0.0)
        } else {
            oval_planes.iter().all(|&(c, n)| (p - c).dot(&n) >= 0.0)
        }
    };

    let key = |i: usize, j: usize| -> u32 { (i * n_v + j) as u32 };
    let mut gpos = vec![vec![Point3::ORIGIN; n_v]; n_u];
    let mut gid = vec![vec![None::<u32>; n_v]; n_u];
    for i in 0..n_u {
        let u = tau * (i as f64) / (n_u as f64);
        for j in 0..n_v {
            let v = tau * (j as f64) / (n_v as f64);
            let Ok(p) = surface.point_at(u, v) else {
                continue;
            };
            gpos[i][j] = p;
            if inside(p) {
                let normal = outward_at(p);
                gid[i][j] = Some(mesh.add_vertex(MeshVertex {
                    position: p,
                    normal,
                    uv: None,
                }));
            }
        }
    }

    // Emit periodic quads where all four corners are inside; record directed
    // (i,j)-key edges in the EMITTED winding so the open boundary (a directed
    // edge with no reverse twin) can be walked.
    let mut dir_edges: HashMap<(u32, u32), i32> = HashMap::new();
    for i in 0..n_u {
        let i2 = (i + 1) % n_u;
        for j in 0..n_v {
            let j2 = (j + 1) % n_v;
            if let (Some(a), Some(b), Some(d), Some(e)) =
                (gid[i][j], gid[i2][j], gid[i][j2], gid[i2][j2])
            {
                let (ka, kb, kd, ke) = (key(i, j), key(i2, j), key(i, j2), key(i2, j2));
                // Triangle vertices use MESH ids (`a`,`b`,…); the directed-edge
                // map uses grid KEYS (`ka`,…) — distinct index spaces. Winding is
                // FIXED, not per-triangle geometric (which was numerically noisy
                // and made the mesh disagree with itself everywhere — 14k
                // inconsistent edges): the (i,j),(i+1,j),(i,j+1) quad normal is
                // ∂u×∂v, the torus's intrinsic OUTWARD normal, so the natural CCW
                // order is outward for `osign > 0` (Union/Intersection) and is
                // flipped for the `osign < 0` Difference cavity. The stitch band
                // uses geometric winding and agrees by the manifold theorem.
                let fwd = osign > 0.0;
                let mut tri = |id: [u32; 3],
                               kk: [u32; 3],
                               de: &mut HashMap<(u32, u32), i32>,
                               m: &mut TriangleMesh| {
                    let (oid, ok) = if fwd {
                        (id, kk)
                    } else {
                        ([id[0], id[2], id[1]], [kk[0], kk[2], kk[1]])
                    };
                    m.add_triangle(oid[0], oid[1], oid[2]);
                    for w in 0..3 {
                        *de.entry((ok[w], ok[(w + 1) % 3])).or_insert(0) += 1;
                    }
                };
                tri([a, b, d], [ka, kb, kd], &mut dir_edges, mesh);
                tri([b, e, d], [kb, ke, kd], &mut dir_edges, mesh);
            }
        }
    }

    // Open boundary: directed edges with no reverse twin → next-vertex chain.
    // Iterate `dir_edges` in SORTED key order: it is a HashMap, and when a
    // boundary vertex has more than one outgoing edge (a degenerate pinch in
    // the membership grid) the last `next.insert` wins, so a per-process hash
    // order would pick a different successor — and the entire stitch-band
    // triangle EMISSION ORDER below derives from this chain. Identical triangles
    // summed in a different order gave the toroidal volume a ~1e-3 run-to-run
    // jitter even once the topology was fixed (#37). Sorting makes the chain a
    // pure function of the grid.
    let mut dir_keys: Vec<(u32, u32)> = dir_edges.keys().copied().collect();
    dir_keys.sort_unstable();
    let mut next: HashMap<u32, u32> = HashMap::new();
    for &(a, b) in &dir_keys {
        let cnt = dir_edges.get(&(a, b)).copied().unwrap_or(0);
        if cnt > 0 && !dir_edges.contains_key(&(b, a)) {
            next.insert(a, b);
        }
    }
    let decode = |k: u32| -> (Point3, u32) {
        let i = (k as usize) / n_v;
        let j = (k as usize) % n_v;
        (gpos[i][j], gid[i][j].unwrap_or(0))
    };
    // Walk the boundary loops from SORTED start vertices. `visited` is shared
    // across starts, so the discovery order decides which chain claims a vertex
    // a degenerate junction shares — a HashMap key order would partition the
    // loops differently each run. Sorted starts make the loop set deterministic.
    let mut next_starts: Vec<u32> = next.keys().copied().collect();
    next_starts.sort_unstable();
    let mut visited: HashSet<u32> = HashSet::new();
    let mut loops: Vec<Vec<u32>> = Vec::new();
    for &start in &next_starts {
        if visited.contains(&start) {
            continue;
        }
        let mut chain = Vec::new();
        let mut cur = start;
        let mut guard = 0;
        loop {
            if !visited.insert(cur) {
                break;
            }
            chain.push(cur);
            cur = match next.get(&cur) {
                Some(&n) => n,
                None => break,
            };
            guard += 1;
            if guard > n_u * n_v + 4 || cur == start {
                break;
            }
        }
        if chain.len() >= 3 {
            loops.push(chain);
        }
    }

    // Pair each grid-boundary loop with its oval rim as a BIJECTION. A per-loop
    // independent nearest-centroid pick is not injective: two boundary loops can
    // both choose the same rim, so that rim is stitched twice (its edges then
    // border the disk cap PLUS two stitch bands → non-manifold) while another
    // rim is stitched by a far-away loop, dragging a band across the torus.
    // Greedy global assignment over all (loop, rim) centroid distances, smallest
    // first, each rim used once, fixes both: every hole stitches to the rim that
    // actually bounds it.
    let centroid = |pts: &[(Point3, u32)]| -> Point3 {
        let n = pts.len().max(1) as f64;
        let s = pts.iter().fold(Point3::ORIGIN, |a, &(p, _)| {
            Point3::new(a.x + p.x, a.y + p.y, a.z + p.z)
        });
        Point3::new(s.x / n, s.y / n, s.z / n)
    };
    let bounds: Vec<Vec<(Point3, u32)>> = loops
        .iter()
        .map(|chain| chain.iter().map(|&k| decode(k)).collect())
        .collect();
    let bound_cen: Vec<Point3> = bounds.iter().map(|b| centroid(b)).collect();
    let rim_cen: Vec<Point3> = rims.iter().map(|r| centroid(r)).collect();
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for (bi, bc) in bound_cen.iter().enumerate() {
        for (ri, rc) in rim_cen.iter().enumerate() {
            pairs.push(((*rc - *bc).magnitude(), bi, ri));
        }
    }
    pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut assign: Vec<Option<usize>> = vec![None; bounds.len()];
    let mut rim_used = vec![false; rims.len()];
    for (_, bi, ri) in pairs {
        if assign[bi].is_none() && !rim_used[ri] {
            assign[bi] = Some(ri);
            rim_used[ri] = true;
        }
    }

    for (bidx, bound) in bounds.iter().enumerate() {
        let bcen = bound_cen[bidx];
        let Some(rim) = assign[bidx].and_then(|ri| rims.get(ri)) else {
            continue;
        };
        // Align rim direction to the bound: best-fit oval-plane normal, compare
        // signed angle sums; reverse the rim if opposite (greedy stitch assumes
        // both rings advance the same way).
        let plane_n = {
            let mut nrm = Vector3::ZERO;
            let n = rim.len();
            for i in 0..n {
                let a = rim[i].0 - bcen;
                let b = rim[(i + 1) % n].0 - bcen;
                nrm = nrm + a.cross(&b);
            }
            nrm.normalize().unwrap_or(axis)
        };
        let (e1, e2) = {
            let helper = if plane_n.x.abs() <= plane_n.y.abs() && plane_n.x.abs() <= plane_n.z.abs()
            {
                Vector3::X
            } else if plane_n.y.abs() <= plane_n.z.abs() {
                Vector3::Y
            } else {
                Vector3::Z
            };
            let e1 = (helper - plane_n * helper.dot(&plane_n))
                .normalize()
                .unwrap_or(Vector3::X);
            (e1, plane_n.cross(&e1))
        };
        let ang = |p: Point3| -> f64 {
            let d = p - bcen;
            d.dot(&e2).atan2(d.dot(&e1))
        };
        let wrap = |mut d: f64| {
            while d > std::f64::consts::PI {
                d -= tau;
            }
            while d < -std::f64::consts::PI {
                d += tau;
            }
            d
        };
        let signed = |ring: &[(Point3, u32)]| -> f64 {
            (0..ring.len())
                .map(|i| wrap(ang(ring[(i + 1) % ring.len()].0) - ang(ring[i].0)))
                .sum()
        };
        let mut rim_ord: Vec<(Point3, u32)> = rim.clone();
        if signed(&bound) * signed(&rim_ord) < 0.0 {
            rim_ord.reverse();
        }
        stitch_rings(&bound, &rim_ord, &outward_at, mesh);
    }
    true
}

/// Conforming tessellation of a chamfer-BLEND cone frustum (the truncated cone
/// produced by `create_closed_edge_chamfer` when a cylinder rim — outer OR bore
/// — is chamfered).
///
/// Such a blend is a RULED frustum band carrying `height_limits` and the
/// canonical seamed-rectangle boundary: a single 4-edge outer loop (lateral trim
/// circle, seam line, cap trim circle, seam line) with no inner loops. The two
/// trim circles are the v-boundaries; the seam line (appearing twice,
/// forward+backward) is the u=0≡2π meridian seam, so the patch is a closed
/// straight-walled band between the two coaxial trim circles.
///
/// The generic curved-CDT path samples that band's boundary independently of the
/// shared [`EdgeSampleCache`], so the chamfer cone's two rims do NOT coincide
/// with the neighbour cap (plane) and lateral (cylinder) tessellations at the
/// seam — leaving a ring of open boundary edges along both trim circles (the
/// bore-rim chamfer leak: 4 open edges at the u=0 seam, the direct analogue of
/// the #89 cone-rim FILLET mis-weld).
///
/// This mesher takes BOTH v-boundary rings VERBATIM from the cache (bit-identical
/// to the cap/lateral tessellation of the same edges, so they weld for ANY
/// radius) and stitches them directly. A cone frustum is ruled — zero curvature
/// along the slant — so a single direct ring stitch is exact; no interior rows
/// are needed. Returns `false` for any non-frustum cone (apex-singular cones,
/// boolean-cut cone bands with single-circle cut loops, or any blend whose rings
/// cannot be sampled) so those keep their existing dedicated meshers untouched.
fn tessellate_blend_cone_conforming(
    face: &Face,
    model: &BRepModel,
    surface: &dyn Surface,
    cache: &EdgeSampleCache,
    mesh: &mut TriangleMesh,
) -> bool {
    use crate::primitives::surface::Cone;

    let Some(cone) = surface.as_any().downcast_ref::<Cone>() else {
        return false;
    };
    // A truncated frustum (the chamfer blend) qualifies. The frustum window may
    // be declared on the surface (`height_limits`, the kernel-built blend) or
    // be IMPLICIT in the trim loop: a cone re-imported from STEP is always a
    // full `CONICAL_SURFACE` (`height_limits == None`) — ISO 10303-42 has no
    // partial conical surface, so the band lives ONLY in the face's trim loop.
    // The band is stitched from the two cached trim RINGS regardless, so the
    // declared window was never more than a gate; its absence is not
    // disqualifying. The structural gate below (4-edge seamed-rectangle loop,
    // exactly 2 closed trim rings, no inner loops) plus the iso-axial ring
    // check after ring extraction are the real discriminators: an
    // apex-singular cone has no second rim (fails the 2-ring check) and a
    // boolean-cut cone band's cut ovals are not axis-perpendicular circles
    // (fails iso-axial), so both keep their existing dedicated meshers.
    // Without this, an IMPORTED chamfer frustum fell through to curved-CDT,
    // which leaves the 4-open-edge quad hole at the u=0 seam this mesher was
    // built to fix (the audit round-trip: 4 open edges per chamfered drilled
    // rim).
    //
    // The blend is the seamed-rectangle: exactly one outer loop of 4 edges and no
    // inner holes. A boolean-trimmed cone carries cut ovals as inner loops and is
    // excluded here (it has its own conforming cut mesher).
    if !face.inner_loops.is_empty() {
        return false;
    }
    let Some(outer) = model.loops.get(face.outer_loop) else {
        return false;
    };
    if outer.edges.len() != 4 {
        return false;
    }

    // The two CLOSED edges of the loop are the trim circles (start == end
    // vertex); the OPEN edge (the seam line, present twice) is the meridian seam.
    let mut ring_edges: Vec<crate::primitives::edge::EdgeId> = Vec::new();
    for &eid in &outer.edges {
        let Some(e) = model.edges.get(eid) else {
            return false;
        };
        if e.start_vertex == e.end_vertex && !ring_edges.contains(&eid) {
            ring_edges.push(eid);
        }
    }
    if ring_edges.len() != 2 {
        return false;
    }

    // Sample each trim circle VERBATIM from the cache: these are the exact
    // polylines the neighbouring cap (plane) and lateral (cylinder) faces weld
    // onto. Both trim circles are built with `Arc::circle(center, axis, r)`,
    // whose t=0 anchors to the same +ref_dir meridian, so both rings start at the
    // shared seam vertex.
    let ring_a: Vec<Point3> = cache.get_or_compute(ring_edges[0], model).as_ref().clone();
    let ring_b: Vec<Point3> = cache.get_or_compute(ring_edges[1], model).as_ref().clone();
    // Drop the duplicated closing sample (closed-edge cache emits n+1 with
    // first == last) so each ring is a clean cycle of distinct vertices.
    let trim = |mut r: Vec<Point3>| -> Vec<Point3> {
        if r.len() >= 2 {
            if let (Some(&f), Some(&l)) = (r.first(), r.last()) {
                if (f - l).magnitude() < 1e-9 {
                    r.pop();
                }
            }
        }
        r
    };
    let ring_a = trim(ring_a);
    let ring_b = trim(ring_b);
    if ring_a.len() < 3 || ring_b.len() < 3 {
        return false;
    }

    // Disqualify anything whose two closed rings are not the frustum's trim
    // circles. A chamfer blend's rings are both axis-PERPENDICULAR circles
    // (constant axial coordinate each, at two DISTINCT heights). A cone face
    // cut by a curved boolean can also present closed loops, but those ovals
    // sweep along the axis (non-constant axial coordinate) — they must keep
    // their dedicated cut mesher. Measured in the cone's own axis frame so an
    // obliquely-placed blend is handled identically. This check replaces the
    // discrimination the kernel-only `height_limits` gate used to provide, so
    // an IMPORTED full cone trimmed by its loop is admitted while non-blend
    // topologies still fall through.
    let axial = |p: Point3| -> f64 { (p - cone.apex).dot(&cone.axis) };
    let axial_range = |ring: &[Point3]| -> (f64, f64) {
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for &p in ring {
            let a = axial(p);
            lo = lo.min(a);
            hi = hi.max(a);
        }
        (lo, hi)
    };
    let (a_lo, a_hi) = axial_range(&ring_a);
    let (b_lo, b_hi) = axial_range(&ring_b);
    let gap = ((a_lo + a_hi) * 0.5 - (b_lo + b_hi) * 0.5).abs();
    if gap < 1e-9 {
        return false;
    }
    let iso_axial_tol = (gap * 0.05).max(1e-6);
    if (a_hi - a_lo) > iso_axial_tol || (b_hi - b_lo) > iso_axial_tol {
        return false;
    }

    // Normalise BOTH cached rings to the SAME rotational sense about the cone
    // axis. The two trim circles are anchored to the same seam vertex but can be
    // built advancing OPPOSITE ways about the axis (the cap and lateral loops
    // unwrap with opposite chirality). Without re-aligning them the merge walk
    // would pair a +u ring against a −u ring and emit ring-spanning garbage.
    // Re-aligning keeps every ring CCW about the axis so the band stitches
    // cleanly; the seam vertex (index 0) is shared by both, so re-aligning never
    // breaks the weld.
    let axis = cone.axis;
    let ring_turn = |ring: &[Point3]| -> f64 {
        let mut acc = 0.0;
        for w in 0..ring.len() {
            let p0 = ring[w] - cone.apex;
            let p1 = ring[(w + 1) % ring.len()] - cone.apex;
            let r0 = p0 - axis * p0.dot(&axis);
            let r1 = p1 - axis * p1.dot(&axis);
            acc += r0.cross(&r1).dot(&axis);
        }
        acc
    };
    let to_ccw = |mut ring: Vec<Point3>| -> Vec<Point3> {
        if ring_turn(&ring) < 0.0 {
            // Reverse but keep the seam vertex (index 0) first.
            ring[1..].reverse();
        }
        ring
    };
    let ring_a = to_ccw(ring_a);
    let ring_b = to_ccw(ring_b);

    // Outward normal at a band vertex: the cone's analytic normal flipped to
    // match the face orientation. Falls back to the radial-from-axis direction if
    // the inversion fails (degenerate sample), which is the same outward sense.
    let osign = face.orientation.sign();
    let outward_normal = |p: Point3| -> Vector3 {
        surface
            .closest_point(&p, Tolerance::default())
            .and_then(|(u, v)| surface.normal_at(u, v))
            .map(|n| n * osign)
            .unwrap_or_else(|_| {
                let w = p - cone.apex;
                (w - axis * w.dot(&axis)).normalize().unwrap_or(axis) * osign
            })
    };

    let mut add_ring = |ring: &[Point3], mesh: &mut TriangleMesh| -> Vec<u32> {
        ring.iter()
            .map(|&p| {
                mesh.add_vertex(MeshVertex {
                    position: p,
                    normal: outward_normal(p),
                    uv: None,
                })
            })
            .collect()
    };

    // A cone frustum is ruled (straight in v), so the band between the two cached
    // rim rings needs no interior rows — a single direct stitch is exact. Reuse
    // the torus blend's per-facet-winding ring stitch (tolerates unequal ring
    // counts and orients each facet by its baked outward normals).
    let lower = add_ring(&ring_a, mesh);
    let upper = add_ring(&ring_b, mesh);
    stitch_blend_rings(&lower, &upper, mesh);
    true
}

/// Conforming tessellation of a fillet-BLEND torus (cylinder-rim or cone-rim
/// quarter-torus produced by `cylinder_rim_fillet` / `cone_rim_fillet`).
///
/// Such a blend carries `param_limits` (a trimmed `[0,2π] × [v0,v1]` window) and
/// the canonical seamed-rectangle boundary: a single 4-edge outer loop
/// (lateral trim circle, seam arc, cap trim circle, seam arc) with no inner
/// loops. The two trim circles are the v-boundaries; the seam arc (appearing
/// twice, forward+backward) is the periodic u=0≡2π meridian seam, so the patch
/// is a closed tube-band between the two coaxial trim circles.
///
/// The naive structured grid in [`tessellate_toroidal_face`] samples that band
/// from `surface.point_at(u,v)` at its OWN `u`-step count, which coincides with
/// the neighbour cap/cone edge samples only when the step counts happen to be
/// equal — true for a wide rim (both trim circles hit the segment ceiling →
/// equal counts), false for a narrow one (the smaller cap circle gets fewer
/// samples). The mismatch leaves the whole blend patch un-welded: a floating
/// band of boundary edges along both trim circles (the cone-rim mis-weld #89,
/// `oriented`-true but `watertight`-false).
///
/// This mesher instead takes the two v-boundary rings VERBATIM from the shared
/// [`EdgeSampleCache`] (so they are bit-identical to the cap/cone tessellation
/// of the same edges and weld for ANY radius), fills interior meridian rows
/// from the torus surface for chord fidelity, and stitches consecutive rings
/// with an angle-merge band that tolerates unequal ring counts. Returns `false`
/// for every other torus (untrimmed full torus, boolean rim-poke trimmed torus
/// with inner loops, or any blend whose rings cannot be sampled) so those keep
/// their existing dedicated meshers untouched.
fn tessellate_blend_torus_conforming(
    face: &Face,
    model: &BRepModel,
    surface: &dyn Surface,
    cache: &EdgeSampleCache,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) -> bool {
    use crate::primitives::surface::Torus;

    let Some(torus) = surface.as_any().downcast_ref::<Torus>() else {
        return false;
    };
    // A trimmed-window torus (the fillet blend) qualifies. The window may be
    // declared on the surface (`param_limits`, the kernel-built blend) or be
    // IMPLICIT in the trim loop: a torus re-imported from STEP is always a full
    // `TOROIDAL_SURFACE` (`param_limits == None`) — ISO 10303-42 has no partial
    // toroidal surface, so the v-window lives ONLY in the face's trim loop
    // (exactly how OCCT/FreeCAD also emit a fillet). Either way the band is
    // stitched from the two cached trim RINGS and the ring-derived v's (see
    // `v_of_ring` below); `param_limits` was never more than a gate plus a
    // span sanity-check, so its absence is not disqualifying. The structural
    // gate that follows (4 edges, exactly 2 closed trim rings, no inner loops)
    // is the real discriminator, so the full torus primitive and the
    // boolean rim-poke body/bump (which do not present that seamed-rectangle
    // loop) still fall through to their own structured/stitch meshers.
    //
    // The blend is the seamed-rectangle: exactly one outer loop of 4 edges and
    // no inner holes. A boolean-trimmed torus carries cut ovals as inner loops
    // and is excluded here (it has its own conforming stitch mesher).
    if !face.inner_loops.is_empty() {
        return false;
    }
    let Some(outer) = model.loops.get(face.outer_loop) else {
        return false;
    };
    if outer.edges.len() != 4 {
        return false;
    }

    // The two CLOSED edges of the loop are the trim circles (start == end
    // vertex); the OPEN edge (the seam arc, present twice) is the meridian seam.
    let mut ring_edges: Vec<crate::primitives::edge::EdgeId> = Vec::new();
    for &eid in &outer.edges {
        let Some(e) = model.edges.get(eid) else {
            return false;
        };
        if e.start_vertex == e.end_vertex && !ring_edges.contains(&eid) {
            ring_edges.push(eid);
        }
    }
    if ring_edges.len() != 2 {
        return false;
    }

    // Sample each trim circle VERBATIM from the cache: these are the exact
    // polylines the neighbouring cap (plane) and lateral (cone/cylinder) faces
    // weld onto. `get_or_compute` samples a closed circular edge over its full
    // param range starting at the curve's t=0, which the blend builders anchor
    // to the torus `ref_dir` (u = 0 seam) — so both rings start at the same
    // meridian and share the seam vertices.
    let ring_a: Vec<Point3> = cache.get_or_compute(ring_edges[0], model).as_ref().clone();
    let ring_b: Vec<Point3> = cache.get_or_compute(ring_edges[1], model).as_ref().clone();
    // Drop the duplicated closing sample (closed-edge cache emits n+1 with
    // first == last) so each ring is a clean cycle of distinct vertices.
    let trim = |mut r: Vec<Point3>| -> Vec<Point3> {
        if r.len() >= 2 {
            if let (Some(&f), Some(&l)) = (r.first(), r.last()) {
                if (f - l).magnitude() < 1e-9 {
                    r.pop();
                }
            }
        }
        r
    };
    let ring_a = trim(ring_a);
    let ring_b = trim(ring_b);
    if ring_a.len() < 3 || ring_b.len() < 3 {
        return false;
    }

    // Normalise BOTH cached rings to the SAME rotational sense about the torus
    // axis (the +u / CCW sense the interior rows are sampled in). The two trim
    // circles are anchored to the same seam but can be built advancing OPPOSITE
    // ways about the axis (`cylinder_rim_fillet` / `cone_rim_fillet` sweep the
    // lateral trim circle with −TAU and the cap trim circle with +TAU so each
    // face's own loop unwraps cleanly). Without re-aligning them the merge walk
    // would pair a +u ring against a −u ring and emit ring-spanning garbage
    // triangles. Re-aligning keeps every ring CCW so the band stitches cleanly;
    // the seam vertex (index 0) is shared by both, so re-aligning never breaks
    // the weld.
    let axis = torus.axis;
    let ring_turn = |ring: &[Point3]| -> f64 {
        // Signed turning about the axis: sum of cross-products of consecutive
        // radial vectors, projected on the axis. Sign > 0 ⇒ CCW about +axis.
        let mut acc = 0.0;
        for w in 0..ring.len() {
            let p0 = ring[w] - torus.center;
            let p1 = ring[(w + 1) % ring.len()] - torus.center;
            let r0 = p0 - axis * p0.dot(&axis);
            let r1 = p1 - axis * p1.dot(&axis);
            acc += r0.cross(&r1).dot(&axis);
        }
        acc
    };
    let to_ccw = |mut ring: Vec<Point3>| -> Vec<Point3> {
        if ring_turn(&ring) < 0.0 {
            // Reverse but keep the seam vertex (index 0) first.
            ring[1..].reverse();
        }
        ring
    };
    let ring_a = to_ccw(ring_a);
    let ring_b = to_ccw(ring_b);

    // Disqualify the genuine FULL torus. Its single face's seam loop ALSO
    // presents as four edges with two closed rings, but those rings are a
    // MAJOR (v-seam) ring and a MINOR (u-seam) ring — the minor ring lies in a
    // meridian plane CONTAINING the axis, so its axial coordinate sweeps
    // ±minor_radius. A fillet blend's two trim rings are instead both MAJOR
    // iso-v rings, each lying in a plane PERPENDICULAR to the axis (constant
    // axial coordinate). Requiring both rings to be iso-axial keeps the full
    // torus on its structured grid mesher (it was formerly excluded by the
    // `param_limits.is_some()` gate, which a re-imported blend cannot satisfy)
    // while still admitting the imported blend window. Measured in the torus's
    // own axis frame, so an obliquely-placed blend is handled identically.
    let axial_spread = |ring: &[Point3]| -> f64 {
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for &p in ring {
            let a = (p - torus.center).dot(&torus.axis);
            lo = lo.min(a);
            hi = hi.max(a);
        }
        hi - lo
    };
    let iso_axial_tol = (torus.minor_radius * 0.05).max(1e-6);
    if axial_spread(&ring_a) > iso_axial_tol || axial_spread(&ring_b) > iso_axial_tol {
        return false;
    }

    // Map each cached ring to its torus v-parameter so interior rows can be
    // sampled BETWEEN them. `closest_point` returns the signed v the blend
    // window uses (`v_straddles_seam`). Average over a few ring points for
    // robustness against per-sample noise.
    let v_of_ring = |ring: &[Point3]| -> f64 {
        let mut acc = 0.0;
        let mut n = 0.0;
        for &p in ring.iter().take(8) {
            if let Ok((_, v)) = surface.closest_point(&p, Tolerance::default()) {
                acc += v;
                n += 1.0;
            }
        }
        if n > 0.0 {
            acc / n
        } else {
            0.0
        }
    };
    let va = v_of_ring(&ring_a);
    let vb = v_of_ring(&ring_b);
    // Order rings by ascending v: row 0 = low-v boundary, last row = high-v.
    let (v_low_ring, v_high_ring, v_low, v_high) = if va <= vb {
        (&ring_a, &ring_b, va, vb)
    } else {
        (&ring_b, &ring_a, vb, va)
    };
    // Guard: the two trim rings must sit at DISTINCT v (else this is not a
    // clean blend band — a degenerate/zero-height window — and we defer to the
    // structured grid). The window is taken from the rings themselves, which is
    // exactly what the declared `param_limits` window (when present) records; a
    // re-imported full torus has no declared window, so the rings are the sole
    // source of truth for the v-band and this guard is computed from them.
    let v_span = (v_high - v_low).abs();
    if v_span < 1e-9 {
        return false;
    }

    // Number of interior meridian rows for chord fidelity across the minor
    // (tube) circle. The two boundary rows already exist; add enough interior
    // rows that the v-chord error stays within tolerance.
    let v_steps = arc_steps_for_quality((v_high - v_low).abs(), torus.minor_radius, params).max(1);

    // Build the rows. Row 0 and row `v_steps` are the cached rings (verbatim);
    // interior rows are torus-surface samples at the SEAM-anchored u-angles of
    // the larger ring so adjacent rows align in u as closely as possible (the
    // angle-merge stitch tolerates any residual count difference).
    //
    // Anchor each interior column to the ACTUAL angular position of the larger
    // boundary ring rather than to the torus's own uniform `[0, 2π)` u-grid.
    // The kernel blend builders align the torus `ref_dir` (u = 0 seam) to the
    // trim rings, so the uniform grid coincided with the rings; but a torus
    // re-imported from a STEP `TOROIDAL_SURFACE` derives its OWN `ref_dir`
    // (`axis.perpendicular()`), which need not coincide with the trim circles'
    // seam (their `AXIS2_PLACEMENT_3D` ref-direction is `$`/defaulted). On the
    // uniform grid the interior rows are then rotated relative to the boundary
    // rings — the band is internally consistent but twisted a fixed angle, so
    // it welds to the cap/lateral neighbours with the OPPOSITE winding (the
    // "oriented == false" round-trip symptom). Reading each column's u from the
    // larger ring's own vertices puts every row on one shared meridian grid, so
    // the stitch pairs aligned columns and the band winds with its neighbours
    // regardless of where the imported surface placed its seam.
    let ref_ring: &[Point3] = if v_low_ring.len() >= v_high_ring.len() {
        v_low_ring
    } else {
        v_high_ring
    };
    let col_u: Vec<f64> = ref_ring
        .iter()
        .map(|p| {
            surface
                .closest_point(p, Tolerance::default())
                .map(|(u, _)| u)
                .unwrap_or(0.0)
        })
        .collect();
    let interior_cols = col_u.len();
    let outward_normal = |p: Point3| -> Vector3 {
        surface
            .closest_point(&p, Tolerance::default())
            .and_then(|(u, v)| surface.normal_at(u, v))
            .map(|n| n * face.orientation.sign())
            .unwrap_or(torus.axis)
    };

    // Add a ring of vertices, return their mesh indices in ring order.
    let mut add_ring = |ring: &[Point3], mesh: &mut TriangleMesh| -> Vec<u32> {
        ring.iter()
            .map(|&p| {
                mesh.add_vertex(MeshVertex {
                    position: p,
                    normal: outward_normal(p),
                    uv: None,
                })
            })
            .collect()
    };

    let mut rows: Vec<Vec<u32>> = Vec::with_capacity(v_steps + 1);
    rows.push(add_ring(v_low_ring, mesh));
    for k in 1..v_steps {
        let v = v_low + (v_high - v_low) * (k as f64) / (v_steps as f64);
        let mut row_pts: Vec<Point3> = Vec::with_capacity(interior_cols);
        for &u in &col_u {
            match surface.point_at(u, v) {
                Ok(p) => row_pts.push(p),
                Err(_) => return false,
            }
        }
        rows.push(add_ring(&row_pts, mesh));
    }
    rows.push(add_ring(v_high_ring, mesh));

    // Stitch consecutive rings. Both rings are closed cycles starting at the
    // u = 0 seam and advancing in the SAME angular direction (cache forward
    // order == curve param order == +u). A normalized-parameter merge walks
    // both rings monotonically, emitting a triangle off whichever ring is
    // "behind" in normalized position — the standard unequal-count tube stitch,
    // crack-free because every ring vertex is consumed exactly once.
    for w in 0..rows.len() - 1 {
        stitch_blend_rings(&rows[w], &rows[w + 1], mesh);
    }
    true
}

/// Stitch two closed vertex rings (a tube band) into triangles, tolerating
/// unequal ring lengths. Both rings start at the same seam and advance in the
/// same angular direction; the walk advances whichever ring is behind in
/// normalized [0,1) position, so every vertex of both rings is used and the
/// band is crack-free.
///
/// Each triangle's winding is set per-facet by agreement with its vertices'
/// outward normals (already baked when the rings were added: `surface.normal_at
/// × face.orientation.sign()`). This makes the band's facet orientation match
/// the face's oriented outward direction unconditionally — independent of the
/// merge-walk traversal sense — so it is consistently wound with the cap and
/// lateral neighbours that share its two boundary rings.
fn stitch_blend_rings(lower: &[u32], upper: &[u32], mesh: &mut TriangleMesh) {
    let nl = lower.len();
    let nu = upper.len();
    if nl == 0 || nu == 0 {
        return;
    }
    // Per-facet winding by agreement with the baked outward vertex normals:
    // each triangle is emitted with the winding whose geometric normal has a
    // non-negative dot with the mean of its three vertices' (outward) normals.
    // This makes the band's facet orientation match the face's oriented outward
    // direction unconditionally, so it is consistently wound with the cap and
    // lateral neighbours that share its two boundary rings — independent of the
    // rings' rotational sense or the merge-walk traversal order.
    let mut emit = |a: u32, b: u32, c: u32, mesh: &mut TriangleMesh| {
        if a == b || b == c || c == a {
            return;
        }
        let (pa, pb, pc) = (
            mesh.vertices[a as usize].position,
            mesh.vertices[b as usize].position,
            mesh.vertices[c as usize].position,
        );
        let geo = (pb - pa).cross(&(pc - pa));
        let outward = mesh.vertices[a as usize].normal
            + mesh.vertices[b as usize].normal
            + mesh.vertices[c as usize].normal;
        if geo.dot(&outward) >= 0.0 {
            mesh.add_triangle(a, b, c);
        } else {
            mesh.add_triangle(a, c, b);
        }
    };
    let mut il = 0usize;
    let mut iu = 0usize;
    // Walk until both rings have been fully traversed (each index reaches its
    // length). Normalized position of the NEXT boundary on each ring decides
    // which side advances.
    while il < nl || iu < nu {
        let pl = il as f64 / nl as f64;
        let pu = iu as f64 / nu as f64;
        let lower_behind = if il >= nl {
            false
        } else if iu >= nu {
            true
        } else {
            pl <= pu
        };
        if lower_behind {
            // Advance the lower ring: triangle (L[il], L[il+1], U[iu]).
            let a = lower[il % nl];
            let b = lower[(il + 1) % nl];
            let c = upper[iu % nu];
            emit(a, b, c, mesh);
            il += 1;
        } else {
            // Advance the upper ring: triangle (L[il], U[iu+1], U[iu]). Same
            // (lower→upper) traversal sense as the lower-advance case, so both
            // triangle kinds carry one consistent winding across the band.
            let a = lower[il % nl];
            let b = upper[(iu + 1) % nu];
            let c = upper[iu % nu];
            emit(a, b, c, mesh);
            iu += 1;
        }
    }
}

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

    // 3-EDGE APEX WEDGE (REVOLVE-POLE part 2): the band touching the pole is a
    // TRIANGLE — two meridian arcs meeting at the single pole vertex (r≈0 on the
    // axis) plus one rim arc. The 4-edge Coons / (u,v)-param paths below need a
    // quad, so the apex band used to fall through to curved-CDT, which left it
    // UNMESHED (~147 open edges at the pole — a leaky dome). Mesh it directly:
    // collect the 3 edge cache-sample chains, lay them out as a non-degenerate
    // triangle in (u,v) — positions stay the EXACT 3D cache samples so the
    // meridian/rim seams match the neighbouring wedges bit-for-bit (watertight);
    // only the 2D connectivity is computed in param space — and triangulate.
    if loop_data.edges.len() == 3 {
        let mut chains: Vec<Vec<Point3>> = Vec::with_capacity(3);
        for (k, &eid) in loop_data.edges.iter().enumerate() {
            let samples = cache.get_or_compute(eid, model);
            if samples.len() < 2 {
                return false;
            }
            let mut ch: Vec<Point3> = samples.iter().copied().collect();
            if !loop_data.orientations.get(k).copied().unwrap_or(true) {
                ch.reverse();
            }
            chains.push(ch);
        }
        let close = |p: Point3, q: Point3| (p - q).magnitude() < 1e-6;
        if !close(chains[0][chains[0].len() - 1], chains[1][0])
            || !close(chains[1][chains[1].len() - 1], chains[2][0])
            || !close(chains[2][chains[2].len() - 1], chains[0][0])
        {
            return false;
        }
        // Map the 3 chains onto a CCW triangle in (u,v): corner0=(0,0),
        // corner1=(1,0), corner2=(0.5,1). Drop each chain's shared last corner.
        let corners = [
            (Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0)),
            (Point3::new(1.0, 0.0, 0.0), Point3::new(0.5, 1.0, 0.0)),
            (Point3::new(0.5, 1.0, 0.0), Point3::new(0.0, 0.0, 0.0)),
        ];
        let mut p3: Vec<Point3> = Vec::new();
        let mut puv: Vec<Point3> = Vec::new();
        for (ch, (c0, c1)) in chains.iter().zip(corners.iter()) {
            let nn = ch.len();
            for i in 0..nn - 1 {
                let t = i as f64 / (nn - 1) as f64;
                p3.push(ch[i]);
                puv.push(Point3::new(
                    c0.x + (c1.x - c0.x) * t,
                    c0.y + (c1.y - c0.y) * t,
                    0.0,
                ));
            }
        }
        if p3.len() < 3 {
            return false;
        }
        let boundaries = [(0usize, puv.len(), true)];
        let tris = triangulate_planar_polygon(&puv, &boundaries, &Vector3::Z);
        if tris.is_empty() {
            return false;
        }
        let outward = newell_normal(&p3)
            .map(|nv| nv * face.orientation.sign())
            .unwrap_or(Vector3::Z);
        let idx: Vec<u32> = p3
            .iter()
            .map(|&position| {
                mesh.add_vertex(MeshVertex {
                    position,
                    normal: outward,
                    uv: None,
                })
            })
            .collect();
        for t in &tris {
            let (i0, i1, i2) = (t[0], t[1], t[2]);
            let gn = (p3[i1] - p3[i0]).cross(&(p3[i2] - p3[i0]));
            if gn.dot(&outward) >= 0.0 {
                mesh.add_triangle(idx[i0], idx[i1], idx[i2]);
            } else {
                mesh.add_triangle(idx[i0], idx[i2], idx[i1]);
            }
        }
        return true;
    }

    // 1-EDGE FULL POLE (smooth SOLID revolve): the loop is a SINGLE closed rim
    // circle and the SurfaceOfRevolution closes to a POLE (apex) at the opposite
    // profile end — a smooth revolved wall that comes to a point (nose cone,
    // dome, teardrop, bullet). The quad/Coons paths need ≥4 corners and a `Cone`
    // gets a dedicated apex collapse, so a CURVED pole wall fell through to
    // curved-CDT, which left the apex OPEN (boundary-edge leak → not watertight,
    // the analytic revolve self-check rolls back to per-segment bands). Mesh it
    // as a fan: the rim ring (EXACT edge-cache samples → watertight with the base
    // disc) marched up through intermediate ON-SURFACE rings to a single
    // collapsed apex vertex.
    if loop_data.edges.len() == 1 {
        let surf = match model.surfaces.get(face.surface_id) {
            Some(s) => s,
            None => return false,
        };
        let rim_eid = loop_data.edges[0];
        let mut rim: Vec<Point3> = cache
            .get_or_compute(rim_eid, model)
            .iter()
            .copied()
            .collect();
        if !loop_data.orientations.first().copied().unwrap_or(true) {
            rim.reverse();
        }
        // Drop the duplicated closing sample of the closed circle.
        if rim.len() >= 2 && (rim[0] - rim[rim.len() - 1]).magnitude() < 1e-6 {
            rim.pop();
        }
        let na = rim.len();
        if na < 3 {
            return false;
        }
        let ((u_lo, u_hi), (v_lo, v_hi)) = surf.parameter_bounds();
        // A profile end is a POLE when all angular samples there collapse to one
        // point. Identify it; without a pole this is not our case.
        let is_pole = |u: f64| match (
            surf.evaluate_full(u, v_lo).ok(),
            surf.evaluate_full(u, 0.5 * (v_lo + v_hi)).ok(),
        ) {
            (Some(a), Some(b)) => (a.position - b.position).magnitude() < 1e-6,
            _ => false,
        };
        let (u_rim, u_apex) = if is_pole(u_hi) {
            (u_lo, u_hi)
        } else if is_pole(u_lo) {
            (u_hi, u_lo)
        } else {
            return false;
        };
        let apex_p = match surf.evaluate_full(u_apex, v_lo) {
            Ok(sp) => sp.position,
            Err(_) => return false,
        };
        // Outward sense from the real rim ring (CCW about the surface normal).
        let outward = newell_normal(&rim)
            .map(|nv| nv * face.orientation.sign())
            .unwrap_or(Vector3::Z);
        // Angle of rim sample i (uniform from the seam, matching the circle edge
        // cache) — used to sample the intermediate rings so they align
        // index-for-index with the exact rim ring above.
        let ang_at = |i: usize| v_lo + (v_hi - v_lo) * (i as f64) / (na as f64);
        // Meridian resolution: interior-only, so watertightness rides entirely on
        // the shared rim ring; pick enough rings for a smooth wall.
        let m = na.clamp(16, 64);
        let wind =
            |a: u32, b: u32, c: u32, pa: Point3, pb: Point3, pc: Point3| -> (u32, u32, u32) {
                let gn = (pb - pa).cross(&(pc - pa));
                if gn.dot(&outward) >= 0.0 {
                    (a, b, c)
                } else {
                    (a, c, b)
                }
            };
        // Ring stack: ring 0 = exact rim; rings 1..m march on the surface toward
        // the apex.
        let mut rings: Vec<Vec<(u32, Point3)>> = Vec::with_capacity(m);
        let mut row0 = Vec::with_capacity(na);
        for i in 0..na {
            let normal = surf
                .evaluate_full(u_rim, ang_at(i))
                .ok()
                .map(|sp| {
                    if sp.normal.dot(&outward) < 0.0 {
                        -sp.normal
                    } else {
                        sp.normal
                    }
                })
                .unwrap_or(outward);
            let id = mesh.add_vertex(MeshVertex {
                position: rim[i],
                normal,
                uv: None,
            });
            row0.push((id, rim[i]));
        }
        rings.push(row0);
        for k in 1..m {
            let t = k as f64 / m as f64;
            let u = u_rim + (u_apex - u_rim) * t;
            let mut row = Vec::with_capacity(na);
            for i in 0..na {
                let sp = match surf.evaluate_full(u, ang_at(i)) {
                    Ok(s) => s,
                    Err(_) => return false,
                };
                let normal = if sp.normal.dot(&outward) < 0.0 {
                    -sp.normal
                } else {
                    sp.normal
                };
                let id = mesh.add_vertex(MeshVertex {
                    position: sp.position,
                    normal,
                    uv: None,
                });
                row.push((id, sp.position));
            }
            rings.push(row);
        }
        let apex_id = mesh.add_vertex(MeshVertex {
            position: apex_p,
            normal: outward,
            uv: None,
        });
        // Quad strips between consecutive rings (wrap i → i+1 mod na).
        for k in 0..rings.len() - 1 {
            for i in 0..na {
                let j = (i + 1) % na;
                let (a, pa) = rings[k][i];
                let (b, pb) = rings[k][j];
                let (c, pc) = rings[k + 1][j];
                let (d, pd) = rings[k + 1][i];
                let (x, y, z) = wind(a, b, c, pa, pb, pc);
                mesh.add_triangle(x, y, z);
                let (x, y, z) = wind(a, c, d, pa, pc, pd);
                mesh.add_triangle(x, y, z);
            }
        }
        // Apex fan from the last ring.
        if let Some(last) = rings.last() {
            for i in 0..na {
                let j = (i + 1) % na;
                let (a, pa) = last[i];
                let (b, pb) = last[j];
                let (x, y, z) = wind(a, b, apex_id, pa, pb, apex_p);
                mesh.add_triangle(x, y, z);
            }
        }
        return true;
    }

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
    // Corner continuity (chains belong to one loop, so these hold exactly for a
    // clean quad; a mismatch means the loop is not a 4-corner patch). Walk
    // c0→c1 (a) → c1→c2 (b) → c2→c3 (c) → c3→c0 (d).
    let close = |p: Point3, q: Point3| (p - q).magnitude() < 1e-6;
    if !close(a[a.len() - 1], b[0])
        || !close(b[b.len() - 1], c[0])
        || !close(c[c.len() - 1], d[0])
        || !close(d[d.len() - 1], a[0])
    {
        return false;
    }

    // GENERAL CASE — unequal opposite boundary counts. A revolve band's two
    // meridian arcs sit at DIFFERENT radii, so the chord-driven edge cache
    // samples them with different counts (`b.len() != d.len()`); congruent
    // profile-arc copies at adjacent stations can differ too (`a.len() !=
    // c.len()`). The Coons grid below needs equal opposite counts, so a CONE
    // band always fails it — and the curved-CDT fallback chokes on the thin 3D
    // sliver, leaving the band UNMESHED (holes → a revolved nozzle renders as
    // nothing). Fix it FUNDAMENTALLY for every profile shape: triangulate in
    // the wedge's (u,v) PARAMETER square, where the patch is well-conditioned
    // regardless of radii (no sliver aspect ratio), using the EXACT boundary
    // cache samples. Each boundary point keeps its real 3D position (so every
    // shared edge matches its neighbour's samples bit-for-bit → watertight);
    // only the 2D triangulation connectivity is computed in (u,v). Boundary-
    // only (no interior Steiner) — exact for the developable band.
    if a.len() != c.len() || b.len() != d.len() {
        let mut p3: Vec<Point3> = Vec::new();
        let mut puv: Vec<Point3> = Vec::new();
        // a: v=0, u 0→1 ; b: u=1, v 0→1 ; c: v=1, u 1→0 ; d: u=0, v 1→0.
        // Drop each chain's last point: it is the shared corner that opens the
        // next chain (corner continuity verified above).
        let na = a.len();
        for i in 0..na - 1 {
            p3.push(a[i]);
            puv.push(Point3::new(i as f64 / (na - 1) as f64, 0.0, 0.0));
        }
        let nb = b.len();
        for j in 0..nb - 1 {
            p3.push(b[j]);
            puv.push(Point3::new(1.0, j as f64 / (nb - 1) as f64, 0.0));
        }
        let nc = c.len();
        for k in 0..nc - 1 {
            p3.push(c[k]);
            puv.push(Point3::new(1.0 - k as f64 / (nc - 1) as f64, 1.0, 0.0));
        }
        let nd = d.len();
        for l in 0..nd - 1 {
            p3.push(d[l]);
            puv.push(Point3::new(0.0, 1.0 - l as f64 / (nd - 1) as f64, 0.0));
        }
        if p3.len() < 3 {
            return false;
        }
        let boundaries = [(0usize, puv.len(), true)];
        let tris = triangulate_planar_polygon(&puv, &boundaries, &Vector3::Z);
        if tris.is_empty() {
            return false;
        }
        // Outward normal: Newell of the real 3D ring (CCW about the surface's
        // natural normal, since the loop is CCW) times the face orientation sign
        // — identical convention to `tessellate_planar_face`.
        let outward = newell_normal(&p3)
            .map(|nv| nv * face.orientation.sign())
            .unwrap_or(Vector3::Z);
        // SMOOTH SHADING: take each vertex's normal from the surface itself
        // (evaluate_full at the vertex's (u,v)), not a single flat per-band
        // normal — otherwise each sloped band renders as a faceted rectangle
        // ("rectangular spots" on a revolved cone). The wedge's (u,v) square
        // maps to the SurfaceOfRevolution params as u=profile∈[0,1],
        // v=angle∈[0,angle]; puv carries (u, v_frac, 0). Orient each to the
        // outward side so shading is consistent; fall back to the flat outward
        // normal if the surface evaluation fails.
        let surf = model.surfaces.get(face.surface_id);
        let ang = surf.map(|s| s.parameter_bounds().1 .1).unwrap_or(0.0);
        let idx: Vec<u32> = (0..p3.len())
            .map(|k| {
                let normal = surf
                    .and_then(|s| s.evaluate_full(puv[k].x, puv[k].y * ang).ok())
                    .map(|sp| {
                        if sp.normal.dot(&outward) < 0.0 {
                            -sp.normal
                        } else {
                            sp.normal
                        }
                    })
                    .unwrap_or(outward);
                mesh.add_vertex(MeshVertex {
                    position: p3[k],
                    normal,
                    uv: None,
                })
            })
            .collect();
        for t in &tris {
            let (i0, i1, i2) = (t[0], t[1], t[2]);
            let gn = (p3[i1] - p3[i0]).cross(&(p3[i2] - p3[i0]));
            if gn.dot(&outward) >= 0.0 {
                mesh.add_triangle(idx[i0], idx[i1], idx[i2]);
            } else {
                mesh.add_triangle(idx[i0], idx[i2], idx[i1]);
            }
        }
        return true;
    }

    let n = a.len(); // i index: 0..n along A (c0→c1) and C (c2→c3)
    let m = b.len(); // j index: 0..m along B (c1→c2) and D (c3→c0)
    if n < 2 || m < 2 {
        return false;
    }

    // Position at grid node (i, j): boundary nodes come verbatim from the cache
    // chains (so shared seams are bit-exact). INTERIOR nodes are the seam meridian
    // ROTATED about the axis — the exact surface-of-revolution point — NOT a Coons
    // blend. A Coons blend is exact only for a DEVELOPABLE wedge (a cone/cylinder
    // band); for a meridian with internal curvature (a throated nozzle, a barrel)
    // it overshoots, pushing interior vertices OFF the surface and spiking the seam
    // (the deformed-nozzle bug — a Ø104 wall reaching r≈60 at the seam, not even
    // rotationally symmetric). Rotating a cached boundary about the surface's TRUE
    // axis keeps interior vertices exactly on the surface; boundaries stay verbatim
    // (watertight weld preserved).
    let (ox, oy, oz, dx, dy, dz) = model
        .surfaces
        .get(face.surface_id)
        .and_then(|s| {
            s.as_any()
                .downcast_ref::<crate::primitives::surface::SurfaceOfRevolution>()
        })
        .map(|sr| {
            let (o, d) = (sr.axis_origin, sr.axis_direction);
            let dl = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt().max(1e-12);
            (o.x, o.y, o.z, d.x / dl, d.y / dl, d.z / dl)
        })
        .unwrap_or((0.0, 0.0, 0.0, 0.0, 0.0, 1.0));
    let radial = |p: Point3| -> (f64, f64, f64) {
        let (vx, vy, vz) = (p.x - ox, p.y - oy, p.z - oz);
        let dot = vx * dx + vy * dy + vz * dz;
        (vx - dx * dot, vy - dy * dot, vz - dz * dot)
    };
    let signed_angle = |r0: (f64, f64, f64), p: Point3| -> f64 {
        let (rx, ry, rz) = radial(p);
        let (cx, cy, cz) = (
            r0.1 * rz - r0.2 * ry,
            r0.2 * rx - r0.0 * rz,
            r0.0 * ry - r0.1 * rx,
        );
        (cx * dx + cy * dy + cz * dz).atan2(r0.0 * rx + r0.1 * ry + r0.2 * rz)
    };
    let rotate = |p: Point3, ang: f64| -> Point3 {
        let (vx, vy, vz) = (p.x - ox, p.y - oy, p.z - oz);
        let dot = vx * dx + vy * dy + vz * dz;
        let (ax, ay, az) = (dx * dot, dy * dot, dz * dot);
        let (rx, ry, rz) = (vx - ax, vy - ay, vz - az);
        let (tx, ty, tz) = (dy * rz - dz * ry, dz * rx - dx * rz, dx * ry - dy * rx);
        let (cs, sn) = (ang.cos(), ang.sin());
        Point3::new(
            ox + ax + rx * cs + tx * sn,
            oy + ay + ry * cs + ty * sn,
            oz + az + rz * cs + tz * sn,
        )
    };
    // Which boundary direction is the SEAM (constant angle) vs the RING (sweeps the
    // wedge angle)? `a`/`c` are one opposite pair, `b`/`d` the other, so exactly one
    // pair is the seams; the seam's angular span is ~0, the ring's is the wedge angle.
    let ang_span = |ch: &[Point3]| -> f64 {
        let r0 = radial(ch[0]);
        let (mut lo, mut hi) = (0.0_f64, 0.0_f64);
        for &p in ch {
            let aa = signed_angle(r0, p);
            lo = lo.min(aa);
            hi = hi.max(aa);
        }
        hi - lo
    };
    let a_is_seam = ang_span(a) <= ang_span(d);
    let (ra0, rb0) = (radial(a[0]), radial(b[0]));
    let node = |i: usize, j: usize| -> Point3 {
        if j == 0 {
            return a[i];
        }
        if j == m - 1 {
            return c[n - 1 - i];
        }
        if i == 0 {
            return d[m - 1 - j];
        }
        if i == n - 1 {
            return b[j];
        }
        // Interior = a cached boundary rotated to the node's angle (exact). If `a`
        // is the seam, rotate a[i] by column j's angle (from ring b); otherwise `a`
        // is the ring, so rotate the seam d[m-1-j] by row i's angle (from ring a).
        if a_is_seam {
            rotate(a[i], signed_angle(rb0, b[j]))
        } else {
            rotate(d[m - 1 - j], signed_angle(ra0, a[i]))
        }
    };

    // Emit grid vertices. Shading normals are taken from local grid tangents
    // (positions are what matter for watertightness; normals need only be
    // smooth), oriented to agree with the face's OUTWARD direction.
    let pos: Vec<Vec<Point3>> = (0..n)
        .map(|i| (0..m).map(|j| node(i, j)).collect())
        .collect();
    let grid_normal = |i: usize, j: usize| -> Vector3 {
        let ip = pos[(i + 1).min(n - 1)][j];
        let im = pos[i.saturating_sub(1)][j];
        let jp = pos[i][(j + 1).min(m - 1)];
        let jm = pos[i][j.saturating_sub(1)];
        (ip - im)
            .cross(&(jp - jm))
            .normalize()
            .unwrap_or(Vector3::Z)
    };

    // The OUTWARD direction at each node = the TRUE surface normal (oriented by
    // the face sign), with the grid-tangent normal as a fallback. The grid (i, j)
    // index runs (angle, profile) for a revolution wall, so the grid-tangent
    // cross product (i+)×(j+) = dv×du = −natural — i.e. the raw grid normal does
    // NOT robustly encode the surface's natural normal sign. Keying the triangle
    // winding on `face.orientation` alone (the old code) therefore inverted the
    // whole 360° band (off-surface normals, max-deviation ≈ 179°). Instead derive
    // the outward target from the surface normal itself and wind each triangle
    // geometrically to match it — exactly the self-consistent rule the unequal-
    // count branch above already uses, and independent of any i/j↔u/v convention.
    let surf = model.surfaces.get(face.surface_id);
    let ang = surf.map(|s| s.parameter_bounds().1 .1).unwrap_or(0.0);
    let sign = face.orientation.sign();
    let surf_outward = |i: usize, j: usize| -> Option<Vector3> {
        // Grid maps to the SurfaceOfRevolution params as u=profile∈[0,1] (the j
        // index, normalised over the seam chain), v=angle∈[0,angle] (the i index,
        // normalised over the ring chain). When chain `a` is the seam these roles
        // swap, mirroring the `node` closure above.
        let (u, vf) = if a_is_seam {
            (i as f64 / (n - 1) as f64, j as f64 / (m - 1) as f64)
        } else {
            (j as f64 / (m - 1) as f64, i as f64 / (n - 1) as f64)
        };
        surf.and_then(|s| s.evaluate_full(u, vf * ang).ok())
            .map(|sp| sp.normal * sign)
            .and_then(|nv| nv.normalize().ok())
    };

    let mut grid: Vec<Vec<u32>> = vec![vec![0u32; m]; n];
    for i in 0..n {
        for j in 0..m {
            // Shading normal: prefer the true surface normal (oriented outward);
            // fall back to the grid-tangent normal flipped onto the outward side.
            let normal = surf_outward(i, j).unwrap_or_else(|| {
                let gn = grid_normal(i, j);
                let reference = surf_outward(i.min(n - 1), j.min(m - 1));
                match reference {
                    Some(r) if gn.dot(&r) < 0.0 => -gn,
                    _ => gn,
                }
            });
            grid[i][j] = mesh.add_vertex(MeshVertex {
                position: pos[i][j],
                normal,
                uv: None,
            });
        }
    }

    // Emit two triangles per cell, winding each so its geometric normal agrees
    // with the cell's OUTWARD direction (true surface normal × face sign). This
    // is robust to the grid's i/j orientation: a Forward face winds +natural, a
    // Backward face −natural, with no reliance on the (i+)×(j+) sign convention.
    let cell_outward = |i: usize, j: usize| -> Vector3 {
        surf_outward(i, j)
            .or_else(|| surf_outward(i + 1, j + 1))
            .unwrap_or_else(|| {
                let gn = grid_normal(i, j);
                gn * if sign >= 0.0 { 1.0 } else { -1.0 }
            })
    };
    for i in 0..n - 1 {
        for j in 0..m - 1 {
            let (v00, v10, v01, v11) = (
                grid[i][j],
                grid[i + 1][j],
                grid[i][j + 1],
                grid[i + 1][j + 1],
            );
            let out = cell_outward(i, j);
            // Triangle 1 (v00, v10, v01): geometric normal sign decides winding.
            let g1 = (pos[i + 1][j] - pos[i][j]).cross(&(pos[i][j + 1] - pos[i][j]));
            if g1.dot(&out) >= 0.0 {
                mesh.add_triangle(v00, v10, v01);
            } else {
                mesh.add_triangle(v00, v01, v10);
            }
            // Triangle 2 (v10, v11, v01).
            let g2 = (pos[i + 1][j + 1] - pos[i + 1][j]).cross(&(pos[i][j + 1] - pos[i + 1][j]));
            if g2.dot(&out) >= 0.0 {
                mesh.add_triangle(v10, v11, v01);
            } else {
                mesh.add_triangle(v10, v01, v11);
            }
        }
    }
    true
}

/// Tessellate a CLOSED-in-U NURBS skin lateral (the `nurbs_loft` wall): one
/// `GeneralNurbsSurface` face whose loop is the cylinder-style 4-edge ring
/// (bottom closed ring at v=0, seam at u=0≡u=1 used twice, top closed ring at
/// v=1). curved-CDT cannot tessellate it — its boundary→(u,v) inversion uses the
/// NURBS surface's grid-search `closest_point`, which is ambiguous/divergent at
/// the seam and emits zero triangles (the lateral vanishes, leaving the caps'
/// rings open). The revolution-wedge Coons path is no good either: a single tall
/// patch carries its v-curvature INTERNALLY, which a boundary-only blend of the
/// two end rings would flatten (a barrel would tessellate as a cone).
///
/// This samples a STRUCTURED grid directly in the surface's (u, v) domain — no
/// `closest_point`. The v=0 / v=1 rows are taken VERBATIM from the
/// `EdgeSampleCache` for the two ring edges, so they are bit-identical to what
/// the adjacent planar caps sample for the same edges (watertight weld).
/// Interior rows are real `surface.point_at(u, v)` samples (the wall follows the
/// NURBS curvature). Consecutive rows — whose point counts may differ (the end
/// rings come from a chord-driven cache, interior rows from a fixed u-grid) — are
/// stitched by parametric fraction. Returns `false` (caller falls back to
/// curved-CDT) if the face is not this closed-lateral form.
fn tessellate_nurbs_skin_lateral(
    surface: &dyn Surface,
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    cache: &EdgeSampleCache,
    mesh: &mut TriangleMesh,
) -> bool {
    // A closed-u skin lateral carrying a blind-pocket hole (task #17) routes
    // through the hole-aware structured CDT below — the generic curved-CDT
    // path cracks at the periodic seam for this surface class. Faces with no
    // hole keep the original phase-aligned ring stitch.
    if !face.inner_loops.is_empty() {
        return tessellate_nurbs_skin_lateral_with_holes(surface, face, model, params, cache, mesh);
    }
    let loop_data = match model.loops.get(face.outer_loop) {
        Some(l) => l,
        None => return false,
    };
    // Cylinder-style loop: two distinct CLOSED ring edges + one seam edge used
    // twice (4 loop entries). A boolean that cuts a notch THROUGH a cap rim (the
    // box reaches above/below the barrel cap, so the cut crosses the planar
    // cap↔lateral edge) leaves the lateral fragment's top (or bottom) ring
    // BROKEN by an angular gap, with the notch's side/bottom cut edges threaded
    // into the single outer loop (no inner loop, edge count ≠ 4). The
    // structured ring stitch below cannot represent that gap; route it to the
    // cap-edge-notch mesher, which unrolls the periodic wall into a clean UV
    // rectangle with the notch carved out of the rim. It self-declines (→
    // curved-CDT) for any loop that is not this notched-rim form, so the clean
    // 4-edge barrel and every already-watertight case are untouched.
    if loop_data.edges.len() != 4 {
        return tessellate_nurbs_skin_lateral_cap_notch(surface, face, model, params, cache, mesh);
    }
    let mut ring_edges: Vec<crate::primitives::edge::EdgeId> = Vec::new();
    for &eid in &loop_data.edges {
        if let Some(e) = model.edges.get(eid) {
            if e.start_vertex == e.end_vertex && !ring_edges.contains(&eid) {
                ring_edges.push(eid);
            }
        }
    }
    if ring_edges.len() != 2 {
        return false;
    }

    // Ring boundary samples (cache emits forward param direction, u: 0→1).
    let s_a = cache.get_or_compute(ring_edges[0], model);
    let s_b = cache.get_or_compute(ring_edges[1], model);
    if s_a.len() < 4 || s_b.len() < 4 {
        return false;
    }
    // Classify v=0 vs v=1 by proximity of each ring's seam point to S(0,0)/S(0,1).
    let p00 = match surface.point_at(0.0, 0.0) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let p01 = match surface.point_at(0.0, 1.0) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let a_is_bottom = (s_a[0] - p00).magnitude() <= (s_a[0] - p01).magnitude();
    let (bottom_s, top_s): (&[Point3], &[Point3]) = if a_is_bottom {
        (&s_a, &s_b)
    } else {
        (&s_b, &s_a)
    };
    // Distinct ring points (drop the duplicated closing sample).
    let bottom_ring = &bottom_s[..bottom_s.len() - 1];
    let top_ring = &top_s[..top_s.len() - 1];

    // v-resolution: chord-driven along the (curved) seam at u=0.
    let m_bands = {
        let probe = 32;
        let mut prev = p00;
        let mut length = 0.0;
        for k in 1..=probe {
            let v = k as f64 / probe as f64;
            if let Ok(p) = surface.point_at(0.0, v) {
                length += (p - prev).magnitude();
                prev = p;
            }
        }
        linear_steps_for_quality(length, params).max(2)
    };
    let n_u = bottom_ring.len().max(top_ring.len()).max(8);
    let orient_sign = face.orientation.sign();

    // Build the rows of mesh-vertex indices.
    let push_ring = |mesh: &mut TriangleMesh, pts: &[Point3], v: f64| -> Vec<u32> {
        let count = pts.len();
        pts.iter()
            .enumerate()
            .map(|(k, p)| {
                let u = k as f64 / count as f64;
                let normal = surface
                    .normal_at(u, v)
                    .map(|nn| nn * orient_sign)
                    .unwrap_or(Vector3::Z);
                mesh.add_vertex(MeshVertex {
                    position: *p,
                    normal,
                    uv: None,
                })
            })
            .collect()
    };

    let mut rows: Vec<Vec<u32>> = Vec::with_capacity(m_bands + 1);
    rows.push(push_ring(mesh, bottom_ring, 0.0));
    for j in 1..m_bands {
        let v = j as f64 / m_bands as f64;
        let mut row = Vec::with_capacity(n_u);
        for i in 0..n_u {
            let u = i as f64 / n_u as f64;
            let p = match surface.point_at(u, v) {
                Ok(p) => p,
                Err(_) => return false,
            };
            let normal = surface
                .normal_at(u, v)
                .map(|nn| nn * orient_sign)
                .unwrap_or(Vector3::Z);
            row.push(mesh.add_vertex(MeshVertex {
                position: p,
                normal,
                uv: None,
            }));
        }
        rows.push(row);
    }
    rows.push(push_ring(mesh, top_ring, 1.0));

    for band in 0..rows.len() - 1 {
        stitch_closed_rings(&rows[band].clone(), &rows[band + 1].clone(), mesh);
    }
    true
}

/// Tessellate a CLOSED-in-u NURBS skin lateral whose top OR bottom cap rim is
/// breached by a boolean notch (task #17 watertight, the cap-edge case). When a
/// cutter reaches above (or below) the barrel's planar cap, the difference's cut
/// curves cross the cap↔lateral rim edge; the lateral fragment then carries a
/// single outer loop in which one ring is interrupted by an angular GAP and the
/// notch's side/bottom cut edges are threaded inline (no inner loop, edge count
/// ≠ 4). Neither the clean ring-stitch nor the hole-window mesher fits this form,
/// and the generic curved-CDT path folds at the periodic seam (its
/// `closest_point` boundary inversion maps both seam sides to one u), so the
/// fragment tessellated with overlapping, off-surface facets (non-manifold mesh).
///
/// This unrolls the periodic wall into a SINGLE simple UV polygon: the seam edge
/// (the only edge walked twice by the loop) is split into the rectangle's two
/// vertical sides (u=0 on one traversal, u=1 on the other) — exactly the
/// structural seam split the hole-window mesher uses — while every other edge is
/// projected to (u,v) by `closest_point` and unwrapped monotonically along the
/// walk, so the notch becomes a clean bite out of the rim. The polygon is
/// triangulated by the shared CDT (`curved_cdt::run_cdt`) with a structured
/// interior Steiner grid; boundary vertices take their 3D VERBATIM from the
/// `EdgeSampleCache` (welding bit-exactly to the adjacent cap, box-wall and
/// pocket faces), interior vertices use `surface.point_at`. Returns `false`
/// (caller → curved-CDT) for any loop that is not this notched-rim form, so the
/// clean barrel and the already-watertight pocket/slot cases are never affected.
fn tessellate_nurbs_skin_lateral_cap_notch(
    surface: &dyn Surface,
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    cache: &EdgeSampleCache,
    mesh: &mut TriangleMesh,
) -> bool {
    use crate::primitives::edge::EdgeId;
    let tol = Tolerance::default();

    let Some(loop_data) = model.loops.get(face.outer_loop) else {
        return false;
    };
    if loop_data.edges.len() < 6 {
        return false;
    }

    // The skin-lateral seam is the unique edge the outer loop walks TWICE (once
    // up, once down). A notched-rim fragment still carries it; if no edge repeats
    // this is not the closed-u skin form — decline.
    let mut seen: HashMap<EdgeId, u32> = HashMap::new();
    for &eid in &loop_data.edges {
        *seen.entry(eid).or_insert(0) += 1;
    }
    let seam_candidates: Vec<EdgeId> = seen
        .iter()
        .filter(|(_, &c)| c == 2)
        .map(|(&e, _)| e)
        .collect();
    if seam_candidates.len() != 1 {
        return false;
    }
    let seam_edge = seam_candidates[0];

    // Surface seam endpoints (u=0 column): S(0,0) bottom, S(0,1) top.
    let (Ok(p00), Ok(p01)) = (surface.point_at(0.0, 0.0), surface.point_at(0.0, 1.0)) else {
        return false;
    };

    // Rotate the loop's edge list so it STARTS at the first seam occurrence. The
    // walk is then: seam · segmentA · seam · segmentB, and the two seam
    // traversals bracket the two non-seam runs cleanly.
    let Some(seam_first) = loop_data.edges.iter().position(|&e| e == seam_edge) else {
        return false;
    };
    let n_edges = loop_data.edges.len();
    let rot_edges: Vec<EdgeId> = (0..n_edges)
        .map(|k| loop_data.edges[(seam_first + k) % n_edges])
        .collect();
    let rot_orient: Vec<bool> = (0..n_edges)
        .map(|k| {
            loop_data
                .orientations
                .get((seam_first + k) % n_edges)
                .copied()
                .unwrap_or(true)
        })
        .collect();

    // A `Vert` is one contour vertex: its unrolled (u,v) and its resolved 3D
    // (verbatim cache for every boundary sample → watertight weld).
    struct Vert {
        u: f64,
        v: f64,
        p: Point3,
    }
    let mut contour: Vec<Vert> = Vec::new();

    // Emit one edge's cache samples into the contour. `pin_u` (when Some) forces
    // every sample's u (the seam sides → u=0 / u=1); otherwise `closest_point`
    // gives u and it is unwrapped to stay continuous with the previous vertex.
    // The last sample of each edge is dropped (shared with the next edge's
    // first), matching the loop-walk convention used elsewhere.
    let mut last_u: Option<f64> = None;
    let mut emit_edge = |eid: EdgeId,
                         fwd: bool,
                         pin_u: Option<f64>,
                         contour: &mut Vec<Vert>,
                         last_u: &mut Option<f64>|
     -> bool {
        let samples = cache.get_or_compute(eid, model);
        if samples.len() < 2 {
            return false;
        }
        let ordered: Vec<Point3> = if fwd {
            samples.to_vec()
        } else {
            samples.iter().rev().copied().collect()
        };
        for p in &ordered[..ordered.len() - 1] {
            let (u, v) = match pin_u {
                Some(pu) => {
                    // Seam side: v from closest_point (the height along the
                    // seam), u pinned to the rectangle wall.
                    let v = surface.closest_point(p, tol).map(|(_, v)| v).unwrap_or(0.0);
                    (pu, v)
                }
                None => {
                    let Ok((mut u, v)) = surface.closest_point(p, tol) else {
                        return false;
                    };
                    if let Some(pu) = *last_u {
                        while u - pu > 0.5 {
                            u -= 1.0;
                        }
                        while u - pu < -0.5 {
                            u += 1.0;
                        }
                    }
                    (u, v)
                }
            };
            *last_u = Some(u);
            contour.push(Vert { u, v, p: *p });
        }
        true
    };

    // Walk: seam#1 (right wall, u=1) · segmentA · seam#2 (left wall, u=0) ·
    // segmentB. Determine each seam traversal's vertical direction from its
    // sample order so the contour stays a simple CCW-or-CW ring; CDT is
    // orientation-agnostic and the winding is fixed at emission.
    let mut walked_first_seam = false;
    for k in 0..n_edges {
        let eid = rot_edges[k];
        let fwd = rot_orient[k];
        if eid == seam_edge {
            let pin = if !walked_first_seam { 1.0 } else { 0.0 };
            walked_first_seam = true;
            // Reset the unwrap anchor to the pinned wall so the following
            // segment unwraps from the correct side.
            if !emit_edge(eid, fwd, Some(pin), &mut contour, &mut last_u) {
                return false;
            }
            last_u = Some(pin);
        } else if !emit_edge(eid, fwd, None, &mut contour, &mut last_u) {
            return false;
        }
    }

    if contour.len() < 6 {
        return false;
    }

    // The unrolled contour must stay inside [0,1] in u (a small epsilon margin)
    // and span a non-trivial u-range; otherwise the seam split mis-fired and we
    // must not emit a folded patch — decline to curved-CDT.
    let (mut u_lo, mut u_hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for w in &contour {
        u_lo = u_lo.min(w.u);
        u_hi = u_hi.max(w.u);
    }
    if !u_lo.is_finite() || !u_hi.is_finite() {
        return false;
    }
    if u_lo < -1e-6 || u_hi > 1.0 + 1e-6 || (u_hi - u_lo) < 0.5 {
        return false;
    }

    // Outer UV polygon for the CDT.
    let outer_uv: Vec<(f64, f64)> = contour.iter().map(|w| (w.u, w.v)).collect();
    // Reject a self-touching / zero-area unrolled polygon (a mis-classified seam
    // would collapse the rectangle).
    let area = cap_notch_polygon_area(&outer_uv);
    if area.abs() < 1e-6 {
        return false;
    }

    // Structured interior Steiner grid: ring u-columns (the distinct boundary
    // u-values) × chord-driven v-bands. Each candidate is kept only if it lies
    // strictly inside the unrolled outer polygon, so the notch bite stays empty.
    let m_bands = {
        let probe = 32;
        let mut prev = p00;
        let mut length = 0.0;
        for k in 1..=probe {
            let v = k as f64 / probe as f64;
            if let Ok(p) = surface.point_at(0.0, v) {
                length += (p - prev).magnitude();
                prev = p;
            }
        }
        linear_steps_for_quality(length, params).max(2)
    };
    // u-columns: sample a uniform set across (0,1) plus the boundary u's, so the
    // interior is well-graded regardless of the ring sampling density.
    let n_cols = {
        let circumference = {
            let mut prev = p01;
            let mut length = 0.0;
            let probe = 48;
            for k in 1..=probe {
                let u = k as f64 / probe as f64;
                if let Ok(p) = surface.point_at(u, 1.0) {
                    length += (p - prev).magnitude();
                    prev = p;
                }
            }
            length
        };
        linear_steps_for_quality(circumference, params).max(8)
    };
    let mut steiner: Vec<(f64, f64)> = Vec::new();
    for ci in 1..n_cols {
        let u = ci as f64 / n_cols as f64;
        for bj in 1..m_bands {
            let v = bj as f64 / m_bands as f64;
            if cap_notch_point_in_polygon(u, v, &outer_uv) {
                steiner.push((u, v));
            }
        }
    }

    // Detach the NOTCH-BOTTOM cut arc from the free CDT and tessellate the thin
    // strip just inside it as a STRUCTURED quad band. The notch's bottom cut edge
    // is the box-cut plane ∩ the barrel — a verbatim-cached arc SHARED with the
    // box-bottom planar face (decimating it would unweld the seam), so the lateral
    // must carry its full dense sample stream. That stream is far denser in u than
    // the interior grid, and its samples are coplanar (in the box's horizontal cut
    // plane); a constrained Delaunay over it inevitably triangulates three
    // consecutive arc samples into a facet that is flat in the cut plane and so
    // folds tens of degrees off the true (radial) wall normal — and no interior
    // Steiner point can prevent it (the constrained arc edges force the fan).
    //
    // Instead: build a parallel OFFSET ROW one band below the arc (real
    // `point_at` samples, off the cut plane), feed the CDT the offset row as its
    // bottom boundary (so the dense flat arc never enters the Delaunay), and stitch
    // the arc → offset row as an explicit quad strip. Every strip triangle spans
    // z=cut down into the wall, so none is flat; the arc samples weld to the box
    // face verbatim and the offset row welds to the CDT region — watertight.
    let band_h = 1.0 / m_bands as f64;
    let mut arc_run: Vec<usize> = Vec::new();
    {
        // The notch-bottom arc is the maximal contiguous block of contour vertices
        // that are cut samples (not ring, not seam) sitting within half a band of
        // the lowest cut v — the horizontal arc, distinct from the side edges that
        // climb to the rim.
        let on_ring = |w: &Vert| w.v <= 1e-4 || w.v >= 1.0 - 1e-4;
        let on_seam = |w: &Vert| w.u <= 1e-4 || w.u >= 1.0 - 1e-4;
        let is_cut = |w: &Vert| !on_ring(w) && !on_seam(w);
        let nc = contour.len();
        let cut_idx: Vec<usize> = (0..nc).filter(|&i| is_cut(&contour[i])).collect();
        if cut_idx.len() >= 3 {
            let v_min = cut_idx
                .iter()
                .map(|&i| contour[i].v)
                .fold(f64::INFINITY, f64::min);
            let is_bottom = |i: usize| is_cut(&contour[i]) && contour[i].v <= v_min + 0.5 * band_h;
            // Find the contiguous (cyclic) bottom run. Pick a start index that is a
            // bottom vertex whose predecessor is NOT, so the run is a single block.
            if (0..nc).any(is_bottom) && !(0..nc).all(is_bottom) {
                let start = (0..nc)
                    .find(|&i| is_bottom(i) && !is_bottom((i + nc - 1) % nc))
                    .unwrap_or(0);
                let mut k = start;
                while is_bottom(k) {
                    arc_run.push(k);
                    k = (k + 1) % nc;
                    if k == start {
                        break;
                    }
                }
            }
        }
    }
    // Require a genuine multi-sample arc to detach; otherwise leave the contour as
    // is (the plain CDT path handles a 1–2 sample bottom without flat fans).
    if arc_run.len() < 3 {
        arc_run.clear();
    }

    // Build the offset row (uv + resolved 3D) parallel to the arc INTERIOR, one
    // band below. The arc's two END vertices are the notch corners (shared with
    // the box side faces) — they STAY in the CDT contour at the cut plane so those
    // welds are preserved; only the interior arc samples are replaced. Resolve 3D
    // via `point_at`; if any sample fails, abandon the detour and fall back to the
    // plain CDT over the original contour.
    let mut offset_uv: Vec<(f64, f64)> = Vec::new();
    let mut offset_p: Vec<Point3> = Vec::new();
    if arc_run.len() >= 3 {
        // Interior arc vertices = arc_run[1..len-1].
        for &i in &arc_run[1..arc_run.len() - 1] {
            let (ou, ov) = (contour[i].u, (contour[i].v - band_h).max(1e-4));
            match surface.point_at(ou, ov) {
                Ok(p) => {
                    offset_uv.push((ou, ov));
                    offset_p.push(p);
                }
                Err(_) => {
                    arc_run.clear();
                    offset_uv.clear();
                    offset_p.clear();
                    break;
                }
            }
        }
        if offset_uv.is_empty() {
            arc_run.clear();
        }
    } else {
        arc_run.clear();
    }

    // Assemble the CDT contour. When the arc is detached, the arc INTERIOR is
    // replaced (in walk order) by the offset row; the two corner vertices remain.
    // The bottom boundary of the CDT region is therefore: corner_left → offset row
    // → corner_right. Track, per CDT-contour index, the resolved 3D + true (u,v).
    struct CVert {
        u: f64,
        v: f64,
        p: Point3,
    }
    let cdt_contour: Vec<CVert> = if arc_run.is_empty() {
        contour
            .iter()
            .map(|w| CVert {
                u: w.u,
                v: w.v,
                p: w.p,
            })
            .collect()
    } else {
        // arc_run is contiguous; replace its INTERIOR span with the offset row,
        // keeping the first/last (corner) vertices. Walk the contour; at the first
        // INTERIOR arc vertex, splice in the offset row, then skip the rest of the
        // interior; the corner vertices fall through as ordinary contour vertices.
        let interior: std::collections::HashSet<usize> =
            arc_run[1..arc_run.len() - 1].iter().copied().collect();
        let interior_first = arc_run[1];
        let mut out: Vec<CVert> = Vec::with_capacity(contour.len());
        for i in 0..contour.len() {
            if i == interior_first {
                for (k, &(ou, ov)) in offset_uv.iter().enumerate() {
                    out.push(CVert {
                        u: ou,
                        v: ov,
                        p: offset_p[k],
                    });
                }
            } else if interior.contains(&i) {
                continue;
            } else {
                out.push(CVert {
                    u: contour[i].u,
                    v: contour[i].v,
                    p: contour[i].p,
                });
            }
        }
        out
    };

    let outer_uv: Vec<(f64, f64)> = cdt_contour.iter().map(|w| (w.u, w.v)).collect();
    // Drop interior Steiner candidates that fell into the thin arc→offset strip we
    // now own explicitly (between the offset row and the original arc v).
    if !arc_run.is_empty() {
        let v_arc = arc_run
            .iter()
            .map(|&i| contour[i].v)
            .fold(f64::NEG_INFINITY, f64::max);
        steiner.retain(|&(_, sv)| sv <= v_arc - band_h + 1e-9 || sv >= v_arc + 1e-9);
        steiner.retain(|&(su, sv)| cap_notch_point_in_polygon(su, sv, &outer_uv) || sv < v_arc);
    }

    if std::env::var("ROSHERA_TESS_TRACE").is_ok() {
        eprintln!(
            "[capnotch] face={:?} contour={} cdt_contour={} arc_run={} steiner={} u=[{:.3},{:.3}] m_bands={} n_cols={}",
            face.id,
            contour.len(),
            cdt_contour.len(),
            arc_run.len(),
            steiner.len(),
            u_lo,
            u_hi,
            m_bands,
            n_cols
        );
    }

    // Triangulate the unrolled rectangle-with-notch (bottom boundary = offset row
    // when the arc was detached).
    let inner_uvs: Vec<Vec<(f64, f64)>> = Vec::new();
    let (pts2d, triangles) = match super::curved_cdt::run_cdt(&outer_uv, &inner_uvs, &steiner) {
        Ok(v) => v,
        Err(_) => return false,
    };

    // Resolve every CDT vertex to 3D: a boundary vertex (index < cdt_contour.len())
    // takes its resolved 3D (verbatim cache for true boundary samples, `point_at`
    // for the offset row); an interior Steiner vertex uses `surface.point_at`.
    let orient_sign = face.orientation.sign();
    let n_boundary = cdt_contour.len();
    let vertex_base = mesh.vertices.len() as u32;
    for (i, &(u, v)) in pts2d.iter().enumerate() {
        let (position, nu, nv) = if i < n_boundary {
            (cdt_contour[i].p, cdt_contour[i].u, cdt_contour[i].v)
        } else {
            match surface.point_at(u, v) {
                Ok(p) => (p, u, v),
                Err(_) => return false,
            }
        };
        let normal = surface
            .normal_at(nu, nv)
            .map(|nn| nn * orient_sign)
            .unwrap_or(Vector3::Z);
        mesh.add_vertex(MeshVertex {
            position,
            normal,
            uv: Some((nu, nv)),
        });
    }
    // Wind each triangle outward against the stored (orientation-corrected)
    // normals — the CDT's UV winding does not encode the 3D outward sense.
    for tri in &triangles {
        emit_outward_triangle(
            mesh,
            vertex_base + tri[0] as u32,
            vertex_base + tri[1] as u32,
            vertex_base + tri[2] as u32,
        );
    }

    // Stitch the detached arc → offset row as a structured quad strip. The strip's
    // TOP row is the full arc (corners + interior); its BOTTOM row is the offset
    // row bracketed by the SAME two corners. The corners and the offset row are
    // already CDT-contour vertices (so they weld to the side edges and the CDT
    // region); their mesh indices are `vertex_base + their cdt_contour position`.
    // Only the interior arc samples (verbatim cut-plane cache) are added fresh.
    // Every strip triangle spans the cut plane down to the off-plane offset row, so
    // none is flat.
    if !arc_run.is_empty() {
        // Map original contour index → cdt_contour position, for the vertices that
        // survived into the CDT contour (everything except the arc interior). The
        // offset row was spliced where the first interior arc vertex sat.
        let interior: std::collections::HashSet<usize> =
            arc_run[1..arc_run.len() - 1].iter().copied().collect();
        let interior_first = arc_run[1];
        let mut cdt_pos: HashMap<usize, usize> = HashMap::new();
        let mut offset_start = 0usize;
        {
            let mut pos = 0usize;
            for i in 0..contour.len() {
                if i == interior_first {
                    offset_start = pos;
                    pos += offset_uv.len();
                } else if interior.contains(&i) {
                    continue;
                } else {
                    cdt_pos.insert(i, pos);
                    pos += 1;
                }
            }
        }
        let corner_l = arc_run[0];
        let corner_r = arc_run[arc_run.len() - 1];
        let (Some(&cl_pos), Some(&cr_pos)) = (cdt_pos.get(&corner_l), cdt_pos.get(&corner_r))
        else {
            return true; // corners not in CDT contour (should not happen) — skip strip
        };
        let cl_idx = vertex_base + cl_pos as u32;
        let cr_idx = vertex_base + cr_pos as u32;
        // Bottom row: [corner_l, offset[0..], corner_r].
        let mut bottom: Vec<u32> = Vec::with_capacity(arc_run.len());
        bottom.push(cl_idx);
        for k in 0..offset_uv.len() {
            bottom.push(vertex_base + (offset_start + k) as u32);
        }
        bottom.push(cr_idx);
        // Top row: [corner_l, fresh(interior arc samples), corner_r].
        let mut top: Vec<u32> = Vec::with_capacity(arc_run.len());
        top.push(cl_idx);
        for &i in &arc_run[1..arc_run.len() - 1] {
            let normal = surface
                .normal_at(contour[i].u, contour[i].v)
                .map(|nn| nn * orient_sign)
                .unwrap_or(Vector3::Z);
            top.push(mesh.add_vertex(MeshVertex {
                position: contour[i].p,
                normal,
                uv: Some((contour[i].u, contour[i].v)),
            }));
        }
        top.push(cr_idx);
        // Stitch the equal-length rows as a quad strip.
        if top.len() == bottom.len() {
            for j in 0..top.len() - 1 {
                emit_outward_triangle(mesh, top[j], top[j + 1], bottom[j + 1]);
                emit_outward_triangle(mesh, top[j], bottom[j + 1], bottom[j]);
            }
        }
    }

    true
}

/// Signed area of a 2D polygon (shoelace). Sign encodes winding; magnitude is
/// the area. Local to the cap-notch mesher's degeneracy guard.
fn cap_notch_polygon_area(poly: &[(f64, f64)]) -> f64 {
    let n = poly.len();
    if n < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    let mut j = n - 1;
    for i in 0..n {
        a += (poly[j].0 + poly[i].0) * (poly[j].1 - poly[i].1);
        j = i;
    }
    0.5 * a
}

/// Even-odd point-in-polygon for the cap-notch interior Steiner filter.
fn cap_notch_point_in_polygon(px: f64, py: f64, poly: &[(f64, f64)]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > py) != (yj > py) {
            let x_int = (xj - xi) * (py - yi) / (yj - yi) + xi;
            if px < x_int {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Ray-cast point-in-polygon in 2D (even-odd rule). Local to the skin-lateral
/// hole mesher; a Steiner grid point landing inside a pocket rim must be
/// skipped so the CDT hole stays empty.
fn uv_point_in_polygon(px: f64, py: f64, poly: &[(f64, f64)]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > py) != (yj > py) {
            let x_int = (xj - xi) * (py - yi) / (yj - yi) + xi;
            if px < x_int {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// Tessellate a CLOSED-in-u NURBS skin lateral that carries one or more blind-
/// pocket holes (task #17). The generic curved-CDT path cracks at the periodic
/// seam of this surface class (its `closest_point` boundary inversion is
/// ambiguous at u=0/u=2π), so a holed barrel came out non-watertight. This
/// builds a constrained Delaunay triangulation in the surface's UNROLLED (u, v)
/// parameter rectangle — non-periodic, so the seam is an ordinary left/right
/// edge — with each pocket rim as a hole contour, then maps every UV vertex
/// back to 3D. Boundary vertices (the two ring caps, the seam, and every hole
/// rim) take their 3D positions VERBATIM from the shared `EdgeSampleCache`, so
/// they coincide bit-exactly with the adjacent cap and pocket-wall faces;
/// interior Steiner points use `surface.point_at`. The left (u=0) and right
/// (u=1) seam columns resolve to the same cached seam samples, so the periodic
/// seam welds shut.
///
/// Returns `false` (caller falls back to curved-CDT) on any structural surprise
/// — wrong loop arity, missing seam edge, CDT failure — so this never silently
/// drops the wall.
///
/// Choose the u at which the structured tessellation grid places its wrap (seam)
/// column for a holed skin lateral. The grid brackets each hole in a contiguous
/// column block inside `[0, 1)` and keeps the wrap cell structured, so the wrap
/// column must avoid every hole's u-extent. Returns `0.0` (no shift, original
/// layout) when no hole straddles the seam — the off-seam case. When a hole DOES
/// straddle u=0, returns the centre of the WIDEST hole-free u-gap (mod 1), so the
/// wrap column lands cleanly behind the pocket. `hole_uv_polys` u-coordinates are
/// the per-rim unwrapped values (a seam-straddling rim therefore reaches below 0
/// or above 1).
fn compute_skin_seam_shift(hole_uv_polys: &[Vec<(f64, f64)>]) -> f64 {
    // Collect each hole's occupied u-interval, wrapped into [0,1) as one or two
    // sub-intervals (a straddling rim splits at the seam).
    let mut occupied: Vec<(f64, f64)> = Vec::new();
    let mut straddles = false;
    for poly in hole_uv_polys {
        let mut ulo = f64::INFINITY;
        let mut uhi = f64::NEG_INFINITY;
        for &(u, _) in poly {
            ulo = ulo.min(u);
            uhi = uhi.max(u);
        }
        if !ulo.is_finite() || !uhi.is_finite() {
            continue;
        }
        if ulo < -1e-9 || uhi > 1.0 + 1e-9 {
            straddles = true;
        }
        // Add a small margin so the gap search keeps clear of the rim.
        let margin = 1e-3;
        let lo = ulo - margin;
        let hi = uhi + margin;
        // Split into [0,1) pieces.
        let mut push_wrapped = |a: f64, b: f64, occ: &mut Vec<(f64, f64)>| {
            let a = a.rem_euclid(1.0);
            let b = b.rem_euclid(1.0);
            if a <= b {
                occ.push((a, b));
            } else {
                occ.push((a, 1.0));
                occ.push((0.0, b));
            }
        };
        push_wrapped(lo, hi, &mut occupied);
    }
    // No hole crosses the seam → keep the original layout exactly.
    if !straddles || occupied.is_empty() {
        return 0.0;
    }
    // Find the widest gap on the circle [0,1) not covered by any occupied
    // interval. Sort interval starts; sweep with coverage merge.
    occupied.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    // Merge overlapping intervals.
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for &(a, b) in &occupied {
        if let Some(last) = merged.last_mut() {
            if a <= last.1 + 1e-9 {
                last.1 = last.1.max(b);
                continue;
            }
        }
        merged.push((a, b));
    }
    if merged.is_empty() {
        return 0.0;
    }
    // Gaps are between consecutive merged intervals, plus the wrap gap from the
    // last interval's end back to the first's start (+1).
    let mut best_gap = -1.0;
    let mut best_mid = 0.0;
    for w in merged.windows(2) {
        let gap = w[1].0 - w[0].1;
        if gap > best_gap {
            best_gap = gap;
            best_mid = 0.5 * (w[0].1 + w[1].0);
        }
    }
    let wrap_gap = (merged[0].0 + 1.0) - merged[merged.len() - 1].1;
    if wrap_gap > best_gap {
        best_gap = wrap_gap;
        best_mid = (0.5 * ((merged[merged.len() - 1].1) + (merged[0].0 + 1.0))).rem_euclid(1.0);
    }
    if best_gap <= 0.0 {
        return 0.0;
    }
    best_mid
}

fn tessellate_nurbs_skin_lateral_with_holes(
    surface: &dyn Surface,
    face: &Face,
    model: &BRepModel,
    params: &TessellationParams,
    cache: &EdgeSampleCache,
    mesh: &mut TriangleMesh,
) -> bool {
    use crate::primitives::edge::EdgeId;
    let tol = Tolerance::default();

    let loop_data = match model.loops.get(face.outer_loop) {
        Some(l) => l,
        None => return false,
    };
    let (Ok(p00), Ok(p01)) = (surface.point_at(0.0, 0.0), surface.point_at(0.0, 1.0)) else {
        return false;
    };

    // Classify each outer-loop edge by the surface-v of its midpoint: v≈0 →
    // bottom ring, v≈1 → top ring, otherwise the seam (a v-spanning edge). The
    // post-boolean barrel's cap circles are split into several arcs and the
    // seam appears twice, so a fixed 4-edge arity no longer holds — classify
    // by geometry instead. Bottom/top arcs are concatenated (in loop order,
    // honoring orientation) into the two rings; the seam edge supplies the
    // interior v-column.
    let mut bottom_pts: Vec<Point3> = Vec::new();
    let mut top_pts: Vec<Point3> = Vec::new();
    let mut seam_edge: Option<EdgeId> = None;
    for (i, &eid) in loop_data.edges.iter().enumerate() {
        let Some(edge) = model.edges.get(eid) else {
            return false;
        };
        let Some(curve) = model.curves.get(edge.curve_id) else {
            return false;
        };
        let t_mid = 0.5 * (edge.param_range.start + edge.param_range.end);
        let Ok(mid) = curve.point_at(t_mid) else {
            return false;
        };
        let Ok((_, v_mid)) = surface.closest_point(&mid, tol) else {
            return false;
        };
        let samples = cache.get_or_compute(eid, model);
        if samples.len() < 2 {
            return false;
        }
        let fwd = loop_data.orientations.get(i).copied().unwrap_or(true);
        let ordered: Vec<Point3> = if fwd {
            samples.to_vec()
        } else {
            samples.iter().rev().copied().collect()
        };
        if v_mid < 0.25 {
            // bottom ring arc — append (drop last, shared with next arc).
            bottom_pts.extend_from_slice(&ordered[..ordered.len() - 1]);
        } else if v_mid > 0.75 {
            top_pts.extend_from_slice(&ordered[..ordered.len() - 1]);
        } else if seam_edge.is_none() {
            seam_edge = Some(eid);
        }
    }
    if bottom_pts.len() < 3 || top_pts.len() < 3 {
        return false;
    }
    let Some(seam_edge) = seam_edge else {
        return false;
    };

    // Bottom ring must wind so u increases; the loop walk already produced one
    // consistent direction. Make both rings start near u=0 (the seam) for the
    // u = k/count mapping below. We accept whatever phase the loop gave; the
    // outer contour is consistent regardless of where u=0 sits because the CDT
    // operates on the actual (u, v) coordinates we assign next.
    let bottom_ring: &[Point3] = &bottom_pts;
    let top_ring: &[Point3] = &top_pts;

    // Seam samples (cache forward direction); order bottom→top.
    let seam_s = cache.get_or_compute(seam_edge, model);
    if seam_s.len() < 2 {
        return false;
    }
    let seam_forward_is_up = (seam_s[0] - p00).magnitude() <= (seam_s[0] - p01).magnitude();
    let seam_up: Vec<Point3> = if seam_forward_is_up {
        seam_s.to_vec()
    } else {
        seam_s.iter().rev().copied().collect()
    };

    // UV-builder: a list of (u, v) points and a parallel list of their 3D
    // positions. Boundary points carry their cache 3D; interior points are
    // None (filled by point_at). The CDT runs on the UV coordinates.
    let mut uv: Vec<(f64, f64)> = Vec::new();
    let mut pos3d: Vec<Option<Point3>> = Vec::new();
    let mut push = |u: f64,
                    v: f64,
                    p: Option<Point3>,
                    uv: &mut Vec<(f64, f64)>,
                    pos3d: &mut Vec<Option<Point3>>|
     -> usize {
        let idx = uv.len();
        uv.push((u, v));
        pos3d.push(p);
        idx
    };

    // ---- Outer contour (CCW in UV): bottom ring (u: 0→1 at v=0), right seam
    //      (v: 0→1 at u=1), top ring (u: 1→0 at v=1), left seam (v: 1→0 at
    //      u=0). Ring point u comes from `closest_point` (the post-boolean ring
    //      is a set of arcs that need not start at the seam), then the ring is
    //      sorted by u and pinned to span exactly [0, 1] with the seam point
    //      duplicated at both ends. Seam columns at u=0 and u=1 carry the SAME
    //      cached seam 3D so the periodic seam welds shut. ----
    // Each ring point gets its true u (from `closest_point`); the ring is
    // sorted by u and the seam point (the cache sample coincident with the seam
    // edge's endpoint) is identified so it can anchor u=0 and be duplicated at
    // u=1. The cap face samples the SAME ring edges via the same cache, so the
    // 3D points are identical on both sides — the only thing this function
    // controls is the (u, v) layout for the CDT.
    let seam_bottom = seam_up[0];
    let seam_top = seam_up[seam_up.len() - 1];
    // Assign u to each ring point by CUMULATIVE CHORD FRACTION in the ring's
    // (already angular, CCW) loop order — NOT `closest_point`, whose u is
    // ambiguous at the periodic seam and silently collides near-seam points on
    // opposite sides (they then dedup away, leaving the CDT to chord across the
    // gap → leaks). The ring is rotated so the seam point is index 0 (u=0); the
    // sequence is strictly increasing in u and spans [0, 1).
    // Returns, per ring vertex: (chord_frac_layout_u, true_surface_u, 3D).
    //  * chord_frac drives the CDT/grid LAYOUT (clean, monotone, seam-anchored).
    //  * true_surface_u is the actual NURBS u-parameter (recovered by inverting
    //    the cached 3D point), used to SAMPLE interior rows so they land at the
    //    same angular position as the rings. The skin's u-parameterisation is
    //    NOT chord-proportional (a chord fraction of 0.53 can map to surface-u
    //    on the OPPOSITE side of the barrel), so sampling interiors with the
    //    chord fraction folded the near-ring facets ~90° off the true normal.
    let ring_uv = |ring: &[Point3], seam_pt: Point3| -> Vec<(f64, f64, Point3)> {
        if ring.len() < 3 {
            return Vec::new();
        }
        // Rotate so the seam point is first.
        let seam_idx = ring
            .iter()
            .position(|p| (*p - seam_pt).magnitude() < 1e-9)
            .unwrap_or(0);
        let rotated: Vec<Point3> = ring[seam_idx..]
            .iter()
            .chain(ring[..seam_idx].iter())
            .copied()
            .collect();
        // Cumulative chord length, closing back to the seam.
        let mut cum: Vec<f64> = Vec::with_capacity(rotated.len());
        let mut acc = 0.0;
        cum.push(0.0);
        for k in 1..rotated.len() {
            acc += (rotated[k] - rotated[k - 1]).magnitude();
            cum.push(acc);
        }
        let closing = acc + (rotated[0] - rotated[rotated.len() - 1]).magnitude();
        if closing <= 1e-12 {
            return Vec::new();
        }
        // Recover the true surface u for each vertex, monotonically unwrapped
        // forward from the seam (true_u[0] = 0) so it stays in [0, 1).
        let mut last_u: Option<f64> = None;
        rotated
            .iter()
            .zip(cum.iter())
            .map(|(p, c)| {
                let cf = c / closing;
                let true_u = match surface.closest_point(p, tol) {
                    Ok((mut su, _)) => {
                        if let Some(pu) = last_u {
                            while su - pu > 0.5 {
                                su -= 1.0;
                            }
                            while su - pu < -0.5 {
                                su += 1.0;
                            }
                        } else {
                            // First (seam) vertex: pin to 0.
                            su = 0.0;
                        }
                        last_u = Some(su);
                        su
                    }
                    Err(_) => {
                        let f = cf;
                        last_u = Some(f);
                        f
                    }
                };
                (cf, true_u, *p)
            })
            .collect()
    };
    let mut bottom_uv = ring_uv(bottom_ring, seam_bottom);
    let mut top_uv = ring_uv(top_ring, seam_top);
    if bottom_uv.len() < 3 || top_uv.len() < 3 {
        return false;
    }
    // The seam point must be the first entry (u=0) of each ring.
    if bottom_uv[0].0 > 1e-9 || top_uv[0].0 > 1e-9 {
        return false;
    }
    let mut seam_bottom = seam_bottom;
    let mut seam_top = seam_top;
    let mut seam_interior: Vec<Point3> = seam_up[1..seam_up.len() - 1].to_vec();
    let ns = seam_interior.len();

    // ---- Hole contours: project each inner-loop rim's cache 3D to UV. The
    //      rim vertices carry their VERBATIM cached 3D so they weld bit-exactly
    //      to the adjacent pocket-wall faces; the structured grid below consumes
    //      these (`uv`/`pos3d`) as the inner-contour anchors of the hole band. ----
    let mut hole_contours: Vec<Vec<usize>> = Vec::new();
    let mut hole_uv_polys: Vec<Vec<(f64, f64)>> = Vec::new();
    for &inner_id in &face.inner_loops {
        let Some(inner) = model.loops.get(inner_id) else {
            return false;
        };
        let mut contour: Vec<usize> = Vec::new();
        let mut poly: Vec<(f64, f64)> = Vec::new();
        let mut last_u: Option<f64> = None;
        for (i, &eid) in inner.edges.iter().enumerate() {
            let fwd = inner.orientations.get(i).copied().unwrap_or(true);
            let samples = cache.get_or_compute(eid, model);
            let n = samples.len();
            if n < 2 {
                continue;
            }
            let emit: Vec<usize> = if fwd {
                (0..n - 1).collect()
            } else {
                (1..n).rev().collect()
            };
            for si in emit {
                let p = samples[si];
                let Ok((mut u, v)) = surface.closest_point(&p, tol) else {
                    return false;
                };
                // Unwrap u against the previous rim sample so the rim doesn't
                // straddle the seam ambiguously.
                if let Some(pu) = last_u {
                    while u - pu > 0.5 {
                        u -= 1.0;
                    }
                    while u - pu < -0.5 {
                        u += 1.0;
                    }
                }
                last_u = Some(u);
                contour.push(push(u, v, Some(p), &mut uv, &mut pos3d));
                poly.push((u, v));
            }
        }
        if contour.len() < 3 {
            return false;
        }
        hole_contours.push(contour);
        hole_uv_polys.push(poly);
    }

    // SEAM-STRADDLING pocket rebase (task #17 watertight). The structured
    // u-column grid below brackets each hole in a CONTIGUOUS column block inside
    // [0, 1] and keeps the wrap (seam) cell structured — valid only while every
    // pocket rim stays clear of the u=0 seam. A pocket placed ON the seam (the
    // +X seam wall) has a rim whose unwrapped u-span crosses 0 (e.g.
    // [-0.04, 0.04]), so a [0,1] column block cannot bracket its negative-u half
    // and the wall tessellates with a crack along the seam.
    //
    // The lateral is PERIODIC in u, so the tessellation seam (the doubled wrap
    // column) may sit at ANY u — it need not coincide with the B-Rep seam edge.
    // Shift the whole u-layout by `u_shift` so the wrap column lands in the
    // widest hole-free gap (the back of the barrel, opposite the pocket). Every
    // u-coordinate (rings, holes, parent rim samples) moves to the rebased frame
    // `(u - u_shift) mod 1`; the seam column then samples
    // `surface.point_at(u_shift, v)` — a regular interior point, equally
    // watertight (the cells either side share it). Off-seam pockets get
    // `u_shift == 0` (the gap is already at u=0) and keep the layout bit-for-bit.
    let u_shift = compute_skin_seam_shift(&hole_uv_polys);
    if u_shift != 0.0 {
        let rebase = |u: f64| -> f64 { (u - u_shift).rem_euclid(1.0) };
        // Re-unwrap a rebased u-sequence forward from its first element so a
        // contiguous rim/ring never splits across the new wrap boundary.
        let reunwrap = |seq: &mut [f64]| {
            let mut last: Option<f64> = None;
            for u in seq.iter_mut() {
                let mut r = rebase(*u);
                if let Some(pu) = last {
                    while r - pu > 0.5 {
                        r -= 1.0;
                    }
                    while r - pu < -0.5 {
                        r += 1.0;
                    }
                }
                last = Some(r);
                *u = r;
            }
        };
        // Rebase ring layout u and true-surface u. Both fields shift by the same
        // amount; the rings are sorted into ascending u after rebase below.
        for entry in bottom_uv.iter_mut().chain(top_uv.iter_mut()) {
            entry.0 = rebase(entry.0); // chord-fraction layout u
            entry.1 = rebase(entry.1); // true surface u
        }
        bottom_uv.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        top_uv.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        // Rebase each hole rim poly and the matching parent-uv entries (the CDT
        // window reads `uv` for the rim vertices), keeping the rim contiguous via
        // re-unwrap. `hole_contours[h]` holds the parent-`uv` index per rim
        // vertex, in the same order as `hole_uv_polys[h]`.
        for (poly, contour) in hole_uv_polys.iter_mut().zip(hole_contours.iter()) {
            let mut us: Vec<f64> = poly.iter().map(|&(u, _)| u).collect();
            reunwrap(&mut us);
            for (k, entry) in poly.iter_mut().enumerate() {
                entry.0 = us[k];
                if let Some(&pidx) = contour.get(k) {
                    if let Some(slot) = uv.get_mut(pidx) {
                        slot.0 = us[k];
                    }
                }
            }
        }
        // The wrap (seam) column now sits at rebased u=0 (original u = u_shift).
        // Its v=0 / v=1 rows MUST be a CACHED ring sample (verbatim 3D), not a
        // fresh `point_at(u_shift, ·)`: the bottom/top cap disc faces tessellate
        // their rim from the SAME cached ring edge, so only a shared cached point
        // welds the lateral's seam column to the cap. Pick the cached ring vertex
        // whose rebased true-u is nearest the wrap boundary (0 ≡ 1) — it lies in
        // the hole-free gap, so it is a clean, pocket-free seam anchor.
        let nearest_ring_at_wrap = |ring: &[(f64, f64, Point3)]| -> Option<Point3> {
            ring.iter()
                .min_by(|a, b| {
                    // Circular distance of the true-u to the wrap boundary {0,1}.
                    let da = a.1.rem_euclid(1.0).min(1.0 - a.1.rem_euclid(1.0));
                    let db = b.1.rem_euclid(1.0).min(1.0 - b.1.rem_euclid(1.0));
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|&(_, _, p)| p)
        };
        if let Some(p) = nearest_ring_at_wrap(&bottom_uv) {
            seam_bottom = p;
        }
        if let Some(p) = nearest_ring_at_wrap(&top_uv) {
            seam_top = p;
        }
        if ns > 0 {
            let mut new_interior = Vec::with_capacity(ns);
            for idx in 0..ns {
                let v = (idx + 1) as f64 / (ns + 1) as f64;
                match surface.point_at(u_shift, v) {
                    Ok(p) => new_interior.push(p),
                    Err(_) => {
                        new_interior.clear();
                        break;
                    }
                }
            }
            if new_interior.len() == ns {
                seam_interior = new_interior;
            }
        }
    }

    // ---- v-resolution (number of horizontal bands). ----
    let m_bands = {
        let probe = 32;
        let mut prev = p00;
        let mut length = 0.0;
        for k in 1..=probe {
            let v = k as f64 / probe as f64;
            if let Ok(p) = surface.point_at(0.0, v) {
                length += (p - prev).magnitude();
                prev = p;
            }
        }
        linear_steps_for_quality(length, params).max(2)
    };

    // Structured columns, indexed by the TRUE NURBS surface u (recovered from the
    // cached ring vertices, monotonically unwrapped from the seam). Sampling and
    // layout share this single u-space, so interior rows (`point_at(true_u, v)`)
    // land at the rings' angular position and the hole rim (also recovered via
    // `closest_point` → true u) lies in the SAME space — eliminating the
    // chord-fraction/surface-u mismatch that folded near-ring facets ~90° off
    // the true normal. Each column keeps the cached ring 3D (`bot`/`top`) so the
    // v=0/v=1 rows weld bit-exactly to the caps. The seam column (u=0) is added
    // by `build_row`; the strip wraps from the last column back to it.
    struct SkinCol {
        u: f64,
        bot: Option<Point3>,
        top: Option<Point3>,
    }
    let mut columns: Vec<SkinCol> = Vec::new();
    {
        let mut merged: Vec<(f64, Option<Point3>, Option<Point3>)> = Vec::new();
        for &(_, tu, p) in &bottom_uv {
            if tu <= 1e-6 || tu >= 1.0 - 1e-6 {
                continue;
            }
            merged.push((tu, Some(p), None));
        }
        for &(_, tu, p) in &top_uv {
            if tu <= 1e-6 || tu >= 1.0 - 1e-6 {
                continue;
            }
            if let Some(slot) = merged.iter_mut().find(|m| (m.0 - tu).abs() <= 1e-6) {
                slot.2 = Some(p);
            } else {
                merged.push((tu, None, Some(p)));
            }
        }
        merged.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        merged.dedup_by(|a, b| (a.0 - b.0).abs() <= 1e-6);
        for (u, bot, top) in merged {
            columns.push(SkinCol { u, bot, top });
        }
    }
    if columns.is_empty() {
        let n_u = bottom_uv.len().max(top_uv.len()).max(8);
        for i in 1..n_u {
            let u = i as f64 / n_u as f64;
            columns.push(SkinCol {
                u,
                bot: None,
                top: None,
            });
        }
    }
    let col_us: Vec<f64> = columns.iter().map(|c| c.u).collect();

    // Hole UV bounding boxes (v-extent drives the band split; u-extent the
    // per-cell skip). A tiny margin keeps a cell that merely grazes a rim in
    // the CDT region rather than the structured region.
    const HOLE_MARGIN: f64 = 1e-6;
    let hole_vbox: Vec<(f64, f64)> = hole_uv_polys
        .iter()
        .map(|poly| {
            let mut vlo = f64::INFINITY;
            let mut vhi = f64::NEG_INFINITY;
            for &(_, v) in poly {
                vlo = vlo.min(v);
                vhi = vhi.max(v);
            }
            (vlo - HOLE_MARGIN, vhi + HOLE_MARGIN)
        })
        .collect();

    // The structured grid replaces the previous single global CDT, which fed
    // the third-party `cdt` crate a ~12k-point quasi-regular lattice whose
    // exactly-collinear ring rows it triangulated into long sliver fans — flat
    // in UV but, because the barrel curves most at its crest, folded ~90° off
    // the true surface normal at v≈0/v≈1 (the cut-region mesh-quality defect:
    // worst_aspect≈60, max_normal_dev≈90°). Here every band whose v-extent is
    // clear of all hole rims is emitted as column-aligned quads (two triangles
    // per cell), which are watertight and faithful by construction; only the
    // thin band(s) the hole rim actually crosses fall to a LOCAL CDT, where a
    // residual sliver spans at most one band height (small chord → small fold).
    let orient_sign = face.orientation.sign();

    // Build a structured row of mesh-vertex indices at the given v over the
    // wrap columns [0 (seam), col_us..., 1 (seam)]. The two seam columns share
    // the SAME cached seam 3D so the periodic seam welds shut. v=0/v=1 rows take
    // their 3D verbatim from the cached ring at the col u (exact: col_us derives
    // from the ring vertices); interior rows use `point_at`. `ring` (when given)
    // is the sorted (u, Point3) ring used to look the 3D up.
    let seam_at = |v: f64| -> Point3 {
        if v <= 1e-9 {
            seam_bottom
        } else if v >= 1.0 - 1e-9 {
            seam_top
        } else {
            // Interior seam point: nearest cached seam-column sample.
            let idx = ((v * (ns + 1) as f64).round() as usize).clamp(1, ns) - 1;
            seam_interior[idx]
        }
    };
    // A structured row is PERIODIC: one seam column at u=0 (no duplicate at
    // u=1). The strip wraps — the last cell joins the last column back to the
    // seam column. A single wrapping seam column tessellates the seam exactly as
    // the (clean) hole-free `stitch_closed_rings` path does. v=0/v=1 rows take
    // the cached ring 3D verbatim (welds to the caps); interior rows sample
    // `point_at(true_u, v)` so they line up with the rings.
    // Map a LAYOUT u (rebased frame, [0,1)) to the ORIGINAL surface u for
    // sampling/normals. `u_shift == 0` makes this the identity, so the off-seam
    // fast path is bit-for-bit unchanged.
    let to_surface_u = |u: f64| -> f64 { (u + u_shift).rem_euclid(1.0) };
    let build_row = |mesh: &mut TriangleMesh, v: f64| -> Vec<u32> {
        let mut row: Vec<u32> = Vec::with_capacity(columns.len() + 1);
        let mut add = |p: Point3, u: f64, mesh: &mut TriangleMesh| {
            let normal = surface
                .normal_at(to_surface_u(u), v)
                .map(|nn| nn * orient_sign)
                .unwrap_or(Vector3::Z);
            row.push(mesh.add_vertex(MeshVertex {
                position: p,
                normal,
                uv: None,
            }));
        };
        // Seam column (rebased u=0; 3D supplied by `seam_at`, already at u_shift).
        add(seam_at(v), 0.0, mesh);
        for col in &columns {
            let p = if v <= 1e-9 {
                col.bot
            } else if v >= 1.0 - 1e-9 {
                col.top
            } else {
                None
            };
            let p = match p.or_else(|| surface.point_at(to_surface_u(col.u), v).ok()) {
                Some(p) => p,
                None => seam_at(v),
            };
            add(p, col.u, mesh);
        }
        row
    };

    // Column lattice (u-coordinates): seam column at u=0 then col_us. The wrap
    // cell (index col_us.len()) spans grid_us.last()..1.0 back to column 0.
    let grid_us: Vec<f64> = std::iter::once(0.0).chain(col_us.iter().copied()).collect();
    let ncols = grid_us.len();
    // Per-hole u-extent in grid-column indices: the CDT only owns the smallest
    // contiguous block of cells [c_lo, c_hi) that fully brackets the hole's u
    // span (plus one cell of margin each side). Everything else in the hole's
    // v-band stays structured quads. A pocket rim never reaches the seam column,
    // so c_lo ≥ 0 and c_hi ≤ ncols-1 (the wrap cell is always structured).
    let hole_ucols: Vec<(usize, usize)> = hole_uv_polys
        .iter()
        .map(|poly| {
            let (mut ulo, mut uhi) = (f64::INFINITY, f64::NEG_INFINITY);
            for &(u, _) in poly {
                ulo = ulo.min(u);
                uhi = uhi.max(u);
            }
            let mut c_lo = 0usize;
            while c_lo + 1 < ncols && grid_us[c_lo + 1] <= ulo - HOLE_MARGIN {
                c_lo += 1;
            }
            let mut c_hi = ncols - 1;
            while c_hi > 0 && grid_us[c_hi - 1] >= uhi + HOLE_MARGIN {
                c_hi -= 1;
            }
            (c_lo, c_hi.max(c_lo + 1).min(ncols - 1))
        })
        .collect();
    // A grid cell `c` (joining column c to column c+1) is owned by a hole CDT
    // region if it lies in that hole's column block AND v-band.
    let cell_owned_by_cdt = |c: usize, v_a: f64, v_b: f64| -> bool {
        hole_vbox
            .iter()
            .zip(hole_ucols.iter())
            .any(|(&(hlo, hhi), &(c_lo, c_hi))| v_b >= hlo && v_a <= hhi && c >= c_lo && c < c_hi)
    };

    // Emit the structured quad strip between two periodic rows, skipping cells
    // owned by a hole CDT region. Cell `c` joins column c to column (c+1) mod
    // ncols; the final cell (c = ncols-1) is the seam wrap.
    let emit_quad_strip = |mesh: &mut TriangleMesh, lo: &[u32], hi: &[u32], v_a: f64, v_b: f64| {
        for c in 0..ncols {
            if cell_owned_by_cdt(c, v_a, v_b) {
                continue;
            }
            let cn = (c + 1) % ncols;
            let (a, b, cc, d) = (lo[c], lo[cn], hi[cn], hi[c]);
            emit_outward_triangle(mesh, a, b, cc);
            emit_outward_triangle(mesh, a, cc, d);
        }
    };

    // Pre-build every structured row.
    let v_of = |j: usize| j as f64 / m_bands as f64;
    let mut rows: Vec<Vec<u32>> = Vec::with_capacity(m_bands + 1);
    for j in 0..=m_bands {
        rows.push(build_row(mesh, v_of(j)));
    }

    // Structured quads everywhere (skipping hole-owned cells).
    for j in 0..m_bands {
        emit_quad_strip(mesh, &rows[j], &rows[j + 1], v_of(j), v_of(j + 1));
    }

    // Local CDT for each hole's small u×v window. The window's bottom/top rows
    // sample the SAME grid columns as the structured strips (so the seam is the
    // shared grid columns), and its left/right boundary columns are exactly the
    // structured grid columns grid_us[c_lo] / grid_us[c_hi] — the CDT vertices
    // there coincide bit-exactly with the structured rows' vertices, so the two
    // regions weld. The hole rim is carved as an inner contour (verbatim cached
    // 3D). A residual CDT sliver now spans at most one band and a few columns,
    // far from the curved crest, so it cannot fold off-surface.
    let mut cdt_ok = true;
    for (hi_idx, &(vlo, vhi)) in hole_vbox.iter().enumerate() {
        // Snap the hole's v-band to the band grid (inclusive of the bands it
        // touches) so the window edges coincide with structured rows.
        let mut jlo = 0usize;
        while jlo + 1 <= m_bands && v_of(jlo + 1) <= vlo {
            jlo += 1;
        }
        let mut jhi = m_bands;
        while jhi > 0 && v_of(jhi - 1) >= vhi {
            jhi -= 1;
        }
        if jhi <= jlo {
            continue;
        }
        let (c_lo, c_hi) = hole_ucols[hi_idx];
        if !tessellate_skin_hole_window(
            surface,
            &grid_us,
            c_lo,
            c_hi,
            v_of(jlo),
            v_of(jhi),
            &rows,
            jlo,
            jhi,
            &hole_contours[hi_idx],
            &uv,
            &pos3d,
            orient_sign,
            u_shift,
            mesh,
        ) {
            cdt_ok = false;
            break;
        }
    }
    cdt_ok
}

/// Tessellate one hole's small u×v window in a holed NURBS skin lateral: the
/// sub-rectangle `[grid_us[c_lo], grid_us[c_hi]] × [v_lo, v_hi]` that brackets a
/// single pocket rim, with the rim carved as an inner CDT contour.
///
/// The window's four boundaries REUSE the structured grid's existing mesh-vertex
/// indices (`rows[j][c]` for the band rows `jlo..=jhi`, columns `c_lo..=c_hi`),
/// so the CDT patch welds to the surrounding structured quads bit-exactly — no
/// re-created boundary vertex, no T-junction. Only the interior Steiner points
/// and the hole rim vertices are new. The rim is taken VERBATIM from the cached
/// rim samples (`hole_contour` indexes into `parent_uv`/`parent_pos`), so it
/// welds to the adjacent pocket-wall face. Returns `false` (caller aborts the
/// holed path → curved-CDT fallback) on any CDT failure, so the wall is never
/// silently dropped.
#[allow(clippy::too_many_arguments)]
fn tessellate_skin_hole_window(
    surface: &dyn Surface,
    grid_us: &[f64],
    c_lo: usize,
    c_hi: usize,
    v_lo: f64,
    v_hi: f64,
    rows: &[Vec<u32>],
    jlo: usize,
    jhi: usize,
    hole_contour: &[usize],
    parent_uv: &[(f64, f64)],
    parent_pos: &[Option<Point3>],
    orient_sign: f64,
    u_shift: f64,
    mesh: &mut TriangleMesh,
) -> bool {
    if v_hi <= v_lo || c_hi <= c_lo || jhi <= jlo || hole_contour.len() < 3 {
        return false;
    }
    // Layout-u (rebased frame) → original surface u for sampling/normals. Identity
    // when `u_shift == 0` (off-seam pockets), so their mesh is unchanged.
    let to_surface_u = |u: f64| -> f64 { (u + u_shift).rem_euclid(1.0) };
    // Local CDT point set. Each entry is (u, v) plus a RESOLUTION: an already-
    // existing mesh-vertex index (boundary, welds to the structured grid) OR a
    // verbatim 3D position (hole rim) OR `None` (interior Steiner, filled by
    // `point_at`).
    enum Loc {
        Vid(u32),
        Pos(Point3),
        Interior,
    }
    let mut luv: Vec<(f64, f64)> = Vec::new();
    let mut lloc: Vec<Loc> = Vec::new();
    let mut lpush = |u: f64, v: f64, loc: Loc| -> usize {
        let i = luv.len();
        luv.push((u, v));
        lloc.push(loc);
        i
    };

    // Outer contour CCW in (u, v): bottom edge (columns c_lo→c_hi at row jlo),
    // right edge (rows jlo→jhi at column c_hi), top edge (c_hi→c_lo at jhi),
    // left edge (jhi→jlo at column c_lo). Each vertex reuses the structured
    // grid's mesh-vertex index, so the patch boundary IS the structured grid.
    let mut outer: Vec<usize> = Vec::new();
    for c in c_lo..=c_hi {
        outer.push(lpush(grid_us[c], v_lo, Loc::Vid(rows[jlo][c])));
    }
    for j in (jlo + 1)..jhi {
        outer.push(lpush(
            grid_us[c_hi],
            v_of_row(j, jlo, jhi, v_lo, v_hi),
            Loc::Vid(rows[j][c_hi]),
        ));
    }
    for c in (c_lo..=c_hi).rev() {
        outer.push(lpush(grid_us[c], v_hi, Loc::Vid(rows[jhi][c])));
    }
    for j in ((jlo + 1)..jhi).rev() {
        outer.push(lpush(
            grid_us[c_lo],
            v_of_row(j, jlo, jhi, v_lo, v_hi),
            Loc::Vid(rows[j][c_lo]),
        ));
    }

    // Hole rim as the inner contour (verbatim cached 3D from the parent).
    let mut rim: Vec<usize> = Vec::with_capacity(hole_contour.len());
    let mut rim_poly: Vec<(f64, f64)> = Vec::with_capacity(hole_contour.len());
    for &k in hole_contour {
        let (u, v) = parent_uv[k];
        let loc = match parent_pos[k] {
            Some(p) => Loc::Pos(p),
            None => Loc::Interior,
        };
        rim.push(lpush(u, v, loc));
        rim_poly.push((u, v));
    }

    // A box pocket rim is a rectangle of STRAIGHT edges, each collinear in 3D
    // and in UV. A CDT triangulates a region bounded by a dense collinear edge
    // into in-plane slivers (the edge fans to a single distant opposite vertex);
    // on the vertical wall those slivers lie in the rim-edge plane and read
    // ~90° off the true (radial) surface normal. To break every rim edge's fan,
    // seed a HALO of Steiner anchors just OUTSIDE the rim — one per rim vertex,
    // offset outward (away from the rim centroid) by a fraction of the local rim
    // segment length. Each rim edge then triangulates against its two adjacent
    // halo points (a near-equilateral quad split), not a remote interior vertex,
    // so no in-plane sliver survives.
    let rim_centroid = {
        let (mut su, mut sv) = (0.0, 0.0);
        for &(u, v) in &rim_poly {
            su += u;
            sv += v;
        }
        let n = rim_poly.len().max(1) as f64;
        (su / n, sv / n)
    };
    // Halo offset: a fraction of the window height, the depth at which a halo
    // anchor sits clearly OUT from the rim. Halo points carry their own UV
    // distance from the rim, so the CDT triangulates each rim edge against an
    // anchor with genuine perpendicular depth — no in-plane sliver.
    let halo_off = (0.18 * (v_hi - v_lo)).max(1e-4);
    let nrim = rim_poly.len();
    // Per-EDGE outward-perpendicular halo midpoints break the collinear fan that
    // a per-vertex (centroid-radial) halo misses on a long straight rim edge.
    let mut halo: Vec<(f64, f64)> = Vec::with_capacity(2 * nrim);
    for i in 0..nrim {
        let (u0, v0) = rim_poly[i];
        let (u1, v1) = rim_poly[(i + 1) % nrim];
        let (mu, mv) = (0.5 * (u0 + u1), 0.5 * (v0 + v1));
        // Edge direction → outward perpendicular (rotate ±90°, pick the side
        // away from the rim centroid).
        let (ex, ey) = (u1 - u0, v1 - v0);
        let em = (ex * ex + ey * ey).sqrt();
        let (px, py) = if em > 1e-12 {
            (-ey / em, ex / em)
        } else {
            let (cx, cy) = (mu - rim_centroid.0, mv - rim_centroid.1);
            let cm = (cx * cx + cy * cy).sqrt().max(1e-12);
            (cx / cm, cy / cm)
        };
        let outward = (mu - rim_centroid.0) * px + (mv - rim_centroid.1) * py;
        let sign = if outward >= 0.0 { 1.0 } else { -1.0 };
        halo.push((mu + sign * px * halo_off, mv + sign * py * halo_off));
    }
    for &(hu, hv) in &halo {
        if uv_point_in_polygon(hu, hv, &rim_poly) {
            continue;
        }
        if hu <= grid_us[c_lo] || hu >= grid_us[c_hi] || hv <= v_lo || hv >= v_hi {
            continue;
        }
        lpush(hu, hv, Loc::Interior);
    }

    // Background interior lattice within the window (finer than one band in v so
    // any residual gap between the halo and the window boundary is well-shaped).
    // Skip points within the halo offset of the rim so they do not crowd the
    // halo anchors into competing slivers.
    let sub = 4usize;
    let n_sub = (jhi - jlo) * sub;
    let near_rim = |u: f64, v: f64| -> bool {
        rim_poly
            .iter()
            .any(|&(ru, rv)| (ru - u).abs() < 0.6 * halo_off && (rv - v).abs() < 0.6 * halo_off)
    };
    for jj in 1..n_sub {
        let v = v_lo + (v_hi - v_lo) * jj as f64 / n_sub as f64;
        for c in (c_lo + 1)..c_hi {
            let u = grid_us[c];
            if uv_point_in_polygon(u, v, &rim_poly) || near_rim(u, v) {
                continue;
            }
            lpush(u, v, Loc::Interior);
        }
    }

    let mut contours: Vec<Vec<usize>> = Vec::with_capacity(2);
    contours.push(
        outer
            .iter()
            .copied()
            .chain(std::iter::once(outer[0]))
            .collect(),
    );
    contours.push(rim.iter().copied().chain(std::iter::once(rim[0])).collect());

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        cdt::triangulate_contours(&luv, &contours)
    }));
    let tris = match outcome {
        Ok(Ok(t)) => t,
        _ => return false,
    };
    if tris.is_empty() {
        return false;
    }

    let mut vidx: Vec<Option<u32>> = vec![None; luv.len()];
    let mut resolve =
        |k: usize, mesh: &mut TriangleMesh, vidx: &mut Vec<Option<u32>>| -> Option<u32> {
            if let Some(v) = vidx[k] {
                return Some(v);
            }
            let (u, vv) = luv[k];
            let id = match lloc[k] {
                Loc::Vid(id) => id,
                Loc::Pos(p) => {
                    let normal = surface
                        .normal_at(to_surface_u(u), vv)
                        .map(|nn| nn * orient_sign)
                        .unwrap_or(Vector3::Z);
                    mesh.add_vertex(MeshVertex {
                        position: p,
                        normal,
                        uv: None,
                    })
                }
                Loc::Interior => {
                    let p = surface.point_at(to_surface_u(u), vv).ok()?;
                    let normal = surface
                        .normal_at(to_surface_u(u), vv)
                        .map(|nn| nn * orient_sign)
                        .unwrap_or(Vector3::Z);
                    mesh.add_vertex(MeshVertex {
                        position: p,
                        normal,
                        uv: None,
                    })
                }
            };
            vidx[k] = Some(id);
            Some(id)
        };
    for (a, b, c) in tris {
        let (Some(ia), Some(ib), Some(ic)) = (
            resolve(a, mesh, &mut vidx),
            resolve(b, mesh, &mut vidx),
            resolve(c, mesh, &mut vidx),
        ) else {
            return false;
        };
        emit_outward_triangle(mesh, ia, ib, ic);
    }
    true
}

/// The v-coordinate of structured band-row `j` within a window spanning rows
/// `[jlo, jhi]` mapped to `[v_lo, v_hi]` — linear in the row index, matching the
/// `v_of` lattice the structured rows were built on.
fn v_of_row(j: usize, jlo: usize, jhi: usize, v_lo: f64, v_hi: f64) -> f64 {
    if jhi == jlo {
        return v_lo;
    }
    v_lo + (v_hi - v_lo) * (j - jlo) as f64 / (jhi - jlo) as f64
}

/// Triangulate the strip between two CLOSED rings of mesh-vertex indices that are
/// phase-aligned (both start at the seam u=0 and wind the same way around u). The
/// rings may have different point counts; advance whichever ring's next vertex
/// has the smaller parametric fraction, emitting one triangle per step, until
/// both rings are fully consumed (wrapping closed). Each triangle is wound so its
/// geometric normal agrees with the rings' (already outward-oriented) vertex
/// normals.
/// Tessellate a shell collar wall: a `RuledSurface` whose outer loop and single
/// inner loop are each ONE closed rim edge (an `offset_solid` curved-cap wall).
/// Stitches the two cached rim rings into a watertight band. Returns `false`
/// WITHOUT touching `mesh` when the face is not this exact structure (so the
/// caller falls through to the generic routing); on success, emits the band and
/// returns `true`.
fn tessellate_ruled_annular_band(
    face: &Face,
    model: &BRepModel,
    cache: &EdgeSampleCache,
    mesh: &mut TriangleMesh,
) -> bool {
    // Exactly one inner loop, and both loops a single closed edge.
    if face.inner_loops.len() != 1 {
        return false;
    }
    let outer = match model.loops.get(face.outer_loop) {
        Some(l) => l,
        None => return false,
    };
    let inner = match model.loops.get(face.inner_loops[0]) {
        Some(l) => l,
        None => return false,
    };
    if outer.edges.len() != 1 || inner.edges.len() != 1 {
        return false;
    }
    let outer_edge_id = outer.edges[0];
    let inner_edge_id = inner.edges[0];
    let closed = |eid| {
        model
            .edges
            .get(eid)
            .map(|e| e.start_vertex == e.end_vertex)
            .unwrap_or(false)
    };
    if !closed(outer_edge_id) || !closed(inner_edge_id) {
        return false;
    }

    // Cached rim rings (forward param direction; last sample duplicates the
    // seam, drop it so the stitch wraps cleanly).
    let outer_s = cache.get_or_compute(outer_edge_id, model);
    let inner_s = cache.get_or_compute(inner_edge_id, model);
    if outer_s.len() < 4 || inner_s.len() < 4 {
        return false;
    }
    let outer_ring = &outer_s[..outer_s.len() - 1];
    let inner_ring = &inner_s[..inner_s.len() - 1];

    // Per-vertex outward normal: the ruled-band surface normal, oriented by the
    // face. A degenerate sample (e.g. on a sliver collar) falls back to the
    // ring-plane normal estimated from the two ring centroids.
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return false,
    };
    let orient_sign = face.orientation.sign();
    let centroid = |ring: &[Point3]| {
        let mut c = Point3::ZERO;
        for p in ring {
            c = c + *p;
        }
        c * (1.0 / ring.len() as f64)
    };
    let oc = centroid(outer_ring);
    let ic = centroid(inner_ring);
    let band_axis = (ic - oc).normalize().unwrap_or(Vector3::Z);

    let ring_row = |mesh: &mut TriangleMesh, ring: &[Point3]| -> Vec<u32> {
        ring.iter()
            .map(|p| {
                let normal = surface
                    .closest_point(p, Tolerance::default())
                    .and_then(|(u, v)| surface.normal_at(u, v))
                    .ok()
                    .and_then(|n| n.normalize().ok())
                    .map(|n| n * orient_sign)
                    .unwrap_or_else(|| {
                        // Radial-from-axis fallback through the band centroid.
                        let mid = Point3::new(
                            0.5 * (oc.x + ic.x),
                            0.5 * (oc.y + ic.y),
                            0.5 * (oc.z + ic.z),
                        );
                        let radial = *p - mid;
                        let along = radial.dot(&band_axis);
                        (radial - band_axis * along)
                            .normalize()
                            .map(|r| r * orient_sign)
                            .unwrap_or(Vector3::Z)
                    });
                mesh.add_vertex(MeshVertex {
                    position: *p,
                    normal,
                    uv: None,
                })
            })
            .collect()
    };

    let outer_idx = ring_row(mesh, outer_ring);
    let inner_idx = ring_row(mesh, inner_ring);
    stitch_closed_rings(&outer_idx, &inner_idx, mesh);
    true
}

fn stitch_closed_rings(lo: &[u32], hi: &[u32], mesh: &mut TriangleMesh) {
    let p = lo.len();
    let q = hi.len();
    if p < 2 || q < 2 {
        return;
    }
    let mut i = 0usize;
    let mut j = 0usize;
    while i < p || j < q {
        let fi = i as f64 / p as f64;
        let fj = j as f64 / q as f64;
        let advance_lo = if i >= p {
            false
        } else if j >= q {
            true
        } else {
            fi <= fj
        };
        let (a, b, c) = if advance_lo {
            // Triangle lo[i] → lo[i+1] → hi[j].
            (lo[i % p], lo[(i + 1) % p], hi[j % q])
        } else {
            // Triangle lo[i] → hi[j+1] → hi[j].
            (lo[i % p], hi[(j + 1) % q], hi[j % q])
        };
        emit_outward_triangle(mesh, a, b, c);
        if advance_lo {
            i += 1;
        } else {
            j += 1;
        }
    }
}

/// Emit a triangle wound so its geometric normal agrees with the average of its
/// vertices' (outward-oriented) stored normals.
fn emit_outward_triangle(mesh: &mut TriangleMesh, a: u32, b: u32, c: u32) {
    let pa = mesh.vertices[a as usize].position;
    let pb = mesh.vertices[b as usize].position;
    let pc = mesh.vertices[c as usize].position;
    let navg = mesh.vertices[a as usize].normal
        + mesh.vertices[b as usize].normal
        + mesh.vertices[c as usize].normal;
    let gn = (pb - pa).cross(&(pc - pa));
    if gn.dot(&navg) >= 0.0 {
        mesh.add_triangle(a, b, c);
    } else {
        mesh.add_triangle(a, c, b);
    }
}

/// SADDLE-35: mesh a cylinder-lateral ANNULUS emitted by
/// `split_cylinder_lateral_by_saddle` — its outer loop is a full-period rim ring
/// and its single inner loop is a full-period WAVY ring of saddle-ellipse arcs
/// (the Steinmetz lens boundary on this wall). Both loops ENCIRCLE the axis, so
/// NEITHER closes into a simple polygon in the cylinder's unrolled `(θ, z)` chart;
/// curved-CDT (which needs a UV-closed boundary) therefore emitted an empty mesh,
/// leaving the two annuli as open holes — the residual `closed=false` /
/// 1408-open-edge state on the otherwise brep-valid saddle solid.
///
/// Since both loops are full-period cylinder rings, the band between them is meshed
/// by directly STITCHING the two cached rims — the same conforming closed-ring
/// stitch the ruled-annular collar (`tessellate_ruled_annular_band`) and the
/// blend-torus stitcher use. It is watertight BY CONSTRUCTION: every rim sample is
/// drawn from the shared `EdgeSampleCache` in canonical loop-edge order, so the
/// wavy ring coincides sample-for-sample with the mating saddle wall's copy (the
/// C+D shared arcs) and the outer rim with the adjacent planar box face — the weld
/// then collapses the coincident boundary vertices with no residual crack.
///
/// Gated tightly on the saddle signature: a `Cylinder` face with exactly one inner
/// loop that (a) carries an analytic `Ellipse` arc and (b) encircles the axis. A
/// plain wall (no hole), a local circular pocket, or a local elliptical (angled-
/// plane) hole all fail (a) or (b) and keep routing through curved-CDT, so no other
/// cylinder face is affected.
fn tessellate_cylinder_saddle_annulus(
    face: &Face,
    model: &BRepModel,
    cache: &EdgeSampleCache,
    mesh: &mut TriangleMesh,
) -> bool {
    use crate::primitives::curve::Ellipse;
    use crate::primitives::surface::Cylinder;

    if face.inner_loops.len() != 1 {
        return false;
    }
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return false,
    };
    let cyl = match surface.as_any().downcast_ref::<Cylinder>() {
        Some(c) => c,
        None => return false,
    };
    let axis = cyl.axis;
    let origin = cyl.origin;

    let outer_loop = match model.loops.get(face.outer_loop) {
        Some(l) => l,
        None => return false,
    };
    let inner_loop = match model.loops.get(face.inner_loops[0]) {
        Some(l) => l,
        None => return false,
    };

    // Saddle marker (a): the inner lens-boundary loop carries analytic Ellipse arcs.
    let inner_has_ellipse = inner_loop.edges.iter().any(|&eid| {
        model
            .edges
            .get(eid)
            .and_then(|e| model.curves.get(e.curve_id))
            .map(|c| c.as_any().downcast_ref::<Ellipse>().is_some())
            .unwrap_or(false)
    });
    if !inner_has_ellipse {
        return false;
    }

    // Cached rings in canonical loop-edge order — the SAME shared polyline the
    // mating wall / adjacent planar face sample, so the weld is exact.
    let mut outer_ring: Vec<Point3> = Vec::new();
    sample_loop_3d_polygon(outer_loop, model, cache, &mut outer_ring);
    let mut inner_ring: Vec<Point3> = Vec::new();
    sample_loop_3d_polygon(inner_loop, model, cache, &mut inner_ring);
    if outer_ring.len() < 3 || inner_ring.len() < 3 {
        return false;
    }

    // θ frame ⟂ axis (identical construction to the saddle splitter).
    let seed = if axis.x.abs() < 0.9 {
        Vector3::new(1.0, 0.0, 0.0)
    } else {
        Vector3::new(0.0, 1.0, 0.0)
    };
    let u1 = match axis.cross(&seed).normalize() {
        Ok(v) => v,
        Err(_) => return false,
    };
    let u2 = axis.cross(&u1);
    let two_pi = std::f64::consts::TAU;
    let theta_of = |p: &Point3| -> f64 {
        let d = *p - origin;
        d.dot(&u2).atan2(d.dot(&u1)).rem_euclid(two_pi)
    };

    // Marker (b): both rings must ENCIRCLE the axis (full period → no angular gap
    // ≥ π). A local hole leaves a > π gap and is declined so it keeps routing
    // through curved-CDT.
    let encircles = |ring: &[Point3]| -> bool {
        let mut ths: Vec<f64> = ring.iter().map(theta_of).collect();
        ths.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mut max_gap = ths[0] + two_pi - ths[ths.len() - 1];
        for w in ths.windows(2) {
            max_gap = max_gap.max(w[1] - w[0]);
        }
        max_gap < std::f64::consts::PI
    };
    if !encircles(&outer_ring) || !encircles(&inner_ring) {
        return false;
    }

    // Wind both rings the SAME angular direction, then rotate the inner ring's
    // start to the outer ring's start angle. Reversal + cyclic rotation preserve
    // the UNDIRECTED boundary polyline (weld unaffected); they only re-seed the
    // fractional stitch so the band is not triangulated with a half-turn twist.
    let signed_sweep = |ring: &[Point3]| -> f64 {
        let mut acc = 0.0;
        for i in 0..ring.len() {
            let a = theta_of(&ring[i]);
            let b = theta_of(&ring[(i + 1) % ring.len()]);
            let mut d = b - a;
            while d > std::f64::consts::PI {
                d -= two_pi;
            }
            while d < -std::f64::consts::PI {
                d += two_pi;
            }
            acc += d;
        }
        acc
    };
    if signed_sweep(&outer_ring) * signed_sweep(&inner_ring) < 0.0 {
        inner_ring.reverse();
    }
    let start_theta = theta_of(&outer_ring[0]);
    let ang_dist = |t: f64| -> f64 {
        let d = (t - start_theta).rem_euclid(two_pi);
        d.min(two_pi - d)
    };
    if let Some((k, _)) = inner_ring
        .iter()
        .enumerate()
        .map(|(i, p)| (i, ang_dist(theta_of(p))))
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    {
        inner_ring.rotate_left(k);
    }

    // Per-vertex outward normals from the cylinder surface, oriented by the face.
    let orient_sign = face.orientation.sign();
    let ring_row = |mesh: &mut TriangleMesh, ring: &[Point3]| -> Vec<u32> {
        ring.iter()
            .map(|p| {
                let normal = surface
                    .closest_point(p, Tolerance::default())
                    .and_then(|(u, v)| surface.normal_at(u, v))
                    .ok()
                    .and_then(|n| n.normalize().ok())
                    .map(|n| n * orient_sign)
                    .unwrap_or_else(|| {
                        // Radial-from-axis fallback (ill-conditioned closest_point
                        // near the axis-symmetric crossing generators).
                        let d = *p - origin;
                        let along = d.dot(&axis);
                        (d - axis * along)
                            .normalize()
                            .map(|radial| radial * orient_sign)
                            .unwrap_or(Vector3::Z)
                    });
                mesh.add_vertex(MeshVertex {
                    position: *p,
                    normal,
                    uv: None,
                })
            })
            .collect()
    };
    let outer_idx = ring_row(mesh, &outer_ring);
    let inner_idx = ring_row(mesh, &inner_ring);
    stitch_closed_rings(&outer_idx, &inner_idx, mesh);
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

    // Winding reconciliation. The grid quad winds (v0,v1,v2) CCW in the
    // (u, v) lattice, so its 3D geometric normal is +(∂P/∂u × ∂P/∂v). That
    // is NOT guaranteed to agree with the face's oriented outward normal: a
    // fillet surface's parametric chart can be left- or right-handed
    // relative to its own `normal` field. `CylindricalFillet` in particular
    // sign-flips its frame_y to keep the blend arc the MINOR arc (see
    // fillet_surfaces.rs frame construction), so du×dv points *opposite* the
    // outward radial normal for ~half the edges of an all-edges box fillet —
    // winding on `orientation` alone then tessellates those faces inward
    // (FILLET-MULTIEDGE-VOLUME: half the cylinders cancelled in the
    // divergence-theorem volume). The curved-CDT path already corrects this
    // via `compute_chart_sign`; fillet faces route here instead, so apply
    // the same correction: keep the CCW grid winding iff the chart handedness
    // `sign(du×dv · normal)` matches the face orientation, making the emitted
    // 3D normal equal `surface.normal · orientation.sign()`.
    let ((cu_min, cu_max), (cv_min, cv_max)) = surface.parameter_bounds();
    let chart_sign = match surface.evaluate_full(0.5 * (cu_min + cu_max), 0.5 * (cv_min + cv_max)) {
        Ok(sp) if sp.du.cross(&sp.dv).dot(&sp.normal) < 0.0 => -1i32,
        _ => 1i32,
    };
    let keep = (chart_sign == 1) == face.orientation.is_forward();
    for v_idx in 0..v_steps {
        for u_idx in 0..u_steps {
            let v0 = vertex_grid[v_idx][u_idx];
            let v1 = vertex_grid[v_idx][u_idx + 1];
            let v2 = vertex_grid[v_idx + 1][u_idx];
            let v3 = vertex_grid[v_idx + 1][u_idx + 1];
            if let (Some(v0), Some(v1), Some(v2), Some(v3)) = (v0, v1, v2, v3) {
                if keep {
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

    // Winding reconciliation. The grid quad winds (v0,v1,v2) CCW in the
    // (u, v) lattice, so its 3D geometric normal is +(∂P/∂u × ∂P/∂v). That
    // is NOT guaranteed to agree with the face's oriented outward normal: a
    // fillet surface's parametric chart can be left- or right-handed
    // relative to its own `normal` field. `CylindricalFillet` in particular
    // sign-flips its frame_y to keep the blend arc the MINOR arc (see
    // fillet_surfaces.rs frame construction), so du×dv points *opposite* the
    // outward radial normal for ~half the edges of an all-edges box fillet —
    // winding on `orientation` alone then tessellates those faces inward
    // (FILLET-MULTIEDGE-VOLUME: half the cylinders cancelled in the
    // divergence-theorem volume). The curved-CDT path already corrects this
    // via `compute_chart_sign`; fillet faces route here instead, so apply
    // the same correction: keep the CCW grid winding iff the chart handedness
    // `sign(du×dv · normal)` matches the face orientation, making the emitted
    // 3D normal equal `surface.normal · orientation.sign()`.
    let ((cu_min, cu_max), (cv_min, cv_max)) = surface.parameter_bounds();
    let chart_sign = match surface.evaluate_full(0.5 * (cu_min + cu_max), 0.5 * (cv_min + cv_max)) {
        Ok(sp) if sp.du.cross(&sp.dv).dot(&sp.normal) < 0.0 => -1i32,
        _ => 1i32,
    };
    let keep = (chart_sign == 1) == face.orientation.is_forward();
    for v_idx in 0..v_steps {
        for u_idx in 0..u_steps {
            let v0 = vertex_grid[v_idx][u_idx];
            let v1 = vertex_grid[v_idx][u_idx + 1];
            let v2 = vertex_grid[v_idx + 1][u_idx];
            let v3 = vertex_grid[v_idx + 1][u_idx + 1];
            if let (Some(v0), Some(v1), Some(v2), Some(v3)) = (v0, v1, v2, v3) {
                if keep {
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
pub(crate) fn is_point_inside_face(u: f64, v: f64, face: &Face, model: &BRepModel) -> bool {
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

    // Cell size ≈ the weld tolerance. The dedup only needs coincident
    // (within-`safe_tol`) vertices to fall within the 3×3×3 neighbourhood
    // scanned below, which holds whenever `grid_size >= safe_tol` (a
    // within-tol pair is ≤ 1 cell apart per axis). A SMALL multiple keeps
    // that guarantee while keeping buckets SPARSE.
    //
    // NO-HANGS (fine-mesh weld blowup): the previous `safe_tol * 1e3` made
    // cells ~1000× the tolerance, so on a dense `fine()` mesh (vertex
    // spacing ~`max_edge_length` 0.01, tol 0.0001 → cells 0.1) every cell
    // held ~1000 vertices. The 3×3×3 scan then degraded to ~O(n·1000),
    // i.e. quasi-quadratic: a 150k-triangle fillet corner-cap fragment
    // (~76k verts) took minutes, wedging the api-server write lock via the
    // mass-props `fine()` tessellation. Sizing cells at the tolerance keeps
    // buckets to a handful (only bit-exact seam duplicates collide), so the
    // weld is genuinely O(n). The `tol_sq` membership test below is
    // unchanged, so the set of welded pairs — and the output mesh — is
    // byte-identical; only the neighbourhood search is faster.
    // Floor at 1e-9 to avoid pathological 0/negative tolerances collapsing
    // every vertex onto the origin cell.
    let safe_tol = weld_tolerance.max(1e-9);
    let grid_size = safe_tol * 2.0;
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

    // Two coincident samples weld into one mesh vertex only when their
    // surface normals also agree — i.e. they sit on a SMOOTH seam (a
    // cylinder/sphere/NURBS u- or v-wrap, or two G1-adjacent faces),
    // where a single shared vertex with one (averaged) normal is exactly
    // right. At a SHARP feature edge — a box edge, a cone/cylinder cap rim,
    // any dihedral — the two faces' normals diverge, so welding them would
    // force the shared vertex to carry ONE face's normal and shade the other
    // face's triangles as if they faced the wrong way (a box's four side
    // faces inheriting a cap's ±axis normal — the bug the tessellation
    // normal-agreement oracle catches). Keeping the coincident samples as
    // distinct vertices there preserves each face's correct normal; the mesh
    // stays *geometrically* watertight (the samples are bit-exact coincident),
    // and every consumer that needs shared-index topology — the manifold
    // oracle, STL export (triangle soup), BVH — re-welds by position anyway,
    // so nothing downstream depends on the sharp seam being index-welded.
    //
    // `cos 60°`: a smooth seam's adjacent normals are near-parallel (dot ≈ 1);
    // a genuine feature edge is ≥ 60° (a box edge is 90°, dot 0; a cap rim is
    // obtuse, dot < 0). The gate splits the latter and merges the former,
    // leaving every smooth-seam weld (the watertight curved-primitive path)
    // bit-for-bit unchanged.
    const WELD_NORMAL_DOT_MIN: f64 = 0.5;

    // remap[i] = canonical index for vertex i, only meaningful for
    // i >= v_start. Earlier vertices are identity-mapped (we don't
    // touch them).
    let mut remap: Vec<u32> = (0..n as u32).collect();

    for i in v_start..n {
        let pos = mesh.vertices[i].position;
        let ni = mesh.vertices[i].normal;
        let (cx, cy, cz) = to_cell(pos);

        // Scan the 3×3×3 neighbourhood. Stop at the first vertex with
        // a strictly-smaller original index (still inside the welding
        // range — `cand >= v_start`) that is within tolerance AND whose
        // normal agrees (smooth seam) — we keep the lowest such index as
        // canonical, a deterministic mapping regardless of insertion order.
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
                            if dp.dot(&dp) <= tol_sq
                                && ni.dot(&mesh.vertices[cand as usize].normal)
                                    >= WELD_NORMAL_DOT_MIN
                            {
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
    // Remove DOUBLED FACETS in the welded range: two triangles sharing the
    // same three (welded) vertices. An opposite-winding pair is a degenerate
    // "fin" that contributes zero surface yet makes every one of its edges
    // border 4 triangles → non-manifold; this is how a curved-CDT sliver
    // emitted TWICE at high density leaves an otherwise-sound solid's mesh
    // non-manifold (KNOWN_BUGS #65). Cancel opposite-winding pairs (drop
    // BOTH — a fin sits on top of the real tiling, which still covers the
    // patch), and collapse same-winding exact duplicates to one
    // representative. This is a no-op on a clean mesh — every facet's
    // vertex-triple is unique — so watertight primitives stay bit-for-bit
    // identical.
    let mut doubled_removed = 0usize;
    {
        let parity = |t: &[u32; 3]| -> bool {
            // true = even permutation of the sorted triple (one winding),
            // false = the opposite. Degenerate triangles (two equal indices)
            // were already dropped above.
            let inv = (t[0] > t[1]) as u8 + (t[0] > t[2]) as u8 + (t[1] > t[2]) as u8;
            inv % 2 == 0
        };
        let mut groups: HashMap<[u32; 3], Vec<usize>> = HashMap::new();
        for i in t_start..new_triangles.len() {
            let mut k = new_triangles[i];
            k.sort_unstable();
            groups.entry(k).or_default().push(i);
        }
        let mut remove = vec![false; new_triangles.len()];
        for idxs in groups.values() {
            if idxs.len() < 2 {
                continue;
            }
            // Greedily pair opposite-winding indices (cancel both); keep one
            // representative of any same-winding surplus so the patch stays
            // covered.
            let mut even: Vec<usize> = Vec::new();
            let mut odd: Vec<usize> = Vec::new();
            for &i in idxs {
                if parity(&new_triangles[i]) {
                    if let Some(j) = odd.pop() {
                        remove[i] = true;
                        remove[j] = true;
                    } else {
                        even.push(i);
                    }
                } else if let Some(j) = even.pop() {
                    remove[i] = true;
                    remove[j] = true;
                } else {
                    odd.push(i);
                }
            }
            for &extra in even.iter().chain(odd.iter()).skip(1) {
                remove[extra] = true;
            }
        }
        doubled_removed = remove.iter().filter(|&&r| r).count();
        if doubled_removed > 0 {
            let mut tris: Vec<[u32; 3]> = Vec::with_capacity(new_triangles.len());
            let mut fmap: Vec<u32> = if has_face_map {
                Vec::with_capacity(new_face_map.len())
            } else {
                Vec::new()
            };
            for i in 0..new_triangles.len() {
                if remove[i] {
                    continue;
                }
                tris.push(new_triangles[i]);
                if has_face_map {
                    fmap.push(new_face_map[i]);
                }
            }
            new_triangles = tris;
            if has_face_map {
                new_face_map = fmap;
            }
        }
    }

    mesh.triangles = new_triangles;
    if has_face_map {
        mesh.face_map = new_face_map;
    }

    if welded > 0 || g1_smoothed > 0 || doubled_removed > 0 {
        tracing::debug!(
            "weld_mesh_watertight_range: collapsed {welded} duplicate vertices, \
             G1-smoothed {g1_smoothed} canonical normals, removed {doubled_removed} \
             doubled-facet triangles (tol={weld_tolerance:e}, v_start={v_start})"
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
    fn planar_face_square_with_circular_hole() {
        // TESS-ANNULAR-CAP repro: 80×80 outer square (CCW) with a centred Ø24
        // circular hole (32-gon, CW). This is the bored-plate cap. Expected face
        // area = 6400 − π·12² ≈ 5947.6. The live bug over-covers to ~8320 (> the
        // outer square 6400) — overlapping triangles fill the bore.
        let r = 12.0_f64;
        let n = 32usize;
        let hole: Vec<(f64, f64)> = (0..n)
            .map(|k| {
                // CW (negative angle) so the hole winds opposite the CCW outer.
                let a = -(k as f64) / (n as f64) * std::f64::consts::TAU;
                (40.0 + r * a.cos(), 40.0 + r * a.sin())
            })
            .collect();
        let (verts, loops) = build_planar_loops(
            &[(0.0, 0.0), (80.0, 0.0), (80.0, 80.0), (0.0, 80.0)],
            &[&hole],
        );
        let tris = triangulate_planar_polygon(&verts, &loops, &Vector3::Z);
        let area = total_tri_area_xy(&verts, &tris);
        let expected = 80.0 * 80.0 - std::f64::consts::PI * r * r;
        // The hole is a 32-gon, so the true polygonal area is slightly under πr².
        let poly_hole = 0.5 * (n as f64) * r * r * (std::f64::consts::TAU / n as f64).sin();
        let expected_poly = 80.0 * 80.0 - poly_hole;
        assert!(
            (area - expected_poly).abs() < 1.0,
            "square+circular-hole tri area {area} ≠ expected {expected_poly:.1} \
             (analytic annulus {expected:.1}); >outer-square 6400 ⇒ overlap"
        );
        for &t in &tris {
            let (cx, cy) = tri_centroid_xy(&verts, t);
            let inside_hole = ((cx - 40.0).powi(2) + (cy - 40.0).powi(2)).sqrt() < r * 0.9;
            assert!(
                !inside_hole,
                "triangle centroid ({cx:.1},{cy:.1}) lies inside the bore — hole not erased"
            );
        }
    }

    /// TESS-ANNULAR-CAP regression: the real bored-plate cap (a SQUARE outer +
    /// circular bore hole) must tessellate to the correct annulus area. The bug:
    /// `annulus_radial_strip` mis-classified the 4-corner square as a circular
    /// ring (its corners are equidistant from the centroid) and radial-stripped
    /// it to the bore, over-covering the cap to area 8320 (vs 5948) and inflating
    /// the bored solid's volume to 107817 (vs 95162). Fixed by the chord<radius
    /// guard in `circular`. Gate on cap AREA (no test checked it before — which is
    /// why a watertight-but-wrong mesh hid).
    #[test]
    fn bored_plate_caps_tessellate_to_annulus() {
        use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
        use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
        let mut m = BRepModel::new();
        let plate = match TopologyBuilder::new(&mut m)
            .create_box_3d(80.0, 80.0, 16.0)
            .unwrap()
        {
            GeometryId::Solid(s) => s,
            o => panic!("{o:?}"),
        };
        let bore = match TopologyBuilder::new(&mut m)
            .create_cylinder_3d(
                Point3::new(0.0, 0.0, -10.0),
                Vector3::new(0.0, 0.0, 1.0),
                12.0,
                36.0,
            )
            .unwrap()
        {
            GeometryId::Solid(s) => s,
            o => panic!("{o:?}"),
        };
        let holed = boolean_operation(
            &mut m,
            plate,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .unwrap();
        let params = TessellationParams::default();
        let cache = EdgeSampleCache::new(&params);
        let solid = m.solids.get(holed).unwrap();
        let mut shells = vec![solid.outer_shell];
        shells.extend_from_slice(&solid.inner_shells);
        let mut caps = 0;
        for sh in shells {
            let shell = m.shells.get(sh).unwrap();
            for &fid in &shell.faces {
                let face = m.faces.get(fid).unwrap();
                let surf = m.surfaces.get(face.surface_id).unwrap();
                if surf.type_name() != "Plane" {
                    continue;
                }
                let n = face.normal_at(0.5, 0.5, &m.surfaces).unwrap_or(Vector3::Z);
                if n.z.abs() < 0.9 || face.inner_loops.is_empty() {
                    continue; // only the horizontal caps that carry the bore
                }
                let mut mesh = TriangleMesh::new();
                tessellate_planar_face(face, &m, &params, &cache, &mut mesh);
                let area: f64 = mesh
                    .triangles
                    .iter()
                    .map(|t| {
                        let a = mesh.vertices[t[0] as usize].position;
                        let b = mesh.vertices[t[1] as usize].position;
                        let c = mesh.vertices[t[2] as usize].position;
                        (b.to_vec() - a.to_vec())
                            .cross(&(c.to_vec() - a.to_vec()))
                            .magnitude()
                            * 0.5
                    })
                    .sum();
                let expected = 80.0 * 80.0 - std::f64::consts::PI * 12.0 * 12.0;
                assert!(
                    (area - expected).abs() < 5.0,
                    "bored cap {fid} area {area:.1} ≠ annulus {expected:.1} (over-cover bug)"
                );
                caps += 1;
            }
        }
        assert_eq!(caps, 2, "expected 2 bored caps, found {caps}");
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
