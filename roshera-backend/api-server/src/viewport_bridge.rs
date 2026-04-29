//! Viewport debug bridge — gives Claude (or any dev tool) eyes into the live
//! Three.js viewport.
//!
//! The frontend opens a single WebSocket to `/ws/viewport-bridge`. The
//! backend exposes REST endpoints under `/api/viewport/*` that push commands
//! to the connected frontend and await its response.
//!
//! # Lifecycle
//!
//! 1. Frontend (`ViewportBridge.tsx`) connects on app load.
//! 2. The connection registers itself as the bridge sink, replacing any
//!    previous sink.
//! 3. REST callers push `BridgeCommand` JSON over the socket and await a
//!    matching `BridgeResponse` keyed by `request_id`.
//!
//! # Safety / scope
//!
//! Routes are mounted only when `ROSHERA_DEV_BRIDGE=1` (see
//! [`enabled`]). All endpoints are localhost-only by virtue of the API
//! server bind, but the env-gate guarantees the routes never reach a
//! production deployment.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::StatusCode,
    response::Response,
    Json,
};
use base64::Engine as _;
use dashmap::DashMap;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, Mutex};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::AppState;

/// Returns true if the bridge should be mounted on this server.
///
/// Bridge routes are absent unless `ROSHERA_DEV_BRIDGE=1`. This double-gate
/// (build flag + env var) makes accidental production exposure impossible.
pub fn enabled() -> bool {
    std::env::var("ROSHERA_DEV_BRIDGE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// How long REST handlers wait for the frontend to acknowledge a command.
const BRIDGE_TIMEOUT: Duration = Duration::from_secs(10);

// ─── Wire protocol ────────────────────────────────────────────────────────

/// Server → frontend command.
#[derive(Debug, Serialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum BridgeCommand {
    Snapshot {
        request_id: Uuid,
        width: Option<u32>,
        height: Option<u32>,
    },
    SetCamera {
        request_id: Uuid,
        position: [f64; 3],
        target: [f64; 3],
        up: [f64; 3],
    },
    LoadStl {
        request_id: Uuid,
        path: String,
        name: String,
        replace_scene: bool,
    },
    SetShading {
        request_id: Uuid,
        mode: String,
    },
    ClearScene {
        request_id: Uuid,
    },
}

/// Frontend → server reply.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum BridgeResponse {
    /// Successful snapshot — `data_base64` is the raw `data:image/png;base64,…`
    /// payload (with or without the prefix).
    SnapshotResult {
        request_id: Uuid,
        data_base64: String,
        width: u32,
        height: u32,
    },
    /// Plain acknowledgement for camera moves and other void commands.
    Ack {
        request_id: Uuid,
    },
    /// Frontend reported a failure executing the command.
    Error {
        request_id: Uuid,
        message: String,
    },
    /// Unsolicited frontend → server push: the scene mutated and the
    /// frontend grabbed a fresh snapshot. Backend writes it to a fixed
    /// path so dev tools (Claude) can `Read` the latest viewport state
    /// without a round-trip.
    AutoSnapshot {
        data_base64: String,
        width: u32,
        height: u32,
    },
}

/// Channel payload waiting for a request to complete.
#[derive(Debug)]
enum ResponseValue {
    Snapshot {
        png_bytes: Vec<u8>,
        width: u32,
        height: u32,
    },
    Ack,
    Error(String),
}

// ─── Bridge state ─────────────────────────────────────────────────────────

/// Shared bridge state held inside [`AppState`].
///
/// One sender at a time — the most recent connection wins. This matches the
/// expected single-tab dev workflow.
#[derive(Default)]
pub struct ViewportBridge {
    /// Sender to the connected frontend, if any.
    sender: Mutex<Option<mpsc::UnboundedSender<BridgeCommand>>>,
    /// Pending requests awaiting a frontend reply.
    pending: DashMap<Uuid, oneshot::Sender<ResponseValue>>,
}

