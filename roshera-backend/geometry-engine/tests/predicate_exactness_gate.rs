//! HARSH GATE: an exact predicate's adaptive sign MUST match an exact rational
//! oracle on adversarial, near-degenerate inputs.
//!
//! The oracle converts each `f64` coordinate to its exact dyadic rational
//! (`BigRational::from_float` — lossless, since every finite `f64` is `m·2^e`)
//! and computes the determinant in arbitrary precision: ground truth. A
//! predicate that disagrees is, by definition, NOT exact. The sweep additionally
//! asserts it actually reaches the regime where naive `f64` flips sign — a test
//! that never stresses the predicate proves nothing.
//!
//! This is the proof-of-exactness the predicates have never had. It is allowed to
//! FAIL on a non-exact predicate — that is the gap, to be filled (never pinned).

use geometry_engine::math::vector2::Vector2;
use geometry_engine::math::{orient2d, Orientation};
use num_rational::BigRational;
use num_traits::Signed;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

fn rat(x: f64) -> BigRational {
    BigRational::from_float(x).expect("finite f64 has an exact rational")
}

/// Exact orientation sign via arbitrary-precision rationals (ground truth).
fn exact_sign(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> i32 {
    let det = (rat(a[0]) - rat(c[0])) * (rat(b[1]) - rat(c[1]))
        - (rat(a[1]) - rat(c[1])) * (rat(b[0]) - rat(c[0]));
    if det.is_positive() {
        1
    } else if det.is_negative() {
        -1
    } else {
        0
    }
}

fn pred_sign(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> i32 {
    match orient2d(
        &Vector2::new(a[0], a[1]),
        &Vector2::new(b[0], b[1]),
        &Vector2::new(c[0], c[1]),
    ) {
        Orientation::CounterClockwise => 1,
        Orientation::Clockwise => -1,
        Orientation::Collinear => 0,
    }
}

fn naive_sign(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> i32 {
    let det = (a[0] - c[0]) * (b[1] - c[1]) - (a[1] - c[1]) * (b[0] - c[0]);
    if det > 0.0 {
        1
    } else if det < 0.0 {
        -1
    } else {
        0
    }
}

#[test]
fn orient2d_matches_exact_rational_oracle_on_adversarial_inputs() {
    let mut rng = StdRng::seed_from_u64(0x00C0_FFEE_D00D_1234);
    let mut mismatches = 0u64;
    let mut hard = 0u64; // naive f64 disagrees with the exact sign
    let mut first_fail: Option<([f64; 2], [f64; 2], [f64; 2], i32, i32)> = None;
    let n = 300_000u64;

    for _ in 0..n {
        // General a, b; place c NEAR the line a->b with a tiny perpendicular
        // nudge so the determinant is small and the sign is delicate.
        let a = [rng.gen_range(-1.0..1.0), rng.gen_range(-1.0..1.0)];
        let b = [rng.gen_range(-1.0..1.0), rng.gen_range(-1.0..1.0)];
        let t: f64 = rng.gen_range(-2.0..3.0);
        let eps: f64 = rng.gen_range(-1.0..1.0) * 1e-13;
        let dx = b[0] - a[0];
        let dy = b[1] - a[1];
        let c = [a[0] + t * dx - eps * dy, a[1] + t * dy + eps * dx];

        let exact = exact_sign(a, b, c);
        let got = pred_sign(a, b, c);
        if got != exact {
            mismatches += 1;
            if first_fail.is_none() {
                first_fail = Some((a, b, c, got, exact));
            }
        }
        if naive_sign(a, b, c) != exact {
            hard += 1;
        }
    }

    assert!(
        hard > 0,
        "the sweep never reached the hard regime (naive f64 always matched exact) — make c closer to the line"
    );
    assert_eq!(
        mismatches, 0,
        "orient2d disagreed with the EXACT rational oracle on {mismatches}/{n} cases \
         (hard cases where naive f64 was wrong: {hard}); first failure: {first_fail:?} \
         — the predicate is NOT exact"
    );
}
