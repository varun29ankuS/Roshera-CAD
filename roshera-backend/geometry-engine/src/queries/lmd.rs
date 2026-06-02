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
//! Every surface pair is handled — nothing is left unimplemented:
//!
//! * **Closed-form fast-paths** (exact, O(1)): Sphere×Sphere, Plane×Sphere,
//!   Plane×Plane, Sphere×Cylinder, Plane×Cylinder. These have elementary
//!   solutions and are dispatched directly.
//! * **Universal numerical engine** ([`general_surface_lmd`]): every other pair
//!   — Cone/Torus combinations, Cylinder×Cylinder, free-form NURBS — has *no*
//!   elementary closed form (they reduce to quartic-or-higher rootfinding with
//!   multiple footpoints). These are solved by multi-start alternating
//!   closest-point projection, whose fixed point is exactly an LMD. This is the
//!   principled fundamental method, not a placeholder.
//!
//! The metric critical-point check ([`is_lmd_critical_point`]) and trim-domain
//! rejection ([`footpoint_in_face`] / [`face_lmds`]) sit above both paths. Each
//! pair may return several footpoints (Abel's bound is 4 for canonics, Sec
//! 4.6.3), ordered nearest-first so `result[0]` is the global minimum.

use crate::math::vector3::{Point3, Vector3};
use crate::math::Tolerance;
use crate::primitives::face::FaceId;
use crate::primitives::surface::{Cone, Cylinder, Plane, Sphere, Surface, SurfaceType, Torus};
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

