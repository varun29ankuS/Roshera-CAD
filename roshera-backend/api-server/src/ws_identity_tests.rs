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
use geometry_engine::math::Vector3;
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::time::Duration;
use timeline_engine::{Author, BranchId, BranchPurpose};
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

// =====================================================================
// WS timeline undo/redo/branch-switch — the three commands that used to
// report success without doing anything (`Undo`/`Redo`/`SwitchBranch`).
// These drive a real socket against a real kernel model, exactly like
// the A3 tests above, and assert on the KERNEL/TIMELINE STATE, not
// merely on the ack — asserting the ack is what the pre-fix stub would
// already pass.
// =====================================================================

/// Build a `size × size × size` box directly against the kernel model
/// (mirrors `router_integration_tests`' `TopologyBuilder` pattern) and
/// return its `SolidId`. The state's attached `TimelineRecorder`
/// auto-records the op — callers must `flush()` the recorder before
/// reading branch events back.
async fn create_box(state: &AppState, size: f64) -> SolidId {
    let mut model_guard = state.model.write().await;
    let model: &mut BRepModel = &mut *model_guard;
    let geom_id = TopologyBuilder::new(model)
        .create_box_3d(size, size, size)
        .expect("box primitive must build for positive finite dimensions");
    match geom_id {
        GeometryId::Solid(id) => id,
        other => panic!("expected Solid from create_box_3d, got {:?}", other),
    }
}

/// A WS `Undo` must genuinely roll the kernel model back one operation,
/// not merely acknowledge success.
///
/// **Fails against the pre-fix stub:** the `Undo` arm always replies
/// `UndoPerformed` without touching the timeline or the model, so the
/// second box would still be present after "undo".
#[tokio::test]
async fn ws_undo_genuinely_undoes_the_last_operation() {
    let state = secure_state().await;
    let token = mint_token(&state, "undo-tester");

    let first_box = create_box(&state, 1.0).await;
    let _second_box = create_box(&state, 2.0).await;
    let _ = state.timeline_recorder.flush().await;

    let solids_before = state.model.read().await.solids.iter().count();
    assert_eq!(solids_before, 2, "setup must have produced two solids");

    let (addr, server) = serve(state.clone()).await;
    let mut socket = connect(addr).await;
    authenticate(&mut socket, &token).await;

    let cmd = timeline_command("ws-undo-1", json!({ "cmd": "Undo", "steps": null }));
    socket
        .send(WsMessage::Text(cmd.to_string().into()))
        .await
        .expect("must send Undo");
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
        "undo must ack with a TimelineUpdate frame; frames = {frames:#?}"
    );

    let remaining: Vec<SolidId> = state
        .model
        .read()
        .await
        .solids
        .iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(
        remaining,
        vec![first_box],
        "WS Undo must have genuinely rolled the kernel model back to just the \
         first box, not merely acked success; frames = {frames:#?}"
    );
}

/// A WS `Undo` and a REST `POST /api/timeline/undo` starting from the SAME
/// state must land on the SAME state — there is exactly one undo
/// implementation (`handlers::timeline::perform_undo`), not two that can
/// drift apart.
#[tokio::test]
async fn ws_undo_and_rest_undo_agree() {
    let session_uuid = crate::handlers::timeline::live_session_id(&BranchId::main());

    // Two independent states, built identically.
    let state_ws = secure_state().await;
    let ws_first_box = create_box(&state_ws, 1.0).await;
    let _ = create_box(&state_ws, 2.0).await;
    let _ = state_ws.timeline_recorder.flush().await;

    let state_rest = secure_state().await;
    let rest_first_box = create_box(&state_rest, 1.0).await;
    let _ = create_box(&state_rest, 2.0).await;
    let _ = state_rest.timeline_recorder.flush().await;

    assert_eq!(
        ws_first_box, rest_first_box,
        "identical setup on two fresh states must allocate the same kernel id"
    );

    // Path 1: WS Undo.
    let token = mint_token(&state_ws, "agree-ws");
    let (addr, server) = serve(state_ws.clone()).await;
    let mut socket = connect(addr).await;
    authenticate(&mut socket, &token).await;
    let cmd = timeline_command("ws-undo-agree", json!({ "cmd": "Undo", "steps": null }));
    socket
        .send(WsMessage::Text(cmd.to_string().into()))
        .await
        .expect("must send Undo");
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
        "WS undo must ack with TimelineUpdate; frames = {frames:#?}"
    );

    // Path 2: REST undo, called through the exact same handler function
    // routed at `POST /api/timeline/undo`, targeting the same live
    // session id the WS arm resolves to (no explicit session → main's
    // live session).
    let rest_result = crate::handlers::timeline::undo_operation(
        axum::extract::State(state_rest.clone()),
        axum::extract::Json(json!({ "session_id": session_uuid.to_string() })),
    )
    .await
    .expect("REST undo handler must return Ok");
    assert_eq!(
        rest_result.0["success"], true,
        "REST undo must succeed; body = {:?}",
        rest_result.0
    );

    // Compare the two resulting kernel states.
    let ws_remaining: Vec<SolidId> = state_ws
        .model
        .read()
        .await
        .solids
        .iter()
        .map(|(id, _)| id)
        .collect();
    let rest_remaining: Vec<SolidId> = state_rest
        .model
        .read()
        .await
        .solids
        .iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(
        ws_remaining,
        vec![ws_first_box],
        "WS undo must leave only the first box"
    );
    assert_eq!(
        ws_remaining, rest_remaining,
        "WS undo and REST undo, given the same starting state, must land on \
         the exact same kernel state"
    );
}

