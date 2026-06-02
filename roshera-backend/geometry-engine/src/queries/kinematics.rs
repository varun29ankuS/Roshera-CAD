//! Contact kinematics — tracking an LMD footpoint across time (CD-φ.6.3).
//!
//! Once contact has been found, a moving mechanism does not need a cold LMD
//! solve every frame: the footpoint moves *continuously* with the bodies, so it
//! can be carried forward (Crozet, *Smooth-BRep CD*, Sec 4.5.3). This is the
//! narrow-phase analogue of the broad-phase temporal coherence in
//! [`crate::queries::bvh::BroadPhase`].
//!
//! Two pieces:
//!
//! * [`ContactTracker`] — re-seeds the LMD solve from last frame's footpoint
//!   ([`crate::queries::lmd::refine_lmd_from`]), so for small motion it converges
//!   in a few alternating projections instead of the cold multi-start, landing
//!   in the same contact basin.
//! * [`closest_point_param_shift`] — the first-order predictor: how a surface's
//!   closest-point parameters `(u, v)` drift when the target point moves by
//!   `dq`. It is exactly the contact-equation step, and it is built from the
//!   surface's **first and second fundamental forms** — the Hessian of the
//!   squared distance, `H = [[E + r·P_uu, F + r·P_uv], [·, G + r·P_vv]]`. Use it
//!   to *predict* the footpoint before refining, shrinking the iteration count
//!   further.

use crate::math::vector3::{Point3, Vector3};
use crate::math::Tolerance;
use crate::primitives::surface::Surface;
use crate::queries::lmd::{refine_lmd_from, Lmd};

/// Tracks one contact (LMD footpoint pair) across frames.
#[derive(Debug, Clone, Copy)]
pub struct ContactTracker {
    last: Lmd,
}

impl ContactTracker {
    /// Begin tracking from a contact found by the cold LMD solve.
    pub fn from_lmd(lmd: Lmd) -> Self {
        Self { last: lmd }
    }

    /// The contact as of the last successful update.
    pub fn state(&self) -> &Lmd {
        &self.last
    }

    /// Carry the contact forward into the current configuration of the two
    /// surfaces, warm-starting from last frame's footpoint. Returns the updated
    /// contact (also stored), or `None` if the projection fails. For small
    /// motion this lands in the same basin the cold solve would, in a handful of
    /// iterations rather than a full multi-start.
    pub fn track(&mut self, a: &dyn Surface, b: &dyn Surface, tol: Tolerance) -> Option<Lmd> {
        let updated = refine_lmd_from(a, b, self.last.point_a, tol)?;
        self.last = updated;
        Some(updated)
    }
}

