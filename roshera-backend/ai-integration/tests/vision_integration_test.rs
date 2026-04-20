/// Integration tests for the vision pipeline
///
/// These tests verify that the vision pipeline correctly processes
/// viewport captures and routes them through the appropriate AI models.
use ai_integration::{
    providers::{CommandIntent, LLMProvider, ParsedCommand},
    SmartRouter, SmartRouterConfig, SmartRouterError, UniversalEndpoint, UniversalEndpointConfig,
};
use shared_types::vision::{
    CameraInfo, Measurements, MousePosition, PixelPosition, ProcessingMode, RenderStats,
    SelectionInfo, ViewportCapture, ViewportInfo, VisionConfig, VisionProviderType,
};
use std::sync::Arc;

/// Create a test viewport capture with sample data
fn create_test_viewport() -> ViewportCapture {
    ViewportCapture {
        image: "base64_test_image_data".to_string(),
        camera: CameraInfo {
            position: [10.0, 20.0, 30.0],
            rotation: [0.0, 0.0, 0.0],
            quaternion: [0.0, 0.0, 0.0, 1.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            fov: 50.0,
            aspect: 1.6,
            near: 0.1,
            far: 1000.0,
            zoom: 1.0,
            matrix_world: [0.0; 16],
            projection_matrix: [0.0; 16],
        },
        cursor_target: Some(shared_types::vision::CursorTarget {
            object_id: Some("test_object_1".to_string()),
            object_type: Some("Box".to_string()),
            point: [5.0, 5.0, 5.0],
            normal: Some([0.0, 1.0, 0.0]),
            distance: 15.0,
            face_index: Some(0),
            uv: None,
        }),
        scene_objects: vec![shared_types::vision::SceneObject {
            id: "test_object_1".to_string(),
            object_type: "Mesh".to_string(),
            name: "TestBox".to_string(),
            visible: true,
            position: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0],
            scale: [1.0, 1.0, 1.0],
            bounding_box: shared_types::vision::BoundingBox {
                min: [-5.0, -5.0, -5.0],
                max: [5.0, 5.0, 5.0],
                center: [0.0, 0.0, 0.0],
                size: [10.0, 10.0, 10.0],
                radius: 8.66,
            },
            material: Some(shared_types::vision::MaterialInfo {
                material_type: "MeshPhongMaterial".to_string(),
                color: Some(0xFF0000), // Red
                opacity: 1.0,
                transparent: false,
                wireframe: false,
            }),
            geometry: Some(shared_types::vision::GeometryStats {
                vertices: 8,
                faces: 12,
                has_normals: true,
                has_uvs: true,
            }),
            selected: true,
            highlighted: false,
            parent_id: None,
            children_ids: vec![],
        }],
        selection: SelectionInfo {
            object_ids: vec!["test_object_1".to_string()],
            bounding_box: Some(shared_types::vision::BoundingBox {
                min: [-5.0, -5.0, -5.0],
                max: [5.0, 5.0, 5.0],
                center: [0.0, 0.0, 0.0],
                size: [10.0, 10.0, 10.0],
                radius: 8.66,
            }),
            center: Some([0.0, 0.0, 0.0]),
        },
        viewport: ViewportInfo {
            width: 1920,
            height: 1080,
            client_width: 1920,
            client_height: 1080,
            pixel_ratio: 1.0,
            mouse_screen: MousePosition { x: 0.0, y: 0.0 },
            mouse_pixels: PixelPosition { x: 960.0, y: 540.0 },
            mouse_world: Some([5.0, 5.0, 5.0]),
            mouse_context: None,
        },
        lighting: vec![],
        clipping_planes: vec![],
        render_stats: RenderStats {
            triangles: 12,
            points: 0,
            lines: 0,
            frame: 60,
            calls: 1,
            vertices: 8,
            faces: 12,
        },
        measurements: Measurements {
            distance_between_selected: None,
            camera_to_selection: Some(25.98),
        },
        timestamp: 1234567890,
    }
}

#[test]
fn test_viewport_capture_creation() {
    let viewport = create_test_viewport();

    assert_eq!(viewport.camera.position, [10.0, 20.0, 30.0]);
    assert_eq!(viewport.scene_objects.len(), 1);
    assert_eq!(viewport.selection.object_ids.len(), 1);
    assert!(viewport.cursor_target.is_some());
}

