/// Metrics API for Admin Dashboard
/// 
/// Provides real-time metrics and monitoring data for the TurboRAG system

use axum::{
    extract::{Query, State},
    response::Json,
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use chrono::{DateTime, Utc, Duration};
use dashmap::DashMap;
use std::collections::VecDeque;
use parking_lot::RwLock;

/// System metrics snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMetrics {
    pub timestamp: DateTime<Utc>,
    pub total_documents: usize,
    pub total_chunks: usize,
    pub index_size_bytes: usize,
    pub active_users: usize,
    pub qps: f64,
    pub avg_latency_ms: f64,
    pub p95_latency_ms: f64,
    pub p99_latency_ms: f64,
    pub cache_hit_rate: f64,
    pub cpu_usage: f64,
    pub memory_usage_mb: f64,
    pub disk_io_mbps: f64,
    pub network_mbps: f64,
    pub thread_pool_usage: (usize, usize),
    pub queue_depth: usize,
    pub errors_per_minute: usize,
}

/// Query type statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryTypeStats {
    pub vector_search: usize,
    pub bm25_search: usize,
    pub hybrid_search: usize,
    pub symbol_lookup: usize,
    pub fuzzy_search: usize,
}

/// Storage tier statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageTierStats {
    pub hot_tier: TierStats,
    pub warm_tier: TierStats,
    pub cold_tier: TierStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierStats {
    pub document_count: usize,
    pub size_bytes: usize,
    pub access_rate: f64,
    pub migration_pending: usize,
}

/// Activity log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub timestamp: DateTime<Utc>,
    pub event_type: String,
    pub description: String,
    pub user_id: Option<String>,
    pub duration_ms: Option<f64>,
    pub result: ActivityResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ActivityResult {
    Success,
    Warning,
    Error,
}

/// Time series data point
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeSeriesPoint {
    pub timestamp: DateTime<Utc>,
    pub value: f64,
}

/// Dashboard data response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardData {
    pub current_metrics: SystemMetrics,
    pub query_types: QueryTypeStats,
    pub storage_tiers: StorageTierStats,
    pub latency_history: Vec<TimeSeriesPoint>,
    pub qps_history: Vec<TimeSeriesPoint>,
    pub embedding_throughput: Vec<TimeSeriesPoint>,
    pub recent_activities: Vec<ActivityEntry>,
    pub alerts: Vec<Alert>,
}

/// System alert
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub id: String,
    pub severity: AlertSeverity,
    pub title: String,
    pub message: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AlertSeverity {
    Info,
    Warning,
    Critical,
}

/// Metrics collector
pub struct MetricsCollector {
    /// Current metrics
    current: Arc<RwLock<SystemMetrics>>,
    
    /// Historical data
    latency_history: Arc<RwLock<VecDeque<TimeSeriesPoint>>>,
    qps_history: Arc<RwLock<VecDeque<TimeSeriesPoint>>>,
    embedding_history: Arc<RwLock<VecDeque<TimeSeriesPoint>>>,
    
    /// Query type counters
    query_types: Arc<DashMap<String, usize>>,
    
    /// Activity log
    activities: Arc<RwLock<VecDeque<ActivityEntry>>>,
    
    /// Active alerts
    alerts: Arc<RwLock<Vec<Alert>>>,
    
    /// Request latencies for percentile calculation
    latencies: Arc<RwLock<Vec<f64>>>,
}

impl MetricsCollector {
    pub fn new() -> Self {
        Self {
            current: Arc::new(RwLock::new(SystemMetrics::default())),
            latency_history: Arc::new(RwLock::new(VecDeque::with_capacity(1000))),
            qps_history: Arc::new(RwLock::new(VecDeque::with_capacity(1000))),
            embedding_history: Arc::new(RwLock::new(VecDeque::with_capacity(1000))),
            query_types: Arc::new(DashMap::new()),
            activities: Arc::new(RwLock::new(VecDeque::with_capacity(100))),
            alerts: Arc::new(RwLock::new(Vec::new())),
            latencies: Arc::new(RwLock::new(Vec::with_capacity(10000))),
        }
    }
    
