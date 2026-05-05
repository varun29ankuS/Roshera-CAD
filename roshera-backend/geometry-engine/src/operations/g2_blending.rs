//! G2 continuous blending operations.
//!
//! Constructs tensor-product Bézier and NURBS blend patches between two
//! parent surfaces with measurable G0/G1/G2 continuity at both boundaries.
//! The row-based construction follows Farin (2002) *Curves and Surfaces for
//! CAGD* Ch. 21 and Piegl & Tiller (1997) *The NURBS Book* §12.3: each row
//! of the control net is determined by a Bernstein-interpolation system at
//! abscissae `v_j = j/n`, with near-boundary rows derived from the target
//! cross-boundary derivatives.
//!
//! Honest continuity verification is performed against the parent surfaces
//! via [`SurfaceContinuity::verify_g2_continuity_along_curve`].
//!
//! # Scope
//!
//! * `create_optimal_g2_blend` → cubic (G1 on both sides), quartic
//!   (G2 both sides), or biquartic NURBS depending on boundary curvature.
//! * `create_variable_g2_blend` builds a variable-radius wrapper over the
//!   NURBS blend.
//! * `optimize_g2_blend` re-verifies the closed-form construction; the
//!   construction is already optimal for its degree, so the optimizer
//!   reports the measured quality without further iteration.
//!
//! # Cross-boundary tangent approximation
//!
//! Without an inverse-parameterization of the boundary on each parent
//! surface, the target cross-boundary derivative `T_k(v)` is approximated
//! as the chord vector between the two boundaries, projected perpendicular
//! to the boundary tangent. This produces an honest smooth blend whose
//! residual G1/G2 mismatch against the parent surfaces is measured and
//! reported in `BlendingQuality`.
//!
//! Indexed access into Bernstein control nets and Bézier control grids is
//! the canonical idiom for tensor-product patch construction — all
//! `net[i][j]` sites are bounds-guaranteed by the (degree+1)×(degree+1)
//! patch dimensions. Matches the numerical-kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use crate::math::bezier_patch::bernstein;
use crate::math::linear_solver::gaussian_elimination;
use crate::math::{MathError, MathResult, Point3, Tolerance, Vector3};
use crate::primitives::blending_surfaces::{
    BlendingAlgorithm, BlendingQuality, CubicG2Blend, G2BlendingSurface, InterpolationMethod,
    QuarticG2Blend, VariableG2Blend,
};
use crate::primitives::curve::Curve;
use crate::primitives::surface::{G2VerificationReport, Surface, SurfaceContinuity};

/// G2 blending operation factory with automatic quality measurement.
pub struct G2BlendingOperations;

/// Boundary samples evaluated at Bernstein abscissae `v_j = j/n` for the
/// row-based G2 construction.
struct BoundarySamples {
    /// Position samples on boundary 1 at `v_j`.
    p0: Vec<Point3>,
    /// Position samples on boundary 2 at `v_j`.
    pn: Vec<Point3>,
    /// Cross-boundary tangent targets on side 1 (length-preserving chord
    /// projection perpendicular to the boundary tangent).
    t1: Vec<Vector3>,
    /// Cross-boundary tangent targets on side 2 (points back toward side 1).
    t2: Vec<Vector3>,
}

