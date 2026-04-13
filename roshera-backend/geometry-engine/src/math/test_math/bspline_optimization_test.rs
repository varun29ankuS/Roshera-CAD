#[cfg(test)]
mod tests {
    use crate::math::{
        bspline::{evaluate_batch_simd, BSplineCurve, BSplineWorkspace},
        Point3,
    };
    use std::time::Instant;

    fn create_test_curve() -> BSplineCurve {
        // Create a cubic B-spline with 6 control points
        let control_points = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 2.0, 0.5),
            Point3::new(2.0, 1.5, 1.0),
            Point3::new(3.0, 3.0, 0.8),
            Point3::new(4.0, 2.5, 0.3),
            Point3::new(5.0, 0.0, 0.0),
        ];

        // For 6 control points and degree 3, we need 6 + 3 + 1 = 10 knots
        let knots = vec![0.0, 0.0, 0.0, 0.0, 0.33, 0.67, 1.0, 1.0, 1.0, 1.0];
        BSplineCurve::new(3, control_points, knots).expect("Failed to create B-spline")
    }

    #[test]
    fn test_optimized_bspline_correctness() {
        let curve = create_test_curve();

        // Compare results at various parameter values
        // The optimized version is now integrated into BSplineCurve
        for i in 0..=20 {
            let u = i as f64 / 20.0;
            let p = curve.evaluate(u).unwrap();

            // Debug print to see what values we're getting
            if !p.x.is_finite() || !p.y.is_finite() || !p.z.is_finite() {
                panic!("NaN or infinity at u={}: p=({}, {}, {})", u, p.x, p.y, p.z);
            }

            // For cubic B-splines, the curve stays within the convex hull of control points
            // Control points range from x=0 to x=5
            // The assertion should be strict - aerospace standards
            assert!(
                p.x >= 0.0 && p.x <= 5.0,
                "X coordinate {} out of range at u={}. This indicates a bug in the B-spline implementation.",
                p.x, u
            );
        }
    }

    #[test]
    fn test_workspace_pooling() {
        let curve = create_test_curve();

        // Test workspace creation
        let mut workspace = BSplineWorkspace::new(curve.degree);

        for _ in 0..10 {
            workspace.reset(curve.degree);
            let result = curve.evaluate(0.5);
            assert!(result.is_ok());
        }
    }

    #[test]
    fn test_batch_evaluation() {
        let curve = create_test_curve();

        let params: Vec<f64> = (0..10).map(|i| i as f64 / 9.0).collect();
        let mut output = vec![Point3::new(0.0, 0.0, 0.0); 10];

        let result = evaluate_batch_simd(&curve, &params, &mut output);
        assert!(result.is_ok());

        // Verify all points were evaluated correctly
        // The first point should be (0,0,0) as that's the first control point
        assert_eq!(output[0], Point3::new(0.0, 0.0, 0.0));

        // Verify last point is the last control point
        assert_eq!(output[9], Point3::new(5.0, 0.0, 0.0));

        // Middle points should be between control points
        for i in 1..9 {
            assert!(output[i].x >= 0.0 && output[i].x <= 5.0);
        }
    }

    #[test]
    fn test_span_lookup_table() {
        let curve = create_test_curve();

        // Test span finding
        let span = curve.find_span(0.5);

        // Verify span is in valid range
        assert!(span >= curve.degree);
        assert!(span < curve.control_points.len());
    }

    #[test]
    fn test_performance_improvement() {
        let curve = create_test_curve();
        const ITERATIONS: usize = 100_000;

        // Warmup
        for _ in 0..1000 {
            let _ = curve.evaluate(0.5);
        }

        // Measure optimized implementation
        let start = Instant::now();
        for _ in 0..ITERATIONS {
            let _ = curve.evaluate(0.5);
        }
        let elapsed = start.elapsed();

        let ns_per_op = elapsed.as_nanos() as f64 / ITERATIONS as f64;
        println!("B-spline evaluation: {:.1} ns/op", ns_per_op);

        // Should be under 100ns
        assert!(
            ns_per_op < 100.0,
            "Performance target not met: {:.1} ns/op > 100ns",
            ns_per_op
        );
    }
}
