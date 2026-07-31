//! WS identity + pre-auth bounds (RBAC A3 / A4, one-lane collapse WS arm).
//!
//! Three defects held hands on the WebSocket surface:
//!
//! * **A3** — the connection minted a random `Uuid::new_v4()` as its
//!   `user_id` at connect time and never revisited it after
//!   `Authenticate` verified a token, so every subsequent operation on
//!   the socket was attributed to a principal that does not exist.
//! * **WS authorship** — the `CreateBranch` and `ExecuteOperation`
//!   timeline arms hardcoded `Author::System` for user-initiated
//!   actions (the exact class AUTHORSHIP-A1 closed for REST), which A3
//!   made unfixable: there was no verified principal to derive from.
//! * **A4** — the pre-auth window was uncapped: a client could open any
//!   number of sockets and hold each forever without authenticating.
//!
//! These tests drive a real server over a real socket (the only vantage
//! point from which connection-scoped state is observable), mirroring
//! the `auth_slice1`/`auth_slice4` harness, then read the recorded
//! authorship back out of the shared `AppState` — the append-only log
//! is the artifact that must tell the truth.

#![cfg(test)]

use crate::auth_middleware::AuthPosture;
use crate::protocol::message_handlers::WsAuthLimits;
use crate::router_integration_tests::make_test_state;
use crate::{build_router, AppState};

use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::time::Duration;
use timeline_engine::{Author, BranchId};
use tokio_tungstenite::tungstenite::Message as WsMessage;

// =====================================================================
// Harness — mirrors auth_slice4_tests (module-private there).
// =====================================================================

/// State with the enforced posture, stated explicitly so the test
/// cannot pass merely because the developer's shell lacked a variable.
async fn secure_state() -> AppState {
    let mut state = make_test_state().await;
    state.auth_posture = AuthPosture::Required;
    state
}

/// Serve `state`'s router on an ephemeral loopback port.
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

async fn connect(
    addr: SocketAddr,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let url = format!("ws://{addr}/ws");
    let (socket, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WebSocket upgrade must succeed (the /ws upgrade is public)");
    socket
}

/// Mint a Human token for `sub` from the process's single AuthManager —
/// the same one the WS `Authenticate` handler verifies against.
fn mint_token(state: &AppState, sub: &str) -> String {
    state
        .session_manager
        .auth_manager()
        .create_token(
            sub,
            None,
            vec!["user".to_string()],
            session_manager::PrincipalKind::Human,
        )
        .expect("token minting must succeed")
        .token
}

/// Drain text frames until `decisive` matches one, the socket closes,
/// or `overall` elapses. Returns everything received.
async fn collect_until<S, F>(socket: &mut S, decisive: F, overall: Duration) -> Vec<Value>
where
    S: futures::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
    F: Fn(&Value) -> bool,
{
    let deadline = std::time::Instant::now() + overall;
    let mut frames = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, socket.next()).await {
            Ok(Some(Ok(WsMessage::Text(t)))) => {
                if let Ok(v) = serde_json::from_str::<Value>(&t) {
                    let hit = decisive(&v);
                    frames.push(v);
                    if hit {
                        break;
                    }
                }
            }
            Ok(Some(Ok(WsMessage::Close(_)))) | Ok(Some(Err(_))) | Ok(None) => break,
            Ok(Some(Ok(_))) => {} // ignore non-text
            Err(_) => break,      // overall deadline
        }
    }
    frames
}

fn frame_type(f: &Value) -> Option<&str> {
    f.get("type").and_then(|t| t.as_str())
}

fn error_code(f: &Value) -> Option<&str> {
    (frame_type(f) == Some("Error"))
        .then(|| f["data"]["error_code"].as_str())
        .flatten()
}

/// Send `Authenticate` and wait for the `Authenticated` reply.
async fn authenticate<S>(socket: &mut S, token: &str)
where
    S: futures::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>>
        + futures::Sink<WsMessage, Error = tokio_tungstenite::tungstenite::Error>
        + Unpin,
{
    let frame = json!({
        "type": "Authenticate",
        "data": { "token": token, "request_id": "ws-id-auth" }
    });
    socket
        .send(WsMessage::Text(frame.to_string().into()))
        .await
        .expect("must send Authenticate");
    let frames = collect_until(
        socket,
        |f| {
            matches!(
                frame_type(f),
                Some("Authenticated") | Some("AuthenticationFailed")
            )
        },
        Duration::from_secs(8),
    )
    .await;
    assert!(
        frames
            .iter()
            .any(|f| frame_type(f) == Some("Authenticated")),
        "the Authenticate frame must verify; frames = {frames:#?}"
    );
}

fn timeline_command(request_id: &str, command: Value) -> Value {
    json!({
        "type": "TimelineCommand",
        "data": { "command": command, "request_id": request_id }
    })
}

