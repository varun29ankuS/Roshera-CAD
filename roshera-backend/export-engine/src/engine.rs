//! Main export engine implementation

use crate::formats::ros::{
    export_brep_to_ros, HistData, RosExportOptions, RosExportPayload, RosFileVerification,
    RosImport, RosWriteSummary,
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

    /// Export a B-Rep model — plus its timeline history and AI
    /// provenance — to .ros v3.1.
    ///
    /// HIST and PROV are MANDATORY chunks, so this single entry point
    /// takes them explicitly: a caller that genuinely has no timeline /
    /// tracker passes `None` and the file records that emptiness
    /// (`HistChunk::empty()` / `ProvChunk::empty()`) as a statement of
    /// fact. There is deliberately no history-less convenience wrapper —
    /// a second path that silently wrote empty mandatory chunks is
    /// exactly the defect this signature closed.
    ///
    /// Returns the written filename plus a [`RosWriteSummary`] stating
    /// what the mandatory chunks actually carry, so the caller's
    /// response can report it.
    pub async fn export_ros(
        &self,
        model: &BRepModel,
        name: &str,
        history: Option<HistData>,
        aipr: Option<ros_format::AICommandTracker>,
        options: RosExportOptions,
    ) -> Result<(String, RosWriteSummary), ExportError> {
        // Ensure output directory exists
        fs::create_dir_all(&self.output_dir)
            .await
            .map_err(|_e| ExportError::FileWriteError {
                path: self.output_dir.to_string_lossy().to_string(),
            })?;

        let filename = format!("{}.ros", name);
        let filepath = self.output_dir.join(&filename);

        let payload = RosExportPayload {
            model,
            history,
            aipr,
        };
        let summary = export_brep_to_ros(payload, &filepath, options).await?;

        Ok((filename, summary))
    }

    /// Import a `.ros` file from the export directory, returning the
    /// FULL structured [`RosImport`] — timeline events, branch
    /// manifests, PROV chunk, optional GEOM snapshot. Callers
    /// materialise geometry via [`RosImport::into_model`] and are
    /// expected to REPORT the history/provenance counts to their own
    /// caller rather than silently dropping the mandatory chunks.
    pub async fn import_ros(
        &self,
        filename: &str,
        password: Option<&str>,
    ) -> Result<RosImport, ExportError> {
        let filepath = self.output_dir.join(filename);

        crate::formats::ros::import_ros(&filepath, password).await
    }

    /// Verify a `.ros` file in the export directory WITHOUT its password.
    ///
    /// Returns the signature verdict, the header facts and the chunk
    /// inventory; never any chunk contents. The v3.2 signature covers
    /// the post-encryption on-disk bytes and SIGN is never encrypted
    /// precisely so an encrypted artifact can be checked for integrity
    /// and authorship by someone who cannot read it — see
    /// [`crate::formats::ros::verify_ros_file`] for exactly what a caller
    /// does and does not learn.
    pub async fn verify_ros(&self, filename: &str) -> Result<RosFileVerification, ExportError> {
        let filepath = self.output_dir.join(filename);

        crate::formats::ros::verify_ros_file(&filepath).await
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
