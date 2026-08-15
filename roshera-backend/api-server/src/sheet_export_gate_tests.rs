//! The SHEET-EXPORT gate, enforced in Rust.
//!
//! Gate 4's own rationale (`roshera-mcp/src/gates.ts:50-69`) is that an
//! exported PDF/DXF/SVG carries no ambient certificate and can never
//! re-verify itself — which is why, uniquely among the six constraint
//! gates, it fails CLOSED. Until this module's subject landed, that rule
//! lived ONLY in TypeScript. An agent that spoke plain REST could
//! `POST /api/parts/{id}/drawing` then `GET /api/drawings/{id}/pdf` and
//! walk straight around it: `drawing_mgr::export_pdf` / `export_dxf` /
//! `export_svg` fetched the handle, rendered, and returned bytes — never
//! reading `certify_drawing`, which the same module already imported and
//! already served at `GET /api/drawings/{id}/certificate`.
//!
//! These tests pin the server-side rule, mirroring `unsound_base_gate_
//! tests.rs`'s both-directions discipline:
//!   1. a STALE sheet (a live-remeasured dimension has drifted) is refused
//!      on every export route, with NO bypass;
//!   2. `acknowledge_layout_issues: true` does NOT open that branch — the
//!      escape is scoped to layout quality only, never to stale/dangling
//!      facts;
//!   3. a sheet with an Error-severity layout-quality finding is refused
//!      unless `acknowledge_layout_issues=true`, which DOES let it through;
//!   4. a SOUND, quality-passing sheet is never refused, on any of the
//!      three routes;
//!   5. the Rust refusal and the `gates.ts` refusal name the same gates and
//!      the same escape token.
//!
//! ## Fixtures
//!
//! - **Stale:** build the standard one-call sheet for a box (sound,
//!   quality-passing by construction), then reach directly into the
//!   registered `Drawing` and bump a PID-bearing dimension's stored value
//!   by 10 mm — past `CERT_DIM_ORACLE_MM` (0.1 mm). The MODEL is left
//!   untouched; the SHEET now disagrees with a model that never moved,
//!   which is exactly what "stale" means. This is cheaper and more
//!   surgical than mutating the underlying solid, and needs no kernel
//!   surgery at all.
//! - **Quality-failing:** `POST /api/drawings {"name": ...}` registers an
//!   EMPTY drawing (zero views). `verify_drawing` reports `NoViews`
//!   (Error-severity) — so `quality.passed == false` — while `certify_
//!   drawing` reports zero facts, so `sound == true` and no stale/dangling
//!   counts exist. That isolates the quality branch from the stale/
//!   dangling branch cleanly: this fixture must NOT trip `sheet_unsound`.
//! - **Sound:** the same one-call standard sheet, unmutated.

#![cfg(test)]

use crate::durability_boot_tests::{dispatch, get, post};
use crate::router_integration_tests::make_test_state;
use crate::AppState;

use axum::http::StatusCode;
use serde_json::json;
use uuid::Uuid;

// =====================================================================
// Fixtures
// =====================================================================

