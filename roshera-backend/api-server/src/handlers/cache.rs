//! Cache management handlers for monitoring and control

use crate::{auth_middleware::AuthInfo, AppState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Json, Result},
};
use serde::{Deserialize, Serialize};
use session_manager::CacheManager;
use shared_types::GeometryId;
use tracing::{info, warn};

#[derive(Debug, Serialize)]
pub struct CacheStats {
    pub sessions: CacheLayerStats,
    pub objects: CacheLayerStats,
    pub permissions: CacheLayerStats,
    pub command_results: CacheLayerStats,
    pub computed_geometry: CacheLayerStats,
    pub total_memory_mb: f64,
    pub hit_rate_percentage: f64,
}

#[derive(Debug, Serialize)]
pub struct CacheLayerStats {
    pub entries: usize,
    pub capacity: usize,
    pub hit_count: u64,
    pub miss_count: u64,
    pub hit_rate: f64,
    pub memory_usage_bytes: usize,
}

#[derive(Debug, Deserialize)]
pub struct CacheQuery {
    pub include_details: Option<bool>,
    pub layer: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ClearCacheRequest {
    pub layers: Option<Vec<String>>,
    pub confirm: bool,
}

#[derive(Debug, Serialize)]
pub struct ClearCacheResponse {
    pub success: bool,
    pub cleared_layers: Vec<String>,
    pub entries_cleared: usize,
    pub memory_freed_mb: f64,
}

/// Get comprehensive cache statistics
pub async fn get_cache_stats(
    State(state): State<AppState>,
    Query(params): Query<CacheQuery>,
    auth_info: AuthInfo,
) -> Result<Json<CacheStats>> {
    info!("Cache stats requested by user: {}", auth_info.user_id);

    let cache_manager = &state.cache_manager;
    let stats = cache_manager.get_stats().await;

    let cache_stats = CacheStats {
        sessions: CacheLayerStats {
            entries: stats.sessions.entry_count,
            capacity: 10000, // Default capacity
            hit_count: stats.sessions.hits,
            miss_count: stats.sessions.misses,
            hit_rate: stats.sessions.hit_ratio(),
            memory_usage_bytes: stats.sessions.size_bytes,
        },
        objects: CacheLayerStats {
            entries: stats.objects.entry_count,
            capacity: 10000, // Default capacity
            hit_count: stats.objects.hits,
            miss_count: stats.objects.misses,
            hit_rate: stats.objects.hit_ratio(),
            memory_usage_bytes: stats.objects.size_bytes,
        },
        permissions: CacheLayerStats {
            entries: stats.permissions.entry_count,
            capacity: 10000, // Default capacity
            hit_count: stats.permissions.hits,
            miss_count: stats.permissions.misses,
            hit_rate: stats.permissions.hit_ratio(),
            memory_usage_bytes: stats.permissions.size_bytes,
        },
        command_results: CacheLayerStats {
            entries: stats.command_results.entry_count,
            capacity: 10000, // Default capacity
            hit_count: stats.command_results.hits,
            miss_count: stats.command_results.misses,
            hit_rate: stats.command_results.hit_ratio(),
            memory_usage_bytes: stats.command_results.size_bytes,
        },
        computed_geometry: CacheLayerStats {
            entries: stats.computed_geometry_count,
            capacity: usize::MAX, // DashMap has no fixed capacity
            hit_count: 0,         // Would need separate tracking
            miss_count: 0,
            hit_rate: 0.0,
            memory_usage_bytes: stats.computed_geometry_count * 1024, // Estimate
        },
        total_memory_mb: stats.total_memory_bytes() as f64 / (1024.0 * 1024.0),
        hit_rate_percentage: (stats.sessions.hit_ratio()
            + stats.objects.hit_ratio()
            + stats.permissions.hit_ratio()
            + stats.command_results.hit_ratio())
            / 4.0
            * 100.0,
    };

    Ok(Json(cache_stats))
}

/// Clear cache layers
pub async fn clear_cache(
    State(state): State<AppState>,
    Json(request): Json<ClearCacheRequest>,
    auth_info: AuthInfo,
) -> Result<Json<ClearCacheResponse>> {
    info!("Cache clear requested by user: {}", auth_info.user_id);

    // Security check - only admins can clear cache
    if !auth_info
        .permissions
        .contains(&session_manager::Permission::ModifySettings)
    {
        return Err((StatusCode::FORBIDDEN, "Insufficient permissions").into());
    }

    if !request.confirm {
        return Err((StatusCode::BAD_REQUEST, "Cache clear must be confirmed").into());
    }

    let cache_manager = &state.cache_manager;
    let mut cleared_layers = Vec::new();
    let mut entries_cleared = 0;
    let memory_before = get_total_cache_memory(cache_manager).await;

    let layers_to_clear = request.layers.unwrap_or_else(|| {
        vec![
            "sessions".to_string(),
            "objects".to_string(),
            "permissions".to_string(),
            "command_results".to_string(),
            "computed_geometry".to_string(),
        ]
    });

    // For now, clear all caches at once
    // In future, could add layer-specific clearing
    if !layers_to_clear.is_empty() {
        cache_manager.clear_all().await;
        cleared_layers = layers_to_clear;
        // Get stats to count entries
        let stats_before = cache_manager.get_stats().await;
        entries_cleared = stats_before.sessions.entry_count
            + stats_before.objects.entry_count
            + stats_before.permissions.entry_count
            + stats_before.command_results.entry_count
            + stats_before.computed_geometry_count;
        info!("Cleared {} total cache entries", entries_cleared);
    }

    let memory_after = get_total_cache_memory(cache_manager).await;
    let memory_freed_mb = (memory_before - memory_after) as f64 / (1024.0 * 1024.0);

    Ok(Json(ClearCacheResponse {
        success: true,
        cleared_layers,
        entries_cleared,
        memory_freed_mb,
    }))
}

/// Get a specific cached object (for debugging)
pub async fn get_cached_object(
    State(state): State<AppState>,
    Path(object_id): Path<String>,
    auth_info: AuthInfo,
) -> Result<Json<serde_json::Value>> {
    info!(
        "Cached object {} requested by user: {}",
        object_id, auth_info.user_id
    );

    let cache_manager = &state.cache_manager;

    // Try to find the object in any cache layer
    if let Some(session) = cache_manager.get_session(&object_id).await {
        return Ok(Json(serde_json::json!({
            "type": "session",
            "data": session
        })));
    }

    // Try to parse as UUID for geometry ID
    if let Ok(uuid) = uuid::Uuid::parse_str(&object_id) {
        let geometry_id = GeometryId(uuid);
        if let Some(object) = cache_manager
            .get_object(&auth_info.user_id, &geometry_id)
            .await
        {
            return Ok(Json(serde_json::json!({
                "type": "object",
                "data": object
            })));
        }
    }

    if let Some(permissions) = cache_manager
        .get_permissions(&auth_info.user_id, &object_id)
        .await
    {
        return Ok(Json(serde_json::json!({
            "type": "permissions",
            "data": permissions
        })));
    }

    if let Some(command_result) = cache_manager.get_command_result(&object_id).await {
        return Ok(Json(serde_json::json!({
            "type": "command_result",
            "data": command_result
        })));
    }

    if let Some(geometry) = cache_manager.get_computed_geometry(&object_id) {
        return Ok(Json(serde_json::json!({
            "type": "computed_geometry",
            "data": geometry
        })));
    }

    Err((StatusCode::NOT_FOUND, "Object not found in cache").into())
}

/// Estimate memory usage of DashMap (rough approximation)
fn estimate_dashmap_memory(dashmap: &dashmap::DashMap<String, serde_json::Value>) -> usize {
    // Rough estimate: key size + value size + overhead
    dashmap.len() * (50 + 200 + 64) // Average 50 bytes key, 200 bytes value, 64 bytes overhead
}

/// Get total cache memory usage
async fn get_total_cache_memory(cache_manager: &CacheManager) -> usize {
    let stats = cache_manager.get_stats().await;
    stats.sessions.size_bytes
        + stats.objects.size_bytes
        + stats.permissions.size_bytes
        + stats.command_results.size_bytes
}

// Wrapper function for clear_cache handler
pub async fn clear_cache_wrapper(
    State(state): State<crate::AppState>,
    axum::extract::Extension(auth_info): axum::extract::Extension<crate::auth_middleware::AuthInfo>,
    Json(request): Json<ClearCacheRequest>,
) -> Result<Json<ClearCacheResponse>> {
    clear_cache(State(state), Json(request), auth_info).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_cache_stats() {
        // Test implementation
    }

    #[tokio::test]
    async fn test_clear_cache() {
        // Test implementation
    }
}