impl ViewportBridge {
    /// Construct a fresh bridge with no connection.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Send `cmd` to the connected frontend and await its matching response.
    async fn dispatch(
        self: &Arc<Self>,
        request_id: Uuid,
        cmd: BridgeCommand,
    ) -> Result<ResponseValue, BridgeError> {
        let sender_guard = self.sender.lock().await;
        let sender = sender_guard.as_ref().ok_or(BridgeError::NotConnected)?;

        let (tx, rx) = oneshot::channel();
        self.pending.insert(request_id, tx);

        sender
            .send(cmd)
            .map_err(|_| BridgeError::SendFailed)?;
        // Drop the sender guard before awaiting so the WS task can use it.
        drop(sender_guard);

        match tokio::time::timeout(BRIDGE_TIMEOUT, rx).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(_)) => {
                self.pending.remove(&request_id);
                Err(BridgeError::Cancelled)
            }
            Err(_) => {
                self.pending.remove(&request_id);
                Err(BridgeError::Timeout)
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum BridgeError {
    #[error("no viewport client connected")]
    NotConnected,
    #[error("failed to send command to viewport client")]
    SendFailed,
    #[error("viewport client did not reply within {} seconds", BRIDGE_TIMEOUT.as_secs())]
    Timeout,
    #[error("viewport client closed connection mid-request")]
    Cancelled,
    #[error("viewport client reported error: {0}")]
    ClientError(String),
    #[error("base64 decode failed: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl BridgeError {
    fn status(&self) -> StatusCode {
        match self {
            BridgeError::NotConnected => StatusCode::SERVICE_UNAVAILABLE,
            BridgeError::Timeout | BridgeError::Cancelled => StatusCode::GATEWAY_TIMEOUT,
            BridgeError::SendFailed => StatusCode::BAD_GATEWAY,
            BridgeError::ClientError(_) => StatusCode::BAD_GATEWAY,
            BridgeError::Base64(_) | BridgeError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

// ─── WebSocket handler ────────────────────────────────────────────────────

/// Axum upgrade handler for `/ws/viewport-bridge`.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> Response {
    let bridge = state.viewport_bridge.clone();
    ws.on_upgrade(move |socket| async move {
        run_socket(socket, bridge).await;
    })
}

async fn run_socket(socket: WebSocket, bridge: Arc<ViewportBridge>) {
    info!("viewport bridge: client connected");

    let (mut sink, mut stream) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<BridgeCommand>();

    // Replace any existing sender; old connection's commands will fail when
    // its socket eventually drops.
    {
        let mut slot = bridge.sender.lock().await;
        *slot = Some(out_tx);
    }

    // Forward outbound commands → socket sink.
    let outbound_bridge = bridge.clone();
    let outbound = tokio::spawn(async move {
        while let Some(cmd) = out_rx.recv().await {
            let json = match serde_json::to_string(&cmd) {
                Ok(s) => s,
                Err(e) => {
                    warn!("viewport bridge: serialize cmd failed: {e}");
                    continue;
                }
            };
            if let Err(e) = sink.send(Message::Text(json.into())).await {
                debug!("viewport bridge: sink closed: {e}");
                break;
            }
        }
        // Sink closed — clean up sender slot if it still points at us.
        let mut slot = outbound_bridge.sender.lock().await;
        *slot = None;
    });

    // Inbound responses from the frontend.
    while let Some(msg) = stream.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                debug!("viewport bridge: stream error: {e}");
                break;
            }
        };
        let text = match msg {
            Message::Text(t) => t,
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) | Message::Binary(_) => continue,
        };

        let resp: BridgeResponse = match serde_json::from_str(&text) {
            Ok(r) => r,
            Err(e) => {
                warn!("viewport bridge: bad response json: {e} — payload={text}");
                continue;
            }
        };
        deliver(&bridge, resp);
    }

    info!("viewport bridge: client disconnected");
    outbound.abort();
    // Clear sender if still ours (race with another connection is benign —
    // worst case the new connection's slot survives only briefly).
    let mut slot = bridge.sender.lock().await;
    *slot = None;
    bridge.pending.clear();
}

