//! 2D circumcircle / encroachment / radius-edge predicates for the
//! constrained-Delaunay refinement pipeline
//! ([`crate::tessellation::curved_cdt`]).
//!
//! All routines operate on UV-space [`Vector2`]. Robustness for the
//! degenerate (collinear) case is delegated to
//! [`crate::math::exact_predicates::orient2d`]; once that gate is
//! cleared, the arithmetic that follows is plain double precision.
//! Consumers (Ruppert iteration in `curved_cdt`) treat a collinear
//! triangle as degenerate and skip it — they do not depend on a
//! "best-effort" circumcenter for that case.
//!
//! References:
//! - Shewchuk, *Delaunay Refinement Mesh Generation* (Ph.D. thesis,
//!   1997), §3 for the encroachment predicate and §6 for the
//!   radius-edge quality criterion (≤ √2 ⇔ min angle ≥ ~20.7°).
//! - Shewchuk, *Adaptive Precision Floating-Point Arithmetic*
//!   (1997), §3.5 for the circumcircle closed form.

use crate::math::exact_predicates::{orient2d, Orientation};
use crate::math::vector2::Vector2;

/// Circumcircle of triangle `(a, b, c)` in 2D. Returns
/// `(center, radius²)`. Returns `None` iff the three points are
/// collinear (per [`orient2d`]).
///
/// Closed form (Shewchuk 1997, §3.5):
///   d = 2 · ((b - a) × (c - a))
///   cx = a.x + ((b - a).|·|² · (c - a).y − (c - a).|·|² · (b - a).y) / d
///   cy = a.y + ((c - a).|·|² · (b - a).x − (b - a).|·|² · (c - a).x) / d
/// where `(·) × (·)` denotes the 2D scalar cross product
/// (`perp_dot`). The `Collinear` short-circuit ensures `d ≠ 0`.
pub fn circumcircle_2d(a: Vector2, b: Vector2, c: Vector2) -> Option<(Vector2, f64)> {
    if matches!(orient2d(&a, &b, &c), Orientation::Collinear) {
        return None;
    }
    let ba = b - a;
    let ca = c - a;
    let d = 2.0 * ba.perp_dot(&ca);
    if d == 0.0 {
        // Defensive: `orient2d` already rejected collinear, but if
        // adaptive precision disagrees with double precision on a
        // border case, return None rather than divide-by-zero.
        return None;
    }
    let ba_sq = ba.magnitude_squared();
    let ca_sq = ca.magnitude_squared();
    let ux = (ca.y * ba_sq - ba.y * ca_sq) / d;
    let uy = (ba.x * ca_sq - ca.x * ba_sq) / d;
    let center = Vector2::new(a.x + ux, a.y + uy);
    let radius_sq = ux * ux + uy * uy;
    Some((center, radius_sq))
}

/// Diametral-disk encroachment test (Ruppert / Shewchuk 1996, §3):
/// returns `true` iff `p` lies in (or on) the closed disk whose
/// diameter is the segment `ab`. Equivalent to `(p - a) · (p - b) ≤ 0`.
#[inline]
pub fn is_encroached(a: Vector2, b: Vector2, p: Vector2) -> bool {
    let ap = p - a;
    let bp = p - b;
    ap.dot(&bp) <= 0.0
}

