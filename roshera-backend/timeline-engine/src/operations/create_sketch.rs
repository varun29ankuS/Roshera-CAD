//! Create sketch operation implementation

use super::brep_helpers::BRepModelExt;
use super::common::brep_to_entity_state;
use crate::{
    execution::{ExecutionContext, OperationImpl, ResourceEstimate},
    CreatedEntity, EntityId, EntityType, Operation, OperationOutputs, SketchElement, SketchPlane,
    TimelineError, TimelineResult,
};
use async_trait::async_trait;
use geometry_engine::{
    math::{Matrix4, Point3, Vector3},
    primitives::{
        edge::EdgeId,
        r#loop::{LoopId, LoopType},
        topology_builder::BRepModel,
        vertex::VertexId,
    },
};

/// Implementation of create sketch operation
pub struct CreateSketchOp;

#[async_trait]
impl OperationImpl for CreateSketchOp {
    fn operation_type(&self) -> &'static str {
        "create_sketch"
    }

    async fn validate(
        &self,
        operation: &Operation,
        _context: &ExecutionContext,
    ) -> TimelineResult<()> {
        if let Operation::CreateSketch { elements, .. } = operation {
            // Validate we have at least one element
            if elements.is_empty() {
                return Err(TimelineError::ValidationError(
                    "Sketch must contain at least one element".to_string(),
                ));
            }

            // Validate each element
            for element in elements {
                validate_sketch_element(element)?;
            }

            Ok(())
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected CreateSketch operation".to_string(),
            ))
        }
    }

    async fn execute(
        &self,
        operation: &Operation,
        context: &mut ExecutionContext,
    ) -> TimelineResult<OperationOutputs> {
        if let Operation::CreateSketch { plane, elements } = operation {
            // Create a new BRep model for the sketch
            let mut brep = BRepModel::new();

            // Get the transformation matrix for the sketch plane
            let transform = get_plane_transform(plane);
            let _inverse_transform = transform.inverse().map_err(|_| {
                TimelineError::ExecutionError(
                    "Failed to compute inverse transform for sketch plane".to_string(),
                )
            })?;

            // Create sketch elements
            let mut vertices = Vec::new();
            let mut edges = Vec::new();

            for element in elements {
                match element {
                    SketchElement::Line { start, end } => {
                        create_line_in_sketch(
                            &mut brep,
                            &transform,
                            *start,
                            *end,
                            &mut vertices,
                            &mut edges,
                        )?;
                    }

                    SketchElement::Arc {
                        center,
                        radius,
                        start_angle,
                        end_angle,
                    } => {
                        create_arc_in_sketch(
                            &mut brep,
                            &transform,
                            *center,
                            *radius,
                            *start_angle,
                            *end_angle,
                            &mut vertices,
                            &mut edges,
                        )?;
                    }

                    SketchElement::Circle { center, radius } => {
                        create_circle_in_sketch(
                            &mut brep,
                            &transform,
                            *center,
                            *radius,
                            &mut vertices,
                            &mut edges,
                        )?;
                    }

                    SketchElement::Rectangle {
                        corner,
                        width,
                        height,
                    } => {
                        create_rectangle_in_sketch(
                            &mut brep,
                            &transform,
                            *corner,
                            *width,
                            *height,
                            &mut vertices,
                            &mut edges,
                        )?;
                    }
                }
            }

            // Create loops from edges if they form closed contours
            let loops = find_loops_from_edges(&mut brep, &edges)?;

            // Store sketch properties
            let sketch_properties = serde_json::json!({
                "plane": plane,
                "element_count": elements.len(),
                "vertex_count": vertices.len(),
                "edge_count": edges.len(),
                "loop_count": loops.len(),
                "transform": transform_to_json(&transform),
            });

            // Create entity for the sketch
            let sketch_id = EntityId::new();
            let entity_state = brep_to_entity_state(
                &brep,
                sketch_id,
                EntityType::Sketch,
                Some("Sketch".to_string()),
            )?;

            // Add properties to entity. `sketch_properties` is always a JSON
            // object (constructed via `json!({...})` above), so this merge is
            // a no-op on the unlikely cons-but-safe `None` branch.
            let mut final_entity = entity_state;
            if let (Some(obj), Some(props)) = (
                final_entity.properties.as_object_mut(),
                sketch_properties.as_object(),
            ) {
                for (key, value) in props {
                    obj.insert(key.clone(), value.clone());
                }
            }

            // Add to context
            context.add_temp_entity(final_entity)?;
            context.increment_geometry_ops();

            // Create output
            let outputs = OperationOutputs {
                created: vec![CreatedEntity {
                    id: sketch_id,
                    entity_type: EntityType::Sketch,
                    name: Some("Sketch".to_string()),
                }],
                modified: vec![],
                deleted: vec![],
                side_effects: vec![],
            };

            Ok(outputs)
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected CreateSketch operation".to_string(),
            ))
        }
    }

    fn estimate_resources(&self, operation: &Operation) -> ResourceEstimate {
        if let Operation::CreateSketch { elements, .. } = operation {
            let element_count = elements.len();
            let estimated_vertices = element_count * 4; // Average vertices per element
            let estimated_edges = element_count * 2;

            ResourceEstimate {
                memory_bytes: (estimated_vertices * 64 + estimated_edges * 128) as u64,
                time_ms: (element_count * 5) as u64, // 5ms per element
                entities_created: 1,
                entities_modified: 0,
            }
        } else {
            ResourceEstimate::default()
        }
    }
}

