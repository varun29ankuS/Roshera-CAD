//! Analytic quadric-surface-intersection curve (QSIC) for the
//! cylinder–cylinder transversal regime — kernel bug #35 Slices 2–3.
//!
//! # Method (research lineage)
//!
//! Two right circular cylinders are quadrics; their intersection is a
//! degree-4 space curve. Levin's pencil method (J. Levin, *A parametric
//! algorithm for drawing pictures of solid objects composed of quadric
//! surfaces*, CACM 19(10), 1976; and *Mathematical models for determining
//! the intersections of quadric surfaces*, CGIP 11(1), 1979) parameterizes
//! the QSIC by finding a RULED quadric in the pencil `Q_a − λQ_b`,
//! parameterizing its rulings, and resolving the remaining quadratic along
//! each ruling. For two cylinders the rulings of the pencil's ruled
//! surrogate can be taken as the *generators of one operand cylinder
//! itself* (the carrier): a point of the carrier's surface is
//! `p(θ, s) = O_b + r_b(cos θ·û + sin θ·ŵ) + s·b̂`, and substituting into
//! the other cylinder's implicit equation gives the resolvent quadratic
//!
//! ```text
//!   A s² + B(θ) s + C(θ) = 0,       A = 1 − (â·b̂)² = sin²γ,
//!   B(θ) = 2 (q·b̂ − (â·b̂)(q·â)),   C(θ) = |q|² − (q·â)² − r_a²,
//!   q(θ) = O_b + r_b(cos θ·û + sin θ·ŵ) − O_a,
//! ```
//!
//! whose two roots `s±(θ)` are the two BRANCHES of the intersection curve.
//! This is exactly Levin's ruling-resolution step with the carrier cylinder
//! playing the ruled parameterization surface; the discriminant
//! `Δ(θ) = B² − 4AC` is (a trigonometric form of) the degree-4 polynomial
//! whose root structure Levin's method — refined by Wang, Goldman & Tu,
//! *Enhancing Levin's method for computing quadric-surface intersections*,
//! CAGD 20 (2003) 401–422 — uses to separate the curve's components, and
//! whose sign classification matches the Shene–Johnstone lower-degree
//! taxonomy (ACM TOG 13(4), 1994).
//!
//! # Exactness
//!
//! A smooth (non-singular) QSIC has genus 1 and admits **no rational
//! parameterization** (Dupont, Lazard, Lazard & Petitjean, SoCG 2003) —
//! down-converting to a `NurbsCurve` is necessarily approximate (the #89
//! regression class). This type instead evaluates the branch **exactly**
//! (position residual on BOTH cylinders at machine precision), with
//! analytic first and second derivatives, so downstream splitting,
//! welding and tessellation sample true on-surface points.
//!
//! # Regime carried by this type
//!
//! `CylCylQuartic` represents one branch over a θ-interval where
//! `Δ(θ) > 0` strictly (a transversal arc/oval). The full-oval case
//! (`Δ > 0` for all θ — the carrier pierces the other cylinder completely,
//! e.g. perpendicular unequal-radius intersecting bores, Slice 2) is a
//! closed loop per branch. Branch-point-bounded arcs (grazing, Slice 3)
//! use sub-interval instances whose endpoints stop short of `Δ = 0` only
//! through the producer's classification; tangential configurations
//! (`Δ` with a double root — the #86 near-tangency class) are refused by
//! the producer, never constructed here.

use crate::math::{consts, MathError, MathResult, Matrix4, Point3, Tolerance, Vector3};
use crate::primitives::curve::{
    Continuity, Curve, CurveIntersection, CurvePoint, NurbsCurve, ParameterRange,
};

