//! Application setup and configuration

use crate::state::{AppState, AppMetrics};
use geometry_engine::GeometryEngine;
use session_manager::{SessionManager, CollaborationManager, BroadcastManager, BroadcastConfig};
use ai_integration::AIProcessor;
use export_engine::ExportEngine;
use std::sync::Arc;
use parking_lot::RwLock;

impl AppState {
    /// Create new application state
    pub async fn new() -> Self {
        // Initialize broadcast manager
        let broadcast_config = BroadcastConfig {
            capacity: 1000,
            compress_messages: false,
            max_message_size: 1024 * 1024, // 1MB
            retention_seconds: 3600, // 1 hour
        };
        let broadcast_manager = BroadcastManager::with_config(broadcast_config);
        
        // Initialize engines and managers
        let geometry_engine = GeometryEngine::new();
        let session_manager = SessionManager::new(broadcast_manager.clone());
        let collaboration_manager = CollaborationManager::new();
        let ai_processor = AIProcessor::new();
        
        // Configure export engine
        let export_dir = std::env::var("EXPORT_DIRECTORY")
            .unwrap_or_else(|_| "./exports".to_string());
        let export_engine = ExportEngine::with_output_directory(export_dir);
        
        Self {
            geometry_engine,
            session_manager,
            collaboration_manager,
            ai_processor,
            export_engine,
            broadcast_manager,
            metrics: Arc::new(RwLock::new(AppMetrics::default())),
        }
    }
    
    /// Get application metrics
    pub fn metrics(&self) -> AppMetrics {
        self.metrics.read().clone()
    }
    
    /// Update request metrics
    pub fn record_request(&self, endpoint: &str, duration_ms: u64) {
        let mut metrics = self.metrics.write();
        metrics.total_requests += 1;
        metrics.endpoint_requests
            .entry(endpoint.to_string())
            .and_modify(|c| *c += 1)
            .or_insert(1);
        
        // Update average response time
        let total_time = metrics.avg_response_time_ms * (metrics.total_requests - 1) as f64;
        metrics.avg_response_time_ms = (total_time + duration_ms as f64) / metrics.total_requests as f64;
    }
}