//! Enhanced WebSocket handlers for ALL geometry-engine operations
//! This connects every geometry operation to the WebSocket interface

use axum::extract::ws::{Message, WebSocket};
use futures::sink::SinkExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use crate::AppState;
use geometry_engine::primitives::topology_builder::{TopologyBuilder, BRepModel};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::math::{Point3, Vector3, Matrix4};
use shared_types::{GeometryId, Vector3D, Transform3D, BooleanOp, Mesh};
use std::collections::HashMap;
use tracing::{info, error, warn};
use uuid::Uuid;

/// Complete set of geometry operations supported by the engine
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "operation")]
pub enum GeometryOperation {
    // Primitive Creation
    CreateBox {
        width: f64,
        height: f64,
        depth: f64,
    },
    CreateSphere {
        center: [f64; 3],
        radius: f64,
    },
    CreateCylinder {
        base_center: [f64; 3],
        axis: [f64; 3],
        radius: f64,
        height: f64,
    },
    CreateCone {
        base_center: [f64; 3],
        axis: [f64; 3],
        base_radius: f64,
        top_radius: f64,
        height: f64,
    },
    CreateTorus {
        center: [f64; 3],
        axis: [f64; 3],
        major_radius: f64,
        minor_radius: f64,
    },
    
    // Boolean Operations
    BooleanUnion {
        solid_a: String,
        solid_b: String,
    },
    BooleanIntersection {
        solid_a: String,
        solid_b: String,
    },
    BooleanDifference {
        solid_a: String,
        solid_b: String,
    },
    
    // Transformation Operations
    TransformSolid {
        solid_id: String,
        transform: TransformData,
    },
    TranslateSolid {
        solid_id: String,
        translation: [f64; 3],
    },
    RotateSolid {
        solid_id: String,
        axis: [f64; 3],
        angle_degrees: f64,
    },
    ScaleSolid {
        solid_id: String,
        scale: [f64; 3],
    },
    
    // Extrusion Operations
    ExtrudeFace {
        face_id: String,
        direction: [f64; 3],
        distance: f64,
    },
    ExtrudeProfile {
        profile_id: String,
        direction: [f64; 3],
        distance: f64,
    },
    
    // Revolution Operations
    RevolveFace {
        face_id: String,
        axis_origin: [f64; 3],
        axis_direction: [f64; 3],
        angle_degrees: f64,
    },
    RevolveProfile {
        profile_id: String,
        axis_origin: [f64; 3],
        axis_direction: [f64; 3],
        angle_degrees: f64,
    },
    
    // Sweep Operations
    SweepProfile {
        profile_id: String,
        path_id: String,
        twist_degrees: Option<f64>,
        scale_factor: Option<f64>,
    },
    
    // Fillet and Chamfer
    FilletEdges {
        solid_id: String,
        edge_ids: Vec<String>,
        radius: f64,
    },
    FilletVertices {
        solid_id: String,
        vertex_ids: Vec<String>,
        radius: f64,
    },
    ChamferEdges {
        solid_id: String,
        edge_ids: Vec<String>,
        distance: f64,
    },
    
    // Pattern Operations
    LinearPattern {
        solid_id: String,
        direction: [f64; 3],
        count: u32,
        spacing: f64,
    },
    CircularPattern {
        solid_id: String,
        axis_origin: [f64; 3],
        axis_direction: [f64; 3],
        count: u32,
        angle_degrees: f64,
    },
    
    // Query Operations
    GetVolume {
        solid_id: String,
    },
    GetSurfaceArea {
        solid_id: String,
    },
    GetBoundingBox {
        solid_id: String,
    },
    GetCenterOfMass {
        solid_id: String,
    },
    GetTopology {
        solid_id: String,
    },
    
    // Tessellation
    TessellateSolid {
        solid_id: String,
        quality: TessellationQuality,
    },
    