/// `SwitchBranch` must report the REAL previous active branch in `from`,
/// never a hardcoded `"main"` — verified by switching TWICE so the second
/// switch's `from` is a non-main branch.
///
/// **Fails against the pre-fix stub:** `from` is always the literal
/// string `"main"` regardless of which branch was actually active, and
/// no branch is actually switched (the kernel's recording branch never
/// moves).
#[tokio::test]
async fn ws_switch_branch_reports_the_real_previous_branch() {
    let state = secure_state().await;
    let token = mint_token(&state, "switch-tester");
    let (addr, server) = serve(state.clone()).await;

    let mut socket = connect(addr).await;
    authenticate(&mut socket, &token).await;

    // Create branch B, then switch to it. `from` must be "main" (the
    // process starts recording on main) and the kernel's active
    // recording branch must actually move.
    let create_b = timeline_command(
        "ws-switch-create-b",
        json!({ "cmd": "CreateBranch", "name": "switch-branch-b", "from_point": null }),
    );
    socket
        .send(WsMessage::Text(create_b.to_string().into()))
        .await
        .expect("must send CreateBranch B");
    let _ = collect_until(
        &mut socket,
        |f| matches!(frame_type(f), Some("TimelineUpdate") | Some("Error")),
        Duration::from_secs(8),
    )
    .await;

    let branch_b_id = {
        let timeline = state.timeline.read().await;
        timeline
            .get_all_branches()
            .into_iter()
            .find(|b| b.name == "switch-branch-b")
            .expect("branch B must have been created")
            .id
    };

    let switch_to_b = timeline_command(
        "ws-switch-to-b",
        json!({ "cmd": "SwitchBranch", "branch_name": "switch-branch-b" }),
    );
    socket
        .send(WsMessage::Text(switch_to_b.to_string().into()))
        .await
        .expect("must send SwitchBranch to B");
    let frames_b = collect_until(
        &mut socket,
        |f| matches!(frame_type(f), Some("TimelineUpdate") | Some("Error")),
        Duration::from_secs(8),
    )
    .await;
    let switched_b = frames_b
        .iter()
        .find(|f| frame_type(f) == Some("TimelineUpdate"))
        .unwrap_or_else(|| panic!("SwitchBranch to B must ack; frames = {frames_b:#?}"));
    assert_eq!(
        switched_b["data"]["update"]["from"], "main",
        "the FIRST switch must report the real previous branch, main; frame = {switched_b}"
    );
    assert_eq!(
        switched_b["data"]["update"]["to"],
        branch_b_id.to_string(),
        "the FIRST switch must report the real target branch; frame = {switched_b}"
    );
    assert_eq!(
        state.timeline_recorder.branch_id(),
        branch_b_id,
        "the kernel's active recording branch must have actually moved to B"
    );

    // Create branch C, then switch to it. `from` must now be B's id —
    // never the hardcoded "main" the pre-fix stub always reported.
    let create_c = timeline_command(
        "ws-switch-create-c",
        json!({ "cmd": "CreateBranch", "name": "switch-branch-c", "from_point": null }),
    );
    socket
        .send(WsMessage::Text(create_c.to_string().into()))
        .await
        .expect("must send CreateBranch C");
    let _ = collect_until(
        &mut socket,
        |f| matches!(frame_type(f), Some("TimelineUpdate") | Some("Error")),
        Duration::from_secs(8),
    )
    .await;

    let branch_c_id = {
        let timeline = state.timeline.read().await;
        timeline
            .get_all_branches()
            .into_iter()
            .find(|b| b.name == "switch-branch-c")
            .expect("branch C must have been created")
            .id
    };

    let switch_to_c = timeline_command(
        "ws-switch-to-c",
        json!({ "cmd": "SwitchBranch", "branch_name": "switch-branch-c" }),
    );
    socket
        .send(WsMessage::Text(switch_to_c.to_string().into()))
        .await
        .expect("must send SwitchBranch to C");
    let frames_c = collect_until(
        &mut socket,
        |f| matches!(frame_type(f), Some("TimelineUpdate") | Some("Error")),
        Duration::from_secs(8),
    )
    .await;
    let _ = socket.close(None).await;
    server.abort();

    let switched_c = frames_c
        .iter()
        .find(|f| frame_type(f) == Some("TimelineUpdate"))
        .unwrap_or_else(|| panic!("SwitchBranch to C must ack; frames = {frames_c:#?}"));
    assert_eq!(
        switched_c["data"]["update"]["from"],
        branch_b_id.to_string(),
        "the SECOND switch must report B as the real previous branch, NOT the \
         hardcoded literal \"main\"; frame = {switched_c}"
    );
    assert_ne!(
        switched_c["data"]["update"]["from"], "main",
        "the previous-active branch was B, not main — reporting \"main\" here \
         would be the exact fabrication this fix closes; frame = {switched_c}"
    );
    assert_eq!(
        switched_c["data"]["update"]["to"],
        branch_c_id.to_string(),
        "the SECOND switch must report the real target branch; frame = {switched_c}"
    );
}

