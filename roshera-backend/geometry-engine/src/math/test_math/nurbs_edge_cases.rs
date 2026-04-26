//! Edge case tests for NURBS curves and surfaces
//! Tests numerical stability and error conditions

use crate::math::nurbs::*;
use crate::math::*;

#[cfg(test)]
mod nurbs_edge_cases {
    use super::*;

    #[test]
    fn test_nurbs_creation_validation() {
        // Empty control points
        let empty_points = vec![];
        let weights = vec![1.0];
        let knots = vec![0.0, 1.0];
        let result = NurbsCurve::new(empty_points, weights, knots, 1);
        assert!(result.is_err());

        // Mismatched weights
        let points = vec![Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0)];
        let bad_weights = vec![1.0]; // Should be 2
        let knots = vec![0.0, 0.0, 1.0, 1.0];
        let result2 = NurbsCurve::new(points.clone(), bad_weights, knots.clone(), 1);
        assert!(result2.is_err());

        // Invalid knot vector (not non-decreasing)
        let good_weights = vec![1.0, 1.0];
        let bad_knots = vec![0.0, 1.0, 0.5, 1.0];
        let result3 = NurbsCurve::new(points.clone(), good_weights.clone(), bad_knots, 1);
        assert!(result3.is_err());

        // Degree too high for number of control points
        let result4 = NurbsCurve::new(points, good_weights, knots, 5);
        assert!(result4.is_err());
    }

    #[test]
    fn test_nurbs_evaluation_edge_cases() {
        // Create a simple linear NURBS curve
        let points = vec![Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0)];
        let weights = vec![1.0, 1.0];
        let knots = vec![0.0, 0.0, 1.0, 1.0];
        let curve = NurbsCurve::new(points, weights, knots, 1)
            .expect("valid NURBS curve should be created");

        // Evaluate at parameter limits
        let p0 = curve.evaluate(0.0);
        assert_eq!(p0.point, Point3::new(0.0, 0.0, 0.0));

        let p1 = curve.evaluate(1.0);
        assert_eq!(p1.point, Point3::new(1.0, 0.0, 0.0));

        // Evaluate outside parameter range (should clamp)
        let p_neg = curve.evaluate(-0.5);
        assert_eq!(p_neg.point, Point3::new(0.0, 0.0, 0.0));

        let p_big = curve.evaluate(1.5);
        assert_eq!(p_big.point, Point3::new(1.0, 0.0, 0.0));
    }

    #[test]
    fn test_nurbs_derivatives_edge_cases() {
        // Create a quadratic NURBS curve
        let points = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(0.5, 1.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
        ];
        let weights = vec![1.0, 1.0, 1.0];
        let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let curve = NurbsCurve::new(points, weights, knots, 2)
            .expect("valid NURBS curve should be created");

        // Test derivatives at endpoints
        let d0 = curve.evaluate_derivatives(0.0, 2);
        assert!(d0.derivative1.is_some());
        assert!(d0.derivative2.is_some());

        let d1 = curve.evaluate_derivatives(1.0, 2);
        assert!(d1.derivative1.is_some());
        assert!(d1.derivative2.is_some());

        // Request more derivatives than degree allows: degree-2 curve has at
        // most 2nd derivative; the request must succeed without panicking.
        let d_many = curve.evaluate_derivatives(0.5, 10);
        assert!(d_many.derivative1.is_some());
        assert!(d_many.derivative2.is_some());
    }

    #[test]
    fn test_nurbs_with_zero_weights() {
        // NURBS with zero weight (degenerate)
        let points = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
            Point3::new(2.0, 0.0, 0.0),
        ];
        let weights = vec![1.0, 0.0, 1.0]; // Middle weight is zero
        let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];

        // This should either fail or handle gracefully
        let result = NurbsCurve::new(points, weights, knots, 2);
        if let Ok(curve) = result {
            // If it succeeds, evaluation should handle the zero weight
            let p = curve.evaluate(0.5);
            assert!(p.point.x.is_finite());
        }
    }

    #[test]
    fn test_nurbs_circular_arc_accuracy() {
        // Test that circular arc is accurate
        let center = Point3::ZERO;
        let radius = 1.0;
        let start_angle = 0.0;
        let sweep_angle = std::f64::consts::PI / 2.0;
        let normal = Vector3::Z;

        let arc = NurbsCurve::circular_arc(center, radius, start_angle, sweep_angle, normal)
            .expect("circular arc should be created");

        // Check points on the arc
        for i in 0..=10 {
            let t = i as f64 / 10.0;
            let p = arc.evaluate(t);
            let dist = p.point.distance(&center);
            assert!((dist - radius).abs() < 1e-10, "Point should be on circle");
        }
    }

    #[test]
    fn test_nurbs_knot_insertion_stability() {
        // Create a simple curve
        let points = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
            Point3::new(2.0, 0.0, 0.0),
        ];
        let weights = vec![1.0, 2.0, 1.0];
        let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let mut curve = NurbsCurve::new(points, weights, knots, 2)
            .expect("valid NURBS curve should be created");

        // Evaluate before insertion
        let p_before = curve.evaluate(0.5);

        // Insert knot
        curve
            .insert_knot(0.5, 1)
            .expect("knot insertion should succeed");

        // Evaluate after insertion
        let p_after = curve.evaluate(0.5);

        // Should be the same point
        assert!(p_before.point.approx_eq(&p_after.point, NORMAL_TOLERANCE));
    }

    #[test]
    fn test_nurbs_surface_edge_cases() {
        // Create a simple bilinear surface
        let control_points = vec![
            vec![Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0)],
            vec![Point3::new(0.0, 1.0, 0.0), Point3::new(1.0, 1.0, 0.0)],
        ];
        let weights = vec![vec![1.0, 1.0], vec![1.0, 1.0]];
        let knots_u = vec![0.0, 0.0, 1.0, 1.0];
        let knots_v = vec![0.0, 0.0, 1.0, 1.0];

        let surface = NurbsSurface::new(control_points, weights, knots_u, knots_v, 1, 1)
            .expect("valid NURBS surface should be created");

        // Evaluate at corners
        let p00 = surface.evaluate(0.0, 0.0);
        assert_eq!(p00.point, Point3::new(0.0, 0.0, 0.0));

        let p11 = surface.evaluate(1.0, 1.0);
        assert_eq!(p11.point, Point3::new(1.0, 1.0, 0.0));

        // Evaluate normal at degenerate point (if any)
        let normal = surface
            .normal_at(0.5, 0.5)
            .expect("normal should be computable");
        assert!(normal.is_normalized(NORMAL_TOLERANCE));
    }

    #[test]
    fn test_nurbs_iso_curves() {
        // Create a surface
        let control_points = vec![
            vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(1.0, 0.0, 0.0),
                Point3::new(2.0, 0.0, 0.0),
            ],
            vec![
                Point3::new(0.0, 1.0, 0.0),
                Point3::new(1.0, 1.0, 1.0),
                Point3::new(2.0, 1.0, 0.0),
            ],
            vec![
                Point3::new(0.0, 2.0, 0.0),
                Point3::new(1.0, 2.0, 0.0),
                Point3::new(2.0, 2.0, 0.0),
            ],
        ];
        let weights = vec![vec![1.0; 3]; 3];
        let knots_u = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let knots_v = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];

        let surface = NurbsSurface::new(control_points, weights, knots_u, knots_v, 2, 2)
            .expect("valid NURBS surface should be created");

        // Extract iso curves
        let iso_u = surface.iso_curve_u(0.5).expect("iso curve should exist");
        let iso_v = surface.iso_curve_v(0.5).expect("iso curve should exist");

        // Check that iso curves match surface evaluation
        let p_surface = surface.evaluate(0.5, 0.5);
        let p_iso_u = iso_u.evaluate(0.5);
        let p_iso_v = iso_v.evaluate(0.5);

        assert!(p_surface.point.approx_eq(&p_iso_u.point, NORMAL_TOLERANCE));
        assert!(p_surface.point.approx_eq(&p_iso_v.point, NORMAL_TOLERANCE));
    }
}
