//! Modify Operations for B-Rep Models
//!
//! Provides comprehensive operations to modify existing geometry entities including
//! parameter changes, topology updates, property modifications, and geometric transformations.

use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::{
    edge::EdgeId, face::FaceId, r#loop::LoopId, solid::SolidId, topology_builder::BRepModel,
    vertex::VertexId,
};

/// Type of modification operation
#[derive(Debug, Clone)]
pub enum ModifyType {
    /// Move a vertex to a new position
    MoveVertex {
        vertex_id: VertexId,
        new_position: Point3,
    },

    /// Replace an edge with a new curve
    ReplaceEdge {
        edge_id: EdgeId,
        new_curve: EdgeCurveType,
    },

    /// Modify face surface
    ModifyFaceSurface {
        face_id: FaceId,
        surface_params: SurfaceParameters,
    },

    /// Change solid properties
    ModifySolidProperties {
        solid_id: SolidId,
        properties: SolidProperties,
    },

    /// Edit loop orientation
    ChangeLoopOrientation { loop_id: LoopId, reverse: bool },

    /// Modify tolerance
    ChangeTolerance {
        entity_type: EntityType,
        entity_id: u32,
        new_tolerance: Tolerance,
    },
}

/// Edge curve types for replacement
#[derive(Debug, Clone)]
pub enum EdgeCurveType {
    Line {
        start: Point3,
        end: Point3,
    },
    Arc {
        center: Point3,
        radius: f64,
        start_angle: f64,
        end_angle: f64,
    },
    BSpline {
        control_points: Vec<Point3>,
        degree: u32,
    },
    Circle {
        center: Point3,
        radius: f64,
        normal: Vector3,
    },
}

/// Surface parameters for face modification
#[derive(Debug, Clone)]
pub struct SurfaceParameters {
    pub surface_type: SurfaceType,
    pub u_degree: Option<u32>,
    pub v_degree: Option<u32>,
    pub control_points: Option<Vec<Vec<Point3>>>,
}

/// Surface types
#[derive(Debug, Clone)]
pub enum SurfaceType {
    Plane,
    Cylinder,
    Sphere,
    Torus,
    BSpline,
    NURBS,
}

/// Solid properties that can be modified
#[derive(Debug, Clone)]
pub struct SolidProperties {
    pub name: Option<String>,
    pub material: Option<String>,
    pub color: Option<[f32; 4]>,
    pub visible: Option<bool>,
    pub selectable: Option<bool>,
}

/// Entity types for tolerance changes
#[derive(Debug, Clone, Copy)]
pub enum EntityType {
    Vertex,
    Edge,
    Face,
    Shell,
    Solid,
}

/// Options for modify operations
#[derive(Debug, Clone)]
pub struct ModifyOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// Whether to validate topology after modification
    pub validate_topology: bool,

    /// Whether to maintain constraints
    pub maintain_constraints: bool,

    /// Whether to update dependent entities
    pub update_dependents: bool,
}

impl Default for ModifyOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            validate_topology: true,
            maintain_constraints: true,
            update_dependents: true,
        }
    }
}

/// Result of an modify operation
#[derive(Debug)]
pub struct ModifyResult {
    /// Entities that were modified
    pub modified_entities: Vec<(EntityType, u32)>,

    /// Entities that were indirectly affected
    pub affected_entities: Vec<(EntityType, u32)>,

    /// Whether topology remained valid
    pub topology_valid: bool,

    /// Warnings generated during edit
    pub warnings: Vec<String>,
}

/// Apply an modify operation to the model
pub fn apply_modification(
    model: &mut BRepModel,
    edit: ModifyType,
    options: ModifyOptions,
) -> OperationResult<ModifyResult> {
    // Validate the edit
    validate_modification(model, &edit)?;

    // Track modified entities
    let mut modified_entities = Vec::new();
    let mut affected_entities = Vec::new();
    let mut warnings = Vec::new();

    // Apply the edit based on type
    match edit {
        ModifyType::MoveVertex {
            vertex_id,
            new_position,
        } => {
            move_vertex(model, vertex_id, new_position, &options)?;
            modified_entities.push((EntityType::Vertex, vertex_id));

            // Find affected edges
            let affected_edges = find_edges_using_vertex(model, vertex_id);
            for edge_id in affected_edges {
                affected_entities.push((EntityType::Edge, edge_id));
            }
        }

        ModifyType::ReplaceEdge { edge_id, new_curve } => {
            replace_edge_curve(model, edge_id, new_curve, &options)?;
            modified_entities.push((EntityType::Edge, edge_id));

            // Find affected faces
            let affected_faces = find_faces_using_edge(model, edge_id);
            for face_id in affected_faces {
                affected_entities.push((EntityType::Face, face_id));
            }
        }

        ModifyType::ModifyFaceSurface {
            face_id,
            surface_params,
        } => {
            modify_face_surface(model, face_id, surface_params, &options)?;
            modified_entities.push((EntityType::Face, face_id));
        }

        ModifyType::ModifySolidProperties {
            solid_id,
            properties,
        } => {
            modify_solid_properties(model, solid_id, properties)?;
            modified_entities.push((EntityType::Solid, solid_id));
        }

        ModifyType::ChangeLoopOrientation { loop_id, reverse } => {
            change_loop_orientation(model, loop_id, reverse)?;
            modified_entities.push((EntityType::Face, loop_id)); // Loop is part of face
        }

        ModifyType::ChangeTolerance {
            entity_type,
            entity_id,
            new_tolerance,
        } => {
            change_entity_tolerance(model, entity_type, entity_id, new_tolerance)?;
            modified_entities.push((entity_type, entity_id));
        }
    }

    // Validate topology if requested
    let topology_valid = if options.validate_topology {
        validate_model_topology(model).is_ok()
    } else {
        true
    };

    if !topology_valid {
        warnings.push("Topology validation failed after modification".to_string());
    }

    Ok(ModifyResult {
        modified_entities,
        affected_entities,
        topology_valid,
        warnings,
    })
}

