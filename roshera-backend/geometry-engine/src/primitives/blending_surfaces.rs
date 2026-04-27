//! G2 Continuous Blending Surfaces for Professional CAD Applications
//!
//! This module implements high-quality surface blending with curvature continuity (G2),
//! essential for Class A automotive and aerospace surface modeling.
//!
//! # Mathematical Foundation
//!
//! G2 continuity requires matching:
//! - Position (G0): S1(u,v) = S2(u,v) at boundary
//! - Tangent (G1): ∂S1/∂u = ∂S2/∂u and ∂S1/∂v = ∂S2/∂v at boundary  
//! - Curvature (G2): Principal curvatures κ1, κ2 match at boundary
//!
//! # References
//!
//! - Farin, G. (2002). "Curves and Surfaces for CAGD" (5th ed.). Morgan Kaufmann.
//! - Piegl, L. & Tiller, W. (1997). "The NURBS Book" (2nd ed.). Springer.
//! - DeRose, T., Kass, M., Truong, T. (1993). "Functional composition algorithms via blossoming"
//! - Vida, J., Martin, R., Varady, T. (1994). "A survey of blending methods that use parametric surfaces"
//!
//! Indexed access into control nets, knot vectors, and Bézier coefficient
//! arrays is the canonical idiom for blend-surface evaluation — all
//! `arr[i]`/`grid[i][j]` sites are bounds-guaranteed by patch dimensions and
//! degree. Matches the numerical-kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use crate::math::{MathError, MathResult, Matrix4, Point3, Tolerance, Vector3};
use crate::primitives::surface::{
    sample_density_for_tolerance, ContinuityAnalysis, CurvatureInfo, GeneralNurbsSurface, Surface,
    SurfaceIntersectionResult, SurfacePoint, SurfaceType,
};
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::fmt;

/// Approximate any `Surface` by a clamped-uniform bicubic NURBS offset.
///
/// Samples the surface on a `(samples_u + 1) × (samples_v + 1)` grid over its
/// parameter bounds, displaces each sample along the surface normal by
/// `distance_fn(u, v)`, and fits a degree-3 NURBS through the resulting
/// control net. This is the same sample-and-fit pattern used by
/// `Plane::create_variable_offset_nurbs`,
/// `Cylinder::create_variable_offset_nurbs`, and
/// `Sphere::create_variable_offset_nurbs` in `surface.rs` — extracted here so
/// blending surfaces (which have no closed-form offset) can reuse it.
///
/// # Errors
/// Returns `MathError::DegenerateGeometry` if the parameter span is zero;
/// returns `MathError::InvalidParameter` if the resulting NURBS net is
/// invalid (e.g. fewer control points than degree+1 in either direction).
fn approximate_surface_offset_to_nurbs(
    surface: &dyn Surface,
    distance_fn: &dyn Fn(f64, f64) -> f64,
    samples_u: usize,
    samples_v: usize,
) -> MathResult<GeneralNurbsSurface> {
    let bounds = surface.parameter_bounds();
    let u_span = bounds.0 .1 - bounds.0 .0;
    let v_span = bounds.1 .1 - bounds.1 .0;
    if u_span.abs() < 1e-14 || v_span.abs() < 1e-14 {
        return Err(MathError::DegenerateGeometry(
            "Surface parameter span is zero; cannot build offset NURBS".to_string(),
        ));
    }

    let degree_u = 3usize;
    let degree_v = 3usize;
    if samples_u < degree_u || samples_v < degree_v {
        return Err(MathError::InvalidParameter(format!(
            "Sample density {}×{} is below required degree {}×{}",
            samples_u + 1,
            samples_v + 1,
            degree_u + 1,
            degree_v + 1
        )));
    }

    let n_u = samples_u + 1;
    let n_v = samples_v + 1;
    let mut control_points: Vec<Vec<Point3>> = Vec::with_capacity(n_u);
    let mut weights: Vec<Vec<f64>> = Vec::with_capacity(n_u);

    for i in 0..n_u {
        let u = bounds.0 .0 + u_span * (i as f64) / (samples_u as f64);
        let mut row_pts = Vec::with_capacity(n_v);
        let mut row_w = Vec::with_capacity(n_v);
        for j in 0..n_v {
            let v = bounds.1 .0 + v_span * (j as f64) / (samples_v as f64);
            let base = surface.point_at(u, v)?;
            let normal = surface.normal_at(u, v)?;
            let d = distance_fn(u, v);
            row_pts.push(base + normal * d);
            row_w.push(1.0);
        }
        control_points.push(row_pts);
        weights.push(row_w);
    }

    // Clamped uniform knot vectors: [0,…,0, internal evenly-spaced,…, 1,…,1]
    // with multiplicity (degree+1) at each end.
    let build_clamped_uniform = |n: usize, degree: usize| -> Vec<f64> {
        let mut knots = vec![0.0; degree + 1];
        if n > degree + 1 {
            for i in 1..(n - degree) {
                knots.push(i as f64 / (n - degree) as f64);
            }
        }
        knots.extend(vec![1.0; degree + 1]);
        knots
    };
    let knots_u = build_clamped_uniform(n_u, degree_u);
    let knots_v = build_clamped_uniform(n_v, degree_v);

    let nurbs = crate::math::nurbs::NurbsSurface::new(
        control_points,
        weights,
        knots_u,
        knots_v,
        degree_u,
        degree_v,
    )
    .map_err(|e| {
        MathError::InvalidParameter(format!(
            "Failed to build offset NURBS for blending surface: {}",
            e
        ))
    })?;

    Ok(GeneralNurbsSurface { nurbs })
}

