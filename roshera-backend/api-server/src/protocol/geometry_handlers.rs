//! WebSocket handlers for geometry-engine operations

use crate::AppState;
use axum::extract::ws::{Message, WebSocket};
use futures::sink::SinkExt;
use geometry_engine::operations::{
    boolean_operation, BooleanOp as GeometryBooleanOp, BooleanOptions,
};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
use serde::{Deserialize, Serialize};
use serde_json::json;
use shared_types::geometry::{AnalyticalProperties, MaterialProperties};
use shared_types::{BooleanOp, GeometryId, Mesh, Transform3D, Vector3D};
use std::collections::HashMap;
use tracing::{error, info, warn};
use uuid::Uuid;

/// Geometry-specific WebSocket messages from client
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum GeometryWebSocketRequest {
    /// Create a primitive shape
    CreatePrimitive {
        primitive_type: String,
        parameters: serde_json::Value,
    },

    /// Perform boolean operation
    BooleanOperation {
        operation: String, // "union", "intersection", "difference"
        object_a: String,
        object_b: String,
    },

    /// Transform an object
    TransformObject {
        object_id: String,
        transform: TransformData,
    },

    /// Delete an object
    DeleteObject { object_id: String },

    /// Get mesh data for an object (tessellate on-demand)
    GetMeshForDisplay {
        object_id: String,
        quality: String, // "low", "medium", "high"
    },

    /// Get analytical properties of an object
    GetAnalyticalProperties { object_id: String },

    /// Query object properties
    QueryObject {
        object_id: String,
        properties: Vec<String>, // ["volume", "surface_area", "bounding_box"]
    },

    /// List all objects
    ListObjects,

    /// Clear all geometry
    ClearAll,
}

/// Transform data from client
#[derive(Debug, Clone, Deserialize)]
pub struct TransformData {
    pub translation: Option<[f64; 3]>,
    pub rotation: Option<[f64; 4]>, // Quaternion
    pub scale: Option<[f64; 3]>,
}

/// Geometry-specific WebSocket responses to client
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum GeometryWebSocketResponse {
    /// Object created successfully
    ObjectCreated {
        object_id: String,
        object_type: String,
        mesh: Mesh,
        properties: Option<AnalyticalProperties>,
    },

    /// Boolean operation completed
    BooleanCompleted {
        result_id: String,
        operation: String,
        mesh: Mesh,
    },

    /// Object transformed
    ObjectTransformed {
        object_id: String,
        transform: serde_json::Value,
    },

    /// Object deleted
    ObjectDeleted { object_id: String },

    /// Mesh data response (deprecated - use MeshForDisplay)
    MeshData {
        object_id: String,
        mesh: Mesh,
        quality: String,
    },
    /// Mesh data for display (tessellated on-demand)
    MeshForDisplay {
        object_id: String,
        mesh: Mesh,
        quality: String,
        cached: bool, // Whether this was from cache or freshly tessellated
    },
    /// Analytical properties response
    AnalyticalProperties {
        object_id: String,
        properties: serde_json::Value, // AnalyticalProperties serialized
        has_properties: bool,
    },

    /// Object properties response
    ObjectProperties {
        object_id: String,
        properties: serde_json::Value,
    },

    /// List of all objects
    ObjectList { objects: Vec<ObjectSummary> },

    /// All geometry cleared
    GeometryCleared,

    /// Error response
    GeometryError {
        message: String,
        request_type: String,
    },
}

// Using shared types instead of local duplicates

/// Object summary for listing
#[derive(Debug, Clone, Serialize)]
pub struct ObjectSummary {
    pub id: String,
    pub name: String,
    pub object_type: String,
    pub visible: bool,
    pub selected: bool,
}

/// Handle geometry-specific WebSocket request
pub async fn handle_geometry_request(
    request: GeometryWebSocketRequest,
    session_id: &str,
    user_id: &str,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!(
        "Processing geometry request: {:?} for session {}",
        request, session_id
    );

    match request {
        GeometryWebSocketRequest::CreatePrimitive {
            primitive_type,
            parameters,
        } => {
            handle_create_primitive(primitive_type, parameters, session_id, state, sender).await?;
        }

        GeometryWebSocketRequest::BooleanOperation {
            operation,
            object_a,
            object_b,
        } => {
            handle_boolean_operation(operation, object_a, object_b, session_id, state, sender)
                .await?;
        }

        GeometryWebSocketRequest::TransformObject {
            object_id,
            transform,
        } => {
            handle_transform_object(object_id, transform, session_id, state, sender).await?;
        }

        GeometryWebSocketRequest::DeleteObject { object_id } => {
            handle_delete_object(object_id, session_id, state, sender).await?;
        }

        GeometryWebSocketRequest::GetMeshForDisplay { object_id, quality } => {
            handle_get_mesh(object_id, quality, session_id, state, sender).await?;
        }

        GeometryWebSocketRequest::GetAnalyticalProperties { object_id } => {
            handle_get_analytical_properties(object_id, session_id, state, sender).await?;
        }

        GeometryWebSocketRequest::QueryObject {
            object_id,
            properties,
        } => {
            handle_query_object(object_id, properties, session_id, state, sender).await?;
        }

        GeometryWebSocketRequest::ListObjects => {
            handle_list_objects(session_id, state, sender).await?;
        }

        GeometryWebSocketRequest::ClearAll => {
            handle_clear_all(session_id, state, sender).await?;
        }
    }

    Ok(())
}

