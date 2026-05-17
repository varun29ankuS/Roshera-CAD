//! F4-α.1 — analytic-when-possible blend surface carrier.
//!
//! The fillet pipeline used to dispatch on a 2×2 heuristic matrix
//! `(is_straight_edge × is_constant_radius)` computed by sampling the
//! discrete rolling-ball centres. That dispatch silently lost the
//! supporting-face information the F3-γ spine solver had already
//! computed, and it misclassified two real cases:
//!
//! * **plane / cylinder, axis ⊥ plane, constant R** — current
//!   heuristic sees curved spine + constant R → toroidal, but the
//!   torus parameters were inferred from the centre samples rather
//!   than read from the cylinder's analytic axis. Re-discovers the
//!   axis numerically every time, with predictable round-off cost.
//! * **plane / sphere, constant R** — uncovered by the heuristic;
//!   no test pins which carrier this routes to.
//!
//! F4-α.1 lifts the routing decision out of `RollingBallData`
//! sampling and into a typed [`BlendSurfaceCarrier`] derived from
//! the [`SpineRail`]'s `solver_kind` and the constancy of its
//! per-station radii. The dispatch table is the algorithm — the
//! 5 × 2 mapping below is pinned by `tests/fillet_analytic_surface_contract.rs`
//! so a regression that silently reroutes a known-analytic case
//! onto `GeneralNurbs` is surfaced immediately.
//!
//! # Dispatch table
//!
//! |   `SolverKind`           | Constant radius | Variable radius |
//! |--------------------------|-----------------|-----------------|
//! | `AnalyticPlanePlane`     | `Cylindrical`   | `GeneralNurbs`  |
//! | `AnalyticPlaneCylinder`  | `Toroidal`      | `GeneralNurbs`  |
//! | `AnalyticPlaneSphere`    | `Toroidal`      | `GeneralNurbs`  |
//! | `AnalyticCylCylCoaxial`  | `Cylindrical`   | `GeneralNurbs`  |
//! | `Marched { .. }`         | `GeneralNurbs`  | `GeneralNurbs`  |
//!
//! # What this slice does *not* do
//!
//! F4-α.1 only lands the routing layer. The existing
//! `create_cylindrical_fillet_surface` / `create_toroidal_fillet_surface`
//! / `create_nurbs_fillet_surface` constructors continue to read
//! sampled centre/contact points from `RollingBallData`. The
//! algorithm-quality upgrade — making the analytic constructors read
//! the supporting face's analytic axis directly from the model and
//! deriving carrier parameters in closed form rather than from
//! samples — is F4-α.2 and is scoped separately.
//!
//! # References
//!
//! The dispatch shape follows Choi & Ju (1989), "Constant-radius
//! blending in surface modelling", CAD 21(4); the case coverage was
//! cross-checked against the structure of OpenCASCADE's
//! `ChFiKPart_ComputeData` (read for case analysis, no code lifted).
//! Patrikalakis & Maekawa §7 provides the underlying rolling-ball
//! geometry.

use crate::math::Tolerance;
use crate::operations::spine_solver::{SolverKind, SpineRail, SpineRailSample};

/// Which analytic surface shape the rolling-ball blend collapses to.
///
/// The variants pin the **type** of surface the constructor emits;
/// they do not pin the construction algorithm itself. Two of the
/// four analytic [`SolverKind`] cases collapse to `Cylindrical` and
/// two to `Toroidal`, so the carrier cardinality is smaller than
/// the solver cardinality. Tests that need to assert the underlying
/// solver kind separately should read `SpineRail::solver_kind` —
/// `BlendSurfaceCarrier` is the *surface-emission contract*, not a
/// shadow of the solver enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendSurfaceCarrier {
    /// Cylindrical patch. Emitted when the spine is a straight line
    /// and the rolling-ball radius is constant along it:
    /// `AnalyticPlanePlane + constant` and `AnalyticCylCylCoaxial + constant`.
    Cylindrical,
    /// Toroidal patch. Emitted when the spine is a planar circular
    /// arc and the rolling-ball radius is constant along it:
    /// `AnalyticPlaneCylinder + constant` and `AnalyticPlaneSphere + constant`.
    Toroidal,
    /// Swept-NURBS / variable-radius NURBS patch. Emitted when
    /// either the spine is a general curve (`Marched`) or the
    /// radius varies along the spine. No closed-form analytic
    /// surface fits the locus, so a `VariableRadiusFillet` /
    /// fitted NURBS is the carrier of last resort.
    GeneralNurbs,
}