/// One branch of the cylinder–cylinder intersection quartic, parameterized
/// on the CARRIER cylinder's angle `θ` (see module docs). The curve
/// parameter `t ∈ [0,1]` maps affinely to `θ = theta0 + sweep·t`.
#[derive(Debug, Clone)]
pub struct CylCylQuartic {
    /// Resolved cylinder (the one whose implicit equation the resolvent
    /// quadratic solves): a point on its axis.
    pub a_origin: Point3,
    /// Resolved cylinder axis (unit).
    pub a_axis: Vector3,
    /// Resolved cylinder radius.
    pub a_radius: f64,
    /// Carrier cylinder (the curve is θ-parameterized on its surface): a
    /// point on its axis.
    pub b_origin: Point3,
    /// Carrier cylinder axis (unit).
    pub b_axis: Vector3,
    /// Carrier cylinder radius.
    pub b_radius: f64,
    /// Carrier θ-frame: radial direction at θ = 0 (unit, ⟂ `b_axis`).
    pub b_ref: Vector3,
    /// Carrier θ-frame: `b_axis × b_ref` (unit, right-handed).
    pub b_ref2: Vector3,
    /// Which root of the resolvent quadratic: `+1.0` or `-1.0`.
    pub branch: f64,
    /// θ at `t = 0` (radians).
    pub theta0: f64,
    /// Signed θ sweep over `t ∈ [0,1]`; `±2π` for a full closed oval.
    pub sweep: f64,
}

/// Internal: everything the evaluator needs at one θ.
struct BranchEval {
    position: Point3,
    d1: Vector3,
    d2: Vector3,
}

impl CylCylQuartic {
    /// Construct one FULL-OVAL branch (θ over a full period) of the
    /// intersection of the carrier cylinder `(b_origin, b_axis, b_radius)`
    /// with the resolved cylinder `(a_origin, a_axis, a_radius)`.
    ///
    /// Validates that the resolvent discriminant `Δ(θ)` is strictly
    /// positive over a dense θ sample (512 points) with margin — i.e. the
    /// carrier genuinely pierces the resolved cylinder for every generator,
    /// so the branch is a smooth closed loop. Returns
    /// `MathError::InvalidParameter` otherwise (the producer must have
    /// classified the regime before constructing).
    pub fn full_oval(
        a_origin: Point3,
        a_axis: Vector3,
        a_radius: f64,
        b_origin: Point3,
        b_axis: Vector3,
        b_radius: f64,
        branch: f64,
    ) -> MathResult<Self> {
        if a_radius <= 0.0 || b_radius <= 0.0 {
            return Err(MathError::InvalidParameter(
                "CylCylQuartic: radii must be positive".to_string(),
            ));
        }
        if branch != 1.0 && branch != -1.0 {
            return Err(MathError::InvalidParameter(
                "CylCylQuartic: branch must be ±1".to_string(),
            ));
        }
        let a_axis = a_axis.normalize()?;
        let b_axis = b_axis.normalize()?;
        let c = a_axis.dot(&b_axis);
        let a_coef = 1.0 - c * c;
        if a_coef < 1.0e-12 {
            return Err(MathError::InvalidParameter(
                "CylCylQuartic: cylinder axes are (near-)parallel — no transversal quartic"
                    .to_string(),
            ));
        }
        // θ frame on the carrier — identical seed rule to the boolean
        // cylinder splitters so downstream angular bookkeeping agrees.
        let seed = if b_axis.x.abs() < 0.9 {
            Vector3::new(1.0, 0.0, 0.0)
        } else {
            Vector3::new(0.0, 1.0, 0.0)
        };
        let b_ref = b_axis.cross(&seed).normalize()?;
        let b_ref2 = b_axis.cross(&b_ref);

        let candidate = Self {
            a_origin,
            a_axis,
            a_radius,
            b_origin,
            b_axis,
            b_radius,
            b_ref,
            b_ref2,
            branch,
            theta0: 0.0,
            sweep: consts::TWO_PI,
        };
        // Strict-transversality validation: Δ(θ) > margin over a dense
        // sample. The margin is relative to the discriminant's natural
        // scale (r_a² for the leading term) so the check is unit-safe.
        let margin = 1.0e-12 * (a_radius * a_radius).max(1.0);
        let mut min_disc = f64::INFINITY;
        for k in 0..512 {
            let theta = consts::TWO_PI * (k as f64) / 512.0;
            let disc = candidate.discriminant_at_theta(theta);
            min_disc = min_disc.min(disc);
        }
        if min_disc <= margin {
            return Err(MathError::InvalidParameter(format!(
                "CylCylQuartic::full_oval: discriminant min {min_disc:.3e} not strictly \
                 positive — the carrier does not pierce the resolved cylinder on every \
                 generator (not a full-oval regime)"
            )));
        }
        Ok(candidate)
    }

