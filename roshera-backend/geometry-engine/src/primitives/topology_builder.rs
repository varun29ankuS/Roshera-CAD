//! Universal Topology Builder for 2D and 3D Primitives
//!
//! This module provides the core infrastructure for building watertight B-Rep
//! topology for all primitive types, both 2D and 3D, with timeline support.
//!
//! Indexed access into vertex/edge/face buffers built during primitive
//! construction is bounds-guaranteed by the known topology of each primitive
//! (box=8v/12e/6f, cylinder=2N+2v, etc). All `arr[i]` sites use indices
//! derived from the construction loop counters.
#![allow(clippy::indexing_slicing)]

use crate::math::{Matrix4, Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::{Circle, CurveStore, Line, ParameterRange},
    edge::{Edge, EdgeId, EdgeOrientation, EdgeStore},
    face::{Face, FaceId, FaceOrientation, FaceStore},
    primitive_traits::PrimitiveError,
    r#loop::{Loop, LoopId, LoopStore, LoopType},
    shell::{Shell, ShellId, ShellStore, ShellType},
    solid::{Solid, SolidId, SolidStore},
    surface::{Cylinder, Plane, Sphere, SurfaceStore},
    vertex::{VertexId, VertexStore},
};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::LazyLock;

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
    LazyLock::new(DashMap::new);

