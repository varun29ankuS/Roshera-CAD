//! Layered caching system for ultra-fast retrieval
//!
//! Implements a multi-level cache:
//! - L1: Thread-local cache (nanoseconds)
//! - L2: Process-wide cache (microseconds)
//! - L3: Distributed cache (milliseconds)

pub mod networking;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::RwLock;

/// Layered cache implementation
pub struct LayeredCache {
    l1_local: Arc<DashMap<String, CachedValue>>,
    l2_process: Arc<DashMap<String, CachedValue>>,
    l3_distributed: Option<Arc<TurboCache>>,
    config: CacheConfig,
}

/// Distributed cache implementation
pub struct TurboCache {
    local: Arc<RwLock<HashMap<String, CachedValue>>>,
    peers: Vec<PeerNode>,
    gossip: GossipProtocol,
    wal: WriteAheadLog,
    node_id: String,
}

/// Cached value with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedValue {
    pub data: Vec<u8>,
    #[serde(skip, default = "Instant::now")]
    pub inserted_at: Instant,  // Not serialized, recreated on deserialization
    pub inserted_at_system: SystemTime,  // Serializable timestamp
    pub ttl: Duration,
    pub hit_count: u64,
}

/// Peer node for distributed caching
#[derive(Debug, Clone)]
pub struct PeerNode {
    pub id: String,
    pub address: String,
    pub last_seen: Instant,
}

/// Gossip protocol for cache synchronization
pub struct GossipProtocol {
    node_id: String,
    peers: Arc<RwLock<Vec<PeerNode>>>,
    gossip_interval: Duration,
}

/// Write-ahead log for durability
pub struct WriteAheadLog {
    path: std::path::PathBuf,
    current_file: Arc<RwLock<std::fs::File>>,
}

/// Cache configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    pub max_size_mb: usize,
    pub ttl_seconds: u64,
    pub enable_distributed: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_size_mb: 100,  // 100MB default cache size
            ttl_seconds: 3600,  // 1 hour TTL
            enable_distributed: false,  // Local cache by default
        }
    }
}

impl LayeredCache {
    /// Create new layered cache
    pub fn new(config: CacheConfig) -> Self {
        let l3_distributed = if config.enable_distributed {
            Some(Arc::new(TurboCache::new()))
        } else {
            None
        };

        Self {
            l1_local: Arc::new(DashMap::new()),
            l2_process: Arc::new(DashMap::new()),
            l3_distributed,
            config,
        }
    }

    /// Get value from cache
    pub async fn get(&self, key: &str) -> Option<Vec<u8>> {
        // Check L1
        if let Some(value) = self.l1_local.get(key) {
            if !self.is_expired(&value) {
                return Some(value.data.clone());
            }
        }

        // Check L2
        if let Some(value) = self.l2_process.get(key) {
            if !self.is_expired(&value) {
                // Promote to L1
                self.l1_local.insert(key.to_string(), value.clone());
                return Some(value.data.clone());
            }
        }

        // Check L3
        if let Some(cache) = &self.l3_distributed {
            if let Some(value) = cache.get(key).await {
                // Promote to L1 and L2
                let cached = CachedValue {
                    data: value.clone(),
                    inserted_at: Instant::now(),
                    inserted_at_system: SystemTime::now(),
                    ttl: Duration::from_secs(self.config.ttl_seconds),
                    hit_count: 1,
                };
                self.l1_local.insert(key.to_string(), cached.clone());
                self.l2_process.insert(key.to_string(), cached);
                return Some(value);
            }
        }

        None
    }

    /// Set value in cache
    pub async fn set(&self, key: String, value: Vec<u8>) {
        let cached = CachedValue {
            data: value.clone(),
            inserted_at: Instant::now(),
            inserted_at_system: SystemTime::now(),
            ttl: Duration::from_secs(self.config.ttl_seconds),
            hit_count: 0,
        };

        // Set in all levels
        self.l1_local.insert(key.clone(), cached.clone());
        self.l2_process.insert(key.clone(), cached.clone());

        if let Some(cache) = &self.l3_distributed {
            cache.set(key, value).await;
        }
    }

