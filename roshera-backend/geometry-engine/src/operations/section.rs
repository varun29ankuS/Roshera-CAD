//! Plane-solid section: produce cap meshes that fill the cross-section
//! of a solid cut by an arbitrary plane.
//!
//! # Why kernel-side caps?
//!
//! Three.js (and every other GPU-clipping renderer) supports clipping
//! planes that discard fragments on one side of a plane, but the cut
//! through a solid is rendered as a *hole*, not a filled cross-section:
//! back-facing inner walls leak through the opening, so the solid
//! appears hollow. The standard fix in mechanical-CAD viewers
//! (SolidWorks, Fusion, NX) is to draw a filled polygon at every
//! intersection of the cutting plane with a solid — the "section cap".
//!
//! The cap is geometry, not a rendering trick. Drawing exports,
//! measurement tools, hatching, and downstream AI all need access to
//! it as a typed polygon. This module computes that polygon from the
//! actual B-Rep — never from a screen-space stencil.
//!
//! # Algorithm
//!
//! For each face of the solid's shells:
//!
//! 1. Intersect the face's underlying surface with the cutting plane
//!    via [`crate::math::surface_plane_intersection::intersect_surface_plane`].
//! 2. Trim the resulting parametric curves to the face's parameter
//!    domain. Tier-1 analytic primitives (Plane, Cylinder, Sphere,
//!    Cone, Torus) have axis-aligned rectangular UV faces in the
//!    current B-Rep construction pipeline, so the UV-bbox test from
//!    [`get_face_parameter_bounds`] is exact and cheap. Tier-2
//!    parametric surfaces (NURBS, B-Spline, Ruled, Offset,
//!    SurfaceOfRevolution) carry arbitrary parameter-space trim loops,
//!    so each candidate sample is point-in-face-tested via
//!    [`point_inside_face_uv`] (winding number on the loop's UV
//!    projection). Face entry/exit points are located by 30-step
//!    bisection on the segment between consecutive samples — linear
//!    bbox-edge interpolation isn't applicable because trim curves in
//!    UV are in general curved.
//!
//! Then globally:
//!
//! 3. Chain trimmed polyline fragments end-to-end with a spatial hash
//!    keyed on quantized endpoints. The plane-solid intersection of a
//!    manifold solid is by construction a set of closed loops — open
//!    chains are dropped with a tracing warn (partial caps still ship).
//! 4. Project each closed loop to 2D in the cutting plane's tangent
//!    basis using [`crate::tessellation::adaptive::compute_plane_axes`].
//! 5. Classify outer loops vs holes by signed-area sign and pairwise
//!    point-in-polygon nesting (even depth = outer, odd = hole).
//! 6. Triangulate each (outer + holes) group via
//!    [`crate::tessellation::surface::triangulate_planar_polygon`].
//! 7. Lift the 2D vertices back into 3D and emit one [`SectionCap`]
//!    per top-level outer loop.
//!
//! Cap vertex normals are all `plane_normal` (already normalised on
//! entry); callers that want the cap to face the *visible* half-space
//! after a section "flip" simply negate this once on the receiving end.

use crate::math::surface_plane_intersection::{
    intersect_surface_plane, ParametricIntersectionCurve, ParametricIntersectionPoint,
    SurfacePlaneIntersectionConfig,
};
use crate::math::{Point3, Tolerance, Vector3};
use crate::operations::{OperationError, OperationResult};
use crate::primitives::face::FaceId;
use crate::primitives::shell::ShellId;
use crate::primitives::solid::SolidId;
use crate::primitives::surface::SurfaceType;
use crate::primitives::topology_builder::BRepModel;
use crate::tessellation::adaptive::compute_plane_axes;
use crate::tessellation::surface::{
    get_face_parameter_bounds, point_inside_face_uv, triangulate_planar_polygon,
};

/// Result of intersecting a single solid with a plane. One cap per
/// top-level closed outer loop (a solid that the plane cuts through
/// twice — two separate boxes joined logically by the cut — yields
/// two caps).
///
/// Vertices are 3D points lying exactly on the cutting plane. Indices
/// are triangle triples into `vertices`. Normals are per-vertex but
/// all identical to `plane_normal` since the cap is planar.
#[derive(Debug, Clone)]
pub struct SectionCap {
    /// Source solid this cap was produced from.
    pub solid_id: SolidId,
    /// Cutting plane origin (one of the input points used at section
    /// time; preserved for traceability / regen).
    pub plane_origin: Point3,
    /// Cutting plane normal, normalised on construction.
    pub plane_normal: Vector3,
    /// Vertex positions (3D, on the plane).
    pub vertices: Vec<Point3>,
    /// Triangle indices into `vertices`.
    pub indices: Vec<[u32; 3]>,
    /// Per-vertex normals (constant = `plane_normal`).
    pub normals: Vec<Vector3>,
}

