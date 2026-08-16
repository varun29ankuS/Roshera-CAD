//! The SOLID-SOUNDNESS gate on the SHEET surface (concern A, 2026-08-15
//! closeout — the largest gap the whole-branch review found, confirmed in
//! the review's own "known list" section).
//!
//! `refuse_unsound_sheet` (`drawing_mgr.rs`) measures **sheet-vs-model**:
//! whether the facts printed on a sheet match the live model
//! (`SheetReadbackCertificate::sound`). It has NO notion of the underlying
//! SOLID's own B-Rep validity — a sheet can be a perfectly faithful drawing
//! of a solid the kernel has already verified is broken, and that gate
//! alone would let it through clean. Before this module's subject landed,
//! there was NO server-side path anywhere — creation or export, any format
//! — that consulted the solid's own soundness before handing out a shop
//! sheet. The literal exploit the review named:
//! `POST /api/parts/{id}/drawing` (sound at creation) → the solid is later
//! independently found unsound → `GET /api/drawings/{id}/pdf` still exports
//! clean, because the sheet is a faithful drawing of a now-broken part.
//!
//! These tests pin the server-side rule, mirroring `export_unsound_gate_
//! tests.rs`'s and `sheet_export_gate_tests.rs`'s both-directions
//! discipline:
//!   1. a verified-UNSOUND solid is refused at CREATION
//!      (`POST /api/parts/{id}/drawing`), typed, `gate: "unsound_base"`;
//!   2. the same at the ONE-CALL svg export
//!      (`GET /api/parts/{id}/drawing.svg`);
//!   3. the same at every REGISTERED export route, for a solid that
//!      verifies unsound AFTER the sheet was already registered SOUND —
//!      the review's literal exploit path;
//!   4. `acknowledge_unsound: true` (a query parameter, matching
//!      `acknowledge_layout_issues`'s own shape) lets each of the three
//!      surfaces above through;
//!   5. the escape is scoped: `acknowledge_unsound` does NOT bypass a
//!      STALE sheet, and `acknowledge_layout_issues` does NOT bypass an
//!      unsound solid — an escape that leaks into the wrong branch is
//!      worse than no escape;
//!   6. a solid that was simply never verified (`SoundnessReading::Stale`)
//!      is NOT refused by this gate — refusing it would claim knowledge
//!      the kernel does not have and would break the ordinary
//!      never-explicitly-verified workflow every other sheet route already
//!      tolerates;
//!   7. junk (`?acknowledge_unsound=1`) does not open the bypass;
//!   8. a verified-SOUND solid is never refused, on any of the three
//!      surfaces — no behaviour change on the happy path.

#![cfg(test)]

use crate::durability_boot_tests::{dispatch, get, post};
use crate::router_integration_tests::{make_test_state, seed_box_with_drifted_construction};
use crate::AppState;

use axum::http::StatusCode;
use serde_json::json;
use uuid::Uuid;

// =====================================================================
// Fixtures
// =====================================================================

