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
use crate::primitives::face::{Face, FaceId};
use crate::primitives::shell::ShellId;
use crate::primitives::solid::SolidId;
use crate::primitives::surface::{Plane, SurfaceType};
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
            let before = fragments.len();
            collect_face_fragments(model, *face_id, plane_origin, normal, &mut fragments);
            if std::env::var("ROSHERA_SEC_TRACE").is_ok() {
                let kind = model
                    .faces
                    .get(*face_id)
                    .and_then(|f| model.surfaces.get(f.surface_id))
                    .map(|s| s.surface_type());
                eprintln!(
                    "[sec] face {} {:?}: +{} frags",
                    face_id,
                    kind,
                    fragments.len() - before
                );
            }
        }
    }

    if fragments.is_empty() {
        return Ok(Vec::new());
    }

    // Chain fragments into closed 3D loops.
    let raw_loops = chain_fragments_into_loops(&fragments, &tolerance);
    let sec_trace = std::env::var("ROSHERA_SEC_TRACE").is_ok();
    let n_raw = raw_loops.len();
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
    if sec_trace {
        eprintln!(
            "[sec] frags={} raw_loops={} simplified(>=3)={}",
            fragments.len(),
            n_raw,
            loops.len()
        );
    }
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

    if sec_trace {
        eprintln!(
            "[sec] deduped_loops={} outers={} caps={}",
            projected.len(),
            nesting.outers.len(),
            caps.len()
        );
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

/// Intersect the infinite 2D line `o + t·d` with segment `a→b`; return the line
/// parameter `t` at the crossing if it lies within the segment.
fn line_seg_intersect_2d(
    o: (f64, f64),
    d: (f64, f64),
    a: (f64, f64),
    b: (f64, f64),
) -> Option<f64> {
    let ex = b.0 - a.0;
    let ey = b.1 - a.1;
    // Solve o + t·d = a + s·e for t,s.  det of [d, -e].
    let det = ex * d.1 - ey * d.0;
    if det.abs() < 1e-12 {
        return None; // line ∥ edge
    }
    let rx = a.0 - o.0;
    let ry = a.1 - o.1;
    let t = (ex * ry - ey * rx) / det;
    let s = (d.0 * ry - d.1 * rx) / det;
    if s >= -1e-9 && s <= 1.0 + 1e-9 {
        Some(t)
    } else {
        None
    }
}

/// EYE-2 / SECTION #83: EXACT planar-face cross-section. Two planes meet in a
/// line `p₀ + t·u` with `u = n_cut × n_face` and `p₀` the closed-form solution
/// of the two-plane system; clip that line to the face by collecting even-odd
/// crossings against every loop edge (outer boundary + holes), then emit the
/// inside intervals as fragments. This replaces the marching-square grid on
/// flat faces — which fragmented a single straight cut line into disjoint
/// pieces on wide/short faces (box sides), so the fragments never chained into
/// a cap. Reference: Liang–Barsky / Cyrus–Beck line clipping + even-odd
/// point-in-polygon (Foley & van Dam). Curved faces keep the marching path.
fn plane_face_fragments(
    model: &BRepModel,
    face: &Face,
    plane: &Plane,
    cut_origin: Point3,
    cut_normal: Vector3,
    out: &mut Vec<Polyline3D>,
) {
    let n_c = cut_normal;
    let n_f = plane.normal;
    let u = n_c.cross(&n_f);
    let uu = u.dot(&u);
    if uu < 1e-18 {
        return; // planes parallel (or coincident) → no proper section line
    }
    let inv_len = 1.0 / uu.sqrt();
    let line_dir = u * inv_len;
    // Closed-form point on both planes: p₀ = (d_c (n_f×u) + d_f (u×n_c)) / (u·u).
    let d_c = n_c.dot(&cut_origin);
    let d_f = n_f.dot(&plane.origin);
    let p0v = (n_f.cross(&u) * d_c + u.cross(&n_c) * d_f) * (1.0 / uu);
    let p0 = Point3::new(p0v.x, p0v.y, p0v.z);

    // Project to the face plane's orthonormal (u_dir, v_dir) basis.
    let to2 = |p: &Point3| -> (f64, f64) {
        let w = Vector3::new(
            p.x - plane.origin.x,
            p.y - plane.origin.y,
            p.z - plane.origin.z,
        );
        (w.dot(&plane.u_dir), w.dot(&plane.v_dir))
    };
    let o2 = to2(&p0);
    let d2 = (line_dir.dot(&plane.u_dir), line_dir.dot(&plane.v_dir));

    // Crossings of the section line with every loop edge (outer + holes).
    //
    // STRAIGHT edges use the exact 2-D segment/line clip (#83). CURVED edges
    // (circular bore rims, arcs) are crossed against the cut PLANE on the TRUE
    // curve — sign-change bracket + bisection on the edge parameter — not against
    // a chord sampling of the rim. #section-404: a 64-chord polyline of a bore
    // rim placed the crossing up to the polygon sagitta (~1.7e-5 mm for a 64-gon
    // of r2.5) off the real circle, so an OFF-AXIS cut's planar-face rim crossing
    // no longer welded to the analytic cylinder-generator fragment at the shared
    // 3-D corner (gap ≫ `weld_eps`=1e-6); the section loop never closed and the
    // whole cap set came back empty → `render_section` None → HTTP 404. The
    // on-axis cut escaped only because the crossing lands at the circle's
    // x-extreme, where the chord sagitta is zero. Evaluating the crossing on the
    // exact curve restores the weld. Ref: exact curve∩plane intersection.
    let tol = Tolerance::default();
    let mut loops = vec![face.outer_loop];
    loops.extend_from_slice(&face.inner_loops);
    let mut ts: Vec<f64> = Vec::new();
    for lid in loops {
        let lp = match model.loops.get(lid) {
            Some(l) => l,
            None => continue,
        };
        for &eid in &lp.edges {
            let e = match model.edges.get(eid) {
                Some(e) => e,
                None => continue,
            };
            let linear = model
                .curves
                .get(e.curve_id)
                .map(|c| c.is_linear(tol))
                .unwrap_or(true);
            if linear {
                // Exact straight-segment clip in the face-plane 2-D basis. A
                // segment is orientation-independent, so the raw endpoint
                // vertices suffice.
                let a = match model.vertices.get(e.start_vertex) {
                    Some(v) => Point3::new(v.position[0], v.position[1], v.position[2]),
                    None => continue,
                };
                let b = match model.vertices.get(e.end_vertex) {
                    Some(v) => Point3::new(v.position[0], v.position[1], v.position[2]),
                    None => continue,
                };
                if let Some(t) = line_seg_intersect_2d(o2, d2, to2(&a), to2(&b)) {
                    ts.push(t);
                }
            } else {
                edge_plane_crossings(
                    e, model, &to2, o2, d2, cut_origin, cut_normal, p0, line_dir, &mut ts,
                );
            }
        }
    }
    if ts.len() < 2 {
        return;
    }
    ts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // Dedup near-coincident crossings (a crossing through a shared vertex is
    // reported by both incident edges).
    let mut xs: Vec<f64> = Vec::new();
    for t in ts {
        if xs.last().map_or(true, |&l| (t - l).abs() > 1e-6) {
            xs.push(t);
        }
    }
    // Even-odd: material lies between crossing pairs [x0,x1], [x2,x3], …
    let mut i = 0;
    while i + 1 < xs.len() {
        let (t0, t1) = (xs[i], xs[i + 1]);
        if t1 - t0 > 1e-6 {
            let pa = Point3::new(
                p0.x + line_dir.x * t0,
                p0.y + line_dir.y * t0,
                p0.z + line_dir.z * t0,
            );
            let pb = Point3::new(
                p0.x + line_dir.x * t1,
                p0.y + line_dir.y * t1,
                p0.z + line_dir.z * t1,
            );
            out.push(Polyline3D {
                points: vec![pa, pb],
            });
        }
        i += 2;
    }
}

/// Push the section-line parameters at which a CURVED loop edge crosses the cut
/// plane, evaluated on the TRUE underlying curve (never on the chord sampling).
///
/// A crossing is *located* the robust way — a chord between consecutive curve
/// samples is tested against the section line with the SAME inclusive-endpoint
/// clip the straight-edge path uses ([`line_seg_intersect_2d`]) — then the exact
/// crossing is *refined* onto the real curve by bisecting the signed distance to
/// the cut plane `g(s) = cut_normal · (curve(s) − cut_origin)` over the bracketing
/// parameter interval. Locating on the chord (not on `g`'s strict sign) is what
/// survives the degenerate on-axis cut, where the crossing lands exactly on a rim
/// seam vertex and `g` there is `±ε` of arbitrary sign; refining onto the curve is
/// what lets an OFF-axis rim crossing weld to the analytic cylinder-generator
/// fragment at the shared 3-D corner (#section-404). `t` is the crossing's signed
/// coordinate along `line_dir` from `p0` — the parameterisation the caller emits
/// fragments in. Crossings coincident at a shared sample vertex are reported by
/// both incident chords and collapsed by the caller's `xs` dedup.
#[allow(clippy::too_many_arguments)]
fn edge_plane_crossings(
    edge: &crate::primitives::edge::Edge,
    model: &BRepModel,
    to2: &dyn Fn(&Point3) -> (f64, f64),
    o2: (f64, f64),
    d2: (f64, f64),
    cut_origin: Point3,
    cut_normal: Vector3,
    p0: Point3,
    line_dir: Vector3,
    ts: &mut Vec<f64>,
) {
    // Enough chords to isolate both crossings of a section line with a closed rim
    // circle, with margin for arcs of higher local curvature.
    const SAMPLES: usize = 128;
    let eval = |s: f64| -> Option<Point3> { edge.evaluate(s, &model.curves).ok() };
    let signed = |p: &Point3| -> f64 {
        cut_normal.dot(&Vector3::new(
            p.x - cut_origin.x,
            p.y - cut_origin.y,
            p.z - cut_origin.z,
        ))
    };
    let param_t = |p: &Point3| -> f64 {
        (p.x - p0.x) * line_dir.x + (p.y - p0.y) * line_dir.y + (p.z - p0.z) * line_dir.z
    };

    let mut prev_s = 0.0;
    let mut prev_p = match eval(0.0) {
        Some(p) => p,
        None => return,
    };
    for k in 1..=SAMPLES {
        let s = k as f64 / SAMPLES as f64;
        let p = match eval(s) {
            Some(p) => p,
            None => {
                // Missing sample breaks the current chord; resume from the next.
                prev_s = s;
                continue;
            }
        };
        // Robust detection: does the chord (prev_p → p) cross the section line?
        if line_seg_intersect_2d(o2, d2, to2(&prev_p), to2(&p)).is_some() {
            // Refine onto the exact curve by bisecting g over [prev_s, s].
            let (mut lo, mut hi) = (prev_s, s);
            let glo = signed(&prev_p);
            let ghi = signed(&p);
            let crossing = if glo == 0.0 {
                Some(prev_p)
            } else if ghi == 0.0 {
                Some(p)
            } else if (glo < 0.0) == (ghi < 0.0) {
                // Same side of the plane: the chord was flagged only via the
                // inclusive-endpoint tolerance, so the true crossing sits at the
                // endpoint nearest the plane.
                Some(if glo.abs() <= ghi.abs() { prev_p } else { p })
            } else {
                let side_lo = glo < 0.0;
                let mut mid_p = prev_p;
                for _ in 0..50 {
                    let mid = 0.5 * (lo + hi);
                    match eval(mid) {
                        Some(pm) => {
                            mid_p = pm;
                            if (signed(&pm) < 0.0) == side_lo {
                                lo = mid;
                            } else {
                                hi = mid;
                            }
                        }
                        None => break,
                    }
                }
                Some(mid_p)
            };
            if let Some(cp) = crossing {
                ts.push(param_t(&cp));
            }
        }
        prev_s = s;
        prev_p = p;
    }
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

    // SECTION #83: planar faces use the exact Plane×Plane line-clip (robust
    // where the marching grid fragments a straight cut line). Curved faces fall
    // through to the marching-square path below.
    if let Some(plane) = surface.as_any().downcast_ref::<Plane>() {
        plane_face_fragments(model, face, plane, plane_origin, plane_normal, out);
        return;
    }

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
    // Clamp the padded search rectangle to the surface's OWN parameter
    // domain. Padding a PERIODIC direction beyond its period — v ∈ [0, 2π]
    // on a full SurfaceOfRevolution or Cylinder — re-enters the surface past
    // its seam, so the marching grid detects the seam iso-curve TWICE (once
    // near v≈0 and once near v≈2π). For an axis-containing (meridian) cut the
    // seam iso-curve IS one of the two section meridians, so it came back as a
    // duplicate that then collided with the winding trim and shattered the
    // clean curve into partial pieces that never chained to a cap
    // (#section-axial: a Rao-bell nozzle meridian cut returned 0 caps → 404).
    // Clamping to the surface domain gives a SINGLE seam detection and matches
    // the exact bounds under which the contour reaches the face's true corner
    // vertices (where it must weld to the neighbouring rim fragments).
    let ((su0, su1), (sv0, sv1)) = surface.parameter_bounds();
    let config = SurfacePlaneIntersectionConfig {
        param_bounds_override: Some((
            ((u_min - pad_u).max(su0), (u_max + pad_u).min(su1)),
            ((v_min - pad_v).max(sv0), (v_max + pad_v).min(sv1)),
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
    // Boundary tolerance. When the cutting plane passes through a periodic
    // surface's SEAM (e.g. a cylinder whose u=0 generator lies in the plane),
    // `intersect_surface_plane` reports that generator at u ≈ 0 — but rounding
    // can place it a hair BELOW u_min (observed −9e-13), so a strict `>= u_min`
    // silently drops the seam generator. That left an axial cylinder section a
    // generator short → the cross-section rectangle never closed → zero caps,
    // but ONLY for cut planes containing the seam (e.g. +Y for an +X-seam
    // cylinder), making section direction-dependent. The intersection search
    // rectangle is already padded (`pad_u`/`pad_v`); pad the trim test to match
    // so a boundary-coincident (seam) generator is kept. A relative+absolute
    // epsilon keeps genuinely-outside samples out.
    let eps_u = ((u_max - u_min).abs() * 1e-6).max(1e-9);
    let eps_v = ((v_max - v_min).abs() * 1e-6).max(1e-9);
    let inside = |p: &ParametricIntersectionPoint| -> bool {
        p.u >= u_min - eps_u && p.u <= u_max + eps_u && p.v >= v_min - eps_v && p.v <= v_max + eps_v
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
    // Boundary tolerance for the parameter-domain rectangle. Relative to the
    // domain span with an absolute floor, so a Newton nudge of a corner sample
    // (a few ×tol in parameter space) still registers as "on the boundary".
    let bnd_u = ((u_max - u_min).abs() * 1e-4).max(1e-7);
    let bnd_v = ((v_max - v_min).abs() * 1e-4).max(1e-7);
    let in_bbox = |u: f64, v: f64| -> bool {
        u >= u_min - bnd_u && u <= u_max + bnd_u && v >= v_min - bnd_v && v <= v_max + bnd_v
    };
    // A sample sitting on the parameter-domain boundary rectangle belongs to a
    // BOUNDARY EDGE of the face — a seam meridian runs the full length of the
    // v-boundary, and every meridian's rim endpoints land on the u-boundary.
    // The winding-number test is ambiguous exactly on the boundary (|wn| ≈ 0.5,
    // which the strict `> 0.5` gate rejects), so it silently clips a meridian
    // cross-section short of the rim it must weld to AND shreds a seam-coincident
    // meridian into disjoint pieces. Treat boundary-coincident samples as inside;
    // the winding test still excludes genuine interior holes (inner loops) and
    // any non-rectangular outer-trim interior. Over-inclusion at a boundary is
    // caught downstream (an unwelded spurious fragment fails to chain and is
    // dropped), whereas under-inclusion here breaks the cap entirely.
    let on_domain_boundary = |u: f64, v: f64| -> bool {
        (u - u_min).abs() <= bnd_u
            || (u_max - u).abs() <= bnd_u
            || (v - v_min).abs() <= bnd_v
            || (v_max - v).abs() <= bnd_v
    };
    let member = |u: f64, v: f64| -> bool {
        in_bbox(u, v) && (on_domain_boundary(u, v) || point_inside_face_uv(u, v, face, model))
    };
    let inside_face = |p: &ParametricIntersectionPoint| -> bool { member(p.u, p.v) };

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
                let mid_inside = member(u_mid, v_mid);
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

/// If `frag` is a straight segment (all points collinear within `eps`), return
/// its canonical line `(foot, dir)`: `dir` is a unit vector with its dominant
/// component positive, `foot` the line's closest point to the origin (as a
/// vector from origin). A point P's parameter along the line is then `P · dir`.
fn fragment_as_line(frag: &Polyline3D, eps: f64) -> Option<(Vector3, Vector3)> {
    let pts = &frag.points;
    if pts.len() < 2 {
        return None;
    }
    let a = Vector3::new(pts[0].x, pts[0].y, pts[0].z);
    // len >= 2 guaranteed above, so the last index is valid.
    let last = &pts[pts.len() - 1];
    let b = Vector3::new(last.x, last.y, last.z);
    let mut dir = (b - a).normalize().ok()?;
    let (ax, ay, az) = (dir.x.abs(), dir.y.abs(), dir.z.abs());
    let dom = if ax >= ay && ax >= az {
        dir.x
    } else if ay >= az {
        dir.y
    } else {
        dir.z
    };
    if dom < 0.0 {
        dir = dir * -1.0;
    }
    for p in pts {
        let pv = Vector3::new(p.x, p.y, p.z) - a;
        let perp = pv - dir * pv.dot(&dir);
        if perp.magnitude() > eps {
            return None;
        }
    }
    let foot = a - dir * a.dot(&dir);
    Some((foot, dir))
}

/// Merge straight fragments lying on the SAME infinite line into the union of
/// their parameter ranges, so the marching-square tracer's overlapping copies
/// of a single generator collapse to one exact-ended segment. Non-straight
/// fragments (curved arcs) pass through unchanged.
fn merge_collinear_fragments(frags: Vec<Polyline3D>, eps: f64) -> Vec<Polyline3D> {
    use std::collections::HashMap;
    let q = |x: f64| (x / eps).round() as i64;
    let mut groups: HashMap<(i64, i64, i64, i64, i64, i64), (Vector3, Vector3, Vec<(f64, f64)>)> =
        HashMap::new();
    let mut out: Vec<Polyline3D> = Vec::new();
    for f in frags {
        if let Some((foot, dir)) = fragment_as_line(&f, eps) {
            // fragment_as_line returned Some, so points.len() >= 2 — indices valid.
            let a = &f.points[0];
            let b = &f.points[f.points.len() - 1];
            let ta = a.x * dir.x + a.y * dir.y + a.z * dir.z;
            let tb = b.x * dir.x + b.y * dir.y + b.z * dir.z;
            let key = (
                q(foot.x),
                q(foot.y),
                q(foot.z),
                q(dir.x),
                q(dir.y),
                q(dir.z),
            );
            let e = groups.entry(key).or_insert_with(|| (foot, dir, Vec::new()));
            e.2.push((ta.min(tb), ta.max(tb)));
        } else {
            out.push(f);
        }
    }
    for (_, (foot, dir, mut spans)) in groups {
        spans.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut cur: Option<(f64, f64)> = None;
        for (lo, hi) in spans {
            match cur {
                Some((c0, c1)) if lo <= c1 + eps => cur = Some((c0, c1.max(hi))),
                Some((c0, c1)) => {
                    out.push(Polyline3D {
                        points: vec![
                            Point3::new(
                                foot.x + dir.x * c0,
                                foot.y + dir.y * c0,
                                foot.z + dir.z * c0,
                            ),
                            Point3::new(
                                foot.x + dir.x * c1,
                                foot.y + dir.y * c1,
                                foot.z + dir.z * c1,
                            ),
                        ],
                    });
                    cur = Some((lo, hi));
                }
                None => cur = Some((lo, hi)),
            }
        }
        if let Some((c0, c1)) = cur {
            out.push(Polyline3D {
                points: vec![
                    Point3::new(
                        foot.x + dir.x * c0,
                        foot.y + dir.y * c0,
                        foot.z + dir.z * c0,
                    ),
                    Point3::new(
                        foot.x + dir.x * c1,
                        foot.y + dir.y * c1,
                        foot.z + dir.z * c1,
                    ),
                ],
            });
        }
    }
    out
}

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
    // Collapse the marching-square tracer's redundant copies of each straight
    // generator (cylinder/cone laterals cut by an axis-containing plane) into
    // their union, so the curved fragments reach the EXACT planar-clip corners
    // and the chain closes. (dedup_fragments_by_endpoints only removes exact
    // endpoint-pair duplicates; the grid emits overlapping near-duplicates whose
    // endpoints differ by the grid step.)
    frags = merge_collinear_fragments(frags, (tolerance.distance() * 1e4).max(1e-2));

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
        // Denominator must be `yj - yi` guarded against zero — keep its MAGNITUDE
        // (floored at 1e-18) AND its sign. The earlier `(yj-yi).max(1e-18)` (no
        // `.abs()`) clobbered any NEGATIVE dy to 1e-18, so every downward edge got
        // a ±1e-18 denominator → the x-intersection blew up to ±huge → spurious /
        // missed crossings. PIP was thus wrong for any polygon with downward edges
        // (every circle), which mis-classified section loop nesting (#85b: a flange
        // cap's bolt/centre holes were read as separate solid discs → 20% over-area).
        let dy = (yj - yi).abs().max(1e-18).copysign(yj - yi);
        let intersects = ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / dy + xi);
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
    use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use crate::operations::revolve::{revolve_smooth_nozzle, revolve_smooth_solid, RevolveOptions};
    use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

    /// Rao-bell nozzle inner flow contour (r, z) from the live #section-404
    /// dogfood: revolve with wall_thickness=2 → two smooth SurfaceOfRevolution
    /// walls + 2 planar annular rims.
    const NOZZLE_INNER: &[(f64, f64)] = &[
        (16.0, 0.0),
        (16.0, 15.0),
        (12.0, 25.0),
        (10.0, 32.0),
        (11.5, 40.0),
        (14.0, 50.0),
        (17.0, 62.0),
        (19.0, 72.0),
        (20.0, 80.0),
    ];

    /// #section-axial: a Rao-bell nozzle (revolve + wall_thickness=2 → two
    /// smooth `SurfaceOfRevolution` walls + 2 planar annular rims) MUST section
    /// with a MERIDIAN (axis-containing) plane — SECTION A-A for every
    /// axisymmetric aerospace part. The y=0 plane (normal +Y, containing the +Z
    /// revolve axis) cuts the tube wall in TWO closed C-shaped profiles (the +X
    /// and −X wall cross-sections). Before the fix this returned ZERO caps —
    /// the marching contour was clean but the winding-number face trim clipped
    /// the wall meridians short of the rims (and the over-period search
    /// duplicated the seam meridian), so nothing chained → `render_section` None
    /// → HTTP 404.
    ///
    /// Analytic area: the outer contour is the inner offset by +wall_thickness
    /// in radius at every station (same z), so each side's cross-section is a
    /// constant-width-`t` band along the profile height ⇒ area = t·Δz per side.
    /// Here t=2, z ∈ [0, 80] ⇒ 160 per side, 320 total.
    #[test]
    fn section_nozzle_axial_meridian_yields_two_wall_profiles() {
        let mut model = BRepModel::new();
        let s = revolve_smooth_nozzle(&mut model, NOZZLE_INNER, 2.0, RevolveOptions::default())
            .expect("nozzle build");

        // GUARD: transverse cut (normal +Z, through the throat) still yields the
        // clean annulus it always did.
        let transverse = section_solid_by_plane(
            &model,
            s,
            Point3::new(0.0, 0.0, 40.0),
            Vector3::new(0.0, 0.0, 1.0),
            Tolerance::default(),
        )
        .expect("transverse section");
        assert_eq!(
            transverse.len(),
            1,
            "transverse cut must stay one annular cap, got {}",
            transverse.len()
        );

        // The meridian (axial) cut — the case that 404'd.
        let axial = section_solid_by_plane(
            &model,
            s,
            Point3::new(0.0, 0.0, 40.0),
            Vector3::new(0.0, 1.0, 0.0),
            Tolerance::default(),
        )
        .expect("axial section must not error");
        assert_eq!(
            axial.len(),
            2,
            "meridian cut of a tube wall must yield 2 wall profiles, got {}",
            axial.len()
        );
        let total: f64 = axial.iter().map(cap_area).sum();
        let expected = 2.0 * 2.0 * 80.0; // 2 sides × width 2 × height 80
        let rel = (total - expected).abs() / expected;
        assert!(
            rel < 0.03,
            "axial wall cross-section area {total:.3} vs analytic {expected:.3} (rel {rel})"
        );
        // Every cap vertex lies on the cut plane (y = 0).
        for c in &axial {
            for vtx in &c.vertices {
                assert!(
                    vtx.y.abs() < 1e-4,
                    "cap vertex off meridian plane: y={}",
                    vtx.y
                );
            }
        }
    }

    /// GUARD: a smooth SOLID of revolution (revolve WITHOUT wall_thickness — one
    /// `SurfaceOfRevolution` wall closing to caps, no bore) sectioned by a
    /// meridian plane yields ONE closed profile (the filled cross-section spans
    /// the axis). Exercises the same tier-2 surface∩plane fragment path as the
    /// nozzle but with a single wall, so it independently covers the
    /// boundary-inclusive trim without depending on the tube topology.
    #[test]
    fn section_solid_of_revolution_axial_meridian_yields_one_profile() {
        let mut model = BRepModel::new();
        let s = revolve_smooth_solid(&mut model, NOZZLE_INNER, RevolveOptions::default())
            .expect("solid vase build");
        let axial = section_solid_by_plane(
            &model,
            s,
            Point3::new(0.0, 0.0, 40.0),
            Vector3::new(0.0, 1.0, 0.0),
            Tolerance::default(),
        )
        .expect("axial section must not error");
        assert!(
            !axial.is_empty(),
            "solid-of-revolution meridian cut returned EMPTY caps"
        );
        let total: f64 = axial.iter().map(cap_area).sum();
        assert!(
            total > 1.0,
            "solid-of-revolution meridian cross-section area {total:.3} implausibly small"
        );
        for c in &axial {
            for vtx in &c.vertices {
                assert!(
                    vtx.y.abs() < 1e-4,
                    "cap vertex off meridian plane: y={}",
                    vtx.y
                );
            }
        }
    }

    fn sid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    fn cap_area(c: &SectionCap) -> f64 {
        c.indices
            .iter()
            .map(|t| {
                let a = c.vertices[t[0] as usize];
                let b = c.vertices[t[1] as usize];
                let e = c.vertices[t[2] as usize];
                let e1 = Vector3::new(b.x - a.x, b.y - a.y, b.z - a.z);
                let e2 = Vector3::new(e.x - a.x, e.y - a.y, e.z - a.z);
                e1.cross(&e2).magnitude() * 0.5
            })
            .sum()
    }

    /// A box with a single vertical (Z-axis) through-bore, embedded (not a
    /// standalone cylinder) — matches the live-reported #section-404 repro
    /// shape (pocketed+4-bore block) minus the pocket and 3 of the 4 bores.
    fn box_with_vertical_bore(bx: f64, by: f64, bz: f64, bore_r: f64) -> (BRepModel, SolidId) {
        let mut model = BRepModel::new();
        let block = sid(TopologyBuilder::new(&mut model)
            .create_box_3d(bx, by, bz)
            .expect("block"));
        let bore = sid(TopologyBuilder::new(&mut model)
            .create_cylinder_3d(
                Point3::new(0.0, 0.0, -bz),
                Vector3::new(0.0, 0.0, 1.0),
                bore_r,
                bz * 4.0,
            )
            .expect("bore"));
        let s = boolean_operation(
            &mut model,
            block,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("through bore");
        (model, s)
    }

    /// #section-404 investigation: TWO horizontal (Y-axis) through-bores at
    /// different X positions, cut PERPENDICULAR to their axis (cut normal = Y
    /// = bore axis) — the outer rectangle carries 2 separate circular HOLE
    /// loops (true nesting, not the "comb" case below). This is one plausible
    /// reading of the live repro ("bores at ±7.07" as horizontal cross-bores).
    /// GUARD: multiple simultaneous holes in one cap must classify as depth-1
    /// children of the SAME outer (one cap, not 3) and their combined area
    /// must match the analytic rectangle-minus-2-circles value.
    #[test]
    fn section_perpendicular_cut_through_two_holes_nests_both_under_one_outer() {
        let mut model = BRepModel::new();
        let block = sid(TopologyBuilder::new(&mut model)
            .create_box_3d(30.0, 30.0, 20.0)
            .expect("block"));
        let bore1 = sid(TopologyBuilder::new(&mut model)
            .create_cylinder_3d(
                Point3::new(7.07, -20.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
                3.0,
                80.0,
            )
            .expect("bore1"));
        let after1 = boolean_operation(
            &mut model,
            block,
            bore1,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("bore1 cut");
        let bore2 = sid(TopologyBuilder::new(&mut model)
            .create_cylinder_3d(
                Point3::new(-7.07, -20.0, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
                3.0,
                80.0,
            )
            .expect("bore2"));
        let s = boolean_operation(
            &mut model,
            after1,
            bore2,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("bore2 cut");

        let caps = section_solid_by_plane(
            &model,
            s,
            Point3::new(0.0, 7.07, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
            Tolerance::default(),
        )
        .expect("section call must not error");
        assert_eq!(
            caps.len(),
            1,
            "2 holes nested under 1 outer must yield exactly 1 cap, got {}",
            caps.len()
        );
        let expected = 30.0 * 20.0 - 2.0 * std::f64::consts::PI * 9.0;
        let area = cap_area(&caps[0]);
        let rel = (area - expected).abs() / expected;
        assert!(
            rel < 0.03,
            "cap area {area:.3} vs analytic {expected:.3} (rel {rel})"
        );
    }

    /// #section-404 investigation: a box with TWO vertical through-bores
    /// (cascaded differences, matching the pocketed+4-bore block's
    /// construction history), cut by a plane whose normal equals the world Y
    /// axis at y = 7.07 (both bore centres sit at y=+7.07) — an AXIAL cut
    /// through both bores simultaneously, the same shape as the live repro's
    /// cutting plane. This is a "comb": the plane's own extent matches the
    /// bores' full through-height, so the two notches split one outer into
    /// 3 disjoint simple loops rather than nesting a hole inside one outer.
    /// GUARD: all 3 strips must come back (not dropped by a broken chain)
    /// and their combined area must match the analytic value.
    #[test]
    fn section_axial_cut_through_two_embedded_bores_yields_three_strips() {
        let mut model = BRepModel::new();
        let block = sid(TopologyBuilder::new(&mut model)
            .create_box_3d(30.0, 30.0, 20.0)
            .expect("block"));
        let bore1 = sid(TopologyBuilder::new(&mut model)
            .create_cylinder_3d(
                Point3::new(7.07, 7.07, -20.0),
                Vector3::new(0.0, 0.0, 1.0),
                3.0,
                80.0,
            )
            .expect("bore1"));
        let after1 = boolean_operation(
            &mut model,
            block,
            bore1,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("bore1 cut");
        let bore2 = sid(TopologyBuilder::new(&mut model)
            .create_cylinder_3d(
                Point3::new(-7.07, 7.07, -20.0),
                Vector3::new(0.0, 0.0, 1.0),
                3.0,
                80.0,
            )
            .expect("bore2"));
        let s = boolean_operation(
            &mut model,
            after1,
            bore2,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("bore2 cut");

        let caps = section_solid_by_plane(
            &model,
            s,
            Point3::new(0.0, 7.07, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
            Tolerance::default(),
        )
        .expect("section call must not error");
        assert_eq!(
            caps.len(),
            3,
            "axial cut through 2 through-bores must yield 3 disjoint strips, got {}",
            caps.len()
        );
        let total: f64 = caps.iter().map(cap_area).sum();
        let expected = 30.0 * 20.0 - 2.0 * (2.0 * 3.0) * 20.0;
        let rel = (total - expected).abs() / expected;
        assert!(
            rel < 0.03,
            "total strip area {total:.3} vs analytic {expected:.3} (rel {rel})"
        );
    }

    /// #section-404 REPRO: the EXACT live-failing topology. Block 60×40×30
    /// centred, SUBTRACT a top pocket (a 40×20×20 box raised +15 in z so it
    /// removes the top 10 mm over a 40(x)×20(y) footprint — a REENTRANT outer
    /// notch), then SUBTRACT 4 vertical through-bores r2.5 at (±7.07,±7.07).
    /// Section by a Y-normal plane at y=7.07 that crosses BOTH the pocket notch
    /// AND the 2 bores centred at y=+7.07.
    ///
    /// The y=7.07 cross-section is 3 DISJOINT pieces: two reentrant L-shaped
    /// outer pieces (pocket carves the top-middle away) flanking one central
    /// rectangle, split apart by the two bore slots (full-height at this y):
    ///   left/right L  = (30−9.57)·20 + 10·10 = 508.6 each
    ///   central rect  = (2·(7.07−2.5))·20    = 182.8
    ///   total         = 1200 mm²
    /// Bores-only (no pocket) works (3-strip test); pocket-only works (y=0 →
    /// 1400). Their COMBINATION — reentrant outer + interior slots at once —
    /// 404s live. This test MUST reproduce (empty / wrong area) before the fix.
    #[test]
    fn section_pocket_notch_plus_two_bores_yields_three_reentrant_pieces() {
        use crate::math::Matrix4;
        use crate::operations::transform::{transform_solid, TransformOptions};
        let mut model = BRepModel::new();
        // Block 60×40×30 centred: x∈[-30,30], y∈[-20,20], z∈[-15,15].
        let block = sid(TopologyBuilder::new(&mut model)
            .create_box_3d(60.0, 40.0, 30.0)
            .expect("block"));
        // Pocket 40×20×20, translated +15 in z → spans z∈[5,25], removing the
        // block's top 10 mm over a 40(x)×20(y) footprint.
        let pocket = sid(TopologyBuilder::new(&mut model)
            .create_box_3d(40.0, 20.0, 20.0)
            .expect("pocket"));
        transform_solid(
            &mut model,
            pocket,
            Matrix4::from_translation(&Vector3::new(0.0, 0.0, 15.0)),
            TransformOptions::default(),
        )
        .expect("translate pocket");
        let mut cur = boolean_operation(
            &mut model,
            block,
            pocket,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("pocket cut");

        // 4 vertical through-bores r2.5 at (±r45, ±r45), z∈[-30,30], where
        // r45 = 10·cos(45°) = 7.0710678… — the EXACT ring-radius-10 hole centres
        // the live `drill_pattern` computes. The section plane sits at y=7.07
        // (below), so it does NOT pass exactly through the bore axis but is
        // offset by ~1.07 µm — the near-axial (not on-axis) cut is what the
        // live repro exercises.
        let r45 = 10.0 * std::f64::consts::FRAC_1_SQRT_2;
        for (cx, cy) in [(r45, r45), (-r45, r45), (r45, -r45), (-r45, -r45)] {
            let bore = sid(TopologyBuilder::new(&mut model)
                .create_cylinder_3d(
                    Point3::new(cx, cy, -30.0),
                    Vector3::new(0.0, 0.0, 1.0),
                    2.5,
                    60.0,
                )
                .expect("bore"));
            cur = boolean_operation(
                &mut model,
                cur,
                bore,
                BooleanOp::Difference,
                BooleanOptions::default(),
            )
            .expect("bore cut");
        }

        let caps = section_solid_by_plane(
            &model,
            cur,
            Point3::new(0.0, 7.07, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
            Tolerance::default(),
        )
        .expect("section call must not error");

        assert!(
            !caps.is_empty(),
            "pocket+bores section returned EMPTY caps (#section-404 repro)"
        );
        let total: f64 = caps.iter().map(cap_area).sum();
        let expected = 1200.0;
        let rel = (total - expected).abs() / expected;
        assert!(
            rel < 0.03,
            "cap total area {total:.3} vs analytic {expected:.3} (rel {rel}); caps={}",
            caps.len()
        );
    }

    /// A single embedded through-bore, axial cut (cut plane contains the
    /// bore's own Z axis) — the minimal case backing the two multi-bore
    /// guards above. Splits one outer into 2 strips of equal area.
    #[test]
    fn section_axial_cut_through_one_embedded_bore_yields_two_strips() {
        let (model, s) = box_with_vertical_bore(40.0, 40.0, 20.0, 5.0);
        let caps = section_solid_by_plane(
            &model,
            s,
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
            Tolerance::default(),
        )
        .expect("section call must not error");
        assert_eq!(caps.len(), 2, "expected 2 strips, got {}", caps.len());
        let expected_each = (20.0 - 5.0) * 20.0;
        for (i, c) in caps.iter().enumerate() {
            let area = cap_area(c);
            let rel = (area - expected_each).abs() / expected_each;
            assert!(
                rel < 0.03,
                "cap{i} area {area:.3} vs analytic {expected_each:.3} (rel {rel})"
            );
        }
    }

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
    fn diag_section_box_perface() {
        // Per-face breakdown for the failing 60×60×20 box, cut z=0 normal +Z.
        let mut model = BRepModel::new();
        let gid = {
            let mut b = TopologyBuilder::new(&mut model);
            b.create_box_3d(60.0, 60.0, 20.0).expect("box")
        };
        let sid = match gid {
            GeometryId::Solid(id) => id,
            o => panic!("{o:?}"),
        };
        let origin = Vector3::ZERO;
        let normal = Vector3::new(0.0, 0.0, 1.0);
        let solid = model.get_solid(sid).expect("solid");
        let shell = model.shells.get(solid.outer_shell).expect("shell");
        for &fid in &shell.faces {
            let face = model.faces.get(fid).expect("face");
            let surf = model.surfaces.get(face.surface_id).expect("surf");
            let (u0, u1, v0, v1) = get_face_parameter_bounds(face, &model);
            let cfg = SurfacePlaneIntersectionConfig {
                param_bounds_override: Some(((u0, u1), (v0, v1))),
                ..Default::default()
            };
            let curves = intersect_surface_plane(surf, origin, normal, &cfg)
                .map(|c| c.len())
                .unwrap_or(usize::MAX);
            let mut frags = Vec::new();
            collect_face_fragments(&model, fid, origin, normal, &mut frags);
            eprintln!(
                "face {fid} {:?} uv=[{u0:.1},{u1:.1}]x[{v0:.1},{v1:.1}] curves={curves} frags={}",
                surf.surface_type(),
                frags.len()
            );
        }
    }

    /// #83 GUARD: a z=0 section through boxes of any aspect ratio must yield
    /// exactly one cap of area w·h. Wide/short boxes (60×60×20, 60×40×20) used
    /// to give 0 caps because the marching SSI fragmented the straight cut line
    /// on the side faces; the exact Plane×Plane clip fixes every aspect ratio.
    #[test]
    fn section_planar_box_dims_match_analytic() {
        // Section a z=0 plane (normal +Z) through boxes of varying dims.
        for (w, h, d) in [
            (10.0, 10.0, 10.0),
            (40.0, 40.0, 40.0),
            (60.0, 60.0, 20.0),
            (60.0, 40.0, 20.0),
            (20.0, 20.0, 60.0),
        ] {
            let mut model = BRepModel::new();
            let gid = {
                let mut b = TopologyBuilder::new(&mut model);
                b.create_box_3d(w, h, d).expect("box")
            };
            let sid = match gid {
                GeometryId::Solid(id) => id,
                o => panic!("{o:?}"),
            };
            let caps = section_solid_by_plane(
                &model,
                sid,
                Vector3::ZERO,
                Vector3::new(0.0, 0.0, 1.0),
                Tolerance::default(),
            )
            .expect("section");
            let area: f64 = caps
                .iter()
                .flat_map(|c| c.indices.iter().map(move |t| (c, t)))
                .map(|(c, t)| {
                    let a = c.vertices[t[0] as usize];
                    let b = c.vertices[t[1] as usize];
                    let e = c.vertices[t[2] as usize];
                    let e1 = Vector3::new(b.x - a.x, b.y - a.y, b.z - a.z);
                    let e2 = Vector3::new(e.x - a.x, e.y - a.y, e.z - a.z);
                    e1.cross(&e2).magnitude() * 0.5
                })
                .sum();
            assert_eq!(
                caps.len(),
                1,
                "box {w}x{h}x{d}: expected 1 cap, got {}",
                caps.len()
            );
            assert!(
                (area - w * h).abs() < 0.5,
                "box {w}x{h}x{d}: section area {area:.2} != {:.2}",
                w * h
            );
        }
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
    fn section_cylinder_axial_plane_produces_rectangle_85() {
        // SECTION #85: a plane CONTAINING the cylinder axis (here x = 0, normal
        // +X, through the z-axis) cuts the solid in a rectangle 2r × h, NOT a
        // circle. Cylinder r2 h10, z ∈ [0,10] ⇒ rectangle 4 × 10, area 40.
        let (model, solid_id) = build_cylinder_model(2.0, 10.0);
        let caps = section_solid_by_plane(
            &model,
            solid_id,
            Point3::new(0.0, 0.0, 5.0),
            Vector3::new(1.0, 0.0, 0.0),
            Tolerance::default(),
        )
        .expect("section call");
        assert_eq!(caps.len(), 1, "axial cylinder section: expected one cap");
        let area: f64 = caps
            .iter()
            .flat_map(|c| c.indices.iter().map(move |t| (c, t)))
            .map(|(c, t)| {
                let a = c.vertices[t[0] as usize];
                let b = c.vertices[t[1] as usize];
                let e = c.vertices[t[2] as usize];
                let e1 = Vector3::new(b.x - a.x, b.y - a.y, b.z - a.z);
                let e2 = Vector3::new(e.x - a.x, e.y - a.y, e.z - a.z);
                e1.cross(&e2).magnitude() * 0.5
            })
            .sum();
        assert!(
            (area - 40.0).abs() < 0.5,
            "axial cylinder section area {area:.2} != 40.0 (2r×h rectangle)"
        );
        for v in &caps[0].vertices {
            assert!(v.x.abs() < 1e-4, "cap vertex off plane: x = {}", v.x);
        }
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