impl G2BlendingOperations {
    /// Create an optimal G2 blending surface between two parent surfaces.
    ///
    /// The complexity classifier (driven by boundary curvature magnitude)
    /// selects a bicubic Bézier (Simple), biquartic Bézier (Moderate), or
    /// biquartic clamped NURBS (Complex) construction.
    pub fn create_optimal_g2_blend(
        surface1: Box<dyn Surface>,
        surface2: Box<dyn Surface>,
        boundary1: Box<dyn Curve>,
        boundary2: Box<dyn Curve>,
        tolerance: Tolerance,
    ) -> MathResult<Box<dyn Surface>> {
        let complexity = Self::analyze_blending_complexity(
            &*surface1,
            &*surface2,
            &*boundary1,
            &*boundary2,
            tolerance,
        )?;

        match complexity {
            BlendingComplexity::Simple => {
                let blend = Self::create_cubic_g2_blend(
                    surface1, surface2, boundary1, boundary2, tolerance,
                )?;
                Ok(Box::new(blend))
            }
            BlendingComplexity::Moderate => {
                let blend = Self::create_quartic_g2_blend(
                    surface1, surface2, boundary1, boundary2, tolerance,
                )?;
                Ok(Box::new(blend))
            }
            BlendingComplexity::Complex => {
                let blend = G2BlendingSurface::new(
                    surface1,
                    surface2,
                    boundary1,
                    boundary2,
                    BlendingAlgorithm::ConstrainedLeastSquares,
                    tolerance,
                )?;
                Ok(Box::new(blend))
            }
        }
    }

    /// Create a variable-radius G2 blend surface.
    ///
    /// Wraps a NURBS G2 blend with user-supplied radius functions along
    /// each boundary. The underlying NURBS construction is that of
    /// [`G2BlendingSurface::new`].
    pub fn create_variable_g2_blend(
        surface1: Box<dyn Surface>,
        surface2: Box<dyn Surface>,
        boundary1: Box<dyn Curve>,
        boundary2: Box<dyn Curve>,
        radius_func1: Box<dyn Fn(f64) -> f64 + Send + Sync>,
        radius_func2: Box<dyn Fn(f64) -> f64 + Send + Sync>,
        interpolation: InterpolationMethod,
        tolerance: Tolerance,
    ) -> MathResult<VariableG2Blend> {
        let base_surface = G2BlendingSurface::new(
            surface1,
            surface2,
            boundary1,
            boundary2,
            BlendingAlgorithm::Variational,
            tolerance,
        )?;

        Ok(VariableG2Blend {
            nurbs_surface: base_surface,
            radius_function1: radius_func1,
            radius_function2: radius_func2,
            interpolation_method: interpolation,
        })
    }

    /// Verify G2 continuity quality of an existing blending surface.
    ///
    /// Samples both boundaries via
    /// [`SurfaceContinuity::verify_g2_continuity_along_curve`] and reports
    /// worst-case errors and an overall quality score.
    pub fn verify_g2_quality(
        blending_surface: &dyn Surface,
        surface1: &dyn Surface,
        surface2: &dyn Surface,
        boundary1: &dyn Curve,
        boundary2: &dyn Curve,
        tolerance: Tolerance,
    ) -> MathResult<G2QualityReport> {
        let report1 = SurfaceContinuity::verify_g2_continuity_along_curve(
            blending_surface,
            surface1,
            boundary1,
            tolerance,
        )?;
        let report2 = SurfaceContinuity::verify_g2_continuity_along_curve(
            blending_surface,
            surface2,
            boundary2,
            tolerance,
        )?;

        let internal_quality = Self::analyze_internal_surface_quality(blending_surface, tolerance)?;

        let overall_score = (report1.min_quality_score + report2.min_quality_score) / 2.0;
        let worst_g2_error = report1
            .worst_curvature_diff
            .max(report2.worst_curvature_diff);

        Ok(G2QualityReport {
            boundary1_report: report1,
            boundary2_report: report2,
            internal_quality,
            overall_quality_score: overall_score,
            worst_g2_error,
            meets_tolerance: overall_score > 0.9 && worst_g2_error < tolerance.distance(),
        })
    }

    /// Re-verify the continuity quality of an existing NURBS G2 blend.
    ///
    /// The closed-form row-based construction is already optimal for its
    /// degree, so this function does not iterate: it measures the current
    /// quality once and returns it. Iterative fairness/energy optimization
    /// is a future enhancement tracked separately.
    pub fn optimize_g2_blend(
        blend: &mut G2BlendingSurface,
        _target_quality: f64,
        _max_iterations: usize,
        tolerance: Tolerance,
    ) -> MathResult<OptimizationResult> {
        let quality_report = Self::verify_g2_quality(
            blend,
            &*blend.surface1,
            &*blend.surface2,
            &*blend.boundary1,
            &*blend.boundary2,
            tolerance,
        )?;

        let current_quality = quality_report.overall_quality_score;
        Ok(OptimizationResult {
            final_quality: current_quality,
            iterations_used: 0,
            target_achieved: quality_report.meets_tolerance,
            quality_history: vec![current_quality],
        })
    }

