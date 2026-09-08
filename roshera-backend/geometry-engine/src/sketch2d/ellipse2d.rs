//! 2D Ellipse primitive for sketching
//!
//! This module implements parametric 2D ellipses for sketching.
//! An ellipse is defined by its center, semi-major and semi-minor axes, and rotation.
//!
//! # Degrees of Freedom
//!
//! A 2D ellipse has 5 degrees of freedom:
//! - 2 for center position (X, Y)
//! - 2 for semi-major and semi-minor axes lengths
//! - 1 for rotation angle
//!
//! When axis-aligned (rotation = 0), it effectively has 4 DOF.

use super::{
    Circle2d, Matrix3, Point2d, Sketch2dError, Sketch2dResult, SketchEntity2d, Tolerance2d,
    Vector2d,
};
use crate::math::tolerance::STRICT_TOLERANCE;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use uuid::Uuid;

/// Unique identifier for a 2D ellipse
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct Ellipse2dId(pub Uuid);

impl Ellipse2dId {
    /// Create a new unique ellipse ID
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for Ellipse2dId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Ellipse2d_{}", &self.0.to_string()[..8])
    }
}

/// A 2D ellipse defined by center, axes, and rotation
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Ellipse2d {
    /// Center point of the ellipse
    pub center: Point2d,
    /// Semi-major axis length (a)
    pub semi_major: f64,
    /// Semi-minor axis length (b)
    pub semi_minor: f64,
    /// Rotation angle in radians (counter-clockwise from positive X-axis)
    pub rotation: f64,
}

impl Ellipse2d {
    /// Create a new ellipse
    pub fn new(
        center: Point2d,
        semi_major: f64,
        semi_minor: f64,
        rotation: f64,
    ) -> Sketch2dResult<Self> {
        if semi_major <= STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "semi_major".to_string(),
                value: semi_major.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        if semi_minor <= STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "semi_minor".to_string(),
                value: semi_minor.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        // Ensure semi_major >= semi_minor by convention
        let (a, b, rot) = if semi_major >= semi_minor {
            (semi_major, semi_minor, rotation)
        } else {
            // Swap axes and adjust rotation by 90 degrees
            (
                semi_minor,
                semi_major,
                rotation + std::f64::consts::PI / 2.0,
            )
        };

        Ok(Self {
            center,
            semi_major: a,
            semi_minor: b,
            rotation: Self::normalize_angle(rot),
        })
    }

    /// Create an axis-aligned ellipse
    pub fn axis_aligned(center: Point2d, semi_major: f64, semi_minor: f64) -> Sketch2dResult<Self> {
        Self::new(center, semi_major, semi_minor, 0.0)
    }

    /// Create a circle (special case of ellipse)
    pub fn circle(center: Point2d, radius: f64) -> Sketch2dResult<Self> {
        Self::new(center, radius, radius, 0.0)
    }

