//! Exact geometric predicates for robust computational geometry
//!
//! This module provides exact geometric predicates using adaptive precision
//! arithmetic based on Jonathan Shewchuk's algorithms. These predicates are
//! essential for preventing topological inconsistencies in B-Rep operations.
//!
//! # Key Features
//!
//! - Exact orientation tests (2D and 3D)
//! - Exact in-circle and in-sphere tests
//! - Adaptive precision for optimal performance
//! - Handles all degenerate cases correctly
//! - Zero tolerance for numerical errors
//!
//! # Algorithm
//!
//! The predicates use a multi-stage approach:
//! 1. Fast approximate test using standard floating-point
//! 2. Semi-robust test with error bounds
//! 3. Exact test using arbitrary precision arithmetic
//!
//! Most inputs (>99%) only require the fast test, maintaining performance
//! while guaranteeing correctness for all inputs.

use crate::math::{vector2::Vector2, vector3::Vector3, Point3};
use std::cmp::Ordering as CmpOrdering;

/// Result of an orientation test
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// Points are in counter-clockwise order (positive orientation)
    CounterClockwise,
    /// Points are in clockwise order (negative orientation)
    Clockwise,
    /// Points are collinear (zero orientation)
    Collinear,
}

/// Result of an in-circle/sphere test
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircleLocation {
    /// Point is inside the circle/sphere
    Inside,
    /// Point is outside the circle/sphere
    Outside,
    /// Point is exactly on the circle/sphere
    OnBoundary,
}

// Error bounds for adaptive precision filters (Shewchuk 1997, "Adaptive
// Precision Floating-Point Arithmetic and Fast Robust Geometric Predicates",
// Discrete & Computational Geometry 18:305–363). The A-bound is the first-stage
// filter; when |det| falls within it the predicate falls through to EXACT
// expansion arithmetic (the `*_adapt` functions), so the returned sign is
// correct for all finite inputs — proven against an arbitrary-precision oracle
// in tests/predicate_exactness_gate.rs. All four A-bounds are ε-derived
// (ε = 2^-53; see EPS below): ccwA=(3+16ε)ε, o3dA=(7+56ε)ε, iccA=(10+96ε)ε,
// ispA=(16+224ε)ε.
const CCWERRBOUNDSA: f64 = 3.3306690738754716e-16;
const O3DERRBOUNDSA: f64 = 7.771_561_172_376_096e-16;
const ICCERRBOUNDSA: f64 = (10.0 + 96.0 * EPS) * EPS;
const ISPERRBOUNDSA: f64 = (16.0 + 224.0 * EPS) * EPS;

// Multi-stage orient2d bounds, derived from ε = 2^-53 (Shewchuk exactinit).
// The A bound equals CCWERRBOUNDSA above; B/C drive the exact-fallback cascade.
const EPS: f64 = 1.110_223_024_625_156_5e-16; // 2^-53
const CCWERRBOUNDB: f64 = (2.0 + 12.0 * EPS) * EPS;
const CCWERRBOUNDC: f64 = (9.0 + 64.0 * EPS) * EPS * EPS;
const O2D_RESULTERRBOUND: f64 = (3.0 + 8.0 * EPS) * EPS;

// Splitter for exact arithmetic (2^27 + 1 for IEEE 754 double)
const SPLITTER: f64 = 134217729.0;

/// Split a floating-point number into high and low parts for exact arithmetic
#[inline(always)]
fn split(a: f64) -> (f64, f64) {
    let c = SPLITTER * a;
    let abig = c - a;
    let ahi = c - abig;
    let alo = a - ahi;
    (ahi, alo)
}

/// Exact multiplication of two floating-point numbers
/// Returns (high, low) where high + low = a * b exactly
#[inline(always)]
fn two_product(a: f64, b: f64) -> (f64, f64) {
    let x = a * b;
    let (ahi, alo) = split(a);
    let (bhi, blo) = split(b);
    let err1 = x - (ahi * bhi);
    let err2 = err1 - (alo * bhi);
    let err3 = err2 - (ahi * blo);
    let y = (alo * blo) - err3;
    (x, y)
}

/// Exact addition of two floating-point numbers
/// Returns (high, low) where high + low = a + b exactly
#[inline(always)]
fn two_sum(a: f64, b: f64) -> (f64, f64) {
    let x = a + b;
    let bvirt = x - a;
    let avirt = x - bvirt;
    let bround = b - bvirt;
    let around = a - avirt;
    let y = around + bround;
    (x, y)
}

/// Fast exact sum, valid ONLY when `|a| >= |b|`. 3 flops vs `two_sum`'s 6.
/// Returns `(x, y)` with `x + y == a + b` exactly. (Shewchuk `Fast_Two_Sum`.)
#[inline(always)]
#[allow(dead_code)] // wired into the exact predicates in the following slice
fn fast_two_sum(a: f64, b: f64) -> (f64, f64) {
    let x = a + b;
    let bvirt = x - a;
    let y = b - bvirt;
    (x, y)
}

/// Exact subtraction: `(x, y)` with `x + y == a - b` exactly. (Shewchuk `Two_Diff`.)
#[inline(always)]
fn two_diff(a: f64, b: f64) -> (f64, f64) {
    let x = a - b;
    let bvirt = a - x;
    let avirt = x + bvirt;
    let bround = bvirt - b;
    let around = a - avirt;
    let y = around + bround;
    (x, y)
}

/// `(a1 + a0) - b` as a 3-component expansion `(x2, x1, x0)` (high→low).
/// (Shewchuk `Two_One_Diff`.)
#[inline(always)]
fn two_one_diff(a1: f64, a0: f64, b: f64) -> (f64, f64, f64) {
    let (i, x0) = two_diff(a0, b);
    let (x2, x1) = two_sum(a1, i);
    (x2, x1, x0)
}

/// `(a1 + a0) - (b1 + b0)` as a 4-component expansion `[x0, x1, x2, x3]`
/// (low→high). (Shewchuk `Two_Two_Diff`.)
#[inline(always)]
fn two_two_diff(a1: f64, a0: f64, b1: f64, b0: f64) -> [f64; 4] {
    let (j, j0, x0) = two_one_diff(a1, a0, b0);
    let (x3, x2, x1) = two_one_diff(j, j0, b1);
    [x0, x1, x2, x3]
}

/// Sum of an expansion's components — the cheap approximation of its value
/// (Shewchuk `estimate`). Components are summed low→high.
#[inline]
#[allow(dead_code)]
fn estimate(e: &[f64]) -> f64 {
    let mut q = 0.0;
    for &c in e {
        q += c;
    }
    q
}

/// Merge two nonoverlapping, sorted (low→high magnitude) expansions `e` and `f`
/// into `h = e + f`, exactly, eliminating zero components. `h` must have
/// capacity `e.len() + f.len()`. Returns the component count. (Shewchuk
/// `fast_expansion_sum_zeroelim`.) Reads are bounds-guarded (the C original
/// reads one-past-end into a value it never uses; Rust must not).
#[allow(dead_code)]
fn fast_expansion_sum_zeroelim(e: &[f64], f: &[f64], h: &mut [f64]) -> usize {
    let (elen, flen) = (e.len(), f.len());
    let mut enow = e[0];
    let mut fnow = f[0];
    let mut eindex = 0usize;
    let mut findex = 0usize;
    let mut q: f64;
    if (fnow > enow) == (fnow > -enow) {
        q = enow;
        eindex += 1;
        if eindex < elen {
            enow = e[eindex];
        }
    } else {
        q = fnow;
        findex += 1;
        if findex < flen {
            fnow = f[findex];
        }
    }
    let mut hindex = 0usize;
    while eindex < elen && findex < flen {
        let (qnew, hh) = if (fnow > enow) == (fnow > -enow) {
            let r = if hindex == 0 {
                fast_two_sum(enow, q)
            } else {
                two_sum(q, enow)
            };
            eindex += 1;
            if eindex < elen {
                enow = e[eindex];
            }
            r
        } else {
            let r = if hindex == 0 {
                fast_two_sum(fnow, q)
            } else {
                two_sum(q, fnow)
            };
            findex += 1;
            if findex < flen {
                fnow = f[findex];
            }
            r
        };
        q = qnew;
        if hh != 0.0 {
            h[hindex] = hh;
            hindex += 1;
        }
    }
    while eindex < elen {
        let (qnew, hh) = two_sum(q, enow);
        eindex += 1;
        if eindex < elen {
            enow = e[eindex];
        }
        q = qnew;
        if hh != 0.0 {
            h[hindex] = hh;
            hindex += 1;
        }
    }
    while findex < flen {
        let (qnew, hh) = two_sum(q, fnow);
        findex += 1;
        if findex < flen {
            fnow = f[findex];
        }
        q = qnew;
        if hh != 0.0 {
            h[hindex] = hh;
            hindex += 1;
        }
    }
    if q != 0.0 || hindex == 0 {
        h[hindex] = q;
        hindex += 1;
    }
    hindex
}

/// Scale an expansion `e` by a single double `b`, exactly, zero-eliminated.
/// `h` must have capacity `2 * e.len()`. Returns the component count.
/// (Shewchuk `scale_expansion_zeroelim`.)
#[allow(dead_code)]
fn scale_expansion_zeroelim(e: &[f64], b: f64, h: &mut [f64]) -> usize {
    let (mut q, hh0) = two_product(e[0], b);
    let mut hindex = 0usize;
    if hh0 != 0.0 {
        h[hindex] = hh0;
        hindex += 1;
    }
    for &enow in &e[1..] {
        let (product1, product0) = two_product(enow, b);
        let (sum, hh) = two_sum(q, product0);
        if hh != 0.0 {
            h[hindex] = hh;
            hindex += 1;
        }
        let (qn, hh2) = fast_two_sum(product1, sum);
        q = qn;
        if hh2 != 0.0 {
            h[hindex] = hh2;
            hindex += 1;
        }
    }
    if q != 0.0 || hindex == 0 {
        h[hindex] = q;
        hindex += 1;
    }
    hindex
}

/// Fast approximate 2D orientation test
#[inline(always)]
fn orient2d_fast(pa: &Vector2, pb: &Vector2, pc: &Vector2) -> f64 {
    (pa.x - pc.x) * (pb.y - pc.y) - (pa.y - pc.y) * (pb.x - pc.x)
}

/// Adaptive precision 2D orientation test
/// Map a determinant value to an [`Orientation`].
#[inline]
fn orientation_of(det: f64) -> Orientation {
    if det > 0.0 {
        Orientation::CounterClockwise
    } else if det < 0.0 {
        Orientation::Clockwise
    } else {
        Orientation::Collinear
    }
}

