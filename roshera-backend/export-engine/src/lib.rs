//! Export engine for various CAD file formats
//!
//! Supports STL, OBJ, and other common formats.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod engine;
pub mod formats;
pub mod ros_fs;
pub mod validation;

pub use engine::*;
pub use validation::*;

use shared_types::*;

/// Export assembly to various formats
pub async fn export_assembly(
    assembly: &geometry_engine::assembly::Assembly,
    format: ExportFormat,
    path: &std::path::Path,
) -> Result<(), ExportError> {
    match format {
        ExportFormat::STEP => formats::step::export_assembly_to_step(assembly, path).await,
        _ => Err(ExportError::UnsupportedFormat {
            format: format!(
                "Assembly export only supports STEP format. {} is not supported for assemblies",
                format
            ),
        }),
    }
}

/// Estimate export time for a mesh
pub fn estimate_export_time(mesh: &Mesh, format: &ExportFormat) -> u64 {
    let base_time = match format {
        ExportFormat::STL => 10,
        ExportFormat::OBJ => 20,
        ExportFormat::ROS => 30, // ROS format includes encryption and AI tracking overhead
        ExportFormat::STEP => 40, // STEP format includes complex B-Rep conversion
        _ => 50,
    };

    let triangle_factor = (mesh.triangle_count() as f64 / 1000.0).max(1.0);
    (base_time as f64 * triangle_factor) as u64
}