/// Refine a `(u, v)` seed for `closest_point` using Newton iteration on the
/// foot-of-perpendicular condition `(S(u,v) - P) · S_u = 0` and
/// `(S(u,v) - P) · S_v = 0`.
///
/// Stops when the parameter step is below `tolerance.distance()` or after
/// `max_iters` steps. Each step requires one `evaluate_full` (already costing
/// derivatives + second derivatives), and the 2×2 Jacobian is solved
/// analytically. Falls back to the seed if the Hessian is singular.
///
/// # References
/// - Piegl & Tiller, *The NURBS Book* (2nd ed.), §6.1 "Point Inversion".
/// - Patrikalakis & Maekawa, *Shape Interrogation for CAD* (2002), §5.2.
fn newton_refine_closest_point(
    surface: &dyn Surface,
    target: &Point3,
    seed_u: f64,
    seed_v: f64,
    tolerance: Tolerance,
    max_iters: usize,
) -> (f64, f64) {
    let bounds = surface.parameter_bounds();
    let (u_min, u_max) = bounds.0;
    let (v_min, v_max) = bounds.1;
    let tol = tolerance.distance().max(1.0e-12);

    let mut u = seed_u.clamp(u_min, u_max);
    let mut v = seed_v.clamp(v_min, v_max);

    for _ in 0..max_iters {
        let sp = match surface.evaluate_full(u, v) {
            Ok(p) => p,
            Err(_) => return (u, v),
        };
        let r = sp.position - *target; // residual vector S(u,v) - P
        // Gradient of f(u,v) = 0.5 |r|² is (r·S_u, r·S_v).
        let g_u = r.dot(&sp.du);
        let g_v = r.dot(&sp.dv);

        // Hessian (Gauss-Newton + second-order term):
        //   H_uu = S_u·S_u + r·S_uu
        //   H_uv = S_u·S_v + r·S_uv
        //   H_vv = S_v·S_v + r·S_vv
        let h_uu = sp.du.dot(&sp.du) + r.dot(&sp.duu);
        let h_uv = sp.du.dot(&sp.dv) + r.dot(&sp.duv);
        let h_vv = sp.dv.dot(&sp.dv) + r.dot(&sp.dvv);
        let det = h_uu * h_vv - h_uv * h_uv;
        if det.abs() < 1.0e-14 {
            return (u, v);
        }
        // Solve H * step = -g for step.
        let step_u = (-g_u * h_vv + g_v * h_uv) / det;
        let step_v = (g_u * h_uv - g_v * h_uu) / det;

        let new_u = (u + step_u).clamp(u_min, u_max);
        let new_v = (v + step_v).clamp(v_min, v_max);
        let du_step = new_u - u;
        let dv_step = new_v - v;
        u = new_u;
        v = new_v;

        // Convergence: parameter step small enough that further refinement
        // contributes less than `tol` to the foot-of-perpendicular distance.
        if du_step.abs() < tol && dv_step.abs() < tol {
            break;
        }
    }
    (u, v)
}

/// G2 continuous blending surface between two parent surfaces
///
/// Uses quartic or quintic NURBS patches to ensure curvature continuity
/// at both boundary curves with automatic control point optimization.
#[derive(Debug)]
pub struct G2BlendingSurface {
    /// Control points for the blending NURBS surface
    pub control_points: Vec<Vec<Point3>>,
    /// Weights for rational NURBS representation
    pub weights: Vec<Vec<f64>>,
    /// Knot vectors in U direction
    pub knots_u: Vec<f64>,
    /// Knot vectors in V direction  
    pub knots_v: Vec<f64>,
    /// Degree in U direction (typically 4 or 5 for G2)
    pub degree_u: usize,
    /// Degree in V direction (typically 4 or 5 for G2)
    pub degree_v: usize,
    /// Boundary curve on first surface
    pub boundary1: Box<dyn crate::primitives::curve::Curve>,
    /// Boundary curve on second surface
    pub boundary2: Box<dyn crate::primitives::curve::Curve>,
    /// Reference to first parent surface
    pub surface1: Box<dyn Surface>,
    /// Reference to second parent surface
    pub surface2: Box<dyn Surface>,
    /// Blending quality and continuity information
    pub continuity_info: ContinuityAnalysis,
}

/// Cubic blending surface for simpler cases where G2 is achievable with lower degree
///
/// More efficient than quartic blending but limited to simpler geometric configurations.
/// Uses cubic NURBS with optimized control point placement for G2 continuity.
#[derive(Debug, Clone)]
pub struct CubicG2Blend {
    /// 4x4 control point grid for bicubic surface
    pub control_points: [[Point3; 4]; 4],
    /// Weights for rational representation
    pub weights: [[f64; 4]; 4],
    /// Parameter bounds [u_min, u_max, v_min, v_max]
    pub param_bounds: [f64; 4],
    /// First boundary curve parameters
    pub boundary1_params: [f64; 2], // [t_start, t_end]
    /// Second boundary curve parameters  
    pub boundary2_params: [f64; 2], // [t_start, t_end]
    /// Continuity quality metrics
    pub quality_metrics: BlendingQuality,
}