    /// Resolvent discriminant `Δ(θ) = B(θ)² − 4·A·C(θ)` (module docs).
    pub fn discriminant_at_theta(&self, theta: f64) -> f64 {
        let c = self.a_axis.dot(&self.b_axis);
        let a_coef = 1.0 - c * c;
        let q = (self.b_origin - self.a_origin)
            + self.b_ref * (self.b_radius * theta.cos())
            + self.b_ref2 * (self.b_radius * theta.sin());
        let b_coef = 2.0 * (q.dot(&self.b_axis) - c * q.dot(&self.a_axis));
        let c_coef = q.dot(&q) - q.dot(&self.a_axis).powi(2) - self.a_radius * self.a_radius;
        b_coef * b_coef - 4.0 * a_coef * c_coef
    }

    /// θ at curve parameter `t`.
    #[inline]
    pub fn theta_at(&self, t: f64) -> f64 {
        self.theta0 + self.sweep * t
    }

    /// Exact evaluation with analytic first/second derivatives w.r.t. `t`.
    fn eval_branch(&self, t: f64) -> MathResult<BranchEval> {
        let theta = self.theta_at(t);
        let (sin_t, cos_t) = theta.sin_cos();

        let c = self.a_axis.dot(&self.b_axis);
        let a_coef = 1.0 - c * c;

        // Radial part and its θ-derivatives (chain rule: d/dt = sweep·d/dθ).
        let radial = self.b_ref * (self.b_radius * cos_t) + self.b_ref2 * (self.b_radius * sin_t);
        let radial_d1 = (self.b_ref * (-self.b_radius * sin_t)
            + self.b_ref2 * (self.b_radius * cos_t))
            * self.sweep;
        let radial_d2 = radial * (-self.sweep * self.sweep);

        let q = (self.b_origin - self.a_origin) + radial;
        let q1 = radial_d1;
        let q2 = radial_d2;

        let qa = q.dot(&self.a_axis);
        let q1a = q1.dot(&self.a_axis);
        let q2a = q2.dot(&self.a_axis);

        let b_coef = 2.0 * (q.dot(&self.b_axis) - c * qa);
        let b1 = 2.0 * (q1.dot(&self.b_axis) - c * q1a);
        let b2 = 2.0 * (q2.dot(&self.b_axis) - c * q2a);

        let c_coef = q.dot(&q) - qa * qa - self.a_radius * self.a_radius;
        let c1 = 2.0 * q.dot(&q1) - 2.0 * qa * q1a;
        let c2 = 2.0 * (q1.dot(&q1) + q.dot(&q2)) - 2.0 * (q1a * q1a + qa * q2a);

        let disc = b_coef * b_coef - 4.0 * a_coef * c_coef;
        if disc <= 0.0 {
            // Constructed instances guarantee Δ > 0 on their θ-interval; a
            // non-positive value here means the caller evaluated outside a
            // validated domain.
            return Err(MathError::NumericalInstability);
        }
        let sqrt_disc = disc.sqrt();
        let disc1 = 2.0 * b_coef * b1 - 4.0 * a_coef * c1;
        let disc2 = 2.0 * (b1 * b1 + b_coef * b2) - 4.0 * a_coef * c2;

        // s = (−B + branch·√Δ) / (2A) and derivatives.
        let s = (-b_coef + self.branch * sqrt_disc) / (2.0 * a_coef);
        let sqrt_d1 = disc1 / (2.0 * sqrt_disc);
        let sqrt_d2 = disc2 / (2.0 * sqrt_disc) - disc1 * disc1 / (4.0 * disc * sqrt_disc);
        let s1 = (-b1 + self.branch * sqrt_d1) / (2.0 * a_coef);
        let s2 = (-b2 + self.branch * sqrt_d2) / (2.0 * a_coef);

        // P = O_b + radial + s·b̂  (== a_origin + q + s·b̂).
        let position = self.b_origin + radial + self.b_axis * s;
        let d1 = q1 + self.b_axis * s1;
        let d2 = q2 + self.b_axis * s2;
        Ok(BranchEval { position, d1, d2 })
    }