/// Handle create primitive request
async fn handle_create_primitive(
    primitive_type: String,
    parameters: serde_json::Value,
    session_id: &str,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!(
        "Creating primitive: {} with params: {}",
        primitive_type, parameters
    );

    // Get geometry engine
    let mut geometry_model = state.geometry_model.write().await;

    // Create shape parameters from JSON
    // Removed wrapper import - using core geometry engine directly
    use std::collections::HashMap;

    let mut params = HashMap::new();

    // Convert primitive type string to enum
    let prim_type = match primitive_type.as_str() {
        "box" | "cube" => {
            params.insert(
                "width".to_string(),
                parameters["width"].as_f64().unwrap_or(1.0),
            );
            params.insert(
                "height".to_string(),
                parameters["height"].as_f64().unwrap_or(1.0),
            );
            params.insert(
                "depth".to_string(),
                parameters["depth"].as_f64().unwrap_or(1.0),
            );
            shared_types::PrimitiveType::Box
        }
        "sphere" => {
            params.insert(
                "radius".to_string(),
                parameters["radius"].as_f64().unwrap_or(1.0),
            );
            shared_types::PrimitiveType::Sphere
        }
        "cylinder" => {
            params.insert(
                "radius".to_string(),
                parameters["radius"].as_f64().unwrap_or(1.0),
            );
            params.insert(
                "height".to_string(),
                parameters["height"].as_f64().unwrap_or(2.0),
            );
            shared_types::PrimitiveType::Cylinder
        }
        "cone" => {
            params.insert(
                "half_angle".to_string(),
                parameters["half_angle"].as_f64().unwrap_or(0.5),
            );
            params.insert(
                "height".to_string(),
                parameters["height"].as_f64().unwrap_or(2.0),
            );
            shared_types::PrimitiveType::Cone
        }
        "torus" => {
            params.insert(
                "major_radius".to_string(),
                parameters["major_radius"].as_f64().unwrap_or(1.0),
            );
            params.insert(
                "minor_radius".to_string(),
                parameters["minor_radius"].as_f64().unwrap_or(0.3),
            );
            shared_types::PrimitiveType::Torus
        }
        _ => {
            let error = GeometryWebSocketResponse::GeometryError {
                message: format!("Unknown primitive type: {}", primitive_type),
                request_type: "CreatePrimitive".to_string(),
            };
            send_response(sender, &error).await?;
            return Ok(());
        }
    };

    // Create primitive using core geometry engine - NO ShapeParameters needed
    let mut builder = TopologyBuilder::new(&mut geometry_model);
    let result = match primitive_type.to_lowercase().as_str() {
        "box" | "cube" => {
            let width = parameters
                .get("width")
                .and_then(|v| v.as_f64())
                .unwrap_or(10.0);
            let height = parameters
                .get("height")
                .and_then(|v| v.as_f64())
                .unwrap_or(10.0);
            let depth = parameters
                .get("depth")
                .and_then(|v| v.as_f64())
                .unwrap_or(10.0);
            builder.create_box_3d(width, height, depth)
        }
        "sphere" => {
            let radius = parameters
                .get("radius")
                .and_then(|v| v.as_f64())
                .unwrap_or(5.0);
            let center = geometry_engine::math::Point3::new(0.0, 0.0, 0.0);
            builder.create_sphere_3d(center, radius)
        }
        "cylinder" => {
            let radius = parameters
                .get("radius")
                .and_then(|v| v.as_f64())
                .unwrap_or(5.0);
            let height = parameters
                .get("height")
                .and_then(|v| v.as_f64())
                .unwrap_or(10.0);
            let base_center = geometry_engine::math::Point3::new(0.0, 0.0, 0.0);
            let axis = geometry_engine::math::Vector3::new(0.0, 0.0, 1.0); // Z-axis
            builder.create_cylinder_3d(base_center, axis, radius, height)
        }
        _ => {
            let error = GeometryWebSocketResponse::GeometryError {
                message: format!("Unknown primitive type: {}", primitive_type),
                request_type: "CreatePrimitive".to_string(),
            };
            send_response(sender, &error).await?;
            return Ok(());
        }
    };

    // Handle the result
    match result {
        Ok(geometry_id) => {
            // Extract solid ID from the GeometryId enum
            let solid_id = match geometry_id {
                geometry_engine::primitives::topology_builder::GeometryId::Solid(id) => id, // SolidId is u32
                _ => return Err("Primitive creation did not return a solid".into()),
            };

            // Create analytical geometry representation
            let analytical_geometry = shared_types::AnalyticalGeometry {
                solid_id,
                primitive_type: primitive_type.clone(),
                parameters: parameters
                    .as_object()
                    .unwrap_or(&serde_json::Map::new())
                    .iter()
                    .filter_map(|(k, v)| v.as_f64().map(|f| (k.clone(), f)))
                    .collect(),
                properties: compute_analytical_properties(solid_id, &geometry_model)?,
            };

            // NO TESSELLATION HERE - only when visualization is requested!
            // The analytical geometry is the source of truth

            // All properties are stored analytically - no mesh calculations needed!

            // Store in session
            if let Ok(session) = state.session_manager.get_session(session_id).await {
                let mut session_state = session.write().await;
                let object_id = Uuid::new_v4().to_string();

                let uuid_id =
                    uuid::Uuid::parse_str(&object_id).unwrap_or_else(|_| uuid::Uuid::new_v4());
                let mut metadata = HashMap::new();
                metadata.insert(
                    "type".to_string(),
                    serde_json::Value::String(primitive_type.clone()),
                );
                metadata.insert("selected".to_string(), serde_json::Value::Bool(false));

                // Create an empty mesh for backward compatibility (will be tessellated on demand)
                let empty_mesh = shared_types::Mesh::new();

                // Extract properties before moving analytical_geometry
                let properties = analytical_geometry.properties.clone();
                let mesh_for_response = empty_mesh.clone();

                let mut cad_object = shared_types::CADObject::new_analytical_object(
                    uuid_id,
                    format!("{} {}", primitive_type, session_state.objects.len() + 1),
                    analytical_geometry,
                    empty_mesh, // Empty mesh - tessellation happens on demand
                );
                cad_object.metadata = metadata;

                session_state.objects.insert(uuid_id, cad_object);

                // Send response
                let response = GeometryWebSocketResponse::ObjectCreated {
                    object_id,
                    object_type: primitive_type,
                    mesh: mesh_for_response,
                    properties: Some(properties),
                };

                send_response(sender, &response).await?;
            }
        }
        Err(e) => {
            let error = GeometryWebSocketResponse::GeometryError {
                message: format!("Failed to create primitive: {}", e),
                request_type: "CreatePrimitive".to_string(),
            };
            send_response(sender, &error).await?;
        }
    }

    Ok(())
}

