//! Integration tests for ROS file format export/import

use export_engine::formats::ros::RosExportOptions;
use export_engine::ExportEngine;
use geometry_engine::primitives::topology_builder::BRepModel;
use shared_types::*;
use tempfile::TempDir;

#[tokio::test]
async fn test_ros_export_basic() {
    // Create temp directory for exports
    let temp_dir = TempDir::new().unwrap();
    let export_engine =
        ExportEngine::with_output_directory(temp_dir.path().to_string_lossy().to_string());

    // Create an empty model for now
    let model = BRepModel::new();

    // Export with default options
    let options = RosExportOptions::default();
    let filename = export_engine
        .export_ros(&model, "test_model", options)
        .await
        .expect("Export should succeed");

    assert_eq!(filename, "test_model.ros");

    // Verify file was created
    let file_path = temp_dir.path().join(&filename);
    assert!(file_path.exists());
}

#[tokio::test]
async fn test_ros_import_nonexistent_file() {
    let temp_dir = TempDir::new().unwrap();
    let export_engine =
        ExportEngine::with_output_directory(temp_dir.path().to_string_lossy().to_string());

    // Try to import a file that doesn't exist
    let result = export_engine.import_ros("nonexistent.ros", None).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_ros_export_with_encryption() {
    let temp_dir = TempDir::new().unwrap();
    let export_engine =
        ExportEngine::with_output_directory(temp_dir.path().to_string_lossy().to_string());

    let model = BRepModel::new();

    // Export with encryption
    let mut options = RosExportOptions::default();
    options.encrypt = true;
    options.password = Some("test_password_123".to_string());

    let filename = export_engine
        .export_ros(&model, "encrypted_model", options)
        .await
        .expect("Encrypted export should succeed");

    assert_eq!(filename, "encrypted_model.ros");

    // Verify file was created
    let file_path = temp_dir.path().join(&filename);
    assert!(file_path.exists());
}

#[tokio::test]
async fn test_ros_export_with_ai_tracking() {
    let temp_dir = TempDir::new().unwrap();
    let export_engine =
        ExportEngine::with_output_directory(temp_dir.path().to_string_lossy().to_string());

    let model = BRepModel::new();

    // Export with AI tracking enabled
    let mut options = RosExportOptions::default();
    options.track_ai = true;
    options.ai_tracking_level = 2; // Forensic level
    options.author = "Test User".to_string();

    let filename = export_engine
        .export_ros(&model, "ai_tracked_model", options)
        .await
        .expect("Export with AI tracking should succeed");

    assert_eq!(filename, "ai_tracked_model.ros");
}

#[tokio::test]
async fn test_ros_export_roundtrip() {
    let temp_dir = TempDir::new().unwrap();
    let export_engine =
        ExportEngine::with_output_directory(temp_dir.path().to_string_lossy().to_string());

    // Create a model with some vertices
    let mut model = BRepModel::new();

    // Add a few vertices
    model.vertices.add(0.0, 0.0, 0.0);
    model.vertices.add(1.0, 0.0, 0.0);
    model.vertices.add(0.0, 1.0, 0.0);

    // Export
    let options = RosExportOptions::default();
    let filename = export_engine
        .export_ros(&model, "roundtrip_test", options)
        .await
        .expect("Export should succeed");

    // Import back
    let imported_model = export_engine
        .import_ros(&filename, None)
        .await
        .expect("Import should succeed");

    // Verify vertices count matches
    assert_eq!(imported_model.vertices.len(), model.vertices.len());
}
