// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! BUG 1b-5 (R2) — degeneracy-robust `normal_at` / `closest_point` on a
//! NURBS patch with a COLLAPSED control row (a parametric pole).
//!
//! The mixed-corner cap (`operations/fillet.rs`) is a rational bi-quadratic
//! whose `u = u_max` row collapses to a single apex point. On that row the
//! `v`-derivative is identically zero, so the naive surface normal
//! `du × dv` is the zero vector: pre-fix `GeneralNurbsSurface::normal_at`
//! (the default `evaluate_full().normal`) returns `Err(DivisionByZero)`, and
//! `closest_point` — which called `evaluate_full` inside its Newton loop —
//! aborted with the same error whenever the footpoint reached the apex row.
//! The tessellation-soundness oracle reads every facet's winding against
//! `closest_point → normal_at`, so those failures HID the apex-adjacent
//! facets (an unresolvable normal is treated as "agreeing") or read a
//! spurious ~114° normal on the few that resolved just off the pole.
//!
//! These fixtures pin the robust behaviour on a minimal collapsed-apex patch
//! of the same class: a well-defined LIMITING normal at the pole (never an
//! error), an unchanged normal away from the pole, and an oracle composite
//! (`closest_point` then `normal_at`) that succeeds in the apex region.

use geometry_engine::math::nurbs::NurbsSurface;
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::primitives::surface::{GeneralNurbsSurface, Surface};

/// A rational bi-quadratic patch whose `u = 1` row collapses to a single
/// apex point — the same degenerate class as the mixed-corner cap. Curved in
/// both directions so the limiting normal at the pole is a genuine tangent
/// plane, not an artefact of a flat patch.
fn collapsed_apex_patch() -> GeneralNurbsSurface {
    let apex = Point3::new(0.0, 1.0, 1.4);
    let control_points = vec![
        vec![
            Point3::new(-1.0, -1.0, 0.0),
            Point3::new(0.0, -1.0, 0.3),
            Point3::new(1.0, -1.0, 0.0),
        ],
        vec![
            Point3::new(-1.0, 0.0, 0.6),
            Point3::new(0.0, 0.0, 1.0),
            Point3::new(1.0, 0.0, 0.6),
        ],
        vec![apex, apex, apex], // collapsed apex row (u = 1): dv ≡ 0 here
    ];
    let weights = vec![vec![1.0; 3]; 3];
    let knots_u = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    let knots_v = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    let nurbs = NurbsSurface::new(control_points, weights, knots_u, knots_v, 2, 2)
        .expect("collapsed-apex bi-quadratic patch is a valid NURBS surface");
    GeneralNurbsSurface { nurbs }
}

/// The apex is at u = 1; `normal_at` there must return the LIMITING normal,
/// never an error. Pre-fix (default `evaluate_full().normal`) `du × dv = 0`
/// on the collapsed row ⇒ `Err(DivisionByZero)`.
#[test]
fn normal_at_collapsed_apex_row_returns_limiting_normal() {
    let s = collapsed_apex_patch();

    let n_apex = s.normal_at(1.0, 0.5);
    assert!(
        n_apex.is_ok(),
        "normal_at on the collapsed apex row must return the limiting normal, \
         not Err (pre-fix du×dv = 0 ⇒ DivisionByZero); got {n_apex:?}"
    );
    let n_apex = n_apex.expect("apex normal ok");

    // Continuity: the pole normal must match the normal a hair inside the
    // domain (the surface normal is continuous up to the pole).
    let n_near = s
        .normal_at(1.0 - 1e-3, 0.5)
        .expect("normal just inside the pole is well-conditioned");
    let deg = n_apex.dot(&n_near).clamp(-1.0, 1.0).acos().to_degrees();
    assert!(
        deg < 2.0,
        "apex limiting normal must agree with the just-inside normal (<2°); \
         got {deg:.3}° (apex={n_apex:?} near={n_near:?})"
    );

    // And it must be a genuine unit vector.
    assert!(
        (n_apex.magnitude() - 1.0).abs() < 1e-9,
        "limiting normal must be unit length; got |n|={}",
        n_apex.magnitude()
    );
}

/// Regression guard: away from the pole, `normal_at` is byte-for-byte the old
/// `du × dv` normal — the robust path must not perturb well-conditioned
/// queries (the overwhelming majority).
#[test]
fn normal_at_interior_matches_du_cross_dv() {
    let s = collapsed_apex_patch();
    for &(u, v) in &[(0.25, 0.25), (0.5, 0.5), (0.3, 0.8), (0.75, 0.4)] {
        let e = s.nurbs.evaluate_derivatives(u, v, 1, 1);
        let du = e.du.expect("du");
        let dv = e.dv.expect("dv");
        let expected = du.cross(&dv).normalize().expect("well-conditioned");
        let got = s.normal_at(u, v).expect("interior normal ok");
        let d = (got - expected).magnitude();
        assert!(
            d < 1e-12,
            "interior normal_at({u},{v}) must equal du×dv normalised; delta={d:.3e}"
        );
    }
}

/// The soundness-oracle composite: for a point in the apex region,
/// `closest_point` must succeed (pre-fix its Newton loop hit `evaluate_full`
/// on the collapsed row and returned `Err`), and the subsequent `normal_at`
/// at that footpoint must also succeed. This is exactly what
/// `harness::watertight::analytic_facet_*` runs per facet.
#[test]
fn closest_point_then_normal_at_survives_apex_region() {
    let s = collapsed_apex_patch();
    let apex = Point3::new(0.0, 1.0, 1.4);
    let tol = Tolerance::default();

    // A query point straight "above" the apex — the apex is the topmost point
    // of the patch, so its true footpoint IS the apex (u = 1), forcing the
    // Newton search onto the collapsed row.
    let probe = apex + Vector3::new(0.0, 0.0, 0.6);

    let foot = s.closest_point(&probe, tol);
    assert!(
        foot.is_ok(),
        "closest_point in the apex region must not error on the collapsed row \
         (pre-fix evaluate_full in the Newton loop ⇒ Err); got {foot:?}"
    );
    let (u, v) = foot.expect("foot ok");

    let n = s.normal_at(u, v);
    assert!(
        n.is_ok(),
        "normal_at at the apex-region footpoint must resolve; got {n:?}"
    );

    // The footpoint must actually be near the apex (u close to 1), i.e. the
    // search converged to the pole rather than bailing early elsewhere.
    let foot_pos = s.point_at(u, v).expect("foot position");
    assert!(
        (foot_pos - apex).magnitude() < 0.35,
        "apex-region footpoint must land near the apex; got {foot_pos:?} (u={u:.4})"
    );
}