    /// Create an ellipse from bounding box
    pub fn from_bounding_box(min: Point2d, max: Point2d, rotation: f64) -> Sketch2dResult<Self> {
        if min.x >= max.x || min.y >= max.y {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "bounding box".to_string(),
                value: format!("min: {:?}, max: {:?}", min, max),
                constraint: "min must be less than max in both dimensions".to_string(),
            });
        }

        let center = Point2d::new((min.x + max.x) / 2.0, (min.y + max.y) / 2.0);
        let semi_major = (max.x - min.x) / 2.0;
        let semi_minor = (max.y - min.y) / 2.0;

        Self::new(center, semi_major, semi_minor, rotation)
    }

    /// Get the area of the ellipse
    pub fn area(&self) -> f64 {
        std::f64::consts::PI * self.semi_major * self.semi_minor
    }

    /// Get the perimeter (approximate using Ramanujan's formula)
    pub fn perimeter(&self) -> f64 {
        let a = self.semi_major;
        let b = self.semi_minor;

        // Ramanujan's first approximation
        let h = ((a - b) * (a - b)) / ((a + b) * (a + b));
        std::f64::consts::PI * (a + b) * (1.0 + (3.0 * h) / (10.0 + (4.0 - 3.0 * h).sqrt()))
    }

    /// Get the eccentricity
    pub fn eccentricity(&self) -> f64 {
        let a = self.semi_major;
        let b = self.semi_minor;
        ((a * a - b * b) / (a * a)).sqrt()
    }

    /// Get the focal distance (distance from center to each focus)
    pub fn focal_distance(&self) -> f64 {
        let a = self.semi_major;
        let b = self.semi_minor;
        (a * a - b * b).sqrt()
    }

    /// Get the two foci of the ellipse
    pub fn foci(&self) -> (Point2d, Point2d) {
        let c = self.focal_distance();
        let cos_r = self.rotation.cos();
        let sin_r = self.rotation.sin();

        let f1 = Point2d::new(self.center.x + c * cos_r, self.center.y + c * sin_r);

        let f2 = Point2d::new(self.center.x - c * cos_r, self.center.y - c * sin_r);

        (f1, f2)
    }

    /// Evaluate a point on the ellipse at parameter t (0 to 2π)
    pub fn evaluate(&self, t: f64) -> Point2d {
        let cos_t = t.cos();
        let sin_t = t.sin();
        let cos_r = self.rotation.cos();
        let sin_r = self.rotation.sin();

        // Point on unit ellipse
        let x_local = self.semi_major * cos_t;
        let y_local = self.semi_minor * sin_t;

        // Rotate and translate
        Point2d::new(
            self.center.x + x_local * cos_r - y_local * sin_r,
            self.center.y + x_local * sin_r + y_local * cos_r,
        )
    }

    /// Get the tangent vector at parameter t
    pub fn tangent(&self, t: f64) -> Vector2d {
        let cos_t = t.cos();
        let sin_t = t.sin();
        let cos_r = self.rotation.cos();
        let sin_r = self.rotation.sin();

        // Derivative on unit ellipse
        let dx_local = -self.semi_major * sin_t;
        let dy_local = self.semi_minor * cos_t;

        // Rotate tangent vector
        Vector2d::new(
            dx_local * cos_r - dy_local * sin_r,
            dx_local * sin_r + dy_local * cos_r,
        )
    }

    /// Get the normal vector at parameter t (outward pointing)
    pub fn normal(&self, t: f64) -> Vector2d {
        let tangent = self.tangent(t);
        // Rotate tangent by 90 degrees clockwise for outward normal
        Vector2d::new(tangent.y, -tangent.x)
    }

    /// Check if a point is inside the ellipse
    pub fn contains_point(&self, point: &Point2d) -> bool {
        // Transform point to local coordinates
        let dx = point.x - self.center.x;
        let dy = point.y - self.center.y;

        let cos_r = self.rotation.cos();
        let sin_r = self.rotation.sin();

        // Rotate point by -rotation to align with ellipse axes
        let x_local = dx * cos_r + dy * sin_r;
        let y_local = -dx * sin_r + dy * cos_r;

        // Check if point is inside using ellipse equation
        let normalized_x = x_local / self.semi_major;
        let normalized_y = y_local / self.semi_minor;

        normalized_x * normalized_x + normalized_y * normalized_y <= 1.0
    }

    /// Check if a point is on the ellipse boundary within tolerance
    pub fn contains_point_on_boundary(&self, point: &Point2d, tolerance: &Tolerance2d) -> bool {
        // Transform point to local coordinates
        let dx = point.x - self.center.x;
        let dy = point.y - self.center.y;

        let cos_r = self.rotation.cos();
        let sin_r = self.rotation.sin();

        let x_local = dx * cos_r + dy * sin_r;
        let y_local = -dx * sin_r + dy * cos_r;

        // Check using ellipse equation
        let normalized_x = x_local / self.semi_major;
        let normalized_y = y_local / self.semi_minor;

        let value = normalized_x * normalized_x + normalized_y * normalized_y;
        (value - 1.0).abs() < tolerance.distance / self.semi_minor.min(self.semi_major)
    }

    /// Find the closest point on the ellipse to a given point.
    ///
    /// Returns the foot of the perpendicular from `point` — the point
    /// of the ellipse's boundary that minimises the distance — or a
    /// typed error. It never returns an approximation it cannot
    /// justify.
    ///
    /// # Method
    ///
    /// Eberly's reduction (*Distance from a Point to an Ellipse, an
    /// Ellipsoid, or a Hyperellipsoid*, Geometric Tools). The point is
    /// taken into the ellipse's local frame and reflected into the
    /// closed first quadrant, where the minimiser is unique. Writing
    /// `z0 = y0/e0`, `z1 = y1/e1`, `r0 = (e0/e1)^2 >= 1` and
    /// `n0 = r0*z0`, the foot is `(r0*y0/((w - 1) + r0), y1/w)` for the
    /// unique root `w` of
    ///
    /// ```text
    /// F(w) = (n0/((w - 1) + r0))^2 + (z1/w)^2 - 1
    /// ```
    ///
    /// (`w` is Eberly's `s + 1`; see `bisect_ratio_root` for why the
    /// shifted variable is the one that survives rounding.) `F` is
    /// continuous and strictly decreasing on `w > 0`, from `+inf` at the
    /// origin to `-1` at infinity, so a root exists and is unique. It is
    /// bracketed below by `w = z1`, where the second term is exactly
    /// zero and `F = (n0/((z1 - 1) + r0))^2 >= 0`, and above by `w = 1`
    /// when the point is inside (`F(1) = z0^2 + z1^2 - 1 < 0`) or
    /// `w = sqrt(n0^2 + z1^2)` when it is not. Each end is then WIDENED
    /// until its sign is measured rather than assumed, so the bracket is
    /// a fact before the first halving. Bisection halves it every step
    /// and stops when the midpoint coincides with an endpoint — i.e.
    /// when the two are adjacent doubles — so the root is located
    /// to the last representable bit OF THAT BRACKET. What the returned
    /// FOOT is worth is settled separately, by the on-curve residual
    /// check in `closest_point`, which is what makes the answer
    /// certified rather than merely converged. The step count is
    /// bounded by the
    /// exponent span of `f64` (`CLOSEST_POINT_MAX_BISECTIONS`), never by
    /// a guess. Real geometry closes the bracket in about sixty steps.
    ///
    /// A previous implementation ran Newton's method on
    /// `f(t) = (P - E(t)) . E'(t)` with `+|E'|^2 + (P - E) . E''` for
    /// `f'(t)`; the derivative is `-|E'|^2 + (P - E) . E''`, so every
    /// step walked away from the root and the result was a point on the
    /// far side of the curve. `tests/sketch2d_ellipse_closest_point.rs`
    /// pins both the sign and the answer.
    ///
    /// # Ambiguity
    ///
    /// A whole SEGMENT of inputs has more than one true minimiser, and
    /// all of them are answered with a documented choice rather than an
    /// error, because every candidate is genuinely closest — nothing is
    /// approximated:
    ///
    /// * every point of the closed major-axis segment from the centre
    ///   out to the evolute cusp `(a^2 - b^2)/a` has TWO minimisers,
    ///   mirrored across that axis: `(a*r, +b*sqrt(1 - r^2))` and its
    ///   negative. The `+` one is returned. The centre (`r = 0`) is the
    ///   familiar end of that segment — the two minor-axis vertices,
    ///   `+minor` returned;
    /// * the centre of a circular ellipse (`a == b`) is equidistant
    ///   from the whole boundary — not two points but all of them; the
    ///   point at parameter `0` is returned, matching
    ///   [`Circle2d::closest_point`].
    ///
    /// # Errors
    ///
    /// * [`Sketch2dError::InvalidParameter`] — `point` is not finite.
    /// * [`Sketch2dError::DegenerateGeometry`] — an axis is not finite
    ///   and positive, or the centre or rotation is not finite. The
    ///   fields are public, so this is reachable without the
    ///   constructor.
    /// * [`Sketch2dError::NumericalError`] — the bracket does not
    ///   straddle the root, or bisection did not close it within
    ///   `CLOSEST_POINT_MAX_BISECTIONS` steps. The message names the
    ///   bracket, the step count and the residual.
    pub fn closest_point(&self, point: &Point2d) -> Sketch2dResult<Point2d> {
        if !point.x.is_finite() || !point.y.is_finite() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "point".to_string(),
                value: format!("({}, {})", point.x, point.y),
                constraint: "finite".to_string(),
            });
        }
        if !self.semi_major.is_finite()
            || !self.semi_minor.is_finite()
            || self.semi_major <= 0.0
            || self.semi_minor <= 0.0
        {
            return Err(Sketch2dError::DegenerateGeometry {
                entity: "Ellipse2d".to_string(),
                reason: format!(
                    "semi-axes ({}, {}) must both be finite and positive",
                    self.semi_major, self.semi_minor
                ),
            });
        }
        if !self.center.x.is_finite() || !self.center.y.is_finite() || !self.rotation.is_finite() {
            return Err(Sketch2dError::DegenerateGeometry {
                entity: "Ellipse2d".to_string(),
                reason: format!(
                    "centre ({}, {}) and rotation {} must be finite",
                    self.center.x, self.center.y, self.rotation
                ),
            });
        }

        let dx = point.x - self.center.x;
        let dy = point.y - self.center.y;

        let cos_r = self.rotation.cos();
        let sin_r = self.rotation.sin();

        let x_local = dx * cos_r + dy * sin_r;
        let y_local = -dx * sin_r + dy * cos_r;

        // The reduction needs the LONGER axis first. `Ellipse2d::new`
        // keeps `semi_major >= semi_minor`, but the fields are public
        // and `ParametricEllipse2d::transform` scales the two
        // independently — a non-uniform scale can leave `semi_minor`
        // the longer one. Sort the roles here, swap the answer back
        // below.
        let axes_swapped = self.semi_minor > self.semi_major;
        let (e0, e1) = if axes_swapped {
            (self.semi_minor, self.semi_major)
        } else {
            (self.semi_major, self.semi_minor)
        };
        let (u, v) = if axes_swapped {
            (y_local, x_local)
        } else {
            (x_local, y_local)
        };

        // The ellipse is symmetric about both local axes, so solving in
        // the closed first quadrant and restoring the point's own signs
        // gives the foot in the original quadrant.
        let (q0, q1) = closest_point_first_quadrant(e0, e1, u.abs(), v.abs())?;

        // Certify the foot before handing it out. The bracket proves a
        // SIGN change; it does not prove that the arithmetic which
        // produced its endpoints stayed inside the representable range,
        // and two inputs make that difference visible:
        //
        // * a cursor far enough out that `sqrt(n0^2 + z1^2)` overflows
        //   gives an upper end of `+inf`. `F(inf) = -1 <= 0` passes the
        //   sign test, the first midpoint IS that endpoint, and the
        //   bisection "converges" to infinity -- returning the ellipse's
        //   CENTRE, whose implicit residual is -1. Measured at
        //   (1e200, 1e200) on a 6x3 ellipse; (1e154, 1e154) is fine;
        //   an aspect ratio of 1e140 turns it into a NaN foot.
        // * subnormal coordinates fail the other way, landing a foot
        //   measurably off the curve (0.006 at 1e-320).
        //
        // Both are caught by the only question that decides whether the
        // answer is an answer: is the point about to be returned ON the
        // ellipse? The bracket's own finiteness is refused separately in
        // `bisect_ratio_root`; this is the check that does not depend on
        // having anticipated the overflow path.
        let on_curve = (q0 / e0) * (q0 / e0) + (q1 / e1) * (q1 / e1) - 1.0;
        if !on_curve.is_finite() || on_curve.abs() > CLOSEST_POINT_ON_CURVE_TOLERANCE {
            return Err(Sketch2dError::NumericalError {
                description: format!(
                    concat!(
                        "closest point on ellipse: the computed foot ({}, {}) in the ",
                        "ellipse frame is not on the ellipse -- implicit residual {}, ",
                        "tolerance {}"
                    ),
                    q0, q1, on_curve, CLOSEST_POINT_ON_CURVE_TOLERANCE
                ),
            });
        }

        let signed0 = if u < 0.0 { -q0 } else { q0 };
        let signed1 = if v < 0.0 { -q1 } else { q1 };
        let (foot_x, foot_y) = if axes_swapped {
            (signed1, signed0)
        } else {
            (signed0, signed1)
        };

        Ok(Point2d::new(
            self.center.x + foot_x * cos_r - foot_y * sin_r,
            self.center.y + foot_x * sin_r + foot_y * cos_r,
        ))
    }

    /// Convert to a circle if semi-major equals semi-minor
    pub fn to_circle(&self) -> Option<Circle2d> {
        if (self.semi_major - self.semi_minor).abs() < STRICT_TOLERANCE.distance() {
            Circle2d::new(self.center, self.semi_major).ok()
        } else {
            None
        }
    }

    /// Get the axis-aligned bounding box
    pub fn bounding_box(&self) -> (Point2d, Point2d) {
        let cos_r = self.rotation.cos();
        let sin_r = self.rotation.sin();

        // Extrema occur where the derivative is zero
        // For rotated ellipse: x(t) = cx + a*cos(t)*cos(r) - b*sin(t)*sin(r)
        // dx/dt = -a*sin(t)*cos(r) - b*cos(t)*sin(r) = 0

        let tx = (self.semi_minor * sin_r).atan2(self.semi_major * cos_r);
        let ty = (-self.semi_minor * cos_r).atan2(self.semi_major * sin_r);

        // Evaluate at extrema
        let mut min_x = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_y = f64::NEG_INFINITY;

        // Check four extrema points
        for &t in &[tx, tx + std::f64::consts::PI, ty, ty + std::f64::consts::PI] {
            let p = self.evaluate(t);
            min_x = min_x.min(p.x);
            max_x = max_x.max(p.x);
            min_y = min_y.min(p.y);
            max_y = max_y.max(p.y);
        }

        (Point2d::new(min_x, min_y), Point2d::new(max_x, max_y))
    }

    /// Intersect with a line
    #[allow(non_snake_case)] // A, B, C are standard quadratic coefficient names
    pub fn intersect_line(&self, line_point: &Point2d, line_dir: &Vector2d) -> Vec<Point2d> {
        // Transform line to local coordinates
        let dx = line_point.x - self.center.x;
        let dy = line_point.y - self.center.y;

        let cos_r = self.rotation.cos();
        let sin_r = self.rotation.sin();

        // Transform line point
        let px = dx * cos_r + dy * sin_r;
        let py = -dx * sin_r + dy * cos_r;

        // Transform line direction
        let vx = line_dir.x * cos_r + line_dir.y * sin_r;
        let vy = -line_dir.x * sin_r + line_dir.y * cos_r;

        // Solve quadratic equation
        let a2 = self.semi_major * self.semi_major;
        let b2 = self.semi_minor * self.semi_minor;

        let A = (vx * vx) / a2 + (vy * vy) / b2;
        let B = 2.0 * ((px * vx) / a2 + (py * vy) / b2);
        let C = (px * px) / a2 + (py * py) / b2 - 1.0;

        let discriminant = B * B - 4.0 * A * C;

        if discriminant < 0.0 {
            Vec::new()
        } else if discriminant.abs() < STRICT_TOLERANCE.distance() {
            // One intersection (tangent)
            let t = -B / (2.0 * A);
            vec![Point2d::new(
                line_point.x + t * line_dir.x,
                line_point.y + t * line_dir.y,
            )]
        } else {
            // Two intersections
            let sqrt_disc = discriminant.sqrt();
            let t1 = (-B - sqrt_disc) / (2.0 * A);
            let t2 = (-B + sqrt_disc) / (2.0 * A);

            vec![
                Point2d::new(
                    line_point.x + t1 * line_dir.x,
                    line_point.y + t1 * line_dir.y,
                ),
                Point2d::new(
                    line_point.x + t2 * line_dir.x,
                    line_point.y + t2 * line_dir.y,
                ),
            ]
        }
    }

    /// Normalize angle to [0, 2π)
    fn normalize_angle(angle: f64) -> f64 {
        let two_pi = 2.0 * std::f64::consts::PI;
        let mut normalized = angle % two_pi;
        if normalized < 0.0 {
            normalized += two_pi;
        }
        normalized
    }
}

