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
use geometry_engine::math::{incircle, orient2d, orient3d, CircleLocation, Orientation, Point3};
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

// ── orient3d (3D) ──────────────────────────────────────────────────────────

/// Exact 3D orientation sign via rationals. Matches orient3d's convention:
/// it returns the sign of `-(adx·(bdy·cdz - bdz·cdy) + ...)`.
fn exact_sign3(a: [f64; 3], b: [f64; 3], c: [f64; 3], d: [f64; 3]) -> i32 {
    let adx = rat(a[0]) - rat(d[0]);
    let ady = rat(a[1]) - rat(d[1]);
    let adz = rat(a[2]) - rat(d[2]);
    let bdx = rat(b[0]) - rat(d[0]);
    let bdy = rat(b[1]) - rat(d[1]);
    let bdz = rat(b[2]) - rat(d[2]);
    let cdx = rat(c[0]) - rat(d[0]);
    let cdy = rat(c[1]) - rat(d[1]);
    let cdz = rat(c[2]) - rat(d[2]);
    let bc = &bdy * &cdz - &bdz * &cdy;
    let ca = &bdz * &cdx - &bdx * &cdz;
    let ab = &bdx * &cdy - &bdy * &cdx;
    let s = &adx * &bc + &ady * &ca + &adz * &ab;
    let neg = -s;
    if neg.is_positive() {
        1
    } else if neg.is_negative() {
        -1
    } else {
        0
    }
}

fn pred_sign3(a: [f64; 3], b: [f64; 3], c: [f64; 3], d: [f64; 3]) -> i32 {
    match orient3d(
        &Point3::new(a[0], a[1], a[2]),
        &Point3::new(b[0], b[1], b[2]),
        &Point3::new(c[0], c[1], c[2]),
        &Point3::new(d[0], d[1], d[2]),
    ) {
        Orientation::CounterClockwise => 1,
        Orientation::Clockwise => -1,
        Orientation::Collinear => 0,
    }
}

fn naive_sign3(a: [f64; 3], b: [f64; 3], c: [f64; 3], d: [f64; 3]) -> i32 {
    let (adx, ady, adz) = (a[0] - d[0], a[1] - d[1], a[2] - d[2]);
    let (bdx, bdy, bdz) = (b[0] - d[0], b[1] - d[1], b[2] - d[2]);
    let (cdx, cdy, cdz) = (c[0] - d[0], c[1] - d[1], c[2] - d[2]);
    let s = adx * (bdy * cdz - bdz * cdy)
        + ady * (bdz * cdx - bdx * cdz)
        + adz * (bdx * cdy - bdy * cdx);
    let det = -s;
    if det > 0.0 {
        1
    } else if det < 0.0 {
        -1
    } else {
        0
    }
}

#[test]
fn orient3d_matches_exact_rational_oracle_on_adversarial_inputs() {
    let mut rng = StdRng::seed_from_u64(0x0BAD_F00D_0042_2718);
    let mut mismatches = 0u64;
    let mut hard = 0u64;
    let mut first_fail = None;
    let n = 150_000u64;

    for _ in 0..n {
        let a = [
            rng.gen_range(-1.0..1.0),
            rng.gen_range(-1.0..1.0),
            rng.gen_range(-1.0..1.0),
        ];
        let b = [
            rng.gen_range(-1.0..1.0),
            rng.gen_range(-1.0..1.0),
            rng.gen_range(-1.0..1.0),
        ];
        let c = [
            rng.gen_range(-1.0..1.0),
            rng.gen_range(-1.0..1.0),
            rng.gen_range(-1.0..1.0),
        ];
        // d placed NEAR the plane of (a,b,c): an affine combination + a tiny
        // per-coordinate nudge, so the determinant is small and sign-delicate.
        let s: f64 = rng.gen_range(-1.0..2.0);
        let t: f64 = rng.gen_range(-1.0..2.0);
        let nudge = 1e-13;
        let d = [
            a[0] + s * (b[0] - a[0]) + t * (c[0] - a[0]) + rng.gen_range(-1.0..1.0) * nudge,
            a[1] + s * (b[1] - a[1]) + t * (c[1] - a[1]) + rng.gen_range(-1.0..1.0) * nudge,
            a[2] + s * (b[2] - a[2]) + t * (c[2] - a[2]) + rng.gen_range(-1.0..1.0) * nudge,
        ];

        let exact = exact_sign3(a, b, c, d);
        let got = pred_sign3(a, b, c, d);
        if got != exact {
            mismatches += 1;
            if first_fail.is_none() {
                first_fail = Some((a, b, c, d, got, exact));
            }
        }
        if naive_sign3(a, b, c, d) != exact {
            hard += 1;
        }
    }

    assert!(
        hard > 0,
        "the sweep never reached the hard regime (naive f64 always matched exact) — make d closer to the plane"
    );
    assert_eq!(
        mismatches, 0,
        "orient3d disagreed with the EXACT rational oracle on {mismatches}/{n} cases \
         (hard cases where naive f64 was wrong: {hard}); first failure: {first_fail:?} \
         — the predicate is NOT exact"
    );
}