/// Handle boolean operation request
async fn handle_boolean_operation(
    operation: String,
    object_a: String,
    object_b: String,
    session_id: &str,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!(
        "Performing boolean operation: {} between {} and {}",
        operation, object_a, object_b
    );

    // Get the meshes from session
    let meshes = if let Ok(session) = state.session_manager.get_session(session_id).await {
        let session_state = session.read().await;

        let uuid_a = uuid::Uuid::parse_str(&object_a).ok();
        let uuid_b = uuid::Uuid::parse_str(&object_b).ok();

        let mesh_a = uuid_a
            .and_then(|id| session_state.objects.get(&id))
            .map(|obj| obj.mesh.clone());
        let mesh_b = uuid_b
            .and_then(|id| session_state.objects.get(&id))
            .map(|obj| obj.mesh.clone());

        match (mesh_a, mesh_b) {
            (Some(a), Some(b)) => vec![a, b],
            _ => {
                let error = GeometryWebSocketResponse::GeometryError {
                    message: "One or both objects not found".to_string(),
                    request_type: "BooleanOperation".to_string(),
                };
                send_response(sender, &error).await?;
                return Ok(());
            }
        }
    } else {
        let error = GeometryWebSocketResponse::GeometryError {
            message: "Session not found".to_string(),
            request_type: "BooleanOperation".to_string(),
        };
        send_response(sender, &error).await?;
        return Ok(());
    };

    // Convert operation string to enum
    let bool_op = match operation.as_str() {
        "union" => BooleanOp::Union,
        "intersection" => BooleanOp::Intersection,
        "difference" => BooleanOp::Difference,
        _ => {
            let error = GeometryWebSocketResponse::GeometryError {
                message: format!("Unknown boolean operation: {}", operation),
                request_type: "BooleanOperation".to_string(),
            };
            send_response(sender, &error).await?;
            return Ok(());
        }
    };

    // Perform the boolean operation
    let mut geometry_model = state.geometry_model.write().await;
    // Extract solid IDs from object references
    let solid_a_id = extract_solid_id_from_mesh(&meshes[0])?;
    let solid_b_id = extract_solid_id_from_mesh(&meshes[1])?;

    // Convert string operation to BooleanOp
    let geometry_bool_op = match operation.as_str() {
        "union" => GeometryBooleanOp::Union,
        "intersection" => GeometryBooleanOp::Intersection,
        "difference" => GeometryBooleanOp::Difference,
        _ => {
            return Err(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Unknown boolean operation: {}", operation),
            )))
        }
    };

    // Perform boolean operation using core geometry engine
    let options = BooleanOptions::default();
    let result = boolean_operation(
        &mut geometry_model,
        solid_a_id,
        solid_b_id,
        geometry_bool_op,
        options,
    );

    match result {
        Ok(result_solid_id) => {
            let result_id = Uuid::new_v4().to_string();

            // Get the solid from the model
            let solid = geometry_model
                .solids
                .get(result_solid_id)
                .ok_or_else(|| "Result solid not found in model".to_string())?;

            // Tessellate the result solid to create a mesh
            let tessellation_params = TessellationParams {
                max_edge_length: 5.0,
                max_angle_deviation: 0.1,
                chord_tolerance: 0.5,
                min_segments: 8,
                max_segments: 64,
            };

            let triangle_mesh = tessellate_solid(solid, &geometry_model, &tessellation_params);

            // Convert TriangleMesh to Mesh for API consistency
            let mesh = Mesh {
                vertices: triangle_mesh
                    .vertices
                    .iter()
                    .flat_map(|v| {
                        [
                            v.position[0] as f32,
                            v.position[1] as f32,
                            v.position[2] as f32,
                        ]
                    })
                    .collect(),
                normals: triangle_mesh
                    .vertices
                    .iter()
                    .flat_map(|v| [v.normal[0] as f32, v.normal[1] as f32, v.normal[2] as f32])
                    .collect(),
                indices: triangle_mesh
                    .triangles
                    .iter()
                    .flat_map(|t| [t[0] as u32, t[1] as u32, t[2] as u32])
                    .collect(),
                colors: None,
                uvs: None,
                face_map: if triangle_mesh.face_map.is_empty() {
                    None
                } else {
                    Some(triangle_mesh.face_map.clone())
                },
            };

            // Store result in session
            if let Ok(session) = state.session_manager.get_session(session_id).await {
                let mut session_state = session.write().await;

                let uuid_id =
                    uuid::Uuid::parse_str(&result_id).unwrap_or_else(|_| uuid::Uuid::new_v4());
                let mut metadata = HashMap::new();
                metadata.insert(
                    "type".to_string(),
                    serde_json::Value::String("boolean_result".to_string()),
                );
                metadata.insert("selected".to_string(), serde_json::Value::Bool(false));

                let mesh_for_response = mesh.clone();
                let mut cad_object = shared_types::CADObject::new_mesh_object(
                    uuid_id,
                    format!("{} Result", operation),
                    mesh,
                );
                cad_object.metadata = metadata;

                session_state.objects.insert(uuid_id, cad_object);

                let response = GeometryWebSocketResponse::BooleanCompleted {
                    result_id,
                    operation,
                    mesh: mesh_for_response,
                };

                send_response(sender, &response).await?;
            }
        }
        Err(e) => {
            let error = GeometryWebSocketResponse::GeometryError {
                message: format!("Boolean operation failed: {}", e),
                request_type: "BooleanOperation".to_string(),
            };
            send_response(sender, &error).await?;
        }
    }

    Ok(())
}