/// A plain sound box, never touched — `?fast:true` skipped so the default
/// full cert runs and marks it verified SOUND at creation. Returns
/// `(uuid, solid_id)`.
async fn sound_verified_box(state: &AppState) -> (Uuid, u32) {
    let (status, body) = dispatch(
        state,
        post(
            "/api/geometry/box",
            json!({ "width": 10.0, "depth": 10.0, "height": 10.0 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "box create must 200; body = {body}");
    assert_eq!(
        body["perception"]["sound"].as_bool(),
        Some(true),
        "fixture precondition: box creation must certify SOUND by default; \
         body = {body}"
    );
    let solid_id = body["solid_id"]
        .as_u64()
        .expect("solid_id must be a number") as u32;
    let uuid = Uuid::parse_str(body["object"]["id"].as_str().expect("box uuid string"))
        .expect("box uuid must parse");
    (uuid, solid_id)
}

/// A box that has NEVER been verified — `fast: true` skips the ambient
/// full-cert box creation runs by default, so `soundness_reading` reads
/// `Stale`, not `Unsound`.
async fn never_verified_box(state: &AppState) -> (Uuid, u32) {
    let (status, body) = dispatch(
        state,
        post(
            "/api/geometry/box",
            json!({ "width": 10.0, "depth": 10.0, "height": 10.0, "fast": true }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "box create must 200; body = {body}");
    let solid_id = body["solid_id"]
        .as_u64()
        .expect("solid_id must be a number") as u32;
    let uuid = Uuid::parse_str(body["object"]["id"].as_str().expect("box uuid string"))
        .expect("box uuid must parse");
    (uuid, solid_id)
}

/// A box verified UNSOUND: drifted construction geometry, then a genuine
/// recompute via the default perception path so the live reading becomes
/// `Unsound`, not `Stale` — same technique `export_unsound_gate_tests::
/// unsound_verified_box` uses (duplicated rather than imported: that
/// module's helper is private to it).
async fn unsound_verified_box(state: &AppState) -> (Uuid, u32) {
    let (uuid, solid_id) = seed_box_with_drifted_construction(state, 10.0).await;
    let (status, body) = dispatch(
        state,
        get(&format!("/api/agent/parts/{solid_id}/perception")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "verify (perception) must 200; body = {body}"
    );
    assert_eq!(
        body["status"].as_str(),
        Some("unsound"),
        "fixture precondition: drifted construction must read Unsound \
         (verified, not stale) once verified; body = {body}"
    );
    (uuid, solid_id)
}

/// Register the standard one-call sheet for `solid_id` (sound at the time
/// of creation) and return the drawing id.
async fn register_drawing_for(state: &AppState, solid_id: u32) -> Uuid {
    let (status, body) = dispatch(
        state,
        post(&format!("/api/parts/{solid_id}/drawing"), json!({})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "part drawing create must 200; body = {body}"
    );
    Uuid::parse_str(body["id"].as_str().expect("drawing id string")).expect("drawing id must parse")
}

/// Reach directly into a registered drawing and bump a PID-bearing
/// dimension past the dimensioning oracle, without touching the model —
/// the same technique `sheet_export_gate_tests::make_a_dimension_stale`
/// uses (duplicated: that helper is private to its module).
async fn make_a_dimension_stale(state: &AppState, drawing_id: Uuid) {
    let handle = state
        .drawings
        .get(&drawing_id)
        .expect("drawing must be registered before it can be mutated");
    let mut guard = handle.write().await;
    let dim = guard
        .views
        .iter_mut()
        .flat_map(|v| v.dimensions.iter_mut())
        .find(|d| d.pid.is_some())
        .expect(
            "fixture precondition: the standard box sheet must carry at \
             least one PID-bearing dimension",
        );
    dim.value += 10.0;
}

fn assert_unsound_base_refusal(status: StatusCode, body: &serde_json::Value, solid_id: u32) {
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "must refuse with 409; body = {body}"
    );
    assert_eq!(
        body["success"].as_bool(),
        Some(false),
        "refusal must carry success:false; body = {body}"
    );
    assert_eq!(
        body["error_code"].as_str(),
        Some("unsound_base"),
        "must be the SAME stable error_code the other unsound-base routes \
         use — not a new vocabulary; body = {body}"
    );
    assert_eq!(
        body["details"]["gate"].as_str(),
        Some("unsound_base"),
        "must be the SAME gate name gates.ts reads; body = {body}"
    );
    assert_eq!(
        body["details"]["solid_id"].as_u64(),
        Some(solid_id as u64),
        "refusal must name the offending solid; body = {body}"
    );
}

// =====================================================================
// 1. Creation — POST /api/parts/{id}/drawing
// =====================================================================

/// THE GATE, creation. RED before it existed: this route registered a
/// drawing of a verified-unsound solid every time — the literal exploit
/// the review named.
#[tokio::test]
async fn create_part_drawing_refuses_a_verified_unsound_solid() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = unsound_verified_box(&state).await;

    let (status, body) = dispatch(
        &state,
        post(&format!("/api/parts/{solid_id}/drawing"), json!({})),
    )
    .await;
    assert_unsound_base_refusal(status, &body, solid_id);
}

#[tokio::test]
async fn acknowledge_unsound_true_lets_part_drawing_create_proceed() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = unsound_verified_box(&state).await;

    let (status, body) = dispatch(
        &state,
        post(
            &format!("/api/parts/{solid_id}/drawing?acknowledge_unsound=true"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "acknowledge_unsound:true must let creation proceed; body = {body}"
    );
}

/// A NEVER-VERIFIED solid (`Stale`, not `Unsound`) is NOT refused by this
/// gate — refusing it would claim knowledge the kernel does not have, and
/// would break the ordinary unverified-solid workflow every other sheet
/// route already tolerates.
#[tokio::test]
async fn a_never_verified_solid_is_not_refused_by_the_solid_soundness_gate() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = never_verified_box(&state).await;

    let (status, body) = dispatch(
        &state,
        post(&format!("/api/parts/{solid_id}/drawing"), json!({})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a never-verified (Stale) solid must NOT be refused by the solid- \
         soundness gate; body = {body}"
    );
}

/// Junk (`?acknowledge_unsound=1`) must not open the bypass.
#[tokio::test]
async fn junk_acknowledge_unsound_does_not_open_the_create_bypass() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = unsound_verified_box(&state).await;

    let (status, body) = dispatch(
        &state,
        post(
            &format!("/api/parts/{solid_id}/drawing?acknowledge_unsound=1"),
            json!({}),
        ),
    )
    .await;
    assert_ne!(
        status,
        StatusCode::OK,
        "junk acknowledge_unsound must never open the bypass; body = {body}"
    );
}

// =====================================================================
// 2. One-call export — GET /api/parts/{id}/drawing.svg
// =====================================================================

#[tokio::test]
async fn part_drawing_svg_refuses_a_verified_unsound_solid() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = unsound_verified_box(&state).await;

    let (status, body) = dispatch(&state, get(&format!("/api/parts/{solid_id}/drawing.svg"))).await;
    assert_unsound_base_refusal(status, &body, solid_id);
}

#[tokio::test]
async fn acknowledge_unsound_true_lets_the_one_call_svg_proceed() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = unsound_verified_box(&state).await;

    let (status, body) = dispatch(
        &state,
        get(&format!(
            "/api/parts/{solid_id}/drawing.svg?acknowledge_unsound=true"
        )),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "acknowledge_unsound:true must let the one-call svg proceed; body = {body}"
    );
}

// =====================================================================
// 3. Registered export — GET /api/drawings/{id}/{pdf,dxf,svg}
//    THE review's literal exploit path: sound at registration, found
//    unsound afterward, still exported clean.
// =====================================================================

/// Registers the drawing while ALREADY unsound (via the creation escape,
/// proven separately above) then exports WITHOUT the escape — proving the
/// export gate independently re-reads the solid's LIVE verdict rather than
/// trusting whatever the creation step decided. This is the mechanical
/// core of the review's literal exploit (`POST .../drawing` → later
/// `GET .../pdf`): the two routes must not share a cached "this solid was
/// fine" answer — each re-checks live, exactly like `refuse_unsound_base`'s
/// own "live, never memoized" contract for the 10 mutation routes.
#[tokio::test]
async fn a_registered_export_refuses_a_solid_the_kernel_has_verified_unsound() {
    for kind in ["pdf", "dxf", "svg"] {
        let state = make_test_state().await;
        let (_uuid, solid_id) = unsound_verified_box(&state).await;
        let drawing_id = {
            let (status, body) = dispatch(
                &state,
                post(
                    &format!("/api/parts/{solid_id}/drawing?acknowledge_unsound=true"),
                    json!({}),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "body = {body}");
            Uuid::parse_str(body["id"].as_str().expect("drawing id string"))
                .expect("drawing id must parse")
        };

        let (status, body) =
            dispatch(&state, get(&format!("/api/drawings/{drawing_id}/{kind}"))).await;
        assert_unsound_base_refusal(status, &body, solid_id);
    }
}

#[tokio::test]
async fn acknowledge_unsound_true_lets_the_registered_export_proceed() {
    for kind in ["pdf", "dxf", "svg"] {
        let state = make_test_state().await;
        let (_uuid, solid_id) = unsound_verified_box(&state).await;
        let drawing_id = {
            let (status, body) = dispatch(
                &state,
                post(
                    &format!("/api/parts/{solid_id}/drawing?acknowledge_unsound=true"),
                    json!({}),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "body = {body}");
            Uuid::parse_str(body["id"].as_str().expect("drawing id string"))
                .expect("drawing id must parse")
        };

        let (status, body) = dispatch(
            &state,
            get(&format!(
                "/api/drawings/{drawing_id}/{kind}?acknowledge_unsound=true"
            )),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{kind} export with acknowledge_unsound=true must proceed; body = {body}"
        );
    }
}

// =====================================================================
// 4. Escape scoping — the highest-value both-directions case
// =====================================================================

/// `acknowledge_unsound` does NOT bypass a STALE sheet — that escape is
/// scoped to solid-soundness only.
#[tokio::test]
async fn acknowledge_unsound_does_not_bypass_a_stale_sheet() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = sound_verified_box(&state).await;
    let drawing_id = register_drawing_for(&state, solid_id).await;
    make_a_dimension_stale(&state, drawing_id).await;

    let (status, body) = dispatch(
        &state,
        get(&format!(
            "/api/drawings/{drawing_id}/pdf?acknowledge_unsound=true"
        )),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "acknowledge_unsound must NOT open the sheet-staleness branch; \
         body = {body}"
    );
    assert_eq!(
        body["details"]["gate"].as_str(),
        Some("sheet_unsound"),
        "body = {body}"
    );
}

/// `acknowledge_layout_issues` does NOT bypass an unsound SOLID — the
/// converse of the case above.
#[tokio::test]
async fn acknowledge_layout_issues_does_not_bypass_an_unsound_solid() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = unsound_verified_box(&state).await;

    let (status, body) = dispatch(
        &state,
        post(
            &format!("/api/parts/{solid_id}/drawing?acknowledge_layout_issues=true"),
            json!({}),
        ),
    )
    .await;
    assert_unsound_base_refusal(status, &body, solid_id);
}

// =====================================================================
// 5. A sound, verified solid is never refused — no behaviour change
// =====================================================================

#[tokio::test]
async fn a_verified_sound_solid_is_never_refused_by_the_solid_soundness_gate() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = sound_verified_box(&state).await;
    let drawing_id = register_drawing_for(&state, solid_id).await;

    for kind in ["pdf", "dxf", "svg"] {
        let (status, body) =
            dispatch(&state, get(&format!("/api/drawings/{drawing_id}/{kind}"))).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{kind} export of a verified-sound solid's sheet must 200; body = {body}"
        );
    }

    let (status, body) = dispatch(&state, get(&format!("/api/parts/{solid_id}/drawing.svg"))).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "one-call svg of a verified-sound solid must 200; body = {body}"
    );
}

