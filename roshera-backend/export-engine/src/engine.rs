//! Main export engine implementation

use crate::formats::ros::{export_brep_to_ros, import_ros_to_brep, RosExportOptions};
use crate::formats::step::{export_brep_to_step, import_step_to_brep};
use geometry_engine::primitives::topology_builder::BRepModel;
use shared_types::*;
use std::path::PathBuf;
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// Export engine for generating CAD files
#[derive(Clone)]
pub struct ExportEngine {
    /// Output directory
    output_dir: PathBuf,
}

impl ExportEngine {
    /// Create new export engine
    pub fn new() -> Self {
        Self {
            output_dir: PathBuf::from("./exports"),
        }
    }

    /// Create with custom output directory
    pub fn with_output_directory(dir: String) -> Self {
        Self {
            output_dir: PathBuf::from(dir),
        }
    }

    /// Export mesh to STL format
    pub async fn export_stl(&self, mesh: &Mesh, name: &str) -> Result<String, ExportError> {
        // Ensure output directory exists
        fs::create_dir_all(&self.output_dir)
            .await
            .map_err(|_e| ExportError::FileWriteError {
                path: self.output_dir.to_string_lossy().to_string(),
            })?;

        let filename = format!("{}.stl", name);
        let filepath = self.output_dir.join(&filename);

        // Generate STL binary content
        let content = crate::formats::stl::generate_binary_stl(mesh, name)?;

        // Write to file
        let mut file =
            fs::File::create(&filepath)
                .await
                .map_err(|_e| ExportError::FileWriteError {
                    path: filepath.to_string_lossy().to_string(),
                })?;

        file.write_all(&content)
            .await
            .map_err(|_e| ExportError::FileWriteError {
                path: filepath.to_string_lossy().to_string(),
            })?;

        Ok(filename)
    }

    /// Export mesh to OBJ format
    pub async fn export_obj(&self, mesh: &Mesh, name: &str) -> Result<String, ExportError> {
        // Ensure output directory exists
        fs::create_dir_all(&self.output_dir)
            .await
            .map_err(|_e| ExportError::FileWriteError {
                path: self.output_dir.to_string_lossy().to_string(),
            })?;

        let filename = format!("{}.obj", name);
        let filepath = self.output_dir.join(&filename);

        // Generate OBJ content
        let content = crate::formats::obj::generate_obj(mesh, name)?;

        // Write to file
        fs::write(&filepath, content)
            .await
            .map_err(|_e| ExportError::FileWriteError {
                path: filepath.to_string_lossy().to_string(),
            })?;

        Ok(filename)
    }

    /// Export B-Rep model to ROS format with encryption and AI tracking
    pub async fn export_ros(
        &self,
        model: &BRepModel,
        name: &str,
        options: RosExportOptions,
    ) -> Result<String, ExportError> {
        // Ensure output directory exists
        fs::create_dir_all(&self.output_dir)
            .await
            .map_err(|_e| ExportError::FileWriteError {
                path: self.output_dir.to_string_lossy().to_string(),
            })?;

        let filename = format!("{}.ros", name);
        let filepath = self.output_dir.join(&filename);

        // Export to ROS format
        export_brep_to_ros(model, &filepath, options).await?;

        Ok(filename)
    }

    /// Import B-Rep model from ROS format
    pub async fn import_ros(
        &self,
        filename: &str,
        password: Option<&str>,
    ) -> Result<BRepModel, ExportError> {
        let filepath = self.output_dir.join(filename);

        // Import from ROS format
        import_ros_to_brep(&filepath, password).await
    }

    /// Export B-Rep model to STEP format
    pub async fn export_step(&self, model: &BRepModel, name: &str) -> Result<String, ExportError> {
        // Ensure output directory exists
        fs::create_dir_all(&self.output_dir)
            .await
            .map_err(|_e| ExportError::FileWriteError {
                path: self.output_dir.to_string_lossy().to_string(),
            })?;

        let filename = format!("{}.step", name);
        let filepath = self.output_dir.join(&filename);

        // Export to STEP format
        export_brep_to_step(model, &filepath).await?;

        Ok(filename)
    }

    /// Import B-Rep model from STEP format
    pub async fn import_step(&self, filename: &str) -> Result<BRepModel, ExportError> {
        let filepath = self.output_dir.join(filename);

        // Import from STEP format
        import_step_to_brep(&filepath).await
    }
}

impl Default for ExportEngine {
    fn default() -> Self {
        Self::new()
    }
}
