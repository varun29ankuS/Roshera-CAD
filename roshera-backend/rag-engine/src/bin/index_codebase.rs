/// TurboRAG Codebase Indexer
/// 
/// Index your entire codebase for intelligent search and retrieval

use anyhow::Result;
use std::path::PathBuf;
use clap::Parser;
use rag_engine::indexer::{FileIndexer, IndexerConfig};
use rag_engine::storage::StorageEngine;
use rag_engine::security::{SecurityManager, SecurityConfig, CacheConfig};
use rag_engine::security::audit::{AuditConfig, ComplianceMode};
use rag_engine::security::encryption::EncryptionConfig;
use sqlx::PgPool;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Directory to index
    #[clap(short, long, default_value = "C:\\Users\\Varun Sharma\\Roshera-CAD")]
    path: PathBuf,
    
    /// Chunk size in lines
    #[clap(short = 's', long, default_value = "100")]
    chunk_size: usize,
    
    /// Number of parallel workers
    #[clap(short, long)]
    workers: Option<usize>,
    
    /// Watch for file changes
    #[clap(short = 'w', long)]
    watch: bool,
    
    /// Include hidden files
    #[clap(short = 'H', long)]
    hidden: bool,
    
    /// Database URL
    #[clap(short = 'd', long, env = "DATABASE_URL")]
    database: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse arguments
    let args = Args::parse();
    
    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║            TurboRAG Enterprise Codebase Indexer          ║");
    println!("╚══════════════════════════════════════════════════════════╝");
    println!();
    
    // Setup storage
    println!("📦 Initializing storage engine...");
    let storage_path = PathBuf::from("./rag_data");
    tokio::fs::create_dir_all(&storage_path).await?;
    let storage = Arc::new(StorageEngine::new(&storage_path).await?);
    
    // Setup security (simplified for local indexing)
    println!("🔒 Initializing security layer...");
    let security = if let Some(db_url) = args.database {
        // Use real database
        let pool = PgPool::connect(&db_url).await?;
        let config = SecurityConfig {
            audit_config: AuditConfig {
                retention_days: 30,
                batch_size: 100,
                flush_interval_secs: 60,
                sign_logs: false,
                encrypt_sensitive: false,
                compliance_mode: ComplianceMode::None,
            },
            encryption_config: EncryptionConfig::default(),
            cache_config: CacheConfig { size: 10000 },
        };
        Arc::new(SecurityManager::new(pool, config).await?)
    } else {
        // Mock security for local testing
        Arc::new(create_mock_security().await?)
    };
    
    // Configure indexer
    let config = IndexerConfig {
        chunk_size: args.chunk_size,
        chunk_overlap: 20,
        max_file_size: 50 * 1024 * 1024, // 50MB
        ignore_patterns: vec![
            "target".to_string(),
            "node_modules".to_string(),
            ".git".to_string(),
            "dist".to_string(),
            "build".to_string(),
            "__pycache__".to_string(),
            "*.pyc".to_string(),
            "*.exe".to_string(),
            "*.dll".to_string(),
            "*.so".to_string(),
            "*.wasm".to_string(),
            "*.lock".to_string(),
            "Cargo.lock".to_string(),
        ],
        include_hidden: args.hidden,
        watch_changes: args.watch,
        parallel_workers: args.workers.unwrap_or_else(num_cpus::get),
    };
    
    // Create indexer
    println!("🚀 Creating indexer with {} workers...", config.parallel_workers);
    let indexer = FileIndexer::new(storage, security, config).await?;
    
    // Display target directory info
    println!("\n📁 Target directory: {}", args.path.display());
    
    // Get directory size estimate
    let dir_size = estimate_directory_size(&args.path).await?;
    println!("   Estimated size: {:.2} MB", dir_size as f64 / 1_048_576.0);
    
    // Start indexing
    println!("\n════════════════════════════════════════════════════════════");
    println!("                    STARTING INDEXING");
    println!("════════════════════════════════════════════════════════════\n");
    
    let start_time = std::time::Instant::now();
    
    // Index the directory
    let stats = indexer.index_directory(&args.path).await?;
    
    let duration = start_time.elapsed();
    
    // Display results
    println!("\n════════════════════════════════════════════════════════════");
    println!("                    INDEXING COMPLETE");
    println!("════════════════════════════════════════════════════════════\n");
    
    println!("📊 Statistics:");
    println!("   ├─ Total files: {}", stats.total_files);
    println!("   ├─ Total chunks: {}", stats.total_chunks);
    println!("   ├─ Total size: {:.2} MB", stats.total_bytes as f64 / 1_048_576.0);
    println!("   └─ Time taken: {:.2} seconds", duration.as_secs_f64());
    
    println!("\n📈 File types indexed:");
    let mut file_types: Vec<_> = stats.file_types.iter()
        .map(|entry| (entry.key().clone(), *entry.value()))
        .collect();
    file_types.sort_by(|a, b| b.1.cmp(&a.1));
    
    for (file_type, count) in file_types.iter().take(10) {
        let bar_length = (*count as f64 / file_types[0].1 as f64 * 30.0) as usize;
        let bar = "█".repeat(bar_length);
        println!("   {:12} {} {}", format!("{:?}", file_type), bar, count);
    }
    
    println!("\n⚡ Performance:");
    println!("   ├─ Files/second: {:.1}", stats.total_files as f64 / duration.as_secs_f64());
    println!("   ├─ MB/second: {:.2}", (stats.total_bytes as f64 / 1_048_576.0) / duration.as_secs_f64());
    println!("   └─ Chunks/second: {:.1}", stats.total_chunks as f64 / duration.as_secs_f64());
    
    if !stats.errors.is_empty() {
        println!("\n⚠️  Errors encountered: {}", stats.errors.len());
        for (i, error) in stats.errors.iter().take(5).enumerate() {
            println!("   {}. {}", i + 1, error);
        }
        if stats.errors.len() > 5 {
            println!("   ... and {} more", stats.errors.len() - 5);
        }
    }
    
    // Keep running if watching
    if args.watch {
        println!("\n👁️  Watching for file changes... Press Ctrl+C to stop");
        tokio::signal::ctrl_c().await?;
        println!("\n👋 Shutting down...");
    }
    
    println!("\n✨ Your codebase is now searchable with TurboRAG!");
    println!("   Use the search API to find anything instantly.");
    
    Ok(())
}

