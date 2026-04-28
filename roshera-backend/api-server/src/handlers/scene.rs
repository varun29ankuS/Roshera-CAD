//! Scene query endpoints for AI awareness

use crate::AppState;
use axum::{
    extract::{Path, State},
    Json,
};
use geometry_engine::math::Point3;
use serde::{Deserialize, Serialize};
use shared_types::{
    CameraState, MassProperties, MaterialRef, ObjectChanges, ObjectProperties, ObjectType,
    ProjectionType, Quaternion, RelationshipType, SceneBoundingBox, SceneFilters,
    SceneGridSettings, SceneMetadata, SceneObject, SceneQuery, SceneQueryType, SceneState,
    SceneStatistics, SceneTransform3D, SceneUpdate, SelectionMode, SelectionState,
    SpatialRelationship, UnitSystem, Viewport,
};

#[derive(Serialize)]
pub struct SceneResponse {
    pub success: bool,
    pub message: String,
    pub scene: Option<SceneState>,
}

#[derive(Serialize)]
pub struct SceneQueryResponse {
    pub success: bool,
    pub message: String,
    pub objects: Vec<SceneObject>,
}

/// Get the complete scene state
pub async fn get_scene_state(State(state): State<AppState>) -> Json<SceneResponse> {
    tracing::info!("Scene state requested");

    let model = state.model.read().await;
    let solids = state.solids.read().await;

    let mut objects = Vec::new();

    // Convert each solid in the model to a SceneObject
    for solid_id in 0..model.solids.len() {
        if let Some(solid) = model.solids.get(solid_id as u32) {
            let shape_type = solids
                .get(&(solid_id as u32))
                .cloned()
                .unwrap_or_else(|| "unknown".to_string());

            // Determine object type from shape description
            let object_type = parse_object_type(&shape_type, &(solid_id as u32));

            // Calculate bounding box (simplified for now)
            let bounding_box = calculate_bounding_box(solid, &model);

            let scene_object = SceneObject {
                id: uuid::Uuid::new_v4(), // Generate a UUID for scene object
                object_type,
                name: format!("Object_{}", solid_id),
                transform: SceneTransform3D::default(),
                bounding_box,
                material: None,
                visible: true,
                locked: false,
                properties: ObjectProperties {
                    custom: std::collections::HashMap::new(),
                    mass_properties: None,
                    created_at: 0, // TODO: Track creation time
                    modified_at: 0,
                    created_by: None,
                },
                parent: None,
                children: Vec::new(),
            };

            objects.push(scene_object);
        }
    }

    // Calculate scene statistics
    let total_vertices = model.vertices.len();
    let total_faces = model.faces.len();

    let scene_state = SceneState {
        objects,
        camera: CameraState::default(),
        selection: SelectionState {
            selected_objects: Vec::new(),
            mode: SelectionMode::Object,
            last_modified: 0,
        },
        active_tool: None,
        metadata: SceneMetadata {
            name: "Current Scene".to_string(),
            units: UnitSystem::Millimeters,
            grid: SceneGridSettings {
                visible: true,
                spacing: 10.0,
                major_lines: 5,
            },
            statistics: SceneStatistics {
                total_objects: solids.len(),
                total_vertices,
                total_faces,
                bounding_box: None, // TODO: Calculate overall bounding box
            },
        },
        relationships: Vec::new(), // TODO: Analyze spatial relationships
    };

    Json(SceneResponse {
        success: true,
        message: "Scene state retrieved successfully".to_string(),
        scene: Some(scene_state),
    })
}