// =====================================================================
// A3 — operations on an authenticated socket record the VERIFIED
// principal, never a connection-local random UUID, never System.
// =====================================================================

/// A timeline operation executed over an authenticated socket must be
/// recorded with the verified principal's identity.
///
/// **Fails against the pre-A3 tree:** the `ExecuteOperation` arm passes
/// `Author::System` into `Timeline::add_operation`, so the append-only
/// log claims the kernel acted on its own when a named human did.
#[tokio::test]
async fn authenticated_ws_operation_records_the_verified_principal() {
    let state = secure_state().await;
    let token = mint_token(&state, "alice-ws");
    let (addr, server) = serve(state.clone()).await;

    let mut socket = connect(addr).await;
    authenticate(&mut socket, &token).await;

    let cmd = timeline_command(
        "ws-id-op",
        json!({
            "cmd": "ExecuteOperation",
            "operation": {
                "operation_type": "CreatePrimitive",
                "primitive_type": "box",
                "parameters": {"width": 1.0, "depth": 1.0, "height": 1.0},
            }
        }),
    );
    socket
        .send(WsMessage::Text(cmd.to_string().into()))
        .await
        .expect("must send ExecuteOperation");
    let frames = collect_until(
        &mut socket,
        |f| matches!(frame_type(f), Some("TimelineUpdate") | Some("Error")),
        Duration::from_secs(8),
    )
    .await;
    let _ = socket.close(None).await;
    server.abort();

    assert!(
        frames
            .iter()
            .any(|f| frame_type(f) == Some("TimelineUpdate")),
        "the operation must execute; frames = {frames:#?}"
    );

    let timeline = state.timeline.read().await;
    let events = timeline
        .get_branch_events(&BranchId::main(), None, None)
        .expect("main branch events must be readable");
    let last = events
        .last()
        .expect("the executed operation must have recorded an event");
    assert_eq!(
        last.author,
        Author::User {
            id: "alice-ws".to_string(),
            name: "alice-ws".to_string(),
        },
        "an operation executed over an authenticated socket must be recorded \
         with the VERIFIED principal (`alice-ws`), never `System` and never a \
         connection-local random UUID; got {:?}",
        last.author
    );
}

/// Joining a session over an authenticated socket must register the
/// verified identity, not the random UUID minted at connect time.
///
/// **Fails against the pre-A3 tree:** `user_id` is set once from
/// `Uuid::new_v4()` and never updated when `Authenticate` verifies, so
/// the roster shows a principal that exists nowhere.
#[tokio::test]
async fn ws_join_session_registers_the_verified_identity() {
    let state = secure_state().await;
    let token = mint_token(&state, "alice-ws");
    let (addr, server) = serve(state.clone()).await;

    let mut socket = connect(addr).await;
    authenticate(&mut socket, &token).await;

    let join = json!({
        "type": "SessionCommand",
        "data": {
            "command": { "cmd": "JoinSession", "session_id": "default" },
            "request_id": "ws-id-join"
        }
    });
    socket
        .send(WsMessage::Text(join.to_string().into()))
        .await
        .expect("must send JoinSession");
    let frames = collect_until(
        &mut socket,
        |f| matches!(frame_type(f), Some("SessionUpdate") | Some("Error")),
        Duration::from_secs(8),
    )
    .await;
    let _ = socket.close(None).await;
    server.abort();

    let joined = frames
        .iter()
        .find(|f| frame_type(f) == Some("SessionUpdate"))
        .unwrap_or_else(|| panic!("JoinSession must emit a SessionUpdate; frames = {frames:#?}"));
    assert_eq!(
        joined["data"]["update"]["user_id"], "alice-ws",
        "the session roster must carry the VERIFIED identity, not a random \
         connection UUID; frame = {joined}"
    );
}

/// A branch created over an authenticated socket must be authored by
/// the verified principal — never `Author::System` for a user action.
///
/// **Fails against the pre-collapse tree twice over:** the arm passed
/// `Author::System` into the never-seeded `BranchManager`, which then
/// failed `BranchNotFound` — so the branch was both mis-authored and
/// never created at all.
#[tokio::test]
async fn ws_created_branch_is_authored_by_the_principal_not_system() {
    let state = secure_state().await;
    let token = mint_token(&state, "alice-ws");
    let (addr, server) = serve(state.clone()).await;

    let mut socket = connect(addr).await;
    authenticate(&mut socket, &token).await;

    let cmd = timeline_command(
        "ws-id-branch",
        json!({ "cmd": "CreateBranch", "name": "ws-honest-branch", "from_point": null }),
    );
    socket
        .send(WsMessage::Text(cmd.to_string().into()))
        .await
        .expect("must send CreateBranch");
    let frames = collect_until(
        &mut socket,
        |f| matches!(frame_type(f), Some("TimelineUpdate") | Some("Error")),
        Duration::from_secs(8),
    )
    .await;
    let _ = socket.close(None).await;
    server.abort();

    let timeline = state.timeline.read().await;
    let branch = timeline
        .get_all_branches()
        .into_iter()
        .find(|b| b.name == "ws-honest-branch")
        .unwrap_or_else(|| {
            panic!(
                "the WS-created branch must exist in the timeline (the retired \
                 BranchManager lane failed BranchNotFound for every caller); \
                 frames = {frames:#?}"
            )
        });
    assert_eq!(
        branch.metadata.created_by,
        Author::User {
            id: "alice-ws".to_string(),
            name: "alice-ws".to_string(),
        },
        "a user-initiated WS branch creation must be authored by the VERIFIED \
         principal, never `System`; got {:?}",
        branch.metadata.created_by
    );
}

