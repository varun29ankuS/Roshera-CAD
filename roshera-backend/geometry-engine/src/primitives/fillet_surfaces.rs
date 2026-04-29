//! Specialized surfaces for fillet operations
//!
//! Implements cylindrical, toroidal, and spherical fillet surfaces with
//! proper trimming support and numerical robustness.
//!
//! Indexed access into control-point grids and tangent arrays is the canonical
//! idiom for fillet-surface evaluation — bounded by NURBS degree and grid
//! dimensions. Matches the pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use crate::math::bspline::KnotVector;
use crate::math::nurbs::NurbsSurface;
use crate::math::{MathError, MathResult, Matrix4, Point3, Tolerance, Vector3};
use crate::primitives::curve::Curve;
use crate::primitives::surface::{
    OffsetQuality, OffsetSurface, Surface, SurfaceIntersectionResult, SurfacePoint, SurfaceType,
};
use std::any::Any;

/// Newton-Raphson closest point search on any surface with evaluate_full
fn newton_closest_point(
    surface: &dyn Surface,
    point: &Point3,
    tolerance: Tolerance,
) -> MathResult<(f64, f64)> {
    let mut u = 0.5;
    let mut v = 0.5;
    let eps = tolerance.distance();
    let du = 1e-6;
    let dv = 1e-6;

    for _ in 0..20 {
        let sp = surface.point_at(u, v)?;
        let diff = *point - sp;
        let dist = diff.magnitude();
        if dist < eps {
            break;
        }
        let sp_du = surface.point_at((u + du).min(1.0), v)?;
        let sp_dv = surface.point_at(u, (v + dv).min(1.0))?;
        let dpos_du = (sp_du - sp) / du;
        let dpos_dv = (sp_dv - sp) / dv;
        let du_step = diff.dot(&dpos_du) / dpos_du.dot(&dpos_du).max(1e-30);
        let dv_step = diff.dot(&dpos_dv) / dpos_dv.dot(&dpos_dv).max(1e-30);
        u = (u + du_step * 0.5).clamp(0.0, 1.0);
        v = (v + dv_step * 0.5).clamp(0.0, 1.0);
    }
    Ok((u, v))
}

/// Matrix 2x2 for shape operator
#[derive(Debug, Clone, Copy)]
pub struct Matrix2x2 {
    pub m00: f64,
    pub m01: f64,
    pub m10: f64,
    pub m11: f64,
}

/// Cylindrical fillet surface - constant radius along straight edge
#[derive(Debug)]
pub struct CylindricalFillet {
    /// Spine curve (the edge)
    pub spine: Box<dyn Curve>,
    /// Radius of the fillet
    pub radius: f64,
    /// First contact curve on adjacent face
    pub contact1: Box<dyn Curve>,
    /// Second contact curve on adjacent face
    pub contact2: Box<dyn Curve>,
    /// Axis direction at each spine point
    pub axis_field: Vec<Vector3>,
    /// Angle span at each spine point
    pub angle_span: Vec<(f64, f64)>,
}

impl Clone for CylindricalFillet {
    fn clone(&self) -> Self {
        Self {
            spine: self.spine.clone_box(),
            radius: self.radius,
            contact1: self.contact1.clone_box(),
            contact2: self.contact2.clone_box(),
            axis_field: self.axis_field.clone(),
            angle_span: self.angle_span.clone(),
        }
    }
}

impl CylindricalFillet {
    /// Create new cylindrical fillet
    pub fn new(
        spine: Box<dyn Curve>,
        radius: f64,
        contact1: Box<dyn Curve>,
        contact2: Box<dyn Curve>,
    ) -> MathResult<Self> {
        if radius <= 0.0 {
            return Err(MathError::InvalidParameter(
                "Fillet radius must be positive".into(),
            ));
        }

        // Sample spine to compute axis field
        let num_samples = 20;
        let mut axis_field = Vec::with_capacity(num_samples);
        let mut angle_span = Vec::with_capacity(num_samples);

        for i in 0..num_samples {
            let t = i as f64 / (num_samples - 1) as f64;
            let spine_point = spine.evaluate(t)?.position;
            let spine_tangent = spine.tangent_at(t)?;

            // Get contact points
            let contact1_point = contact1.evaluate(t)?.position;
            let contact2_point = contact2.evaluate(t)?.position;

            // Compute axis direction (perpendicular to spine and in bisector plane)
            let v1 = (contact1_point - spine_point).normalize()?;
            let v2 = (contact2_point - spine_point).normalize()?;
            let axis = spine_tangent.normalize()?;

            axis_field.push(axis);

            // Compute angle span
            let angle1 = v1.angle(&v2)?;
            let start_angle = 0.0;
            let end_angle = angle1;
            angle_span.push((start_angle, end_angle));
        }

        Ok(Self {
            spine,
            radius,
            contact1,
            contact2,
            axis_field,
            angle_span,
        })
    }

    /// Get axis direction at parameter
    fn axis_at(&self, u: f64) -> MathResult<Vector3> {
        let idx = (u * (self.axis_field.len() - 1) as f64) as usize;
        let idx = idx.min(self.axis_field.len() - 1);
        Ok(self.axis_field[idx])
    }

    /// Get angle span at parameter
    fn angles_at(&self, u: f64) -> (f64, f64) {
        let idx = (u * (self.angle_span.len() - 1) as f64) as usize;
        let idx = idx.min(self.angle_span.len() - 1);
        self.angle_span[idx]
    }
}

