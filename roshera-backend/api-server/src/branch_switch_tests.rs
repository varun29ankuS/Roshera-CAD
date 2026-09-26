//! Task 100 — one branch switch, honest about the live model; undo/redo
//! rebuilt off to the side, or refused.
//!
//! Three routes switched the active branch — `POST /api/branches/active`,
//! the WS `SwitchBranch` command, and `POST /api/timeline/branch/switch/{id}`.
//! The timeline route did not even move the recorder, and answered success on
//! a merged branch. All three now perform one switch that moves recording (or
//! refuses, typed) and SAYS the live model is not rebuilt (`live_model:
//! "not_rebuilt"`): rebuilding on switch is Task 100b, which needs Task
//! 79's document-stable solid ids, and the test that pins that deferred
//! behaviour is `#[ignore]`d under that name.
//!
//! Undo/redo rebuild the live model from a history prefix. They now do it off
//! to the side and refuse, typed, a prefix whose replay would bind a face,
//! edge or solid that history never produced (Task 68's guards, now shared
//! with boot), leaving the model, registry and session pointer untouched. A
//! new undo session starts on the branch being recorded, not on `main`.
//!
//! These tests drive the real router (and a real socket for the WS route) and
//! assert on the LIVE MODEL — the volumes the kernel measures and the uuids
//! clients address it by — never merely on an ack.

#![cfg(test)]

use crate::durability_boot_tests::{
    branch_history, build_state, create_cube, create_part, create_side_cylinder, dispatch,
    fork_from_main, get, live_volumes, open_db, post, settle, temp_db_path, top_face_of,
};
use crate::{build_router, AppState};

use axum::http::StatusCode;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::time::Duration;
use timeline_engine::BranchId;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use uuid::Uuid;

/// The catalog code a switch or undo carries when the target history cannot
/// be replayed faithfully. A literal, so the test pins the wire string.
const BRANCH_REPLAY_REFUSED: &str = "branch_replay_refused";

/// A 10-cube centred on the origin is 1000; a Ø4 × 20 cylinder along Z
/// through it is 80π ≈ 251.327; the cube bored by it is 1000 − 40π ≈ 874.336.
const CUBE: f64 = 1000.0;
const CYLINDER: f64 = 80.0 * std::f64::consts::PI;
const BORED_CUBE: f64 = 1000.0 - 40.0 * std::f64::consts::PI;

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.1
}

fn same_volumes(got: &[f64], want: &[f64]) -> bool {
    let mut want = want.to_vec();
    want.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    got.len() == want.len() && got.iter().zip(&want).all(|(g, w)| close(*g, *w))
}

async fn fresh_state() -> AppState {
    build_state(open_db(&temp_db_path()).await, true).await
}

async fn create_through_cylinder(state: &AppState) -> String {
    create_part(
        state,
        "/api/geometry/cylinder",
        json!({ "center": [0.0, 0.0, -5.0], "axis": [0.0, 0.0, 1.0], "radius": 2.0, "height": 20.0 }),
    )
    .await
}

async fn create_ten_cube(state: &AppState) -> String {
    create_part(
        state,
        "/api/geometry/box",
        json!({ "width": 10.0, "depth": 10.0, "height": 10.0 }),
    )
    .await
}

/// The volume a public uuid resolves to, or `None` when it resolves to no
/// live solid (a 404 from the mass route).
async fn volume_of(state: &AppState, uuid: &str) -> Option<f64> {
    let (s, body) = dispatch(state, get(&format!("/api/agent/parts/uuid/{uuid}/mass"))).await;
    if s == StatusCode::NOT_FOUND {
        return None;
    }
    assert_eq!(s, StatusCode::OK, "mass must resolve or 404; body = {body}");
    body["volume"]
        .as_f64()
        .or_else(|| body["volume"]["value"].as_f64())
}

/// The pre-Task-100 switch path at its core: the recorder retargeted, the
/// live model left alone. Seeds a LEGACY document — one whose kernel ids were
/// allocated across an interleaved multi-branch log in one shared model — by
/// the mechanism itself rather than through a route, so the seed keeps its
/// shape when a switch starts rebuilding the model (Task 100b).
async fn legacy_retarget(state: &AppState, branch: BranchId) {
    settle(state).await;
    state.timeline_recorder.set_branch_id(branch);
}

/// The uuid ↔ kernel-id registry, sorted — what clients address.
fn registry(state: &AppState) -> Vec<(Uuid, u32)> {
    let mut rows: Vec<(Uuid, u32)> = state
        .uuid_to_local
        .iter()
        .map(|e| (*e.key(), *e.value()))
        .collect();
    rows.sort();
    rows
}

