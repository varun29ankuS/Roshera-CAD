//! Internal regression-budget benchmarks for the geometry engine.
//!
//! This module measures the absolute performance of a fixed set of core
//! geometry operations and asserts each one against an **internal
//! regression budget**. We do not publish or assert comparisons against any
//! third-party kernel; performance numbers here exist to catch regressions,
//! not to make competitive claims.
//!
//! ## Internal regression targets
//!
//! | Operation                    | Internal budget |
//! |------------------------------|-----------------|
//! | Box primitive creation       | < 100 µs        |
//! | Sphere primitive creation    | < 1.0 ms        |
//! | Cylinder primitive creation  | < 1.0 ms        |
//! | NURBS Curve Eval (1M pts)    | < 25 ms         |
//! | B-Spline Curve Eval (1M pts) | < 10 ms         |
//! | Memory per 1M vertices       | < 192 MB        |
//!
//! See `roshera-backend/CLAUDE.md` ("Performance targets") for the single
//! source of truth these numbers are synchronized against.

use crate::math::Point3;
use crate::primitives::{
    box_primitive::{BoxParameters, BoxPrimitive},
    cylinder_primitive::{CylinderParameters, CylinderPrimitive},
    primitive_traits::Primitive,
    sphere_primitive::{SphereParameters, SpherePrimitive},
    topology_builder::BRepModel,
};
use std::time::{Duration, Instant};

/// One row of measured timings, in milliseconds (or microseconds where noted).
#[derive(Debug, Clone, Default)]
pub struct BenchmarkMetrics {
    /// Average box-primitive creation time in microseconds.
    pub box_creation_us: f64,
    /// Average sphere-primitive creation time in milliseconds.
    pub sphere_creation_ms: f64,
    /// Average cylinder-primitive creation time in milliseconds.
    pub cylinder_creation_ms: f64,
    /// NURBS evaluation time per 1M points, in milliseconds.
    pub nurbs_eval_1m_ms: f64,
    /// B-Spline evaluation time per 1M points, in milliseconds.
    pub bspline_eval_1m_ms: f64,
    /// Theoretical SoA memory per 1M vertices, in megabytes.
    pub memory_per_1m_vertices_mb: f64,
}

/// Internal regression budget. Any measurement that exceeds the matching
/// budget should be treated as a regression and investigated.
#[derive(Debug, Clone)]
pub struct RegressionBudget {
    pub box_creation_us: f64,
    pub sphere_creation_ms: f64,
    pub cylinder_creation_ms: f64,
    pub nurbs_eval_1m_ms: f64,
    pub bspline_eval_1m_ms: f64,
    pub memory_per_1m_vertices_mb: f64,
}

impl RegressionBudget {
    /// Budgets matching the `Performance targets` table in
    /// `roshera-backend/CLAUDE.md`.
    pub const fn default_budget() -> Self {
        Self {
            box_creation_us: 100.0,
            sphere_creation_ms: 1.0,
            cylinder_creation_ms: 1.0,
            nurbs_eval_1m_ms: 25.0,
            bspline_eval_1m_ms: 10.0,
            memory_per_1m_vertices_mb: 192.0,
        }
    }
}

/// List of regressions found by `RegressionReport::evaluate`.
#[derive(Debug, Clone, Default)]
pub struct RegressionReport {
    /// One entry per metric that exceeded its budget.
    pub regressions: Vec<String>,
}

impl RegressionReport {
    /// Compare measured metrics against the budget and collect any over-runs.
    pub fn evaluate(metrics: &BenchmarkMetrics, budget: &RegressionBudget) -> Self {
        let mut report = Self::default();

        if metrics.box_creation_us > budget.box_creation_us {
            report.regressions.push(format!(
                "box_creation: {:.1}µs exceeds budget {:.1}µs",
                metrics.box_creation_us, budget.box_creation_us
            ));
        }
        if metrics.sphere_creation_ms > budget.sphere_creation_ms {
            report.regressions.push(format!(
                "sphere_creation: {:.3}ms exceeds budget {:.3}ms",
                metrics.sphere_creation_ms, budget.sphere_creation_ms
            ));
        }
        if metrics.cylinder_creation_ms > budget.cylinder_creation_ms {
            report.regressions.push(format!(
                "cylinder_creation: {:.3}ms exceeds budget {:.3}ms",
                metrics.cylinder_creation_ms, budget.cylinder_creation_ms
            ));
        }
        if metrics.nurbs_eval_1m_ms > budget.nurbs_eval_1m_ms {
            report.regressions.push(format!(
                "nurbs_eval_1m: {:.1}ms exceeds budget {:.1}ms",
                metrics.nurbs_eval_1m_ms, budget.nurbs_eval_1m_ms
            ));
        }
        if metrics.bspline_eval_1m_ms > budget.bspline_eval_1m_ms {
            report.regressions.push(format!(
                "bspline_eval_1m: {:.1}ms exceeds budget {:.1}ms",
                metrics.bspline_eval_1m_ms, budget.bspline_eval_1m_ms
            ));
        }
        if metrics.memory_per_1m_vertices_mb > budget.memory_per_1m_vertices_mb {
            report.regressions.push(format!(
                "memory_per_1m_vertices: {:.1}MB exceeds budget {:.1}MB",
                metrics.memory_per_1m_vertices_mb, budget.memory_per_1m_vertices_mb
            ));
        }

        report
    }

