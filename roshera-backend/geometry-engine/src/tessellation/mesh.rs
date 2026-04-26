//! Mesh data structures for tessellation
//!
//! Indexed access into vertex/index arrays is the canonical idiom — all
//! `arr[i]` sites use indices bounded by mesh dimensions. Matches the
//! numerical-kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

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

        // Transform normals using the inverse-transpose of the upper-left 3x3.
        // This is required for correctness under non-uniform scaling and shear:
        // a normal `n` to a surface, after transforming the surface by M, must
        // be transformed by (M⁻¹)ᵀ to remain perpendicular to the transformed
        // surface (Lengyel, *Mathematics for 3D Game Programming*, §4.5).
        // `Matrix4::transform_normal` implements this exactly.
        //
        // Singular linear part (e.g. zero scale on some axis) → fall back to
        // an identity-mapped normal rather than panicking; downstream
        // tessellation will still render, just with a degenerate normal that
        // the caller can re-derive from the geometry if needed.
        for i in 0..self.vertex_count() {
            let normal = Vector3::new(
                self.normals[i * 3] as f64,
                self.normals[i * 3 + 1] as f64,
                self.normals[i * 3 + 2] as f64,
            );

            let transformed = matrix.transform_normal(&normal).unwrap_or(normal);

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
    /// Compute mesh quality metrics over the triangle list.
    ///
    /// For each triangle the three edge lengths and three interior angles
    /// are evaluated. The reported metrics are the global extrema and
    /// average over all edges (each shared edge counted twice — fine for
    /// quality reporting, bad for topology). The aspect ratio reported is
    /// the worst (largest) per-triangle ratio of longest-to-shortest edge.
    ///
    /// Empty mesh → all-zero metrics. Degenerate triangles (zero-length
    /// edge) contribute 0° to `min_angle` and skip aspect-ratio updates.
    ///
    /// Angles are reported in **degrees**.
    pub fn quality_metrics(&self) -> MeshQuality {
        if self.triangles.is_empty() {
            return MeshQuality {
                min_edge_length: 0.0,
                max_edge_length: 0.0,
                avg_edge_length: 0.0,
                min_angle: 0.0,
                max_angle: 0.0,
                aspect_ratio: 0.0,
            };
        }

        let mut min_edge = f64::INFINITY;
        let mut max_edge: f64 = 0.0;
        let mut edge_sum = 0.0_f64;
        let mut edge_count: usize = 0;

        let mut min_angle = f64::INFINITY;
        let mut max_angle: f64 = 0.0;

        let mut max_aspect: f64 = 0.0;

        for tri in &self.triangles {
            // Bounds-checked vertex access; bad indices yield a degenerate
            // triangle that is skipped.
            let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            let n = self.vertices.len();
            if i0 >= n || i1 >= n || i2 >= n {
                continue;
            }
            let p0 = self.vertices[i0].position;
            let p1 = self.vertices[i1].position;
            let p2 = self.vertices[i2].position;

            let e01 = p1 - p0;
            let e12 = p2 - p1;
            let e20 = p0 - p2;

            let l0 = e01.magnitude();
            let l1 = e12.magnitude();
            let l2 = e20.magnitude();

            for &len in &[l0, l1, l2] {
                if len < min_edge {
                    min_edge = len;
                }
                if len > max_edge {
                    max_edge = len;
                }
                edge_sum += len;
                edge_count += 1;
            }

            // Per-triangle aspect ratio: longest / shortest edge. The
            // canonical metric for triangle "fatness". Skip degenerate.
            let tri_min = l0.min(l1).min(l2);
            let tri_max = l0.max(l1).max(l2);
            if tri_min > 0.0 {
                let ar = tri_max / tri_min;
                if ar > max_aspect {
                    max_aspect = ar;
                }
            }

            // Interior angles via dot product of incoming/outgoing edges
            // at each vertex. At vertex v0: angle between (-e20) and e01.
            // Use clamped acos to suppress |cos|>1 from rounding.
            let angle_at = |a: Vector3, b: Vector3| -> f64 {
                let am = a.magnitude();
                let bm = b.magnitude();
                if am == 0.0 || bm == 0.0 {
                    return 0.0;
                }
                let c = (a.dot(&b) / (am * bm)).clamp(-1.0, 1.0);
                c.acos().to_degrees()
            };

            // (-e20) · e01 → angle at p0
            // (-e01) · e12 → angle at p1
            // (-e12) · e20 → angle at p2
            let a0 = angle_at(-e20, e01);
            let a1 = angle_at(-e01, e12);
            let a2 = angle_at(-e12, e20);

            for &a in &[a0, a1, a2] {
                if a < min_angle {
                    min_angle = a;
                }
                if a > max_angle {
                    max_angle = a;
                }
            }
        }

        let avg_edge = if edge_count > 0 {
            edge_sum / edge_count as f64
        } else {
            0.0
        };

        // If we never saw a finite edge, report zero rather than infinity.
        if !min_edge.is_finite() {
            min_edge = 0.0;
        }
        if !min_angle.is_finite() {
            min_angle = 0.0;
        }

        MeshQuality {
            min_edge_length: min_edge,
            max_edge_length: max_edge,
            avg_edge_length: avg_edge,
            min_angle,
            max_angle,
            aspect_ratio: max_aspect,
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
