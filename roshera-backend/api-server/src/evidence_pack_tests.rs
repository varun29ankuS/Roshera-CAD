//! Evidence-pack export — wire tests driving [`build_router`] end-to-end.
//!
//! The feature: `GET /api/evidence-pack` bundles a document's recorded
//! design history — per-operation record + certificate (AS RECORDED) +
//! measured metrics + the agent's notebook — into one machine-readable JSON
//! pack, the reviewable-evidence format the AI-training-data industry
//! assembles by hand.
//!
//! Same philosophy as `auth_slice5_tests`: every test drives the fully
//! assembled router, because only the assembled router proves what a caller
//! on the wire actually receives — routing, the global auth layer, extractors,
//! and the JSON body all at once.
//!
//! # Honesty, pinned
//!
//! The pack REPORTS recorded history. These tests pin the contract that it
//! never fabricates a certificate for an operation that carries none (the
//! `certificate` field is present-but-`null` with a reason), and that a
//! re-measured verdict lives only under the separately-labeled `recomputed`
//! field — so a fresh measurement can never masquerade as recorded history.

#![cfg(test)]

use crate::auth_middleware::AuthPosture;
use crate::blackboard::{BlackboardScope, LineAuthor};
use crate::router_integration_tests::make_test_state;
use crate::{build_router, AppState};

use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

// =====================================================================
// Harness (mirrors auth_slice5_tests)
// =====================================================================

/// Dispatch through the fully-assembled router; return status + JSON body.
async fn dispatch(state: &AppState, request: Request<Body>) -> (StatusCode, Value) {
    let response = build_router(state.clone())
        .oneshot(request)
        .await
        .expect("router must produce a response (oneshot is infallible)");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body must serialise to finite bytes");
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, body)
}

fn request(method: Method, path: &str, auth: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(header) = auth {
        builder = builder.header("Authorization", header);
    }
    match body {
        Some(v) => builder
            .header("Content-Type", "application/json")
            .body(Body::from(v.to_string()))
            .expect("request must build"),
        None => builder.body(Body::empty()).expect("request must build"),
    }
}

/// Mint a Bearer credential for `user_id` exactly as the login handler would.
fn bearer_for(state: &AppState, user_id: &str) -> String {
    let token = state
        .session_manager
        .auth_manager()
        .create_token(user_id, None, vec!["user".to_string()])
        .expect("test token must mint");
    format!("Bearer {}", token.token)
}

