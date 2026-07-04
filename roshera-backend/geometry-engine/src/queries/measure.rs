//! Exact face-pair measurement — kernel half of the interactive dimension layer.
//!
//! [`measure`] answers the analytic geometry question that clicking two faces
//! asks: *what is the exact relation between these surfaces?*  Results are typed
//! to the geometric case (distance between parallel planes, angle between
//! non-parallel planes, axis distance between parallel cylinders, etc.) so the
//! REST layer can surface a 422 with the kernel's own reason when the pair does
//! not admit a crisp measurement, rather than guessing.
//!
//! ## Case matrix
//! | Inputs                                    | Output                        |
//! |-------------------------------------------|-------------------------------|
//! | single Cylinder face                      | `Diameter { 2r, axis }`       |
//! | single Plane face                         | `FaceInfo { area, normal }`   |
//! | single other face                         | `FaceInfo { area, None }`     |
//! | Plane × Plane, parallel                   | `Distance { plane_plane }`    |
//! | Plane × Plane, non-parallel               | `Angle { degrees ∈ [0°,180°] between OUTWARD normals }` |
//! | Cylinder × Cylinder, parallel axes        | `Distance { axis_axis }`      |
//! | Cylinder × Cylinder, skew axes            | `Unsupported`                 |
//! | Cylinder × Plane, axis ⊥ normal           | `Distance { axis_plane }`     |
//! | Cylinder × Plane, not ⊥                   | `Unsupported`                 |
//! | any other pair                            | `Distance { nearest }` or `Unsupported` |
//!
//! ## Area note
//! Area is computed by [`Face::area`] which requires `&mut` to populate its
//! cache. The function therefore takes `&mut BRepModel`, matching the pattern
//! established by `BRepModel::query_face` / `BRepModel::mass_properties_for`.

use crate::math::{Point3, Vector3};
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::surface::{Cylinder, Plane};
use crate::primitives::topology_builder::BRepModel;

// ─── Tolerances ───────────────────────────────────────────────────────────────

/// `|n_a · n_b|` ABOVE this ⇒ the two directions count as PARALLEL
/// (cone half-angle ≈ 0.81°).  Deliberately loose: a parallel-plate thickness
/// measurement should tolerate drafting slop — a plate tilted a fraction of a
/// degree still has one honest thickness.
const PARALLEL_DOT_TOL: f64 = 0.9999;

/// `|a · n|` BELOW this ⇒ the two directions count as PERPENDICULAR
/// (≈ 0.0057°).  Four orders of magnitude tighter than [`PARALLEL_DOT_TOL`]
/// on purpose: an axis⟂plane distance is only well-defined while the axis
/// stays parallel to the plane — the moment the axis tilts, every point of
/// the axis is at a different distance and "the" distance stops existing, so
/// the gate must be near-exact.  Also used as the sine bound for cylinder
/// axis parallelism (`|a×b| ≤ tol`), where the same argument applies to the
/// axis-to-axis distance.
const PERP_DOT_TOL: f64 = 1e-4;

// ─── Public types ─────────────────────────────────────────────────────────────

/// What the caller is measuring.  Extensible to Edge later.
#[derive(Debug, Clone, Copy)]
pub enum MeasureSubject {
    Face { solid: SolidId, face: FaceId },
}

/// The result of a successful measurement.
#[derive(Debug, Clone)]
pub enum MeasureResult {
    /// A translational gap.
    ///
    /// * `value`     — positive distance in model units (mm)
    /// * `anchor`    — representative world-space point on the gap
    /// * `direction` — unit vector pointing from surface a toward surface b
    /// * `kind`      — one of `"plane_plane"`, `"axis_axis"`, `"axis_plane"`, `"nearest"`
    ///
    /// The analytic kinds (`plane_plane`, `axis_axis`, `axis_plane`) are
    /// exact.  `"nearest"` is the LOCAL minimum found by alternating
    /// projection from a deterministic seed (face a's UV mid-point) with
    /// both footpoints verified inside their trims — for non-convex face
    /// pairs with multiple closest-approach candidates it is not certified
    /// to be the global nearest.
    Distance {
        value: f64,
        anchor: [f64; 3],
        direction: [f64; 3],
        kind: &'static str,
    },
    /// Angle between two non-parallel planar faces, measured between their
    /// OUTWARD normals: `degrees = acos(n_a · n_b) ∈ [0°, 180°]`.
    ///
    /// The full range is reported — no folding into [0°, 90°].  Convention
    /// notes: for two faces of the SAME solid meeting at a common edge, the
    /// outward-normal angle is the SUPPLEMENT of the interior dihedral —
    /// a box corner (90° interior) reads 90°, faces meeting at a 120°
    /// interior bevel read 60°, and near-coplanar neighbours read near 0°.
    /// Two faces of DIFFERENT bodies tilted θ apart read θ directly (e.g.
    /// a plate rotated 120° against a reference plate reads 120°, never the
    /// folded 60°).  Exactly parallel or anti-parallel pairs never reach
    /// this variant; they report `Distance { kind: "plane_plane" }` instead.
    ///
    /// `anchor` is the midpoint between the two plane origins.
    Angle { degrees: f64, anchor: [f64; 3] },
    /// Cylinder diameter with axis anchor and direction.
    Diameter {
        value: f64,
        anchor: [f64; 3],
        axis: [f64; 3],
    },
    /// Generic face information returned for a single-face query.
    ///
    /// `normal` is present only when the face is planar, and is the OUTWARD
    /// normal (surface normal flipped when the face is oriented Backward).
    /// `anchor` is an approximation of the face centroid.
    FaceInfo {
        area: f64,
        normal: Option<[f64; 3]>,
        anchor: [f64; 3],
    },
}