// ── incircle (cocircular) ──────────────────────────────────────────────────

/// Exact in-circle sign via rationals. Matches incircle's determinant:
/// `alift·bcdet + blift·cadet + clift·abdet` (>0 ⇒ Inside).
fn exact_incircle_sign(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> i32 {
    let adx = rat(a[0]) - rat(d[0]);
    let ady = rat(a[1]) - rat(d[1]);
    let bdx = rat(b[0]) - rat(d[0]);
    let bdy = rat(b[1]) - rat(d[1]);
    let cdx = rat(c[0]) - rat(d[0]);
    let cdy = rat(c[1]) - rat(d[1]);
    let alift = &adx * &adx + &ady * &ady;
    let blift = &bdx * &bdx + &bdy * &bdy;
    let clift = &cdx * &cdx + &cdy * &cdy;
    let bcdet = &bdx * &cdy - &cdx * &bdy;
    let cadet = &cdx * &ady - &adx * &cdy;
    let abdet = &adx * &bdy - &bdx * &ady;
    let det = &alift * &bcdet + &blift * &cadet + &clift * &abdet;
    if det.is_positive() {
        1
    } else if det.is_negative() {
        -1
    } else {
        0
    }
}

fn pred_incircle_sign(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> i32 {
    match incircle(
        &Vector2::new(a[0], a[1]),
        &Vector2::new(b[0], b[1]),
        &Vector2::new(c[0], c[1]),
        &Vector2::new(d[0], d[1]),
    ) {
        CircleLocation::Inside => 1,
        CircleLocation::Outside => -1,
        CircleLocation::OnBoundary => 0,
    }
}

fn naive_incircle_sign(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> i32 {
    let (adx, ady) = (a[0] - d[0], a[1] - d[1]);
    let (bdx, bdy) = (b[0] - d[0], b[1] - d[1]);
    let (cdx, cdy) = (c[0] - d[0], c[1] - d[1]);
    let alift = adx * adx + ady * ady;
    let blift = bdx * bdx + bdy * bdy;
    let clift = cdx * cdx + cdy * cdy;
    let det = alift * (bdx * cdy - cdx * bdy)
        + blift * (cdx * ady - adx * cdy)
        + clift * (adx * bdy - bdx * ady);
    if det > 0.0 {
        1
    } else if det < 0.0 {
        -1
    } else {
        0
    }
}

#[test]
fn incircle_matches_exact_rational_oracle_on_adversarial_inputs() {
    let mut rng = StdRng::seed_from_u64(0xCAFE_BABE_1357_9BDF);
    let mut mismatches = 0u64;
    let mut hard = 0u64;
    let mut first_fail = None;
    let n = 150_000u64;
    use std::f64::consts::TAU;

    for _ in 0..n {
        // a, b, c, d on (nearly) a common circle: the in-circle determinant is
        // then small and sign-delicate. d is nudged radially off the circle.
        let cx = rng.gen_range(-1.0..1.0);
        let cy = rng.gen_range(-1.0..1.0);
        let r = rng.gen_range(0.5..2.0);
        let on = |ang: f64, dr: f64| [cx + (r + dr) * ang.cos(), cy + (r + dr) * ang.sin()];
        let a = on(rng.gen_range(0.0..TAU), 0.0);
        let b = on(rng.gen_range(0.0..TAU), 0.0);
        let c = on(rng.gen_range(0.0..TAU), 0.0);
        let d = on(rng.gen_range(0.0..TAU), rng.gen_range(-1.0..1.0) * 1e-13);

        let exact = exact_incircle_sign(a, b, c, d);
        let got = pred_incircle_sign(a, b, c, d);
        if got != exact {
            mismatches += 1;
            if first_fail.is_none() {
                first_fail = Some((a, b, c, d, got, exact));
            }
        }
        if naive_incircle_sign(a, b, c, d) != exact {
            hard += 1;
        }
    }

    assert!(
        hard > 0,
        "the sweep never reached the hard regime (naive f64 always matched exact) — make d closer to the circle"
    );
    assert_eq!(
        mismatches, 0,
        "incircle disagreed with the EXACT rational oracle on {mismatches}/{n} cases \
         (hard cases where naive f64 was wrong: {hard}); first failure: {first_fail:?} \
         — the predicate is NOT exact"
    );
}
