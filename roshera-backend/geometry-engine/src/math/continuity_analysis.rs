//! Continuity Analysis for Curves and Surfaces
//!
//! Provides tools for analyzing G0, G1, and G2 continuity between
//! curves and surfaces at their boundaries.

use crate::math::bspline::BSplineCurve;
use crate::math::nurbs::{NurbsCurve, NurbsSurface};
use crate::math::{MathError, MathResult, Tolerance, Vector3};
use std::fmt;

/// Continuity type between geometric entities
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContinuityType {
    /// No continuity
    None = 0,
    /// G0: Position continuity only
    G0 = 1,
    /// G1: Position and tangent continuity
    G1 = 2,
    /// G2: Position, tangent, and curvature continuity
    G2 = 3,
}

impl fmt::Display for ContinuityType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContinuityType::None => write!(f, "None"),
            ContinuityType::G0 => write!(f, "G0 (Position)"),
            ContinuityType::G1 => write!(f, "G1 (Tangent)"),
            ContinuityType::G2 => write!(f, "G2 (Curvature)"),
        }
    }
}

/// Result of continuity analysis
#[derive(Debug, Clone)]
pub struct ContinuityResult {
    /// Achieved continuity level
    pub continuity: ContinuityType,
    /// Position error (for G0)
    pub position_error: f64,
    /// Tangent angle error in radians (for G1)
    pub tangent_error: Option<f64>,
    /// Curvature error (for G2)
    pub curvature_error: Option<f64>,
    /// Detailed message
    pub message: String,
}

/// Analyze continuity between two B-spline curves at their junction
pub fn analyze_curve_continuity(
    curve1: &BSplineCurve,
    at_end1: bool,
    curve2: &BSplineCurve,
    at_start2: bool,
    tolerance: Tolerance,
) -> MathResult<ContinuityResult> {
    // Get parameter values at junction
    let t1 = if at_end1 {
        curve1.param_range.1
    } else {
        curve1.param_range.0
    };
    let t2 = if at_start2 {
        curve2.param_range.0
    } else {
        curve2.param_range.1
    };

    // Evaluate positions
    let pos1 = curve1.evaluate(t1)?;
    let pos2 = curve2.evaluate(t2)?;
    let position_error = (pos2 - pos1).magnitude();

    // Check G0 continuity
    if position_error > tolerance.distance() {
        return Ok(ContinuityResult {
            continuity: ContinuityType::None,
            position_error,
            tangent_error: None,
            curvature_error: None,
            message: format!("Position discontinuity: error = {:.6}", position_error),
        });
    }

    // Evaluate first derivatives (tangents)
    let derivs1 = curve1.evaluate_derivatives(t1, 2)?;
    let derivs2 = curve2.evaluate_derivatives(t2, 2)?;

    let tangent1 = if at_end1 { derivs1[1] } else { -derivs1[1] };
    let tangent2 = if at_start2 { derivs2[1] } else { -derivs2[1] };

    // Normalize tangents
    let t1_norm = tangent1.normalize()?;
    let t2_norm = tangent2.normalize()?;

    // Check G1 continuity
    let tangent_dot = t1_norm.dot(&t2_norm);
    let tangent_angle = tangent_dot.clamp(-1.0, 1.0).acos();

    if tangent_angle > tolerance.angle() {
        return Ok(ContinuityResult {
            continuity: ContinuityType::G0,
            position_error,
            tangent_error: Some(tangent_angle),
            curvature_error: None,
            message: format!("Tangent discontinuity: angle = {:.6} rad", tangent_angle),
        });
    }

    // For G2 continuity, check curvature
    let curvature1 = compute_curve_curvature(&derivs1)?;
    let curvature2 = compute_curve_curvature(&derivs2)?;

    // Account for direction at junction
    let curvature1_adj = if at_end1 { curvature1 } else { -curvature1 };
    let curvature2_adj = if at_start2 { curvature2 } else { -curvature2 };

    let curvature_error = (curvature2_adj - curvature1_adj).magnitude();

    // Check if curvatures match
    let curvature_tolerance = tolerance.distance() * 10.0; // Scaled tolerance for curvature

    if curvature_error > curvature_tolerance {
        return Ok(ContinuityResult {
            continuity: ContinuityType::G1,
            position_error,
            tangent_error: Some(tangent_angle),
            curvature_error: Some(curvature_error),
            message: format!("Curvature discontinuity: error = {:.6}", curvature_error),
        });
    }

    // G2 continuity achieved
    Ok(ContinuityResult {
        continuity: ContinuityType::G2,
        position_error,
        tangent_error: Some(tangent_angle),
        curvature_error: Some(curvature_error),
        message: "G2 continuity achieved".to_string(),
    })
}

