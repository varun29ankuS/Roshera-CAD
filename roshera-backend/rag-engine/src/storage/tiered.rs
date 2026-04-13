/// Enterprise Tiered Storage System
/// 
/// Implements Hot/Warm/Cold storage tiers for optimal performance and cost
/// Hot: Redis Cluster (last 24 hours)
/// Warm: PostgreSQL with pgvector (6 months)
/// Cold: S3/MinIO (archive)

use std::sync::Arc;
use std::time::{Duration, Instant};
use std::collections::HashMap;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use tokio::sync::RwLock;
use std::sync::atomic::{AtomicUsize, AtomicU64, Ordering};

// Storage backends
use redis::{Client as RedisClient, AsyncCommands, aio::ConnectionManager};
use sqlx::{PgPool, postgres::PgRow};
use aws_sdk_s3::{Client as S3Client, primitives::ByteStream};
use lz4_flex;
use zstd;

use crate::security::{UserId, DocumentId, SecurityManager, UserContext, Permission};
use crate::search::vamana::VamanaIndex;

/// Storage tier configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TieredStorageConfig {
    // Hot tier settings
    pub hot_ttl: Duration,
    pub hot_max_size_gb: usize,
    pub redis_urls: Vec<String>,
    
    // Warm tier settings
    pub warm_ttl: Duration,
    pub warm_max_size_gb: usize,
    pub postgres_url: String,
    pub postgres_pool_size: u32,
    
    // Cold tier settings
    pub s3_bucket: String,
    pub s3_region: String,
    pub s3_endpoint: Option<String>, // For MinIO
    pub compression: CompressionType,
    
    // Performance settings
    pub cache_size_mb: usize,
    pub prefetch_enabled: bool,
    pub async_promotion: bool,
}

/// Compression types for cold storage
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum CompressionType {
    None,
    Lz4,
    Zstd,
    Snappy,
}

/// Document with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: DocumentId,
    pub content: Vec<u8>,
    pub metadata: DocumentMetadata,
    pub embeddings: Option<Vec<Vec<f32>>>,
    pub entities: Option<Vec<ExtractedEntity>>,
}

/// Document metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentMetadata {
    pub title: Option<String>,
    pub author: UserId,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    pub accessed_at: DateTime<Utc>,
    pub access_count: u64,
    pub size_bytes: usize,
    pub content_type: String,
    pub language: String,
    pub classification: String,
    pub tags: Vec<String>,
    pub source: String,
    pub checksum: String,
}

/// Extracted entity reference
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedEntity {
    pub entity_type: String,
    pub text: String,
    pub confidence: f32,
}

/// Storage statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageStats {
    pub hot_count: usize,
    pub hot_size_bytes: usize,
    pub warm_count: usize,
    pub warm_size_bytes: usize,
    pub cold_count: usize,
    pub cold_size_bytes: usize,
    pub total_reads: u64,
    pub total_writes: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub avg_latency_ms: f64,
}

/// Main tiered storage system
pub struct TieredStorage {
    hot_tier: Arc<HotTier>,
    warm_tier: Arc<WarmTier>,
    cold_tier: Arc<ColdTier>,
    tier_manager: Arc<TierManager>,
    cache: Arc<L2Cache>,
    stats: Arc<StorageStats>,
    security: Arc<SecurityManager>,
}

impl TieredStorage {
    pub async fn new(
        config: TieredStorageConfig,
        security: Arc<SecurityManager>,
    ) -> Result<Self> {
        let hot_tier = Arc::new(HotTier::new(&config).await?);
        let warm_tier = Arc::new(WarmTier::new(&config).await?);
        let cold_tier = Arc::new(ColdTier::new(&config).await?);
        
        let cache = Arc::new(L2Cache::new(config.cache_size_mb));
        let stats = Arc::new(StorageStats::default());
        
        let tier_manager = Arc::new(TierManager::new(
            hot_tier.clone(),
            warm_tier.clone(),
            cold_tier.clone(),
            config.clone(),
        ));
        
        // Start background tier management
        let manager_clone = tier_manager.clone();
        tokio::spawn(async move {
            manager_clone.run_management_loop().await;
        });
        
        Ok(Self {
            hot_tier,
            warm_tier,
            cold_tier,
            tier_manager,
            cache,
            stats,
            security,
        })
    }
    