/// A WS `Undo` with nothing to undo must surface a TYPED error, never a
/// success ack — the honest-refusal rule this whole fix exists to
/// enforce.
///
/// **Fails against the pre-fix stub:** `Undo` always replies
/// `UndoPerformed` even on a branch with zero events.
#[tokio::test]
async fn ws_undo_with_nothing_to_undo_returns_typed_error() {
    let state = secure_state().await;
    let token = mint_token(&state, "empty-undo-tester");
    let (addr, server) = serve(state.clone()).await;

    let mut socket = connect(addr).await;
    authenticate(&mut socket, &token).await;

    let cmd = timeline_command("ws-undo-empty", json!({ "cmd": "Undo", "steps": null }));
    socket
        .send(WsMessage::Text(cmd.to_string().into()))
        .await
        .expect("must send Undo");
    let frames = collect_until(
        &mut socket,
        |f| matches!(frame_type(f), Some("TimelineUpdate") | Some("Error")),
        Duration::from_secs(8),
    )
    .await;
    let _ = socket.close(None).await;
    server.abort();

    assert_eq!(
        error_code(
            frames
                .iter()
                .find(|f| frame_type(f) == Some("Error"))
                .unwrap_or_else(|| panic!(
                    "an undo with nothing to undo must return a typed Error, not a \
                     success ack; frames = {frames:#?}"
                ))
        ),
        Some("NO_MORE_UNDO"),
        "frames = {frames:#?}"
    );
    assert!(
        !frames
            .iter()
            .any(|f| frame_type(f) == Some("TimelineUpdate")),
        "must never send UndoPerformed when there was nothing to undo; \
         frames = {frames:#?}"
    );
}

// =====================================================================
// WS timeline merge — `TimelineWSCommand::MergeBranch` used to reply
// `Success` with a payload literally asserting "Merged X into Y" while
// never touching the timeline (message_handlers.rs:1853 pre-fix). These
// tests assert on the TIMELINE'S ACTUAL EVENT LOG, not the ack — the
// pre-fix stub would already pass a test that only checked the reply.
// =====================================================================