/// Query scene objects based on criteria
pub async fn query_scene(
    State(state): State<AppState>,
    Json(query): Json<SceneQuery>,
) -> Json<SceneQueryResponse> {
    tracing::info!("Scene query: {:?}", query);

    let model = state.model.read().await;
    let solids = state.solids.read().await;

    let mut matching_objects = Vec::new();

    match query.query_type {
        SceneQueryType::AllObjects => {
            // Return all objects
            for solid_id in 0..model.solids.len() {
                if let Some(solid) = model.solids.get(solid_id as u32) {
                    let shape_type = solids
                        .get(&(solid_id as u32))
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_string());
                    let object_type = parse_object_type(&shape_type, &(solid_id as u32));
                    let bounding_box = calculate_bounding_box(solid, &model);

                    let scene_object = SceneObject {
                        id: uuid::Uuid::new_v4(),
                        object_type,
                        name: format!("Object_{}", solid_id),
                        transform: SceneTransform3D::default(),
                        bounding_box,
                        material: None,
                        visible: true,
                        locked: false,
                        properties: ObjectProperties {
                            custom: std::collections::HashMap::new(),
                            mass_properties: None,
                            created_at: 0,
                            modified_at: 0,
                            created_by: None,
                        },
                        parent: None,
                        children: Vec::new(),
                    };

                    matching_objects.push(scene_object);
                }
            }
        }
        SceneQueryType::ByType { object_type } => {
            // Filter by object type
            for solid_id in 0..model.solids.len() {
                if let Some(solid) = model.solids.get(solid_id as u32) {
                    let shape_type = solids
                        .get(&(solid_id as u32))
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_string());
                    if shape_type.contains(&object_type) {
                        let obj_type = parse_object_type(&shape_type, &(solid_id as u32));
                        let bounding_box = calculate_bounding_box(solid, &model);

                        let scene_object = SceneObject {
                            id: uuid::Uuid::new_v4(),
                            object_type: obj_type,
                            name: format!("Object_{}", solid_id),
                            transform: SceneTransform3D::default(),
                            bounding_box,
                            material: None,
                            visible: true,
                            locked: false,
                            properties: ObjectProperties {
                                custom: std::collections::HashMap::new(),
                                mass_properties: None,
                                created_at: 0,
                                modified_at: 0,
                                created_by: None,
                            },
                            parent: None,
                            children: Vec::new(),
                        };

                        matching_objects.push(scene_object);
                    }
                }
            }
        }
        SceneQueryType::InRegion {
            bounding_box: query_box,
        } => {
            // Filter by region
            for solid_id in 0..model.solids.len() {
                if let Some(solid) = model.solids.get(solid_id as u32) {
                    let shape_type = solids
                        .get(&(solid_id as u32))
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_string());
                    let object_type = parse_object_type(&shape_type, &(solid_id as u32));
                    let bbox = calculate_bounding_box(solid, &model);

                    // Check if bounding boxes intersect
                    if bboxes_intersect(&bbox, &query_box) {
                        let scene_object = SceneObject {
                            id: uuid::Uuid::new_v4(),
                            object_type,
                            name: format!("Object_{}", solid_id),
                            transform: SceneTransform3D::default(),
                            bounding_box: bbox,
                            material: None,
                            visible: true,
                            locked: false,
                            properties: ObjectProperties {
                                custom: std::collections::HashMap::new(),
                                mass_properties: None,
                                created_at: 0,
                                modified_at: 0,
                                created_by: None,
                            },
                            parent: None,
                            children: Vec::new(),
                        };

                        matching_objects.push(scene_object);
                    }
                }
            }
        }
        _ => {
            // Other query types not implemented yet
            tracing::warn!("Query type not implemented: {:?}", query.query_type);
        }
    }

    Json(SceneQueryResponse {
        success: true,
        message: format!("Found {} matching objects", matching_objects.len()),
        objects: matching_objects,
    })
}

