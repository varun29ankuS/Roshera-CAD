//! The EXPORT unsound-solid gate, enforced in Rust (item 8, audit S5,
//! 2026-08-15).
//!
//! `POST /api/export` already refused a solid that had NEVER been verified
//! (`SoundnessReading::Stale`) — the pre-existing P1 staleness check
//! (`export_refuses_a_never_verified_solid`, `router_integration_tests.rs`).
//! This module closes a DIFFERENT gap the audit found (S5): a solid that WAS
//! explicitly verified and found UNSOUND (`SoundnessReading::Unsound`, not
//! `Stale`) read `is_stale() == false` and exported clean — no certificate
//! consulted, no refusal anywhere. Gate 4's own rationale — "a PDF/DXF on
//! disk carries NO ambient certificate, so unlike a kernel op there is no
//! downstream truth-teller after this point" — applies verbatim to the
//! STL/OBJ/STEP file that actually reaches a machine, and this is the same
//! class of hole `4b1ef771` closed for drawing exports.
//!
//! These tests pin the server-side rule, mirroring `unsound_base_gate_
//! tests.rs` and `sheet_export_gate_tests.rs`'s both-directions discipline:
//!   1. a verified-UNSOUND solid is refused on export, typed — `gate:
//!      "unsound_base"`, 409, the SAME gate name and status the other 10
//!      REST routes already use (this is an unsound-BASE question, so it
//!      reuses that vocabulary rather than inventing a new one);
//!   2. `acknowledge_unsound: true` — the documented repair-flow escape —
//!      lets it through;
//!   3. that escape does NOT bypass the SEPARATE stale (never-verified)
//!      branch, which has no escape of its own;
//!   4. a verified-SOUND solid is never refused;
//!   5. `gates.ts::BASE_REFS` covers `export_part` (the client half of item
//!      8), read from disk.
//!
//! ## Fixtures
//!
//! - **Never-verified (stale):** `POST /api/geometry/box` with `fast: true`
//!   — `body_verify_flag` skips the ambient full-cert that route runs by
//!   default, so the solid's `verified_certificate` stays `None` exactly as
//!   a fresh solid's does.
//! - **Verified-sound:** the SAME route WITHOUT `fast: true` — box creation
//!   runs `certify_solid` by default ("feedback-as-default: a primitive is
//!   sound by construction, but report the SOUND verdict anyway"), which
//!   marks the solid verified in the same call. No separate perception call
//!   needed.
//! - **Verified-unsound:** `seed_box_with_drifted_construction`
//!   (`router_integration_tests.rs`) sets construction geometry ~1000 units
//!   away, but that seam ALSO invalidates verification — the solid reads
//!   `Stale`, not `Unsound`, immediately afterward. `GET /api/agent/parts/
//!   {id}/perception` (the default, full path — what `verify_part` calls)
//!   forces a genuine recompute: the construction mismatch makes the fresh
//!   certificate unsound, and the recompute itself marks the solid
//!   verified — `soundness_reading` now reads `Unsound`, the exact state
//!   this gate exists to catch.

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

/// A box that has NEVER been verified (`fast: true` skips the ambient
/// full-cert box creation runs by default).
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
    let uuid = Uuid::parse_str(
        body["object"]["id"]
            .as_str()
            .expect("object.id must be a string"),
    )
    .expect("object id must parse as a uuid");
    (uuid, solid_id)
}

/// A box verified SOUND at creation (no `fast: true` — the default full
/// cert runs and marks the solid verified in the same call).
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
        "fixture precondition: box creation must certify SOUND by default \
         (no `fast:true`); body = {body}"
    );
    let solid_id = body["solid_id"]
        .as_u64()
        .expect("solid_id must be a number") as u32;
    let uuid = Uuid::parse_str(
        body["object"]["id"]
            .as_str()
            .expect("object.id must be a string"),
    )
    .expect("object id must parse as a uuid");
    (uuid, solid_id)
}