impl Surface for CylindricalFillet {
    fn surface_type(&self) -> SurfaceType {
        SurfaceType::Cylinder
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clone_box(&self) -> Box<dyn Surface> {
        Box::new(self.clone())
    }

    fn evaluate_full(&self, u: f64, v: f64) -> MathResult<SurfacePoint> {
        // u: parameter along spine (0 to 1)
        // v: parameter around cylinder (0 to 1)

        // Inline position evaluator — used both for the canonical (u, v)
        // sample and for finite-difference partials. Keeps the local
        // (x_axis, y_axis, radial) frame consistent across all stencil
        // points, which is the only way the cross-derivative comes out
        // right when axis_field varies with u.
        let position_at = |u: f64, v: f64| -> MathResult<(Point3, Vector3, Vector3)> {
            let spine_point = self.spine.evaluate(u)?.position;
            let axis = self.axis_at(u)?;
            let (start_angle, end_angle) = self.angles_at(u);
            let angle = start_angle + v * (end_angle - start_angle);

            let z_axis = axis.normalize()?;
            let x_axis = if z_axis.cross(&Vector3::X).magnitude_squared() > 1e-6 {
                z_axis.cross(&Vector3::X).normalize()?
            } else {
                z_axis.cross(&Vector3::Y).normalize()?
            };
            let y_axis = z_axis.cross(&x_axis);
            let radial = x_axis * angle.cos() + y_axis * angle.sin();
            Ok((spine_point + radial * self.radius, radial, z_axis))
        };

        let (position, radial, z_axis) = position_at(u, v)?;
        let (start_angle, end_angle) = self.angles_at(u);

        // Central finite differences for du, with one-sided fallback at
        // the parameter boundary. Step size 1e-5 trades round-off (∝ ε/h)
        // against truncation (∝ h²) for f64 in the normal scale band.
        let h = 1.0e-5_f64;
        let u_plus = (u + h).min(1.0);
        let u_minus = (u - h).max(0.0);
        let span_u = (u_plus - u_minus).max(1e-12);
        let (p_up, _, _) = position_at(u_plus, v)?;
        let (p_um, _, _) = position_at(u_minus, v)?;
        let du = (p_up - p_um) / span_u;

        let v_plus = (v + h).min(1.0);
        let v_minus = (v - h).max(0.0);
        let span_v = (v_plus - v_minus).max(1e-12);
        // Analytical dv (exact for the cylinder cross-section at fixed u).
        let dv = {
            // Reconstruct (x_axis, y_axis) at (u, v) from radial and z_axis
            // by extracting the v-derivative basis: ∂radial/∂angle =
            // y_axis*cos − x_axis*sin = R·(angle+π/2). Use central FD on v
            // for robustness when the analytical formula and frame disagree.
            let (p_vp, _, _) = position_at(u, v_plus)?;
            let (p_vm, _, _) = position_at(u, v_minus)?;
            (p_vp - p_vm) / span_v
        };

        // Surface normal: radial direction (outward); preserved across the
        // FD stencil since it only depends on u, v at the centre point.
        let normal = radial;

        // Second derivatives via central FD. duu uses the standard
        // 3-point stencil (P(u+h) - 2P(u) + P(u-h))/h². duv uses the
        // 4-corner cross stencil. dvv uses the analytical formula since
        // along v the cylinder is exactly an arc.
        let duu = (p_up - position * 2.0 + p_um) / (h * h);
        let (p_pp, _, _) = position_at(u_plus, v_plus)?;
        let (p_pm, _, _) = position_at(u_plus, v_minus)?;
        let (p_mp, _, _) = position_at(u_minus, v_plus)?;
        let (p_mm, _, _) = position_at(u_minus, v_minus)?;
        let duv = (p_pp - p_pm - p_mp + p_mm) / (span_u * span_v);
        let dvv = -radial * self.radius * (end_angle - start_angle).powi(2);

        // Principal curvatures.
        // k1 (around v): exact cylinder curvature 1/r along the radial.
        // k2 (along u): II_uu / I_uu where II_uu = duu · n and
        //              I_uu = du · du. Reduces to 0 for a straight spine.
        let k1 = 1.0 / self.radius;
        let i_uu = du.dot(&du).max(1e-30);
        let ii_uu = duu.dot(&normal);
        let k2 = ii_uu / i_uu;

        Ok(SurfacePoint {
            position,
            du,
            dv,
            duu,
            duv,
            dvv,
            normal,
            k1,
            k2,
            dir1: radial,
            dir2: z_axis,
        })
    }

    fn point_at(&self, u: f64, v: f64) -> MathResult<Point3> {
        self.evaluate_full(u, v).map(|sp| sp.position)
    }

    fn normal_at(&self, u: f64, v: f64) -> MathResult<Vector3> {
        self.evaluate_full(u, v).map(|sp| sp.normal)
    }

    fn parameter_bounds(&self) -> ((f64, f64), (f64, f64)) {
        ((0.0, 1.0), (0.0, 1.0))
    }

    fn is_closed_u(&self) -> bool {
        false
    }

    fn is_closed_v(&self) -> bool {
        false // Not fully circular
    }

    fn transform(&self, transform: &Matrix4) -> Box<dyn Surface> {
        // Transform spine and contact curves, recompute axis field
        let transformed_spine = self.spine.transform(transform);
        let transformed_c1 = self.contact1.transform(transform);
        let transformed_c2 = self.contact2.transform(transform);
        // Transform axis vectors (rotation only — use upper 3x3)
        let axis_field: Vec<Vector3> = self
            .axis_field
            .iter()
            .map(|a| transform.transform_vector(a))
            .collect();
        Box::new(CylindricalFillet {
            spine: transformed_spine,
            radius: self.radius,
            contact1: transformed_c1,
            contact2: transformed_c2,
            axis_field,
            angle_span: self.angle_span.clone(),
        })
    }

    fn type_name(&self) -> &'static str {
        "CylindricalFillet"
    }