/// Fork `branch_name` from main's current head and return its `BranchId`.
async fn create_branch_direct(state: &AppState, branch_name: &str) -> BranchId {
    state
        .timeline
        .write()
        .await
        .create_branch(
            branch_name.to_string(),
            BranchId::main(),
            None,
            Author::System,
            BranchPurpose::UserExploration {
                description: "ws merge test".to_string(),
            },
        )
        .await
        .expect("branch creation must succeed for a valid parent")
}

/// A WS `MergeBranch` must genuinely fold the source branch's events into
/// the target — not merely acknowledge success.
///
/// **Fails against the pre-fix stub:** `MergeBranch` always replies
/// `Success` with a fabricated "Merged X into Y" message and never calls
/// into the timeline, so main's event log would still show only the
/// original box.
#[tokio::test]
async fn ws_merge_genuinely_merges_branch_events_into_target() {
    let state = secure_state().await;
    let token = mint_token(&state, "merge-tester");

    // One event on main, then fork B from main's head and record ONE
    // more event on B only — a clean fast-forward candidate.
    let _first_box = create_box(&state, 1.0).await;
    let _ = state.timeline_recorder.flush().await;
    let branch_b = create_branch_direct(&state, "merge-source").await;

    state.timeline_recorder.set_branch_id(branch_b);
    let _second_box = create_box(&state, 2.0).await;
    let _ = state.timeline_recorder.flush().await;
    state.timeline_recorder.set_branch_id(BranchId::main());

    let main_events_before = {
        let timeline = state.timeline.read().await;
        timeline
            .get_branch_events(&BranchId::main(), None, None)
            .expect("main branch events must be readable")
            .len()
    };
    assert_eq!(
        main_events_before, 1,
        "setup must leave main with only the first box before the merge"
    );

    let (addr, server) = serve(state.clone()).await;
    let mut socket = connect(addr).await;
    authenticate(&mut socket, &token).await;

    let cmd = timeline_command(
        "ws-merge-1",
        json!({
            "cmd": "MergeBranch",
            "source": branch_b.to_string(),
            "target": "main",
            "strategy": "PreferSource",
        }),
    );
    socket
        .send(WsMessage::Text(cmd.to_string().into()))
        .await
        .expect("must send MergeBranch");
    let frames = collect_until(
        &mut socket,
        |f| matches!(frame_type(f), Some("Success") | Some("Error")),
        Duration::from_secs(8),
    )
    .await;
    let _ = socket.close(None).await;
    server.abort();

    assert!(
        frames.iter().any(|f| frame_type(f) == Some("Success")),
        "a clean fast-forwardable merge must ack Success; frames = {frames:#?}"
    );

    let main_events_after = {
        let timeline = state.timeline.read().await;
        timeline
            .get_branch_events(&BranchId::main(), None, None)
            .expect("main branch events must be readable")
            .len()
    };
    assert_eq!(
        main_events_after, 2,
        "WS MergeBranch must have genuinely folded B's event into main's \
         event log (1 -> 2), not merely acked success; frames = {frames:#?}"
    );
}

/// A WS `MergeBranch` naming a branch that does not exist must surface a
/// TYPED error, never a success ack.
///
/// **Fails against the pre-fix stub:** any `source`/`target` string,
/// including nonsense, replies `Success`.
#[tokio::test]
async fn ws_merge_with_unknown_branch_returns_typed_error() {
    let state = secure_state().await;
    let token = mint_token(&state, "merge-unknown-tester");
    let (addr, server) = serve(state.clone()).await;

    let mut socket = connect(addr).await;
    authenticate(&mut socket, &token).await;

    let cmd = timeline_command(
        "ws-merge-unknown",
        json!({
            "cmd": "MergeBranch",
            "source": "this-branch-does-not-exist",
            "target": "main",
            "strategy": "PreferSource",
        }),
    );
    socket
        .send(WsMessage::Text(cmd.to_string().into()))
        .await
        .expect("must send MergeBranch");
    let frames = collect_until(
        &mut socket,
        |f| matches!(frame_type(f), Some("Success") | Some("Error")),
        Duration::from_secs(8),
    )
    .await;
    let _ = socket.close(None).await;
    server.abort();

    assert_eq!(
        error_code(
            frames
                .iter()
                .find(|f| frame_type(f) == Some("Error"))
                .unwrap_or_else(|| panic!(
                    "merging an unknown branch must return a typed Error, not a \
                     success ack; frames = {frames:#?}"
                ))
        ),
        Some("BRANCH_NOT_FOUND"),
        "frames = {frames:#?}"
    );
    assert!(
        !frames.iter().any(|f| frame_type(f) == Some("Success")),
        "must never ack Success for a branch that does not exist; \
         frames = {frames:#?}"
    );
}

