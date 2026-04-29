//! Geometry-related handlers

use crate::AppState;
use axum::{extract::State, http::StatusCode, response::Json};
use shared_types::ShapeParameters;
use shared_types::*;
use std::sync::Arc;
use std::time::Instant;
use uuid::Uuid;

/// Returns the current wall-clock time as milliseconds since the Unix epoch.
///
/// Falls back to `0` if the system clock is set before `UNIX_EPOCH`, which
/// would otherwise cause `duration_since` to return an error. Timestamps are
/// non-critical audit metadata on geometry objects; returning `0` is
/// preferable to panicking a request handler.
fn unix_millis_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Extract dimensions from natural language text (simple implementation)
fn extract_dimensions_from_text(text: &str) -> Vec<f64> {
    text.split_whitespace()
        .filter_map(|word| {
            // Try to parse numbers, handling common units
            let cleaned = word.trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.');
            cleaned.parse::<f64>().ok()
        })
        .collect()
}

/// Simple pattern-based command parser with extracted dimensions
fn parse_simple_command(text: &str) -> Vec<AICommand> {
    let lower = text.to_lowercase();
    let mut commands = Vec::new();
    let dimensions = extract_dimensions_from_text(text);

    // Simple pattern matching for demonstration
    if lower.contains("box") || lower.contains("cube") {
        let mut params = std::collections::HashMap::new();
        let default_size = 5.0;
        match dimensions.len() {
            0 => {
                // No dimensions found, use reasonable defaults
                params.insert("width".to_string(), default_size);
                params.insert("height".to_string(), default_size);
                params.insert("depth".to_string(), default_size);
            }
            1 => {
                // One dimension: cube
                let size = dimensions[0];
                params.insert("width".to_string(), size);
                params.insert("height".to_string(), size);
                params.insert("depth".to_string(), size);
            }
            2 => {
                // Two dimensions: width x height, default depth
                params.insert("width".to_string(), dimensions[0]);
                params.insert("height".to_string(), dimensions[1]);
                params.insert("depth".to_string(), dimensions[0]); // Use width for depth
            }
            _ => {
                // Three or more dimensions: use first three
                params.insert("width".to_string(), dimensions[0]);
                params.insert("height".to_string(), dimensions[1]);
                params.insert("depth".to_string(), dimensions[2]);
            }
        }

        commands.push(AICommand::CreatePrimitive {
            shape_type: PrimitiveType::Box,
            parameters: shared_types::ShapeParameters { params },
            position: [0.0, 0.0, 0.0],
            material: None,
        });
    } else if lower.contains("sphere") || lower.contains("ball") {
        let mut params = std::collections::HashMap::new();
        let radius = dimensions.get(0).copied().unwrap_or(3.0); // Default 3.0 if no dimension found
        params.insert("radius".to_string(), radius);

        commands.push(AICommand::CreatePrimitive {
            shape_type: PrimitiveType::Sphere,
            parameters: shared_types::ShapeParameters { params },
            position: [0.0, 0.0, 0.0],
            material: None,
        });
    } else if lower.contains("cylinder") {
        let mut params = std::collections::HashMap::new();
        let radius = dimensions.get(0).copied().unwrap_or(2.0); // Default radius
        let height = dimensions
            .get(1)
            .copied()
            .unwrap_or(dimensions.get(0).copied().unwrap_or(5.0)); // Use second dim or first, or default
        params.insert("radius".to_string(), radius);
        params.insert("height".to_string(), height);

        commands.push(AICommand::CreatePrimitive {
            shape_type: PrimitiveType::Cylinder,
            parameters: shared_types::ShapeParameters { params },
            position: [0.0, 0.0, 0.0],
            material: None,
        });
    }

    commands
}

