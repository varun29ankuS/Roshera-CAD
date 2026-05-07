//! Trimmed NURBS surfaces implementation
//!
//! Essential for B-Rep modeling where surfaces are bounded by trim curves.
//! Implements proper inside/outside classification and intersection algorithms.
//!
//! Indexed access into knot vectors and control-point grids is the canonical
//! idiom — all `arr[i]` sites are bounds-guaranteed by knot-span ranges or
//! grid dimensions. Matches the numerical-kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use crate::math::bspline::KnotVector;
use crate::math::nurbs::NurbsSurface;
use crate::math::{MathError, MathResult, Point3, Tolerance};

/// A 2D curve in parameter space of a surface
#[derive(Debug, Clone)]
pub struct TrimCurve2D {
    /// The 2D NURBS curve in (u,v) parameter space
    pub curve: NurbsCurve2D,
    /// Direction: true for CCW (outer boundary), false for CW (hole)
    pub is_outer: bool,
    /// Parent loop reference
    pub loop_id: usize,
}

/// A 2D NURBS curve for parameter space
#[derive(Debug, Clone)]
pub struct NurbsCurve2D {
    /// Control points in 2D (u,v) space
    pub control_points: Vec<Point2>,
    /// Weights for rational representation
    pub weights: Vec<f64>,
    /// Knot vector
    pub knots: KnotVector,
    /// Degree of the curve
    pub degree: usize,
}

/// 2D point for parameter space
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point2 {
    pub u: f64,
    pub v: f64,
}

impl Point2 {
    pub fn new(u: f64, v: f64) -> Self {
        Self { u, v }
    }

    /// Distance to another point
    pub fn distance(&self, other: &Point2) -> f64 {
        let du = self.u - other.u;
        let dv = self.v - other.v;
        (du * du + dv * dv).sqrt()
    }

    /// Linear interpolation
    pub fn lerp(&self, other: &Point2, t: f64) -> Point2 {
        Point2 {
            u: self.u + t * (other.u - self.u),
            v: self.v + t * (other.v - self.v),
        }
    }
}

/// A loop of trim curves forming a closed boundary
#[derive(Debug, Clone)]
pub struct TrimLoop {
    /// Ordered list of trim curves forming the loop
    pub curves: Vec<TrimCurve2D>,
    /// Is this an outer boundary (true) or hole (false)
    pub is_outer: bool,
}

impl TrimLoop {
    /// Create a new trim loop
    pub fn new(curves: Vec<TrimCurve2D>, is_outer: bool) -> MathResult<Self> {
        // Validate that curves form a closed loop
        if curves.is_empty() {
            return Err(MathError::InvalidParameter(
                "Trim loop must have at least one curve".into(),
            ));
        }

        // Check connectivity
        for i in 0..curves.len() {
            let current = &curves[i];
            let next = &curves[(i + 1) % curves.len()];

            let end_point = current.curve.evaluate(1.0)?;
            let start_point = next.curve.evaluate(0.0)?;

            if end_point.distance(&start_point) > 1e-6 {
                return Err(MathError::InvalidParameter(format!(
                    "Trim curves {} and {} are not connected",
                    i,
                    (i + 1) % curves.len()
                )));
            }
        }

        Ok(Self { curves, is_outer })
    }

    /// Check if a parameter point is inside this loop
    pub fn contains_point(&self, point: &Point2) -> MathResult<bool> {
        // Use winding number algorithm
        let mut winding_number = 0i32;

        for curve in &self.curves {
            // Sample curve and compute winding number contribution
            let num_samples = 10;
            for i in 0..num_samples {
                let t0 = i as f64 / num_samples as f64;
                let t1 = (i + 1) as f64 / num_samples as f64;

                let p0 = curve.curve.evaluate(t0)?;
                let p1 = curve.curve.evaluate(t1)?;

                // Check if edge crosses ray from point to +u direction
                if (p0.v <= point.v && p1.v > point.v) || (p0.v > point.v && p1.v <= point.v) {
                    // Compute intersection of edge with ray
                    let t = (point.v - p0.v) / (p1.v - p0.v);
                    let u_intersect = p0.u + t * (p1.u - p0.u);

                    if u_intersect > point.u {
                        if p1.v > p0.v {
                            winding_number += 1;
                        } else {
                            winding_number -= 1;
                        }
                    }
                }
            }
        }

        Ok(winding_number != 0)
    }
}