/// A WS `MergeBranch` that runs into a REAL cross-branch conflict must
/// surface `ServerMessage::Error`, never `Success` — a conflicted merge
/// is a real, expected outcome, and reporting it as success would be
/// exactly the class of lie this fix exists to close. Also asserts
/// nothing was silently folded into main despite the conflict.
///
/// Setup mirrors `router_integration_tests::seed_conflicting_divergence`:
/// box on main -> fork -> DIFFERENT transforms of the SAME solid on each
/// branch, so both post-fork events output `solid:0` and the merge
/// taxonomy must report a `concurrent_modification`.
#[tokio::test]
async fn ws_merge_conflict_surfaces_as_error_not_success() {
    let state = secure_state().await;
    let token = mint_token(&state, "merge-conflict-tester");

    let box_id = create_box(&state, 10.0).await;
    let _ = state.timeline_recorder.flush().await;
    let branch_b = create_branch_direct(&state, "merge-conflict-source").await;

    // Transform on B.
    state.timeline_recorder.set_branch_id(branch_b);
    {
        let mut model_guard = state.model.write().await;
        translate(
            &mut model_guard,
            vec![box_id],
            Vector3::new(1.0, 0.0, 0.0),
            5.0,
            TransformOptions::default(),
        )
        .expect("branch transform must succeed");
    }
    let _ = state.timeline_recorder.flush().await;

    // A DIFFERENT transform of the SAME solid on main.
    state.timeline_recorder.set_branch_id(BranchId::main());
    {
        let mut model_guard = state.model.write().await;
        translate(
            &mut model_guard,
            vec![box_id],
            Vector3::new(0.0, 1.0, 0.0),
            7.0,
            TransformOptions::default(),
        )
        .expect("main transform must succeed");
    }
    let _ = state.timeline_recorder.flush().await;

    let main_events_before = {
        let timeline = state.timeline.read().await;
        timeline
            .get_branch_events(&BranchId::main(), None, None)
            .expect("main branch events must be readable")
            .len()
    };
    assert_eq!(
        main_events_before, 2,
        "setup must leave main with box-create + its own transform"
    );

    let (addr, server) = serve(state.clone()).await;
    let mut socket = connect(addr).await;
    authenticate(&mut socket, &token).await;

    let cmd = timeline_command(
        "ws-merge-conflict",
        json!({
            "cmd": "MergeBranch",
            "source": branch_b.to_string(),
            "target": "main",
            "strategy": "PreferSource",
        }),
    );
    socket
        .send(WsMessage::Text(cmd.to_string().into()))
        .await
        .expect("must send MergeBranch");
    let frames = collect_until(
        &mut socket,
        |f| matches!(frame_type(f), Some("Success") | Some("Error")),
        Duration::from_secs(8),
    )
    .await;
    let _ = socket.close(None).await;
    server.abort();

    assert_eq!(
        error_code(
            frames
                .iter()
                .find(|f| frame_type(f) == Some("Error"))
                .unwrap_or_else(|| panic!(
                    "a real cross-branch conflict must return a typed Error, \
                     never a Success ack; frames = {frames:#?}"
                ))
        ),
        Some("BRANCH_MERGE_CONFLICT"),
        "frames = {frames:#?}"
    );
    assert!(
        !frames.iter().any(|f| frame_type(f) == Some("Success")),
        "a conflicted merge must never ack Success; frames = {frames:#?}"
    );

    let main_events_after = {
        let timeline = state.timeline.read().await;
        timeline
            .get_branch_events(&BranchId::main(), None, None)
            .expect("main branch events must be readable")
            .len()
    };
    assert_eq!(
        main_events_after, 2,
        "a conflicted merge must mutate NOTHING on the target branch — \
         main's event log must be unchanged"
    );
}
