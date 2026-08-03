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
        .create_token(
            user_id,
            None,
            vec!["user".to_string()],
            session_manager::PrincipalKind::Human,
        )
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
    // The honesty core, re-pointed after `bda82817`: kernel-recorded ops with
    // exactly one distinct output solid (create_box et al.) are now
    // certified AT RECORD TIME by `attach_record_time_certificate`
    // (`geometry-engine/src/primitives/topology_builder.rs`), so a
    // box-create op legitimately carries a real certificate — that is
    // correct, not a fabrication, and is pinned separately (below and in
    // `sketch_extrude_records_a_per_op_certificate_read_back_verbatim`).
    //
    // The honesty property this test pins — a certificate is never
    // fabricated, and genuine absence carries an explicit reason — still
    // needs a surviving case. `set_name` is the multi/zero-output rule's
    // other side: renaming a solid produces no NEW output solid at all
    // (zero distinct outputs), so `attach_record_time_certificate` returns
    // before computing anything. Every `create_box` call already emits one
    // (the auto-name event), so no extra endpoint is needed.
    let state = make_test_state().await;
    create_box(&state, 10.0, 10.0, 10.0).await;

    let pack = fetch_pack(&state).await;
    let ops = pack["operations"].as_array().expect("operations array");
    let op = ops
        .iter()
        .find(|op| op["op_kind"].as_str() == Some("set_name"))
        .expect("create_box must also record its auto-name set_name event");

    // Mutation guard: an impl that fabricated a certificate for an
    // uncertified op (e.g. a synthetic skipped_solid) would put an object
    // here and drop the reason — this assertion fails in that case.
    assert!(
        op["certificate"].is_null(),
        "a zero-output op (set_name) must report certificate=null, never a fabricated verdict; op = {op}"
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
    // The recorded operation must NOT borrow the recomputed verdict. Since
    // `bda82817`, `operations[0]` (create_box_3d) is itself legitimately
    // certified at record time (single output solid, no labels) — that is
    // correct and not what this assertion is testing. Re-pointed at
    // `set_name` (create_box's auto-name event): a zero-output op that
    // stays honestly uncertified regardless of what `recomputed` reports,
    // proving the recorded side never borrows the recompute's verdict.
    let ops = pack["operations"].as_array().expect("operations array");
    let set_name_op = ops
        .iter()
        .find(|op| op["op_kind"].as_str() == Some("set_name"))
        .expect("create_box must also record its auto-name set_name event");
    assert!(
        set_name_op["certificate"].is_null(),
        "recorded op certificate must stay null — the recompute is not recorded history; op = {set_name_op}"
    );
}