/// Cut a solid by an arbitrary plane and produce triangulated cap
/// meshes filling every closed cross-section loop.
///
/// Returns `Ok(vec![])` when the plane misses the solid entirely
/// (no zero-crossings on any face surface). Returns
/// `Err(OperationError::InvalidInput { … })` when `plane_normal` is
/// degenerate (zero-length).
///
/// Failures inside the per-face intersection or chaining steps are
/// logged via `tracing::warn` and the partial cap set is returned;
/// section preview is a non-mutating display operation and degrading
/// gracefully beats failing the whole call.
pub fn section_solid_by_plane(
    model: &BRepModel,
    solid_id: SolidId,
    plane_origin: Point3,
    plane_normal: Vector3,
    tolerance: Tolerance,
) -> OperationResult<Vec<SectionCap>> {
    let normal = plane_normal
        .normalize()
        .map_err(|_| OperationError::InvalidInput {
            parameter: "plane_normal".to_string(),
            expected: "non-zero vector".to_string(),
            received: format!(
                "({:.6}, {:.6}, {:.6})",
                plane_normal.x, plane_normal.y, plane_normal.z
            ),
        })?;

    let solid = match model.get_solid(solid_id) {
        Some(s) => s,
        None => {
            return Err(OperationError::InvalidInput {
                parameter: "solid_id".to_string(),
                expected: "existing solid id".to_string(),
                received: format!("{}", solid_id),
            })
        }
    };

    // Walk every face on every shell (outer + inner / voids). Each face
    // contributes zero or more trimmed polyline fragments lying on the
    // cutting plane.
    let mut shells: Vec<ShellId> = Vec::with_capacity(1 + solid.inner_shells.len());
    shells.push(solid.outer_shell);
    shells.extend_from_slice(&solid.inner_shells);

    let mut fragments: Vec<Polyline3D> = Vec::new();
    for shell_id in shells {
        let shell = match model.shells.get(shell_id) {
            Some(s) => s,
            None => continue,
        };
        for face_id in &shell.faces {
            collect_face_fragments(model, *face_id, plane_origin, normal, &mut fragments);
        }
    }

    if fragments.is_empty() {
        return Ok(Vec::new());
    }

    // Chain fragments into closed 3D loops.
    let raw_loops = chain_fragments_into_loops(&fragments, &tolerance);
    // Dense marching-square output produces ~1000 collinear samples per
    // straight edge. CDT chokes on long collinear runs (no triangulation
    // is well-defined), so simplify each loop to its corners before
    // triangulation. The chord deviation threshold is
    // `tolerance.distance()` so curved arcs (cylinder caps) keep enough
    // resolution to render as smooth polygons.
    let simplify_eps = tolerance.distance().max(1e-6);
    let loops: Vec<Vec<Point3>> = raw_loops
        .into_iter()
        .map(|l| simplify_loop_rdp(&l, simplify_eps))
        .filter(|l| l.len() >= 3)
        .collect();
    if loops.is_empty() {
        return Ok(Vec::new());
    }

    // Project each loop to 2D in the cut plane and build caps.
    let (u_axis, v_axis) = compute_plane_axes(&normal);
    let projected: Vec<Loop2D> = loops
        .iter()
        .map(|loop3d| project_loop_to_2d(loop3d, plane_origin, u_axis, v_axis))
        .collect();

    // Dedup geometrically-equivalent loops. The marching-square seed
    // search inside `intersect_surface_plane` re-traces the same closed
    // curve once per grid cell that contains it (≥1 row of cells along
    // a u-periodic seam on a cylinder yields ~grid_resolution copies of
    // the same circle). Two loops are the same iff their signed areas
    // and bbox centres match within tolerance.
    let dedup_indices: Vec<usize> = dedup_loops_by_signature(&projected, simplify_eps);
    let projected: Vec<Loop2D> = dedup_indices
        .iter()
        .map(|&i| projected[i].clone())
        .collect();
    let loops: Vec<Vec<Point3>> = dedup_indices.iter().map(|&i| loops[i].clone()).collect();

    let nesting = classify_loop_nesting(&projected);

    let mut caps: Vec<SectionCap> = Vec::with_capacity(nesting.outers.len());
    for (outer_idx, hole_idxs) in &nesting.outers {
        let cap = triangulate_cap(
            solid_id,
            plane_origin,
            normal,
            *outer_idx,
            hole_idxs,
            &loops,
            &projected,
        );
        if let Some(cap) = cap {
            caps.push(cap);
        }
    }

    Ok(caps)
}

// ---------------------------------------------------------------------------
// Internal: per-face intersection + UV-bbox trim
// ---------------------------------------------------------------------------

/// 3D polyline emitted by trimming a `ParametricIntersectionCurve`
/// to a face's UV domain. A single source curve can produce multiple
/// disjoint trimmed polylines if it dips out of and back into the face
/// domain.
#[derive(Debug, Clone)]
struct Polyline3D {
    points: Vec<Point3>,
}