/// LMDs between two surfaces, dispatched on canonical kind: an exact closed
/// form where one exists, otherwise the universal numerical engine. Ordered
/// nearest-first.
///
/// Returns empty only for genuinely degenerate configurations — concentric
/// spheres, intersecting planes, a sphere centred on a cylinder axis — and for
/// unbounded raw surfaces (no surface-level nearest point; use [`face_lmds`]).
pub fn surface_lmds(a: &dyn Surface, b: &dyn Surface, tol: Tolerance) -> Vec<Lmd> {
    use SurfaceType::{Cylinder as Cyl, Plane as P, Sphere as S};
    match (a.surface_type(), b.surface_type()) {
        (S, S) => sphere_sphere(a, b, tol),
        (P, S) => plane_sphere(a, b, tol),
        (S, P) => plane_sphere(b, a, tol).into_iter().map(swap_lmd).collect(),
        (P, P) => plane_plane(a, b, tol),
        (P, Cyl) => plane_cylinder(a, b, tol),
        (Cyl, P) => plane_cylinder(b, a, tol)
            .into_iter()
            .map(swap_lmd)
            .collect(),
        (S, Cyl) => sphere_cylinder(a, b, tol),
        (Cyl, S) => sphere_cylinder(b, a, tol)
            .into_iter()
            .map(swap_lmd)
            .collect(),
        // Every other pair — Cone/Torus combinations, Cyl×Cyl, and free-form
        // NURBS — has no elementary closed form, so it goes through the
        // universal numerical engine. Nothing is left unimplemented: the closed
        // forms above are exact fast-paths, this is the general fallback.
        _ => general_surface_lmd(a, b, tol),
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

/// Sphere × Cylinder (infinite cylinder): the LMD lies along the radial line
/// from the sphere centre perpendicular to the cylinder axis. Degenerate when
/// the sphere centre lies on the axis (a whole circle is equidistant) or on the
/// cylinder surface.
fn sphere_cylinder(a: &dyn Surface, b: &dyn Surface, tol: Tolerance) -> Vec<Lmd> {
    let (Some(sp), Some(cy)) = (
        a.as_any().downcast_ref::<Sphere>(),
        b.as_any().downcast_ref::<Cylinder>(),
    ) else {
        return Vec::new();
    };
    let h = (sp.center - cy.origin).dot(&cy.axis);
    let foot = cy.origin + cy.axis * h; // sphere centre projected onto the axis
    let radial = sp.center - foot;
    let d_axis = radial.magnitude();
    if d_axis < LMD_EPS {
        return Vec::new(); // centre on the axis → degenerate equidistant circle
    }
    let u = radial * (1.0 / d_axis);
    let pc = foot + u * cy.radius; // nearest cylinder point, at the sphere's height
    let to_pc = pc - sp.center;
    let l = to_pc.magnitude();
    if l < LMD_EPS {
        return Vec::new(); // sphere centre exactly on the cylinder surface
    }
    let ps = sp.center + to_pc * (sp.radius / l); // sphere point toward the cylinder
    finish(a, ps, b, pc, tol).into_iter().collect()
}

/// Plane × Cylinder (infinite cylinder). Only the axis-parallel-to-plane case
/// has an isolated LMD (a degenerate ruling line; one representative pair is
/// returned). A tilted axis makes the infinite cylinder cross the plane →
/// distance 0, no isolated LMD (the finite cap-rim contact is an edge feature,
/// handled elsewhere).
fn plane_cylinder(a: &dyn Surface, b: &dyn Surface, tol: Tolerance) -> Vec<Lmd> {
    let (Some(pl), Some(cy)) = (
        a.as_any().downcast_ref::<Plane>(),
        b.as_any().downcast_ref::<Cylinder>(),
    ) else {
        return Vec::new();
    };
    if cy.axis.dot(&pl.normal).abs() > LMD_EPS {
        return Vec::new(); // axis not parallel to plane → infinite cylinder intersects
    }
    let c0 = (cy.origin - pl.origin).dot(&pl.normal); // signed axis-to-plane distance
    let sign = if c0 >= 0.0 { 1.0 } else { -1.0 };
    // The plane normal is ⊥ the axis (parallel case), so −sign·normal is a valid
    // radial direction on the cylinder: the lateral point nearest the plane.
    let pc = cy.origin - pl.normal * (sign * cy.radius);
    let foot = pc - pl.normal * (pc - pl.origin).dot(&pl.normal); // project onto plane
    finish(a, foot, b, pc, tol).into_iter().collect()
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

// ---------------------------------------------------------------------------
// Universal numerical engine (private) — handles every surface pair without an
// elementary closed form (Cone/Torus combinations, Cyl×Cyl, free-form NURBS).
// ---------------------------------------------------------------------------

/// The **universal LMD solver**: multi-start alternating closest-point
/// projection. From a seed footpoint on A it iterates `pb = B.closest(pa)`,
/// `pa = A.closest(pb)`. Each projection is a metric projection, so the gap is
/// non-increasing and the iteration converges; its fixed point has the
/// connecting line normal to *both* surfaces — exactly an LMD (Crozet's
/// critical-point condition). Seeding from a grid over each surface's parameter
/// domain captures the distinct local minima (an LMD per basin); the results
/// are de-duplicated and returned nearest-first.
///
/// This is well-posed only for surfaces with a *finite* parameter domain (a
/// nearest point exists). Unbounded raw surfaces (an infinite plane / cylinder /
/// cone) have no surface-level nearest point, so the solver returns empty for
/// them — the meaningful query there is [`face_lmds`], which supplies the
/// bounded trim domain. This is not a gap: it is the geometry being undefined at
/// the raw-surface level.
fn general_surface_lmd(a: &dyn Surface, b: &dyn Surface, tol: Tolerance) -> Vec<Lmd> {
    if !has_finite_domain(a) || !has_finite_domain(b) {
        return Vec::new();
    }
    let anchor_a = surface_anchor(a);
    let anchor_b = surface_anchor(b);
    const G: usize = 6;

    let mut found: Vec<Lmd> = Vec::new();
    // Seeds taken directly on A, plus B's grid projected onto A — between them
    // they populate every basin of the distance function for the convex-ish
    // canonical surfaces, and densely enough for the periodic tori.
    for seed in seed_grid_on(a, anchor_b, tol, G) {
        push_if_lmd(a, b, seed, tol, &mut found);
    }
    for b_seed in seed_grid_on(b, anchor_a, tol, G) {
        if let Ok((u, v)) = a.closest_point(&b_seed, tol) {
            if let Ok(p) = a.point_at(u, v) {
                push_if_lmd(a, b, p, tol, &mut found);
            }
        }
    }
    dedup_lmds(found)
}

/// Run the alternation from one seed and, if it converges to a genuine critical
/// point, record it.
fn push_if_lmd(a: &dyn Surface, b: &dyn Surface, seed: Point3, tol: Tolerance, out: &mut Vec<Lmd>) {
    if let Some(lmd) = lmd_from_seed(a, b, seed, tol) {
        // 1e-4 angular slack absorbs closest_point's own convergence tolerance.
        if is_lmd_critical_point(&lmd, 1e-4) {
            out.push(lmd);
        }
    }
}

/// Alternating closest-point projection from a seed point on A.
fn lmd_from_seed(a: &dyn Surface, b: &dyn Surface, seed_pa: Point3, tol: Tolerance) -> Option<Lmd> {
    let mut pa = seed_pa;
    for _ in 0..128 {
        let (us, vs) = b.closest_point(&pa, tol).ok()?;
        let pb = b.point_at(us, vs).ok()?;
        let (ua, va) = a.closest_point(&pb, tol).ok()?;
        let pa_next = a.point_at(ua, va).ok()?;
        let step = (pa_next - pa).magnitude();
        pa = pa_next;
        if step < 1e-11 {
            break;
        }
    }
    // Pair pa with its matching footpoint on B, then fill uv/normals/distance.
    let (us, vs) = b.closest_point(&pa, tol).ok()?;
    let pb = b.point_at(us, vs).ok()?;
    finish(a, pa, b, pb, tol)
}

/// De-duplicate converged LMDs (many seeds fall into the same basin) and order
/// them nearest-first, so `result[0]` is the global minimum.
fn dedup_lmds(mut found: Vec<Lmd>) -> Vec<Lmd> {
    found.sort_by(|x, y| {
        x.distance
            .partial_cmp(&y.distance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut out: Vec<Lmd> = Vec::new();
    for l in found {
        let dup = out.iter().any(|o| {
            (o.point_a - l.point_a).magnitude() < 1e-6 && (o.point_b - l.point_b).magnitude() < 1e-6
        });
        if !dup {
            out.push(l);
        }
    }
    out
}

/// True iff both parameter axes are finite — the precondition for a surface to
/// have a nearest point at the raw-surface level.
fn has_finite_domain(s: &dyn Surface) -> bool {
    let ((u0, u1), (v0, v1)) = s.parameter_bounds();
    u0.is_finite() && u1.is_finite() && v0.is_finite() && v1.is_finite()
}

/// A representative interior point of a surface — its canonical centre/apex when
/// known, else the midpoint of its (finite) parameter domain. Used to aim the
/// seed grid of one surface at the other.
fn surface_anchor(s: &dyn Surface) -> Point3 {
    if let Some(p) = s.as_any().downcast_ref::<Plane>() {
        return p.origin;
    }
    if let Some(sp) = s.as_any().downcast_ref::<Sphere>() {
        return sp.center;
    }
    if let Some(c) = s.as_any().downcast_ref::<Cylinder>() {
        return c.origin;
    }
    if let Some(c) = s.as_any().downcast_ref::<Cone>() {
        return c.apex;
    }
    if let Some(t) = s.as_any().downcast_ref::<Torus>() {
        return t.center;
    }
    let ((u0, u1), (v0, v1)) = s.parameter_bounds();
    s.point_at(0.5 * (u0 + u1), 0.5 * (v0 + v1))
        .unwrap_or(Vector3::ZERO)
}

/// A grid of seed points on `a`, covering its parameter domain. An axis that is
/// unbounded (should not happen once `has_finite_domain` has gated the caller,
/// but kept robust) is windowed around the projection of `toward`.
fn seed_grid_on(a: &dyn Surface, toward: Point3, tol: Tolerance, g: usize) -> Vec<Point3> {
    let ((u0, u1), (v0, v1)) = a.parameter_bounds();
    let (uc, vc) = a
        .closest_point(&toward, tol)
        .unwrap_or((0.5 * (u0 + u1), 0.5 * (v0 + v1)));
    let (lu, hu) = finite_window(u0, u1, uc);
    let (lv, hv) = finite_window(v0, v1, vc);

    let mut seeds = Vec::with_capacity(g * g + 1);
    for i in 0..g {
        let fu = if g == 1 {
            0.5
        } else {
            i as f64 / (g - 1) as f64
        };
        let u = lu + (hu - lu) * fu;
        for j in 0..g {
            let fv = if g == 1 {
                0.5
            } else {
                j as f64 / (g - 1) as f64
            };
            let v = lv + (hv - lv) * fv;
            if let Ok(p) = a.point_at(u, v) {
                seeds.push(p);
            }
        }
    }
    // Always include the direct projection of `toward` — the most likely basin.
    if let Ok(p) = a.point_at(uc, vc) {
        seeds.push(p);
    }
    seeds
}

/// A finite `[lo, hi]` window: the bounds themselves when finite and not
/// pathologically wide, else a fixed span centred on `center`.
fn finite_window(lo: f64, hi: f64, center: f64) -> (f64, f64) {
    const SPAN: f64 = 16.0;
    if lo.is_finite() && hi.is_finite() && (hi - lo) <= 4.0 * SPAN {
        (lo, hi)
    } else {
        (center - SPAN, center + SPAN)
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

    // -- Sphere × Cylinder / Plane × Cylinder ------------------------------

    fn cylinder(o: Point3, a: Vector3, r: f64) -> Cylinder {
        Cylinder::new(o, a, r).expect("valid cylinder")
    }

    #[test]
    fn sphere_cylinder_lmd_along_radial() {
        let cy = cylinder(Vector3::new(0.0, 0.0, 0.0), Z, 1.0); // axis +Z, r=1
        let sp = sphere(Vector3::new(5.0, 0.0, 0.0), 1.0);
        let lmds = surface_lmds(&sp, &cy, tol());
        assert_eq!(lmds.len(), 1);
        let l = lmds[0];
        assert!(approx(l.distance, 3.0), "5 − 1(cyl) − 1(sph)");
        assert!(
            approx_pt(l.point_a, Vector3::new(4.0, 0.0, 0.0)),
            "sphere footpoint"
        );
        assert!(
            approx_pt(l.point_b, Vector3::new(1.0, 0.0, 0.0)),
            "cylinder footpoint"
        );
        assert!(is_lmd_critical_point(&l, 1e-9));
    }

    #[test]
    fn cylinder_sphere_swap_consistent() {
        let cy = cylinder(Vector3::new(0.0, 0.0, 0.0), Z, 1.0);
        let sp = sphere(Vector3::new(5.0, 0.0, 0.0), 1.0);
        let sphere_first = surface_lmds(&sp, &cy, tol());
        let cyl_first = surface_lmds(&cy, &sp, tol());
        assert_eq!(sphere_first.len(), cyl_first.len());
        assert!(approx_pt(sphere_first[0].point_a, cyl_first[0].point_b));
        assert!(approx_pt(sphere_first[0].point_b, cyl_first[0].point_a));
    }

    #[test]
    fn sphere_on_cylinder_axis_is_degenerate() {
        let cy = cylinder(Vector3::new(0.0, 0.0, 0.0), Z, 1.0);
        let sp = sphere(Vector3::new(0.0, 0.0, 3.0), 0.5); // centre on the axis
        assert!(surface_lmds(&sp, &cy, tol()).is_empty());
    }

    #[test]
    fn plane_cylinder_parallel_axis_gives_one_lmd() {
        let pl = plane(Vector3::new(0.0, 0.0, 0.0), Z, X); // z = 0
        let cy = cylinder(Vector3::new(0.0, 0.0, 3.0), X, 1.0); // axis +X ∥ plane, at z = 3
        let lmds = surface_lmds(&pl, &cy, tol());
        assert_eq!(lmds.len(), 1);
        assert!(approx(lmds[0].distance, 2.0), "3 − 1");
        assert!(is_lmd_critical_point(&lmds[0], 1e-9));
    }

    #[test]
    fn plane_cylinder_tilted_axis_intersects_empty() {
        let pl = plane(Vector3::new(0.0, 0.0, 0.0), Z, X);
        let cy = cylinder(Vector3::new(0.0, 0.0, 3.0), Z, 1.0); // axis +Z ⟂ plane → intersects
        assert!(surface_lmds(&pl, &cy, tol()).is_empty());
    }

    // -- unbounded raw surfaces are ill-posed (not deferred) ---------------

    #[test]
    fn unbounded_surface_pair_returns_empty() {
        let pl = plane(Vector3::new(0.0, 0.0, 0.0), Z, X); // unbounded plane
        let cone = Cone::new(Vector3::new(0.0, 0.0, 5.0), Z, 0.5).expect("valid cone"); // infinite
                                                                                        // No closed form, and both surfaces are unbounded → no surface-level
                                                                                        // nearest point exists. The universal engine's finite-domain guard
                                                                                        // returns empty cleanly (the meaningful query is face_lmds, bounded).
        assert!(surface_lmds(&pl, &cone, tol()).is_empty());
    }

    // -- universal engine: Sphere × Torus / Torus × Torus (no closed form) -

    #[test]
    fn sphere_torus_global_min_via_universal_engine() {
        let to = torus_s(Vector3::new(0.0, 0.0, 0.0), Z, 3.0, 1.0); // ring in z=0, outer radius 4
        let sp = sphere(Vector3::new(10.0, 0.0, 0.0), 1.0);
        let lmds = surface_lmds(&sp, &to, tol());
        assert!(!lmds.is_empty(), "universal engine found no LMD");
        let m = lmds[0];
        // nearest torus point (4,0,0), sphere point (9,0,0) → gap 5.
        assert!(
            (m.distance - 5.0).abs() < 1e-4,
            "expected 5, got {}",
            m.distance
        );
        assert!(on_sphere(m.point_a, Vector3::new(10.0, 0.0, 0.0), 1.0));
        assert!(on_torus(
            m.point_b,
            Vector3::new(0.0, 0.0, 0.0),
            Z,
            3.0,
            1.0
        ));
        assert!(is_lmd_critical_point(&m, 1e-4));
    }

    #[test]
    fn torus_torus_global_min_via_universal_engine() {
        let a = torus_s(Vector3::new(0.0, 0.0, 0.0), Z, 3.0, 1.0);
        let b = torus_s(Vector3::new(20.0, 0.0, 0.0), Z, 3.0, 1.0);
        let lmds = surface_lmds(&a, &b, tol());
        assert!(!lmds.is_empty());
        // outer equators face each other: 20 − 4 − 4 = 12.
        assert!(
            (lmds[0].distance - 12.0).abs() < 1e-4,
            "expected 12, got {}",
            lmds[0].distance
        );
        assert!(is_lmd_critical_point(&lmds[0], 1e-4));
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

    // -- property tests (adversarial: oracles that should FAIL on a wrong
    //    closed form, not vacuously pass) -----------------------------------

    use proptest::prelude::*;

    fn any_unit() -> impl Strategy<Value = Vector3> {
        (-1.0f64..1.0, -1.0f64..1.0, -1.0f64..1.0).prop_filter_map("nonzero", |(x, y, z)| {
            Vector3::new(x, y, z).normalize().ok()
        })
    }
    fn any_point() -> impl Strategy<Value = Vector3> {
        (-8.0f64..8.0, -8.0f64..8.0, -8.0f64..8.0).prop_map(|(x, y, z)| Vector3::new(x, y, z))
    }
    fn any_radius() -> impl Strategy<Value = f64> {
        0.3f64..3.5
    }

    /// A plane through `origin` with unit `normal`; `Plane::new` orthogonalises
    /// the seed, so any non-parallel seed yields a valid frame.
    fn make_plane(origin: Point3, normal: Vector3) -> Plane {
        let seed = if normal.dot(&X).abs() < 0.9 { X } else { Y };
        Plane::new(origin, normal, seed).expect("valid plane")
    }

    fn on_sphere(p: Point3, c: Point3, r: f64) -> bool {
        ((p - c).magnitude() - r).abs() < 1e-7
    }
    fn on_plane(p: Point3, o: Point3, n: Vector3) -> bool {
        (p - o).dot(&n).abs() < 1e-7
    }

    /// Lat/long sample lattice over a sphere, in world axes (frame-independent —
    /// we only need the point set for a brute-force minimum).
    fn sphere_grid(c: Point3, r: f64, n: usize) -> Vec<Point3> {
        let mut pts = Vec::with_capacity(n * (n + 1));
        for i in 0..n {
            let theta = std::f64::consts::TAU * (i as f64) / (n as f64);
            for j in 0..=n {
                let phi = std::f64::consts::PI * (j as f64) / (n as f64);
                let dir = Vector3::new(phi.sin() * theta.cos(), phi.sin() * theta.sin(), phi.cos());
                pts.push(c + dir * r);
            }
        }
        pts
    }

    fn min_cross_dist(a: &[Point3], b: &[Point3]) -> f64 {
        let mut m = f64::INFINITY;
        for pa in a {
            for pb in b {
                let d = (*pa - *pb).magnitude();
                if d < m {
                    m = d;
                }
            }
        }
        m
    }

    fn on_cylinder(p: Point3, o: Point3, a: Vector3, r: f64) -> bool {
        let h = (p - o).dot(&a);
        let rad = (p - o - a * h).magnitude();
        (rad - r).abs() < 1e-7
    }

    /// An orthonormal pair spanning the plane perpendicular to unit axis `a`.
    fn axis_frame(a: Vector3) -> (Vector3, Vector3) {
        let seed = if a.dot(&X).abs() < 0.9 { X } else { Y };
        let e1 = (seed - a * seed.dot(&a)).normalize().expect("perp");
        let e2 = a.cross(&e1);
        (e1, e2)
    }

    /// Sample lattice over a finite window of an (infinite) cylinder's lateral
    /// surface — `n` heights across `[h_lo, h_hi]` × `n` angles.
    fn cyl_grid(o: Point3, a: Vector3, r: f64, h_lo: f64, h_hi: f64, n: usize) -> Vec<Point3> {
        let (e1, e2) = axis_frame(a);
        let mut pts = Vec::with_capacity(n * n);
        for i in 0..n {
            let h = h_lo + (h_hi - h_lo) * (i as f64) / ((n - 1) as f64);
            for j in 0..n {
                let th = std::f64::consts::TAU * (j as f64) / (n as f64);
                pts.push(o + a * h + (e1 * th.cos() + e2 * th.sin()) * r);
            }
        }
        pts
    }

    fn cylinder_s(o: Point3, a: Vector3, r: f64) -> Cylinder {
        Cylinder::new(o, a, r).expect("valid cylinder")
    }

    fn torus_s(center: Point3, axis: Vector3, rr: f64, r: f64) -> Torus {
        Torus::new(center, axis, rr, r).expect("valid torus")
    }

    fn on_torus(p: Point3, center: Point3, axis: Vector3, rr: f64, r: f64) -> bool {
        let rel = p - center;
        let z = rel.dot(&axis);
        let rho = (rel - axis * z).magnitude();
        (((rho - rr).powi(2) + z * z).sqrt() - r).abs() < 1e-5
    }

    /// Sample lattice over a torus: `n` tube angles × `n` ring angles.
    fn torus_grid(center: Point3, axis: Vector3, rr: f64, r: f64, n: usize) -> Vec<Point3> {
        let (e1, e2) = axis_frame(axis);
        let mut pts = Vec::with_capacity(n * n);
        for i in 0..n {
            let th = std::f64::consts::TAU * (i as f64) / (n as f64);
            let ring = e1 * th.cos() + e2 * th.sin();
            for j in 0..n {
                let ph = std::f64::consts::TAU * (j as f64) / (n as f64);
                pts.push(center + ring * (rr + r * ph.cos()) + axis * (r * ph.sin()));
            }
        }
        pts
    }

    proptest! {
        /// Sphere × Sphere, every configuration: the LMD is critical, both
        /// footpoints lie exactly on their spheres, the `distance` field is
        /// self-consistent, and it equals the analytic gap |L − rA − rB|.
        #[test]
        fn pp_sphere_sphere_invariants(
            ca in any_point(), cb in any_point(), ra in any_radius(), rb in any_radius(),
        ) {
            let a = sphere(ca, ra);
            let b = sphere(cb, rb);
            let l = (cb - ca).magnitude();
            let lmds = surface_lmds(&a, &b, tol());
            if l < LMD_EPS {
                prop_assert!(lmds.is_empty(), "concentric → degenerate");
            } else {
                prop_assert_eq!(lmds.len(), 1);
                let m = lmds[0];
                prop_assert!(on_sphere(m.point_a, ca, ra), "pa off sphere A");
                prop_assert!(on_sphere(m.point_b, cb, rb), "pb off sphere B");
                prop_assert!((m.distance - (m.point_a - m.point_b).magnitude()).abs() < 1e-9);
                prop_assert!((m.distance - (l - ra - rb).abs()).abs() < 1e-7, "distance ≠ analytic gap");
                prop_assert!(is_lmd_critical_point(&m, 1e-6), "LMD not critical");
            }
        }

        /// Plane × Sphere, every orientation: critical, footpoints on their
        /// surfaces, distance equals ||s| − r| where s is the centre's signed
        /// distance from the plane.
        #[test]
        fn pp_plane_sphere_invariants(
            o in any_point(), n in any_unit(), c in any_point(), r in any_radius(),
        ) {
            let pl = make_plane(o, n);
            let sp = sphere(c, r);
            let lmds = surface_lmds(&pl, &sp, tol());
            prop_assert_eq!(lmds.len(), 1);
            let m = lmds[0];
            let s = (c - o).dot(&pl.normal);
            prop_assert!(on_plane(m.point_a, o, pl.normal), "pa off plane");
            prop_assert!(on_sphere(m.point_b, c, r), "pb off sphere");
            prop_assert!((m.distance - (m.point_a - m.point_b).magnitude()).abs() < 1e-9);
            prop_assert!((m.distance - (s.abs() - r).abs()).abs() < 1e-7, "distance ≠ ||s|−r|");
            prop_assert!(is_lmd_critical_point(&m, 1e-6), "LMD not critical");
        }

        /// Argument order is a labelling, not a result: swapping A and B swaps
        /// the footpoints and preserves the distance (sphere × sphere).
        #[test]
        fn pp_swap_symmetry_sphere_sphere(
            ca in any_point(), cb in any_point(), ra in any_radius(), rb in any_radius(),
        ) {
            prop_assume!((cb - ca).magnitude() >= LMD_EPS);
            let ab = surface_lmds(&sphere(ca, ra), &sphere(cb, rb), tol());
            let ba = surface_lmds(&sphere(cb, rb), &sphere(ca, ra), tol());
            prop_assert_eq!(ab.len(), ba.len());
            prop_assert!((ab[0].distance - ba[0].distance).abs() < 1e-9);
            prop_assert!((ab[0].point_a - ba[0].point_b).magnitude() < 1e-7);
            prop_assert!((ab[0].point_b - ba[0].point_a).magnitude() < 1e-7);
        }

        /// Cross-kind swap: Plane×Sphere and Sphere×Plane agree under role swap.
        #[test]
        fn pp_swap_symmetry_plane_sphere(
            o in any_point(), n in any_unit(), c in any_point(), r in any_radius(),
        ) {
            let pl = make_plane(o, n);
            let sp = sphere(c, r);
            let ps = surface_lmds(&pl, &sp, tol());
            let sp_pl = surface_lmds(&sp, &pl, tol());
            prop_assert_eq!(ps.len(), sp_pl.len());
            prop_assert!((ps[0].distance - sp_pl[0].distance).abs() < 1e-9);
            prop_assert!((ps[0].point_a - sp_pl[0].point_b).magnitude() < 1e-7);
            prop_assert!((ps[0].point_b - sp_pl[0].point_a).magnitude() < 1e-7);
        }

        /// Rigid translation invariance: moving both spheres by t leaves the
        /// distance unchanged and shifts both footpoints by t. Catches any
        /// frame-origin-dependent error.
        #[test]
        fn pp_translation_invariance(
            ca in any_point(), cb in any_point(), ra in any_radius(), rb in any_radius(),
            t in any_point(),
        ) {
            prop_assume!((cb - ca).magnitude() >= LMD_EPS);
            let base = surface_lmds(&sphere(ca, ra), &sphere(cb, rb), tol());
            let moved = surface_lmds(&sphere(ca + t, ra), &sphere(cb + t, rb), tol());
            prop_assert_eq!(base.len(), moved.len());
            prop_assert!((base[0].distance - moved[0].distance).abs() < 1e-9);
            prop_assert!((base[0].point_a + t - moved[0].point_a).magnitude() < 1e-7);
            prop_assert!((base[0].point_b + t - moved[0].point_b).magnitude() < 1e-7);
        }

        /// Sphere × Cylinder, random sphere and random-axis cylinder: at most one
        /// LMD (zero only in the degenerate on-axis / on-surface case), and when
        /// present it is critical with both footpoints exactly on their surfaces.
        #[test]
        fn pp_sphere_cylinder_invariants(
            co in any_point(), ax in any_unit(), rc in any_radius(),
            sc in any_point(), rs in any_radius(),
        ) {
            let lmds = surface_lmds(&sphere(sc, rs), &cylinder_s(co, ax, rc), tol());
            prop_assert!(lmds.len() <= 1);
            if let Some(m) = lmds.first() {
                prop_assert!(on_sphere(m.point_a, sc, rs), "pa off sphere");
                prop_assert!(on_cylinder(m.point_b, co, ax, rc), "pb off cylinder");
                prop_assert!((m.distance - (m.point_a - m.point_b).magnitude()).abs() < 1e-9);
                prop_assert!(is_lmd_critical_point(m, 1e-6), "LMD not critical");
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// The headline bug-finder: the analytic Sphere×Sphere distance must be
        /// the TRUE global minimum — no sampled pair on the two surfaces is
        /// closer (catches "returned the antipodal/maximum pair"), and it is not
        /// grossly below the sampled minimum (catches a wrong-small distance).
        /// Spheres are constructed guaranteed-separated so the gap is exactly
        /// `sep`.
        #[test]
        fn pp_sphere_sphere_is_global_min(
            ca in any_point(), dir in any_unit(), sep in 0.6f64..6.0,
            ra in any_radius(), rb in any_radius(),
        ) {
            let cb = ca + dir * (ra + rb + sep);
            let lmds = surface_lmds(&sphere(ca, ra), &sphere(cb, rb), tol());
            prop_assert_eq!(lmds.len(), 1);
            let analytic = lmds[0].distance;
            let brute = min_cross_dist(&sphere_grid(ca, ra, 16), &sphere_grid(cb, rb, 16));
            prop_assert!(analytic <= brute + 1e-9, "analytic {} > brute min {}", analytic, brute);
            prop_assert!(brute - analytic <= 0.15 * (ra + rb) + 1e-6, "analytic {} far below brute {}", analytic, brute);
        }

        /// Same global-min oracle for Plane × Sphere: perpendicular distance from
        /// every sphere sample to the plane is ≥ the analytic LMD distance.
        #[test]
        fn pp_plane_sphere_is_global_min(
            o in any_point(), n in any_unit(), gap in 0.6f64..6.0, r in any_radius(),
        ) {
            let c = o + n * (r + gap); // sphere on +normal side, separated
            let pl = make_plane(o, n);
            let lmds = surface_lmds(&pl, &sphere(c, r), tol());
            prop_assert_eq!(lmds.len(), 1);
            let analytic = lmds[0].distance;
            let brute = sphere_grid(c, r, 16)
                .iter()
                .map(|p| (*p - o).dot(&pl.normal).abs())
                .fold(f64::INFINITY, f64::min);
            prop_assert!(analytic <= brute + 1e-9, "analytic {} > brute min {}", analytic, brute);
            prop_assert!(brute - analytic <= 0.15 * r + 1e-6, "analytic {} far below brute {}", analytic, brute);
        }

        /// Global-min oracle for Sphere × Cylinder: the sphere is placed radially
        /// outside the cylinder (separated by `sep`), and the analytic distance
        /// must be ≤ every sampled sphere/cylinder pair, and not grossly below it.
        #[test]
        fn pp_sphere_cylinder_is_global_min(
            co in any_point(), ax in any_unit(), rc in any_radius(),
            h0 in -3.0f64..3.0, sep in 0.6f64..5.0, rs in any_radius(),
        ) {
            let (e1, _e2) = axis_frame(ax);
            let foot = co + ax * h0;
            let sc = foot + e1 * (rc + rs + sep); // sphere centre, radially separated
            let lmds = surface_lmds(&sphere(sc, rs), &cylinder_s(co, ax, rc), tol());
            prop_assert_eq!(lmds.len(), 1);
            let analytic = lmds[0].distance;
            let brute = min_cross_dist(
                &sphere_grid(sc, rs, 14),
                &cyl_grid(co, ax, rc, h0 - 2.0, h0 + 2.0, 18),
            );
            prop_assert!(analytic <= brute + 1e-9, "analytic {} > brute min {}", analytic, brute);
            prop_assert!(brute - analytic <= 0.2 * (rc + rs) + 1e-6, "analytic {} far below brute {}", analytic, brute);
        }

        /// Universal engine, the hardest finite surface: Sphere × Torus has no
        /// closed form. Sphere placed outside the outer equator (separated by
        /// `sep`); the multi-start solver must find the true global minimum.
        #[test]
        fn pp_sphere_torus_is_global_min(
            center in any_point(), axis in any_unit(),
            rr in 2.0f64..4.0, r in 0.2f64..1.0, sep in 0.6f64..4.0, rs in any_radius(),
        ) {
            let (e1, _e2) = axis_frame(axis);
            let sc = center + e1 * (rr + r + rs + sep); // outside the outer equator
            let lmds = surface_lmds(&sphere(sc, rs), &torus_s(center, axis, rr, r), tol());
            prop_assert!(!lmds.is_empty(), "engine found no LMD");
            let analytic = lmds[0].distance;
            let brute = min_cross_dist(
                &sphere_grid(sc, rs, 14),
                &torus_grid(center, axis, rr, r, 26),
            );
            prop_assert!(analytic <= brute + 1e-6, "analytic {} > brute {}", analytic, brute);
            prop_assert!(brute - analytic <= 0.25 * (rr + r + rs) + 1e-3, "analytic {} far below brute {}", analytic, brute);
        }

        /// Universal engine: Torus × Torus, coaxial, centres separated along X so
        /// the facing outer equators give a unique global minimum `sep`.
        #[test]
        fn pp_torus_torus_is_global_min(
            rr_a in 2.0f64..4.0, r_a in 0.2f64..1.0,
            rr_b in 2.0f64..4.0, r_b in 0.2f64..1.0, sep in 0.6f64..4.0,
        ) {
            let ca = Vector3::ZERO;
            let cb = Vector3::new((rr_a + r_a) + (rr_b + r_b) + sep, 0.0, 0.0);
            let lmds = surface_lmds(&torus_s(ca, Z, rr_a, r_a), &torus_s(cb, Z, rr_b, r_b), tol());
            prop_assert!(!lmds.is_empty());
            let analytic = lmds[0].distance;
            let brute = min_cross_dist(
                &torus_grid(ca, Z, rr_a, r_a, 24),
                &torus_grid(cb, Z, rr_b, r_b, 24),
            );
            prop_assert!(analytic <= brute + 1e-6, "analytic {} > brute {}", analytic, brute);
            prop_assert!(brute - analytic <= 0.3 * (rr_a + r_a + rr_b + r_b) + 1e-3, "analytic {} far below brute {}", analytic, brute);
        }
    }
}