/// Create a plain 10x10x10 box through the live REST route and return its
/// kernel `SolidId`.
async fn create_box(state: &AppState) -> u32 {
    let (status, body) = dispatch(
        state,
        post(
            "/api/geometry/box",
            json!({ "width": 10.0, "depth": 10.0, "height": 10.0 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "box create must 200; body = {body}");
    body["solid_id"]
        .as_u64()
        .expect("solid_id must be a number") as u32
}

/// Build and register the standard one-call sheet for a fresh box (the
/// "right-click → drawing" path). Pins the fixture precondition: the
/// standard sheet must be SOUND and quality-PASSING before any test relies
/// on that as a baseline — a silently-broken fixture must fail loudly here,
/// not let a gate test pass vacuously.
async fn sound_passing_drawing(state: &AppState) -> Uuid {
    let solid_id = create_box(state).await;
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
    assert_eq!(
        body["quality"]["passed"].as_bool(),
        Some(true),
        "fixture precondition: the standard one-call sheet must pass its \
         own layout-quality check; body = {body}"
    );
    let drawing_id = Uuid::parse_str(body["id"].as_str().expect("drawing id string"))
        .expect("drawing id must parse");

    let (status, cert) = dispatch(
        state,
        get(&format!("/api/drawings/{drawing_id}/certificate")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "certificate GET must 200; body = {cert}"
    );
    assert_eq!(
        cert["sound"].as_bool(),
        Some(true),
        "fixture precondition: the freshly-built sheet must certify SOUND \
         against the model that produced it; cert = {cert}"
    );
    drawing_id
}

/// Reach directly into the registered drawing and bump a PID-bearing
/// dimension's stored value past the dimensioning oracle, without touching
/// the model at all. Returns once it has mutated one dimension; panics if
/// the standard sheet carried no PID-bearing dimension (the fixture's own
/// precondition — the standard box sheet is expected to dimension the
/// box's X/Y/Z extents, each carrying a PID via `extent_dim_pid`).
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
             least one PID-bearing dimension (its X/Y/Z extents)",
        );
    dim.value += 10.0;
}

/// Register an EMPTY drawing (zero views) — `verify_drawing` reports
/// `NoViews` (Error severity), so `quality.passed == false`, while
/// `certify_drawing` reports zero facts, so `sound == true`. Isolates the
/// quality-only branch of the gate.
async fn quality_failing_drawing(state: &AppState) -> Uuid {
    let (status, body) = dispatch(
        state,
        post("/api/drawings", json!({ "name": "Blank Sheet" })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "empty drawing create must 200; body = {body}"
    );
    let drawing_id = Uuid::parse_str(body["id"].as_str().expect("drawing id string"))
        .expect("drawing id must parse");

    let (status, cert) = dispatch(
        state,
        get(&format!("/api/drawings/{drawing_id}/certificate")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "certificate GET must 200; body = {cert}"
    );
    assert_eq!(
        cert["quality"]["passed"].as_bool(),
        Some(false),
        "fixture precondition: an empty drawing must fail its own \
         layout-quality check (NoViews); cert = {cert}"
    );
    assert_eq!(
        cert["sound"].as_bool(),
        Some(true),
        "fixture precondition: an empty drawing carries no facts, so it \
         must certify SOUND — this fixture is meant to isolate the \
         quality-only branch from stale/dangling; cert = {cert}"
    );
    drawing_id
}

/// Assert a response is THE typed sheet-export refusal for `expected_gate`.
fn assert_sheet_refusal(
    status: StatusCode,
    body: &serde_json::Value,
    drawing_id: Uuid,
    expected_gate: &str,
    expected_status: StatusCode,
    route: &str,
) {
    assert_eq!(
        status, expected_status,
        "{route} must refuse with {expected_status}; body = {body}"
    );
    assert_eq!(
        body["success"].as_bool(),
        Some(false),
        "{route} refusal must carry success:false; body = {body}"
    );
    assert_eq!(
        body["details"]["gate"].as_str(),
        Some(expected_gate),
        "{route} refusal must carry the gate name {expected_gate:?}; body = {body}"
    );
    assert_eq!(
        body["details"]["drawing_id"].as_str(),
        Some(drawing_id.to_string().as_str()),
        "{route} refusal must name the offending drawing; body = {body}"
    );
}

fn export_routes(drawing_id: Uuid) -> [(&'static str, String); 3] {
    [
        ("pdf", format!("/api/drawings/{drawing_id}/pdf")),
        ("dxf", format!("/api/drawings/{drawing_id}/dxf")),
        ("svg", format!("/api/drawings/{drawing_id}/svg")),
    ]
}

// =====================================================================
// 1. Stale sheet — refused on every export route, no bypass
// =====================================================================

/// THE GATE, stale branch. A drawing whose live-remeasured facts have
/// drifted past the dimensioning oracle is refused on all three export
/// routes.
///
/// RED before the Rust gate existed: every one of these three GETs
/// returned 200 OK with rendered bytes, regardless of the sheet's live
/// certificate — that is exactly S1 in the audit.
#[tokio::test]
async fn a_stale_sheet_is_refused_on_every_export_route() {
    for kind in ["pdf", "dxf", "svg"] {
        let state = make_test_state().await;
        let drawing_id = sound_passing_drawing(&state).await;
        make_a_dimension_stale(&state, drawing_id).await;

        let (status, body) =
            dispatch(&state, get(&format!("/api/drawings/{drawing_id}/{kind}"))).await;
        assert_sheet_refusal(
            status,
            &body,
            drawing_id,
            "sheet_unsound",
            StatusCode::CONFLICT,
            &format!("{kind} export of a stale sheet"),
        );
        assert!(
            body["details"]["stale"].as_u64().unwrap_or(0) > 0,
            "{kind} refusal must report a nonzero stale count; body = {body}"
        );
    }
}

/// `acknowledge_layout_issues: true` does NOT open the stale/dangling
/// branch — that escape is scoped to layout quality only. The TS gate has
/// no bypass on this arm at all (`gates.ts:672-694`); this is the
/// highest-value both-directions case in the suite because a gate whose
/// escape leaks into the wrong branch is worse than no escape.
#[tokio::test]
async fn acknowledge_layout_issues_does_not_bypass_a_stale_sheet() {
    let state = make_test_state().await;
    let drawing_id = sound_passing_drawing(&state).await;
    make_a_dimension_stale(&state, drawing_id).await;

    let (status, body) = dispatch(
        &state,
        get(&format!(
            "/api/drawings/{drawing_id}/pdf?acknowledge_layout_issues=true"
        )),
    )
    .await;
    assert_sheet_refusal(
        status,
        &body,
        drawing_id,
        "sheet_unsound",
        StatusCode::CONFLICT,
        "pdf export of a stale sheet with acknowledge_layout_issues=true",
    );
}

// =====================================================================
// 2. Layout-quality failure — refused, with a working bypass
// =====================================================================

/// A quality-failing (but SOUND) sheet is refused without the escape.
#[tokio::test]
async fn a_quality_failing_sheet_is_refused_without_acknowledgement() {
    let state = make_test_state().await;
    let drawing_id = quality_failing_drawing(&state).await;

    let (status, body) = dispatch(&state, get(&format!("/api/drawings/{drawing_id}/pdf"))).await;
    assert_sheet_refusal(
        status,
        &body,
        drawing_id,
        "sheet_quality",
        StatusCode::CONFLICT,
        "pdf export of a quality-failing sheet",
    );
}

/// `acknowledge_layout_issues=true` DOES let a quality-failing (but sound)
/// sheet export — the documented draft-for-human-review escape, on every
/// route.
#[tokio::test]
async fn acknowledge_layout_issues_true_lets_the_draft_export_proceed() {
    for kind in ["pdf", "dxf", "svg"] {
        let state = make_test_state().await;
        let drawing_id = quality_failing_drawing(&state).await;

        let (status, body) = dispatch(
            &state,
            get(&format!(
                "/api/drawings/{drawing_id}/{kind}?acknowledge_layout_issues=true"
            )),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{kind} export with acknowledge_layout_issues=true must proceed; body = {body}"
        );
    }
}

/// Non-boolean junk (`?acknowledge_layout_issues=1`) must NOT open the
/// bypass either. `ExportQuery`'s doc comment claims axum's `Query<bool>`
/// deserialization rejects this before the handler ever runs (unlike
/// `acknowledge_unsound`, which arrives as a raw JSON body `Value` and must
/// hand-check `== Some(true)`) — pinned here rather than left as an
/// unverified claim: an escape hatch on a CRITICAL gate that opens on
/// truthy junk is not an escape hatch. Whichever layer refuses (a 400 from
/// query rejection, or a 409 from the gate reading the default `false`),
/// the pinned property is fail-CLOSED on junk — a 200 here would be a real
/// hole.
#[tokio::test]
async fn junk_acknowledge_layout_issues_value_does_not_open_the_bypass() {
    let state = make_test_state().await;
    let drawing_id = quality_failing_drawing(&state).await;

    let (status, body) = dispatch(
        &state,
        get(&format!(
            "/api/drawings/{drawing_id}/pdf?acknowledge_layout_issues=1"
        )),
    )
    .await;
    assert_ne!(
        status,
        StatusCode::OK,
        "junk acknowledge_layout_issues must never open the bypass; body = {body}"
    );
}

/// An explicit `acknowledge_layout_issues=false` must NOT acknowledge —
/// only the literal `true` opens the bypass.
#[tokio::test]
async fn explicit_false_does_not_acknowledge_layout_issues() {
    let state = make_test_state().await;
    let drawing_id = quality_failing_drawing(&state).await;

    let (status, body) = dispatch(
        &state,
        get(&format!(
            "/api/drawings/{drawing_id}/pdf?acknowledge_layout_issues=false"
        )),
    )
    .await;
    assert_sheet_refusal(
        status,
        &body,
        drawing_id,
        "sheet_quality",
        StatusCode::CONFLICT,
        "pdf export with acknowledge_layout_issues=false",
    );
}

// =====================================================================
// 3. A sound, passing sheet is never refused
// =====================================================================

/// No behaviour change on the happy path: a sound sheet that also passes
/// its layout-quality check exports exactly as it did before the gate
/// existed, on all three routes.
#[tokio::test]
async fn a_sound_passing_sheet_is_never_refused() {
    let state = make_test_state().await;
    let drawing_id = sound_passing_drawing(&state).await;

    for (label, route) in export_routes(drawing_id) {
        let (status, body) = dispatch(&state, get(&route)).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{label} export of a sound, passing sheet must 200; body = {body}"
        );
    }
}

// =====================================================================
// 4. The Rust gate and gates.ts agree
// =====================================================================

/// The two gates name the same rules and the same escape. Reads
/// `gates.ts` FROM DISK — the same technique `unsound_base_gate_tests::
/// the_rust_gate_and_gates_ts_name_the_same_rule_and_escape` uses — and
/// asserts token-level agreement.
#[tokio::test]
async fn the_rust_gate_and_gates_ts_name_the_same_gates_and_escape() {
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

    for needle in [
        "gate: \"sheet_uncertified\"",
        "gate: \"sheet_unsound\"",
        "gate: \"sheet_quality\"",
        "acknowledge_layout_issues",
    ] {
        assert!(
            src.contains(needle),
            "gates.ts no longer contains {needle:?} — the client gate changed shape; \
             re-check that the Rust gate in drawing_mgr.rs still agrees with it"
        );
    }

    // Now the Rust side of the same facts, read off a live refusal.
    let state = make_test_state().await;
    let drawing_id = sound_passing_drawing(&state).await;
    make_a_dimension_stale(&state, drawing_id).await;
    let (status, body) = dispatch(&state, get(&format!("/api/drawings/{drawing_id}/pdf"))).await;
    assert_sheet_refusal(
        status,
        &body,
        drawing_id,
        "sheet_unsound",
        StatusCode::CONFLICT,
        "pdf (TS-agreement check)",
    );
    assert!(
        body["retryable"].as_bool() == Some(false),
        "the refusal is not transient — the same drawing_id gets the same \
         answer until it is regenerated; body = {body}"
    );
}