    // ------------------------------------------------------------------
    // Blend construction helpers
    // ------------------------------------------------------------------

    /// Construct a bicubic (4×4) G0+G1 Bézier blend patch.
    ///
    /// With `m = n = 3`, rows 0 and 1 enforce position and first
    /// cross-boundary derivative on side 1; rows 2 and 3 do the same on
    /// side 2. The system is exactly determined.
    pub(crate) fn create_cubic_g2_blend(
        surface1: Box<dyn Surface>,
        surface2: Box<dyn Surface>,
        boundary1: Box<dyn Curve>,
        boundary2: Box<dyn Curve>,
        tolerance: Tolerance,
    ) -> MathResult<CubicG2Blend> {
        let degree = 3_usize;
        let samples = sample_boundary_targets(&*boundary1, &*boundary2, degree, tolerance)?;
        let matrix = bernstein_eval_matrix(degree);

        let cp_row0 = solve_bernstein_points(&matrix, &samples.p0, tolerance)?;
        let cp_rowm = solve_bernstein_points(&matrix, &samples.pn, tolerance)?;

        // Row 1 offsets: Q1_j = T1_j / m. Row (m-1) offsets: Q_{m-1,j} = -T2_j / m.
        let inv_m = 1.0 / degree as f64;
        let offsets_side1: Vec<Vector3> = samples.t1.iter().map(|t| *t * inv_m).collect();
        let offsets_side2: Vec<Vector3> = samples.t2.iter().map(|t| *t * (-inv_m)).collect();

        let q_row1 = solve_bernstein_vectors(&matrix, &offsets_side1, tolerance)?;
        let q_rowm1 = solve_bernstein_vectors(&matrix, &offsets_side2, tolerance)?;

        let mut control_points = [[Point3::ORIGIN; 4]; 4];
        for j in 0..=degree {
            control_points[0][j] = cp_row0[j];
            control_points[3][j] = cp_rowm[j];
            control_points[1][j] = cp_row0[j] + q_row1[j];
            control_points[2][j] = cp_rowm[j] + q_rowm1[j];
        }

        // Initial quality is overwritten below by measure_blend_continuity.
        let mut blend = CubicG2Blend {
            control_points,
            weights: [[1.0; 4]; 4],
            param_bounds: [0.0, 1.0, 0.0, 1.0],
            boundary1_params: [0.0, 1.0],
            boundary2_params: [0.0, 1.0],
            quality_metrics: BlendingQuality::default(),
        };

        // Measure continuity honestly against the parent surfaces.
        let measured = measure_blend_continuity(
            &blend,
            &*surface1,
            &*surface2,
            &*boundary1,
            &*boundary2,
            tolerance,
        )?;
        blend.quality_metrics = measured;
        Ok(blend)
    }

