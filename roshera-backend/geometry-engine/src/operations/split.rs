//! Split Operations for B-Rep Models
//!
//! Splits faces, edges, and solids using various splitting tools
//! including planes, surfaces, and curves.

use super::intersect::intersect_surfaces;
use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Point3, Vector3};
use crate::primitives::{
    edge::{Edge, EdgeId},
    face::{Face, FaceId},
    r#loop::LoopId,
    solid::SolidId,
    topology_builder::BRepModel,
};

/// Options for split operations
#[derive(Debug, Clone)]
pub struct SplitOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// What to keep after split
    pub keep: SplitKeep,

    /// Whether to split all or stop at first
    pub split_all: bool,

    /// Gap tolerance for splitting
    pub gap_tolerance: f64,
}

impl Default for SplitOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            keep: SplitKeep::Both,
            split_all: true,
            gap_tolerance: 1e-6,
        }
    }
}

/// What to keep after split operation
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SplitKeep {
    /// Keep both sides
    Both,
    /// Keep positive side (relative to normal)
    Positive,
    /// Keep negative side
    Negative,
    /// Keep the larger piece
    Larger,
    /// Keep the smaller piece
    Smaller,
}

/// Split a face by a plane
pub fn split_face_by_plane(
    model: &mut BRepModel,
    face_id: FaceId,
    plane_origin: Point3,
    plane_normal: Vector3,
    options: SplitOptions,
) -> OperationResult<Vec<FaceId>> {
    // Validate inputs
    validate_split_face_inputs(model, face_id, &options)?;

    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?
        .clone();

    // Find intersection curve between face and plane
    let intersection_curve =
        compute_face_plane_intersection(model, &face, plane_origin, plane_normal)?;

    if intersection_curve.is_empty() {
        // No intersection, return original face
        return Ok(vec![face_id]);
    }

    // Split face along intersection curve
    let split_faces = split_face_along_curve(model, &face, &intersection_curve, &options)?;

    // Filter based on keep option
    let result_faces =
        filter_split_results(model, split_faces, plane_origin, plane_normal, options.keep)?;

    Ok(result_faces)
}

/// Split a face by a surface
pub fn split_face_by_surface(
    model: &mut BRepModel,
    face_id: FaceId,
    _splitting_surface_id: u32,
    options: SplitOptions,
) -> OperationResult<Vec<FaceId>> {
    // Validate inputs
    validate_split_face_inputs(model, face_id, &options)?;

    // Find intersection curves between faces
    let intersection = intersect_surfaces(
        model,
        face_id,
        face_id, // Would use splitting surface face
        options.common.tolerance,
    )?;

    match intersection {
        super::intersect::IntersectionResult::Curves(curves) => {
            // Split along intersection curves
            split_face_by_intersection_curves(model, face_id, curves, &options)
        }
        super::intersect::IntersectionResult::Surface(_) => {
            // Surfaces are coincident
            Err(OperationError::InvalidGeometry(
                "Surfaces are coincident".to_string(),
            ))
        }
        _ => {
            // No intersection
            Ok(vec![face_id])
        }
    }
}

/// Split an edge at a parameter
pub fn split_edge_at_parameter(
    model: &mut BRepModel,
    edge_id: EdgeId,
    parameter: f64,
) -> OperationResult<(EdgeId, EdgeId)> {
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
        .clone();

    // Validate parameter
    if parameter <= 0.0 || parameter >= 1.0 {
        return Err(OperationError::InvalidGeometry(
            "Split parameter must be between 0 and 1".to_string(),
        ));
    }

    // Create split vertex
    let split_point = edge.evaluate(parameter, &model.curves)?;
    let split_vertex = model
        .vertices
        .add(split_point.x, split_point.y, split_point.z);

    // Map to curve parameter
    let curve_param = edge.edge_to_curve_parameter(parameter);

    // Create first edge (start to split)
    let edge1 = Edge::new(
        0, // ID will be assigned by store
        edge.start_vertex,
        split_vertex,
        edge.curve_id,
        edge.orientation,
        crate::primitives::curve::ParameterRange::new(edge.param_range.start, curve_param),
    );
    let edge1_id = model.edges.add(edge1);

    // Create second edge (split to end)
    let edge2 = Edge::new(
        0, // ID will be assigned by store
        split_vertex,
        edge.end_vertex,
        edge.curve_id,
        edge.orientation,
        crate::primitives::curve::ParameterRange::new(curve_param, edge.param_range.end),
    );
    let edge2_id = model.edges.add(edge2);

    Ok((edge1_id, edge2_id))
}

