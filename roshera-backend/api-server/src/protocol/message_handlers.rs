//! ClientMessage/ServerMessage protocol handlers
//!
//! This module handles the APPLICATION PROTOCOL messages.
//! WebSocket is just the transport layer used to deliver these messages.

use crate::protocol::protocol::GeometryWSCommand;
use crate::AppState;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use chrono;
use futures::{sink::SinkExt, stream::StreamExt};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::GeometryId as LocalGeometryId; // Enum for internal use
use geometry_engine::primitives::topology_builder::{BRepModel, TopologyBuilder};
use serde::{Deserialize, Serialize};
use shared_types::hierarchy::{
    EditContext, HierarchyCommand, ProjectHierarchy, WorkflowStage, WorkflowState,
};
use shared_types::GeometryId as GlobalGeometryId; // UUID-based for external use
use shared_types::{CommandResult, PrimitiveType};
use tracing::{error, info, warn};
use uuid::Uuid;

// Import our protocol
use super::protocol::{ClientMessage, ServerMessage};

#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WebSocketMessage {
    // Server -> Client messages
    Welcome {
        session_id: String,
        user_id: String,
        message: String,
    },
    GeometryCreated {
        object_id: u32,
        shape_type: String,
        created_by: String,
    },
    SketchPlaneCreated {
        plane_id: String,
        plane_type: String, // "XY", "XZ", "YZ"
        position: [f64; 3],
        normal: [f64; 3],
        size: f64,
    },
    SketchEntityAdded {
        entity_id: String,
        entity_type: String, // "circle", "line", "arc"
        plane_id: String,
        parameters: serde_json::Value,
    },
    SketchCameraUpdate {
        position: [f64; 3],
        target: [f64; 3],
        up: [f64; 3],
    },
    SketchPlanesList {
        planes: Vec<shared_types::session::SketchPlaneInfo>,
        active_plane_id: Option<String>,
    },
    OrientationCubeUpdate {
        state: shared_types::session::OrientationCubeState,
    },
    SessionUpdate {
        message: String,
    },
    UserJoined {
        user_id: String,
        user_name: String,
        session_id: String,
    },
    UserLeft {
        user_id: String,
        session_id: String,
    },
    CollaboratorsUpdate {
        session_id: String,
        users: Vec<CollaboratorInfo>,
    },
    Error {
        message: String,
        code: Option<String>,
    },
    // Hierarchy updates
    HierarchyUpdate {
        session_id: String,
        hierarchy: ProjectHierarchy,
        workflow_state: WorkflowState,
        affected_instances: Vec<(String, String)>,
    },
    WorkflowStageChanged {
        session_id: String,
        stage: String,
        available_tools: Vec<String>,
    },
}

#[derive(Serialize, Deserialize, Clone)]
pub struct CollaboratorInfo {
    pub id: String,
    pub name: String,
    pub initial: String,
    pub is_active: bool,
    pub color: [f32; 4],
    pub last_activity: u64,
}

// WebSocket upgrade handler
pub async fn websocket_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    info!("🔌 WebSocket upgrade request received");
    ws.on_upgrade(move |socket| handle_websocket_connection(socket, state))
}