// =====================================================================
// 6. `record_event` fires only when an escape was actually taken (H2,
//    2026-08-15 closeout wave 2). Before this fix every export —
//    escaped or not — wrote `acknowledge_layout_issues: false,
//    acknowledge_unsound: false` to the timeline unconditionally: a
//    fabricated "an escape was considered and declined" on an ordinary
//    GET, and behaviour change on a route that used to record nothing
//    at all (M4). Mirrors `unsound_base_gate_tests::
//    no_acknowledge_unsound_argument_records_no_facet` for the facet
//    mechanism.
// =====================================================================

/// Every event in `main`'s history whose `command_type` is one of the
/// drawing-export kinds, paired with its recorded `params` — the same
/// `operation.parameters.params` accessor `router_integration_tests.rs`
/// already uses for this envelope shape (`to_timeline_operation`,
/// `recorder_bridge.rs:1068-1092`: `parameters` on the wire is
/// `{"params": <RecordedOperation::parameters>, "inputs": [...],
/// "outputs": [...], "facets"?: {...}}`).
async fn drawing_export_events_in_history(state: &AppState) -> Vec<(String, serde_json::Value)> {
    let (status, body) = dispatch(state, get("/api/timeline/history/main")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "timeline history must 200; body = {body}"
    );
    let events = body.as_array().cloned().unwrap_or_else(|| {
        panic!("expected a bare event array (durability off in test state); got {body}")
    });
    events
        .into_iter()
        .filter_map(|e| {
            let kind = e["operation"]["command_type"].as_str()?.to_string();
            if kind == "drawing.export" || kind == "drawing.svg_export" {
                Some((kind, e["operation"]["parameters"]["params"].clone()))
            } else {
                None
            }
        })
        .collect()
}

