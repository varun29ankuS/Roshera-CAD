//! The UNSOUND-BASE gate, enforced in Rust.
//!
//! Roshera's honesty thesis is that an agent cannot build on a lie. Until
//! this module's subject landed, the unsound-base rule lived ONLY in
//! `roshera-mcp/src/gates.ts` — TypeScript, in the MCP client. An agent
//! that spoke plain REST instead of MCP could stack a fillet, a shell or a
//! boolean onto a solid the kernel had already certified `sound: false`,
//! and every downstream certificate would inherit the defect. `gates.ts`
//! was a linter, not a gate: the agent could decline to use it.
//!
//! These tests pin the server-side rule:
//!   1. a mutating REST call on an UNSOUND base is refused, typed;
//!   2. `acknowledge_unsound: true` — the documented repair-flow escape —
//!      still proceeds;
//!   3. a SOUND base is untouched;
//!   4. the refusal re-reads LIVE state: repair the base and the very next
//!      call succeeds, with no restart and no cache flush;
//!   5. the Rust refusal and the `gates.ts` refusal tell the same story
//!      (same verdict string, same escape token, same gate name), so an
//!      agent that hits both does not get two accounts of one condition.
//!
//! ## Fixture: why NOT `face/extrude`
//!
//! The obvious unsound base is `POST /api/geometry/face/extrude`, which
//! currently yields a solid with open boundary edges. It is also being
//! actively repaired by other work (`face_extrude_adds_footprint_times_
//! distance_volume` is the suite's one owned red), so a fixture built on it
//! would go green-for-the-wrong-reason the moment that fix lands. Instead
//! these tests reuse `router_integration_tests::seed_box_with_drifted_
//! construction`: a topologically VALID box whose linked construction
//! geometry sits ~1000 units away, so the full certificate's
//! `construction_consistent` dimension reports `inconsistent` and
//! `is_sound()` is false. Two properties make it the right fixture:
//!   - it is independent of every operation under active repair, and
//!   - it is REVERSIBLE through a public seam (`set_solid_construction`,
//!     which invalidates the certificate cache), which is what makes the
//!     live-verdict test (4) possible at all.

#![cfg(test)]

use crate::durability_boot_tests::{dispatch, get, post};
use crate::router_integration_tests::{make_test_state, seed_box_with_drifted_construction};
use crate::AppState;

use axum::http::StatusCode;
use geometry_engine::math::Point3;
use geometry_engine::primitives::provenance::ConstructionGeometry;
use geometry_engine::primitives::solid::SolidId;
use serde_json::json;
use uuid::Uuid;

// =====================================================================
// Helpers
// =====================================================================