    /// Store a document
    pub async fn store(
        &self,
        context: &UserContext,
        document: Document,
    ) -> Result<()> {
        // Check write permission
        let can_write = self.security.check_access(
            context,
            document.id,
            Permission::Write,
        ).await?;
        
        if !matches!(can_write, crate::security::AccessDecision::Allow) {
            return Err(anyhow!("Permission denied"));
        }
        
        // Always store in hot tier first
        self.hot_tier.store(document.clone()).await?;
        
        // Update cache
        self.cache.put(document.id, document.clone());
        
        // Update stats
        self.stats.total_writes.fetch_add(1, Ordering::Relaxed);
        
        Ok(())
    }
    
    /// Retrieve a document
    pub async fn get(
        &self,
        context: &UserContext,
        doc_id: DocumentId,
    ) -> Result<Option<Document>> {
        // Check read permission
        let can_read = self.security.check_access(
            context,
            doc_id,
            Permission::Read,
        ).await?;
        
        if !matches!(can_read, crate::security::AccessDecision::Allow) {
            return Err(anyhow!("Permission denied"));
        }
        
        let start = Instant::now();
        
        // Check L2 cache first
        if let Some(doc) = self.cache.get(&doc_id) {
            self.stats.cache_hits.fetch_add(1, Ordering::Relaxed);
            self.update_latency(start.elapsed());
            return Ok(Some(doc));
        }
        
        self.stats.cache_misses.fetch_add(1, Ordering::Relaxed);
        
        // Try hot tier
        if let Some(doc) = self.hot_tier.get(doc_id).await? {
            self.cache.put(doc_id, doc.clone());
            self.update_latency(start.elapsed());
            return Ok(Some(doc));
        }
        
        // Try warm tier
        if let Some(doc) = self.warm_tier.get(doc_id).await? {
            // Optionally promote to hot tier
            if self.should_promote(&doc) {
                self.tier_manager.promote_to_hot(doc.clone()).await?;
            }
            self.cache.put(doc_id, doc.clone());
            self.update_latency(start.elapsed());
            return Ok(Some(doc));
        }
        
        // Try cold tier
        if let Some(doc) = self.cold_tier.get(doc_id).await? {
            // Promote to warm tier for future access
            self.tier_manager.promote_to_warm(doc.clone()).await?;
            self.cache.put(doc_id, doc.clone());
            self.update_latency(start.elapsed());
            return Ok(Some(doc));
        }
        
        self.update_latency(start.elapsed());
        Ok(None)
    }
    
