// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Task #55 — `KnotVector::validate` must be linear in the knot count.
//!
//! The multiplicity check used to call `multiplicity()` once per distinct knot,
//! and `multiplicity()` rescans the WHOLE vector — an O(n²) validator on a knot
//! vector that is non-decreasing, where equal values are necessarily
//! contiguous and a single run-length pass suffices. `NurbsCurve::new` runs it
//! on every construction, and the per-evaluation reconstruction of the math
//! curve put that quadratic cost on the single-point evaluation path: the
//! measured cause of the "frustum union hangs" report (160 ms per point on a
//! 5,441-control-point curve). This pins the complexity so the quadratic form
//! cannot return unnoticed.

use geometry_engine::math::bspline::KnotVector;
use std::time::Instant;

/// A valid clamped knot vector with several thousand distinct interior knots is
/// the worst case for the old validator: every distinct knot triggered a full
/// rescan. The single-pass validator finishes in well under a comfortable
/// wall-clock bound that the O(n²) form provably blows.
///
/// Sizing: at 5,441 control points the quadratic validator cost ≈ 160 ms; the
/// cost scales with n², so at ~24,000 control points it is ≈ 20× that — several
/// seconds. The linear validator does ~24,000 comparisons: sub-millisecond.
/// The 500 ms assertion sits ~1–2 orders of magnitude below the quadratic time
/// and ~2–3 orders above the linear time, so the test is a decisive separator
/// rather than a flaky timing check.
#[test]
fn knot_validator_is_linear_not_quadratic() {
    let degree = 3usize;
    let num_control_points = 24_000usize;

    // `uniform` builds a valid clamped vector: (degree+1)-fold ends plus
    // distinct interior knots — exactly the distinct-knot-dense worst case.
    let kv = KnotVector::uniform(degree, num_control_points);
    assert_eq!(
        kv.len(),
        num_control_points + degree + 1,
        "uniform knot vector has the wrong length"
    );

    let t0 = Instant::now();
    let result = kv.validate(degree, num_control_points);
    let elapsed = t0.elapsed();

    result.expect("a valid clamped uniform knot vector must validate");

    eprintln!(
        "knot_validator: n_cp={num_control_points} knots={} validate={elapsed:.3?}",
        kv.len()
    );

    assert!(
        elapsed.as_millis() < 500,
        "KnotVector::validate took {elapsed:.3?} on {} knots — the O(n²) \
         per-distinct-knot rescan has returned; the run-length pass is O(n) and \
         completes in well under 500 ms",
        kv.len()
    );
}

/// Behaviour-preservation guard for the run-length multiplicity check: an
/// interior knot whose multiplicity exceeds `degree + 1` must still be
/// rejected, exactly as the old full-rescan `multiplicity()` did. Guards the
/// new counting loop against an off-by-one that would let an over-multiple knot
/// slip through.
#[test]
fn run_length_multiplicity_rejects_over_multiple_interior_knot() {
    let degree = 3usize; // degree + 1 = 4 is the multiplicity ceiling.

    // 12 knots for 8 control points at degree 3. The interior value 0.5 appears
    // 5 times (> degree + 1 = 4), which must be rejected. Non-decreasing and
    // correct length, so the length check passes and the multiplicity check is
    // the one under test.
    let knots = vec![0.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.5, 0.5, 0.5, 1.0, 1.0, 1.0];
    let num_control_points = knots.len() - degree - 1; // 8
    let kv = KnotVector::new(knots).expect("non-decreasing knot vector");

    let result = kv.validate(degree, num_control_points);
    assert!(
        result.is_err(),
        "an interior knot of multiplicity 5 (> degree + 1 = 4) must be rejected; \
         the run-length count under-reported the multiplicity"
    );

    // A well-formed clamped vector of the same shape (0.5 with multiplicity 1)
    // must still validate — the rejection is specific to over-multiplicity, not
    // a blanket failure.
    let ok_knots = vec![
        0.0, 0.0, 0.0, 0.0, 0.2, 0.4, 0.5, 0.6, 0.8, 1.0, 1.0, 1.0, 1.0,
    ];
    let ok_cp = ok_knots.len() - degree - 1; // 9
    let ok_kv = KnotVector::new(ok_knots).expect("non-decreasing knot vector");
    ok_kv
        .validate(degree, ok_cp)
        .expect("a simple-interior clamped vector must validate");
}