/// RED before the fix: an ordinary export of a clean sheet — neither gate's
/// escape ever invoked — wrote a `drawing.export`/`drawing.svg_export`
/// event on every call. It must now write nothing at all.
#[tokio::test]
async fn a_clean_export_with_no_escape_records_nothing() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = sound_verified_box(&state).await;
    let drawing_id = register_drawing_for(&state, solid_id).await;

    for kind in ["pdf", "dxf", "svg"] {
        let (status, body) =
            dispatch(&state, get(&format!("/api/drawings/{drawing_id}/{kind}"))).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{kind} export must 200; body = {body}"
        );
    }
    let (status, body) = dispatch(&state, get(&format!("/api/parts/{solid_id}/drawing.svg"))).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "one-call svg must 200; body = {body}"
    );

    let events = drawing_export_events_in_history(&state).await;
    assert!(
        events.is_empty(),
        "four clean exports (no escape taken on either gate) must leave \
         ZERO drawing-export events on the timeline; found {events:?}"
    );
}

/// A registered export that DID take the `acknowledge_unsound` escape
/// records exactly that flag as `true`, and carries no
/// `acknowledge_layout_issues` key at all — that escape was never taken,
/// and absence (not a stored `false`) is how "not taken" is represented.
#[tokio::test]
async fn a_registered_export_using_only_acknowledge_unsound_records_only_that_flag() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = unsound_verified_box(&state).await;
    let drawing_id = {
        let (status, body) = dispatch(
            &state,
            post(
                &format!("/api/parts/{solid_id}/drawing?acknowledge_unsound=true"),
                json!({}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body = {body}");
        Uuid::parse_str(body["id"].as_str().expect("drawing id string"))
            .expect("drawing id must parse")
    };

    let (status, body) = dispatch(
        &state,
        get(&format!(
            "/api/drawings/{drawing_id}/pdf?acknowledge_unsound=true"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "pdf export must 200; body = {body}");

    let events = drawing_export_events_in_history(&state).await;
    let export_events: Vec<_> = events
        .iter()
        .filter(|(kind, _)| kind == "drawing.export")
        .collect();
    assert_eq!(
        export_events.len(),
        1,
        "exactly one export event — the pdf export that used the escape; \
         found {events:?}"
    );
    let params = &export_events[0].1;
    assert_eq!(
        params["acknowledge_unsound"].as_bool(),
        Some(true),
        "the escape actually taken must be recorded true; params = {params}"
    );
    assert!(
        params.get("acknowledge_layout_issues").is_none(),
        "the escape NOT taken must be absent, never a stored false; \
         params = {params}"
    );
}

/// The one-call svg route (`drawing.svg_export`) mirrors the same rule:
/// only the escape actually taken is recorded, by its own key, and the
/// other stays absent.
#[tokio::test]
async fn the_one_call_svg_export_using_only_acknowledge_unsound_records_only_that_flag() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = unsound_verified_box(&state).await;

    let (status, body) = dispatch(
        &state,
        get(&format!(
            "/api/parts/{solid_id}/drawing.svg?acknowledge_unsound=true"
        )),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "one-call svg must 200; body = {body}"
    );

    let events = drawing_export_events_in_history(&state).await;
    let svg_events: Vec<_> = events
        .iter()
        .filter(|(kind, _)| kind == "drawing.svg_export")
        .collect();
    assert_eq!(
        svg_events.len(),
        1,
        "exactly one svg_export event; found {events:?}"
    );
    let params = &svg_events[0].1;
    assert_eq!(
        params["acknowledge_unsound"].as_bool(),
        Some(true),
        "params = {params}"
    );
    assert!(
        params.get("acknowledge_layout_issues").is_none(),
        "params = {params}"
    );
}

// =====================================================================
// L3 (2026-08-16 residuals) — one escape, two durable vocabularies
// =====================================================================
//
// The ten gate-3 kernel routes record `acknowledge_unsound: true` as the
// `roshera.acknowledge_unsound` FACET (`AckUnsoundFacet`), the vocabulary
// `recorder_bridge.rs:184-189` documents as canonical and
// `unsound_base_gate_tests` pins across every one of those routes. The
// four drawing routes recorded ONLY their own plain JSON parameter (pinned
// above) — a lineage query that only knows the facet-shaped vocabulary
// would silently miss every drawing-route escape. Fixed by stamping the
// facet alongside the parameter (`drawing_mgr.rs`, `ACK_UNSOUND_OVERRIDE.
// sync_scope`, the same mechanism the kernel routes already use, since
// these routes run entirely on the request task).

/// Facet-shaped reading of `roshera.acknowledge_unsound` off the
/// `drawing.export` / `drawing.svg_export` events in durable history — the
/// sibling of `drawing_export_events_in_history` above, which reads the
/// PARAMETER-shaped vocabulary instead.
async fn ack_unsound_facets_in_drawing_history(state: &AppState) -> Vec<serde_json::Value> {
    let (status, body) = dispatch(state, get("/api/timeline/history/main")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "timeline history must 200; body = {body}"
    );
    let events = body.as_array().cloned().unwrap_or_else(|| {
        panic!("expected a bare event array (durability off in test state); got {body}")
    });
    events
        .iter()
        .filter(|e| {
            matches!(
                e["operation"]["command_type"].as_str(),
                Some("drawing.export")
                    | Some("drawing.svg_export")
                    | Some("drawing.create_from_part")
            )
        })
        .filter_map(|e| {
            e["operation"]["parameters"]["facets"]["roshera.acknowledge_unsound"].as_object()
        })
        .map(|obj| serde_json::Value::Object(obj.clone()))
        .collect()
}

/// THE RED for L3 on the CREATION path (`POST /api/parts/{id}/drawing`,
/// `drawing.create_from_part`) — the fifth site the closeout's own line
/// list named. Unlike the four export-shaped routes, this one records its
/// event UNCONDITIONALLY (the quality report always ships), so the correct
/// pin is two-directional: `acknowledge_unsound=true` on a verified-unsound
/// solid must stamp the facet, and `acknowledge_unsound` omitted on a
/// verified-sound solid (the ordinary case, which still records an event)
/// must NOT fabricate one.
#[tokio::test]
async fn creation_using_acknowledge_unsound_also_stamps_the_canonical_facet() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = unsound_verified_box(&state).await;

    let (status, body) = dispatch(
        &state,
        post(
            &format!("/api/parts/{solid_id}/drawing?acknowledge_unsound=true"),
            json!({}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body = {body}");

    let facets = ack_unsound_facets_in_drawing_history(&state).await;
    assert!(
        !facets.is_empty(),
        "no event in history carries the canonical roshera.acknowledge_unsound \
         FACET after a creation that used the escape — only the route's own \
         parameter was stamped before this fix; facets = {facets:?}"
    );
    for facet in &facets {
        assert_eq!(
            facet["acknowledged"],
            json!(true),
            "facet must read `acknowledged: true`; got {facet}"
        );
    }
}

/// The converse of the test above: an ORDINARY creation (sound solid, no
/// escape) still records an event (unconditionally) but must NOT stamp the
/// facet — `ACK_UNSOUND_OVERRIDE.sync_scope` is entered with `q.
/// acknowledge_unsound` itself (here `false`), and `record()` only stamps
/// on `true`.
#[tokio::test]
async fn ordinary_creation_with_no_escape_does_not_stamp_the_acknowledge_unsound_facet() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = sound_verified_box(&state).await;

    let (status, body) = dispatch(
        &state,
        post(&format!("/api/parts/{solid_id}/drawing"), json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body = {body}");

    let facets = ack_unsound_facets_in_drawing_history(&state).await;
    assert!(
        facets.is_empty(),
        "an ordinary creation with no escape must never stamp the \
         acknowledge_unsound facet, even though this route always records \
         an event; facets = {facets:?}"
    );
}

/// THE RED for L3 on the registered-export path: a PDF export that used
/// `acknowledge_unsound=true` must stamp the SAME canonical facet the ten
/// kernel routes stamp, not just its own `acknowledge_unsound` JSON
/// parameter (pinned separately above).
///
/// **Isolation, corrected (L-3 residual, 2026-08-16 ownership residuals):**
/// the original version of this test created its drawing with
/// `?acknowledge_unsound=true` on an UNSOUND solid — but the CREATION
/// route stamps this same facet too (see
/// `creation_using_acknowledge_unsound_also_stamps_the_canonical_facet`
/// above), so `ack_unsound_facets_in_drawing_history` was non-empty
/// BEFORE the PDF export ever ran, and would have stayed non-empty even
/// with `export_pdf`'s own `sync_scope` wrapper deleted — a test that
/// would stay green if the code under it were deleted is not a test.
/// Registers the drawing on a SOUND solid with no escape (creation stamps
/// nothing — pinned by the ordinary-creation test above), so the export
/// call below is the ONLY event in history that can carry the facet;
/// `?acknowledge_unsound=true` on export still enters the gate's bypass
/// and the recording scope regardless of the solid's actual soundness —
/// `refuse_unsound_solid` short-circuits on the flag alone.
#[tokio::test]
async fn a_registered_export_using_acknowledge_unsound_also_stamps_the_canonical_facet() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = sound_verified_box(&state).await;
    let drawing_id = register_drawing_for(&state, solid_id).await;
    assert!(
        ack_unsound_facets_in_drawing_history(&state)
            .await
            .is_empty(),
        "fixture precondition: an ordinary creation with no escape must not \
         have already stamped the facet, or this test cannot attribute the \
         facet below to the export call alone"
    );

    let (status, body) = dispatch(
        &state,
        get(&format!(
            "/api/drawings/{drawing_id}/pdf?acknowledge_unsound=true"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "pdf export must 200; body = {body}");

    let facets = ack_unsound_facets_in_drawing_history(&state).await;
    assert!(
        !facets.is_empty(),
        "no event in history carries the canonical roshera.acknowledge_unsound \
         FACET after a registered export that used the escape — the drawing \
         route's own parameter is not the vocabulary a lineage query for \
         'which operations escaped the unsound gate' actually reads"
    );
    for facet in &facets {
        assert_eq!(
            facet["acknowledged"],
            json!(true),
            "facet must read `acknowledged: true`; got {facet}"
        );
    }
}

/// The `export_svg` sibling of the PDF test above — same isolation
/// discipline (sound solid, no-escape creation, escape used only at
/// export) so the facet in history is attributable to `export_svg`'s own
/// `sync_scope` wrapper alone. THE RED for L-3's `export_svg` gap: before
/// this test, `export_svg` had no facet pin at all, so a future edit that
/// dropped its `sync_scope` wrapper would leave the whole suite green.
#[tokio::test]
async fn a_registered_svg_export_using_acknowledge_unsound_also_stamps_the_canonical_facet() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = sound_verified_box(&state).await;
    let drawing_id = register_drawing_for(&state, solid_id).await;
    assert!(
        ack_unsound_facets_in_drawing_history(&state)
            .await
            .is_empty(),
        "fixture precondition: an ordinary creation with no escape must not \
         have already stamped the facet, or this test cannot attribute the \
         facet below to the export call alone"
    );

    let (status, body) = dispatch(
        &state,
        get(&format!(
            "/api/drawings/{drawing_id}/svg?acknowledge_unsound=true"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "svg export must 200; body = {body}");

    let facets = ack_unsound_facets_in_drawing_history(&state).await;
    assert!(
        !facets.is_empty(),
        "no event in history carries the canonical roshera.acknowledge_unsound \
         FACET after a registered svg export that used the escape — \
         export_svg's own sync_scope wrapper must stamp it, matching \
         export_pdf"
    );
    for facet in &facets {
        assert_eq!(
            facet["acknowledged"],
            json!(true),
            "facet must read `acknowledged: true`; got {facet}"
        );
    }
}

/// The `export_dxf` sibling — same isolation discipline. THE RED for
/// L-3's `export_dxf` gap: before this test, `export_dxf` had no facet
/// pin at all, so a future edit that dropped its `sync_scope` wrapper
/// would leave the whole suite green.
#[tokio::test]
async fn a_registered_dxf_export_using_acknowledge_unsound_also_stamps_the_canonical_facet() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = sound_verified_box(&state).await;
    let drawing_id = register_drawing_for(&state, solid_id).await;
    assert!(
        ack_unsound_facets_in_drawing_history(&state)
            .await
            .is_empty(),
        "fixture precondition: an ordinary creation with no escape must not \
         have already stamped the facet, or this test cannot attribute the \
         facet below to the export call alone"
    );

    let (status, body) = dispatch(
        &state,
        get(&format!(
            "/api/drawings/{drawing_id}/dxf?acknowledge_unsound=true"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "dxf export must 200; body = {body}");

    let facets = ack_unsound_facets_in_drawing_history(&state).await;
    assert!(
        !facets.is_empty(),
        "no event in history carries the canonical roshera.acknowledge_unsound \
         FACET after a registered dxf export that used the escape — \
         export_dxf's own sync_scope wrapper must stamp it, matching \
         export_pdf"
    );
    for facet in &facets {
        assert_eq!(
            facet["acknowledged"],
            json!(true),
            "facet must read `acknowledged: true`; got {facet}"
        );
    }
}

/// THE RED for L3 on the one-call SVG path — same rule, the route with no
/// registered drawing at all.
#[tokio::test]
async fn the_one_call_svg_export_using_acknowledge_unsound_also_stamps_the_canonical_facet() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = unsound_verified_box(&state).await;

    let (status, body) = dispatch(
        &state,
        get(&format!(
            "/api/parts/{solid_id}/drawing.svg?acknowledge_unsound=true"
        )),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "one-call svg must 200; body = {body}"
    );

    let facets = ack_unsound_facets_in_drawing_history(&state).await;
    assert!(
        !facets.is_empty(),
        "no event in history carries the canonical roshera.acknowledge_unsound \
         FACET after a one-call svg export that used the escape; only the \
         route's own parameter was stamped before this fix"
    );
    for facet in &facets {
        assert_eq!(
            facet["acknowledged"],
            json!(true),
            "facet must read `acknowledged: true`; got {facet}"
        );
    }
}

/// A registered export that used ONLY `acknowledge_layout_issues` (never
/// `acknowledge_unsound`) must NOT stamp the `roshera.acknowledge_unsound`
/// facet — the facet is scoped to the ONE escape it names, never a
/// blanket "some escape was used" flag. `ACK_UNSOUND_OVERRIDE.sync_scope`
/// is entered with `q.acknowledge_unsound` specifically, not with whether
/// any event was recorded at all.
#[tokio::test]
async fn acknowledge_layout_issues_alone_does_not_stamp_the_acknowledge_unsound_facet() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = sound_verified_box(&state).await;
    // `?scale=1000` on a 10mm box overflows the fixed A3 sheet by three
    // orders of magnitude, reliably tripping the layout-quality Error
    // branch (`ViewOutsideFrame`) without touching solid soundness — the
    // same trick `sheet_export_gate_tests::quality_failing_drawing` uses.
    // `create_part_drawing_inner` does not itself refuse on quality (only
    // export does), so registration still 200s.
    let drawing_id = {
        let (status, body) = dispatch(
            &state,
            post(
                &format!("/api/parts/{solid_id}/drawing?scale=1000"),
                json!({}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body = {body}");
        Uuid::parse_str(body["id"].as_str().expect("drawing id string"))
            .expect("drawing id must parse")
    };

    let (status, body) = dispatch(
        &state,
        get(&format!(
            "/api/drawings/{drawing_id}/pdf?acknowledge_layout_issues=true"
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body = {body}");

    let facets = ack_unsound_facets_in_drawing_history(&state).await;
    assert!(
        facets.is_empty(),
        "acknowledge_layout_issues alone must never stamp the \
         acknowledge_unsound facet; facets = {facets:?}"
    );
}

// =====================================================================
// L2 (2026-08-16 residuals) — `/semantic` and `/certificate` disclose,
// rather than refuse, an unsound solid's sheet
// =====================================================================
//
// Neither read-only route ever refused on solid soundness (H1 gated only
// the routes that hand out bytes: export + the one-call SVG). The ruling:
// disclose rather than refuse — these two routes must still 200 a
// dimensioned sheet for an unsound solid (a caller diagnosing the broken
// thing needs to SEE it), but the response must now carry a live,
// never-fabricated `solid_soundness` reading rather than staying silent
// about the solid's own B-Rep validity.

/// THE RED for L2 on `/certificate`: a sheet built on a solid the kernel
/// has verified UNSOUND must disclose that reading in `solid_soundness`,
/// not merely stay silent about it while still returning `sound: true`
/// (sheet-vs-model, a different question) at 200.
#[tokio::test]
async fn certificate_discloses_an_unsound_solid_reading_without_refusing() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = unsound_verified_box(&state).await;
    // Registration itself gates on solid soundness (Concern A) — pass the
    // escape to get an unsound solid with a REGISTERED sheet at all, the
    // same trick `a_registered_export_refuses_a_solid_the_kernel_has_
    // verified_unsound` uses. `/certificate` and `/semantic` never gate on
    // this escape (that is the whole point of L2): no escape is passed on
    // the read below.
    let drawing_id = {
        let (status, body) = dispatch(
            &state,
            post(
                &format!("/api/parts/{solid_id}/drawing?acknowledge_unsound=true"),
                json!({}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body = {body}");
        Uuid::parse_str(body["id"].as_str().expect("drawing id string"))
            .expect("drawing id must parse")
    };

    let (status, body) = dispatch(
        &state,
        get(&format!("/api/drawings/{drawing_id}/certificate")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a read-only inspection surface must never refuse — disclose, not \
         refuse; body = {body}"
    );
    assert_eq!(
        body["sound"].as_bool(),
        Some(true),
        "the SHEET-vs-model certificate is a different question from solid \
         validity and must be unaffected by this fix (flatten preserves the \
         existing top-level key); body = {body}"
    );
    let readings = body["solid_soundness"]
        .as_array()
        .expect("solid_soundness must be a JSON array");
    assert!(
        readings
            .iter()
            .any(|r| r["reading"] == "unsound" && r["solid_id"] == solid_id),
        "solid_soundness must disclose the verified-unsound reading for \
         solid {solid_id}, live and never fabricated as sound by omission; \
         body = {body}"
    );
}

/// The `/semantic` sibling of the test above — same disclosure, the fuller
/// response.
#[tokio::test]
async fn semantic_discloses_an_unsound_solid_reading_without_refusing() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = unsound_verified_box(&state).await;
    // See the `/certificate` sibling test above for why the escape is
    // needed at REGISTRATION (Concern A gates creation) but not on the
    // read below (L2's whole point: this route never gates on it).
    let drawing_id = {
        let (status, body) = dispatch(
            &state,
            post(
                &format!("/api/parts/{solid_id}/drawing?acknowledge_unsound=true"),
                json!({}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body = {body}");
        Uuid::parse_str(body["id"].as_str().expect("drawing id string"))
            .expect("drawing id must parse")
    };

    let (status, body) =
        dispatch(&state, get(&format!("/api/drawings/{drawing_id}/semantic"))).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a read-only inspection surface must never refuse — disclose, not \
         refuse; body = {body}"
    );
    let readings = body["solid_soundness"]
        .as_array()
        .expect("solid_soundness must be a JSON array");
    assert!(
        readings
            .iter()
            .any(|r| r["reading"] == "unsound" && r["solid_id"] == solid_id),
        "solid_soundness must disclose the verified-unsound reading for \
         solid {solid_id}; body = {body}"
    );
}

/// The converse: a verified-SOUND solid's sheet discloses `"sound"`, not
/// merely the absence of an unsound reading — a stated positive, not a
/// default.
#[tokio::test]
async fn certificate_discloses_a_sound_solid_reading() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = sound_verified_box(&state).await;
    let drawing_id = register_drawing_for(&state, solid_id).await;

    let (status, body) = dispatch(
        &state,
        get(&format!("/api/drawings/{drawing_id}/certificate")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body = {body}");
    let readings = body["solid_soundness"]
        .as_array()
        .expect("solid_soundness must be a JSON array");
    assert!(
        readings
            .iter()
            .any(|r| r["reading"] == "sound" && r["solid_id"] == solid_id),
        "solid_soundness must disclose the verified-sound reading; \
         body = {body}"
    );
}

/// A NEVER-verified solid (no certificate computed at all — the ordinary
/// state of most solids most of the time) must disclose `"stale"`, never
/// silently read as sound.
#[tokio::test]
async fn certificate_discloses_a_stale_reading_for_a_never_verified_solid() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = never_verified_box(&state).await;
    let drawing_id = register_drawing_for(&state, solid_id).await;

    let (status, body) = dispatch(
        &state,
        get(&format!("/api/drawings/{drawing_id}/certificate")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body = {body}");
    let readings = body["solid_soundness"]
        .as_array()
        .expect("solid_soundness must be a JSON array");
    assert!(
        readings
            .iter()
            .any(|r| r["reading"] == "stale" && r["solid_id"] == solid_id),
        "an unverified solid must disclose `stale`, never silently read as \
         sound by omission; body = {body}"
    );
}

/// THE RED for the `Unresolvable` arm specifically — pins the branch a
/// mutation that replaced `None => SolidSoundnessDisclosure::Unresolvable`
/// with `None => SolidSoundnessDisclosure::Sound` would sail straight
/// through undetected without this test. Reaches directly into the
/// registered drawing (same technique `make_a_dimension_stale` uses) and
/// rewrites a view's `solid_id` to one the active model does not contain —
/// the drawing-vs-model mismatch `drawing_solid_ids`'s own doc names as the
/// honest failure mode, distinct from the aliasing risk (L8b) it also
/// names.
#[tokio::test]
async fn certificate_discloses_unresolvable_for_a_solid_the_model_does_not_contain() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = sound_verified_box(&state).await;
    let drawing_id = register_drawing_for(&state, solid_id).await;

    const ABSENT_SOLID_ID: u32 = 999_999;
    {
        let handle = state
            .drawings
            .get(&drawing_id)
            .expect("drawing must be registered before it can be mutated");
        let mut guard = handle.write().await;
        for view in guard.views.iter_mut() {
            match &mut view.source {
                geometry_engine::drawing::ViewSource::Part { solid_id, .. } => {
                    *solid_id = ABSENT_SOLID_ID;
                }
            }
        }
    }

    let (status, body) = dispatch(
        &state,
        get(&format!("/api/drawings/{drawing_id}/certificate")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a view referencing a solid the active model lacks must still 200 \
         (disclose, not refuse); body = {body}"
    );
    let readings = body["solid_soundness"]
        .as_array()
        .expect("solid_soundness must be a JSON array");
    assert!(
        readings
            .iter()
            .any(|r| r["reading"] == "unresolvable" && r["solid_id"] == ABSENT_SOLID_ID),
        "a solid absent from the active model must disclose `unresolvable`, \
         never silently read as `sound` by omission; body = {body}"
    );
}

// =====================================================================
// 6. L-4 (2026-08-16 ownership residuals) — cardinality and the empty case
// =====================================================================
//
// `drawing_solid_ids` maps over VIEWS, not distinct solids. Before the
// dedup fix, a standard one-call sheet (multiple views, all sourced from
// the SAME solid) produced one identical entry per view in
// `solid_soundness` — and a registered-but-empty drawing produced `[]`,
// a shape a consumer could reasonably misread as "nothing unsound here."
// Both are pinned below.

/// THE RED for the dedup: a standard one-call sheet of ONE solid must
/// disclose exactly ONE `solid_soundness` entry, not one per view. Before
/// the fix this asserted `readings.len() == <the standard auto-drawing's
/// actual view count>` instead — this test fails against that shape,
/// proving the dedup actually happened rather than merely being
/// documented. The fixture-precondition check below reads the real view
/// count off the raw drawing rather than hardcoding it, since the exact
/// number is standard-auto-drawing's own implementation detail.
#[tokio::test]
async fn certificate_discloses_one_entry_per_distinct_solid_not_per_view() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = sound_verified_box(&state).await;
    let drawing_id = register_drawing_for(&state, solid_id).await;

    // Fixture precondition, checked against the RAW drawing (not the
    // disclosure under test): the standard one-call sheet must genuinely
    // register more than one view of the same solid, or this test cannot
    // distinguish "deduped to one" from "never triplicated in the first
    // place."
    let (status, drawing_body) =
        dispatch(&state, get(&format!("/api/drawings/{drawing_id}"))).await;
    assert_eq!(status, StatusCode::OK, "body = {drawing_body}");
    let view_count = drawing_body["views"]
        .as_array()
        .expect("views must be a JSON array")
        .len();
    assert!(
        view_count >= 2,
        "fixture precondition: the standard one-call sheet must register \
         more than one view of the same solid; drawing = {drawing_body:?}"
    );

    let (status, body) = dispatch(
        &state,
        get(&format!("/api/drawings/{drawing_id}/certificate")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body = {body}");
    let readings = body["solid_soundness"]
        .as_array()
        .expect("solid_soundness must be a JSON array");
    assert_eq!(
        readings.len(),
        1,
        "a single-solid drawing must disclose exactly one solid_soundness \
         entry, not one per view referencing it (the fixture has \
         {view_count} views of the same solid); readings = {readings:?}"
    );
    assert_eq!(readings[0]["solid_id"], solid_id);
}

/// THE RED for the empty case: a registered drawing with NO views yet
/// discloses `solid_soundness: []`, and this is the ONLY shape that
/// array can take when nothing has been measured — pinned so a later
/// change cannot quietly turn "no views" into a refusal or a fabricated
/// reading instead of the honest empty disclosure the doc now names.
#[tokio::test]
async fn certificate_discloses_an_empty_array_for_a_drawing_with_no_views() {
    let state = make_test_state().await;
    let (status, body) = dispatch(
        &state,
        post("/api/drawings", json!({ "name": "empty sheet" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body = {body}");
    let drawing_id = body["id"].as_str().expect("drawing id string").to_string();

    let (status, body) = dispatch(
        &state,
        get(&format!("/api/drawings/{drawing_id}/certificate")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a read-only inspection surface must never refuse; body = {body}"
    );
    let readings = body["solid_soundness"]
        .as_array()
        .expect("solid_soundness must be a JSON array");
    assert!(
        readings.is_empty(),
        "a drawing with no views must disclose an empty solid_soundness \
         array, not a fabricated reading; readings = {readings:?}"
    );
}