/// Validate a sketch element
fn validate_sketch_element(element: &SketchElement) -> TimelineResult<()> {
    match element {
        SketchElement::Line { start, end } => {
            if (start[0] - end[0]).abs() < 1e-10 && (start[1] - end[1]).abs() < 1e-10 {
                return Err(TimelineError::ValidationError(
                    "Line start and end points are the same".to_string(),
                ));
            }
        }

        SketchElement::Arc {
            radius,
            start_angle,
            end_angle,
            ..
        } => {
            if *radius <= 0.0 {
                return Err(TimelineError::ValidationError(
                    "Arc radius must be positive".to_string(),
                ));
            }
            if (*start_angle - *end_angle).abs() < 1e-10 {
                return Err(TimelineError::ValidationError(
                    "Arc start and end angles are the same".to_string(),
                ));
            }
        }

        SketchElement::Circle { radius, .. } => {
            if *radius <= 0.0 {
                return Err(TimelineError::ValidationError(
                    "Circle radius must be positive".to_string(),
                ));
            }
        }

        SketchElement::Rectangle { width, height, .. } => {
            if *width <= 0.0 {
                return Err(TimelineError::ValidationError(
                    "Rectangle width must be positive".to_string(),
                ));
            }
            if *height <= 0.0 {
                return Err(TimelineError::ValidationError(
                    "Rectangle height must be positive".to_string(),
                ));
            }
        }
    }

    Ok(())
}