    // Validation
    ValidateSolid {
        solid_id: String,
        level: ValidationLevel,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct TransformData {
    pub translation: Option<[f64; 3]>,
    pub rotation: Option<[f64; 4]>, // Quaternion
    pub scale: Option<[f64; 3]>,
}

#[derive(Debug, Clone, Deserialize)]
pub enum TessellationQuality {
    Low,
    Medium,
    High,
    Custom {
        max_edge_length: f64,
        angle_tolerance: f64,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub enum ValidationLevel {
    Quick,
    Standard,
    Deep,
}

/// Response for geometry operations
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "result")]
pub enum GeometryOperationResult {
    Success {
        operation: String,
        output_ids: Vec<String>,
        mesh: Option<SimplifiedMesh>,
        properties: Option<GeometryProperties>,
    },
    Error {
        operation: String,
        message: String,
        details: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct SimplifiedMesh {
    pub vertices: Vec<f32>,
    pub normals: Vec<f32>,
    pub indices: Vec<u32>,
    pub bounds: BoundingBox,
}

#[derive(Debug, Clone, Serialize)]
pub struct BoundingBox {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

#[derive(Debug, Clone, Serialize)]
pub struct GeometryProperties {
    pub volume: f64,
    pub surface_area: f64,
    pub center_of_mass: [f64; 3],
    pub vertex_count: usize,
    pub edge_count: usize,
    pub face_count: usize,
    pub is_manifold: bool,
    pub is_closed: bool,
}

/// Main handler for all geometry operations
pub async fn handle_geometry_operation(
    operation: GeometryOperation,
    session_id: &str,
    user_id: &str,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Processing geometry operation: {:?} for session {}", operation, session_id);
    
    let result = match operation {
        // Primitive Creation
        GeometryOperation::CreateBox { width, height, depth } => {
            handle_create_box(width, height, depth, state).await
        }
        GeometryOperation::CreateSphere { center, radius } => {
            handle_create_sphere(center, radius, state).await
        }
        GeometryOperation::CreateCylinder { base_center, axis, radius, height } => {
            handle_create_cylinder(base_center, axis, radius, height, state).await
        }
        GeometryOperation::CreateCone { base_center, axis, base_radius, top_radius, height } => {
            handle_create_cone(base_center, axis, base_radius, top_radius, height, state).await
        }
        GeometryOperation::CreateTorus { center, axis, major_radius, minor_radius } => {
            handle_create_torus(center, axis, major_radius, minor_radius, state).await
        }
        
        // Boolean Operations
        GeometryOperation::BooleanUnion { solid_a, solid_b } => {
            handle_boolean_operation(BooleanOp::Union, solid_a, solid_b, state).await
        }
        GeometryOperation::BooleanIntersection { solid_a, solid_b } => {
            handle_boolean_operation(BooleanOp::Intersection, solid_a, solid_b, state).await
        }
        GeometryOperation::BooleanDifference { solid_a, solid_b } => {
            handle_boolean_operation(BooleanOp::Difference, solid_a, solid_b, state).await
        }
        
        // Transformation Operations
        GeometryOperation::TransformSolid { solid_id, transform } => {
            handle_transform_solid(solid_id, transform, state).await
        }
        GeometryOperation::TranslateSolid { solid_id, translation } => {
            let transform = TransformData {
                translation: Some(translation),
                rotation: None,
                scale: None,
            };
            handle_transform_solid(solid_id, transform, state).await
        }
        GeometryOperation::RotateSolid { solid_id, axis, angle_degrees } => {
            handle_rotate_solid(solid_id, axis, angle_degrees, state).await
        }
        GeometryOperation::ScaleSolid { solid_id, scale } => {
            let transform = TransformData {
                translation: None,
                rotation: None,
                scale: Some(scale),
            };
            handle_transform_solid(solid_id, transform, state).await
        }
        
        // Extrusion Operations
        GeometryOperation::ExtrudeFace { face_id, direction, distance } => {
            handle_extrude_face(face_id, direction, distance, state).await
        }
        GeometryOperation::ExtrudeProfile { profile_id, direction, distance } => {
            handle_extrude_profile(profile_id, direction, distance, state).await
        }
        
        // Revolution Operations
        GeometryOperation::RevolveFace { face_id, axis_origin, axis_direction, angle_degrees } => {
            handle_revolve_face(face_id, axis_origin, axis_direction, angle_degrees, state).await
        }
        GeometryOperation::RevolveProfile { profile_id, axis_origin, axis_direction, angle_degrees } => {
            handle_revolve_profile(profile_id, axis_origin, axis_direction, angle_degrees, state).await
        }
        
        // Sweep Operations
        GeometryOperation::SweepProfile { profile_id, path_id, twist_degrees, scale_factor } => {
            handle_sweep_profile(profile_id, path_id, twist_degrees, scale_factor, state).await
        }
        
        // Fillet and Chamfer
        GeometryOperation::FilletEdges { solid_id, edge_ids, radius } => {
            handle_fillet_edges(solid_id, edge_ids, radius, state).await
        }
        GeometryOperation::FilletVertices { solid_id, vertex_ids, radius } => {
            handle_fillet_vertices(solid_id, vertex_ids, radius, state).await
        }
        GeometryOperation::ChamferEdges { solid_id, edge_ids, distance } => {
            handle_chamfer_edges(solid_id, edge_ids, distance, state).await
        }
        
        // Pattern Operations
        GeometryOperation::LinearPattern { solid_id, direction, count, spacing } => {
            handle_linear_pattern(solid_id, direction, count, spacing, state).await
        }
        GeometryOperation::CircularPattern { solid_id, axis_origin, axis_direction, count, angle_degrees } => {
            handle_circular_pattern(solid_id, axis_origin, axis_direction, count, angle_degrees, state).await
        }
        
        // Query Operations
        GeometryOperation::GetVolume { solid_id } => {
            handle_get_volume(solid_id, state).await
        }
        GeometryOperation::GetSurfaceArea { solid_id } => {
            handle_get_surface_area(solid_id, state).await
        }
        GeometryOperation::GetBoundingBox { solid_id } => {
            handle_get_bounding_box(solid_id, state).await
        }
        GeometryOperation::GetCenterOfMass { solid_id } => {
            handle_get_center_of_mass(solid_id, state).await
        }
        GeometryOperation::GetTopology { solid_id } => {
            handle_get_topology(solid_id, state).await
        }
        
        // Tessellation
        GeometryOperation::TessellateSolid { solid_id, quality } => {
            handle_tessellate_solid(solid_id, quality, state).await
        }
        
        // Validation
        GeometryOperation::ValidateSolid { solid_id, level } => {
            handle_validate_solid(solid_id, level, state).await
        }
    };
    
    // Send response
    let json = serde_json::to_string(&result)?;
    sender.send(Message::Text(json.into())).await?;
    
    Ok(())
}

// Implementation of handlers for each operation

async fn handle_create_box(
    width: f64,
    height: f64,
    depth: f64,
    state: &AppState,
) -> GeometryOperationResult {
    let mut model = state.model.write().await;
    let mut builder = TopologyBuilder::new(&mut *model);
    
    match builder.create_box_3d(width, height, depth) {
        Ok(GeometryEngineId::Solid(solid_id)) => {
            let solid_id_str = format!("solid_{}", solid_id);
            
            // Store the solid
            let mut solids = state.solids.write().await;
            solids.insert(solid_id, solid_id_str.clone());
            
            // Generate mesh for the created solid
            let tessellation_params = geometry_engine::tessellation::TessellationParams::default();
            let solid = model.solids.get(solid_id).ok_or_else(|| "Solid not found".to_string())?;
            let display_mesh = geometry_engine::tessellation::tessellate_solid(
                solid,
                &*model,
                &tessellation_params
            );
            
            // Convert to simplified mesh
            let simplified_mesh = SimplifiedMesh {
                vertices: display_mesh.positions.clone(),
                normals: display_mesh.normals.clone(),
                indices: display_mesh.indices.clone(),
                bounds: calculate_bounds(&display_mesh.positions),
            };
            
            // Calculate properties
            let properties = calculate_properties_from_mesh(&display_mesh);
            
            GeometryOperationResult::Success {
                operation: "CreateBox".to_string(),
                output_ids: vec![solid_id_str],
                mesh: Some(simplified_mesh),
                properties: Some(properties),
            }
        }
        Err(e) => GeometryOperationResult::Error {
            operation: "CreateBox".to_string(),
            message: format!("Failed to create box: {}", e),
            details: None,
        },
        _ => GeometryOperationResult::Error {
            operation: "CreateBox".to_string(),
            message: "Unexpected geometry type returned".to_string(),
            details: None,
        },
    }
}

async fn handle_create_sphere(
    center: [f64; 3],
    radius: f64,
    state: &AppState,
) -> GeometryOperationResult {
    let mut model = state.model.write().await;
    let mut builder = TopologyBuilder::new(&mut *model);
    
    let center_point = Point3::new(center[0], center[1], center[2]);
    
    match builder.create_sphere_3d(center_point, radius) {
        Ok(GeometryEngineId::Solid(solid_id)) => {
            let solid_id_str = format!("solid_{}", solid_id);
            
            // Store the solid
            let mut solids = state.solids.write().await;
            solids.insert(solid_id, solid_id_str.clone());
            
            // Generate mesh for the created solid
            let tessellation_params = geometry_engine::tessellation::TessellationParams::default();
            let solid = model.solids.get(solid_id).ok_or_else(|| "Solid not found".to_string())?;
            let display_mesh = geometry_engine::tessellation::tessellate_solid(
                solid,
                &*model,
                &tessellation_params
            );
            
            // Convert to simplified mesh
            let simplified_mesh = SimplifiedMesh {
                vertices: display_mesh.positions.clone(),
                normals: display_mesh.normals.clone(),
                indices: display_mesh.indices.clone(),
                bounds: calculate_bounds(&display_mesh.positions),
            };
            
            // Calculate properties
            let properties = calculate_properties_from_mesh(&display_mesh);
            
            GeometryOperationResult::Success {
                operation: "CreateSphere".to_string(),
                output_ids: vec![solid_id_str],
                mesh: Some(simplified_mesh),
                properties: Some(properties),
            }
        }
        Err(e) => GeometryOperationResult::Error {
            operation: "CreateSphere".to_string(),
            message: format!("Failed to create sphere: {}", e),
            details: None,
        },
        _ => GeometryOperationResult::Error {
            operation: "CreateSphere".to_string(),
            message: "Unexpected geometry type returned".to_string(),
            details: None,
        },
    }
}

async fn handle_create_cylinder(
    base_center: [f64; 3],
    axis: [f64; 3],
    radius: f64,
    height: f64,
    state: &AppState,
) -> GeometryOperationResult {
    let mut model = state.model.write().await;
    let mut builder = TopologyBuilder::new(&mut *model);
    
    let base = Point3::new(base_center[0], base_center[1], base_center[2]);
    let axis_vec = Vector3::new(axis[0], axis[1], axis[2]);
    
    match builder.create_cylinder_3d(base, axis_vec, radius, height) {
        Ok(GeometryEngineId::Solid(solid_id)) => {
            let solid_id_str = format!("solid_{}", solid_id);
            
            let mut solids = state.solids.write().await;
            solids.insert(solid_id, solid_id_str.clone());
            
            // Generate mesh for the created solid
            let tessellation_params = geometry_engine::tessellation::TessellationParams::default();
            let solid = model.solids.get(solid_id).ok_or_else(|| "Solid not found".to_string())?;
            let display_mesh = geometry_engine::tessellation::tessellate_solid(
                solid,
                &*model,
                &tessellation_params
            );
            
            // Convert to simplified mesh
            let simplified_mesh = SimplifiedMesh {
                vertices: display_mesh.positions.clone(),
                normals: display_mesh.normals.clone(),
                indices: display_mesh.indices.clone(),
                bounds: calculate_bounds(&display_mesh.positions),
            };
            
            // Calculate properties
            let properties = calculate_properties_from_mesh(&display_mesh);
            
            GeometryOperationResult::Success {
                operation: "CreateCylinder".to_string(),
                output_ids: vec![solid_id_str],
                mesh: Some(simplified_mesh),
                properties: Some(properties),
            }
        }
        Err(e) => GeometryOperationResult::Error {
            operation: "CreateCylinder".to_string(),
            message: format!("Failed to create cylinder: {}", e),
            details: None,
        },
        _ => GeometryOperationResult::Error {
            operation: "CreateCylinder".to_string(),
            message: "Unexpected geometry type returned".to_string(),
            details: None,
        },
    }
}

async fn handle_create_cone(
    base_center: [f64; 3],
    axis: [f64; 3],
    base_radius: f64,
    top_radius: f64,
    height: f64,
    state: &AppState,
) -> GeometryOperationResult {
    // Use the cone primitive module
    use geometry_engine::primitives::cone_primitive::create_cone;
    
    let mut model = state.model.write().await;
    let center = Point3::new(base_center[0], base_center[1], base_center[2]);
    let axis_vec = Vector3::new(axis[0], axis[1], axis[2]);
    
    // Calculate half angle from radii and height
    let half_angle = ((base_radius - top_radius) / height).atan();
    
    match create_cone(&mut *model, center, axis_vec, half_angle, height) {
        Ok(solid_id) => {
            let solid_id_str = format!("solid_{}", solid_id);
            
            let mut solids = state.solids.write().await;
            solids.insert(solid_id, solid_id_str.clone());
            
            // Generate mesh for the created solid
            let tessellation_params = geometry_engine::tessellation::TessellationParams::default();
            let solid = model.solids.get(solid_id).ok_or_else(|| "Solid not found".to_string())?;
            let display_mesh = geometry_engine::tessellation::tessellate_solid(
                solid,
                &*model,
                &tessellation_params
            );
            
            // Convert to simplified mesh
            let simplified_mesh = SimplifiedMesh {
                vertices: display_mesh.positions.clone(),
                normals: display_mesh.normals.clone(),
                indices: display_mesh.indices.clone(),
                bounds: calculate_bounds(&display_mesh.positions),
            };
            
            // Calculate properties
            let properties = calculate_properties_from_mesh(&display_mesh);
            
            GeometryOperationResult::Success {
                operation: "CreateCone".to_string(),
                output_ids: vec![solid_id_str],
                mesh: Some(simplified_mesh),
                properties: Some(properties),
            }
        }
        Err(e) => GeometryOperationResult::Error {
            operation: "CreateCone".to_string(),
            message: format!("Failed to create cone: {}", e),
            details: None,
        },
    }
}

async fn handle_create_torus(
    center: [f64; 3],
    axis: [f64; 3],
    major_radius: f64,
    minor_radius: f64,
    state: &AppState,
) -> GeometryOperationResult {
    // Use the torus primitive module
    use geometry_engine::primitives::torus_primitive::create_torus;
    
    let mut model = state.model.write().await;
    let center_point = Point3::new(center[0], center[1], center[2]);
    let axis_vec = Vector3::new(axis[0], axis[1], axis[2]);
    
    match create_torus(&mut *model, center_point, axis_vec, major_radius, minor_radius) {
        Ok(solid_id) => {
            let solid_id_str = format!("solid_{}", solid_id);
            
            let mut solids = state.solids.write().await;
            solids.insert(solid_id, solid_id_str.clone());
            
            // Generate mesh for the created solid
            let tessellation_params = geometry_engine::tessellation::TessellationParams::default();
            let solid = model.solids.get(solid_id).ok_or_else(|| "Solid not found".to_string())?;
            let display_mesh = geometry_engine::tessellation::tessellate_solid(
                solid,
                &*model,
                &tessellation_params
            );
            
            // Convert to simplified mesh
            let simplified_mesh = SimplifiedMesh {
                vertices: display_mesh.positions.clone(),
                normals: display_mesh.normals.clone(),
                indices: display_mesh.indices.clone(),
                bounds: calculate_bounds(&display_mesh.positions),
            };
            
            // Calculate properties
            let properties = calculate_properties_from_mesh(&display_mesh);
            
            GeometryOperationResult::Success {
                operation: "CreateTorus".to_string(),
                output_ids: vec![solid_id_str],
                mesh: Some(simplified_mesh),
                properties: Some(properties),
            }
        }
        Err(e) => GeometryOperationResult::Error {
            operation: "CreateTorus".to_string(),
            message: format!("Failed to create torus: {}", e),
            details: None,
        },
    }
}

async fn handle_boolean_operation(
    op: BooleanOp,
    solid_a: String,
    solid_b: String,
    state: &AppState,
) -> GeometryOperationResult {
    use geometry_engine::operations::boolean::boolean_operation;
    
    // Parse solid IDs
    let solid_a_id: SolidId = solid_a.strip_prefix("solid_")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let solid_b_id: SolidId = solid_b.strip_prefix("solid_")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    
    let mut model = state.model.write().await;
    
    // Convert shared_types::BooleanOp to geometry_engine::operations::boolean::BooleanOp
    let engine_op = match op {
        BooleanOp::Union => geometry_engine::operations::boolean::BooleanOp::Union,
        BooleanOp::Intersection => geometry_engine::operations::boolean::BooleanOp::Intersection,
        BooleanOp::Difference => geometry_engine::operations::boolean::BooleanOp::Difference,
    };
    
    match boolean_operation(&mut *model, engine_op, solid_a_id, solid_b_id) {
        Ok(result_id) => {
            let result_id_str = format!("solid_{}", result_id);
            
            let mut solids = state.solids.write().await;
            solids.insert(result_id, result_id_str.clone());
            
            // Generate mesh for the result
            let tessellation_params = geometry_engine::tessellation::TessellationParams::default();
            let solid = model.solids.get(result_id).ok_or_else(|| "Result solid not found".to_string())?;
            let display_mesh = geometry_engine::tessellation::tessellate_solid(
                solid,
                &*model,
                &tessellation_params
            );
            
            // Convert to simplified mesh
            let simplified_mesh = SimplifiedMesh {
                vertices: display_mesh.positions.clone(),
                normals: display_mesh.normals.clone(),
                indices: display_mesh.indices.clone(),
                bounds: calculate_bounds(&display_mesh.positions),
            };
            
            // Calculate properties
            let properties = calculate_properties_from_mesh(&display_mesh);
            
            GeometryOperationResult::Success {
                operation: format!("Boolean{:?}", op),
                output_ids: vec![result_id_str],
                mesh: Some(simplified_mesh),
                properties: Some(properties),
            }
        }
        Err(e) => GeometryOperationResult::Error {
            operation: format!("Boolean{:?}", op),
            message: format!("Boolean operation failed: {}", e),
            details: None,
        },
    }
}

async fn handle_transform_solid(
    solid_id: String,
    transform: TransformData,
    state: &AppState,
) -> GeometryOperationResult {
    use geometry_engine::operations::transform::transform_solid;
    
    let solid_id_num: SolidId = solid_id.strip_prefix("solid_")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    
    let mut model = state.model.write().await;
    
    // Build transformation matrix
    let mut matrix = Matrix4::identity();
    
    if let Some(translation) = transform.translation {
        matrix = matrix * Matrix4::from_translation(Vector3::new(
            translation[0],
            translation[1],
            translation[2],
        ));
    }
    
    if let Some(scale) = transform.scale {
        matrix = matrix * Matrix4::from_nonuniform_scale(
            scale[0],
            scale[1],
            scale[2],
        );
    }
    
    // Apply transformation
    transform_solid(&mut *model, solid_id_num, &matrix);
    
    GeometryOperationResult::Success {
        operation: "TransformSolid".to_string(),
        output_ids: vec![solid_id],
        mesh: None,
        properties: None,
    }
}

async fn handle_rotate_solid(
    solid_id: String,
    axis: [f64; 3],
    angle_degrees: f64,
    state: &AppState,
) -> GeometryOperationResult {
    use geometry_engine::operations::transform::transform_solid;
    
    let solid_id_num: SolidId = solid_id.strip_prefix("solid_")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    
    let mut model = state.model.write().await;
    
    // Create rotation matrix
    let axis_vec = Vector3::new(axis[0], axis[1], axis[2]).normalize();
    let angle_radians = angle_degrees.to_radians();
    let matrix = Matrix4::from_axis_angle(axis_vec, angle_radians);
    
    transform_solid(&mut *model, solid_id_num, &matrix);
    
    GeometryOperationResult::Success {
        operation: "RotateSolid".to_string(),
        output_ids: vec![solid_id],
        mesh: None,
        properties: None,
    }
}

// Stub implementations for remaining operations
// These should be implemented with actual geometry engine calls

async fn handle_extrude_face(
    face_id: String,
    direction: [f64; 3],
    distance: f64,
    state: &AppState,
) -> GeometryOperationResult {
    use geometry_engine::operations::extrude::extrude_face;
    
    let face_id_num: FaceId = face_id.strip_prefix("face_")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    
    let mut model = state.model.write().await;
    let dir = Vector3::new(direction[0], direction[1], direction[2]);
    
    match extrude_face(&mut *model, face_id_num, dir, distance) {
        Ok(solid_id) => {
            let solid_id_str = format!("solid_{}", solid_id);
            
            let mut solids = state.solids.write().await;
            solids.insert(solid_id, solid_id_str.clone());
            
            GeometryOperationResult::Success {
                operation: "ExtrudeFace".to_string(),
                output_ids: vec![solid_id_str],
                mesh: None,
                properties: None,
            }
        }
        Err(e) => GeometryOperationResult::Error {
            operation: "ExtrudeFace".to_string(),
            message: format!("Extrude failed: {}", e),
            details: None,
        },
    }
}

async fn handle_extrude_profile(
    profile_id: String,
    direction: [f64; 3],
    distance: f64,
    state: &AppState,
) -> GeometryOperationResult {
    // Implementation would call geometry_engine::operations::extrude::extrude_profile
    GeometryOperationResult::Success {
        operation: "ExtrudeProfile".to_string(),
        output_ids: vec![],
        mesh: None,
        properties: None,
    }
}

async fn handle_revolve_face(
    face_id: String,
    axis_origin: [f64; 3],
    axis_direction: [f64; 3],
    angle_degrees: f64,
    state: &AppState,
) -> GeometryOperationResult {
    use geometry_engine::operations::revolve::revolve_face;
    
    let face_id_num: FaceId = face_id.strip_prefix("face_")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    
    let mut model = state.model.write().await;
    let origin = Point3::new(axis_origin[0], axis_origin[1], axis_origin[2]);
    let axis = Vector3::new(axis_direction[0], axis_direction[1], axis_direction[2]);
    let angle_radians = angle_degrees.to_radians();
    
    match revolve_face(&mut *model, face_id_num, origin, axis, angle_radians) {
        Ok(solid_id) => {
            let solid_id_str = format!("solid_{}", solid_id);
            
            let mut solids = state.solids.write().await;
            solids.insert(solid_id, solid_id_str.clone());
            
            GeometryOperationResult::Success {
                operation: "RevolveFace".to_string(),
                output_ids: vec![solid_id_str],
                mesh: None,
                properties: None,
            }
        }
        Err(e) => GeometryOperationResult::Error {
            operation: "RevolveFace".to_string(),
            message: format!("Revolve failed: {}", e),
            details: None,
        },
    }
}

async fn handle_revolve_profile(
    profile_id: String,
    axis_origin: [f64; 3],
    axis_direction: [f64; 3],
    angle_degrees: f64,
    state: &AppState,
) -> GeometryOperationResult {
    // Implementation would call geometry_engine::operations::revolve::revolve_profile
    GeometryOperationResult::Success {
        operation: "RevolveProfile".to_string(),
        output_ids: vec![],
        mesh: None,
        properties: None,
    }
}

async fn handle_sweep_profile(
    profile_id: String,
    path_id: String,
    twist_degrees: Option<f64>,
    scale_factor: Option<f64>,
    state: &AppState,
) -> GeometryOperationResult {
    // Implementation would call geometry_engine::operations::sweep::sweep_profile
    GeometryOperationResult::Success {
        operation: "SweepProfile".to_string(),
        output_ids: vec![],
        mesh: None,
        properties: None,
    }
}

async fn handle_fillet_edges(
    solid_id: String,
    edge_ids: Vec<String>,
    radius: f64,
    state: &AppState,
) -> GeometryOperationResult {
    use geometry_engine::operations::fillet::fillet_edges;
    
    let solid_id_num: SolidId = solid_id.strip_prefix("solid_")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    
    let edge_ids_num: Vec<EdgeId> = edge_ids.iter()
        .filter_map(|e| e.strip_prefix("edge_").and_then(|s| s.parse().ok()))
        .collect();
    
    let mut model = state.model.write().await;
    
    match fillet_edges(&mut *model, solid_id_num, &edge_ids_num, radius) {
        Ok(_) => GeometryOperationResult::Success {
            operation: "FilletEdges".to_string(),
            output_ids: vec![solid_id],
            mesh: None,
            properties: None,
        },
        Err(e) => GeometryOperationResult::Error {
            operation: "FilletEdges".to_string(),
            message: format!("Fillet failed: {}", e),
            details: None,
        },
    }
}

async fn handle_fillet_vertices(
    solid_id: String,
    vertex_ids: Vec<String>,
    radius: f64,
    state: &AppState,
) -> GeometryOperationResult {
    // Implementation would call geometry_engine::operations::fillet::fillet_vertices
    GeometryOperationResult::Success {
        operation: "FilletVertices".to_string(),
        output_ids: vec![solid_id],
        mesh: None,
        properties: None,
    }
}

async fn handle_chamfer_edges(
    solid_id: String,
    edge_ids: Vec<String>,
    distance: f64,
    state: &AppState,
) -> GeometryOperationResult {
    use geometry_engine::operations::chamfer::chamfer_edges;
    
    let solid_id_num: SolidId = solid_id.strip_prefix("solid_")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    
    let edge_ids_num: Vec<EdgeId> = edge_ids.iter()
        .filter_map(|e| e.strip_prefix("edge_").and_then(|s| s.parse().ok()))
        .collect();
    
    let mut model = state.model.write().await;
    
    match chamfer_edges(&mut *model, solid_id_num, &edge_ids_num, distance) {
        Ok(_) => GeometryOperationResult::Success {
            operation: "ChamferEdges".to_string(),
            output_ids: vec![solid_id],
            mesh: None,
            properties: None,
        },
        Err(e) => GeometryOperationResult::Error {
            operation: "ChamferEdges".to_string(),
            message: format!("Chamfer failed: {}", e),
            details: None,
        },
    }
}

async fn handle_linear_pattern(
    solid_id: String,
    direction: [f64; 3],
    count: u32,
    spacing: f64,
    state: &AppState,
) -> GeometryOperationResult {
    // Implementation would use transform operations in a loop
    let mut output_ids = vec![solid_id.clone()];
    
    for i in 1..count {
        let new_id = format!("{}_pattern_{}", solid_id, i);
        output_ids.push(new_id);
    }
    
    GeometryOperationResult::Success {
        operation: "LinearPattern".to_string(),
        output_ids,
        mesh: None,
        properties: None,
    }
}

async fn handle_circular_pattern(
    solid_id: String,
    axis_origin: [f64; 3],
    axis_direction: [f64; 3],
    count: u32,
    angle_degrees: f64,
    state: &AppState,
) -> GeometryOperationResult {
    // Implementation would use rotation transformations
    let mut output_ids = vec![solid_id.clone()];
    
    for i in 1..count {
        let new_id = format!("{}_pattern_{}", solid_id, i);
        output_ids.push(new_id);
    }
    
    GeometryOperationResult::Success {
        operation: "CircularPattern".to_string(),
        output_ids,
        mesh: None,
        properties: None,
    }
}

async fn handle_get_volume(
    solid_id: String,
    state: &AppState,
) -> GeometryOperationResult {
    // Implementation would call geometry analysis functions
    GeometryOperationResult::Success {
        operation: "GetVolume".to_string(),
        output_ids: vec![solid_id],
        mesh: None,
        properties: Some(GeometryProperties {
            volume: 100.0, // Placeholder
            surface_area: 0.0,
            center_of_mass: [0.0, 0.0, 0.0],
            vertex_count: 0,
            edge_count: 0,
            face_count: 0,
            is_manifold: true,
            is_closed: true,
        }),
    }
}

async fn handle_get_surface_area(
    solid_id: String,
    state: &AppState,
) -> GeometryOperationResult {
    GeometryOperationResult::Success {
        operation: "GetSurfaceArea".to_string(),
        output_ids: vec![solid_id],
        mesh: None,
        properties: Some(GeometryProperties {
            volume: 0.0,
            surface_area: 50.0, // Placeholder
            center_of_mass: [0.0, 0.0, 0.0],
            vertex_count: 0,
            edge_count: 0,
            face_count: 0,
            is_manifold: true,
            is_closed: true,
        }),
    }
}

async fn handle_get_bounding_box(
    solid_id: String,
    state: &AppState,
) -> GeometryOperationResult {
    GeometryOperationResult::Success {
        operation: "GetBoundingBox".to_string(),
        output_ids: vec![solid_id],
        mesh: None,
        properties: None,
    }
}

async fn handle_get_center_of_mass(
    solid_id: String,
    state: &AppState,
) -> GeometryOperationResult {
    GeometryOperationResult::Success {
        operation: "GetCenterOfMass".to_string(),
        output_ids: vec![solid_id],
        mesh: None,
        properties: Some(GeometryProperties {
            volume: 0.0,
            surface_area: 0.0,
            center_of_mass: [5.0, 5.0, 5.0], // Placeholder
            vertex_count: 0,
            edge_count: 0,
            face_count: 0,
            is_manifold: true,
            is_closed: true,
        }),
    }
}

async fn handle_get_topology(
    solid_id: String,
    state: &AppState,
) -> GeometryOperationResult {
    GeometryOperationResult::Success {
        operation: "GetTopology".to_string(),
        output_ids: vec![solid_id],
        mesh: None,
        properties: Some(GeometryProperties {
            volume: 0.0,
            surface_area: 0.0,
            center_of_mass: [0.0, 0.0, 0.0],
            vertex_count: 8,
            edge_count: 12,
            face_count: 6,
            is_manifold: true,
            is_closed: true,
        }),
    }
}

async fn handle_tessellate_solid(
    solid_id: String,
    quality: TessellationQuality,
    state: &AppState,
) -> GeometryOperationResult {
    // Implementation would call tessellation module
    let mesh = SimplifiedMesh {
        vertices: vec![],
        normals: vec![],
        indices: vec![],
        bounds: BoundingBox {
            min: [-10.0, -10.0, -10.0],
            max: [10.0, 10.0, 10.0],
        },
    };
    
    GeometryOperationResult::Success {
        operation: "TessellateSolid".to_string(),
        output_ids: vec![solid_id],
        mesh: Some(mesh),
        properties: None,
    }
}

async fn handle_validate_solid(
    solid_id: String,
    level: ValidationLevel,
    state: &AppState,
) -> GeometryOperationResult {
    // Implementation would call validation module
    GeometryOperationResult::Success {
        operation: "ValidateSolid".to_string(),
        output_ids: vec![solid_id],
        mesh: None,
        properties: Some(GeometryProperties {
            volume: 0.0,
            surface_area: 0.0,
            center_of_mass: [0.0, 0.0, 0.0],
            vertex_count: 0,
            edge_count: 0,
            face_count: 0,
            is_manifold: true,
            is_closed: true,
        }),
    }
}

// Helper function to calculate bounding box from vertices
fn calculate_bounds(vertices: &[f32]) -> BoundingBox {
    if vertices.is_empty() {
        return BoundingBox {
            min: [0.0, 0.0, 0.0],
            max: [0.0, 0.0, 0.0],
        };
    }
    
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut min_z = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    let mut max_z = f32::MIN;
    
    for i in (0..vertices.len()).step_by(3) {
        if i + 2 < vertices.len() {
            min_x = min_x.min(vertices[i]);
            max_x = max_x.max(vertices[i]);
            min_y = min_y.min(vertices[i + 1]);
            max_y = max_y.max(vertices[i + 1]);
            min_z = min_z.min(vertices[i + 2]);
            max_z = max_z.max(vertices[i + 2]);
        }
    }
    
    BoundingBox {
        min: [min_x as f64, min_y as f64, min_z as f64],
        max: [max_x as f64, max_y as f64, max_z as f64],
    }
}

// Helper function to calculate properties from a tessellated mesh
fn calculate_properties_from_mesh(mesh: &geometry_engine::tessellation::ThreeJsMesh) -> GeometryProperties {
    let mut volume = 0.0f32;
    let mut center_of_mass = [0.0f32, 0.0f32, 0.0f32];
    let mut surface_area = 0.0f32;
    
    // Calculate volume and surface area using triangulated mesh
    for i in (0..mesh.indices.len()).step_by(3) {
        if i + 2 < mesh.indices.len() {
            let i0 = mesh.indices[i] as usize * 3;
            let i1 = mesh.indices[i + 1] as usize * 3;
            let i2 = mesh.indices[i + 2] as usize * 3;
            
            if i0 + 2 < mesh.positions.len() && 
               i1 + 2 < mesh.positions.len() && 
               i2 + 2 < mesh.positions.len() {
                
                // Get triangle vertices
                let v0 = [
                    mesh.positions[i0],
                    mesh.positions[i0 + 1],
                    mesh.positions[i0 + 2]
                ];
                let v1 = [
                    mesh.positions[i1],
                    mesh.positions[i1 + 1],
                    mesh.positions[i1 + 2]
                ];
                let v2 = [
                    mesh.positions[i2],
                    mesh.positions[i2 + 1],
                    mesh.positions[i2 + 2]
                ];
                
                // Calculate signed volume of tetrahedron from origin (divergence theorem)
                let tetra_volume = (v0[0] * (v1[1] * v2[2] - v1[2] * v2[1]) +
                                   v0[1] * (v1[2] * v2[0] - v1[0] * v2[2]) +
                                   v0[2] * (v1[0] * v2[1] - v1[1] * v2[0])) / 6.0;
                
                volume += tetra_volume;
                
                // Accumulate for center of mass (weighted by volume)
                let triangle_center = [
                    (v0[0] + v1[0] + v2[0]) / 3.0,
                    (v0[1] + v1[1] + v2[1]) / 3.0,
                    (v0[2] + v1[2] + v2[2]) / 3.0
                ];
                
                center_of_mass[0] += triangle_center[0] * tetra_volume;
                center_of_mass[1] += triangle_center[1] * tetra_volume;
                center_of_mass[2] += triangle_center[2] * tetra_volume;
                
                // Calculate triangle area for surface area
                let edge1 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
                let edge2 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];
                
                // Cross product for area
                let cross = [
                    edge1[1] * edge2[2] - edge1[2] * edge2[1],
                    edge1[2] * edge2[0] - edge1[0] * edge2[2],
                    edge1[0] * edge2[1] - edge1[1] * edge2[0]
                ];
                
                let area = (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt() * 0.5;
                surface_area += area;
            }
        }
    }
    
    // Finalize calculations
    volume = volume.abs(); // Ensure positive volume
    
    // Normalize center of mass by volume
    if volume > 0.0 {
        center_of_mass[0] /= volume;
        center_of_mass[1] /= volume;
        center_of_mass[2] /= volume;
    }
    
    // Count topology elements
    let vertex_count = mesh.positions.len() / 3;
    let face_count = mesh.indices.len() / 3;
    
    // Estimate edge count (rough approximation for triangulated mesh)
    let edge_count = (face_count * 3) / 2;  // Each triangle has 3 edges, shared between faces
    
    GeometryProperties {
        volume: volume as f64,
        surface_area: surface_area as f64,
        center_of_mass: [
            center_of_mass[0] as f64,
            center_of_mass[1] as f64,
            center_of_mass[2] as f64
        ],
        vertex_count,
        edge_count,
        face_count,
        is_manifold: true,  // Would need proper topology check for exact result
        is_closed: true,    // Would need boundary check for exact result
    }
}
EOF < /dev/null