    /// Search across all tiers
    pub async fn search(
        &self,
        context: &UserContext,
        query_embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<Document>> {
        let start = Instant::now();
        
        // Get accessible documents for user
        let accessible_docs = self.security.get_accessible_documents(
            context,
            Permission::Read,
        ).await?;
        
        // Search hot tier first (most relevant)
        let mut results = self.hot_tier.search(query_embedding, limit * 2).await?;
        
        // If not enough results, search warm tier
        if results.len() < limit {
            let warm_results = self.warm_tier.search(
                query_embedding,
                limit * 2 - results.len(),
            ).await?;
            results.extend(warm_results);
        }
        
        // Filter by permissions
        results.retain(|doc| accessible_docs.contains(&doc.id));
        
        // Limit results
        results.truncate(limit);
        
        self.update_latency(start.elapsed());
        Ok(results)
    }
    
    /// Delete a document from all tiers
    pub async fn delete(
        &self,
        context: &UserContext,
        doc_id: DocumentId,
    ) -> Result<()> {
        // Check delete permission
        let can_delete = self.security.check_access(
            context,
            doc_id,
            Permission::Delete,
        ).await?;
        
        if !matches!(can_delete, crate::security::AccessDecision::Allow) {
            return Err(anyhow!("Permission denied"));
        }
        
        // Delete from all tiers
        self.hot_tier.delete(doc_id).await?;
        self.warm_tier.delete(doc_id).await?;
        self.cold_tier.delete(doc_id).await?;
        
        // Remove from cache
        self.cache.remove(&doc_id);
        
        Ok(())
    }
    
    /// Get storage statistics
    pub async fn get_stats(&self) -> StorageStats {
        StorageStats {
            hot_count: self.hot_tier.count().await,
            hot_size_bytes: self.hot_tier.size_bytes().await,
            warm_count: self.warm_tier.count().await,
            warm_size_bytes: self.warm_tier.size_bytes().await,
            cold_count: self.cold_tier.count().await,
            cold_size_bytes: self.cold_tier.size_bytes().await,
            total_reads: self.stats.total_reads.load(Ordering::Relaxed),
            total_writes: self.stats.total_writes.load(Ordering::Relaxed),
            cache_hits: self.stats.cache_hits.load(Ordering::Relaxed),
            cache_misses: self.stats.cache_misses.load(Ordering::Relaxed),
            avg_latency_ms: self.get_avg_latency(),
        }
    }
    
    // Helper methods
    
    fn should_promote(&self, doc: &Document) -> bool {
        // Promote if accessed frequently
        doc.metadata.access_count > 10 ||
        // Or if recently modified
        (Utc::now() - doc.metadata.modified_at).num_hours() < 24
    }
    
    fn update_latency(&self, duration: Duration) {
        // Simple moving average (implement properly in production)
        let ms = duration.as_millis() as f64;
        // Update average latency
    }
    
    fn get_avg_latency(&self) -> f64 {
        // Return average latency
        5.0 // Placeholder
    }
}

/// Hot tier - Redis Cluster
pub struct HotTier {
    redis_clients: Vec<ConnectionManager>,
    current_client: AtomicUsize,
    ttl: Duration,
    max_size: usize,
    current_size: AtomicUsize,
}

impl HotTier {
    pub async fn new(config: &TieredStorageConfig) -> Result<Self> {
        let mut clients = Vec::new();
        
        for url in &config.redis_urls {
            let client = RedisClient::open(url.as_str())?;
            let conn = ConnectionManager::new(client).await?;
            clients.push(conn);
        }
        
        Ok(Self {
            redis_clients: clients,
            current_client: AtomicUsize::new(0),
            ttl: config.hot_ttl,
            max_size: config.hot_max_size_gb * 1024 * 1024 * 1024,
            current_size: AtomicUsize::new(0),
        })
    }
    
    pub async fn store(&self, document: Document) -> Result<()> {
        let key = format!("doc:{}", document.id.0);
        let value = bincode::serialize(&document)?;
        
        // Check size limit
        if self.current_size.load(Ordering::Relaxed) + value.len() > self.max_size {
            self.evict_lru().await?;
        }
        
        // Round-robin client selection
        let client_idx = self.current_client.fetch_add(1, Ordering::Relaxed) % self.redis_clients.len();
        let mut conn = self.redis_clients[client_idx].clone();
        
        // Store with TTL
        conn.set_ex(key, value.clone(), self.ttl.as_secs() as usize).await?;
        
        // Update size tracking
        self.current_size.fetch_add(value.len(), Ordering::Relaxed);
        
        Ok(())
    }
    
    pub async fn get(&self, doc_id: DocumentId) -> Result<Option<Document>> {
        let key = format!("doc:{}", doc_id.0);
        
        // Try all Redis nodes
        for conn in &self.redis_clients {
            let mut conn = conn.clone();
            if let Ok(value) = conn.get::<_, Vec<u8>>(&key).await {
                let doc: Document = bincode::deserialize(&value)?;
                
                // Update access time
                self.update_access_time(doc_id).await?;
                
                return Ok(Some(doc));
            }
        }
        
        Ok(None)
    }
    
    pub async fn search(&self, embedding: &[f32], limit: usize) -> Result<Vec<Document>> {
        // In production, use RedisSearch or RedisAI for vector search
        // For now, return empty
        Ok(Vec::new())
    }
    
    pub async fn delete(&self, doc_id: DocumentId) -> Result<()> {
        let key = format!("doc:{}", doc_id.0);
        
        for conn in &self.redis_clients {
            let mut conn = conn.clone();
            let _: Option<()> = conn.del(&key).await?;
        }
        
        Ok(())
    }
    
    pub async fn count(&self) -> usize {
        // Get count from all nodes
        let mut total = 0;
        for conn in &self.redis_clients {
            let mut conn = conn.clone();
            if let Ok(count) = conn.dbsize().await {
                total += count;
            }
        }
        total
    }
    
    pub async fn size_bytes(&self) -> usize {
        self.current_size.load(Ordering::Relaxed)
    }
    
    async fn evict_lru(&self) -> Result<()> {
        // Implement LRU eviction
        // In production, use Redis's built-in LRU
        Ok(())
    }
    