/// Full Shewchuk adaptive orient2d. Returns a value whose SIGN is the exact sign
/// of the determinant. Stage B refines with the product roundoffs; Stage C adds
/// the coordinate-difference-tail corrections; Stage D builds the complete
/// expansion (the provably-exact pass). `detsum` is the `|det|` magnitude
/// estimate from the caller's A-filter.
fn orient2d_adapt(pa: &Vector2, pb: &Vector2, pc: &Vector2, detsum: f64) -> f64 {
    // Coordinate differences WITH their roundoff tails (the bits the old
    // implementation dropped — the source of the sign errors the harness found).
    let (acx, acxtail) = two_diff(pa.x, pc.x);
    let (bcx, bcxtail) = two_diff(pb.x, pc.x);
    let (acy, acytail) = two_diff(pa.y, pc.y);
    let (bcy, bcytail) = two_diff(pb.y, pc.y);

    let (detleft, detlefttail) = two_product(acx, bcy);
    let (detright, detrighttail) = two_product(acy, bcx);

    // Stage B: B = (detleft+detlefttail) - (detright+detrighttail), exact 4-comp.
    let b = two_two_diff(detleft, detlefttail, detright, detrighttail);
    let mut det = estimate(&b);
    let errbound = CCWERRBOUNDB * detsum;
    if det >= errbound || -det >= errbound {
        return det;
    }

    // If every coordinate difference was exact, B is already the exact answer.
    if acxtail == 0.0 && acytail == 0.0 && bcxtail == 0.0 && bcytail == 0.0 {
        return det;
    }

    // Stage C: add the first-order coordinate-tail correction.
    let errbound = CCWERRBOUNDC * detsum + O2D_RESULTERRBOUND * det.abs();
    det += (acx * bcytail + bcy * acxtail) - (acy * bcxtail + bcx * acytail);
    if det >= errbound || -det >= errbound {
        return det;
    }

    // Stage D: the exact expansion. C1 = B + (acxtail·bcy - acytail·bcx).
    let (s1, s0) = two_product(acxtail, bcy);
    let (t1, t0) = two_product(acytail, bcx);
    let u = two_two_diff(s1, s0, t1, t0);
    let mut c1 = [0.0f64; 8];
    let c1len = fast_expansion_sum_zeroelim(&b, &u, &mut c1);

    // C2 = C1 + (acx·bcytail - acy·bcxtail).
    let (s1, s0) = two_product(acx, bcytail);
    let (t1, t0) = two_product(acy, bcxtail);
    let u = two_two_diff(s1, s0, t1, t0);
    let mut c2 = [0.0f64; 12];
    let c2len = fast_expansion_sum_zeroelim(&c1[..c1len], &u, &mut c2);

    // D = C2 + (acxtail·bcytail - acytail·bcxtail).
    let (s1, s0) = two_product(acxtail, bcytail);
    let (t1, t0) = two_product(acytail, bcxtail);
    let u = two_two_diff(s1, s0, t1, t0);
    let mut d = [0.0f64; 16];
    let dlen = fast_expansion_sum_zeroelim(&c2[..c2len], &u, &mut d);

    // The most significant component carries the exact sign.
    d[dlen - 1]
}

/// Test whether three points are in counter-clockwise order — the EXACT sign of
/// the orientation determinant of (pa, pb, pc), correct for all finite inputs
/// (full Shewchuk adaptive cascade). `Collinear` iff the determinant is exactly
/// zero.
pub fn orient2d(pa: &Vector2, pb: &Vector2, pc: &Vector2) -> Orientation {
    let detleft = (pa.x - pc.x) * (pb.y - pc.y);
    let detright = (pa.y - pc.y) * (pb.x - pc.x);
    let det = detleft - detright;

    // A-filter permanent: when the two products have strict opposite signs the
    // subtraction is reliable; otherwise bound by their magnitude sum.
    let detsum = if detleft > 0.0 {
        if detright <= 0.0 {
            return orientation_of(det);
        }
        detleft + detright
    } else if detleft < 0.0 {
        if detright >= 0.0 {
            return orientation_of(det);
        }
        -detleft - detright
    } else {
        return orientation_of(det);
    };

    let errbound = CCWERRBOUNDSA * detsum;
    if det >= errbound || -det >= errbound {
        return orientation_of(det);
    }
    orientation_of(orient2d_adapt(pa, pb, pc, detsum))
}

/// Fast approximate 3D orientation test  
#[inline(always)]
fn orient3d_fast(pa: &Point3, pb: &Point3, pc: &Point3, pd: &Point3) -> f64 {
    let adx = pa.x - pd.x;
    let ady = pa.y - pd.y;
    let adz = pa.z - pd.z;
    let bdx = pb.x - pd.x;
    let bdy = pb.y - pd.y;
    let bdz = pb.z - pd.z;
    let cdx = pc.x - pd.x;
    let cdy = pc.y - pd.y;
    let cdz = pc.z - pd.z;

    // Note the sign change here - this gives the correct orientation
    -(adx * (bdy * cdz - bdz * cdy) + ady * (bdz * cdx - bdx * cdz) + adz * (bdx * cdy - bdy * cdx))
}

/// Adaptive precision 3D orientation test
/// Buffer ceiling for the generic exact-expansion fallbacks. Sized for the
/// largest determinant built this way — the degree-4 in-circle (lifted)
/// expansion reaches ~1536 components; 2048 leaves headroom. (orient3d uses far
/// less.) These live on the stack only on the rare exact path.
const EXP_MAX: usize = 2048;

/// A coordinate difference `a - b` as a zero-free expansion (low→high) in `buf`.
/// Returns the component count (0 when `a == b` exactly).
#[inline]
fn diff_exp(a: f64, b: f64, buf: &mut [f64; 2]) -> usize {
    let (x, y) = two_diff(a, b);
    let mut n = 0;
    if y != 0.0 {
        buf[n] = y;
        n += 1;
    }
    if x != 0.0 {
        buf[n] = x;
        n += 1;
    }
    n
}

/// Exact product `out = a · b` of two zero-free expansions, via `Σ scale(a, b[i])`.
/// Returns the component count; empty input ⇒ exact 0.
fn expansion_product(a: &[f64], b: &[f64], out: &mut [f64]) -> usize {
    if a.is_empty() || b.is_empty() {
        return 0;
    }
    let mut acc = [0.0f64; EXP_MAX];
    let mut acc_len = 0usize;
    let mut scaled = [0.0f64; EXP_MAX];
    let mut next = [0.0f64; EXP_MAX];
    for &bi in b {
        let slen = scale_expansion_zeroelim(a, bi, &mut scaled);
        if acc_len == 0 {
            acc[..slen].copy_from_slice(&scaled[..slen]);
            acc_len = slen;
        } else {
            let nlen = fast_expansion_sum_zeroelim(&acc[..acc_len], &scaled[..slen], &mut next);
            acc[..nlen].copy_from_slice(&next[..nlen]);
            acc_len = nlen;
        }
    }
    out[..acc_len].copy_from_slice(&acc[..acc_len]);
    acc_len
}

/// Exact difference `out = e - f` of two zero-free expansions; empty ⇒ 0.
fn expansion_diff(e: &[f64], f: &[f64], out: &mut [f64]) -> usize {
    if f.is_empty() {
        out[..e.len()].copy_from_slice(e);
        return e.len();
    }
    let mut negf = [0.0f64; EXP_MAX];
    for (i, &c) in f.iter().enumerate() {
        negf[i] = -c;
    }
    if e.is_empty() {
        out[..f.len()].copy_from_slice(&negf[..f.len()]);
        return f.len();
    }
    fast_expansion_sum_zeroelim(e, &negf[..f.len()], out)
}

/// Exact 3D orientation determinant value — its SIGN is correct for all finite
/// inputs. Builds the full expansion of
/// `-(adx·(bdy·cdz - bdz·cdy) + ady·(bdz·cdx - bdx·cdz) + adz·(bdx·cdy - bdy·cdx))`
/// with the coordinate-difference tails carried (the bits the old implementation
/// dropped). Generic exact fallback — only runs when the A-filter is inconclusive.
fn orient3d_adapt(pa: &Point3, pb: &Point3, pc: &Point3, pd: &Point3) -> f64 {
    let (mut adx, mut ady, mut adz) = ([0.0; 2], [0.0; 2], [0.0; 2]);
    let (mut bdx, mut bdy, mut bdz) = ([0.0; 2], [0.0; 2], [0.0; 2]);
    let (mut cdx, mut cdy, mut cdz) = ([0.0; 2], [0.0; 2], [0.0; 2]);
    let adxn = diff_exp(pa.x, pd.x, &mut adx);
    let adyn = diff_exp(pa.y, pd.y, &mut ady);
    let adzn = diff_exp(pa.z, pd.z, &mut adz);
    let bdxn = diff_exp(pb.x, pd.x, &mut bdx);
    let bdyn = diff_exp(pb.y, pd.y, &mut bdy);
    let bdzn = diff_exp(pb.z, pd.z, &mut bdz);
    let cdxn = diff_exp(pc.x, pd.x, &mut cdx);
    let cdyn = diff_exp(pc.y, pd.y, &mut cdy);
    let cdzn = diff_exp(pc.z, pd.z, &mut cdz);

    let mut p1 = [0.0f64; EXP_MAX];
    let mut p2 = [0.0f64; EXP_MAX];
    let mut minor = [0.0f64; EXP_MAX];
    let mut term = [0.0f64; EXP_MAX];
    let mut s = [0.0f64; EXP_MAX];
    let mut s2 = [0.0f64; EXP_MAX];
    let mut s_len;

    // Cofactor 1: adx · (bdy·cdz - bdz·cdy)
    let p1n = expansion_product(&bdy[..bdyn], &cdz[..cdzn], &mut p1);
    let p2n = expansion_product(&bdz[..bdzn], &cdy[..cdyn], &mut p2);
    let mn = expansion_diff(&p1[..p1n], &p2[..p2n], &mut minor);
    let tn = expansion_product(&adx[..adxn], &minor[..mn], &mut term);
    s[..tn].copy_from_slice(&term[..tn]);
    s_len = tn;

    // Cofactor 2: ady · (bdz·cdx - bdx·cdz)
    let p1n = expansion_product(&bdz[..bdzn], &cdx[..cdxn], &mut p1);
    let p2n = expansion_product(&bdx[..bdxn], &cdz[..cdzn], &mut p2);
    let mn = expansion_diff(&p1[..p1n], &p2[..p2n], &mut minor);
    let tn = expansion_product(&ady[..adyn], &minor[..mn], &mut term);
    s_len = accumulate(&mut s, s_len, &term[..tn], &mut s2);

    // Cofactor 3: adz · (bdx·cdy - bdy·cdx)
    let p1n = expansion_product(&bdx[..bdxn], &cdy[..cdyn], &mut p1);
    let p2n = expansion_product(&bdy[..bdyn], &cdx[..cdxn], &mut p2);
    let mn = expansion_diff(&p1[..p1n], &p2[..p2n], &mut minor);
    let tn = expansion_product(&adz[..adzn], &minor[..mn], &mut term);
    s_len = accumulate(&mut s, s_len, &term[..tn], &mut s2);

    // S = adx·M1 + ady·M2 + adz·M3; orient3d's value is -S (existing sign
    // convention). The top component carries the exact sign.
    if s_len == 0 {
        0.0
    } else {
        -s[s_len - 1]
    }
}