impl BlendSurfaceCarrier {
    /// Derive the carrier from a [`SpineRail`].
    ///
    /// Radius constancy is judged across `SpineRail::samples` with
    /// the supplied tolerance. The radius arm is intentionally
    /// permissive — any two samples differing by `>tolerance` route
    /// the whole rail to `GeneralNurbs`, even on an analytic solver
    /// kind. This matches the surface-construction reality: an
    /// analytic [`SolverKind`] with a varying radius profile no
    /// longer has a closed-form carrier (a cylinder with varying
    /// radius is a cone of revolution only for *linear* variation
    /// along a plane/plane spine, and only when the dr/ds ratio
    /// equals a specific function of the dihedral half-angle — a
    /// special case not worth a fourth carrier variant in F4-α).
    pub fn from_spine_rail(rail: &SpineRail, tolerance: &Tolerance) -> Self {
        let radius_is_constant = is_radius_constant(&rail.samples, tolerance);
        Self::dispatch(rail.solver_kind, radius_is_constant)
    }

    /// Pure dispatch table — exposed so the 5×2 contract can be
    /// exhaustively unit-tested without needing to build a real
    /// [`SpineRail`] (which requires constructed curve trait
    /// objects). The body is the canonical statement of the F4-α.1
    /// routing policy; [`from_spine_rail`] is a thin wrapper that
    /// only adds the per-rail radius-constancy check.
    pub fn dispatch(solver_kind: SolverKind, radius_is_constant: bool) -> Self {
        match (solver_kind, radius_is_constant) {
            (SolverKind::AnalyticPlanePlane, true) => Self::Cylindrical,
            (SolverKind::AnalyticCylCylCoaxial, true) => Self::Cylindrical,
            (SolverKind::AnalyticPlaneCylinder, true) => Self::Toroidal,
            (SolverKind::AnalyticPlaneSphere, true) => Self::Toroidal,
            (SolverKind::Marched { .. }, _) => Self::GeneralNurbs,
            (_, false) => Self::GeneralNurbs,
        }
    }
}

