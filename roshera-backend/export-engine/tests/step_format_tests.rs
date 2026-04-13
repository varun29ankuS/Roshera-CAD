//! Integration tests for STEP file format export

use export_engine::ExportEngine;
use geometry_engine::primitives::topology_builder::BRepModel;
use tempfile::TempDir;

#[tokio::test]
async fn test_step_export_basic() {
    // Create temp directory for exports
    let temp_dir = TempDir::new().unwrap();
    let export_engine =
        ExportEngine::with_output_directory(temp_dir.path().to_string_lossy().to_string());

    // Create an empty model
    let model = BRepModel::new();

    // Export to STEP
    let filename = export_engine
        .export_step(&model, "test_model")
        .await
        .expect("Export should succeed");

    assert_eq!(filename, "test_model.step");

    // Verify file was created
    let file_path = temp_dir.path().join(&filename);
    assert!(file_path.exists());
}

#[tokio::test]
async fn test_step_export_with_vertices() {
    let temp_dir = TempDir::new().unwrap();
    let export_engine =
        ExportEngine::with_output_directory(temp_dir.path().to_string_lossy().to_string());

    // Create a model with vertices
    let mut model = BRepModel::new();

    // Add some vertices
    model.vertices.add(0.0, 0.0, 0.0);
    model.vertices.add(10.0, 0.0, 0.0);
    model.vertices.add(10.0, 10.0, 0.0);
    model.vertices.add(0.0, 10.0, 0.0);

    // Export to STEP
    let filename = export_engine
        .export_step(&model, "vertices_model")
        .await
        .expect("Export should succeed");

    assert_eq!(filename, "vertices_model.step");

    // Verify file exists and has content
    let file_path = temp_dir.path().join(&filename);
    assert!(file_path.exists());

    // Read and verify it contains STEP header
    let content = std::fs::read_to_string(&file_path).expect("Should read file");
    assert!(content.contains("ISO-10303-21"));
    assert!(content.contains("CARTESIAN_POINT"));
}

#[tokio::test]
async fn test_step_export_with_edges() {
    let temp_dir = TempDir::new().unwrap();
    let export_engine =
        ExportEngine::with_output_directory(temp_dir.path().to_string_lossy().to_string());

    // Create a model with vertices
    let mut model = BRepModel::new();

    // Add vertices
    let _v1 = model.vertices.add(0.0, 0.0, 0.0);
    let _v2 = model.vertices.add(100.0, 0.0, 0.0);

    // For now, just test basic export with vertices
    // TODO: Add edge support when API is clarified

    // Export to STEP
    let filename = export_engine
        .export_step(&model, "edges_model")
        .await
        .expect("Export should succeed");

    // Verify file was created
    let file_path = temp_dir.path().join(&filename);
    assert!(file_path.exists());

    // Verify basic STEP content
    let content = std::fs::read_to_string(&file_path).expect("Should read file");
    assert!(content.contains("ISO-10303-21"));
    assert!(content.contains("CARTESIAN_POINT"));
}

#[tokio::test]
async fn test_step_export_large_model() {
    let temp_dir = TempDir::new().unwrap();
    let export_engine =
        ExportEngine::with_output_directory(temp_dir.path().to_string_lossy().to_string());

    // Create a model with many vertices
    let mut model = BRepModel::new();

    // Add 1000 vertices in a grid pattern
    for i in 0..100 {
        for j in 0..10 {
            let x = i as f64 * 10.0;
            let y = j as f64 * 10.0;
            let z = 0.0;
            model.vertices.add(x, y, z);
        }
    }

    // Export to STEP
    let filename = export_engine
        .export_step(&model, "large_model")
        .await
        .expect("Export should succeed");

    // Verify file was created
    let file_path = temp_dir.path().join(&filename);
    assert!(file_path.exists());

    // Check file size is reasonable
    let metadata = std::fs::metadata(&file_path).expect("Should get metadata");
    assert!(metadata.len() > 1000); // Should have substantial content
}

#[tokio::test]
async fn test_step_export_with_faces() {
    let temp_dir = TempDir::new().unwrap();
    let export_engine =
        ExportEngine::with_output_directory(temp_dir.path().to_string_lossy().to_string());

    // Create a model with vertices for a square
    let mut model = BRepModel::new();

    // Add vertices for a square
    let _v0 = model.vertices.add(0.0, 0.0, 0.0);
    let _v1 = model.vertices.add(100.0, 0.0, 0.0);
    let _v2 = model.vertices.add(100.0, 100.0, 0.0);
    let _v3 = model.vertices.add(0.0, 100.0, 0.0);

    // For now, just test basic export
    // TODO: Add face support when API is clarified

    // Export to STEP
    let filename = export_engine
        .export_step(&model, "face_model")
        .await
        .expect("Export should succeed");

    // Verify file was created and has basic content
    let file_path = temp_dir.path().join(&filename);
    let content = std::fs::read_to_string(&file_path).expect("Should read file");
    assert!(content.contains("ISO-10303-21"));
    assert!(content.contains("CARTESIAN_POINT"));
}

#[tokio::test]
async fn test_step_export_special_characters_filename() {
    let temp_dir = TempDir::new().unwrap();
    let export_engine =
        ExportEngine::with_output_directory(temp_dir.path().to_string_lossy().to_string());

    let model = BRepModel::new();

    // Test with underscores and numbers
    let filename = export_engine
        .export_step(&model, "test_model_v2_final")
        .await
        .expect("Export should succeed");

    assert_eq!(filename, "test_model_v2_final.step");

    let file_path = temp_dir.path().join(&filename);
    assert!(file_path.exists());
}