/// Get detailed information about a specific object
pub async fn get_object_details(
    State(state): State<AppState>,
    Path(object_id): Path<u32>,
) -> Json<SceneResponse> {
    tracing::info!("Object details requested for ID: {}", object_id);

    // Write lock is required because Solid::compute_mass_properties mutates
    // the solid (cached_mass_props) and the shell/face/loop stores during
    // the divergence-theorem volume integration. The lock window is short:
    // results are cached on the solid after the first computation.
    let mut model_guard = state.model.write().await;
    let solids = state.solids.read().await;

    if model_guard.solids.get(object_id).is_some() {
        let shape_type = solids
            .get(&object_id)
            .cloned()
            .unwrap_or_else(|| "unknown".to_string());
        let object_type = parse_object_type(&shape_type, &object_id);

        // Reborrow immutably for bounding box, then mutably for mass props.
        let bounding_box = {
            let solid = model_guard
                .solids
                .get(object_id)
                .expect("solid presence verified above");
            calculate_bounding_box(solid, &model_guard)
        };

        // Real kernel mass properties (volume + COG + inertia tensor) via
        // divergence-theorem integration. Returns None on numerical failure
        // — we never silently fall back to bbox approximations.
        let mass_properties = calculate_mass_properties(object_id, &mut model_guard);

        let scene_object = SceneObject {
            id: uuid::Uuid::new_v4(),
            object_type,
            name: format!("Object_{}", object_id),
            transform: SceneTransform3D::default(),
            bounding_box: bounding_box.clone(),
            material: None,
            visible: true,
            locked: false,
            properties: ObjectProperties {
                custom: std::collections::HashMap::new(),
                mass_properties,
                created_at: 0,
                modified_at: 0,
                created_by: None,
            },
            parent: None,
            children: Vec::new(),
        };

        let scene_state = SceneState {
            objects: vec![scene_object],
            camera: CameraState::default(),
            selection: SelectionState {
                selected_objects: vec![uuid::Uuid::new_v4()],
                mode: SelectionMode::Object,
                last_modified: 0,
            },
            active_tool: None,
            metadata: SceneMetadata {
                name: "Object Details".to_string(),
                units: UnitSystem::Millimeters,
                grid: SceneGridSettings {
                    visible: false,
                    spacing: 10.0,
                    major_lines: 5,
                },
                statistics: SceneStatistics {
                    total_objects: 1,
                    total_vertices: 0, // TODO: Count vertices for this object
                    total_faces: 0,    // TODO: Count faces for this object
                    bounding_box: Some(bounding_box),
                },
            },
            relationships: Vec::new(),
        };

        Json(SceneResponse {
            success: true,
            message: "Object details retrieved successfully".to_string(),
            scene: Some(scene_state),
        })
    } else {
        Json(SceneResponse {
            success: false,
            message: format!("Object {} not found", object_id),
            scene: None,
        })
    }
}

// Helper functions

fn parse_object_type(shape_type: &str, _solid_id: &u32) -> ObjectType {
    if shape_type.contains("box") {
        // Try to parse dimensions from shape type (e.g., "box_10x10x10")
        let parts: Vec<&str> = shape_type.split('_').collect();
        if parts.len() > 1 {
            let dims: Vec<&str> = parts[1].split('x').collect();
            if dims.len() == 3 {
                if let (Ok(w), Ok(h), Ok(d)) = (
                    dims[0].parse::<f32>(),
                    dims[1].parse::<f32>(),
                    dims[2].parse::<f32>(),
                ) {
                    return ObjectType::Box {
                        width: w,
                        height: h,
                        depth: d,
                    };
                }
            }
        }
        ObjectType::Box {
            width: 1.0,
            height: 1.0,
            depth: 1.0,
        }
    } else if shape_type.contains("sphere") {
        ObjectType::Sphere { radius: 1.0 }
    } else if shape_type.contains("cylinder") {
        ObjectType::Cylinder {
            radius: 1.0,
            height: 2.0,
        }
    } else if shape_type.contains("cone") {
        ObjectType::Cone {
            bottom_radius: 1.0,
            top_radius: 0.5,
            height: 2.0,
        }
    } else if shape_type.contains("torus") {
        ObjectType::Torus {
            major_radius: 1.0,
            minor_radius: 0.3,
        }
    } else if shape_type.contains("extruded") {
        ObjectType::Compound { part_count: 2 }
    } else {
        ObjectType::Mesh {
            vertex_count: 0,
            face_count: 0,
        }
    }
}

