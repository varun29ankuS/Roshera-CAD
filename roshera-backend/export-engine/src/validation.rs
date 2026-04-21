//! Export validation utilities

use shared_types::*;

/// Validation report
#[derive(Debug, Clone)]
pub struct ValidationReport {
    /// Is mesh valid for export
    pub valid: bool,
    /// Validation errors
    pub errors: Vec<String>,
    /// Mesh statistics
    pub statistics: MeshStatistics,
}

/// Mesh statistics
#[derive(Debug, Clone)]
pub struct MeshStatistics {
    /// Number of vertices
    pub vertex_count: usize,
    /// Number of triangles
    pub triangle_count: usize,
    /// Bounding box
    pub bounding_box: ([f32; 3], [f32; 3]),
}

/// Export validator
pub struct ExportValidator;

impl ExportValidator {
    /// Create new validator
    pub fn new() -> Self {
        Self
    }

    /// Validate mesh for export
    pub fn validate_for_export(&self, mesh: &Mesh, format: &ExportFormat) -> ValidationReport {
        let mut errors = Vec::new();

        // Basic validation
        if let Err(e) = mesh.validate() {
            errors.push(format!("Mesh validation failed: {:?}", e));
        }

        // Format-specific validation
        match format {
            ExportFormat::STL => {
                if mesh.triangle_count() == 0 {
                    errors.push("STL requires at least one triangle".to_string());
                }
            }
            ExportFormat::OBJ => {
                if mesh.vertex_count() == 0 {
                    errors.push("OBJ requires at least one vertex".to_string());
                }
            }
            _ => {}
        }

        let bounds = mesh.bounds();

        ValidationReport {
            valid: errors.is_empty(),
            errors,
            statistics: MeshStatistics {
                vertex_count: mesh.vertex_count(),
                triangle_count: mesh.triangle_count(),
                bounding_box: (bounds.min, bounds.max),
            },
        }
    }
}

/// Mesh optimizer for export
pub struct MeshOptimizer {
    target_triangles: Option<usize>,
}

impl MeshOptimizer {
    /// Create new optimizer
    pub fn new() -> Self {
        Self {
            target_triangles: None,
        }
    }

    /// Set target triangle count
    pub fn with_target_triangles(mut self, count: usize) -> Self {
        self.target_triangles = Some(count);
        self
    }

    /// Optimize mesh for export by deduplicating vertices and optionally decimating
    pub fn optimize_for_export(
        &self,
        mesh: &Mesh,
        _format: &ExportFormat,
    ) -> Result<Mesh, ExportError> {
        let mut optimized = mesh.clone();

        // Step 1: Deduplicate vertices within tolerance
        let tolerance = 1e-6_f32;
        let original_count = optimized.vertices.len() / 3;
        if original_count == 0 {
            return Ok(optimized);
        }

        // Build a vertex remap table: old index -> new index
        let mut remap = vec![0u32; original_count];
        let mut unique_vertices: Vec<[f32; 3]> = Vec::with_capacity(original_count);
        let mut unique_normals: Vec<[f32; 3]> = Vec::with_capacity(original_count);

        for i in 0..original_count {
            let vx = optimized.vertices[i * 3];
            let vy = optimized.vertices[i * 3 + 1];
            let vz = optimized.vertices[i * 3 + 2];

            // Search for an existing matching vertex
            let mut found = None;
            for (j, uv) in unique_vertices.iter().enumerate() {
                let dx = vx - uv[0];
                let dy = vy - uv[1];
                let dz = vz - uv[2];
                if dx * dx + dy * dy + dz * dz < tolerance * tolerance {
                    found = Some(j);
                    break;
                }
            }

            if let Some(j) = found {
                remap[i] = j as u32;
            } else {
                remap[i] = unique_vertices.len() as u32;
                unique_vertices.push([vx, vy, vz]);
                if i * 3 + 2 < optimized.normals.len() {
                    unique_normals.push([
                        optimized.normals[i * 3],
                        optimized.normals[i * 3 + 1],
                        optimized.normals[i * 3 + 2],
                    ]);
                } else {
                    unique_normals.push([0.0, 0.0, 1.0]);
                }
            }
        }

        // Remap indices
        for idx in optimized.indices.iter_mut() {
            if (*idx as usize) < remap.len() {
                *idx = remap[*idx as usize];
            }
        }

        // Flatten back
        optimized.vertices = unique_vertices
            .iter()
            .flat_map(|v| v.iter().copied())
            .collect();
        optimized.normals = unique_normals
            .iter()
            .flat_map(|n| n.iter().copied())
            .collect();

        // Remove degenerate triangles (where two or more indices are the same)
        let mut clean_indices = Vec::with_capacity(optimized.indices.len());
        for tri in optimized.indices.chunks(3) {
            if tri.len() == 3 && tri[0] != tri[1] && tri[1] != tri[2] && tri[0] != tri[2] {
                clean_indices.extend_from_slice(tri);
            }
        }
        optimized.indices = clean_indices;

        Ok(optimized)
    }
}