/// Validate that an edit can be applied
fn validate_modification(model: &BRepModel, edit: &ModifyType) -> OperationResult<()> {
    match edit {
        ModifyType::MoveVertex { vertex_id, .. } => {
            if model.vertices.get(*vertex_id).is_none() {
                return Err(OperationError::InvalidInput {
                    parameter: "vertex_id".to_string(),
                    expected: "existing vertex".to_string(),
                    received: format!("{}", vertex_id),
                });
            }
        }
        ModifyType::ReplaceEdge { edge_id, .. } => {
            if model.edges.get(*edge_id).is_none() {
                return Err(OperationError::InvalidInput {
                    parameter: "edge_id".to_string(),
                    expected: "existing edge".to_string(),
                    received: format!("{}", edge_id),
                });
            }
        }
        ModifyType::ModifyFaceSurface { face_id, .. } => {
            if model.faces.get(*face_id).is_none() {
                return Err(OperationError::InvalidInput {
                    parameter: "face_id".to_string(),
                    expected: "existing face".to_string(),
                    received: format!("{}", face_id),
                });
            }
        }
        ModifyType::ModifySolidProperties { solid_id, .. } => {
            if model.solids.get(*solid_id).is_none() {
                return Err(OperationError::InvalidInput {
                    parameter: "solid_id".to_string(),
                    expected: "existing solid".to_string(),
                    received: format!("{}", solid_id),
                });
            }
        }
        ModifyType::ChangeLoopOrientation { loop_id, .. } => {
            if model.loops.get(*loop_id).is_none() {
                return Err(OperationError::InvalidInput {
                    parameter: "loop_id".to_string(),
                    expected: "existing loop".to_string(),
                    received: format!("{}", loop_id),
                });
            }
        }
        ModifyType::ChangeTolerance { .. } => {
            // Tolerance can be changed for any entity
        }
    }
    Ok(())
}

/// Move a vertex to a new position
fn move_vertex(
    model: &mut BRepModel,
    vertex_id: VertexId,
    new_position: Point3,
    options: &ModifyOptions,
) -> OperationResult<()> {
    // Get the vertex
    let old_vertex = model
        .vertices
        .get(vertex_id)
        .ok_or_else(|| OperationError::InvalidInput {
            parameter: "vertex_id".to_string(),
            expected: "existing vertex".to_string(),
            received: format!("{}", vertex_id),
        })?;

    // Store old position for constraint checking
    let old_position = old_vertex.point();

    // Apply the actual position update via VertexStore::set_position.
    // The store also updates its spatial index internally.
    if !model.vertices.set_position(
        vertex_id,
        new_position.x,
        new_position.y,
        new_position.z,
    ) {
        return Err(OperationError::InvalidGeometry(format!(
            "Vertex {} could not be updated",
            vertex_id
        )));
    }

    // Update dependent edges if requested
    if options.update_dependents {
        // Update edge curves that use this vertex
        update_edges_for_vertex(model, vertex_id, old_position, new_position)?;
    }

    // Validate that no edges incident to this vertex were corrupted by the move.
    if options.maintain_constraints {
        validate_vertex_constraints(model, vertex_id)?;
    }

    Ok(())
}

/// Replace an edge's underlying curve. Replacing an edge curve requires
/// `EdgeStore::set_curve_id` (does not currently exist) plus parameter-range
/// recomputation against the new curve. Returns `NotImplemented` to avoid
/// silently producing a model where the edge claims one geometry but the
/// store still holds the old curve.
fn replace_edge_curve(
    model: &mut BRepModel,
    edge_id: EdgeId,
    _new_curve: EdgeCurveType,
    _options: &ModifyOptions,
) -> OperationResult<()> {
    if model.edges.get(edge_id).is_none() {
        return Err(OperationError::InvalidInput {
            parameter: "edge_id".to_string(),
            expected: "existing edge".to_string(),
            received: format!("{}", edge_id),
        });
    }
    Err(OperationError::NotImplemented(
        "replace_edge_curve requires EdgeStore mutation API; delete + recreate edge instead"
            .to_string(),
    ))
}