    /// Order-`k` derivative (k ≥ 2) at `t`: analytic at k = 2, central
    /// finite differences of the (k−1)-th derivative above that. Domain
    /// clamping at the chart ends makes the boundary stencils one-sided.
    fn fd_derivative(&self, t: f64, k: usize, h: f64) -> MathResult<Vector3> {
        if k <= 2 {
            return Ok(self.eval_branch(t)?.d2);
        }
        let t_lo = (t - h).max(0.0);
        let t_hi = (t + h).min(1.0);
        let lo = self.fd_derivative(t_lo, k - 1, h)?;
        let hi = self.fd_derivative(t_hi, k - 1, h)?;
        Ok((hi - lo) * (1.0 / (t_hi - t_lo).max(f64::MIN_POSITIVE)))
    }
}

impl Curve for CylCylQuartic {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn evaluate(&self, t: f64) -> MathResult<CurvePoint> {
        let e = self.eval_branch(t)?;
        Ok(CurvePoint {
            position: e.position,
            derivative1: e.d1,
            derivative2: Some(e.d2),
            derivative3: None,
        })
    }

    fn evaluate_derivatives(&self, t: f64, order: usize) -> MathResult<Vec<Vector3>> {
        let e = self.eval_branch(t)?;
        let mut out = Vec::with_capacity(order + 1);
        out.push(Vector3::new(e.position.x, e.position.y, e.position.z));
        if order >= 1 {
            out.push(e.d1);
        }
        if order >= 2 {
            out.push(e.d2);
        }
        // Orders ≥ 3: central finite differences chained ABOVE the analytic
        // second derivative (`fd_derivative`), one FD level per extra order.
        // Precision decays with order (≈h per level); no kernel consumer
        // requests order > 3 today, and this keeps the API total and honest
        // — genuinely computed, never a silent zero.
        if order >= 3 {
            let h = 1.0e-5;
            for k in 3..=order {
                out.push(self.fd_derivative(t, k, h)?);
            }
        }
        Ok(out)
    }

    fn parameter_range(&self) -> ParameterRange {
        ParameterRange::new(0.0, 1.0)
    }

    fn is_closed(&self) -> bool {
        (self.sweep.abs() - consts::TWO_PI).abs() < 1.0e-12
    }

    fn is_periodic(&self) -> bool {
        self.is_closed()
    }

    fn period(&self) -> Option<f64> {
        if self.is_closed() {
            Some(1.0)
        } else {
            None
        }
    }

    fn is_linear(&self, _tolerance: Tolerance) -> bool {
        false
    }

    fn is_planar(&self, _tolerance: Tolerance) -> bool {
        // A transversal cyl-cyl quartic branch is a genuinely spatial curve:
        // planar intersections of two cylinders are the degenerate
        // (conic-splitting) cases, which the equal-radius/coaxial producers
        // own and never route to this type.
        false
    }

    fn get_plane(&self, _tolerance: Tolerance) -> Option<crate::primitives::surface::Plane> {
        None
    }

    fn reversed(&self) -> Box<dyn Curve> {
        let mut rev = self.clone();
        rev.theta0 = self.theta0 + self.sweep;
        rev.sweep = -self.sweep;
        Box::new(rev)
    }