/// Compute curvature vector from derivatives
fn compute_curve_curvature(derivatives: &[Vector3]) -> MathResult<Vector3> {
    if derivatives.len() < 3 {
        return Err(MathError::InvalidParameter(
            "Need at least 3 derivatives".into(),
        ));
    }

    let r_prime = derivatives[1];
    let r_double_prime = derivatives[2];

    let speed = r_prime.magnitude();
    if speed < 1e-10 {
        return Ok(Vector3::ZERO);
    }

    // Curvature vector κ = (r' × r'') / |r'|³
    let cross = r_prime.cross(&r_double_prime);
    Ok(cross / speed.powi(3))
}

/// Analyze continuity between NURBS curves
pub fn analyze_nurbs_curve_continuity(
    curve1: &NurbsCurve,
    at_end1: bool,
    curve2: &NurbsCurve,
    at_start2: bool,
    tolerance: Tolerance,
) -> MathResult<ContinuityResult> {
    // Convert to evaluation parameters
    let t1 = if at_end1 { 1.0 } else { 0.0 };
    let t2 = if at_start2 { 0.0 } else { 1.0 };

    // Evaluate positions
    let pos1 = curve1.evaluate(t1).point;
    let pos2 = curve2.evaluate(t2).point;
    let position_error = (pos2 - pos1).magnitude();

    // Check G0 continuity
    if position_error > tolerance.distance() {
        return Ok(ContinuityResult {
            continuity: ContinuityType::None,
            position_error,
            tangent_error: None,
            curvature_error: None,
            message: format!("Position discontinuity: error = {:.6}", position_error),
        });
    }

    // Get derivatives
    let derivs1 = curve1.evaluate_derivatives(t1, 2);
    let derivs2 = curve2.evaluate_derivatives(t2, 2);
    let _pos1 = derivs1.point;
    let _pos2 = derivs2.point;
    let deriv1 = derivs1.derivative1.unwrap_or(Vector3::ZERO);
    let deriv2 = derivs2.derivative1.unwrap_or(Vector3::ZERO);
    let deriv2_1 = derivs1.derivative2.unwrap_or(Vector3::ZERO);
    let deriv2_2 = derivs2.derivative2.unwrap_or(Vector3::ZERO);

    // Adjust for direction
    let tangent1 = if at_end1 { deriv1 } else { -deriv1 };
    let tangent2 = if at_start2 { deriv2 } else { -deriv2 };

    // Check tangent continuity
    let t1_norm = tangent1.normalize()?;
    let t2_norm = tangent2.normalize()?;
    let tangent_angle = t1_norm.angle(&t2_norm)?;

    if tangent_angle > tolerance.angle() {
        return Ok(ContinuityResult {
            continuity: ContinuityType::G0,
            position_error,
            tangent_error: Some(tangent_angle),
            curvature_error: None,
            message: format!("Tangent discontinuity: angle = {:.6} rad", tangent_angle),
        });
    }

    // Compute curvatures
    let speed1 = tangent1.magnitude();
    let speed2 = tangent2.magnitude();

    if speed1 < 1e-10 || speed2 < 1e-10 {
        return Ok(ContinuityResult {
            continuity: ContinuityType::G1,
            position_error,
            tangent_error: Some(tangent_angle),
            curvature_error: None,
            message: "Cannot compute curvature at zero-speed point".to_string(),
        });
    }

    // Second derivatives are direction-independent (curvature vector
    // depends on |C'|^3, sign cancels). The at_end1 / at_start2 flags
    // are unused here — kept for future tangent-direction work.
    let accel1 = deriv2_1;
    let accel2 = deriv2_2;

    // Compute curvature vectors
    let kappa1 = tangent1.cross(&accel1) / speed1.powi(3);
    let kappa2 = tangent2.cross(&accel2) / speed2.powi(3);

    let curvature_error = (kappa2 - kappa1).magnitude();
    let curvature_tolerance = tolerance.distance() * 10.0;

    if curvature_error > curvature_tolerance {
        return Ok(ContinuityResult {
            continuity: ContinuityType::G1,
            position_error,
            tangent_error: Some(tangent_angle),
            curvature_error: Some(curvature_error),
            message: format!("Curvature discontinuity: error = {:.6}", curvature_error),
        });
    }

    Ok(ContinuityResult {
        continuity: ContinuityType::G2,
        position_error,
        tangent_error: Some(tangent_angle),
        curvature_error: Some(curvature_error),
        message: "G2 continuity achieved".to_string(),
    })
}

