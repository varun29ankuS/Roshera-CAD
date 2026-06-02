//! Local minimum distance (LMD) between surfaces — the core of CD-φ.4.
//!
//! An **LMD** is a footpoint pair `(pA on A, pB on B)` that is a *local minimum*
//! of the distance `‖pA − pB‖` over the two parametric domains. At such a pair
//! the connecting line `pB − pA` is **normal to both surfaces** (Crozet, *Smooth-
//! BRep CD*, Sec 1.5 / Eq 1.23 in footpoint form). This is the engine the cone
//! substrate in [`crate::queries::cd`] was built to feed: the cones *cull and
//! gate*, the LMD solver *generates* the actual footpoints, distances, and
//! contacts.
//!
//! This module is the **analytic core** (φ.4.1): the result type, the
//! canonical-kind dispatch, the metric critical-point check, the trim-domain
//! rejection, and the first closed-form pairs (Sphere×Sphere, Plane×Sphere,
//! Plane×Plane). The remaining canonical pairs (Plane×{Cyl,Cone,Torus} in φ.4.2;
//! non-plane canonical pairs in φ.4.3) extend the [`surface_lmds`] dispatch; the
//! free-form Bézier-Newton path is φ.5. Each closed-form pair returns at most
//! the LMDs Abel's bound permits (Sec 4.6.3) — the simple cases here return one.

use crate::math::vector3::{Point3, Vector3};
use crate::math::Tolerance;
use crate::primitives::face::FaceId;
use crate::primitives::surface::{Plane, Sphere, Surface, SurfaceType};
use crate::primitives::topology_builder::BRepModel;

/// Degenerate-geometry threshold (coincident points, parallel normals).
const LMD_EPS: f64 = 1e-9;

/// One local-minimum-distance footpoint pair between two surfaces A and B.
///
/// `point_a` / `point_b` are the 3D footpoints; `uv_a` / `uv_b` their parameters
/// on the respective surfaces; `normal_a` / `normal_b` the unit surface normals
/// at them (outward for the canonical analytic cases). `distance` is the
/// Euclidean gap — zero (within tolerance) means the features are in contact,
/// and a negative signed interpretation is left to the caller (the surfaces here
/// are unsigned).
#[derive(Debug, Clone, Copy)]
pub struct Lmd {
    pub uv_a: (f64, f64),
    pub point_a: Point3,
    pub normal_a: Vector3,
    pub uv_b: (f64, f64),
    pub point_b: Point3,
    pub normal_b: Vector3,
    pub distance: f64,
}

/// Closed-form LMDs between two surfaces, dispatched on their canonical kinds.
///
/// Returns an empty vector for kinds not yet wired (φ.4.2/4.3/φ.5) and for
/// degenerate configurations (concentric spheres, intersecting planes) where no
/// isolated local minimum exists.
pub fn surface_lmds(a: &dyn Surface, b: &dyn Surface, tol: Tolerance) -> Vec<Lmd> {
    use SurfaceType::{Plane as P, Sphere as S};
    match (a.surface_type(), b.surface_type()) {
        (S, S) => sphere_sphere(a, b, tol),
        (P, S) => plane_sphere(a, b, tol),
        (S, P) => plane_sphere(b, a, tol).into_iter().map(swap_lmd).collect(),
        (P, P) => plane_plane(a, b, tol),
        // φ.4.2 (Plane × {Cyl,Cone,Torus}), φ.4.3 (non-plane), φ.5 (free-form).
        _ => Vec::new(),
    }
}