    fn transform(&self, matrix: &Matrix4) -> Box<dyn Curve> {
        // Rigid (or uniform-similarity) transforms preserve the two
        // cylinders and therefore the quartic. Renormalize the frame after
        // mapping; on a degenerate matrix fall back to the untransformed
        // clone (Ellipse precedent).
        let a_origin = matrix.transform_point(&self.a_origin);
        let b_origin = matrix.transform_point(&self.b_origin);
        let scale = matrix
            .transform_vector(&self.b_ref)
            .magnitude()
            .max(1.0e-300);
        let map_dir =
            |v: &Vector3| -> Option<Vector3> { matrix.transform_vector(v).normalize().ok() };
        match (
            map_dir(&self.a_axis),
            map_dir(&self.b_axis),
            map_dir(&self.b_ref),
            map_dir(&self.b_ref2),
        ) {
            (Some(a_axis), Some(b_axis), Some(b_ref), Some(b_ref2)) => Box::new(Self {
                a_origin,
                a_axis,
                a_radius: self.a_radius * scale,
                b_origin,
                b_axis,
                b_radius: self.b_radius * scale,
                b_ref,
                b_ref2,
                branch: self.branch,
                theta0: self.theta0,
                sweep: self.sweep,
            }),
            _ => Box::new(self.clone()),
        }
    }

    fn arc_length_between(&self, t1: f64, t2: f64, _tolerance: Tolerance) -> MathResult<f64> {
        // Composite Simpson on |P'(t)| — smooth on the validated domain.
        let n = 256usize; // even
        let h = (t2 - t1) / (n as f64);
        if h == 0.0 {
            return Ok(0.0);
        }
        let speed = |t: f64| -> MathResult<f64> { Ok(self.eval_branch(t)?.d1.magnitude()) };
        let mut acc = speed(t1)? + speed(t2)?;
        for i in 1..n {
            let w = if i % 2 == 1 { 4.0 } else { 2.0 };
            acc += w * speed(t1 + h * i as f64)?;
        }
        Ok((acc * h / 3.0).abs())
    }

    fn parameter_at_length(&self, length: f64, tolerance: Tolerance) -> MathResult<f64> {
        let total = self.arc_length(tolerance);
        if total <= 0.0 || length <= 0.0 {
            return Ok(0.0);
        }
        if length >= total {
            return Ok(1.0);
        }
        // Cumulative table + linear interpolation (256 spans).
        let n = 256usize;
        let mut acc = 0.0;
        let mut prev = self.eval_branch(0.0)?.position;
        for i in 1..=n {
            let t = i as f64 / n as f64;
            let p = self.eval_branch(t)?.position;
            let seg = (p - prev).magnitude();
            if acc + seg >= length {
                let frac = if seg > 0.0 { (length - acc) / seg } else { 0.0 };
                return Ok((i as f64 - 1.0 + frac) / n as f64);
            }
            acc += seg;
            prev = p;
        }
        Ok(1.0)
    }

    fn closest_point(&self, point: &Point3, _tolerance: Tolerance) -> MathResult<(f64, Point3)> {
        // Dense global sample + golden-section refinement of the winning span.
        let n = 256usize;
        let mut best_i = 0usize;
        let mut best_d = f64::INFINITY;
        for i in 0..=n {
            let t = i as f64 / n as f64;
            let p = self.eval_branch(t)?.position;
            let d = (*point - p).magnitude_squared();
            if d < best_d {
                best_d = d;
                best_i = i;
            }
        }
        let mut lo = ((best_i as f64) - 1.0).max(0.0) / n as f64;
        let mut hi = ((best_i as f64) + 1.0).min(n as f64) / n as f64;
        const PHI: f64 = 0.618_033_988_749_894_9;
        let mut x1 = hi - PHI * (hi - lo);
        let mut x2 = lo + PHI * (hi - lo);
        let dist_sq = |s: &Self, t: f64| -> MathResult<f64> {
            Ok((*point - s.eval_branch(t)?.position).magnitude_squared())
        };
        let mut f1 = dist_sq(self, x1)?;
        let mut f2 = dist_sq(self, x2)?;
        for _ in 0..48 {
            if f1 < f2 {
                hi = x2;
                x2 = x1;
                f2 = f1;
                x1 = hi - PHI * (hi - lo);
                f1 = dist_sq(self, x1)?;
            } else {
                lo = x1;
                x1 = x2;
                f1 = f2;
                x2 = lo + PHI * (hi - lo);
                f2 = dist_sq(self, x2)?;
            }
        }
        let t = 0.5 * (lo + hi);
        Ok((t, self.eval_branch(t)?.position))
    }