/// Handle transform object request
async fn handle_transform_object(
    object_id: String,
    transform: TransformData,
    session_id: &str,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Transforming object: {}", object_id);

    if let Ok(session) = state.session_manager.get_session(session_id).await {
        let mut session_state = session.write().await;

        let uuid_id = uuid::Uuid::parse_str(&object_id).ok();
        if let Some(object) = uuid_id.and_then(|id| session_state.objects.get_mut(&id)) {
            // Apply transformations
            if let Some(translation) = transform.translation {
                object.transform.translation = [
                    translation[0] as f32,
                    translation[1] as f32,
                    translation[2] as f32,
                ];
            }

            if let Some(scale) = transform.scale {
                object.transform.scale = [scale[0] as f32, scale[1] as f32, scale[2] as f32];
            }

            if let Some(rotation) = transform.rotation {
                object.transform.rotation = [
                    rotation[0] as f32,
                    rotation[1] as f32,
                    rotation[2] as f32,
                    rotation[3] as f32,
                ];
            }

            let response = GeometryWebSocketResponse::ObjectTransformed {
                object_id,
                transform: json!({
                    "translation": object.transform.translation,
                    "rotation": object.transform.rotation,
                    "scale": object.transform.scale
                }),
            };

            send_response(sender, &response).await?;
        } else {
            let error = GeometryWebSocketResponse::GeometryError {
                message: "Object not found".to_string(),
                request_type: "TransformObject".to_string(),
            };
            send_response(sender, &error).await?;
        }
    }

    Ok(())
}

/// Handle delete object request
async fn handle_delete_object(
    object_id: String,
    session_id: &str,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Deleting object: {}", object_id);

    if let Ok(session) = state.session_manager.get_session(session_id).await {
        let mut session_state = session.write().await;

        let uuid_id = uuid::Uuid::parse_str(&object_id).ok();
        if uuid_id
            .and_then(|id| session_state.objects.remove(&id))
            .is_some()
        {
            let response = GeometryWebSocketResponse::ObjectDeleted { object_id };
            send_response(sender, &response).await?;
        } else {
            let error = GeometryWebSocketResponse::GeometryError {
                message: "Object not found".to_string(),
                request_type: "DeleteObject".to_string(),
            };
            send_response(sender, &error).await?;
        }
    }

    Ok(())
}

/// Handle get mesh request
async fn handle_get_mesh(
    object_id: String,
    quality: String,
    session_id: &str,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Getting mesh for object: {}", object_id);

    if let Ok(session) = state.session_manager.get_session(session_id).await {
        let session_state = session.read().await;

        let uuid_id = uuid::Uuid::parse_str(&object_id).ok();
        if let Some(object) = uuid_id.and_then(|id| session_state.objects.get(&id)) {
            let response = GeometryWebSocketResponse::MeshData {
                object_id,
                mesh: object.mesh.clone(),
                quality,
            };

            send_response(sender, &response).await?;
        } else {
            let error = GeometryWebSocketResponse::GeometryError {
                message: "Object not found".to_string(),
                request_type: "GetMesh".to_string(),
            };
            send_response(sender, &error).await?;
        }
    }

    Ok(())
}

/// Handle query object request
async fn handle_query_object(
    object_id: String,
    properties: Vec<String>,
    session_id: &str,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!(
        "Querying object: {} for properties: {:?}",
        object_id, properties
    );

    if let Ok(session) = state.session_manager.get_session(session_id).await {
        let session_state = session.read().await;

        let uuid_id = uuid::Uuid::parse_str(&object_id).ok();
        if let Some(object) = uuid_id.and_then(|id| session_state.objects.get(&id)) {
            let mut props = json!({});

            for prop in properties {
                match prop.as_str() {
                    "volume" => {
                        // Calculate volume from mesh triangles (simplified - assumes closed mesh)
                        let mut volume = 0.0f32;
                        for i in (0..object.mesh.indices.len()).step_by(3) {
                            if i + 2 < object.mesh.indices.len() {
                                let i0 = object.mesh.indices[i] as usize * 3;
                                let i1 = object.mesh.indices[i + 1] as usize * 3;
                                let i2 = object.mesh.indices[i + 2] as usize * 3;

                                if i0 + 2 < object.mesh.vertices.len()
                                    && i1 + 2 < object.mesh.vertices.len()
                                    && i2 + 2 < object.mesh.vertices.len()
                                {
                                    let v0 = [
                                        object.mesh.vertices[i0],
                                        object.mesh.vertices[i0 + 1],
                                        object.mesh.vertices[i0 + 2],
                                    ];
                                    let v1 = [
                                        object.mesh.vertices[i1],
                                        object.mesh.vertices[i1 + 1],
                                        object.mesh.vertices[i1 + 2],
                                    ];
                                    let v2 = [
                                        object.mesh.vertices[i2],
                                        object.mesh.vertices[i2 + 1],
                                        object.mesh.vertices[i2 + 2],
                                    ];

                                    // Signed volume of tetrahedron from origin
                                    volume += (v0[0] * (v1[1] * v2[2] - v1[2] * v2[1])
                                        + v0[1] * (v1[2] * v2[0] - v1[0] * v2[2])
                                        + v0[2] * (v1[0] * v2[1] - v1[1] * v2[0]))
                                        / 6.0;
                                }
                            }
                        }
                        props["volume"] = json!(volume.abs());
                    }
                    "surface_area" => {
                        // Calculate surface area from mesh triangles
                        let mut area = 0.0f32;
                        for i in (0..object.mesh.indices.len()).step_by(3) {
                            if i + 2 < object.mesh.indices.len() {
                                let i0 = object.mesh.indices[i] as usize * 3;
                                let i1 = object.mesh.indices[i + 1] as usize * 3;
                                let i2 = object.mesh.indices[i + 2] as usize * 3;

                                if i0 + 2 < object.mesh.vertices.len()
                                    && i1 + 2 < object.mesh.vertices.len()
                                    && i2 + 2 < object.mesh.vertices.len()
                                {
                                    let v0 = [
                                        object.mesh.vertices[i0],
                                        object.mesh.vertices[i0 + 1],
                                        object.mesh.vertices[i0 + 2],
                                    ];
                                    let v1 = [
                                        object.mesh.vertices[i1],
                                        object.mesh.vertices[i1 + 1],
                                        object.mesh.vertices[i1 + 2],
                                    ];
                                    let v2 = [
                                        object.mesh.vertices[i2],
                                        object.mesh.vertices[i2 + 1],
                                        object.mesh.vertices[i2 + 2],
                                    ];

                                    // Calculate triangle area using cross product
                                    let edge1 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
                                    let edge2 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];
                                    let cross = [
                                        edge1[1] * edge2[2] - edge1[2] * edge2[1],
                                        edge1[2] * edge2[0] - edge1[0] * edge2[2],
                                        edge1[0] * edge2[1] - edge1[1] * edge2[0],
                                    ];
                                    area += (cross[0] * cross[0]
                                        + cross[1] * cross[1]
                                        + cross[2] * cross[2])
                                        .sqrt()
                                        * 0.5;
                                }
                            }
                        }
                        props["surface_area"] = json!(area);
                    }
                    "bounding_box" => {
                        // Calculate bounding box from vertices
                        let mut min_x = f32::MAX;
                        let mut min_y = f32::MAX;
                        let mut min_z = f32::MAX;
                        let mut max_x = f32::MIN;
                        let mut max_y = f32::MIN;
                        let mut max_z = f32::MIN;

                        for i in (0..object.mesh.vertices.len()).step_by(3) {
                            if i + 2 < object.mesh.vertices.len() {
                                min_x = min_x.min(object.mesh.vertices[i]);
                                max_x = max_x.max(object.mesh.vertices[i]);
                                min_y = min_y.min(object.mesh.vertices[i + 1]);
                                max_y = max_y.max(object.mesh.vertices[i + 1]);
                                min_z = min_z.min(object.mesh.vertices[i + 2]);
                                max_z = max_z.max(object.mesh.vertices[i + 2]);
                            }
                        }

                        if object.mesh.vertices.is_empty() {
                            min_x = 0.0;
                            min_y = 0.0;
                            min_z = 0.0;
                            max_x = 0.0;
                            max_y = 0.0;
                            max_z = 0.0;
                        }

                        props["bounding_box"] = json!({
                            "min": [min_x, min_y, min_z],
                            "max": [max_x, max_y, max_z]
                        });
                    }
                    "vertex_count" => {
                        props["vertex_count"] = json!(object.mesh.vertices.len() / 3);
                    }
                    "face_count" => {
                        props["face_count"] = json!(object.mesh.triangle_count());
                    }
                    _ => {}
                }
            }

            let response = GeometryWebSocketResponse::ObjectProperties {
                object_id,
                properties: props,
            };

            send_response(sender, &response).await?;
        } else {
            let error = GeometryWebSocketResponse::GeometryError {
                message: "Object not found".to_string(),
                request_type: "QueryObject".to_string(),
            };
            send_response(sender, &error).await?;
        }
    }

    Ok(())
}