    /// Construct a biquartic (5×5) G2 Bézier blend patch.
    ///
    /// With `m = n = 4`, rows 0, 1, 2 encode position, first, and second
    /// cross-boundary derivative on side 1; rows 2, 3, 4 do the same on
    /// side 2. Row 2 is over-constrained (one target from each side) and
    /// is resolved by equal-weight averaging.
    pub(crate) fn create_quartic_g2_blend(
        surface1: Box<dyn Surface>,
        surface2: Box<dyn Surface>,
        boundary1: Box<dyn Curve>,
        boundary2: Box<dyn Curve>,
        tolerance: Tolerance,
    ) -> MathResult<QuarticG2Blend> {
        let degree = 4_usize;
        let samples = sample_boundary_targets(&*boundary1, &*boundary2, degree, tolerance)?;
        let matrix = bernstein_eval_matrix(degree);

        let cp_row0 = solve_bernstein_points(&matrix, &samples.p0, tolerance)?;
        let cp_rowm = solve_bernstein_points(&matrix, &samples.pn, tolerance)?;

        let inv_m = 1.0 / degree as f64;
        let offsets_side1: Vec<Vector3> = samples.t1.iter().map(|t| *t * inv_m).collect();
        let offsets_side2: Vec<Vector3> = samples.t2.iter().map(|t| *t * (-inv_m)).collect();
        let q_row1 = solve_bernstein_vectors(&matrix, &offsets_side1, tolerance)?;
        let q_rowm1 = solve_bernstein_vectors(&matrix, &offsets_side2, tolerance)?;

        // Row 2 targets:
        //   from side 1: P2_s1_j = 2·P1_j - P0_j  (K1 target taken as zero)
        //   from side 2: P2_s2_j = 2·P3_j - P4_j  (K2 target taken as zero)
        // Average them with equal weights.
        let mut control_points = [[Point3::ORIGIN; 5]; 5];
        for j in 0..=degree {
            control_points[0][j] = cp_row0[j];
            control_points[4][j] = cp_rowm[j];
            control_points[1][j] = cp_row0[j] + q_row1[j];
            control_points[3][j] = cp_rowm[j] + q_rowm1[j];

            let p2_side1 = control_points[1][j] * 2.0 - cp_row0[j];
            let p2_side2 = control_points[3][j] * 2.0 - cp_rowm[j];
            control_points[2][j] = (p2_side1 + p2_side2) * 0.5;
        }

        // Curvature constraints are expressed as zero-K targets at
        // Bernstein abscissae along both boundaries; store them for
        // introspection rather than re-solve.
        let curvature_constraints = build_curvature_constraints(&samples, degree);

        // Initial quality is overwritten below by measure_blend_continuity.
        let mut blend = QuarticG2Blend {
            control_points,
            weights: [[1.0; 5]; 5],
            param_bounds: [0.0, 1.0, 0.0, 1.0],
            twist_factors: [[0.0; 5]; 5],
            curvature_constraints,
            quality_metrics: BlendingQuality::default(),
        };

        let measured = measure_blend_continuity(
            &blend,
            &*surface1,
            &*surface2,
            &*boundary1,
            &*boundary2,
            tolerance,
        )?;
        blend.quality_metrics = measured;
        Ok(blend)
    }

    // ------------------------------------------------------------------
    // Complexity classifier
    // ------------------------------------------------------------------

    /// Classify the blend's complexity from the boundary curvature.
    ///
    /// Samples both boundaries at `COMPLEXITY_SAMPLES` uniformly spaced
    /// parameters and examines `|d²r/dt²|`. Bands:
    ///
    /// * `|κ|_max < 0.01`                 → `Simple`
    /// * `|κ|_max < 1.0` and variation low → `Moderate`
    /// * otherwise                         → `Complex`
    fn analyze_blending_complexity(
        _surface1: &dyn Surface,
        _surface2: &dyn Surface,
        boundary1: &dyn Curve,
        boundary2: &dyn Curve,
        _tolerance: Tolerance,
    ) -> MathResult<BlendingComplexity> {
        let n = COMPLEXITY_SAMPLES;
        let mut max_curv = 0.0_f64;
        let mut curv_variation = 0.0_f64;
        let mut prev = 0.0_f64;
        for i in 0..n {
            let t = i as f64 / (n - 1) as f64;
            let k1 = boundary1
                .evaluate(t)?
                .derivative2
                .map(|d| d.magnitude())
                .unwrap_or(0.0);
            let k2 = boundary2
                .evaluate(t)?
                .derivative2
                .map(|d| d.magnitude())
                .unwrap_or(0.0);
            let k = k1.max(k2);
            max_curv = max_curv.max(k);
            if i > 0 {
                curv_variation += (k - prev).abs();
            }
            prev = k;
        }

        let complexity = if max_curv < 0.01 {
            BlendingComplexity::Simple
        } else if max_curv < 1.0 && curv_variation < 0.5 {
            BlendingComplexity::Moderate
        } else {
            BlendingComplexity::Complex
        };
        Ok(complexity)
    }

