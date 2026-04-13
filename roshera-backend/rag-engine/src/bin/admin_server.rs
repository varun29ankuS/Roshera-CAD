/// Admin Dashboard Server for TurboRAG
/// 
/// Serves the monitoring dashboard with real-time metrics

use axum::{
    extract::State,
    http::{StatusCode, header},
    response::{Html, Response},
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;
use rag_engine::api::{MetricsCollector, metrics};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║              TurboRAG Admin Dashboard Server             ║");
    println!("╚══════════════════════════════════════════════════════════╝");
    
    // Initialize metrics collector
    let metrics = Arc::new(MetricsCollector::new());
    
    // Start metric simulation (in production, connect to real systems)
    start_metric_simulation(metrics.clone()).await;
    
    // Build our application with routes
    let app = Router::new()
        .route("/", get(dashboard_handler))
        .route("/api/metrics", get(metrics::get_metrics))
        .route("/api/health", get(metrics::get_health))
        .route("/api/simulate", post(simulate_activity))
        .layer(CorsLayer::permissive())
        .with_state(metrics);

    // Start server
    let addr = "127.0.0.1:3001";
    println!("\n🚀 Admin Dashboard starting on http://{}", addr);
    println!("   Dashboard: http://localhost:3001/");
    println!("   Metrics API: http://localhost:3001/api/metrics");
    println!("   Health Check: http://localhost:3001/api/health");
    
    let listener = TcpListener::bind(addr).await?;
    println!("\n✅ Server ready! Open http://localhost:3001 to view dashboard");
    
    axum::serve(listener, app).await?;

    Ok(())
}

/// Serve the admin dashboard HTML
async fn dashboard_handler() -> Html<String> {
    let dashboard_html = include_str!("../../static/admin-dashboard.html");
    Html(dashboard_html.to_string())
}

/// Simulate some activity for demo purposes
async fn simulate_activity(
    State(metrics): State<Arc<MetricsCollector>>,
) -> Result<StatusCode, StatusCode> {
    // Simulate a search
    metrics.record_search("hybrid", 0.5 + rand::random::<f64>());
    
    // Log activity
    metrics.log_activity(
        "api_test",
        "Simulated search activity for demo".to_string(),
        rag_engine::api::ActivityResult::Success,
    );
    
    Ok(StatusCode::OK)
}

/// Start background task to simulate metrics
async fn start_metric_simulation(metrics: Arc<MetricsCollector>) {
    println!("🔧 Starting metric simulation...");
    
    // Simulate initial activity
    metrics.log_activity(
        "startup", 
        "TurboRAG Admin Dashboard started".to_string(),
        rag_engine::api::ActivityResult::Success,
    );
    
    metrics.add_alert(
        rag_engine::api::AlertSeverity::Info,
        "System Online".to_string(),
        "TurboRAG is running normally".to_string(),
    );
    
    // Start background simulation
    let metrics_clone = metrics.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        let activities = vec![
            ("search", "User searched for 'NURBS surface evaluation'"),
            ("search", "Vector search performed on geometry module"),
            ("indexing", "Indexed 15 new files from workspace"),
            ("embedding", "Generated embeddings for 200 chunks"),
            ("cache", "Cache optimization completed"),
            ("storage", "Data migrated from hot to warm tier"),
            ("security", "Access control check passed"),
            ("backup", "Incremental backup completed"),
        ];
        
        loop {
            interval.tick().await;
            
            // Random activity
            if rand::random::<f64>() > 0.3 {
                let (event_type, description) = &activities[rand::random::<usize>() % activities.len()];
                
                // Simulate different types of activities
                match *event_type {
                    "search" => {
                        let query_types = ["vector", "bm25", "hybrid", "symbol"];
                        let query_type = query_types[rand::random::<usize>() % query_types.len()];
                        let latency = 0.2 + rand::random::<f64>() * 2.0;
                        metrics_clone.record_search(query_type, latency);
                        
                        metrics_clone.log_activity(
                            "search",
                            format!("{} ({} search, {:.1}ms)", description, query_type, latency),
                            rag_engine::api::ActivityResult::Success,
                        );
                    },
                    "indexing" => {
                        let doc_count = 5 + rand::random::<usize>() % 20;
                        let duration = 100.0 + rand::random::<f64>() * 500.0;
                        metrics_clone.record_indexing(doc_count, duration);
                    },
                    "embedding" => {
                        let count = 50 + rand::random::<usize>() % 200;
                        let duration = 10.0 + rand::random::<f64>() * 50.0;
                        metrics_clone.record_embedding(count, duration);
                        
                        metrics_clone.log_activity(
                            "embedding",
                            format!("Generated {} embeddings in {:.1}ms", count, duration),
                            rag_engine::api::ActivityResult::Success,
                        );
                    },
                    _ => {
                        metrics_clone.log_activity(
                            event_type,
                            description.to_string(),
                            rag_engine::api::ActivityResult::Success,
                        );
                    }
                }
            }
            
            // Occasionally add alerts
            if rand::random::<f64>() > 0.95 {
                let alert_types = [
                    ("Info", "Cache hit rate optimized", "Cache performance improved to 96.8%"),
                    ("Warning", "High query load", "Query rate is above normal threshold"),
                    ("Info", "Index optimization", "Background index optimization completed"),
                ];
                
                let (severity, title, message) = &alert_types[rand::random::<usize>() % alert_types.len()];
                let severity = match *severity {
                    "Info" => rag_engine::api::AlertSeverity::Info,
                    "Warning" => rag_engine::api::AlertSeverity::Warning,
                    _ => rag_engine::api::AlertSeverity::Critical,
                };
                
                metrics_clone.add_alert(severity, title.to_string(), message.to_string());
            }
        }
    });
}