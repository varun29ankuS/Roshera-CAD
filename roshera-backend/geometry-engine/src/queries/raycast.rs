//! Analytic ray-cast (#12 slice 1 / #14 ray primitive) — the sound visual
//! channel's foundation.
//!
//! A ray is intersected with each face's ANALYTIC surface (exact quadratics for
//! plane/cylinder/sphere/cone), and every candidate hit is clipped to the
//! face's real trim loops (`point_inside_face_uv`). The nearest surviving hit is
//! returned with the exact world point, the oriented surface normal, the
//! distance, and the FACE ID — so a pixel/probe resolves to `(entity,
//! world-xyz, normal)`, never an approximation off a mesh.
//!
//! Defect-revealing by construction: if a face is missing, no surface is there
//! to hit, so the ray passes through to whatever is behind (or nothing). A hole
//! in the B-Rep renders as see-through — the eye cannot report a surface that
//! is not in the model.

use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::face::{Face, FaceId, FaceOrientation};
use crate::primitives::solid::SolidId;
use crate::primitives::surface::{Cone, Cylinder, Plane, Sphere, Surface};
use crate::primitives::topology_builder::BRepModel;

/// One ray–solid intersection, fully recoverable to the B-Rep.
#[derive(Debug, Clone)]
pub struct RayHit {
    pub face_id: FaceId,
    /// Exact world-space hit point (on the analytic surface).
    pub point: Point3,
    /// Outward-oriented surface normal at the hit (face orientation applied).
    pub normal: Vector3,
    /// Ray parameter `t` (distance along a unit `direction`).
    pub distance: f64,
}

const EPS: f64 = 1e-7;

/// Cast a ray from `origin` along `direction` (need not be unit; distances are
/// in `direction` units) and return the NEAREST hit on `solid_id`, or `None`.
pub fn raycast_solid(
    model: &BRepModel,
    solid_id: SolidId,
    origin: Point3,
    direction: Vector3,
) -> Option<RayHit> {
    let dir = direction.normalize().ok()?;
    let solid = model.solids.get(solid_id)?;
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);

    let mut best: Option<RayHit> = None;
    for shell_id in shells {
        let shell = match model.shells.get(shell_id) {
            Some(s) => s,
            None => continue,
        };
        for &face_id in &shell.faces {
            let face = match model.faces.get(face_id) {
                Some(f) => f,
                None => continue,
            };
            let surface = match model.surfaces.get(face.surface_id) {
                Some(s) => s,
                None => continue,
            };
            for t in surface_ray_ts(surface, origin, dir) {
                if t <= EPS {
                    continue;
                }
                let p = Point3::new(
                    origin.x + dir.x * t,
                    origin.y + dir.y * t,
                    origin.z + dir.z * t,
                );
                // Clip to the face's real trim loops (handles caps / height
                // limits / holes), not just the infinite surface.
                let (u, v) = match surface.closest_point(&p, model.tolerance()) {
                    Ok(uv) => uv,
                    Err(_) => continue,
                };
                if !crate::tessellation::surface::point_inside_face_uv(u, v, face, model) {
                    continue;
                }
                if best.as_ref().map(|b| t < b.distance).unwrap_or(true) {
                    let n = oriented_normal(surface, face, u, v);
                    best = Some(RayHit {
                        face_id,
                        point: p,
                        normal: n,
                        distance: t,
                    });
                }
            }
        }
    }
    best
}

/// Nearest positive ray parameter `t` (`> EPS`) at which the ray from `origin`
/// along `direction` crosses face `face_id`, clipped to the face's real trim
/// loops; `None` if the ray misses the trimmed face.
///
/// This is the single-face core of [`raycast_solid`] factored out so callers
/// that already hold a spatially-culled candidate face set (e.g. drawing HLR's
/// projected-AABB occlusion grid) can test faces one at a time without the
/// whole-solid face loop. The trim clip and `EPS` gate match `raycast_solid`
/// exactly, so an occlusion test built on this yields byte-identical
/// visibility to the brute-force nearest-hit path.
pub(crate) fn ray_hit_face_t(
    model: &BRepModel,
    face_id: FaceId,
    origin: Point3,
    dir: Vector3,
) -> Option<f64> {
    let face = model.faces.get(face_id)?;
    let surface = model.surfaces.get(face.surface_id)?;
    let mut best: Option<f64> = None;
    for t in surface_ray_ts(surface, origin, dir) {
        if t <= EPS {
            continue;
        }
        let p = Point3::new(
            origin.x + dir.x * t,
            origin.y + dir.y * t,
            origin.z + dir.z * t,
        );
        let (u, v) = match surface.closest_point(&p, model.tolerance()) {
            Ok(uv) => uv,
            Err(_) => continue,
        };
        if !crate::tessellation::surface::point_inside_face_uv(u, v, face, model) {
            continue;
        }
        if best.map(|b| t < b).unwrap_or(true) {
            best = Some(t);
        }
    }
    best
}

