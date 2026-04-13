//! B-Rep tessellation module
//!
//! Converts analytical B-Rep models to triangle meshes for visualization and export.

pub mod adaptive;
pub mod cache;
pub mod curve;
pub mod mesh;
pub mod parallel;
pub mod simple_box;
pub mod surface;

// Re-export main types
pub use adaptive::AdaptiveTessellator;
pub use curve::{tessellate_curve, tessellate_edge};
pub use mesh::{MeshVertex, ThreeJsMesh, TriangleMesh};
pub use surface::{tessellate_face, tessellate_surface};

use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::{builder::BRepModel, shell::Shell, solid::Solid};

/// Tessellation parameters for controlling mesh quality
#[derive(Debug, Clone)]
pub struct TessellationParams {
    /// Maximum edge length in the mesh
    pub max_edge_length: f64,
    /// Maximum angle deviation from true surface (radians)
    pub max_angle_deviation: f64,
    /// Maximum distance from chord to curve
    pub chord_tolerance: f64,
    /// Minimum number of segments for curves
    pub min_segments: usize,
    /// Maximum number of segments for curves
    pub max_segments: usize,
}

impl Default for TessellationParams {
    fn default() -> Self {
        Self {
            max_edge_length: 0.1,
            max_angle_deviation: 0.1,
            chord_tolerance: 0.001,
            min_segments: 3,
            max_segments: 100,
        }
    }
}

impl TessellationParams {
    /// Create parameters for coarse tessellation (preview quality)
    pub fn coarse() -> Self {
        Self {
            max_edge_length: 0.5,
            max_angle_deviation: 0.3,
            chord_tolerance: 0.01,
            min_segments: 3,
            max_segments: 20,
        }
    }

    /// Create parameters for fine tessellation (high quality)
    pub fn fine() -> Self {
        Self {
            max_edge_length: 0.01,
            max_angle_deviation: 0.02,
            chord_tolerance: 0.0001,
            min_segments: 8,
            max_segments: 200,
        }
    }

    /// Create parameters for ultra-fast real-time preview
    pub fn realtime() -> Self {
        Self {
            max_edge_length: 1.0,
            max_angle_deviation: 0.5,
            chord_tolerance: 0.1,
            min_segments: 3,
            max_segments: 8, // Very low for speed
        }
    }
}

/// Tessellate a solid into a triangle mesh
pub fn tessellate_solid(
    solid: &Solid,
    model: &BRepModel,
    params: &TessellationParams,
) -> TriangleMesh {
    let mut mesh = TriangleMesh::new();

    // Tessellate outer shell
    if let Some(shell) = model.shells.get(solid.outer_shell) {
        tessellate_shell(shell, model, params, &mut mesh);
    }

    // Tessellate inner shells (voids)
    for &inner_shell_id in &solid.inner_shells {
        if let Some(shell) = model.shells.get(inner_shell_id) {
            tessellate_shell(shell, model, params, &mut mesh);
        }
    }

    mesh
}

/// Tessellate a shell and append to existing mesh.
/// Populates `mesh.face_map` so each triangle maps back to its B-Rep FaceId.
pub fn tessellate_shell(
    shell: &Shell,
    model: &BRepModel,
    params: &TessellationParams,
    mesh: &mut TriangleMesh,
) {
    for &face_id in &shell.faces {
        if let Some(face) = model.faces.get(face_id) {
            let tri_start = mesh.triangles.len();
            surface::tessellate_face(face, model, params, mesh);
            let tri_end = mesh.triangles.len();
            // Record which B-Rep face each new triangle came from
            for _ in tri_start..tri_end {
                mesh.face_map.push(face_id);
            }
        }
    }
}