// =====================================================================
// The three switch routes
// =====================================================================

#[derive(Clone, Copy, Debug)]
enum Route {
    /// `POST /api/branches/active`
    RestActive,
    /// `POST /api/timeline/branch/switch/{id}`
    TimelinePath,
    /// WS `TimelineCommand { SwitchBranch }`
    Ws,
}

const ROUTES: [Route; 3] = [Route::RestActive, Route::TimelinePath, Route::Ws];

/// Serve `state`'s router on an ephemeral loopback port (the WS route needs
/// a real socket).
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

/// The outcome of one switch, normalised across routes: `Ok(ack)` when the
/// route acknowledged the switch (the REST body, or the WS frame's `update`),
/// `Err((error_code, message, details))` when it refused.
type SwitchResult = Result<Value, (String, String, Value)>;

async fn switch_via(
    state: &AppState,
    addr: SocketAddr,
    route: Route,
    branch: BranchId,
) -> SwitchResult {
    match route {
        Route::RestActive => {
            let (s, body) = dispatch(
                state,
                post(
                    "/api/branches/active",
                    json!({ "branch_id": branch.to_string() }),
                ),
            )
            .await;
            rest_outcome(s, body)
        }
        Route::TimelinePath => {
            let (s, body) = dispatch(
                state,
                post(&format!("/api/timeline/branch/switch/{branch}"), json!({})),
            )
            .await;
            rest_outcome(s, body)
        }
        Route::Ws => ws_switch(addr, branch).await,
    }
}

fn rest_outcome(s: StatusCode, body: Value) -> SwitchResult {
    if s.is_success() {
        Ok(body)
    } else {
        Err((
            body["error_code"]
                .as_str()
                .unwrap_or("<untyped>")
                .to_string(),
            body["error"].as_str().unwrap_or_default().to_string(),
            body["details"].clone(),
        ))
    }
}

async fn ws_switch(addr: SocketAddr, branch: BranchId) -> SwitchResult {
    let url = format!("ws://{addr}/ws");
    let (mut socket, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WebSocket upgrade must succeed");
    let request_id = format!("switch-{}", Uuid::new_v4());
    let frame = json!({
        "type": "TimelineCommand",
        "data": {
            "command": { "cmd": "SwitchBranch", "branch_name": branch.to_string() },
            "request_id": request_id,
        }
    });
    socket
        .send(WsMessage::Text(frame.to_string().into()))
        .await
        .expect("must send SwitchBranch");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let mut seen = Vec::new();
    let outcome = loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break Err((
                "<no reply>".to_string(),
                format!("no switch reply within the deadline; frames = {seen:?}"),
                Value::Null,
            ));
        }
        match tokio::time::timeout(remaining, socket.next()).await {
            Ok(Some(Ok(WsMessage::Text(t)))) => {
                let Ok(v) = serde_json::from_str::<Value>(&t) else {
                    continue;
                };
                let kind = v["type"].as_str().unwrap_or_default().to_string();
                if kind == "TimelineUpdate" && v["data"]["update"]["to"].is_string() {
                    break Ok(v["data"]["update"].clone());
                }
                if kind == "Error" && v["data"]["request_id"] == request_id.as_str() {
                    break Err((
                        v["data"]["error_code"]
                            .as_str()
                            .unwrap_or("<untyped>")
                            .to_string(),
                        v["data"]["message"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        v["data"]["details"].clone(),
                    ));
                }
                seen.push(kind);
            }
            Ok(Some(Ok(_))) => {}
            _ => {
                break Err((
                    "<socket closed>".to_string(),
                    format!("socket closed before a switch reply; frames = {seen:?}"),
                    Value::Null,
                ))
            }
        }
    };
    let _ = socket.close(None).await;
    outcome
}

// =====================================================================
// (b) — one switch through every route
// =====================================================================