/// **Critical-point check** (Crozet Eq 1.23, footpoint form). True iff the line
/// joining the two footpoints is normal to *both* surfaces — the metric
/// condition every genuine LMD satisfies. For the closed-form pairs here it
/// holds by construction; its real job is to reject saddle / non-minimal
/// footpoints returned by the φ.5 Newton solve. `angle_tol` is `1 − |cos θ|`
/// slack (e.g. `1e-9` analytic, `~6e-3` for a 4° quasi-LMD band).
pub fn is_lmd_critical_point(lmd: &Lmd, angle_tol: f64) -> bool {
    let d = lmd.point_b - lmd.point_a;
    let len = d.magnitude();
    if len < LMD_EPS {
        // Coincident footpoints (contact): the connecting direction is
        // undefined, and a touching pair is trivially a distance minimum.
        return true;
    }
    let u = d * (1.0 / len);
    (1.0 - u.dot(&lmd.normal_a).abs()) < angle_tol && (1.0 - u.dot(&lmd.normal_b).abs()) < angle_tol
}

/// Does the 3D point `p` lie inside the *trimmed* domain of `face`? Projects `p`
/// to the supporting surface's parameters and runs the face's even-odd
/// trim-loop test. This is what keeps an LMD found on an infinite analytic
/// surface from being reported when its footpoint falls outside the actual face.
pub fn footpoint_in_face(model: &BRepModel, face_id: FaceId, p: &Point3) -> bool {
    let Some(face) = model.faces.get(face_id) else {
        return false;
    };
    let Some(surface) = model.surfaces.get(face.surface_id) else {
        return false;
    };
    let Ok((u, v)) = surface.closest_point(p, model.tolerance()) else {
        return false;
    };
    crate::tessellation::surface::point_inside_face_uv(u, v, face, model)
}