/// Bisection budget for [`Ellipse2d::closest_point`].
///
/// Bisection halves the bracket every step and stops when the midpoint
/// coincides with an endpoint, so the step count is bounded by the
/// number of times the widest representable interval can be halved
/// before its ends become adjacent doubles: `log2(f64::MAX /
/// f64::MIN_POSITIVE subnormal) = 1024 + 1074 = 2098`. The budget
/// stands just above that, so exhausting it is impossible for a
/// bracket the solver actually builds and the error below is a guard,
/// not a rounding policy. Brackets that arise from real geometry close
/// in about sixty steps.
const CLOSEST_POINT_MAX_BISECTIONS: usize = 2100;

/// How far off the ellipse a computed foot may sit before
/// [`Ellipse2d::closest_point`] refuses it, as the dimensionless
/// implicit residual `(x/a)^2 + (y/b)^2 - 1`.
///
/// Every foot the solver returns for an input inside the representable
/// range measures under `1e-12` -- asserted over 200 cursors on four
/// ellipses, including a 160:1 sliver, in
/// `tests/sketch2d_ellipse_closest_point.rs`. The refusal threshold
/// sits three orders looser so that rounding never costs a caller a
/// legitimate answer, while still catching the two failure modes that
/// motivated it: an overflowed bracket (residual -1, the centre) and
/// subnormal coordinates (residual ~4e-3).
const CLOSEST_POINT_ON_CURVE_TOLERANCE: f64 = 1e-9;