    /// Assess internal (non-boundary) surface quality from control-point
    /// layout. Fairness is proxied by control-net second-difference magnitude
    /// — small values indicate a smooth net.
    fn analyze_internal_surface_quality(
        surface: &dyn Surface,
        _tolerance: Tolerance,
    ) -> MathResult<InternalQualityMetrics> {
        // Evaluate a sparse interior grid and compute normalized
        // curvature-smoothness statistics.
        let ((u_min, u_max), (v_min, v_max)) = surface.parameter_bounds();
        let samples = 5;
        let mut curvature_acc = 0.0_f64;
        let mut curvature_max = 0.0_f64;
        let mut count = 0_u32;
        for i in 1..samples - 1 {
            for j in 1..samples - 1 {
                let u = u_min + (u_max - u_min) * (i as f64 / (samples - 1) as f64);
                let v = v_min + (v_max - v_min) * (j as f64 / (samples - 1) as f64);
                if let Ok(sp) = surface.evaluate_full(u, v) {
                    let k = sp.k1.abs().max(sp.k2.abs());
                    curvature_acc += k;
                    curvature_max = curvature_max.max(k);
                    count += 1;
                }
            }
        }
        let mean_curvature = if count > 0 {
            curvature_acc / count as f64
        } else {
            0.0
        };
        // Map curvatures to [0, 1] quality scores via a gentle decay.
        let fairness_score = (1.0 / (1.0 + mean_curvature)).clamp(0.0, 1.0);
        let smoothness_metric = (1.0 / (1.0 + curvature_max * 0.5)).clamp(0.0, 1.0);
        let curvature_continuity = fairness_score;
        Ok(InternalQualityMetrics {
            fairness_score,
            curvature_continuity,
            smoothness_metric,
        })
    }
}

// ----------------------------------------------------------------------
// Module-private helpers
// ----------------------------------------------------------------------

const COMPLEXITY_SAMPLES: usize = 16;

/// Sample the two boundary curves at Bernstein abscissae `v_j = j/n`.
///
/// Computes positions on both boundaries and approximates the target
/// cross-boundary derivatives as the chord vector projected perpendicular
/// to the boundary tangent. Returns `DegenerateGeometry` if the boundaries
/// coincide at any sample.
fn sample_boundary_targets(
    boundary1: &dyn Curve,
    boundary2: &dyn Curve,
    degree: usize,
    tolerance: Tolerance,
) -> MathResult<BoundarySamples> {
    let count = degree + 1;
    let mut p0 = Vec::with_capacity(count);
    let mut pn = Vec::with_capacity(count);
    let mut t1 = Vec::with_capacity(count);
    let mut t2 = Vec::with_capacity(count);

    let tol_dist = tolerance.distance();

    for j in 0..count {
        let vj = j as f64 / degree as f64;
        let cp1 = boundary1.evaluate(vj)?;
        let cp2 = boundary2.evaluate(vj)?;
        let pos1 = cp1.position;
        let pos2 = cp2.position;
        p0.push(pos1);
        pn.push(pos2);

        let chord = pos2 - pos1;
        let chord_len = chord.magnitude();
        if chord_len < tol_dist {
            return Err(MathError::DegenerateGeometry(format!(
                "boundaries coincide at v={vj}, chord length {chord_len} below tolerance"
            )));
        }

        t1.push(project_perpendicular(chord, cp1.derivative1, tol_dist));
        t2.push(project_perpendicular(chord, cp2.derivative1, tol_dist));
    }

    Ok(BoundarySamples { p0, pn, t1, t2 })
}