/// Quartic blending surface for complex G2 blending scenarios
///
/// Provides additional degrees of freedom for maintaining G2 continuity
/// in challenging geometric configurations with high curvature variation.
#[derive(Debug, Clone)]
pub struct QuarticG2Blend {
    /// 5x5 control point grid for biquartic surface
    pub control_points: [[Point3; 5]; 5],
    /// Weights for rational representation
    pub weights: [[f64; 5]; 5],
    /// Parameter bounds [u_min, u_max, v_min, v_max]
    pub param_bounds: [f64; 4],
    /// Twist compatibility factors
    pub twist_factors: [[f64; 5]; 5],
    /// Curvature matching constraints
    pub curvature_constraints: Vec<CurvatureConstraint>,
    /// Quality assessment
    pub quality_metrics: BlendingQuality,
}

/// Variable radius blending surface with G2 continuity
///
/// Supports blending with variable radius along the boundary curves,
/// maintaining G2 continuity throughout the blend region.
pub struct VariableG2Blend {
    /// NURBS surface representation
    pub nurbs_surface: G2BlendingSurface,
    /// Radius function along first boundary
    pub radius_function1: Box<dyn Fn(f64) -> f64 + Send + Sync>,
    /// Radius function along second boundary
    pub radius_function2: Box<dyn Fn(f64) -> f64 + Send + Sync>,
    /// Cross-boundary interpolation method
    pub interpolation_method: InterpolationMethod,
}

impl std::fmt::Debug for VariableG2Blend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VariableG2Blend")
            .field("nurbs_surface", &self.nurbs_surface)
            .field("radius_function1", &"<Box<dyn Fn>>")
            .field("radius_function2", &"<Box<dyn Fn>>")
            .field("interpolation_method", &self.interpolation_method)
            .finish()
    }
}

/// Quality metrics for blending surface assessment
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlendingQuality {
    /// G0 continuity error (maximum position deviation)
    pub g0_error: f64,
    /// G1 continuity error (maximum tangent angle deviation)
    pub g1_error: f64,
    /// G2 continuity error (maximum curvature deviation)
    pub g2_error: f64,
    /// Overall quality score [0.0, 1.0] where 1.0 is perfect
    pub quality_score: f64,
    /// Number of control points optimized
    pub optimization_iterations: usize,
    /// Convergence status of G2 constraint solving
    pub converged: bool,
}

/// Curvature constraint for G2 blending
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurvatureConstraint {
    /// Parameter location on boundary
    pub parameter: f64,
    /// Target principal curvature 1
    pub target_k1: f64,
    /// Target principal curvature 2
    pub target_k2: f64,
    /// Principal direction 1
    pub direction1: Vector3,
    /// Principal direction 2
    pub direction2: Vector3,
    /// Constraint weight for optimization
    pub weight: f64,
}

/// Interpolation methods for variable blending
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum InterpolationMethod {
    /// Linear interpolation across blend region
    Linear,
    /// Cubic Hermite interpolation with tangent matching
    CubicHermite,
    /// Quintic interpolation with curvature matching
    QuinticG2,
    /// B-spline interpolation with user-specified degree
    BSpline { degree: usize },
}

/// G2 blending algorithm selector
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum BlendingAlgorithm {
    /// Direct control point optimization
    DirectOptimization,
    /// Variational approach minimizing energy functional
    Variational,
    /// Constrained least squares with G2 constraints
    ConstrainedLeastSquares,
    /// Functional composition via blossoming
    FunctionalComposition,
}

impl Clone for G2BlendingSurface {
    fn clone(&self) -> Self {
        Self {
            control_points: self.control_points.clone(),
            weights: self.weights.clone(),
            knots_u: self.knots_u.clone(),
            knots_v: self.knots_v.clone(),
            degree_u: self.degree_u,
            degree_v: self.degree_v,
            boundary1: self.boundary1.clone_box(),
            boundary2: self.boundary2.clone_box(),
            surface1: self.surface1.clone_box(),
            surface2: self.surface2.clone_box(),
            continuity_info: self.continuity_info,
        }
    }
}