    fn parameters_at_point(&self, point: &Point3, tolerance: Tolerance) -> Vec<f64> {
        let n = 512usize;
        let tol = tolerance.distance();
        let mut out: Vec<f64> = Vec::new();
        for i in 0..=n {
            let t = i as f64 / n as f64;
            if let Ok(e) = self.eval_branch(t) {
                if (e.position - *point).magnitude() < tol {
                    if let Some(&last) = out.last() {
                        if (t - last) < 2.0 / n as f64 {
                            continue; // same contact run
                        }
                    }
                    out.push(t);
                }
            }
        }
        out
    }

    fn split(&self, t: f64) -> MathResult<(Box<dyn Curve>, Box<dyn Curve>)> {
        Ok((self.subcurve(0.0, t)?, self.subcurve(t, 1.0)?))
    }

    fn subcurve(&self, t1: f64, t2: f64) -> MathResult<Box<dyn Curve>> {
        if !(0.0..=1.0).contains(&t1) || !(0.0..=1.0).contains(&t2) || t1 >= t2 {
            return Err(MathError::InvalidParameter(format!(
                "CylCylQuartic::subcurve: invalid interval [{t1}, {t2}]"
            )));
        }
        // Exact: the sub-arc is the same analytic branch over a θ-subrange.
        let mut sub = self.clone();
        sub.theta0 = self.theta_at(t1);
        sub.sweep = self.sweep * (t2 - t1);
        Ok(Box::new(sub))
    }

    fn check_continuity(
        &self,
        other: &dyn Curve,
        at_end: bool,
        tolerance: Tolerance,
    ) -> Continuity {
        let self_t = if at_end { 1.0 } else { 0.0 };
        let other_t = if at_end {
            other.parameter_range().end
        } else {
            other.parameter_range().start
        };
        if let (Ok(sp), Ok(op)) = (self.evaluate(self_t), other.evaluate(other_t)) {
            if (sp.position - op.position).magnitude() > tolerance.distance() {
                return Continuity::G0;
            }
            match (sp.tangent(), op.tangent()) {
                (Ok(t1), Ok(t2)) => {
                    let angle = t1.dot(&t2).clamp(-1.0, 1.0).acos();
                    if angle < tolerance.angle() {
                        Continuity::G1
                    } else {
                        Continuity::G0
                    }
                }
                _ => Continuity::G0,
            }
        } else {
            Continuity::G0
        }
    }

    #[allow(clippy::expect_used)] // fallback net below is a fixed, always-valid 2-point linear NURBS
    fn to_nurbs(&self) -> NurbsCurve {
        // A smooth QSIC has genus 1 → NO exact rational form exists (module
        // docs, DLLP 2003); this is a dense cubic APPROXIMATION for
        // consumers that require NURBS (export, generic curve-curve
        // intersection). The boolean pipeline never down-converts — it
        // consumes the analytic evaluation directly.
        let n = 128usize;
        let control_points: Vec<Point3> = (0..=n)
            .map(|i| {
                let t = i as f64 / n as f64;
                self.eval_branch(t)
                    .map(|e| e.position)
                    .unwrap_or(self.b_origin)
            })
            .collect();
        let count = control_points.len();
        let degree = 3usize;
        // Clamped uniform knot vector.
        let mut knots = vec![0.0; degree + 1];
        let interior = count - degree - 1;
        for i in 1..=interior {
            knots.push(i as f64 / (interior as f64 + 1.0));
        }
        knots.extend(std::iter::repeat(1.0).take(degree + 1));
        let weights = vec![1.0; count];
        NurbsCurve::new(degree, control_points, weights, knots).unwrap_or_else(|_| {
            NurbsCurve::new(
                1,
                vec![self.b_origin, self.b_origin + self.b_ref * self.b_radius],
                vec![1.0, 1.0],
                vec![0.0, 0.0, 1.0, 1.0],
            )
            .expect("fallback linear NURBS is statically valid")
        })
    }