/// Every route moves recording to the branch, reports the branch it came
/// from, and SAYS the live model is not rebuilt — the live model still holds
/// what it held, and the next op is recorded on the branch switched to. The
/// timeline route used to leave the recorder where it was.
#[tokio::test]
async fn every_switch_route_moves_recording_and_says_the_live_model_is_not_rebuilt() {
    for route in ROUTES {
        let state = fresh_state().await;
        let (addr, server) = serve(state.clone()).await;
        let main = BranchId::main();
        create_cube(&state, 10.0).await;
        let b = fork_from_main(&state, &format!("retarget-{route:?}")).await;
        let main_before = branch_history(&state, main).await;
        let b_before = branch_history(&state, b).await;

        let ack = switch_via(&state, addr, route, b)
            .await
            .unwrap_or_else(|e| panic!("{route:?}: switch to B must succeed; {e:?}"));
        assert_eq!(
            state.timeline_recorder.branch_id(),
            b,
            "{route:?}: recording must move to B; ack = {ack}"
        );
        assert_eq!(
            ack["live_model"], "not_rebuilt",
            "{route:?}: the switch must say the live model is not rebuilt; ack = {ack}"
        );
        assert!(
            same_volumes(&live_volumes(&state).await, &[CUBE]),
            "{route:?}: the live model still holds what it held"
        );

        create_side_cylinder(&state).await;
        let b_after = branch_history(&state, b).await;
        assert!(
            b_after.len() > b_before.len() && b_after.starts_with(&b_before),
            "{route:?}: the op after the switch is recorded on B"
        );
        assert_eq!(
            branch_history(&state, main).await,
            main_before,
            "{route:?}: and not on main"
        );

        let ack = switch_via(&state, addr, route, main)
            .await
            .unwrap_or_else(|e| panic!("{route:?}: switch back to main must succeed; {e:?}"));
        assert_eq!(state.timeline_recorder.branch_id(), main, "{route:?}");
        assert_eq!(ack["live_model"], "not_rebuilt", "{route:?}: ack = {ack}");
        server.abort();
    }
}

// =====================================================================
// Deferred (Task 100b, after Task 79) — each branch's model, through every route
// =====================================================================

/// main: 10-cube → fork B → switch B → cylinder through the cube → switch
/// main → switch B → bore the cube on B → round trip. After every switch the
/// live model is exactly what that branch's history describes, the op run
/// after a switch sees that branch's geometry, and the three routes behave
/// identically.
///
/// RED for the deferred work: a switch does not rebuild the live model yet.
/// Rebuilding it needs document-stable solid ids first — each rebuild
/// allocates solid ids from zero, so two branches record different solids
/// under the same id and a three-way merge of them reports false conflicts.
#[tokio::test]
#[ignore = "Task 100b (needs Task 79's document-stable solid ids)"]
async fn every_switch_route_rebuilds_the_model_the_target_branch_describes() {
    for route in ROUTES {
        let state = fresh_state().await;
        let (addr, server) = serve(state.clone()).await;
        let main = BranchId::main();

        let cube = create_ten_cube(&state).await;
        let b = fork_from_main(&state, &format!("switch-{route:?}")).await;
        switch_via(&state, addr, route, b)
            .await
            .unwrap_or_else(|e| panic!("{route:?}: switch to B must succeed; {e:?}"));
        assert_eq!(
            state.timeline_recorder.branch_id(),
            b,
            "{route:?}: recording must move to B"
        );
        let cylinder = create_through_cylinder(&state).await;
        assert!(
            same_volumes(&live_volumes(&state).await, &[CUBE, CYLINDER]),
            "{route:?}: sanity — B holds the cube and the cylinder"
        );

        switch_via(&state, addr, route, main)
            .await
            .unwrap_or_else(|e| panic!("{route:?}: switch to main must succeed; {e:?}"));
        let on_main = live_volumes(&state).await;
        assert!(
            same_volumes(&on_main, &[CUBE]),
            "{route:?}: main's history holds only the cube, so its live model must too; got {on_main:?}"
        );
        assert_eq!(
            volume_of(&state, &cylinder).await,
            None,
            "{route:?}: B's cylinder must not be addressable on main"
        );
        assert!(
            volume_of(&state, &cube)
                .await
                .is_some_and(|v| close(v, CUBE)),
            "{route:?}: the shared cube keeps its uuid across the switch"
        );
        let (s, body) = dispatch(
            &state,
            post(
                "/api/geometry/boolean",
                json!({ "operation": "difference", "object_a": cube, "object_b": cylinder }),
            ),
        )
        .await;
        assert!(
            !s.is_success(),
            "{route:?}: a boolean against B's cylinder must be refused on main; body = {body}"
        );
        assert!(
            same_volumes(&live_volumes(&state).await, &[CUBE]),
            "{route:?}: the refused boolean leaves main's model alone"
        );

        switch_via(&state, addr, route, b)
            .await
            .unwrap_or_else(|e| panic!("{route:?}: switch back to B must succeed; {e:?}"));
        let on_b = live_volumes(&state).await;
        assert!(
            same_volumes(&on_b, &[CUBE, CYLINDER]),
            "{route:?}: B's model comes back — cube and cylinder; got {on_b:?}"
        );
        assert!(
            volume_of(&state, &cylinder)
                .await
                .is_some_and(|v| close(v, CYLINDER)),
            "{route:?}: the cylinder is addressable by the uuid it had on B"
        );
        let (s, body) = dispatch(
            &state,
            post(
                "/api/geometry/boolean",
                json!({ "operation": "difference", "object_a": cube, "object_b": cylinder }),
            ),
        )
        .await;
        assert_eq!(
            s,
            StatusCode::OK,
            "{route:?}: on B the boolean sees B's geometry and applies; body = {body}"
        );
        assert!(
            same_volumes(&live_volumes(&state).await, &[BORED_CUBE]),
            "{route:?}: B's cube is bored"
        );

        // Round trip: ids recorded AFTER a switch replay cleanly.
        switch_via(&state, addr, route, main)
            .await
            .unwrap_or_else(|e| panic!("{route:?}: round-trip switch to main; {e:?}"));
        assert!(
            same_volumes(&live_volumes(&state).await, &[CUBE]),
            "{route:?}: main is still just the cube"
        );
        switch_via(&state, addr, route, b)
            .await
            .unwrap_or_else(|e| panic!("{route:?}: round-trip switch to B; {e:?}"));
        let round_trip = live_volumes(&state).await;
        assert!(
            same_volumes(&round_trip, &[BORED_CUBE]),
            "{route:?}: B replays to its bored cube; got {round_trip:?}"
        );
        server.abort();
    }
}