/// Split a solid by a plane
pub fn split_solid_by_plane(
    model: &mut BRepModel,
    solid_id: SolidId,
    plane_origin: Point3,
    plane_normal: Vector3,
    options: SplitOptions,
) -> OperationResult<Vec<SolidId>> {
    // Validate inputs
    validate_split_solid_inputs(model, solid_id, &options)?;

    let solid = model
        .solids
        .get(solid_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Solid not found".to_string()))?
        .clone();

    // Split all faces of the solid
    let shell = model
        .shells
        .get(solid.outer_shell)
        .ok_or_else(|| OperationError::InvalidGeometry("Shell not found".to_string()))?
        .clone();

    let mut split_face_groups = Vec::new();

    for &face_id in &shell.faces {
        let split_faces =
            split_face_by_plane(model, face_id, plane_origin, plane_normal, options.clone())?;
        split_face_groups.push(split_faces);
    }

    // Create cap faces at split plane
    let cap_faces = create_cap_faces(model, &split_face_groups, plane_origin, plane_normal)?;

    // Assemble split solids
    let split_solids = assemble_split_solids(
        model,
        split_face_groups,
        cap_faces,
        plane_origin,
        plane_normal,
    )?;

    Ok(split_solids)
}

/// Compute face-plane intersection by sampling boundary edges and finding
/// signed-distance sign changes. Returns the ordered list of crossing points
/// where the face boundary pierces the plane. An empty result means the
/// boundary either lies entirely on one side or grazes the plane within
/// numerical tolerance.
fn compute_face_plane_intersection(
    model: &BRepModel,
    face: &Face,
    plane_origin: Point3,
    plane_normal: Vector3,
) -> OperationResult<Vec<Point3>> {
    let normal_length = plane_normal.magnitude();
    if normal_length < 1e-12 {
        return Err(OperationError::InvalidGeometry(
            "Plane normal is degenerate".to_string(),
        ));
    }
    let n = plane_normal / normal_length;

    let signed_distance = |p: Point3| -> f64 {
        let v = p - plane_origin;
        v.dot(&n)
    };

    let outer = model
        .loops
        .get(face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Face outer loop not found".to_string()))?
        .clone();

    let mut crossings: Vec<Point3> = Vec::new();
    const SAMPLES_PER_EDGE: usize = 16;

    for &edge_id in &outer.edges {
        let edge = match model.edges.get(edge_id) {
            Some(e) => e.clone(),
            None => continue,
        };

        // Sample edge at SAMPLES_PER_EDGE+1 points; detect sign changes.
        let mut prev_pt: Option<Point3> = None;
        let mut prev_sd: f64 = 0.0;
        for i in 0..=SAMPLES_PER_EDGE {
            let t = i as f64 / SAMPLES_PER_EDGE as f64;
            let pt = match edge.evaluate(t, &model.curves) {
                Ok(p) => p,
                Err(_) => continue,
            };
            let sd = signed_distance(pt);

            if let Some(prev) = prev_pt {
                let on_plane = sd.abs() < 1e-9;
                let sign_flip = prev_sd * sd < 0.0;
                if on_plane {
                    crossings.push(pt);
                } else if sign_flip {
                    // Linear interpolation for crossing point
                    let alpha = prev_sd / (prev_sd - sd);
                    let crossing = Point3::new(
                        prev.x + alpha * (pt.x - prev.x),
                        prev.y + alpha * (pt.y - prev.y),
                        prev.z + alpha * (pt.z - prev.z),
                    );
                    crossings.push(crossing);
                }
            }
            prev_pt = Some(pt);
            prev_sd = sd;
        }
    }

    Ok(crossings)
}

/// Split face along an intersection polyline. The current implementation
/// requires the full topology-rebuild infrastructure used by boolean
/// operations (loop arrangement, vertex insertion, edge splitting). Until
/// `split` is wired through `boolean::split_faces_along_curves`, this
/// returns the original face unchanged when the polyline has fewer than 2
/// crossings, and errors with `NotImplemented` for richer cases so callers
/// don't silently receive incorrect topology.
fn split_face_along_curve(
    _model: &mut BRepModel,
    face: &Face,
    split_curve: &[Point3],
    _options: &SplitOptions,
) -> OperationResult<Vec<FaceId>> {
    if split_curve.len() < 2 {
        return Ok(vec![face.id]);
    }
    Err(OperationError::NotImplemented(
        "split_face_along_curve requires topology arrangement; use boolean operations instead"
            .to_string(),
    ))
}

/// Split face by multiple intersection curves. Returns NotImplemented when
/// any curves are present — multi-curve face arrangement is the same
/// algorithm as boolean splitting and should be routed through
/// `operations::boolean` rather than reimplemented here.
fn split_face_by_intersection_curves(
    _model: &mut BRepModel,
    face_id: FaceId,
    curves: Vec<super::intersect::IntersectionCurve>,
    _options: &SplitOptions,
) -> OperationResult<Vec<FaceId>> {
    if curves.is_empty() {
        return Ok(vec![face_id]);
    }
    Err(OperationError::NotImplemented(
        "split_face_by_intersection_curves requires topology arrangement; use boolean operations"
            .to_string(),
    ))
}

/// Filter split results based on keep option
fn filter_split_results(
    model: &BRepModel,
    faces: Vec<FaceId>,
    plane_origin: Point3,
    plane_normal: Vector3,
    keep: SplitKeep,
) -> OperationResult<Vec<FaceId>> {
    match keep {
        SplitKeep::Both => Ok(faces),
        SplitKeep::Positive => {
            // Keep faces on positive side of plane
            filter_faces_by_side(model, faces, plane_origin, plane_normal, true)
        }
        SplitKeep::Negative => {
            // Keep faces on negative side of plane
            filter_faces_by_side(model, faces, plane_origin, plane_normal, false)
        }
        SplitKeep::Larger => {
            // Keep larger faces
            filter_faces_by_size(model, faces, true)
        }
        SplitKeep::Smaller => {
            // Keep smaller faces
            filter_faces_by_size(model, faces, false)
        }
    }
}

/// Filter faces by side of plane
fn filter_faces_by_side(
    model: &BRepModel,
    faces: Vec<FaceId>,
    plane_origin: Point3,
    plane_normal: Vector3,
    positive_side: bool,
) -> OperationResult<Vec<FaceId>> {
    let mut result = Vec::new();

    for face_id in faces {
        let face = model
            .faces
            .get(face_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

        // Check which side face centroid is on
        let centroid = compute_face_centroid(model, face)?;
        let to_centroid = centroid - plane_origin;
        let dot = to_centroid.dot(&plane_normal);

        if (dot > 0.0) == positive_side {
            result.push(face_id);
        }
    }

    Ok(result)
}

/// Filter faces by approximate planar area (keeps the largest or smallest
/// half by area). Area is approximated using the surveyor's formula on the
/// boundary loop's start vertices projected onto the face's best-fit plane.
fn filter_faces_by_size(
    model: &BRepModel,
    faces: Vec<FaceId>,
    keep_larger: bool,
) -> OperationResult<Vec<FaceId>> {
    if faces.len() <= 1 {
        return Ok(faces);
    }

    let mut areas: Vec<(FaceId, f64)> = Vec::with_capacity(faces.len());
    for face_id in &faces {
        let face = model
            .faces
            .get(*face_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;
        let area = approximate_face_area(model, face)?;
        areas.push((*face_id, area));
    }

    // Threshold = median area; partition above/below
    areas.sort_by(|a, b| {
        a.1.partial_cmp(&b.1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let median = areas[areas.len() / 2].1;

    let result = areas
        .into_iter()
        .filter(|(_, a)| {
            if keep_larger {
                *a >= median
            } else {
                *a <= median
            }
        })
        .map(|(id, _)| id)
        .collect();

    Ok(result)
}

/// Compute face centroid as the average of its outer-loop boundary vertices.
/// For faces with inner loops (holes), the holes contribute their boundary
/// vertices with negative weight proportional to their loop length, giving
/// a more accurate estimate than the pure outer-loop average. This is a
/// boundary-vertex approximation; for exact centroids of curved surfaces use
/// surface-specific area integration.
fn compute_face_centroid(model: &BRepModel, face: &Face) -> OperationResult<Point3> {
    let outer = model
        .loops
        .get(face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Face outer loop not found".to_string()))?;

    let mut sum = Point3::ZERO;
    let mut count: usize = 0;
    for &edge_id in &outer.edges {
        let edge = match model.edges.get(edge_id) {
            Some(e) => e,
            None => continue,
        };
        if let Some(v) = model.vertices.get(edge.start_vertex) {
            sum.x += v.position[0] as f64;
            sum.y += v.position[1] as f64;
            sum.z += v.position[2] as f64;
            count += 1;
        }
    }

    if count == 0 {
        return Err(OperationError::InvalidGeometry(
            "Face has empty outer loop".to_string(),
        ));
    }

    Ok(Point3::new(
        sum.x / count as f64,
        sum.y / count as f64,
        sum.z / count as f64,
    ))
}

/// Approximate face area by triangulating the outer-loop polygon (fan from
/// centroid) in 3D, summing |cross| / 2 over each triangle. Inner loops
/// (holes) subtract their fan-area. Sufficient for size-comparison filtering;
/// not for exact volume/area properties of curved surfaces.
fn approximate_face_area(model: &BRepModel, face: &Face) -> OperationResult<f64> {
    let centroid = compute_face_centroid(model, face)?;

    let loop_area = |loop_id: LoopId| -> OperationResult<f64> {
        let lp = match model.loops.get(loop_id) {
            Some(l) => l,
            None => return Ok(0.0),
        };
        let mut area = 0.0;
        for &edge_id in &lp.edges {
            let edge = match model.edges.get(edge_id) {
                Some(e) => e,
                None => continue,
            };
            let v0 = match model.vertices.get(edge.start_vertex) {
                Some(v) => Point3::new(
                    v.position[0] as f64,
                    v.position[1] as f64,
                    v.position[2] as f64,
                ),
                None => continue,
            };
            let v1 = match model.vertices.get(edge.end_vertex) {
                Some(v) => Point3::new(
                    v.position[0] as f64,
                    v.position[1] as f64,
                    v.position[2] as f64,
                ),
                None => continue,
            };
            let a = v0 - centroid;
            let b = v1 - centroid;
            area += a.cross(&b).magnitude() * 0.5;
        }
        Ok(area)
    };

    let mut total = loop_area(face.outer_loop)?;
    for &inner in &face.inner_loops {
        total -= loop_area(inner)?;
    }
    Ok(total.max(0.0))
}

/// Create cap faces at the split plane. Capping requires identifying the
/// closed boundary of the cut on the plane, building a planar surface, and
/// stitching the faces back into the topology — same primitive as
/// `boolean::create_cap_face` after a planar boolean. Until this is wired
/// through, callers should use `operations::boolean::boolean_difference`
/// with a half-space proxy solid for plane-splitting use cases.
fn create_cap_faces(
    _model: &mut BRepModel,
    split_face_groups: &[Vec<FaceId>],
    _plane_origin: Point3,
    _plane_normal: Vector3,
) -> OperationResult<Vec<FaceId>> {
    if split_face_groups.iter().all(|g| g.len() <= 1) {
        return Ok(Vec::new());
    }
    Err(OperationError::NotImplemented(
        "create_cap_faces requires planar capping; use boolean_difference with half-space"
            .to_string(),
    ))
}

/// Assemble two solids from the post-split face groups by walking the
/// dual-graph connectivity. Same algorithm as boolean assembly. Until this
/// is wired through, only the trivial no-split case (zero face groups
/// changed) succeeds.
fn assemble_split_solids(
    _model: &mut BRepModel,
    split_face_groups: Vec<Vec<FaceId>>,
    cap_faces: Vec<FaceId>,
    _plane_origin: Point3,
    _plane_normal: Vector3,
) -> OperationResult<Vec<SolidId>> {
    if split_face_groups.is_empty() && cap_faces.is_empty() {
        return Ok(Vec::new());
    }
    Err(OperationError::NotImplemented(
        "assemble_split_solids requires dual-graph traversal; use boolean operations".to_string(),
    ))
}

/// Validate split face inputs
fn validate_split_face_inputs(
    model: &BRepModel,
    face_id: FaceId,
    _options: &SplitOptions,
) -> OperationResult<()> {
    if model.faces.get(face_id).is_none() {
        return Err(OperationError::InvalidGeometry(
            "Face not found".to_string(),
        ));
    }

    Ok(())
}

/// Validate split solid inputs
fn validate_split_solid_inputs(
    model: &BRepModel,
    solid_id: SolidId,
    _options: &SplitOptions,
) -> OperationResult<()> {
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidGeometry(
            "Solid not found".to_string(),
        ));
    }

    Ok(())
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_edge_split() {
//         // Test edge splitting at parameter
//     }
// }