    fn is_expired(&self, value: &CachedValue) -> bool {
        value.inserted_at.elapsed() > value.ttl
    }
}

impl TurboCache {
    /// Create new distributed cache
    pub fn new() -> Self {
        Self {
            local: Arc::new(RwLock::new(HashMap::new())),
            peers: vec![],
            gossip: GossipProtocol::new(),
            wal: WriteAheadLog::new("./rag_data/wal"),
            node_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    /// Get value from distributed cache
    pub async fn get(&self, key: &str) -> Option<Vec<u8>> {
        // Check local first
        if let Some(value) = self.local.read().await.get(key) {
            return Some(value.data.clone());
        }

        // Ask peers
        for peer in &self.peers {
            if let Ok(value) = self.get_from_peer(peer, key).await {
                // Cache locally
                self.local.write().await.insert(
                    key.to_string(),
                    CachedValue {
                        data: value.clone(),
                        inserted_at: Instant::now(),
                        inserted_at_system: SystemTime::now(),
                        ttl: Duration::from_secs(3600),
                        hit_count: 1,
                    },
                );
                return Some(value);
            }
        }

        None
    }

    /// Set value in distributed cache
    pub async fn set(&self, key: String, value: Vec<u8>) {
        // Write to WAL
        self.wal.append(&key, &value).await.ok();

        // Update local
        self.local.write().await.insert(
            key.clone(),
            CachedValue {
                data: value.clone(),
                inserted_at: Instant::now(),
                inserted_at_system: SystemTime::now(),
                ttl: Duration::from_secs(3600),
                hit_count: 0,
            },
        );

        // Gossip to peers
        self.gossip.broadcast(CacheOp::Set { key, value }).await;
    }

    async fn get_from_peer(&self, peer: &PeerNode, key: &str) -> anyhow::Result<Vec<u8>> {
        use networking::NetworkedTurboCache;
        let cache = NetworkedTurboCache::new(self.node_id.clone());
        cache.get_from_peer(peer, key).await
    }
}

impl GossipProtocol {
    pub fn new() -> Self {
        Self {
            node_id: uuid::Uuid::new_v4().to_string(),
            peers: Arc::new(RwLock::new(vec![])),
            gossip_interval: Duration::from_secs(5),
        }
    }

    pub async fn broadcast(&self, op: CacheOp) {
        // Broadcast to all peers
        let peers = self.peers.read().await;
        for peer in peers.iter() {
            // Send operation to peer
            self.send_to_peer(peer, &op).await.ok();
        }
    }

    async fn send_to_peer(&self, peer: &PeerNode, op: &CacheOp) -> anyhow::Result<()> {
        use networking::{NetworkedTurboCache, NetworkMessage};
        let cache = NetworkedTurboCache::new(self.node_id.clone());
        cache.broadcast(op.clone()).await
    }
}

impl WriteAheadLog {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        let path = path.into();
        std::fs::create_dir_all(&path).ok();
        let file_path = path.join("wal.log");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
            .unwrap();

        Self {
            path,
            current_file: Arc::new(RwLock::new(file)),
        }
    }

    pub async fn append(&self, key: &str, value: &[u8]) -> anyhow::Result<()> {
        use std::io::Write;
        
        let entry = WalEntry {
            timestamp: chrono::Utc::now(),
            key: key.to_string(),
            value: value.to_vec(),
        };

        let serialized = bincode::serialize(&entry)?;
        let mut file = self.current_file.write().await;
        file.write_all(&serialized)?;
        file.sync_all()?;

        Ok(())
    }
}

/// Cache operation for gossip
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CacheOp {
    Get { key: String },
    Set { key: String, value: Vec<u8> },
    Delete { key: String },
    Clear,
}

/// WAL entry
#[derive(Debug, Serialize, Deserialize)]
struct WalEntry {
    timestamp: chrono::DateTime<chrono::Utc>,
    key: String,
    value: Vec<u8>,
}