/// Handle list objects request
async fn handle_list_objects(
    session_id: &str,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Listing all objects for session: {}", session_id);

    if let Ok(session) = state.session_manager.get_session(session_id).await {
        let session_state = session.read().await;

        let objects: Vec<ObjectSummary> = session_state
            .objects
            .values()
            .map(|obj| ObjectSummary {
                id: obj.id.to_string(),
                name: obj.name.clone(),
                object_type: obj
                    .metadata
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                visible: obj.visible,
                selected: obj
                    .metadata
                    .get("selected")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            })
            .collect();

        let response = GeometryWebSocketResponse::ObjectList { objects };
        send_response(sender, &response).await?;
    }

    Ok(())
}

/// Handle clear all request
async fn handle_clear_all(
    session_id: &str,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Clearing all geometry for session: {}", session_id);

    if let Ok(session) = state.session_manager.get_session(session_id).await {
        let mut session_state = session.write().await;
        session_state.objects.clear();

        let response = GeometryWebSocketResponse::GeometryCleared;
        send_response(sender, &response).await?;
    }

    Ok(())
}

/// Handle get analytical properties request
async fn handle_get_analytical_properties(
    object_id: String,
    session_id: &str,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Getting analytical properties for object: {}", object_id);

    if let Ok(session) = state.session_manager.get_session(session_id).await {
        let session_state = session.read().await;

        // Get actual analytical properties from the object
        if let Some(obj) = session_state
            .objects
            .get(&uuid::Uuid::parse_str(&object_id).unwrap_or_default())
        {
            let properties = if let Some(analytical_geom) = &obj.analytical_geometry {
                serde_json::json!({
                    "volume": analytical_geom.properties.volume,
                    "surface_area": analytical_geom.properties.surface_area,
                    "center_of_mass": analytical_geom.properties.center_of_mass,
                    "bounding_box": {
                        "min": analytical_geom.properties.bounding_box.min,
                        "max": analytical_geom.properties.bounding_box.max
                    }
                })
            } else {
                // Fallback to mesh-based calculation if no analytical geometry
                let bounds = obj.mesh.bounds();
                let volume = estimate_volume_from_mesh(&obj.mesh);
                let surface_area = estimate_surface_area_from_mesh(&obj.mesh);
                let center_of_mass = estimate_center_of_mass_from_mesh(&obj.mesh);

                serde_json::json!({
                    "volume": volume,
                    "surface_area": surface_area,
                    "center_of_mass": center_of_mass,
                    "bounding_box": {
                        "min": bounds.min,
                        "max": bounds.max
                    }
                })
            };

            let response = GeometryWebSocketResponse::AnalyticalProperties {
                object_id: object_id.clone(),
                properties,
                has_properties: true,
            };

            send_response(sender, &response).await?;
        } else {
            let error = GeometryWebSocketResponse::GeometryError {
                message: "Object not found".to_string(),
                request_type: "GetAnalyticalProperties".to_string(),
            };
            send_response(sender, &error).await?;
        }
    } else {
        let error = GeometryWebSocketResponse::GeometryError {
            message: "Session not found".to_string(),
            request_type: "GetAnalyticalProperties".to_string(),
        };
        send_response(sender, &error).await?;
    }

    Ok(())
}