    /// `true` when no metric exceeded its budget.
    pub fn within_budget(&self) -> bool {
        self.regressions.is_empty()
    }
}

/// Internal regression benchmark suite.
pub struct PerformanceBenchmarks;

impl PerformanceBenchmarks {
    /// Run every measurement and return the populated metrics.
    pub fn run_full_suite() -> BenchmarkMetrics {
        BenchmarkMetrics {
            box_creation_us: Self::benchmark_box_creation_us(),
            sphere_creation_ms: Self::benchmark_sphere_creation_ms(),
            cylinder_creation_ms: Self::benchmark_cylinder_creation_ms(),
            nurbs_eval_1m_ms: Self::benchmark_nurbs_evaluation_ms(),
            bspline_eval_1m_ms: Self::benchmark_bspline_evaluation_ms(),
            memory_per_1m_vertices_mb: Self::theoretical_memory_per_1m_vertices_mb(),
        }
    }

    /// Subset suitable for a fast regression gate (no curve-evaluation pass).
    pub fn quick_check() -> BenchmarkMetrics {
        BenchmarkMetrics {
            box_creation_us: Self::benchmark_box_creation_us(),
            sphere_creation_ms: 0.0,
            cylinder_creation_ms: 0.0,
            nurbs_eval_1m_ms: 0.0,
            bspline_eval_1m_ms: 0.0,
            memory_per_1m_vertices_mb: Self::theoretical_memory_per_1m_vertices_mb(),
        }
    }

    fn benchmark_box_creation_us() -> f64 {
        const ITERATIONS: usize = 1000;
        let mut total_time = Duration::ZERO;

        for _ in 0..ITERATIONS {
            let mut model = BRepModel::new();
            // Box parameters are validated input — a hard-coded 10×10×10 box is
            // guaranteed to construct, so the unwrap is invariant-guarded.
            let params = BoxParameters::new(10.0, 10.0, 10.0)
                .expect("BoxParameters::new(10,10,10) is always valid");

            let start = Instant::now();
            let _ = BoxPrimitive::create(params, &mut model)
                .expect("BoxPrimitive::create with valid params should not fail");
            total_time += start.elapsed();
        }

        total_time.as_micros() as f64 / ITERATIONS as f64
    }

    fn benchmark_sphere_creation_ms() -> f64 {
        const ITERATIONS: usize = 100;
        let mut total_time = Duration::ZERO;

        for _ in 0..ITERATIONS {
            let mut model = BRepModel::new();
            let params = SphereParameters::new(5.0, Point3::ORIGIN)
                .expect("SphereParameters::new with positive radius is valid")
                .with_segments(8, 6)
                .expect("with_segments(8,6) is within the documented range");

            let start = Instant::now();
            let _ = SpherePrimitive::create(params, &mut model)
                .expect("SpherePrimitive::create with valid params should not fail");
            total_time += start.elapsed();
        }

        total_time.as_micros() as f64 / (ITERATIONS as f64 * 1000.0)
    }

    fn benchmark_cylinder_creation_ms() -> f64 {
        const ITERATIONS: usize = 1000;
        let mut total_time = Duration::ZERO;

        for _ in 0..ITERATIONS {
            let mut model = BRepModel::new();
            let params = CylinderParameters::new(5.0, 10.0)
                .expect("CylinderParameters::new with positive radius/height is valid")
                .with_segments(8)
                .expect("with_segments(8) is within the documented range");

            let start = Instant::now();
            let _ = CylinderPrimitive::create(params, &mut model)
                .expect("CylinderPrimitive::create with valid params should not fail");
            total_time += start.elapsed();
        }

        let avg_us = total_time.as_micros() as f64 / ITERATIONS as f64;
        avg_us / 1000.0
    }

    /// Theoretical memory per 1M vertices in the SoA layout: 3×f64 + u32 flags
    /// + 8 bytes per-vertex overhead. This is a structural property, not a
    /// run-time measurement.
    fn theoretical_memory_per_1m_vertices_mb() -> f64 {
        let vertex_size = std::mem::size_of::<f64>() * 3;
        let vertex_flags = std::mem::size_of::<u32>();
        let total_per_vertex = vertex_size + vertex_flags + 8;
        (total_per_vertex * 1_000_000) as f64 / (1024.0 * 1024.0)
    }

