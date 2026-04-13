/// API module for TurboRAG
/// 
/// Provides REST API endpoints for search, indexing, and monitoring

pub mod metrics;
pub mod search;

pub use metrics::{MetricsCollector, DashboardData, SystemMetrics, ActivityEntry, Alert, ActivityResult, AlertSeverity};
pub use search::{search_handler, stats_handler, SearchRequest, SearchResponse};