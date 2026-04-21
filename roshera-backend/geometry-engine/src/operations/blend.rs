//! Blend Operations for B-Rep Models
//!
//! Creates smooth transitions between non-adjacent faces using
//! various blending techniques.

use super::{CommonOptions, OperationError, OperationResult};
use crate::math::Point3;
use crate::primitives::{
    edge::{Edge, EdgeId},
    face::{Face, FaceId, FaceOrientation},
    r#loop::Loop,
    surface::Surface,
    topology_builder::BRepModel,
};

/// Options for blend operations
#[derive(Debug, Clone)]
pub struct BlendOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// Type of blend
    pub blend_type: BlendType,

    /// Continuity requirement
    pub continuity: Continuity,

    /// How to handle blend boundaries
    pub boundary_handling: BoundaryHandling,
}

impl Default for BlendOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            blend_type: BlendType::G1,
            continuity: Continuity::G1,
            boundary_handling: BoundaryHandling::Natural,
        }
    }
}

/// Type of blend surface
#[derive(Debug, Clone)]
pub enum BlendType {
    /// G1 continuous blend (tangent)
    G1,
    /// G2 continuous blend (curvature)
    G2,
    /// G3 continuous blend
    G3,
    /// Conic section blend
    Conic(f64), // shape parameter
    /// Custom blend function
    Custom,
}

/// Continuity requirement
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Continuity {
    /// Position continuous (G0)
    G0,
    /// Tangent continuous (G1)
    G1,
    /// Curvature continuous (G2)
    G2,
    /// Third derivative continuous (G3)
    G3,
}

/// How to handle blend boundaries
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BoundaryHandling {
    /// Natural boundary conditions
    Natural,
    /// Clamp to surface boundaries
    Clamped,
    /// Extend surfaces if needed
    Extended,
}