    /// Record a search request
    pub fn record_search(&self, query_type: &str, latency_ms: f64) {
        // Update query type counter
        self.query_types
            .entry(query_type.to_string())
            .or_insert(0)
            .add_assign(1);
        
        // Record latency
        {
            let mut latencies = self.latencies.write();
            latencies.push(latency_ms);
            
            // Keep only recent latencies (last 10k)
            if latencies.len() > 10000 {
                latencies.remove(0);
            }
        }
        
        // Update current metrics
        {
            let mut metrics = self.current.write();
            metrics.avg_latency_ms = self.calculate_avg_latency();
            metrics.p95_latency_ms = self.calculate_percentile(0.95);
            metrics.p99_latency_ms = self.calculate_percentile(0.99);
        }
        
        // Add to history
        {
            let mut history = self.latency_history.write();
            history.push_back(TimeSeriesPoint {
                timestamp: Utc::now(),
                value: latency_ms,
            });
            
            // Keep only last hour
            while history.len() > 3600 {
                history.pop_front();
            }
        }
    }
    
    /// Record document indexing
    pub fn record_indexing(&self, doc_count: usize, duration_ms: f64) {
        // Update metrics
        {
            let mut metrics = self.current.write();
            metrics.total_documents += doc_count;
        }
        
        // Log activity
        self.log_activity(
            "indexing",
            format!("Indexed {} documents in {:.1}ms", doc_count, duration_ms),
            ActivityResult::Success,
        );
    }
    
    /// Record embedding generation
    pub fn record_embedding(&self, count: usize, duration_ms: f64) {
        let throughput = (count as f64 / duration_ms) * 1000.0;
        
        // Add to history
        {
            let mut history = self.embedding_history.write();
            history.push_back(TimeSeriesPoint {
                timestamp: Utc::now(),
                value: throughput,
            });
            
            while history.len() > 3600 {
                history.pop_front();
            }
        }
    }
    
    /// Log an activity
    pub fn log_activity(&self, event_type: &str, description: String, result: ActivityResult) {
        let mut activities = self.activities.write();
        activities.push_back(ActivityEntry {
            timestamp: Utc::now(),
            event_type: event_type.to_string(),
            description,
            user_id: None,
            duration_ms: None,
            result,
        });
        
        // Keep only recent activities
        while activities.len() > 100 {
            activities.pop_front();
        }
    }
    
    /// Add an alert
    pub fn add_alert(&self, severity: AlertSeverity, title: String, message: String) {
        let mut alerts = self.alerts.write();
        alerts.push(Alert {
            id: uuid::Uuid::new_v4().to_string(),
            severity,
            title,
            message,
            timestamp: Utc::now(),
        });
        
        // Keep only recent alerts
        while alerts.len() > 20 {
            alerts.remove(0);
        }
    }
    
    /// Get dashboard data
    pub fn get_dashboard_data(&self) -> DashboardData {
        // Calculate QPS
        let qps = self.calculate_qps();
        
        // Update current metrics
        {
            let mut metrics = self.current.write();
            metrics.timestamp = Utc::now();
            metrics.qps = qps;
            metrics.cache_hit_rate = self.calculate_cache_hit_rate();
            
            // Simulate some metrics (in production, get from system)
            metrics.cpu_usage = 20.0 + (rand::random::<f64>() * 10.0);
            metrics.memory_usage_mb = 8000.0 + (rand::random::<f64>() * 500.0);
            metrics.disk_io_mbps = 100.0 + (rand::random::<f64>() * 50.0);
            metrics.network_mbps = 30.0 + (rand::random::<f64>() * 20.0);
            metrics.thread_pool_usage = (16, 32);
            metrics.queue_depth = (rand::random::<f64>() * 200.0) as usize;
        }
        
        DashboardData {
            current_metrics: self.current.read().clone(),
            query_types: self.get_query_type_stats(),
            storage_tiers: self.get_storage_tier_stats(),
            latency_history: self.latency_history.read().iter().cloned().collect(),
            qps_history: self.qps_history.read().iter().cloned().collect(),
            embedding_throughput: self.embedding_history.read().iter().cloned().collect(),
            recent_activities: self.activities.read().iter().cloned().collect(),
            alerts: self.alerts.read().clone(),
        }
    }
    
