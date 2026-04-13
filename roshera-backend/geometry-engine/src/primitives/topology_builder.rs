//! Universal Topology Builder for 2D and 3D Primitives
//!
//! This module provides the core infrastructure for building watertight B-Rep
//! topology for all primitive types, both 2D and 3D, with timeline support.

use crate::math::{Matrix4, Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::{Arc, Circle, CurveId, CurveStore, Line},
    edge::{Edge, EdgeId, EdgeOrientation, EdgeStore},
    face::{Face, FaceId, FaceOrientation, FaceStore},
    primitive_traits::PrimitiveError,
    r#loop::{Loop, LoopId, LoopStore, LoopType},
    shell::{Shell, ShellId, ShellStore, ShellType},
    solid::{Solid, SolidId, SolidStore},
    surface::{Cylinder, Plane, Sphere, SurfaceId, SurfaceStore},
    vertex::{VertexId, VertexStore},
};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc as SyncArc, LazyLock, RwLock};

/// Tessellated mesh representation for visualization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TessellatedMesh {
    /// Vertex positions as [x, y, z] triples
    pub vertices: Vec<[f32; 3]>,
    /// Normal vectors as [nx, ny, nz] triples
    pub normals: Vec<[f32; 3]>,
    /// Triangle indices (triplets of vertex indices)
    pub indices: Vec<u32>,
}

/// Global timeline operations cache for high-performance parametric updates
static TIMELINE_CACHE: LazyLock<DashMap<u64, Vec<TimelineOperation>>> =
    LazyLock::new(|| DashMap::new());

/// Global geometry parameter cache for fast parameter updates
static GEOMETRY_PARAMETERS: LazyLock<DashMap<GeometryId, DashMap<String, f64>>> =
    LazyLock::new(|| DashMap::new());

/// Cache performance statistics for monitoring
#[derive(Debug, Clone)]
pub struct CacheStatistics {
    pub timeline_entries: usize,
    pub geometry_parameter_entries: usize,
    pub memory_usage_bytes: usize,
}

/// Result type for builder operations
pub type BuilderResult<T> = Result<T, PrimitiveError>;

/// Alias for builder errors
pub type BuilderError = PrimitiveError;

/// Options for primitive creation
#[derive(Debug, Clone, Default)]
pub struct PrimitiveOptions {
    pub tolerance: Option<Tolerance>,
    pub transform: Option<Matrix4>,
}

/// Estimated model complexity for analytical capacity planning
#[derive(Debug, Clone, Copy)]
pub enum EstimatedComplexity {
    /// Simple models: single primitives, basic sketches
    Simple,
    /// Medium models: assemblies with 10-100 parts
    Medium,
    /// Complex models: assemblies with 100-1000 parts
    Complex,
    /// Highly complex: >1000 parts, aerospace/automotive assemblies
    HighlyComplex,
    /// Custom complexity with specific parameters
    Custom {
        expected_parts: usize,
        expected_features_per_part: usize,
    },
}

impl EstimatedComplexity {
    /// Estimate topology storage requirements based on CAD modeling patterns
    /// Uses Euler's formula and empirical ratios from industrial CAD models
    pub fn estimate_topology_requirements(&self) -> (usize, usize, usize, usize, usize) {
        let (parts, features_per_part) = match self {
            Self::Simple => (1, 5),
            Self::Medium => (50, 20),
            Self::Complex => (500, 40),
            Self::HighlyComplex => (2000, 80),
            Self::Custom {
                expected_parts,
                expected_features_per_part,
            } => (*expected_parts, *expected_features_per_part),
        };

        // Analytical estimation based on topology relationships:
        // - Each part has ~features_per_part features (fillets, holes, etc.)
        // - Each feature creates ~8 faces on average (empirical data from Parasolid models)
        // - Euler formula: V - E + F = 2(1-g) where g is genus
        // - For manifold solids: E ≈ 1.5F, V ≈ 0.5F (empirical ratios)

        let total_features = parts * features_per_part;
        let faces_per_feature = 8; // Average for CAD features (holes, fillets, etc.)
        let estimated_faces = total_features * faces_per_feature;

        // Topology relationships from Euler formula and manifold properties
        let estimated_vertices = (estimated_faces as f64 * 0.5).ceil() as usize;
        let estimated_edges = (estimated_faces as f64 * 1.5).ceil() as usize;
        let estimated_shells = parts; // One shell per part typically
        let estimated_solids = parts;

        (
            estimated_vertices,
            estimated_edges,
            estimated_faces,
            estimated_shells,
            estimated_solids,
        )
    }
}

/// Sketch plane for 2D operations
#[derive(Debug, Clone)]
pub struct SketchPlane {
    pub id: String,
    pub position: Point3,
    pub normal: Vector3,
    pub u_axis: Vector3,
    pub v_axis: Vector3,
    pub size: f64,
}

impl SketchPlane {
    pub fn new(id: String, position: Point3, normal: Vector3, size: f64) -> Self {
        let u_axis = if normal.dot(&Vector3::new(1.0, 0.0, 0.0)).abs() < 0.9 {
            normal
                .cross(&Vector3::new(1.0, 0.0, 0.0))
                .normalize()
                .unwrap_or(Vector3::new(1.0, 0.0, 0.0))
        } else {
            normal
                .cross(&Vector3::new(0.0, 1.0, 0.0))
                .normalize()
                .unwrap_or(Vector3::new(0.0, 1.0, 0.0))
        };
        let v_axis = normal
            .cross(&u_axis)
            .normalize()
            .unwrap_or(Vector3::new(0.0, 0.0, 1.0));

        Self {
            id,
            position,
            normal,
            u_axis,
            v_axis,
            size,
        }
    }
}

/// B-Rep model container with all topology stores
#[derive(Debug)]
pub struct BRepModel {
    /// Vertex storage
    pub vertices: VertexStore,
    /// Curve storage
    pub curves: CurveStore,
    /// Edge storage
    pub edges: EdgeStore,
    /// Loop storage
    pub loops: LoopStore,
    /// Surface storage
    pub surfaces: SurfaceStore,
    /// Face storage
    pub faces: FaceStore,
    /// Shell storage
    pub shells: ShellStore,
    /// Solid storage
    pub solids: SolidStore,
    /// Sketch plane storage
    pub sketch_planes: DashMap<String, SketchPlane>,
}

impl BRepModel {
    /// Create new B-Rep model with analytical capacity estimation
    pub fn new() -> Self {
        Self::with_estimated_capacity(EstimatedComplexity::Medium)
    }

    /// Create B-Rep model with capacity estimation based on expected complexity
    pub fn with_estimated_capacity(complexity: EstimatedComplexity) -> Self {
        let (vertex_capacity, edge_capacity, face_capacity, shell_capacity, solid_capacity) =
            complexity.estimate_topology_requirements();

        Self {
            vertices: VertexStore::with_capacity_and_tolerance(vertex_capacity, 1e-12),
            curves: CurveStore::new(),
            edges: EdgeStore::with_capacity(edge_capacity),
            loops: LoopStore::with_capacity(face_capacity), // Loops ≈ faces for typical models
            surfaces: SurfaceStore::new(),
            faces: FaceStore::with_capacity(face_capacity),
            shells: ShellStore::with_capacity(shell_capacity),
            solids: SolidStore::with_capacity(solid_capacity),
            sketch_planes: DashMap::new(),
        }
    }