// =====================================================================
// A4 — the pre-auth window is bounded: handshake deadline + cap, both
// refused with typed errors, never a silent drop.
// =====================================================================

/// A socket that never authenticates is refused at the handshake
/// deadline with a TYPED error (`auth_handshake_timeout`), then closed.
///
/// **Fails against the pre-A4 tree:** nothing bounds the pre-auth
/// window — the socket sits open forever and no refusal ever arrives.
#[tokio::test]
async fn unauthenticated_socket_is_refused_at_the_handshake_deadline() {
    let mut state = secure_state().await;
    state.ws_auth_limits = WsAuthLimits::new(8, Duration::from_millis(400));
    let (addr, server) = serve(state).await;

    let mut socket = connect(addr).await;
    // Send nothing. The server must act, we merely listen.
    let frames = collect_until(
        &mut socket,
        |f| error_code(f) == Some("auth_handshake_timeout"),
        Duration::from_secs(6),
    )
    .await;
    server.abort();

    assert!(
        frames
            .iter()
            .any(|f| error_code(f) == Some("auth_handshake_timeout")),
        "an unauthenticated socket must be refused at the handshake deadline \
         with the typed `auth_handshake_timeout` error, not held open \
         indefinitely; frames = {frames:#?}"
    );
}

/// The cap on concurrently-unauthenticated sockets refuses the
/// over-cap connection with a TYPED error
/// (`unauthenticated_connection_limit`) — and the slot is RELEASED the
/// moment a holder authenticates, so the cap governs the anonymous
/// window only, never authenticated capacity.
///
/// **Fails against the pre-A4 tree:** the second socket is welcomed —
/// there is no cap at all.
#[tokio::test]
async fn unauthenticated_connection_cap_is_typed_and_released_on_auth() {
    let mut state = secure_state().await;
    state.ws_auth_limits = WsAuthLimits::new(1, Duration::from_secs(60));
    let token = mint_token(&state, "cap-holder");
    let (addr, server) = serve(state).await;

    // Socket 1 claims the single pre-auth slot and holds it.
    let mut socket1 = connect(addr).await;
    let s1_frames = collect_until(
        &mut socket1,
        |f| frame_type(f) == Some("Welcome"),
        Duration::from_secs(8),
    )
    .await;
    assert!(
        s1_frames.iter().any(|f| frame_type(f) == Some("Welcome")),
        "the first pre-auth socket fits the cap and must be welcomed; \
         frames = {s1_frames:#?}"
    );

    // Socket 2 exceeds the cap: typed refusal, then close.
    let mut socket2 = connect(addr).await;
    let s2_frames = collect_until(
        &mut socket2,
        |f| error_code(f) == Some("unauthenticated_connection_limit"),
        Duration::from_secs(8),
    )
    .await;
    assert!(
        s2_frames
            .iter()
            .any(|f| error_code(f) == Some("unauthenticated_connection_limit")),
        "the over-cap unauthenticated socket must be refused with the typed \
         `unauthenticated_connection_limit` error; frames = {s2_frames:#?}"
    );
    assert!(
        !s2_frames.iter().any(|f| frame_type(f) == Some("Welcome")),
        "the over-cap socket must be refused BEFORE the Welcome handshake; \
         frames = {s2_frames:#?}"
    );

    // Socket 1 authenticates → its slot frees → socket 3 is admitted.
    authenticate(&mut socket1, &token).await;
    let mut socket3 = connect(addr).await;
    let s3_frames = collect_until(
        &mut socket3,
        |f| frame_type(f) == Some("Welcome") || error_code(f).is_some(),
        Duration::from_secs(8),
    )
    .await;
    assert!(
        s3_frames.iter().any(|f| frame_type(f) == Some("Welcome")),
        "once the holder authenticates, its pre-auth slot must be released \
         (RAII) and a new connection admitted; frames = {s3_frames:#?}"
    );

    let _ = socket1.close(None).await;
    let _ = socket3.close(None).await;
    server.abort();
}