    async fn update_access_time(&self, doc_id: DocumentId) -> Result<()> {
        let key = format!("access:{}", doc_id.0);
        let mut conn = self.redis_clients[0].clone();
        conn.set(key, Utc::now().timestamp()).await?;
        Ok(())
    }
}

/// Warm tier - PostgreSQL with pgvector
pub struct WarmTier {
    pool: PgPool,
    vamana: Arc<VamanaIndex>,
    ttl: Duration,
    max_size: usize,
}

impl WarmTier {
    pub async fn new(config: &TieredStorageConfig) -> Result<Self> {
        let pool = PgPool::connect(&config.postgres_url).await?;
        
        // Initialize Vamana index
        let vamana = Arc::new(VamanaIndex::new(
            64,  // R
            100, // L
            1.2, // alpha
            crate::search::vamana::DistanceFunction::Cosine,
            true, // normalize
            true, // use SQ
        ));
        
        // Load existing vectors into Vamana
        let vectors = sqlx::query!(
            "SELECT chunk_id, chunk_embedding FROM chunks WHERE created_at > NOW() - INTERVAL '6 months'"
        )
        .fetch_all(&pool)
        .await?;
        
        for row in vectors {
            if let Some(embedding) = row.chunk_embedding {
                // Convert pgvector to Vec<f32>
                let vec: Vec<f32> = vec![]; // Parse from pgvector format
                vamana.insert(vec);
            }
        }
        
        Ok(Self {
            pool,
            vamana,
            ttl: config.warm_ttl,
            max_size: config.warm_max_size_gb * 1024 * 1024 * 1024,
        })
    }
    
    pub async fn store(&self, document: Document) -> Result<()> {
        let doc_bytes = bincode::serialize(&document)?;
        
        sqlx::query!(
            r#"
            INSERT INTO documents (id, content, metadata, size_bytes, created_at)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (id) DO UPDATE SET
                content = $2,
                metadata = $3,
                modified_at = NOW()
            "#,
            document.id.0,
            doc_bytes,
            serde_json::to_value(&document.metadata)?,
            document.metadata.size_bytes as i64,
            document.metadata.created_at
        )
        .execute(&self.pool)
        .await?;
        
        // Store embeddings if present
        if let Some(embeddings) = &document.embeddings {
            for (i, embedding) in embeddings.iter().enumerate() {
                // Store in Vamana
                let vec_id = self.vamana.insert(embedding.clone());
                
                // Store mapping in PostgreSQL
                sqlx::query!(
                    r#"
                    INSERT INTO document_vectors (document_id, vector_id, embedding)
                    VALUES ($1, $2, $3)
                    "#,
                    document.id.0,
                    vec_id as i64,
                    embedding as _
                )
                .execute(&self.pool)
                .await?;
            }
        }
        
        Ok(())
    }
    
    pub async fn get(&self, doc_id: DocumentId) -> Result<Option<Document>> {
        let row = sqlx::query!(
            r#"
            SELECT content, metadata
            FROM documents
            WHERE id = $1 AND deleted_at IS NULL
            "#,
            doc_id.0
        )
        .fetch_optional(&self.pool)
        .await?;
        
        if let Some(row) = row {
            let doc: Document = bincode::deserialize(&row.content)?;
            
            // Update access count
            sqlx::query!(
                r#"
                UPDATE documents 
                SET accessed_at = NOW(), access_count = access_count + 1
                WHERE id = $1
                "#,
                doc_id.0
            )
            .execute(&self.pool)
            .await?;
            
            Ok(Some(doc))
        } else {
            Ok(None)
        }
    }
    
    pub async fn search(&self, embedding: &[f32], limit: usize) -> Result<Vec<Document>> {
        // Search using Vamana
        let results = self.vamana.search(embedding, limit, None);
        
        // Get document IDs from vector IDs
        let mut documents = Vec::new();
        for result in results {
            let doc = sqlx::query!(
                r#"
                SELECT d.content
                FROM documents d
                JOIN document_vectors dv ON d.id = dv.document_id
                WHERE dv.vector_id = $1
                "#,
                result.id as i64
            )
            .fetch_optional(&self.pool)
            .await?;
            
            if let Some(row) = doc {
                let document: Document = bincode::deserialize(&row.content)?;
                documents.push(document);
            }
        }
        
        Ok(documents)
    }
    