/// Analyze continuity between surfaces along a common edge
pub fn analyze_surface_continuity(
    surface1: &NurbsSurface,
    edge1: SurfaceEdge,
    surface2: &NurbsSurface,
    edge2: SurfaceEdge,
    num_samples: usize,
    tolerance: Tolerance,
) -> MathResult<ContinuityResult> {
    let mut max_position_error = 0.0;
    let mut max_tangent_error = 0.0;
    let mut max_curvature_error = 0.0;
    let mut min_continuity = ContinuityType::G2;

    // Sample along the edge
    for i in 0..=num_samples {
        let t = i as f64 / num_samples as f64;

        // Get parameters on both surfaces
        let (u1, v1) = edge1.get_parameters(t);
        let (u2, v2) = edge2.get_parameters(t);

        // Check position continuity
        let pos1 = surface1.evaluate(u1, v1).point;
        let pos2 = surface2.evaluate(u2, v2).point;
        let pos_error = (pos2 - pos1).magnitude();
        max_position_error = f64::max(max_position_error, pos_error);

        if pos_error > tolerance.distance() {
            min_continuity = ContinuityType::None;
            continue;
        }

        // Get normals
        let normal1 = surface1.normal_at(u1, v1)?;
        let normal2 = surface2.normal_at(u2, v2)?;

        // For G1 continuity, normals should be parallel (same or opposite)
        let normal_dot = normal1.dot(&normal2).abs();
        let normal_angle = (1.0 - normal_dot).acos();
        max_tangent_error = f64::max(max_tangent_error, normal_angle);

        if normal_angle > tolerance.angle() {
            min_continuity = min_continuity.min(ContinuityType::G0);
            continue;
        }

        // For G2 continuity, check curvatures
        let curvature_error = estimate_surface_curvature_difference(
            surface1,
            u1,
            v1,
            surface2,
            u2,
            v2,
            &edge1.tangent_at(t)?,
        )?;

        max_curvature_error = f64::max(max_curvature_error, curvature_error);

        if curvature_error > tolerance.distance() * 10.0 {
            min_continuity = min_continuity.min(ContinuityType::G1);
        }
    }

    Ok(ContinuityResult {
        continuity: min_continuity,
        position_error: max_position_error,
        tangent_error: if min_continuity >= ContinuityType::G0 {
            Some(max_tangent_error)
        } else {
            None
        },
        curvature_error: if min_continuity >= ContinuityType::G1 {
            Some(max_curvature_error)
        } else {
            None
        },
        message: format!("Surface continuity: {}", min_continuity),
    })
}