/// Squared radius-edge ratio = `circumradius² / min_edge_length²`.
///
/// Returns [`f64::INFINITY`] when the triangle is degenerate
/// (collinear input). Comparing squared values keeps Ruppert's hot
/// loop sqrt-free: a triangle is "skinny" (per Shewchuk 1996, §6)
/// iff this value exceeds `(√2)² = 2.0`, equivalent to min angle
/// less than `arcsin(1 / (2√2)) ≈ 20.7°`.
pub fn radius_edge_ratio_sq(a: Vector2, b: Vector2, c: Vector2) -> f64 {
    let Some((_center, r_sq)) = circumcircle_2d(a, b, c) else {
        return f64::INFINITY;
    };
    let e_ab_sq = (b - a).magnitude_squared();
    let e_bc_sq = (c - b).magnitude_squared();
    let e_ca_sq = (a - c).magnitude_squared();
    let min_edge_sq = e_ab_sq.min(e_bc_sq).min(e_ca_sq);
    if min_edge_sq <= 0.0 {
        return f64::INFINITY;
    }
    r_sq / min_edge_sq
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tolerance for circumcenter comparison in unit tests.
    const TOL: f64 = 1e-10;

    #[test]
    fn circumcircle_equilateral() {
        // Vertices: (0,0), (1,0), (0.5, √3/2). Circumcenter at the
        // centroid: (0.5, √3/6). Radius² = 1/3.
        let a = Vector2::new(0.0, 0.0);
        let b = Vector2::new(1.0, 0.0);
        let c = Vector2::new(0.5, 3.0_f64.sqrt() / 2.0);
        let (center, r_sq) =
            circumcircle_2d(a, b, c).expect("equilateral triangle has a circumcircle");
        assert!(
            (center.x - 0.5).abs() < TOL,
            "expected cx=0.5, got {}",
            center.x
        );
        assert!(
            (center.y - 3.0_f64.sqrt() / 6.0).abs() < TOL,
            "expected cy=√3/6, got {}",
            center.y
        );
        assert!(
            (r_sq - 1.0 / 3.0).abs() < TOL,
            "expected r²=1/3, got {}",
            r_sq
        );
    }

    #[test]
    fn circumcircle_right_triangle() {
        // Right triangle (0,0), (2,0), (0,2). Hypotenuse from (2,0)
        // to (0,2). Circumcenter is at the hypotenuse midpoint (1,1);
        // radius is half the hypotenuse = √2, r² = 2.
        let a = Vector2::new(0.0, 0.0);
        let b = Vector2::new(2.0, 0.0);
        let c = Vector2::new(0.0, 2.0);
        let (center, r_sq) = circumcircle_2d(a, b, c).expect("right triangle has a circumcircle");
        assert!((center.x - 1.0).abs() < TOL);
        assert!((center.y - 1.0).abs() < TOL);
        assert!((r_sq - 2.0).abs() < TOL);
    }

    #[test]
    fn circumcircle_collinear_returns_none() {
        let a = Vector2::new(0.0, 0.0);
        let b = Vector2::new(1.0, 0.0);
        let c = Vector2::new(2.0, 0.0);
        assert!(circumcircle_2d(a, b, c).is_none());
    }

    #[test]
    fn is_encroached_midpoint_true() {
        let a = Vector2::new(0.0, 0.0);
        let b = Vector2::new(2.0, 0.0);
        // Midpoint is the center of the diametral disk → on the disk
        // (interior, since the disk is closed).
        let mid = Vector2::new(1.0, 0.0);
        assert!(is_encroached(a, b, mid));
    }

    #[test]
    fn is_encroached_far_point_false() {
        let a = Vector2::new(0.0, 0.0);
        let b = Vector2::new(2.0, 0.0);
        // Point at (1, 5) is outside the diametral disk (radius 1).
        let far = Vector2::new(1.0, 5.0);
        assert!(!is_encroached(a, b, far));
    }

    #[test]
    fn is_encroached_endpoint_is_boundary() {
        // The endpoints themselves satisfy (p - a) · (p - b) = 0,
        // which is `≤ 0`, so the predicate returns true (closed disk).
        let a = Vector2::new(0.0, 0.0);
        let b = Vector2::new(1.0, 0.0);
        assert!(is_encroached(a, b, a));
        assert!(is_encroached(a, b, b));
    }

    #[test]
    fn radius_edge_ratio_sq_equilateral_below_threshold() {
        // Equilateral has r/e = 1/√3, so r²/e² = 1/3 ≈ 0.333 < 2.
        let a = Vector2::new(0.0, 0.0);
        let b = Vector2::new(1.0, 0.0);
        let c = Vector2::new(0.5, 3.0_f64.sqrt() / 2.0);
        let ratio_sq = radius_edge_ratio_sq(a, b, c);
        assert!(
            ratio_sq < 2.0,
            "equilateral should be non-skinny, got {}",
            ratio_sq
        );
        assert!((ratio_sq - 1.0 / 3.0).abs() < TOL);
    }

    #[test]
    fn radius_edge_ratio_sq_sliver_above_threshold() {
        // Very elongated triangle: (0,0), (100,0), (50, 0.5).
        // Min edge is the short height side, circumradius is large.
        let a = Vector2::new(0.0, 0.0);
        let b = Vector2::new(100.0, 0.0);
        let c = Vector2::new(50.0, 0.5);
        let ratio_sq = radius_edge_ratio_sq(a, b, c);
        assert!(
            ratio_sq > 2.0,
            "100:1 sliver should be skinny, got ratio_sq={}",
            ratio_sq
        );
    }

    #[test]
    fn radius_edge_ratio_sq_collinear_infinity() {
        let a = Vector2::new(0.0, 0.0);
        let b = Vector2::new(1.0, 0.0);
        let c = Vector2::new(2.0, 0.0);
        assert_eq!(radius_edge_ratio_sq(a, b, c), f64::INFINITY);
    }
}