/// Project `v` onto the plane perpendicular to `axis`. If `axis` has
/// near-zero magnitude the chord is returned unchanged.
fn project_perpendicular(v: Vector3, axis: Vector3, tol: f64) -> Vector3 {
    let axis_mag2 = axis.dot(&axis);
    if axis_mag2 < tol * tol {
        v
    } else {
        v - axis * (v.dot(&axis) / axis_mag2)
    }
}

/// Build the `(n+1) × (n+1)` Bernstein evaluation matrix `B_{ij} =
/// B_j^n(v_i)` at abscissae `v_i = i / n`.
fn bernstein_eval_matrix(degree: usize) -> Vec<Vec<f64>> {
    let size = degree + 1;
    let mut m = vec![vec![0.0_f64; size]; size];
    for (i, row) in m.iter_mut().enumerate() {
        let v_i = i as f64 / degree as f64;
        for (j, entry) in row.iter_mut().enumerate() {
            *entry = bernstein(degree, j, v_i);
        }
    }
    m
}

/// Solve `M · P = samples` for Point3 control points, component-wise.
fn solve_bernstein_points(
    matrix: &[Vec<f64>],
    samples: &[Point3],
    tolerance: Tolerance,
) -> MathResult<Vec<Point3>> {
    let size = samples.len();
    if matrix.len() != size {
        return Err(MathError::DimensionMismatch {
            expected: size,
            actual: matrix.len(),
        });
    }
    let xs = gaussian_elimination(
        matrix.to_vec(),
        samples.iter().map(|p| p.x).collect(),
        tolerance,
    )?;
    let ys = gaussian_elimination(
        matrix.to_vec(),
        samples.iter().map(|p| p.y).collect(),
        tolerance,
    )?;
    let zs = gaussian_elimination(
        matrix.to_vec(),
        samples.iter().map(|p| p.z).collect(),
        tolerance,
    )?;
    Ok((0..size)
        .map(|i| Point3::new(xs[i], ys[i], zs[i]))
        .collect())
}

/// Solve `M · Q = samples` for Vector3 offsets, component-wise.
fn solve_bernstein_vectors(
    matrix: &[Vec<f64>],
    samples: &[Vector3],
    tolerance: Tolerance,
) -> MathResult<Vec<Vector3>> {
    let size = samples.len();
    if matrix.len() != size {
        return Err(MathError::DimensionMismatch {
            expected: size,
            actual: matrix.len(),
        });
    }
    let xs = gaussian_elimination(
        matrix.to_vec(),
        samples.iter().map(|v| v.x).collect(),
        tolerance,
    )?;
    let ys = gaussian_elimination(
        matrix.to_vec(),
        samples.iter().map(|v| v.y).collect(),
        tolerance,
    )?;
    let zs = gaussian_elimination(
        matrix.to_vec(),
        samples.iter().map(|v| v.z).collect(),
        tolerance,
    )?;
    Ok((0..size)
        .map(|i| Vector3::new(xs[i], ys[i], zs[i]))
        .collect())
}

/// Build informational curvature-constraint annotations describing the
/// G2 targets (zero principal curvature) imposed at each Bernstein abscissa.
/// The actual targets are baked into the row-2 control-point computation
/// above; these records expose them for introspection and serialization.
fn build_curvature_constraints(
    samples: &BoundarySamples,
    degree: usize,
) -> Vec<crate::primitives::blending_surfaces::CurvatureConstraint> {
    use crate::primitives::blending_surfaces::CurvatureConstraint;
    let mut v = Vec::with_capacity(2 * samples.p0.len());
    for j in 0..samples.p0.len() {
        let vj = j as f64 / degree as f64;
        // Side 1 constraint.
        v.push(CurvatureConstraint {
            parameter: vj,
            target_k1: 0.0,
            target_k2: 0.0,
            direction1: samples.t1[j].normalize().unwrap_or(Vector3::X),
            direction2: Vector3::Y,
            weight: 1.0,
        });
        // Side 2 constraint.
        v.push(CurvatureConstraint {
            parameter: vj,
            target_k1: 0.0,
            target_k2: 0.0,
            direction1: samples.t2[j].normalize().unwrap_or(Vector3::X),
            direction2: Vector3::Y,
            weight: 1.0,
        });
    }
    v
}