/// LMDs between two **faces**: the analytic surface LMDs, kept only when both
/// footpoints lie inside their faces' trim domains. This is the trimmed,
/// model-level entry point a narrow-phase would call per feature-pair.
pub fn face_lmds(model: &BRepModel, face_a: FaceId, face_b: FaceId) -> Vec<Lmd> {
    let (Some(fa), Some(fb)) = (model.faces.get(face_a), model.faces.get(face_b)) else {
        return Vec::new();
    };
    let (Some(sa), Some(sb)) = (
        model.surfaces.get(fa.surface_id),
        model.surfaces.get(fb.surface_id),
    ) else {
        return Vec::new();
    };
    surface_lmds(sa, sb, model.tolerance())
        .into_iter()
        .filter(|lmd| {
            footpoint_in_face(model, face_a, &lmd.point_a)
                && footpoint_in_face(model, face_b, &lmd.point_b)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Closed-form canonical pairs (private)
// ---------------------------------------------------------------------------

/// Sphere × Sphere: a single LMD along the centre-to-centre line (the near
/// pair; the antipodal pair is a local *maximum*, not an LMD). Concentric
/// spheres are degenerate → no isolated LMD.
fn sphere_sphere(a: &dyn Surface, b: &dyn Surface, tol: Tolerance) -> Vec<Lmd> {
    let (Some(sa), Some(sb)) = (
        a.as_any().downcast_ref::<Sphere>(),
        b.as_any().downcast_ref::<Sphere>(),
    ) else {
        return Vec::new();
    };
    let d = sb.center - sa.center;
    let len = d.magnitude();
    if len < LMD_EPS {
        return Vec::new();
    }
    let u = d * (1.0 / len); // unit A→B
    let pa = sa.center + u * sa.radius;
    let pb = sb.center - u * sb.radius;
    finish(a, pa, b, pb, tol).into_iter().collect()
}

/// Plane × Sphere: the LMD along the plane normal through the sphere centre.
fn plane_sphere(a: &dyn Surface, b: &dyn Surface, tol: Tolerance) -> Vec<Lmd> {
    let (Some(pl), Some(sp)) = (
        a.as_any().downcast_ref::<Plane>(),
        b.as_any().downcast_ref::<Sphere>(),
    ) else {
        return Vec::new();
    };
    // Signed distance of the sphere centre from the plane, along the normal.
    let s = (sp.center - pl.origin).dot(&pl.normal);
    let sign = if s >= 0.0 { 1.0 } else { -1.0 };
    let foot_plane = sp.center - pl.normal * s; // centre projected onto the plane
    let near_sphere = sp.center - pl.normal * (sign * sp.radius); // sphere point nearest the plane
    finish(a, foot_plane, b, near_sphere, tol)
        .into_iter()
        .collect()
}

/// Plane × Plane: parallel planes have a degenerate continuum of LMDs — one
/// representative pair is returned (every aligned pair shares the gap distance).
/// Non-parallel planes intersect (distance 0 along their meeting line) → no
/// isolated LMD.
fn plane_plane(a: &dyn Surface, b: &dyn Surface, tol: Tolerance) -> Vec<Lmd> {
    let (Some(pa), Some(pb)) = (
        a.as_any().downcast_ref::<Plane>(),
        b.as_any().downcast_ref::<Plane>(),
    ) else {
        return Vec::new();
    };
    if pa.normal.dot(&pb.normal).abs() < 1.0 - LMD_EPS {
        return Vec::new(); // non-parallel → intersect
    }
    // Representative: project plane-A's origin onto plane-B along B's normal.
    let s = (pa.origin - pb.origin).dot(&pb.normal);
    let foot_b = pa.origin - pb.normal * s;
    finish(a, pa.origin, b, foot_b, tol).into_iter().collect()
}

/// Fill the parameter/normal/distance fields of an LMD from its two 3D
/// footpoints by querying each surface (closest-point for `(u,v)`, `normal_at`
/// for the unit normal). `None` if either surface cannot place the point.
fn finish(a: &dyn Surface, pa: Point3, b: &dyn Surface, pb: Point3, tol: Tolerance) -> Option<Lmd> {
    let (ua, va) = a.closest_point(&pa, tol).ok()?;
    let na = a.normal_at(ua, va).ok()?.normalize().ok()?;
    let (ub, vb) = b.closest_point(&pb, tol).ok()?;
    let nb = b.normal_at(ub, vb).ok()?.normalize().ok()?;
    Some(Lmd {
        uv_a: (ua, va),
        point_a: pa,
        normal_a: na,
        uv_b: (ub, vb),
        point_b: pb,
        normal_b: nb,
        distance: (pa - pb).magnitude(),
    })
}

/// Swap the A and B roles of an LMD (used to normalise `(Sphere, Plane)` back to
/// the caller's argument order after dispatching through `plane_sphere`).
fn swap_lmd(l: Lmd) -> Lmd {
    Lmd {
        uv_a: l.uv_b,
        point_a: l.point_b,
        normal_a: l.normal_b,
        uv_b: l.uv_a,
        point_b: l.point_a,
        normal_b: l.normal_a,
        distance: l.distance,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::topology_builder::TopologyBuilder;

    const X: Vector3 = Vector3::X;
    const Y: Vector3 = Vector3::Y;
    const Z: Vector3 = Vector3::Z;

    fn tol() -> Tolerance {
        Tolerance::default()
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-7
    }

    fn approx_pt(a: Point3, b: Point3) -> bool {
        (a - b).magnitude() < 1e-7
    }

    fn plane(origin: Point3, normal: Vector3, u_dir: Vector3) -> Plane {
        Plane::new(origin, normal, u_dir).expect("valid plane")
    }

    fn sphere(center: Point3, radius: f64) -> Sphere {
        Sphere::new(center, radius).expect("valid sphere")
    }

    // -- Sphere × Sphere ---------------------------------------------------

    #[test]
    fn sphere_sphere_single_lmd_on_centre_line() {
        let a = sphere(Vector3::new(0.0, 0.0, 0.0), 1.0);
        let b = sphere(Vector3::new(5.0, 0.0, 0.0), 1.0);
        let lmds = surface_lmds(&a, &b, tol());
        assert_eq!(lmds.len(), 1, "two separated spheres have one LMD");
        let l = lmds[0];
        assert!(approx(l.distance, 3.0), "gap = 5 - 1 - 1");
        assert!(approx_pt(l.point_a, Vector3::new(1.0, 0.0, 0.0)));
        assert!(approx_pt(l.point_b, Vector3::new(4.0, 0.0, 0.0)));
        // Outward normals point toward the other sphere along the centre line.
        assert!((l.normal_a - X).magnitude() < 1e-7);
        assert!((l.normal_b - (-X)).magnitude() < 1e-7);
        assert!(is_lmd_critical_point(&l, 1e-9));
    }

    #[test]
    fn overlapping_spheres_report_negative_gap_via_points() {
        // Centres 1 apart, radii 1 each → surfaces interpenetrate. The footpoints
        // cross over; the unsigned distance is small but the geometry is captured.
        let a = sphere(Vector3::new(0.0, 0.0, 0.0), 1.0);
        let b = sphere(Vector3::new(1.0, 0.0, 0.0), 1.0);
        let lmds = surface_lmds(&a, &b, tol());
        assert_eq!(lmds.len(), 1);
        // pa = (1,0,0), pb = (0,0,0): the points have swapped past each other,
        // the signature of penetration (centre gap 1 < r_a + r_b = 2).
        assert!(approx_pt(lmds[0].point_a, Vector3::new(1.0, 0.0, 0.0)));
        assert!(approx_pt(lmds[0].point_b, Vector3::new(0.0, 0.0, 0.0)));
    }

    #[test]
    fn concentric_spheres_are_degenerate() {
        let a = sphere(Vector3::new(0.0, 0.0, 0.0), 1.0);
        let b = sphere(Vector3::new(0.0, 0.0, 0.0), 2.0);
        assert!(surface_lmds(&a, &b, tol()).is_empty());
    }

    // -- Plane × Sphere ----------------------------------------------------

    #[test]
    fn plane_sphere_lmd_along_normal() {
        let pl = plane(Vector3::new(0.0, 0.0, 0.0), Z, X); // z = 0
        let sp = sphere(Vector3::new(0.0, 0.0, 4.0), 1.0);
        let lmds = surface_lmds(&pl, &sp, tol());
        assert_eq!(lmds.len(), 1);
        let l = lmds[0];
        assert!(approx(l.distance, 3.0), "4 - 1");
        assert!(approx_pt(l.point_a, Vector3::new(0.0, 0.0, 0.0)));
        assert!(approx_pt(l.point_b, Vector3::new(0.0, 0.0, 3.0)));
        assert!(is_lmd_critical_point(&l, 1e-9));
    }

    #[test]
    fn sphere_plane_swaps_roles_consistently() {
        let pl = plane(Vector3::new(0.0, 0.0, 0.0), Z, X);
        let sp = sphere(Vector3::new(0.0, 0.0, 4.0), 1.0);
        let lmds = surface_lmds(&sp, &pl, tol()); // sphere first
        assert_eq!(lmds.len(), 1);
        let l = lmds[0];
        // A is now the sphere, B the plane.
        assert!(approx_pt(l.point_a, Vector3::new(0.0, 0.0, 3.0)));
        assert!(approx_pt(l.point_b, Vector3::new(0.0, 0.0, 0.0)));
        assert!(approx(l.distance, 3.0));
        assert!(is_lmd_critical_point(&l, 1e-9));
    }

    // -- Plane × Plane -----------------------------------------------------

    #[test]
    fn parallel_planes_give_one_representative_lmd() {
        let a = plane(Vector3::new(0.0, 0.0, 0.0), Z, X); // z = 0
        let b = plane(Vector3::new(0.0, 0.0, 2.0), Z, X); // z = 2
        let lmds = surface_lmds(&a, &b, tol());
        assert_eq!(lmds.len(), 1);
        assert!(approx(lmds[0].distance, 2.0));
        assert!(is_lmd_critical_point(&lmds[0], 1e-9));
    }

    #[test]
    fn intersecting_planes_have_no_isolated_lmd() {
        let a = plane(Vector3::new(0.0, 0.0, 0.0), Z, X); // z = 0
        let b = plane(Vector3::new(0.0, 0.0, 0.0), X, Y); // x = 0
        assert!(surface_lmds(&a, &b, tol()).is_empty());
    }

    // -- critical-point check ---------------------------------------------

    #[test]
    fn non_normal_connection_fails_the_critical_check() {
        // Footpoints joined by +X, but A's normal is +Z — the line is not normal
        // to A, so this is not an LMD.
        let bogus = Lmd {
            uv_a: (0.0, 0.0),
            point_a: Vector3::new(0.0, 0.0, 0.0),
            normal_a: Z,
            uv_b: (0.0, 0.0),
            point_b: Vector3::new(3.0, 0.0, 0.0),
            normal_b: Z,
            distance: 3.0,
        };
        assert!(!is_lmd_critical_point(&bogus, 1e-6));
    }

    // -- unimplemented pairs return empty (no panics) ----------------------

    #[test]
    fn unwired_pair_returns_empty() {
        let pl = plane(Vector3::new(0.0, 0.0, 0.0), Z, X);
        let cyl = crate::primitives::surface::Cylinder::new(Vector3::new(0.0, 0.0, 0.0), Z, 1.0)
            .expect("valid cylinder");
        // Plane × Cylinder is φ.4.2 — not wired yet, must return empty cleanly.
        assert!(surface_lmds(&pl, &cyl, tol()).is_empty());
    }

    // -- face-level trim rejection ----------------------------------------

    /// A planar box face whose supporting-plane predicate matches.
    fn box_plane_face(model: &BRepModel, pred: impl Fn(&Plane) -> bool) -> FaceId {
        model
            .faces
            .iter()
            .find(|(_, face)| {
                model
                    .surfaces
                    .get(face.surface_id)
                    .and_then(|s| s.as_any().downcast_ref::<Plane>().map(&pred))
                    .unwrap_or(false)
            })
            .map(|(id, _)| id)
            .expect("box has a matching planar face")
    }

    fn unit_box() -> BRepModel {
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_box_3d(2.0, 2.0, 2.0)
            .expect("box");
        model
    }

    #[test]
    fn face_lmds_between_opposite_box_faces() {
        let model = unit_box(); // corners ±1
        let plus_x = box_plane_face(&model, |p| {
            p.normal.dot(&X).abs() > 0.99 && p.origin.dot(&X) > 0.5
        });
        let minus_x = box_plane_face(&model, |p| {
            p.normal.dot(&X).abs() > 0.99 && p.origin.dot(&X) < -0.5
        });
        let lmds = face_lmds(&model, plus_x, minus_x);
        assert_eq!(
            lmds.len(),
            1,
            "two parallel opposite faces → one trimmed LMD"
        );
        assert!(approx(lmds[0].distance, 2.0), "box width");
    }

    #[test]
    fn face_lmds_between_perpendicular_box_faces_is_empty() {
        let model = unit_box();
        let plus_x = box_plane_face(&model, |p| {
            p.normal.dot(&X).abs() > 0.99 && p.origin.dot(&X) > 0.5
        });
        let plus_y = box_plane_face(&model, |p| {
            p.normal.dot(&Y).abs() > 0.99 && p.origin.dot(&Y) > 0.5
        });
        // Perpendicular planes intersect → no isolated LMD even before trimming.
        assert!(face_lmds(&model, plus_x, plus_y).is_empty());
    }

    #[test]
    fn footpoint_inside_and_outside_a_box_face() {
        let model = unit_box();
        let plus_x = box_plane_face(&model, |p| {
            p.normal.dot(&X).abs() > 0.99 && p.origin.dot(&X) > 0.5
        });
        // Centre of the +X face is inside its trim domain; a point far off the
        // face (same plane, way out in +y) projects outside it.
        assert!(footpoint_in_face(
            &model,
            plus_x,
            &Vector3::new(1.0, 0.0, 0.0)
        ));
        assert!(!footpoint_in_face(
            &model,
            plus_x,
            &Vector3::new(1.0, 50.0, 0.0)
        ));
    }
}