/// Seed a plain, SOUND box through the live `/api/geometry/box` route and
/// return its object UUID.
async fn sound_box(state: &AppState) -> Uuid {
    let (status, body) = dispatch(
        state,
        post(
            "/api/geometry/box",
            json!({ "width": 10.0, "depth": 10.0, "height": 10.0 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "box create must 200; body = {body}");
    Uuid::parse_str(body["object"]["id"].as_str().expect("box uuid string"))
        .expect("box uuid must parse")
}

/// The live `sound` flag and `verdict` string the kernel reports for a
/// solid, read through the SAME endpoint `gates.ts::liveVerdict` reads
/// (`GET /api/agent/parts/{id}/perception`, full certificate by default).
/// Reading the verdict here rather than hardcoding it is deliberate: it is
/// what makes "the two gates tell the same story" a mechanical check
/// instead of a prose claim.
async fn live_verdict(state: &AppState, solid_id: SolidId) -> (bool, String) {
    let (status, body) = dispatch(
        state,
        get(&format!("/api/agent/parts/{solid_id}/perception")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "perception GET must 200; body = {body}"
    );
    (
        body["sound"]
            .as_bool()
            .expect("perception must carry sound"),
        body["verdict"]
            .as_str()
            .expect("perception must carry verdict")
            .to_string(),
    )
}

/// Re-link a drifted solid's construction geometry back onto the solid, so
/// the `construction_consistent` dimension reads `consistent` again. This
/// is the REPAIR the gate's escape hatch exists for, performed through
/// `set_solid_construction` — the same public seam the fixture used to
/// break it, and one that invalidates the certificate cache (see its doc
/// in `topology_builder.rs`).
async fn repair_construction(state: &AppState, solid_id: SolidId) {
    let mut model = state.model.write().await;
    let near = Point3::new(0.0, 0.0, 0.0);
    model.set_solid_construction(
        solid_id,
        ConstructionGeometry::new(near, vec![near, Point3::new(1.0, 0.0, 0.0)]),
    );
}

/// Assert a response is THE typed unsound-base refusal, and return its body.
fn assert_unsound_refusal(
    status: StatusCode,
    body: &serde_json::Value,
    solid_id: SolidId,
    route: &str,
) {
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "{route} on an unsound base must refuse with 409 CONFLICT; body = {body}"
    );
    assert_eq!(
        body["error_code"].as_str(),
        Some("unsound_base"),
        "{route} must refuse with the typed unsound_base code; body = {body}"
    );
    assert_eq!(
        body["success"].as_bool(),
        Some(false),
        "{route} refusal must carry success:false; body = {body}"
    );
    // Names the solid — an agent must be able to act on the refusal without
    // guessing which operand was the defective one.
    assert_eq!(
        body["details"]["solid_id"].as_u64(),
        Some(solid_id as u64),
        "{route} refusal must name the offending solid; body = {body}"
    );
    // Names the exact escape.
    let hint = body["hint"].as_str().unwrap_or_default();
    assert!(
        hint.contains("acknowledge_unsound: true"),
        "{route} refusal must name the escape hatch verbatim; hint = {hint:?}"
    );
    // Same gate identity the MCP client uses, so a refusal from either side
    // is recognisable as the same rule.
    assert_eq!(
        body["details"]["gate"].as_str(),
        Some("unsound_base"),
        "{route} refusal must carry the shared gate name; body = {body}"
    );
}

// =====================================================================
// 1. Refused without the acknowledgement
// =====================================================================

/// THE GATE. A mutating REST call whose base solid is UNSOUND by the
/// kernel's live verdict is refused with a typed error naming the solid,
/// the reason, and the escape — WITHOUT `acknowledge_unsound`.
///
/// RED before the Rust gate existed: `/api/geometry/transform` returned
/// 200 OK and cheerfully transformed the defective solid (that is exactly
/// what `router_integration_tests::
/// transform_outlier_reports_unsound_automatically_via_full_cert` pinned).
#[tokio::test]
async fn mutating_an_unsound_base_is_refused_without_acknowledgement() {
    let state = make_test_state().await;
    let (uuid, solid_id) = seed_box_with_drifted_construction(&state, 10.0).await;

    // Pin the fixture against the gate's OWN verdict source before relying
    // on it — a silently-repaired fixture must fail loudly here rather than
    // let the gate test pass vacuously.
    let (sound, _verdict) = live_verdict(&state, solid_id).await;
    assert!(
        !sound,
        "fixture precondition: the drifted-construction box must read sound=false"
    );

    let (status, body) = dispatch(
        &state,
        post(
            "/api/geometry/transform",
            json!({ "object": uuid.to_string(), "translation": [0.0, 0.0, 1.0] }),
        ),
    )
    .await;
    assert_unsound_refusal(status, &body, solid_id, "/api/geometry/transform");

    // The refusal is a REFUSAL, not a silent failure: nothing was recorded
    // and the solid still exists untouched at its original verdict.
    let (still_unsound, _) = live_verdict(&state, solid_id).await;
    assert!(
        !still_unsound,
        "a refused op must not have mutated the base; it is still the same unsound solid"
    );
}

/// The refusal PROSE names the solid and the inheritance argument — the
/// same reason `gates.ts::unsoundBaseGateRefusal` gives. An agent reading
/// only `error` must learn what is wrong and why it matters.
#[tokio::test]
async fn the_refusal_states_the_solid_and_the_inheritance_reason() {
    let state = make_test_state().await;
    let (uuid, solid_id) = seed_box_with_drifted_construction(&state, 10.0).await;

    let (_status, body) = dispatch(
        &state,
        post(
            "/api/geometry/shell",
            json!({ "object": uuid.to_string(), "thickness": 1.0 }),
        ),
    )
    .await;
    let msg = body["error"].as_str().unwrap_or_default();
    assert!(
        msg.contains(&format!("solid {solid_id}")),
        "the refusal must name the solid; error = {msg:?}"
    );
    assert!(
        msg.contains("UNSOUND"),
        "the refusal must state the verdict; error = {msg:?}"
    );
    assert!(
        msg.contains("inherit"),
        "the refusal must give the inheritance reason (why an unsound base \
         matters), matching gates.ts's wording; error = {msg:?}"
    );
    assert!(
        msg.contains("shell"),
        "the refusal must name the operation that was refused; error = {msg:?}"
    );
}

// =====================================================================
// 2. The escape hatch still works
// =====================================================================

/// `acknowledge_unsound: true` is a DELIBERATE, documented bypass — an
/// agent that knowingly proceeds (a boolean used to heal a shell, a rebuild
/// from a known-good state) is behaving correctly. The gate must let it
/// through untouched.
#[tokio::test]
async fn acknowledge_unsound_true_lets_the_deliberate_repair_proceed() {
    let state = make_test_state().await;
    let (uuid, _solid_id) = seed_box_with_drifted_construction(&state, 10.0).await;

    let (status, body) = dispatch(
        &state,
        post(
            "/api/geometry/transform",
            json!({
                "object": uuid.to_string(),
                "translation": [0.0, 0.0, 1.0],
                "acknowledge_unsound": true
            }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an acknowledged unsound base must proceed; body = {body}"
    );
    // And the response STILL tells the truth about what it built — the
    // escape hatch suppresses the refusal, never the verdict.
    assert_eq!(
        body["perception"]["sound"].as_bool(),
        Some(false),
        "acknowledging the defect must not silence the certificate; body = {body}"
    );
}

/// Only the literal boolean `true` acknowledges. A string "true", a 1, or a
/// `false` must NOT open the gate — an escape hatch that opens on truthy
/// junk is not an escape hatch.
#[tokio::test]
async fn only_a_literal_true_acknowledges_the_unsound_base() {
    for junk in [json!("true"), json!(1), json!(false), json!(null)] {
        let label = format!("transform with acknowledge_unsound={junk}");
        let state = make_test_state().await;
        let (uuid, solid_id) = seed_box_with_drifted_construction(&state, 10.0).await;
        let (status, body) = dispatch(
            &state,
            post(
                "/api/geometry/transform",
                json!({
                    "object": uuid.to_string(),
                    "translation": [0.0, 0.0, 1.0],
                    "acknowledge_unsound": junk
                }),
            ),
        )
        .await;
        assert_unsound_refusal(status, &body, solid_id, &label);
    }
}

// =====================================================================
// 3. A sound base is unaffected
// =====================================================================

/// No behaviour change on the happy path: a SOUND base mutates exactly as
/// it did before the gate existed, with no new refusal and no new field
/// required.
#[tokio::test]
async fn a_sound_base_is_never_refused() {
    let state = make_test_state().await;
    let uuid = sound_box(&state).await;

    for (route, payload) in [
        (
            "/api/geometry/transform",
            json!({ "object": uuid.to_string(), "translation": [0.0, 0.0, 1.0] }),
        ),
        (
            "/api/geometry/mirror",
            json!({
                "object": uuid.to_string(),
                "plane_origin": [0.0, 0.0, 0.0],
                "plane_normal": [1.0, 0.0, 0.0]
            }),
        ),
    ] {
        let (status, body) = dispatch(&state, post(route, payload)).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{route} on a sound base must be unaffected by the gate; body = {body}"
        );
        assert_ne!(
            body["error_code"].as_str(),
            Some("unsound_base"),
            "{route} on a sound base must never produce an unsound_base refusal; body = {body}"
        );
    }
}

// =====================================================================
// 4. LIVE verdict — never a memoized one
// =====================================================================

/// ★ The gate re-reads LIVE state on every call. `gates.ts` deliberately
/// never caches an unsound-base refusal (`LIVE_FACT_GATES`) because the
/// base may have been repaired by anyone, at any time. The Rust gate must
/// match: repair the base and the VERY NEXT identical call succeeds — no
/// restart, no cache flush, no second process.
///
/// This also exercises the underlying kernel contract the gate depends on:
/// `set_solid_construction` invalidates the per-solid certificate cache, so
/// `certify_solid` recomputes rather than replaying the stale verdict it
/// just produced for the refusal.
#[tokio::test]
async fn a_repaired_base_stops_being_refused_with_no_restart() {
    let state = make_test_state().await;
    let (uuid, solid_id) = seed_box_with_drifted_construction(&state, 10.0).await;
    let request = || json!({ "object": uuid.to_string(), "translation": [0.0, 0.0, 1.0] });

    // (a) Refused while unsound. This call WARMS the certificate cache —
    // a memoizing gate would keep answering from it forever after.
    let (status, body) = dispatch(&state, post("/api/geometry/transform", request())).await;
    assert_unsound_refusal(status, &body, solid_id, "transform (before repair)");

    // (b) Repair, in-process, through the public kernel seam.
    repair_construction(&state, solid_id).await;
    let (sound_now, _) = live_verdict(&state, solid_id).await;
    assert!(
        sound_now,
        "precondition for the live-verdict claim: the repair must actually \
         restore soundness (if this fails, the fixture repair — not the gate — is wrong)"
    );

    // (c) The IDENTICAL request now proceeds. Same process, same AppState,
    // same router, nothing flushed.
    let (status, body) = dispatch(&state, post("/api/geometry/transform", request())).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the repaired base must stop being refused on the very next call — \
         the gate must read the LIVE certificate, not a memoized verdict; body = {body}"
    );
}

/// The converse direction of the same property: a base that is SOUND when
/// first mutated and goes UNSOUND afterwards starts being refused, again
/// with no restart. Together with the test above this pins the gate to the
/// live verdict in BOTH directions — a gate that only ever loosened (or
/// only ever tightened) would pass one of these and fail the other.
#[tokio::test]
async fn a_base_that_goes_unsound_starts_being_refused() {
    let state = make_test_state().await;
    let uuid = sound_box(&state).await;
    let solid_id = state
        .get_local_id(&uuid)
        .expect("the freshly created box must have an id mapping");
    let request = || json!({ "object": uuid.to_string(), "translation": [0.0, 0.0, 1.0] });

    let (status, body) = dispatch(&state, post("/api/geometry/transform", request())).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a sound base must proceed; body = {body}"
    );

    // Break it: link construction geometry ~1000 units away.
    {
        let mut model = state.model.write().await;
        let far = Point3::new(1000.0, 1000.0, 1000.0);
        model.set_solid_construction(
            solid_id,
            ConstructionGeometry::new(far, vec![far, Point3::new(1001.0, 1000.0, 1000.0)]),
        );
    }

    let (status, body) = dispatch(&state, post("/api/geometry/transform", request())).await;
    assert_unsound_refusal(status, &body, solid_id, "transform (after the base broke)");
}

// =====================================================================
// 5. The Rust gate and the TypeScript gate agree
// =====================================================================

/// ★ The MCP path now hits BOTH gates. The client gate stays (it is the
/// faster, cheaper refusal — it saves a round trip), so the two must agree
/// in shape and reason or an agent gets two different stories about one
/// condition.
///
/// The load-bearing agreement is the VERDICT STRING: `gates.ts::liveVerdict`
/// relays `verdict` verbatim from `GET /api/agent/parts/{id}/perception`
/// into its refusal. This test reads that same endpoint and asserts the
/// Rust refusal carries the identical string — so the two refusals quote
/// the kernel's verdict identically BY CONSTRUCTION rather than by two
/// hand-synced copies of a sentence.
#[tokio::test]
async fn the_rust_refusal_quotes_the_same_verdict_the_ts_gate_relays() {
    let state = make_test_state().await;
    let (uuid, solid_id) = seed_box_with_drifted_construction(&state, 10.0).await;

    // What `gates.ts::liveVerdict` would read for this solid.
    let (sound, ts_verdict) = live_verdict(&state, solid_id).await;
    assert!(!sound, "fixture precondition: the base must read unsound");

    let (_status, body) = dispatch(
        &state,
        post(
            "/api/geometry/transform",
            json!({ "object": uuid.to_string(), "translation": [0.0, 0.0, 1.0] }),
        ),
    )
    .await;
    assert_eq!(
        body["details"]["verdict"].as_str(),
        Some(ts_verdict.as_str()),
        "the Rust refusal must quote the SAME verdict string the TS gate \
         relays from the perception endpoint; body = {body}"
    );
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains(&ts_verdict),
        "the refusal prose must carry that verdict too, exactly as \
         gates.ts::unsoundBaseGateRefusal interpolates it; body = {body}"
    );
}

/// ★ The two gates name the same rule and the same escape. Reads
/// `gates.ts` FROM DISK (the same technique
/// `timeline.rs::regex_copies_agree_across_the_three_packages` uses to keep
/// the checkpoint-name regex honest across packages) and asserts token-level
/// agreement with the live Rust refusal. Token-level, not prose-equality:
/// asserting whole sentences match would fail on the next reword without
/// catching a single real divergence.
#[tokio::test]
async fn the_rust_gate_and_gates_ts_name_the_same_rule_and_escape() {
    let gates_ts =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../roshera-mcp/src/gates.ts");
    let src = std::fs::read_to_string(&gates_ts).unwrap_or_else(|e| {
        panic!(
            "the MCP client gate must be readable at {} (this test exists to keep \
             the two enforcement points in step; if the file moved, re-point it \
             rather than deleting the check): {e}",
            gates_ts.display()
        )
    });

    // The TS gate's identity, escape token, and live-fact (never-cached)
    // policy — the three things the Rust gate is asserted to mirror below.
    for needle in [
        "gate: \"unsound_base\"",
        "acknowledge_unsound: true",
        "acknowledge_unsound !== true",
        "LIVE_FACT_GATES",
    ] {
        assert!(
            src.contains(needle),
            "gates.ts no longer contains {needle:?} — the client gate changed shape; \
             re-check that the Rust gate in main.rs still agrees with it"
        );
    }

    // Coverage (does the TS key SET match the Rust call-site SET, modulo an
    // explicit exemption list?) used to be checked here as five hardcoded
    // substring lookups — pure presence, unable to fail on a dropped Rust
    // route, an added TS key, or an escape-semantics mismatch (item 6, audit
    // S8: "the two sets already differ in both directions, measured"). That
    // coverage question now has its OWN test, derived from both files'
    // source text rather than hardcoded twice:
    // `gate3_drift_set_equality_tests::the_two_base_ref_surfaces_are_equal_
    // modulo_the_exemption_list`. This test keeps the identity/escape needle
    // checks above (a DIFFERENT question — do the two sides agree on the
    // gate's NAME and ESCAPE TOKEN, not on which routes it covers) and the
    // live-refusal verdict-string check below.

    // Now the Rust side of the same three facts, read off a live refusal.
    let state = make_test_state().await;
    let (uuid, solid_id) = seed_box_with_drifted_construction(&state, 10.0).await;
    let (status, body) = dispatch(
        &state,
        post(
            "/api/geometry/transform",
            json!({ "object": uuid.to_string(), "translation": [0.0, 0.0, 1.0] }),
        ),
    )
    .await;
    assert_unsound_refusal(status, &body, solid_id, "transform (TS-agreement check)");
    assert!(
        body["retryable"].as_bool() == Some(false),
        "the refusal is not transient — the same call on the same state gets \
         the same answer, so it must not advertise a blind retry; body = {body}"
    );
}

// =====================================================================
// 6. Coverage: every base-taking geometry route over plain REST
// =====================================================================

/// Every mutating geometry route that stacks work onto an EXISTING solid is
/// gated — not just the one the other tests drive. This is the point of the
/// whole change: the defect was that an agent speaking REST could pick any
/// of these and bypass a gate that only existed in the client.
///
/// Payloads are otherwise-valid (the gate runs after each handler's own
/// parameter validation and base resolution, so a malformed body would fail
/// with 400 here and this test would notice).
#[tokio::test]
async fn every_base_taking_geometry_route_refuses_an_unsound_base() {
    let cases: Vec<(&str, Box<dyn Fn(&Uuid, &Uuid) -> serde_json::Value>)> = vec![
        (
            "/api/geometry/boolean",
            Box::new(
                |bad: &Uuid, good: &Uuid| json!({ "operation": "difference", "object_a": bad.to_string(), "object_b": good.to_string() }),
            ),
        ),
        (
            "/api/geometry/shell",
            Box::new(
                |bad: &Uuid, _g: &Uuid| json!({ "object": bad.to_string(), "thickness": 1.0 }),
            ),
        ),
        (
            "/api/geometry/fillet",
            Box::new(
                |bad: &Uuid, _g: &Uuid| json!({ "object": bad.to_string(), "edges": [0], "radius": 1.0 }),
            ),
        ),
        (
            "/api/geometry/chamfer",
            Box::new(
                |bad: &Uuid, _g: &Uuid| json!({ "object": bad.to_string(), "edges": [0], "distance": 1.0 }),
            ),
        ),
        (
            "/api/geometry/transform",
            Box::new(
                |bad: &Uuid, _g: &Uuid| json!({ "object": bad.to_string(), "translation": [0.0, 0.0, 1.0] }),
            ),
        ),
        (
            "/api/geometry/face/extrude",
            Box::new(
                |bad: &Uuid, _g: &Uuid| json!({ "object_uuid": bad.to_string(), "face_id": 0, "distance": 1.0 }),
            ),
        ),
        (
            "/api/geometry/mirror",
            Box::new(
                |bad: &Uuid, _g: &Uuid| json!({ "object": bad.to_string(), "plane_origin": [0.0, 0.0, 0.0], "plane_normal": [1.0, 0.0, 0.0] }),
            ),
        ),
        (
            "/api/geometry/pattern/linear",
            Box::new(
                |bad: &Uuid, _g: &Uuid| json!({ "object": bad.to_string(), "direction": [1.0, 0.0, 0.0], "spacing": 20.0, "count": 2 }),
            ),
        ),
        (
            "/api/geometry/pattern/circular",
            Box::new(|bad: &Uuid, _g: &Uuid| json!({ "object": bad.to_string(), "count": 3 })),
        ),
    ];

    for (route, build) in cases {
        let state = make_test_state().await;
        let (bad_uuid, bad_solid) = seed_box_with_drifted_construction(&state, 10.0).await;
        let good_uuid = sound_box(&state).await;

        let (status, body) = dispatch(&state, post(route, build(&bad_uuid, &good_uuid))).await;
        assert_unsound_refusal(status, &body, bad_solid, route);
    }
}

/// A boolean gates BOTH operands, exactly as `gates.ts::BASE_REFS` does
/// (`boolean: (a) => [{uuid: a.object_a}, {uuid: a.object_b}]`): an unsound
/// TOOL solid poisons the result precisely as an unsound base does.
#[tokio::test]
async fn boolean_gates_the_tool_operand_too_not_only_the_base() {
    let state = make_test_state().await;
    let good_uuid = sound_box(&state).await;
    let (bad_uuid, bad_solid) = seed_box_with_drifted_construction(&state, 4.0).await;

    let (status, body) = dispatch(
        &state,
        post(
            "/api/geometry/boolean",
            json!({
                "operation": "difference",
                "object_a": good_uuid.to_string(),  // sound base
                "object_b": bad_uuid.to_string(),   // UNSOUND tool
            }),
        ),
    )
    .await;
    assert_unsound_refusal(status, &body, bad_solid, "boolean (unsound tool operand)");
}

/// Read-only routes are NOT gated: perception, previews and measurement of
/// a defective solid are exactly how an agent diagnoses it. Gating them
/// would make the defect undiagnosable — the refusal's own hint points at
/// the perception endpoint.
#[tokio::test]
async fn read_only_routes_are_not_gated() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = seed_box_with_drifted_construction(&state, 10.0).await;

    let (status, body) = dispatch(
        &state,
        get(&format!("/api/agent/parts/{solid_id}/perception")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "diagnosing an unsound solid must never be refused; body = {body}"
    );

    // `GET /api/geometry/{id}` is addressed by the KERNEL solid id, not the
    // public object UUID (the two id spaces are deliberate — see
    // `get_geometry`'s parse).
    let (status, body) = dispatch(&state, get(&format!("/api/geometry/{solid_id}"))).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "reading an unsound solid must never be refused; body = {body}"
    );
}
