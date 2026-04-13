//! Demo of delta updates for efficient real-time collaboration

use session_manager::{BroadcastConfig, BroadcastManager, SessionManager};
use shared_types::{AICommand, ObjectId, PrimitiveType, ShapeParameters};
use tokio;
use tracing::{info, Level};
use tracing_subscriber;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt().with_max_level(Level::INFO).init();

    // Create broadcast manager with delta updates enabled
    let mut config = BroadcastConfig::default();
    config.use_delta_updates = true;
    config.compress_messages = true;
    config.batch_deltas = true;
    config.delta_batch_window_ms = 100; // 100ms batching

    let broadcast_manager = BroadcastManager::with_config(config);

    // Create session manager
    let session_manager = SessionManager::new(broadcast_manager.clone());

    // Create a session
    let session_id = session_manager
        .create_session("Demo User".to_string())
        .await;
    info!("Created session: {}", session_id);

    // Subscribe to broadcast updates
    let session_uuid: ObjectId = session_id.parse().expect("Invalid session ID");
    let mut receiver = broadcast_manager.create_session_channel(session_uuid);

    // Spawn a task to receive and log delta updates
    let receiver_task = tokio::spawn(async move {
        while let Ok(msg) = receiver.recv().await {
            match msg {
                session_manager::BroadcastMessage::DeltaUpdate { delta, .. } => {
                    info!("Received delta update:");
                    info!("  - Sequence: {}", delta.sequence);
                    info!("  - Object changes: {}", delta.object_deltas.len());
                    info!("  - Timeline changes: {}", delta.timeline_delta.is_some());
                    info!("  - User changes: {}", delta.user_changes.is_some());
                }
                session_manager::BroadcastMessage::CompressedDeltaUpdate {
                    data,
                    original_size,
                    ..
                } => {
                    let compression_ratio =
                        100.0 - (data.len() as f64 / original_size as f64 * 100.0);
                    info!("Received compressed delta:");
                    info!("  - Compressed size: {} bytes", data.len());
                    info!("  - Original size: {} bytes", original_size);
                    info!("  - Compression ratio: {:.1}%", compression_ratio);
                }
                _ => {}
            }
        }
    });

    // Simulate collaborative editing with multiple operations

    // Operation 1: Create a box
    info!("\nOperation 1: Creating a box...");
    let create_box = AICommand::CreatePrimitive {
        shape_type: PrimitiveType::Box,
        parameters: ShapeParameters::box_params(10.0, 10.0, 10.0),
        position: [0.0, 0.0, 0.0],
        material: Some("steel".to_string()),
    };
    let result = session_manager
        .process_command(&session_id.to_string(), create_box, "user1")
        .await?;
    let box_id = result
        .objects_affected
        .first()
        .copied()
        .expect("Box should have been created");

    // Small delay to show batching
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Operation 2: Create a sphere
    info!("\nOperation 2: Creating a sphere...");
    let create_sphere = AICommand::CreatePrimitive {
        shape_type: PrimitiveType::Sphere,
        parameters: ShapeParameters::sphere_params(5.0),
        position: [5.0, 0.0, 0.0],
        material: Some("aluminum".to_string()),
    };
    let result = session_manager
        .process_command(&session_id.to_string(), create_sphere, "user1")
        .await?;
    let sphere_id = result
        .objects_affected
        .first()
        .copied()
        .expect("Sphere should have been created");

    // Operation 3: Transform the box (within batch window)
    info!("\nOperation 3: Transforming the box...");
    let transform = AICommand::Transform {
        object_id: box_id,
        transform_type: shared_types::TransformType::Translate {
            offset: [10.0, 0.0, 0.0],
        },
    };
    session_manager
        .process_command(&session_id.to_string(), transform, "user1")
        .await?;

    // Wait for batch window to complete
    tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

    // Operation 4: Simulate a new client joining by creating a new session
    info!("\nSimulating new client joining...");
    let new_session_id = session_manager
        .create_session("New Client User".to_string())
        .await;
    info!("New client session created: {}", new_session_id);

    // Operation 5: Boolean operation
    info!("\nOperation 5: Boolean union...");
    let boolean_op = AICommand::BooleanOperation {
        operation: shared_types::BooleanOp::Union,
        target_objects: vec![box_id, sphere_id],
        keep_originals: false,
    };
    let result = session_manager
        .process_command(&session_id.to_string(), boolean_op, "user1")
        .await?;
    let union_id = result
        .objects_affected
        .first()
        .copied()
        .expect("Union should have been created");

    // Wait for final batch
    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

    // Operation 6: Export the result
    info!("\nOperation 6: Exporting the result...");
    let export_cmd = AICommand::Export {
        format: shared_types::commands::ExportFormat::STL,
        objects: vec![union_id],
        options: shared_types::commands::ExportOptions::default(),
    };
    session_manager
        .process_command(&session_id.to_string(), export_cmd, "user1")
        .await?;

    // Wait a bit for all messages to be processed
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    info!("\nDemo completed! Check the logs above to see how delta updates were sent.");
    info!("Session statistics:");
    match session_manager.get_session(&session_id).await {
        Ok(session) => {
            let session_lock = session.read().await;
            info!("  - Total objects: {}", session_lock.objects.len());
            info!("  - History entries: {}", session_lock.history.len());
        }
        Err(e) => {
            info!("  - Error getting session: {}", e);
        }
    }

    // Clean shutdown
    receiver_task.abort();
    Ok(())
}