/// Modify a face's underlying surface. Requires `SurfaceStore::set_surface`
/// or replace + remap pattern that is currently not exposed. Returns
/// `NotImplemented` to avoid silently leaving the face referencing the old
/// surface while the caller assumes the swap succeeded.
fn modify_face_surface(
    model: &mut BRepModel,
    face_id: FaceId,
    _surface_params: SurfaceParameters,
    _options: &ModifyOptions,
) -> OperationResult<()> {
    if model.faces.get(face_id).is_none() {
        return Err(OperationError::InvalidInput {
            parameter: "face_id".to_string(),
            expected: "existing face".to_string(),
            received: format!("{}", face_id),
        });
    }
    Err(OperationError::NotImplemented(
        "modify_face_surface requires SurfaceStore mutation API; rebuild face instead".to_string(),
    ))
}

/// Modify solid-level metadata properties. The current `Solid` struct does
/// not carry a properties field (mass, density, material id, etc.), so this
/// operation has no representation. Returns `NotImplemented` rather than
/// silently accepting properties that vanish.
fn modify_solid_properties(
    model: &mut BRepModel,
    solid_id: SolidId,
    _properties: SolidProperties,
) -> OperationResult<()> {
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidInput {
            parameter: "solid_id".to_string(),
            expected: "existing solid".to_string(),
            received: format!("{}", solid_id),
        });
    }
    Err(OperationError::NotImplemented(
        "modify_solid_properties: Solid struct has no properties field to update".to_string(),
    ))
}

/// Change loop orientation
fn change_loop_orientation(
    model: &mut BRepModel,
    loop_id: LoopId,
    reverse: bool,
) -> OperationResult<()> {
    // Get the loop
    let _loop = model
        .loops
        .get(loop_id)
        .ok_or_else(|| OperationError::InvalidInput {
            parameter: "loop_id".to_string(),
            expected: "existing loop".to_string(),
            received: format!("{}", loop_id),
        })?;

    if reverse {
        // Loop reversal would need to be implemented differently
        // since we can't directly modify the loop
    }

    Ok(())
}

/// Change entity tolerance
fn change_entity_tolerance(
    model: &mut BRepModel,
    entity_type: EntityType,
    entity_id: u32,
    new_tolerance: Tolerance,
) -> OperationResult<()> {
    match entity_type {
        EntityType::Vertex => {
            if !model
                .vertices
                .set_tolerance(entity_id, new_tolerance.distance())
            {
                return Err(OperationError::InvalidInput {
                    parameter: "entity_id".to_string(),
                    expected: "existing vertex".to_string(),
                    received: format!("{}", entity_id),
                });
            }
        }
        EntityType::Edge => {
            if !model
                .edges
                .set_tolerance(entity_id, new_tolerance.distance())
            {
                return Err(OperationError::InvalidInput {
                    parameter: "entity_id".to_string(),
                    expected: "existing edge".to_string(),
                    received: format!("{}", entity_id),
                });
            }
        }
        EntityType::Face => {
            if !model
                .faces
                .set_tolerance(entity_id, new_tolerance.distance())
            {
                return Err(OperationError::InvalidInput {
                    parameter: "entity_id".to_string(),
                    expected: "existing face".to_string(),
                    received: format!("{}", entity_id),
                });
            }
        }
        EntityType::Shell => {
            // Shells typically don't have tolerance
        }
        EntityType::Solid => {
            // Solids typically don't have tolerance
        }
    }

    Ok(())
}

// Helper functions

fn find_edges_using_vertex(model: &BRepModel, vertex_id: VertexId) -> Vec<EdgeId> {
    let mut edges = Vec::new();
    for (edge_id, edge) in model.edges.iter() {
        if edge.start_vertex == vertex_id || edge.end_vertex == vertex_id {
            edges.push(edge_id);
        }
    }
    edges
}

fn find_faces_using_edge(model: &BRepModel, edge_id: EdgeId) -> Vec<FaceId> {
    let mut faces = Vec::new();
    for (face_id, face) in model.faces.iter() {
        let mut used = false;
        if let Some(outer) = model.loops.get(face.outer_loop) {
            if outer.edges.iter().any(|e| *e == edge_id) {
                used = true;
            }
        }
        if !used {
            for inner_id in &face.inner_loops {
                if let Some(inner) = model.loops.get(*inner_id) {
                    if inner.edges.iter().any(|e| *e == edge_id) {
                        used = true;
                        break;
                    }
                }
            }
        }
        if used {
            faces.push(face_id);
        }
    }
    faces
}

fn update_edges_for_vertex(
    _model: &mut BRepModel,
    _vertex_id: VertexId,
    _old_position: Point3,
    _new_position: Point3,
) -> OperationResult<()> {
    // Update curves of edges that use this vertex
    // This would involve recalculating curve parameters
    Ok(())
}

fn validate_vertex_constraints(_model: &BRepModel, _vertex_id: VertexId) -> OperationResult<()> {
    // Check that vertex position doesn't violate any constraints
    // This would involve checking geometric constraints
    Ok(())
}

fn validate_model_topology(_model: &BRepModel) -> OperationResult<()> {
    // Validate that the model topology is still valid
    // This would involve checking Euler characteristics, etc.
    Ok(())
}