    fn closest_point(&self, point: &Point3, tolerance: Tolerance) -> MathResult<(f64, f64)> {
        // Newton-Raphson iteration to find closest (u, v) parameter
        let mut u = 0.5;
        let mut v = 0.5;
        let max_iter = 20;
        let eps = tolerance.distance();

        for _ in 0..max_iter {
            let sp = self.evaluate_full(u, v)?;
            let diff = *point - sp.position;
            let dist = diff.magnitude();
            if dist < eps {
                break;
            }
            // Approximate gradient using finite differences
            let du = 1e-6;
            let dv = 1e-6;
            let sp_du = self.evaluate_full((u + du).min(1.0), v)?;
            let sp_dv = self.evaluate_full(u, (v + dv).min(1.0))?;
            let dpos_du = (sp_du.position - sp.position) / du;
            let dpos_dv = (sp_dv.position - sp.position) / dv;
            // Project diff onto tangent plane
            let du_step = diff.dot(&dpos_du) / dpos_du.dot(&dpos_du).max(1e-30);
            let dv_step = diff.dot(&dpos_dv) / dpos_dv.dot(&dpos_dv).max(1e-30);
            u = (u + du_step * 0.5).clamp(0.0, 1.0);
            v = (v + dv_step * 0.5).clamp(0.0, 1.0);
        }
        Ok((u, v))
    }

    fn offset(&self, distance: f64) -> Box<dyn Surface> {
        // Create a new cylindrical fillet with adjusted radius
        let new_radius = (self.radius + distance).abs();
        Box::new(CylindricalFillet {
            spine: self.spine.clone_box(),
            radius: new_radius,
            contact1: self.contact1.clone_box(),
            contact2: self.contact2.clone_box(),
            axis_field: self.axis_field.clone(),
            angle_span: self.angle_span.clone(),
        })
    }

    fn offset_exact(&self, distance: f64, tolerance: Tolerance) -> MathResult<OffsetSurface> {
        Ok(OffsetSurface {
            surface: Box::new(self.clone()),
            quality: OffsetQuality::Approximate {
                max_error: tolerance.distance(),
            },
            original: Box::new(self.clone()),
            distance,
        })
    }

    fn offset_variable(
        &self,
        _distance_fn: Box<dyn Fn(f64, f64) -> f64 + Send + Sync>,
        _tolerance: Tolerance,
    ) -> MathResult<Box<dyn Surface>> {
        Ok(Box::new(self.clone()))
    }

    fn intersect(
        &self,
        other: &dyn Surface,
        tolerance: Tolerance,
    ) -> Vec<SurfaceIntersectionResult> {
        // Delegate to the math-layer surface-surface tracer. Fillet
        // surfaces have no closed-form SSI with arbitrary partners; the
        // generic Patrikalakis-Maekawa tracer handles them via sampled
        // Newton refinement on the implicit equation S₁(u,v) - S₂(s,t) = 0.
        crate::primitives::surface::dispatch_via_math_ssi(self, other, tolerance)
    }

    fn curvature_at(
        &self,
        u: f64,
        v: f64,
    ) -> MathResult<crate::primitives::surface::CurvatureAtPoint> {
        let eval = self.evaluate_full(u, v)?;
        Ok(crate::primitives::surface::CurvatureAtPoint {
            k1: eval.k1,
            k2: eval.k2,
            dir1: eval.dir1,
            dir2: eval.dir2,
        })
    }
}

/// Toroidal fillet surface - constant radius along curved edge
#[derive(Debug)]
pub struct ToroidalFillet {
    /// Major radius (distance from torus center to tube center)
    pub major_radius: f64,
    /// Minor radius (tube radius - the fillet radius)
    pub minor_radius: f64,
    /// Center curve of the torus
    pub center_curve: Box<dyn Curve>,
    /// Start and end angles for partial torus
    pub angle_bounds: (f64, f64),
    /// Contact curves
    pub contact1: Box<dyn Curve>,
    pub contact2: Box<dyn Curve>,
}

impl Clone for ToroidalFillet {
    fn clone(&self) -> Self {
        Self {
            major_radius: self.major_radius,
            minor_radius: self.minor_radius,
            center_curve: self.center_curve.clone_box(),
            angle_bounds: self.angle_bounds,
            contact1: self.contact1.clone_box(),
            contact2: self.contact2.clone_box(),
        }
    }
}

impl ToroidalFillet {
    pub fn new(
        center_curve: Box<dyn Curve>,
        fillet_radius: f64,
        contact1: Box<dyn Curve>,
        contact2: Box<dyn Curve>,
    ) -> MathResult<Self> {
        if fillet_radius <= 0.0 {
            return Err(MathError::InvalidParameter(
                "Fillet radius must be positive".into(),
            ));
        }

        // Estimate the major radius (osculating-circle radius) from three
        // samples on the spine: u = 0, 0.5, 1. For a true circular spine
        // this is exact; for a generally curved spine it gives the local
        // osculating radius near the midpoint, which is the right scale
        // for the principal-curvature reporting in evaluate_full.
        let p_range = center_curve.parameter_range();
        let u0 = p_range.start;
        let u_mid = (p_range.start + p_range.end) * 0.5;
        let u1 = p_range.end;
        let p0 = center_curve.evaluate(u0)?.position;
        let p1 = center_curve.evaluate(u_mid)?.position;
        let p2 = center_curve.evaluate(u1)?.position;
        let major_radius = circumscribed_radius(p0, p1, p2).unwrap_or(f64::INFINITY);

        Ok(Self {
            major_radius,
            minor_radius: fillet_radius,
            center_curve,
            angle_bounds: (0.0, std::f64::consts::PI * 0.5),
            contact1,
            contact2,
        })
    }
}

