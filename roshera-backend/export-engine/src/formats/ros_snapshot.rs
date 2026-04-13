//! Serializable snapshot of B-Rep model for ROS format
//!
//! Since BRepModel uses DashMap for concurrent access, we need a
//! serializable representation for export/import

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::{
    edge::Edge, face::Face, r#loop::Loop, shell::Shell, solid::Solid, surface::Surface,
    topology_builder::BRepModel, vertex::Vertex,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Serializable snapshot of a B-Rep model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BRepSnapshot {
    /// Vertices with their IDs
    pub vertices: Vec<(Uuid, VertexData)>,

    /// Curves with their IDs  
    pub curves: Vec<(Uuid, CurveData)>,

    /// Edges with their IDs
    pub edges: Vec<(Uuid, EdgeData)>,

    /// Loops with their IDs
    pub loops: Vec<(Uuid, LoopData)>,

    /// Faces with their IDs
    pub faces: Vec<(Uuid, FaceData)>,

    /// Surfaces with their IDs
    pub surfaces: Vec<(Uuid, SurfaceData)>,

    /// Shells with their IDs
    pub shells: Vec<(Uuid, ShellData)>,

    /// Solids with their IDs
    pub solids: Vec<(Uuid, SolidData)>,

    /// Metadata
    pub metadata: BRepMetadata,
}

/// Serializable vertex data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VertexData {
    pub position: [f64; 3],
    pub tolerance: f64,
}

/// Serializable curve data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CurveData {
    Line {
        start: [f64; 3],
        end: [f64; 3],
    },
    Circle {
        center: [f64; 3],
        normal: [f64; 3],
        radius: f64,
    },
    Arc {
        center: [f64; 3],
        normal: [f64; 3],
        radius: f64,
        start_angle: f64,
        end_angle: f64,
    },
    BSpline {
        control_points: Vec<[f64; 3]>,
        knots: Vec<f64>,
        degree: u32,
    },
    Nurbs {
        control_points: Vec<[f64; 3]>,
        weights: Vec<f64>,
        knots: Vec<f64>,
        degree: u32,
    },
}

/// Serializable edge data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeData {
    pub start_vertex: Uuid,
    pub end_vertex: Uuid,
    pub curve: Option<Uuid>,
    pub orientation: bool,
}

/// Serializable loop data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopData {
    pub edges: Vec<Uuid>,
    pub orientations: Vec<bool>,
    pub is_outer: bool,
}

/// Serializable face data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaceData {
    pub surface: Option<Uuid>,
    pub outer_loop: Option<Uuid>,
    pub inner_loops: Vec<Uuid>,
    pub orientation: bool,
}

/// Serializable surface data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SurfaceData {
    Plane {
        origin: [f64; 3],
        normal: [f64; 3],
    },
    Cylinder {
        origin: [f64; 3],
        axis: [f64; 3],
        radius: f64,
    },
    Sphere {
        center: [f64; 3],
        radius: f64,
    },
    Cone {
        apex: [f64; 3],
        axis: [f64; 3],
        half_angle: f64,
    },
    Torus {
        center: [f64; 3],
        axis: [f64; 3],
        major_radius: f64,
        minor_radius: f64,
    },
    BSpline {
        control_points: Vec<Vec<[f64; 3]>>,
        knots_u: Vec<f64>,
        knots_v: Vec<f64>,
        degree_u: u32,
        degree_v: u32,
    },
    Nurbs {
        control_points: Vec<Vec<[f64; 3]>>,
        weights: Vec<Vec<f64>>,
        knots_u: Vec<f64>,
        knots_v: Vec<f64>,
        degree_u: u32,
        degree_v: u32,
    },
}

/// Serializable shell data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellData {
    pub faces: Vec<Uuid>,
    pub is_closed: bool,
    pub shell_type: ShellType,
}

/// Shell type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ShellType {
    Open,
    Closed,
    Compound,
}

/// Serializable solid data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolidData {
    pub shells: Vec<Uuid>,
    pub feature_type: Option<String>,
}

/// B-Rep metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BRepMetadata {
    /// Creation timestamp
    pub created_at: u64,

    /// Last modified timestamp
    pub modified_at: u64,

    /// Unit of measurement
    pub units: String,

    /// Tolerance value
    pub tolerance: f64,

    /// Additional properties
    pub properties: HashMap<String, serde_json::Value>,
}

impl BRepSnapshot {
    /// Create a new empty snapshot
    pub fn new() -> Self {
        Self {
            vertices: Vec::new(),
            curves: Vec::new(),
            edges: Vec::new(),
            loops: Vec::new(),
            faces: Vec::new(),
            surfaces: Vec::new(),
            shells: Vec::new(),
            solids: Vec::new(),
            metadata: BRepMetadata {
                created_at: crate::ros_fs::current_time_ms(),
                modified_at: crate::ros_fs::current_time_ms(),
                units: "millimeters".to_string(),
                tolerance: 1e-6,
                properties: HashMap::new(),
            },
        }
    }

    /// Convert from BRepModel to snapshot
    pub fn from_model(model: &BRepModel) -> Self {
        let mut snapshot = Self::new();

        // TODO: Iterate through DashMap stores and extract data
        // This would require methods on BRepModel to iterate its contents
        // For now, return empty snapshot

        snapshot
    }

    /// Convert from snapshot to BRepModel
    pub fn to_model(&self) -> BRepModel {
        let model = BRepModel::new();

        // TODO: Reconstruct BRepModel from snapshot data
        // This would require methods on BRepModel to add entities with specific IDs
        // For now, return empty model

        model
    }
}

impl Default for BRepSnapshot {
    fn default() -> Self {
        Self::new()
    }
}