/// Errors returned by [`measure`].
#[derive(Debug, thiserror::Error)]
pub enum MeasureError {
    /// A referenced solid or face id was not found in the model, or the face
    /// does not belong to the requested solid (it may have been consumed by a
    /// later boolean or other mutating operation and persists only in the
    /// arena, not in any shell of the solid).
    #[error(
        "Face {face} is not part of solid {solid} — it may have been consumed by a later operation"
    )]
    NotFound { solid: SolidId, face: FaceId },

    /// The face pair does not admit the requested measurement; `reason` is
    /// actionable and non-empty.
    #[error("Unsupported measurement: {reason}")]
    Unsupported { reason: String },
}

// ─── Public entry point ───────────────────────────────────────────────────────

/// Measure the geometric relation between one or two faces.
///
/// Takes `&mut BRepModel` because area computation warms a per-face cache on
/// first call (`Face::area` → `Face::compute_stats`).  After the cache is warm
/// subsequent calls are O(1).
pub fn measure(
    model: &mut BRepModel,
    a: MeasureSubject,
    b: Option<MeasureSubject>,
) -> Result<MeasureResult, MeasureError> {
    match b {
        None => measure_one(model, a),
        Some(b) => measure_two(model, a, b),
    }
}

// ─── Single-face measurement ──────────────────────────────────────────────────

fn measure_one(model: &mut BRepModel, subj: MeasureSubject) -> Result<MeasureResult, MeasureError> {
    let MeasureSubject::Face { solid, face } = subj;

    let classified = surface_classify(model, solid, face)?;
    let anchor = face_surface_midpoint(model, face)
        .map(pt_to_arr)
        .unwrap_or([0.0, 0.0, 0.0]);

    // Area is only needed for the FaceInfo branches; a cylinder's diameter is
    // read straight off the surface and must not be refused because the area
    // integral failed.  When the area cannot be computed we refuse with a
    // typed reason — never a fabricated 0.0 (a real zero-area face and a
    // failed computation must stay distinguishable).
    let area_or_refuse = |model: &mut BRepModel| -> Result<f64, MeasureError> {
        face_area(model, face).ok_or_else(|| MeasureError::Unsupported {
            reason: format!(
                "Face {face} resolved but its trimmed area could not be computed \
                 (area integral failed on the supporting surface). \
                 The face may have a degenerate trim loop."
            ),
        })
    };

    match classified {
        Classified::Cylinder {
            radius,
            axis,
            origin,
        } => Ok(MeasureResult::Diameter {
            value: 2.0 * radius,
            anchor: pt_to_arr(origin),
            axis: vec_to_arr(axis),
        }),
        Classified::Plane { normal, .. } => Ok(MeasureResult::FaceInfo {
            area: area_or_refuse(model)?,
            normal: Some(vec_to_arr(normal)),
            anchor,
        }),
        Classified::Other => Ok(MeasureResult::FaceInfo {
            area: area_or_refuse(model)?,
            normal: None,
            anchor,
        }),
    }
}

// ─── Two-face measurement ─────────────────────────────────────────────────────

fn measure_two(
    model: &mut BRepModel,
    a: MeasureSubject,
    b: MeasureSubject,
) -> Result<MeasureResult, MeasureError> {
    let MeasureSubject::Face {
        solid: sa,
        face: fa,
    } = a;
    let MeasureSubject::Face {
        solid: sb,
        face: fb,
    } = b;

    let class_a = surface_classify(model, sa, fa)?;
    let class_b = surface_classify(model, sb, fb)?;

    match (class_a, class_b) {
        // ── Plane × Plane ────────────────────────────────────────────────────
        (
            Classified::Plane {
                normal: na,
                origin: oa,
            },
            Classified::Plane {
                normal: nb,
                origin: ob,
            },
        ) => measure_plane_plane(na, oa, nb, ob),

        // ── Cylinder × Cylinder ──────────────────────────────────────────────
        (
            Classified::Cylinder {
                radius: _,
                axis: aa,
                origin: pa,
            },
            Classified::Cylinder {
                radius: _,
                axis: ab,
                origin: pb,
            },
        ) => measure_cyl_cyl(aa, pa, ab, pb),

        // ── Cylinder × Plane ─────────────────────────────────────────────────
        (
            Classified::Cylinder {
                radius: _,
                axis: aa,
                origin: pa,
            },
            Classified::Plane {
                normal: nb,
                origin: ob,
            },
        ) => measure_cyl_plane(aa, pa, nb, ob),

        // ── Plane × Cylinder (symmetric) ─────────────────────────────────────
        (
            Classified::Plane {
                normal: na,
                origin: oa,
            },
            Classified::Cylinder {
                radius: _,
                axis: ab,
                origin: pb,
            },
        ) => measure_cyl_plane(ab, pb, na, oa),

        // ── Any other pair → nearest-point fallback ───────────────────────────
        _ => measure_nearest(model, fa, fb),
    }
}

// ─── Plane × Plane ───────────────────────────────────────────────────────────

fn measure_plane_plane(
    na: Vector3,
    oa: Point3,
    nb: Vector3,
    ob: Point3,
) -> Result<MeasureResult, MeasureError> {
    // Parallelism uses |dot| so anti-parallel (facing) planes — the common
    // plate-thickness case — also measure as a distance.
    if na.dot(&nb).abs() > PARALLEL_DOT_TOL {
        // Parallel planes: perpendicular offset.
        let signed = (ob - oa).dot(&na);
        let value = signed.abs();
        let mid = oa + na * (signed * 0.5);
        Ok(MeasureResult::Distance {
            value,
            anchor: pt_to_arr(mid),
            direction: vec_to_arr(na),
            kind: "plane_plane",
        })
    } else {
        // Non-parallel: angle between OUTWARD normals, full [0°, 180°] range.
        // No |·| fold — a 120° relation reports 120°, not 60° (see the Angle
        // variant doc).
        let cos_theta = na.dot(&nb).clamp(-1.0, 1.0);
        let degrees = cos_theta.acos().to_degrees();
        let anchor = pt_to_arr(midpoint(oa, ob));
        Ok(MeasureResult::Angle { degrees, anchor })
    }
}