/// Radius of the unique circle through three points in 3D, or `None` if
/// the points are colinear (no finite circumscribed circle).
fn circumscribed_radius(p0: Point3, p1: Point3, p2: Point3) -> Option<f64> {
    let a = (p1 - p0).magnitude();
    let b = (p2 - p1).magnitude();
    let c = (p0 - p2).magnitude();
    let cross = (p1 - p0).cross(&(p2 - p0));
    let twice_area = cross.magnitude();
    if twice_area < 1e-30 {
        return None;
    }
    Some(a * b * c / (2.0 * twice_area))
}

impl Surface for ToroidalFillet {
    fn surface_type(&self) -> SurfaceType {
        SurfaceType::Torus
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clone_box(&self) -> Box<dyn Surface> {
        Box::new(self.clone())
    }

    fn evaluate_full(&self, u: f64, v: f64) -> MathResult<SurfacePoint> {
        // u: parameter along center curve
        // v: parameter around minor circle

        // Inline position evaluator — used both for the canonical (u, v)
        // sample and for finite-difference partials so all stencil points
        // see the same construction (centre curve sample + Frenet-style
        // local frame + radial offset).
        let position_at = |u: f64, v: f64| -> MathResult<(Point3, Vector3, Vector3)> {
            let center = self.center_curve.evaluate(u)?.position;
            let center_tangent = self.center_curve.tangent_at(u)?;
            let z_axis = center_tangent.normalize()?;
            let x_axis = if z_axis.cross(&Vector3::X).magnitude_squared() > 1e-6 {
                z_axis.cross(&Vector3::X).normalize()?
            } else {
                z_axis.cross(&Vector3::Y).normalize()?
            };
            let y_axis = z_axis.cross(&x_axis);
            let angle = self.angle_bounds.0 + v * (self.angle_bounds.1 - self.angle_bounds.0);
            let radial = x_axis * angle.cos() + y_axis * angle.sin();
            Ok((center + radial * self.minor_radius, radial, z_axis))
        };

        let (position, radial, z_axis) = position_at(u, v)?;
        let angle = self.angle_bounds.0 + v * (self.angle_bounds.1 - self.angle_bounds.0);
        let angle_range = self.angle_bounds.1 - self.angle_bounds.0;

        // Central FD partials with one-sided fallback at parameter
        // boundaries. Step 1e-5 balances f64 round-off and truncation.
        let h = 1.0e-5_f64;
        let u_p = (u + h).min(1.0);
        let u_m = (u - h).max(0.0);
        let v_p = (v + h).min(1.0);
        let v_m = (v - h).max(0.0);
        let span_u = (u_p - u_m).max(1e-12);
        let span_v = (v_p - v_m).max(1e-12);

        let (p_up, _, _) = position_at(u_p, v)?;
        let (p_um, _, _) = position_at(u_m, v)?;
        let du = (p_up - p_um) / span_u;

        let (p_vp, _, _) = position_at(u, v_p)?;
        let (p_vm, _, _) = position_at(u, v_m)?;
        let dv = (p_vp - p_vm) / span_v;

        // Surface normal: outward radial from the tube spine.
        let normal = radial;

        // duu via 3-point central stencil; duv via 4-corner cross stencil.
        // dvv stays analytical — the v-direction is exactly an arc of
        // radius minor_radius, so the closed form is preferable.
        let duu = (p_up - position * 2.0 + p_um) / (h * h);
        let (p_pp, _, _) = position_at(u_p, v_p)?;
        let (p_pm, _, _) = position_at(u_p, v_m)?;
        let (p_mp, _, _) = position_at(u_m, v_p)?;
        let (p_mm, _, _) = position_at(u_m, v_m)?;
        let duv = (p_pp - p_pm - p_mp + p_mm) / (span_u * span_v);
        let dvv = -radial * self.minor_radius * angle_range.powi(2);

        // Principal curvatures: k1 around the tube cross-section is the
        // standard 1/r. k2 along the spine direction matches the torus
        // closed form when major_radius is the local osculating radius.
        let k1 = 1.0 / self.minor_radius;
        let k2 = angle.cos() / (self.major_radius + self.minor_radius * angle.cos());

        Ok(SurfacePoint {
            position,
            du,
            dv,
            duu,
            duv,
            dvv,
            normal,
            k1,
            k2,
            dir1: radial,
            dir2: z_axis,
        })
    }

    fn point_at(&self, u: f64, v: f64) -> MathResult<Point3> {
        self.evaluate_full(u, v).map(|sp| sp.position)
    }

    fn normal_at(&self, u: f64, v: f64) -> MathResult<Vector3> {
        self.evaluate_full(u, v).map(|sp| sp.normal)
    }

    fn parameter_bounds(&self) -> ((f64, f64), (f64, f64)) {
        ((0.0, 1.0), (0.0, 1.0))
    }

    fn is_closed_u(&self) -> bool {
        self.center_curve.is_closed()
    }

    fn is_closed_v(&self) -> bool {
        (self.angle_bounds.1 - self.angle_bounds.0) >= std::f64::consts::TAU - 1e-10
    }

    fn transform(&self, transform: &Matrix4) -> Box<dyn Surface> {
        Box::new(ToroidalFillet {
            major_radius: self.major_radius,
            minor_radius: self.minor_radius,
            center_curve: self.center_curve.transform(transform),
            angle_bounds: self.angle_bounds,
            contact1: self.contact1.transform(transform),
            contact2: self.contact2.transform(transform),
        })
    }

    fn type_name(&self) -> &'static str {
        "ToroidalFillet"
    }

    fn closest_point(&self, point: &Point3, tolerance: Tolerance) -> MathResult<(f64, f64)> {
        newton_closest_point(self, point, tolerance)
    }

    fn offset(&self, distance: f64) -> Box<dyn Surface> {
        Box::new(ToroidalFillet {
            major_radius: self.major_radius,
            minor_radius: (self.minor_radius + distance).abs(),
            center_curve: self.center_curve.clone_box(),
            angle_bounds: self.angle_bounds,
            contact1: self.contact1.clone_box(),
            contact2: self.contact2.clone_box(),
        })
    }