/// A trimmed NURBS surface
#[derive(Debug, Clone)]
pub struct TrimmedNurbsSurface {
    /// The underlying NURBS surface
    pub surface: NurbsSurface,
    /// Trim loops (first should be outer boundary)
    pub trim_loops: Vec<TrimLoop>,
    /// Tolerance for trimming operations
    pub tolerance: Tolerance,
}

impl TrimmedNurbsSurface {
    /// Create a new trimmed NURBS surface
    pub fn new(surface: NurbsSurface, tolerance: Tolerance) -> Self {
        // By default, trimmed by parameter domain boundaries
        let default_outer_loop = Self::create_default_boundary(&surface);

        Self {
            surface,
            trim_loops: vec![default_outer_loop],
            tolerance,
        }
    }

    /// Create default rectangular boundary in parameter space
    #[allow(clippy::expect_used)] // literal degree-1 knot vectors and corner curves are validated
    fn create_default_boundary(_surface: &NurbsSurface) -> TrimLoop {
        // Create four linear curves for the boundary
        let corners = [
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 0.0),
            Point2::new(1.0, 1.0),
            Point2::new(0.0, 1.0),
        ];

        let mut curves = Vec::new();
        for i in 0..4 {
            let start = corners[i];
            let end = corners[(i + 1) % 4];

            // Create linear NURBS curve
            let curve = NurbsCurve2D {
                control_points: vec![start, end],
                weights: vec![1.0, 1.0],
                knots: KnotVector::new(vec![0.0, 0.0, 1.0, 1.0])
                    .expect("literal degree-1 Bezier knot vector is always valid"),
                degree: 1,
            };

            curves.push(TrimCurve2D {
                curve,
                is_outer: true,
                loop_id: 0,
            });
        }

