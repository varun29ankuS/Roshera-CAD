//! Mesh data structures for tessellation

use crate::math::{Point3, Vector3};
use serde::{Deserialize, Serialize};
use shared_types;

/// Vertex in a triangle mesh
#[derive(Debug, Clone, Copy)]
pub struct MeshVertex {
    pub position: Point3,
    pub normal: Vector3,
    pub uv: Option<(f64, f64)>,
}

/// Triangle mesh representation
#[derive(Debug, Clone)]
pub struct TriangleMesh {
    pub vertices: Vec<MeshVertex>,
    pub triangles: Vec<[u32; 3]>,
    /// Maps each triangle index to the B-Rep FaceId it was tessellated from.
    /// Length equals `triangles.len()`. Used for face picking in the viewport.
    pub face_map: Vec<u32>,
}

impl TriangleMesh {
    pub fn new() -> Self {
        Self {
            vertices: Vec::new(),
            triangles: Vec::new(),
            face_map: Vec::new(),
        }
    }

    /// Add a vertex and return its index
    pub fn add_vertex(&mut self, vertex: MeshVertex) -> u32 {
        let index = self.vertices.len() as u32;
        self.vertices.push(vertex);
        index
    }

    /// Add a triangle
    pub fn add_triangle(&mut self, v0: u32, v1: u32, v2: u32) {
        self.triangles.push([v0, v1, v2]);
    }

    /// Convert to Three.js compatible format
    pub fn to_threejs(&self) -> ThreeJsMesh {
        let mut mesh = ThreeJsMesh::new();

        for vertex in &self.vertices {
            mesh.add_vertex(vertex.position, vertex.normal);
        }

        for triangle in &self.triangles {
            mesh.add_triangle(triangle[0], triangle[1], triangle[2]);
        }

        if !self.face_map.is_empty() {
            mesh.face_map = Some(self.face_map.clone());
        }

        mesh
    }

    /// Convert to shared_types::Mesh format
    pub fn to_shared_mesh(&self) -> shared_types::Mesh {
        let mut mesh = shared_types::Mesh {
            vertices: Vec::with_capacity(self.vertices.len() * 3),
            normals: Vec::with_capacity(self.vertices.len() * 3),
            indices: Vec::with_capacity(self.triangles.len() * 3),
            uvs: None,
            colors: None,
            face_map: if self.face_map.is_empty() {
                None
            } else {
                Some(self.face_map.clone())
            },
        };

        // Convert vertices and normals
        for vertex in &self.vertices {
            mesh.vertices.push(vertex.position.x as f32);
            mesh.vertices.push(vertex.position.y as f32);
            mesh.vertices.push(vertex.position.z as f32);

            mesh.normals.push(vertex.normal.x as f32);
            mesh.normals.push(vertex.normal.y as f32);
            mesh.normals.push(vertex.normal.z as f32);
        }

        // Convert indices
        for triangle in &self.triangles {
            mesh.indices.push(triangle[0]);
            mesh.indices.push(triangle[1]);
            mesh.indices.push(triangle[2]);
        }

        // Add UVs if present
        if self.vertices.iter().any(|v| v.uv.is_some()) {
            let mut uvs = Vec::with_capacity(self.vertices.len() * 2);
            for vertex in &self.vertices {
                if let Some((u, v)) = vertex.uv {
                    uvs.push(u as f32);
                    uvs.push(v as f32);
                } else {
                    uvs.push(0.0);
                    uvs.push(0.0);
                }
            }
            mesh.uvs = Some(uvs);
        }

        mesh
    }
}

/// Three.js compatible mesh data (for web export)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreeJsMesh {
    /// Vertex positions (x,y,z flattened)
    pub positions: Vec<f32>,
    /// Normal vectors (x,y,z flattened)
    pub normals: Vec<f32>,
    /// Triangle indices
    pub indices: Vec<u32>,
    /// Optional vertex colors (r,g,b flattened)
    pub colors: Option<Vec<f32>>,
    /// Optional UV coordinates (u,v flattened)
    pub uvs: Option<Vec<f32>>,
    /// Maps each triangle index to the B-Rep FaceId it was tessellated from.
    /// `face_map[triangle_index] = face_id`. Used for face picking.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub face_map: Option<Vec<u32>>,
}

impl ThreeJsMesh {
    pub fn new() -> Self {
        Self {
            positions: Vec::new(),
            normals: Vec::new(),
            indices: Vec::new(),
            colors: None,
            uvs: None,
            face_map: None,
        }
    }

    /// Add a vertex with position and normal
    pub fn add_vertex(&mut self, position: Point3, normal: Vector3) -> u32 {
        let index = (self.positions.len() / 3) as u32;

        self.positions.push(position.x as f32);
        self.positions.push(position.y as f32);
        self.positions.push(position.z as f32);

        self.normals.push(normal.x as f32);
        self.normals.push(normal.y as f32);
        self.normals.push(normal.z as f32);

        index
    }

    /// Add a vertex with position, normal, and UV
    pub fn add_vertex_with_uv(&mut self, position: Point3, normal: Vector3, u: f64, v: f64) -> u32 {
        let index = self.add_vertex(position, normal);

        // Initialize UVs if needed
        if self.uvs.is_none() {
            self.uvs = Some(Vec::with_capacity(self.positions.len() / 3 * 2));
            // Fill in missing UVs with zeros
            if let Some(uvs) = &mut self.uvs {
                uvs.resize((index * 2) as usize, 0.0);
            }
        }

        if let Some(uvs) = &mut self.uvs {
            uvs.push(u as f32);
            uvs.push(v as f32);
        }

        index
    }