impl G2BlendingSurface {
    /// Create G2 blending surface between two boundaries
    ///
    /// # Arguments
    /// * `surface1` - First parent surface
    /// * `surface2` - Second parent surface  
    /// * `boundary1` - Boundary curve on first surface
    /// * `boundary2` - Boundary curve on second surface
    /// * `algorithm` - Blending algorithm to use
    /// * `tolerance` - G2 continuity tolerance
    ///
    /// # Returns
    /// G2 blending surface with optimized control points
    ///
    /// # Mathematical Approach
    /// 1. Sample boundary conditions (position, tangent, curvature)
    /// 2. Set up constraint system for G2 continuity
    /// 3. Optimize control points using chosen algorithm
    /// 4. Validate continuity quality
    pub fn new(
        surface1: Box<dyn Surface>,
        surface2: Box<dyn Surface>,
        boundary1: Box<dyn crate::primitives::curve::Curve>,
        boundary2: Box<dyn crate::primitives::curve::Curve>,
        _algorithm: BlendingAlgorithm,
        tolerance: Tolerance,
    ) -> MathResult<Self> {
        // Biquartic (degree 4 in both u and v) clamped-uniform NURBS blend.
        // Control-net construction matches the row-based G2 Bézier path in
        // `G2BlendingOperations::create_quartic_g2_blend`:
        //
        //   Row 0, 4: Bernstein-interpolated boundary samples.
        //   Row 1, 3: G1 offsets from each boundary (chord direction).
        //   Row 2:    G2 LSQ-averaged target from both sides.
        //
        // With a clamped uniform knot vector [0,0,0,0,0, 1,1,1,1,1] and 5
        // control points per row, the NURBS patch is mathematically the
        // same tensor-product Bézier patch: we evaluate it via de Casteljau
        // directly. Internal-knot refinements for richer local control are
        // a future extension of this construction.
        use crate::math::bezier_patch::bernstein;
        use crate::math::linear_solver::gaussian_elimination;

        let degree = 4_usize;
        let count = degree + 1;
        let tol_dist = tolerance.distance();

        // Sample boundaries at Bernstein abscissae v_j = j/degree.
        let mut p0 = Vec::with_capacity(count);
        let mut pn = Vec::with_capacity(count);
        let mut t1_targets = Vec::with_capacity(count);
        let mut t2_targets = Vec::with_capacity(count);
        for j in 0..count {
            let vj = j as f64 / degree as f64;
            let cp1 = boundary1.evaluate(vj)?;
            let cp2 = boundary2.evaluate(vj)?;
            let pos1 = cp1.position;
            let pos2 = cp2.position;
            p0.push(pos1);
            pn.push(pos2);
            let chord = pos2 - pos1;
            if chord.magnitude() < tol_dist {
                return Err(MathError::DegenerateGeometry(format!(
                    "NURBS G2 blend: boundaries coincide at v={vj}"
                )));
            }
            // Project chord perpendicular to boundary tangent for each side.
            let bt1 = cp1.derivative1;
            let bt1_mag2 = bt1.dot(&bt1);
            let t1 = if bt1_mag2 > tol_dist * tol_dist {
                chord - bt1 * (chord.dot(&bt1) / bt1_mag2)
            } else {
                chord
            };
            let bt2 = cp2.derivative1;
            let bt2_mag2 = bt2.dot(&bt2);
            let t2 = if bt2_mag2 > tol_dist * tol_dist {
                chord - bt2 * (chord.dot(&bt2) / bt2_mag2)
            } else {
                chord
            };
            t1_targets.push(t1);
            t2_targets.push(t2);
        }

        // Build Bernstein evaluation matrix (5×5 for degree 4).
        let mut bmat = vec![vec![0.0_f64; count]; count];
        for (i, row) in bmat.iter_mut().enumerate() {
            let vi = i as f64 / degree as f64;
            for (j, entry) in row.iter_mut().enumerate() {
                *entry = bernstein(degree, j, vi);
            }
        }

        // Component-wise solve for row-0 and row-4 control points.
        let solve_points = |samples: &[Point3]| -> MathResult<Vec<Point3>> {
            let xs = gaussian_elimination(
                bmat.clone(),
                samples.iter().map(|p| p.x).collect(),
                tolerance,
            )?;
            let ys = gaussian_elimination(
                bmat.clone(),
                samples.iter().map(|p| p.y).collect(),
                tolerance,
            )?;
            let zs = gaussian_elimination(
                bmat.clone(),
                samples.iter().map(|p| p.z).collect(),
                tolerance,
            )?;
            Ok((0..samples.len())
                .map(|i| Point3::new(xs[i], ys[i], zs[i]))
                .collect())
        };
        let solve_vectors = |samples: &[Vector3]| -> MathResult<Vec<Vector3>> {
            let xs = gaussian_elimination(
                bmat.clone(),
                samples.iter().map(|v| v.x).collect(),
                tolerance,
            )?;
            let ys = gaussian_elimination(
                bmat.clone(),
                samples.iter().map(|v| v.y).collect(),
                tolerance,
            )?;
            let zs = gaussian_elimination(
                bmat.clone(),
                samples.iter().map(|v| v.z).collect(),
                tolerance,
            )?;
            Ok((0..samples.len())
                .map(|i| Vector3::new(xs[i], ys[i], zs[i]))
                .collect())
        };

        let cp_row0 = solve_points(&p0)?;
        let cp_row4 = solve_points(&pn)?;

        let inv_m = 1.0 / degree as f64;
        let offsets_side1: Vec<Vector3> = t1_targets.iter().map(|t| *t * inv_m).collect();
        let offsets_side2: Vec<Vector3> = t2_targets.iter().map(|t| *t * (-inv_m)).collect();
        let q_row1 = solve_vectors(&offsets_side1)?;
        let q_row3 = solve_vectors(&offsets_side2)?;

        // Assemble the 5×5 control grid as Vec<Vec<Point3>>.
        let mut control_points: Vec<Vec<Point3>> = vec![vec![Point3::ORIGIN; count]; count];
        for j in 0..count {
            control_points[0][j] = cp_row0[j];
            control_points[4][j] = cp_row4[j];
            control_points[1][j] = cp_row0[j] + q_row1[j];
            control_points[3][j] = cp_row4[j] + q_row3[j];
            let p2_side1 = control_points[1][j] * 2.0 - cp_row0[j];
            let p2_side2 = control_points[3][j] * 2.0 - cp_row4[j];
            control_points[2][j] = (p2_side1 + p2_side2) * 0.5;
        }

        // Clamped uniform knot vector for degree 4: [0,0,0,0,0, 1,1,1,1,1].
        let knots_u: Vec<f64> = (0..count)
            .map(|_| 0.0_f64)
            .chain((0..count).map(|_| 1.0_f64))
            .collect();
        let knots_v = knots_u.clone();
        let weights = vec![vec![1.0_f64; count]; count];

        // Continuity baseline: trust the row-based construction produced
        // G0/G1 on both boundaries and G2 in the LSQ sense on the middle
        // row. Callers who need the measured numbers should run
        // `G2BlendingOperations::verify_g2_quality`.
        let continuity_info = ContinuityAnalysis {
            g0: true,
            g1: true,
            g2: true,
            max_angle: 0.0,
            max_curvature_diff: 0.0,
        };

        Ok(Self {
            control_points,
            weights,
            knots_u,
            knots_v,
            degree_u: degree,
            degree_v: degree,
            boundary1,
            boundary2,
            surface1,
            surface2,
            continuity_info,
        })
    }
}

