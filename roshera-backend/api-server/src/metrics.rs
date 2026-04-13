use axum::Json;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Serialize, Deserialize)]
pub struct SystemMetrics {
    pub system: SystemInfo,
    pub users: UserMetrics,
    pub ai: AIMetrics,
    pub geometry: GeometryMetrics,
    pub database: DatabaseMetrics,
    pub performance: PerformanceMetrics,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SystemInfo {
    pub cpu_usage: f32,
    pub memory_mb: u64,
    pub memory_percent: f32,
    pub uptime_seconds: u64,
    pub thread_count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserMetrics {
    pub active_sessions: usize,
    pub websocket_connections: usize,
    pub commands_per_minute: f32,
    pub peak_concurrent: usize,
    pub total_users: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AIMetrics {
    pub ollama_available: bool,
    pub avg_response_ms: u64,
    pub success_rate: f32,
    pub total_commands: u64,
    pub create_ops: u64,
    pub boolean_ops: u64,
    pub query_ops: u64,
    pub vision_commands: u64,
    pub vision_processing_ms: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GeometryMetrics {
    pub total_solids: usize,
    pub total_faces: usize,
    pub total_edges: usize,
    pub total_vertices: usize,
    pub box_creation_ms: u64,
    pub boolean_union_ms: u64,
    pub tessellation_ms: u64,
    pub cache_hit_rate: f32,
    pub valid_geometry_rate: f32,
    pub boolean_success_rate: f32,
    pub invalid_detected: u64,
    pub auto_healed: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DatabaseMetrics {
    pub connected: bool,
    pub avg_query_ms: u64,
    pub active_connections: usize,
    pub max_connections: usize,
    pub size_mb: u64,
    pub timeline_events: u64,
    pub save_time_ms: u64,
    pub load_time_ms: u64,
    pub snapshots: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PerformanceMetrics {
    pub requests_per_second: f32,
    pub avg_response_time_ms: u64,
    pub error_rate: f32,
    pub cache_hit_rate: f32,
}

pub async fn get_metrics(
    axum::extract::State(state): axum::extract::State<crate::AppState>,
) -> Json<SystemMetrics> {
    // Use the global system monitor for accurate CPU readings
    let mut sys = crate::SYSTEM_MONITOR.lock().await;

    // Refresh only what we need
    sys.refresh_cpu_usage();
    sys.refresh_memory();

    // Get current process
    let pid = sysinfo::Pid::from(std::process::id() as usize);
    sys.refresh_process(pid);

    let current_process = sys.process(pid);

    // Calculate CPU usage - on Windows, process CPU can exceed 100% (per core)
    // So we need to normalize it
    let cpu_usage = if let Some(process) = current_process {
        let usage = process.cpu_usage();
        // If the value is unreasonably high, it's probably not initialized yet
        if usage > 1000.0 {
            0.0
        } else {
            // On Windows with multiple cores, divide by core count if > 100
            let core_count = sys.cpus().len() as f32;
            if usage > 100.0 {
                (usage / core_count).min(100.0)
            } else {
                usage.min(100.0)
            }
        }
    } else {
        // Fallback to system-wide average
        let total: f32 = sys.cpus().iter().map(|cpu| cpu.cpu_usage()).sum();
        (total / sys.cpus().len() as f32).min(100.0)
    };

    let memory_mb = if let Some(process) = current_process {
        process.memory() / 1024 / 1024
    } else {
        sys.used_memory() / 1024 / 1024
    };

    let total_memory_mb = sys.total_memory() / 1024 / 1024;
    let memory_percent = if total_memory_mb > 0 {
        ((memory_mb as f32 / total_memory_mb as f32) * 100.0).min(100.0)
    } else {
        0.0
    };

    // Get uptime from the global start time
    let uptime_seconds = crate::SERVER_START_TIME.elapsed().as_secs();

    // Get active WebSocket connections
    let ws_connections = crate::ACTIVE_WEBSOCKETS.load(std::sync::atomic::Ordering::Relaxed);

    // Get total requests
    let total_requests = crate::TOTAL_REQUESTS.load(std::sync::atomic::Ordering::Relaxed);

    // Calculate requests per second (simple average over uptime)
    let requests_per_second = if uptime_seconds > 0 {
        total_requests as f32 / uptime_seconds as f32
    } else {
        0.0
    };

    // Get session count
    let active_sessions = state.session_manager.list_sessions().await.len();

    // Check Ollama availability
    let ollama_available = check_ollama_status().await;

    // Get geometry statistics from the model
    let (total_solids, total_faces, total_edges, total_vertices) = {
        let model = state.geometry_model.read().await;

        // Get counts from the model's stores
        let solid_count = model.solids.stats.total_created as usize;
        let face_count = model.faces.len();
        let edge_count = model.edges.len();
        let vertex_count = model.vertices.len();

        (solid_count, face_count, edge_count, vertex_count)
    };

    // Get command statistics from the metrics tracker
    let command_stats = {
        let stats = state.command_metrics.lock().await;
        (
            stats.total_commands,
            stats.create_commands,
            stats.boolean_commands,
            stats.query_commands,
            stats.vision_commands,
        )
    };

    // Get performance metrics
    let perf_metrics = {
        let perf = state.performance_metrics.lock().await;
        (
            perf.avg_response_time_ms,
            perf.error_count,
            perf.success_count,
            perf.cache_hits,
            perf.cache_misses,
            perf.ai_response_time_ms,
            perf.vision_processing_time_ms,
            perf.box_creation_time_ms,
            perf.boolean_time_ms,
            perf.tessellation_time_ms,
        )
    };

    // Calculate rates
    let total_ops = perf_metrics.2 + perf_metrics.1; // success + error
    let error_rate = if total_ops > 0 {
        (perf_metrics.1 as f32 / total_ops as f32) * 100.0
    } else {
        0.0
    };

    let success_rate = if total_ops > 0 {
        (perf_metrics.2 as f32 / total_ops as f32) * 100.0
    } else {
        100.0
    };

    let cache_total = perf_metrics.3 + perf_metrics.4; // hits + misses
    let cache_hit_rate = if cache_total > 0 {
        (perf_metrics.3 as f32 / cache_total as f32) * 100.0
    } else {
        0.0
    };

    // Get database metrics
    let db_metrics = {
        // For now, we'll return basic info
        // In production, query actual PostgreSQL stats
        DatabaseMetrics {
            connected: true, // We know it's connected if server started
            avg_query_ms: 5, // Typical for local PostgreSQL
            active_connections: 1,
            max_connections: 20,
            size_mb: 50,                      // Would need actual DB query
            timeline_events: command_stats.0, // Use total commands as proxy
            save_time_ms: 10,
            load_time_ms: 15,
            snapshots: 0,
        }
    };

    Json(SystemMetrics {
        system: SystemInfo {
            cpu_usage,
            memory_mb, // This is the API server process memory usage in MB
            memory_percent,
            uptime_seconds,
            thread_count: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
        },
        users: UserMetrics {
            active_sessions,
            websocket_connections: ws_connections,
            commands_per_minute: if uptime_seconds > 60 {
                (command_stats.0 as f32 / (uptime_seconds as f32 / 60.0))
            } else {
                command_stats.0 as f32
            },
            peak_concurrent: ws_connections.max(active_sessions),
            total_users: active_sessions, // For now, same as active
        },
        ai: AIMetrics {
            ollama_available,
            avg_response_ms: perf_metrics.5,
            success_rate,
            total_commands: command_stats.0,
            create_ops: command_stats.1,
            boolean_ops: command_stats.2,
            query_ops: command_stats.3,
            vision_commands: command_stats.4,
            vision_processing_ms: perf_metrics.6,
        },
        geometry: GeometryMetrics {
            total_solids,
            total_faces,
            total_edges,
            total_vertices,
            box_creation_ms: perf_metrics.7,
            boolean_union_ms: perf_metrics.8,
            tessellation_ms: perf_metrics.9,
            cache_hit_rate,
            valid_geometry_rate: 100.0, // Assume all valid for now
            boolean_success_rate: 95.0, // Typical success rate
            invalid_detected: 0,
            auto_healed: 0,
        },
        database: db_metrics,
        performance: PerformanceMetrics {
            requests_per_second,
            avg_response_time_ms: perf_metrics.0,
            error_rate,
            cache_hit_rate,
        },
    })
}

async fn check_ollama_status() -> bool {
    // Check if Ollama is running - use a fresh client each time to avoid caching
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(500))
        .build()
        .unwrap();

    // Try to get Ollama version endpoint which is lightweight
    match client
        .get("http://localhost:11434/api/version")
        .send()
        .await
    {
        Ok(response) => {
            // Only consider it online if we get a 200 OK
            response.status() == reqwest::StatusCode::OK
        }
        Err(_) => false,
    }
}

// Metrics tracking structures to add to AppState
#[derive(Debug, Default)]
pub struct CommandMetrics {
    pub total_commands: u64,
    pub create_commands: u64,
    pub boolean_commands: u64,
    pub query_commands: u64,
    pub vision_commands: u64,
}

#[derive(Debug, Default)]
pub struct PerformanceTracker {
    pub avg_response_time_ms: u64,
    pub error_count: u64,
    pub success_count: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub ai_response_time_ms: u64,
    pub vision_processing_time_ms: u64,
    pub box_creation_time_ms: u64,
    pub boolean_time_ms: u64,
    pub tessellation_time_ms: u64,
}

impl CommandMetrics {
    pub fn record_command(&mut self, command_type: &str) {
        self.total_commands += 1;
        match command_type {
            "create" | "primitive" => self.create_commands += 1,
            "boolean" | "union" | "intersection" | "difference" => self.boolean_commands += 1,
            "query" | "select" | "info" => self.query_commands += 1,
            "vision" => self.vision_commands += 1,
            _ => {}
        }
    }
}

impl PerformanceTracker {
    pub fn record_response(&mut self, time_ms: u64, success: bool) {
        // Update rolling average
        let total = self.success_count + self.error_count;
        if total > 0 {
            self.avg_response_time_ms =
                ((self.avg_response_time_ms * total) + time_ms) / (total + 1);
        } else {
            self.avg_response_time_ms = time_ms;
        }

        if success {
            self.success_count += 1;
        } else {
            self.error_count += 1;
        }
    }

    pub fn record_cache(&mut self, hit: bool) {
        if hit {
            self.cache_hits += 1;
        } else {
            self.cache_misses += 1;
        }
    }

    pub fn record_operation(&mut self, op_type: &str, time_ms: u64) {
        match op_type {
            "ai_response" => self.ai_response_time_ms = time_ms,
            "vision" => self.vision_processing_time_ms = time_ms,
            "create_box" => self.box_creation_time_ms = time_ms,
            "boolean" => self.boolean_time_ms = time_ms,
            "tessellation" => self.tessellation_time_ms = time_ms,
            _ => {}
        }
    }
}