/// Get transformation matrix for sketch plane
fn get_plane_transform(plane: &SketchPlane) -> Matrix4 {
    match plane {
        SketchPlane::XY => Matrix4::identity(),

        SketchPlane::XZ => Matrix4::from_rows_array([
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, -1.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ]),

        SketchPlane::YZ => Matrix4::from_rows_array([
            [0.0, 1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ]),

        SketchPlane::Custom {
            origin,
            normal,
            x_dir,
        } => {
            let z = Vector3 {
                x: normal[0],
                y: normal[1],
                z: normal[2],
            }
            .normalize_or_zero();
            let x = Vector3 {
                x: x_dir[0],
                y: x_dir[1],
                z: x_dir[2],
            }
            .normalize_or_zero();
            let y = z.cross(&x).normalize_or_zero();

            Matrix4::from_rows_array([
                [x.x, y.x, z.x, origin[0]],
                [x.y, y.y, z.y, origin[1]],
                [x.z, y.z, z.z, origin[2]],
                [0.0, 0.0, 0.0, 1.0],
            ])
        }
    }
}

/// Convert transform matrix to JSON
fn transform_to_json(transform: &Matrix4) -> serde_json::Value {
    let mut rows = Vec::new();
    for i in 0..4 {
        let mut row = Vec::new();
        for j in 0..4 {
            row.push(transform[(i, j)]);
        }
        rows.push(row);
    }
    serde_json::json!(rows)
}

/// Create a line in the sketch
fn create_line_in_sketch(
    brep: &mut BRepModel,
    transform: &Matrix4,
    start: [f64; 2],
    end: [f64; 2],
    vertices: &mut Vec<VertexId>,
    edges: &mut Vec<EdgeId>,
) -> TimelineResult<()> {
    // Transform 2D points to 3D
    let start_3d = transform.transform_point(&Point3::new(start[0], start[1], 0.0));
    let end_3d = transform.transform_point(&Point3::new(end[0], end[1], 0.0));

    // Create or find vertices
    let v1 = find_or_create_vertex(brep, start_3d, vertices);
    let v2 = find_or_create_vertex(brep, end_3d, vertices);

    // Create edge
    let edge = brep.add_edge(v1, v2, None);
    edges.push(edge);

    Ok(())
}

/// Create an arc in the sketch
fn create_arc_in_sketch(
    brep: &mut BRepModel,
    transform: &Matrix4,
    center: [f64; 2],
    radius: f64,
    start_angle: f64,
    end_angle: f64,
    vertices: &mut Vec<VertexId>,
    edges: &mut Vec<EdgeId>,
) -> TimelineResult<()> {
    // Convert angles to radians
    let start_rad = start_angle.to_radians();
    let end_rad = end_angle.to_radians();

    // Calculate start and end points
    let start_2d = [
        center[0] + radius * start_rad.cos(),
        center[1] + radius * start_rad.sin(),
    ];
    let end_2d = [
        center[0] + radius * end_rad.cos(),
        center[1] + radius * end_rad.sin(),
    ];

    // Transform to 3D
    let start_3d = transform.transform_point(&Point3::new(start_2d[0], start_2d[1], 0.0));
    let end_3d = transform.transform_point(&Point3::new(end_2d[0], end_2d[1], 0.0));
    let _center_3d = transform.transform_point(&Point3::new(center[0], center[1], 0.0));

    // Create vertices
    let v1 = find_or_create_vertex(brep, start_3d, vertices);
    let v2 = find_or_create_vertex(brep, end_3d, vertices);

    // Create arc edge with curve information
    let edge = brep.add_edge(v1, v2, None);
    edges.push(edge);

    Ok(())
}

/// Create a circle in the sketch
fn create_circle_in_sketch(
    brep: &mut BRepModel,
    transform: &Matrix4,
    center: [f64; 2],
    radius: f64,
    vertices: &mut Vec<VertexId>,
    edges: &mut Vec<EdgeId>,
) -> TimelineResult<()> {
    // For a circle, we create multiple segments
    const NUM_SEGMENTS: usize = 16;

    let mut circle_vertices = Vec::new();

    // Create vertices around the circle
    for i in 0..NUM_SEGMENTS {
        let angle = (i as f64) * 2.0 * std::f64::consts::PI / (NUM_SEGMENTS as f64);
        let x = center[0] + radius * angle.cos();
        let y = center[1] + radius * angle.sin();

        let point_3d = transform.transform_point(&Point3::new(x, y, 0.0));
        let vertex = find_or_create_vertex(brep, point_3d, vertices);
        circle_vertices.push(vertex);
    }

    // Create edges connecting the vertices
    for i in 0..NUM_SEGMENTS {
        let v1 = circle_vertices[i];
        let v2 = circle_vertices[(i + 1) % NUM_SEGMENTS];
        let edge = brep.add_edge(v1, v2, None);
        edges.push(edge);
    }

    Ok(())
}

/// Create a rectangle in the sketch
fn create_rectangle_in_sketch(
    brep: &mut BRepModel,
    transform: &Matrix4,
    corner: [f64; 2],
    width: f64,
    height: f64,
    vertices: &mut Vec<VertexId>,
    edges: &mut Vec<EdgeId>,
) -> TimelineResult<()> {
    // Four corners of rectangle
    let corners_2d = [
        corner,
        [corner[0] + width, corner[1]],
        [corner[0] + width, corner[1] + height],
        [corner[0], corner[1] + height],
    ];

    // Transform to 3D and create vertices
    let mut rect_vertices = Vec::new();
    for corner_2d in &corners_2d {
        let point_3d = transform.transform_point(&Point3::new(corner_2d[0], corner_2d[1], 0.0));
        let vertex = find_or_create_vertex(brep, point_3d, vertices);
        rect_vertices.push(vertex);
    }

    // Create edges
    for i in 0..4 {
        let v1 = rect_vertices[i];
        let v2 = rect_vertices[(i + 1) % 4];
        let edge = brep.add_edge(v1, v2, None);
        edges.push(edge);
    }

    Ok(())
}

/// Find or create a vertex at the given position
fn find_or_create_vertex(
    brep: &mut BRepModel,
    position: Point3,
    vertices: &mut Vec<VertexId>,
) -> VertexId {
    // Check if vertex already exists at this position
    const TOLERANCE: f64 = 1e-10;

    for &vertex_id in vertices.iter() {
        if let Some(vertex) = brep.vertices.get(vertex_id) {
            let vertex_pos = vertex.point();
            if (vertex_pos - position).magnitude() < TOLERANCE {
                return vertex_id;
            }
        }
    }

    // Create new vertex
    let vertex_id = brep.add_vertex(position);
    vertices.push(vertex_id);
    vertex_id
}

/// Find loops from edges
fn find_loops_from_edges(brep: &mut BRepModel, edges: &[EdgeId]) -> TimelineResult<Vec<LoopId>> {
    let mut loops = Vec::new();
    let mut used_edges = std::collections::HashSet::new();

    // Try to form loops from edges
    for &edge_id in edges {
        if used_edges.contains(&edge_id) {
            continue;
        }

        // Try to follow edges to form a loop
        if let Some(loop_edges) = try_form_loop(brep, edge_id, edges, &used_edges) {
            // Create loop
            let loop_id = brep.add_loop(LoopType::Outer);

            if let Some(loop_) = brep.loops.get_mut(loop_id) {
                for &edge in &loop_edges {
                    loop_.edges.push(edge);
                    loop_.orientations.push(true); // Assuming forward orientation
                    used_edges.insert(edge);
                }
            }

            loops.push(loop_id);
        }
    }

    Ok(loops)
}

/// Try to form a loop starting from the given edge
fn try_form_loop(
    brep: &BRepModel,
    start_edge: EdgeId,
    all_edges: &[EdgeId],
    used_edges: &std::collections::HashSet<EdgeId>,
) -> Option<Vec<EdgeId>> {
    let mut loop_edges = vec![start_edge];
    let mut current_edge = start_edge;

    // Get start vertex of the loop
    let start_vertex = if let Some(edge) = brep.edges.get(start_edge) {
        edge.start_vertex
    } else {
        return None;
    };

    // Follow edges until we return to start
    loop {
        // Get end vertex of current edge
        let end_vertex = if let Some(edge) = brep.edges.get(current_edge) {
            edge.end_vertex
        } else {
            return None;
        };

        // If we're back at start, we have a loop
        if end_vertex == start_vertex && loop_edges.len() > 2 {
            return Some(loop_edges);
        }

        // Find next edge starting from end_vertex
        let mut found_next = false;
        for &edge_id in all_edges {
            if used_edges.contains(&edge_id) || loop_edges.contains(&edge_id) {
                continue;
            }

            if let Some(edge) = brep.edges.get(edge_id) {
                if edge.start_vertex == end_vertex {
                    loop_edges.push(edge_id);
                    current_edge = edge_id;
                    found_next = true;
                    break;
                }
            }
        }

        // If we can't continue, this doesn't form a loop
        if !found_next {
            return None;
        }

        // Prevent infinite loops
        if loop_edges.len() > all_edges.len() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::EntityStateStore;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_create_sketch_validation() {
        let op = CreateSketchOp;
        let store = Arc::new(EntityStateStore::new());
        let context = ExecutionContext::new(crate::BranchId::main(), store);

        // Empty sketch should fail validation
        let operation = Operation::CreateSketch {
            plane: SketchPlane::XY,
            elements: vec![],
        };

        assert!(op.validate(&operation, &context).await.is_err());

        // Valid sketch should pass
        let operation = Operation::CreateSketch {
            plane: SketchPlane::XY,
            elements: vec![SketchElement::Line {
                start: [0.0, 0.0],
                end: [1.0, 0.0],
            }],
        };

        assert!(op.validate(&operation, &context).await.is_ok());
    }

    #[test]
    fn test_plane_transform() {
        // XY plane should be identity
        let transform = get_plane_transform(&SketchPlane::XY);
        let point = transform.transform_point(&Point3::new(1.0, 2.0, 0.0));
        assert!((point.x - 1.0).abs() < 1e-10);
        assert!((point.y - 2.0).abs() < 1e-10);
        assert!((point.z - 0.0).abs() < 1e-10);

        // Custom plane
        let transform = get_plane_transform(&SketchPlane::Custom {
            origin: [10.0, 20.0, 30.0],
            normal: [0.0, 0.0, 1.0],
            x_dir: [1.0, 0.0, 0.0],
        });
        let point = transform.transform_point(&Point3::new(1.0, 2.0, 0.0));
        assert!((point.x - 11.0).abs() < 1e-10);
        assert!((point.y - 22.0).abs() < 1e-10);
        assert!((point.z - 30.0).abs() < 1e-10);
    }
}