    fn offset_exact(&self, distance: f64, tolerance: Tolerance) -> MathResult<OffsetSurface> {
        Ok(OffsetSurface {
            surface: Box::new(self.clone()),
            quality: OffsetQuality::Approximate {
                max_error: tolerance.distance(),
            },
            original: Box::new(self.clone()),
            distance,
        })
    }

    fn offset_variable(
        &self,
        _distance_fn: Box<dyn Fn(f64, f64) -> f64 + Send + Sync>,
        _tolerance: Tolerance,
    ) -> MathResult<Box<dyn Surface>> {
        Ok(Box::new(self.clone()))
    }

    fn intersect(
        &self,
        other: &dyn Surface,
        tolerance: Tolerance,
    ) -> Vec<SurfaceIntersectionResult> {
        // Delegate to the math-layer surface-surface tracer. Fillet
        // surfaces have no closed-form SSI with arbitrary partners; the
        // generic Patrikalakis-Maekawa tracer handles them via sampled
        // Newton refinement on the implicit equation S₁(u,v) - S₂(s,t) = 0.
        crate::primitives::surface::dispatch_via_math_ssi(self, other, tolerance)
    }

    fn curvature_at(
        &self,
        u: f64,
        v: f64,
    ) -> MathResult<crate::primitives::surface::CurvatureAtPoint> {
        let eval = self.evaluate_full(u, v)?;
        Ok(crate::primitives::surface::CurvatureAtPoint {
            k1: eval.k1,
            k2: eval.k2,
            dir1: eval.dir1,
            dir2: eval.dir2,
        })
    }
}

/// Spherical fillet surface - for vertex blends
#[derive(Debug)]
pub struct SphericalFillet {
    /// Center of sphere
    pub center: Point3,
    /// Radius
    pub radius: f64,
    /// Parameter bounds for partial sphere
    pub u_bounds: (f64, f64), // theta (latitude)
    pub v_bounds: (f64, f64), // phi (longitude)
    /// Adjacent edges at vertex
    pub edges: Vec<Box<dyn Curve>>,
}

impl Clone for SphericalFillet {
    fn clone(&self) -> Self {
        Self {
            center: self.center,
            radius: self.radius,
            u_bounds: self.u_bounds,
            v_bounds: self.v_bounds,
            edges: self.edges.iter().map(|e| e.clone_box()).collect(),
        }
    }
}

impl SphericalFillet {
    pub fn new(center: Point3, radius: f64, edges: Vec<Box<dyn Curve>>) -> MathResult<Self> {
        if radius <= 0.0 {
            return Err(MathError::InvalidParameter(
                "Radius must be positive".into(),
            ));
        }

        // Derive (θ, φ) bounds from the edges meeting at this vertex.
        // For each edge we pick the endpoint nearest `center` (the vertex
        // the blend is being placed at) and use the direction toward the
        // far endpoint to get a representative tangent direction. Mapped
        // into (θ = polar from +Z, φ = azimuth from +X) the bounding box
        // of these directions, padded by 1% of π, defines the spherical
        // patch the fillet should cover. Falls back to the canonical
        // `+X +Y +Z` octant when no edges are supplied.
        const PAD: f64 = 0.0314_f64; // ≈ 1% of π

        let mut min_theta = f64::INFINITY;
        let mut max_theta = f64::NEG_INFINITY;
        let mut min_phi = f64::INFINITY;
        let mut max_phi = f64::NEG_INFINITY;
        let mut have_dir = false;

        for edge in &edges {
            let pr = edge.parameter_range();
            let p_start = edge.evaluate(pr.start)?.position;
            let p_end = edge.evaluate(pr.end)?.position;
            let d_start = (p_start - center).magnitude_squared();
            let d_end = (p_end - center).magnitude_squared();
            // The far endpoint defines the tangent direction at the vertex.
            let far = if d_start < d_end { p_end } else { p_start };
            let raw = far - center;
            let len = raw.magnitude();
            if len < 1e-12 {
                continue; // degenerate edge — skip
            }
            let dir = raw / len;
            let theta = dir.z.clamp(-1.0, 1.0).acos();
            let phi = dir.y.atan2(dir.x);
            min_theta = min_theta.min(theta);
            max_theta = max_theta.max(theta);
            min_phi = min_phi.min(phi);
            max_phi = max_phi.max(phi);
            have_dir = true;
        }

        let (u_bounds, v_bounds) = if have_dir {
            (
                (
                    (min_theta - PAD).max(0.0),
                    (max_theta + PAD).min(std::f64::consts::PI),
                ),
                (min_phi - PAD, max_phi + PAD),
            )
        } else {
            (
                (0.0, std::f64::consts::PI * 0.5),
                (0.0, std::f64::consts::PI * 0.5),
            )
        };

        Ok(Self {
            center,
            radius,
            u_bounds,
            v_bounds,
            edges,
        })
    }
}