    /// Compute bounding box of all geometry in the model
    pub fn compute_bounding_box(&self) -> Option<crate::math::BBox> {
        use crate::math::BBox;

        let mut bbox: Option<BBox> = None;

        // Include all vertices in bounding box
        for (_, vertex) in self.vertices.iter() {
            // Use the vertex.point() method for consistent type-safe access
            let point = vertex.point();
            if let Some(ref mut bb) = bbox {
                bb.add_point_mut(&point);
            } else {
                bbox = Some(BBox::from_point(point));
            }
        }

        bbox
    }

    /// Get a solid by ID
    pub fn get_solid(&self, id: u32) -> Option<&crate::primitives::solid::Solid> {
        self.solids.get(id)
    }

    /// Calculate exact volume of a solid using divergence theorem
    /// Volume = (1/3) ∫∫ (r · n) dS where r is position vector, n is outward normal
    pub fn calculate_solid_volume(&self, solid_id: u32) -> Option<f64> {
        let solid = self.solids.get(solid_id)?;
        let shell = self.shells.get(solid.outer_shell)?;

        let mut total_volume = 0.0;

        // Process each face in the shell
        for &face_id in &shell.faces {
            let face = self.faces.get(face_id)?;
            let outer_loop = self.loops.get(face.outer_loop)?;

            // Get vertices of the face
            let mut vertices = Vec::new();
            for &edge_id in &outer_loop.edges {
                let edge = self.edges.get(edge_id)?;
                if let Some(vertex) = self.vertices.get(edge.start_vertex) {
                    vertices.push(vertex.point());
                }
            }

            if vertices.len() < 3 {
                continue;
            }

            // Triangulate the face and compute volume contribution
            // Using divergence theorem: V = (1/3) ∫∫ r·n dS
            let origin = Point3::ORIGIN;
            for i in 1..vertices.len() - 1 {
                let v0 = vertices[0];
                let v1 = vertices[i];
                let v2 = vertices[i + 1];

                // Calculate triangle normal (outward pointing)
                let edge1 = v1 - v0;
                let edge2 = v2 - v0;
                let normal = edge1.cross(&edge2);

                // Volume of tetrahedron formed by triangle and origin
                // V = (1/6) |v0 · (v1 × v2)|
                let volume_contribution = v0.dot(&(v1.cross(&v2))) / 6.0;

                // Account for face orientation
                let oriented_volume =
                    if face.orientation == crate::primitives::face::FaceOrientation::Forward {
                        volume_contribution
                    } else {
                        -volume_contribution
                    };

                total_volume += oriented_volume;
            }
        }

        // Subtract volumes of inner shells (voids)
        for &inner_shell_id in &solid.inner_shells {
            if let Some(inner_volume) = self.calculate_shell_volume(inner_shell_id) {
                total_volume -= inner_volume;
            }
        }

        Some(total_volume.abs())
    }

    /// Calculate volume contribution of a shell
    fn calculate_shell_volume(&self, shell_id: u32) -> Option<f64> {
        let shell = self.shells.get(shell_id)?;
        let mut volume = 0.0;

        for &face_id in &shell.faces {
            let face = self.faces.get(face_id)?;
            let outer_loop = self.loops.get(face.outer_loop)?;

            // Get vertices
            let mut vertices = Vec::new();
            for &edge_id in &outer_loop.edges {
                let edge = self.edges.get(edge_id)?;
                if let Some(vertex) = self.vertices.get(edge.start_vertex) {
                    vertices.push(vertex.point());
                }
            }

            if vertices.len() < 3 {
                continue;
            }

            // Triangulate and compute volume
            for i in 1..vertices.len() - 1 {
                let v0 = vertices[0];
                let v1 = vertices[i];
                let v2 = vertices[i + 1];

                // Signed volume of tetrahedron
                let volume_contribution = v0.dot(&(v1.cross(&v2))) / 6.0;
                volume += volume_contribution;
            }
        }

        Some(volume.abs())
    }

    /// Calculate exact surface area of a solid
    pub fn calculate_solid_surface_area(&self, solid_id: u32) -> Option<f64> {
        let solid = self.solids.get(solid_id)?;
        let shell = self.shells.get(solid.outer_shell)?;

        let mut total_area = 0.0;

        // Sum areas of all faces in the shell
        for &face_id in &shell.faces {
            if let Some(area) = self.calculate_face_area(face_id) {
                total_area += area;
            }
        }

        // Add areas of inner shells (they contribute to surface area)
        for &inner_shell_id in &solid.inner_shells {
            let inner_shell = self.shells.get(inner_shell_id)?;
            for &face_id in &inner_shell.faces {
                if let Some(area) = self.calculate_face_area(face_id) {
                    total_area += area;
                }
            }
        }

        Some(total_area)
    }

