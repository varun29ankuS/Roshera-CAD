// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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
use geometry_engine::math::{
    incircle, insphere, orient2d, orient3d, signed_area_2d, CircleLocation, Orientation, Point3,
};
use num_rational::BigRational;
use num_traits::{Signed, Zero};
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
    // 2026-07-15: reduced from 150_000 to 40_000. The in-circle exact oracle is
    // the most expensive of the four predicate sweeps — its 4×4 determinant is
    // taken over SQUARED coordinates, so the `BigRational` numerators grow far
    // larger than the square-free orient2d/orient3d determinants, and in a debug
    // build the bignum arithmetic dominates. At 150_000 this sweep ran ~281 s
    // locally and tipped the 300 s CI job timeout; 40_000 (~75 s local) keeps a
    // wide margin while still throwing tens of thousands of hard, near-degenerate
    // cases at the predicate (the `hard > 0` guard below proves the adversarial
    // regime is still reached). Matches the same reasoning that already caps the
    // insphere sweep at 30_000.
    let n = 40_000u64;
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

// ── insphere (cospherical) ─────────────────────────────────────────────────

/// Exact in-sphere sign via rationals. Mirrors insphere_fast's determinant
/// `dlift·abc - clift·dab + blift·cda - alift·bcd` (>0 ⇒ Inside).
#[allow(clippy::too_many_arguments)]
fn exact_insphere_sign(a: [f64; 3], b: [f64; 3], c: [f64; 3], d: [f64; 3], e: [f64; 3]) -> i32 {
    let aex = rat(a[0]) - rat(e[0]);
    let aey = rat(a[1]) - rat(e[1]);
    let aez = rat(a[2]) - rat(e[2]);
    let bex = rat(b[0]) - rat(e[0]);
    let bey = rat(b[1]) - rat(e[1]);
    let bez = rat(b[2]) - rat(e[2]);
    let cex = rat(c[0]) - rat(e[0]);
    let cey = rat(c[1]) - rat(e[1]);
    let cez = rat(c[2]) - rat(e[2]);
    let dex = rat(d[0]) - rat(e[0]);
    let dey = rat(d[1]) - rat(e[1]);
    let dez = rat(d[2]) - rat(e[2]);
    let ab = &aex * &bey - &bex * &aey;
    let bc = &bex * &cey - &cex * &bey;
    let cd = &cex * &dey - &dex * &cey;
    let da = &dex * &aey - &aex * &dey;
    let ac = &aex * &cey - &cex * &aey;
    let bd = &bex * &dey - &dex * &bey;
    let abc = &aez * &bc - &bez * &ac + &cez * &ab;
    let bcd = &bez * &cd - &cez * &bd + &dez * &bc;
    let cda = &cez * &da + &dez * &ac + &aez * &cd;
    let dab = &dez * &ab + &aez * &bd + &bez * &da;
    let alift = &aex * &aex + &aey * &aey + &aez * &aez;
    let blift = &bex * &bex + &bey * &bey + &bez * &bez;
    let clift = &cex * &cex + &cey * &cey + &cez * &cez;
    let dlift = &dex * &dex + &dey * &dey + &dez * &dez;
    let det = &dlift * &abc - &clift * &dab + &blift * &cda - &alift * &bcd;
    if det.is_positive() {
        1
    } else if det.is_negative() {
        -1
    } else {
        0
    }
}

fn pred_insphere_sign(a: [f64; 3], b: [f64; 3], c: [f64; 3], d: [f64; 3], e: [f64; 3]) -> i32 {
    match insphere(
        &Point3::new(a[0], a[1], a[2]),
        &Point3::new(b[0], b[1], b[2]),
        &Point3::new(c[0], c[1], c[2]),
        &Point3::new(d[0], d[1], d[2]),
        &Point3::new(e[0], e[1], e[2]),
    ) {
        CircleLocation::Inside => 1,
        CircleLocation::Outside => -1,
        CircleLocation::OnBoundary => 0,
    }
}

#[allow(clippy::too_many_arguments)]
fn naive_insphere_sign(a: [f64; 3], b: [f64; 3], c: [f64; 3], d: [f64; 3], e: [f64; 3]) -> i32 {
    let (aex, aey, aez) = (a[0] - e[0], a[1] - e[1], a[2] - e[2]);
    let (bex, bey, bez) = (b[0] - e[0], b[1] - e[1], b[2] - e[2]);
    let (cex, cey, cez) = (c[0] - e[0], c[1] - e[1], c[2] - e[2]);
    let (dex, dey, dez) = (d[0] - e[0], d[1] - e[1], d[2] - e[2]);
    let ab = aex * bey - bex * aey;
    let bc = bex * cey - cex * bey;
    let cd = cex * dey - dex * cey;
    let da = dex * aey - aex * dey;
    let ac = aex * cey - cex * aey;
    let bd = bex * dey - dex * bey;
    let abc = aez * bc - bez * ac + cez * ab;
    let bcd = bez * cd - cez * bd + dez * bc;
    let cda = cez * da + dez * ac + aez * cd;
    let dab = dez * ab + aez * bd + bez * da;
    let alift = aex * aex + aey * aey + aez * aez;
    let blift = bex * bex + bey * bey + bez * bez;
    let clift = cex * cex + cey * cey + cez * cez;
    let dlift = dex * dex + dey * dey + dez * dez;
    let det = dlift * abc - clift * dab + blift * cda - alift * bcd;
    if det > 0.0 {
        1
    } else if det < 0.0 {
        -1
    } else {
        0
    }
}