/// Surface edge representation
#[derive(Debug, Clone)]
pub struct SurfaceEdge {
    /// Which edge: 0=u_min, 1=u_max, 2=v_min, 3=v_max
    pub edge_type: usize,
    /// Parameter range [0,1] along the edge
    pub range: (f64, f64),
}

impl SurfaceEdge {
    /// Get (u,v) parameters at t ∈ [0,1] along edge
    pub fn get_parameters(&self, t: f64) -> (f64, f64) {
        let s = self.range.0 + t * (self.range.1 - self.range.0);
        match self.edge_type {
            0 => (0.0, s), // u_min edge
            1 => (1.0, s), // u_max edge
            2 => (s, 0.0), // v_min edge
            3 => (s, 1.0), // v_max edge
            _ => (0.0, 0.0),
        }
    }

    /// Get tangent direction along edge
    pub fn tangent_at(&self, _t: f64) -> MathResult<Vector3> {
        match self.edge_type {
            0 | 1 => Ok(Vector3::new(0.0, 1.0, 0.0)), // Along v
            2 | 3 => Ok(Vector3::new(1.0, 0.0, 0.0)), // Along u
            _ => Err(MathError::InvalidParameter("Invalid edge type".into())),
        }
    }
}

/// Estimate curvature difference between surfaces
fn estimate_surface_curvature_difference(
    surface1: &NurbsSurface,
    u1: f64,
    v1: f64,
    surface2: &NurbsSurface,
    u2: f64,
    v2: f64,
    edge_tangent: &Vector3,
) -> MathResult<f64> {
    // Get second derivatives
    let h = 1e-6;

    // Compute normal curvature in direction perpendicular to edge
    let n1 = surface1.normal_at(u1, v1)?;
    let n2 = surface2.normal_at(u2, v2)?;

    // Direction perpendicular to edge and in surface
    let perp1 = n1.cross(edge_tangent).normalize()?;
    let perp2 = n2.cross(edge_tangent).normalize()?;

    // Approximate curvature by finite differences
    let p1_0 = surface1.evaluate(u1, v1).point;
    let p1_1 = surface1.evaluate(u1 + h * perp1.x, v1 + h * perp1.y).point;
    let p1_2 = surface1.evaluate(u1 - h * perp1.x, v1 - h * perp1.y).point;

    let p2_0 = surface2.evaluate(u2, v2).point;
    let p2_1 = surface2.evaluate(u2 + h * perp2.x, v2 + h * perp2.y).point;
    let p2_2 = surface2.evaluate(u2 - h * perp2.x, v2 - h * perp2.y).point;

    // Second derivative approximation
    let d2_1 = (p1_1 - 2.0 * p1_0 + p1_2) / (h * h);
    let d2_2 = (p2_1 - 2.0 * p2_0 + p2_2) / (h * h);

    Ok((d2_2 - d2_1).magnitude())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Point3;

    #[test]
    fn test_g0_continuity() {
        // Create two line segments that meet
        let points1 = vec![Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0)];
        let points2 = vec![Point3::new(1.0, 0.0, 0.0), Point3::new(2.0, 0.0, 0.0)];

        let curve1 = BSplineCurve::open_uniform(1, points1).unwrap();
        let curve2 = BSplineCurve::open_uniform(1, points2).unwrap();

        let result =
            analyze_curve_continuity(&curve1, true, &curve2, true, Tolerance::default()).unwrap();

        assert_eq!(result.continuity, ContinuityType::G2); // Lines have zero curvature
        assert!(result.position_error < 1e-10);
    }

    #[test]
    fn test_continuity_types() {
        assert_eq!(format!("{}", ContinuityType::G0), "G0 (Position)");
        assert_eq!(format!("{}", ContinuityType::G1), "G1 (Tangent)");
        assert_eq!(format!("{}", ContinuityType::G2), "G2 (Curvature)");
    }
}