/// Global geometry parameter cache for fast parameter updates
static GEOMETRY_PARAMETERS: LazyLock<DashMap<GeometryId, DashMap<String, f64>>> =
    LazyLock::new(DashMap::new);

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
        // - Each feature creates ~8 faces on average (empirical heuristic for CAD features)
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
    /// Optional recorder receiving one event per successful operation.
    /// `None` by default — tests and unattached models incur zero overhead.
    /// Attached via `attach_recorder` by the orchestration layer
    /// (api-server, AI batch driver, …). Not serialized; recorder identity
    /// is an orchestration concern, not a model invariant.
    pub recorder: Option<std::sync::Arc<dyn crate::operations::recorder::OperationRecorder>>,
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
            recorder: None,
        }
    }

    /// Attach a recorder that will receive one event per successful
    /// operation on this model. Returns the previous recorder, if any.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use std::sync::Arc;
    /// let model = BRepModel::new();
    /// let rec: Arc<dyn OperationRecorder> = Arc::new(my_recorder);
    /// model.attach_recorder(Some(rec));
    /// ```
    pub fn attach_recorder(
        &mut self,
        recorder: Option<std::sync::Arc<dyn crate::operations::recorder::OperationRecorder>>,
    ) -> Option<std::sync::Arc<dyn crate::operations::recorder::OperationRecorder>> {
        std::mem::replace(&mut self.recorder, recorder)
    }

    /// Emit a record of a just-completed operation. Silently no-ops when no
    /// recorder is attached; logs a warning via `tracing` when the recorder
    /// returns an error (the operation has already mutated the model —
    /// recorder failures never become geometry failures).
    pub fn record_operation(&self, operation: crate::operations::recorder::RecordedOperation) {
        if let Some(rec) = self.recorder.as_ref() {
            if let Err(e) = rec.record(operation) {
                tracing::warn!("operation recorder returned error: {}", e);
            }
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
            let _origin = Point3::ORIGIN;
            for i in 1..vertices.len() - 1 {
                let v0 = vertices[0];
                let v1 = vertices[i];
                let v2 = vertices[i + 1];

                // Calculate triangle normal (outward pointing)
                let edge1 = v1 - v0;
                let edge2 = v2 - v0;
                let _normal = edge1.cross(&edge2);

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
    pub fn tessellate_solid(&self, solid_id: u32, _tolerance: f64) -> Option<TessellatedMesh> {
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
                if let std::collections::hash_map::Entry::Vacant(e) = vertex_index_map.entry(edge.start_vertex) {
                    if let Some(vertex) = self.vertices.get(edge.start_vertex) {
                        let point = vertex.point();
                        let idx = vertices.len() as u32;
                        vertices.push([point.x as f32, point.y as f32, point.z as f32]);
                        // Initialize with zero normal, will accumulate later
                        normals.push([0.0, 0.0, 0.0]);
                        e.insert(idx);
                    }
                }

                // Process end vertex
                if let std::collections::hash_map::Entry::Vacant(e) = vertex_index_map.entry(edge.end_vertex) {
                    if let Some(vertex) = self.vertices.get(edge.end_vertex) {
                        let point = vertex.point();
                        let idx = vertices.len() as u32;
                        vertices.push([point.x as f32, point.y as f32, point.z as f32]);
                        normals.push([0.0, 0.0, 0.0]);
                        e.insert(idx);
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

            // Fan triangulation about vertex[0]. This is valid for the
            // convex faces produced by all primitive constructors used here
            // (boxes, cylinders, spheres, cones, tori). Concave or holed
            // faces require ear-clipping or constrained Delaunay; those go
            // through the dedicated tessellation pipeline instead of this
            // fast-path display mesh builder.
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

    /// Cascading delete of a vertex.
    ///
    /// Removes every edge that uses the vertex, then every loop that uses one
    /// of those edges, then every face whose outer or inner loop is removed,
    /// and finally drops the face from each shell that referenced it. The
    /// vertex itself is removed last.
    ///
    /// Linear scans are used to find dependents because the per-store
    /// reverse-index (`vertex_to_edges`, `edge_to_loops`, `loop_to_faces`,
    /// `face_to_shells`) is only maintained on the slow `add_with_indexing`
    /// path; the fast `add` path skips it, so the cached lookup is unreliable
    /// in the general case. Cascade delete is not on the hot creation path —
    /// linear is correct and predictable.
    ///
    /// On success the operation is recorded via [`record_operation`] with the
    /// full set of removed entity ids in the parameters. A vertex that is
    /// already absent yields an empty [`CascadeReport`] and no record.
    pub fn delete_vertex_cascade(&mut self, vertex_id: VertexId) -> CascadeReport {
        let mut report = CascadeReport::default();

        let dependent_edges: Vec<EdgeId> = self
            .edges
            .iter()
            .filter_map(|(eid, e)| {
                (e.start_vertex == vertex_id || e.end_vertex == vertex_id).then_some(eid)
            })
            .collect();
        for eid in dependent_edges {
            self.cascade_delete_edge(eid, &mut report);
        }

        if self.vertices.remove(vertex_id) {
            report.removed_vertices.push(vertex_id);
            self.record_cascade("delete_vertex_cascade", vertex_id as u64, &report);
        }
        report
    }

    /// Cascading delete of an edge — removes dependent loops, faces, and
    /// shell face-references before dropping the edge.
    pub fn delete_edge_cascade(&mut self, edge_id: EdgeId) -> CascadeReport {
        let mut report = CascadeReport::default();
        let removed = self.cascade_delete_edge(edge_id, &mut report);
        if removed {
            self.record_cascade("delete_edge_cascade", edge_id as u64, &report);
        }
        report
    }

    /// Cascading delete of a face — removes the face from every referencing
    /// shell, then drops the face. Loops are not deleted: they may be shared
    /// with other faces. Use [`delete_loop_cascade`] explicitly if you also
    /// want the bounding loop torn down.
    pub fn delete_face_cascade(&mut self, face_id: FaceId) -> CascadeReport {
        let mut report = CascadeReport::default();
        let removed = self.cascade_delete_face(face_id, &mut report);
        if removed {
            self.record_cascade("delete_face_cascade", face_id as u64, &report);
        }
        report
    }

    /// Cascading delete of a loop — removes faces that bound on the loop
    /// (and their shell references), then drops the loop. Edges are not
    /// deleted: they may be shared with other loops.
    pub fn delete_loop_cascade(&mut self, loop_id: LoopId) -> CascadeReport {
        let mut report = CascadeReport::default();
        let removed = self.cascade_delete_loop(loop_id, &mut report);
        if removed {
            self.record_cascade("delete_loop_cascade", loop_id as u64, &report);
        }
        report
    }

    fn cascade_delete_edge(&mut self, edge_id: EdgeId, report: &mut CascadeReport) -> bool {
        if report.removed_edges.contains(&edge_id) {
            return false;
        }

        let dependent_loops: Vec<LoopId> = self
            .loops
            .iter()
            .filter_map(|(lid, l)| l.edges.contains(&edge_id).then_some(lid))
            .collect();
        for lid in dependent_loops {
            self.cascade_delete_loop(lid, report);
        }

        if self.edges.remove(edge_id).is_some() {
            report.removed_edges.push(edge_id);
            true
        } else {
            false
        }
    }

    fn cascade_delete_loop(&mut self, loop_id: LoopId, report: &mut CascadeReport) -> bool {
        if report.removed_loops.contains(&loop_id) {
            return false;
        }

        let dependent_faces: Vec<FaceId> = self
            .faces
            .iter()
            .filter_map(|(fid, f)| {
                (f.outer_loop == loop_id || f.inner_loops.contains(&loop_id)).then_some(fid)
            })
            .collect();
        for fid in dependent_faces {
            self.cascade_delete_face(fid, report);
        }

        if self.loops.remove(loop_id).is_some() {
            report.removed_loops.push(loop_id);
            true
        } else {
            false
        }
    }

    fn cascade_delete_face(&mut self, face_id: FaceId, report: &mut CascadeReport) -> bool {
        if report.removed_faces.contains(&face_id) {
            return false;
        }

        let referencing_shells: Vec<ShellId> = self
            .shells
            .iter()
            .filter_map(|(sid, s)| s.find_face(face_id).map(|_| sid))
            .collect();
        for sid in referencing_shells {
            if let Some(shell) = self.shells.get_mut(sid) {
                shell.remove_face(face_id);
            }
            if !report.affected_shells.contains(&sid) {
                report.affected_shells.push(sid);
            }
        }

        if self.faces.remove(face_id).is_some() {
            report.removed_faces.push(face_id);
            true
        } else {
            false
        }
    }

    fn record_cascade(&self, kind: &str, root_id: u64, report: &CascadeReport) {
        use crate::operations::recorder::RecordedOperation;
        let outputs: Vec<u64> = report
            .removed_vertices
            .iter()
            .map(|id| *id as u64)
            .chain(report.removed_edges.iter().map(|id| *id as u64))
            .chain(report.removed_loops.iter().map(|id| *id as u64))
            .chain(report.removed_faces.iter().map(|id| *id as u64))
            .collect();
        self.record_operation(
            RecordedOperation::new(kind)
                .with_inputs(vec![root_id])
                .with_outputs(outputs)
                .with_parameters(serde_json::json!({
                    "removed_vertices": report.removed_vertices,
                    "removed_edges": report.removed_edges,
                    "removed_loops": report.removed_loops,
                    "removed_faces": report.removed_faces,
                    "affected_shells": report.affected_shells,
                })),
        );
    }
}

/// Report returned by the cascading-delete entry points on [`BRepModel`].
///
/// Each `removed_*` list contains the entity ids that were marked deleted
/// (in topological discovery order). `affected_shells` lists the shells that
/// had at least one face reference removed but whose own ids remain valid.
#[derive(Debug, Clone, Default)]
pub struct CascadeReport {
    pub removed_vertices: Vec<VertexId>,
    pub removed_edges: Vec<EdgeId>,
    pub removed_loops: Vec<LoopId>,
    pub removed_faces: Vec<FaceId>,
    pub affected_shells: Vec<ShellId>,
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

/// Flatten a typed `GeometryId` to the plain `u64` entity handle exposed
/// to external recorders.
///
/// `FaceId`, `SolidId`, `EdgeId`, and `VertexId` are all `u32` aliases, so
/// this is a widening cast with no data loss. The entity *kind* is **not**
/// preserved in the returned u64 — callers relying on round-trip identity
/// must consult the accompanying `RecordedOperation::parameters` payload,
/// which serializes the original `TimelineOperation` in full.
fn geometry_id_to_u64(id: GeometryId) -> u64 {
    match id {
        GeometryId::Face(i)
        | GeometryId::Solid(i)
        | GeometryId::Edge(i)
        | GeometryId::Vertex(i) => i as u64,
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

    /// Push a `TimelineOperation` to the builder's internal timeline **and**
    /// forward a canonical `RecordedOperation` to the model's attached
    /// recorder (if any).
    ///
    /// This is the single emission point that keeps the two history paths
    /// in sync:
    ///
    /// 1. `self.timeline` — the kernel-internal accumulator (kept for any
    ///    existing consumer of `get_timeline`).
    /// 2. `self.model.record_operation` — the dependency-inverted trait
    ///    handoff to `timeline-engine` (or any other recorder) living
    ///    outside the kernel.
    ///
    /// `outputs` should list entity IDs produced by the operation (e.g. the
    /// newly created solid/face/edge). Pass an empty `Vec` when the call
    /// is purely destructive or modifies existing entities in place.
    fn record_and_push(&mut self, operation: TimelineOperation, outputs: Vec<u64>) {
        // Preserve existing in-builder timeline semantics verbatim.
        self.timeline.push(operation.clone());

        // Build the canonical outward record.
        let kind = match &operation {
            TimelineOperation::Create2D { primitive_type, .. } => {
                format!("create_{}_2d", primitive_type)
            }
            TimelineOperation::Create3D { primitive_type, .. } => {
                format!("create_{}_3d", primitive_type)
            }
            TimelineOperation::Extrude { .. } => "extrude".to_string(),
            TimelineOperation::Revolve { .. } => "revolve".to_string(),
            TimelineOperation::Boolean {
                operation: op_kind, ..
            } => {
                let suffix = match op_kind {
                    BooleanOp::Union => "union",
                    BooleanOp::Intersection => "intersection",
                    BooleanOp::Difference => "difference",
                    BooleanOp::SymmetricDifference => "symmetric_difference",
                };
                format!("boolean_{}", suffix)
            }
            TimelineOperation::UpdateParameters { .. } => "update_parameters".to_string(),
        };

        // Derive inputs structurally from variants that reference existing
        // entities. Downstream recorders rely on `parameters` (below) for
        // full semantic detail; `inputs`/`outputs` are opaque entity
        // handles for lineage tracking.
        let inputs: Vec<u64> = match &operation {
            TimelineOperation::Extrude { profile_id, .. } => {
                vec![geometry_id_to_u64(*profile_id)]
            }
            TimelineOperation::Boolean { operand_ids, .. } => operand_ids
                .iter()
                .copied()
                .map(geometry_id_to_u64)
                .collect(),
            TimelineOperation::UpdateParameters { geometry_id, .. } => {
                vec![geometry_id_to_u64(*geometry_id)]
            }
            _ => Vec::new(),
        };

        // Serialize the full TimelineOperation as the parameters payload
        // so a recorder can replay without lossy encoding.
        let parameters = match serde_json::to_value(&operation) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    "failed to serialize TimelineOperation for recorder: {}",
                    e
                );
                serde_json::Value::Null
            }
        };

        let record = crate::operations::recorder::RecordedOperation::new(kind)
            .with_parameters(parameters)
            .with_inputs(inputs)
            .with_outputs(outputs);

        self.model.record_operation(record);
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

        // Record in timeline + forward to attached recorder.
        let operation = TimelineOperation::Create2D {
            primitive_type: "point".to_string(),
            parameters: [("x".to_string(), x), ("y".to_string(), y)].into(),
            timestamp: self.next_timestamp(),
        };
        self.record_and_push(operation, vec![vertex_id as u64]);

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
        let edge = Edge::new(
            0, // temporary ID
            start_vertex,
            end_vertex,
            curve_id,
            EdgeOrientation::Forward,
            crate::primitives::curve::ParameterRange::new(0.0, 1.0),
        );
        let edge_id = self.model.edges.add(edge);

        // Record in timeline + forward to attached recorder.
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
        self.record_and_push(operation, vec![edge_id as u64]);

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
        let edge = Edge::new(
            0, // temporary ID
            vertex_id,
            vertex_id, // same vertex for closed curve
            curve_id,
            EdgeOrientation::Forward,
            crate::primitives::curve::ParameterRange::new(0.0, 1.0),
        );
        let edge_id = self.model.edges.add(edge);

        // Record in timeline + forward to attached recorder.
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
        self.record_and_push(operation, vec![edge_id as u64]);

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

        // Record in timeline + forward to attached recorder.
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
        self.record_and_push(operation, vec![face_id as u64]);

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

        // Record in timeline + forward to attached recorder.
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
        self.record_and_push(operation, vec![solid_id as u64]);

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
        let loop_obj = Loop::new(0, LoopType::Outer);
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

        // Record in timeline + forward to attached recorder.
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
        self.record_and_push(operation, vec![solid_id as u64]);

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

        // Record in timeline + forward to attached recorder.
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
        self.record_and_push(operation, vec![solid_id as u64]);

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

        // Record in timeline + forward to attached recorder.
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
        self.record_and_push(operation, vec![solid_id as u64]);

        Ok(GeometryId::Solid(solid_id))
    }

    /// Create 3D torus
    ///
    /// Delegates topology construction to
    /// [`crate::primitives::torus_primitive::TorusPrimitive::create`] and
    /// records the operation on the timeline. The axis is normalised by
    /// `TorusParameters::new`; pass any non-zero direction.
    pub fn create_torus_3d(
        &mut self,
        center: Point3,
        axis: Vector3,
        major_radius: f64,
        minor_radius: f64,
    ) -> Result<GeometryId, PrimitiveError> {
        // Build & validate parameters (also normalises the axis and
        // rejects degenerate radii / self-intersecting tori).
        let params = crate::primitives::torus_primitive::TorusParameters::new(
            center,
            axis,
            major_radius,
            minor_radius,
        )?;

        let solid_id =
            crate::primitives::torus_primitive::TorusPrimitive::create(&params, self.model)?;

        // Record in timeline + forward to attached recorder.
        let operation = TimelineOperation::Create3D {
            primitive_type: "torus".to_string(),
            parameters: [
                ("center_x".to_string(), center.x),
                ("center_y".to_string(), center.y),
                ("center_z".to_string(), center.z),
                ("axis_x".to_string(), params.axis.x),
                ("axis_y".to_string(), params.axis.y),
                ("axis_z".to_string(), params.axis.z),
                ("major_radius".to_string(), major_radius),
                ("minor_radius".to_string(), minor_radius),
            ]
            .into(),
            timestamp: self.next_timestamp(),
        };
        self.record_and_push(operation, vec![solid_id as u64]);

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

        // Record in timeline + forward to attached recorder.
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
        self.record_and_push(operation, vec![solid_id as u64]);

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
            let edge = Edge::new(
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

            let edge = Edge::new(
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
        // Face topology: edges and per-edge orientations chosen so that the
        // outer-loop vertex traversal is CCW when viewed from outside the
        // solid, i.e. the right-hand-rule normal of the loop matches the
        // outward face normal stored on the surface.
        //
        // Vertex layout (set in `create_box_vertices`):
        //   v0=(-,-,-) v1=(+,-,-) v2=(+,+,-) v3=(-,+,-)   bottom (z=-hd)
        //   v4=(-,-,+) v5=(+,-,+) v6=(+,+,+) v7=(-,+,+)   top    (z=+hd)
        //
        // Edge layout (set in `create_box_edges`, all stored start→end):
        //   e0:v0→v1  e1:v1→v2  e2:v2→v3  e3:v3→v0  (bottom)
        //   e4:v4→v5  e5:v5→v6  e6:v6→v7  e7:v7→v4  (top)
        //   e8:v0→v4  e9:v1→v5  e10:v2→v6 e11:v3→v7 (vertical)
        //
        // `Loop::vertices_cached` derives vertex i as edge.start if
        // orientations[i] is true, else edge.end. The arrays below were
        // chosen so that the resulting vertex chain is a continuous,
        // non-degenerate quad whose right-hand normal matches the face
        // surface normal.
        let face_edge_data = [
            // Bottom (z=-hd, outward -Z): traversal v0→v3→v2→v1→v0
            //   v0→v3 = e3 reversed (e3:v3→v0)
            //   v3→v2 = e2 reversed (e2:v2→v3)
            //   v2→v1 = e1 reversed (e1:v1→v2)
            //   v1→v0 = e0 reversed (e0:v0→v1)
            (
                [3, 2, 1, 0],
                [false, false, false, false],
                Point3::new(0.0, 0.0, -hd),
                Vector3::new(0.0, 0.0, -1.0),
            ),
            // Top (z=+hd, outward +Z): traversal v4→v5→v6→v7→v4
            (
                [4, 5, 6, 7],
                [true, true, true, true],
                Point3::new(0.0, 0.0, hd),
                Vector3::new(0.0, 0.0, 1.0),
            ),
            // Front (y=-hh, outward -Y): traversal v0→v1→v5→v4→v0
            //   vertices come out as [e0.start, e9.start, e4.end, e8.end]
            //   = [v0, v1, v5, v4] — Newell normal in (x,z) = -Y. ✓
            (
                [0, 9, 4, 8],
                [true, true, false, false],
                Point3::new(0.0, -hh, 0.0),
                Vector3::new(0.0, -1.0, 0.0),
            ),
            // Back (y=+hh, outward +Y): traversal v2→v3→v7→v6→v2
            //   v2→v3 = e2 forward, v3→v7 = e11 forward,
            //   v7→v6 = e6 reversed, v6→v2 = e10 reversed
            (
                [2, 11, 6, 10],
                [true, true, false, false],
                Point3::new(0.0, hh, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
            ),
            // Left (x=-hw, outward -X): traversal v0→v4→v7→v3→v0
            //   v0→v4 = e8 forward, v4→v7 = e7 reversed (e7:v7→v4),
            //   v7→v3 = e11 reversed (e11:v3→v7), v3→v0 = e3 forward
            (
                [8, 7, 11, 3],
                [true, false, false, true],
                Point3::new(-hw, 0.0, 0.0),
                Vector3::new(-1.0, 0.0, 0.0),
            ),
            // Right (x=+hw, outward +X): traversal v1→v2→v6→v5→v1
            //   v1→v2 = e1 forward, v2→v6 = e10 forward,
            //   v6→v5 = e5 reversed, v5→v1 = e9 reversed
            (
                [1, 10, 5, 9],
                [true, true, false, false],
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

    /// Create a watertight B-Rep cylinder solid.
    ///
    /// Topology produced:
    /// - 2 vertices on the seam (one on each circular cap, at the
    ///   `ref_dir = axis.perpendicular()` reference direction).
    /// - 3 edges: a closed circle on the bottom cap, a closed circle on
    ///   the top cap, and a linear seam connecting the two seam vertices.
    /// - 3 faces:
    ///   - Bottom cap: planar surface with normal `-axis`. Outer loop
    ///     traverses the bottom circle in the orientation that yields
    ///     a CCW boundary when viewed from outside (along `-axis`),
    ///     i.e. `Backward` relative to the underlying parametric circle.
    ///   - Top cap: planar surface with normal `+axis`. Outer loop
    ///     traverses the top circle `Forward`.
    ///   - Lateral cylindrical face: outer loop is the canonical
    ///     seamed rectangle in (u, v) parameter space — bottom-circle
    ///     forward, seam forward, top-circle backward, seam backward.
    /// - 1 closed shell containing all three faces.
    ///
    /// References: Mäntylä §4 (B-Rep solid modelling), Stroud §3
    /// (seamed surfaces), Hoffmann §5 (analytical primitives).
    fn create_cylinder_topology(
        &mut self,
        base_center: Point3,
        axis: Vector3,
        radius: f64,
        height: f64,
    ) -> Result<SolidId, PrimitiveError> {
        let topology_err = |msg: String| PrimitiveError::TopologyError {
            message: msg,
            euler_characteristic: None,
        };

        // Reference direction must match the one Cylinder::new uses so
        // the seam vertex lands at u=0 in the lateral face's parametric
        // frame. `axis.perpendicular()` returns a unit-length vector.
        let ref_dir = axis.perpendicular();
        let top_center = base_center + axis * height;

        // ---- vertices: one seam vertex per cap. ----
        let v_bottom = self.model.vertices.add_or_find(
            base_center.x + ref_dir.x * radius,
            base_center.y + ref_dir.y * radius,
            base_center.z + ref_dir.z * radius,
            self.tolerance.distance(),
        );
        let v_top = self.model.vertices.add_or_find(
            top_center.x + ref_dir.x * radius,
            top_center.y + ref_dir.y * radius,
            top_center.z + ref_dir.z * radius,
            self.tolerance.distance(),
        );

        // ---- curves: two circles + one line. ----
        let bottom_circle = Circle::new(base_center, axis, radius)
            .map_err(|e| topology_err(format!("bottom circle: {e}")))?;
        let top_circle = Circle::new(top_center, axis, radius)
            .map_err(|e| topology_err(format!("top circle: {e}")))?;
        let seam_line = Line::new(
            base_center + ref_dir * radius,
            top_center + ref_dir * radius,
        );
        let bottom_circle_id = self.model.curves.add(Box::new(bottom_circle));
        let top_circle_id = self.model.curves.add(Box::new(top_circle));
        let seam_line_id = self.model.curves.add(Box::new(seam_line));

        // ---- edges: closed circles + linear seam. ----
        // Closed circle edges: start_vertex == end_vertex (the seam vertex).
        // Parameter range is the full angular sweep [0, 2π).
        let two_pi = std::f64::consts::TAU;
        let bottom_edge = self.model.edges.add(Edge::new(
            0,
            v_bottom,
            v_bottom,
            bottom_circle_id,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, two_pi),
        ));
        let top_edge = self.model.edges.add(Edge::new(
            0,
            v_top,
            v_top,
            top_circle_id,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, two_pi),
        ));
        let seam_edge = self.model.edges.add(Edge::new(
            0,
            v_bottom,
            v_top,
            seam_line_id,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        ));

        // ---- surfaces: 2 planes + 1 finite cylinder. ----
        let bottom_plane = Plane::from_point_normal(base_center, -axis)
            .map_err(|e| topology_err(format!("bottom plane: {e}")))?;
        let top_plane = Plane::from_point_normal(top_center, axis)
            .map_err(|e| topology_err(format!("top plane: {e}")))?;
        let lateral_cyl = Cylinder::new_finite(base_center, axis, radius, height)
            .map_err(|e| topology_err(format!("lateral cylinder: {e}")))?;
        let bottom_surface_id = self.model.surfaces.add(Box::new(bottom_plane));
        let top_surface_id = self.model.surfaces.add(Box::new(top_plane));
        let lateral_surface_id = self.model.surfaces.add(Box::new(lateral_cyl));

        // ---- loops. ----
        // Bottom cap: outward normal is `-axis`. The Circle is
        // parameterized CCW when viewed from `+axis`. Looking from
        // `-axis` (outside the bottom cap), that traversal appears CW,
        // so we walk the edge `Backward` to get an outward-CCW loop.
        let mut bottom_loop = Loop::new(0, LoopType::Outer);
        bottom_loop.add_edge(bottom_edge, false);
        let bottom_loop_id = self.model.loops.add(bottom_loop);

        // Top cap: outward normal is `+axis`, same orientation as the
        // Circle's parametric CCW direction → walk `Forward`.
        let mut top_loop = Loop::new(0, LoopType::Outer);
        top_loop.add_edge(top_edge, true);
        let top_loop_id = self.model.loops.add(top_loop);

        // Lateral seamed face: in (u, v) parameter space the outer loop
        // is a CCW rectangle with corners at (0, 0), (2π, 0), (2π, h),
        // (0, h). The seam is the degenerate segment u=0 ≡ u=2π
        // traversed twice (once forward, once backward) to close the
        // rectangle. Edge sequence:
        //   (0,0)→(2π,0): bottom_circle forward
        //   (2π,0)→(2π,h): seam forward
        //   (2π,h)→(0,h): top_circle backward
        //   (0,h)→(0,0): seam backward
        let mut lateral_loop = Loop::new(0, LoopType::Outer);
        lateral_loop.add_edge(bottom_edge, true);
        lateral_loop.add_edge(seam_edge, true);
        lateral_loop.add_edge(top_edge, false);
        lateral_loop.add_edge(seam_edge, false);
        let lateral_loop_id = self.model.loops.add(lateral_loop);

        // ---- faces. ----
        let mut bottom_face = Face::new(
            0,
            bottom_surface_id,
            bottom_loop_id,
            FaceOrientation::Forward,
        );
        bottom_face.outer_loop = bottom_loop_id;
        let bottom_face_id = self.model.faces.add(bottom_face);

        let mut top_face = Face::new(0, top_surface_id, top_loop_id, FaceOrientation::Forward);
        top_face.outer_loop = top_loop_id;
        let top_face_id = self.model.faces.add(top_face);

        let mut lateral_face = Face::new(
            0,
            lateral_surface_id,
            lateral_loop_id,
            FaceOrientation::Forward,
        );
        lateral_face.outer_loop = lateral_loop_id;
        let lateral_face_id = self.model.faces.add(lateral_face);

        // ---- shell + solid. ----
        let mut shell = Shell::new(0, ShellType::Closed);
        shell.add_face(bottom_face_id);
        shell.add_face(top_face_id);
        shell.add_face(lateral_face_id);
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
        ConePrimitive::create(&params, self.model)
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
        // Purely mutating — no new outputs produced. Inputs are derived
        // inside `record_and_push` from the variant itself.
        self.record_and_push(operation.clone(), Vec::new());

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
            .or_default()
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

    /// Validate updated box parameters.
    ///
    /// The actual topology rewrite is performed by
    /// `BoxPrimitive::update_parameters` (delete + recreate path); this
    /// function exists only to surface invalid cached parameters early so
    /// the timeline doesn't accept obviously bad updates.
    fn rebuild_box(
        &mut self,
        _geometry_id: GeometryId,
        params: &DashMap<String, f64>,
    ) -> Result<(), PrimitiveError> {
        let width = params.get("width").map(|v| *v).unwrap_or(1.0);
        let height = params.get("height").map(|v| *v).unwrap_or(1.0);
        let depth = params.get("depth").map(|v| *v).unwrap_or(1.0);

        if width <= 0.0 || height <= 0.0 || depth <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "dimensions".to_string(),
                value: format!("{}x{}x{}", width, height, depth),
                constraint: "all dimensions must be positive".to_string(),
            });
        }
        Ok(())
    }

    /// Validate updated sphere parameters.
    ///
    /// Topology rewrite happens in `SpherePrimitive::update_parameters`.
    fn rebuild_sphere(
        &mut self,
        _geometry_id: GeometryId,
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

    /// Validate updated cylinder parameters.
    ///
    /// Topology rewrite happens in `CylinderPrimitive::update_parameters`.
    fn rebuild_cylinder(
        &mut self,
        _geometry_id: GeometryId,
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
        Ok(())
    }

    /// Validate updated 2D rectangle parameters.
    ///
    /// Topology rewrite happens through the 2D primitive update path.
    fn rebuild_rectangle(
        &mut self,
        _geometry_id: GeometryId,
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

    /// Validate updated 2D circle parameters.
    ///
    /// Topology rewrite happens through the 2D primitive update path.
    fn rebuild_circle_2d(
        &mut self,
        _geometry_id: GeometryId,
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

}

#[cfg(test)]
mod cascade_tests {
    use super::*;
    use crate::primitives::edge::{Edge, EdgeOrientation};
    use crate::primitives::r#loop::Loop;
    use crate::primitives::shell::Shell;

    /// Build a single-face triangle on z = 0:
    ///     v1 = (0, 0, 0)
    ///     v2 = (1, 0, 0)
    ///     v3 = (0.5, 1, 0)
    /// returns (model, [v1, v2, v3], [e1_v1v2, e2_v2v3, e3_v3v1], loop_id,
    /// face_id, shell_id).
    fn make_triangle() -> (
        BRepModel,
        [VertexId; 3],
        [EdgeId; 3],
        LoopId,
        FaceId,
        ShellId,
    ) {
        let mut model = BRepModel::new();
        let tol = Tolerance::default().distance();

        let v1 = model.vertices.add_or_find(0.0, 0.0, 0.0, tol);
        let v2 = model.vertices.add_or_find(1.0, 0.0, 0.0, tol);
        let v3 = model.vertices.add_or_find(0.5, 1.0, 0.0, tol);

        let c1 = model.curves.add(Box::new(Line::new(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
        )));
        let c2 = model.curves.add(Box::new(Line::new(
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.5, 1.0, 0.0),
        )));
        let c3 = model.curves.add(Box::new(Line::new(
            Point3::new(0.5, 1.0, 0.0),
            Point3::new(0.0, 0.0, 0.0),
        )));

        let e1 = model.edges.add_or_find(Edge::new(
            0,
            v1,
            v2,
            c1,
            EdgeOrientation::Forward,
            ParameterRange::unit(),
        ));
        let e2 = model.edges.add_or_find(Edge::new(
            0,
            v2,
            v3,
            c2,
            EdgeOrientation::Forward,
            ParameterRange::unit(),
        ));
        let e3 = model.edges.add_or_find(Edge::new(
            0,
            v3,
            v1,
            c3,
            EdgeOrientation::Forward,
            ParameterRange::unit(),
        ));

        let mut face_loop = Loop::new(0, LoopType::Outer);
        face_loop.add_edge(e1, true);
        face_loop.add_edge(e2, true);
        face_loop.add_edge(e3, true);
        let loop_id = model.loops.add(face_loop);

        let plane = Plane::new(Point3::ORIGIN, Vector3::Z, Vector3::X)
            .expect("plane construction must succeed for axis-aligned XY plane");
        let surface_id = model.surfaces.add(Box::new(plane));
        let face = Face::new(0, surface_id, loop_id, FaceOrientation::Forward);
        let face_id = model.faces.add(face);

        let mut shell = Shell::new(0, ShellType::Open);
        shell.add_face(face_id);
        let shell_id = model.shells.add(shell);

        (model, [v1, v2, v3], [e1, e2, e3], loop_id, face_id, shell_id)
    }

    #[test]
    fn delete_face_cascade_drops_face_and_shell_reference() {
        let (mut model, _v, _e, _loop_id, face_id, shell_id) = make_triangle();

        let report = model.delete_face_cascade(face_id);

        assert_eq!(report.removed_faces, vec![face_id]);
        assert!(report.removed_loops.is_empty());
        assert!(report.removed_edges.is_empty());
        assert!(report.removed_vertices.is_empty());
        assert_eq!(report.affected_shells, vec![shell_id]);

        assert_eq!(model.faces.iter().count(), 0);
        assert_eq!(model.loops.iter().count(), 1);
        assert_eq!(model.edges.iter().count(), 3);
        let live_shell = model
            .shells
            .get(shell_id)
            .expect("shell still exists after face cascade");
        assert!(live_shell.find_face(face_id).is_none());
    }

    #[test]
    fn delete_edge_cascade_propagates_through_loop_and_face() {
        let (mut model, _v, e, loop_id, face_id, shell_id) = make_triangle();

        let report = model.delete_edge_cascade(e[1]);

        assert!(report.removed_edges.contains(&e[1]));
        assert_eq!(report.removed_loops, vec![loop_id]);
        assert_eq!(report.removed_faces, vec![face_id]);
        assert_eq!(report.affected_shells, vec![shell_id]);

        let live_edges: Vec<_> = model.edges.iter().map(|(eid, _)| eid).collect();
        assert!(!live_edges.contains(&e[1]));
        assert_eq!(model.loops.iter().count(), 0);
        assert_eq!(model.faces.iter().count(), 0);
    }

    #[test]
    fn delete_loop_cascade_drops_face_but_preserves_edges_and_vertices() {
        let (mut model, _v, _e, loop_id, face_id, _shell_id) = make_triangle();

        let report = model.delete_loop_cascade(loop_id);

        assert_eq!(report.removed_loops, vec![loop_id]);
        assert_eq!(report.removed_faces, vec![face_id]);
        assert!(report.removed_edges.is_empty());
        assert!(report.removed_vertices.is_empty());

        assert_eq!(model.loops.iter().count(), 0);
        assert_eq!(model.faces.iter().count(), 0);
        // Edges and vertices belong to no other face, but cascading does not
        // chase ownership downward — they stay live.
        assert_eq!(model.edges.iter().count(), 3);
        assert_eq!(model.vertices.iter().count(), 3);
    }

    #[test]
    fn delete_vertex_cascade_on_missing_id_is_a_noop() {
        let mut model = BRepModel::new();
        let report = model.delete_vertex_cascade(99);
        assert!(report.removed_vertices.is_empty());
        assert!(report.removed_edges.is_empty());
        assert!(report.removed_loops.is_empty());
        assert!(report.removed_faces.is_empty());
        assert!(report.affected_shells.is_empty());
    }

    #[test]
    fn delete_vertex_cascade_on_isolated_vertex_does_not_touch_topology() {
        let (mut model, v, _e, loop_id, face_id, _shell_id) = make_triangle();
        let tol = Tolerance::default().distance();
        let isolated = model.vertices.add_or_find(5.0, 5.0, 5.0, tol);

        let report = model.delete_vertex_cascade(isolated);

        assert_eq!(report.removed_vertices, vec![isolated]);
        assert!(report.removed_edges.is_empty());
        assert!(report.removed_loops.is_empty());
        assert!(report.removed_faces.is_empty());

        // Original triangle survives intact.
        assert!(model.loops.get(loop_id).is_some());
        assert!(model.faces.get(face_id).is_some());
        for vid in v {
            assert!(model.vertices.get(vid).is_some());
        }
    }
}

// Circle and Sphere implementations are in their respective modules