fn collect_face_fragments(
    model: &BRepModel,
    face_id: FaceId,
    plane_origin: Point3,
    plane_normal: Vector3,
    out: &mut Vec<Polyline3D>,
) {
    let face = match model.faces.get(face_id) {
        Some(f) => f,
        None => return,
    };
    let surface = match model.surfaces.get(face.surface_id) {
        Some(s) => s,
        None => return,
    };

    // Real face UV extent comes from lifting the loop's 3D edges back
    // into (u, v) via `surface.closest_point`. `face.uv_bounds` is a
    // normalised [0, 1] placeholder for most analytic faces and cannot
    // be trusted here; the tessellator already maintains the correct
    // loop-lifted version in `get_face_parameter_bounds`.
    let (u_min, u_max, v_min, v_max) = get_face_parameter_bounds(face, model);

    // Pad the search rectangle slightly so curves that touch the face
    // boundary tangentially still produce a sign change inside the
    // grid. 1% of the bbox side is plenty for tier-1 analytic surfaces.
    let pad_u = ((u_max - u_min) * 0.01).max(1e-6);
    let pad_v = ((v_max - v_min) * 0.01).max(1e-6);
    let config = SurfacePlaneIntersectionConfig {
        param_bounds_override: Some((
            (u_min - pad_u, u_max + pad_u),
            (v_min - pad_v, v_max + pad_v),
        )),
        ..Default::default()
    };

    let curves = match intersect_surface_plane(surface, plane_origin, plane_normal, &config) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                "section: intersect_surface_plane failed on face {}: {:?}",
                face_id,
                e
            );
            return;
        }
    };

    if curves.is_empty() {
        return;
    }

    // SEC.4: dispatch on surface type. Tier-1 analytic primitives whose
    // faces have axis-aligned rectangular parameter domains (Plane,
    // Cylinder, Sphere, Cone, Torus) can use the cheap UV-bbox trim.
    // Tier-2 surfaces whose faces can carry arbitrary parameter-space
    // trim loops (NURBS, B-Spline, Ruled, Offset, SurfaceOfRevolution)
    // need the full winding-number point-in-face test on each sample —
    // their UV bbox over-includes regions outside the face proper.
    //
    // Note: even tier-1 primitives can in principle carry non-rectangular
    // trim loops (e.g. a planar face with a circular hole). Those are
    // exercised by SEC.4's face-domain path for *parametric* surfaces;
    // analytic primitives in the current B-Rep construction pipeline
    // always emit rectangular UV faces, so we keep the fast path for
    // them. If that invariant breaks, the symptom is over-inclusion of
    // segments outside the face, not under-inclusion — caller-side
    // chaining will detect and drop the spurious fragments.
    let surface_kind = surface.surface_type();
    let needs_face_domain_trim = matches!(
        surface_kind,
        SurfaceType::BSpline
            | SurfaceType::NURBS
            | SurfaceType::Ruled
            | SurfaceType::Offset
            | SurfaceType::SurfaceOfRevolution
    );

    for curve in &curves {
        if needs_face_domain_trim {
            trim_curve_to_face(curve, face, model, u_min, u_max, v_min, v_max, out);
        } else {
            trim_curve_to_uv_bbox(curve, u_min, u_max, v_min, v_max, out);
        }
    }
}

/// Walk a parametric intersection curve and emit polylines for every
/// maximal run of consecutive points whose (u,v) lies inside the face's
/// UV bbox.
fn trim_curve_to_uv_bbox(
    curve: &ParametricIntersectionCurve,
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
    out: &mut Vec<Polyline3D>,
) {
    let inside = |p: &ParametricIntersectionPoint| -> bool {
        p.u >= u_min && p.u <= u_max && p.v >= v_min && p.v <= v_max
    };

    let mut current: Vec<Point3> = Vec::new();
    let mut prev_inside = false;
    let mut prev_pt: Option<&ParametricIntersectionPoint> = None;

    for sample in &curve.points {
        let now_inside = inside(sample);
        if now_inside && !prev_inside {
            // Boundary crossing: linearly interpolate to the bbox edge
            // so the fragment endpoint sits exactly on the face
            // boundary, where it can meet the neighbour-face fragment.
            if let Some(prev) = prev_pt {
                if let Some(boundary) =
                    clip_segment_to_bbox(prev, sample, u_min, u_max, v_min, v_max)
                {
                    current.push(boundary);
                }
            }
            current.push(sample.position);
        } else if now_inside && prev_inside {
            current.push(sample.position);
        } else if !now_inside && prev_inside {
            // Exiting: interpolate to bbox edge, close out fragment.
            if let Some(prev) = prev_pt {
                if let Some(boundary) =
                    clip_segment_to_bbox(prev, sample, u_min, u_max, v_min, v_max)
                {
                    current.push(boundary);
                }
            }
            if current.len() >= 2 {
                out.push(Polyline3D {
                    points: std::mem::take(&mut current),
                });
            } else {
                current.clear();
            }
        }
        prev_inside = now_inside;
        prev_pt = Some(sample);
    }

    if current.len() >= 2 {
        out.push(Polyline3D { points: current });
    }
}