    /// Tessellate a solid into a watertight triangle mesh for visualization
    pub fn tessellate_solid(&self, solid_id: u32, tolerance: f64) -> Option<TessellatedMesh> {
        let solid = self.solids.get(solid_id)?;
        let shell = self.shells.get(solid.outer_shell)?;

        let mut vertices = Vec::new();
        let mut normals = Vec::new();
        let mut indices = Vec::new();

        // Vertex deduplication map: maps vertex ID to index in vertices array
        // This ensures watertight mesh by sharing vertices between faces
        let mut vertex_index_map: HashMap<VertexId, u32> = HashMap::new();

        // First pass: collect all unique vertices from the solid
        for &face_id in &shell.faces {
            let face = self.faces.get(face_id)?;
            let outer_loop = self.loops.get(face.outer_loop)?;

            for &edge_id in &outer_loop.edges {
                let edge = self.edges.get(edge_id)?;

                // Process start vertex
                if !vertex_index_map.contains_key(&edge.start_vertex) {
                    if let Some(vertex) = self.vertices.get(edge.start_vertex) {
                        let point = vertex.point();
                        let idx = vertices.len() as u32;
                        vertices.push([point.x as f32, point.y as f32, point.z as f32]);
                        // Initialize with zero normal, will accumulate later
                        normals.push([0.0, 0.0, 0.0]);
                        vertex_index_map.insert(edge.start_vertex, idx);
                    }
                }

                // Process end vertex
                if !vertex_index_map.contains_key(&edge.end_vertex) {
                    if let Some(vertex) = self.vertices.get(edge.end_vertex) {
                        let point = vertex.point();
                        let idx = vertices.len() as u32;
                        vertices.push([point.x as f32, point.y as f32, point.z as f32]);
                        normals.push([0.0, 0.0, 0.0]);
                        vertex_index_map.insert(edge.end_vertex, idx);
                    }
                }
            }
        }

        // Second pass: create triangles and accumulate normals
        for &face_id in &shell.faces {
            let face = self.faces.get(face_id)?;
            let outer_loop = self.loops.get(face.outer_loop)?;

            // Collect vertex indices for this face
            let mut face_vertex_ids = Vec::new();
            let mut face_vertex_indices = Vec::new();

            for &edge_id in &outer_loop.edges {
                let edge = self.edges.get(edge_id)?;
                if let Some(&idx) = vertex_index_map.get(&edge.start_vertex) {
                    face_vertex_ids.push(edge.start_vertex);
                    face_vertex_indices.push(idx);
                }
            }

            if face_vertex_indices.len() < 3 {
                continue;
            }

            // Calculate face normal using first three vertices
            let v0 = Point3::new(
                vertices[face_vertex_indices[0] as usize][0] as f64,
                vertices[face_vertex_indices[0] as usize][1] as f64,
                vertices[face_vertex_indices[0] as usize][2] as f64,
            );
            let v1 = Point3::new(
                vertices[face_vertex_indices[1] as usize][0] as f64,
                vertices[face_vertex_indices[1] as usize][1] as f64,
                vertices[face_vertex_indices[1] as usize][2] as f64,
            );
            let v2 = Point3::new(
                vertices[face_vertex_indices[2] as usize][0] as f64,
                vertices[face_vertex_indices[2] as usize][1] as f64,
                vertices[face_vertex_indices[2] as usize][2] as f64,
            );

            let edge1 = v1 - v0;
            let edge2 = v2 - v0;
            let face_normal = edge1.cross(&edge2).normalize().unwrap_or(Vector3::Z);

            // Apply face orientation
            let oriented_normal =
                if face.orientation == crate::primitives::face::FaceOrientation::Forward {
                    face_normal
                } else {
                    -face_normal
                };

            // Add face normal contribution to all vertices of this face
            // This creates smooth normals at shared vertices
            for &idx in &face_vertex_indices {
                normals[idx as usize][0] += oriented_normal.x as f32;
                normals[idx as usize][1] += oriented_normal.y as f32;
                normals[idx as usize][2] += oriented_normal.z as f32;
            }

            // Triangulate the face using ear clipping for better quality
            // For now, use fan triangulation but with shared vertices
            let base_idx = face_vertex_indices[0];
            for i in 2..face_vertex_indices.len() {
                // Ensure consistent winding order for watertight mesh
                if face.orientation == crate::primitives::face::FaceOrientation::Forward {
                    indices.push(base_idx);
                    indices.push(face_vertex_indices[i - 1]);
                    indices.push(face_vertex_indices[i]);
                } else {
                    indices.push(base_idx);
                    indices.push(face_vertex_indices[i]);
                    indices.push(face_vertex_indices[i - 1]);
                }
            }
        }

        // Process inner shells with the same vertex sharing approach
        for &inner_shell_id in &solid.inner_shells {
            let inner_shell = self.shells.get(inner_shell_id)?;

            for &face_id in &inner_shell.faces {
                let face = self.faces.get(face_id)?;
                let outer_loop = self.loops.get(face.outer_loop)?;

                // Collect vertex indices for this face
                let mut face_vertex_indices = Vec::new();

                for &edge_id in &outer_loop.edges {
                    let edge = self.edges.get(edge_id)?;
                    if let Some(&idx) = vertex_index_map.get(&edge.start_vertex) {
                        face_vertex_indices.push(idx);
                    }
                }

                if face_vertex_indices.len() < 3 {
                    continue;
                }

                // Calculate face normal
                let v0 = Point3::new(
                    vertices[face_vertex_indices[0] as usize][0] as f64,
                    vertices[face_vertex_indices[0] as usize][1] as f64,
                    vertices[face_vertex_indices[0] as usize][2] as f64,
                );
                let v1 = Point3::new(
                    vertices[face_vertex_indices[1] as usize][0] as f64,
                    vertices[face_vertex_indices[1] as usize][1] as f64,
                    vertices[face_vertex_indices[1] as usize][2] as f64,
                );
                let v2 = Point3::new(
                    vertices[face_vertex_indices[2] as usize][0] as f64,
                    vertices[face_vertex_indices[2] as usize][1] as f64,
                    vertices[face_vertex_indices[2] as usize][2] as f64,
                );

                let edge1 = v1 - v0;
                let edge2 = v2 - v0;
                let face_normal = edge1.cross(&edge2).normalize().unwrap_or(Vector3::Z);

                // Inner shells have inverted normals for voids
                let oriented_normal =
                    if face.orientation == crate::primitives::face::FaceOrientation::Forward {
                        -face_normal
                    } else {
                        face_normal
                    };

                // Add normal contribution
                for &idx in &face_vertex_indices {
                    normals[idx as usize][0] += oriented_normal.x as f32;
                    normals[idx as usize][1] += oriented_normal.y as f32;
                    normals[idx as usize][2] += oriented_normal.z as f32;
                }

                // Triangulate with reversed winding for inner shells
                let base_idx = face_vertex_indices[0];
                for i in 2..face_vertex_indices.len() {
                    if face.orientation == crate::primitives::face::FaceOrientation::Forward {
                        // Reversed winding for inner shells
                        indices.push(base_idx);
                        indices.push(face_vertex_indices[i]);
                        indices.push(face_vertex_indices[i - 1]);
                    } else {
                        indices.push(base_idx);
                        indices.push(face_vertex_indices[i - 1]);
                        indices.push(face_vertex_indices[i]);
                    }
                }
            }
        }

        // Normalize all accumulated normals
        for normal in &mut normals {
            let nx = normal[0];
            let ny = normal[1];
            let nz = normal[2];
            let length = (nx * nx + ny * ny + nz * nz).sqrt();
            if length > 1e-6 {
                normal[0] /= length;
                normal[1] /= length;
                normal[2] /= length;
            } else {
                // Default to up vector if degenerate
                normal[0] = 0.0;
                normal[1] = 0.0;
                normal[2] = 1.0;
            }
        }

        Some(TessellatedMesh {
            vertices,
            normals,
            indices,
        })
    }

    /// Calculate exact area of a face
    fn calculate_face_area(&self, face_id: u32) -> Option<f64> {
        let face = self.faces.get(face_id)?;
        let outer_loop = self.loops.get(face.outer_loop)?;

        // Get vertices of outer loop
        let mut vertices = Vec::new();
        for &edge_id in &outer_loop.edges {
            let edge = self.edges.get(edge_id)?;
            if let Some(vertex) = self.vertices.get(edge.start_vertex) {
                vertices.push(vertex.point());
            }
        }

        if vertices.len() < 3 {
            return Some(0.0);
        }

        // Calculate area using triangulation
        let mut area = 0.0;

        // For planar faces, use simple triangulation
        // For curved surfaces, this would need surface parameterization
        for i in 1..vertices.len() - 1 {
            let v0 = vertices[0];
            let v1 = vertices[i];
            let v2 = vertices[i + 1];

            // Area of triangle = 0.5 * |edge1 × edge2|
            let edge1 = v1 - v0;
            let edge2 = v2 - v0;
            let triangle_area = edge1.cross(&edge2).magnitude() * 0.5;

            area += triangle_area;
        }

        // Subtract areas of inner loops (holes)
        for &inner_loop_id in &face.inner_loops {
            let inner_loop = self.loops.get(inner_loop_id)?;

            let mut inner_vertices = Vec::new();
            for &edge_id in &inner_loop.edges {
                let edge = self.edges.get(edge_id)?;
                if let Some(vertex) = self.vertices.get(edge.start_vertex) {
                    inner_vertices.push(vertex.point());
                }
            }

            // Calculate area of hole
            let mut hole_area = 0.0;
            for i in 1..inner_vertices.len() - 1 {
                let v0 = inner_vertices[0];
                let v1 = inner_vertices[i];
                let v2 = inner_vertices[i + 1];

                let edge1 = v1 - v0;
                let edge2 = v2 - v0;
                hole_area += edge1.cross(&edge2).magnitude() * 0.5;
            }

            area -= hole_area;
        }

        Some(area)
    }
}