/// The foot of the perpendicular for a point already reduced to the
/// closed first quadrant of an axis-aligned ellipse with `e0 >= e1 > 0`
/// and `y0, y1 >= 0`.
///
/// The three arms are Eberly's: an interior/exterior point off both
/// axes needs the bisection; a point on the minor axis has the minor
/// vertex as its foot; a point on the major axis has one either side of
/// the evolute cusp at `(e0^2 - e1^2)/e0`.
fn closest_point_first_quadrant(e0: f64, e1: f64, y0: f64, y1: f64) -> Sketch2dResult<(f64, f64)> {
    if y1 > 0.0 {
        if y0 > 0.0 {
            let z0 = y0 / e0;
            let z1 = y1 / e1;
            let g = z0 * z0 + z1 * z1 - 1.0;
            let r0 = (e0 / e1) * (e0 / e1);
            let w = bisect_ratio_root(r0, z0, z1, g)?;
            Ok((r0 * y0 / ((w - 1.0) + r0), y1 / w))
        } else {
            // On the minor axis: the foot is the minor vertex. For the
            // centre of a proper ellipse this is one of the two true
            // minimisers — the documented choice.
            Ok((0.0, e1))
        }
    } else {
        // On the major axis. Inside the evolute cusp the foot leaves
        // the axis; outside it (and for every circular ellipse, where
        // `denom` is zero and the comparison is false) it is the major
        // vertex — the same choice `Circle2d::closest_point` makes for
        // a point at the centre.
        let numer = e0 * y0;
        let denom = e0 * e0 - e1 * e1;
        if numer < denom {
            let ratio = numer / denom;
            // `1 - ratio^2` is non-negative for `ratio < 1`, which the
            // branch guarantees; the clamp only absorbs rounding at the
            // cusp itself.
            Ok((e0 * ratio, e1 * (1.0 - ratio * ratio).max(0.0).sqrt()))
        } else {
            Ok((e0, 0.0))
        }
    }
}