    fn type_name(&self) -> &'static str {
        "CylCylQuartic"
    }

    fn bounding_box(&self) -> (Point3, Point3) {
        let n = 128usize;
        let mut min_pt = Point3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
        let mut max_pt = Point3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for i in 0..=n {
            let t = i as f64 / n as f64;
            if let Ok(e) = self.eval_branch(t) {
                min_pt.x = min_pt.x.min(e.position.x);
                min_pt.y = min_pt.y.min(e.position.y);
                min_pt.z = min_pt.z.min(e.position.z);
                max_pt.x = max_pt.x.max(e.position.x);
                max_pt.y = max_pt.y.max(e.position.y);
                max_pt.z = max_pt.z.max(e.position.z);
            }
        }
        // Sampled hull, inflated by the maximal chord sag ~ L²·κ/8 with a
        // conservative curvature bound r_b (carrier) — cheap and safe.
        let pad = {
            let chord = self.sweep.abs() * self.b_radius / n as f64;
            (chord * chord / (8.0 * self.b_radius.max(1.0e-9))).max(1.0e-9)
        };
        (
            Point3::new(min_pt.x - pad, min_pt.y - pad, min_pt.z - pad),
            Point3::new(max_pt.x + pad, max_pt.y + pad, max_pt.z + pad),
        )
    }

    fn intersect_curve(&self, other: &dyn Curve, tolerance: Tolerance) -> Vec<CurveIntersection> {
        // Generic curve-curve intersection via the NURBS approximation
        // (Ellipse precedent); the boolean pipeline does not consume this
        // for quartic cuts (crossing bookkeeping is analytic there).
        self.to_nurbs().intersect_curve(other, tolerance)
    }

    fn intersect_plane(
        &self,
        plane: &crate::primitives::surface::Plane,
        _tolerance: Tolerance,
    ) -> Vec<f64> {
        // Sample the signed plane distance and bisect each sign change —
        // robust for the transversal crossings this curve exhibits.
        let n = 512usize;
        let dist = |t: f64| -> Option<f64> {
            self.eval_branch(t)
                .ok()
                .map(|e| plane.distance_to_point(&e.position))
        };
        let mut out = Vec::new();
        let mut prev_t = 0.0;
        let Some(mut prev_d) = dist(0.0) else {
            return out;
        };
        for i in 1..=n {
            let t = i as f64 / n as f64;
            let Some(d) = dist(t) else { continue };
            if prev_d == 0.0 {
                out.push(prev_t);
            } else if prev_d * d < 0.0 {
                let (mut lo, mut hi, mut flo) = (prev_t, t, prev_d);
                for _ in 0..60 {
                    let mid = 0.5 * (lo + hi);
                    let Some(fm) = dist(mid) else { break };
                    if flo * fm <= 0.0 {
                        hi = mid;
                    } else {
                        lo = mid;
                        flo = fm;
                    }
                }
                out.push(0.5 * (lo + hi));
            }
            prev_t = t;
            prev_d = d;
        }
        out
    }

    fn project_point(&self, point: &Point3, tolerance: Tolerance) -> Vec<(f64, Point3)> {
        match self.closest_point(point, tolerance) {
            Ok(hit) => vec![hit],
            Err(_) => vec![],
        }
    }

    fn offset(&self, distance: f64, normal: &Vector3) -> MathResult<Box<dyn Curve>> {
        self.to_nurbs().offset(distance, normal)
    }

    fn clone_box(&self) -> Box<dyn Curve> {
        Box::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Perpendicular unequal-radius intersecting-axes pair (the Slice-2
    /// regime): carrier r=5 along X through (80,15,10); resolved r=8 along
    /// Z through (80,15,0). Both branches must be smooth closed loops lying
    /// exactly on BOTH cylinders.
    fn slice2_pair(branch: f64) -> CylCylQuartic {
        #[allow(clippy::expect_used)] // fixed valid fixture
        CylCylQuartic::full_oval(
            Point3::new(80.0, 15.0, 0.0),
            Vector3::Z,
            8.0,
            Point3::new(55.0, 15.0, 10.0),
            Vector3::X,
            5.0,
            branch,
        )
        .expect("slice-2 regime is a valid full oval")
    }

    #[test]
    fn full_oval_lies_on_both_cylinders_to_machine_precision() {
        for branch in [1.0, -1.0] {
            let q = slice2_pair(branch);
            let mut max_a = 0.0_f64;
            let mut max_b = 0.0_f64;
            for i in 0..=1024 {
                let t = i as f64 / 1024.0;
                let p = q.evaluate(t).expect("eval").position;
                let da = {
                    let d = p - q.a_origin;
                    let ax = d.dot(&q.a_axis);
                    ((d - q.a_axis * ax).magnitude() - q.a_radius).abs()
                };
                let db = {
                    let d = p - q.b_origin;
                    let ax = d.dot(&q.b_axis);
                    ((d - q.b_axis * ax).magnitude() - q.b_radius).abs()
                };
                max_a = max_a.max(da);
                max_b = max_b.max(db);
            }
            assert!(
                max_a < 1.0e-10 && max_b < 1.0e-10,
                "branch {branch}: residuals A={max_a:.3e} B={max_b:.3e}"
            );
        }
    }

    #[test]
    fn derivatives_match_finite_differences() {
        let q = slice2_pair(1.0);
        let h = 1.0e-7;
        for i in 1..16 {
            let t = i as f64 / 16.0;
            let e = q.evaluate(t).expect("eval");
            let p_lo = q.evaluate(t - h).expect("eval").position;
            let p_hi = q.evaluate(t + h).expect("eval").position;
            let fd1 = (p_hi - p_lo) * (1.0 / (2.0 * h));
            let err1 = (fd1 - e.derivative1).magnitude() / e.derivative1.magnitude().max(1.0);
            assert!(err1 < 1.0e-5, "t={t}: d1 FD mismatch {err1:.3e}");
            let d1_lo = q.evaluate(t - h).expect("eval").derivative1;
            let d1_hi = q.evaluate(t + h).expect("eval").derivative1;
            let fd2 = (d1_hi - d1_lo) * (1.0 / (2.0 * h));
            let d2 = e.derivative2.expect("d2 present");
            let err2 = (fd2 - d2).magnitude() / d2.magnitude().max(1.0);
            assert!(err2 < 1.0e-4, "t={t}: d2 FD mismatch {err2:.3e}");
        }
    }

    #[test]
    fn closure_reversal_and_subcurve_are_consistent() {
        let q = slice2_pair(-1.0);
        assert!(q.is_closed());
        let p0 = q.evaluate(0.0).expect("eval").position;
        let p1 = q.evaluate(1.0).expect("eval").position;
        assert!((p0 - p1).magnitude() < 1.0e-12, "closed loop endpoints");

        let r = q.reversed();
        for i in 0..=8 {
            let t = i as f64 / 8.0;
            let a = q.evaluate(t).expect("eval").position;
            let b = r.evaluate(1.0 - t).expect("eval").position;
            assert!((a - b).magnitude() < 1.0e-12, "reversal at t={t}");
        }

        let sub = q.subcurve(0.25, 0.5).expect("subcurve");
        for i in 0..=8 {
            let t = i as f64 / 8.0;
            let a = sub.evaluate(t).expect("eval").position;
            let b = q.evaluate(0.25 + 0.25 * t).expect("eval").position;
            assert!((a - b).magnitude() < 1.0e-12, "subcurve at t={t}");
        }
        assert!(!sub.is_closed());
    }

    #[test]
    fn full_oval_refuses_non_piercing_regime() {
        // Carrier r=5 along X, axis offset 9 in Y (the direction ⟂ BOTH
        // axes) from a resolved r=8 axis along Z: carrier generators sit at
        // y(θ) = 24 + 5·cos-component ∈ [19, 29]; those with y > 23 MISS
        // the resolved cylinder (|y−15| > 8) → Δ < 0 there → grazing, not a
        // full oval. (An offset ALONG the resolved axis would be invisible —
        // the implicit cylinder is unbounded in z.)
        let res = CylCylQuartic::full_oval(
            Point3::new(80.0, 15.0, 0.0),
            Vector3::Z,
            8.0,
            Point3::new(55.0, 24.0, 10.0),
            Vector3::X,
            5.0,
            1.0,
        );
        assert!(res.is_err(), "grazing/non-piercing must be refused");
    }
}
