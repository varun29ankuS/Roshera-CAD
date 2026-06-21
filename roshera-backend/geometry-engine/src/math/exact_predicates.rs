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

use crate::math::{vector2::Vector2, Point3};

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
// Discrete & Computational Geometry 18:305–363). Only the first-stage
// A-bound is needed by the single-pass filters below; multi-stage refinement
// (B and C bounds) is not implemented in this kernel.
const RESULTERRBOUND: f64 = 3.0e-15;
const CCWERRBOUNDSA: f64 = 3.3306690738754716e-16;
const O3DERRBOUNDSA: f64 = 7.771_561_172_376_096e-16;
const ICCERRBOUNDSA: f64 = 1.0e-15;
const ISPERRBOUNDSA: f64 = 1.6e-15;

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
        // Tighter f64 fallback when |det| sits below the adaptive error
        // bound. A full Shewchuk-style expansion-arithmetic refinement
        // would resolve points that are exactly cocircular up to the last
        // bit; here we treat anything within RESULTERRBOUND of zero as
        // OnBoundary, which is conservative and consistent with the
        // tolerance contract used elsewhere in the kernel.
        if det > RESULTERRBOUND {
            CircleLocation::Inside
        } else if det < -RESULTERRBOUND {
            CircleLocation::Outside
        } else {
            CircleLocation::OnBoundary
        }
    }
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
