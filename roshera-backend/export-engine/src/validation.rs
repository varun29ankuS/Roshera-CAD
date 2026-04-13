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

    /// Optimize mesh for export
    pub fn optimize_for_export(
        &self,
        mesh: &Mesh,
        _format: &ExportFormat,
    ) -> Result<Mesh, ExportError> {
        // For now, just return a clone
        // In production, implement actual optimization
        Ok(mesh.clone())
    }
}
