//! Auth Slice 4 — the WebSocket `SessionCommand` gate.
//!
//! Slice 1 gated four of the five WebSocket command families
//! (`GeometryCommand`, `AICommand`, `TimelineCommand`, `ExportCommand`)
//! behind the in-band `Authenticate` handshake. The fifth —
//! `SessionCommand` (create/join/leave a session, share an object,
//! publish presence) — was left ungated: its match arm carried no
//! `require_ws_auth!` invocation, so any connected socket could create
//! and join sessions, register itself as a collaborator, and publish
//! presence without ever presenting a credential, in the default
//! `AuthPosture::Required` posture too.
//!
//! That is the same #44 silent-lie family Slice 1 exists to close: the
//! surface *reads* as authenticated (four of five arms are gated and the
//! module doc-comment claims the command surface is gated) while one arm
//! silently accepts anonymous mutation. These tests drive a real server
//! over a real socket — the only vantage point from which the
//! connection-path hole is observable — exactly as the Slice 1 WS tests
//! do.

#![cfg(test)]

use crate::auth_middleware::AuthPosture;
use crate::router_integration_tests::make_test_state;
use crate::{build_router, AppState};

use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::net::SocketAddr;
use tokio_tungstenite::tungstenite::Message as WsMessage;

// =====================================================================
// Harness — mirrors auth_slice1_tests so the two slices assert the same
// wire behaviour with the same fixtures. Kept local because the Slice 1
// helpers are module-private to that file.
// =====================================================================

/// Build a state whose auth posture is explicitly `Required` rather than
/// inherited from the ambient process environment. The default posture
/// is separately pinned in `auth_middleware`'s own tests; here we state
/// the posture we exercise so the test cannot pass merely because the
/// developer's shell lacked a variable.
async fn secure_state() -> AppState {
    let mut state = make_test_state().await;
    state.auth_posture = AuthPosture::Required;
    state
}

/// Serve `state`'s router on an ephemeral loopback port and return the
/// bound address plus the serving task's handle (aborted by the caller
/// to shut the server down).
async fn serve(state: AppState) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("must bind an ephemeral loopback port");
    let addr = listener
        .local_addr()
        .expect("bound listener has an address");
    let router = build_router(state);
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (addr, handle)
}

/// Connect a WebSocket to `/ws`, send `command_json`, and collect the
/// text frames the server emits until a decisive frame arrives or the
/// deadline passes.
async fn ws_send_and_collect(addr: SocketAddr, command_json: Value) -> Vec<Value> {
    let url = format!("ws://{addr}/ws");
    let (mut socket, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WebSocket upgrade must succeed (the /ws upgrade is public)");

    socket
        .send(WsMessage::Text(command_json.to_string().into()))
        .await
        .expect("must send the command frame");

    let frames = collect_ws_frames(&mut socket).await;
    let _ = socket.close(None).await;
    frames
}

/// Drain text frames until a decisive frame (an auth refusal, a session
/// effect, or an authentication failure) arrives or an overall deadline
/// passes. Breaking on a decisive frame keeps the test robust when the
/// whole suite runs in parallel and server tasks are scheduling-starved.
async fn collect_ws_frames<S>(socket: &mut S) -> Vec<Value>
where
    S: futures::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    let mut frames = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let recv = tokio::time::timeout(
            remaining.min(std::time::Duration::from_millis(2500)),
            socket.next(),
        )
        .await;
        match recv {
            Ok(Some(Ok(WsMessage::Text(t)))) => {
                if let Ok(v) = serde_json::from_str::<Value>(&t) {
                    let decisive = is_auth_refusal_frame(&v)
                        || is_session_effect_frame(&v)
                        || v.get("type").and_then(|t| t.as_str()) == Some("AuthenticationFailed");
                    frames.push(v);
                    if decisive {
                        break;
                    }
                }
            }
            Ok(Some(Ok(_))) => {}                 // ignore non-text
            Ok(Some(Err(_))) | Ok(None) => break, // socket closed
            Err(_) => break,                      // overall/idle deadline
        }
    }
    frames
}

/// A well-formed `ClientMessage::SessionCommand` that creates a session.
///
/// The wire shape is load-bearing for the negative assertion: it must
/// deserialize into the real `SessionCommand`/`CreateSession` arm so
/// that, absent the gate, it actually executes and emits a
/// `ServerMessage::Success` carrying the new `session_id`. A frame that
/// failed to parse would trivially "not execute" and make the test
/// vacuous. `ClientMessage` is `tag="type", content="data"`;
/// `SessionWSCommand` is internally tagged on `cmd`.
fn session_create_frame(request_id: &str, name: &str) -> Value {
    json!({
        "type": "SessionCommand",
        "data": {
            "command": { "cmd": "CreateSession", "name": name, "description": null },
            "request_id": request_id
        }
    })
}

/// True if a frame is a `ServerMessage::Error` whose `error_code` marks
/// an authentication refusal. `ServerMessage` is `tag="type",
/// content="data"`, so `error_code` sits under `data`.
fn is_auth_refusal_frame(f: &Value) -> bool {
    f.get("type").and_then(|t| t.as_str()) == Some("Error")
        && f.get("data")
            .and_then(|d| d.get("error_code"))
            .and_then(|c| c.as_str())
            == Some("auth_required")
}