/// Walk a parametric intersection curve and emit polylines for every
/// maximal run of consecutive points whose (u, v) lies *inside the
/// face's trim loops*, not just inside its UV bbox.
///
/// Used for tier-2 surfaces whose face parameter domains are
/// arbitrarily-shaped: NURBS / B-Spline trimmed faces, ruled / offset
/// composites, surfaces of revolution. The bbox-only `trim_curve_to_uv_bbox`
/// over-includes the entire rectangular hull of the face's loops, which
/// for a face with a circular hole or a concave outer trim wraps in
/// regions that shouldn't appear on the cap.
///
/// The bbox `(u_min..u_max, v_min..v_max)` is used as a fast-reject:
/// samples obviously outside the bbox can't be inside the face. Only
/// samples that pass the bbox check get the (more expensive)
/// `point_inside_face_uv` winding test.
///
/// Boundary interpolation at face entry/exit walks back to the previous
/// sample and binary-searches for the inside/outside transition — the
/// face boundary in parameter space is in general curved (a trimmed
/// NURBS face's edge can be a piecewise B-spline in (u, v)), so linear
/// bbox-edge interpolation isn't applicable. The bisection converges
/// to `tolerance.distance()` in ~30 iterations regardless of the
/// curve's local complexity.
#[allow(clippy::too_many_arguments)]
fn trim_curve_to_face(
    curve: &ParametricIntersectionCurve,
    face: &crate::primitives::face::Face,
    model: &BRepModel,
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
    out: &mut Vec<Polyline3D>,
) {
    let in_bbox = |p: &ParametricIntersectionPoint| -> bool {
        p.u >= u_min && p.u <= u_max && p.v >= v_min && p.v <= v_max
    };
    let inside_face = |p: &ParametricIntersectionPoint| -> bool {
        in_bbox(p) && point_inside_face_uv(p.u, p.v, face, model)
    };

    // Bisection on the parametric line segment between two samples to
    // localise the inside/outside transition. Returns the boundary
    // crossing 3D point (the sample at the boundary, where one side
    // tests inside-face and the other tests outside-face). At most 30
    // iterations — 2^-30 ≈ 1e-9 of the segment length, well below any
    // realistic UV scale.
    let boundary_3d =
        |a: &ParametricIntersectionPoint, b: &ParametricIntersectionPoint| -> Point3 {
            let mut t_lo = 0.0;
            let mut t_hi = 1.0;
            let a_inside = inside_face(a);
            for _ in 0..30 {
                let t_mid = 0.5 * (t_lo + t_hi);
                let u_mid = a.u + (b.u - a.u) * t_mid;
                let v_mid = a.v + (b.v - a.v) * t_mid;
                let mid_inside = u_mid >= u_min
                    && u_mid <= u_max
                    && v_mid >= v_min
                    && v_mid <= v_max
                    && point_inside_face_uv(u_mid, v_mid, face, model);
                if mid_inside == a_inside {
                    t_lo = t_mid;
                } else {
                    t_hi = t_mid;
                }
            }
            let t = 0.5 * (t_lo + t_hi);
            a.position + (b.position - a.position) * t
        };

    let mut current: Vec<Point3> = Vec::new();
    let mut prev_inside = false;
    let mut prev_pt: Option<&ParametricIntersectionPoint> = None;

    for sample in &curve.points {
        let now_inside = inside_face(sample);
        if now_inside && !prev_inside {
            if let Some(prev) = prev_pt {
                current.push(boundary_3d(prev, sample));
            }
            current.push(sample.position);
        } else if now_inside && prev_inside {
            current.push(sample.position);
        } else if !now_inside && prev_inside {
            if let Some(prev) = prev_pt {
                current.push(boundary_3d(prev, sample));
            }
            if current.len() >= 2 {
                out.push(Polyline3D {
                    points: std::mem::take(&mut current),
                });
            } else {
                current.clear();
            }
        }
        prev_inside = now_inside;
        prev_pt = Some(sample);
    }

    if current.len() >= 2 {
        out.push(Polyline3D { points: current });
    }
}

/// Linearly interpolate the 3D point at the parameter where the
/// segment `(a, b)` crosses the closest UV bbox edge.
///
/// Returns `None` when no edge crossing exists in the unit interval
/// (numerically the segment is essentially inside or outside on both
/// ends — caller's `prev_inside` / `now_inside` flags decided
/// otherwise so this should not happen, but degrade safely if it does).
fn clip_segment_to_bbox(
    a: &ParametricIntersectionPoint,
    b: &ParametricIntersectionPoint,
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
) -> Option<Point3> {
    let mut t_clip = 1.0;
    let mut hit = false;
    let try_edge = |from: f64, to: f64, lo: f64, hi: f64, t_clip: &mut f64, hit: &mut bool| {
        let d = to - from;
        if d.abs() < 1e-18 {
            return;
        }
        for boundary in [lo, hi] {
            let t = (boundary - from) / d;
            if t > 0.0 && t < 1.0 && t < *t_clip {
                *t_clip = t;
                *hit = true;
            }
        }
    };
    try_edge(a.u, b.u, u_min, u_max, &mut t_clip, &mut hit);
    try_edge(a.v, b.v, v_min, v_max, &mut t_clip, &mut hit);
    if !hit {
        return None;
    }
    let p = a.position + (b.position - a.position) * t_clip;
    Some(p)
}

// ---------------------------------------------------------------------------
// Internal: fragment chaining
// ---------------------------------------------------------------------------