fn deliver(bridge: &Arc<ViewportBridge>, resp: BridgeResponse) {
    // Unsolicited auto-snapshot pushes don't correlate to a pending
    // request — they fire off the scene-store subscription. Save them
    // straight to `target/snapshots/latest.png` and return.
    if let BridgeResponse::AutoSnapshot {
        data_base64,
        width,
        height,
    } = &resp
    {
        match decode_png_data_url(data_base64) {
            Ok(png_bytes) => persist_latest_snapshot(png_bytes, *width, *height),
            Err(e) => warn!("viewport bridge: auto-snapshot decode failed: {e}"),
        }
        return;
    }

    let request_id = match &resp {
        BridgeResponse::SnapshotResult { request_id, .. }
        | BridgeResponse::Ack { request_id }
        | BridgeResponse::Error { request_id, .. } => *request_id,
        BridgeResponse::AutoSnapshot { .. } => unreachable!("handled above"),
    };

    let Some((_, tx)) = bridge.pending.remove(&request_id) else {
        debug!("viewport bridge: response for unknown request {request_id}");
        return;
    };

    let value = match resp {
        BridgeResponse::SnapshotResult {
            data_base64,
            width,
            height,
            ..
        } => match decode_png_data_url(&data_base64) {
            Ok(png_bytes) => ResponseValue::Snapshot {
                png_bytes,
                width,
                height,
            },
            Err(e) => ResponseValue::Error(format!("png decode failed: {e}")),
        },
        BridgeResponse::Ack { .. } => ResponseValue::Ack,
        BridgeResponse::Error { message, .. } => ResponseValue::Error(message),
        BridgeResponse::AutoSnapshot { .. } => unreachable!("handled above"),
    };

    // If the receiver has already been dropped (timeout), the send fails
    // silently — that's expected and harmless.
    let _ = tx.send(value);
}

/// Write the latest auto-snapshot to `target/snapshots/latest.png`,
/// creating the directory if needed. Errors are logged but never
/// propagated — auto-snapshots are best-effort.
fn persist_latest_snapshot(png_bytes: Vec<u8>, width: u32, height: u32) {
    let dir = std::path::Path::new("target/snapshots");
    if let Err(e) = std::fs::create_dir_all(dir) {
        warn!("viewport bridge: mkdir for auto-snapshot failed: {e}");
        return;
    }
    let path = dir.join("latest.png");
    match std::fs::write(&path, &png_bytes) {
        Ok(()) => debug!(
            "viewport bridge: wrote auto-snapshot {}x{} ({} bytes) → {}",
            width,
            height,
            png_bytes.len(),
            path.display()
        ),
        Err(e) => warn!("viewport bridge: write auto-snapshot failed: {e}"),
    }
}

fn decode_png_data_url(data: &str) -> Result<Vec<u8>, base64::DecodeError> {
    let payload = data
        .strip_prefix("data:image/png;base64,")
        .unwrap_or(data);
    base64::engine::general_purpose::STANDARD.decode(payload.trim())
}

// ─── REST endpoints ───────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SnapshotRequest {
    /// Filename stem (no extension). Defaults to `snap_<uuid>`.
    pub name: Option<String>,
    /// Optional output directory. Defaults to `target/snapshots`.
    pub out_dir: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Serialize)]
pub struct SnapshotResponse {
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: usize,
}

pub async fn snapshot(
    State(state): State<AppState>,
    Json(req): Json<SnapshotRequest>,
) -> Result<Json<SnapshotResponse>, (StatusCode, String)> {
    let bridge = state.viewport_bridge.clone();
    let request_id = Uuid::new_v4();

    let value = bridge
        .dispatch(
            request_id,
            BridgeCommand::Snapshot {
                request_id,
                width: req.width,
                height: req.height,
            },
        )
        .await
        .map_err(into_http_err)?;

    let (png_bytes, width, height) = match value {
        ResponseValue::Snapshot {
            png_bytes,
            width,
            height,
        } => (png_bytes, width, height),
        ResponseValue::Error(m) => {
            return Err(into_http_err(BridgeError::ClientError(m)));
        }
        ResponseValue::Ack => {
            return Err(into_http_err(BridgeError::ClientError(
                "expected snapshot result, got ack".to_string(),
            )));
        }
    };

    let dir = req
        .out_dir
        .clone()
        .unwrap_or_else(|| "target/snapshots".to_string());
    std::fs::create_dir_all(&dir).map_err(|e| into_http_err(BridgeError::Io(e)))?;

    let stem = req
        .name
        .clone()
        .unwrap_or_else(|| format!("snap_{}", request_id.simple()));
    let path = std::path::Path::new(&dir).join(format!("{stem}.png"));
    std::fs::write(&path, &png_bytes).map_err(|e| into_http_err(BridgeError::Io(e)))?;

    let abs = std::fs::canonicalize(&path).unwrap_or(path.clone());

    Ok(Json(SnapshotResponse {
        path: abs.to_string_lossy().to_string(),
        width,
        height,
        size_bytes: png_bytes.len(),
    }))
}

#[derive(Deserialize)]
pub struct CameraRequest {
    pub position: [f64; 3],
    pub target: Option<[f64; 3]>,
    pub up: Option<[f64; 3]>,
}

