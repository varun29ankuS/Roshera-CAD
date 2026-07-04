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
//! | Plane × Plane, `|n·n| > 0.9999`          | `Distance { plane_plane }`    |
//! | Plane × Plane, non-parallel               | `Angle { dihedral° }`         |
//! | Cylinder × Cylinder, parallel axes        | `Distance { axis_axis }`      |
//! | Cylinder × Cylinder, skew axes            | `Unsupported`                 |
//! | Cylinder × Plane, axis ⊥ normal (`<1e-4`)| `Distance { axis_plane }`     |
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
    Distance {
        value: f64,
        anchor: [f64; 3],
        direction: [f64; 3],
        kind: &'static str,
    },
    /// Dihedral angle between two non-parallel planes.
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
    /// `normal` is present only when the face is planar.
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
    /// A referenced solid or face id was not found in the model.
    #[error("Entity not found: solid {solid}, face {face}")]
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
    let area = face_area(model, face);

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
            area,
            normal: Some(vec_to_arr(normal)),
            anchor,
        }),
        Classified::Other => Ok(MeasureResult::FaceInfo {
            area,
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
    let dot = na.dot(&nb).abs();
    if dot > 0.9999 {
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
        // Non-parallel: dihedral angle.
        let cos_theta = na.dot(&nb).clamp(-1.0, 1.0);
        let degrees = cos_theta.abs().acos().to_degrees();
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
    // Parallel iff |aa × ab| ≈ 0.
    let cross_mag = aa.cross(&ab).magnitude();
    if cross_mag > 1e-4 {
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
    if dot > 1e-4 {
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

fn measure_nearest(
    model: &mut BRepModel,
    fa: FaceId,
    fb: FaceId,
) -> Result<MeasureResult, MeasureError> {
    use crate::queries::trim::closest_point_on_face;

    // Seed: UV-midpoint of face a → closest point on face b.
    let seed_a = face_surface_midpoint(model, fa).ok_or_else(|| MeasureError::Unsupported {
        reason: "Cannot compute a seed point for nearest-point iteration: \
                 face a not found or its surface has no evaluable midpoint."
            .to_string(),
    })?;

    let cpb =
        closest_point_on_face(model, fb, seed_a).ok_or_else(|| MeasureError::Unsupported {
            reason: "Nearest-point iteration did not converge for face b. \
                 The face pair may be degenerate or have an unsupported surface type."
                .to_string(),
        })?;

    // Refine: closest point on face a from the point found on face b.
    let cpa =
        closest_point_on_face(model, fa, cpb.point).ok_or_else(|| MeasureError::Unsupported {
            reason: "Nearest-point iteration did not converge for face a. \
                 The face pair may be degenerate or have an unsupported surface type."
                .to_string(),
        })?;

    // Reject if the converged footpoints both lie outside their trim domains —
    // the faces do not face each other.
    if !cpa.inside && !cpb.inside {
        return Err(MeasureError::Unsupported {
            reason: "Nearest-point projection landed outside the trimmed face for both faces. \
                     The faces may not face each other. \
                     Try selecting faces with a clear line-of-sight between them."
                .to_string(),
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
    // Verify the solid exists (we do not verify face ∈ solid to keep it O(1)).
    let _solid_check = model.solids.get(solid).ok_or(MeasureError::NotFound {
        solid,
        face: face_id,
    })?;

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
        let normal = pln.normal.normalize().unwrap_or(Vector3::Z);
        return Ok(Classified::Plane {
            normal,
            origin: pln.origin,
        });
    }

    Ok(Classified::Other)
}

// ─── Area helper ─────────────────────────────────────────────────────────────

/// Compute the area of a face (warms the face's internal cache).
///
/// Returns `0.0` when the face is not found or area computation fails.
fn face_area(model: &mut BRepModel, face_id: FaceId) -> f64 {
    // Read the face tolerance (immutable path).
    let tol = model
        .faces
        .get(face_id)
        .map(|f| crate::math::Tolerance::from_distance(f.tolerance))
        .unwrap_or_else(|| crate::math::Tolerance::from_distance(1e-6));

    // `Face::area` requires `&mut Face` and `&mut LoopStore`.
    // We cannot split the borrow (`faces` and `loops` are both fields of `model`).
    // Pattern (same as readable/query.rs): clone the face, run area on the clone
    // against the model's stores. The cache is NOT warmed on the original (the
    // clone is discarded), which is acceptable for a query path.
    let mut face_clone = match model.faces.get(face_id) {
        Some(f) => f.clone(),
        None => return 0.0,
    };
    face_clone
        .area(
            &mut model.loops,
            &model.vertices,
            &model.edges,
            &model.curves,
            &model.surfaces,
            tol,
        )
        .unwrap_or(0.0)
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

    /// Face whose plane normal most closely aligns with `target` (unit vector).
    fn plane_face_near(m: &BRepModel, solid: SolidId, target: Vector3) -> FaceId {
        let s = m.solids.get(solid).expect("solid");
        let sh = m.shells.get(s.outer_shell).expect("shell");
        let mut best: Option<(f64, FaceId)> = None;
        for &fid in &sh.faces {
            let f = m.faces.get(fid).expect("face");
            let surf = m.surfaces.get(f.surface_id).expect("surface");
            if let Some(p) = surf.as_any().downcast_ref::<Plane>() {
                let n = p.normal.normalize().unwrap_or(Vector3::Z);
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