    /// Add a triangle
    pub fn add_triangle(&mut self, v0: u32, v1: u32, v2: u32) {
        self.indices.push(v0);
        self.indices.push(v1);
        self.indices.push(v2);
    }

    /// Get vertex count
    pub fn vertex_count(&self) -> usize {
        self.positions.len() / 3
    }

    /// Get triangle count
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Convert to shared_types::Mesh format
    pub fn to_shared_mesh(&self) -> shared_types::Mesh {
        shared_types::Mesh {
            vertices: self.positions.clone(),
            normals: self.normals.clone(),
            indices: self.indices.clone(),
            uvs: self.uvs.clone(),
            colors: self.colors.clone(),
            face_map: self.face_map.clone(),
        }
    }

    /// Merge another mesh into this one
    pub fn merge(&mut self, other: &ThreeJsMesh) {
        let vertex_offset = self.vertex_count() as u32;

        // Copy vertices
        self.positions.extend(&other.positions);
        self.normals.extend(&other.normals);

        // Copy and offset indices
        for &index in &other.indices {
            self.indices.push(index + vertex_offset);
        }

        // Handle optional attributes
        if let (Some(self_colors), Some(other_colors)) = (&mut self.colors, &other.colors) {
            self_colors.extend(other_colors);
        }

        if let (Some(self_uvs), Some(other_uvs)) = (&mut self.uvs, &other.uvs) {
            self_uvs.extend(other_uvs);
        }

        if let (Some(self_fm), Some(other_fm)) = (&mut self.face_map, &other.face_map) {
            self_fm.extend(other_fm);
        }
    }

    /// Apply a transformation matrix to all vertices
    pub fn transform(&mut self, matrix: &crate::math::Matrix4) {
        use crate::math::Transform;

        // Transform positions
        for i in 0..self.vertex_count() {
            let pos = Point3::new(
                self.positions[i * 3] as f64,
                self.positions[i * 3 + 1] as f64,
                self.positions[i * 3 + 2] as f64,
            );

            let transformed = matrix.transform_point(&pos);

            self.positions[i * 3] = transformed.x as f32;
            self.positions[i * 3 + 1] = transformed.y as f32;
            self.positions[i * 3 + 2] = transformed.z as f32;
        }

        // Transform normals (use inverse transpose for correct normal transformation)
        // For now, just transform as vectors (works for uniform scaling and rotation)
        for i in 0..self.vertex_count() {
            let normal = Vector3::new(
                self.normals[i * 3] as f64,
                self.normals[i * 3 + 1] as f64,
                self.normals[i * 3 + 2] as f64,
            );

            let transformed = matrix
                .transform_vector(&normal)
                .normalize()
                .unwrap_or(Vector3::Z);

            self.normals[i * 3] = transformed.x as f32;
            self.normals[i * 3 + 1] = transformed.y as f32;
            self.normals[i * 3 + 2] = transformed.z as f32;
        }
    }
}

impl Default for TriangleMesh {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for ThreeJsMesh {
    fn default() -> Self {
        Self::new()
    }
}

/// Mesh quality metrics
#[derive(Debug)]
pub struct MeshQuality {
    pub min_edge_length: f64,
    pub max_edge_length: f64,
    pub avg_edge_length: f64,
    pub min_angle: f64,
    pub max_angle: f64,
    pub aspect_ratio: f64,
}

impl TriangleMesh {
    /// Calculate mesh quality metrics
    pub fn quality_metrics(&self) -> MeshQuality {
        // TODO: Implement quality metrics calculation
        MeshQuality {
            min_edge_length: 0.0,
            max_edge_length: 1.0,
            avg_edge_length: 0.5,
            min_angle: 30.0,
            max_angle: 90.0,
            aspect_ratio: 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mesh_creation() {
        let mut mesh = TriangleMesh::new();

        let v0 = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 0.0, 0.0),
            normal: Vector3::Z,
            uv: Some((0.0, 0.0)),
        });

        let v1 = mesh.add_vertex(MeshVertex {
            position: Point3::new(1.0, 0.0, 0.0),
            normal: Vector3::Z,
            uv: Some((1.0, 0.0)),
        });

        let v2 = mesh.add_vertex(MeshVertex {
            position: Point3::new(0.0, 1.0, 0.0),
            normal: Vector3::Z,
            uv: Some((0.0, 1.0)),
        });

        mesh.add_triangle(v0, v1, v2);

        assert_eq!(mesh.vertices.len(), 3);
        assert_eq!(mesh.triangles.len(), 1);
    }

    #[test]
    fn test_threejs_conversion() {
        let mut mesh = ThreeJsMesh::new();

        let v0 = mesh.add_vertex(Point3::new(0.0, 0.0, 0.0), Vector3::Z);
        let v1 = mesh.add_vertex(Point3::new(1.0, 0.0, 0.0), Vector3::Z);
        let v2 = mesh.add_vertex(Point3::new(0.0, 1.0, 0.0), Vector3::Z);

        mesh.add_triangle(v0, v1, v2);

        assert_eq!(mesh.vertex_count(), 3);
        assert_eq!(mesh.triangle_count(), 1);
        assert_eq!(mesh.positions.len(), 9);
        assert_eq!(mesh.normals.len(), 9);
        assert_eq!(mesh.indices.len(), 3);
    }
}