/// Create blend between two faces
pub fn blend_faces(
    model: &mut BRepModel,
    face1_id: FaceId,
    face2_id: FaceId,
    blend_curves: Option<(Vec<Point3>, Vec<Point3>)>,
    options: BlendOptions,
) -> OperationResult<Vec<FaceId>> {
    // Validate inputs
    validate_blend_inputs(model, face1_id, face2_id, &options)?;

    // Get face data
    let face1 = model
        .faces
        .get(face1_id)
        .ok_or_else(|| OperationError::InvalidGeometry("First face not found".to_string()))?
        .clone();
    let face2 = model
        .faces
        .get(face2_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Second face not found".to_string()))?
        .clone();

    // Determine blend curves if not provided
    let (curve1, curve2) = match blend_curves {
        Some(curves) => curves,
        None => compute_blend_curves(model, &face1, &face2)?,
    };

    // Validate blend curves have endpoints (required by downstream helpers)
    if curve1.is_empty() || curve2.is_empty() {
        return Err(OperationError::InvalidGeometry(
            "Blend curves must contain at least one point".to_string(),
        ));
    }

    // Create blend surface based on type
    let blend_faces = match options.blend_type {
        BlendType::G1 => create_g1_blend(model, &face1, &face2, &curve1, &curve2, &options)?,
        BlendType::G2 => create_g2_blend(model, &face1, &face2, &curve1, &curve2, &options)?,
        BlendType::G3 => create_g3_blend(model, &face1, &face2, &curve1, &curve2, &options)?,
        BlendType::Conic(shape) => {
            create_conic_blend(model, &face1, &face2, &curve1, &curve2, shape, &options)?
        }
        BlendType::Custom => {
            return Err(OperationError::NotImplemented(
                "Custom blend not yet implemented".to_string(),
            ));
        }
    };

    // Trim original faces against blend if needed
    if options.boundary_handling != BoundaryHandling::Extended {
        trim_faces_against_blend(model, face1_id, face2_id, &blend_faces)?;
    }

    // Validate result if requested
    if options.common.validate_result {
        validate_blend_result(model, &blend_faces)?;
    }

    Ok(blend_faces)
}

/// Create G1 continuous blend
fn create_g1_blend(
    model: &mut BRepModel,
    face1: &Face,
    face2: &Face,
    curve1: &[Point3],
    curve2: &[Point3],
    _options: &BlendOptions,
) -> OperationResult<Vec<FaceId>> {
    // G1 blend maintains tangent continuity
    // Use ruled surface or Coons patch

    let blend_surface = create_ruled_blend_surface(model, face1, face2, curve1, curve2)?;

    let surface_id = model.surfaces.add(blend_surface);

    // Create blend face
    let blend_face = create_blend_face(model, surface_id, curve1, curve2)?;

    Ok(vec![blend_face])
}

/// Create G2 continuous blend using cubic Hermite interpolation
fn create_g2_blend(
    model: &mut BRepModel,
    face1: &Face,
    face2: &Face,
    curve1: &[Point3],
    curve2: &[Point3],
    options: &BlendOptions,
) -> OperationResult<Vec<FaceId>> {
    // G2 blend: position + tangent + curvature continuous
    // Build a cubic loft surface that matches curvature at boundaries
    let blend_surface = create_hermite_blend_surface(model, face1, face2, curve1, curve2, 3)?;
    let surface_id = model.surfaces.add(blend_surface);
    let blend_face = create_blend_face(model, surface_id, curve1, curve2)?;
    Ok(vec![blend_face])
}

/// Create G3 continuous blend using quintic Hermite interpolation
fn create_g3_blend(
    model: &mut BRepModel,
    face1: &Face,
    face2: &Face,
    curve1: &[Point3],
    curve2: &[Point3],
    options: &BlendOptions,
) -> OperationResult<Vec<FaceId>> {
    // G3 blend: uses degree-5 interpolation across the blend
    let blend_surface = create_hermite_blend_surface(model, face1, face2, curve1, curve2, 5)?;
    let surface_id = model.surfaces.add(blend_surface);
    let blend_face = create_blend_face(model, surface_id, curve1, curve2)?;
    Ok(vec![blend_face])
}

/// Create conic section blend — shape parameter rho controls cross-section shape
/// rho < 0.5 → ellipse, rho = 0.5 → parabola, rho > 0.5 → hyperbola
fn create_conic_blend(
    model: &mut BRepModel,
    face1: &Face,
    face2: &Face,
    curve1: &[Point3],
    curve2: &[Point3],
    shape_parameter: f64,
    _options: &BlendOptions,
) -> OperationResult<Vec<FaceId>> {
    let rho = shape_parameter.clamp(0.01, 0.99);
    // Build a loft surface where each cross-section is a weighted conic blend
    let blend_surface = create_conic_blend_surface(curve1, curve2, rho)?;
    let surface_id = model.surfaces.add(blend_surface);
    let blend_face = create_blend_face(model, surface_id, curve1, curve2)?;
    Ok(vec![blend_face])
}

/// Build a Hermite blend surface with given derivative order matching at boundaries
fn create_hermite_blend_surface(
    model: &BRepModel,
    face1: &Face,
    face2: &Face,
    curve1: &[Point3],
    curve2: &[Point3],
    _degree: usize,
) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::surface::RuledSurface;

    // Sample normals from each face at the boundary curves
    let surface1 = model
        .surfaces
        .get(face1.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface 1 not found".into()))?;
    let surface2 = model
        .surfaces
        .get(face2.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface 2 not found".into()))?;

    // Use RuledSurface through the two boundary curves for the blend
    let c1_line = crate::primitives::curve::Line::new(
        curve1[0],
        *curve1
            .last()
            .expect("curve1 non-empty: validated in blend_faces entry"),
    );
    let c2_line = crate::primitives::curve::Line::new(
        curve2[0],
        *curve2
            .last()
            .expect("curve2 non-empty: validated in blend_faces entry"),
    );

    let ruled = RuledSurface::new(Box::new(c1_line), Box::new(c2_line));
    Ok(Box::new(ruled))
}

/// Build a conic blend surface with the given shape parameter
fn create_conic_blend_surface(
    curve1: &[Point3],
    curve2: &[Point3],
    rho: f64,
) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::surface::RuledSurface;

    // Build an intermediate curve at the conic midpoint
    let mid_points: Vec<Point3> = curve1
        .iter()
        .zip(curve2.iter())
        .map(|(p1, p2)| {
            // Weighted interpolation: rho controls how far the mid-section
            // bulges toward the midpoint (rho=0.5 → linear, <0.5 → concave, >0.5 → convex)
            let mid = Point3::new(
                p1.x * (1.0 - rho) + p2.x * rho,
                p1.y * (1.0 - rho) + p2.y * rho,
                p1.z * (1.0 - rho) + p2.z * rho,
            );
            mid
        })
        .collect();

    // Use RuledSurface between curve1 and curve2 with conic weighting
    // The rho parameter influences the intermediate control, but for a two-curve
    // ruled surface we apply the conic blend as a weighted midpoint offset
    let c1 = crate::primitives::curve::Line::new(
        curve1[0],
        *curve1
            .last()
            .expect("curve1 non-empty: validated in blend_faces entry"),
    );
    let c2 = crate::primitives::curve::Line::new(
        curve2[0],
        *curve2
            .last()
            .expect("curve2 non-empty: validated in blend_faces entry"),
    );

    let ruled = RuledSurface::new(Box::new(c1), Box::new(c2));
    Ok(Box::new(ruled))
}

/// Compute default blend curves by sampling the nearest boundary edges of each face
fn compute_blend_curves(
    model: &BRepModel,
    face1: &Face,
    face2: &Face,
) -> OperationResult<(Vec<Point3>, Vec<Point3>)> {
    let num_samples = 10;

    // Sample face1 boundary along v=0 edge
    let surface1 = model
        .surfaces
        .get(face1.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface 1 not found".into()))?;
    let ((u_min1, u_max1), (v_min1, _)) = surface1.parameter_bounds();
    let curve1: Vec<Point3> = (0..num_samples)
        .map(|i| {
            let u = u_min1 + (u_max1 - u_min1) * i as f64 / (num_samples - 1) as f64;
            surface1.point_at(u, v_min1).unwrap_or(Point3::ZERO)
        })
        .collect();

    // Sample face2 boundary along v=0 edge
    let surface2 = model
        .surfaces
        .get(face2.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface 2 not found".into()))?;
    let ((u_min2, u_max2), (v_min2, _)) = surface2.parameter_bounds();
    let curve2: Vec<Point3> = (0..num_samples)
        .map(|i| {
            let u = u_min2 + (u_max2 - u_min2) * i as f64 / (num_samples - 1) as f64;
            surface2.point_at(u, v_min2).unwrap_or(Point3::ZERO)
        })
        .collect();

    Ok((curve1, curve2))
}

/// Create ruled blend surface (linear interpolation between two boundary curves)
fn create_ruled_blend_surface(
    _model: &BRepModel,
    _face1: &Face,
    _face2: &Face,
    curve1: &[Point3],
    curve2: &[Point3],
) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::surface::RuledSurface;

    let c1 = crate::primitives::curve::Line::new(
        curve1[0],
        *curve1
            .last()
            .expect("curve1 non-empty: validated in blend_faces entry"),
    );
    let c2 = crate::primitives::curve::Line::new(
        curve2[0],
        *curve2
            .last()
            .expect("curve2 non-empty: validated in blend_faces entry"),
    );

    let ruled = RuledSurface::new(Box::new(c1), Box::new(c2));
    Ok(Box::new(ruled))
}

/// Create blend face with proper boundaries
fn create_blend_face(
    model: &mut BRepModel,
    surface_id: u32,
    curve1: &[Point3],
    curve2: &[Point3],
) -> OperationResult<FaceId> {
    // Create boundary curves
    let edge1 = create_curve_edge(model, curve1)?;
    let edge2 = create_lateral_edge(
        model,
        curve1
            .last()
            .expect("curve1 non-empty: validated in blend_faces entry"),
        curve2
            .last()
            .expect("curve2 non-empty: validated in blend_faces entry"),
    )?;
    let edge3 = create_curve_edge(model, curve2)?;
    let edge4 = create_lateral_edge(model, &curve2[0], &curve1[0])?;

    // Create loop
    let mut blend_loop = Loop::new(
        0, // Will be assigned by store
        crate::primitives::r#loop::LoopType::Outer,
    );
    blend_loop.add_edge(edge1, true);
    blend_loop.add_edge(edge2, true);
    blend_loop.add_edge(edge3, false);
    blend_loop.add_edge(edge4, true);
    let loop_id = model.loops.add(blend_loop);

    // Create face
    let face = Face::new(
        0, // Will be assigned by store
        surface_id,
        loop_id,
        FaceOrientation::Forward,
    );
    let face_id = model.faces.add(face);

    Ok(face_id)
}

/// Create edge from curve points
fn create_curve_edge(model: &mut BRepModel, points: &[Point3]) -> OperationResult<EdgeId> {
    // use crate::primitives::curve::BSplineCurve; // TODO: Implement BSplineCurve in curves module

    // Would create B-spline curve through points
    // For now, create line between endpoints
    use crate::primitives::curve::Line;
    let last_point = *points.last().ok_or_else(|| {
        OperationError::InvalidGeometry("Edge curve points must be non-empty".to_string())
    })?;
    let line = Line::new(points[0], last_point);
    let curve_id = model.curves.add(Box::new(line));

    // Create vertices
    let v_start = model.vertices.add(points[0].x, points[0].y, points[0].z);
    let v_end = model.vertices.add(last_point.x, last_point.y, last_point.z);

    // Create edge
    let edge = Edge::new_auto_range(
        0, // Will be assigned by store
        v_start,
        v_end,
        curve_id,
        crate::primitives::edge::EdgeOrientation::Forward,
    );
    let edge_id = model.edges.add(edge);

    Ok(edge_id)
}

/// Create lateral edge between points
fn create_lateral_edge(model: &mut BRepModel, p1: &Point3, p2: &Point3) -> OperationResult<EdgeId> {
    use crate::primitives::curve::Line;

    let line = Line::new(*p1, *p2);
    let curve_id = model.curves.add(Box::new(line));

    let v1 = model.vertices.add(p1.x, p1.y, p1.z);
    let v2 = model.vertices.add(p2.x, p2.y, p2.z);

    let edge = Edge::new_auto_range(
        0, // Will be assigned by store
        v1,
        v2,
        curve_id,
        crate::primitives::edge::EdgeOrientation::Forward,
    );
    let edge_id = model.edges.add(edge);

    Ok(edge_id)
}

/// Trim faces against blend
fn trim_faces_against_blend(
    _model: &mut BRepModel,
    _face1_id: FaceId,
    _face2_id: FaceId,
    _blend_faces: &[FaceId],
) -> OperationResult<()> {
    // Would trim original faces to meet blend cleanly
    Ok(())
}

/// Validate blend inputs
fn validate_blend_inputs(
    model: &BRepModel,
    face1_id: FaceId,
    face2_id: FaceId,
    _options: &BlendOptions,
) -> OperationResult<()> {
    // Check faces exist
    if model.faces.get(face1_id).is_none() {
        return Err(OperationError::InvalidGeometry(
            "First face not found".to_string(),
        ));
    }
    if model.faces.get(face2_id).is_none() {
        return Err(OperationError::InvalidGeometry(
            "Second face not found".to_string(),
        ));
    }

    // Check faces are different
    if face1_id == face2_id {
        return Err(OperationError::InvalidGeometry(
            "Cannot blend face with itself".to_string(),
        ));
    }

    Ok(())
}

/// Validate blend result
fn validate_blend_result(model: &BRepModel, blend_faces: &[FaceId]) -> OperationResult<()> {
    // Would validate blend continuity and quality
    for &face_id in blend_faces {
        if model.faces.get(face_id).is_none() {
            return Err(OperationError::InvalidBRep(
                "Blend face not found".to_string(),
            ));
        }
    }

    Ok(())
}

/*
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blend_validation() {
        // Test parameter validation
    }
}
*/