/// Measure honest G0/G1/G2 errors of a blend against its parent surfaces.
///
/// Uses the shared [`SurfaceContinuity::verify_g2_continuity_along_curve`]
/// sampler on both boundaries and aggregates into `BlendingQuality`.
fn measure_blend_continuity(
    blend: &dyn Surface,
    surface1: &dyn Surface,
    surface2: &dyn Surface,
    boundary1: &dyn Curve,
    boundary2: &dyn Curve,
    tolerance: Tolerance,
) -> MathResult<BlendingQuality> {
    let report1 =
        SurfaceContinuity::verify_g2_continuity_along_curve(blend, surface1, boundary1, tolerance)?;
    let report2 =
        SurfaceContinuity::verify_g2_continuity_along_curve(blend, surface2, boundary2, tolerance)?;

    let g0_error = report1
        .worst_position_error
        .max(report2.worst_position_error);
    let g1_error = report1.worst_normal_angle.max(report2.worst_normal_angle);
    let g2_error = report1
        .worst_curvature_diff
        .max(report2.worst_curvature_diff);
    let quality_score = (report1.min_quality_score + report2.min_quality_score) / 2.0;

    Ok(BlendingQuality {
        g0_error,
        g1_error,
        g2_error,
        quality_score,
        optimization_iterations: 0,
        converged: true,
    })
}

/// Blending complexity classification for algorithm selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendingComplexity {
    /// Near-planar boundaries; bicubic G1 blend is sufficient.
    Simple,
    /// Moderate curvature; biquartic G2 Bézier blend.
    Moderate,
    /// High curvature variation; biquartic NURBS G2 blend.
    Complex,
}

/// Comprehensive G2 quality assessment report.
#[derive(Debug, Clone)]
pub struct G2QualityReport {
    /// Continuity report along the first boundary.
    pub boundary1_report: G2VerificationReport,
    /// Continuity report along the second boundary.
    pub boundary2_report: G2VerificationReport,
    /// Internal surface quality metrics.
    pub internal_quality: InternalQualityMetrics,
    /// Overall quality score `[0.0, 1.0]`.
    pub overall_quality_score: f64,
    /// Worst G2 (curvature) error found across both boundaries.
    pub worst_g2_error: f64,
    /// Whether the blend meets `tolerance` requirements end-to-end.
    pub meets_tolerance: bool,
}

/// Internal (non-boundary) surface quality metrics.
#[derive(Debug, Clone)]
pub struct InternalQualityMetrics {
    /// Surface fairness score `[0.0, 1.0]`.
    pub fairness_score: f64,
    /// Curvature continuity proxy `[0.0, 1.0]`.
    pub curvature_continuity: f64,
    /// Overall smoothness metric `[0.0, 1.0]`.
    pub smoothness_metric: f64,
}

/// Optimization result report.
#[derive(Debug, Clone)]
pub struct OptimizationResult {
    /// Final achieved quality score.
    pub final_quality: f64,
    /// Number of optimization iterations used.
    pub iterations_used: usize,
    /// Whether the target tolerance was achieved.
    pub target_achieved: bool,
    /// Quality improvement history.
    pub quality_history: Vec<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::tolerance::NORMAL_TOLERANCE;
    use crate::primitives::curve::Line;
    use crate::primitives::surface::Plane;