// ─── Cylinder × Cylinder ─────────────────────────────────────────────────────

fn measure_cyl_cyl(
    aa: Vector3,
    pa: Point3,
    ab: Vector3,
    pb: Point3,
) -> Result<MeasureResult, MeasureError> {
    // Parallel iff |aa × ab| ≈ 0 (sine of the inter-axis angle; same bound as
    // the perpendicularity gate — see PERP_DOT_TOL for why it is tight).
    let cross_mag = aa.cross(&ab).magnitude();
    if cross_mag > PERP_DOT_TOL {
        return Err(MeasureError::Unsupported {
            reason: "Skew-axis cylinder pair: axis-to-axis distance for non-parallel \
                     cylinders is not yet supported. \
                     Select two cylinders whose axes are parallel (e.g. both bores \
                     along the same direction) to get center-to-center distance."
                .to_string(),
        });
    }

    // Parallel axes: distance between the two axis lines.
    // d = |(pb − pa) × aa| (aa is already unit).
    let diff = pb - pa;
    let value = diff.cross(&aa).magnitude();

    // Closest point on axis-a to pb: pa + aa * ((pb − pa) · aa).
    let t = diff.dot(&aa);
    let closest_a = pa + aa * t;
    let anchor = midpoint(closest_a, pb);

    // Direction from closest_a to pb (perpendicular to the axes).
    let dir_vec = if value > 1e-12 {
        (pb - closest_a)
            .normalize()
            .unwrap_or_else(|_| aa.perpendicular())
    } else {
        aa.perpendicular()
    };

    Ok(MeasureResult::Distance {
        value,
        anchor: pt_to_arr(anchor),
        direction: vec_to_arr(dir_vec),
        kind: "axis_axis",
    })
}

// ─── Cylinder × Plane ────────────────────────────────────────────────────────

fn measure_cyl_plane(
    cyl_axis: Vector3,
    cyl_origin: Point3,
    plane_normal: Vector3,
    plane_origin: Point3,
) -> Result<MeasureResult, MeasureError> {
    // Axis perpendicular to plane normal iff |axis · normal| ≈ 0.
    let dot = cyl_axis.dot(&plane_normal).abs();
    if dot > PERP_DOT_TOL {
        return Err(MeasureError::Unsupported {
            reason: format!(
                "Cylinder axis is not perpendicular to the plane normal \
                 (|axis·normal| = {dot:.6}). \
                 Select a cylinder whose axis runs parallel to the face \
                 (e.g. a bore drilled through the face)."
            ),
        });
    }

    // Signed distance from cyl_origin to the plane:
    //   d = (cyl_origin − plane_origin) · plane_normal
    let d = (cyl_origin - plane_origin).dot(&plane_normal);
    let value = d.abs();

    // Foot of the perpendicular from cyl_origin onto the plane.
    let foot = cyl_origin - plane_normal * d;
    let anchor = midpoint(cyl_origin, foot);

    Ok(MeasureResult::Distance {
        value,
        anchor: pt_to_arr(anchor),
        direction: vec_to_arr(plane_normal),
        kind: "axis_plane",
    })
}

// ─── Nearest-point fallback ───────────────────────────────────────────────────

/// Maximum alternating-projection sweeps for the nearest fallback.  The
/// projection contracts linearly with ratio ≈ r/d (surface curvature radius
/// over separation); 256 sweeps cover ratios up to ~0.9 to full f64 precision.
const NEAREST_MAX_ITERS: usize = 256;

/// Absolute floor of the convergence test: iteration stops when the realized
/// distance changes by less than this between sweeps (mm — far below
/// manufacturing relevance).  Combined with [`NEAREST_RELATIVE_TOL`] so the
/// effective tolerance scales with the distance being measured.
const NEAREST_CONVERGENCE_TOL: f64 = 1e-12;

/// Relative component of the convergence test: `tol = max(ABS, REL × d)`.
/// For geometry far from the origin the distance value carries ulp-scale
/// noise proportional to its own magnitude (an f64 near 1e6 mm has ulps of
/// ~1e-10 mm), so a purely absolute 1e-12 test could oscillate forever and
/// refuse spuriously.  1e-9 relative keeps nine significant digits — far
/// beyond any manufacturing tolerance — while making the test scale-free.
const NEAREST_RELATIVE_TOL: f64 = 1e-9;

