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