#[test]
fn insphere_matches_exact_rational_oracle_on_adversarial_inputs() {
    let mut rng = StdRng::seed_from_u64(0xFEED_FACE_2468_ACE0);
    let mut mismatches = 0u64;
    let mut hard = 0u64;
    let mut first_fail = None;
    // Fewer cases than the 2D sweeps: the exact path runs heap expansion
    // arithmetic on thousands of components, and the oracle is a 4×4 rational
    // determinant — both costly. 30k still hits the hard regime densely.
    let n = 30_000u64;

    let mut unit = |rng: &mut StdRng| -> [f64; 3] {
        loop {
            let v = [
                rng.gen_range(-1.0..1.0),
                rng.gen_range(-1.0..1.0),
                rng.gen_range(-1.0..1.0),
            ];
            let len2: f64 = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
            if len2 > 0.05 {
                let len = len2.sqrt();
                return [v[0] / len, v[1] / len, v[2] / len];
            }
        }
    };

    for _ in 0..n {
        let center = [
            rng.gen_range(-1.0..1.0),
            rng.gen_range(-1.0..1.0),
            rng.gen_range(-1.0..1.0),
        ];
        let r = rng.gen_range(0.5..2.0);
        let on = |dir: [f64; 3], dr: f64| {
            [
                center[0] + (r + dr) * dir[0],
                center[1] + (r + dr) * dir[1],
                center[2] + (r + dr) * dir[2],
            ]
        };
        let da = unit(&mut rng);
        let db = unit(&mut rng);
        let dc = unit(&mut rng);
        let dd = unit(&mut rng);
        let de = unit(&mut rng);
        let a = on(da, 0.0);
        let b = on(db, 0.0);
        let c = on(dc, 0.0);
        let d = on(dd, 0.0);
        // e nudged radially off the sphere — small enough to be sign-delicate.
        let e = on(de, rng.gen_range(-1.0..1.0) * 1e-13);

        let exact = exact_insphere_sign(a, b, c, d, e);
        let got = pred_insphere_sign(a, b, c, d, e);
        if got != exact {
            mismatches += 1;
            if first_fail.is_none() {
                first_fail = Some((a, b, c, d, e, got, exact));
            }
        }
        if naive_insphere_sign(a, b, c, d, e) != exact {
            hard += 1;
        }
    }

    assert!(
        hard > 0,
        "the sweep never reached the hard regime (naive f64 always matched exact) — make e closer to the sphere"
    );
    assert_eq!(
        mismatches, 0,
        "insphere disagreed with the EXACT rational oracle on {mismatches}/{n} cases \
         (hard cases where naive f64 was wrong: {hard}); first failure: {first_fail:?} \
         — the predicate is NOT exact"
    );
}

// ── signed_area_2d (polygon winding) ───────────────────────────────────────

/// Exact polygon shoelace sign via rationals: Σ (x_i·y_{i+1} − x_{i+1}·y_i).
fn exact_signed_area_sign(poly: &[[f64; 2]]) -> i32 {
    let n = poly.len();
    let mut sum = BigRational::zero();
    for i in 0..n {
        let p = &poly[i];
        let q = &poly[(i + 1) % n];
        sum += rat(p[0]) * rat(q[1]) - rat(q[0]) * rat(p[1]);
    }
    if sum.is_positive() {
        1
    } else if sum.is_negative() {
        -1
    } else {
        0
    }
}

fn pred_signed_area_sign(poly: &[[f64; 2]]) -> i32 {
    let pts: Vec<Vector2> = poly.iter().map(|p| Vector2::new(p[0], p[1])).collect();
    match signed_area_2d(&pts) {
        Orientation::CounterClockwise => 1,
        Orientation::Clockwise => -1,
        Orientation::Collinear => 0,
    }
}

fn naive_signed_area_sign(poly: &[[f64; 2]]) -> i32 {
    let n = poly.len();
    let mut sum = 0.0f64;
    for i in 0..n {
        let p = &poly[i];
        let q = &poly[(i + 1) % n];
        sum += p[0] * q[1] - q[0] * p[1];
    }
    if sum > 0.0 {
        1
    } else if sum < 0.0 {
        -1
    } else {
        0
    }
}

#[test]
fn signed_area_2d_matches_exact_rational_oracle_on_adversarial_inputs() {
    let mut rng = StdRng::seed_from_u64(0x5161_5249_4E47_2218);
    let mut mismatches = 0u64;
    let mut hard = 0u64;
    let mut first_fail = None;
    let n = 200_000u64;

    for _ in 0..n {
        // Near-degenerate slivers: vertices hug the line y = x (O(1) coords, so
        // the shoelace PRODUCTS are ~1) with a ~1e-15 perpendicular nudge, so the
        // net area sits near the f64 rounding floor and the products cancel
        // catastrophically — the regime where naive f64 flips sign. Vertex 3..7.
        let vcount = 3 + (rng.gen_range(0u32..4) as usize);
        let poly: Vec<[f64; 2]> = (0..vcount)
            .map(|_| {
                let x = rng.gen_range(-1.0..1.0);
                let y = x + rng.gen_range(-1.0..1.0) * 1e-15;
                [x, y]
            })
            .collect();

        let exact = exact_signed_area_sign(&poly);
        let got = pred_signed_area_sign(&poly);
        if got != exact {
            mismatches += 1;
            if first_fail.is_none() {
                first_fail = Some((poly.clone(), got, exact));
            }
        }
        if naive_signed_area_sign(&poly) != exact {
            hard += 1;
        }
    }

    assert!(
        hard > 0,
        "the sweep never reached the hard regime (naive f64 always matched exact) — squeeze the polygons thinner"
    );
    assert_eq!(
        mismatches, 0,
        "signed_area_2d disagreed with the EXACT rational oracle on {mismatches}/{n} cases \
         (hard cases where naive f64 was wrong: {hard}); first failure: {first_fail:?} \
         — the predicate is NOT exact"
    );
}