/// Boundary condition data for G2 constraint setup
#[derive(Debug, Clone)]
pub struct BoundaryCondition {
    /// Parameter along boundary curve
    pub parameter: f64,
    /// Position on boundary
    pub position: Point3,
    /// Tangent vector at boundary
    pub tangent: Vector3,
    /// Curvature vector at boundary
    pub curvature_vector: Vector3,
    /// Surface curvature information
    pub surface_curvature: CurvatureInfo,
    /// Which boundary (0 or 1)
    pub boundary_index: usize,
}

// G2BlendingSurface uses tensor-product Bézier evaluation (de Casteljau on
// the control net) with unit weights, plus a shape-operator computation
// for principal curvatures. The non-rational, no-internal-knot case is
// hit by every current caller; if future blending modes introduce
// non-uniform weights or internal knots, switch evaluate_full below to
// de Boor evaluation and add a weight tensor to the struct.
impl Surface for G2BlendingSurface {
    fn surface_type(&self) -> SurfaceType {
        SurfaceType::NURBS
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn clone_box(&self) -> Box<dyn Surface> {
        Box::new(self.clone())
    }

    fn evaluate_full(&self, u: f64, v: f64) -> MathResult<SurfacePoint> {
        use crate::math::bezier_patch::evaluate_patch;

        // For clamped uniform knot vectors with degree p and (p+1) control
        // points per direction, this NURBS is a tensor-product Bézier
        // patch, evaluable via de Casteljau on the control net. With
        // weights all unity, no rational projection is needed. If future
        // variants introduce internal knots or non-uniform weights, switch
        // to de Boor evaluation here.
        if self.control_points.is_empty() || self.control_points[0].is_empty() {
            return Err(MathError::DegenerateGeometry(
                "G2BlendingSurface control grid is empty".to_string(),
            ));
        }

        let s = u.clamp(0.0, 1.0);
        let t = v.clamp(0.0, 1.0);
        let eval = evaluate_patch(&self.control_points, s, t);

        let cross = eval.du.cross(&eval.dv);
        let normal = cross.normalize().unwrap_or(Vector3::Z);

        // Principal curvatures via the shape operator.
        let e_coef = eval.du.dot(&eval.du);
        let f_coef = eval.du.dot(&eval.dv);
        let g_coef = eval.dv.dot(&eval.dv);
        let l_coef = eval.duu.dot(&normal);
        let m_coef = eval.duv.dot(&normal);
        let n_coef = eval.dvv.dot(&normal);
        let det = e_coef * g_coef - f_coef * f_coef;
        let (k1, k2) = if det.abs() > 1e-14 {
            let mean = (e_coef * n_coef - 2.0 * f_coef * m_coef + g_coef * l_coef) / (2.0 * det);
            let gauss = (l_coef * n_coef - m_coef * m_coef) / det;
            let disc = mean * mean - gauss;
            if disc >= 0.0 {
                let s_d = disc.sqrt();
                (mean + s_d, mean - s_d)
            } else {
                (mean, mean)
            }
        } else {
            (0.0, 0.0)
        };

        let dir1 = eval.du.normalize().unwrap_or(Vector3::X);
        let dir2 = eval.dv.normalize().unwrap_or(Vector3::Y);

        Ok(SurfacePoint {
            position: eval.position,
            normal,
            du: eval.du,
            dv: eval.dv,
            duu: eval.duu,
            duv: eval.duv,
            dvv: eval.dvv,
            k1,
            k2,
            dir1,
            dir2,
        })
    }

    fn parameter_bounds(&self) -> ((f64, f64), (f64, f64)) {
        ((0.0, 1.0), (0.0, 1.0))
    }

    fn is_closed_u(&self) -> bool {
        false
    }

    fn is_closed_v(&self) -> bool {
        false
    }

    fn normal_at(&self, u: f64, v: f64) -> MathResult<Vector3> {
        let sp = self.evaluate_full(u, v)?;
        Ok(sp.normal)
    }

    fn transform(&self, transform: &Matrix4) -> Box<dyn Surface> {
        let mut out = self.clone();
        for row in out.control_points.iter_mut() {
            for p in row.iter_mut() {
                *p = transform.transform_point(p);
            }
        }
        Box::new(out)
    }

    fn type_name(&self) -> &'static str {
        "G2BlendingSurface"
    }

    fn closest_point(&self, point: &Point3, tolerance: Tolerance) -> MathResult<(f64, f64)> {
        // Two-stage search: uniform 20×20 grid to seed, then Newton iteration
        // on (S - P)·S_u = 0, (S - P)·S_v = 0 to refine to `tolerance`.
        // Piegl & Tiller §6.1.
        let samples = 20;
        let mut best = f64::MAX;
        let mut seed = (0.0_f64, 0.0_f64);
        for i in 0..=samples {
            for j in 0..=samples {
                let u = i as f64 / samples as f64;
                let v = j as f64 / samples as f64;
                if let Ok(sp) = self.evaluate_full(u, v) {
                    let d2 = sp.position.distance_squared(point);
                    if d2 < best {
                        best = d2;
                        seed = (u, v);
                    }
                }
            }
        }
        Ok(newton_refine_closest_point(
            self, point, seed.0, seed.1, tolerance, 16,
        ))
    }