/// The refusal a bracket that is not usable earns: the interval, the
/// number of widenings actually performed, and both residuals AS THEY
/// STAND AT THE EXIT, so a reader can see which end failed and by how
/// much. The count is the live one -- a budget-shaped constant here
/// would report 2100 for the zero-widening exits and hide which path
/// refused.
fn bracket_error(
    lower: f64,
    upper: f64,
    f_lower: f64,
    f_upper: f64,
    widenings: usize,
) -> Sketch2dError {
    Sketch2dError::NumericalError {
        description: format!(
            concat!(
                "closest point on ellipse: the bracket [{}, {}] is not usable ",
                "after {} widenings (F = {} and {}, wanted a finite interval ",
                "with F >= 0 and <= 0)"
            ),
            lower, upper, widenings, f_lower, f_upper
        ),
    }
}

/// The unique root of
/// `F(w) = (r0*z0/((w - 1) + r0))^2 + (z1/w)^2 - 1` on `w > 0`, by
/// bisection over a bracket the function itself is made to certify.
///
/// `w` is Eberly's `s + 1`. Solving in `w` rather than `s` is not
/// cosmetic: the lower end of the bracket is `w = z1`, where the second
/// term is `(z1/z1)^2 - 1`, exactly zero in IEEE arithmetic, so `F` is
/// provably non-negative there. Written in `s` the same endpoint is
/// `z1 - 1` and the term becomes `(z1/((z1 - 1) + 1))^2 - 1`, whose
/// denominator loses the cancellation: for `z1 = 0.05` it evaluates to
/// -2e-15 and a straddle test on the true sign refuses a perfectly
/// ordinary point. Measured, on the 160:1 sliver in
/// `tests/sketch2d_ellipse_closest_point.rs`.
///
/// `g` is `F(1)` (Eberly's `F(0)`), i.e. `z0^2 + z1^2 - 1`: negative
/// inside the ellipse, positive outside, zero on it.
fn bisect_ratio_root(r0: f64, z0: f64, z1: f64, g: f64) -> Sketch2dResult<f64> {
    let n0 = r0 * z0;
    // `(w - 1) + r0` is grouped so that `w = 1` reproduces `n0/r0`,
    // which is `z0` to within a ulp — `(r0*z0)/r0 != z0` for about an
    // eighth of all pairs — making `F(1) = g` exact to a ulp rather
    // than merely close. The widening walk below absorbs that last bit.
    let residual = |w: f64| {
        let ratio0 = n0 / ((w - 1.0) + r0);
        let ratio1 = z1 / w;
        ratio0 * ratio0 + ratio1 * ratio1 - 1.0
    };

    // `F` falls monotonically from `+inf` at `w -> 0+` to `-1` as
    // `w -> inf`, so widening either end can only move it towards the
    // sign that end needs. Rather than trusting the closed forms to
    // round the right way, walk each end until the sign is a MEASURED
    // fact. Both loops are no-ops for every well-formed input; they
    // terminate for the same reason the root exists, and a budget that
    // runs out is a non-finite input, reported below.
    let mut lower = z1;
    let mut upper = if g < 0.0 {
        1.0
    } else {
        (n0 * n0 + z1 * z1).sqrt()
    };

    // Finiteness first, and BEFORE the sign walk. `sqrt(n0^2 + z1^2)`
    // overflows for a cursor around 1e155 out from a 6x3 ellipse, and
    // `+inf` passes every sign test there is: `F(inf) = -1 <= 0`, so
    // the walk breaks immediately, and the first bisection midpoint
    // `0.5*(lower + inf)` IS `inf`, which equals the endpoint and
    // "converges". The bracket is only a certificate if it is an
    // interval.
    let mut widenings = 0usize;
    if !lower.is_finite() || !upper.is_finite() || lower <= 0.0 {
        return Err(bracket_error(
            lower,
            upper,
            residual(lower),
            residual(upper),
            widenings,
        ));
    }

    loop {
        if residual(lower) >= 0.0 {
            break;
        }
        lower *= 0.5;
        widenings += 1;
        if widenings > CLOSEST_POINT_MAX_BISECTIONS || lower <= 0.0 {
            return Err(bracket_error(
                lower,
                upper,
                residual(lower),
                residual(upper),
                widenings,
            ));
        }
    }
    loop {
        if residual(upper) <= 0.0 {
            break;
        }
        upper *= 2.0;
        widenings += 1;
        if widenings > CLOSEST_POINT_MAX_BISECTIONS || !upper.is_finite() {
            return Err(bracket_error(
                lower,
                upper,
                residual(lower),
                residual(upper),
                widenings,
            ));
        }
    }
    if lower > upper {
        return Err(bracket_error(
            lower,
            upper,
            residual(lower),
            residual(upper),
            widenings,
        ));
    }

    for _ in 0..CLOSEST_POINT_MAX_BISECTIONS {
        let mid = 0.5 * (lower + upper);
        // Reason: the bracket has closed to adjacent doubles, which is
        // the exact termination this loop is built around; a tolerance
        // here would stop it early and hand back a coarser root.
        if mid == lower || mid == upper {
            return Ok(mid);
        }
        let f_mid = residual(mid);
        if f_mid > 0.0 {
            lower = mid;
        } else if f_mid < 0.0 {
            upper = mid;
        } else {
            return Ok(mid);
        }
    }

    let mid = 0.5 * (lower + upper);
    Err(Sketch2dError::NumericalError {
        description: format!(
            concat!(
                "closest point on ellipse: bisection did not close the bracket ",
                "[{}, {}] in {} steps (residual {})"
            ),
            lower,
            upper,
            CLOSEST_POINT_MAX_BISECTIONS,
            residual(mid)
        ),
    })
}