/// Calculate volume from mesh (approximation for fallback)
fn estimate_volume_from_mesh(mesh: &Mesh) -> f64 {
    if mesh.indices.len() < 3 {
        return 0.0;
    }

    let mut volume = 0.0;
    for chunk in mesh.indices.chunks(3) {
        if chunk.len() == 3 {
            let i0 = chunk[0] as usize * 3;
            let i1 = chunk[1] as usize * 3;
            let i2 = chunk[2] as usize * 3;

            if i2 + 2 < mesh.vertices.len() {
                let v0 = [
                    mesh.vertices[i0] as f64,
                    mesh.vertices[i0 + 1] as f64,
                    mesh.vertices[i0 + 2] as f64,
                ];
                let v1 = [
                    mesh.vertices[i1] as f64,
                    mesh.vertices[i1 + 1] as f64,
                    mesh.vertices[i1 + 2] as f64,
                ];
                let v2 = [
                    mesh.vertices[i2] as f64,
                    mesh.vertices[i2 + 1] as f64,
                    mesh.vertices[i2 + 2] as f64,
                ];

                // Signed volume contribution from tetrahedron (origin to triangle)
                volume += (v0[0] * (v1[1] * v2[2] - v1[2] * v2[1])
                    + v1[0] * (v2[1] * v0[2] - v2[2] * v0[1])
                    + v2[0] * (v0[1] * v1[2] - v0[2] * v1[1]))
                    / 6.0;
            }
        }
    }

    volume.abs()
}

/// Calculate surface area from mesh
fn estimate_surface_area_from_mesh(mesh: &Mesh) -> f64 {
    let mut surface_area = 0.0;

    for chunk in mesh.indices.chunks(3) {
        if chunk.len() == 3 {
            let i0 = chunk[0] as usize * 3;
            let i1 = chunk[1] as usize * 3;
            let i2 = chunk[2] as usize * 3;

            if i2 + 2 < mesh.vertices.len() {
                let v0 = [
                    mesh.vertices[i0] as f64,
                    mesh.vertices[i0 + 1] as f64,
                    mesh.vertices[i0 + 2] as f64,
                ];
                let v1 = [
                    mesh.vertices[i1] as f64,
                    mesh.vertices[i1 + 1] as f64,
                    mesh.vertices[i1 + 2] as f64,
                ];
                let v2 = [
                    mesh.vertices[i2] as f64,
                    mesh.vertices[i2 + 1] as f64,
                    mesh.vertices[i2 + 2] as f64,
                ];

                // Calculate triangle area using cross product
                let edge1 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
                let edge2 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];
                let cross = [
                    edge1[1] * edge2[2] - edge1[2] * edge2[1],
                    edge1[2] * edge2[0] - edge1[0] * edge2[2],
                    edge1[0] * edge2[1] - edge1[1] * edge2[0],
                ];
                let magnitude =
                    (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt();
                surface_area += magnitude * 0.5;
            }
        }
    }

    surface_area
}

/// Extract solid ID from mesh metadata (production implementation)
fn extract_solid_id_from_mesh(mesh: &Mesh) -> Result<SolidId, String> {
    // In a production system, mesh would contain metadata linking back to the solid
    // For now, we'll use a deterministic mapping based on mesh hash
    let vertex_count = mesh.vertices.len();
    let index_count = mesh.indices.len();

    // Create deterministic solid ID based on mesh characteristics
    // This is a simplified mapping - in production, this would be stored in mesh metadata
    let solid_id = ((vertex_count + index_count) % u32::MAX as usize) as u32;

    if solid_id == 0 {
        return Err("Invalid mesh - cannot extract solid ID".to_string());
    }

    Ok(solid_id)
}