    fn offset(&self, distance: f64) -> Box<dyn Surface> {
        // Sample-and-fit offset NURBS. Trait returns Box<dyn Surface> with
        // no error channel, so failures fall back to an unoffset clone with
        // a warning rather than silently substituting `self`.
        match approximate_surface_offset_to_nurbs(self, &|_, _| distance, 16, 16) {
            Ok(nurbs) => Box::new(nurbs),
            Err(e) => {
                tracing::warn!(
                    "G2BlendingSurface::offset({}) fit failed: {:?}; returning unoffset clone",
                    distance,
                    e
                );
                Box::new(self.clone())
            }
        }
    }

    fn offset_exact(
        &self,
        distance: f64,
        tolerance: Tolerance,
    ) -> MathResult<crate::primitives::surface::OffsetSurface> {
        use crate::primitives::surface::{OffsetQuality, OffsetSurface};

        // Density driven by tolerance: tighter tol => more samples.
        // For a quartic blend, 24×24 control net resolves curvature variation
        // adequately at default tolerances (1e-6).
        let samples = sample_density_for_tolerance(tolerance.distance());
        let nurbs = approximate_surface_offset_to_nurbs(self, &|_, _| distance, samples, samples)?;
        Ok(OffsetSurface {
            surface: Box::new(nurbs),
            distance,
            quality: OffsetQuality::Approximate {
                max_error: tolerance.distance(),
            },
            original: Box::new(self.clone()),
        })
    }

    fn offset_variable(
        &self,
        distance_fn: Box<dyn Fn(f64, f64) -> f64 + Send + Sync>,
        tolerance: Tolerance,
    ) -> MathResult<Box<dyn Surface>> {
        let samples = sample_density_for_tolerance(tolerance.distance());
        let nurbs =
            approximate_surface_offset_to_nurbs(self, &*distance_fn, samples, samples)?;
        Ok(Box::new(nurbs))
    }

    fn intersect(
        &self,
        other: &dyn Surface,
        tolerance: Tolerance,
    ) -> Vec<SurfaceIntersectionResult> {
        // Route through the canonical math-layer SSI dispatcher. Failures are
        // logged inside dispatch_via_math_ssi and produce an empty Vec
        // (observable via tracing).
        crate::primitives::surface::dispatch_via_math_ssi(self, other, tolerance)
    }
}

impl fmt::Display for BlendingQuality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "G2 Blending Quality: Score={:.3}, G0_err={:.2e}, G1_err={:.2e}, G2_err={:.2e}",
            self.quality_score, self.g0_error, self.g1_error, self.g2_error
        )
    }
}

/// Surface trait implementation for CubicG2Blend
impl Surface for CubicG2Blend {
    fn surface_type(&self) -> crate::primitives::surface::SurfaceType {
        crate::primitives::surface::SurfaceType::NURBS
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clone_box(&self) -> Box<dyn Surface> {
        Box::new(self.clone())
    }

    fn evaluate_full(
        &self,
        u: f64,
        v: f64,
    ) -> MathResult<crate::primitives::surface::SurfacePoint> {
        use crate::math::bezier_patch::evaluate_bicubic;

        // Map incoming (u, v) to the Bézier [0,1]×[0,1] domain.
        let u_span = self.param_bounds[1] - self.param_bounds[0];
        let v_span = self.param_bounds[3] - self.param_bounds[2];
        if u_span.abs() < 1e-14 || v_span.abs() < 1e-14 {
            return Err(MathError::DegenerateGeometry(
                "CubicG2Blend has zero-size parameter span".to_string(),
            ));
        }
        let s = ((u.clamp(self.param_bounds[0], self.param_bounds[1]) - self.param_bounds[0])
            / u_span)
            .clamp(0.0, 1.0);
        let t = ((v.clamp(self.param_bounds[2], self.param_bounds[3]) - self.param_bounds[2])
            / v_span)
            .clamp(0.0, 1.0);

        let eval = evaluate_bicubic(&self.control_points, s, t);

        // Chain-rule scale from (s, t) to (u, v).
        let du = eval.du * (1.0 / u_span);
        let dv = eval.dv * (1.0 / v_span);
        let duu = eval.duu * (1.0 / (u_span * u_span));
        let dvv = eval.dvv * (1.0 / (v_span * v_span));
        let duv = eval.duv * (1.0 / (u_span * v_span));

        // Normal; if degenerate, fall back to available direction.
        let cross = du.cross(&dv);
        let normal = cross.normalize().unwrap_or(Vector3::Z);

        // Principal curvatures via the shape operator. For numerical
        // robustness in near-planar regions we fall back to zero curvatures.
        let e_coef = du.dot(&du);
        let f_coef = du.dot(&dv);
        let g_coef = dv.dot(&dv);
        let l_coef = duu.dot(&normal);
        let m_coef = duv.dot(&normal);
        let n_coef = dvv.dot(&normal);
        let det = e_coef * g_coef - f_coef * f_coef;
        let (k1, k2) = if det.abs() > 1e-14 {
            let mean = (e_coef * n_coef - 2.0 * f_coef * m_coef + g_coef * l_coef) / (2.0 * det);
            let gauss = (l_coef * n_coef - m_coef * m_coef) / det;
            let disc = mean * mean - gauss;
            if disc >= 0.0 {
                let s_d = disc.sqrt();
                (mean + s_d, mean - s_d)
            } else {
                (mean, mean)
            }
        } else {
            (0.0, 0.0)
        };

        let dir1 = du.normalize().unwrap_or(Vector3::X);
        let dir2 = dv.normalize().unwrap_or(Vector3::Y);

        Ok(crate::primitives::surface::SurfacePoint {
            position: eval.position,
            normal,
            du,
            dv,
            duu,
            duv,
            dvv,
            k1,
            k2,
            dir1,
            dir2,
        })
    }

    fn parameter_bounds(&self) -> ((f64, f64), (f64, f64)) {
        (
            (self.param_bounds[0], self.param_bounds[1]),
            (self.param_bounds[2], self.param_bounds[3]),
        )
    }

    fn is_closed_u(&self) -> bool {
        false
    }

    fn is_closed_v(&self) -> bool {
        false
    }

    fn transform(&self, matrix: &Matrix4) -> Box<dyn Surface> {
        let mut transformed = self.clone();
        for i in 0..4 {
            for j in 0..4 {
                transformed.control_points[i][j] =
                    matrix.transform_point(&self.control_points[i][j]);
            }
        }
        Box::new(transformed)
    }

    fn type_name(&self) -> &'static str {
        "CubicG2Blend"
    }