/// Create a box through the REAL geometry endpoint so it lands on the kernel
/// model AND records a timeline event, exactly as production. The default
/// `make_test_state` posture is the dev bypass, so no credential is needed;
/// the auth boundary is pinned separately.
async fn create_box(state: &AppState, w: f64, d: f64, h: f64) {
    let (status, body) = dispatch(
        state,
        request(
            Method::POST,
            "/api/geometry/box",
            None,
            Some(json!({ "width": w, "depth": d, "height": h })),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "box create must succeed to seed recorded history; body: {body}"
    );
}

/// Fetch the pack for the default (main) scope and assert 200.
async fn fetch_pack(state: &AppState) -> Value {
    let (status, body) = dispatch(
        state,
        request(Method::GET, "/api/evidence-pack", None, None),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "evidence-pack must return 200; body: {body}"
    );
    body
}

// =====================================================================
// Auth boundary
// =====================================================================

#[tokio::test]
async fn evidence_pack_requires_a_credential() {
    // Under the enforced posture, an unauthenticated caller must never reach
    // the pack — it can carry the whole document's design history.
    let mut state = make_test_state().await;
    state.auth_posture = AuthPosture::Required;

    let (status, _body) = dispatch(
        &state,
        request(Method::GET, "/api/evidence-pack", None, None),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "an unauthenticated caller must not be able to export an evidence pack"
    );

    // A valid Bearer credential passes the front door and gets the pack.
    let bearer = bearer_for(&state, "reviewer_alpha");
    let (status, body) = dispatch(
        &state,
        request(Method::GET, "/api/evidence-pack", Some(&bearer), None),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a credentialed caller must receive the pack; body: {body}"
    );
}

// =====================================================================
// Recorded operations + certificates
// =====================================================================

#[tokio::test]
async fn pack_reports_exactly_the_recorded_operations() {
    let state = make_test_state().await;
    create_box(&state, 10.0, 10.0, 10.0).await;
    create_box(&state, 20.0, 20.0, 20.0).await;

    // Ground truth: the recorded event log itself, read through the existing
    // history projection. The pack must report EXACTLY this — no filtering,
    // no invention. (Two `/api/geometry/box` calls record more than two
    // events: each also records the positioning `transform_solid` and the
    // auto-name `set_name`. The pack faithfully carries the full history.)
    let (status, history) = dispatch(
        &state,
        request(Method::GET, "/api/timeline/history/main", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let recorded_count = history.as_array().expect("history is an array").len();
    assert!(
        recorded_count >= 2,
        "the two box creations must have recorded at least two events; got {recorded_count}"
    );

    let pack = fetch_pack(&state).await;
    let ops = pack["operations"]
        .as_array()
        .expect("operations must be an array");
    assert_eq!(
        ops.len(),
        recorded_count,
        "the pack must contain exactly the recorded operations (N = {recorded_count}); pack: {pack}"
    );
    assert_eq!(
        pack["manifest"]["operation_count"],
        json!(recorded_count),
        "manifest.operation_count must equal the entry count"
    );

    // Exactly the two box creations are present among the recorded ops.
    let box_creates = ops
        .iter()
        .filter(|op| op["op_kind"].as_str() == Some("create_box_3d"))
        .count();
    assert_eq!(
        box_creates, 2,
        "exactly the two recorded box creations must appear; pack: {pack}"
    );

    // Each entry is sequence-ordered and carries the recorded op kind,
    // timestamp, author, and a `certificate` FIELD (present — possibly null,
    // never absent).
    let mut last_seq: Option<u64> = None;
    for (i, op) in ops.iter().enumerate() {
        let obj = op.as_object().expect("each operation is an object");
        let seq = op["sequence"]
            .as_u64()
            .expect("op carries a numeric sequence");
        if let Some(prev) = last_seq {
            assert!(
                seq >= prev,
                "operations must be sequence-ordered; op {i} = {op}"
            );
        }
        last_seq = Some(seq);
        assert!(
            op["op_kind"].as_str().is_some_and(|k| !k.is_empty()),
            "op {i} carries a non-empty kernel op_kind; op = {op}"
        );
        assert!(
            op["timestamp"].as_str().is_some(),
            "op {i} carries an RFC3339 timestamp; op = {op}"
        );
        assert!(op["author"].as_str().is_some(), "op {i} names an author");
        assert!(
            obj.contains_key("certificate"),
            "op {i} must carry a `certificate` field (present, even when null); op = {op}"
        );
    }
}

#[tokio::test]
async fn absent_certificate_is_null_with_a_reason_never_fabricated() {
    // The honesty core: today no producer writes a per-op EventCertificate, so
    // every recorded op reports `certificate: null` WITH an explicit reason —
    // an honest "not certified", never a fabricated green. If/when producers
    // begin recording certificates, this reads them back verbatim instead.
    let state = make_test_state().await;
    create_box(&state, 10.0, 10.0, 10.0).await;

    let pack = fetch_pack(&state).await;
    let op = &pack["operations"][0];

    // Mutation guard: an impl that fabricated a certificate for an
    // uncertified op (e.g. a synthetic skipped_solid) would put an object
    // here and drop the reason — this assertion fails in that case.
    assert!(
        op["certificate"].is_null(),
        "an uncertified op must report certificate=null, never a fabricated verdict; op = {op}"
    );
    assert!(
        op["certificate_absent_reason"]
            .as_str()
            .is_some_and(|r| !r.is_empty()),
        "a null certificate must carry an explicit reason; op = {op}"
    );
}

#[tokio::test]
async fn recomputed_verdict_is_separate_from_recorded_history() {
    // A re-measured verdict must live ONLY under the labeled `recomputed`
    // field — never inlined into an operation's recorded `certificate`.
    let state = make_test_state().await;
    create_box(&state, 10.0, 10.0, 10.0).await;

    let pack = fetch_pack(&state).await;
    let recomputed = &pack["recomputed"];
    assert!(
        recomputed["recomputed_at"].as_str().is_some(),
        "recompute is stamped with recomputed_at; recomputed = {recomputed}"
    );
    assert!(
        recomputed["rebuild_certificate"]["verdicts"].is_array(),
        "recompute carries a rebuild certificate with per-feature verdicts; recomputed = {recomputed}"
    );
    assert!(
        recomputed["rebuild_certificate"].get("is_sound").is_some(),
        "recompute carries a re-measured is_sound verdict; recomputed = {recomputed}"
    );
    // The recorded operation must NOT borrow the recomputed verdict.
    assert!(
        pack["operations"][0]["certificate"].is_null(),
        "recorded op certificate must stay null — the recompute is not recorded history"
    );
}

// =====================================================================
// Final-state metrics with provenance
// =====================================================================

#[tokio::test]
async fn part_mass_properties_carry_provenance_labels() {
    let state = make_test_state().await;
    create_box(&state, 10.0, 10.0, 10.0).await;

    let pack = fetch_pack(&state).await;
    let parts = pack["final_state"]["parts"]
        .as_array()
        .expect("final_state.parts is an array");
    assert_eq!(parts.len(), 1, "one box → one part; pack: {pack}");

    let mp = &parts[0]["mass_properties"];
    assert!(
        mp["volume"].as_f64().is_some_and(|v| v > 0.0),
        "a solid box must report a positive volume as a JSON number; mp = {mp}"
    );
    // Provenance labels — the honesty contract on the metric itself.
    assert!(
        mp["provenance"]["volume"]["exactness"].as_str().is_some(),
        "volume must carry a per-quantity exactness provenance label; mp = {mp}"
    );
    assert!(
        mp["provenance"]["inertia"]["exactness"].as_str().is_some(),
        "inertia must carry a per-quantity exactness provenance label; mp = {mp}"
    );
    // Units labels — no consumer has to assume a convention.
    assert_eq!(
        mp["units"]["volume"].as_str(),
        Some("mm^3"),
        "the volume unit label must ride on the wire; mp = {mp}"
    );
}

// =====================================================================
// Notebook (the agent's blackboard, verbatim)
// =====================================================================

#[tokio::test]
async fn notebook_lines_appear_verbatim_with_author_and_timestamps() {
    let state = make_test_state().await;
    // The agent writes a derivation into the document notebook.
    let line = state
        .blackboard
        .add(
            &BlackboardScope::Document,
            None,
            "wall thickness t = P·r / σ_allow".to_string(),
            LineAuthor::Agent,
        )
        .await;

    let pack = fetch_pack(&state).await;
    let notebook = pack["notebook"]
        .as_array()
        .expect("notebook must be an array");
    assert_eq!(
        notebook.len(),
        1,
        "the one written line appears; pack: {pack}"
    );

    let entry = &notebook[0];
    assert_eq!(
        entry["text"].as_str(),
        Some("wall thickness t = P·r / σ_allow"),
        "the line text must appear verbatim; entry = {entry}"
    );
    assert_eq!(entry["author"].as_str(), Some("agent"));
    assert_eq!(entry["id"].as_str(), Some(line.id.as_str()));
    assert!(
        entry["createdAt"].as_u64().is_some() && entry["updatedAt"].as_u64().is_some(),
        "the line must carry create/update timestamps; entry = {entry}"
    );
}

// =====================================================================
// Empty session
// =====================================================================

#[tokio::test]
async fn empty_session_yields_a_valid_empty_pack_not_an_error() {
    let state = make_test_state().await;

    let (status, body) = dispatch(
        &state,
        request(Method::GET, "/api/evidence-pack", None, None),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an empty session must yield a valid pack, not an error; body: {body}"
    );
    assert_eq!(
        body["operations"].as_array().map(Vec::len),
        Some(0),
        "no recorded ops → empty operations; body: {body}"
    );
    assert_eq!(body["manifest"]["operation_count"], json!(0));
    assert_eq!(
        body["final_state"]["parts"].as_array().map(Vec::len),
        Some(0),
        "no geometry → empty parts"
    );
    assert_eq!(
        body["notebook"].as_array().map(Vec::len),
        Some(0),
        "no notebook lines → empty notebook"
    );
    // The manifest still stamps provenance for the bundle itself.
    assert!(body["manifest"]["generated_at"].as_str().is_some());
    assert!(body["manifest"]["kernel_version"].as_str().is_some());
    assert!(
        body["manifest"]["durability"].get("state").is_some(),
        "the durability boot outcome (quarantine surface) is always reported; body: {body}"
    );
}