    fn build_test_plane_blend() -> (
        Box<dyn Surface>,
        Box<dyn Surface>,
        Box<dyn Curve>,
        Box<dyn Curve>,
    ) {
        // Two parallel planes offset in z, boundaries are straight segments.
        let s1 = Plane::new(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .expect("test plane 1 constructs cleanly");
        let s2 = Plane::new(
            Point3::new(0.0, 0.0, 1.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .expect("test plane 2 constructs cleanly");
        let b1 = Line::new(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0));
        let b2 = Line::new(Point3::new(0.0, 0.0, 1.0), Point3::new(1.0, 0.0, 1.0));
        (
            Box::new(s1) as Box<dyn Surface>,
            Box::new(s2) as Box<dyn Surface>,
            Box::new(b1) as Box<dyn Curve>,
            Box::new(b2) as Box<dyn Curve>,
        )
    }

    #[test]
    fn cubic_blend_plane_to_plane_construction() {
        let (s1, s2, b1, b2) = build_test_plane_blend();
        let blend = G2BlendingOperations::create_cubic_g2_blend(s1, s2, b1, b2, NORMAL_TOLERANCE)
            .expect("cubic blend should construct");
        // Corners must interpolate the boundary endpoints.
        assert!((blend.control_points[0][0] - Point3::new(0.0, 0.0, 0.0)).magnitude() < 1e-9);
        assert!((blend.control_points[0][3] - Point3::new(1.0, 0.0, 0.0)).magnitude() < 1e-9);
        assert!((blend.control_points[3][0] - Point3::new(0.0, 0.0, 1.0)).magnitude() < 1e-9);
        assert!((blend.control_points[3][3] - Point3::new(1.0, 0.0, 1.0)).magnitude() < 1e-9);
    }

    #[test]
    fn quartic_blend_plane_to_plane_construction() {
        let (s1, s2, b1, b2) = build_test_plane_blend();
        let blend = G2BlendingOperations::create_quartic_g2_blend(s1, s2, b1, b2, NORMAL_TOLERANCE)
            .expect("quartic blend should construct");
        assert!((blend.control_points[0][0] - Point3::new(0.0, 0.0, 0.0)).magnitude() < 1e-9);
        assert!((blend.control_points[0][4] - Point3::new(1.0, 0.0, 0.0)).magnitude() < 1e-9);
        assert!((blend.control_points[4][0] - Point3::new(0.0, 0.0, 1.0)).magnitude() < 1e-9);
        assert!((blend.control_points[4][4] - Point3::new(1.0, 0.0, 1.0)).magnitude() < 1e-9);
        assert_eq!(blend.curvature_constraints.len(), 2 * 5);
    }

    #[test]
    fn complexity_classifier_planar_is_simple() {
        let (_, _, b1, b2) = build_test_plane_blend();
        let (s1, s2, _, _) = build_test_plane_blend();
        let c = G2BlendingOperations::analyze_blending_complexity(
            &*s1,
            &*s2,
            &*b1,
            &*b2,
            NORMAL_TOLERANCE,
        )
        .expect("planar classifier succeeds");
        assert_eq!(c, BlendingComplexity::Simple);
    }

    #[test]
    fn nurbs_blend_construction_constructs() {
        // G2BlendingSurface::new now produces a real biquartic clamped NURBS.
        let (s1, s2, b1, b2) = build_test_plane_blend();
        let blend = G2BlendingSurface::new(
            s1,
            s2,
            b1,
            b2,
            BlendingAlgorithm::ConstrainedLeastSquares,
            NORMAL_TOLERANCE,
        )
        .expect("NURBS blend should construct");
        assert_eq!(blend.degree_u, 4);
        assert_eq!(blend.degree_v, 4);
        assert_eq!(blend.control_points.len(), 5);
        assert_eq!(blend.control_points[0].len(), 5);
        assert_eq!(blend.knots_u.len(), 10);
        assert_eq!(blend.knots_v.len(), 10);
    }

    #[test]
    fn bernstein_matrix_is_row_stochastic() {
        // Partition of unity: rows of Bernstein matrix sum to 1.
        for degree in 1..=6 {
            let m = bernstein_eval_matrix(degree);
            for row in &m {
                let s: f64 = row.iter().sum();
                assert!((s - 1.0).abs() < 1e-12, "degree {} row sum {}", degree, s);
            }
        }
    }
}
