//! Complete networking implementation for distributed cache
//!
//! Implements TCP-based peer communication with connection pooling,
//! health checks, and automatic retry logic.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use std::collections::HashMap;
use tokio::net::{TcpStream, TcpListener};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{RwLock, Mutex};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use bytes::{Bytes, BytesMut};
use serde::{Serialize, Deserialize};
use futures::SinkExt;
use futures::stream::StreamExt;

use super::{CacheOp, PeerNode, CachedValue};

/// Connection pool for peer connections
pub struct ConnectionPool {
    connections: Arc<DashMap<String, Arc<Mutex<PeerConnection>>>>,
    max_connections: usize,
    connection_timeout: Duration,
    retry_policy: RetryPolicy,
}

/// Individual peer connection
pub struct PeerConnection {
    stream: Framed<TcpStream, LengthDelimitedCodec>,
    peer_id: String,
    last_used: Instant,
    healthy: bool,
}

/// Retry policy for failed connections
#[derive(Clone)]
pub struct RetryPolicy {
    max_retries: u32,
    initial_backoff: Duration,
    max_backoff: Duration,
    backoff_multiplier: f64,
}

/// Network message types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetworkMessage {
    /// Cache operation request
    CacheRequest {
        id: u64,
        operation: CacheOp,
    },
    /// Cache operation response
    CacheResponse {
        id: u64,
        result: CacheResult,
    },
    /// Gossip protocol message
    GossipMessage {
        from: String,
        operation: CacheOp,
        vector_clock: VectorClock,
    },
    /// Health check ping
    Ping {
        timestamp: i64,
    },
    /// Health check pong
    Pong {
        timestamp: i64,
    },
}

/// Cache operation result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CacheResult {
    Success(Option<Vec<u8>>),
    Error(String),
    NotFound,
}

/// Vector clock for distributed consistency
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorClock {
    clocks: HashMap<String, u64>,
}

impl ConnectionPool {
    /// Create new connection pool
    pub fn new(max_connections: usize) -> Self {
        Self {
            connections: Arc::new(DashMap::new()),
            max_connections,
            connection_timeout: Duration::from_secs(5),
            retry_policy: RetryPolicy::default(),
        }
    }

    /// Get or create connection to peer
    pub async fn get_connection(&self, peer: &PeerNode) -> anyhow::Result<Arc<Mutex<PeerConnection>>> {
        // Check if we have an existing healthy connection
        if let Some(conn) = self.connections.get(&peer.id) {
            let is_healthy = {
                let c = conn.lock().await;
                c.healthy && c.last_used.elapsed() < Duration::from_secs(60)
            };
            
            if is_healthy {
                return Ok(conn.clone());
            }
        }

        // Create new connection
        let conn = self.create_connection(peer).await?;
        let conn = Arc::new(Mutex::new(conn));
        self.connections.insert(peer.id.clone(), conn.clone());
        Ok(conn)
    }

    /// Create new connection with retry logic
    async fn create_connection(&self, peer: &PeerNode) -> anyhow::Result<PeerConnection> {
        let mut retry_count = 0;
        let mut backoff = self.retry_policy.initial_backoff;

        loop {
            match tokio::time::timeout(
                self.connection_timeout,
                TcpStream::connect(&peer.address)
            ).await {
                Ok(Ok(stream)) => {
                    let framed = Framed::new(stream, LengthDelimitedCodec::new());
                    return Ok(PeerConnection {
                        stream: framed,
                        peer_id: peer.id.clone(),
                        last_used: Instant::now(),
                        healthy: true,
                    });
                }
                Ok(Err(conn_err)) => {
                    // Connection established but protocol error
                    retry_count += 1;
                    if retry_count >= self.retry_policy.max_retries {
                        return Err(anyhow::anyhow!("Protocol error after {} retries: {}", retry_count, conn_err));
                    }
                }
                Err(timeout_err) => {
                    // Connection timeout
                    retry_count += 1;
                    if retry_count >= self.retry_policy.max_retries {
                        return Err(anyhow::anyhow!("Connection timeout after {} retries: {}", retry_count, timeout_err));
                    }
                    
                    tokio::time::sleep(backoff).await;
                    backoff = std::cmp::min(
                        self.retry_policy.max_backoff,
                        Duration::from_secs_f64(backoff.as_secs_f64() * self.retry_policy.backoff_multiplier)
                    );
                }
            }
        }
    }

    /// Send message to peer
    pub async fn send_message(&self, peer: &PeerNode, msg: NetworkMessage) -> anyhow::Result<()> {
        let conn = self.get_connection(peer).await?;
        let mut conn = conn.lock().await;
        
        let data = bincode::serialize(&msg)?;
        conn.stream.send(Bytes::from(data)).await?;
        conn.last_used = Instant::now();
        
        Ok(())
    }

    /// Send and receive message
    pub async fn request(&self, peer: &PeerNode, msg: NetworkMessage) -> anyhow::Result<NetworkMessage> {
        let conn = self.get_connection(peer).await?;
        let mut conn = conn.lock().await;
        
        // Send request
        let data = bincode::serialize(&msg)?;
        conn.stream.send(Bytes::from(data)).await?;
        
        // Receive response
        if let Some(response) = conn.stream.next().await {
            let data = response?;
            let msg: NetworkMessage = bincode::deserialize(&data)?;
            conn.last_used = Instant::now();
            Ok(msg)
        } else {
            conn.healthy = false;
            Err(anyhow::anyhow!("Connection closed"))
        }
    }
}