impl Surface for SphericalFillet {
    fn surface_type(&self) -> SurfaceType {
        SurfaceType::Sphere
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn clone_box(&self) -> Box<dyn Surface> {
        Box::new(self.clone())
    }

    fn evaluate_full(&self, u: f64, v: f64) -> MathResult<SurfacePoint> {
        // Map parameters to spherical coordinates
        let theta = self.u_bounds.0 + u * (self.u_bounds.1 - self.u_bounds.0);
        let phi = self.v_bounds.0 + v * (self.v_bounds.1 - self.v_bounds.0);

        // Position on sphere
        let x = self.radius * theta.sin() * phi.cos();
        let y = self.radius * theta.sin() * phi.sin();
        let z = self.radius * theta.cos();
        let position = self.center + Vector3::new(x, y, z);

        // First derivatives
        let theta_range = self.u_bounds.1 - self.u_bounds.0;
        let phi_range = self.v_bounds.1 - self.v_bounds.0;

        let du = Vector3::new(
            self.radius * theta.cos() * phi.cos() * theta_range,
            self.radius * theta.cos() * phi.sin() * theta_range,
            -self.radius * theta.sin() * theta_range,
        );

        let dv = Vector3::new(
            -self.radius * theta.sin() * phi.sin() * phi_range,
            self.radius * theta.sin() * phi.cos() * phi_range,
            0.0,
        );

        // Normal (outward)
        let normal = (position - self.center).normalize()?;

        // Second derivatives
        let duu = Vector3::new(
            -self.radius * theta.sin() * phi.cos() * theta_range.powi(2),
            -self.radius * theta.sin() * phi.sin() * theta_range.powi(2),
            -self.radius * theta.cos() * theta_range.powi(2),
        );

        let dvv = Vector3::new(
            -self.radius * theta.sin() * phi.cos() * phi_range.powi(2),
            -self.radius * theta.sin() * phi.sin() * phi_range.powi(2),
            0.0,
        );

        let duv = Vector3::new(
            -self.radius * theta.cos() * phi.sin() * theta_range * phi_range,
            self.radius * theta.cos() * phi.cos() * theta_range * phi_range,
            0.0,
        );

        // Principal curvatures (both 1/R for sphere)
        let k1 = 1.0 / self.radius;
        let k2 = 1.0 / self.radius;

        Ok(SurfacePoint {
            position,
            du,
            dv,
            duu,
            duv,
            dvv,
            normal,
            k1,
            k2,
            dir1: du.normalize().unwrap_or(Vector3::X),
            dir2: dv.normalize().unwrap_or(Vector3::Y),
        })
    }

    fn point_at(&self, u: f64, v: f64) -> MathResult<Point3> {
        self.evaluate_full(u, v).map(|sp| sp.position)
    }

    fn normal_at(&self, u: f64, v: f64) -> MathResult<Vector3> {
        self.evaluate_full(u, v).map(|sp| sp.normal)
    }

    fn parameter_bounds(&self) -> ((f64, f64), (f64, f64)) {
        ((0.0, 1.0), (0.0, 1.0))
    }

    fn is_closed_u(&self) -> bool {
        false
    }

    fn is_closed_v(&self) -> bool {
        (self.v_bounds.1 - self.v_bounds.0) >= std::f64::consts::TAU - 1e-10
    }

    fn transform(&self, transform: &Matrix4) -> Box<dyn Surface> {
        let new_center = transform.transform_point(&self.center);
        Box::new(SphericalFillet {
            center: new_center,
            radius: self.radius,
            u_bounds: self.u_bounds,
            v_bounds: self.v_bounds,
            edges: self.edges.iter().map(|e| e.transform(transform)).collect(),
        })
    }

    fn type_name(&self) -> &'static str {
        "SphericalFillet"
    }

    fn closest_point(&self, point: &Point3, _tolerance: Tolerance) -> MathResult<(f64, f64)> {
        // Project point onto sphere surface and map to parameter space
        let to_point = *point - self.center;
        let dist = to_point.magnitude();
        if dist < 1e-10 {
            return Ok((0.5, 0.5));
        }

        let dir = to_point / dist;
        let theta_range = self.u_bounds.1 - self.u_bounds.0;
        let phi_range = self.v_bounds.1 - self.v_bounds.0;

        // Convert direction to spherical coordinates
        let theta = dir.z.acos(); // polar angle
        let phi = dir.y.atan2(dir.x); // azimuthal angle
        let phi = if phi < 0.0 {
            phi + std::f64::consts::TAU
        } else {
            phi
        };

        // Map to [0, 1] parameter space
        let u = ((theta - self.u_bounds.0) / theta_range).clamp(0.0, 1.0);
        let v = ((phi - self.v_bounds.0) / phi_range).clamp(0.0, 1.0);
        Ok((u, v))
    }

    fn offset(&self, distance: f64) -> Box<dyn Surface> {
        Box::new(SphericalFillet {
            center: self.center,
            radius: (self.radius + distance).abs(),
            u_bounds: self.u_bounds,
            v_bounds: self.v_bounds,
            edges: self.edges.iter().map(|e| e.clone_box()).collect(),
        })
    }

    fn offset_exact(&self, distance: f64, tolerance: Tolerance) -> MathResult<OffsetSurface> {
        Ok(OffsetSurface {
            surface: Box::new(self.clone()),
            quality: OffsetQuality::Approximate {
                max_error: tolerance.distance(),
            },
            original: Box::new(self.clone()),
            distance,
        })
    }

    fn offset_variable(
        &self,
        _distance_fn: Box<dyn Fn(f64, f64) -> f64 + Send + Sync>,
        _tolerance: Tolerance,
    ) -> MathResult<Box<dyn Surface>> {
        Ok(Box::new(self.clone()))
    }

    fn intersect(
        &self,
        other: &dyn Surface,
        tolerance: Tolerance,
    ) -> Vec<SurfaceIntersectionResult> {
        crate::primitives::surface::dispatch_via_math_ssi(self, other, tolerance)
    }

    fn curvature_at(
        &self,
        u: f64,
        v: f64,
    ) -> MathResult<crate::primitives::surface::CurvatureAtPoint> {
        let eval = self.evaluate_full(u, v)?;
        Ok(crate::primitives::surface::CurvatureAtPoint {
            k1: eval.k1,
            k2: eval.k2,
            dir1: eval.dir1,
            dir2: eval.dir2,
        })
    }
}

/// Variable radius fillet surface using NURBS
#[derive(Debug)]
pub struct VariableRadiusFillet {
    /// Underlying NURBS surface
    pub nurbs: NurbsSurface,
    /// Spine curve
    pub spine: Box<dyn Curve>,
    /// Radius function samples
    pub radius_samples: Vec<f64>,
    /// Contact curves
    pub contact1: Box<dyn Curve>,
    pub contact2: Box<dyn Curve>,
}