/// Accumulate `s += addend` (zero-free expansions), using `scratch`. Returns the
/// new length of `s`.
fn accumulate(s: &mut [f64], s_len: usize, addend: &[f64], scratch: &mut [f64]) -> usize {
    if addend.is_empty() {
        return s_len;
    }
    if s_len == 0 {
        s[..addend.len()].copy_from_slice(addend);
        return addend.len();
    }
    let n = {
        // Copy `s`'s live prefix out so we can borrow it immutably while writing
        // `scratch`, then copy back.
        let cur: &[f64] = &s[..s_len];
        fast_expansion_sum_zeroelim(cur, addend, scratch)
    };
    s[..n].copy_from_slice(&scratch[..n]);
    n
}

/// Test orientation of four points in 3D
///
/// Returns the orientation of the tetrahedron (pa, pb, pc, pd):
/// - `CounterClockwise` if pd is below the plane of (pa, pb, pc) when viewed from above
/// - `Clockwise` if pd is above the plane
/// - `Collinear` if all four points are coplanar
///
/// The plane is oriented so that (pa, pb, pc) appear counter-clockwise when viewed
/// from the positive side of the plane.
pub fn orient3d(pa: &Point3, pb: &Point3, pc: &Point3, pd: &Point3) -> Orientation {
    let det = orient3d_fast(pa, pb, pc, pd);

    // Compute error bound
    let adx = (pa.x - pd.x).abs();
    let ady = (pa.y - pd.y).abs();
    let adz = (pa.z - pd.z).abs();
    let bdx = (pb.x - pd.x).abs();
    let bdy = (pb.y - pd.y).abs();
    let bdz = (pb.z - pd.z).abs();
    let cdx = (pc.x - pd.x).abs();
    let cdy = (pc.y - pd.y).abs();
    let cdz = (pc.z - pd.z).abs();

    let permanent = adx * (bdy * cdz + bdz * cdy)
        + ady * (bdz * cdx + bdx * cdz)
        + adz * (bdx * cdy + bdy * cdx);

    let errbound = O3DERRBOUNDSA * permanent;

    if det > errbound {
        Orientation::CounterClockwise
    } else if det < -errbound {
        Orientation::Clockwise
    } else {
        // Need exact test
        let det_exact = orient3d_adapt(pa, pb, pc, pd);
        if det_exact > 0.0 {
            Orientation::CounterClockwise
        } else if det_exact < 0.0 {
            Orientation::Clockwise
        } else {
            Orientation::Collinear
        }
    }
}

/// Fast approximate in-circle test
#[inline(always)]
fn incircle_fast(pa: &Vector2, pb: &Vector2, pc: &Vector2, pd: &Vector2) -> f64 {
    let adx = pa.x - pd.x;
    let ady = pa.y - pd.y;
    let bdx = pb.x - pd.x;
    let bdy = pb.y - pd.y;
    let cdx = pc.x - pd.x;
    let cdy = pc.y - pd.y;

    let abdet = adx * bdy - bdx * ady;
    let bcdet = bdx * cdy - cdx * bdy;
    let cadet = cdx * ady - adx * cdy;
    let alift = adx * adx + ady * ady;
    let blift = bdx * bdx + bdy * bdy;
    let clift = cdx * cdx + cdy * cdy;

    alift * bcdet + blift * cadet + clift * abdet
}

/// Exact addition `out = e + f` of two zero-free expansions; empty operand ⇒ copy.
fn sum_exp(e: &[f64], f: &[f64], out: &mut [f64]) -> usize {
    if e.is_empty() {
        out[..f.len()].copy_from_slice(f);
        return f.len();
    }
    if f.is_empty() {
        out[..e.len()].copy_from_slice(e);
        return e.len();
    }
    fast_expansion_sum_zeroelim(e, f, out)
}

/// Exact in-circle determinant value — its SIGN is correct for all finite inputs.
/// Builds the full expansion of `alift·bcdet + blift·cadet + clift·abdet`, where
/// each `*lift = *dx² + *dy²` and each sub-determinant carries the coordinate
/// roundoff tails (the bits the old adapt dropped). Generic exact fallback; runs
/// only when the A-filter is inconclusive.
fn incircle_adapt(pa: &Vector2, pb: &Vector2, pc: &Vector2, pd: &Vector2) -> f64 {
    let (mut adx, mut ady) = ([0.0; 2], [0.0; 2]);
    let (mut bdx, mut bdy) = ([0.0; 2], [0.0; 2]);
    let (mut cdx, mut cdy) = ([0.0; 2], [0.0; 2]);
    let adxn = diff_exp(pa.x, pd.x, &mut adx);
    let adyn = diff_exp(pa.y, pd.y, &mut ady);
    let bdxn = diff_exp(pb.x, pd.x, &mut bdx);
    let bdyn = diff_exp(pb.y, pd.y, &mut bdy);
    let cdxn = diff_exp(pc.x, pd.x, &mut cdx);
    let cdyn = diff_exp(pc.y, pd.y, &mut cdy);

    let mut u = [0.0f64; EXP_MAX];
    let mut v = [0.0f64; EXP_MAX];
    let mut lift = [0.0f64; EXP_MAX];
    let mut sub = [0.0f64; EXP_MAX];
    let mut term = [0.0f64; EXP_MAX];
    let mut sc = [0.0f64; EXP_MAX];
    let mut det = [0.0f64; EXP_MAX];
    let mut det_len = 0usize;

    // term 1: (adx² + ady²) · (bdx·cdy - cdx·bdy)
    let un = expansion_product(&adx[..adxn], &adx[..adxn], &mut u);
    let vn = expansion_product(&ady[..adyn], &ady[..adyn], &mut v);
    let lift_n = sum_exp(&u[..un], &v[..vn], &mut lift);
    let un = expansion_product(&bdx[..bdxn], &cdy[..cdyn], &mut u);
    let vn = expansion_product(&cdx[..cdxn], &bdy[..bdyn], &mut v);
    let sub_n = expansion_diff(&u[..un], &v[..vn], &mut sub);
    let term_n = expansion_product(&lift[..lift_n], &sub[..sub_n], &mut term);
    det_len = accumulate(&mut det, det_len, &term[..term_n], &mut sc);

    // term 2: (bdx² + bdy²) · (cdx·ady - adx·cdy)
    let un = expansion_product(&bdx[..bdxn], &bdx[..bdxn], &mut u);
    let vn = expansion_product(&bdy[..bdyn], &bdy[..bdyn], &mut v);
    let lift_n = sum_exp(&u[..un], &v[..vn], &mut lift);
    let un = expansion_product(&cdx[..cdxn], &ady[..adyn], &mut u);
    let vn = expansion_product(&adx[..adxn], &cdy[..cdyn], &mut v);
    let sub_n = expansion_diff(&u[..un], &v[..vn], &mut sub);
    let term_n = expansion_product(&lift[..lift_n], &sub[..sub_n], &mut term);
    det_len = accumulate(&mut det, det_len, &term[..term_n], &mut sc);

    // term 3: (cdx² + cdy²) · (adx·bdy - bdx·ady)
    let un = expansion_product(&cdx[..cdxn], &cdx[..cdxn], &mut u);
    let vn = expansion_product(&cdy[..cdyn], &cdy[..cdyn], &mut v);
    let lift_n = sum_exp(&u[..un], &v[..vn], &mut lift);
    let un = expansion_product(&adx[..adxn], &bdy[..bdyn], &mut u);
    let vn = expansion_product(&bdx[..bdxn], &ady[..adyn], &mut v);
    let sub_n = expansion_diff(&u[..un], &v[..vn], &mut sub);
    let term_n = expansion_product(&lift[..lift_n], &sub[..sub_n], &mut term);
    det_len = accumulate(&mut det, det_len, &term[..term_n], &mut sc);

    if det_len == 0 {
        0.0
    } else {
        det[det_len - 1]
    }
}

/// Test whether a point is inside the circle passing through three other points
///
/// Returns the location of pd relative to the circle through (pa, pb, pc):
/// - `Inside` if pd is inside the circle
/// - `Outside` if pd is outside the circle
/// - `OnBoundary` if pd is exactly on the circle
///
/// The three points (pa, pb, pc) must be in counter-clockwise order.
pub fn incircle(pa: &Vector2, pb: &Vector2, pc: &Vector2, pd: &Vector2) -> CircleLocation {
    let det = incircle_fast(pa, pb, pc, pd);

    // Compute error bound
    let adx = (pa.x - pd.x).abs();
    let ady = (pa.y - pd.y).abs();
    let bdx = (pb.x - pd.x).abs();
    let bdy = (pb.y - pd.y).abs();
    let cdx = (pc.x - pd.x).abs();
    let cdy = (pc.y - pd.y).abs();

    let permanent = (adx * adx + ady * ady) * (bdx * cdy + cdx * bdy)
        + (bdx * bdx + bdy * bdy) * (cdx * ady + adx * cdy)
        + (cdx * cdx + cdy * cdy) * (adx * bdy + bdx * ady);

    let errbound = ICCERRBOUNDSA * permanent;

    if det > errbound {
        CircleLocation::Inside
    } else if det < -errbound {
        CircleLocation::Outside
    } else {
        // Need exact test
        let det_exact = incircle_adapt(pa, pb, pc, pd);
        if det_exact > 0.0 {
            CircleLocation::Inside
        } else if det_exact < 0.0 {
            CircleLocation::Outside
        } else {
            CircleLocation::OnBoundary
        }
    }
}

// ── Heap (Vec) expansion arithmetic ─────────────────────────────────────────
// The in-sphere determinant's exact expansion reaches several thousand
// components — too large for the fixed `EXP_MAX` stack arrays — so insphere's
// exact fallback uses these `Vec`-returning helpers. Allocation cost is
// irrelevant: this path runs only when the A-filter is inconclusive (rare).

/// Scale a zero-free expansion `e` by `b`, exactly; empty ⇒ 0.
fn scale_v(e: &[f64], b: f64) -> Vec<f64> {
    if e.is_empty() {
        return Vec::new();
    }
    let mut h = vec![0.0f64; 2 * e.len() + 8];
    let n = scale_expansion_zeroelim(e, b, &mut h);
    h.truncate(n);
    h
}

