//! BRep serialization and deserialization
//!
//! This module handles converting BRep models to/from serializable formats
//! for storage in the timeline system.

use crate::{TimelineError, TimelineResult};
use geometry_engine::{
    math::{Point3, Vector3},
    primitives::{
        curve::{CurveId, ParameterRange},
        edge::{Edge, EdgeId, EdgeOrientation},
        face::{Face, FaceId, FaceOrientation},
        r#loop::{Loop, LoopId, LoopType},
        shell::{Shell, ShellId, ShellType},
        solid::{Solid, SolidId},
        surface::SurfaceId,
        topology_builder::BRepModel,
        vertex::VertexId,
    },
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Serializable representation of a BRep model
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SerializedBRep {
    /// Vertices as coordinate arrays
    pub vertices: Vec<VertexData>,

    /// Edges with connectivity
    pub edges: Vec<EdgeData>,

    /// Loops (ordered edge sequences)
    pub loops: Vec<LoopData>,

    /// Faces with boundary loops
    pub faces: Vec<FaceData>,

    /// Shells (collections of faces)
    pub shells: Vec<ShellData>,

    /// Solids (one or more shells)
    pub solids: Vec<SolidData>,

    /// Curves (geometry for edges)
    pub curves: Vec<CurveData>,

    /// Surfaces (geometry for faces)
    pub surfaces: Vec<SurfaceData>,
}

/// Serializable vertex data
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct VertexData {
    pub id: VertexId,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// Serializable edge data
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct EdgeData {
    pub id: EdgeId,
    pub start_vertex: VertexId,
    pub end_vertex: VertexId,
    pub curve_id: Option<CurveId>,
    pub is_reversed: bool,
    pub param_start: f64,
    pub param_end: f64,
}

/// Serializable loop data
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LoopData {
    pub id: LoopId,
    pub edges: Vec<EdgeId>,
    pub orientations: Vec<bool>, // true = forward, false = reversed
    pub loop_type: String,       // "outer" or "inner"
}

/// Serializable face data
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FaceData {
    pub id: FaceId,
    pub outer_loop: LoopId,
    pub inner_loops: Vec<LoopId>,
    pub surface_id: Option<SurfaceId>,
    pub is_reversed: bool,
}

/// Serializable shell data
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ShellData {
    pub id: ShellId,
    pub faces: Vec<FaceId>,
    pub is_closed: bool,
}

/// Serializable solid data
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SolidData {
    pub id: SolidId,
    pub outer_shell: ShellId,
    pub inner_shells: Vec<ShellId>,
}

/// Serializable curve data (simplified)
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CurveData {
    pub id: CurveId,
    pub curve_type: String,
    pub data: Vec<f64>, // Type-specific data
}

/// Serializable surface data (simplified)
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SurfaceData {
    pub id: SurfaceId,
    pub surface_type: String,
    pub data: Vec<f64>, // Type-specific data
}

/// Convert BRep model to serializable format
pub fn brep_to_serialized(brep: &BRepModel) -> TimelineResult<SerializedBRep> {
    let mut serialized = SerializedBRep {
        vertices: Vec::new(),
        edges: Vec::new(),
        loops: Vec::new(),
        faces: Vec::new(),
        shells: Vec::new(),
        solids: Vec::new(),
        curves: Vec::new(),
        surfaces: Vec::new(),
    };

    // Serialize vertices
    for (id, vertex) in brep.vertices.iter() {
        let point = vertex.point();
        serialized.vertices.push(VertexData {
            id,
            x: point.x,
            y: point.y,
            z: point.z,
        });
    }

    // Serialize edges
    for (_id, edge) in brep.edges.iter() {
        serialized.edges.push(EdgeData {
            id: edge.id,
            start_vertex: edge.start_vertex,
            end_vertex: edge.end_vertex,
            curve_id: Some(edge.curve_id),
            is_reversed: matches!(edge.orientation, EdgeOrientation::Backward),
            param_start: edge.param_range.start,
            param_end: edge.param_range.end,
        });
    }

    // Serialize loops
    for (_id, loop_) in brep.loops.iter() {
        let mut edges = Vec::new();
        let mut orientations = Vec::new();

        // Access edges and orientations from the loop
        for i in 0..loop_.edges.len() {
            edges.push(loop_.edges[i]);
            orientations.push(loop_.orientations[i]);
        }

        serialized.loops.push(LoopData {
            id: loop_.id,
            edges,
            orientations,
            loop_type: match loop_.loop_type {
                LoopType::Outer => "outer".to_string(),
                LoopType::Inner => "inner".to_string(),
                _ => "outer".to_string(),
            },
        });
    }

    // Serialize faces
    for (_id, face) in brep.faces.iter() {
        serialized.faces.push(FaceData {
            id: face.id,
            outer_loop: face.outer_loop,
            inner_loops: face.inner_loops.clone(),
            surface_id: Some(face.surface_id),
            is_reversed: matches!(face.orientation, FaceOrientation::Backward),
        });
    }

    // Serialize shells
    for (_id, shell) in brep.shells.iter() {
        serialized.shells.push(ShellData {
            id: shell.id,
            faces: shell.faces.clone(),
            is_closed: matches!(shell.shell_type, ShellType::Closed),
        });
    }

    // Serialize solids
    for (_id, solid) in brep.solids.iter() {
        serialized.solids.push(SolidData {
            id: solid.id,
            outer_shell: solid.outer_shell,
            inner_shells: solid.inner_shells.clone(),
        });
    }

    // Serialize curves (simplified - just store IDs for now)
    // In a full implementation, we would serialize curve geometry
    for (id, _curve) in brep.curves.iter() {
        serialized.curves.push(CurveData {
            id,
            curve_type: "line".to_string(), // Simplified
            data: vec![],
        });
    }

    // Serialize surfaces (simplified - just store IDs for now)
    // In a full implementation, we would serialize surface geometry
    for (id, _surface) in brep.surfaces.iter() {
        serialized.surfaces.push(SurfaceData {
            id,
            surface_type: "plane".to_string(), // Simplified
            data: vec![],
        });
    }

    Ok(serialized)
}

/// Serialize BRep model to JSON
pub fn serialize_brep(brep: &BRepModel) -> TimelineResult<String> {
    let serialized = brep_to_serialized(brep)?;
    serde_json::to_string(&serialized)
        .map_err(|e| TimelineError::SerializationError(format!("Failed to serialize BRep: {}", e)))
}

/// Deserialize BRep model from JSON
pub fn deserialize_brep(json: &str) -> TimelineResult<BRepModel> {
    let data: SerializedBRep = serde_json::from_str(json).map_err(|e| {
        TimelineError::DeserializationError(format!("Failed to deserialize BRep: {}", e))
    })?;
    serialized_to_brep(&data)
}

/// Convert serialized format back to BRep model
pub fn serialized_to_brep(data: &SerializedBRep) -> TimelineResult<BRepModel> {
    let mut brep = BRepModel::new();

    // Create ID mappings to handle the fact that IDs might not be sequential
    let mut vertex_id_map: HashMap<VertexId, VertexId> = HashMap::new();
    let mut edge_id_map: HashMap<EdgeId, EdgeId> = HashMap::new();
    let mut loop_id_map: HashMap<LoopId, LoopId> = HashMap::new();
    let mut face_id_map: HashMap<FaceId, FaceId> = HashMap::new();
    let mut shell_id_map: HashMap<ShellId, ShellId> = HashMap::new();
    let _solid_id_map: HashMap<SolidId, SolidId> = HashMap::new();

    // Reconstruct vertices
    for vertex_data in &data.vertices {
        let new_id = brep
            .vertices
            .add(vertex_data.x, vertex_data.y, vertex_data.z);
        vertex_id_map.insert(vertex_data.id, new_id);
    }

    // Reconstruct curves based on type
    for curve_data in &data.curves {
        use geometry_engine::primitives::curve::{Circle, Line};

        let curve: Box<dyn geometry_engine::primitives::curve::Curve> =
            match curve_data.curve_type.as_str() {
                "Line" => {
                    // Extract line data from the data array
                    // Format: [start_x, start_y, start_z, end_x, end_y, end_z]
                    let start = if curve_data.data.len() >= 3 {
                        Point3::new(curve_data.data[0], curve_data.data[1], curve_data.data[2])
                    } else {
                        Point3::ZERO
                    };

                    let end = if curve_data.data.len() >= 6 {
                        Point3::new(curve_data.data[3], curve_data.data[4], curve_data.data[5])
                    } else {
                        Point3::new(1.0, 0.0, 0.0)
                    };

                    Box::new(Line::new(start, end))
                }
                "Arc" | "Circle" => {
                    // Extract circle/arc data from the data array
                    // Format: [center_x, center_y, center_z, radius]
                    let center = if curve_data.data.len() >= 3 {
                        Point3::new(curve_data.data[0], curve_data.data[1], curve_data.data[2])
                    } else {
                        Point3::ZERO
                    };

                    let radius = if curve_data.data.len() >= 4 {
                        curve_data.data[3]
                    } else {
                        1.0
                    };

                    Box::new(Circle::new(center, Vector3::Z, radius).map_err(|e| {
                        TimelineError::SerializationError(format!(
                            "Invalid circle in serialized BRep: {:?}",
                            e
                        ))
                    })?)
                }
                _ => {
                    // Default to a line for unknown types
                    Box::new(Line::new(Point3::ZERO, Point3::new(1.0, 0.0, 0.0)))
                }
            };

        brep.curves.add(curve);
    }

    // Reconstruct surfaces based on type
    for surface_data in &data.surfaces {
        use geometry_engine::primitives::surface::{Cylinder, Plane, Sphere};

        let surface: Box<dyn geometry_engine::primitives::surface::Surface> =
            match surface_data.surface_type.as_str() {
                "Plane" => {
                    // Extract plane data from the data array
                    // Format: [point_x, point_y, point_z, normal_x, normal_y, normal_z]
                    let point = if surface_data.data.len() >= 3 {
                        Point3::new(
                            surface_data.data[0],
                            surface_data.data[1],
                            surface_data.data[2],
                        )
                    } else {
                        Point3::ZERO
                    };

                    let normal = if surface_data.data.len() >= 6 {
                        Vector3::new(
                            surface_data.data[3],
                            surface_data.data[4],
                            surface_data.data[5],
                        )
                    } else {
                        Vector3::Z
                    };

                    Box::new(Plane::from_point_normal(point, normal).map_err(|e| {
                        TimelineError::SerializationError(format!(
                            "Invalid plane in serialized BRep: {:?}",
                            e
                        ))
                    })?)
                }
                "Cylinder" => {
                    // Extract cylinder data from the data array
                    // Format: [center_x, center_y, center_z, axis_x, axis_y, axis_z, radius]
                    let center = if surface_data.data.len() >= 3 {
                        Point3::new(
                            surface_data.data[0],
                            surface_data.data[1],
                            surface_data.data[2],
                        )
                    } else {
                        Point3::ZERO
                    };

                    let axis = if surface_data.data.len() >= 6 {
                        Vector3::new(
                            surface_data.data[3],
                            surface_data.data[4],
                            surface_data.data[5],
                        )
                    } else {
                        Vector3::Z
                    };

                    let radius = if surface_data.data.len() >= 7 {
                        surface_data.data[6]
                    } else {
                        1.0
                    };

                    Box::new(Cylinder::new(center, axis, radius).map_err(|e| {
                        TimelineError::SerializationError(format!(
                            "Invalid cylinder in serialized BRep: {:?}",
                            e
                        ))
                    })?)
                }
                "Sphere" => {
                    // Extract sphere data from the data array
                    // Format: [center_x, center_y, center_z, radius]
                    let center = if surface_data.data.len() >= 3 {
                        Point3::new(
                            surface_data.data[0],
                            surface_data.data[1],
                            surface_data.data[2],
                        )
                    } else {
                        Point3::ZERO
                    };

                    let radius = if surface_data.data.len() >= 4 {
                        surface_data.data[3]
                    } else {
                        1.0
                    };

                    Box::new(Sphere::new(center, radius).map_err(|e| {
                        TimelineError::SerializationError(format!(
                            "Invalid sphere in serialized BRep: {:?}",
                            e
                        ))
                    })?)
                }
                _ => {
                    // Default to a plane for unknown types
                    Box::new(
                        Plane::from_point_normal(Point3::ZERO, Vector3::Z).map_err(|e| {
                            TimelineError::SerializationError(format!(
                                "Failed to construct default plane for unknown surface type: {:?}",
                                e
                            ))
                        })?,
                    )
                }
            };

        brep.surfaces.add(surface);
    }

    // Reconstruct edges
    for edge_data in &data.edges {
        let orientation = if edge_data.is_reversed {
            EdgeOrientation::Backward
        } else {
            EdgeOrientation::Forward
        };

        // Map vertex IDs
        let start_vertex = *vertex_id_map.get(&edge_data.start_vertex).ok_or_else(|| {
            TimelineError::DeserializationError(format!(
                "Invalid start vertex ID: {}",
                edge_data.start_vertex
            ))
        })?;
        let end_vertex = *vertex_id_map.get(&edge_data.end_vertex).ok_or_else(|| {
            TimelineError::DeserializationError(format!(
                "Invalid end vertex ID: {}",
                edge_data.end_vertex
            ))
        })?;

        let edge = Edge::new(
            0, // Will be assigned by store
            start_vertex,
            end_vertex,
            edge_data.curve_id.unwrap_or(0),
            orientation,
            ParameterRange::new(edge_data.param_start, edge_data.param_end),
        );
        let new_id = brep.edges.add(edge);
        edge_id_map.insert(edge_data.id, new_id);
    }

    // Reconstruct loops
    for loop_data in &data.loops {
        let loop_type = match loop_data.loop_type.as_str() {
            "outer" => LoopType::Outer,
            "inner" => LoopType::Inner,
            _ => LoopType::Outer,
        };

        let mut loop_ = Loop::new(0, loop_type); // ID will be assigned by store

        // Add edges to the loop
        for (i, &edge_id) in loop_data.edges.iter().enumerate() {
            let mapped_edge_id = *edge_id_map.get(&edge_id).ok_or_else(|| {
                TimelineError::DeserializationError(format!("Invalid edge ID in loop: {}", edge_id))
            })?;
            let forward = loop_data.orientations[i];
            loop_.add_edge(mapped_edge_id, forward);
        }

        let new_id = brep.loops.add(loop_);
        loop_id_map.insert(loop_data.id, new_id);
    }

    // Reconstruct faces
    for face_data in &data.faces {
        let orientation = if face_data.is_reversed {
            FaceOrientation::Backward
        } else {
            FaceOrientation::Forward
        };

        // Map loop IDs
        let outer_loop = *loop_id_map.get(&face_data.outer_loop).ok_or_else(|| {
            TimelineError::DeserializationError(format!(
                "Invalid outer loop ID: {}",
                face_data.outer_loop
            ))
        })?;

        let mut face = Face::new(
            0, // Will be assigned by store
            face_data.surface_id.unwrap_or(0),
            outer_loop,
            orientation,
        );

        // Add inner loops
        for &inner_loop_id in &face_data.inner_loops {
            let mapped_loop_id = *loop_id_map.get(&inner_loop_id).ok_or_else(|| {
                TimelineError::DeserializationError(format!(
                    "Invalid inner loop ID: {}",
                    inner_loop_id
                ))
            })?;
            face.add_inner_loop(mapped_loop_id);
        }

        let new_id = brep.faces.add(face);
        face_id_map.insert(face_data.id, new_id);
    }

    // Reconstruct shells
    for shell_data in &data.shells {
        let shell_type = if shell_data.is_closed {
            ShellType::Closed
        } else {
            ShellType::Open
        };

        let mut shell = Shell::new(0, shell_type); // ID will be assigned by store

        // Add faces to the shell
        for &face_id in &shell_data.faces {
            let mapped_face_id = *face_id_map.get(&face_id).ok_or_else(|| {
                TimelineError::DeserializationError(format!(
                    "Invalid face ID in shell: {}",
                    face_id
                ))
            })?;
            shell.add_face(mapped_face_id);
        }

        let new_id = brep.shells.add(shell);
        shell_id_map.insert(shell_data.id, new_id);
    }

    // Reconstruct solids
    for solid_data in &data.solids {
        // Map shell IDs
        let outer_shell = *shell_id_map.get(&solid_data.outer_shell).ok_or_else(|| {
            TimelineError::DeserializationError(format!(
                "Invalid outer shell ID: {}",
                solid_data.outer_shell
            ))
        })?;

        let mut solid = Solid::new(0, outer_shell); // ID will be assigned by store

        // Add inner shells
        for &inner_shell_id in &solid_data.inner_shells {
            let mapped_shell_id = *shell_id_map.get(&inner_shell_id).ok_or_else(|| {
                TimelineError::DeserializationError(format!(
                    "Invalid inner shell ID: {}",
                    inner_shell_id
                ))
            })?;
            solid.add_inner_shell(mapped_shell_id);
        }

        brep.solids.add(solid);
    }

    Ok(brep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_brep_serialization_roundtrip() {
        // Create a simple BRep model
        let mut brep = BRepModel::new();

        // Add some vertices
        let v1 = brep.vertices.add_or_find(0.0, 0.0, 0.0, 1e-6);
        let v2 = brep.vertices.add_or_find(1.0, 0.0, 0.0, 1e-6);

        // Serialize and deserialize
        let serialized = serialize_brep(&brep).unwrap();
        let deserialized = deserialize_brep(&serialized).unwrap();

        // Check that we got the same number of vertices back
        assert_eq!(brep.vertices.len(), deserialized.vertices.len());
    }
}