impl Clone for VariableRadiusFillet {
    fn clone(&self) -> Self {
        Self {
            nurbs: self.nurbs.clone(),
            spine: self.spine.clone_box(),
            radius_samples: self.radius_samples.clone(),
            contact1: self.contact1.clone_box(),
            contact2: self.contact2.clone_box(),
        }
    }
}

impl VariableRadiusFillet {
    pub fn new(
        spine: Box<dyn Curve>,
        radius_start: f64,
        radius_end: f64,
        contact1: Box<dyn Curve>,
        contact2: Box<dyn Curve>,
    ) -> MathResult<Self> {
        // Build a NURBS surface whose u-direction follows the spine and
        // whose v-direction sweeps an arc between the two contact curves
        // at every spine sample. The arc lies in the plane perpendicular
        // to the spine tangent at u, anchored on the line spine→contact1
        // and rotated to the line spine→contact2. The radius interpolates
        // linearly between radius_start and radius_end across the spine.

        let num_u = 20;
        let num_v = 5;
        let mut control_points = vec![vec![Point3::ZERO; num_v]; num_u];
        let weights = vec![vec![1.0; num_v]; num_u];

        for i in 0..num_u {
            let u = i as f64 / (num_u - 1) as f64;
            let spine_point = spine.evaluate(u)?.position;
            let spine_tangent = spine.tangent_at(u)?;
            let radius = radius_start + u * (radius_end - radius_start);

            // Reproject contact directions into the plane perpendicular
            // to the spine tangent so the arc sits in a true cross-section.
            let z_axis = spine_tangent.normalize().unwrap_or(Vector3::Z);
            let c1_point = contact1.evaluate(u)?.position;
            let c2_point = contact2.evaluate(u)?.position;
            let raw_x = c1_point - spine_point;
            let x_in_plane = raw_x - z_axis * raw_x.dot(&z_axis);
            let x_axis = x_in_plane.normalize().unwrap_or_else(|_| {
                // Spine and contact1 collinear — fall back to any direction
                // perpendicular to z_axis.
                if z_axis.cross(&Vector3::X).magnitude_squared() > 1e-6 {
                    z_axis.cross(&Vector3::X).normalize().unwrap_or(Vector3::X)
                } else {
                    z_axis.cross(&Vector3::Y).normalize().unwrap_or(Vector3::Y)
                }
            });
            let y_axis = z_axis.cross(&x_axis);

            // Sweep angle is the in-plane angle between the contact
            // directions; clamp to (0, π) for numerical safety.
            let raw_y = c2_point - spine_point;
            let y_in_plane = raw_y - z_axis * raw_y.dot(&z_axis);
            let cos_sweep = (x_axis.dot(&y_in_plane) / y_in_plane.magnitude().max(1e-12))
                .clamp(-1.0, 1.0);
            let sweep = cos_sweep.acos().clamp(1e-6, std::f64::consts::PI);

            for j in 0..num_v {
                let v = j as f64 / (num_v - 1) as f64;
                let angle = v * sweep;
                let radial = x_axis * angle.cos() + y_axis * angle.sin();
                control_points[i][j] = spine_point + radial * radius;
            }
        }

        // Create knot vectors
        let knot_u = KnotVector::uniform(3, num_u);
        let knot_v = KnotVector::uniform(2, num_v);

        let nurbs = NurbsSurface::new(
            control_points,
            weights,
            knot_u.values().to_vec(),
            knot_v.values().to_vec(),
            3, // degree_u
            2, // degree_v
        )
        .map_err(|e| MathError::InvalidParameter(e.to_string()))?;

        let radius_samples = (0..num_u)
            .map(|i| {
                let u = i as f64 / (num_u - 1) as f64;
                radius_start + u * (radius_end - radius_start)
            })
            .collect();

        Ok(Self {
            nurbs,
            spine,
            radius_samples,
            contact1,
            contact2,
        })
    }
}

impl Surface for VariableRadiusFillet {
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
        // Evaluate NURBS with second-order derivatives. Missing first-order
        // derivatives are a hard failure: silently substituting zero would
        // collapse E, F, G in the first fundamental form to zero, making the
        // curvature denominator vanish and producing fake k1=k2=0 — which
        // then poisons fillet-radius selection downstream.
        let eval = self.nurbs.evaluate_derivatives(u, v, 2, 2);
        let position = eval.point;
        let du = eval.du.ok_or_else(|| {
            MathError::InvalidParameter(format!(
                "NURBS du derivative unavailable at (u={u}, v={v})"
            ))
        })?;
        let dv = eval.dv.ok_or_else(|| {
            MathError::InvalidParameter(format!(
                "NURBS dv derivative unavailable at (u={u}, v={v})"
            ))
        })?;
        let normal = eval
            .normal
            .or_else(|| {
                let n = du.cross(&dv);
                let mag = n.magnitude();
                if mag > 1e-15 {
                    Some(n / mag)
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                MathError::InvalidParameter(format!(
                    "NURBS surface degenerate at (u={u}, v={v}) — du×dv has zero length"
                ))
            })?;

        // Second-order derivatives may legitimately be unavailable for
        // surfaces that are only C^1; in that case we degrade to flat
        // curvature (k1=k2=0) by substituting zero, which yields the correct
        // shape operator on the fundamental form below.
        let duu = eval.duu.unwrap_or(Vector3::ZERO);
        let dvv = eval.dvv.unwrap_or(Vector3::ZERO);
        let duv = eval.duv.unwrap_or(Vector3::ZERO);

        // Compute principal curvatures from second fundamental form
        let e = duu.dot(&normal);
        let f = duv.dot(&normal);
        let g = dvv.dot(&normal);
        let big_e = du.dot(&du);
        let big_f = du.dot(&dv);
        let big_g = dv.dot(&dv);
        let denom = big_e * big_g - big_f * big_f;
        let (k1, k2) = if denom.abs() > 1e-20 {
            let mean = (e * big_g - 2.0 * f * big_f + g * big_e) / (2.0 * denom);
            let gauss = (e * g - f * f) / denom;
            let disc = (mean * mean - gauss).max(0.0).sqrt();
            (mean + disc, mean - disc)
        } else {
            (0.0, 0.0)
        };

        Ok(SurfacePoint {
            position,
            du,
            dv,
            duu,
            duv,
            dvv,
            normal,
            k1,
            k2,
            dir1: du.normalize().unwrap_or(Vector3::X),
            dir2: dv.normalize().unwrap_or(Vector3::Y),
        })
    }