/// `true` iff every `SpineRailSample.radius` agrees within
/// `tolerance.distance()` of the first sample's radius.
///
/// Empty / single-sample rails are trivially constant — the caller's
/// invariant is that `samples` is non-empty (see
/// [`SpineRail::samples`] doc); single-element vectors flow through
/// this helper untouched.
fn is_radius_constant(samples: &[SpineRailSample], tolerance: &Tolerance) -> bool {
    let Some(first) = samples.first() else {
        return true;
    };
    let tol = tolerance.distance();
    let r0 = first.radius;
    samples.iter().all(|s| (s.radius - r0).abs() <= tol)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Point3;
    use crate::operations::spine_solver::SpineRailSample;

    fn sample(t: f64, radius: f64) -> SpineRailSample {
        SpineRailSample {
            edge_parameter: t,
            arc_length: 0.0,
            center: Point3::ORIGIN,
            contact_a: Point3::ORIGIN,
            contact_b: Point3::ORIGIN,
            radius,
        }
    }

    #[test]
    fn radius_constant_within_default_tolerance() {
        let tol = Tolerance::default();
        let samples = vec![sample(0.0, 0.5), sample(0.5, 0.5), sample(1.0, 0.5)];
        assert!(is_radius_constant(&samples, &tol));
    }

    #[test]
    fn radius_constant_under_tiny_jitter() {
        let tol = Tolerance::default();
        let r = 0.5;
        let samples = vec![
            sample(0.0, r),
            sample(0.5, r + tol.distance() * 0.5),
            sample(1.0, r - tol.distance() * 0.5),
        ];
        assert!(is_radius_constant(&samples, &tol));
    }

    #[test]
    fn radius_variable_above_tolerance() {
        let tol = Tolerance::default();
        let samples = vec![sample(0.0, 0.5), sample(0.5, 0.6), sample(1.0, 0.5)];
        assert!(!is_radius_constant(&samples, &tol));
    }

    #[test]
    fn empty_radius_is_trivially_constant() {
        let tol = Tolerance::default();
        assert!(is_radius_constant(&[], &tol));
    }

    #[test]
    fn single_sample_is_trivially_constant() {
        let tol = Tolerance::default();
        assert!(is_radius_constant(&[sample(0.5, 1.7)], &tol));
    }

    // --- Dispatch-table coverage (pure, no SpineRail needed) ---
    //
    // 5 `SolverKind` × 2 radius-constancy = 10 cells. Every cell is
    // pinned by an assertion below. A change to the dispatch table
    // requires updating both `BlendSurfaceCarrier::dispatch` and
    // this test in lock-step — exactly the audit point F4-α.1 is
    // here to defend.

    fn marched() -> SolverKind {
        SolverKind::Marched {
            predictor_steps: 4,
            corrector_iters: 2,
        }
    }

    #[test]
    fn dispatch_plane_plane_constant_is_cylindrical() {
        assert_eq!(
            BlendSurfaceCarrier::dispatch(SolverKind::AnalyticPlanePlane, true),
            BlendSurfaceCarrier::Cylindrical
        );
    }

    #[test]
    fn dispatch_plane_plane_variable_is_general_nurbs() {
        assert_eq!(
            BlendSurfaceCarrier::dispatch(SolverKind::AnalyticPlanePlane, false),
            BlendSurfaceCarrier::GeneralNurbs
        );
    }

    #[test]
    fn dispatch_plane_cylinder_constant_is_toroidal() {
        assert_eq!(
            BlendSurfaceCarrier::dispatch(SolverKind::AnalyticPlaneCylinder, true),
            BlendSurfaceCarrier::Toroidal
        );
    }

    #[test]
    fn dispatch_plane_cylinder_variable_is_general_nurbs() {
        assert_eq!(
            BlendSurfaceCarrier::dispatch(SolverKind::AnalyticPlaneCylinder, false),
            BlendSurfaceCarrier::GeneralNurbs
        );
    }

    #[test]
    fn dispatch_plane_sphere_constant_is_toroidal() {
        assert_eq!(
            BlendSurfaceCarrier::dispatch(SolverKind::AnalyticPlaneSphere, true),
            BlendSurfaceCarrier::Toroidal
        );
    }

    #[test]
    fn dispatch_plane_sphere_variable_is_general_nurbs() {
        assert_eq!(
            BlendSurfaceCarrier::dispatch(SolverKind::AnalyticPlaneSphere, false),
            BlendSurfaceCarrier::GeneralNurbs
        );
    }

    #[test]
    fn dispatch_cyl_cyl_coaxial_constant_is_cylindrical() {
        assert_eq!(
            BlendSurfaceCarrier::dispatch(SolverKind::AnalyticCylCylCoaxial, true),
            BlendSurfaceCarrier::Cylindrical
        );
    }

    #[test]
    fn dispatch_cyl_cyl_coaxial_variable_is_general_nurbs() {
        assert_eq!(
            BlendSurfaceCarrier::dispatch(SolverKind::AnalyticCylCylCoaxial, false),
            BlendSurfaceCarrier::GeneralNurbs
        );
    }

    #[test]
    fn dispatch_marched_constant_is_general_nurbs() {
        assert_eq!(
            BlendSurfaceCarrier::dispatch(marched(), true),
            BlendSurfaceCarrier::GeneralNurbs
        );
    }

    #[test]
    fn dispatch_marched_variable_is_general_nurbs() {
        assert_eq!(
            BlendSurfaceCarrier::dispatch(marched(), false),
            BlendSurfaceCarrier::GeneralNurbs
        );
    }
}