fn measure_nearest(
    model: &mut BRepModel,
    fa: FaceId,
    fb: FaceId,
) -> Result<MeasureResult, MeasureError> {
    use crate::queries::trim::closest_point_on_face;

    // Seed: UV-midpoint of face a.
    let seed_a = face_surface_midpoint(model, fa).ok_or_else(|| MeasureError::Unsupported {
        reason: "Cannot compute a seed point for nearest-point iteration: \
                 face a not found or its surface has no evaluable midpoint."
            .to_string(),
    })?;

    // Alternating projection (Cheney–Goldstein): project onto face b, then
    // back onto face a, until the realized distance stops improving.  Each
    // projection is the exact surface closest-point (Newton-refined); the
    // distance sequence is non-increasing, so convergence-or-refusal is
    // guaranteed within the sweep budget.
    let mut q = seed_a;
    let mut prev_dist = f64::INFINITY;
    let mut converged = false;
    let mut pair: Option<(
        crate::queries::trim::FaceClosestPoint,
        crate::queries::trim::FaceClosestPoint,
    )> = None;

    for _ in 0..NEAREST_MAX_ITERS {
        let cpb = closest_point_on_face(model, fb, q).ok_or_else(|| MeasureError::Unsupported {
            reason: "Nearest-point projection failed on face b. \
                         The face pair may be degenerate or have an unsupported surface type."
                .to_string(),
        })?;
        let cpa = closest_point_on_face(model, fa, cpb.point).ok_or_else(|| {
            MeasureError::Unsupported {
                reason: "Nearest-point projection failed on face a. \
                         The face pair may be degenerate or have an unsupported surface type."
                    .to_string(),
            }
        })?;

        let d = cpa.point.distance(&cpb.point);
        let improved = prev_dist - d;
        q = cpa.point;
        pair = Some((cpa, cpb));
        // Scale-aware convergence: absolute floor for near-touching pairs,
        // relative term so far-from-origin geometry doesn't spuriously
        // refuse on ulp-scale oscillation of the distance value.
        let tol = NEAREST_CONVERGENCE_TOL.max(NEAREST_RELATIVE_TOL * d);
        if improved.abs() < tol {
            converged = true;
            break;
        }
        prev_dist = d;
    }

    let Some((cpa, cpb)) = pair else {
        return Err(MeasureError::Unsupported {
            reason: "Nearest-point iteration produced no footpoint pair.".to_string(),
        });
    };
    if !converged {
        return Err(MeasureError::Unsupported {
            reason: format!(
                "Nearest-point iteration did not converge within {NEAREST_MAX_ITERS} sweeps \
                 (last distance {prev_dist:.6} mm still improving). \
                 The face pair may oscillate between multiple local minima; \
                 try a geometrically crisper pair (plane/plane, cylinder/cylinder)."
            ),
        });
    }

    // Refuse when EITHER converged footpoint lies outside its face's trimmed
    // boundary.  The iteration runs on the UNTRIMMED supporting surfaces, so
    // an outside footpoint means the reported distance would be measured to a
    // point where no face material exists — a fabricated number (e.g. a
    // sphere hovering past a plate's edge would read the gap to phantom
    // plate).  The honest answer in that configuration is the distance to
    // the trim BOUNDARY (edge), which needs boundary-constrained iteration —
    // a documented future upgrade, not silently approximated here.
    if !cpa.inside || !cpb.inside {
        let culprit = if !cpa.inside && !cpb.inside {
            "both faces' nearest points lie"
        } else if !cpa.inside {
            "the first face's nearest point lies"
        } else {
            "the second face's nearest point lies"
        };
        return Err(MeasureError::Unsupported {
            reason: format!(
                "Nearest-point iteration converged, but {culprit} outside that face's \
                 trimmed boundary — the distance would be measured to a point where no \
                 face material exists. Distance to a trimmed boundary (face edge) is \
                 not yet supported; select faces whose closest approach is interior \
                 to both faces."
            ),
        });
    }

    let pa = cpa.point;
    let pb = cpb.point;
    let value = pa.distance(&pb);
    let anchor = midpoint(pa, pb);

    let direction = if value > 1e-12 {
        (pb - pa).normalize().unwrap_or(Vector3::Z)
    } else {
        Vector3::Z
    };

    Ok(MeasureResult::Distance {
        value,
        anchor: pt_to_arr(anchor),
        direction: vec_to_arr(direction),
        kind: "nearest",
    })
}

// ─── Surface classification ───────────────────────────────────────────────────

/// Rich enum capturing all analytic parameters we need per surface type.
#[derive(Debug, Clone, Copy)]
enum Classified {
    Cylinder {
        radius: f64,
        axis: Vector3,
        origin: Point3,
    },
    Plane {
        normal: Vector3,
        origin: Point3,
    },
    Other,
}

fn surface_classify(
    model: &BRepModel,
    solid: SolidId,
    face_id: FaceId,
) -> Result<Classified, MeasureError> {
    // Verify the solid exists.
    let solid_data = model.solids.get(solid).ok_or(MeasureError::NotFound {
        solid,
        face: face_id,
    })?;

    // Verify the face BELONGS to this solid — walk its outer shell and all
    // inner shells. Input solids persist in the model arena after booleans
    // (pid lineage relies on this), so a face that was consumed by a later
    // operation resolves by id but no longer belongs to any shell of the
    // solid the caller named. Measuring against a dead face fabricates a
    // stale number; the honest answer is NotFound.
    //
    // Complexity: O(faces in solid's shells) — bounded by the solid's own
    // face count, not the total arena size. Typical solids have O(10..100)
    // faces; this check is negligible next to the surface evaluation that
    // follows.
    let face_in_solid = {
        let outer_shell = model.shells.get(solid_data.outer_shell);
        let in_outer = outer_shell
            .map(|sh| sh.faces.contains(&face_id))
            .unwrap_or(false);
        if in_outer {
            true
        } else {
            solid_data.inner_shells.iter().any(|&shell_id| {
                model
                    .shells
                    .get(shell_id)
                    .map(|sh| sh.faces.contains(&face_id))
                    .unwrap_or(false)
            })
        }
    };
    if !face_in_solid {
        return Err(MeasureError::NotFound {
            solid,
            face: face_id,
        });
    }

    let face = model.faces.get(face_id).ok_or(MeasureError::NotFound {
        solid,
        face: face_id,
    })?;

    let surf = model
        .surfaces
        .get(face.surface_id)
        .ok_or(MeasureError::NotFound {
            solid,
            face: face_id,
        })?;

    if let Some(cyl) = surf.as_any().downcast_ref::<Cylinder>() {
        let axis = cyl.axis.normalize().unwrap_or(Vector3::Z);
        return Ok(Classified::Cylinder {
            radius: cyl.radius,
            axis,
            origin: cyl.origin,
        });
    }

    if let Some(pln) = surf.as_any().downcast_ref::<Plane>() {
        // OUTWARD normal: flip the surface normal when the face is oriented
        // Backward relative to its surface (same convention as
        // `cd::face_outward_normal_at`).  The Angle result's [0°, 180°] range
        // is only meaningful over outward normals; distance cases use |·| and
        // are sign-invariant.
        let normal = pln.normal.normalize().unwrap_or(Vector3::Z) * face.orientation.sign();
        return Ok(Classified::Plane {
            normal,
            origin: pln.origin,
        });
    }

    Ok(Classified::Other)
}

