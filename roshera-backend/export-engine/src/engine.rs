//! Main export engine implementation

use crate::formats::ros::{
    export_brep_to_ros, import_ros_to_brep, RosExportOptions, RosExportPayload,
};
use crate::formats::step::{
    export_brep_to_step, import_step_text_with_report, import_step_to_brep, ImportReport,
};
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

        // Export to .ros v3.1. The engine-level wrapper writes an empty
        // HIST + PROV manifest; richer callers that own a Timeline /
        // AICommandTracker should call `export_brep_to_ros` directly
        // with a populated `RosExportPayload`.
        let payload = RosExportPayload {
            model,
            history: None,
            aipr: None,
        };
        export_brep_to_ros(payload, &filepath, options).await?;

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

    /// Import a STEP exchange structure supplied inline (no file I/O),
    /// returning the reconstructed [`BRepModel`] and the structured
    /// [`ImportReport`] (validity verdict, per-entity coverage counts,
    /// warnings). This is the entry point the agent/REST import path
    /// uses: the client posts STEP content, the engine reconstructs the
    /// B-Rep, and the caller splices the resulting solids into the live
    /// session model via
    /// [`crate::formats::step::merge_solids_into`].
    pub fn import_step_content(
        &self,
        content: &str,
        source_hint: &str,
    ) -> Result<(BRepModel, ImportReport), ExportError> {
        import_step_text_with_report(content, source_hint)
    }
}

impl Default for ExportEngine {
    fn default() -> Self {
        Self::new()
    }
}