#[derive(Serialize)]
pub struct AckResponse {
    pub ok: bool,
}

pub async fn set_camera(
    State(state): State<AppState>,
    Json(req): Json<CameraRequest>,
) -> Result<Json<AckResponse>, (StatusCode, String)> {
    let bridge = state.viewport_bridge.clone();
    let request_id = Uuid::new_v4();
    let value = bridge
        .dispatch(
            request_id,
            BridgeCommand::SetCamera {
                request_id,
                position: req.position,
                target: req.target.unwrap_or([0.0, 0.0, 0.0]),
                up: req.up.unwrap_or([0.0, 1.0, 0.0]),
            },
        )
        .await
        .map_err(into_http_err)?;

    expect_ack(value)?;
    Ok(Json(AckResponse { ok: true }))
}

#[derive(Deserialize)]
pub struct LoadStlRequest {
    /// Absolute or relative path on the *server's* filesystem. The bridge
    /// streams the file contents over the WebSocket so the frontend doesn't
    /// need its own FS access.
    pub path: String,
    pub name: Option<String>,
    /// If true, drop existing scene contents before loading.
    pub replace_scene: Option<bool>,
}

pub async fn load_stl(
    State(state): State<AppState>,
    Json(req): Json<LoadStlRequest>,
) -> Result<Json<AckResponse>, (StatusCode, String)> {
    // Resolve and read the STL on the server, then base64-stream it via the
    // command. Cleaner than asking the frontend to fetch from disk.
    let canonical = std::fs::canonicalize(&req.path)
        .map_err(|e| into_http_err(BridgeError::Io(e)))?;
    let bytes = std::fs::read(&canonical).map_err(|e| into_http_err(BridgeError::Io(e)))?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let name = req
        .name
        .unwrap_or_else(|| {
            canonical
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "stl".to_string())
        });
    let bridge = state.viewport_bridge.clone();
    let request_id = Uuid::new_v4();
    let value = bridge
        .dispatch(
            request_id,
            BridgeCommand::LoadStl {
                request_id,
                path: encoded,
                name,
                replace_scene: req.replace_scene.unwrap_or(true),
            },
        )
        .await
        .map_err(into_http_err)?;
    expect_ack(value)?;
    Ok(Json(AckResponse { ok: true }))
}

#[derive(Deserialize)]
pub struct ShadingRequest {
    /// One of: `lit`, `normals`, `wireframe`, `edges`.
    pub mode: String,
}

pub async fn set_shading(
    State(state): State<AppState>,
    Json(req): Json<ShadingRequest>,
) -> Result<Json<AckResponse>, (StatusCode, String)> {
    let bridge = state.viewport_bridge.clone();
    let request_id = Uuid::new_v4();
    let value = bridge
        .dispatch(
            request_id,
            BridgeCommand::SetShading {
                request_id,
                mode: req.mode,
            },
        )
        .await
        .map_err(into_http_err)?;
    expect_ack(value)?;
    Ok(Json(AckResponse { ok: true }))
}

pub async fn clear_scene(
    State(state): State<AppState>,
) -> Result<Json<AckResponse>, (StatusCode, String)> {
    let bridge = state.viewport_bridge.clone();
    let request_id = Uuid::new_v4();
    let value = bridge
        .dispatch(request_id, BridgeCommand::ClearScene { request_id })
        .await
        .map_err(into_http_err)?;
    expect_ack(value)?;
    Ok(Json(AckResponse { ok: true }))
}

#[derive(Serialize)]
pub struct StatusResponse {
    pub connected: bool,
    pub pending_requests: usize,
}

pub async fn status(State(state): State<AppState>) -> Json<StatusResponse> {
    let connected = state.viewport_bridge.sender.lock().await.is_some();
    let pending_requests = state.viewport_bridge.pending.len();
    Json(StatusResponse {
        connected,
        pending_requests,
    })
}

// ─── helpers ──────────────────────────────────────────────────────────────

fn expect_ack(value: ResponseValue) -> Result<(), (StatusCode, String)> {
    match value {
        ResponseValue::Ack => Ok(()),
        ResponseValue::Error(m) => Err(into_http_err(BridgeError::ClientError(m))),
        ResponseValue::Snapshot { .. } => Err(into_http_err(BridgeError::ClientError(
            "expected ack, got snapshot".to_string(),
        ))),
    }
}

fn into_http_err(err: BridgeError) -> (StatusCode, String) {
    (err.status(), err.to_string())
}