fn calculate_bounding_box(
    solid: &geometry_engine::primitives::solid::Solid,
    model: &geometry_engine::primitives::topology_builder::BRepModel,
) -> SceneBoundingBox {
    // This is a simplified version - a real implementation would
    // calculate the actual bounding box from the geometry
    let mut min = [f32::MAX, f32::MAX, f32::MAX];
    let mut max = [f32::MIN, f32::MIN, f32::MIN];

    // Get shell
    if let Some(shell) = model.shells.get(solid.outer_shell) {
        // Iterate through faces
        for &face_id in &shell.faces {
            if let Some(face) = model.faces.get(face_id) {
                // Get vertices of face (simplified - just using outer loop)
                let outer_loop = face.outer_loop;
                if let Some(loop_data) = model.loops.get(outer_loop) {
                    for &edge_id in &loop_data.edges {
                        if let Some(edge) = model.edges.get(edge_id) {
                            if let Some(vertex) = model.vertices.get(edge.start_vertex) {
                                let point = vertex.point();
                                min[0] = min[0].min(point.x as f32);
                                min[1] = min[1].min(point.y as f32);
                                min[2] = min[2].min(point.z as f32);
                                max[0] = max[0].max(point.x as f32);
                                max[1] = max[1].max(point.y as f32);
                                max[2] = max[2].max(point.z as f32);
                            }
                        }
                    }
                }
            }
        }
    }

    // If no valid bounds found, use defaults
    if min[0] > max[0] {
        min = [-5.0, -5.0, -5.0];
        max = [5.0, 5.0, 5.0];
    }

    SceneBoundingBox { min, max }
}

/// Compute real mass properties (volume, surface area, center of mass) by
/// invoking the kernel's divergence-theorem integration.
///
/// Replaces the previous bbox-volume placeholder. The kernel caches results
/// on the solid after the first computation, so subsequent calls are O(1).
///
/// Returns `None` if either the surface area integration or the mass-property
/// integration fails — the caller treats the absence of mass properties as a
/// signal that the topology is degenerate, not as a license to fall back to a
/// bounding-box approximation.
fn calculate_mass_properties(
    solid_id: u32,
    model: &mut geometry_engine::primitives::topology_builder::BRepModel,
) -> Option<MassProperties> {
    use geometry_engine::math::Tolerance;

    // Disjoint-field borrows: the kernel's mass-property API requires a
    // mutable borrow on the solid plus mutable borrows on the shell, face,
    // and loop stores, plus immutable borrows on the rest of the model.
    // Splitting the model reference field-by-field is the only way to
    // satisfy that signature without restructuring the kernel.
    //
    // Compute surface area first (also mutates) so the mass-prop call can
    // run on top of cached intermediates.
    let surface_area = {
        let solid = model.solids.get_mut(solid_id)?;
        match solid.surface_area(
            &mut model.shells,
            &mut model.faces,
            &mut model.loops,
            &model.vertices,
            &model.edges,
            &model.surfaces,
            Tolerance::default(),
        ) {
            Ok(area) => area,
            Err(err) => {
                tracing::warn!(
                    "surface_area failed for solid {}: {} — omitting mass properties",
                    solid_id,
                    err
                );
                return None;
            }
        }
    };

    let solid = model.solids.get_mut(solid_id)?;
    let props = match solid.compute_mass_properties(
        &mut model.shells,
        &mut model.faces,
        &mut model.loops,
        &model.vertices,
        &model.edges,
        &model.curves,
        &model.surfaces,
    ) {
        Ok(p) => p.clone(),
        Err(err) => {
            tracing::warn!(
                "compute_mass_properties failed for solid {}: {} — omitting mass properties",
                solid_id,
                err
            );
            return None;
        }
    };

    Some(MassProperties {
        volume: props.volume as f32,
        surface_area: surface_area as f32,
        center_of_mass: [
            props.center_of_mass.x as f32,
            props.center_of_mass.y as f32,
            props.center_of_mass.z as f32,
        ],
        // Mass requires material density which the API does not yet thread
        // through. The kernel's `props.mass` uses a unit-density placeholder;
        // returning None here keeps the contract honest until material
        // assignment is wired end-to-end.
        mass: None,
    })
}

fn bboxes_intersect(a: &SceneBoundingBox, b: &SceneBoundingBox) -> bool {
    a.min[0] <= b.max[0]
        && a.max[0] >= b.min[0]
        && a.min[1] <= b.max[1]
        && a.max[1] >= b.min[1]
        && a.min[2] <= b.max[2]
        && a.max[2] >= b.min[2]
}