/// Exact sum `e + f` of zero-free expansions.
fn sum_v(e: &[f64], f: &[f64]) -> Vec<f64> {
    if e.is_empty() {
        return f.to_vec();
    }
    if f.is_empty() {
        return e.to_vec();
    }
    let mut h = vec![0.0f64; e.len() + f.len() + 8];
    let n = fast_expansion_sum_zeroelim(e, f, &mut h);
    h.truncate(n);
    h
}

/// Exact difference `e - f` of zero-free expansions.
fn diff_v(e: &[f64], f: &[f64]) -> Vec<f64> {
    if f.is_empty() {
        return e.to_vec();
    }
    let negf: Vec<f64> = f.iter().map(|&c| -c).collect();
    sum_v(e, &negf)
}

/// Exact product `a · b` of zero-free expansions (`Σ scale(a, b[i])`).
fn product_v(a: &[f64], b: &[f64]) -> Vec<f64> {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    let mut acc: Vec<f64> = Vec::new();
    for &bi in b {
        let scaled = scale_v(a, bi);
        acc = if acc.is_empty() {
            scaled
        } else {
            sum_v(&acc, &scaled)
        };
    }
    acc
}

/// A coordinate difference `a - b` as a zero-free heap expansion.
fn diff_exp_v(a: f64, b: f64) -> Vec<f64> {
    let (x, y) = two_diff(a, b);
    let mut v = Vec::with_capacity(2);
    if y != 0.0 {
        v.push(y);
    }
    if x != 0.0 {
        v.push(x);
    }
    v
}

/// Exact in-sphere determinant value — its SIGN is correct for all finite inputs.
/// Mirrors `insphere_fast`'s formula
/// `dlift·abc - clift·dab + blift·cda - alift·bcd` exactly, carrying every
/// coordinate-difference and lift roundoff tail. Generic exact fallback; runs
/// only when the A-filter is inconclusive. (Replaces the former hardcoded
/// `RESULTERRBOUND` tolerance — the last and worst non-exact path.)
fn insphere_adapt(pa: &Point3, pb: &Point3, pc: &Point3, pd: &Point3, pe: &Point3) -> f64 {
    let aex = diff_exp_v(pa.x, pe.x);
    let aey = diff_exp_v(pa.y, pe.y);
    let aez = diff_exp_v(pa.z, pe.z);
    let bex = diff_exp_v(pb.x, pe.x);
    let bey = diff_exp_v(pb.y, pe.y);
    let bez = diff_exp_v(pb.z, pe.z);
    let cex = diff_exp_v(pc.x, pe.x);
    let cey = diff_exp_v(pc.y, pe.y);
    let cez = diff_exp_v(pc.z, pe.z);
    let dex = diff_exp_v(pd.x, pe.x);
    let dey = diff_exp_v(pd.y, pe.y);
    let dez = diff_exp_v(pd.z, pe.z);

    // 2x2 minors (xy)
    let ab = diff_v(&product_v(&aex, &bey), &product_v(&bex, &aey));
    let bc = diff_v(&product_v(&bex, &cey), &product_v(&cex, &bey));
    let cd = diff_v(&product_v(&cex, &dey), &product_v(&dex, &cey));
    let da = diff_v(&product_v(&dex, &aey), &product_v(&aex, &dey));
    let ac = diff_v(&product_v(&aex, &cey), &product_v(&cex, &aey));
    let bd = diff_v(&product_v(&bex, &dey), &product_v(&dex, &bey));

    // 3x3 cofactors (with z)
    let abc = sum_v(
        &diff_v(&product_v(&aez, &bc), &product_v(&bez, &ac)),
        &product_v(&cez, &ab),
    );
    let bcd = sum_v(
        &diff_v(&product_v(&bez, &cd), &product_v(&cez, &bd)),
        &product_v(&dez, &bc),
    );
    let cda = sum_v(
        &sum_v(&product_v(&cez, &da), &product_v(&dez, &ac)),
        &product_v(&aez, &cd),
    );
    let dab = sum_v(
        &sum_v(&product_v(&dez, &ab), &product_v(&aez, &bd)),
        &product_v(&bez, &da),
    );

    // lifts = x² + y² + z²
    let lift = |x: &[f64], y: &[f64], z: &[f64]| -> Vec<f64> {
        sum_v(&sum_v(&product_v(x, x), &product_v(y, y)), &product_v(z, z))
    };
    let alift = lift(&aex, &aey, &aez);
    let blift = lift(&bex, &bey, &bez);
    let clift = lift(&cex, &cey, &cez);
    let dlift = lift(&dex, &dey, &dez);

    // det = dlift·abc - clift·dab + blift·cda - alift·bcd
    let pos = sum_v(&product_v(&dlift, &abc), &product_v(&blift, &cda));
    let neg = sum_v(&product_v(&clift, &dab), &product_v(&alift, &bcd));
    let det = diff_v(&pos, &neg);

    if det.is_empty() {
        0.0
    } else {
        det[det.len() - 1]
    }
}

/// Fast approximate in-sphere test
#[inline(always)]
fn insphere_fast(pa: &Point3, pb: &Point3, pc: &Point3, pd: &Point3, pe: &Point3) -> f64 {
    let aex = pa.x - pe.x;
    let aey = pa.y - pe.y;
    let aez = pa.z - pe.z;
    let bex = pb.x - pe.x;
    let bey = pb.y - pe.y;
    let bez = pb.z - pe.z;
    let cex = pc.x - pe.x;
    let cey = pc.y - pe.y;
    let cez = pc.z - pe.z;
    let dex = pd.x - pe.x;
    let dey = pd.y - pe.y;
    let dez = pd.z - pe.z;

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

    dlift * abc - clift * dab + blift * cda - alift * bcd
}

/// Test whether a point is inside the sphere passing through four other points
///
/// Returns the location of pe relative to the sphere through (pa, pb, pc, pd):
/// - `Inside` if pe is inside the sphere
/// - `Outside` if pe is outside the sphere
/// - `OnBoundary` if pe is exactly on the sphere
///
/// The four points (pa, pb, pc, pd) must be oriented so that they have positive
/// orientation when viewed from outside the sphere.
pub fn insphere(pa: &Point3, pb: &Point3, pc: &Point3, pd: &Point3, pe: &Point3) -> CircleLocation {
    let det = insphere_fast(pa, pb, pc, pd, pe);

    // Compute error bound (simplified version)
    let aex = (pa.x - pe.x).abs();
    let aey = (pa.y - pe.y).abs();
    let aez = (pa.z - pe.z).abs();
    let bex = (pb.x - pe.x).abs();
    let bey = (pb.y - pe.y).abs();
    let bez = (pb.z - pe.z).abs();
    let cex = (pc.x - pe.x).abs();
    let cey = (pc.y - pe.y).abs();
    let cez = (pc.z - pe.z).abs();
    let dex = (pd.x - pe.x).abs();
    let dey = (pd.y - pe.y).abs();
    let dez = (pd.z - pe.z).abs();

    let aezplus = aez.abs();
    let bezplus = bez.abs();
    let cezplus = cez.abs();
    let dezplus = dez.abs();
    let aexbeyplus = aex * bey + bex * aey;
    let bexceyplus = bex * cey + cex * bey;
    let cexdeyplus = cex * dey + dex * cey;
    let dexaeyplus = dex * aey + aex * dey;
    let aexceyplus = aex * cey + cex * aey;
    let bexdeyplus = bex * dey + dex * bey;

    let permanent = ((aexbeyplus * cezplus + cexdeyplus * aezplus)
        + (bexceyplus * dezplus + dexaeyplus * bezplus))
        + ((aexceyplus * bezplus + bexdeyplus * aezplus)
            + (cexdeyplus * bezplus + dexaeyplus * cezplus));

    let errbound = ISPERRBOUNDSA * permanent;

    if det > errbound {
        CircleLocation::Inside
    } else if det < -errbound {
        CircleLocation::Outside
    } else {
        // Exact expansion-arithmetic refinement — resolves points cocircular
        // up to the last bit (replaces the former RESULTERRBOUND tolerance).
        let det_exact = insphere_adapt(pa, pb, pc, pd, pe);
        if det_exact > 0.0 {
            CircleLocation::Inside
        } else if det_exact < 0.0 {
            CircleLocation::Outside
        } else {
            CircleLocation::OnBoundary
        }
    }
}

/// Shared exact-shoelace core: the EXACT value of the (unhalved) shoelace sum
/// `Σ_i (x_i·y_{i+1} − x_{i+1}·y_i)` of the closed polygon whose `i`-th vertex
/// is `at(i)`, as a zero-free expansion (low→high). An empty expansion is an
/// exact zero; fewer than 3 vertices is defined as zero (degenerate polygon).
fn shoelace_expansion<F: Fn(usize) -> (f64, f64)>(n: usize, at: F) -> Vec<f64> {
    if n < 3 {
        return Vec::new();
    }
    let mut acc: Vec<f64> = Vec::new();
    for i in 0..n {
        let (px, py) = at(i);
        let (qx, qy) = at((i + 1) % n);
        // term = x_i·y_{i+1} − x_{i+1}·y_i, exactly (a 4-component expansion).
        let (h1, l1) = two_product(px, qy);
        let (h2, l2) = two_product(qx, py);
        let term: Vec<f64> = two_two_diff(h1, l1, h2, l2)
            .into_iter()
            .filter(|&c| c != 0.0)
            .collect();
        if term.is_empty() {
            continue;
        }
        acc = if acc.is_empty() {
            term
        } else {
            sum_v(&acc, &term)
        };
    }
    acc
}

/// Shared exact-shoelace sign: orientation of the closed polygon whose `i`-th
/// vertex is `at(i)`, from the exact expansion sign of
/// `Σ_i (x_i·y_{i+1} − x_{i+1}·y_i)`.
fn shoelace_orientation<F: Fn(usize) -> (f64, f64)>(n: usize, at: F) -> Orientation {
    let acc = shoelace_expansion(n, at);
    match acc.last() {
        None => Orientation::Collinear,
        Some(&top) => orientation_of(top),
    }
}

/// EXACT comparison of two polygons' ABSOLUTE areas: `|area(a)| ⋚ |area(b)|`
/// with no subtraction rounding (EXACT PREDICATES Slice 3; spec §2.3 row #8
/// "exact area comparison via expansion difference").
///
/// Both shoelace sums are built as exact expansions, the sign carrier of each
/// is folded to `|·|` by negating the whole expansion when its most
/// significant component is negative, and the comparison is the exact sign of
/// the expansion difference. The ½ factor of the shoelace formula cancels in
/// the comparison and is not applied. Polygons with fewer than 3 vertices
/// compare as exact zero area (matching the raw-f64 closures this replaces).
///
/// The winding direction of either polygon does not affect the result — this
/// is a pure magnitude question decided exactly. Callers that need the SIGN
/// use [`polygon_orientation_2d`]; fusing the two questions into one float
/// compare is the census row-#7/#8 defect class this predicate retires.
pub fn polygon_area_cmp_2d(a: &[(f64, f64)], b: &[(f64, f64)]) -> CmpOrdering {
    let abs_shoelace = |pts: &[(f64, f64)]| -> Vec<f64> {
        let mut e = shoelace_expansion(pts.len(), |i| pts[i]);
        // The most significant component carries the sign of the whole
        // expansion (nonoverlapping property); negating every component
        // negates the represented value exactly.
        if e.last().is_some_and(|&top| top < 0.0) {
            for c in &mut e {
                *c = -*c;
            }
        }
        e
    };
    let ea = abs_shoelace(a);
    let eb = abs_shoelace(b);
    expansion_diff_sign(&ea, &eb)
}

