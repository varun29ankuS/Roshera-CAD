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
//!      on every REGISTERED export route, with NO bypass;
//!   2. `acknowledge_layout_issues: true` does NOT open that branch — the
//!      escape is scoped to layout quality only, never to stale/dangling
//!      facts;
//!   3. a sheet with an Error-severity layout-quality finding is refused
//!      unless `acknowledge_layout_issues=true`, which DOES let it through;
//!   4. a SOUND, quality-passing sheet is never refused, on any of the
//!      three registered-export routes;
//!   5. the Rust refusal and the `gates.ts` refusal name the same gates and
//!      the same escape token.
//!
//! ## Five routes hand out sheet bytes, not three (H1, 2026-08-15 review)
//!
//! `export_pdf` / `export_dxf` / `export_svg` (gated by `4b1ef771`, proven in
//! section 1-4 below) all address a REGISTERED drawing by `drawing_id`. Two
//! more routes produce the identical third-angle HLR sheet from a live SOLID
//! directly, registering nothing: `GET /api/parts/{id}/drawing.svg` and
//! `GET /api/parts/uuid/{uuid}/drawing.svg` (`drawing_mgr::part_drawing_svg`
//! / `part_drawing_svg_by_uuid`). Section 5 below gates and proves those two.
//! They cannot share `registered_export_routes()`'s helper (there is no
//! `drawing_id` to key on — the sheet is built AND rendered within the one
//! request) and the STALE branch cannot structurally apply to them (nothing
//! is left registered between projection and export for the model to move
//! out from under) — which is exactly why the original name of test 1 below
//! overclaimed "every export route" while its `export_routes()` helper
//! enumerated exactly three: a reader learned a completeness the route table
//! (five routes) contradicted. Renamed to say what it actually proves;
//! coverage of the two one-call routes is added in section 5, for the checks
//! that genuinely apply to them (quality-Error, sound-passthrough).
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
    let (id, _uuid) = create_box_full(state).await;
    id
}