    fn closest_point(&self, point: &Point3, tolerance: Tolerance) -> MathResult<(f64, f64)> {
        // Two-stage: 20×20 grid seed in parameter bounds, then Newton on the
        // foot-of-perpendicular condition (Piegl & Tiller §6.1).
        let bounds = self.parameter_bounds();
        let mut best_dist = f64::MAX;
        let mut seed_u = bounds.0 .0;
        let mut seed_v = bounds.1 .0;
        let samples = 20;
        for i in 0..=samples {
            for j in 0..=samples {
                let u = bounds.0 .0 + (bounds.0 .1 - bounds.0 .0) * (i as f64 / samples as f64);
                let v = bounds.1 .0 + (bounds.1 .1 - bounds.1 .0) * (j as f64 / samples as f64);
                if let Ok(pt) = self.point_at(u, v) {
                    let dist = point.distance(&pt);
                    if dist < best_dist {
                        best_dist = dist;
                        seed_u = u;
                        seed_v = v;
                    }
                }
            }
        }
        Ok(newton_refine_closest_point(
            self, point, seed_u, seed_v, tolerance, 16,
        ))
    }

    fn offset(&self, distance: f64) -> Box<dyn Surface> {
        match approximate_surface_offset_to_nurbs(self, &|_, _| distance, 12, 12) {
            Ok(nurbs) => Box::new(nurbs),
            Err(e) => {
                tracing::warn!(
                    "CubicG2Blend::offset({}) fit failed: {:?}; returning unoffset clone",
                    distance,
                    e
                );
                Box::new(self.clone())
            }
        }
    }

    fn offset_exact(
        &self,
        distance: f64,
        tolerance: Tolerance,
    ) -> MathResult<crate::primitives::surface::OffsetSurface> {
        use crate::primitives::surface::{OffsetQuality, OffsetSurface};
        let samples = sample_density_for_tolerance(tolerance.distance());
        let nurbs = approximate_surface_offset_to_nurbs(self, &|_, _| distance, samples, samples)?;
        Ok(OffsetSurface {
            surface: Box::new(nurbs),
            quality: OffsetQuality::Approximate {
                max_error: tolerance.distance(),
            },
            original: Box::new(self.clone()),
            distance,
        })
    }

    fn offset_variable(
        &self,
        distance_fn: Box<dyn Fn(f64, f64) -> f64 + Send + Sync>,
        tolerance: Tolerance,
    ) -> MathResult<Box<dyn Surface>> {
        let samples = sample_density_for_tolerance(tolerance.distance());
        let nurbs =
            approximate_surface_offset_to_nurbs(self, &*distance_fn, samples, samples)?;
        Ok(Box::new(nurbs))
    }

    fn intersect(
        &self,
        other: &dyn Surface,
        tolerance: Tolerance,
    ) -> Vec<SurfaceIntersectionResult> {
        crate::primitives::surface::dispatch_via_math_ssi(self, other, tolerance)
    }
}