/// First-order change in a surface's closest-point parameters `(u, v)` when the
/// target point moves by `dq`, evaluated at the current footpoint `(u, v)` whose
/// closest target is `q`.
///
/// The closest-point conditions are `(P − q)·P_u = 0` and `(P − q)·P_v = 0`.
/// Differentiating in `q` gives `H · [du; dv] = [P_u·dq; P_v·dq]`, where `H` is
/// the Hessian of `½‖P − q‖²`:
/// `H = [[E + r·P_uu, F + r·P_uv], [F + r·P_uv, G + r·P_vv]]`, with `r = P − q`,
/// the first-form coefficients `E,F,G = P_u·P_u, P_u·P_v, P_v·P_v`, and the
/// second derivatives `P_uu, P_uv, P_vv`. Returns `None` at a degenerate
/// (parabolic/umbilic-singular) footpoint where `H` is not invertible.
pub fn closest_point_param_shift(
    surface: &dyn Surface,
    u: f64,
    v: f64,
    q: Point3,
    dq: Vector3,
) -> Option<(f64, f64)> {
    let sp = surface.evaluate_full(u, v).ok()?;
    let r = sp.position - q;

    let e = sp.du.dot(&sp.du) + r.dot(&sp.duu);
    let f = sp.du.dot(&sp.dv) + r.dot(&sp.duv);
    let g = sp.dv.dot(&sp.dv) + r.dot(&sp.dvv);

    let det = e * g - f * f;
    if det.abs() < 1e-12 {
        return None;
    }

    let rhs_u = sp.du.dot(&dq);
    let rhs_v = sp.dv.dot(&dq);

    // Inverse of the 2×2 symmetric Hessian applied to the right-hand side.
    let du = (g * rhs_u - f * rhs_v) / det;
    let dv = (e * rhs_v - f * rhs_u) / det;
    Some((du, dv))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Matrix4;
    use crate::primitives::surface::Sphere;
    use crate::queries::lmd::surface_lmds;

    fn tol() -> Tolerance {
        Tolerance::default()
    }

    // -- fundamental-form predictor ----------------------------------------

    #[test]
    fn param_shift_matches_finite_difference_on_a_sphere() {
        // Closest point on a sphere to an external target; predict how the
        // footpoint parameters drift when the target moves, and check against
        // the actual re-projection to first order.
        let sphere = Sphere::new(Vector3::new(0.0, 0.0, 0.0), 2.0).expect("sphere");
        let q = Vector3::new(5.0, 1.0, 0.5); // generic direction (off seam/poles)
        let (u0, v0) = sphere.closest_point(&q, tol()).expect("closest");

        let dq = Vector3::new(0.012, -0.018, 0.009); // small target motion
        let (du, dv) = closest_point_param_shift(&sphere, u0, v0, q, dq).expect("shift");

        let (u1, v1) = sphere
            .closest_point(&(q + dq), tol())
            .expect("closest moved");
        // First-order prediction matches the true parameter change to O(|dq|²).
        assert!(
            (u0 + du - u1).abs() < 1e-3,
            "u: predicted {} vs actual {}",
            u0 + du,
            u1
        );
        assert!(
            (v0 + dv - v1).abs() < 1e-3,
            "v: predicted {} vs actual {}",
            v0 + dv,
            v1
        );
    }

    #[test]
    fn zero_target_motion_yields_zero_shift() {
        let sphere = Sphere::new(Vector3::new(1.0, 0.0, 0.0), 1.5).expect("sphere");
        let q = Vector3::new(6.0, 2.0, -1.0);
        let (u0, v0) = sphere.closest_point(&q, tol()).expect("closest");
        let (du, dv) = closest_point_param_shift(&sphere, u0, v0, q, Vector3::ZERO).expect("shift");
        assert!(du.abs() < 1e-12 && dv.abs() < 1e-12);
    }

    // -- warm-start contact tracking ---------------------------------------

    #[test]
    fn tracker_follows_a_moving_contact_and_matches_cold_solve() {
        let a = Sphere::new(Vector3::new(0.0, 0.0, 0.0), 1.0).expect("a");
        let b0 = Sphere::new(Vector3::new(5.0, 0.0, 0.0), 1.0).expect("b0");

        let lmd0 = surface_lmds(&a, &b0, tol());
        assert_eq!(lmd0.len(), 1);
        let mut tracker = ContactTracker::from_lmd(lmd0[0]);

        // Move B by a small step and carry the contact forward.
        let step = Vector3::new(0.15, 0.08, -0.05);
        let b1 = b0.transform(&Matrix4::from_translation(&step));

        let tracked = tracker.track(&a, b1.as_ref(), tol()).expect("track");
        let cold = surface_lmds(&a, b1.as_ref(), tol());
        assert_eq!(cold.len(), 1);

        // Warm-tracked contact equals the cold re-solve.
        assert!(
            (tracked.point_a - cold[0].point_a).magnitude() < 1e-6,
            "footpoint A: tracked {:?} vs cold {:?}",
            tracked.point_a,
            cold[0].point_a
        );
        assert!(
            (tracked.point_b - cold[0].point_b).magnitude() < 1e-6,
            "footpoint B drift"
        );
        assert!((tracked.distance - cold[0].distance).abs() < 1e-6);
        // Tracker state advanced.
        assert!((tracker.state().point_b - cold[0].point_b).magnitude() < 1e-6);
    }

    #[test]
    fn tracker_survives_a_sequence_of_steps() {
        // Many small steps: the contact should track the analytic sphere-sphere
        // LMD the whole way (footpoint stays on the centre line).
        let a = Sphere::new(Vector3::new(0.0, 0.0, 0.0), 1.0).expect("a");
        let mut center = Vector3::new(5.0, 0.0, 0.0);
        let b0 = Sphere::new(center, 1.0).expect("b0");
        let mut tracker = ContactTracker::from_lmd(surface_lmds(&a, &b0, tol())[0]);

        for _ in 0..8 {
            center = center + Vector3::new(0.1, 0.05, 0.02);
            let b = Sphere::new(center, 1.0).expect("b");
            let tracked = tracker.track(&a, &b, tol()).expect("track step");
            let cold = surface_lmds(&a, &b, tol());
            assert!(
                (tracked.point_a - cold[0].point_a).magnitude() < 1e-6,
                "drift accumulated over the sequence"
            );
        }
    }
}