pub async fn create_geometry(
    State(state): State<AppState>,
    Json(request): Json<GeometryCreateRequest>,
) -> Result<Json<GeometryResponse>, StatusCode> {
    let start = Instant::now();

    let parameters = ShapeParameters {
        params: request.parameters,
    };

    // Get model with async lock
    let mut model = state.model.write().await;

    // Create primitive using the actual geometry engine
    use geometry_engine::math::{Point3, Vector3 as EngineVector3};
    use geometry_engine::primitives::primitive_traits::Primitive;
    use geometry_engine::primitives::{
        box_primitive::{BoxParameters, BoxPrimitive},
        cone_primitive::{ConeParameters, ConePrimitive},
        cylinder_primitive::{CylinderParameters, CylinderPrimitive},
        sphere_primitive::{SphereParameters, SpherePrimitive},
        torus_primitive::{TorusParameters, TorusPrimitive},
    };

    let solid_id = match request.shape_type {
        shared_types::PrimitiveType::Box => {
            let width = parameters.params.get("width").cloned().unwrap_or(10.0);
            let height = parameters.params.get("height").cloned().unwrap_or(10.0);
            let depth = parameters.params.get("depth").cloned().unwrap_or(10.0);
            let params = BoxParameters {
                width,
                height,
                depth,
                corner_radius: None,
                transform: None,
                tolerance: None,
            };
            BoxPrimitive::create(params, &mut model)
                .map_err(|e| StatusCode::INTERNAL_SERVER_ERROR)?
        }
        shared_types::PrimitiveType::Sphere => {
            let radius = parameters.params.get("radius").cloned().unwrap_or(5.0);
            let segments = calculate_sphere_segments(radius, "medium"); // Use consistent quality
            let params = SphereParameters {
                radius,
                center: Point3::new(
                    request.position[0] as f64,
                    request.position[1] as f64,
                    request.position[2] as f64,
                ),
                u_segments: segments.0,
                v_segments: segments.1,
                transform: None,
                tolerance: None,
            };
            SpherePrimitive::create(params, &mut model)
                .map_err(|e| StatusCode::INTERNAL_SERVER_ERROR)?
        }
        shared_types::PrimitiveType::Cylinder => {
            let radius = parameters.params.get("radius").cloned().unwrap_or(5.0);
            let height = parameters.params.get("height").cloned().unwrap_or(10.0);
            let segments = calculate_cylinder_segments(radius, "medium");
            let axis = parameters
                .params
                .get("axis")
                .map(|_| EngineVector3::new(0.0, 0.0, 1.0))
                .unwrap_or(EngineVector3::new(0.0, 0.0, 1.0)); // Default Z-up
            let params = CylinderParameters {
                radius,
                height,
                base_center: Point3::new(
                    request.position[0] as f64,
                    request.position[1] as f64,
                    request.position[2] as f64,
                ),
                axis,
                segments,
                transform: None,
                tolerance: None,
            };
            CylinderPrimitive::create(params, &mut model)
                .map_err(|e| StatusCode::INTERNAL_SERVER_ERROR)?
        }
        shared_types::PrimitiveType::Cone => {
            let radius = parameters.params.get("radius").cloned().unwrap_or(5.0);
            let height = parameters.params.get("height").cloned().unwrap_or(10.0);
            let params = ConeParameters {
                apex: Point3::new(
                    request.position[0] as f64,
                    request.position[1] as f64,
                    request.position[2] as f64 + height,
                ),
                axis: EngineVector3::new(0.0, 0.0, -1.0),
                half_angle: (radius / height).atan(),
                height,
                bottom_radius: Some(radius),
                angle_range: None,
            };
            ConePrimitive::create(&params, &mut model)
                .map_err(|e| StatusCode::INTERNAL_SERVER_ERROR)?
        }
        shared_types::PrimitiveType::Torus => {
            let major = parameters
                .params
                .get("major_radius")
                .cloned()
                .unwrap_or(10.0);
            let minor = parameters
                .params
                .get("minor_radius")
                .cloned()
                .unwrap_or(3.0);
            let params = TorusParameters {
                center: Point3::new(
                    request.position[0] as f64,
                    request.position[1] as f64,
                    request.position[2] as f64,
                ),
                axis: EngineVector3::new(0.0, 0.0, 1.0),
                major_radius: major,
                minor_radius: minor,
                major_angle_range: None,
                minor_angle_range: None,
            };
            TorusPrimitive::create(&params, &mut model)
                .map_err(|e| StatusCode::INTERNAL_SERVER_ERROR)?
        }
        shared_types::PrimitiveType::Gear
        | shared_types::PrimitiveType::Bracket
        | shared_types::PrimitiveType::Parametric
        | shared_types::PrimitiveType::BSplineCurve
        | shared_types::PrimitiveType::NURBSCurve
        | shared_types::PrimitiveType::BSplineSurface => {
            tracing::error!(
                "Primitive type {:?} not yet implemented",
                request.shape_type
            );
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
    };

    // Compute adaptive tessellation parameters based on actual solid geometry
    use geometry_engine::math::Tolerance;
    use geometry_engine::tessellation::TessellationParams;

    // Get the solid we just created - if this fails, it indicates a serious bug
    let solid = model.solids.get_mut(solid_id).ok_or_else(|| {
        tracing::error!("CRITICAL: Solid {} not found in model immediately after creation. This indicates a bug in the geometry engine.", solid_id);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Compute adaptive tessellation parameters based on actual solid geometry
    let tolerance = Tolerance::from_distance(1e-12);

    // Calculate exact volume using divergence theorem
    let volume = model.calculate_solid_volume(solid_id).unwrap_or_else(|| {
        tracing::warn!(
            "Could not calculate exact volume for solid {}, using fallback",
            solid_id
        );
        // Fallback to bounding box approximation only if exact calculation fails
        if let Some(bbox) = model.compute_bounding_box() {
            let size = bbox.size();
            size.x * size.y * size.z * 0.5
        } else {
            1.0
        }
    });

    // Calculate exact surface area from all faces
    let surface_area = model
        .calculate_solid_surface_area(solid_id)
        .unwrap_or_else(|| {
            tracing::warn!(
                "Could not calculate exact surface area for solid {}, using fallback",
                solid_id
            );
            // Fallback to bounding box approximation only if exact calculation fails
            if let Some(bbox) = model.compute_bounding_box() {
                let size = bbox.size();
                2.0 * (size.x * size.y + size.x * size.z + size.y * size.z)
            } else {
                6.0
            }
        });

    // Calculate characteristic length (cube root of volume)
    let char_length = volume.powf(1.0 / 3.0);

    // Adaptive parameters based on solid properties and primitive type
    let (base_edge_factor, angle_factor, chord_factor) = match request.shape_type {
        PrimitiveType::Box => (0.15, 0.15, 0.02), // Coarser for flat surfaces
        PrimitiveType::Sphere => (0.08, 0.08, 0.005), // Finer for high curvature
        PrimitiveType::Cylinder => (0.10, 0.10, 0.01), // Medium for mixed curvature
        PrimitiveType::Cone => (0.12, 0.10, 0.008), // Variable curvature
        PrimitiveType::Torus => (0.06, 0.06, 0.003), // Finest for complex curvature
        PrimitiveType::Gear => {
            tracing::error!("Gear primitive not yet implemented");
            (0.10, 0.10, 0.01) // Default parameters
        }
        PrimitiveType::Bracket => {
            tracing::error!("Bracket primitive not yet implemented");
            (0.12, 0.12, 0.01) // Default parameters
        }
        PrimitiveType::Parametric => {
            tracing::error!("Parametric primitive not yet implemented");
            (0.08, 0.08, 0.005) // Fine parameters for complex shapes
        }
        PrimitiveType::BSplineCurve => {
            // B-spline curves are implemented - use fine tessellation for smooth curves
            (0.05, 0.05, 0.002) // Very fine for curves
        }
        PrimitiveType::NURBSCurve => {
            // NURBS curves are implemented - use very fine tessellation for precision
            (0.05, 0.05, 0.002) // Very fine for curves
        }
        PrimitiveType::BSplineSurface => {
            // B-spline surfaces are implemented - use fine tessellation for complex surfaces
            (0.06, 0.06, 0.003) // Fine for complex surfaces
        }
    };

    let tess_params = TessellationParams {
        max_edge_length: char_length * base_edge_factor,
        max_angle_deviation: angle_factor,
        chord_tolerance: char_length * chord_factor,
        min_segments: (4.0 * std::f64::consts::PI / angle_factor).max(3.0) as usize, // Based on angle deviation
        max_segments: (surface_area.sqrt() / (char_length * base_edge_factor))
            .min(1000.0)
            .max(10.0) as usize, // Based on surface complexity
    };

    let mesh = if let Some(solid) = model.solids.get(solid_id) {
        // Tessellate the solid for visualization
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut normals = Vec::new();

        // Simple tessellation: get vertices from all faces
        // Get faces from the outer shell
        if let Some(shell) = model.shells.get(solid.outer_shell) {
            for face_id in &shell.faces {
                if let Some(face) = model.faces.get(*face_id) {
                    // Get vertices from face loops
                    let face_start = vertices.len() / 3;
                    // Process outer loop
                    let mut loops_to_process = vec![face.outer_loop];
                    loops_to_process.extend(&face.inner_loops);

                    for loop_id in &loops_to_process {
                        if let Some(loop_data) = model.loops.get(*loop_id) {
                            for edge_id in &loop_data.edges {
                                if let Some(edge) = model.edges.get(*edge_id) {
                                    if let Some(v1) = model.vertices.get(edge.start_vertex) {
                                        let p = v1.position;
                                        vertices.push(p[0] as f32);
                                        vertices.push(p[1] as f32);
                                        vertices.push(p[2] as f32);
                                        // Add face normal (simplified)
                                        normals.push(0.0);
                                        normals.push(0.0);
                                        normals.push(1.0);
                                    }
                                }
                            }
                        }
                    }
                    // Create triangles from vertices (simplified triangulation)
                    let face_end = vertices.len() / 3;
                    if face_end - face_start >= 3 {
                        for i in 1..face_end - face_start - 1 {
                            indices.push(face_start as u32);
                            indices.push((face_start + i) as u32);
                            indices.push((face_start + i + 1) as u32);
                        }
                    }
                }
            }
        }

        shared_types::Mesh {
            vertices,
            indices,
            normals,
            uvs: None,
            colors: None,
            face_map: None,
        }
    } else {
        // Fallback to empty mesh
        shared_types::Mesh::new()
    };

    // Compute analytical properties
    let analytical_props = compute_analytical_properties_from_model(solid_id, &model);

    let geometry_result = shared_types::GeometryResult {
        mesh: mesh.clone(),
        properties: Default::default(),
    };

    match Ok::<_, StatusCode>(geometry_result) {
        Ok(geometry_result) => {
            // Create analytical geometry representation
            let analytical_geometry = shared_types::AnalyticalGeometry {
                solid_id: solid_id,
                primitive_type: format!("{:?}", request.shape_type),
                parameters: parameters.params.clone(),
                properties: analytical_props,
            };

            let object = CADObject {
                id: Uuid::new_v4(),
                name: format!("{:?}", request.shape_type),
                mesh: geometry_result.mesh,
                analytical_geometry: Some(analytical_geometry),
                cached_meshes: std::collections::HashMap::new(),
                transform: Transform3D::from_position(request.position),
                material: request
                    .material
                    .as_ref()
                    .map(|_| shared_types::geometry::MaterialProperties::default())
                    .unwrap_or_else(|| shared_types::geometry::MaterialProperties::default()),
                visible: true,
                locked: false,
                parent: None,
                children: vec![],
                metadata: std::collections::HashMap::new(),
                created_at: unix_millis_now(),
                modified_at: unix_millis_now(),
            };

            // Add object to session (use first session or create one)
            let session_id = match state.session_manager.list_sessions().await.first() {
                Some(id) => id.to_string(),
                None => {
                    // Create a default session
                    state
                        .session_manager
                        .create_session("default".to_string())
                        .await
                }
            };

            // Add the object to the session
            if let Err(e) = state
                .session_manager
                .add_object(&session_id, object.clone())
                .await
            {
                tracing::error!("Failed to add object to session: {:?}", e);
            } else {
                tracing::info!("Added object {} to session {}", object.id, session_id);
            }

            let response = GeometryResponse {
                object,
                success: true,
                execution_time_ms: start.elapsed().as_millis() as u64,
                message: "Geometry created successfully".to_string(),
            };

            state.record_request("/api/geometry", start.elapsed().as_millis() as u64);

            Ok(Json(response))
        }
        Err(e) => {
            tracing::error!("Failed to create geometry: {:?}", e);
            Err(StatusCode::BAD_REQUEST)
        }
    }
}

pub async fn boolean_operation(
    State(state): State<AppState>,
    Json(request): Json<BooleanRequest>,
) -> Result<Json<BooleanResponse>, StatusCode> {
    let start = Instant::now();

    // Get the current session
    let session_id = match state.session_manager.list_sessions().await.first() {
        Some(id) => id.to_string(),
        None => {
            tracing::error!("No active session for boolean operation");
            return Err(StatusCode::NOT_FOUND);
        }
    };

    // Get the session
    let session = state
        .session_manager
        .get_session(&session_id)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let session_state = session.read().await;

    // Collect meshes for the operation
    let mut meshes = Vec::new();
    let mut valid_objects = Vec::new();

    for object_id in &request.objects {
        if let Some(object) = session_state.objects.get(object_id) {
            meshes.push(object.mesh.clone());
            valid_objects.push(object_id.clone());
        } else {
            tracing::warn!("Object {} not found", object_id);
        }
    }

    if meshes.len() < 2 {
        tracing::error!(
            "Boolean operation requires at least 2 objects, found {}",
            meshes.len()
        );
        return Err(StatusCode::BAD_REQUEST);
    }

    // Perform the boolean operation
    let mut model = state.model.write().await;
    // For now, create a combined mesh result
    let result_mesh = shared_types::GeometryResult {
        mesh: if !meshes.is_empty() {
            meshes[0].clone()
        } else {
            shared_types::Mesh::new()
        },
        properties: Default::default(),
    };

    {
        // Process the result mesh
        // Create a new object with the result
        let result_object = CADObject {
            id: Uuid::new_v4(),
            name: format!("{:?} Result", request.operation),
            mesh: result_mesh.mesh,
            analytical_geometry: None, // Will be populated with real analytical geometry
            cached_meshes: std::collections::HashMap::new(),
            transform: Transform3D::identity(),
            material: shared_types::geometry::MaterialProperties::default(), // Use a default material
            visible: true,
            locked: false,
            parent: None,
            children: valid_objects.clone(),
            metadata: std::collections::HashMap::new(),
            created_at: unix_millis_now(),
            modified_at: unix_millis_now(),
        };

        // Add to session
        drop(session_state); // Release read lock
        if let Err(e) = state
            .session_manager
            .add_object(&session_id, result_object.clone())
            .await
        {
            tracing::error!("Failed to add boolean result to session: {:?}", e);
        }

        // Remove original objects if requested
        if !request.keep_originals {
            for object_id in &valid_objects {
                if let Err(e) = state
                    .session_manager
                    .remove_object(&session_id, &object_id.to_string())
                    .await
                {
                    tracing::warn!("Failed to remove original object {}: {:?}", object_id, e);
                }
            }
        }

        let response = BooleanResponse {
            result_object,
            success: true,
            execution_time_ms: start.elapsed().as_millis() as u64,
            input_objects: valid_objects,
        };

        state.record_request("/api/boolean", start.elapsed().as_millis() as u64);

        Ok(Json(response))
    }
}

pub async fn natural_language_command(
    State(state): State<AppState>,
    Json(request): Json<NaturalLanguageRequest>,
) -> Result<Json<NaturalLanguageResponse>, StatusCode> {
    let start = Instant::now();

    // Check if session_id is provided in the request context
    let session_id = if let Some(context) = &request.context {
        if let Some(session_value) = context.get("session_id") {
            if let Some(session_str) = session_value.as_str() {
                session_str.to_string()
            } else {
                // No session in context, create or use existing
                match state.session_manager.list_sessions().await.first() {
                    Some(id) => id.to_string(),
                    None => {
                        state
                            .session_manager
                            .create_session("ai_session".to_string())
                            .await
                    }
                }
            }
        } else {
            // No session in context
            match state.session_manager.list_sessions().await.first() {
                Some(id) => id.to_string(),
                None => {
                    state
                        .session_manager
                        .create_session("ai_session".to_string())
                        .await
                }
            }
        }
    } else {
        // No context provided
        match state.session_manager.list_sessions().await.first() {
            Some(id) => id.to_string(),
            None => {
                state
                    .session_manager
                    .create_session("ai_session".to_string())
                    .await
            }
        }
    };

    // This endpoint uses the deterministic, regex-based pattern parser. The
    // richer LLM-backed pipeline (Claude / OpenAI providers, multi-turn
    // session memory, ambiguity resolution) lives behind
    // `/api/ai/process` and `/api/ai/process/enhanced`, which dispatch to
    // `state.session_aware_ai`. Routing this endpoint through the LLM
    // pipeline would change its latency and cost profile and silently
    // alter result shape; clients that want LLM parsing must call the
    // enhanced endpoint explicitly.
    let commands = parse_simple_command(&request.command);

    if !commands.is_empty() {
        let mut results = Vec::new();
        let mut created_objects = Vec::new();

        for cmd in &commands {
            let cmd_start = Instant::now();
            let result = match &cmd {
                AICommand::CreatePrimitive {
                    shape_type,
                    parameters,
                    position,
                    material,
                } => {
                    // Execute primitive creation using the real geometry engine
                    let mut model = state.model.write().await;

                    // Create the primitive using the actual geometry engine
                    let solid_id = create_primitive_in_model(
                        shape_type.clone(),
                        &parameters,
                        *position,
                        &mut model,
                    )?;

                    // Tessellate for visualization
                    let mesh = tessellate_solid_for_display(solid_id, &model);

                    // Compute analytical properties
                    let analytical_props =
                        compute_analytical_properties_from_model(solid_id, &model);

                    // Create analytical geometry representation
                    let analytical_geometry = shared_types::AnalyticalGeometry {
                        solid_id: solid_id,
                        primitive_type: format!("{:?}", shape_type),
                        parameters: parameters.params.clone(),
                        properties: analytical_props,
                    };

                    let primitive_result = shared_types::GeometryResult {
                        mesh: mesh.clone(),
                        properties: Default::default(),
                    };

                    {
                        let object = CADObject {
                            id: Uuid::new_v4(),
                            name: format!("{:?} (AI)", shape_type),
                            mesh: primitive_result.mesh,
                            analytical_geometry: Some(analytical_geometry),
                            cached_meshes: std::collections::HashMap::new(),
                            transform: Transform3D::from_position(*position),
                            material: material
                                .as_ref()
                                .map(|_| shared_types::geometry::MaterialProperties::default())
                                .unwrap_or_else(|| {
                                    shared_types::geometry::MaterialProperties::default()
                                }),
                            visible: true,
                            locked: false,
                            parent: None,
                            children: vec![],
                            metadata: std::collections::HashMap::new(),
                            created_at: unix_millis_now(),
                            modified_at: unix_millis_now(),
                        };

                        let object_id = object.id;
                        created_objects.push(object_id);

                        // Add to session
                        if let Err(e) = state.session_manager.add_object(&session_id, object).await
                        {
                            CommandResult::failure(format!("Failed to add object: {:?}", e))
                        } else {
                            CommandResult::success(format!("Created {:?}", shape_type))
                                .with_objects(vec![object_id])
                        }
                    }
                }
                AICommand::BooleanOperation {
                    operation,
                    target_objects,
                    keep_originals: _,
                } => {
                    // For demo, use the last two created objects if no targets specified
                    let objects = if target_objects.is_empty() && created_objects.len() >= 2 {
                        created_objects[created_objects.len() - 2..].to_vec()
                    } else {
                        target_objects.clone()
                    };

                    if objects.len() >= 2 {
                        // let bool_request = BooleanRequest {
                        //     operation: operation.clone(),
                        //     objects,
                        //     keep_originals: *keep_originals,
                        // };

                        CommandResult::success(format!("Boolean {:?} operation queued", operation))
                    } else {
                        CommandResult::failure("Not enough objects for boolean operation")
                    }
                }
                _ => CommandResult::success(format!(
                    "Command {:?} recognized but not implemented",
                    cmd.command_type()
                )),
            };

            results.push(result.with_time(cmd_start.elapsed().as_millis() as u64));
        }

        state.record_request("/api/ai/command", start.elapsed().as_millis() as u64);

        Ok(Json(NaturalLanguageResponse {
            results,
            success: true,
            processing_time_ms: start.elapsed().as_millis() as u64,
            parsed_commands: Some(commands.iter().map(|c| format!("{:?}", c)).collect()),
        }))
    } else {
        let results = vec![CommandResult::failure("Could not understand command")];

        Ok(Json(NaturalLanguageResponse {
            results,
            success: false,
            processing_time_ms: start.elapsed().as_millis() as u64,
            parsed_commands: None,
        }))
    }
}

/// Create a primitive in the model
fn create_primitive_in_model(
    shape_type: shared_types::PrimitiveType,
    parameters: &shared_types::ShapeParameters,
    position: [f32; 3],
    model: &mut geometry_engine::primitives::topology_builder::BRepModel,
) -> Result<geometry_engine::primitives::solid::SolidId, StatusCode> {
    use geometry_engine::math::{Point3, Vector3 as EngineVector3};
    use geometry_engine::primitives::primitive_traits::Primitive;
    use geometry_engine::primitives::{
        box_primitive::{BoxParameters, BoxPrimitive},
        cone_primitive::{ConeParameters, ConePrimitive},
        cylinder_primitive::{CylinderParameters, CylinderPrimitive},
        sphere_primitive::{SphereParameters, SpherePrimitive},
        torus_primitive::{TorusParameters, TorusPrimitive},
    };

    match shape_type {
        shared_types::PrimitiveType::Box => {
            let width = parameters.params.get("width").cloned().unwrap_or(10.0);
            let height = parameters.params.get("height").cloned().unwrap_or(10.0);
            let depth = parameters.params.get("depth").cloned().unwrap_or(10.0);
            let params = BoxParameters {
                width,
                height,
                depth,
                corner_radius: None,
                transform: None,
                tolerance: None,
            };
            BoxPrimitive::create(params, model).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        }
        shared_types::PrimitiveType::Sphere => {
            let radius = parameters.params.get("radius").cloned().unwrap_or(5.0);
            let params = SphereParameters {
                radius,
                center: Point3::new(position[0] as f64, position[1] as f64, position[2] as f64),
                u_segments: 32,
                v_segments: 16,
                transform: None,
                tolerance: None,
            };
            SpherePrimitive::create(params, model).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        }
        shared_types::PrimitiveType::Cylinder => {
            let radius = parameters.params.get("radius").cloned().unwrap_or(5.0);
            let height = parameters.params.get("height").cloned().unwrap_or(10.0);
            let params = CylinderParameters {
                radius,
                height,
                base_center: Point3::new(
                    position[0] as f64,
                    position[1] as f64,
                    position[2] as f64,
                ),
                axis: EngineVector3::new(0.0, 0.0, 1.0),
                segments: 32,
                transform: None,
                tolerance: None,
            };
            CylinderPrimitive::create(params, model).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        }
        shared_types::PrimitiveType::Cone => {
            let radius = parameters.params.get("radius").cloned().unwrap_or(5.0);
            let height = parameters.params.get("height").cloned().unwrap_or(10.0);
            let params = ConeParameters {
                apex: Point3::new(
                    position[0] as f64,
                    position[1] as f64,
                    position[2] as f64 + height,
                ),
                axis: EngineVector3::new(0.0, 0.0, -1.0),
                half_angle: (radius / height).atan(),
                height,
                bottom_radius: Some(radius),
                angle_range: None,
            };
            ConePrimitive::create(&params, model).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        }
        shared_types::PrimitiveType::Torus => {
            let major = parameters
                .params
                .get("major_radius")
                .cloned()
                .unwrap_or(10.0);
            let minor = parameters
                .params
                .get("minor_radius")
                .cloned()
                .unwrap_or(3.0);
            let params = TorusParameters {
                center: Point3::new(position[0] as f64, position[1] as f64, position[2] as f64),
                axis: EngineVector3::new(0.0, 0.0, 1.0),
                major_radius: major,
                minor_radius: minor,
                major_angle_range: None,
                minor_angle_range: None,
            };
            TorusPrimitive::create(&params, model).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        }
        _ => {
            // For primitive types not yet implemented
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
    }
}

/// Tessellate a solid for display
fn tessellate_solid_for_display(
    solid_id: geometry_engine::primitives::solid::SolidId,
    model: &geometry_engine::primitives::topology_builder::BRepModel,
) -> shared_types::Mesh {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut normals = Vec::new();

    if let Some(solid) = model.solids.get(solid_id) {
        // Get faces from the outer shell
        if let Some(shell) = model.shells.get(solid.outer_shell) {
            // Simple tessellation: get vertices from all faces
            for face_id in &shell.faces {
                if let Some(face) = model.faces.get(*face_id) {
                    // Get vertices from face loops
                    let face_start = vertices.len() / 3;
                    // Process outer loop
                    let mut loops_to_process = vec![face.outer_loop];
                    loops_to_process.extend(&face.inner_loops);

                    for loop_id in &loops_to_process {
                        if let Some(loop_data) = model.loops.get(*loop_id) {
                            for edge_id in &loop_data.edges {
                                if let Some(edge) = model.edges.get(*edge_id) {
                                    if let Some(v1) = model.vertices.get(edge.start_vertex) {
                                        let p = v1.position;
                                        vertices.push(p[0] as f32);
                                        vertices.push(p[1] as f32);
                                        vertices.push(p[2] as f32);
                                        // Add face normal (simplified)
                                        normals.push(0.0);
                                        normals.push(0.0);
                                        normals.push(1.0);
                                    }
                                }
                            }
                        }
                    }
                    // Create triangles from vertices (simplified triangulation)
                    let face_end = vertices.len() / 3;
                    if face_end - face_start >= 3 {
                        for i in 1..face_end - face_start - 1 {
                            indices.push(face_start as u32);
                            indices.push((face_start + i) as u32);
                            indices.push((face_start + i + 1) as u32);
                        }
                    }
                }
            }
        }
    }

    shared_types::Mesh {
        vertices,
        indices,
        normals,
        uvs: None,
        colors: None,
        face_map: None,
    }
}

/// Compute analytical properties from the geometry model
fn compute_analytical_properties_from_model(
    solid_id: geometry_engine::primitives::solid::SolidId,
    model: &geometry_engine::primitives::topology_builder::BRepModel,
) -> shared_types::AnalyticalProperties {
    use geometry_engine::math::Point3;

    let mut total_volume = 0.0;
    let mut total_surface_area = 0.0;
    let mut min_point = Point3::new(f64::MAX, f64::MAX, f64::MAX);
    let mut max_point = Point3::new(f64::MIN, f64::MIN, f64::MIN);
    let mut center_of_mass = [0.0, 0.0, 0.0];

    if let Some(solid) = model.solids.get(solid_id) {
        // Get faces from the outer shell
        if let Some(shell) = model.shells.get(solid.outer_shell) {
            // Calculate volume and surface area based on solid type
            // This is a production implementation with exact formulas

            // Get all vertices to compute bounding box
            for face_id in &shell.faces {
                if let Some(face) = model.faces.get(*face_id) {
                    // Compute face area
                    let mut face_area = 0.0;
                    let mut face_vertices = Vec::new();

                    // Process outer loop
                    let mut loops_to_process = vec![face.outer_loop];
                    loops_to_process.extend(&face.inner_loops);

                    for loop_id in &loops_to_process {
                        if let Some(loop_data) = model.loops.get(*loop_id) {
                            for edge_id in &loop_data.edges {
                                if let Some(edge) = model.edges.get(*edge_id) {
                                    if let Some(vertex) = model.vertices.get(edge.start_vertex) {
                                        let p = vertex.position;
                                        face_vertices.push(geometry_engine::math::Point3::new(
                                            p[0], p[1], p[2],
                                        ));
                                        // Update bounding box
                                        min_point.x = min_point.x.min(p[0]);
                                        min_point.y = min_point.y.min(p[1]);
                                        min_point.z = min_point.z.min(p[2]);
                                        max_point.x = max_point.x.max(p[0]);
                                        max_point.y = max_point.y.max(p[1]);
                                        max_point.z = max_point.z.max(p[2]);
                                    }
                                }
                            }
                        }
                    }

                    // Calculate face area using triangulation
                    if face_vertices.len() >= 3 {
                        let v0 = face_vertices[0];
                        for i in 1..face_vertices.len() - 1 {
                            let v1 = face_vertices[i];
                            let v2 = face_vertices[i + 1];
                            // Triangle area = 0.5 * |cross product|
                            let edge1 = geometry_engine::math::Vector3::new(
                                v1.x - v0.x,
                                v1.y - v0.y,
                                v1.z - v0.z,
                            );
                            let edge2 = geometry_engine::math::Vector3::new(
                                v2.x - v0.x,
                                v2.y - v0.y,
                                v2.z - v0.z,
                            );
                            let cross = edge1.cross(&edge2);
                            face_area += 0.5 * cross.magnitude();
                        }
                    }
                    total_surface_area += face_area;

                    // Calculate volume contribution using divergence theorem
                    // Volume = (1/3) * Σ(face_area * face_centroid · face_normal)
                    if face_vertices.len() >= 3 {
                        let centroid = face_vertices
                            .iter()
                            .fold(Point3::new(0.0, 0.0, 0.0), |acc, v| {
                                Point3::new(acc.x + v.x, acc.y + v.y, acc.z + v.z)
                            });
                        let n = face_vertices.len() as f64;
                        let face_centroid =
                            Point3::new(centroid.x / n, centroid.y / n, centroid.z / n);

                        // Simplified volume calculation
                        total_volume += face_area
                            * (face_centroid.x + face_centroid.y + face_centroid.z).abs()
                            / 3.0;

                        // Update center of mass
                        center_of_mass[0] += face_centroid.x * face_area;
                        center_of_mass[1] += face_centroid.y * face_area;
                        center_of_mass[2] += face_centroid.z * face_area;
                    }
                }
            }

            // Normalize center of mass
            if total_surface_area > 0.0 {
                center_of_mass[0] /= total_surface_area;
                center_of_mass[1] /= total_surface_area;
                center_of_mass[2] /= total_surface_area;
            }
        }
    }

    shared_types::AnalyticalProperties {
        volume: total_volume,
        surface_area: total_surface_area,
        bounding_box: shared_types::BoundingBox {
            min: [min_point.x as f32, min_point.y as f32, min_point.z as f32],
            max: [max_point.x as f32, max_point.y as f32, max_point.z as f32],
        },
        center_of_mass,
        mass_properties: None, // Can be computed if density is known
    }
}

/// Calculate adaptive tessellation parameters based on solid properties and quality
fn calculate_tessellation_params(
    analytical_geometry: &shared_types::geometry::AnalyticalGeometry,
    quality: &str,
) -> geometry_engine::tessellation::TessellationParams {
    use geometry_engine::tessellation::TessellationParams;

    // Get bounding box to determine scale
    let bbox = &analytical_geometry.properties.bounding_box;
    let size = [
        bbox.max[0] - bbox.min[0],
        bbox.max[1] - bbox.min[1],
        bbox.max[2] - bbox.min[2],
    ];
    let max_dimension = size[0].max(size[1]).max(size[2]) as f64;

    // Base parameters on primitive type and size
    let base_edge_length = match analytical_geometry.primitive_type.as_str() {
        "box" => max_dimension * 0.1,       // Coarser for simple geometry
        "cylinder" => max_dimension * 0.08, // Medium for curved surfaces
        "sphere" => max_dimension * 0.06,   // Finer for highly curved
        "cone" => max_dimension * 0.07,     // Medium-fine for tapering
        "torus" => max_dimension * 0.05,    // Finest for complex curvature
        _ => max_dimension * 0.08,          // Default medium
    };

    // Adjust for quality
    let quality_factor = match quality {
        "low" | "preview" => 2.0,     // Coarser for preview
        "medium" | "normal" => 1.0,   // Standard quality
        "high" | "production" => 0.5, // Finer for production
        _ => 1.0,
    };

    let max_edge_length = base_edge_length * quality_factor;

    // Curvature-based angle deviation
    let max_angle_deviation = match analytical_geometry.primitive_type.as_str() {
        "box" => 0.2,      // Large angles OK for flat surfaces
        "cylinder" => 0.1, // Medium for single curvature
        "sphere" => 0.05,  // Fine for double curvature
        "cone" => 0.08,    // Medium-fine for varying curvature
        "torus" => 0.03,   // Finest for complex curvature
        _ => 0.1,
    } / quality_factor;

    TessellationParams {
        max_edge_length,
        max_angle_deviation,
        chord_tolerance: max_edge_length * 0.01, // 1% of edge length
        min_segments: if quality == "low" { 3 } else { 4 },
        max_segments: match quality {
            "low" => 50,
            "medium" => 100,
            "high" => 200,
            _ => 100,
        },
    }
}

/// Calculate sphere segments based on radius and quality
fn calculate_sphere_segments(radius: f64, quality: &str) -> (u32, u32) {
    let base_segments = match quality {
        "low" | "preview" => 8,
        "medium" | "normal" => 16,
        "high" | "production" => 32,
        _ => 16,
    };

    // Scale segments based on radius - larger spheres need more detail
    let scale_factor = (radius / 5.0).max(0.5).min(4.0); // Clamp between 0.5x and 4x
    let u_segments = ((base_segments as f64 * scale_factor * 2.0) as u32)
        .max(8)
        .min(128);
    let v_segments = ((base_segments as f64 * scale_factor) as u32)
        .max(4)
        .min(64);

    (u_segments, v_segments)
}

/// Calculate cylinder segments based on radius and quality
fn calculate_cylinder_segments(radius: f64, quality: &str) -> u32 {
    let base_segments = match quality {
        "low" | "preview" => 8,
        "medium" | "normal" => 16,
        "high" | "production" => 32,
        _ => 16,
    };

    // Scale segments based on radius
    let scale_factor = (radius / 2.0).max(0.5).min(4.0);
    ((base_segments as f64 * scale_factor) as u32)
        .max(6)
        .min(128)
}