// ─── Area helper ─────────────────────────────────────────────────────────────

/// Compute the trimmed area of a face.
///
/// Returns `None` when the face is not found or the area integral fails —
/// NEVER a sentinel value, so a genuine zero-area face and a failed
/// computation stay distinguishable.  Callers convert `None` into a typed
/// `Unsupported` refusal rather than forwarding a fabricated number.
fn face_area(model: &mut BRepModel, face_id: FaceId) -> Option<f64> {
    // Read the face tolerance (immutable path).
    let tol = model
        .faces
        .get(face_id)
        .map(|f| crate::math::Tolerance::from_distance(f.tolerance))?;

    // `Face::area` requires `&mut Face` and `&mut LoopStore`.
    // We cannot split the borrow (`faces` and `loops` are both fields of `model`).
    // Pattern (same as readable/query.rs): clone the face, run area on the clone
    // against the model's stores. The cache is NOT warmed on the original (the
    // clone is discarded), which is acceptable for a query path.
    let mut face_clone = model.faces.get(face_id)?.clone();
    face_clone
        .area(
            &mut model.loops,
            &model.vertices,
            &model.edges,
            &model.curves,
            &model.surfaces,
            tol,
        )
        .ok()
}

// ─── Geometry helpers ─────────────────────────────────────────────────────────

/// Evaluate a face's supporting surface at the UV centre of its parameter bounds.
fn face_surface_midpoint(model: &BRepModel, face_id: FaceId) -> Option<Point3> {
    let face = model.faces.get(face_id)?;
    let surf = model.surfaces.get(face.surface_id)?;
    let [u0, u1, v0, v1] = face.uv_bounds;
    let u = 0.5 * (u0 + u1);
    let v = 0.5 * (v0 + v1);
    surf.evaluate_full(u, v).ok().map(|sp| sp.position)
}

#[inline]
fn pt_to_arr(p: Point3) -> [f64; 3] {
    [p.x, p.y, p.z]
}

#[inline]
fn vec_to_arr(v: Vector3) -> [f64; 3] {
    [v.x, v.y, v.z]
}