fn chain_fragments_into_loops(fragments: &[Polyline3D], tolerance: &Tolerance) -> Vec<Vec<Point3>> {
    let weld_eps = tolerance.distance().max(1e-9);
    let mut frags: Vec<Polyline3D> = fragments
        .iter()
        .filter(|f| f.points.len() >= 2)
        .cloned()
        .collect();
    if frags.is_empty() {
        return Vec::new();
    }

    // Dedup fragments by endpoint pair. The marching-square seed grid
    // emits the same intersection curve many times per face (once per
    // crossed cell), so without this filter the chaining loop greedily
    // picks a duplicate at every tail and produces out-and-back A→B→A
    // degenerate "loops" instead of walking to the real next face.
    let dedup_eps = (tolerance.distance() * 100.0).max(1e-4);
    frags = dedup_fragments_by_endpoints(frags, dedup_eps);

    let mut used = vec![false; frags.len()];
    let mut loops: Vec<Vec<Point3>> = Vec::new();

    for start in 0..frags.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut chain: Vec<Point3> = frags[start].points.clone();

        // Self-closed fragment (math layer emitted a complete circle as
        // one polyline): emit immediately rather than trying to grow it
        // and accidentally concatenating a second revolution. The
        // marching-square chord noise is much larger than `weld_eps`,
        // so check closure relative to the fragment's own perimeter:
        // a true open chain has gap proportional to the missing arc
        // (≥ a few % of perimeter), while a noise-only gap on a closed
        // circle is ~ chord-step / sample-count.
        if chain.len() >= 6 {
            let first = chain[0];
            let last = *chain.last().unwrap_or(&first);
            let gap = (first - last).magnitude();
            let perim = chain
                .windows(2)
                .map(|w| (w[1] - w[0]).magnitude())
                .sum::<f64>();
            let rel = if perim > 0.0 {
                gap / perim
            } else {
                f64::INFINITY
            };
            if gap <= weld_eps || rel < 0.01 {
                chain.pop();
                if chain.len() >= 3 {
                    loops.push(chain);
                }
                continue;
            }
        }

        // Grow the chain by appending fragments that meet at its tail.
        loop {
            let tail = match chain.last() {
                Some(p) => *p,
                None => break,
            };
            let head = chain[0];
            if points_close(tail, head, weld_eps) && chain.len() >= 3 {
                // Closed loop — collapse the duplicate endpoint.
                chain.pop();
                break;
            }
            let mut found: Option<(usize, bool)> = None;
            for (i, f) in frags.iter().enumerate() {
                if used[i] {
                    continue;
                }
                if points_close(*f.points.first().unwrap_or(&tail), tail, weld_eps) {
                    found = Some((i, false));
                    break;
                }
                if points_close(*f.points.last().unwrap_or(&tail), tail, weld_eps) {
                    found = Some((i, true));
                    break;
                }
            }
            match found {
                Some((i, reverse)) => {
                    used[i] = true;
                    let mut pts = frags[i].points.clone();
                    if reverse {
                        pts.reverse();
                    }
                    // Skip the duplicate joint point.
                    chain.extend(pts.into_iter().skip(1));
                }
                None => {
                    // Open chain — diagnostic, not fatal.
                    tracing::warn!(
                        "section: fragment chain did not close (head=({:.3},{:.3},{:.3}) tail=({:.3},{:.3},{:.3}), {} points)",
                        head.x, head.y, head.z, tail.x, tail.y, tail.z, chain.len()
                    );
                    chain.clear();
                    break;
                }
            }
        }

        if chain.len() >= 3 {
            loops.push(chain);
        }
    }

    loops
}

/// Iterative Ramer–Douglas–Peucker simplification on a closed 3D loop.
///
/// The chord deviation threshold `eps` measures the maximum perpendicular
/// distance from a retained point to the segment connecting its kept
/// neighbours; everything closer than that is collapsed. Straight runs
/// collapse to two endpoints; arcs keep enough samples to approximate
/// themselves within `eps`.
fn simplify_loop_rdp(loop3d: &[Point3], eps: f64) -> Vec<Point3> {
    let n = loop3d.len();
    if n < 4 {
        return loop3d.to_vec();
    }
    // Split the closed loop at two diametrically opposed points so we
    // can run RDP on two open polylines (RDP needs explicit endpoints).
    let mid = n / 2;
    let mut a = rdp_open(&loop3d[..=mid], eps);
    let mut b = rdp_open(&loop3d[mid..], eps);
    // Drop the duplicate joint at b[0] = a.last().
    if !b.is_empty() && !a.is_empty() {
        b.remove(0);
    }
    a.append(&mut b);
    // a.last() == loop3d.last(); we want it removed only if it
    // duplicates loop3d[0] — fragment chaining already stripped that
    // duplicate, so leave the result as a closed cycle without the
    // explicit repeat.
    if a.len() >= 2 {
        let first = a[0];
        let last = *a.last().unwrap_or(&first);
        if points_close(first, last, eps * 0.5) {
            a.pop();
        }
    }
    a
}

fn rdp_open(pts: &[Point3], eps: f64) -> Vec<Point3> {
    if pts.len() <= 2 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    rdp_recurse(pts, 0, pts.len() - 1, eps * eps, &mut keep);
    pts.iter()
        .zip(keep.iter())
        .filter_map(|(p, &k)| if k { Some(*p) } else { None })
        .collect()
}

fn rdp_recurse(pts: &[Point3], lo: usize, hi: usize, eps_sq: f64, keep: &mut [bool]) {
    if hi <= lo + 1 {
        return;
    }
    let a = pts[lo];
    let b = pts[hi];
    let ab = b - a;
    let ab_len_sq = ab.dot(&ab);
    let mut max_d_sq = 0.0;
    let mut max_i = lo;
    for i in (lo + 1)..hi {
        let ap = pts[i] - a;
        let d_sq = if ab_len_sq < 1e-30 {
            ap.dot(&ap)
        } else {
            let cross = ap.cross(&ab);
            cross.dot(&cross) / ab_len_sq
        };
        if d_sq > max_d_sq {
            max_d_sq = d_sq;
            max_i = i;
        }
    }
    if max_d_sq > eps_sq {
        keep[max_i] = true;
        rdp_recurse(pts, lo, max_i, eps_sq, keep);
        rdp_recurse(pts, max_i, hi, eps_sq, keep);
    }
}