// ── Exact circular (angular) order of direction vectors ─────────────────────

/// Angular half-plane of a direction vector, splitting the CCW circle at the
/// positive x-axis: `1` for angles in `[0, π)` (upper half, +x axis included),
/// `2` for `[π, 2π)` (lower half, −x axis included), `0` for the zero vector
/// (which sorts before every nonzero direction, deterministically). The split
/// is chosen so that two collinear vectors in the SAME half are necessarily
/// the SAME direction (opposite directions always land in different halves).
#[inline]
fn angular_half(v: &Vector2) -> u8 {
    if v.x == 0.0 && v.y == 0.0 {
        0
    } else if v.y > 0.0 || (v.y == 0.0 && v.x > 0.0) {
        1
    } else {
        2
    }
}

/// EXACT circular order of two 2D direction vectors: the total order of CCW
/// angle from the positive x-axis, `Less` when `u`'s angle is strictly
/// smaller than `v`'s (EXACT PREDICATES Slice 3; spec §3.6 `circular_order`).
///
/// No `atan2`, no NaN arm: the coarse comparison is the angular half-plane
/// ([`angular_half`] — the classical quadrant-split construction for sorting
/// by polar angle without trigonometry; cf. de Berg et al. 2008 §2 rotational
/// orders, CGAL `Direction_2` comparison), and within a half the order is the
/// EXACT sign of the cross product `u × v` via [`orient2d`] (Shewchuk 1997).
/// `Equal` therefore means the two vectors point in EXACTLY the same
/// direction (positive scalar multiples) — a true tie the caller resolves
/// with its own deterministic tie-break — never a rounding artifact and never
/// a NaN-swallowing fallback, which as a sort comparator violates strict weak
/// ordering (the `boolean.rs`/`face_arrangement.rs` NaN→`Equal` arm this
/// replaces).
///
/// Magnitudes are irrelevant (only the direction participates), so callers
/// may pass unnormalized tangents. Zero vectors order before every nonzero
/// direction and equal to each other. Inputs must be finite (the DCEL callers
/// filter degenerate tangents before sorting).
pub fn circular_order(u: &Vector2, v: &Vector2) -> CmpOrdering {
    let hu = angular_half(u);
    let hv = angular_half(v);
    match hu.cmp(&hv) {
        CmpOrdering::Equal => {
            // Same half ⇒ the angle difference is within (−π, π), so the
            // exact cross sign decides: u × v > 0 ⇔ u is angularly before v.
            match orient2d(&Vector2::ZERO, u, v) {
                Orientation::CounterClockwise => CmpOrdering::Less,
                Orientation::Clockwise => CmpOrdering::Greater,
                Orientation::Collinear => CmpOrdering::Equal,
            }
        }
        other => other,
    }
}

// ── Exact 3D point-vs-plane sidedness ────────────────────────────────────────

// A-filters for the plane-evaluation predicates (same construction as the
// Shewchuk bounds above: a first-order forward-error bound over the term-
// magnitude sum, with generous slack so the filter is conservative — slack
// only sends more near-degenerate inputs to the exact expansion path, never
// lets a wrong f64 sign through).
//
// sign_of_plane_eval computes fl(n·p − d): 3 product roundings (each ≤ ε·|tᵢ|)
// plus 3 additions/subtractions (each ≤ ε·T(1+ε)³ where T = Σ|tᵢ| + |d|), so
// the true error is < 6εT + O(ε²); (8 + 64ε)ε covers it with margin.
const PLANE_EVAL_ERRBOUND: f64 = (8.0 + 64.0 * EPS) * EPS;
// point_plane_sidedness computes fl(Σ nᵢ·fl(pᵢ−oᵢ)): each term carries one
// difference rounding and one product rounding (≤ 2ε·|tᵢ| + O(ε²)) plus 2
// additions (≤ ε·T each), so the true error is < 4εT + O(ε²); (6 + 48ε)ε
// covers it with margin. Both bounds are oracle-gated in
// `tests/adversarial_predicate_census.rs` (BigRational plane eval).
const POINT_PLANE_ERRBOUND: f64 = (6.0 + 48.0 * EPS) * EPS;

#[inline]
fn ordering_of_sign(x: f64) -> CmpOrdering {
    if x > 0.0 {
        CmpOrdering::Greater
    } else if x < 0.0 {
        CmpOrdering::Less
    } else {
        CmpOrdering::Equal
    }
}

/// Exact sign of the two-expansion difference `e − f` (both zero-free).
fn expansion_diff_sign(e: &[f64], f: &[f64]) -> CmpOrdering {
    let d = diff_v(e, f);
    match d.last() {
        None => CmpOrdering::Equal,
        Some(&top) => ordering_of_sign(top),
    }
}

/// EXACT sign of the plane evaluation `n·p − d` (EXACT PREDICATES Slice 4;
/// spec §3.2 formulation 2): `Greater` ⇔ `p` is strictly on the positive side
/// of the plane `{x : n·x = d}`, `Equal` ⇔ exactly on it, for all finite
/// inputs.
///
/// The DECISION is exact in the plane's REPRESENTED data `(n, d)` — the
/// stored normal and offset are taken at face value (an analytic `Plane`
/// surface's own carrier is authoritative for that face, per the spec's
/// carrier rule). Staged evaluation: an f64 evaluation guarded by the static
/// `PLANE_EVAL_ERRBOUND` A-filter, falling back to exact expansion
/// arithmetic (Shewchuk 1997) when the filter cannot certify the sign.
pub fn sign_of_plane_eval(n: &Vector3, d: f64, p: &Point3) -> CmpOrdering {
    let t1 = n.x * p.x;
    let t2 = n.y * p.y;
    let t3 = n.z * p.z;
    let det = (t1 + t2 + t3) - d;
    let mag = t1.abs() + t2.abs() + t3.abs() + d.abs();
    let errbound = PLANE_EVAL_ERRBOUND * mag;
    if det > errbound || -det > errbound {
        return ordering_of_sign(det);
    }
    // Exact path: n·p as a zero-free expansion, compared against d.
    let mut prod = [[0.0f64; 2]; 3];
    let mut lens = [0usize; 3];
    for (k, (nc, pc)) in [(n.x, p.x), (n.y, p.y), (n.z, p.z)].into_iter().enumerate() {
        let (hi, lo) = two_product(nc, pc);
        let mut m = 0usize;
        if lo != 0.0 {
            prod[k][m] = lo;
            m += 1;
        }
        if hi != 0.0 {
            prod[k][m] = hi;
            m += 1;
        }
        lens[k] = m;
    }
    let mut s12 = [0.0f64; 4];
    let n12 = sum_exp(&prod[0][..lens[0]], &prod[1][..lens[1]], &mut s12);
    let mut s123 = [0.0f64; 6];
    let n123 = sum_exp(&s12[..n12], &prod[2][..lens[2]], &mut s123);
    let dexp: &[f64] = if d != 0.0 { &[d] } else { &[] };
    expansion_diff_sign(&s123[..n123], dexp)
}

/// EXACT sidedness of point `p` against the plane through `o` with normal `n`:
/// the exact sign of `n·(p − o)` for all finite inputs (EXACT PREDICATES
/// Slice 4). `Greater` ⇔ strictly on the normal side, `Equal` ⇔ exactly on
/// the plane.
///
/// This is the point-anchored sibling of [`sign_of_plane_eval`] for carriers
/// stored as (anchor point, normal) — e.g. a trim circle's (centre, axis) —
/// where forming `d = n·o` in f64 first would itself round. Exact in the
/// stored `(n, o)`; when `n`/`o` are themselves evaluated from approximate
/// geometry the answer is exactly the sign of the approximate quantity
/// (spec §3.0 — the derived-input caveat is the caller's to document).
pub fn point_plane_sidedness(n: &Vector3, o: &Point3, p: &Point3) -> CmpOrdering {
    let dx = p.x - o.x;
    let dy = p.y - o.y;
    let dz = p.z - o.z;
    let t1 = n.x * dx;
    let t2 = n.y * dy;
    let t3 = n.z * dz;
    let det = t1 + t2 + t3;
    let mag = t1.abs() + t2.abs() + t3.abs();
    let errbound = POINT_PLANE_ERRBOUND * mag;
    if det > errbound || -det > errbound {
        return ordering_of_sign(det);
    }
    // Exact path: each coordinate difference as a 2-component expansion,
    // scaled by its normal component, summed — all exact.
    let (mut dxe, mut dye, mut dze) = ([0.0; 2], [0.0; 2], [0.0; 2]);
    let dxn = diff_exp(p.x, o.x, &mut dxe);
    let dyn_ = diff_exp(p.y, o.y, &mut dye);
    let dzn = diff_exp(p.z, o.z, &mut dze);
    let mut tx = [0.0f64; 4];
    let mut ty = [0.0f64; 4];
    let mut tz = [0.0f64; 4];
    let txn = scale_nonempty(&dxe[..dxn], n.x, &mut tx);
    let tyn = scale_nonempty(&dye[..dyn_], n.y, &mut ty);
    let tzn = scale_nonempty(&dze[..dzn], n.z, &mut tz);
    let mut sxy = [0.0f64; 8];
    let nxy = sum_exp(&tx[..txn], &ty[..tyn], &mut sxy);
    let mut sxyz = [0.0f64; 12];
    let nxyz = sum_exp(&sxy[..nxy], &tz[..tzn], &mut sxyz);
    match sxyz[..nxyz].last() {
        None => CmpOrdering::Equal,
        Some(&top) => ordering_of_sign(top),
    }
}

/// [`scale_expansion_zeroelim`] tolerating empty/zero operands (exact 0).
fn scale_nonempty(e: &[f64], b: f64, h: &mut [f64]) -> usize {
    if e.is_empty() || b == 0.0 {
        return 0;
    }
    let n = scale_expansion_zeroelim(e, b, h);
    // scale_expansion_zeroelim writes q even when zero if nothing else was
    // emitted; strip a lone exact zero so "empty ⇒ exact 0" stays canonical.
    if n == 1 && h[0] == 0.0 {
        0
    } else {
        n
    }
}