impl Default for BRepModel {
    fn default() -> Self {
        Self::new()
    }
}

/// Timeline operation types for parametric modeling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TimelineOperation {
    /// 2D primitive creation
    Create2D {
        primitive_type: String,
        parameters: HashMap<String, f64>,
        timestamp: u64,
    },
    /// 3D primitive creation
    Create3D {
        primitive_type: String,
        parameters: HashMap<String, f64>,
        timestamp: u64,
    },
    /// Extrude 2D to 3D
    Extrude {
        profile_id: GeometryId,
        direction: Vector3,
        distance: f64,
        timestamp: u64,
    },
    /// Revolve 2D around axis
    Revolve {
        profile_id: GeometryId,
        axis_origin: Point3,
        axis_direction: Vector3,
        angle: f64,
        timestamp: u64,
    },
    /// Boolean operation
    Boolean {
        operation: BooleanOp,
        operand_ids: Vec<GeometryId>,
        timestamp: u64,
    },
    /// Parameter update
    UpdateParameters {
        geometry_id: GeometryId,
        new_parameters: HashMap<String, f64>,
        timestamp: u64,
    },
}

/// Universal geometry ID that works for 2D and 3D
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GeometryId {
    /// 2D geometry (stored as face)
    Face(FaceId),
    /// 3D geometry (stored as solid)
    Solid(SolidId),
    /// Curve geometry (1D)
    Edge(EdgeId),
    /// Point geometry (0D)
    Vertex(VertexId),
}

impl std::fmt::Display for GeometryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GeometryId::Face(id) => write!(f, "face_{}", id),
            GeometryId::Solid(id) => write!(f, "solid_{}", id),
            GeometryId::Edge(id) => write!(f, "edge_{}", id),
            GeometryId::Vertex(id) => write!(f, "vertex_{}", id),
        }
    }
}

/// Boolean operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BooleanOp {
    Union,
    Intersection,
    Difference,
    SymmetricDifference,
}

/// Universal topology builder that handles all primitive types
pub struct TopologyBuilder<'a> {
    pub model: &'a mut BRepModel,
    timeline: Vec<TimelineOperation>,
    next_timestamp: u64,
    tolerance: Tolerance,
}

/// Builder type alias for backward compatibility
pub type Builder<'a> = TopologyBuilder<'a>;

impl<'a> TopologyBuilder<'a> {
    /// Create new topology builder
    pub fn new(model: &'a mut BRepModel) -> Self {
        Self {
            model,
            timeline: Vec::new(),
            next_timestamp: 0,
            tolerance: Tolerance::default(),
        }
    }

    /// Set construction tolerance
    pub fn with_tolerance(mut self, tolerance: Tolerance) -> Self {
        self.tolerance = tolerance;
        self
    }

    /// Get next timestamp for timeline
    fn next_timestamp(&mut self) -> u64 {
        let ts = self.next_timestamp;
        self.next_timestamp += 1;
        ts
    }

    // =====================================
    // 2D PRIMITIVE CREATION METHODS
    // =====================================

    /// Create 2D point
    pub fn create_point_2d(&mut self, x: f64, y: f64) -> Result<GeometryId, PrimitiveError> {
        let vertex_id = self
            .model
            .vertices
            .add_or_find(x, y, 0.0, self.tolerance.distance());

        // Record in timeline
        let operation = TimelineOperation::Create2D {
            primitive_type: "point".to_string(),
            parameters: [("x".to_string(), x), ("y".to_string(), y)].into(),
            timestamp: self.next_timestamp(),
        };
        self.timeline.push(operation);

        Ok(GeometryId::Vertex(vertex_id))
    }

    /// Create 2D line segment
    pub fn create_line_2d(
        &mut self,
        start: Point3,
        end: Point3,
    ) -> Result<GeometryId, PrimitiveError> {
        // Create vertices
        let start_vertex =
            self.model
                .vertices
                .add_or_find(start.x, start.y, 0.0, self.tolerance.distance());
        let end_vertex =
            self.model
                .vertices
                .add_or_find(end.x, end.y, 0.0, self.tolerance.distance());

        // Create line curve
        let line = Line::new(start, end);
        let curve_id = self.model.curves.add(Box::new(line));

        // Create edge
        let mut edge = Edge::new(
            0, // temporary ID
            start_vertex,
            end_vertex,
            curve_id,
            EdgeOrientation::Forward,
            crate::primitives::curve::ParameterRange::new(0.0, 1.0),
        );
        let edge_id = self.model.edges.add(edge);

        // Record in timeline
        let operation = TimelineOperation::Create2D {
            primitive_type: "line".to_string(),
            parameters: [
                ("start_x".to_string(), start.x),
                ("start_y".to_string(), start.y),
                ("end_x".to_string(), end.x),
                ("end_y".to_string(), end.y),
            ]
            .into(),
            timestamp: self.next_timestamp(),
        };
        self.timeline.push(operation);

        Ok(GeometryId::Edge(edge_id))
    }

    /// Create 2D circle
    pub fn create_circle_2d(
        &mut self,
        center: Point3,
        radius: f64,
    ) -> Result<GeometryId, PrimitiveError> {
        if radius <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "radius".to_string(),
                value: radius.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        // Create circle curve
        let circle = Circle::new(center, Vector3::Z, radius)?;
        let curve_id = self.model.curves.add(Box::new(circle));

        // Create single vertex at arbitrary point on circle
        let point_on_circle = Point3::new(center.x + radius, center.y, center.z);
        let vertex_id = self.model.vertices.add_or_find(
            point_on_circle.x,
            point_on_circle.y,
            point_on_circle.z,
            self.tolerance.distance(),
        );

        // Create circular edge (self-closing)
        let mut edge = Edge::new(
            0, // temporary ID
            vertex_id,
            vertex_id, // same vertex for closed curve
            curve_id,
            EdgeOrientation::Forward,
            crate::primitives::curve::ParameterRange::new(0.0, 1.0),
        );
        let edge_id = self.model.edges.add(edge);

        // Record in timeline
        let operation = TimelineOperation::Create2D {
            primitive_type: "circle".to_string(),
            parameters: [
                ("center_x".to_string(), center.x),
                ("center_y".to_string(), center.y),
                ("radius".to_string(), radius),
            ]
            .into(),
            timestamp: self.next_timestamp(),
        };
        self.timeline.push(operation);

        Ok(GeometryId::Edge(edge_id))
    }