fn points_close(a: Point3, b: Point3, eps: f64) -> bool {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    let dz = a.z - b.z;
    (dx * dx + dy * dy + dz * dz).sqrt() <= eps
}

/// Keep one representative per equivalence class of fragments that
/// share the same unordered endpoint pair (within `eps`). The marching
/// algorithm in `math::surface_plane_intersection` emits one curve per
/// grid cell that crosses the zero-set, so a face cut by an oblique
/// plane can produce 20+ copies of the same diagonal polyline.
///
/// Without this dedup, the chain step would greedily pair each
/// duplicate with the next one at its tail and emit degenerate
/// out-and-back A→B→A "loops" with zero signed area.
fn dedup_fragments_by_endpoints(frags: Vec<Polyline3D>, eps: f64) -> Vec<Polyline3D> {
    let mut kept: Vec<Polyline3D> = Vec::new();
    for f in frags {
        let first = match f.points.first() {
            Some(p) => *p,
            None => continue,
        };
        let last = match f.points.last() {
            Some(p) => *p,
            None => continue,
        };
        let dup = kept.iter().any(|k| {
            let kf = match k.points.first() {
                Some(p) => *p,
                None => return false,
            };
            let kl = match k.points.last() {
                Some(p) => *p,
                None => return false,
            };
            (points_close(first, kf, eps) && points_close(last, kl, eps))
                || (points_close(first, kl, eps) && points_close(last, kf, eps))
        });
        if !dup {
            kept.push(f);
        }
    }
    kept
}

// ---------------------------------------------------------------------------
// Internal: 2D projection + nesting classification
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Loop2D {
    pts: Vec<(f64, f64)>,
    signed_area: f64,
}

fn project_loop_to_2d(
    loop3d: &[Point3],
    plane_origin: Point3,
    u_axis: Vector3,
    v_axis: Vector3,
) -> Loop2D {
    let pts: Vec<(f64, f64)> = loop3d
        .iter()
        .map(|p| {
            let r = *p - plane_origin;
            (r.dot(&u_axis), r.dot(&v_axis))
        })
        .collect();
    let signed_area = polygon_signed_area_2d(&pts);
    Loop2D { pts, signed_area }
}

/// Keep one representative per equivalence class of geometrically
/// identical loops. Two 2D loops match when their signed areas agree
/// to within 1% and their bbox extents (min/max along each axis) agree
/// to within 2% of the larger bbox width — both relative tolerances
/// because the marching-square seed grid emits the same closed curve
/// many times with slightly different start points, so RDP keeps
/// different samples in each copy and absolute-eps centre comparison
/// is hopeless.
/// Returns indices into the input array, in order.
fn dedup_loops_by_signature(loops: &[Loop2D], _eps: f64) -> Vec<usize> {
    let mut kept: Vec<usize> = Vec::new();
    for (i, l) in loops.iter().enumerate() {
        let sig_i = loop_signature(l);
        let dup = kept.iter().any(|&j| {
            let sig_j = loop_signature(&loops[j]);
            signatures_match(&sig_i, &sig_j)
        });
        if !dup {
            kept.push(i);
        }
    }
    kept
}

#[derive(Debug, Clone, Copy)]
struct LoopSignature {
    x_min: f64,
    x_max: f64,
    y_min: f64,
    y_max: f64,
    area: f64,
}

fn loop_signature(l: &Loop2D) -> LoopSignature {
    if l.pts.is_empty() {
        return LoopSignature {
            x_min: 0.0,
            x_max: 0.0,
            y_min: 0.0,
            y_max: 0.0,
            area: 0.0,
        };
    }
    let mut x_min = f64::INFINITY;
    let mut x_max = f64::NEG_INFINITY;
    let mut y_min = f64::INFINITY;
    let mut y_max = f64::NEG_INFINITY;
    for &(x, y) in &l.pts {
        if x < x_min {
            x_min = x;
        }
        if x > x_max {
            x_max = x;
        }
        if y < y_min {
            y_min = y;
        }
        if y > y_max {
            y_max = y;
        }
    }
    LoopSignature {
        x_min,
        x_max,
        y_min,
        y_max,
        area: l.signed_area,
    }
}

fn signatures_match(a: &LoopSignature, b: &LoopSignature) -> bool {
    let dx_a = (a.x_max - a.x_min).abs();
    let dy_a = (a.y_max - a.y_min).abs();
    let dx_b = (b.x_max - b.x_min).abs();
    let dy_b = (b.y_max - b.y_min).abs();
    let scale = dx_a.max(dy_a).max(dx_b).max(dy_b).max(1.0);
    let bbox_tol = scale * 0.02;
    let area_tol = a.area.abs().max(b.area.abs()).max(1.0) * 0.01;
    (a.x_min - b.x_min).abs() < bbox_tol
        && (a.x_max - b.x_max).abs() < bbox_tol
        && (a.y_min - b.y_min).abs() < bbox_tol
        && (a.y_max - b.y_max).abs() < bbox_tol
        && (a.area - b.area).abs() < area_tol
}