/// Every route refuses a branch that can take no events (merged), typed,
/// and moves nothing — the timeline route used to answer success on it.
#[tokio::test]
async fn every_switch_route_refuses_a_merged_branch_typed() {
    for route in ROUTES {
        let state = fresh_state().await;
        let (addr, server) = serve(state.clone()).await;
        create_cube(&state, 10.0).await;
        let merged = fork_from_main(&state, &format!("merged-{route:?}")).await;
        let (s, body) = dispatch(
            &state,
            post(&format!("/api/branches/{merged}/merge"), json!({})),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "merge must apply; body = {body}");
        let before = registry(&state);

        let outcome = switch_via(&state, addr, route, merged).await;
        let (code, message, _details) = outcome
            .err()
            .unwrap_or_else(|| panic!("{route:?}: a merged branch must be refused"));
        assert_eq!(code, "branch_not_active", "{route:?}: {message}");
        assert!(!message.contains("  "), "{route:?}: {message:?}");
        assert_eq!(state.timeline_recorder.branch_id(), BranchId::main());
        assert_eq!(registry(&state), before, "{route:?}: registry untouched");
        server.abort();
    }
}

// =====================================================================
// Legacy seeds — documents recorded in one shared model across branches
// =====================================================================

/// Legacy seed: main 10-cube → side branch cylinder (recorded into the SAME
/// model — the pre-fix switch) → main 4-cube → pull its top face by 3. The
/// extrude names its face by a raw kernel id allocated after the side
/// branch's cylinder, so main's history replayed alone binds a different
/// face (measured by Task 68: 80 instead of 112). Returns (side, extrude seq).
async fn seed_legacy_face_pull(state: &AppState) -> (BranchId, u64) {
    create_cube(state, 10.0).await;
    let side = fork_from_main(state, "legacy-side").await;
    legacy_retarget(state, side).await;
    create_side_cylinder(state).await;
    legacy_retarget(state, BranchId::main()).await;
    let small = create_part(
        state,
        "/api/geometry/box",
        json!({ "width": 4.0, "depth": 4.0, "height": 4.0 }),
    )
    .await;
    let solid_id = state
        .get_local_id(&Uuid::parse_str(&small).expect("uuid"))
        .expect("the 4-cube must be registered");
    let top = top_face_of(state, solid_id).await;
    let (s, body) = dispatch(
        state,
        post(
            "/api/geometry/face/extrude",
            json!({ "object_uuid": small, "face_id": top, "distance": 3.0 }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "face/extrude must apply; body = {body}");
    assert!(
        live_volumes(state).await.iter().any(|v| close(*v, 112.0)),
        "sanity: the pulled 4-cube is 112 in the live (shared) model"
    );
    let seq = branch_history(state, BranchId::main())
        .await
        .last()
        .map(|e| e.0)
        .expect("main holds the extrude");
    (side, seq)
}

// =====================================================================
// (d) — undo on an interleaved history that would shift ids
// =====================================================================

#[tokio::test]
async fn an_undo_whose_replay_would_shift_face_ids_is_refused_and_moves_nothing() {
    let state = fresh_state().await;
    let (_side, extrude_seq) = seed_legacy_face_pull(&state).await;
    // One more op on main, so the undo's replay runs through the extrude.
    create_cube(&state, 2.0).await;
    settle(&state).await;
    let volumes_before = live_volumes(&state).await;
    let registry_before = registry(&state);
    let session = crate::handlers::timeline::live_session_id(&BranchId::main());

    let (s, body) = dispatch(
        &state,
        post(
            "/api/timeline/undo",
            json!({ "session_id": session.to_string() }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "undo answers in-band; body = {body}");
    assert_eq!(
        body["success"], false,
        "the undo's replay binds a shifted face id, so it must be refused; body = {body}"
    );
    assert_eq!(body["error_code"], BRANCH_REPLAY_REFUSED, "body = {body}");
    assert_eq!(
        body["details"]["kind"], "interleaved_foreign_history",
        "body = {body}"
    );
    assert_eq!(
        body["details"]["sequence"],
        json!(extrude_seq),
        "body = {body}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(!message.contains("  "), "{message:?}");
    let after = live_volumes(&state).await;
    assert_eq!(
        after, volumes_before,
        "the refused undo leaves the model alone"
    );
    assert!(
        !after.iter().any(|v| close(*v, 80.0)),
        "never the wrong face"
    );
    assert_eq!(registry(&state), registry_before, "registry untouched");

    // The session's pointer was put back to the head it was planted at.
    let head = branch_history(&state, BranchId::main()).await.len() as u64;
    let pointer = state
        .timeline
        .read()
        .await
        .get_session_position(session)
        .map(|p| p.event_index);
    assert_eq!(
        pointer,
        Some(head),
        "the refused undo must leave the session pointer at the head"
    );
    // The identical undo earns the identical refusal, not a drifted
    // "nothing to undo" or a deeper undo.
    let (_, again) = dispatch(
        &state,
        post(
            "/api/timeline/undo",
            json!({ "session_id": session.to_string() }),
        ),
    )
    .await;
    assert_eq!(again["error_code"], BRANCH_REPLAY_REFUSED, "body = {again}");
    assert_eq!(again["details"]["sequence"], json!(extrude_seq));
}

/// The solid guard at runtime: a legacy main boolean against a side
/// branch's cylinder (main's history never produced it), then one more op on
/// main. Undoing that op replays main's prefix through the boolean, whose
/// `solid_b` would bind whatever solid now carries the cylinder's number
/// (measured by Task 68: the 4-cube, 936) — refused, typed, nothing moved.
#[tokio::test]
async fn an_undo_whose_replay_would_bind_a_foreign_solid_is_refused_and_moves_nothing() {
    let state = fresh_state().await;
    let cube = create_ten_cube(&state).await;
    let side = fork_from_main(&state, "legacy-solid-side").await;
    legacy_retarget(&state, side).await;
    let cylinder = create_through_cylinder(&state).await;
    legacy_retarget(&state, BranchId::main()).await;
    let _small = create_part(
        &state,
        "/api/geometry/box",
        json!({ "width": 4.0, "depth": 4.0, "height": 4.0 }),
    )
    .await;
    let (s, body) = dispatch(
        &state,
        post(
            "/api/geometry/boolean",
            json!({ "operation": "difference", "object_a": cube, "object_b": cylinder }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "legacy boolean applies; body = {body}");
    create_cube(&state, 2.0).await;
    settle(&state).await;
    let volumes_before = live_volumes(&state).await;
    let registry_before = registry(&state);
    let session = crate::handlers::timeline::live_session_id(&BranchId::main());

    let (s, body) = dispatch(
        &state,
        post(
            "/api/timeline/undo",
            json!({ "session_id": session.to_string() }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "undo answers in-band; body = {body}");
    assert_eq!(body["success"], false, "body = {body}");
    assert_eq!(body["error_code"], BRANCH_REPLAY_REFUSED, "body = {body}");
    assert_eq!(
        body["details"]["kind"], "foreign_solid_input",
        "body = {body}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("solid:") && !message.contains("  "),
        "the refusal names the solid, single-spaced: {message:?}"
    );
    let after = live_volumes(&state).await;
    assert_eq!(
        after, volumes_before,
        "the refused undo leaves the model alone"
    );
    assert!(
        !after.iter().any(|v| close(*v, 936.0)),
        "never the wrong operand"
    );
    assert_eq!(registry(&state), registry_before, "registry untouched");
    let head = branch_history(&state, BranchId::main()).await.len() as u64;
    let pointer = state
        .timeline
        .read()
        .await
        .get_session_position(session)
        .map(|p| p.event_index);
    assert_eq!(
        pointer,
        Some(head),
        "the refused undo must leave the session pointer at the head"
    );
}

// =====================================================================
// undo after a switch undoes the branch being recorded
// =====================================================================

/// After switching to B, an undo from a session that has never undone
/// before must step back B's last op — not plant itself on main and replay
/// main into the live model while recording stays on B.
#[tokio::test]
async fn an_undo_after_a_switch_steps_back_the_recording_branch() {
    let state = fresh_state().await;
    let (addr, server) = serve(state.clone()).await;
    create_cube(&state, 10.0).await;
    let b = fork_from_main(&state, "undo-b").await;
    switch_via(&state, addr, Route::RestActive, b)
        .await
        .unwrap_or_else(|e| panic!("switch to B; {e:?}"));
    create_through_cylinder(&state).await;
    create_cube(&state, 2.0).await;
    settle(&state).await;

    // A cube records two ops (create, place): two undos step it back.
    let session = Uuid::new_v4();
    for _ in 0..2 {
        let (s, body) = dispatch(
            &state,
            post(
                "/api/timeline/undo",
                json!({ "session_id": session.to_string() }),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "body = {body}");
        assert_eq!(body["success"], true, "body = {body}");
    }
    let after = live_volumes(&state).await;
    assert!(
        same_volumes(&after, &[CUBE, CYLINDER]),
        "the undo steps back B's 2-cube and keeps B's cylinder; got {after:?}"
    );
    assert_eq!(state.timeline_recorder.branch_id(), b);
    server.abort();
}

// =====================================================================
// Round 2 — mutations that cannot be undone check BEFORE mutating
// =====================================================================

/// Ordinary post-change usage (no legacy seed): main 10-cube → fork B →
/// main cylinder → switch to B (recording only; the model is shared) → B
/// 4-cube, pull its top face by 3 → B 2-cube. B's history now addresses a
/// raw face after main's cylinder ran in the same model, so B's replay is
/// refused. Returns (B, the extrude's event id, the 4-cube's create event
/// id, the switch server's address, its handle).
async fn seed_switched_face_pull(
    state: &AppState,
) -> (
    BranchId,
    String,
    String,
    SocketAddr,
    tokio::task::JoinHandle<()>,
) {
    let (addr, server) = serve(state.clone()).await;
    create_cube(state, 10.0).await;
    let b = fork_from_main(state, "pull-on-b").await;
    create_side_cylinder(state).await;
    switch_via(state, addr, Route::RestActive, b)
        .await
        .unwrap_or_else(|e| panic!("switch to B; {e:?}"));
    let before_small = branch_history(state, b).await;
    let small = create_part(
        state,
        "/api/geometry/box",
        json!({ "width": 4.0, "depth": 4.0, "height": 4.0 }),
    )
    .await;
    let small_create = branch_history(state, b)
        .await
        .into_iter()
        .find(|e| !before_small.contains(e))
        .map(|e| e.1)
        .expect("the 4-cube recorded its create on B");
    let solid_id = state
        .get_local_id(&Uuid::parse_str(&small).expect("uuid"))
        .expect("the 4-cube must be registered");
    let top = top_face_of(state, solid_id).await;
    let (s, body) = dispatch(
        state,
        post(
            "/api/geometry/face/extrude",
            json!({ "object_uuid": small, "face_id": top, "distance": 3.0 }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "face/extrude must apply; body = {body}");
    let extrude = branch_history(state, b)
        .await
        .last()
        .map(|e| e.1.clone())
        .expect("B holds the extrude");
    create_cube(state, 2.0).await;
    settle(state).await;
    (b, extrude, small_create, addr, server)
}

/// The reviewer's probe: truncating B after the extrude would drop the
/// 2-cube from the ledger and then have its rebuild refused — leaving a live
/// model that still holds the 2-cube the ledger no longer has. A truncate
/// cannot be undone, so it is refused BEFORE anything changes.
#[tokio::test]
async fn a_truncate_whose_rebuild_would_be_refused_changes_nothing() {
    let state = fresh_state().await;
    let (b, extrude, _, _addr, server) = seed_switched_face_pull(&state).await;
    let history_before = branch_history(&state, b).await;
    let volumes_before = live_volumes(&state).await;
    let registry_before = registry(&state);

    let (s, body) = dispatch(
        &state,
        post(
            "/api/timeline/truncate",
            json!({
                "session_id": Uuid::new_v4().to_string(),
                "event_id": extrude,
                "branch_id": b.to_string(),
                "mode": "after_here",
            }),
        ),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "the truncate must be refused before it mutates; body = {body}"
    );
    assert_eq!(body["error_code"], BRANCH_REPLAY_REFUSED, "body = {body}");
    assert_eq!(
        body["details"]["kind"], "interleaved_foreign_history",
        "body = {body}"
    );
    assert_ne!(body["success"], true, "body = {body}");
    let message = body["error"].as_str().unwrap_or_default();
    assert!(!message.contains("  "), "{message:?}");
    assert_eq!(
        branch_history(&state, b).await,
        history_before,
        "the ledger is untouched"
    );
    assert_eq!(
        live_volumes(&state).await,
        volumes_before,
        "the model is untouched"
    );
    assert_eq!(
        registry(&state),
        registry_before,
        "the registry is untouched"
    );
    server.abort();
}

/// The same history, moulded: the mould's live replay (and the candidate
/// replay its reported objects and certificate come from) would be refused,
/// so nothing is appended and nothing is reported as applied.
#[tokio::test]
async fn a_mould_whose_rebuild_would_be_refused_appends_nothing() {
    let state = fresh_state().await;
    let (b, _, small_create, _addr, server) = seed_switched_face_pull(&state).await;
    let history_before = branch_history(&state, b).await;
    let volumes_before = live_volumes(&state).await;

    let (s, body) = dispatch(
        &state,
        post(
            "/api/timeline/mould",
            json!({
                "branch_id": b.to_string(),
                "target_event_id": small_create,
                "parameter": "width",
                "value": 5.0,
            }),
        ),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "the mould must be refused before it appends; body = {body}"
    );
    assert_eq!(body["error_code"], BRANCH_REPLAY_REFUSED, "body = {body}");
    assert_ne!(body["status"], "MouldApplied", "body = {body}");
    assert!(
        body["objects"].is_null(),
        "no objects claimed; body = {body}"
    );
    assert_eq!(
        branch_history(&state, b).await,
        history_before,
        "nothing appended"
    );
    assert_eq!(
        live_volumes(&state).await,
        volumes_before,
        "the model is untouched"
    );
    server.abort();
}

// =====================================================================
// Round 2 — an op that lands during the off-lock rebuild is never lost
// =====================================================================

/// An undo rebuilds its prefix off the model lock. A cube created WHILE
/// that rebuild runs (driven deterministically through the test gate) is in
/// the live model and the ledger but not in the rebuild; swapping the
/// rebuild in would drop it. The undo is refused (`history_moved`) and the
/// cube stays live and addressable.
#[tokio::test]
async fn an_op_landing_during_an_undo_rebuild_is_never_dropped() {
    let state = fresh_state().await;
    create_cube(&state, 10.0).await;
    create_cube(&state, 3.0).await;
    settle(&state).await;
    let session = Uuid::new_v4();
    let (rebuilt, proceed) = crate::handlers::timeline::replay_race_hook::install(session);

    let undo_state = state.clone();
    let undo = tokio::spawn(async move {
        dispatch(
            &undo_state,
            post(
                "/api/timeline/undo",
                json!({ "session_id": session.to_string() }),
            ),
        )
        .await
    });

    rebuilt.notified().await;
    let landed = create_part(
        &state,
        "/api/geometry/box",
        json!({ "width": 2.0, "depth": 2.0, "height": 2.0 }),
    )
    .await;
    proceed.notify_one();
    let (s, body) = undo.await.expect("the undo task must finish");

    assert_eq!(s, StatusCode::OK, "undo answers in-band; body = {body}");
    assert_eq!(
        body["success"], false,
        "the undo must not swap; body = {body}"
    );
    assert_eq!(body["error_code"], BRANCH_REPLAY_REFUSED, "body = {body}");
    assert_eq!(body["details"]["kind"], "history_moved", "body = {body}");
    assert!(
        volume_of(&state, &landed)
            .await
            .is_some_and(|v| close(v, 8.0)),
        "the cube created during the rebuild must stay live and addressable"
    );
}

// =====================================================================
// Round 2 — the WS undo refusal carries the typed body
// =====================================================================

#[tokio::test]
async fn a_ws_undo_refusal_carries_the_typed_details() {
    let state = fresh_state().await;
    let (addr, server) = serve(state.clone()).await;
    let (_side, extrude_seq) = seed_legacy_face_pull(&state).await;
    create_cube(&state, 2.0).await;
    settle(&state).await;
    let volumes_before = live_volumes(&state).await;

    let url = format!("ws://{addr}/ws");
    let (mut socket, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WebSocket upgrade must succeed");
    let request_id = format!("undo-{}", Uuid::new_v4());
    let frame = json!({
        "type": "TimelineCommand",
        "data": { "command": { "cmd": "Undo", "steps": null }, "request_id": request_id }
    });
    socket
        .send(WsMessage::Text(frame.to_string().into()))
        .await
        .expect("must send Undo");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let reply = loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        match tokio::time::timeout(remaining, socket.next()).await {
            Ok(Some(Ok(WsMessage::Text(t)))) => {
                let Ok(v) = serde_json::from_str::<Value>(&t) else {
                    continue;
                };
                let kind = v["type"].as_str().unwrap_or_default().to_string();
                if kind == "TimelineUpdate" || kind == "Error" {
                    break v;
                }
            }
            Ok(Some(Ok(_))) => {}
            _ => panic!("no undo reply"),
        }
    };
    let _ = socket.close(None).await;
    server.abort();

    assert_eq!(
        reply["type"], "Error",
        "the undo must be refused; frame = {reply}"
    );
    assert_eq!(
        reply["data"]["error_code"], BRANCH_REPLAY_REFUSED,
        "frame = {reply}"
    );
    assert_eq!(
        reply["data"]["details"]["kind"], "interleaved_foreign_history",
        "frame = {reply}"
    );
    assert_eq!(
        reply["data"]["details"]["sequence"],
        json!(extrude_seq),
        "frame = {reply}"
    );
    assert_eq!(live_volumes(&state).await, volumes_before, "nothing moved");
}

// =====================================================================
// Round 3 — a mutation whose rebuild then fails says the ledger changed
// =====================================================================

/// A truncate that passes its pre-flight but whose rebuild is then refused
/// — here because an op lands during the rebuild (driven through the test
/// gate) — has already changed the ledger. The answer must say so, and its
/// hint must not claim "the history is intact": it points to the replay
/// route that rebuilds the model from the ledger as it now is.
#[tokio::test]
async fn a_truncate_whose_rebuild_fails_afterwards_says_the_ledger_changed() {
    let state = fresh_state().await;
    let (addr, server) = serve(state.clone()).await;
    create_cube(&state, 10.0).await;
    let b = fork_from_main(&state, "truncate-then-race").await;
    switch_via(&state, addr, Route::RestActive, b)
        .await
        .unwrap_or_else(|e| panic!("switch to B; {e:?}"));
    create_cube(&state, 3.0).await;
    settle(&state).await;
    let cut_at = branch_history(&state, b)
        .await
        .last()
        .map(|e| e.1.clone())
        .expect("B holds the 3-cube");
    let session = Uuid::new_v4();
    let (rebuilt, proceed) = crate::handlers::timeline::replay_race_hook::install(session);

    let truncate_state = state.clone();
    let truncate = tokio::spawn(async move {
        dispatch(
            &truncate_state,
            post(
                "/api/timeline/truncate",
                json!({
                    "session_id": session.to_string(),
                    "event_id": cut_at,
                    "branch_id": b.to_string(),
                    "mode": "from_here",
                }),
            ),
        )
        .await
    });
    rebuilt.notified().await;
    create_cube(&state, 2.0).await;
    proceed.notify_one();
    let (s, body) = truncate.await.expect("the truncate task must finish");
    server.abort();

    assert_eq!(s, StatusCode::CONFLICT, "body = {body}");
    assert_eq!(body["success"], false, "body = {body}");
    assert_eq!(body["ledger_changed"], true, "body = {body}");
    assert_eq!(body["model_reconciled"], false, "body = {body}");
    assert_eq!(body["details"]["kind"], "history_moved", "body = {body}");
    let hint = body["hint"].as_str().unwrap_or_default();
    assert!(
        hint.contains("POST /api/timeline/replay") && !hint.contains("intact"),
        "the hint must point to the replay route, not claim an intact history: {hint:?}"
    );
    assert!(!hint.contains("  "), "{hint:?}");
}

/// Truncating the protected trunk is refused for THAT reason first — even
/// when the rebuild after it would also be refused.
#[tokio::test]
async fn a_truncate_on_the_protected_trunk_is_refused_as_protected_first() {
    let state = fresh_state().await;
    let (_side, _extrude_seq) = seed_legacy_face_pull(&state).await;
    create_cube(&state, 2.0).await;
    settle(&state).await;
    let extrude = branch_history(&state, BranchId::main())
        .await
        .iter()
        .rev()
        .nth(2)
        .map(|e| e.1.clone())
        .expect("main holds the extrude");
    let history_before = branch_history(&state, BranchId::main()).await;

    let (s, body) = dispatch(
        &state,
        post(
            "/api/timeline/truncate",
            json!({
                "session_id": Uuid::new_v4().to_string(),
                "event_id": extrude,
                "branch_id": "main",
                "mode": "after_here",
            }),
        ),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::INTERNAL_SERVER_ERROR,
        "the protected-branch refusal answers first (truncate_branch's own status); body = {body}"
    );
    assert_ne!(body["error_code"], BRANCH_REPLAY_REFUSED, "body = {body}");
    assert_eq!(
        branch_history(&state, BranchId::main()).await,
        history_before
    );
}