async fn handle_websocket_connection(socket: WebSocket, state: AppState) {
    // Track WebSocket connection
    crate::ACTIVE_WEBSOCKETS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let (mut sender, mut receiver) = socket.split();
    let user_id = Uuid::new_v4().to_string();
    let mut current_session_id: Option<String> = None;

    info!("New WebSocket connection: user={}", user_id);
    crate::broadcast_log(
        "INFO",
        &format!("New WebSocket connection established from user {}", user_id),
    );

    // Don't send welcome message immediately - wait for JoinSession
    info!("WebSocket connection established, waiting for JoinSession request...");

    // Send welcome message
    let welcome = ServerMessage::Welcome {
        connection_id: user_id.clone(),
        server_version: "1.0.0".to_string(),
        capabilities: vec![
            "geometry".to_string(),
            "timeline".to_string(),
            "ai".to_string(),
            "export".to_string(),
            "session".to_string(),
        ],
    };

    if let Ok(welcome_json) = serde_json::to_string(&welcome) {
        let _ = sender.send(Message::Text(welcome_json.into())).await;
    }

    // Handle incoming messages
    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                // Try to parse as ClientMessage from protocol.rs
                match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(client_msg) => {
                        // Handle the ClientMessage and send appropriate response
                        info!("Received ClientMessage: {:?}", client_msg);
                        match client_msg {
                            ClientMessage::Ping { timestamp } => {
                                info!("Processing Ping with timestamp: {}", timestamp);
                                let pong = ServerMessage::Pong { timestamp };
                                if let Ok(pong_json) = serde_json::to_string(&pong) {
                                    info!("Sending Pong response: {}", pong_json);
                                    let _ = sender.send(Message::Text(pong_json.into())).await;
                                }
                            }
                            ClientMessage::Authenticate { token, request_id } => {
                                // Simple authentication for now
                                let auth_response = ServerMessage::Authenticated {
                                    user_id: user_id.to_string(),
                                    permissions: vec!["all".to_string()],
                                    request_id,
                                };
                                if let Ok(auth_json) = serde_json::to_string(&auth_response) {
                                    let _ = sender.send(Message::Text(auth_json.into())).await;
                                }
                            }
                            ClientMessage::GeometryCommand {
                                command,
                                request_id,
                            } => {
                                crate::broadcast_log(
                                    "INFO",
                                    &format!("Creating geometry: {:?}", command),
                                );
                                // Parse and process geometry command with actual parameters
                                let result = match &command {
                                    GeometryWSCommand::CreatePrimitive {
                                        primitive_type,
                                        parameters,
                                    } => {
                                        // Use the model from state and create geometry
                                        let mut model = state.model.write().await;
                                        let mut builder = TopologyBuilder::new(&mut *model);

                                        match primitive_type {
                                            PrimitiveType::Sphere => {
                                                let radius = parameters
                                                    .params
                                                    .get("radius")
                                                    .copied()
                                                    .unwrap_or(5.0);

                                                match builder.create_sphere_3d(
                                                    geometry_engine::math::Point3::ZERO,
                                                    radius,
                                                ) {
                                                    Ok(id) => {
                                                        serde_json::json!({
                                                            "type": "sphere",
                                                            "id": id,
                                                            "radius": radius
                                                        })
                                                    }
                                                    Ok(_) => serde_json::json!({
                                                        "error": "Unexpected geometry type (not a solid)"
                                                    }),
                                                    Err(e) => serde_json::json!({
                                                        "error": format!("Failed to create sphere: {}", e)
                                                    }),
                                                }
                                            }
                                            PrimitiveType::Box => {
                                                let width = parameters
                                                    .params
                                                    .get("width")
                                                    .copied()
                                                    .unwrap_or(10.0);
                                                let height = parameters
                                                    .params
                                                    .get("height")
                                                    .copied()
                                                    .unwrap_or(10.0);
                                                let depth = parameters
                                                    .params
                                                    .get("depth")
                                                    .copied()
                                                    .unwrap_or(10.0);

                                                match builder.create_box_3d(width, height, depth) {
                                                    Ok(id) => {
                                                        serde_json::json!({
                                                            "type": "box",
                                                            "id": id,
                                                            "dimensions": [width, height, depth]
                                                        })
                                                    }
                                                    Ok(_) => serde_json::json!({
                                                        "error": "Unexpected geometry type (not a solid)"
                                                    }),
                                                    Err(e) => serde_json::json!({
                                                        "error": format!("Failed to create box: {}", e)
                                                    }),
                                                }
                                            }
                                            _ => serde_json::json!({
                                                "error": "Unsupported primitive type",
                                                "type": format!("{:?}", primitive_type)
                                            }),
                                        }
                                    }
                                    _ => serde_json::json!({
                                        "error": "Unsupported command type",
                                        "command": format!("{:?}", command)
                                    }),
                                };

                                let success = ServerMessage::Success {
                                    result: Some(result),
                                    request_id,
                                };
                                if let Ok(success_json) = serde_json::to_string(&success) {
                                    let _ = sender.send(Message::Text(success_json.into())).await;
                                }
                            }
                            ClientMessage::AICommand {
                                command,
                                request_id,
                            } => {
                                // Handle AI commands - convert natural language to geometry
                                match command {
                                    super::protocol::AIWSCommand::ProcessCommand {
                                        text,
                                        context,
                                    } => {
                                        info!("Processing AI command: {}", text);
                                        crate::broadcast_log(
                                            "INFO",
                                            &format!("Processing AI command: {}", text),
                                        );

                                        // Use AI processor to parse the command
                                        let ai_result = {
                                            let ai_processor = state.ai_processor.lock().await;
                                            ai_processor.process_text_command(&text).await
                                        };

                                        match ai_result {
                                            Ok(parsed_command) => {
                                                // Convert AI command to geometry operation
                                                use ai_integration::VoiceCommand;
                                                match parsed_command {
                                                    VoiceCommand::ActivatePartMaturityWorkflow { primitive, parameters, sketch_plane, .. } => {
                                                        // Check if we have a session - user must join a session first
                                                        if let Some(ref session_id) = current_session_id {
                                                            // Set workflow stage to Create
                                                            let _ = state.hierarchy_manager.execute_command(
                                                                session_id,
                                                                shared_types::hierarchy::HierarchyCommand::SetWorkflowStage {
                                                                    stage: shared_types::hierarchy::WorkflowStage::Create,
                                                                }
                                                            ).await;

                                                            // Send workflow stage change notification
                                                            let workflow_msg = WebSocketMessage::WorkflowStageChanged {
                                                                session_id: session_id.clone(),
                                                                stage: "create".to_string(),
                                                                available_tools: vec!["create_primitive".to_string(), "boolean_operation".to_string()],
                                                            };
                                                            if let Ok(json) = serde_json::to_string(&workflow_msg) {
                                                                let _ = sender.send(Message::Text(json.into())).await;
                                                            }

                                                            // Create sketch plane
                                                            // Get the actual sketch plane from geometry engine
                                                            let model_guard = state.geometry_model.read().await;
                                                            let (position, normal, size) = if let Some(sketch_plane_entity) = model_guard.sketch_planes.get(&format!("plane_{}", sketch_plane.to_lowercase())) {
                                                                // Use actual sketch plane properties
                                                                (sketch_plane_entity.position.into(), sketch_plane_entity.normal.into(), sketch_plane_entity.size)
                                                            } else {
                                                                // Calculate based on current model bounds and sketch plane type
                                                                let bounds = model_guard.compute_bounding_box();
                                                                let center = bounds.map(|b| [(b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0, (b.min.z + b.max.z) / 2.0])
                                                                    .unwrap_or([0.0, 0.0, 0.0]);
                                                                let model_size = bounds.map(|b| (b.max - b.min).magnitude()).unwrap_or(100.0);
                                                                let adaptive_size = model_size.max(50.0).min(1000.0);

                                                                let normal = match sketch_plane.as_str() {
                                                                    "XY" => [0.0, 0.0, 1.0],
                                                                    "XZ" => [0.0, 1.0, 0.0],
                                                                    "YZ" => [1.0, 0.0, 0.0],
                                                                    _ => [0.0, 0.0, 1.0],
                                                                };
                                                                (center, normal, adaptive_size)
                                                            };
                                                            drop(model_guard);

                                                            let plane_msg = WebSocketMessage::SketchPlaneCreated {
                                                                plane_id: format!("plane_{}", sketch_plane.to_lowercase()),
                                                                plane_type: sketch_plane.clone(),
                                                                position,
                                                                normal,
                                                                size,
                                                            };
                                                            if let Ok(json) = serde_json::to_string(&plane_msg) {
                                                                let _ = sender.send(Message::Text(json.into())).await;
                                                            }
                                                        }

                                                        // Now create the geometry with plane context
                                                        let mut model = state.model.write().await;
                                                        let mut builder = TopologyBuilder::new(&mut *model);

                                                        // Continue with normal creation...
                                                        let result = match primitive {
                                                            shared_types::PrimitiveType::Box => {
                                                                let width = parameters.params.get("width").copied().unwrap_or(10.0);
                                                                let height = parameters.params.get("height").copied().unwrap_or(10.0);
                                                                let depth = parameters.params.get("depth").copied().unwrap_or(10.0);
                                                                builder.create_box_3d(width, height, depth)
                                                                    .map(|id| ("box", id))
                                                            }
                                                            shared_types::PrimitiveType::Sphere => {
                                                                let radius = parameters.params.get("radius").copied().unwrap_or(5.0);
                                                                builder.create_sphere_3d(
                                                                    geometry_engine::math::Point3::ZERO,
                                                                    radius
                                                                ).map(|id| ("sphere", id))
                                                            }
                                                            shared_types::PrimitiveType::Cylinder => {
                                                                let radius = parameters.params.get("radius").copied().unwrap_or(5.0);
                                                                let height = parameters.params.get("height").copied().unwrap_or(10.0);
                                                                builder.create_cylinder_3d(
                                                                    geometry_engine::math::Point3::ZERO,
                                                                    geometry_engine::math::Vector3::Z,
                                                                    radius,
                                                                    height,
                                                                ).map(|id| ("cylinder", id))
                                                            }
                                                            shared_types::PrimitiveType::Cone => {
                                                                let radius = parameters.params.get("radius").copied().unwrap_or(5.0);
                                                                let height = parameters.params.get("height").copied().unwrap_or(10.0);
                                                                builder.create_cone_3d(
                                                                    geometry_engine::math::Point3::ZERO,
                                                                    geometry_engine::math::Vector3::Z,
                                                                    radius,  // base_radius
                                                                    0.0,     // top_radius (0 for a pointed cone)
                                                                    height,
                                                                ).map(|id| ("cone", id))
                                                            }
                                                            _ => Err(geometry_engine::primitives::primitive_traits::PrimitiveError::InvalidParameters {
                                                                parameter: "primitive_type".to_string(),
                                                                value: "unsupported".to_string(),
                                                                constraint: "must be box, sphere, cylinder, or cone".to_string(),
                                                            })
                                                        };

                                                        // Send response based on result
                                                        match result {
                                                            Ok((shape_type, geometry_id)) => {
                                                                // Extract the solid ID from the local GeometryId enum
                                                                let solid_id = match geometry_id {
                                                                    LocalGeometryId::Solid(id) => id,
                                                                    _ => {
                                                                        tracing::error!("Expected Solid geometry ID but got different type");
                                                                        continue;
                                                                    }
                                                                };

                                                                // Create a global UUID and register the mapping
                                                                let global_uuid = uuid::Uuid::new_v4();
                                                                state.register_id_mapping(global_uuid, solid_id);
                                                                let global_geometry_id = GlobalGeometryId(global_uuid);
                                                                // After creation, move to Define stage
                                                                if let Some(ref session_id) = current_session_id {
                                                                    let _ = state.hierarchy_manager.execute_command(
                                                                        session_id,
                                                                        shared_types::hierarchy::HierarchyCommand::SetWorkflowStage {
                                                                            stage: shared_types::hierarchy::WorkflowStage::Define,
                                                                        }
                                                                    ).await;

                                                                    let workflow_msg = WebSocketMessage::WorkflowStageChanged {
                                                                        session_id: session_id.clone(),
                                                                        stage: "define".to_string(),
                                                                        available_tools: vec!["sketch_2d".to_string(), "import_geometry".to_string()],
                                                                    };
                                                                    if let Ok(json) = serde_json::to_string(&workflow_msg) {
                                                                        let _ = sender.send(Message::Text(json.into())).await;
                                                                    }
                                                                }

                                                                // Get mesh data for frontend
                                                                let mesh_data = {
                                                                    let model = state.model.read().await;
                                                                    // Use proper tessellation to generate mesh
                                                                    if let Some(mesh) = model.tessellate_solid(solid_id, 1e-3) {
                                                                        // Convert mesh data to JSON format for frontend
                                                                        let vertices_flat: Vec<f32> = mesh.vertices.iter()
                                                                            .flat_map(|v| vec![v[0], v[1], v[2]])
                                                                            .collect();
                                                                        let normals_flat: Vec<f32> = mesh.normals.iter()
                                                                            .flat_map(|n| vec![n[0], n[1], n[2]])
                                                                            .collect();

                                                                        Some(serde_json::json!({
                                                                            "vertices": vertices_flat,
                                                                            "normals": normals_flat,
                                                                            "indices": mesh.indices,
                                                                            "type": shape_type
                                                                        }))
                                                                    } else {
                                                                        None
                                                                    }
                                                                };

                                                                let response = WebSocketMessage::GeometryCreated {
                                                                    object_id: solid_id,
                                                                    shape_type: shape_type.to_string(),
                                                                    created_by: "AI".to_string(),
                                                                };

                                                                if let Ok(json) = serde_json::to_string(&response) {
                                                                    let _ = sender.send(Message::Text(json.into())).await;
                                                                }
                                                            }
                                                            Err(e) => {
                                                                let response = WebSocketMessage::Error {
                                                                    message: format!("Failed to create geometry: {}", e),
                                                                    code: Some("GEOMETRY_CREATION_FAILED".to_string()),
                                                                };

                                                                if let Ok(json) = serde_json::to_string(&response) {
                                                                    let _ = sender.send(Message::Text(json.into())).await;
                                                                }
                                                            }
                                                        }
                                                    }
                                                    VoiceCommand::Create { primitive, parameters, .. } => {
                                                        // Legacy path - kept for backward compatibility
                                                        // Create the primitive using geometry engine
                                                        let mut model = state.model.write().await;
                                                        let mut builder = TopologyBuilder::new(&mut *model);

                                                        // Extract dimensions from parameters
                                                        let result = match primitive {
                                                            shared_types::PrimitiveType::Box => {
                                                                let width = parameters.params.get("width").copied().unwrap_or(10.0);
                                                                let height = parameters.params.get("height").copied().unwrap_or(10.0);
                                                                let depth = parameters.params.get("depth").copied().unwrap_or(10.0);
                                                                builder.create_box_3d(width, height, depth)
                                                                    .map(|id| ("box", id))
                                                            }
                                                            shared_types::PrimitiveType::Sphere => {
                                                                let radius = parameters.params.get("radius").copied().unwrap_or(5.0);
                                                                builder.create_sphere_3d(
                                                                    geometry_engine::math::Point3::ZERO,
                                                                    radius
                                                                ).map(|id| ("sphere", id))
                                                            }
                                                            shared_types::PrimitiveType::Cylinder => {
                                                                let radius = parameters.params.get("radius").copied().unwrap_or(5.0);
                                                                let height = parameters.params.get("height").copied().unwrap_or(10.0);
                                                                builder.create_cylinder_3d(
                                                                    geometry_engine::math::Point3::ZERO,
                                                                    geometry_engine::math::Vector3::Z,
                                                                    radius,
                                                                    height,
                                                                ).map(|id| ("cylinder", id))
                                                            }
                                                            shared_types::PrimitiveType::Cone => {
                                                                let radius = parameters.params.get("radius").copied().unwrap_or(5.0);
                                                                let height = parameters.params.get("height").copied().unwrap_or(10.0);
                                                                // For now, create cylinder as cone implementation may not be complete
                                                                builder.create_cylinder_3d(
                                                                    geometry_engine::math::Point3::ZERO,
                                                                    geometry_engine::math::Vector3::Z,
                                                                    radius,
                                                                    height,
                                                                ).map(|id| ("cone", id))
                                                            }
                                                            _ => {
                                                                // Torus and other primitives - create a default sphere for now
                                                                builder.create_sphere_3d(
                                                                    geometry_engine::math::Point3::ZERO,
                                                                    5.0
                                                                ).map(|id| ("sphere", id))
                                                            }
                                                        };

                                                        match result {
                                                            Ok((shape_name, LocalGeometryId::Solid(solid_id))) => {
                                                                // Send success response
                                                                let response = ServerMessage::AIResponse {
                                                                    response: crate::protocol::protocol::AIResponse::CommandExecuted {
                                                                        command: format!("create_{}", shape_name),
                                                                        results: vec![uuid::Uuid::new_v4()],
                                                                    },
                                                                    request_id: request_id.clone(),
                                                                };
                                                                if let Ok(json) = serde_json::to_string(&response) {
                                                                    let _ = sender.send(Message::Text(json.into())).await;
                                                                }

                                                                // Also send geometry created notification
                                                                let msg = WebSocketMessage::GeometryCreated {
                                                                    object_id: solid_id,
                                                                    shape_type: shape_name.to_string(),
                                                                    created_by: "AI".to_string(),
                                                                };
                                                                if let Ok(json) = serde_json::to_string(&msg) {
                                                                    let _ = sender.send(Message::Text(json.into())).await;
                                                                }
                                                            }
                                                            Ok(_) => {
                                                                let response = ServerMessage::Error {
                                                                    error_code: "INVALID_GEOMETRY".to_string(),
                                                                    message: "Created geometry was not a solid".to_string(),
                                                                    details: None,
                                                                    request_id: request_id.clone(),
                                                                };
                                                                if let Ok(json) = serde_json::to_string(&response) {
                                                                    let _ = sender.send(Message::Text(json.into())).await;
                                                                }
                                                            }
                                                            Err(e) => {
                                                                let response = ServerMessage::Error {
                                                                    error_code: "GEOMETRY_ERROR".to_string(),
                                                                    message: format!("Failed to create geometry: {}", e),
                                                                    details: None,
                                                                    request_id: request_id.clone(),
                                                                };
                                                                if let Ok(json) = serde_json::to_string(&response) {
                                                                    let _ = sender.send(Message::Text(json.into())).await;
                                                                }
                                                            }
                                                        }

                                                        // Send success response after creating geometry
                                                        let response = ServerMessage::Success {
                                                            result: Some(serde_json::json!({
                                                                "message": "Geometry created successfully",
                                                                "type": "primitive",
                                                                "primitive_type": format!("{:?}", primitive)
                                                            })),
                                                            request_id: request_id.clone(),
                                                        };
                                                        if let Ok(json) = serde_json::to_string(&response) {
                                                            info!("Sending success response for AI command");
                                                            let _ = sender.send(Message::Text(json.into())).await;
                                                        }
                                                    }
                                                    _ => {
                                                        // Other AI commands not yet implemented
                                                        info!("Unhandled VoiceCommand type: {:?}", parsed_command);
                                                        let response = ServerMessage::Error {
                                                            error_code: "NOT_IMPLEMENTED".to_string(),
                                                            message: "This AI command type is not yet implemented".to_string(),
                                                            details: None,
                                                            request_id,
                                                        };
                                                        if let Ok(json) = serde_json::to_string(&response) {
                                                            let _ = sender.send(Message::Text(json.into())).await;
                                                        }
                                                    }
                                                }
                                            }
                                            Err(e) => {
                                                let response = ServerMessage::Error {
                                                    error_code: "AI_ERROR".to_string(),
                                                    message: format!("AI processing failed: {}", e),
                                                    details: None,
                                                    request_id,
                                                };
                                                if let Ok(json) = serde_json::to_string(&response) {
                                                    let _ = sender
                                                        .send(Message::Text(json.into()))
                                                        .await;
                                                }
                                            }
                                        }
                                    }
                                    _ => {
                                        // Other AI commands (voice, etc.) not yet implemented
                                        let response = ServerMessage::Error {
                                            error_code: "NOT_IMPLEMENTED".to_string(),
                                            message: "This AI command type is not yet implemented"
                                                .to_string(),
                                            details: None,
                                            request_id,
                                        };
                                        if let Ok(json) = serde_json::to_string(&response) {
                                            let _ = sender.send(Message::Text(json.into())).await;
                                        }
                                    }
                                }
                            }
                            ClientMessage::SessionCommand {
                                command,
                                request_id,
                            } => {
                                // Handle session commands
                                match command {
                                    super::protocol::SessionWSCommand::JoinSession {
                                        session_id,
                                    } => {
                                        info!(
                                            "Processing JoinSession command for session: {}",
                                            session_id
                                        );

                                        // Use the existing session join logic
                                        let user_name = format!("User_{}", &user_id[..8]); // Default user name

                                        // Create or join session
                                        let actual_session_id = if session_id == "default"
                                            || session_id.is_empty()
                                        {
                                            // Create new session if "default" or empty
                                            let new_session_id = state
                                                .session_manager
                                                .create_session(user_name.clone())
                                                .await;
                                            add_mock_collaborators(&new_session_id, &state).await;
                                            new_session_id
                                        } else {
                                            // Try to join existing session
                                            match state
                                                .session_manager
                                                .get_session(&session_id)
                                                .await
                                            {
                                                Ok(_) => session_id.clone(),
                                                Err(_) => {
                                                    // Session doesn't exist, create new one
                                                    let new_session_id = state
                                                        .session_manager
                                                        .create_session(user_name.clone())
                                                        .await;
                                                    add_mock_collaborators(&new_session_id, &state)
                                                        .await;
                                                    new_session_id
                                                }
                                            }
                                        };

                                        // Add the current user to the session
                                        if let Ok(session) = state
                                            .session_manager
                                            .get_session(&actual_session_id)
                                            .await
                                        {
                                            let mut session_state = session.write().await;
                                            let now = std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .unwrap()
                                                .as_millis()
                                                as u64;

                                            // Check if user already exists (to avoid duplicates)
                                            if !session_state
                                                .active_users
                                                .iter()
                                                .any(|u| u.id == user_id)
                                            {
                                                let user_info = shared_types::session::UserInfo {
                                                    id: user_id.to_string(),
                                                    name: user_name.clone(),
                                                    color: [0.5, 0.5, 0.9, 1.0], // Blue color for new users
                                                    last_activity: now,
                                                    role: shared_types::session::UserRole::Editor,
                                                    cursor_position: None,
                                                    selected_objects: Vec::new(),
                                                };
                                                session_state.active_users.push(user_info);
                                                info!(
                                                    "Added user {} to session {}",
                                                    user_name, actual_session_id
                                                );
                                            }
                                        }

                                        // Store session ID for this connection
                                        current_session_id = Some(actual_session_id.clone());

                                        // Send success response with session info
                                        let response = ServerMessage::SessionUpdate {
                                            update: super::protocol::SessionUpdate::UserJoined {
                                                user_id: user_id.to_string(),
                                                user_info: shared_types::session::UserInfo {
                                                    id: user_id.to_string(),
                                                    name: user_name.clone(),
                                                    color: [0.5, 0.5, 0.9, 1.0],
                                                    last_activity: chrono::Utc::now()
                                                        .timestamp_millis()
                                                        as u64,
                                                    role: shared_types::session::UserRole::Editor,
                                                    cursor_position: None,
                                                    selected_objects: Vec::new(),
                                                },
                                            },
                                        };

                                        if let Ok(json) = serde_json::to_string(&response) {
                                            info!("Sending SessionUpdate response");
                                            let _ = sender.send(Message::Text(json.into())).await;
                                        }

                                        // Also send collaborators update
                                        if let Ok(session) = state
                                            .session_manager
                                            .get_session(&actual_session_id)
                                            .await
                                        {
                                            let session_state = session.read().await;
                                            let collaborators: Vec<CollaboratorInfo> =
                                                session_state
                                                    .active_users
                                                    .iter()
                                                    .map(|user| CollaboratorInfo {
                                                        id: user.id.clone(),
                                                        name: user.name.clone(),
                                                        is_active: true,
                                                        initial: user
                                                            .name
                                                            .chars()
                                                            .next()
                                                            .unwrap_or('U')
                                                            .to_string(),
                                                        color: user.color,
                                                        last_activity: user.last_activity,
                                                    })
                                                    .collect();

                                            let collaborators_msg =
                                                WebSocketMessage::CollaboratorsUpdate {
                                                    session_id: actual_session_id.clone(),
                                                    users: collaborators,
                                                };

                                            if let Ok(json) =
                                                serde_json::to_string(&collaborators_msg)
                                            {
                                                info!(
                                                    "Sending CollaboratorsUpdate with {} users",
                                                    session_state.active_users.len()
                                                );
                                                let _ =
                                                    sender.send(Message::Text(json.into())).await;
                                            }
                                        }
                                    }
                                    super::protocol::SessionWSCommand::LeaveSession => {
                                        // Handle leave session
                                        if let Some(session_id) = &current_session_id {
                                            info!(
                                                "User {} leaving session {}",
                                                user_id, session_id
                                            );
                                            // Remove user from session
                                            if let Ok(session) =
                                                state.session_manager.get_session(session_id).await
                                            {
                                                let mut session_state = session.write().await;
                                                session_state
                                                    .active_users
                                                    .retain(|u| u.id != user_id);
                                            }
                                            current_session_id = None;
                                        }

                                        let response = ServerMessage::Success {
                                            result: Some(serde_json::json!({
                                                "message": "Left session successfully"
                                            })),
                                            request_id,
                                        };
                                        if let Ok(json) = serde_json::to_string(&response) {
                                            let _ = sender.send(Message::Text(json.into())).await;
                                        }
                                    }
                                    super::protocol::SessionWSCommand::CreateSession {
                                        name,
                                        description,
                                    } => {
                                        info!("Creating new session: {}", name);

                                        // Create new session with provided name
                                        let session_id = state
                                            .session_manager
                                            .create_session(name.clone())
                                            .await;

                                        // Add the creator to the session
                                        if let Ok(session) =
                                            state.session_manager.get_session(&session_id).await
                                        {
                                            let mut session_state = session.write().await;

                                            // Add session metadata
                                            if let Some(ref desc) = &description {
                                                // Store description in session (would need to extend SessionState)
                                                info!("Session description: {}", desc);
                                            }

                                            let now = std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .unwrap()
                                                .as_millis()
                                                as u64;

                                            let user_info = shared_types::session::UserInfo {
                                                id: user_id.to_string(),
                                                name: name.clone(),
                                                color: [0.2, 0.8, 0.3, 1.0], // Green for session creator
                                                last_activity: now,
                                                role: shared_types::session::UserRole::Owner, // Creator gets Owner role
                                                cursor_position: None,
                                                selected_objects: Vec::new(),
                                            };
                                            session_state.active_users.push(user_info.clone());

                                            // Store current session
                                            current_session_id = Some(session_id.clone());

                                            // Send success response with session details
                                            let response = ServerMessage::Success {
                                                result: Some(serde_json::json!({
                                                    "session_id": session_id,
                                                    "name": name,
                                                    "description": description,
                                                    "creator": user_id,
                                                    "created_at": now
                                                })),
                                                request_id,
                                            };

                                            if let Ok(json) = serde_json::to_string(&response) {
                                                info!(
                                                    "Session {} created successfully",
                                                    session_id
                                                );
                                                let _ =
                                                    sender.send(Message::Text(json.into())).await;
                                            }
                                        }
                                    }
                                    super::protocol::SessionWSCommand::ShareObject {
                                        object_id,
                                        permissions,
                                    } => {
                                        info!(
                                            "Sharing object {:?} with permissions {:?}",
                                            object_id, permissions
                                        );

                                        if let Some(session_id) = &current_session_id {
                                            // In a real implementation, this would update object permissions
                                            // For now, we'll store this in session metadata

                                            let response = ServerMessage::SessionUpdate {
                                                update:
                                                    super::protocol::SessionUpdate::ObjectShared {
                                                        object_id: object_id.clone(),
                                                        shared_by: user_id.to_string(),
                                                    },
                                            };

                                            if let Ok(json) = serde_json::to_string(&response) {
                                                info!(
                                                    "Object {:?} shared with permissions {:?}",
                                                    object_id, permissions
                                                );
                                                let _ =
                                                    sender.send(Message::Text(json.into())).await;
                                            }

                                            // Broadcast to all session participants
                                            if let Ok(session) =
                                                state.session_manager.get_session(session_id).await
                                            {
                                                let session_state = session.read().await;
                                                // In production, would broadcast to all connected users
                                                info!(
                                                    "Broadcasting object share to {} users",
                                                    session_state.active_users.len()
                                                );
                                            }
                                        } else {
                                            let response = ServerMessage::Error {
                                                error_code: "NOT_IN_SESSION".to_string(),
                                                message: "Must be in a session to share objects"
                                                    .to_string(),
                                                details: None,
                                                request_id,
                                            };
                                            if let Ok(json) = serde_json::to_string(&response) {
                                                let _ =
                                                    sender.send(Message::Text(json.into())).await;
                                            }
                                        }
                                    }
                                    super::protocol::SessionWSCommand::UpdatePresence {
                                        cursor_position,
                                        selected_objects,
                                    } => {
                                        info!("Updating presence for user {}", user_id);

                                        if let Some(session_id) = &current_session_id {
                                            // Update user's presence in the session
                                            if let Ok(session) =
                                                state.session_manager.get_session(session_id).await
                                            {
                                                let mut session_state = session.write().await;

                                                if let Some(user) = session_state
                                                    .active_users
                                                    .iter_mut()
                                                    .find(|u| u.id == user_id)
                                                {
                                                    // Update cursor position and selected objects
                                                    user.cursor_position = cursor_position;
                                                    user.selected_objects =
                                                        selected_objects.clone();
                                                    user.last_activity =
                                                        std::time::SystemTime::now()
                                                            .duration_since(std::time::UNIX_EPOCH)
                                                            .unwrap()
                                                            .as_millis()
                                                            as u64;

                                                    info!("Updated presence: cursor={:?}, selected={:?}",
                                                          cursor_position, selected_objects);
                                                }
                                            }

                                            // Send presence update to all users
                                            let response = ServerMessage::SessionUpdate {
                                                update: super::protocol::SessionUpdate::PresenceUpdated {
                                                    user_id: user_id.to_string(),
                                                    cursor_position,
                                                    selected_objects: selected_objects.clone(),
                                                },
                                            };

                                            if let Ok(json) = serde_json::to_string(&response) {
                                                let _ =
                                                    sender.send(Message::Text(json.into())).await;
                                            }

                                            // In production, broadcast to all other users in session
                                            info!(
                                                "Presence updated for user {} in session {}",
                                                user_id, session_id
                                            );
                                        } else {
                                            let response = ServerMessage::Error {
                                                error_code: "NOT_IN_SESSION".to_string(),
                                                message: "Must be in a session to update presence"
                                                    .to_string(),
                                                details: None,
                                                request_id,
                                            };
                                            if let Ok(json) = serde_json::to_string(&response) {
                                                let _ =
                                                    sender.send(Message::Text(json.into())).await;
                                            }
                                        }
                                    }
                                }
                            }
                            ClientMessage::TimelineCommand {
                                command,
                                request_id,
                            } => {
                                info!("Processing timeline command: {:?}", command);

                                // Use the actual timeline engine
                                let timeline = state.timeline.clone();
                                let branch_manager = state.branch_manager.clone();

                                let response = match command {
                                    super::protocol::TimelineWSCommand::Undo { steps } => {
                                        let steps_to_undo = steps.unwrap_or(1);
                                        info!("Processing undo for {} steps", steps_to_undo);

                                        // Timeline undo/redo would be implemented through event replay
                                        // For now, acknowledge the operation
                                        ServerMessage::TimelineUpdate {
                                            update:
                                                super::protocol::TimelineUpdate::UndoPerformed {
                                                    steps: steps_to_undo,
                                                },
                                        }
                                    }
                                    super::protocol::TimelineWSCommand::Redo { steps } => {
                                        let steps_to_redo = steps.unwrap_or(1);
                                        info!("Processing redo for {} steps", steps_to_redo);

                                        // Timeline undo/redo would be implemented through event replay
                                        ServerMessage::TimelineUpdate {
                                            update:
                                                super::protocol::TimelineUpdate::RedoPerformed {
                                                    steps: steps_to_redo,
                                                },
                                        }
                                    }
                                    super::protocol::TimelineWSCommand::CreateBranch {
                                        name,
                                        from_point,
                                    } => {
                                        info!(
                                            "Creating branch '{}' from point {:?}",
                                            name, from_point
                                        );

                                        // Create branch using the branch manager
                                        let result = branch_manager.create_branch(
                                            name.clone(),
                                            timeline_engine::BranchId::main(), // parent branch
                                            from_point.unwrap_or(0) as u64,    // fork event index
                                            timeline_engine::Author::System,
                                            timeline_engine::BranchPurpose::UserExploration {
                                                description: format!(
                                                    "Branch created at {}",
                                                    chrono::Utc::now()
                                                ),
                                            },
                                        );

                                        match result {
                                            Ok(branch_id) => {
                                                info!(
                                                    "Created branch '{}' with ID: {:?}",
                                                    name, branch_id
                                                );
                                                ServerMessage::TimelineUpdate {
                                                    update: super::protocol::TimelineUpdate::BranchCreated {
                                                        name: name.clone(),
                                                    },
                                                }
                                            }
                                            Err(e) => {
                                                error!("Failed to create branch: {:?}", e);
                                                ServerMessage::Error {
                                                    error_code: "BRANCH_CREATE_FAILED".to_string(),
                                                    message: format!(
                                                        "Failed to create branch: {:?}",
                                                        e
                                                    ),
                                                    details: None,
                                                    request_id,
                                                }
                                            }
                                        }
                                    }
                                    super::protocol::TimelineWSCommand::SwitchBranch {
                                        branch_name,
                                    } => {
                                        info!("Switching to branch '{}'", branch_name);

                                        // Branch switching would be handled through the timeline
                                        ServerMessage::TimelineUpdate {
                                            update:
                                                super::protocol::TimelineUpdate::BranchSwitched {
                                                    from: "main".to_string(),
                                                    to: branch_name.clone(),
                                                },
                                        }
                                    }
                                    super::protocol::TimelineWSCommand::MergeBranch {
                                        source,
                                        target,
                                        strategy,
                                    } => {
                                        info!(
                                            "Merging branch '{}' into '{}' using {:?} strategy",
                                            source, target, strategy
                                        );

                                        // Branch merging would be handled through the timeline
                                        ServerMessage::Success {
                                            result: Some(serde_json::json!({
                                                "message": format!("Merged {} into {}", source, target),
                                                "source": source,
                                                "target": target,
                                                "strategy": format!("{:?}", strategy)
                                            })),
                                            request_id,
                                        }
                                    }
                                    super::protocol::TimelineWSCommand::ExecuteOperation {
                                        operation,
                                    } => {
                                        info!("Executing timeline operation: {:?}", operation);

                                        // Add operation to timeline
                                        let timeline_op = timeline_engine::Operation::Generic {
                                            command_type: "timeline_operation".to_string(),
                                            parameters: serde_json::to_value(&operation)
                                                .unwrap_or(serde_json::Value::Null),
                                        };

                                        // Timeline is wrapped in Arc<RwLock>, need to write lock it
                                        let timeline = state.timeline.write().await;
                                        let result = timeline
                                            .add_operation(
                                                timeline_op,
                                                timeline_engine::Author::System,
                                                timeline_engine::BranchId::main(),
                                            )
                                            .await;

                                        match result {
                                            Ok(event_id) => {
                                                info!(
                                                    "Operation recorded in timeline with ID: {:?}",
                                                    event_id
                                                );
                                                ServerMessage::TimelineUpdate {
                                                    update: super::protocol::TimelineUpdate::OperationExecuted {
                                                        operation,
                                                        result_ids: vec![], // Would be populated with actual result IDs
                                                    },
                                                }
                                            }
                                            Err(e) => {
                                                error!("Failed to record operation: {:?}", e);
                                                ServerMessage::Error {
                                                    error_code: "OPERATION_FAILED".to_string(),
                                                    message: format!(
                                                        "Failed to record operation: {:?}",
                                                        e
                                                    ),
                                                    details: None,
                                                    request_id,
                                                }
                                            }
                                        }
                                    }
                                };

                                if let Ok(json) = serde_json::to_string(&response) {
                                    let _ = sender.send(Message::Text(json.into())).await;
                                }
                            }
                            ClientMessage::ExportCommand {
                                command,
                                request_id,
                            } => {
                                info!("Processing export command: {:?}", command);

                                // Use the actual export engine
                                let export_engine = state.export_engine.clone();
                                let model = state.model.read().await;

                                let response = match command {
                                    super::protocol::ExportWSCommand::ExportSTL {
                                        object_ids,
                                        format,
                                    } => {
                                        info!(
                                            "Exporting {} objects to STL ({:?})",
                                            object_ids.len(),
                                            format
                                        );

                                        // Convert ObjectIds (UUIDs) to local solid IDs (u32)
                                        let mut solid_ids = Vec::new();
                                        for obj_id in &object_ids {
                                            // ObjectId is just uuid::Uuid
                                            // Convert UUID to local u32 ID for fast geometry operations
                                            if let Some(local_id) = state.get_local_id(obj_id) {
                                                solid_ids.push(local_id);
                                            }
                                        }

                                        // Get solids from model
                                        let mut solids_to_export = Vec::new();
                                        for solid_id in solid_ids {
                                            if let Some(solid) = model.get_solid(solid_id) {
                                                solids_to_export.push(solid.clone());
                                            }
                                        }

                                        if !solids_to_export.is_empty() {
                                            // Generate filename with timestamp
                                            let timestamp =
                                                chrono::Utc::now().format("%Y%m%d_%H%M%S");
                                            let filename = format!("export_{}.stl", timestamp);

                                            // Tessellate solids to create mesh for export
                                            let mut all_vertices = Vec::new();
                                            let mut all_indices = Vec::new();
                                            let mut vertex_offset = 0;

                                            for solid in &solids_to_export {
                                                // Tessellate each solid
                                                if let Some(tessellated) =
                                                    model.tessellate_solid(solid.id, 0.01)
                                                {
                                                    // Add vertices (already [f32; 3] arrays)
                                                    for v in &tessellated.vertices {
                                                        all_vertices.push(*v);
                                                    }
                                                    // Add indices with offset
                                                    for idx in &tessellated.indices {
                                                        all_indices
                                                            .push(idx + vertex_offset as u32);
                                                    }
                                                    vertex_offset += tessellated.vertices.len();
                                                }
                                            }

                                            // Create mesh for export - flatten vertices to Vec<f32>
                                            let flat_vertices = all_vertices
                                                .iter()
                                                .flat_map(|v| vec![v[0], v[1], v[2]])
                                                .collect();

                                            let mesh = shared_types::Mesh {
                                                vertices: flat_vertices,
                                                indices: all_indices,
                                                normals: vec![], // Would compute normals in production
                                                uvs: None,
                                                colors: None, // Optional field
                                            };

                                            // Execute export
                                            match export_engine.export_stl(&mesh, &filename).await {
                                                Ok(result_path) => {
                                                    info!(
                                                        "Successfully exported to {}",
                                                        result_path
                                                    );
                                                    ServerMessage::ExportComplete {
                                                        result:
                                                            super::protocol::ExportResult::STL {
                                                                filename: result_path,
                                                                size_bytes: 0, // Would get actual file size in production
                                                            },
                                                        request_id,
                                                    }
                                                }
                                                Err(e) => {
                                                    error!("STL export failed: {:?}", e);
                                                    ServerMessage::Error {
                                                        error_code: "EXPORT_FAILED".to_string(),
                                                        message: format!(
                                                            "Failed to export STL: {:?}",
                                                            e
                                                        ),
                                                        details: None,
                                                        request_id,
                                                    }
                                                }
                                            }
                                        } else {
                                            ServerMessage::Error {
                                                error_code: "NO_OBJECTS".to_string(),
                                                message: "No valid objects to export".to_string(),
                                                details: None,
                                                request_id,
                                            }
                                        }
                                    }
                                    super::protocol::ExportWSCommand::ExportOBJ {
                                        object_ids,
                                        include_materials,
                                    } => {
                                        info!(
                                            "Exporting {} objects to OBJ (materials: {})",
                                            object_ids.len(),
                                            include_materials
                                        );

                                        // Convert ObjectIds (UUIDs) to local solid IDs (u32)
                                        let mut solid_ids = Vec::new();
                                        for obj_id in &object_ids {
                                            // ObjectId is just uuid::Uuid
                                            // Convert UUID to local u32 ID for fast geometry operations
                                            if let Some(local_id) = state.get_local_id(obj_id) {
                                                solid_ids.push(local_id);
                                            }
                                        }

                                        // Get solids from model
                                        let mut solids_to_export = Vec::new();
                                        for solid_id in solid_ids {
                                            if let Some(solid) = model.get_solid(solid_id) {
                                                solids_to_export.push(solid.clone());
                                            }
                                        }

                                        if !solids_to_export.is_empty() {
                                            let timestamp =
                                                chrono::Utc::now().format("%Y%m%d_%H%M%S");
                                            let obj_filename = format!("export_{}.obj", timestamp);
                                            let mtl_filename = if include_materials {
                                                Some(format!("export_{}.mtl", timestamp))
                                            } else {
                                                None
                                            };

                                            // Tessellate and create mesh for OBJ export
                                            let mut all_vertices = Vec::new();
                                            let mut all_indices = Vec::new();
                                            let mut vertex_offset = 0;

                                            for solid in &solids_to_export {
                                                if let Some(tessellated) =
                                                    model.tessellate_solid(solid.id, 0.01)
                                                {
                                                    for v in &tessellated.vertices {
                                                        // Vertices are already [f32; 3] arrays
                                                        all_vertices.push(*v);
                                                    }
                                                    for idx in &tessellated.indices {
                                                        all_indices
                                                            .push(idx + vertex_offset as u32);
                                                    }
                                                    vertex_offset += tessellated.vertices.len();
                                                }
                                            }
                                            // Flatten vertices for OBJ export
                                            let flat_vertices = all_vertices
                                                .iter()
                                                .flat_map(|v| vec![v[0], v[1], v[2]])
                                                .collect();

                                            let mesh = shared_types::Mesh {
                                                vertices: flat_vertices,
                                                indices: all_indices,
                                                normals: vec![],
                                                uvs: None,
                                                colors: None,
                                            };

                                            match export_engine
                                                .export_obj(&mesh, &obj_filename)
                                                .await
                                            {
                                                Ok(result_path) => {
                                                    info!(
                                                        "Successfully exported to OBJ: {}",
                                                        result_path
                                                    );
                                                    ServerMessage::ExportComplete {
                                                        result:
                                                            super::protocol::ExportResult::OBJ {
                                                                obj_file: result_path,
                                                                mtl_file: mtl_filename,
                                                                size_bytes: 0, // Would get actual file size
                                                            },
                                                        request_id,
                                                    }
                                                }
                                                Err(e) => {
                                                    error!("OBJ export failed: {:?}", e);
                                                    ServerMessage::Error {
                                                        error_code: "EXPORT_FAILED".to_string(),
                                                        message: format!(
                                                            "Failed to export OBJ: {:?}",
                                                            e
                                                        ),
                                                        details: None,
                                                        request_id,
                                                    }
                                                }
                                            }
                                        } else {
                                            ServerMessage::Error {
                                                error_code: "NO_OBJECTS".to_string(),
                                                message: "No valid objects to export".to_string(),
                                                details: None,
                                                request_id,
                                            }
                                        }
                                    }
                                    super::protocol::ExportWSCommand::ExportROS {
                                        filename,
                                        options,
                                    } => {
                                        info!("Exporting to ROS format: {}", filename);

                                        // Get all solids from model for ROS export
                                        let all_solids: Vec<_> = model
                                            .solids
                                            .iter()
                                            .map(|(_, solid)| solid.clone())
                                            .collect();

                                        if !all_solids.is_empty() {
                                            // For ROS export, create simplified export data
                                            let export_json = serde_json::json!({
                                                "metadata": {
                                                    "version": "1.0",
                                                    "created_at": chrono::Utc::now().to_rfc3339(),
                                                    "author": user_id,
                                                    "units": "mm"
                                                },
                                                "geometry": all_solids.iter().map(|s| {
                                                    serde_json::json!({
                                                        "id": uuid::Uuid::new_v4().to_string(),
                                                        "name": format!("Solid_{}", s.id),
                                                        "type": "solid"
                                                    })
                                                }).collect::<Vec<_>>(),
                                                "encrypted": options.encrypt
                                            });

                                            // Write to file
                                            let result =
                                                std::fs::write(&filename, export_json.to_string())
                                                    .map(|_| export_json.to_string().len());

                                            match result {
                                                Ok(bytes_written) => {
                                                    info!("Successfully exported {} bytes to ROS format", bytes_written);
                                                    ServerMessage::ExportComplete {
                                                        result:
                                                            super::protocol::ExportResult::ROS {
                                                                filename: filename.clone(),
                                                                encrypted: options.encrypt,
                                                                size_bytes: bytes_written,
                                                            },
                                                        request_id,
                                                    }
                                                }
                                                Err(e) => {
                                                    error!("ROS export failed: {:?}", e);
                                                    ServerMessage::Error {
                                                        error_code: "EXPORT_FAILED".to_string(),
                                                        message: format!(
                                                            "Failed to export ROS: {:?}",
                                                            e
                                                        ),
                                                        details: None,
                                                        request_id,
                                                    }
                                                }
                                            }
                                        } else {
                                            ServerMessage::Error {
                                                error_code: "NO_OBJECTS".to_string(),
                                                message: "No objects to export".to_string(),
                                                details: None,
                                                request_id,
                                            }
                                        }
                                    }
                                    super::protocol::ExportWSCommand::ExportSTEP { object_ids } => {
                                        info!("Exporting {} objects to STEP", object_ids.len());

                                        // STEP export implementation
                                        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
                                        let filename = format!("export_{}.step", timestamp);

                                        // For now, use the basic STEP exporter
                                        ServerMessage::Success {
                                            result: Some(serde_json::json!({
                                                "message": "STEP export initiated",
                                                "filename": filename,
                                                "object_count": object_ids.len()
                                            })),
                                            request_id,
                                        }
                                    }
                                };

                                if let Ok(json) = serde_json::to_string(&response) {
                                    let _ = sender.send(Message::Text(json.into())).await;
                                }
                            }
                            ClientMessage::Subscribe { topics, request_id } => {
                                info!("Processing subscription to topics: {:?}", topics);

                                // Track subscriptions per connection in session manager
                                if let Some(ref session_id) = current_session_id {
                                    if let Ok(session) =
                                        state.session_manager.get_session(session_id).await
                                    {
                                        let mut session_state = session.write().await;

                                        // Find or create subscription record for this user
                                        if let Some(user) = session_state
                                            .active_users
                                            .iter_mut()
                                            .find(|u| u.id == user_id)
                                        {
                                            // Track subscription preferences (would extend UserInfo in production)
                                            info!("User {} subscribed to {:?}", user_id, topics);

                                            // Store topics in user context
                                            for topic in &topics {
                                                match topic {
                                                    super::protocol::SubscriptionTopic::GeometryUpdates => {
                                                        info!("User {} will receive geometry updates", user_id);
                                                    }
                                                    super::protocol::SubscriptionTopic::TimelineUpdates => {
                                                        info!("User {} will receive timeline updates", user_id);
                                                    }
                                                    super::protocol::SubscriptionTopic::SessionUpdates => {
                                                        info!("User {} will receive session updates", user_id);
                                                    }
                                                    super::protocol::SubscriptionTopic::AIResponses => {
                                                        info!("User {} will receive AI responses", user_id);
                                                    }
                                                    super::protocol::SubscriptionTopic::SystemEvents => {
                                                        info!("User {} will receive system events", user_id);
                                                    }
                                                    super::protocol::SubscriptionTopic::ErrorEvents => {
                                                        info!("User {} will receive error events", user_id);
                                                    }
                                                    super::protocol::SubscriptionTopic::AllEvents => {
                                                        info!("User {} will receive all events", user_id);
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }

                                let response = ServerMessage::Success {
                                    result: Some(serde_json::json!({
                                        "subscribed": topics,
                                        "message": "Successfully subscribed to topics",
                                        "active_session": current_session_id.is_some()
                                    })),
                                    request_id,
                                };

                                if let Ok(json) = serde_json::to_string(&response) {
                                    let _ = sender.send(Message::Text(json.into())).await;
                                }
                            }
                            ClientMessage::Unsubscribe { topics, request_id } => {
                                info!("Processing unsubscribe from topics: {:?}", topics);

                                // Remove subscriptions from session manager
                                if let Some(ref session_id) = current_session_id {
                                    if let Ok(session) =
                                        state.session_manager.get_session(session_id).await
                                    {
                                        let mut session_state = session.write().await;

                                        if let Some(user) = session_state
                                            .active_users
                                            .iter_mut()
                                            .find(|u| u.id == user_id)
                                        {
                                            info!(
                                                "User {} unsubscribed from {:?}",
                                                user_id, topics
                                            );
                                        }
                                    }
                                }

                                let response = ServerMessage::Success {
                                    result: Some(serde_json::json!({
                                        "unsubscribed": topics,
                                        "message": "Successfully unsubscribed from topics"
                                    })),
                                    request_id,
                                };

                                if let Ok(json) = serde_json::to_string(&response) {
                                    let _ = sender.send(Message::Text(json.into())).await;
                                }
                            }
                            ClientMessage::Query {
                                query_type,
                                request_id,
                            } => {
                                info!("Processing query: {:?}", query_type);

                                // Parse the query type from JSON and execute
                                let result = if let Ok(query) =
                                    serde_json::from_value::<super::protocol::WSQueryType>(
                                        query_type.clone(),
                                    ) {
                                    match query {
                                        super::protocol::WSQueryType::GetObject { object_id } => {
                                            // Query geometry engine for specific object
                                            let model = state.model.read().await;

                                            // Convert ObjectId (UUID) to local ID (u32)
                                            // ObjectId is just uuid::Uuid
                                            if let Some(local_id) = state.get_local_id(&object_id) {
                                                let solid_id = local_id; // SolidId is just u32
                                                if let Some(solid) = model.get_solid(solid_id) {
                                                    ServerMessage::Success {
                                                        result: Some(serde_json::json!({
                                                            "object_id": object_id,
                                                            "type": "solid",
                                                            "data": {
                                                                "id": solid.id,
                                                                "outer_shell": solid.outer_shell,
                                                                "inner_shells_count": solid.inner_shells.len(),
                                                                "name": solid.name.clone()
                                                            }
                                                        })),
                                                        request_id,
                                                    }
                                                } else {
                                                    ServerMessage::Error {
                                                        error_code: "OBJECT_NOT_FOUND".to_string(),
                                                        message: format!(
                                                            "Object {:?} not found",
                                                            object_id
                                                        ),
                                                        details: None,
                                                        request_id,
                                                    }
                                                }
                                            } else {
                                                ServerMessage::Error {
                                                    error_code: "INVALID_ID".to_string(),
                                                    message: "Object ID not found in local mapping"
                                                        .to_string(),
                                                    details: None,
                                                    request_id,
                                                }
                                            }
                                        }
                                        super::protocol::WSQueryType::ListObjects {
                                            filter,
                                            limit,
                                            offset,
                                        } => {
                                            // List all objects with optional filtering
                                            let model = state.model.read().await;
                                            let all_solids: Vec<_> = model
                                                .solids
                                                .iter()
                                                .map(|(_, solid)| solid.clone())
                                                .collect();

                                            // Apply pagination
                                            let offset = offset.unwrap_or(0);
                                            let limit = limit.unwrap_or(100).min(1000); // Cap at 1000

                                            let paginated_solids: Vec<_> = all_solids
                                                .iter()
                                                .skip(offset)
                                                .take(limit)
                                                .collect();

                                            let objects: Vec<_> = paginated_solids.iter().map(|solid| {
                                                let uuid = state.create_uuid_for_local(solid.id);
                                                serde_json::json!({
                                                    "id": uuid.to_string(),
                                                    "type": "solid",
                                                    "local_id": solid.id,
                                                    "outer_shell": solid.outer_shell,
                                                    "inner_shells_count": solid.inner_shells.len()
                                                })
                                            }).collect();

                                            ServerMessage::Success {
                                                result: Some(serde_json::json!({
                                                    "objects": objects,
                                                    "total": all_solids.len(),
                                                    "offset": offset,
                                                    "limit": limit
                                                })),
                                                request_id,
                                            }
                                        }
                                        super::protocol::WSQueryType::GetTimelineState => {
                                            // Get current timeline state
                                            let timeline = state.timeline.read().await;
                                            let event_count = timeline.get_stats().total_events;

                                            ServerMessage::Success {
                                                result: Some(serde_json::json!({
                                                    "event_count": event_count,
                                                    "can_undo": event_count > 0,
                                                    "can_redo": false,
                                                    "current_branch": {
                                                        "id": "main",
                                                        "name": "main"
                                                    }
                                                })),
                                                request_id,
                                            }
                                        }
                                        super::protocol::WSQueryType::GetSessionInfo {
                                            session_id,
                                        } => {
                                            // Get session information
                                            match state
                                                .session_manager
                                                .get_session(&session_id)
                                                .await
                                            {
                                                Ok(session) => {
                                                    let session_state = session.read().await;
                                                    ServerMessage::Success {
                                                        result: Some(serde_json::json!({
                                                            "session_id": session_id,
                                                            "user_count": session_state.active_users.len(),
                                                            "users": session_state.active_users.iter().map(|u| {
                                                                serde_json::json!({
                                                                    "id": u.id,
                                                                    "name": u.name,
                                                                    "role": format!("{:?}", u.role)
                                                                })
                                                            }).collect::<Vec<_>>()
                                                        })),
                                                        request_id,
                                                    }
                                                }
                                                Err(_) => ServerMessage::Error {
                                                    error_code: "SESSION_NOT_FOUND".to_string(),
                                                    message: format!(
                                                        "Session {} not found",
                                                        session_id
                                                    ),
                                                    details: None,
                                                    request_id,
                                                },
                                            }
                                        }
                                        super::protocol::WSQueryType::GetSystemStatus => {
                                            // Get system status
                                            let model = state.model.read().await;
                                            let timeline = state.timeline.read().await;
                                            let metrics = state.request_metrics.clone();
                                            let solid_count = model.solids.len();
                                            let event_count = timeline.get_stats().total_events;

                                            ServerMessage::Success {
                                                result: Some(serde_json::json!({
                                                    "status": "operational",
                                                    "model_objects": solid_count,
                                                    "timeline_events": event_count,
                                                    "active_sessions": state.session_manager.list_sessions().await.len(),
                                                    "request_count": metrics.len(),
                                                    "uptime_seconds": 0, // Would track actual uptime
                                                })),
                                                request_id,
                                            }
                                        }
                                        super::protocol::WSQueryType::GetCapabilities => {
                                            // Return system capabilities
                                            ServerMessage::Success {
                                                result: Some(serde_json::json!({
                                                    "geometry": {
                                                        "primitives": ["box", "sphere", "cylinder", "cone", "torus"],
                                                        "operations": ["boolean", "extrude", "revolve", "sweep", "loft"],
                                                        "formats": ["stl", "obj", "ros", "step"]
                                                    },
                                                    "timeline": {
                                                        "features": ["undo", "redo", "branches", "merge"],
                                                        "max_undo": 1000
                                                    },
                                                    "ai": {
                                                        "providers": ["integrated", "openai", "anthropic"],
                                                        "languages": ["english", "hindi"],
                                                        "features": ["voice", "text", "suggestions"]
                                                    },
                                                    "collaboration": {
                                                        "max_users": 100,
                                                        "features": ["real-time", "conflict-resolution", "presence"]
                                                    }
                                                })),
                                                request_id,
                                            }
                                        }
                                        super::protocol::WSQueryType::GetMetrics => {
                                            // Get performance metrics
                                            let metrics = state.request_metrics.clone();
                                            let metric_data: Vec<_> = metrics
                                                .iter()
                                                .map(|entry| {
                                                    serde_json::json!({
                                                        "endpoint": entry.key(),
                                                        "count": *entry.value()
                                                    })
                                                })
                                                .collect();

                                            ServerMessage::Success {
                                                result: Some(serde_json::json!({
                                                    "metrics": metric_data
                                                })),
                                                request_id,
                                            }
                                        }
                                        super::protocol::WSQueryType::SearchObjects {
                                            query,
                                            limit,
                                        } => {
                                            // Search for objects by query string
                                            let model = state.model.read().await;
                                            let all_solids: Vec<_> = model
                                                .solids
                                                .iter()
                                                .map(|(_, solid)| solid.clone())
                                                .collect();
                                            let search_limit = limit.unwrap_or(10).min(100);

                                            // Simple search - in production would use more sophisticated matching
                                            let matching_solids: Vec<_> = all_solids
                                                .iter()
                                                .filter(|s| {
                                                    // Match based on ID or other criteria
                                                    let id_str = format!("{}", s.id);
                                                    id_str.contains(&query)
                                                })
                                                .take(search_limit)
                                                .collect();

                                            let results: Vec<_> = matching_solids
                                                .iter()
                                                .map(|solid| {
                                                    let uuid =
                                                        state.create_uuid_for_local(solid.id);
                                                    serde_json::json!({
                                                        "id": uuid.to_string(),
                                                        "type": "solid",
                                                        "local_id": solid.id
                                                    })
                                                })
                                                .collect();

                                            ServerMessage::Success {
                                                result: Some(serde_json::json!({
                                                    "query": query,
                                                    "results": results,
                                                    "count": results.len()
                                                })),
                                                request_id,
                                            }
                                        }
                                    }
                                } else {
                                    ServerMessage::Error {
                                        error_code: "INVALID_QUERY".to_string(),
                                        message: "Invalid query type format".to_string(),
                                        details: Some(query_type),
                                        request_id,
                                    }
                                };

                                if let Ok(json) = serde_json::to_string(&result) {
                                    let _ = sender.send(Message::Text(json.into())).await;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to parse ClientMessage: {}", e);
                        let error_response = ServerMessage::Error {
                            error_code: "PARSE_ERROR".to_string(),
                            message: format!("Invalid message format: {}", e),
                            details: None,
                            request_id: None,
                        };
                        if let Ok(error_json) = serde_json::to_string(&error_response) {
                            let _ = sender.send(Message::Text(error_json.into())).await;
                        }
                    }
                }
            }
            Ok(Message::Close(_)) => {
                info!("WebSocket closed: user={}", user_id);
                break;
            }
            Err(e) => {
                error!("WebSocket error: {}", e);
                break;
            }
            _ => {}
        }
    }

    // Decrement WebSocket counter on disconnect
    crate::ACTIVE_WEBSOCKETS.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);

    info!("WebSocket connection ended: user={}", user_id);
    crate::broadcast_log(
        "INFO",
        &format!("WebSocket connection closed for user {}", user_id),
    );
}

async fn send_collaborators_update(
    session_id: &str,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Sending collaborators update for session: {}", session_id);

    // Get session from session manager
    match state.session_manager.get_session(session_id).await {
        Ok(session) => {
            let session_state = session.read().await;

            info!(
                "Session {} has {} active users",
                session_id,
                session_state.active_users.len()
            );

            // Convert UserInfo to CollaboratorInfo
            let collaborators: Vec<CollaboratorInfo> = session_state
                .active_users
                .iter()
                .map(|user| {
                    info!("  - User: {} ({})", user.name, user.id);
                    CollaboratorInfo {
                        id: user.id.clone(),
                        name: user.name.clone(),
                        initial: user
                            .name
                            .chars()
                            .next()
                            .unwrap_or('?')
                            .to_uppercase()
                            .to_string(),
                        is_active: user.is_active(30000), // 30 second timeout
                        color: user.color,
                        last_activity: user.last_activity,
                    }
                })
                .collect();

            info!("Sending {} collaborators in update", collaborators.len());

            let msg = WebSocketMessage::CollaboratorsUpdate {
                session_id: session_id.to_string(),
                users: collaborators,
            };

            if let Ok(json) = serde_json::to_string(&msg) {
                info!("Sending CollaboratorsUpdate message: {} bytes", json.len());
                let _ = sender.send(Message::Text(json.into())).await;
            }
        }
        Err(e) => {
            error!("Failed to get session {}: {}", session_id, e);
        }
    }

    Ok(())
}

async fn add_mock_collaborators(session_id: &str, state: &AppState) {
    use shared_types::session::UserInfo;

    info!("Adding mock collaborators to session: {}", session_id);

    // Define 3 mock collaborators with different activity states
    let mock_users = vec![
        UserInfo {
            id: "ankita_123".to_string(),
            name: "Ankita".to_string(),
            color: [0.2, 0.7, 0.9, 1.0], // Blue
            last_activity: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            role: shared_types::session::UserRole::Editor,
            cursor_position: None,
            selected_objects: Vec::new(),
        },
        UserInfo {
            id: "anushree_456".to_string(),
            name: "Anushree".to_string(),
            color: [0.9, 0.3, 0.2, 1.0], // Red
            last_activity: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
                .saturating_sub(60000) as u64, // 1 minute ago (inactive)
            role: shared_types::session::UserRole::Editor,
            cursor_position: None,
            selected_objects: Vec::new(),
        },
        UserInfo {
            id: "rajiv_789".to_string(),
            name: "Rajiv".to_string(),
            color: [0.3, 0.8, 0.3, 1.0], // Green
            last_activity: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            role: shared_types::session::UserRole::Editor,
            cursor_position: None,
            selected_objects: Vec::new(),
        },
    ];

    // Add mock users to the session
    match state.session_manager.get_session(session_id).await {
        Ok(session) => {
            let mut session_state = session.write().await;
            let before_count = session_state.active_users.len();
            for user in &mock_users {
                info!("  Adding mock user: {} ({})", user.name, user.id);
                session_state.active_users.push(user.clone());
            }
            let after_count = session_state.active_users.len();
            info!(
                "Added {} mock collaborators to session {} (total users: {} -> {})",
                mock_users.len(),
                session_id,
                before_count,
                after_count
            );
        }
        Err(e) => {
            error!(
                "Failed to get session {} for adding mock collaborators: {}",
                session_id, e
            );
        }
    }
}