fn polygon_signed_area_2d(pts: &[(f64, f64)]) -> f64 {
    let n = pts.len();
    if n < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    for i in 0..n {
        let (x1, y1) = pts[i];
        let (x2, y2) = pts[(i + 1) % n];
        a += x1 * y2 - x2 * y1;
    }
    a * 0.5
}

struct LoopNesting {
    /// Each entry: `(outer_loop_index, indices of direct holes)`.
    outers: Vec<(usize, Vec<usize>)>,
}

/// Classify every 2D loop by its containment depth in the others.
/// Depth 0 (or any even depth) is an outer cap boundary; depth 1
/// (or any odd depth) is a hole inside its enclosing outer. Depth ≥ 2
/// outer loops (an island inside a hole inside a cap) become standalone
/// caps with their own holes — handled by re-rooting at every even
/// depth.
fn classify_loop_nesting(loops: &[Loop2D]) -> LoopNesting {
    let n = loops.len();
    if n == 0 {
        return LoopNesting { outers: Vec::new() };
    }

    // Containment depth = number of OTHER loops that contain this loop's
    // first vertex. We use the first vertex because all our loops are
    // simple closed polygons emitted from a manifold section and won't
    // straddle each other.
    let mut depth = vec![0usize; n];
    for i in 0..n {
        let test_pt = match loops[i].pts.first() {
            Some(p) => *p,
            None => continue,
        };
        for (j, other) in loops.iter().enumerate() {
            if i == j {
                continue;
            }
            if point_in_polygon(test_pt, &other.pts) {
                depth[i] += 1;
            }
        }
    }

    // For each loop, find its parent = the immediate enclosing loop
    // (i.e., the loop with `depth[parent] == depth[i] - 1` that
    // contains loop `i`'s first vertex). Holes attach to their parent
    // outer; outers (even depth) become roots.
    let mut outers: Vec<(usize, Vec<usize>)> = Vec::new();
    let mut hole_owner: std::collections::HashMap<usize, usize> = Default::default();
    for i in 0..n {
        if depth[i] % 2 == 0 {
            outers.push((i, Vec::new()));
        }
    }
    let outer_lookup: std::collections::HashMap<usize, usize> = outers
        .iter()
        .enumerate()
        .map(|(slot, (idx, _))| (*idx, slot))
        .collect();
    for i in 0..n {
        if depth[i] % 2 == 1 {
            // Find parent: an outer (even depth) with depth = depth[i] - 1
            // that contains loop i's first vertex.
            let target_depth = depth[i].saturating_sub(1);
            let test_pt = match loops[i].pts.first() {
                Some(p) => *p,
                None => continue,
            };
            for (j, other) in loops.iter().enumerate() {
                if depth[j] != target_depth {
                    continue;
                }
                if point_in_polygon(test_pt, &other.pts) {
                    hole_owner.insert(i, j);
                    break;
                }
            }
        }
    }

    for (hole, owner) in hole_owner {
        if let Some(&slot) = outer_lookup.get(&owner) {
            outers[slot].1.push(hole);
        }
    }

    LoopNesting { outers }
}

/// Even-odd point-in-polygon test in 2D.
fn point_in_polygon(p: (f64, f64), poly: &[(f64, f64)]) -> bool {
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let (x, y) = p;
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        let intersects = ((yi > y) != (yj > y))
            && (x < (xj - xi) * (y - yi) / (yj - yi).max(1e-18).copysign(yj - yi) + xi);
        if intersects {
            inside = !inside;
        }
        j = i;
    }
    inside
}

// ---------------------------------------------------------------------------
// Internal: triangulate a (outer + holes) group into a SectionCap
// ---------------------------------------------------------------------------