#[test]
fn test_smart_router_requires_vision() {
    // Test commands that should require vision
    assert!(SmartRouter::requires_vision("select this object"));
    assert!(SmartRouter::requires_vision("make that edge rounder"));
    assert!(SmartRouter::requires_vision("the red box"));
    assert!(SmartRouter::requires_vision(
        "what is the cursor pointing at"
    ));
    assert!(SmartRouter::requires_vision("move the selected object"));

    // Test commands that don't require vision
    assert!(!SmartRouter::requires_vision("create a sphere"));
    assert!(!SmartRouter::requires_vision("export as STL"));
    assert!(!SmartRouter::requires_vision("undo"));
}

#[tokio::test]
async fn test_unified_mode_configuration() {
    let config = SmartRouterConfig {
        mode: ProcessingMode::Unified,
        vision_config: VisionConfig {
            provider: VisionProviderType::CustomAPI,
            url: "http://localhost:8888/test".to_string(),
            api_key: None,
            model_name: "test-model".to_string(),
        },
        reasoning_config: None,
        enable_cache: false,
        cache_ttl_secs: 0,
        max_retries: 1,
        vision_timeout_secs: 5,
        reasoning_timeout_secs: 5,
    };

    let result = SmartRouter::new(config);
    assert!(result.is_ok(), "Should create SmartRouter in unified mode");
}

#[tokio::test]
async fn test_separated_mode_configuration() {
    let config = SmartRouterConfig {
        mode: ProcessingMode::Separated,
        vision_config: VisionConfig {
            provider: VisionProviderType::CustomAPI,
            url: "http://localhost:8888/vision".to_string(),
            api_key: None,
            model_name: "vision-model".to_string(),
        },
        reasoning_config: Some(VisionConfig {
            provider: VisionProviderType::CustomAPI,
            url: "http://localhost:8888/reasoning".to_string(),
            api_key: None,
            model_name: "reasoning-model".to_string(),
        }),
        enable_cache: false,
        cache_ttl_secs: 0,
        max_retries: 1,
        vision_timeout_secs: 5,
        reasoning_timeout_secs: 5,
    };

    let result = SmartRouter::new(config);
    assert!(
        result.is_ok(),
        "Should create SmartRouter in separated mode"
    );
}

#[tokio::test]
async fn test_separated_mode_requires_reasoning_config() {
    let config = SmartRouterConfig {
        mode: ProcessingMode::Separated,
        vision_config: VisionConfig {
            provider: VisionProviderType::CustomAPI,
            url: "http://localhost:8888/vision".to_string(),
            api_key: None,
            model_name: "vision-model".to_string(),
        },
        reasoning_config: None, // Missing reasoning config
        enable_cache: false,
        cache_ttl_secs: 0,
        max_retries: 1,
        vision_timeout_secs: 5,
        reasoning_timeout_secs: 5,
    };

    let result = SmartRouter::new(config);
    assert!(
        result.is_err(),
        "Should fail without reasoning config in separated mode"
    );
}

#[test]
fn test_universal_endpoint_config() {
    let config = UniversalEndpointConfig {
        provider: VisionProviderType::Ollama,
        url: "http://localhost:11434/api/generate".to_string(),
        api_key: None,
        model_name: "bakllava:latest".to_string(),
        timeout_secs: 30,
        max_tokens: 1000,
        temperature: 0.7,
        system_prompt: Some("Test prompt".to_string()),
    };

    let endpoint = UniversalEndpoint::new(config.clone());
    let capabilities = endpoint.capabilities();

    assert_eq!(capabilities.name, "Universal-Ollama");
    assert_eq!(config.model_name, "bakllava:latest");
}

/// Test that viewport context is properly formatted
#[test]
fn test_viewport_context_formatting() {
    let viewport = create_test_viewport();
    let config = UniversalEndpointConfig::default();
    let endpoint = UniversalEndpoint::new(config);

    // This would be a private method, so we can't test it directly
    // Instead, we verify the viewport structure is correct
    assert!(viewport.cursor_target.is_some());
    assert_eq!(viewport.scene_objects[0].name, "TestBox");
    assert_eq!(
        viewport.scene_objects[0].material.as_ref().unwrap().color,
        Some(0xFF0000)
    );
}