#[tokio::test]
async fn sketch_extrude_records_a_per_op_certificate_read_back_verbatim() {
    // THE PRODUCER, end-to-end on the wire (certified timeline, Move 02):
    // the `sketch_extrude` handler certifies the solid it just produced and
    // attaches the proof to its consolidated `RecordedOperation`; the
    // recorder bridge stores it on the event; the evidence pack reads it
    // back AS RECORDED. RED before the producer was wired: every op —
    // including sketch_extrude — reported `certificate: null`.
    let state = make_test_state().await;

    // Build a closed profile through the real endpoints: 10×10 rectangle.
    let (status, body) = dispatch(&state, request(Method::POST, "/api/csketch", None, None)).await;
    assert_eq!(status, StatusCode::CREATED, "csketch create; body: {body}");
    let sketch_id = body["id"].as_str().expect("csketch id").to_string();

    let (status, body) = dispatch(
        &state,
        request(
            Method::POST,
            &format!("/api/csketch/{sketch_id}/rectangle"),
            None,
            Some(json!({ "x1": 0.0, "y1": 0.0, "x2": 10.0, "y2": 10.0 })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "add rectangle; body: {body}");

    let (status, body) = dispatch(
        &state,
        request(
            Method::POST,
            &format!("/api/csketch/{sketch_id}/extrude"),
            None,
            Some(json!({ "distance": 5.0 })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "extrude; body: {body}");

    // The pack must carry the recorded per-op certificate — read from the
    // event's metadata, not recomputed.
    let pack = fetch_pack(&state).await;
    let ops = pack["operations"].as_array().expect("operations array");
    let extrude_op = ops
        .iter()
        .find(|op| op["op_kind"].as_str() == Some("sketch_extrude"))
        .expect("the sketch_extrude event must be recorded");

    let cert = &extrude_op["certificate"];
    assert!(
        cert.is_object(),
        "a certified sketch_extrude must carry a non-null recorded certificate; op = {extrude_op}"
    );
    assert_eq!(
        cert["is_sound"],
        json!(true),
        "a clean 10x10x5 extrusion must record the kernel's sound verdict; cert = {cert}"
    );
    assert_eq!(cert["skipped"], json!(false));
    for check in [
        "brep_valid",
        "watertight",
        "manifold",
        "oriented",
        "self_intersection_free",
    ] {
        assert!(
            cert["checks"][check].is_boolean(),
            "the per-check breakdown must be recorded (missing {check}); cert = {cert}"
        );
    }
    assert!(
        cert["volume"]
            .as_f64()
            .is_some_and(|v| (v - 500.0).abs() < 1e-6),
        "recorded volume must be the cheap structural fact (10*10*5); cert = {cert}"
    );
    // Per-event-type honesty: a solid op never carries sketch/assembly fields.
    for absent in ["dof", "constrainedness", "conflict", "mates_satisfied"] {
        assert!(
            cert.get(absent).is_none(),
            "a solid op certificate must not carry `{absent}`; cert = {cert}"
        );
    }
    assert!(
        extrude_op.get("certificate_absent_reason").is_none()
            || extrude_op["certificate_absent_reason"].is_null(),
        "a present certificate must not carry an absence reason; op = {extrude_op}"
    );

    // Fabrication guard, re-pointed: since `bda82817`, a kernel-recorded op
    // with exactly one distinct output solid (create_box_3d included) is
    // now legitimately certified at record time — that is correct product
    // behaviour, not a fabrication, and this guard's original premise (box
    // creates always stay null) no longer holds. The genuinely surviving
    // absence is `set_name` (create_box's auto-name event): zero distinct
    // output solids, so it stays honestly uncertified regardless of how
    // many other ops in this same flow are certified.
    create_box(&state, 4.0, 4.0, 4.0).await;
    let pack = fetch_pack(&state).await;
    let set_name_op = pack["operations"]
        .as_array()
        .expect("operations array")
        .iter()
        .find(|op| op["op_kind"].as_str() == Some("set_name"))
        .cloned()
        .expect("the box's auto-name set_name event must be recorded");
    assert!(
        set_name_op["certificate"].is_null(),
        "a zero-output op (set_name) carries no producer and must stay honestly null; op = {set_name_op}"
    );
}

// =====================================================================
// `DELETE /api/agent/parts` clear-sweep provenance (BUG 1: clear_geometry
// destroyed orphaned geometry with zero timeline trace)
// =====================================================================

/// RED before the fix: `clear_all_geometry` swept leftover orphans — entities
/// materialised by an earlier op that never got folded into a solid (the
/// documented scenario: a sketch lifted into edges/curves, then a kernel
/// op that fails and rolls back ITS OWN additions, leaving the
/// pre-materialised sketch entities behind) — via `BRepModel::clear_geometry`,
/// with no call to `record_operation` at all. Replay could never reproduce
/// the wipe, and the lineage graph would carry the swept entities as live
/// forever.
///
/// Deliberately NO real solid in this test (see the discovery note below):
/// this is also the more faithful repro of the documented scenario, which
/// requires no solid to exist at all — a sketch materialised into raw
/// edges/curves BEFORE any solid-producing op runs.
///
/// There is no HTTP path that leaves this residue on purpose (a real one
/// requires an op failing mid-build), so it is manufactured directly
/// against the kernel model — the same fixture shape as
/// `geometry-engine/tests/clear_geometry.rs::add_orphans`.
///
/// # Discovery: a sibling gap, out of THIS fix's scope
///
/// An earlier version of this test also created a real box before adding
/// orphans, to prove `clear_geometry`'s fix doesn't disturb the existing
/// per-solid `delete_solid` recording. That version failed — not because
/// of a bug in this fix, but because `delete_solid`'s cascade
/// (`geometry-engine/src/operations/delete.rs::prune_boolean_orphan_topology`,
/// called unconditionally at the end of every `delete_solid_body`) ALREADY
/// sweeps every orphaned vertex/edge/loop/face/shell **model-wide** —
/// including orphans unrelated to the solid being deleted — and, like
/// `clear_geometry` before this fix, discards what it removed with no
/// record at all (`find_orphaned_entities`'s return value is never
/// threaded into `delete_solid`'s own `deleted` output). So in a flow
/// where a real solid exists and is deleted first, `prune_boolean_orphan_topology`
/// eats the vertex/edge/loop/face orphans BEFORE `clear_geometry` ever
/// runs, leaving only curves (curves have no `EntityType` variant, so
/// neither sweep touches them) for `clear_geometry` to find. That is a
/// second, structurally identical silent-deletion gap — genuinely
/// out of scope for the two defects this change fixes (BUG 1 was
/// scoped to `clear_geometry` specifically), and is reported rather than
/// fixed here.
#[tokio::test]
async fn clear_all_geometry_records_the_orphan_sweep() {
    use geometry_engine::math::Point3;
    use geometry_engine::primitives::curve::{Line, ParameterRange};
    use geometry_engine::primitives::edge::{Edge, EdgeOrientation};

    let state = make_test_state().await;

    let (vertex_ref, edge_ref, curve_ref) = {
        let mut model = state.model.write().await;
        let v0 = model.vertices.add(1.0, 2.0, 3.0);
        let v1 = model.vertices.add(4.0, 5.0, 6.0);
        let cid = model.curves.add(Box::new(Line::new(
            Point3::new(1.0, 2.0, 3.0),
            Point3::new(4.0, 5.0, 6.0),
        )));
        let eid = model.edges.add(Edge::new(
            0,
            v0,
            v1,
            cid,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        ));
        (
            format!("vertex:{v0}"),
            format!("edge:{eid}"),
            format!("curve:{cid}"),
        )
    };

    let (status, body) = dispatch(
        &state,
        request(Method::DELETE, "/api/agent/parts", None, None),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "clear-all must succeed; body: {body}"
    );

    let pack = fetch_pack(&state).await;
    let ops = pack["operations"].as_array().expect("operations array");
    let clear_op = ops
        .iter()
        .find(|op| op["op_kind"].as_str() == Some("clear_geometry"))
        .expect(
            "clear_geometry must now be recorded — before this fix, an orphan \
             sweep destroyed geometry with zero trace on the timeline",
        );

    let deleted: Vec<&str> = clear_op["params"]["deleted"]
        .as_array()
        .expect("clear_geometry event must carry a non-empty `deleted` channel")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    for want in [vertex_ref.as_str(), edge_ref.as_str(), curve_ref.as_str()] {
        assert!(
            deleted.contains(&want),
            "clear_geometry must declare the orphan it actually swept ({want}); deleted = {deleted:?}"
        );
    }

    // The ratchet's real question, not just its scanner: an operation that
    // consumed model entities must name them as inputs too, not rely on an
    // allowlisted "constructive root" exemption — a clear genuinely
    // consumes the orphans it destroys. Mirrors `delete_solid`'s recorded
    // shape (`with_input_solids` + `with_deleted_refs` on the same id).
    let inputs: Vec<&str> = clear_op["params"]["inputs"]
        .as_array()
        .expect("clear_geometry event must carry an `inputs` channel")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    for want in [vertex_ref.as_str(), edge_ref.as_str(), curve_ref.as_str()] {
        assert!(
            inputs.contains(&want),
            "clear_geometry must also declare the swept orphan as an input \
             it consumed ({want}); inputs = {inputs:?}"
        );
    }

    assert!(
        clear_op["certificate"].is_null(),
        "a clear has no single output solid to certify; must stay honestly null"
    );
}

/// The empty-sweep case: with no orphans (and no solids at all), the sweep
/// finds nothing, so nothing is recorded — a `clear_geometry` event with
/// empty `inputs`/`outputs`/`deleted` would assert a clear with zero
/// lineage content, which is noise, not history.
#[tokio::test]
async fn clear_all_geometry_skips_the_record_when_nothing_to_sweep() {
    let state = make_test_state().await;

    let (status, body) = dispatch(
        &state,
        request(Method::DELETE, "/api/agent/parts", None, None),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "clear-all must succeed; body: {body}"
    );

    let pack = fetch_pack(&state).await;
    let ops = pack["operations"].as_array().expect("operations array");
    assert!(
        ops.iter()
            .find(|op| op["op_kind"].as_str() == Some("clear_geometry"))
            .is_none(),
        "an empty sweep must record nothing; ops = {ops:?}"
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
    let document_id = state.active_document.read().await.clone();
    let line = state
        .blackboard
        .add(
            &document_id,
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