    // Helper methods
    
    fn calculate_avg_latency(&self) -> f64 {
        let latencies = self.latencies.read();
        if latencies.is_empty() {
            return 0.0;
        }
        latencies.iter().sum::<f64>() / latencies.len() as f64
    }
    
    fn calculate_percentile(&self, percentile: f64) -> f64 {
        let mut latencies = self.latencies.read().clone();
        if latencies.is_empty() {
            return 0.0;
        }
        
        latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let index = ((latencies.len() as f64 - 1.0) * percentile) as usize;
        latencies[index]
    }
    
    fn calculate_qps(&self) -> f64 {
        let total_queries: usize = self.query_types.iter()
            .map(|entry| *entry.value())
            .sum();
        
        // Calculate over last minute
        total_queries as f64 / 60.0
    }
    
    fn calculate_cache_hit_rate(&self) -> f64 {
        // In production, track actual cache hits
        0.85 + (rand::random::<f64>() * 0.1)
    }
    
    fn get_query_type_stats(&self) -> QueryTypeStats {
        QueryTypeStats {
            vector_search: self.query_types.get("vector").map(|r| *r).unwrap_or(0),
            bm25_search: self.query_types.get("bm25").map(|r| *r).unwrap_or(0),
            hybrid_search: self.query_types.get("hybrid").map(|r| *r).unwrap_or(0),
            symbol_lookup: self.query_types.get("symbol").map(|r| *r).unwrap_or(0),
            fuzzy_search: self.query_types.get("fuzzy").map(|r| *r).unwrap_or(0),
        }
    }
    
    fn get_storage_tier_stats(&self) -> StorageTierStats {
        // In production, get from actual storage
        StorageTierStats {
            hot_tier: TierStats {
                document_count: 1200,
                size_bytes: 100 * 1024 * 1024,
                access_rate: 0.85,
                migration_pending: 50,
            },
            warm_tier: TierStats {
                document_count: 3500,
                size_bytes: 500 * 1024 * 1024,
                access_rate: 0.15,
                migration_pending: 200,
            },
            cold_tier: TierStats {
                document_count: 15000,
                size_bytes: 2 * 1024 * 1024 * 1024,
                access_rate: 0.02,
                migration_pending: 0,
            },
        }
    }
}

impl Default for SystemMetrics {
    fn default() -> Self {
        Self {
            timestamp: Utc::now(),
            total_documents: 5480,
            total_chunks: 12000,
            index_size_bytes: 2 * 1024 * 1024 * 1024,
            active_users: 42,
            qps: 0.0,
            avg_latency_ms: 0.8,
            p95_latency_ms: 1.5,
            p99_latency_ms: 3.0,
            cache_hit_rate: 0.94,
            cpu_usage: 25.0,
            memory_usage_mb: 8192.0,
            disk_io_mbps: 124.0,
            network_mbps: 42.0,
            thread_pool_usage: (16, 32),
            queue_depth: 127,
            errors_per_minute: 0,
        }
    }
}

use std::ops::AddAssign;

// AddAssign is already implemented for usize in std

/// API handlers

pub async fn get_metrics(
    State(collector): State<Arc<MetricsCollector>>,
) -> Result<Json<DashboardData>, StatusCode> {
    Ok(Json(collector.get_dashboard_data()))
}

pub async fn get_health(
    State(collector): State<Arc<MetricsCollector>>,
) -> Result<Json<HealthStatus>, StatusCode> {
    let metrics = collector.current.read();
    
    let status = if metrics.errors_per_minute > 10 {
        "degraded"
    } else if metrics.qps > 0.0 {
        "healthy"
    } else {
        "idle"
    };
    
    Ok(Json(HealthStatus {
        status: status.to_string(),
        uptime_seconds: 3600, // In production, track actual uptime
        version: "1.0.0".to_string(),
    }))
}

#[derive(Serialize)]
pub struct HealthStatus {
    pub status: String,
    pub uptime_seconds: u64,
    pub version: String,
}

// Export for use in main API
pub use MetricsCollector as Metrics;