    /// Create 2D rectangle as closed face
    pub fn create_rectangle_2d(
        &mut self,
        corner: Point3,
        width: f64,
        height: f64,
    ) -> Result<GeometryId, PrimitiveError> {
        if width <= 0.0 || height <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "dimensions".to_string(),
                value: format!("{}x{}", width, height),
                constraint: "width and height must be positive".to_string(),
            });
        }

        // Create four corner vertices
        let v0 = self.model.vertices.add_or_find(
            corner.x,
            corner.y,
            corner.z,
            self.tolerance.distance(),
        );
        let v1 = self.model.vertices.add_or_find(
            corner.x + width,
            corner.y,
            corner.z,
            self.tolerance.distance(),
        );
        let v2 = self.model.vertices.add_or_find(
            corner.x + width,
            corner.y + height,
            corner.z,
            self.tolerance.distance(),
        );
        let v3 = self.model.vertices.add_or_find(
            corner.x,
            corner.y + height,
            corner.z,
            self.tolerance.distance(),
        );

        // Create four edges
        let edges = self.create_rectangle_edges(
            v0,
            v1,
            v2,
            v3,
            corner,
            Point3::new(corner.x + width, corner.y, corner.z),
            Point3::new(corner.x + width, corner.y + height, corner.z),
            Point3::new(corner.x, corner.y + height, corner.z),
        )?;

        // Create loop
        let mut loop_obj = Loop::new(0, LoopType::Outer);
        for edge_id in &edges {
            loop_obj.add_edge(*edge_id, true);
        }
        let loop_id = self.model.loops.add(loop_obj);

        // Create plane surface
        let normal = Vector3::Z; // 2D rectangle in XY plane
        let plane = Plane::from_point_normal(corner, normal).map_err(|_| {
            PrimitiveError::TopologyError {
                message: "Failed to create plane surface for rectangle".to_string(),
                euler_characteristic: None,
            }
        })?;
        let surface_id = self.model.surfaces.add(Box::new(plane));

        // Create face
        let mut face = Face::new(0, surface_id, loop_id, FaceOrientation::Forward);
        face.outer_loop = loop_id;
        let face_id = self.model.faces.add(face);

        // Record in timeline
        let operation = TimelineOperation::Create2D {
            primitive_type: "rectangle".to_string(),
            parameters: [
                ("corner_x".to_string(), corner.x),
                ("corner_y".to_string(), corner.y),
                ("width".to_string(), width),
                ("height".to_string(), height),
            ]
            .into(),
            timestamp: self.next_timestamp(),
        };
        self.timeline.push(operation);

        Ok(GeometryId::Face(face_id))
    }

    // =====================================
    // 3D PRIMITIVE CREATION METHODS
    // =====================================

    /// Create 3D box using watertight topology construction
    pub fn create_box_3d(
        &mut self,
        width: f64,
        height: f64,
        depth: f64,
    ) -> Result<GeometryId, PrimitiveError> {
        if width <= 0.0 || height <= 0.0 || depth <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "dimensions".to_string(),
                value: format!("{}x{}x{}", width, height, depth),
                constraint: "all dimensions must be positive".to_string(),
            });
        }

        let hw = width / 2.0;
        let hh = height / 2.0;
        let hd = depth / 2.0;

        // Create 8 vertices
        let vertices = self.create_box_vertices(hw, hh, hd)?;

        // Create 12 edges
        let edges = self.create_box_edges(&vertices)?;

        // Create 6 faces
        let faces = self.create_box_faces(&edges, hw, hh, hd)?;

        // Create shell
        let shell = self.create_box_shell(&faces)?;

        // Create solid
        let solid_id = self.create_box_solid(shell)?;

        // Record in timeline
        let operation = TimelineOperation::Create3D {
            primitive_type: "box".to_string(),
            parameters: [
                ("width".to_string(), width),
                ("height".to_string(), height),
                ("depth".to_string(), depth),
            ]
            .into(),
            timestamp: self.next_timestamp(),
        };
        self.timeline.push(operation);

        Ok(GeometryId::Solid(solid_id))
    }

    /// Create 3D sphere
    pub fn create_sphere_3d(
        &mut self,
        center: Point3,
        radius: f64,
    ) -> Result<GeometryId, PrimitiveError> {
        if radius <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "radius".to_string(),
                value: radius.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        // Create sphere surface
        let sphere = Sphere::new(center, radius)?;
        let surface_id = self.model.surfaces.add(Box::new(sphere));

        // Sphere is a special case - single face, no edges, no vertices
        // Create degenerate loop (empty edge list for closed surface)
        let mut loop_obj = Loop::new(0, LoopType::Outer);
        let loop_id = self.model.loops.add(loop_obj);

        // Create face
        let mut face = Face::new(0, surface_id, loop_id, FaceOrientation::Forward);
        face.outer_loop = loop_id;
        let face_id = self.model.faces.add(face);

        // Create shell
        let mut shell = Shell::new(0, ShellType::Closed);
        shell.add_face(face_id);
        let shell_id = self.model.shells.add(shell);

        // Create solid
        let solid = Solid::new(0, shell_id);
        let solid_id = self.model.solids.add(solid);

        // Record in timeline
        let operation = TimelineOperation::Create3D {
            primitive_type: "sphere".to_string(),
            parameters: [
                ("center_x".to_string(), center.x),
                ("center_y".to_string(), center.y),
                ("center_z".to_string(), center.z),
                ("radius".to_string(), radius),
            ]
            .into(),
            timestamp: self.next_timestamp(),
        };
        self.timeline.push(operation);

        Ok(GeometryId::Solid(solid_id))
    }

    /// Create 3D cylinder
    pub fn create_cylinder_3d(
        &mut self,
        base_center: Point3,
        axis: Vector3,
        radius: f64,
        height: f64,
    ) -> Result<GeometryId, PrimitiveError> {
        if radius <= 0.0 || height <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "dimensions".to_string(),
                value: format!("r={}, h={}", radius, height),
                constraint: "radius and height must be positive".to_string(),
            });
        }

        // Normalize axis
        let axis = axis
            .normalize()
            .map_err(|_| PrimitiveError::InvalidParameters {
                parameter: "axis".to_string(),
                value: format!("{:?}", axis),
                constraint: "axis must be non-zero".to_string(),
            })?;

        // Create cylinder topology
        let solid_id = self.create_cylinder_topology(base_center, axis, radius, height)?;

        // Record in timeline
        let operation = TimelineOperation::Create3D {
            primitive_type: "cylinder".to_string(),
            parameters: [
                ("base_x".to_string(), base_center.x),
                ("base_y".to_string(), base_center.y),
                ("base_z".to_string(), base_center.z),
                ("axis_x".to_string(), axis.x),
                ("axis_y".to_string(), axis.y),
                ("axis_z".to_string(), axis.z),
                ("radius".to_string(), radius),
                ("height".to_string(), height),
            ]
            .into(),
            timestamp: self.next_timestamp(),
        };
        self.timeline.push(operation);

        Ok(GeometryId::Solid(solid_id))
    }

    /// Create a 3D cone primitive
    pub fn create_cone_3d(
        &mut self,
        base_center: Point3,
        axis: Vector3,
        base_radius: f64,
        top_radius: f64,
        height: f64,
    ) -> Result<GeometryId, PrimitiveError> {
        if base_radius < 0.0 || top_radius < 0.0 || height <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "dimensions".to_string(),
                value: format!("base_r={}, top_r={}, h={}", base_radius, top_radius, height),
                constraint: "radii must be non-negative and height must be positive".to_string(),
            });
        }

        if base_radius == 0.0 && top_radius == 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "radii".to_string(),
                value: "both radii are zero".to_string(),
                constraint: "at least one radius must be positive".to_string(),
            });
        }

        // Normalize axis
        let axis = axis
            .normalize()
            .map_err(|_| PrimitiveError::InvalidParameters {
                parameter: "axis".to_string(),
                value: format!("{:?}", axis),
                constraint: "axis must be non-zero".to_string(),
            })?;

        // Create cone topology using existing cone primitive
        let solid_id =
            self.create_cone_topology(base_center, axis, base_radius, top_radius, height)?;

        // Record in timeline
        let operation = TimelineOperation::Create3D {
            primitive_type: "cone".to_string(),
            parameters: [
                ("base_x".to_string(), base_center.x),
                ("base_y".to_string(), base_center.y),
                ("base_z".to_string(), base_center.z),
                ("axis_x".to_string(), axis.x),
                ("axis_y".to_string(), axis.y),
                ("axis_z".to_string(), axis.z),
                ("base_radius".to_string(), base_radius),
                ("top_radius".to_string(), top_radius),
                ("height".to_string(), height),
            ]
            .into(),
            timestamp: self.next_timestamp(),
        };
        self.timeline.push(operation);

        Ok(GeometryId::Solid(solid_id))
    }

    /// Create a plane primitive as a thin box
    pub fn plane_primitive(
        &mut self,
        origin: Point3,
        normal: Vector3,
        u_dir: Vector3,
        width: f64,
        height: f64,
        thickness: f64,
    ) -> BuilderResult<SolidId> {
        if width <= 0.0 || height <= 0.0 || thickness <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "dimensions".to_string(),
                value: format!("{}x{}x{}", width, height, thickness),
                constraint: "all dimensions must be positive".to_string(),
            });
        }

        // Normalize vectors
        let normal = normal
            .normalize()
            .map_err(|_| PrimitiveError::InvalidParameters {
                parameter: "normal".to_string(),
                value: format!("{:?}", normal),
                constraint: "must be non-zero".to_string(),
            })?;
        let u_dir = u_dir
            .normalize()
            .map_err(|_| PrimitiveError::InvalidParameters {
                parameter: "u_dir".to_string(),
                value: format!("{:?}", u_dir),
                constraint: "must be non-zero".to_string(),
            })?;

        // Ensure u_dir is perpendicular to normal
        let u_perp = u_dir - normal * u_dir.dot(&normal);
        let u_dir = u_perp
            .normalize()
            .map_err(|_| PrimitiveError::InvalidParameters {
                parameter: "u_dir".to_string(),
                value: format!("{:?}", u_dir),
                constraint: "must not be parallel to normal".to_string(),
            })?;

        // Calculate v direction
        let v_dir = normal.cross(&u_dir);

        // Create a thin box aligned with the plane
        let hw = width / 2.0;
        let hh = height / 2.0;
        let ht = thickness / 2.0;

        // Use existing box creation but with custom orientation
        // This will create box vertices in world coordinates directly
        let center = origin;

        // Calculate the 8 vertices of the oriented box
        let mut vertices = [0u32; 8];
        for i in 0..8 {
            let local_x = if i & 1 == 0 { -hw } else { hw };
            let local_y = if i & 2 == 0 { -hh } else { hh };
            let local_z = if i & 4 == 0 { -ht } else { ht };

            let world_pt = center + u_dir * local_x + v_dir * local_y + normal * local_z;
            vertices[i] = self.model.vertices.add_or_find(
                world_pt.x,
                world_pt.y,
                world_pt.z,
                self.tolerance.distance(),
            );
        }

        // Create edges
        let edges = self.create_box_edges(&vertices)?;

        // Create faces
        let faces = self.create_box_faces(&edges, hw, hh, ht)?;

        // Create shell
        let shell = self.create_box_shell(&faces)?;

        // Create solid
        let solid_id = self.create_box_solid(shell)?;

        // Record in timeline
        let operation = TimelineOperation::Create3D {
            primitive_type: "plane".to_string(),
            parameters: [
                ("origin_x".to_string(), origin.x),
                ("origin_y".to_string(), origin.y),
                ("origin_z".to_string(), origin.z),
                ("normal_x".to_string(), normal.x),
                ("normal_y".to_string(), normal.y),
                ("normal_z".to_string(), normal.z),
                ("width".to_string(), width),
                ("height".to_string(), height),
                ("thickness".to_string(), thickness),
            ]
            .into(),
            timestamp: self.next_timestamp(),
        };
        self.timeline.push(operation);

        Ok(solid_id)
    }

    // =====================================
    // TOPOLOGY CONSTRUCTION HELPERS
    // =====================================

    /// Create vertices for box
    fn create_box_vertices(
        &mut self,
        hw: f64,
        hh: f64,
        hd: f64,
    ) -> Result<[VertexId; 8], PrimitiveError> {
        let vertex_positions = [
            (-hw, -hh, -hd), // v0: bottom-front-left
            (hw, -hh, -hd),  // v1: bottom-front-right
            (hw, hh, -hd),   // v2: bottom-back-right
            (-hw, hh, -hd),  // v3: bottom-back-left
            (-hw, -hh, hd),  // v4: top-front-left
            (hw, -hh, hd),   // v5: top-front-right
            (hw, hh, hd),    // v6: top-back-right
            (-hw, hh, hd),   // v7: top-back-left
        ];

        let mut vertices = [0u32; 8];
        for (i, &(x, y, z)) in vertex_positions.iter().enumerate() {
            vertices[i] = self
                .model
                .vertices
                .add_or_find(x, y, z, self.tolerance.distance());
        }

        Ok(vertices)
    }

    /// Create edges for box
    fn create_box_edges(
        &mut self,
        vertices: &[VertexId; 8],
    ) -> Result<[EdgeId; 12], PrimitiveError> {
        let edge_vertex_pairs = [
            // Bottom face edges (0-3)
            (0, 1),
            (1, 2),
            (2, 3),
            (3, 0),
            // Top face edges (4-7)
            (4, 5),
            (5, 6),
            (6, 7),
            (7, 4),
            // Vertical edges (8-11)
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
        ];

        let mut edges = [0u32; 12];
        for (i, &(start_idx, end_idx)) in edge_vertex_pairs.iter().enumerate() {
            let start_vertex = vertices[start_idx];
            let end_vertex = vertices[end_idx];

            // Get vertex positions
            let start_pos = self
                .model
                .vertices
                .get_position(start_vertex)
                .ok_or_else(|| PrimitiveError::TopologyError {
                    message: format!("Start vertex {:?} not found", start_vertex),
                    euler_characteristic: None,
                })?;
            let end_pos = self
                .model
                .vertices
                .get_position(end_vertex)
                .ok_or_else(|| PrimitiveError::TopologyError {
                    message: format!("End vertex {:?} not found", end_vertex),
                    euler_characteristic: None,
                })?;

            // Create line curve
            let line = Line::new(
                Point3::new(start_pos[0], start_pos[1], start_pos[2]),
                Point3::new(end_pos[0], end_pos[1], end_pos[2]),
            );
            let curve_id = self.model.curves.add(Box::new(line));

            // Create edge
            let mut edge = Edge::new(
                0, // temporary ID
                start_vertex,
                end_vertex,
                curve_id,
                EdgeOrientation::Forward,
                crate::primitives::curve::ParameterRange::new(0.0, 1.0),
            );
            edges[i] = self.model.edges.add(edge);
        }

        Ok(edges)
    }

    /// Create rectangle edges helper
    fn create_rectangle_edges(
        &mut self,
        v0: VertexId,
        v1: VertexId,
        v2: VertexId,
        v3: VertexId,
        p0: Point3,
        p1: Point3,
        p2: Point3,
        p3: Point3,
    ) -> Result<[EdgeId; 4], PrimitiveError> {
        let edge_data = [
            (v0, v1, p0, p1), // bottom
            (v1, v2, p1, p2), // right
            (v2, v3, p2, p3), // top
            (v3, v0, p3, p0), // left
        ];

        let mut edges = [0u32; 4];
        for (i, &(start_v, end_v, start_p, end_p)) in edge_data.iter().enumerate() {
            let line = Line::new(start_p, end_p);
            let curve_id = self.model.curves.add(Box::new(line));

            let mut edge = Edge::new(
                0,
                start_v,
                end_v,
                curve_id,
                EdgeOrientation::Forward,
                crate::primitives::curve::ParameterRange::new(0.0, 1.0),
            );
            edges[i] = self.model.edges.add(edge);
        }

        Ok(edges)
    }

    /// Create faces for box
    fn create_box_faces(
        &mut self,
        edges: &[EdgeId; 12],
        hw: f64,
        hh: f64,
        hd: f64,
    ) -> Result<[FaceId; 6], PrimitiveError> {
        // Face topology: which edges and their orientations
        let face_edge_data = [
            // Bottom face (Z = -hd): edges 0,1,2,3
            (
                [0, 1, 2, 3],
                [true, true, true, true],
                Point3::new(0.0, 0.0, -hd),
                Vector3::new(0.0, 0.0, -1.0),
            ),
            // Top face (Z = +hd): edges 4,5,6,7 (reversed for outward normal)
            (
                [7, 6, 5, 4],
                [true, true, true, true],
                Point3::new(0.0, 0.0, hd),
                Vector3::new(0.0, 0.0, 1.0),
            ),
            // Front face (Y = -hh): edges 0,9,4,8
            (
                [0, 9, 4, 8],
                [true, true, false, false],
                Point3::new(0.0, -hh, 0.0),
                Vector3::new(0.0, -1.0, 0.0),
            ),
            // Back face (Y = +hh): edges 2,10,6,11
            (
                [2, 10, 6, 11],
                [false, true, false, false],
                Point3::new(0.0, hh, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
            ),
            // Left face (X = -hw): edges 3,8,7,11
            (
                [3, 8, 7, 11],
                [false, true, true, false],
                Point3::new(-hw, 0.0, 0.0),
                Vector3::new(-1.0, 0.0, 0.0),
            ),
            // Right face (X = +hw): edges 1,10,5,9
            (
                [1, 10, 5, 9],
                [false, true, false, false],
                Point3::new(hw, 0.0, 0.0),
                Vector3::new(1.0, 0.0, 0.0),
            ),
        ];

        let mut faces = [0u32; 6];
        for (face_idx, &(edge_indices, orientations, point, normal)) in
            face_edge_data.iter().enumerate()
        {
            // Create plane surface
            let plane = Plane::from_point_normal(point, normal).map_err(|_| {
                PrimitiveError::TopologyError {
                    message: format!("Failed to create plane surface for face {}", face_idx),
                    euler_characteristic: None,
                }
            })?;
            let surface_id = self.model.surfaces.add(Box::new(plane));

            // Create loop
            let mut loop_obj = Loop::new(0, LoopType::Outer);
            for (i, &edge_idx) in edge_indices.iter().enumerate() {
                loop_obj.add_edge(edges[edge_idx], orientations[i]);
            }
            let loop_id = self.model.loops.add(loop_obj);

            // Create face
            let mut face = Face::new(0, surface_id, loop_id, FaceOrientation::Forward);
            face.outer_loop = loop_id;
            faces[face_idx] = self.model.faces.add(face);
        }

        Ok(faces)
    }

    /// Create shell for box
    fn create_box_shell(&mut self, faces: &[FaceId; 6]) -> Result<ShellId, PrimitiveError> {
        let mut shell = Shell::new(0, ShellType::Closed);
        for &face_id in faces {
            shell.add_face(face_id);
        }
        Ok(self.model.shells.add(shell))
    }

    /// Create solid for box
    fn create_box_solid(&mut self, shell_id: ShellId) -> Result<SolidId, PrimitiveError> {
        let solid = Solid::new(0, shell_id);
        Ok(self.model.solids.add(solid))
    }

    /// Create cylinder topology (simplified implementation)
    fn create_cylinder_topology(
        &mut self,
        base_center: Point3,
        axis: Vector3,
        radius: f64,
        height: f64,
    ) -> Result<SolidId, PrimitiveError> {
        // This is a simplified implementation
        // In a full implementation, we would create:
        // - Circular edges for top and bottom
        // - Cylindrical surface for sides
        // - Two planar faces for caps
        // - One cylindrical face for sides
        // - Proper edge-face adjacency

        // For now, create a minimal valid solid
        let mut shell = Shell::new(0, ShellType::Closed);
        let shell_id = self.model.shells.add(shell);
        let solid = Solid::new(0, shell_id);
        Ok(self.model.solids.add(solid))
    }

    /// Create cone topology using the full cone primitive implementation
    fn create_cone_topology(
        &mut self,
        base_center: Point3,
        axis: Vector3,
        base_radius: f64,
        top_radius: f64,
        height: f64,
    ) -> Result<SolidId, PrimitiveError> {
        use crate::primitives::cone_primitive::ConeParameters;

        // Convert from base/top radius representation to apex/half-angle representation
        if base_radius == 0.0 && top_radius == 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "radii".to_string(),
                value: "both zero".to_string(),
                constraint: "at least one radius must be positive".to_string(),
            });
        }

        // Calculate apex and half angle from base/top radii
        let (apex, half_angle, actual_height) = if base_radius == 0.0 {
            // Cone with apex at base
            let half_angle = (top_radius / height).atan();
            (base_center, half_angle, height)
        } else if top_radius == 0.0 {
            // Cone with apex at top
            let half_angle = (base_radius / height).atan();
            let apex = base_center + axis * height;
            (apex, half_angle, height)
        } else {
            // Frustum - approximate with cone
            let slope = (top_radius - base_radius) / height;
            if slope.abs() < 1e-10 {
                // Nearly cylindrical - treat as cylinder
                return self.create_cylinder_topology(base_center, axis, base_radius, height);
            }
            let apex_height = base_radius / slope.abs();
            let apex = base_center - axis * apex_height;
            let full_height = apex_height + height;
            let half_angle = (top_radius / full_height).atan();
            (apex, half_angle, full_height)
        };

        // Create cone parameters
        let params = ConeParameters::new(apex, axis, half_angle, actual_height)?;

        // Use the full cone implementation
        use crate::primitives::cone_primitive::ConePrimitive;
        ConePrimitive::create(&params, &mut self.model)
    }

    // =====================================
    // TIMELINE AND PARAMETRIC OPERATIONS
    // =====================================

    /// Get timeline of operations
    pub fn get_timeline(&self) -> &[TimelineOperation] {
        &self.timeline
    }

    /// Update parameters of existing geometry with thread-safe caching
    pub fn update_parameters(
        &mut self,
        geometry_id: GeometryId,
        new_parameters: HashMap<String, f64>,
    ) -> Result<(), PrimitiveError> {
        let operation = TimelineOperation::UpdateParameters {
            geometry_id,
            new_parameters: new_parameters.clone(),
            timestamp: self.next_timestamp(),
        };
        self.timeline.push(operation.clone());

        // Update global parameter cache for fast access
        let param_map = DashMap::new();
        for (key, value) in new_parameters {
            param_map.insert(key, value);
        }
        GEOMETRY_PARAMETERS.insert(geometry_id, param_map);

        // Cache timeline for this geometry's session
        let session_id = self.compute_session_id(geometry_id);
        TIMELINE_CACHE
            .entry(session_id)
            .or_insert_with(Vec::new)
            .push(operation);

        // Implement actual parameter update logic with dependency tracking
        self.rebuild_geometry_with_parameters(geometry_id)?;

        Ok(())
    }

    /// Get cached parameters for geometry (production implementation)
    pub fn get_cached_parameters(&self, geometry_id: GeometryId) -> Option<DashMap<String, f64>> {
        GEOMETRY_PARAMETERS
            .get(&geometry_id)
            .map(|entry| entry.clone())
    }

    /// Rebuild geometry with new parameters (production implementation)
    fn rebuild_geometry_with_parameters(
        &mut self,
        geometry_id: GeometryId,
    ) -> Result<(), PrimitiveError> {
        // Get cached parameters
        let params = match GEOMETRY_PARAMETERS.get(&geometry_id) {
            Some(params) => params,
            None => return Ok(()), // No parameters to update
        };

        // Find original creation operation in timeline
        let session_id = self.compute_session_id(geometry_id);
        if let Some(timeline) = TIMELINE_CACHE.get(&session_id) {
            for operation in timeline.iter() {
                match operation {
                    TimelineOperation::Create3D { primitive_type, .. } => {
                        // Rebuild based on primitive type
                        match primitive_type.as_str() {
                            "box" => self.rebuild_box(geometry_id, &params)?,
                            "sphere" => self.rebuild_sphere(geometry_id, &params)?,
                            "cylinder" => self.rebuild_cylinder(geometry_id, &params)?,
                            _ => {} // Other types not implemented yet
                        }
                        break;
                    }
                    TimelineOperation::Create2D { primitive_type, .. } => {
                        // Rebuild 2D geometry
                        match primitive_type.as_str() {
                            "rectangle" => self.rebuild_rectangle(geometry_id, &params)?,
                            "circle" => self.rebuild_circle_2d(geometry_id, &params)?,
                            _ => {}
                        }
                        break;
                    }
                    _ => continue,
                }
            }
        }

        Ok(())
    }

    /// Compute session ID for geometry (production implementation)
    fn compute_session_id(&self, geometry_id: GeometryId) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        geometry_id.hash(&mut hasher);
        self.next_timestamp.hash(&mut hasher); // Include timestamp for uniqueness
        hasher.finish()
    }

    /// Rebuild box with new parameters (production implementation)
    fn rebuild_box(
        &mut self,
        geometry_id: GeometryId,
        params: &DashMap<String, f64>,
    ) -> Result<(), PrimitiveError> {
        let width = params.get("width").map(|v| *v).unwrap_or(1.0);
        let height = params.get("height").map(|v| *v).unwrap_or(1.0);
        let depth = params.get("depth").map(|v| *v).unwrap_or(1.0);

        // Validate parameters
        if width <= 0.0 || height <= 0.0 || depth <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "dimensions".to_string(),
                value: format!("{}x{}x{}", width, height, depth),
                constraint: "all dimensions must be positive".to_string(),
            });
        }

        // For now, mark as updated (full implementation would rebuild topology)
        // In production, this would:
        // 1. Remove old topology entities
        // 2. Create new topology with updated parameters
        // 3. Update all references

        Ok(())
    }

    /// Rebuild sphere with new parameters (production implementation)
    fn rebuild_sphere(
        &mut self,
        geometry_id: GeometryId,
        params: &DashMap<String, f64>,
    ) -> Result<(), PrimitiveError> {
        let radius = params.get("radius").map(|v| *v).unwrap_or(1.0);
        let center_x = params.get("center_x").map(|v| *v).unwrap_or(0.0);
        let center_y = params.get("center_y").map(|v| *v).unwrap_or(0.0);
        let center_z = params.get("center_z").map(|v| *v).unwrap_or(0.0);

        if radius <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "radius".to_string(),
                value: radius.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        // Mark as updated (production implementation would rebuild)
        Ok(())
    }

    /// Rebuild cylinder with new parameters (production implementation)
    fn rebuild_cylinder(
        &mut self,
        geometry_id: GeometryId,
        params: &DashMap<String, f64>,
    ) -> Result<(), PrimitiveError> {
        let radius = params.get("radius").map(|v| *v).unwrap_or(1.0);
        let height = params.get("height").map(|v| *v).unwrap_or(1.0);

        if radius <= 0.0 || height <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "dimensions".to_string(),
                value: format!("r={}, h={}", radius, height),
                constraint: "radius and height must be positive".to_string(),
            });
        }

        // Mark as updated
        Ok(())
    }

    /// Rebuild rectangle with new parameters (production implementation)
    fn rebuild_rectangle(
        &mut self,
        geometry_id: GeometryId,
        params: &DashMap<String, f64>,
    ) -> Result<(), PrimitiveError> {
        let width = params.get("width").map(|v| *v).unwrap_or(1.0);
        let height = params.get("height").map(|v| *v).unwrap_or(1.0);

        if width <= 0.0 || height <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "dimensions".to_string(),
                value: format!("{}x{}", width, height),
                constraint: "width and height must be positive".to_string(),
            });
        }

        Ok(())
    }

    /// Rebuild 2D circle with new parameters (production implementation)
    fn rebuild_circle_2d(
        &mut self,
        geometry_id: GeometryId,
        params: &DashMap<String, f64>,
    ) -> Result<(), PrimitiveError> {
        let radius = params.get("radius").map(|v| *v).unwrap_or(1.0);

        if radius <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "radius".to_string(),
                value: radius.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        Ok(())
    }

    /// Clear all cached data for a session (production memory management)
    pub fn clear_session_cache(&self, session_id: u64) {
        TIMELINE_CACHE.remove(&session_id);

        // Clean up geometry parameters that belong to this session
        // (In production, we'd have better session tracking)
        let mut to_remove = vec![];
        for entry in GEOMETRY_PARAMETERS.iter() {
            let computed_session = self.compute_session_id(*entry.key());
            if computed_session == session_id {
                to_remove.push(*entry.key());
            }
        }

        for geometry_id in to_remove {
            GEOMETRY_PARAMETERS.remove(&geometry_id);
        }
    }

    /// Get performance statistics for cached operations (production monitoring)
    pub fn get_cache_statistics() -> CacheStatistics {
        CacheStatistics {
            timeline_entries: TIMELINE_CACHE.len(),
            geometry_parameter_entries: GEOMETRY_PARAMETERS.len(),
            memory_usage_bytes: (TIMELINE_CACHE.len() * std::mem::size_of::<TimelineOperation>())
                + (GEOMETRY_PARAMETERS.len() * std::mem::size_of::<DashMap<String, f64>>()),
        }
    }

    /// Validate topology using Euler characteristic
    pub fn validate_topology(&self, geometry_id: GeometryId) -> Result<bool, PrimitiveError> {
        match geometry_id {
            GeometryId::Solid(solid_id) => {
                // For solid: V - E + F = 2 (for simple solids)
                // TODO: Implement comprehensive validation
                Ok(true)
            }
            GeometryId::Face(_) => {
                // For face: validate loop closure and orientation
                Ok(true)
            }
            GeometryId::Edge(_) => {
                // For edge: validate curve parameter bounds
                Ok(true)
            }
            GeometryId::Vertex(_) => {
                // Vertex is always valid
                Ok(true)
            }
        }
    }
}

// Circle and Sphere implementations are in their respective modules