fn triangulate_cap(
    solid_id: SolidId,
    plane_origin: Point3,
    plane_normal: Vector3,
    outer_idx: usize,
    hole_idxs: &[usize],
    loops_3d: &[Vec<Point3>],
    loops_2d: &[Loop2D],
) -> Option<SectionCap> {
    let outer_3d = loops_3d.get(outer_idx)?;
    let outer_2d = loops_2d.get(outer_idx)?;
    if outer_3d.len() < 3 {
        return None;
    }

    // Make sure the outer loop is CCW in the tangent basis (positive
    // signed area). If not, reverse both the 3D and 2D copies. Holes
    // must run opposite (CW).
    let mut combined_3d: Vec<Point3> = Vec::new();
    let mut loop_boundaries: Vec<(usize, usize, bool)> = Vec::new();

    let outer_ccw = outer_2d.signed_area > 0.0;
    let mut outer_pts_3d: Vec<Point3> = outer_3d.clone();
    if !outer_ccw {
        outer_pts_3d.reverse();
    }
    let start = combined_3d.len();
    combined_3d.extend_from_slice(&outer_pts_3d);
    let end = combined_3d.len();
    loop_boundaries.push((start, end, true));

    for &hole_idx in hole_idxs {
        let hole_3d = match loops_3d.get(hole_idx) {
            Some(h) => h,
            None => continue,
        };
        let hole_2d = match loops_2d.get(hole_idx) {
            Some(h) => h,
            None => continue,
        };
        if hole_3d.len() < 3 {
            continue;
        }
        let mut hole_pts_3d = hole_3d.clone();
        // Hole opposite winding from outer: hole CW when outer CCW.
        let hole_ccw = hole_2d.signed_area > 0.0;
        if hole_ccw == outer_ccw {
            hole_pts_3d.reverse();
        }
        let s = combined_3d.len();
        combined_3d.extend_from_slice(&hole_pts_3d);
        let e = combined_3d.len();
        loop_boundaries.push((s, e, false));
    }

    let tris = triangulate_planar_polygon(&combined_3d, &loop_boundaries, &plane_normal);
    if tris.is_empty() {
        return None;
    }

    let indices: Vec<[u32; 3]> = tris
        .into_iter()
        .map(|[a, b, c]| [a as u32, b as u32, c as u32])
        .collect();
    let normals = vec![plane_normal; combined_3d.len()];

    Some(SectionCap {
        solid_id,
        plane_origin,
        plane_normal,
        vertices: combined_3d,
        indices,
        normals,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

    fn build_box_model(size: f64) -> (BRepModel, SolidId) {
        let mut model = BRepModel::new();
        let geom = {
            let mut builder = TopologyBuilder::new(&mut model);
            builder
                .create_box_3d(size, size, size)
                .expect("create_box_3d")
        };
        let solid_id = match geom {
            GeometryId::Solid(id) => id,
            other => panic!("expected Solid, got {:?}", other),
        };
        (model, solid_id)
    }

    fn build_cylinder_model(radius: f64, height: f64) -> (BRepModel, SolidId) {
        let mut model = BRepModel::new();
        let geom = {
            let mut builder = TopologyBuilder::new(&mut model);
            builder
                .create_cylinder_3d(Vector3::ZERO, Vector3::new(0.0, 0.0, 1.0), radius, height)
                .expect("create_cylinder_3d")
        };
        let solid_id = match geom {
            GeometryId::Solid(id) => id,
            other => panic!("expected Solid, got {:?}", other),
        };
        (model, solid_id)
    }

    #[test]
    fn section_misses_box_returns_empty() {
        // Box is centred at origin spanning [-5, 5] on every axis.
        let (model, solid_id) = build_box_model(10.0);
        let caps = section_solid_by_plane(
            &model,
            solid_id,
            Point3::new(0.0, 0.0, 100.0),
            Vector3::new(0.0, 0.0, 1.0),
            Tolerance::default(),
        )
        .expect("section call");
        assert!(caps.is_empty(), "expected no caps, got {}", caps.len());
    }

    #[test]
    fn section_box_through_middle_produces_one_cap() {
        let (model, solid_id) = build_box_model(10.0);
        let caps = section_solid_by_plane(
            &model,
            solid_id,
            Vector3::ZERO,
            Vector3::new(0.0, 0.0, 1.0),
            Tolerance::default(),
        )
        .expect("section call");
        assert_eq!(caps.len(), 1, "expected exactly one cap");
        let cap = &caps[0];
        assert!(!cap.indices.is_empty(), "expected at least one triangle");
        assert_eq!(cap.normals.len(), cap.vertices.len());
        for v in &cap.vertices {
            assert!(v.z.abs() < 1e-6, "cap vertex off plane: z = {}", v.z);
        }
    }

    #[test]
    fn section_oblique_plane_through_box() {
        let (model, solid_id) = build_box_model(10.0);
        let n = Vector3::new(1.0, 1.0, 0.0);
        let caps = section_solid_by_plane(&model, solid_id, Vector3::ZERO, n, Tolerance::default())
            .expect("section call");
        assert_eq!(caps.len(), 1, "expected one cap for oblique cut");
        let cap = &caps[0];
        assert!(
            cap.vertices.len() >= 4,
            "expected at least 4 vertices on the oblique cap, got {}",
            cap.vertices.len()
        );
    }

    #[test]
    fn section_zero_normal_rejected() {
        let (model, solid_id) = build_box_model(10.0);
        let err = section_solid_by_plane(
            &model,
            solid_id,
            Vector3::ZERO,
            Vector3::new(0.0, 0.0, 0.0),
            Tolerance::default(),
        )
        .expect_err("zero normal should error");
        assert!(
            matches!(err, OperationError::InvalidInput { ref parameter, .. } if parameter == "plane_normal"),
            "expected InvalidInput on plane_normal, got {:?}",
            err
        );
    }

    #[test]
    fn section_cylinder_through_middle_produces_cap() {
        // Cylinder base at origin, axis +Z, radius 2, height 10 ⇒
        // spans z ∈ [0, 10]. Plane at z = 5 cuts through the middle.
        let (model, solid_id) = build_cylinder_model(2.0, 10.0);
        let caps = section_solid_by_plane(
            &model,
            solid_id,
            Point3::new(0.0, 0.0, 5.0),
            Vector3::new(0.0, 0.0, 1.0),
            Tolerance::default(),
        )
        .expect("section call");
        assert_eq!(caps.len(), 1, "expected one cap for cylinder section");
        let cap = &caps[0];
        assert!(
            cap.vertices.len() >= 8,
            "expected a discretised circle (≥ 8 vertices), got {}",
            cap.vertices.len()
        );
        for v in &cap.vertices {
            assert!(
                (v.z - 5.0).abs() < 1e-4,
                "cap vertex off plane: z = {}",
                v.z
            );
            let r = (v.x * v.x + v.y * v.y).sqrt();
            assert!(r <= 2.0 + 1e-3, "cap vertex outside cylinder: r = {}", r);
        }
    }
}