/// A parametric ellipse entity with constraint tracking
pub struct ParametricEllipse2d {
    /// Unique identifier
    pub id: Ellipse2dId,
    /// Ellipse geometry
    pub ellipse: Ellipse2d,
    /// Number of constraints applied
    constraint_count: usize,
    /// Construction geometry flag
    pub is_construction: bool,
}

impl ParametricEllipse2d {
    /// Create a new parametric ellipse
    pub fn new(ellipse: Ellipse2d) -> Self {
        Self {
            id: Ellipse2dId::new(),
            ellipse,
            constraint_count: 0,
            is_construction: false,
        }
    }

    /// Add a constraint
    pub fn add_constraint(&mut self) {
        self.constraint_count += 1;
    }

    /// Remove a constraint
    pub fn remove_constraint(&mut self) {
        if self.constraint_count > 0 {
            self.constraint_count -= 1;
        }
    }
}

impl SketchEntity2d for ParametricEllipse2d {
    fn degrees_of_freedom(&self) -> usize {
        if (self.ellipse.rotation - 0.0).abs() < STRICT_TOLERANCE.distance()
            || (self.ellipse.rotation - std::f64::consts::PI / 2.0).abs()
                < STRICT_TOLERANCE.distance()
            || (self.ellipse.rotation - std::f64::consts::PI).abs() < STRICT_TOLERANCE.distance()
            || (self.ellipse.rotation - 3.0 * std::f64::consts::PI / 2.0).abs()
                < STRICT_TOLERANCE.distance()
        {
            4 // Axis-aligned: center x, center y, semi_major, semi_minor
        } else {
            5 // Rotated: adds rotation angle
        }
    }