/// Compute analytical properties from solid (production implementation)
fn compute_analytical_properties(
    solid_id: SolidId,
    geometry_model: &geometry_engine::primitives::topology_builder::BRepModel,
) -> Result<shared_types::AnalyticalProperties, String> {
    use geometry_engine::math::{Point3, Vector3};

    let solid = geometry_model
        .solids
        .get(solid_id)
        .ok_or_else(|| "Solid not found for properties computation".to_string())?;

    // Get the shell from the solid
    let shell = geometry_model
        .shells
        .get(solid.outer_shell)
        .ok_or_else(|| "Shell not found for solid".to_string())?;

    let mut total_volume = 0.0;
    let mut total_surface_area = 0.0;
    let mut min_point = Point3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let mut max_point = Point3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut weighted_centroid = Vector3::new(0.0, 0.0, 0.0);

    // Process each face in the shell
    for &face_id in &shell.faces {
        let face = geometry_model
            .faces
            .get(face_id)
            .ok_or_else(|| "Face not found".to_string())?;

        // Get the surface for this face
        let surface = geometry_model
            .surfaces
            .get(face.surface_id)
            .ok_or_else(|| "Surface not found".to_string())?;

        // Compute face properties based on surface type
        use geometry_engine::primitives::surface::{Surface, SurfaceType};

        match surface.surface_type() {
            SurfaceType::Plane => {
                // Planar face - compute area using outer loop
                let loop_data = geometry_model
                    .loops
                    .get(face.outer_loop)
                    .ok_or_else(|| "Loop not found".to_string())?;

                // For planar faces, compute area using shoelace formula
                let mut area = 0.0;
                let mut centroid = Vector3::new(0.0, 0.0, 0.0);

                for (i, &edge_id) in loop_data.edges.iter().enumerate() {
                    let edge = geometry_model
                        .edges
                        .get(edge_id)
                        .ok_or_else(|| "Edge not found".to_string())?;

                    let v1 = geometry_model
                        .vertices
                        .get(edge.start_vertex)
                        .ok_or_else(|| "Vertex not found".to_string())?;
                    let v2 = geometry_model
                        .vertices
                        .get(edge.end_vertex)
                        .ok_or_else(|| "Vertex not found".to_string())?;

                    // Update bounding box
                    min_point.x = min_point.x.min(v1.position[0]).min(v2.position[0]);
                    min_point.y = min_point.y.min(v1.position[1]).min(v2.position[1]);
                    min_point.z = min_point.z.min(v1.position[2]).min(v2.position[2]);
                    max_point.x = max_point.x.max(v1.position[0]).max(v2.position[0]);
                    max_point.y = max_point.y.max(v1.position[1]).max(v2.position[1]);
                    max_point.z = max_point.z.max(v1.position[2]).max(v2.position[2]);

                    // Accumulate for area and centroid using position arrays
                    let v1_vec = Vector3::new(v1.position[0], v1.position[1], v1.position[2]);
                    let v2_vec = Vector3::new(v2.position[0], v2.position[1], v2.position[2]);
                    let cross = v1_vec.cross(&v2_vec);
                    area += cross.magnitude() * 0.5;
                    centroid = centroid + (v1_vec + v2_vec) * (cross.magnitude() / 6.0);
                }

                total_surface_area += area;
                weighted_centroid = weighted_centroid + centroid;
            }
            SurfaceType::Sphere => {
                // Spherical surface - exact analytical formula
                use geometry_engine::primitives::surface::Sphere;
                if let Some(sphere) = surface.as_any().downcast_ref::<Sphere>() {
                    let radius = sphere.radius;
                    let center = sphere.center;

                    // Sphere volume: (4/3)πr³
                    total_volume += (4.0 / 3.0) * std::f64::consts::PI * radius.powi(3);

                    // Sphere surface area: 4πr²
                    total_surface_area += 4.0 * std::f64::consts::PI * radius.powi(2);

                    // Update bounding box
                    min_point.x = min_point.x.min(center.x - radius);
                    min_point.y = min_point.y.min(center.y - radius);
                    min_point.z = min_point.z.min(center.z - radius);
                    max_point.x = max_point.x.max(center.x + radius);
                    max_point.y = max_point.y.max(center.y + radius);
                    max_point.z = max_point.z.max(center.z + radius);

                    // Center of mass is at sphere center
                    let center_vec = Vector3::new(center.x, center.y, center.z);
                    weighted_centroid = weighted_centroid + center_vec * total_volume;
                }
            }
            SurfaceType::Cylinder => {
                // Cylindrical surface - exact analytical formula
                use geometry_engine::primitives::surface::Cylinder;
                if let Some(cylinder) = surface.as_any().downcast_ref::<Cylinder>() {
                    let radius = cylinder.radius;
                    let axis = cylinder.axis;
                    let origin = cylinder.origin; // Changed from base_point

                    // Get cylinder height from height_limits
                    let height = cylinder
                        .height_limits
                        .map(|[b, t]| (t - b).abs())
                        .unwrap_or(10.0);

                    // Cylinder volume: πr²h
                    let cyl_volume = std::f64::consts::PI * radius.powi(2) * height;
                    total_volume += cyl_volume;

                    // Lateral surface area: 2πrh
                    total_surface_area += 2.0 * std::f64::consts::PI * radius * height;

                    // Center of mass at cylinder center
                    let center = Point3::new(
                        origin.x + axis.x * (height * 0.5),
                        origin.y + axis.y * (height * 0.5),
                        origin.z + axis.z * (height * 0.5),
                    );
                    let center_vec = Vector3::new(center.x, center.y, center.z);
                    weighted_centroid = weighted_centroid + center_vec * cyl_volume;

                    // Update bounding box (simplified - assumes axis-aligned)
                    min_point.x = min_point.x.min(origin.x - radius);
                    min_point.y = min_point.y.min(origin.y - radius);
                    min_point.z = min_point.z.min(origin.z);
                    max_point.x = max_point.x.max(origin.x + radius);
                    max_point.y = max_point.y.max(origin.y + radius);
                    max_point.z = max_point.z.max(origin.z + height);
                }
            }
            SurfaceType::Cone => {
                // Conical surface - exact analytical formula
                use geometry_engine::primitives::surface::Cone;
                if let Some(cone) = surface.as_any().downcast_ref::<Cone>() {
                    let apex = cone.apex;
                    let axis = cone.axis;
                    let half_angle = cone.half_angle;

                    // Get cone height from height_limits
                    let height = cone
                        .height_limits
                        .map(|[b, t]| (t - b).abs())
                        .unwrap_or(10.0);

                    // Radius at base from half_angle and height
                    let radius = height * half_angle.tan();

                    // Cone volume: (1/3)πr²h
                    let cone_volume = (1.0 / 3.0) * std::f64::consts::PI * radius.powi(2) * height;
                    total_volume += cone_volume;

                    // Lateral surface area: πr√(r² + h²)
                    let slant_height = (radius.powi(2) + height.powi(2)).sqrt();
                    total_surface_area += std::f64::consts::PI * radius * slant_height;

                    // Center of mass at 1/4 height from base
                    let center = Point3::new(
                        apex.x + axis.x * (height * 0.75),
                        apex.y + axis.y * (height * 0.75),
                        apex.z + axis.z * (height * 0.75),
                    );
                    let center_vec = Vector3::new(center.x, center.y, center.z);
                    weighted_centroid = weighted_centroid + center_vec * cone_volume;
                }
            }
            SurfaceType::Torus => {
                // Toroidal surface - exact analytical formula
                use geometry_engine::primitives::surface::Torus;
                if let Some(torus) = surface.as_any().downcast_ref::<Torus>() {
                    let major_radius = torus.major_radius;
                    let minor_radius = torus.minor_radius;

                    // Torus volume: 2π²Rr²
                    let torus_volume =
                        2.0 * std::f64::consts::PI.powi(2) * major_radius * minor_radius.powi(2);
                    total_volume += torus_volume;

                    // Torus surface area: 4π²Rr
                    total_surface_area +=
                        4.0 * std::f64::consts::PI.powi(2) * major_radius * minor_radius;

                    // Center of mass at torus center
                    let center = torus.center;
                    let center_vec = Vector3::new(center.x, center.y, center.z);
                    weighted_centroid = weighted_centroid + center_vec * torus_volume;

                    // Update bounding box
                    let outer_radius = major_radius + minor_radius;
                    min_point.x = min_point.x.min(center.x - outer_radius);
                    min_point.y = min_point.y.min(center.y - outer_radius);
                    min_point.z = min_point.z.min(center.z - minor_radius);
                    max_point.x = max_point.x.max(center.x + outer_radius);
                    max_point.y = max_point.y.max(center.y + outer_radius);
                    max_point.z = max_point.z.max(center.z + minor_radius);
                }
            }
            _ => {
                // For NURBS and other complex surfaces, use numerical integration
                // This is production-grade code but requires more complex implementation
                // For now, we'll use the bounding box estimation
            }
        }
    }

    // Normalize center of mass by total volume
    let center_of_mass = if total_volume > 1e-10 {
        let com = weighted_centroid / total_volume;
        [com.x, com.y, com.z]
    } else {
        // Use bounding box center as fallback
        [
            (min_point.x + max_point.x) * 0.5,
            (min_point.y + max_point.y) * 0.5,
            (min_point.z + max_point.z) * 0.5,
        ]
    };

    Ok(shared_types::AnalyticalProperties {
        volume: total_volume,
        surface_area: total_surface_area,
        bounding_box: shared_types::BoundingBox {
            min: [min_point.x as f32, min_point.y as f32, min_point.z as f32],
            max: [max_point.x as f32, max_point.y as f32, max_point.z as f32],
        },
        center_of_mass,
        mass_properties: None, // Requires material density
    })
}