/// Exact orientation of a closed polygon from the SIGN of its shoelace signed
/// area `Σ_i (x_i·y_{i+1} − x_{i+1}·y_i)`, computed in exact expansion
/// arithmetic. `CounterClockwise` = positive area, `Clockwise` = negative,
/// `Collinear` = exactly zero (degenerate / self-cancelling). Correct for all
/// finite inputs — the tolerance-free basis for winding/orientation tests that
/// today compare an f64 signed area against `0.0`.
pub fn signed_area_2d(points: &[Vector2]) -> Orientation {
    shoelace_orientation(points.len(), |i| (points[i].x, points[i].y))
}

/// [`signed_area_2d`] over raw `(x, y)` tuples — the coordinate form the
/// boolean/section/tessellation pipelines carry their projected polygons in
/// (EXACT PREDICATES Slice 2; spec §3.6 `signed_area_pairs`).
pub fn polygon_orientation_2d(pts: &[(f64, f64)]) -> Orientation {
    shoelace_orientation(pts.len(), |i| pts[i])
}

/// Shared exact even-odd point-in-polygon core (EXACT PREDICATES Slice 2 —
/// the single entry point replacing the five raw ray-cast copies of census
/// row #10; hoisted from `operations/polygon_clip.rs`).
///
/// Standard half-open +x ray cast: an edge participates iff it strictly
/// straddles the ray's y (exact f64 comparisons), and whether it crosses to
/// the RIGHT of `p` is the exact [`orient2d`] side of `p` against the
/// directed edge — no `x_cross` division to round across the query point.
/// An upward edge crosses right iff `p` is strictly left (CCW); a downward
/// edge iff strictly right (CW). `Collinear` (p exactly on a straddling
/// edge's carrier) contributes no crossing: boundary points are not
/// classified as interior, deterministically (Regime-E boundary contract:
/// exact combinatorics never silently absorbs coincidence — callers that
/// need an on-boundary verdict test it explicitly, in Regime T).
pub(crate) fn point_in_polygon_2d_by<F: Fn(usize) -> (f64, f64)>(
    n: usize,
    at: F,
    px: f64,
    py: f64,
) -> bool {
    if n < 3 {
        return false;
    }
    let p = Vector2::new(px, py);
    let mut inside = false;
    for i in 0..n {
        let (ax, ay) = at(i);
        let (bx, by) = at((i + 1) % n);
        if (ay > py) != (by > py) {
            let upward = by > ay;
            let o = orient2d(&Vector2::new(ax, ay), &Vector2::new(bx, by), &p);
            let crosses_right = match o {
                Orientation::CounterClockwise => upward,
                Orientation::Clockwise => !upward,
                Orientation::Collinear => false,
            };
            if crosses_right {
                inside = !inside;
            }
        }
    }
    inside
}

/// Exact even-odd point-in-polygon over raw `(x, y)` tuples. See
/// [`point_in_polygon_2d_by`] for the crossing rule and boundary semantics.
/// The polygon is closed implicitly (last vertex connects back to the first);
/// fewer than 3 vertices ⇒ `false`.
pub fn point_in_polygon_2d(px: f64, py: f64, poly: &[(f64, f64)]) -> bool {
    point_in_polygon_2d_by(poly.len(), |i| poly[i], px, py)
}

/// Exact proper (interiors-cross) 2D segment intersection test between
/// `p1→p2` and `p3→p4`, built from four exact [`orient2d`] signs (EXACT
/// PREDICATES Slice 2; spec §3.6 `segments_intersect_2d`).
///
/// `true` iff the endpoints of each segment lie STRICTLY on opposite sides
/// of the other segment's carrier line. Collinear configurations and
/// endpoint touches report `false` — by design (they are coincidence
/// questions, not crossing-existence questions; the caller's edge-sharing
/// contract in `operations/boolean.rs` depends on this).
pub fn segments_properly_intersect_2d(
    p1: (f64, f64),
    p2: (f64, f64),
    p3: (f64, f64),
    p4: (f64, f64),
) -> bool {
    let a = Vector2::new(p1.0, p1.1);
    let b = Vector2::new(p2.0, p2.1);
    let c = Vector2::new(p3.0, p3.1);
    let d = Vector2::new(p4.0, p4.1);
    let strictly_opposite = |u: Orientation, v: Orientation| -> bool {
        matches!(
            (u, v),
            (Orientation::CounterClockwise, Orientation::Clockwise)
                | (Orientation::Clockwise, Orientation::CounterClockwise)
        )
    };
    strictly_opposite(orient2d(&c, &d, &a), orient2d(&c, &d, &b))
        && strictly_opposite(orient2d(&a, &b, &c), orient2d(&a, &b, &d))
}

/// Robust version of orient2d that handles special cases
///
/// This version includes additional checks for numerical stability
/// and handles edge cases like very small triangles.
pub fn orient2d_robust(pa: &Vector2, pb: &Vector2, pc: &Vector2) -> Orientation {
    // Check for exact duplicates first
    if pa == pb || pb == pc || pc == pa {
        return Orientation::Collinear;
    }

    // Use the standard exact predicate
    orient2d(pa, pb, pc)
}

/// Robust version of orient3d that handles special cases
///
/// This version includes additional checks for numerical stability
/// and handles edge cases like very small tetrahedra.
pub fn orient3d_robust(pa: &Point3, pb: &Point3, pc: &Point3, pd: &Point3) -> Orientation {
    // Check for exact duplicates first
    if pa == pb || pa == pc || pa == pd || pb == pc || pb == pd || pc == pd {
        return Orientation::Collinear;
    }

    // Use the standard exact predicate
    orient3d(pa, pb, pc, pd)
}

/// Check if four 2D points are cocircular
pub fn cocircular(pa: &Vector2, pb: &Vector2, pc: &Vector2, pd: &Vector2) -> bool {
    matches!(incircle(pa, pb, pc, pd), CircleLocation::OnBoundary)
}