/// Same fixture as `create_box`, but also returns the object UUID — the
/// one-call `drawing.svg` uuid route (`part_drawing_svg_by_uuid`) addresses
/// solids by UUID, not by kernel `SolidId`.
async fn create_box_full(state: &AppState) -> (u32, Uuid) {
    let (status, body) = dispatch(
        state,
        post(
            "/api/geometry/box",
            json!({ "width": 10.0, "depth": 10.0, "height": 10.0 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "box create must 200; body = {body}");
    let id = body["solid_id"]
        .as_u64()
        .expect("solid_id must be a number") as u32;
    let uuid = Uuid::parse_str(body["object"]["id"].as_str().expect("box uuid string"))
        .expect("box uuid must parse");
    (id, uuid)
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

/// The three routes that export a REGISTERED drawing by `drawing_id`. Named
/// for what it covers (see the module doc's "Five routes" note) — the two
/// one-call `drawing.svg` routes (section 5) address a live solid directly
/// and have no `drawing_id` to key this helper on.
fn registered_export_routes(drawing_id: Uuid) -> [(&'static str, String); 3] {
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
/// drifted past the dimensioning oracle is refused on all three REGISTERED
/// export routes.
///
/// Named for exactly what it covers (H1, 2026-08-15 review): staleness is a
/// property of a REGISTERED drawing whose sheet was projected in an EARLIER
/// request than the one exporting it — the one-call `drawing.svg` routes
/// build and render within the SAME request, so there is no gap for the
/// model to have moved in and this scenario cannot be constructed against
/// them. The former name (`..._on_every_export_route`) claimed a
/// completeness this test never covered even before the one-call routes
/// existed to expose it: `export_routes()` (now `registered_export_routes`)
/// always enumerated exactly three of what were already five sheet-emitting
/// routes.
///
/// RED before the Rust gate existed: every one of these three GETs
/// returned 200 OK with rendered bytes, regardless of the sheet's live
/// certificate — that is exactly S1 in the audit.
#[tokio::test]
async fn a_stale_sheet_is_refused_on_every_registered_export_route() {
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
/// existed, on all three registered-export routes.
#[tokio::test]
async fn a_sound_passing_sheet_is_never_refused() {
    let state = make_test_state().await;
    let drawing_id = sound_passing_drawing(&state).await;

    for (label, route) in registered_export_routes(drawing_id) {
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

// =====================================================================
// 5. F2 (2026-08-15 review H1) — the two ONE-CALL `drawing.svg` routes
// =====================================================================
//
// `GET /api/parts/{id}/drawing.svg` and `GET /api/parts/uuid/{uuid}/
// drawing.svg` hand out the identical third-angle HLR sheet as the three
// routes above, addressing a live solid directly rather than a registered
// drawing. `refuse_unsound_sheet` is called in the same shape `4b1ef771`
// established (`drawing_svg_for_solid`, drawing_mgr.rs); no `drawing_id`
// exists for this path, so the refusal names the SOLID instead
// (`details.solid_id`, no `drawing_id` key at all) — proven below, not
// merely asserted. Before M5 (2026-08-16 residuals) this route threaded
// `Uuid::nil()` through the same shared constructor the registered routes
// use, producing a refusal naming a drawing that does not exist and a hint
// prescribing a remedy ("export the new drawing_id") this route cannot
// follow; `ApiError::sheet_quality_for_solid` / `sheet_unsound_for_solid`
// close that. The stale/dangling branch is not exercised here: it cannot
// be constructed against a sheet that is built and rendered within one
// request (see the rename note on section 1 above) — only the
// layout-quality branch and the sound-passthrough path genuinely apply.
//
// L-2 (2026-08-16 ownership residuals): the third constructor of this
// sibling set, `ApiError::sheet_uncertified_for_solid`, is likewise not
// exercised through this router — for a different reason than
// stale/dangling. Its branch fires only when `certify_off_lock`'s
// `spawn_blocking` join fails (the certification task itself panicked or
// was cancelled), which is not constructible against any well-formed
// request; there is no query parameter or fixture trick that reaches it,
// the same way none reaches the analogous branch on the three registered
// routes. It is pinned as a unit test instead
// (`error_catalog.rs::tests::sheet_uncertified_for_solid_names_the_solid_
// not_a_nil_drawing`), matching its two siblings' unit pins.
//
// Forcing a quality-Error finding needs a different trick than
// `make_a_dimension_stale` (there is no registered `Drawing` to reach into
// before the request runs). It used to be `?scale=1000` on a 10mm box, which
// overflowed the fixed A3 sheet by three orders of magnitude and reliably
// tripped `ViewOutsideFrame` (Error-severity, geometry-engine/src/drawing/
// verify.rs:47,253-258).
//
// Task 40 closed that trick at the source: the explicit-scale route now
// computes its placement from the SCALED extents and REFUSES a scale no
// arrangement fits, so it cannot produce an off-frame sheet for the quality
// gate to catch.
//
// ONE trick still works, and it was deliberately NOT taken: a bored plate at
// `?scale=<= 0.01` yields `RedundantDimension` (Error-severity). It stops
// working above 0.05, which is the tell — the check quantizes SHEET-space
// positions, so at a small enough scale two GENUINELY DISTINCT dimensions
// collapse onto the same quantized interval and are reported as one repeated
// callout. That is a verifier false positive, not a drawing defect. Building
// this suite's fixture on it would pin the false positive in place and make
// fixing it look like a regression.
//
// No other lever is known: the remaining Error kinds (dimension-on-geometry,
// label and GD&T collisions) have no HTTP handle, and the two sheets this
// route can build (`standard_drawing_auto`, and `standard_drawing_hlr` at a
// scale that fits) are laid out clean by construction. The gate itself is
// unchanged and still live; what is gone is the HTTP RED for it on THIS route.
// Recorded rather than papered over.

/// The one-call-route sibling of [`assert_sheet_refusal`] (M5, 2026-08-16
/// residuals). This route registers no [`Drawing`](geometry_engine::drawing::Drawing),
/// so its refusal must name the SOLID (`details.solid_id`) rather than a
/// `drawing_id` — and, per this project's absence discipline, a field with
/// no value must be OMITTED, never defaulted to a nil UUID. Both halves are
/// checked: the solid is named, and `drawing_id` is genuinely absent from
/// `details`, not merely unequal to the caller's expectation.
// Reason: retained without a caller. Its two callers asserted the one-call
// `sheet_quality` refusal, which Task 40 made unforcible over HTTP (see the
// note above). The shape it encodes is still the contract every one-call gate
// refusal must satisfy, `error_catalog.rs`'s
// `sheet_uncertified_for_solid_names_the_solid_not_a_nil_drawing` cites the
// note beside it by name, and deleting both would erase the reachability
// analysis that explains why the pin is missing. Re-wired the moment a
// one-call gate refusal becomes reachable again.
#[allow(dead_code)]
fn assert_one_call_sheet_refusal(
    status: StatusCode,
    body: &serde_json::Value,
    solid_id: u32,
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
        body["details"]["solid_id"].as_u64(),
        Some(solid_id as u64),
        "{route} refusal must name the offending SOLID (this route has no \
         registered drawing); body = {body}"
    );
    assert!(
        body["details"].get("drawing_id").is_none(),
        "{route} refusal must OMIT drawing_id — this route never registers \
         one, and a nil UUID would name a drawing that does not exist; \
         body = {body}"
    );
}

/// An unfittable `?scale=` is refused by the KERNEL, before the export gate
/// ever sees a sheet — and the refusal is not the layout-quality one.
///
/// This test used to assert `gate: "sheet_quality"` at 409: `?scale=1000` on a
/// 10 mm box overflowed the fixed A3 sheet by three orders of magnitude, the
/// route built the off-frame sheet anyway, and the export gate caught it.
/// Task 40 removed that possibility at the source — `standard_drawing_hlr` now
/// computes its placement from the SCALED extents and returns
/// `ProjectionError::ScaleDoesNotFitSheet` rather than emitting views that hang
/// off the frame — so there is no longer a sheet for the quality gate to
/// judge. The bytes are still refused; they are refused EARLIER and for a
/// truer reason.
///
/// The refusal is `400 invalid_parameter`, NON-retryable, and carries the
/// kernel's own sentence: the kernel did not fault, the caller asked for a
/// scale that does not fit, and it will not fit on any retry. `details.gate` is
/// absent — no export gate ran, because there was no sheet to grade.
///
/// The four measurements behind the sentence are asserted as TYPED fields in
/// `details`, not merely as substrings of the prose. A client that has to regex
/// a human-readable message to recover the overflow has been handed prose where
/// it asked for a measurement — and this is the route whose primary caller is
/// an agent.
#[tokio::test]
async fn an_unfittable_scale_is_refused_before_the_one_call_svg_quality_gate() {
    for label in ["id", "uuid"] {
        let state = make_test_state().await;
        let (id, uuid) = create_box_full(&state).await;
        let path = if label == "id" {
            format!("/api/parts/{id}/drawing.svg?scale=1000")
        } else {
            format!("/api/parts/uuid/{uuid}/drawing.svg?scale=1000")
        };
        let (status, body) = dispatch(&state, get(&path)).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "one-call svg ({label}): an unfittable scale is a CALLER error refused by the kernel, not a sheet to draw; body = {body}"
        );
        assert_eq!(
            body["success"].as_bool(),
            Some(false),
            "one-call svg ({label}) refusal must carry success:false; body = {body}"
        );
        assert_eq!(
            body["retryable"].as_bool(),
            Some(false),
            "one-call svg ({label}): an unfittable scale fails identically on every retry; body = {body}"
        );
        assert_ne!(
            body["details"]["gate"].as_str(),
            Some("sheet_quality"),
            "one-call svg ({label}) must refuse at the kernel, not by grading an off-frame sheet the kernel should never have produced; body = {body}"
        );
        let message = body["error"].as_str().unwrap_or_default();
        assert!(
            message.contains("1000") && message.contains("A3") && message.contains("mm"),
            "one-call svg ({label}): the kernel's refusal must reach the client naming the scale, the sheet and the overflow; body = {body}"
        );
        assert!(
            !message.contains("  "),
            "one-call svg ({label}): refusal message carries absorbed indentation; body = {body}"
        );

        // The measurements, as typed fields — not as prose an agent must parse.
        let d = &body["details"];
        assert_eq!(
            d["parameter"].as_str(),
            Some("scale"),
            "one-call svg ({label}): the refusal must name WHICH parameter it refuses; body = {body}"
        );
        assert_eq!(
            d["scale"].as_f64(),
            Some(1000.0),
            "one-call svg ({label}): details.scale; body = {body}"
        );
        assert_eq!(
            d["sheet"].as_str(),
            Some("A3"),
            "one-call svg ({label}): details.sheet; body = {body}"
        );
        // A 10 mm box on A3: usable area 368 x 211 mm, fit height 211 - 8 = 203.
        // Unit spans are 10 in every column/row, so at 1000:1 the group is
        // 1000*20 + 30 = 20030 wide and 1000*20 + 32 = 20032 tall.
        assert_eq!(
            d["overflow_x_mm"].as_f64(),
            Some(19662.0),
            "one-call svg ({label}): details.overflow_x_mm = 20030 - 368; body = {body}"
        );
        assert_eq!(
            d["overflow_y_mm"].as_f64(),
            Some(19829.0),
            "one-call svg ({label}): details.overflow_y_mm = 20032 - 203; body = {body}"
        );
    }
}

/// A `?scale=` that is not a RATIO at all — NaN, infinite, zero, negative — is
/// refused as a caller error too, and never drawn.
///
/// Measured before the guard existed, on the kernel route: `NaN`, `-inf` and
/// `0` each produced a sheet that `verify_drawing` certified CLEAN (every
/// comparison against NaN is false; a zero-area footprint is inside every
/// frame), and `-4` produced a mirrored, right-to-left sheet. Over HTTP those
/// became `200 image/svg+xml` — a drawing of nothing, served as a drawing.
///
/// `details.value` is a STRING, deliberately: NaN and the infinities have no
/// JSON number form, and emitting `null` for them would erase the very value
/// being refused.
#[tokio::test]
async fn a_scale_that_is_not_a_ratio_is_refused_on_the_one_call_svg() {
    for raw in ["NaN", "0", "-4", "inf", "-inf"] {
        let state = make_test_state().await;
        let (id, _uuid) = create_box_full(&state).await;
        let (status, body) = dispatch(
            &state,
            get(&format!("/api/parts/{id}/drawing.svg?scale={raw}")),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "?scale={raw} must be refused as a caller error, never drawn; body = {body}"
        );
        assert_eq!(
            body["error_code"].as_str(),
            Some("invalid_parameter"),
            "?scale={raw}: the kernel did not fault, the caller did; body = {body}"
        );
        assert_eq!(
            body["retryable"].as_bool(),
            Some(false),
            "?scale={raw} fails identically on every retry; body = {body}"
        );
        assert_eq!(
            body["details"]["parameter"].as_str(),
            Some("scale"),
            "?scale={raw}: the refusal must name WHICH parameter it refuses; body = {body}"
        );
        assert!(
            body["details"]["value"].is_string(),
            "?scale={raw}: the refused value must survive as a string, not become null; body = {body}"
        );
        let message = body["error"].as_str().unwrap_or_default();
        assert!(
            !message.contains("mm"),
            "?scale={raw}: a non-ratio has no overflow, so the refusal must not report one; body = {body}"
        );
        assert!(
            !message.contains("  "),
            "?scale={raw}: refusal message carries absorbed indentation; body = {body}"
        );
    }
}

/// The REGISTERED route refuses a bad `?scale=` too, in the same shape.
///
/// `POST /api/parts/{id}/drawing` takes the same `PartDrawingQuery` and the
/// same `build_standard_drawing_off_lock`, but reaches it through
/// `create_part_drawing_inner`, whose error mapping is a SEPARATE `match` arm
/// (`drawing_mgr.rs`) from the one the `.svg` route uses. Nothing exercised it
/// with a `scale`: every `scale=` test in this crate hit `/drawing.svg`, so
/// changing that arm to 422 — or dropping the `InvalidScale` case from it —
/// broke no test at all.
///
/// This route is the one that used to succeed most damagingly: a 200 here
/// REGISTERED the off-sheet drawing, so the bad sheet outlived the request and
/// could be exported later.
#[tokio::test]
async fn a_bad_scale_is_refused_on_the_registered_part_drawing_route() {
    // (query, is-a-fit-failure) — one of each class, so a mapping that handles
    // only one of the two typed refusals cannot pass.
    for (raw, unfittable) in [("1000", true), ("NaN", false)] {
        let state = make_test_state().await;
        let (id, _uuid) = create_box_full(&state).await;
        let (status, body) = dispatch(
            &state,
            post(&format!("/api/parts/{id}/drawing?scale={raw}"), json!({})),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "POST drawing ?scale={raw} must refuse as a caller error, and must NOT register a sheet; body = {body}"
        );
        assert_eq!(
            body["error_code"].as_str(),
            Some("invalid_parameter"),
            "POST drawing ?scale={raw}: the kernel did not fault, the caller did; body = {body}"
        );
        assert_eq!(
            body["success"].as_bool(),
            Some(false),
            "POST drawing ?scale={raw}; body = {body}"
        );
        assert_eq!(
            body["retryable"].as_bool(),
            Some(false),
            "POST drawing ?scale={raw} fails identically on every retry; body = {body}"
        );
        assert_eq!(
            body["details"]["parameter"].as_str(),
            Some("scale"),
            "POST drawing ?scale={raw}: typed details must reach this route too; body = {body}"
        );
        if unfittable {
            // A 10 mm box on A3: usable area 368 x 211, fit height 203; unit
            // spans 10 everywhere, so at 1000:1 the group is 20030 x 20032.
            assert_eq!(
                body["details"]["overflow_x_mm"].as_f64(),
                Some(19662.0),
                "POST drawing ?scale={raw}: details.overflow_x_mm; body = {body}"
            );
            assert_eq!(
                body["details"]["overflow_y_mm"].as_f64(),
                Some(19829.0),
                "POST drawing ?scale={raw}: details.overflow_y_mm; body = {body}"
            );
        } else {
            assert!(
                body["details"]["value"].is_string(),
                "POST drawing ?scale={raw}: the refused value must survive as a string; body = {body}"
            );
        }

        // Nothing was registered: the refusal is not a sheet with a bad body.
        // `list_drawings` returns a bare JSON array, and the array is REQUIRED
        // to be found — a shape change must fail this test loudly rather than
        // let `unwrap_or(0)` report an empty registry that was never read.
        let registered = |v: &serde_json::Value| -> usize {
            v.as_array()
                .unwrap_or_else(|| panic!("GET /api/drawings must return an array; got {v}"))
                .len()
        };
        let (status, list) = dispatch(&state, get("/api/drawings")).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "drawing list must 200; body = {list}"
        );
        assert_eq!(
            registered(&list),
            0,
            "POST drawing ?scale={raw}: a refused sheet must never be registered; list = {list}"
        );

        // Positive control, so the assertion above cannot pass because the
        // registry is simply never populated on this path: the SAME request
        // without the bad scale registers exactly one sheet.
        let (status, ok_body) =
            dispatch(&state, post(&format!("/api/parts/{id}/drawing"), json!({}))).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "control: the same POST without a scale must succeed; body = {ok_body}"
        );
        let (_, list) = dispatch(&state, get("/api/drawings")).await;
        assert_eq!(
            registered(&list),
            1,
            "control: a successful POST must register exactly one sheet, which is what makes the zero above meaningful; list = {list}"
        );
    }
}

/// `acknowledge_layout_issues=true` CANNOT resurrect an unfittable scale.
///
/// The escape waives a layout VERDICT on a sheet that exists ("I have looked at
/// the collisions, ship the draft"). It is not a licence to draw a sheet that
/// cannot be drawn: no arrangement of a 10 mm box at 1000:1 fits an A3 frame,
/// so there is no draft to acknowledge. Before Task 40 this same request
/// returned 200 with three views hanging off the sheet — which is precisely the
/// silent-wrong-answer the escape was never meant to authorise.
#[tokio::test]
async fn acknowledge_layout_issues_cannot_resurrect_an_unfittable_one_call_scale() {
    for label in ["id", "uuid"] {
        let state = make_test_state().await;
        let (id, uuid) = create_box_full(&state).await;
        let path = if label == "id" {
            format!("/api/parts/{id}/drawing.svg?scale=1000&acknowledge_layout_issues=true")
        } else {
            format!("/api/parts/uuid/{uuid}/drawing.svg?scale=1000&acknowledge_layout_issues=true")
        };
        let (status, body) = dispatch(&state, get(&path)).await;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "one-call svg ({label}): acknowledging layout issues must not make an unfittable scale fittable; body = {body}"
        );
    }
}

/// No behaviour change on the happy path: a sound, quality-passing one-call
/// sheet exports exactly as it did before this gate was wired in, on both
/// routes.
#[tokio::test]
async fn a_sound_passing_one_call_svg_is_never_refused() {
    for label in ["id", "uuid"] {
        let state = make_test_state().await;
        let (id, uuid) = create_box_full(&state).await;
        let path = if label == "id" {
            format!("/api/parts/{id}/drawing.svg")
        } else {
            format!("/api/parts/uuid/{uuid}/drawing.svg")
        };
        let (status, body) = dispatch(&state, get(&path)).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "one-call svg ({label}) of a sound, passing sheet must 200; body = {body}"
        );
    }
}

/// Junk (`?acknowledge_layout_issues=1`) must not open the bypass, on
/// either route — the same fail-CLOSED-on-junk discipline
/// `junk_acknowledge_layout_issues_value_does_not_open_the_bypass` pins for
/// the registered routes, `Query<PartDrawingQuery>`'s `bool` deserialization
/// rejecting non-boolean junk before the handler runs.
///
/// The request carries NO `?scale=`, deliberately. It used to carry
/// `scale=1000`, which since Task 40 the kernel refuses outright — the
/// response would then be non-200 whatever the junk parameter did, and this
/// test would pass without ever exercising the thing it names. On a sheet that
/// would otherwise export cleanly, `!= OK` means the junk was rejected and
/// nothing else.
#[tokio::test]
async fn junk_acknowledge_layout_issues_does_not_open_the_one_call_svg_bypass() {
    let state = make_test_state().await;
    let (id, _uuid) = create_box_full(&state).await;
    let (status, body) = dispatch(
        &state,
        get(&format!(
            "/api/parts/{id}/drawing.svg?acknowledge_layout_issues=1"
        )),
    )
    .await;
    assert_ne!(
        status,
        StatusCode::OK,
        "junk acknowledge_layout_issues must never open the one-call svg bypass; body = {body}"
    );
}

// =====================================================================
// 6. Omission — the sheet leaves out a bore the model carries
// =====================================================================

/// Build a 40x40x20 plate with one Ø10 through bore directly in the live
/// model, and register its standard sheet.
///
/// The boolean is done in the kernel rather than through
/// `POST /api/geometry/boolean` because that route sits behind
/// `require_declared_intent`, and the subject under test is the CERTIFICATE,
/// not the intent gate. Same spirit as `make_a_dimension_stale`: touch the
/// one thing the fixture is about, through the shortest honest path.
async fn bored_part_drawing(state: &AppState) -> (Uuid, u32) {
    use geometry_engine::math::{Point3, Vector3};
    use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use geometry_engine::primitives::topology_builder::{GeometryId, TopologyBuilder};

    let solid_id = {
        let mut model = state.model.write().await;
        model.set_event_key(Some("gate-bored-plate".to_string()));
        let plate = match TopologyBuilder::new(&mut model).create_box_3d(40.0, 40.0, 20.0) {
            Ok(GeometryId::Solid(s)) => s,
            other => panic!("expected a solid plate, got {other:?}"),
        };
        let bore = match TopologyBuilder::new(&mut model).create_cylinder_3d(
            Point3::new(0.0, 0.0, -20.0),
            Vector3::Z,
            5.0,
            80.0,
        ) {
            Ok(GeometryId::Solid(s)) => s,
            other => panic!("expected a solid bore, got {other:?}"),
        };
        let part = boolean_operation(
            &mut model,
            plate,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("difference must succeed");
        model.set_event_key(None);
        part
    };

    let (status, body) = dispatch(
        state,
        post(&format!("/api/parts/{solid_id}/drawing"), json!({})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "bored part drawing create must 200; body = {body}"
    );
    let drawing_id = Uuid::parse_str(body["id"].as_str().expect("drawing id string"))
        .expect("drawing id must parse");
    (drawing_id, solid_id)
}

/// Reach into the registered drawing and DROP a hole row, leaving the model
/// untouched — the sheet now omits a bore the part carries. The exact mirror
/// of `make_a_dimension_stale`: mutate the sheet, never the model, so the
/// certificate has exactly one thing to find.
async fn drop_a_hole_row(state: &AppState, drawing_id: Uuid) {
    let handle = state
        .drawings
        .get(&drawing_id)
        .expect("drawing must be registered before it can be mutated");
    let mut guard = handle.write().await;
    assert!(
        !guard.hole_sites.is_empty(),
        "fixture precondition: the bored plate's sheet must table its bore"
    );
    guard.hole_sites.remove(0);
}

/// THE GATE, omission branch. A sheet that omits a bore the model carries is
/// refused — and the refusal SAYS SO.
///
/// Before this fix the gate fired correctly (`!cert.sound`) and then built its
/// message from `stale` and `dangling` alone, so an omission-only refusal read
/// "0 stale fact(s) ... and 0 dangling fact(s)": a refusal whose stated reason
/// is empty, handed to an agent that must now guess what to change. In a
/// product whose thesis is that the kernel cannot lie, a refusal that will not
/// say what it found is the same defect one level up.
///
/// Mutation: drop `omitted` from the message → RED on the message assertion.
#[tokio::test]
async fn an_omitted_bore_is_refused_with_a_refusal_that_names_the_omission() {
    for kind in ["pdf", "dxf", "svg"] {
        let state = make_test_state().await;
        let (drawing_id, _solid_id) = bored_part_drawing(&state).await;
        drop_a_hole_row(&state, drawing_id).await;

        let (status, body) =
            dispatch(&state, get(&format!("/api/drawings/{drawing_id}/{kind}"))).await;
        assert_sheet_refusal(
            status,
            &body,
            drawing_id,
            "sheet_unsound",
            StatusCode::CONFLICT,
            &format!("{kind} export of a sheet omitting a bore"),
        );

        // The counts must disclose the omission, not report an empty reason.
        assert_eq!(
            body["details"]["omitted"].as_u64(),
            Some(1),
            "{kind} refusal must report the omitted count; body = {body}"
        );
        assert_eq!(
            body["details"]["stale"].as_u64(),
            Some(0),
            "{kind} fixture must be omission-ONLY; body = {body}"
        );
        assert_eq!(
            body["details"]["dangling"].as_u64(),
            Some(0),
            "{kind} fixture must be omission-ONLY; body = {body}"
        );

        // The message must name what was found, in words a reader acts on.
        // The catalog serialises the human message under "error".
        let msg = body["error"].as_str().unwrap_or_default();
        assert!(
            msg.contains("1 omitted"),
            "{kind} refusal message must name the omission; message = {msg}"
        );
        assert!(
            !msg.contains("  "),
            "no absorbed indentation in a user-facing message; message = {msg}"
        );

        // And it must hand over a WITNESS: which feature is missing.
        let witness = body["details"]["omitted_facts"]
            .as_array()
            .unwrap_or(&Vec::new())
            .clone();
        assert!(
            !witness.is_empty(),
            "{kind} refusal must name the omitted feature, not just count it; body = {body}"
        );
        assert!(
            witness
                .iter()
                .any(|w| w.as_str().unwrap_or_default().contains("bore")),
            "{kind} witness must identify the bore; witness = {witness:?}"
        );
    }
}