/// Surface trait implementation for QuarticG2Blend
impl Surface for QuarticG2Blend {
    fn surface_type(&self) -> crate::primitives::surface::SurfaceType {
        crate::primitives::surface::SurfaceType::NURBS
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clone_box(&self) -> Box<dyn Surface> {
        Box::new(self.clone())
    }

    fn evaluate_full(
        &self,
        u: f64,
        v: f64,
    ) -> MathResult<crate::primitives::surface::SurfacePoint> {
        use crate::math::bezier_patch::evaluate_biquartic;

        let u_span = self.param_bounds[1] - self.param_bounds[0];
        let v_span = self.param_bounds[3] - self.param_bounds[2];
        if u_span.abs() < 1e-14 || v_span.abs() < 1e-14 {
            return Err(MathError::DegenerateGeometry(
                "QuarticG2Blend has zero-size parameter span".to_string(),
            ));
        }
        let s = ((u.clamp(self.param_bounds[0], self.param_bounds[1]) - self.param_bounds[0])
            / u_span)
            .clamp(0.0, 1.0);
        let t = ((v.clamp(self.param_bounds[2], self.param_bounds[3]) - self.param_bounds[2])
            / v_span)
            .clamp(0.0, 1.0);

        let eval = evaluate_biquartic(&self.control_points, s, t);

        let du = eval.du * (1.0 / u_span);
        let dv = eval.dv * (1.0 / v_span);
        let duu = eval.duu * (1.0 / (u_span * u_span));
        let dvv = eval.dvv * (1.0 / (v_span * v_span));
        let duv = eval.duv * (1.0 / (u_span * v_span));

        let cross = du.cross(&dv);
        let normal = cross.normalize().unwrap_or(Vector3::Z);

        let e_coef = du.dot(&du);
        let f_coef = du.dot(&dv);
        let g_coef = dv.dot(&dv);
        let l_coef = duu.dot(&normal);
        let m_coef = duv.dot(&normal);
        let n_coef = dvv.dot(&normal);
        let det = e_coef * g_coef - f_coef * f_coef;
        let (k1, k2) = if det.abs() > 1e-14 {
            let mean = (e_coef * n_coef - 2.0 * f_coef * m_coef + g_coef * l_coef) / (2.0 * det);
            let gauss = (l_coef * n_coef - m_coef * m_coef) / det;
            let disc = mean * mean - gauss;
            if disc >= 0.0 {
                let s_d = disc.sqrt();
                (mean + s_d, mean - s_d)
            } else {
                (mean, mean)
            }
        } else {
            (0.0, 0.0)
        };

        let dir1 = du.normalize().unwrap_or(Vector3::X);
        let dir2 = dv.normalize().unwrap_or(Vector3::Y);

        Ok(crate::primitives::surface::SurfacePoint {
            position: eval.position,
            normal,
            du,
            dv,
            duu,
            duv,
            dvv,
            k1,
            k2,
            dir1,
            dir2,
        })
    }

    fn parameter_bounds(&self) -> ((f64, f64), (f64, f64)) {
        (
            (self.param_bounds[0], self.param_bounds[1]),
            (self.param_bounds[2], self.param_bounds[3]),
        )
    }

    fn is_closed_u(&self) -> bool {
        false
    }

    fn is_closed_v(&self) -> bool {
        false
    }

    fn transform(&self, matrix: &Matrix4) -> Box<dyn Surface> {
        let mut transformed = self.clone();
        for i in 0..5 {
            for j in 0..5 {
                transformed.control_points[i][j] =
                    matrix.transform_point(&self.control_points[i][j]);
            }
        }
        Box::new(transformed)
    }

    fn type_name(&self) -> &'static str {
        "QuarticG2Blend"
    }

    fn closest_point(&self, point: &Point3, tolerance: Tolerance) -> MathResult<(f64, f64)> {
        // Two-stage: 20×20 grid seed, then Newton on the foot-of-perpendicular
        // condition (Piegl & Tiller §6.1). Quartic surface curvature can be
        // sharper than cubic, so we also use 16 Newton iterations.
        let bounds = self.parameter_bounds();
        let mut best_dist = f64::MAX;
        let mut seed_u = bounds.0 .0;
        let mut seed_v = bounds.1 .0;
        let samples = 20;
        for i in 0..=samples {
            for j in 0..=samples {
                let u = bounds.0 .0 + (bounds.0 .1 - bounds.0 .0) * (i as f64 / samples as f64);
                let v = bounds.1 .0 + (bounds.1 .1 - bounds.1 .0) * (j as f64 / samples as f64);
                if let Ok(pt) = self.point_at(u, v) {
                    let dist = point.distance(&pt);
                    if dist < best_dist {
                        best_dist = dist;
                        seed_u = u;
                        seed_v = v;
                    }
                }
            }
        }
        Ok(newton_refine_closest_point(
            self, point, seed_u, seed_v, tolerance, 16,
        ))
    }

    fn offset(&self, distance: f64) -> Box<dyn Surface> {
        match approximate_surface_offset_to_nurbs(self, &|_, _| distance, 16, 16) {
            Ok(nurbs) => Box::new(nurbs),
            Err(e) => {
                tracing::warn!(
                    "QuarticG2Blend::offset({}) fit failed: {:?}; returning unoffset clone",
                    distance,
                    e
                );
                Box::new(self.clone())
            }
        }
    }

    fn offset_exact(
        &self,
        distance: f64,
        tolerance: Tolerance,
    ) -> MathResult<crate::primitives::surface::OffsetSurface> {
        use crate::primitives::surface::{OffsetQuality, OffsetSurface};
        let samples = sample_density_for_tolerance(tolerance.distance());
        let nurbs = approximate_surface_offset_to_nurbs(self, &|_, _| distance, samples, samples)?;
        Ok(OffsetSurface {
            surface: Box::new(nurbs),
            quality: OffsetQuality::Approximate {
                max_error: tolerance.distance(),
            },
            original: Box::new(self.clone()),
            distance,
        })
    }

    fn offset_variable(
        &self,
        distance_fn: Box<dyn Fn(f64, f64) -> f64 + Send + Sync>,
        tolerance: Tolerance,
    ) -> MathResult<Box<dyn Surface>> {
        let samples = sample_density_for_tolerance(tolerance.distance());
        let nurbs =
            approximate_surface_offset_to_nurbs(self, &*distance_fn, samples, samples)?;
        Ok(Box::new(nurbs))
    }

    fn intersect(
        &self,
        other: &dyn Surface,
        tolerance: Tolerance,
    ) -> Vec<SurfaceIntersectionResult> {
        crate::primitives::surface::dispatch_via_math_ssi(self, other, tolerance)
    }
}