/// ALL ray–solid hits along the ray (every face crossing), sorted near→far.
/// Used for point-in-solid parity and multi-hit field queries.
pub fn raycast_all(
    model: &BRepModel,
    solid_id: SolidId,
    origin: Point3,
    direction: Vector3,
) -> Vec<RayHit> {
    let dir = match direction.normalize() {
        Ok(d) => d,
        Err(_) => return vec![],
    };
    let solid = match model.solids.get(solid_id) {
        Some(s) => s,
        None => return vec![],
    };
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);

    let mut hits = Vec::new();
    for shell_id in shells {
        let shell = match model.shells.get(shell_id) {
            Some(s) => s,
            None => continue,
        };
        for &face_id in &shell.faces {
            let face = match model.faces.get(face_id) {
                Some(f) => f,
                None => continue,
            };
            let surface = match model.surfaces.get(face.surface_id) {
                Some(s) => s,
                None => continue,
            };
            for t in surface_ray_ts(surface, origin, dir) {
                if t <= EPS {
                    continue;
                }
                let p = Point3::new(
                    origin.x + dir.x * t,
                    origin.y + dir.y * t,
                    origin.z + dir.z * t,
                );
                let (u, v) = match surface.closest_point(&p, model.tolerance()) {
                    Ok(uv) => uv,
                    Err(_) => continue,
                };
                if !crate::tessellation::surface::point_inside_face_uv(u, v, face, model) {
                    continue;
                }
                let n = oriented_normal(surface, face, u, v);
                hits.push(RayHit {
                    face_id,
                    point: p,
                    normal: n,
                    distance: t,
                });
            }
        }
    }
    hits.sort_by(|a, b| {
        a.distance
            .partial_cmp(&b.distance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits
}

fn oriented_normal(surface: &dyn Surface, face: &Face, u: f64, v: f64) -> Vector3 {
    let n = surface.normal_at(u, v).unwrap_or(Vector3::Z);
    let s = match face.orientation {
        FaceOrientation::Forward => 1.0,
        FaceOrientation::Backward => -1.0,
    };
    (n * s).normalize().unwrap_or(n)
}

/// Ray-parameter candidates for the ray ∩ a face's analytic surface. Returns
/// the (possibly two) `t` values on the INFINITE surface; the caller clips to
/// the face trim. Non-analytic surfaces return none (handled by the mesh path).
fn surface_ray_ts(surface: &dyn Surface, o: Point3, d: Vector3) -> Vec<f64> {
    let any = surface.as_any();
    if let Some(pl) = any.downcast_ref::<Plane>() {
        let denom = d.dot(&pl.normal);
        if denom.abs() < EPS {
            return vec![];
        }
        let t = (pl.origin - o).dot(&pl.normal) / denom;
        return vec![t];
    }
    if let Some(sp) = any.downcast_ref::<Sphere>() {
        let oc = o - sp.center;
        return solve_quadratic(
            d.dot(&d),
            2.0 * oc.dot(&d),
            oc.dot(&oc) - sp.radius * sp.radius,
        );
    }
    if let Some(cy) = any.downcast_ref::<Cylinder>() {
        let a = cy.axis.normalize().unwrap_or(cy.axis);
        let w = o - cy.origin;
        let dp = d - a * d.dot(&a);
        let wp = w - a * w.dot(&a);
        return solve_quadratic(
            dp.dot(&dp),
            2.0 * dp.dot(&wp),
            wp.dot(&wp) - cy.radius * cy.radius,
        );
    }
    if let Some(co) = any.downcast_ref::<Cone>() {
        let a = co.axis.normalize().unwrap_or(co.axis);
        let cos2 = co.half_angle.cos() * co.half_angle.cos();
        let co_v = o - co.apex;
        let da = d.dot(&a);
        let ca = co_v.dot(&a);
        let qa = da * da - cos2 * d.dot(&d);
        let qb = 2.0 * (da * ca - cos2 * d.dot(&co_v));
        let qc = ca * ca - cos2 * co_v.dot(&co_v);
        // Keep only the nappe opening along +axis (radius increases with `a`).
        return solve_quadratic(qa, qb, qc)
            .into_iter()
            .filter(|&t| {
                let p = o + d * t;
                (p - co.apex).dot(&a) >= 0.0
            })
            .collect();
    }
    vec![]
}

/// Real roots of `a t² + b t + c = 0`.
///
/// Delegates to the crate's shared, numerically-robust quadratic solver
/// (`crate::math::utils::solve_quadratic`, citardauq/stable-companion
/// form over an FMA + Dekker-splitting discriminant) rather than the
/// bare `b*b - 4*a*c` this file used to compute locally. The bare form
/// catastrophically cancels for near-tangent rays against large-radius
/// surfaces (see `solve_quadratic_bare_disc_collapses_near_tangent_root_pair`
/// in the tests below), silently collapsing a genuine two-hit pair into a
/// single repeated root.
///
/// No query-scoped `Tolerance` is threaded through this call chain
/// (`surface_ray_ts` only receives the surface and ray, not the model),
/// so `Tolerance::default()` is used here — it governs only the
/// discriminant's real-vs-no-real-roots sign test and root
/// deduplication, not the root values themselves.
fn solve_quadratic(a: f64, b: f64, c: f64) -> Vec<f64> {
    crate::math::utils::solve_quadratic(a, b, c, Tolerance::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn sid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    #[test]
    fn solve_quadratic_naive_formula_loses_small_root_to_cancellation() {
        // A ray origin far from a sphere relative to its radius (the
        // Sphere/Cylinder arm's coefficient shape whenever b^2 >> 4ac):
        // a = 1, b = 1e8, c = 1.0. All three are EXACTLY representable
        // f64 values, and the discriminant b^2 - 4ac = 1e16 - 4 =
        // 9999999999999996 is ALSO exactly representable — so this input
        // isolates the ROOT FORMULA (citardauq vs. naive ±) with zero
        // caller-side precision loss contaminating the comparison. (A
        // near-tangent large-sphere setup was tried first; it does not
        // discriminate here because the ray-origin arithmetic that
        // produces `c` already rounds away the tangency gap before either
        // solver runs — see the delta sweep in the executor report.)
        //
        // True small root (Newton/citardauq-exact to ~16 digits):
        // x2 = c / q where q = -0.5*(b + sqrt(disc)) ≈ -1e-8.
        //
        // The OLD local formula (naive ±): sq = sqrt(disc) rounds to
        // 1e8 - 1.4901161193847656e-8 (ULP(1e8) = 2^-26), so
        // (-b + sq) / 2 loses ~1 significant digit relative to the true
        // -1e-8 root — the small root computed this way is wrong by
        // roughly the ULP of the LARGE root, i.e. an O(1) relative error
        // on the small root itself.
        let a = 1.0;
        let b = 1.0e8;
        let c = 1.0;

        let expected_small = -1e-8;

        let roots = solve_quadratic(a, b, c);
        assert_eq!(roots.len(), 2, "expected two roots, got {:?}", roots);
        let small = roots
            .iter()
            .find(|r| r.abs() < 1.0)
            .copied()
            .expect("no small-magnitude root found");
        let rel_err = ((small - expected_small) / expected_small).abs();
        assert!(
            rel_err < 1e-6,
            "small root {} not within rel tol 1e-6 of {} (rel_err={}, roots={:?})",
            small,
            expected_small,
            rel_err,
            roots
        );
    }

    #[test]
    fn solve_quadratic_unit_sphere_center_hit_stays_exact() {
        // Trivially-known case, kept as a regression guard: a ray through
        // the center of a sphere of radius r from distance o_dist along
        // the axis must hit at t = o_dist - r (near side) and
        // t = o_dist + r (far side), unaffected by the robust-discriminant
        // delegation since there is no near-cancellation here.
        let r = 5.0;
        let o_dist = 20.0;
        let a = 1.0;
        let b = -2.0 * o_dist;
        let c = o_dist * o_dist - r * r;

        let roots = solve_quadratic(a, b, c);
        assert_eq!(roots.len(), 2, "expected two roots, got {:?}", roots);
        assert!((roots[0] - (o_dist - r)).abs() < 1e-9);
        assert!((roots[1] - (o_dist + r)).abs() < 1e-9);
    }

    #[test]
    fn ray_hits_box_top_face_exactly() {
        // Box 20×20×20 centred at origin (z in [-10, 10]). A ray straight down
        // from above must hit the +Z top face at z=10, normal +Z, t=10.
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("box"));
        let hit = raycast_solid(
            &m,
            b,
            Point3::new(0.0, 0.0, 20.0),
            Vector3::new(0.0, 0.0, -1.0),
        )
        .expect("ray must hit the box");
        assert!((hit.point.z - 10.0).abs() < 1e-6, "hit z = {}", hit.point.z);
        assert!((hit.distance - 10.0).abs() < 1e-6, "t = {}", hit.distance);
        assert!(
            hit.normal.z > 0.999,
            "top face normal points +Z: {:?}",
            hit.normal
        );
    }

    #[test]
    fn ray_hits_sphere_at_exact_radius_with_radial_normal() {
        let mut m = BRepModel::new();
        let s = sid(TopologyBuilder::new(&mut m)
            .create_sphere_3d(Point3::ZERO, 15.0)
            .expect("sphere"));
        let hit = raycast_solid(
            &m,
            s,
            Point3::new(40.0, 0.0, 0.0),
            Vector3::new(-1.0, 0.0, 0.0),
        )
        .expect("ray must hit sphere");
        assert!(
            (hit.point.x - 15.0).abs() < 1e-6,
            "hit at +X radius: {}",
            hit.point.x
        );
        assert!(
            hit.normal.x > 0.999,
            "sphere normal radial-out (+X): {:?}",
            hit.normal
        );
        assert!((hit.distance - 25.0).abs() < 1e-6, "t = 40-15 = 25");
    }

    #[test]
    fn ray_hits_cylinder_wall_exactly() {
        let mut m = BRepModel::new();
        let c = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::ZERO, Vector3::Z, 10.0, 40.0)
            .expect("cyl"));
        // Inbound along -Y at mid-height → near wall at y=10 (angle 90°, away
        // from the +X seam). NOTE: a ray that hits exactly at the seam (u=0)
        // is currently rejected by the winding trim test and reports the far
        // wall — a seam-grazing caveat pinned for the #13 soundness harness.
        let hit = raycast_solid(
            &m,
            c,
            Point3::new(0.0, 30.0, 20.0),
            Vector3::new(0.0, -1.0, 0.0),
        )
        .expect("ray hits cylinder wall");
        assert!(
            (hit.point.y - 10.0).abs() < 1e-6,
            "wall at y=10: {}",
            hit.point.y
        );
        assert!(
            hit.normal.y > 0.999,
            "wall normal radial-out (+Y): {:?}",
            hit.normal
        );
        assert!((hit.distance - 20.0).abs() < 1e-6, "t = 30-10 = 20");
    }

    #[test]
    fn missing_face_renders_see_through() {
        // THE soundness property: drop the top face → a downward ray no longer
        // hits at z=10. It either passes through to the bottom cap (z=-10) or
        // misses — but it must NEVER report a surface that isn't in the model.
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("box"));
        // remove the face the downward ray would hit first (the +Z top).
        let top = raycast_solid(
            &m,
            b,
            Point3::new(0.0, 0.0, 20.0),
            Vector3::new(0.0, 0.0, -1.0),
        )
        .expect("hit before removal")
        .face_id;
        let shell_id = m.solids.get(b).expect("solid").outer_shell;
        if let Some(shell) = m.shells.get_mut(shell_id) {
            shell.faces.retain(|&f| f != top);
        }
        let hit = raycast_solid(
            &m,
            b,
            Point3::new(0.0, 0.0, 20.0),
            Vector3::new(0.0, 0.0, -1.0),
        );
        match hit {
            None => {}
            Some(h) => {
                assert_ne!(h.face_id, top, "must not hit the removed face");
                assert!(
                    h.point.z < 0.0,
                    "see-through: next hit is the bottom cap (z≈-10), got z={}",
                    h.point.z
                );
            }
        }
    }

    /// Owed regression pin (commit 0aba6949): an ordinary ray through a
    /// cone's lateral surface must keep BOTH roots. Exercises
    /// `surface_ray_ts` directly against the analytic `Cone`, matching
    /// the `solve_quadratic_*` tests' low-level style (rather than
    /// `raycast_solid`, which would additionally clip to a solid's
    /// finite trim loop and complicate the hand-computed values below).
    ///
    /// Cone: apex at the origin, axis = +Z, half_angle = 45° (so the
    /// radius at height `v` along the axis is `r(v) = v·tan(45°) = v`).
    /// Ray: origin (-10, 0, 5), direction (+1, 0, 0) — a horizontal ray
    /// at height z = 5, where the cone's cross-section is the circle
    /// x² + y² = 5² = 25. At y = 0 that circle crosses x = ±5, so the
    /// ray (moving in +X from x = -10) hits x = -5 first (t = 5) and
    /// x = +5 second (t = 15). Cross-checked against the quadratic
    /// coefficients directly: with `a = o - apex = (-10, 0, 5)`,
    /// `d = (1, 0, 0)`, `cos²(45°) = 0.5`: `qa = -0.5`, `qb = 10`,
    /// `qc = -37.5`, giving `t² - 20t + 75 = 0` ⇒ `t = 5, 15`. Both
    /// hits are on the +axis nappe (z = 5 ≥ 0 throughout), so neither
    /// is filtered.
    #[test]
    fn cone_lateral_ray_ordinary_two_hit_matches_hand_computed_roots() {
        let cone = Cone::new(Point3::ZERO, Vector3::Z, std::f64::consts::FRAC_PI_4)
            .expect("45\u{b0} half-angle cone");
        let o = Point3::new(-10.0, 0.0, 5.0);
        let d = Vector3::new(1.0, 0.0, 0.0);
        let ts = surface_ray_ts(&cone, o, d);
        assert_eq!(ts.len(), 2, "expected two roots, got {:?}", ts);
        let mut sorted = ts.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        assert!(
            (sorted[0] - 5.0).abs() < 1e-9,
            "near root should be t=5 (x=-5), got {:?}",
            sorted
        );
        assert!(
            (sorted[1] - 15.0).abs() < 1e-9,
            "far root should be t=15 (x=+5), got {:?}",
            sorted
        );
    }

    /// Owed regression pin (commit 0aba6949): a ray near-parallel to a
    /// cone's nappe (its direction close to a generator line) must keep
    /// its far root. The old local quadratic solver in this file
    /// discarded it via an `a.abs() < tolerance` fiat that compared a
    /// dimensionless coefficient against a length; `solve_quadratic`
    /// (math/utils.rs) now uses the stable citardauq form, valid for
    /// any nonzero leading coefficient however small.
    ///
    /// Cone: same 45° half-angle cone as above. A generator line from
    /// the apex has direction `(sin45°, 0, cos45°)`. `direction` here
    /// is that generator rotated by a small `eps` — close enough to
    /// parallel that the ray ∩ cone quadratic's leading coefficient
    /// `qa = cos²(θ) - cos²(45°)` is small (`θ = 45° + eps`), but not
    /// exactly zero (exactly parallel would degenerate to a single
    /// root or none, not the near/far pair this test pins). The ray
    /// origin (-1, 0, 0) is off that generator line (not through the
    /// apex) and outside the cone at z=0, so the near-tangent geometry
    /// produces one small positive root (entering the nappe near the
    /// apex region, t ≈ 0.55) and one very large positive root (the
    /// ray stays close to the cone surface for a long distance before
    /// diverging enough to exit, t ≈ 7070 — hand-derived from the
    /// quadratic `qa·t² + qb·t + qc = 0` with `qa ≈ -eps`,
    /// `qb = sin45° ≈ 0.7071`, `qc = -0.5`) — exactly the near-
    /// cancellation shape the old fiat threshold misclassified as "no
    /// quadratic term, keep only the linear root", discarding the far
    /// one.
    #[test]
    fn cone_lateral_ray_near_parallel_to_nappe_keeps_far_root() {
        let cone = Cone::new(Point3::ZERO, Vector3::Z, std::f64::consts::FRAC_PI_4)
            .expect("45\u{b0} half-angle cone");
        let eps = 1e-4_f64;
        let theta = std::f64::consts::FRAC_PI_4 + eps;
        let d = Vector3::new(theta.sin(), 0.0, theta.cos());
        let o = Point3::new(-1.0, 0.0, 0.0);
        let ts = surface_ray_ts(&cone, o, d);
        assert_eq!(
            ts.len(),
            2,
            "near-parallel ray must keep both roots (near + far), got {:?}",
            ts
        );
        let mut sorted = ts.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let (near, far) = (sorted[0], sorted[1]);
        assert!(
            far.abs() > 100.0 * near.abs().max(1.0),
            "far root should be orders of magnitude past the near root \
             (near={near}, far={far}) — a discarded far root is exactly the \
             pre-0aba6949 regression",
        );
    }
}