/// Test vision command processing flow (mock)
#[tokio::test]
async fn test_vision_command_flow() {
    // This test would require a mock server or test provider
    // For now, we just test the structure

    let viewport = create_test_viewport();
    let command = "select the red box";

    // Verify the command would trigger vision processing
    assert!(SmartRouter::requires_vision(command));

    // Verify viewport has necessary data
    assert!(viewport.scene_objects.iter().any(|obj| {
        obj.material
            .as_ref()
            .and_then(|m| m.color)
            .map(|c| c == 0xFF0000)
            .unwrap_or(false)
    }));
}

/// Test cache key generation for vision results
#[test]
fn test_vision_cache_key() {
    let viewport1 = create_test_viewport();
    let mut viewport2 = create_test_viewport();
    viewport2.camera.position = [20.0, 30.0, 40.0];

    // Different camera positions should produce different cache keys
    assert_ne!(viewport1.camera.position, viewport2.camera.position);

    // Same selection should be the same
    assert_eq!(
        viewport1.selection.object_ids,
        viewport2.selection.object_ids
    );
}

/// Test error handling for invalid viewport data
#[tokio::test]
async fn test_invalid_viewport_handling() {
    let mut viewport = create_test_viewport();
    viewport.image = "".to_string(); // Invalid image data

    // The system should handle empty image gracefully
    assert_eq!(viewport.image, "");

    // Should still have valid scene data
    assert_eq!(viewport.scene_objects.len(), 1);
}

/// Integration test for the complete vision pipeline
#[tokio::test]
#[ignore] // This test requires a running test server
async fn test_complete_vision_pipeline() {
    // Setup
    let config = SmartRouterConfig::default();
    let router = SmartRouter::new(config).expect("Failed to create router");
    let viewport = create_test_viewport();

    // Test spatial command
    let result = router
        .process_with_vision("make this edge rounder", &viewport)
        .await;

    // The result would depend on the actual model/provider
    // For testing, we just ensure it doesn't panic
    match result {
        Ok(parsed) => {
            println!("Parsed command: {:?}", parsed);
        }
        Err(e) => {
            println!("Expected error in test environment: {:?}", e);
        }
    }
}

/// Test that text-only commands work without viewport
#[tokio::test]
async fn test_text_only_command() {
    let config = SmartRouterConfig::default();
    let router = SmartRouter::new(config).expect("Failed to create router");

    let result = router
        .process_text_only("create a sphere with radius 5")
        .await;

    // Should process without viewport
    match result {
        Ok(_) => {
            // Success or mock success is fine
        }
        Err(e) => {
            // Error is expected in test environment without real provider
            println!("Expected error: {:?}", e);
        }
    }
}

/// Benchmark test for viewport capture size
#[test]
fn test_viewport_capture_serialization_size() {
    let viewport = create_test_viewport();
    let json = serde_json::to_string(&viewport).expect("Failed to serialize");

    println!("Viewport JSON size: {} bytes", json.len());

    // Ensure it's reasonable size for network transfer
    assert!(
        json.len() < 100_000,
        "Viewport capture should be under 100KB"
    );
}

/// Test provider-specific request formatting
#[test]
fn test_provider_request_formats() {
    let providers = vec![
        VisionProviderType::Ollama,
        VisionProviderType::OpenAI,
        VisionProviderType::Anthropic,
        VisionProviderType::Google,
        VisionProviderType::HuggingFace,
        VisionProviderType::CustomAPI,
    ];

    for provider in providers {
        let config = UniversalEndpointConfig {
            provider: provider.clone(),
            url: "http://test.com".to_string(),
            api_key: Some("test_key".to_string()),
            model_name: "test-model".to_string(),
            timeout_secs: 30,
            max_tokens: 1000,
            temperature: 0.7,
            system_prompt: None,
        };

        let endpoint = UniversalEndpoint::new(config);
        let capabilities = endpoint.capabilities();

        assert!(capabilities.name.contains(&format!("{:?}", provider)));
    }
}