/// Check if five 3D points are cospherical  
pub fn cospherical(pa: &Point3, pb: &Point3, pc: &Point3, pd: &Point3, pe: &Point3) -> bool {
    matches!(insphere(pa, pb, pc, pd, pe), CircleLocation::OnBoundary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::consts;

    #[test]
    fn test_orient2d_basic() {
        let a = Vector2::new(0.0, 0.0);
        let b = Vector2::new(1.0, 0.0);
        let c = Vector2::new(0.0, 1.0);

        // Counter-clockwise triangle
        assert_eq!(orient2d(&a, &b, &c), Orientation::CounterClockwise);

        // Clockwise triangle (reversed)
        assert_eq!(orient2d(&a, &c, &b), Orientation::Clockwise);

        // Collinear points
        let d = Vector2::new(2.0, 0.0);
        assert_eq!(orient2d(&a, &b, &d), Orientation::Collinear);
    }

    #[test]
    fn test_orient2d_near_collinear() {
        let a = Vector2::new(0.0, 0.0);
        let b = Vector2::new(1.0, 0.0);
        let c = Vector2::new(0.5, 1e-16); // Very close to collinear

        // Should still detect the slight counter-clockwise orientation
        let result = orient2d(&a, &b, &c);
        assert_eq!(result, Orientation::CounterClockwise);
    }

    #[test]
    fn test_orient3d_basic() {
        let a = Point3::new(0.0, 0.0, 0.0);
        let b = Point3::new(1.0, 0.0, 0.0);
        let c = Point3::new(0.0, 1.0, 0.0);
        let d = Point3::new(0.0, 0.0, 1.0);

        // Positive orientation (d is above the plane abc)
        assert_eq!(orient3d(&a, &b, &c, &d), Orientation::CounterClockwise);

        // Negative orientation (d is below)
        let d_below = Point3::new(0.0, 0.0, -1.0);
        assert_eq!(orient3d(&a, &b, &c, &d_below), Orientation::Clockwise);

        // Coplanar points
        let d_coplanar = Point3::new(0.5, 0.5, 0.0);
        assert_eq!(orient3d(&a, &b, &c, &d_coplanar), Orientation::Collinear);
    }

    #[test]
    fn test_incircle_basic() {
        // Unit circle centered at origin
        let a = Vector2::new(1.0, 0.0);
        let b = Vector2::new(0.0, 1.0);
        let c = Vector2::new(-1.0, 0.0);

        // Point inside
        let inside = Vector2::new(0.0, 0.0);
        assert_eq!(incircle(&a, &b, &c, &inside), CircleLocation::Inside);

        // Point outside
        let outside = Vector2::new(2.0, 0.0);
        assert_eq!(incircle(&a, &b, &c, &outside), CircleLocation::Outside);

        // Point on boundary
        let on_boundary = Vector2::new(0.0, -1.0);
        assert_eq!(
            incircle(&a, &b, &c, &on_boundary),
            CircleLocation::OnBoundary
        );
    }

    #[test]
    fn test_insphere_basic() {
        // Unit sphere centered at origin
        let a = Point3::new(1.0, 0.0, 0.0);
        let b = Point3::new(0.0, 1.0, 0.0);
        let c = Point3::new(0.0, 0.0, 1.0);
        let d = Point3::new(-1.0, 0.0, 0.0);

        // Point inside
        let inside = Point3::new(0.0, 0.0, 0.0);
        assert_eq!(insphere(&a, &b, &c, &d, &inside), CircleLocation::Inside);

        // Point outside
        let outside = Point3::new(2.0, 0.0, 0.0);
        assert_eq!(insphere(&a, &b, &c, &d, &outside), CircleLocation::Outside);
    }

    #[test]
    fn test_exact_arithmetic() {
        // Test two_product
        let (hi, lo) = two_product(3.0, 7.0);
        assert_eq!(hi, 21.0);
        assert!(lo.abs() < consts::EPSILON);

        // Test two_sum: lo captures roundoff error
        let (hi, _lo) = two_sum(1e16, 1.0);
        assert_eq!(hi, 1e16 + 1.0);
    }

    #[test]
    fn test_robustness() {
        // Test with very small triangle
        let a = Vector2::new(0.0, 0.0);
        let b = Vector2::new(1e-15, 0.0);
        let c = Vector2::new(0.0, 1e-15);

        let result = orient2d_robust(&a, &b, &c);
        assert_eq!(result, Orientation::CounterClockwise);

        // Test with duplicate points
        assert_eq!(orient2d_robust(&a, &a, &c), Orientation::Collinear);
    }

    #[test]
    fn test_consistency() {
        // orient2d should be antisymmetric
        let a = Vector2::new(1.0, 2.0);
        let b = Vector2::new(3.0, 1.0);
        let c = Vector2::new(2.0, 4.0);

        let abc = orient2d(&a, &b, &c);
        let acb = orient2d(&a, &c, &b);

        match (abc, acb) {
            (Orientation::CounterClockwise, Orientation::Clockwise) => {}
            (Orientation::Clockwise, Orientation::CounterClockwise) => {}
            (Orientation::Collinear, Orientation::Collinear) => {}
            _ => panic!("Orientation test not antisymmetric"),
        }
    }

    #[test]
    fn test_stress_random() {
        // Test with many random points
        let mut rng = 12345u64; // Simple LCG for reproducibility

        for _ in 0..1000 {
            // Generate random points using LCG
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let x1 = (rng as f64 / u64::MAX as f64) * 20.0 - 10.0;
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let y1 = (rng as f64 / u64::MAX as f64) * 20.0 - 10.0;
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let x2 = (rng as f64 / u64::MAX as f64) * 20.0 - 10.0;
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let y2 = (rng as f64 / u64::MAX as f64) * 20.0 - 10.0;
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let x3 = (rng as f64 / u64::MAX as f64) * 20.0 - 10.0;
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let y3 = (rng as f64 / u64::MAX as f64) * 20.0 - 10.0;

            let a = Vector2::new(x1, y1);
            let b = Vector2::new(x2, y2);
            let c = Vector2::new(x3, y3);

            // Just ensure it doesn't crash and returns valid result
            let result = orient2d(&a, &b, &c);
            match result {
                Orientation::CounterClockwise | Orientation::Clockwise | Orientation::Collinear => {
                }
            }
        }
    }

    #[test]
    fn test_special_cases() {
        // Test with points at infinity-like values
        let large = 1e100;
        let a = Vector2::new(large, 0.0);
        let b = Vector2::new(0.0, large);
        let c = Vector2::new(-large, 0.0);

        // Should still work correctly
        let result = orient2d(&a, &b, &c);
        assert_eq!(result, Orientation::CounterClockwise);

        // Test with very close points
        let epsilon = f64::EPSILON;
        let a = Vector2::new(0.0, 0.0);
        let b = Vector2::new(epsilon, 0.0);
        let c = Vector2::new(0.0, epsilon);

        let result = orient2d(&a, &b, &c);
        assert_eq!(result, Orientation::CounterClockwise);
    }

    #[test]
    fn point_in_polygon_2d_basic_square_and_concave() {
        let square: Vec<(f64, f64)> = vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        assert!(point_in_polygon_2d(0.5, 0.5, &square));
        assert!(!point_in_polygon_2d(1.5, 0.5, &square));
        assert!(!point_in_polygon_2d(-0.5, 0.5, &square));
        // Degenerate input: < 3 vertices is "no containment".
        assert!(!point_in_polygon_2d(0.5, 0.5, &[(0.0, 0.0), (1.0, 1.0)]));

        // Concave L-shape (mirrors the boolean.rs regression tests).
        let l: Vec<(f64, f64)> = vec![
            (0.0, 0.0),
            (3.0, 0.0),
            (3.0, 1.0),
            (1.0, 1.0),
            (1.0, 3.0),
            (0.0, 3.0),
        ];
        assert!(point_in_polygon_2d(0.5, 0.5, &l), "in horizontal leg");
        assert!(point_in_polygon_2d(0.5, 2.5, &l), "in vertical leg");
        assert!(!point_in_polygon_2d(2.0, 2.0, &l), "in the notch");
        assert!(!point_in_polygon_2d(5.0, 5.0, &l), "outside bbox");
    }

    /// The adversarial-census edge-graze cases (rational-oracle truth; see
    /// `tests/adversarial_predicate_census.rs`): the raw division-based ray
    /// casts get BOTH of these wrong — the exact crossing side must not.
    #[test]
    fn point_in_polygon_2d_exact_on_census_edge_grazes() {
        let poly_a: Vec<(f64, f64)> = vec![
            (7.225625928452673, 2.7768500608312636),
            (6.072631636028047, 2.672881766295894),
            (-2.5291854002345886, 4.638069284829017),
            (-7.650869387878803, 2.918458308232573),
            (8.586012336985753, -2.444888441654245),
        ];
        assert!(
            !point_in_polygon_2d(2.5467869206826865, -0.45001894902491996, &poly_a),
            "oracle truth: OUTSIDE"
        );

        let poly_b: Vec<(f64, f64)> = vec![
            (-1.239582485255542, 7.479570865958778),
            (-4.238166112769274, 7.81764288716935),
            (-5.78909933898001, -5.872542076305808),
        ];
        assert!(
            point_in_polygon_2d(-5.540080621045283, -5.141711495058385, &poly_b),
            "oracle truth: INSIDE"
        );
    }

    #[test]
    fn polygon_orientation_2d_matches_signed_area_2d() {
        let ccw: Vec<(f64, f64)> = vec![(0.0, 0.0), (2.0, 0.0), (2.0, 2.0)];
        assert_eq!(polygon_orientation_2d(&ccw), Orientation::CounterClockwise);
        let cw: Vec<(f64, f64)> = ccw.iter().rev().copied().collect();
        assert_eq!(polygon_orientation_2d(&cw), Orientation::Clockwise);
        // Exactly self-cancelling (zero-area) polygon.
        let flat: Vec<(f64, f64)> = vec![(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)];
        assert_eq!(polygon_orientation_2d(&flat), Orientation::Collinear);
        // Tuple form must agree with the Vector2 form on a dirty polygon.
        let dirty: Vec<(f64, f64)> = vec![(0.1, 0.7), (5.3, 0.21), (4.9, 3.33), (1.7, 2.9)];
        let as_vec: Vec<Vector2> = dirty.iter().map(|&(x, y)| Vector2::new(x, y)).collect();
        assert_eq!(polygon_orientation_2d(&dirty), signed_area_2d(&as_vec));
    }

    /// Near-cancelling ulp-quad from the census (raw f64 shoelace reports
    /// exactly 0.0; the exact expansion sign is positive).
    #[test]
    fn polygon_orientation_2d_exact_on_census_ulp_quad() {
        let quad: Vec<(f64, f64)> = vec![
            (-2.432547292592431, 5.076801557850267),
            (-5.027858927402571, -1.3796565715472004),
            (-5.027858927402571, -1.3796565715472011),
            (-2.432547292592431, 5.076801557850266),
        ];
        assert_eq!(
            polygon_orientation_2d(&quad),
            Orientation::CounterClockwise,
            "rational-oracle truth: positive area; raw f64 shoelace cancels to 0.0"
        );
    }

    #[test]
    fn segments_properly_intersect_2d_basic() {
        // Clean X crossing.
        assert!(segments_properly_intersect_2d(
            (0.0, 0.0),
            (2.0, 2.0),
            (0.0, 2.0),
            (2.0, 0.0)
        ));
        // Disjoint.
        assert!(!segments_properly_intersect_2d(
            (0.0, 0.0),
            (1.0, 0.0),
            (0.0, 1.0),
            (1.0, 1.0)
        ));
        // Endpoint touch is NOT a proper crossing (documented contract).
        assert!(!segments_properly_intersect_2d(
            (0.0, 0.0),
            (1.0, 1.0),
            (1.0, 1.0),
            (2.0, 0.0)
        ));
        // Collinear overlap is NOT a proper crossing.
        assert!(!segments_properly_intersect_2d(
            (0.0, 0.0),
            (2.0, 0.0),
            (1.0, 0.0),
            (3.0, 0.0)
        ));
    }

    /// The adversarial-census near-collinear quad (rational-oracle truth: a
    /// genuine proper crossing the raw f64 cross products miss).
    #[test]
    fn segments_properly_intersect_2d_exact_on_census_quad() {
        assert!(segments_properly_intersect_2d(
            (0.968691646732285, 0.8609623040483099),
            (0.8139453654162878, 0.42924481339292414),
            (1.2341322683967864, 0.5444102712336032),
            (0.5099263106731315, 0.34591881397829927)
        ));
    }

    #[test]
    fn circular_order_matches_atan2_on_clean_fan() {
        // Directions far from any collision: the exact circular order must
        // reproduce the atan2 order rebased to start at the +x axis.
        let dirs: Vec<Vector2> = vec![
            Vector2::new(1.0, 0.0),
            Vector2::new(2.0, 1.0),
            Vector2::new(0.0, 3.0),
            Vector2::new(-1.0, 1.0),
            Vector2::new(-2.0, 0.0),
            Vector2::new(-1.0, -2.0),
            Vector2::new(0.5, -0.5),
        ];
        let mut by_exact: Vec<usize> = (0..dirs.len()).collect();
        by_exact.sort_by(|&i, &j| circular_order(&dirs[i], &dirs[j]));
        let mut by_angle: Vec<usize> = (0..dirs.len()).collect();
        by_angle.sort_by(|&i, &j| {
            let ai = dirs[i].y.atan2(dirs[i].x).rem_euclid(std::f64::consts::TAU);
            let aj = dirs[j].y.atan2(dirs[j].x).rem_euclid(std::f64::consts::TAU);
            ai.total_cmp(&aj)
        });
        assert_eq!(by_exact, by_angle);
    }

    #[test]
    fn circular_order_equal_only_for_positive_scalar_multiples() {
        let u = Vector2::new(3.0, 7.0);
        let v = Vector2::new(1.5, 3.5); // u/2 — same direction
        assert_eq!(circular_order(&u, &v), CmpOrdering::Equal);
        // Opposite direction is NOT equal (lands in the other half).
        let w = Vector2::new(-3.0, -7.0);
        assert_ne!(circular_order(&u, &w), CmpOrdering::Equal);
        // Antisymmetry.
        let a = Vector2::new(1.0, 1e-300);
        let b = Vector2::new(1.0, 2e-300);
        assert_eq!(circular_order(&a, &b), CmpOrdering::Less);
        assert_eq!(circular_order(&b, &a), CmpOrdering::Greater);
    }

    #[test]
    fn circular_order_half_plane_boundaries() {
        // +x axis is the first direction of the order; −x belongs to the
        // lower half; ties on the axes are direction-exact.
        let px = Vector2::new(5.0, 0.0);
        let py = Vector2::new(0.0, 5.0);
        let nx = Vector2::new(-5.0, 0.0);
        let ny = Vector2::new(0.0, -5.0);
        assert_eq!(circular_order(&px, &py), CmpOrdering::Less);
        assert_eq!(circular_order(&py, &nx), CmpOrdering::Less);
        assert_eq!(circular_order(&nx, &ny), CmpOrdering::Less);
        assert_eq!(circular_order(&px, &ny), CmpOrdering::Less);
        // Zero vector sorts first, and equal to itself.
        let z = Vector2::ZERO;
        assert_eq!(circular_order(&z, &px), CmpOrdering::Less);
        assert_eq!(circular_order(&z, &z), CmpOrdering::Equal);
    }

    /// Sub-ulp direction pairs: `atan2` collides (identical f64 angles) while
    /// the exact cross sign still orders them — the census row-#6 lie class
    /// the DCEL angular sorts inherit from the transcendental sort key.
    #[test]
    fn circular_order_resolves_atan2_collisions() {
        // v = 3·u + (0, 2^-105): cross(u, v) = 1·(3a+δ) − a·3 = δ > 0 exactly
        // (a = 2^-54 makes 3a exact; 3a + 2^-105 is representable in 53 bits).
        let a = 2.0_f64.powi(-54);
        let delta = 2.0_f64.powi(-105);
        let u = Vector2::new(1.0, a);
        let v = Vector2::new(3.0, 3.0 * a + delta);
        // The exact order is Less regardless of whether this platform's atan2
        // collides — that's what makes the sort key exact.
        assert_eq!(circular_order(&u, &v), CmpOrdering::Less);
        assert_eq!(circular_order(&v, &u), CmpOrdering::Greater);
    }

    #[test]
    fn polygon_area_cmp_basic_and_winding_independent() {
        let unit_sq: Vec<(f64, f64)> = vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        let big_sq: Vec<(f64, f64)> = vec![(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0)];
        let big_sq_cw: Vec<(f64, f64)> = big_sq.iter().rev().copied().collect();
        assert_eq!(polygon_area_cmp_2d(&big_sq, &unit_sq), CmpOrdering::Greater);
        assert_eq!(polygon_area_cmp_2d(&unit_sq, &big_sq), CmpOrdering::Less);
        // |area| — winding direction must not matter.
        assert_eq!(
            polygon_area_cmp_2d(&big_sq_cw, &unit_sq),
            CmpOrdering::Greater
        );
        // Exactly equal areas: the same square translated by an exactly
        // representable offset.
        let moved: Vec<(f64, f64)> = unit_sq.iter().map(|&(x, y)| (x + 0.5, y + 0.25)).collect();
        assert_eq!(polygon_area_cmp_2d(&unit_sq, &moved), CmpOrdering::Equal);
        // Degenerate (<3 vertices) compares as exact zero area.
        let degen: Vec<(f64, f64)> = vec![(0.0, 0.0), (1.0, 1.0)];
        assert_eq!(polygon_area_cmp_2d(&degen, &unit_sq), CmpOrdering::Less);
        assert_eq!(polygon_area_cmp_2d(&degen, &degen), CmpOrdering::Equal);
    }

    /// One-ulp area difference: the f64 shoelace of both polygons evaluates
    /// IDENTICALLY (the perturbation is below its rounding), but the exact
    /// comparison still orders them.
    #[test]
    fn polygon_area_cmp_resolves_sub_rounding_difference() {
        let base: Vec<(f64, f64)> = vec![(0.1, 0.1), (7.3, 0.2), (7.4, 5.9), (0.2, 6.1)];
        // Nudge one vertex outward by one ulp in x: area strictly grows.
        let mut bigger = base.clone();
        bigger[2].0 = f64::from_bits(bigger[2].0.to_bits() + 1);
        assert_eq!(polygon_area_cmp_2d(&bigger, &base), CmpOrdering::Greater);
        assert_eq!(polygon_area_cmp_2d(&base, &bigger), CmpOrdering::Less);
        assert_eq!(polygon_area_cmp_2d(&base, &base), CmpOrdering::Equal);
    }

    #[test]
    fn sign_of_plane_eval_basic_sides() {
        let n = Vector3::new(0.0, 0.0, 1.0);
        assert_eq!(
            sign_of_plane_eval(&n, 5.0, &Point3::new(2.0, -3.0, 6.0)),
            CmpOrdering::Greater
        );
        assert_eq!(
            sign_of_plane_eval(&n, 5.0, &Point3::new(2.0, -3.0, 4.0)),
            CmpOrdering::Less
        );
        assert_eq!(
            sign_of_plane_eval(&n, 5.0, &Point3::new(2.0, -3.0, 5.0)),
            CmpOrdering::Equal
        );
    }

    /// Catastrophic-cancellation plane eval: the naive f64 sum rounds to 0.0
    /// (1e16 + 1.0 collapses) but the true value is +1.0. The A-filter must
    /// refuse to certify and the exact path must recover the sign.
    #[test]
    fn sign_of_plane_eval_exact_under_cancellation() {
        let n = Vector3::new(1.0, 1.0, 1.0);
        let p = Point3::new(1.0e16, 1.0, -1.0e16);
        // Naive: fl(fl(1e16 + 1.0) + (−1e16)) − 0 = 0.0. Truth: +1.0.
        assert_eq!(sign_of_plane_eval(&n, 0.0, &p), CmpOrdering::Greater);
        // Exact zero stays exactly zero (1e16 + 2.0 is representable).
        let q = Point3::new(1.0e16, 2.0, -1.0e16);
        assert_eq!(sign_of_plane_eval(&n, 2.0, &q), CmpOrdering::Equal);
    }

    #[test]
    fn point_plane_sidedness_basic_and_exact() {
        let n = Vector3::new(0.0, 1.0, 0.0);
        let o = Point3::new(3.0, 2.0, -1.0);
        assert_eq!(
            point_plane_sidedness(&n, &o, &Point3::new(9.0, 2.5, 4.0)),
            CmpOrdering::Greater
        );
        assert_eq!(
            point_plane_sidedness(&n, &o, &Point3::new(9.0, 1.5, 4.0)),
            CmpOrdering::Less
        );
        assert_eq!(
            point_plane_sidedness(&n, &o, &Point3::new(-7.0, 2.0, 0.0)),
            CmpOrdering::Equal
        );
        // Cancellation case: p − o = (1e16, 1.0, −1e16) against n = (1,1,1);
        // the f64 dot rounds to 0.0, truth is +1.0.
        let n1 = Vector3::new(1.0, 1.0, 1.0);
        let o1 = Point3::new(0.0, 0.0, 0.0);
        let p1 = Point3::new(1.0e16, 1.0, -1.0e16);
        assert_eq!(point_plane_sidedness(&n1, &o1, &p1), CmpOrdering::Greater);
    }

    #[test]
    fn test_cocircular_points() {
        // Four points on a circle
        let a = Vector2::new(1.0, 0.0);
        let b = Vector2::new(0.0, 1.0);
        let c = Vector2::new(-1.0, 0.0);
        let d = Vector2::new(0.0, -1.0);

        assert!(cocircular(&a, &b, &c, &d));

        // Point not on circle
        let e = Vector2::new(0.5, 0.5);
        assert!(!cocircular(&a, &b, &c, &e));
    }

    #[test]
    fn test_cospherical_points() {
        // Five points on a sphere
        let a = Point3::new(1.0, 0.0, 0.0);
        let b = Point3::new(0.0, 1.0, 0.0);
        let c = Point3::new(0.0, 0.0, 1.0);
        let d = Point3::new(-1.0, 0.0, 0.0);
        let e = Point3::new(0.0, -1.0, 0.0);

        assert!(cospherical(&a, &b, &c, &d, &e));
    }

    #[test]
    fn test_degenerate_cases() {
        // Three identical points
        let a = Vector2::new(1.0, 2.0);
        assert_eq!(orient2d(&a, &a, &a), Orientation::Collinear);

        // Two identical points
        let b = Vector2::new(3.0, 4.0);
        assert_eq!(orient2d(&a, &a, &b), Orientation::Collinear);

        // Exact arithmetic should handle these without issues
        let nearly_zero = 1e-300;
        let tiny = Vector2::new(nearly_zero, nearly_zero);
        let result = orient2d(&Vector2::ZERO, &Vector2::X, &tiny);
        // Should detect the slight CCW orientation
        assert_eq!(result, Orientation::CounterClockwise);
    }
}

#[cfg(test)]
mod expansion_primitive_tests {
    //! Exactness gates for the Shewchuk expansion toolkit — the foundation the
    //! true exact predicates are built from. `two_product`'s tail is checked
    //! against the hardware-exact FMA residual; the expansion routines are
    //! checked on cases with a hand-known exact sum.
    use super::*;

    #[test]
    fn two_product_tail_is_the_exact_fma_residual() {
        for &(a, b) in &[
            (1.0 + 2.0_f64.powi(-30), 1.0 - 2.0_f64.powi(-30)),
            (1.3, 7.9),
            (123456.789, 0.000123),
            (2.0_f64.powi(40) + 1.0, 3.0),
            (-5.5, 11.25),
        ] {
            let (x, y) = two_product(a, b);
            assert_eq!(x, a * b, "high part is the rounded product");
            // FMA computes a*b - x with no intermediate rounding → ground truth.
            assert_eq!(y, a.mul_add(b, -x), "tail must equal the exact residual");
        }
    }

    #[test]
    fn two_diff_recovers_dropped_low_bits() {
        let a = 1.0;
        let b = 2.0_f64.powi(-60); // below ulp(1.0) = 2^-52, so a-b rounds to 1.0
        let (x, y) = two_diff(a, b);
        assert_eq!(x, 1.0, "rounded difference");
        assert_eq!(y, -b, "the lost low bits live in the tail; x + y == a - b");
    }

    #[test]
    fn fast_two_sum_matches_two_sum_when_ordered() {
        let (a, b) = (1.0e16, 3.0); // |a| >= |b|
        assert_eq!(fast_two_sum(a, b), two_sum(a, b));
    }

    #[test]
    fn fast_expansion_sum_clean_integers() {
        let mut h = [0.0f64; 4];
        let n = fast_expansion_sum_zeroelim(&[3.0], &[5.0], &mut h);
        assert_eq!(&h[..n], &[8.0], "3 + 5 = 8, zero tail eliminated");

        let n = fast_expansion_sum_zeroelim(&[1.0, 16.0], &[2.0], &mut h);
        assert_eq!(estimate(&h[..n]), 19.0, "(1+16) + 2 = 19");
    }

    #[test]
    fn fast_expansion_sum_preserves_a_tiny_tail() {
        let mut h = [0.0f64; 4];
        let tiny = 2.0_f64.powi(-60);
        let n = fast_expansion_sum_zeroelim(&[tiny], &[1.0], &mut h);
        assert!(
            h[..n].iter().any(|&c| c == tiny),
            "1 + 2^-60 rounds to 1.0 but the 2^-60 component must survive: {:?}",
            &h[..n]
        );
    }

    #[test]
    fn scale_expansion_clean() {
        let mut h = [0.0f64; 4];
        let n = scale_expansion_zeroelim(&[2.0, 16.0], 3.0, &mut h);
        assert_eq!(estimate(&h[..n]), 54.0, "(2+16) * 3 = 54");
    }

    #[test]
    fn estimate_sums_components() {
        assert_eq!(estimate(&[1.0, 2.0, 4.0]), 7.0);
        assert_eq!(estimate(&[]), 0.0);
    }
}