/// True if a frame looks like an *executed* session command — a
/// `ServerMessage::Success` (CreateSession's acknowledgement, carrying
/// the new session id), a `ServerMessage::SessionUpdate` (join/presence),
/// or the `CollaboratorsUpdate` roster broadcast.
fn is_session_effect_frame(f: &Value) -> bool {
    matches!(
        f.get("type").and_then(|t| t.as_str()),
        Some("Success") | Some("SessionUpdate") | Some("CollaboratorsUpdate")
    )
}

fn has_auth_refusal(frames: &[Value]) -> bool {
    frames.iter().any(is_auth_refusal_frame)
}

fn has_session_effect(frames: &[Value]) -> bool {
    frames.iter().any(is_session_effect_frame)
}

// =====================================================================
// RED — an unauthenticated SessionCommand must be refused, not honoured
// =====================================================================

/// Connect to `/ws` and send a `SessionCommand::CreateSession` without
/// ever authenticating. The server must refuse it with an `auth_required`
/// error and must not create the session.
///
/// **Fails against the pre-Slice-4 tree:** the `SessionCommand` arm
/// carries no `require_ws_auth!`, so the session is created and a
/// `ServerMessage::Success` frame is emitted instead of an
/// `auth_required` error.
#[tokio::test]
async fn unauthenticated_websocket_session_command_is_rejected() {
    let state = secure_state().await;
    let (addr, server) = serve(state).await;

    let frames = ws_send_and_collect(addr, session_create_frame("ws-sess-1", "s4-anon")).await;
    server.abort();

    assert!(
        has_auth_refusal(&frames),
        "an unauthenticated WebSocket SessionCommand must be refused with an \
         `auth_required` error before it touches the session manager. Frames \
         received: {frames:#?}"
    );
    assert!(
        !has_session_effect(&frames),
        "an unauthenticated WebSocket SessionCommand must NOT execute — no \
         session-effect frame (Success / SessionUpdate / CollaboratorsUpdate) \
         may be emitted. Frames received: {frames:#?}"
    );
}

// =====================================================================
// GREEN companions — the gate opens for a valid credential and for the
// dev bypass, so it is a gate and not a blanket refusal.
// =====================================================================

/// Positive path: a client that authenticates in-band with a valid JWT
/// may then drive `SessionCommand`. Proves the gate *opens*.
#[tokio::test]
async fn websocket_session_command_after_authenticate_executes() {
    let state = secure_state().await;

    // Mint a token from the process's single AuthManager — the same one
    // the WS `Authenticate` handler verifies against.
    let token = state
        .session_manager
        .auth_manager()
        .create_token("user_ws_session", None, vec!["user".to_string()])
        .expect("token minting must succeed")
        .token;

    let (addr, server) = serve(state).await;
    let url = format!("ws://{addr}/ws");
    let (mut socket, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WebSocket upgrade must succeed");

    let authenticate = json!({
        "type": "Authenticate",
        "data": { "token": token, "request_id": "ws-auth-s4" }
    });
    socket
        .send(WsMessage::Text(authenticate.to_string().into()))
        .await
        .expect("must send Authenticate");
    socket
        .send(WsMessage::Text(
            session_create_frame("ws-sess-authed", "s4-authed")
                .to_string()
                .into(),
        ))
        .await
        .expect("must send SessionCommand");

    let frames = collect_ws_frames(&mut socket).await;
    let _ = socket.close(None).await;
    server.abort();

    assert!(
        !has_auth_refusal(&frames),
        "an authenticated WebSocket SessionCommand must not be auth-refused. \
         Frames received: {frames:#?}"
    );
    assert!(
        has_session_effect(&frames),
        "after a valid Authenticate frame, the SessionCommand must execute and \
         emit a session-effect frame. Frames received: {frames:#?}"
    );
}

/// Positive control: under the `InsecureDevBypass` posture the same
/// SessionCommand is admitted without a credential — the gate keys on
/// posture (the connection starts authenticated under the bypass) rather
/// than refusing unconditionally, so local development is unaffected.
#[tokio::test]
async fn dev_bypass_admits_websocket_session_command() {
    // make_test_state defaults to InsecureDevBypass.
    let state = make_test_state().await;
    assert_eq!(
        state.auth_posture,
        AuthPosture::InsecureDevBypass,
        "this control must run under the dev-insecure posture"
    );
    let (addr, server) = serve(state).await;

    let frames = ws_send_and_collect(addr, session_create_frame("ws-sess-dev", "s4-dev")).await;
    server.abort();

    assert!(
        !has_auth_refusal(&frames),
        "under InsecureDevBypass a WebSocket SessionCommand must not be \
         auth-refused. Frames received: {frames:#?}"
    );
    assert!(
        has_session_effect(&frames),
        "under InsecureDevBypass the SessionCommand must execute. Frames \
         received: {frames:#?}"
    );
}