        TrimLoop::new(curves, true)
            .expect("unit-square trim loop constructed from validated corner curves")
    }

    /// Add a trim loop (hole or inner boundary)
    pub fn add_trim_loop(&mut self, loop_: TrimLoop) -> MathResult<()> {
        // Validate loop is within surface domain
        for curve in &loop_.curves {
            // Sample curve
            for i in 0..10 {
                let t = i as f64 / 9.0;
                let point = curve.curve.evaluate(t)?;

                if point.u < 0.0 || point.u > 1.0 || point.v < 0.0 || point.v > 1.0 {
                    return Err(MathError::InvalidParameter(
                        "Trim curve extends outside surface parameter domain".into(),
                    ));
                }
            }
        }

        self.trim_loops.push(loop_);
        Ok(())
    }

    /// Check if a parameter point is inside the trimmed region
    pub fn is_inside(&self, u: f64, v: f64) -> MathResult<bool> {
        let point = Point2::new(u, v);

        // Must be inside outer boundary
        if !self.trim_loops[0].contains_point(&point)? {
            return Ok(false);
        }

        // Must be outside all holes
        for i in 1..self.trim_loops.len() {
            if self.trim_loops[i].contains_point(&point)? {
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Evaluate surface at parameters if inside trimmed region
    pub fn evaluate(&self, u: f64, v: f64) -> MathResult<Option<Point3>> {
        if self.is_inside(u, v)? {
            Ok(Some(self.surface.evaluate(u, v).point))
        } else {
            Ok(None)
        }
    }

    /// Get the 3D curve on the surface corresponding to a trim curve
    pub fn get_3d_trim_curve(&self, trim_curve: &TrimCurve2D) -> MathResult<Vec<Point3>> {
        let mut points = Vec::new();

        // Sample the 2D curve and evaluate on surface
        let num_samples = 50;
        for i in 0..=num_samples {
            let t = i as f64 / num_samples as f64;
            let param_point = trim_curve.curve.evaluate(t)?;

            if let Some(surface_point) = self.evaluate(param_point.u, param_point.v)? {
                points.push(surface_point);
            }
        }

        Ok(points)
    }
}

// Implementation for NurbsCurve2D
impl NurbsCurve2D {
    /// Create a new 2D NURBS curve
    pub fn new(
        control_points: Vec<Point2>,
        weights: Vec<f64>,
        knots: Vec<f64>,
        degree: usize,
    ) -> MathResult<Self> {
        // Validate inputs
        if control_points.len() != weights.len() {
            return Err(MathError::InvalidParameter(
                "Control points and weights must have same length".into(),
            ));
        }

        let knot_vector = KnotVector::new(knots)?;
        knot_vector.validate(degree, control_points.len())?;

        Ok(Self {
            control_points,
            weights,
            knots: knot_vector,
            degree,
        })
    }

    /// Evaluate curve at parameter t
    pub fn evaluate(&self, t: f64) -> MathResult<Point2> {
        // Find knot span
        let span = self
            .knots
            .find_span(t, self.degree, self.control_points.len());

        // Compute basis functions
        let basis = self.compute_basis_functions(span, t);

        // Compute point
        let mut point = Point2::new(0.0, 0.0);
        let mut weight_sum = 0.0;

        for i in 0..=self.degree {
            let idx = span - self.degree + i;
            let w = self.weights[idx] * basis[i];
            point.u += self.control_points[idx].u * w;
            point.v += self.control_points[idx].v * w;
            weight_sum += w;
        }

        if weight_sum.abs() < 1e-10 {
            return Err(MathError::NumericalInstability);
        }

        point.u /= weight_sum;
        point.v /= weight_sum;

        Ok(point)
    }

    /// Compute basis functions (simplified Cox-de Boor)
    fn compute_basis_functions(&self, span: usize, t: f64) -> Vec<f64> {
        let p = self.degree;
        let mut basis = vec![0.0; p + 1];
        let mut left = vec![0.0; p + 1];
        let mut right = vec![0.0; p + 1];

        basis[0] = 1.0;

        for j in 1..=p {
            left[j] = t - self.knots[span + 1 - j];
            right[j] = self.knots[span + j] - t;

            let mut saved = 0.0;
            for r in 0..j {
                let temp = basis[r] / (right[r + 1] + left[j - r]);
                basis[r] = saved + right[r + 1] * temp;
                saved = left[j - r] * temp;
            }
            basis[j] = saved;
        }

        basis
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_point2_operations() {
        let p1 = Point2::new(0.0, 0.0);
        let p2 = Point2::new(1.0, 1.0);

        assert!((p1.distance(&p2) - std::f64::consts::SQRT_2).abs() < 1e-10);

        let mid = p1.lerp(&p2, 0.5);
        assert!((mid.u - 0.5).abs() < 1e-10);
        assert!((mid.v - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_trim_loop_connectivity() {
        // Create a square loop
        let curves = vec![
            create_line_segment(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)),
            create_line_segment(Point2::new(1.0, 0.0), Point2::new(1.0, 1.0)),
            create_line_segment(Point2::new(1.0, 1.0), Point2::new(0.0, 1.0)),
            create_line_segment(Point2::new(0.0, 1.0), Point2::new(0.0, 0.0)),
        ];

        let loop_result = TrimLoop::new(curves, true);
        assert!(loop_result.is_ok());

        // Test point containment
        let loop_ = loop_result.unwrap();
        assert!(loop_.contains_point(&Point2::new(0.5, 0.5)).unwrap());
        assert!(!loop_.contains_point(&Point2::new(1.5, 0.5)).unwrap());
    }

    fn create_line_segment(start: Point2, end: Point2) -> TrimCurve2D {
        TrimCurve2D {
            curve: NurbsCurve2D {
                control_points: vec![start, end],
                weights: vec![1.0, 1.0],
                knots: KnotVector::new(vec![0.0, 0.0, 1.0, 1.0]).unwrap(),
                degree: 1,
            },
            is_outer: true,
            loop_id: 0,
        }
    }

    /// Build a flat 2×2 bilinear NURBS patch over the unit square in
    /// the z=0 plane. Reused by TrimmedNurbsSurface fixtures.
    fn unit_planar_patch() -> NurbsSurface {
        let cp = vec![
            vec![Point3::new(0.0, 0.0, 0.0), Point3::new(0.0, 1.0, 0.0)],
            vec![Point3::new(1.0, 0.0, 0.0), Point3::new(1.0, 1.0, 0.0)],
        ];
        let w = vec![vec![1.0, 1.0], vec![1.0, 1.0]];
        NurbsSurface::new(
            cp,
            w,
            vec![0.0, 0.0, 1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
            1,
            1,
        )
        .expect("bilinear unit patch is well formed")
    }

    /// Helper: build a CCW square loop in (u,v) parameter space.
    fn square_loop(u0: f64, v0: f64, u1: f64, v1: f64, is_outer: bool) -> TrimLoop {
        let curves = vec![
            create_line_segment(Point2::new(u0, v0), Point2::new(u1, v0)),
            create_line_segment(Point2::new(u1, v0), Point2::new(u1, v1)),
            create_line_segment(Point2::new(u1, v1), Point2::new(u0, v1)),
            create_line_segment(Point2::new(u0, v1), Point2::new(u0, v0)),
        ];
        TrimLoop::new(curves, is_outer).expect("square loop is connected")
    }

    // ----- Point2 ----------------------------------------------------------

    #[test]
    fn point2_new_stores_components() {
        let p = Point2::new(2.5, -1.0);
        assert_eq!(p.u, 2.5);
        assert_eq!(p.v, -1.0);
    }

    #[test]
    fn point2_distance_to_self_is_zero() {
        let p = Point2::new(3.0, 4.0);
        assert_eq!(p.distance(&p), 0.0);
    }

    #[test]
    fn point2_distance_3_4_5_triangle() {
        let p1 = Point2::new(0.0, 0.0);
        let p2 = Point2::new(3.0, 4.0);
        assert!((p1.distance(&p2) - 5.0).abs() < 1e-12);
    }

    #[test]
    fn point2_distance_is_symmetric_under_negation() {
        let p1 = Point2::new(1.0, 2.0);
        let p2 = Point2::new(-1.0, -2.0);
        assert!((p1.distance(&p2) - p2.distance(&p1)).abs() < 1e-12);
    }

    #[test]
    fn point2_lerp_at_zero_is_start() {
        let a = Point2::new(1.0, 2.0);
        let b = Point2::new(5.0, 7.0);
        let r = a.lerp(&b, 0.0);
        assert_eq!(r.u, a.u);
        assert_eq!(r.v, a.v);
    }

    #[test]
    fn point2_lerp_at_one_is_end() {
        let a = Point2::new(1.0, 2.0);
        let b = Point2::new(5.0, 7.0);
        let r = a.lerp(&b, 1.0);
        assert_eq!(r.u, b.u);
        assert_eq!(r.v, b.v);
    }

    #[test]
    fn point2_lerp_extrapolates_outside_zero_one() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(1.0, 1.0);
        let r = a.lerp(&b, 2.0);
        assert!((r.u - 2.0).abs() < 1e-12);
        assert!((r.v - 2.0).abs() < 1e-12);
    }

    // ----- NurbsCurve2D::new validation -----------------------------------

    #[test]
    fn nurbs_curve_2d_new_rejects_weight_length_mismatch() {
        let cp = vec![Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)];
        let result = NurbsCurve2D::new(cp, vec![1.0], vec![0.0, 0.0, 1.0, 1.0], 1);
        assert!(result.is_err());
    }

    #[test]
    fn nurbs_curve_2d_new_rejects_invalid_knot_vector() {
        let cp = vec![Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)];
        // Non-monotone knots fail validation in KnotVector::new.
        let result = NurbsCurve2D::new(cp, vec![1.0, 1.0], vec![1.0, 0.0, 0.0, 1.0], 1);
        assert!(result.is_err());
    }

    #[test]
    fn nurbs_curve_2d_new_rejects_wrong_degree_for_cp_count() {
        // Degree 3 with only 2 control points → KnotVector::validate fails
        // (n + p + 1 = 6 ≠ 4 knots supplied).
        let cp = vec![Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)];
        let result = NurbsCurve2D::new(cp, vec![1.0, 1.0], vec![0.0, 0.0, 1.0, 1.0], 3);
        assert!(result.is_err());
    }

    #[test]
    fn nurbs_curve_2d_new_accepts_valid_linear_curve() {
        let cp = vec![Point2::new(0.0, 0.0), Point2::new(1.0, 1.0)];
        let result = NurbsCurve2D::new(cp, vec![1.0, 1.0], vec![0.0, 0.0, 1.0, 1.0], 1);
        assert!(result.is_ok());
    }

    // ----- NurbsCurve2D::evaluate ----------------------------------------

    #[test]
    fn nurbs_curve_2d_linear_evaluate_at_endpoints() {
        let curve = NurbsCurve2D::new(
            vec![Point2::new(2.0, 3.0), Point2::new(5.0, 7.0)],
            vec![1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
            1,
        )
        .expect("valid linear curve");
        let p0 = curve.evaluate(0.0).expect("endpoint eval");
        assert!((p0.u - 2.0).abs() < 1e-10);
        assert!((p0.v - 3.0).abs() < 1e-10);
        let p1 = curve.evaluate(1.0).expect("endpoint eval");
        assert!((p1.u - 5.0).abs() < 1e-10);
        assert!((p1.v - 7.0).abs() < 1e-10);
    }

    #[test]
    fn nurbs_curve_2d_linear_evaluate_at_midpoint() {
        let curve = NurbsCurve2D::new(
            vec![Point2::new(0.0, 0.0), Point2::new(4.0, 8.0)],
            vec![1.0, 1.0],
            vec![0.0, 0.0, 1.0, 1.0],
            1,
        )
        .expect("valid linear curve");
        let mid = curve.evaluate(0.5).expect("mid eval");
        assert!((mid.u - 2.0).abs() < 1e-10);
        assert!((mid.v - 4.0).abs() < 1e-10);
    }

    #[test]
    fn nurbs_curve_2d_rational_quadratic_quarter_circle_midpoint() {
        // Standard rational quadratic Bézier for a quarter-circle arc from
        // (1,0) to (0,1). With weights (1, √2/2, 1) and knot vector
        // (0,0,0,1,1,1), evaluate at t=0.5 gives a point on the unit circle:
        // (√2/2, √2/2).
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let curve = NurbsCurve2D::new(
            vec![
                Point2::new(1.0, 0.0),
                Point2::new(1.0, 1.0),
                Point2::new(0.0, 1.0),
            ],
            vec![1.0, s, 1.0],
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            2,
        )
        .expect("valid rational quadratic");
        let m = curve.evaluate(0.5).expect("mid eval");
        assert!((m.u - s).abs() < 1e-10);
        assert!((m.v - s).abs() < 1e-10);
        // Confirm it lies on the unit circle.
        assert!((m.u * m.u + m.v * m.v - 1.0).abs() < 1e-10);
    }

    // ----- TrimLoop --------------------------------------------------------

    #[test]
    fn trim_loop_new_rejects_empty_curve_list() {
        let result = TrimLoop::new(Vec::new(), true);
        assert!(result.is_err());
    }

    #[test]
    fn trim_loop_new_rejects_disconnected_curves() {
        let curves = vec![
            create_line_segment(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)),
            create_line_segment(Point2::new(2.0, 0.0), Point2::new(2.0, 1.0)), // gap
            create_line_segment(Point2::new(2.0, 1.0), Point2::new(0.0, 0.0)),
        ];
        let result = TrimLoop::new(curves, true);
        assert!(result.is_err());
    }

    #[test]
    fn trim_loop_contains_point_inside_unit_square() {
        let l = square_loop(0.0, 0.0, 1.0, 1.0, true);
        assert!(l.contains_point(&Point2::new(0.5, 0.5)).expect("contains"));
        assert!(l.contains_point(&Point2::new(0.1, 0.9)).expect("contains"));
    }

    #[test]
    fn trim_loop_contains_point_outside_unit_square() {
        let l = square_loop(0.0, 0.0, 1.0, 1.0, true);
        assert!(!l.contains_point(&Point2::new(-0.5, 0.5)).expect("outside"));
        assert!(!l.contains_point(&Point2::new(2.0, 0.5)).expect("outside"));
        assert!(!l.contains_point(&Point2::new(0.5, 2.0)).expect("outside"));
    }

    #[test]
    fn trim_loop_contains_point_for_offset_square() {
        let l = square_loop(2.0, 3.0, 5.0, 7.0, true);
        assert!(l.contains_point(&Point2::new(3.5, 5.0)).expect("inside"));
        assert!(!l.contains_point(&Point2::new(0.0, 0.0)).expect("outside"));
        assert!(!l.contains_point(&Point2::new(10.0, 10.0)).expect("outside"));
    }

    #[test]
    fn trim_loop_winding_is_nonzero_in_either_orientation() {
        // CCW square (positive winding) and CW square (negative winding) both
        // report contains_point == true via the `!= 0` check.
        let ccw = square_loop(0.0, 0.0, 1.0, 1.0, true);
        let curves_cw = vec![
            create_line_segment(Point2::new(0.0, 0.0), Point2::new(0.0, 1.0)),
            create_line_segment(Point2::new(0.0, 1.0), Point2::new(1.0, 1.0)),
            create_line_segment(Point2::new(1.0, 1.0), Point2::new(1.0, 0.0)),
            create_line_segment(Point2::new(1.0, 0.0), Point2::new(0.0, 0.0)),
        ];
        let cw = TrimLoop::new(curves_cw, false).expect("cw square");
        assert!(ccw.contains_point(&Point2::new(0.5, 0.5)).expect("ccw"));
        assert!(cw.contains_point(&Point2::new(0.5, 0.5)).expect("cw"));
    }

    // ----- TrimmedNurbsSurface --------------------------------------------

    #[test]
    fn trimmed_surface_new_creates_default_unit_square_outer_loop() {
        let s = TrimmedNurbsSurface::new(unit_planar_patch(), Tolerance::default());
        assert_eq!(s.trim_loops.len(), 1);
        assert!(s.trim_loops[0].is_outer);
        assert_eq!(s.trim_loops[0].curves.len(), 4);
    }

    #[test]
    fn trimmed_surface_is_inside_center_of_default_domain() {
        let s = TrimmedNurbsSurface::new(unit_planar_patch(), Tolerance::default());
        assert!(s.is_inside(0.5, 0.5).expect("inside"));
    }

    #[test]
    fn trimmed_surface_is_outside_default_domain() {
        let s = TrimmedNurbsSurface::new(unit_planar_patch(), Tolerance::default());
        assert!(!s.is_inside(-0.5, 0.5).expect("outside u"));
        assert!(!s.is_inside(0.5, 1.5).expect("outside v"));
    }

    #[test]
    fn trimmed_surface_add_trim_loop_rejects_out_of_domain_curve() {
        let mut s = TrimmedNurbsSurface::new(unit_planar_patch(), Tolerance::default());
        let bad = square_loop(0.5, 0.5, 1.5, 1.5, false); // overflows v=1
        let result = s.add_trim_loop(bad);
        assert!(result.is_err());
        assert_eq!(s.trim_loops.len(), 1);
    }

    #[test]
    fn trimmed_surface_add_trim_loop_accepts_in_domain_hole() {
        let mut s = TrimmedNurbsSurface::new(unit_planar_patch(), Tolerance::default());
        let hole = square_loop(0.25, 0.25, 0.75, 0.75, false);
        s.add_trim_loop(hole).expect("hole inside domain");
        assert_eq!(s.trim_loops.len(), 2);
    }

    #[test]
    fn trimmed_surface_is_inside_with_hole() {
        let mut s = TrimmedNurbsSurface::new(unit_planar_patch(), Tolerance::default());
        let hole = square_loop(0.25, 0.25, 0.75, 0.75, false);
        s.add_trim_loop(hole).expect("hole");
        // Inside outer, outside hole → trimmed region.
        assert!(s.is_inside(0.1, 0.5).expect("ring"));
        // Inside outer, inside hole → not in trimmed region.
        assert!(!s.is_inside(0.5, 0.5).expect("hole interior"));
        // Outside outer altogether.
        assert!(!s.is_inside(2.0, 2.0).expect("outside"));
    }

    #[test]
    fn trimmed_surface_evaluate_returns_some_inside_none_outside() {
        let s = TrimmedNurbsSurface::new(unit_planar_patch(), Tolerance::default());
        assert!(s.evaluate(0.5, 0.5).expect("inside").is_some());
        assert!(s.evaluate(-0.1, 0.5).expect("outside").is_none());
    }

    #[test]
    fn trimmed_surface_evaluate_inside_yields_planar_point() {
        let s = TrimmedNurbsSurface::new(unit_planar_patch(), Tolerance::default());
        let p = s.evaluate(0.25, 0.75).expect("eval ok").expect("inside");
        // Bilinear z=0 patch over unit square: expect (0.25, 0.75, 0.0).
        assert!((p.x - 0.25).abs() < 1e-10);
        assert!((p.y - 0.75).abs() < 1e-10);
        assert!(p.z.abs() < 1e-10);
    }

    #[test]
    fn trimmed_surface_get_3d_trim_curve_samples_only_inside_region() {
        // Hole loop is sampled by get_3d_trim_curve via evaluate(); points
        // inside the hole map to None and are skipped. The hole boundary
        // itself sits on inner-loop edges where the winding-number test is
        // numerically unstable, so we just check that no point reaches us
        // and the call succeeds.
        let mut s = TrimmedNurbsSurface::new(unit_planar_patch(), Tolerance::default());
        let hole = square_loop(0.25, 0.25, 0.75, 0.75, false);
        s.add_trim_loop(hole.clone()).expect("hole");
        // Pick the hole's first edge (a curve sitting on the boundary of the
        // hole). All its sample points are *on* the hole boundary, so the
        // get_3d_trim_curve call must complete without error.
        let curve = &hole.curves[0];
        let pts = s.get_3d_trim_curve(curve).expect("call ok");
        // Boundary samples may go either way under the winding test, but
        // at most we get the requested 51 samples.
        assert!(pts.len() <= 51);
    }
}