    fn constraint_count(&self) -> usize {
        self.constraint_count
    }

    fn bounding_box(&self) -> (Point2d, Point2d) {
        self.ellipse.bounding_box()
    }

    fn transform(&mut self, matrix: &Matrix3) {
        // Transform center
        self.ellipse.center = matrix.transform_point(&self.ellipse.center);

        // Transform axes (approximate - assumes uniform scale)
        let scale_x =
            (matrix.data[0][0] * matrix.data[0][0] + matrix.data[1][0] * matrix.data[1][0]).sqrt();
        let scale_y =
            (matrix.data[0][1] * matrix.data[0][1] + matrix.data[1][1] * matrix.data[1][1]).sqrt();

        self.ellipse.semi_major *= scale_x;
        self.ellipse.semi_minor *= scale_y;

        // Transform rotation
        let rotation_delta = matrix.data[1][0].atan2(matrix.data[0][0]);
        self.ellipse.rotation = Ellipse2d::normalize_angle(self.ellipse.rotation + rotation_delta);
    }

    fn clone_entity(&self) -> Box<dyn SketchEntity2d> {
        Box::new(ParametricEllipse2d {
            id: Ellipse2dId::new(),
            ellipse: self.ellipse,
            constraint_count: 0,
            is_construction: self.is_construction,
        })
    }
}

/// Storage for ellipses using DashMap
pub struct Ellipse2dStore {
    /// All ellipses indexed by ID
    ellipses: Arc<DashMap<Ellipse2dId, ParametricEllipse2d>>,
    /// Spatial index for efficient queries
    spatial_index: Arc<DashMap<(i32, i32), Vec<Ellipse2dId>>>,
    /// Grid size for spatial indexing
    grid_size: f64,
}

impl Ellipse2dStore {
    /// Create a new ellipse store
    pub fn new(grid_size: f64) -> Self {
        Self {
            ellipses: Arc::new(DashMap::new()),
            spatial_index: Arc::new(DashMap::new()),
            grid_size,
        }
    }

    /// Add an ellipse to the store
    pub fn add(&self, ellipse: ParametricEllipse2d) -> Ellipse2dId {
        let id = ellipse.id;

        // Update spatial index
        let (min, max) = ellipse.bounding_box();
        self.update_spatial_index(id, min, max);

        self.ellipses.insert(id, ellipse);
        id
    }