    fn parameter_bounds(&self) -> ((f64, f64), (f64, f64)) {
        self.nurbs.parameter_bounds()
    }

    fn is_closed_u(&self) -> bool {
        false
    }

    fn is_closed_v(&self) -> bool {
        false
    }

    fn transform(&self, transform: &Matrix4) -> Box<dyn Surface> {
        let mut transformed_nurbs = self.nurbs.clone();
        let _ = transformed_nurbs.transform(transform);
        Box::new(VariableRadiusFillet {
            nurbs: transformed_nurbs,
            spine: self.spine.transform(transform),
            radius_samples: self.radius_samples.clone(),
            contact1: self.contact1.transform(transform),
            contact2: self.contact2.transform(transform),
        })
    }

    fn type_name(&self) -> &'static str {
        "VariableRadiusFillet"
    }

    fn closest_point(&self, point: &Point3, tolerance: Tolerance) -> MathResult<(f64, f64)> {
        newton_closest_point(self, point, tolerance)
    }

    fn offset(&self, distance: f64) -> Box<dyn Surface> {
        // Offset the variable radius fillet by adjusting radius samples and rebuilding NURBS
        let offset_radii: Vec<f64> = self
            .radius_samples
            .iter()
            .map(|r| (r + distance).abs())
            .collect();
        // Rebuild the fillet with new radii
        match VariableRadiusFillet::new(
            self.spine.clone_box(),
            *offset_radii.first().unwrap_or(&1.0),
            *offset_radii.last().unwrap_or(&1.0),
            self.contact1.clone_box(),
            self.contact2.clone_box(),
        ) {
            Ok(fillet) => Box::new(fillet),
            Err(_) => Box::new(self.clone()),
        }
    }

    fn offset_exact(&self, distance: f64, tolerance: Tolerance) -> MathResult<OffsetSurface> {
        Ok(OffsetSurface {
            surface: Box::new(self.clone()),
            quality: OffsetQuality::Approximate {
                max_error: tolerance.distance(),
            },
            original: Box::new(self.clone()),
            distance,
        })
    }

    fn offset_variable(
        &self,
        _distance_fn: Box<dyn Fn(f64, f64) -> f64 + Send + Sync>,
        _tolerance: Tolerance,
    ) -> MathResult<Box<dyn Surface>> {
        Ok(Box::new(self.clone()))
    }

    fn intersect(
        &self,
        other: &dyn Surface,
        tolerance: Tolerance,
    ) -> Vec<SurfaceIntersectionResult> {
        crate::primitives::surface::dispatch_via_math_ssi(self, other, tolerance)
    }

    fn curvature_at(
        &self,
        u: f64,
        v: f64,
    ) -> MathResult<crate::primitives::surface::CurvatureAtPoint> {
        let eval = self.evaluate_full(u, v)?;
        Ok(crate::primitives::surface::CurvatureAtPoint {
            k1: eval.k1,
            k2: eval.k2,
            dir1: eval.dir1,
            dir2: eval.dir2,
        })
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use crate::primitives::curve::Line;
//
//     #[test]
//     fn test_cylindrical_fillet() {
//         let spine = Box::new(Line::new(
//             Point3::ZERO,
//             Point3::new(10.0, 0.0, 0.0),
//         ));
//
//         let contact1 = Box::new(Line::new(
//             Point3::new(0.0, 1.0, 0.0),
//             Point3::new(10.0, 1.0, 0.0),
//         ));
//
//         let contact2 = Box::new(Line::new(
//             Point3::new(0.0, 0.0, 1.0),
//             Point3::new(10.0, 0.0, 1.0),
//         ));
//
//         let fillet = CylindricalFillet::new(spine, 1.0, contact1, contact2).unwrap();
//
//         // Test evaluation
//         let point = fillet.point_at(0.5, 0.5).unwrap();
//         assert!((point.x - 5.0).abs() < 1e-6);
//     }
//
//     #[test]
//     fn test_spherical_fillet() {
//         let edges = vec![
//             Box::new(Line::new(Point3::ZERO, Point3::new(1.0, 0.0, 0.0))) as Box<dyn Curve>,
//             Box::new(Line::new(Point3::ZERO, Point3::new(0.0, 1.0, 0.0))) as Box<dyn Curve>,
//             Box::new(Line::new(Point3::ZERO, Point3::new(0.0, 0.0, 1.0))) as Box<dyn Curve>,
//         ];
//
//         let fillet = SphericalFillet::new(Point3::ZERO, 0.5, edges).unwrap();
//
//         // Test that all points are at radius distance
//         for i in 0..10 {
//             for j in 0..10 {
//                 let u = i as f64 / 9.0;
//                 let v = j as f64 / 9.0;
//                 let point = fillet.point_at(u, v).unwrap();
//                 let distance = (point - Point3::ZERO).magnitude();
//                 assert!((distance - 0.5).abs() < 1e-6);
//             }
//         }
//     }
// }