/// A box verified UNSOUND: drifted construction geometry, then a genuine
/// recompute via the default perception path so the live reading becomes
/// `Unsound`, not `Stale` — see module doc.
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
        body["sound"].as_bool(),
        Some(false),
        "fixture precondition: drifted construction must certify UNSOUND \
         once verified; body = {body}"
    );
    assert_eq!(
        body["status"].as_str(),
        Some("unsound"),
        "fixture precondition: the freshness gate must read Unsound \
         (verified, not stale) after this recompute — otherwise this \
         fixture exercises the wrong branch; body = {body}"
    );
    (uuid, solid_id)
}

// =====================================================================
// 1. A verified-unsound solid is refused, typed
// =====================================================================

/// THE GATE. RED before it existed: this export returned 200 OK with a
/// download_url, regardless of the solid's live verdict — exactly S5 in the
/// audit.
#[tokio::test]
async fn a_verified_unsound_solid_is_refused_on_export() {
    let state = make_test_state().await;
    let (uuid, solid_id) = unsound_verified_box(&state).await;

    let (status, body) = dispatch(
        &state,
        post(
            "/api/export",
            json!({ "format": "STL", "objects": [uuid.to_string()] }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "export of a verified-unsound solid must 409; body = {body}"
    );
    assert_eq!(
        body["success"].as_bool(),
        Some(false),
        "refusal must carry success:false; body = {body}"
    );
    assert_eq!(
        body["error_code"].as_str(),
        Some("unsound_base"),
        "must be the SAME stable error_code the other 10 unsound-base \
         routes use — not a new vocabulary; body = {body}"
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
// 2. acknowledge_unsound lets it through
// =====================================================================

#[tokio::test]
async fn acknowledge_unsound_true_lets_the_export_proceed() {
    let state = make_test_state().await;
    let (uuid, _solid_id) = unsound_verified_box(&state).await;

    let (status, body) = dispatch(
        &state,
        post(
            "/api/export",
            json!({
                "format": "STL",
                "objects": [uuid.to_string()],
                "acknowledge_unsound": true,
            }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "acknowledge_unsound:true must let the export proceed; body = {body}"
    );
    assert_eq!(body["success"].as_bool(), Some(true), "body = {body}");
}

// =====================================================================
// 3. The escape is scoped — it does NOT bypass the separate stale branch
// =====================================================================

/// The highest-value both-directions case: an escape hatch that leaks into
/// the wrong branch is worse than no escape (mirrors the sheet-export
/// suite's `acknowledge_layout_issues_does_not_bypass_a_stale_sheet`).
#[tokio::test]
async fn acknowledge_unsound_does_not_bypass_a_never_verified_solid() {
    let state = make_test_state().await;
    let (uuid, solid_id) = never_verified_box(&state).await;

    let (status, body) = dispatch(
        &state,
        post(
            "/api/export",
            json!({
                "format": "STL",
                "objects": [uuid.to_string()],
                "acknowledge_unsound": true,
            }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "acknowledge_unsound must NOT open the stale (never-verified) \
         branch — that escape is scoped to the unsound branch only; \
         solid {solid_id}; status = {status}"
    );
}

// =====================================================================
// 4. A verified-sound solid is never refused
// =====================================================================

/// No behaviour change on the happy path.
#[tokio::test]
async fn a_verified_sound_solid_is_never_refused() {
    let state = make_test_state().await;
    let (uuid, _solid_id) = sound_verified_box(&state).await;

    let (status, body) = dispatch(
        &state,
        post(
            "/api/export",
            json!({ "format": "STL", "objects": [uuid.to_string()] }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "export of a verified-sound solid must 200; body = {body}"
    );
}

// =====================================================================
// 5. gates.ts covers export_part (the client half of item 8)
// =====================================================================

#[tokio::test]
async fn gates_ts_base_refs_covers_export_part() {
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
    assert!(
        src.contains("export_part:"),
        "gates.ts::BASE_REFS no longer covers export_part — item 8's client \
         half regressed"
    );

    let state = make_test_state().await;
    let (uuid, _solid_id) = unsound_verified_box(&state).await;
    let (status, body) = dispatch(
        &state,
        post(
            "/api/export",
            json!({ "format": "STL", "objects": [uuid.to_string()] }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "body = {body}");
    assert_eq!(body["details"]["gate"].as_str(), Some("unsound_base"));
}