#[inline]
fn midpoint(a: Point3, b: Point3) -> Point3 {
    Point3::new(0.5 * (a.x + b.x), 0.5 * (a.y + b.y), 0.5 * (a.z + b.z))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Point3, Vector3};
    use crate::primitives::surface::{Cylinder, Plane};
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn make_solid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            other => panic!("expected Solid, got {other:?}"),
        }
    }

    /// First face whose supporting surface is a [`Sphere`].
    fn first_sphere_face(m: &BRepModel, solid: SolidId) -> FaceId {
        use crate::primitives::surface::Sphere;
        let s = m.solids.get(solid).expect("solid");
        let sh = m.shells.get(s.outer_shell).expect("shell");
        for &fid in &sh.faces {
            let f = m.faces.get(fid).expect("face");
            let surf = m.surfaces.get(f.surface_id).expect("surface");
            if surf.as_any().downcast_ref::<Sphere>().is_some() {
                return fid;
            }
        }
        panic!("no sphere face in solid {solid}");
    }

    /// First face whose supporting surface is a [`Cylinder`].
    fn first_cyl_face(m: &BRepModel, solid: SolidId) -> FaceId {
        let s = m.solids.get(solid).expect("solid");
        let sh = m.shells.get(s.outer_shell).expect("shell");
        for &fid in &sh.faces {
            let f = m.faces.get(fid).expect("face");
            let surf = m.surfaces.get(f.surface_id).expect("surface");
            if surf.as_any().downcast_ref::<Cylinder>().is_some() {
                return fid;
            }
        }
        panic!("no cylinder face in solid {solid}");
    }

    /// Face whose plane OUTWARD normal most closely aligns with `target`
    /// (unit vector).  Outward = surface normal × orientation sign, the same
    /// convention `measure` itself uses.
    fn plane_face_near(m: &BRepModel, solid: SolidId, target: Vector3) -> FaceId {
        let s = m.solids.get(solid).expect("solid");
        let sh = m.shells.get(s.outer_shell).expect("shell");
        let mut best: Option<(f64, FaceId)> = None;
        for &fid in &sh.faces {
            let f = m.faces.get(fid).expect("face");
            let surf = m.surfaces.get(f.surface_id).expect("surface");
            if let Some(p) = surf.as_any().downcast_ref::<Plane>() {
                let n = p.normal.normalize().unwrap_or(Vector3::Z) * f.orientation.sign();
                let d = n.dot(&target);
                if best.map_or(true, |(prev, _)| d > prev) {
                    best = Some((d, fid));
                }
            }
        }
        best.expect("no plane face matching target in solid").1
    }

    // ── Case 1: parallel plate faces → exact plane_plane distance ─────────────

    #[test]
    fn plate_parallel_planes_exact_distance() {
        let mut m = BRepModel::new();
        // 40×40×10 box: top (+Z) and bottom (−Z) planes 10 mm apart.
        let b = make_solid(
            TopologyBuilder::new(&mut m)
                .create_box_3d(40.0, 40.0, 10.0)
                .expect("box"),
        );
        let top = plane_face_near(&m, b, Vector3::Z);
        let bot = plane_face_near(&m, b, Vector3::new(0.0, 0.0, -1.0));

        let result = measure(
            &mut m,
            MeasureSubject::Face {
                solid: b,
                face: top,
            },
            Some(MeasureSubject::Face {
                solid: b,
                face: bot,
            }),
        )
        .expect("measure succeeded");

        match result {
            MeasureResult::Distance { value, kind, .. } => {
                assert_eq!(kind, "plane_plane");
                assert!((value - 10.0).abs() < 1e-9, "expected 10.0 mm, got {value}");
            }
            other => panic!("expected Distance(plane_plane), got {other:?}"),
        }
    }

    // ── Case 2: adjacent box faces → 90° dihedral ────────────────────────────

    #[test]
    fn adjacent_box_faces_ninety_degrees() {
        let mut m = BRepModel::new();
        let b = make_solid(
            TopologyBuilder::new(&mut m)
                .create_box_3d(20.0, 20.0, 20.0)
                .expect("box"),
        );
        let top = plane_face_near(&m, b, Vector3::Z);
        // Select a side face (the one whose normal has the largest −Y component).
        let front = plane_face_near(&m, b, Vector3::new(0.0, -1.0, 0.0));

        let result = measure(
            &mut m,
            MeasureSubject::Face {
                solid: b,
                face: top,
            },
            Some(MeasureSubject::Face {
                solid: b,
                face: front,
            }),
        )
        .expect("measure succeeded");

        match result {
            MeasureResult::Angle { degrees, .. } => {
                assert!(
                    (degrees - 90.0).abs() < 1e-9,
                    "expected 90.0°, got {degrees}"
                );
            }
            other => panic!("expected Angle, got {other:?}"),
        }
    }

    // ── Case 3: two parallel Ø8 cylinders at 30 mm centres → axis_axis 30.0 ──

    #[test]
    fn two_parallel_bores_axis_axis_distance() {
        let mut m = BRepModel::new();
        let c1 = make_solid(
            TopologyBuilder::new(&mut m)
                .create_cylinder_3d(Point3::ZERO, Vector3::Z, 4.0, 20.0)
                .expect("cyl 1"),
        );
        let c2 = make_solid(
            TopologyBuilder::new(&mut m)
                .create_cylinder_3d(Point3::new(30.0, 0.0, 0.0), Vector3::Z, 4.0, 20.0)
                .expect("cyl 2"),
        );
        let f1 = first_cyl_face(&m, c1);
        let f2 = first_cyl_face(&m, c2);

        let result = measure(
            &mut m,
            MeasureSubject::Face {
                solid: c1,
                face: f1,
            },
            Some(MeasureSubject::Face {
                solid: c2,
                face: f2,
            }),
        )
        .expect("measure succeeded");

        match result {
            MeasureResult::Distance { value, kind, .. } => {
                assert_eq!(kind, "axis_axis");
                assert!((value - 30.0).abs() < 1e-9, "expected 30.0 mm, got {value}");
            }
            other => panic!("expected Distance(axis_axis), got {other:?}"),
        }
    }

    // ── Case 4: bore axis to a parallel side face → axis_plane exact ─────────

    #[test]
    fn bore_axis_to_side_face_axis_plane() {
        let mut m = BRepModel::new();
        // Box 60×60×20 centred at origin → the +X face is a plane at X = 30.
        let plate = make_solid(
            TopologyBuilder::new(&mut m)
                .create_box_3d(60.0, 60.0, 20.0)
                .expect("plate"),
        );
        // Cylinder at X=10, axis along Z.  Axis-origin is at (10, 0, 0).
        // Distance from (10,0,0) to the plane X=30 is 20 mm.
        let bore = make_solid(
            TopologyBuilder::new(&mut m)
                .create_cylinder_3d(Point3::new(10.0, 0.0, 0.0), Vector3::Z, 4.0, 20.0)
                .expect("bore"),
        );
        let cyl_f = first_cyl_face(&m, bore);
        let side_f = plane_face_near(&m, plate, Vector3::X);

        let result = measure(
            &mut m,
            MeasureSubject::Face {
                solid: bore,
                face: cyl_f,
            },
            Some(MeasureSubject::Face {
                solid: plate,
                face: side_f,
            }),
        )
        .expect("measure succeeded");

        match result {
            MeasureResult::Distance { value, kind, .. } => {
                assert_eq!(kind, "axis_plane");
                assert!((value - 20.0).abs() < 1e-9, "expected 20.0 mm, got {value}");
            }
            other => panic!("expected Distance(axis_plane), got {other:?}"),
        }
    }

    // ── Case 5: single cylinder face → Diameter ───────────────────────────────

    #[test]
    fn single_cylinder_face_gives_diameter() {
        let mut m = BRepModel::new();
        let c = make_solid(
            TopologyBuilder::new(&mut m)
                .create_cylinder_3d(Point3::ZERO, Vector3::Z, 5.0, 15.0)
                .expect("cyl"),
        );
        let f = first_cyl_face(&m, c);
        let result = measure(&mut m, MeasureSubject::Face { solid: c, face: f }, None)
            .expect("measure succeeded");

        match result {
            MeasureResult::Diameter { value, .. } => {
                assert!((value - 10.0).abs() < 1e-9, "expected Ø10.0, got {value}");
            }
            other => panic!("expected Diameter, got {other:?}"),
        }
    }

    // ── Case 6: single plane face → FaceInfo with normal ─────────────────────

    #[test]
    fn single_plane_face_gives_face_info_with_normal() {
        let mut m = BRepModel::new();
        let b = make_solid(
            TopologyBuilder::new(&mut m)
                .create_box_3d(10.0, 10.0, 10.0)
                .expect("box"),
        );
        let top = plane_face_near(&m, b, Vector3::Z);
        let result = measure(
            &mut m,
            MeasureSubject::Face {
                solid: b,
                face: top,
            },
            None,
        )
        .expect("measure succeeded");

        match result {
            MeasureResult::FaceInfo { normal, area, .. } => {
                assert!(normal.is_some(), "plane face must carry a normal");
                // Top face of 10×10×10 box: analytic area = 100 mm².
                // The computed value may differ slightly from the trimmed-face
                // integral; allow ±5 mm² as a generous bound.
                assert!(
                    (area - 100.0).abs() < 5.0,
                    "expected area ≈ 100.0, got {area}"
                );
            }
            other => panic!("expected FaceInfo, got {other:?}"),
        }
    }

    // ── Case 2b: obtuse dihedral must NOT fold to its acute supplement ───────
    //
    // Derivation of the expected value:
    //   Box A's top face outward normal:  nA = (0, 0, 1).
    //   Box B is rotated 120° about the X axis through its centre; its top
    //   face outward normal (0,0,1) rotates to
    //     nB = (0, −sin 120°, cos 120°) = (0, −√3/2, −1/2).
    //   dot(nA, nB) = −1/2  →  acos(−1/2) = 120°.
    // The folded convention acos(|dot|) would report 60° — the wrong number.
    #[test]
    fn obtuse_dihedral_reports_full_angle() {
        use crate::operations::transform::{rotate, TransformOptions};

        let mut m = BRepModel::new();
        let a = make_solid(
            TopologyBuilder::new(&mut m)
                .create_box_3d(20.0, 20.0, 20.0)
                .expect("box A"),
        );
        let b = make_solid(
            TopologyBuilder::new(&mut m)
                .create_box_3d(20.0, 20.0, 20.0)
                .expect("box B"),
        );
        // Rotate box B 120° about the X axis through its centre (the origin —
        // create_box_3d builds centred on the origin).
        rotate(
            &mut m,
            vec![b],
            Point3::ZERO,
            Vector3::X,
            120.0_f64.to_radians(),
            TransformOptions::default(),
        )
        .expect("rotate box B");

        let top_a = plane_face_near(&m, a, Vector3::Z);
        // B's rotated top face: outward normal (0, −√3/2, −1/2).
        let rot_normal = Vector3::new(0.0, -(3.0_f64.sqrt()) / 2.0, -0.5);
        let top_b = plane_face_near(&m, b, rot_normal);

        let result = measure(
            &mut m,
            MeasureSubject::Face {
                solid: a,
                face: top_a,
            },
            Some(MeasureSubject::Face {
                solid: b,
                face: top_b,
            }),
        )
        .expect("measure succeeded");

        match result {
            MeasureResult::Angle { degrees, .. } => {
                assert!(
                    (degrees - 120.0).abs() < 1e-9,
                    "expected 120.0° (angle between outward normals), got {degrees}"
                );
            }
            other => panic!("expected Angle, got {other:?}"),
        }
    }

    // ── Case 8: sphere × plane → nearest distance, analytically exact ─────────
    //
    // Box 40×40×20 centred at origin → top face is the plane z = +10.
    // Sphere r = 6 centred at (0, 0, 20) → lowest point (0, 0, 14).
    // Nearest gap = 20 − 10 − 6 = 4.0 exactly.
    #[test]
    fn sphere_plane_nearest_exact() {
        let mut m = BRepModel::new();
        let plate = make_solid(
            TopologyBuilder::new(&mut m)
                .create_box_3d(40.0, 40.0, 20.0)
                .expect("plate"),
        );
        let ball = make_solid(
            TopologyBuilder::new(&mut m)
                .create_sphere_3d(Point3::new(0.0, 0.0, 20.0), 6.0)
                .expect("sphere"),
        );
        let sphere_f = first_sphere_face(&m, ball);
        let top_f = plane_face_near(&m, plate, Vector3::Z);

        let result = measure(
            &mut m,
            MeasureSubject::Face {
                solid: ball,
                face: sphere_f,
            },
            Some(MeasureSubject::Face {
                solid: plate,
                face: top_f,
            }),
        )
        .expect("measure succeeded");

        match result {
            MeasureResult::Distance { value, kind, .. } => {
                assert_eq!(kind, "nearest");
                assert!(
                    (value - 4.0).abs() < 1e-6,
                    "expected 4.0 mm (20 − 10 − 6), got {value}"
                );
            }
            other => panic!("expected Distance(nearest), got {other:?}"),
        }
    }

    // ── Case 8b (R-4): footpoint outside trim → REFUSE, never phantom material ─
    //
    // Construction (deterministic — a box face trim IS bounded):
    //   Plate 40×40×20 centred at origin → top face is the plane z = +10
    //   TRIMMED to x, y ∈ [−20, 20].
    //   Sphere r = 6 centred at (30, 0, 20) — laterally 10 mm PAST the
    //   plate's x = +20 edge.
    //   The alternating projection converges on the UNTRIMMED plane to the
    //   footpoint (30, 0, 10), which is outside the trim (inside = false);
    //   the sphere footpoint (30, 0, 14) is inside (full sphere).
    //   An untrimmed answer would report a 4.0 mm gap to a point where the
    //   plate does not exist; the true trimmed distance (to the plate edge
    //   at x = 20) is √(10² + 10²) − 6 ≈ 8.14 mm.  The kernel must REFUSE
    //   rather than fabricate the 4.0.
    #[test]
    fn sphere_past_plate_edge_refuses_phantom_distance() {
        let mut m = BRepModel::new();
        let plate = make_solid(
            TopologyBuilder::new(&mut m)
                .create_box_3d(40.0, 40.0, 20.0)
                .expect("plate"),
        );
        let ball = make_solid(
            TopologyBuilder::new(&mut m)
                .create_sphere_3d(Point3::new(30.0, 0.0, 20.0), 6.0)
                .expect("sphere"),
        );
        let sphere_f = first_sphere_face(&m, ball);
        let top_f = plane_face_near(&m, plate, Vector3::Z);

        let err = measure(
            &mut m,
            MeasureSubject::Face {
                solid: ball,
                face: sphere_f,
            },
            Some(MeasureSubject::Face {
                solid: plate,
                face: top_f,
            }),
        )
        .expect_err("must refuse: nearest point lies beyond the plate's trimmed boundary");

        match &err {
            MeasureError::Unsupported { reason } => {
                assert!(!reason.is_empty(), "reason must not be empty");
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    // ── Case I-2 (RED → GREEN): stale face from a consumed solid → NotFound ────
    //
    // Specimen: a 40×40×10 plate. Its top (+Z) face has id `old_top_fid`.
    // A 20×20×20 cutter centred at the origin is boolean-differenced out of
    // the plate, producing `new_solid` — the result is a notched plate whose
    // original top face was SPLIT by the cut (the old face id no longer
    // belongs to any shell of new_solid). The old top face persists in the
    // model arena (the pid lineage tests rely on arena persistence) but it
    // does NOT belong to `new_solid`. Measuring (new_solid, old_top_fid) must
    // be a `NotFound` refusal — a confident stale number is worse than none.
    //
    // RED transcript (pre-fix behaviour captured 2026-07-04):
    //   `measure(new_solid, old_top_fid)` returned
    //   `Ok(FaceInfo { area: 1600.0, normal: Some([0.0, 0.0, 1.0]),
    //   anchor: [-0.5, 0.5, 5.0] })` — the kernel measured the pre-cut face
    //   directly from the arena without verifying it belongs to new_solid,
    //   fabricating a stale area of 1600 mm² (the old full 40×40 top face)
    //   even though that face was split by the boolean and no longer exists
    //   on any shell of new_solid. This is the "stale number" the spec (§3)
    //   forbids: "REMOVED … if the faces no longer resolve — a stale number
    //   is worse than none."
    //
    // GREEN: after the membership check in `surface_classify` the same call
    //   returns `Err(MeasureError::NotFound { solid: new_solid, face: old_top_fid })`.
    #[test]
    fn stale_face_from_consumed_solid_gives_not_found() {
        use crate::operations::{boolean_operation, BooleanOp, BooleanOptions};

        let mut m = BRepModel::new();
        // Build a 40×40×10 plate centred at the origin.
        // Faces at z = +5 (top) and z = -5 (bottom), footprint ±20 in x,y.
        let plate = make_solid(
            TopologyBuilder::new(&mut m)
                .create_box_3d(40.0, 40.0, 10.0)
                .expect("plate box"),
        );
        // Capture the id of the top (+Z) face BEFORE the boolean. This face
        // will be SPLIT by the cut — the split produces new faces in new_solid
        // but the original face id is not reused in the result shell.
        let old_top_fid = plane_face_near(&m, plate, Vector3::Z);

        // A 20×20×20 cutter centred at the origin (z = -10..+10) — it
        // intersects the plate and cuts a 20×20 pocket all the way through,
        // splitting the top face into two pieces. The old top face id is not
        // present in new_solid's outer shell after the Difference.
        let cutter = make_solid(
            TopologyBuilder::new(&mut m)
                .create_box_3d(20.0, 20.0, 20.0)
                .expect("cutter box"),
        );

        // Boolean Difference: plate − cutter → new_solid.
        // Both `plate` and `cutter` are removed from model.solids; their
        // faces persist in model.faces (the arena).
        let new_solid = boolean_operation(
            &mut m,
            plate,
            cutter,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("boolean difference must succeed — plate notched by cutter");

        // Confirm old_top_fid is still in the arena (this is what makes the
        // bug possible: the face resolves by id even though new_solid never
        // owns it).
        assert!(
            m.faces.get(old_top_fid).is_some(),
            "old top face must persist in the arena after the boolean"
        );

        // The old top face must NOT belong to new_solid. Measuring
        // (new_solid, old_top_fid) must be a NotFound refusal — not a stale
        // success measured against the dead face in the arena.
        let err = measure(
            &mut m,
            MeasureSubject::Face {
                solid: new_solid,
                face: old_top_fid,
            },
            None,
        )
        .expect_err("stale face (consumed by boolean) must give NotFound, not a fabricated result");

        match &err {
            MeasureError::NotFound { solid, face } => {
                assert_eq!(*solid, new_solid, "NotFound must name the requested solid");
                assert_eq!(*face, old_top_fid, "NotFound must name the stale face");
            }
            other => panic!(
                "expected NotFound for stale face, got {other:?} — \
                 the membership check in surface_classify is missing"
            ),
        }
    }

    // ── Case 7: unsupported pair → Unsupported with non-empty reason ──────────

    #[test]
    fn skew_cylinders_give_unsupported_with_reason() {
        let mut m = BRepModel::new();
        // c1 axis along Z, c2 axis along X → perpendicular (skew) axes.
        let c1 = make_solid(
            TopologyBuilder::new(&mut m)
                .create_cylinder_3d(Point3::ZERO, Vector3::Z, 4.0, 20.0)
                .expect("cyl Z"),
        );
        let c2 = make_solid(
            TopologyBuilder::new(&mut m)
                .create_cylinder_3d(Point3::new(0.0, 10.0, 0.0), Vector3::X, 4.0, 20.0)
                .expect("cyl X"),
        );
        let f1 = first_cyl_face(&m, c1);
        let f2 = first_cyl_face(&m, c2);

        let err = measure(
            &mut m,
            MeasureSubject::Face {
                solid: c1,
                face: f1,
            },
            Some(MeasureSubject::Face {
                solid: c2,
                face: f2,
            }),
        )
        .expect_err("must be Unsupported");

        match &err {
            MeasureError::Unsupported { reason } => {
                assert!(!reason.is_empty(), "reason must not be empty");
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }
}