    pub async fn delete(&self, doc_id: DocumentId) -> Result<()> {
        sqlx::query!(
            "UPDATE documents SET deleted_at = NOW() WHERE id = $1",
            doc_id.0
        )
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
    
    pub async fn count(&self) -> usize {
        let count = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM documents WHERE deleted_at IS NULL"
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        
        count as usize
    }
    
    pub async fn size_bytes(&self) -> usize {
        let size = sqlx::query_scalar!(
            "SELECT COALESCE(SUM(size_bytes), 0) FROM documents WHERE deleted_at IS NULL"
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        
        size as usize
    }
}

/// Cold tier - S3/MinIO
pub struct ColdTier {
    s3_client: S3Client,
    bucket: String,
    compression: CompressionType,
}

impl ColdTier {
    pub async fn new(config: &TieredStorageConfig) -> Result<Self> {
        let sdk_config = aws_config::from_env()
            .region(aws_config::Region::new(config.s3_region.clone()))
            .load()
            .await;
        
        let s3_client = if let Some(endpoint) = &config.s3_endpoint {
            // MinIO configuration
            let s3_config = aws_sdk_s3::config::Builder::from(&sdk_config)
                .endpoint_url(endpoint)
                .force_path_style(true)
                .build();
            S3Client::from_conf(s3_config)
        } else {
            S3Client::new(&sdk_config)
        };
        
        Ok(Self {
            s3_client,
            bucket: config.s3_bucket.clone(),
            compression: config.compression,
        })
    }
    
    pub async fn store(&self, document: Document) -> Result<()> {
        let doc_bytes = bincode::serialize(&document)?;
        
        // Compress based on configuration
        let compressed = match self.compression {
            CompressionType::None => doc_bytes,
            CompressionType::Lz4 => lz4_flex::compress_prepend_size(&doc_bytes),
            CompressionType::Zstd => zstd::encode_all(&doc_bytes[..], 3)?,
            CompressionType::Snappy => {
                let mut encoder = snap::raw::Encoder::new();
                encoder.compress_vec(&doc_bytes)?
            }
        };
        
        // Generate S3 key with date partitioning
        let key = format!(
            "documents/{}/{}/{}.bin",
            document.metadata.created_at.format("%Y/%m/%d"),
            document.id.0
        );
        
        // Upload to S3
        self.s3_client
            .put_object()
            .bucket(&self.bucket)
            .key(&key)
            .body(ByteStream::from(compressed))
            .metadata("compression", self.compression_to_string())
            .metadata("original_size", doc_bytes.len().to_string())
            .metadata("created_at", document.metadata.created_at.to_rfc3339())
            .send()
            .await?;
        
        Ok(())
    }
    
    pub async fn get(&self, doc_id: DocumentId) -> Result<Option<Document>> {
        // List objects with prefix to find document
        let prefix = format!("documents/");
        let response = self.s3_client
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(&prefix)
            .send()
            .await?;
        
        // Find matching document
        for object in response.contents().unwrap_or_default() {
            if let Some(key) = object.key() {
                if key.contains(&doc_id.0.to_string()) {
                    // Get object
                    let obj = self.s3_client
                        .get_object()
                        .bucket(&self.bucket)
                        .key(key)
                        .send()
                        .await?;
                    
                    let compressed = obj.body.collect().await?.to_vec();
                    
                    // Decompress
                    let decompressed = match self.compression {
                        CompressionType::None => compressed,
                        CompressionType::Lz4 => lz4_flex::decompress_size_prepended(&compressed)?,
                        CompressionType::Zstd => zstd::decode_all(&compressed[..])?,...
                        CompressionType::Snappy => {
                            let mut decoder = snap::raw::Decoder::new();
                            decoder.decompress_vec(&compressed)?
                        }
                    };
                    
                    let document: Document = bincode::deserialize(&decompressed)?;
                    return Ok(Some(document));
                }
            }
        }
        
        Ok(None)
    }
    
    pub async fn delete(&self, doc_id: DocumentId) -> Result<()> {
        // Find and delete object
        let prefix = format!("documents/");
        let response = self.s3_client
            .list_objects_v2()
            .bucket(&self.bucket)
            .prefix(&prefix)
            .send()
            .await?;
        
        for object in response.contents().unwrap_or_default() {
            if let Some(key) = object.key() {
                if key.contains(&doc_id.0.to_string()) {
                    self.s3_client
                        .delete_object()
                        .bucket(&self.bucket)
                        .key(key)
                        .send()
                        .await?;
                    break;
                }
            }
        }
        
        Ok(())
    }
    
    pub async fn count(&self) -> usize {
        // Approximate count using S3 metrics
        0 // Implement with CloudWatch metrics in production
    }
    
    pub async fn size_bytes(&self) -> usize {
        // Get from S3 metrics
        0 // Implement with CloudWatch metrics in production
    }
    
    fn compression_to_string(&self) -> &str {
        match self.compression {
            CompressionType::None => "none",
            CompressionType::Lz4 => "lz4",
            CompressionType::Zstd => "zstd",
            CompressionType::Snappy => "snappy",
        }
    }
}

/// Tier management - handles promotion/demotion
pub struct TierManager {
    hot: Arc<HotTier>,
    warm: Arc<WarmTier>,
    cold: Arc<ColdTier>,
    config: TieredStorageConfig,
}

impl TierManager {
    pub fn new(
        hot: Arc<HotTier>,
        warm: Arc<WarmTier>,
        cold: Arc<ColdTier>,
        config: TieredStorageConfig,
    ) -> Self {
        Self { hot, warm, cold, config }
    }
    
    pub async fn run_management_loop(&self) {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        
        loop {
            interval.tick().await;
            
            // Run tier management tasks
            if let Err(e) = self.manage_tiers().await {
                eprintln!("Tier management error: {}", e);
            }
        }
    }
    
    async fn manage_tiers(&self) -> Result<()> {
        // Demote old documents from hot to warm
        self.demote_from_hot().await?;
        
        // Archive old documents from warm to cold
        self.archive_from_warm().await?;
        
        // Clean up expired documents
        self.cleanup_expired().await?;
        
        Ok(())
    }
    
    async fn demote_from_hot(&self) -> Result<()> {
        // Get documents older than hot TTL
        // Move them to warm tier
        Ok(())
    }
    
    async fn archive_from_warm(&self) -> Result<()> {
        // Get documents older than warm TTL
        // Move them to cold tier
        Ok(())
    }
    
    async fn cleanup_expired(&self) -> Result<()> {
        // Delete documents past retention period
        Ok(())
    }
    
    pub async fn promote_to_hot(&self, document: Document) -> Result<()> {
        self.hot.store(document).await
    }
    
    pub async fn promote_to_warm(&self, document: Document) -> Result<()> {
        self.warm.store(document).await
    }
}

/// L2 Cache for frequently accessed documents
pub struct L2Cache {
    cache: DashMap<DocumentId, Document>,
    max_size: usize,
    current_size: AtomicUsize,
}

impl L2Cache {
    pub fn new(max_size_mb: usize) -> Self {
        Self {
            cache: DashMap::new(),
            max_size: max_size_mb * 1024 * 1024,
            current_size: AtomicUsize::new(0),
        }
    }
    
    pub fn get(&self, id: &DocumentId) -> Option<Document> {
        self.cache.get(id).map(|entry| entry.clone())
    }
    
    pub fn put(&self, id: DocumentId, document: Document) {
        let size = std::mem::size_of_val(&document);
        
        // Check if we need to evict
        if self.current_size.load(Ordering::Relaxed) + size > self.max_size {
            self.evict_lru();
        }
        
        self.cache.insert(id, document);
        self.current_size.fetch_add(size, Ordering::Relaxed);
    }
    
    pub fn remove(&self, id: &DocumentId) {
        if let Some((_, doc)) = self.cache.remove(id) {
            let size = std::mem::size_of_val(&doc);
            self.current_size.fetch_sub(size, Ordering::Relaxed);
        }
    }
    
    fn evict_lru(&self) {
        // Simple eviction: remove 10% of cache
        let to_remove = self.cache.len() / 10;
        let mut removed = 0;
        
        for entry in self.cache.iter() {
            if removed >= to_remove {
                break;
            }
            self.cache.remove(entry.key());
            removed += 1;
        }
    }
}

impl Default for StorageStats {
    fn default() -> Self {
        Self {
            hot_count: 0,
            hot_size_bytes: 0,
            warm_count: 0,
            warm_size_bytes: 0,
            cold_count: 0,
            cold_size_bytes: 0,
            total_reads: 0,
            total_writes: 0,
            cache_hits: 0,
            cache_misses: 0,
            avg_latency_ms: 0.0,
        }
    }
}