    /// Update spatial index for an ellipse
    fn update_spatial_index(&self, id: Ellipse2dId, min: Point2d, max: Point2d) {
        let min_grid_x = (min.x / self.grid_size).floor() as i32;
        let min_grid_y = (min.y / self.grid_size).floor() as i32;
        let max_grid_x = (max.x / self.grid_size).ceil() as i32;
        let max_grid_y = (max.y / self.grid_size).ceil() as i32;

        for x in min_grid_x..=max_grid_x {
            for y in min_grid_y..=max_grid_y {
                self.spatial_index.entry((x, y)).or_default().push(id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn test_ellipse_creation() {
        let ellipse = Ellipse2d::new(Point2d::new(5.0, 3.0), 10.0, 6.0, 0.0).unwrap();
        assert_eq!(ellipse.center.x, 5.0);
        assert_eq!(ellipse.center.y, 3.0);
        assert_eq!(ellipse.semi_major, 10.0);
        assert_eq!(ellipse.semi_minor, 6.0);
        assert_eq!(ellipse.rotation, 0.0);

        // Test invalid dimensions
        assert!(Ellipse2d::new(Point2d::ORIGIN, 0.0, 5.0, 0.0).is_err());
        assert!(Ellipse2d::new(Point2d::ORIGIN, 5.0, 0.0, 0.0).is_err());
    }

    #[test]
    fn test_ellipse_properties() {
        let ellipse = Ellipse2d::new(Point2d::ORIGIN, 5.0, 3.0, 0.0).unwrap();

        // Area
        let expected_area = PI * 5.0 * 3.0;
        assert!((ellipse.area() - expected_area).abs() < 1e-10);

        // Eccentricity
        let e = ellipse.eccentricity();
        assert!(e > 0.0 && e < 1.0);

        // Focal distance
        let c = ellipse.focal_distance();
        assert_eq!(c, 4.0); // sqrt(25 - 9) = 4
    }

    #[test]
    fn test_ellipse_evaluation() {
        let ellipse = Ellipse2d::new(Point2d::new(1.0, 2.0), 4.0, 2.0, 0.0).unwrap();

        // At t = 0 (rightmost point)
        let p0 = ellipse.evaluate(0.0);
        assert_eq!(p0, Point2d::new(5.0, 2.0));

        // At t = π/2 (topmost point)
        let p1 = ellipse.evaluate(PI / 2.0);
        assert!((p1.x - 1.0).abs() < 1e-10);
        assert!((p1.y - 4.0).abs() < 1e-10);

        // At t = π (leftmost point)
        let p2 = ellipse.evaluate(PI);
        assert!((p2.x - (-3.0)).abs() < 1e-10);
        assert!((p2.y - 2.0).abs() < 1e-10);
    }

    #[test]
    fn test_ellipse_contains_point() {
        let ellipse = Ellipse2d::new(Point2d::ORIGIN, 5.0, 3.0, 0.0).unwrap();

        // Points inside
        assert!(ellipse.contains_point(&Point2d::new(0.0, 0.0)));
        assert!(ellipse.contains_point(&Point2d::new(4.0, 0.0)));
        assert!(ellipse.contains_point(&Point2d::new(0.0, 2.0)));

        // Points on boundary
        assert!(ellipse.contains_point(&Point2d::new(5.0, 0.0)));
        assert!(ellipse.contains_point(&Point2d::new(0.0, 3.0)));

        // Points outside
        assert!(!ellipse.contains_point(&Point2d::new(6.0, 0.0)));
        assert!(!ellipse.contains_point(&Point2d::new(0.0, 4.0)));
        assert!(!ellipse.contains_point(&Point2d::new(5.0, 3.0)));
    }

    #[test]
    fn test_rotated_ellipse() {
        let ellipse = Ellipse2d::new(Point2d::ORIGIN, 5.0, 3.0, PI / 4.0).unwrap();

        // Evaluate at t = 0
        let p = ellipse.evaluate(0.0);
        let expected_x = 5.0 * (PI / 4.0).cos();
        let expected_y = 5.0 * (PI / 4.0).sin();
        assert!((p.x - expected_x).abs() < 1e-10);
        assert!((p.y - expected_y).abs() < 1e-10);
    }

    #[test]
    fn test_ellipse_line_intersection() {
        let ellipse = Ellipse2d::new(Point2d::ORIGIN, 5.0, 3.0, 0.0).unwrap();

        // Horizontal line through center
        let intersections = ellipse.intersect_line(&Point2d::ORIGIN, &Vector2d::UNIT_X);
        assert_eq!(intersections.len(), 2);
        assert!(intersections
            .iter()
            .any(|p| (p.x - 5.0).abs() < 1e-10 && p.y.abs() < 1e-10));
        assert!(intersections
            .iter()
            .any(|p| (p.x - (-5.0)).abs() < 1e-10 && p.y.abs() < 1e-10));

        // Line that misses
        let intersections = ellipse.intersect_line(&Point2d::new(0.0, 10.0), &Vector2d::UNIT_X);
        assert_eq!(intersections.len(), 0);

        // Tangent line
        let intersections = ellipse.intersect_line(&Point2d::new(0.0, 3.0), &Vector2d::UNIT_X);
        assert_eq!(intersections.len(), 1);
    }

    #[test]
    fn test_ellipse_bounding_box() {
        // Axis-aligned ellipse
        let ellipse = Ellipse2d::new(Point2d::new(1.0, 2.0), 4.0, 2.0, 0.0).unwrap();
        let (min, max) = ellipse.bounding_box();
        assert_eq!(min, Point2d::new(-3.0, 0.0));
        assert_eq!(max, Point2d::new(5.0, 4.0));

        // Rotated ellipse (45 degrees)
        let ellipse = Ellipse2d::new(Point2d::ORIGIN, 4.0, 2.0, PI / 4.0).unwrap();
        let (min, max) = ellipse.bounding_box();

        // For 45-degree rotation, the bounding box should be larger
        assert!(min.x < -2.8 && min.x > -3.2);
        assert!(min.y < -2.8 && min.y > -3.2);
        assert!(max.x > 2.8 && max.x < 3.2);
        assert!(max.y > 2.8 && max.y < 3.2);
    }
}