/// Enhanced TurboCache with complete networking
pub struct NetworkedTurboCache {
    pool: Arc<ConnectionPool>,
    local: Arc<RwLock<HashMap<String, CachedValue>>>,
    peers: Arc<RwLock<Vec<PeerNode>>>,
    node_id: String,
}

impl NetworkedTurboCache {
    /// Create new networked cache
    pub fn new(node_id: String) -> Self {
        Self {
            pool: Arc::new(ConnectionPool::new(100)),
            local: Arc::new(RwLock::new(HashMap::new())),
            peers: Arc::new(RwLock::new(Vec::new())),
            node_id,
        }
    }

    /// Get value from peer
    pub async fn get_from_peer(&self, peer: &PeerNode, key: &str) -> anyhow::Result<Vec<u8>> {
        let msg = NetworkMessage::CacheRequest {
            id: rand::random(),
            operation: CacheOp::Get { key: key.to_string() },
        };

        let response = self.pool.request(peer, msg).await?;
        
        match response {
            NetworkMessage::CacheResponse { result: CacheResult::Success(Some(data)), .. } => {
                Ok(data)
            }
            NetworkMessage::CacheResponse { result: CacheResult::NotFound, .. } => {
                Err(anyhow::anyhow!("Key not found"))
            }
            NetworkMessage::CacheResponse { result: CacheResult::Error(e), .. } => {
                Err(anyhow::anyhow!("Peer error: {}", e))
            }
            _ => Err(anyhow::anyhow!("Unexpected response"))
        }
    }

    /// Broadcast operation to all peers
    pub async fn broadcast(&self, op: CacheOp) -> anyhow::Result<()> {
        let peers = self.peers.read().await;
        let msg = NetworkMessage::GossipMessage {
            from: self.node_id.clone(),
            operation: op,
            vector_clock: VectorClock::new(),
        };

        let mut tasks = vec![];
        for peer in peers.iter() {
            let pool = self.pool.clone();
            let peer = peer.clone();
            let msg = msg.clone();
            
            let task = tokio::spawn(async move {
                pool.send_message(&peer, msg).await
            });
            tasks.push(task);
        }

        // Wait for all broadcasts to complete
        for task in tasks {
            task.await?.ok(); // Ignore individual failures
        }

        Ok(())
    }

    /// Start server to handle incoming connections
    pub async fn start_server(&self, addr: &str) -> anyhow::Result<()> {
        let listener = TcpListener::bind(addr).await?;
        let cache = self.local.clone();
        
        tokio::spawn(async move {
            loop {
                if let Ok((stream, _)) = listener.accept().await {
                    let cache = cache.clone();
                    tokio::spawn(async move {
                        handle_connection(stream, cache).await;
                    });
                }
            }
        });

        Ok(())
    }
}

/// Handle incoming connection
async fn handle_connection(
    stream: TcpStream,
    cache: Arc<RwLock<HashMap<String, CachedValue>>>
) {
    let mut framed = Framed::new(stream, LengthDelimitedCodec::new());
    
    while let Some(result) = framed.next().await {
        if let Ok(data) = result {
            if let Ok(msg) = bincode::deserialize::<NetworkMessage>(&data) {
                let response = match msg {
                    NetworkMessage::CacheRequest { id, operation } => {
                        let result = handle_cache_op(operation, &cache).await;
                        NetworkMessage::CacheResponse { id, result }
                    }
                    NetworkMessage::Ping { timestamp } => {
                        NetworkMessage::Pong { timestamp }
                    }
                    _ => continue,
                };
                
                if let Ok(data) = bincode::serialize(&response) {
                    framed.send(Bytes::from(data)).await.ok();
                }
            }
        }
    }
}

/// Handle cache operation
async fn handle_cache_op(
    op: CacheOp,
    cache: &Arc<RwLock<HashMap<String, CachedValue>>>
) -> CacheResult {
    match op {
        CacheOp::Get { key } => {
            let cache = cache.read().await;
            match cache.get(&key) {
                Some(value) => CacheResult::Success(Some(value.data.clone())),
                None => CacheResult::NotFound,
            }
        }
        CacheOp::Set { key, value } => {
            let mut cache = cache.write().await;
            cache.insert(key, CachedValue {
                data: value,
                inserted_at: Instant::now(),
                inserted_at_system: SystemTime::now(),
                ttl: Duration::from_secs(3600),
                hit_count: 0,
            });
            CacheResult::Success(None)
        }
        CacheOp::Delete { key } => {
            let mut cache = cache.write().await;
            cache.remove(&key);
            CacheResult::Success(None)
        }
        CacheOp::Clear => {
            let mut cache = cache.write().await;
            cache.clear();
            CacheResult::Success(None)
        }
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(10),
            backoff_multiplier: 2.0,
        }
    }
}

impl VectorClock {
    pub fn new() -> Self {
        Self {
            clocks: HashMap::new(),
        }
    }

    pub fn increment(&mut self, node_id: &str) {
        *self.clocks.entry(node_id.to_string()).or_insert(0) += 1;
    }

    pub fn merge(&mut self, other: &VectorClock) {
        for (node, &clock) in &other.clocks {
            let current = self.clocks.entry(node.clone()).or_insert(0);
            *current = (*current).max(clock);
        }
    }
}

use dashmap::DashMap;
use rand;