/// Estimate directory size
async fn estimate_directory_size(path: &PathBuf) -> Result<u64> {
    let mut total_size = 0u64;
    let mut entries = tokio::fs::read_dir(path).await?;
    
    while let Some(entry) = entries.next_entry().await? {
        let metadata = entry.metadata().await?;
        if metadata.is_file() {
            total_size += metadata.len();
        } else if metadata.is_dir() {
            // Recursively estimate subdirectories (limited depth)
            if let Ok(subdir_size) = Box::pin(estimate_directory_size(&entry.path())).await {
                total_size += subdir_size;
            }
        }
    }
    
    Ok(total_size)
}

/// Create mock security manager for local testing
async fn create_mock_security() -> Result<SecurityManager> {
    // Create in-memory SQLite for testing
    let pool = PgPool::connect("sqlite::memory:").await?;
    
    // Create minimal schema
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            clearance_level TEXT DEFAULT 'public'
        );
        
        CREATE TABLE IF NOT EXISTS documents (
            id TEXT PRIMARY KEY,
            classification TEXT DEFAULT 'public'
        );
        
        CREATE TABLE IF NOT EXISTS document_acls (
            document_id TEXT,
            user_id TEXT,
            permission_type TEXT,
            granted_by TEXT,
            granted_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            expires_at TIMESTAMP,
            PRIMARY KEY (document_id, user_id, permission_type)
        );
        
        CREATE TABLE IF NOT EXISTS audit_logs (
            id TEXT PRIMARY KEY,
            timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            event_type TEXT,
            user_id TEXT,
            session_id TEXT,
            resource_id TEXT,
            resource_type TEXT,
            action TEXT,
            result TEXT,
            ip_address TEXT,
            user_agent TEXT,
            location TEXT,
            details TEXT,
            risk_score REAL,
            hash TEXT,
            previous_hash TEXT,
            signature BLOB
        );
        "#
    ).execute(&pool).await?;
    
    let config = SecurityConfig {
        audit_config: AuditConfig {
            retention_days: 7,
            batch_size: 10,
            flush_interval_secs: 60,
            sign_logs: false,
            encrypt_sensitive: false,
            compliance_mode: ComplianceMode::None,
        },
        encryption_config: EncryptionConfig::default(),
        cache_config: CacheConfig { size: 1000 },
    };
    
    SecurityManager::new(pool, config).await
}