    fn benchmark_nurbs_evaluation_ms() -> f64 {
        use crate::math::nurbs::NurbsCurve;

        const EVALUATIONS: usize = 1_000_000;
        const ITERATIONS: usize = 10;
        let mut total_time = Duration::ZERO;

        let control_points = vec![
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(-1.0, 1.0, 0.0),
            Point3::new(-1.0, 0.0, 0.0),
        ];
        let weights = vec![1.0, 0.707, 1.0, 0.707, 1.0];
        let knots = vec![0.0, 0.0, 0.0, 0.5, 0.5, 1.0, 1.0, 1.0];

        let nurbs = NurbsCurve::new(control_points, weights, knots, 2)
            .expect("hand-built clamped quadratic NURBS is valid");

        for _ in 0..ITERATIONS {
            let start = Instant::now();
            for i in 0..EVALUATIONS {
                let t = i as f64 / (EVALUATIONS - 1) as f64;
                let _ = std::hint::black_box(nurbs.evaluate(t).point);
            }
            total_time += start.elapsed();
        }

        total_time.as_millis() as f64 / ITERATIONS as f64
    }

    fn benchmark_bspline_evaluation_ms() -> f64 {
        use crate::math::bspline::BSplineCurve;

        const EVALUATIONS: usize = 1_000_000;
        const ITERATIONS: usize = 10;
        let mut total_time = Duration::ZERO;

        let control_points = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 2.0, 1.0),
            Point3::new(3.0, 1.0, 2.0),
            Point3::new(4.0, 3.0, 0.0),
            Point3::new(5.0, 0.0, 1.0),
        ];
        let knots = vec![0.0, 0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0, 1.0];

        let bspline = BSplineCurve::new(3, control_points, knots)
            .expect("hand-built clamped cubic B-spline is valid");

        for _ in 0..ITERATIONS {
            let start = Instant::now();
            for i in 0..EVALUATIONS {
                let t = i as f64 / (EVALUATIONS - 1) as f64;
                let _ = std::hint::black_box(
                    bspline
                        .evaluate(t)
                        .expect("evaluate within the curve domain should succeed"),
                );
            }
            total_time += start.elapsed();
        }

        total_time.as_millis() as f64 / ITERATIONS as f64
    }

    /// Render a regression report as a Markdown summary.
    pub fn generate_report(metrics: &BenchmarkMetrics, report: &RegressionReport) -> String {
        let mut out = String::new();
        out.push_str("# Geometry-engine regression report\n\n");
        out.push_str(&format!(
            "| Metric                         | Measured        |\n|--------------------------------|-----------------|\n| Box creation                   | {:.1} µs        |\n| Sphere creation                | {:.3} ms        |\n| Cylinder creation              | {:.3} ms        |\n| NURBS eval (1M pts)            | {:.1} ms        |\n| B-Spline eval (1M pts)         | {:.1} ms        |\n| Memory / 1M vertices (theor.)  | {:.1} MB        |\n\n",
            metrics.box_creation_us,
            metrics.sphere_creation_ms,
            metrics.cylinder_creation_ms,
            metrics.nurbs_eval_1m_ms,
            metrics.bspline_eval_1m_ms,
            metrics.memory_per_1m_vertices_mb,
        ));

        if report.within_budget() {
            out.push_str("**Status:** within budget for every metric.\n");
        } else {
            out.push_str("**Status:** regressions detected:\n\n");
            for r in &report.regressions {
                out.push_str(&format!("- {r}\n"));
            }
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quick_regression_gate() {
        let metrics = PerformanceBenchmarks::quick_check();
        assert!(metrics.box_creation_us > 0.0);
        assert!(metrics.memory_per_1m_vertices_mb > 0.0);

        // Theoretical SoA memory must always satisfy the structural budget.
        let budget = RegressionBudget::default_budget();
        assert!(
            metrics.memory_per_1m_vertices_mb <= budget.memory_per_1m_vertices_mb,
            "theoretical SoA memory ({:.1}MB) exceeds the structural budget ({:.1}MB) — \
             check Vector3 / VertexFlags layout",
            metrics.memory_per_1m_vertices_mb,
            budget.memory_per_1m_vertices_mb
        );
    }

    #[test]
    fn test_report_generation() {
        let metrics = PerformanceBenchmarks::quick_check();
        let report = RegressionReport::evaluate(&metrics, &RegressionBudget::default_budget());
        let text = PerformanceBenchmarks::generate_report(&metrics, &report);
        assert!(text.contains("# Geometry-engine regression report"));
        assert!(text.contains("Memory / 1M vertices"));
    }

    #[test]
    fn test_default_budget_is_self_consistent() {
        let b = RegressionBudget::default_budget();
        assert!(b.box_creation_us > 0.0);
        assert!(b.sphere_creation_ms > 0.0);
        assert!(b.cylinder_creation_ms > 0.0);
        assert!(b.nurbs_eval_1m_ms > 0.0);
        assert!(b.bspline_eval_1m_ms > 0.0);
        assert!(b.memory_per_1m_vertices_mb > 0.0);
    }
}