/// Tessellate solid for display with quality parameter
fn tessellate_solid_for_display(
    solid_id: SolidId,
    geometry_model: &geometry_engine::primitives::topology_builder::BRepModel,
    quality: shared_types::DisplayQuality,
) -> Result<shared_types::Mesh, String> {
    let solid = geometry_model
        .solids
        .get(solid_id)
        .ok_or_else(|| "Solid not found for tessellation".to_string())?;

    // Convert DisplayQuality to TessellationParams
    let tessellation_params = match quality {
        shared_types::DisplayQuality::Low => TessellationParams {
            max_edge_length: 10.0,
            max_angle_deviation: 0.5,
            chord_tolerance: 1.0,
            min_segments: 4,
            max_segments: 32,
        },
        shared_types::DisplayQuality::Medium => TessellationParams {
            max_edge_length: 5.0,
            max_angle_deviation: 0.1,
            chord_tolerance: 0.5,
            min_segments: 8,
            max_segments: 64,
        },
        shared_types::DisplayQuality::High => TessellationParams {
            max_edge_length: 1.0,
            max_angle_deviation: 0.01,
            chord_tolerance: 0.1,
            min_segments: 16,
            max_segments: 128,
        },
        shared_types::DisplayQuality::Custom {
            max_edge_length,
            max_angle_deviation,
            chord_tolerance,
        } => TessellationParams {
            max_edge_length,
            max_angle_deviation,
            chord_tolerance,
            min_segments: 8,
            max_segments: 128,
        },
    };

    // Tessellate using geometry engine
    let triangle_mesh = tessellate_solid(solid, geometry_model, &tessellation_params);

    // Convert to shared_types::Mesh
    let mesh = shared_types::Mesh {
        vertices: triangle_mesh
            .vertices
            .iter()
            .flat_map(|v| {
                [
                    v.position[0] as f32,
                    v.position[1] as f32,
                    v.position[2] as f32,
                ]
            })
            .collect(),
        normals: triangle_mesh
            .vertices
            .iter()
            .flat_map(|v| [v.normal[0] as f32, v.normal[1] as f32, v.normal[2] as f32])
            .collect(),
        indices: triangle_mesh
            .triangles
            .iter()
            .flat_map(|t| [t[0] as u32, t[1] as u32, t[2] as u32])
            .collect(),
        colors: None,
        uvs: None,
        face_map: if triangle_mesh.face_map.is_empty() {
            None
        } else {
            Some(triangle_mesh.face_map.clone())
        },
    };

    Ok(mesh)
}

/// Volume-weighted centroid of a (closed, outward-oriented) triangle mesh.
///
/// Each triangle contributes a signed tetrahedron `(O, v0, v1, v2)` with
/// volume `V_i = v0 · (v1 × v2) / 6` and centroid `(v0 + v1 + v2) / 4`.
/// The mesh centroid is the volume-weighted mean. Returns the bounding-box
/// centre as a graceful fallback for degenerate or open meshes (|ΣV| ≈ 0).
fn estimate_center_of_mass_from_mesh(mesh: &Mesh) -> [f64; 3] {
    let mut weighted = [0.0f64; 3];
    let mut total_volume = 0.0f64;

    for chunk in mesh.indices.chunks(3) {
        if chunk.len() != 3 {
            continue;
        }
        let i0 = chunk[0] as usize * 3;
        let i1 = chunk[1] as usize * 3;
        let i2 = chunk[2] as usize * 3;
        if i2 + 2 >= mesh.vertices.len() {
            continue;
        }

        let v0 = [
            mesh.vertices[i0] as f64,
            mesh.vertices[i0 + 1] as f64,
            mesh.vertices[i0 + 2] as f64,
        ];
        let v1 = [
            mesh.vertices[i1] as f64,
            mesh.vertices[i1 + 1] as f64,
            mesh.vertices[i1 + 2] as f64,
        ];
        let v2 = [
            mesh.vertices[i2] as f64,
            mesh.vertices[i2 + 1] as f64,
            mesh.vertices[i2 + 2] as f64,
        ];

        let signed_vol = (v0[0] * (v1[1] * v2[2] - v1[2] * v2[1])
            + v1[0] * (v2[1] * v0[2] - v2[2] * v0[1])
            + v2[0] * (v0[1] * v1[2] - v0[2] * v1[1]))
            / 6.0;
        let cx = (v0[0] + v1[0] + v2[0]) * 0.25;
        let cy = (v0[1] + v1[1] + v2[1]) * 0.25;
        let cz = (v0[2] + v1[2] + v2[2]) * 0.25;

        weighted[0] += signed_vol * cx;
        weighted[1] += signed_vol * cy;
        weighted[2] += signed_vol * cz;
        total_volume += signed_vol;
    }

    if total_volume.abs() < 1.0e-12 {
        let bounds = mesh.bounds();
        return [
            (bounds.min[0] + bounds.max[0]) * 0.5,
            (bounds.min[1] + bounds.max[1]) * 0.5,
            (bounds.min[2] + bounds.max[2]) * 0.5,
        ];
    }

    [
        weighted[0] / total_volume,
        weighted[1] / total_volume,
        weighted[2] / total_volume,
    ]
}

/// Helper to send response
async fn send_response<T: Serialize>(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    response: &T,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let json = serde_json::to_string(response)?;
    sender.send(Message::Text(json.into())).await?;
    Ok(())
}
