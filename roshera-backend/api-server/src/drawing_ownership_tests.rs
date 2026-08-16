//! Drawing ownership — RED for L8b (`drawing_mgr.rs::drawing_solid_ids`'s
//! own documented-not-fixed aliasing hazard, closed 2026-08-16). See
//! `.superpowers/sdd/2026-08-16-drawing-ownership/`.
//!
//! `state.drawings` was one flat registry: `drawing_solid_ids` resolved a
//! stored drawing's `SolidId`s against whatever `ActiveModel` yields NOW.
//! Kernel solid ids are small integers reused across every document/part —
//! so document/part B's solid N can gate, and CERTIFY, document/part A's
//! sheet. These two tests are the two legs the design doc names:
//!
//!   1. the PART-header leg: a drawing built from part A's SOUND solid
//!      reads back as UNSOUND when the caller merely names a DIFFERENT
//!      part (B) in `X-Roshera-Part-Id` — the certificate lies, confidently,
//!      at HTTP 200.
//!   2. the LEGACY/document leg: a drawing built under document 1 is wiped
//!      outright by `documents::activate`'s `state.drawings.clear()` the
//!      moment document 2 is opened — the destructive mitigation this fix
//!      replaces with an honest, typed refusal instead of data loss.
//!
//! Both assert the POST-FIX behaviour (so they stand as the fix's own
//! regression pin going forward) and are therefore RED against the
//! pre-fix code — their failure output is the required RED evidence.

#![cfg(test)]

use crate::durability_boot_tests::{dispatch, get, post};
use crate::router_integration_tests::make_test_state;
use crate::AppState;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use geometry_engine::math::Point3;
use geometry_engine::primitives::provenance::ConstructionGeometry;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use serde_json::json;
use uuid::Uuid;

const PART_HEADER: &str = "X-Roshera-Part-Id";

fn get_with_header(uri: String, header_name: &str, header_value: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header_name, header_value)
        .body(Body::empty())
        .expect("static request must build")
}

fn post_with_header(
    uri: String,
    header_name: &str,
    header_value: &str,
    payload: serde_json::Value,
) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("content-type", "application/json")
        .header(header_name, header_value)
        .body(Body::from(payload.to_string()))
        .expect("static request must build")
}

async fn create_part(state: &AppState, name: &str) -> Uuid {
    let (status, body) = dispatch(state, post("/api/parts", json!({ "name": name }))).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "part create must 200; body = {body}"
    );
    Uuid::parse_str(body["id"].as_str().expect("part id string")).expect("part id must parse")
}

/// A sound box created inside a specific part-tab, addressed with the
/// `X-Roshera-Part-Id` header exactly as a real multi-tab client would.
/// Returns the box's kernel `solid_id`.
async fn sound_box_in_part(state: &AppState, part_id: Uuid, size: f64) -> u32 {
    let (status, body) = dispatch(
        state,
        post_with_header(
            "/api/geometry/box".to_string(),
            PART_HEADER,
            &part_id.to_string(),
            json!({ "width": size, "depth": size, "height": size }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "box create must 200; body = {body}");
    assert_eq!(
        body["perception"]["sound"].as_bool(),
        Some(true),
        "fixture precondition: default box creation must certify SOUND; \
         body = {body}"
    );
    body["solid_id"]
        .as_u64()
        .expect("solid_id must be a number") as u32
}

/// A box created inside a specific part-tab whose construction geometry is
/// drifted (same technique `router_integration_tests::
/// seed_box_with_drifted_construction` uses on the legacy model), then
/// verified through the REST perception route SCOPED TO THAT PART so the
/// live reading becomes `Unsound`, not merely `Stale`. Returns the solid_id.
async fn unsound_box_in_part(state: &AppState, part_id: Uuid, size: f64) -> u32 {
    let handle = state
        .parts
        .get(&part_id)
        .expect("part must exist before seeding geometry into it");
    let solid_id = {
        let mut model_guard = handle.write().await;
        let model: &mut BRepModel = &mut model_guard;
        let solid_id = {
            let mut builder = TopologyBuilder::new(model);
            match builder
                .create_box_3d(size, size, size)
                .expect("box primitive must build for positive size")
            {
                GeometryId::Solid(id) => id,
                other => panic!("expected solid, got {other:?}"),
            }
        };
        // Construction geometry ~1000 units away from the box — far outside
        // the consistency tolerance band, so the cert reports
        // `construction_consistent = inconsistent`, same technique
        // `seed_box_with_drifted_construction` uses on the legacy model.
        let far = Point3::new(1000.0, 1000.0, 1000.0);
        model.set_solid_construction(
            solid_id,
            ConstructionGeometry::new(far, vec![far, Point3::new(1001.0, 1000.0, 1000.0)]),
        );
        solid_id
    };

    let (status, body) = dispatch(
        state,
        get_with_header(
            format!("/api/agent/parts/{solid_id}/perception"),
            PART_HEADER,
            &part_id.to_string(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "perception must 200; body = {body}");
    assert_eq!(
        body["status"].as_str(),
        Some("unsound"),
        "fixture precondition: drifted construction must read Unsound once \
         verified through the SAME part's own header; body = {body}"
    );
    solid_id
}

/// Register the standard one-call sheet for `solid_id`, scoped to `part_id`
/// via the header, and return the registered drawing id.
async fn register_drawing_for_part(state: &AppState, part_id: Uuid, solid_id: u32) -> Uuid {
    let (status, body) = dispatch(
        state,
        post_with_header(
            format!("/api/parts/{solid_id}/drawing"),
            PART_HEADER,
            &part_id.to_string(),
            json!({}),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "part drawing create must 200; body = {body}"
    );
    Uuid::parse_str(body["id"].as_str().expect("drawing id string")).expect("drawing id must parse")
}

// =====================================================================
// 1. THE PART-HEADER LEG — the certificate lies
// =====================================================================

/// THE RED this whole fix hangs on (part leg). Part A holds a SOUND box;
/// part B holds an UNSOUND one at the SAME kernel solid id (both are the
/// first solid created in a fresh per-part `BRepModel`, so their ids
/// coincide by construction — asserted below as a fixture precondition,
/// not assumed). A's drawing is registered from A's own solid.
///
/// Reading A's drawing certificate while the caller's `X-Roshera-Part-Id`
/// header names B is the exploit: `drawing_solid_ids` returns A's numeric
/// solid id with no notion of WHICH part it came from, `ActiveModel`
/// resolves the header to B's model, and the disclosed reading is
/// confidently `unsound` for a drawing whose real, only-ever owner (A) is
/// sound.
///
/// Post-fix: the drawing's OWNER (captured at creation, immune to the
/// caller's header) is what every read resolves against, so this exact
/// swap becomes inexpressible — the B-header read must return EXACTLY
/// what the A-header read returns.
#[tokio::test]
async fn red_drawing_certificate_lies_when_read_under_a_different_parts_header() {
    let state = make_test_state().await;
    let part_a = create_part(&state, "A").await;
    let part_b = create_part(&state, "B").await;

    let sid_a = sound_box_in_part(&state, part_a, 10.0).await;
    let sid_b = unsound_box_in_part(&state, part_b, 10.0).await;
    assert_eq!(
        sid_a, sid_b,
        "fixture precondition: two freshly-created single-solid parts must \
         assign the SAME kernel SolidId (a per-BRepModel counter starting \
         fresh for each part) — this identical numeric id colliding across \
         two DIFFERENT BRepModels is the exact aliasing surface this fix \
         closes; got sid_a={sid_a} sid_b={sid_b}"
    );

    let drawing_id = register_drawing_for_part(&state, part_a, sid_a).await;

    // Ground truth: read under A's OWN header.
    let (status_a, body_a) = dispatch(
        &state,
        get_with_header(
            format!("/api/drawings/{drawing_id}/certificate"),
            PART_HEADER,
            &part_a.to_string(),
        ),
    )
    .await;
    assert_eq!(status_a, StatusCode::OK, "body_a = {body_a}");
    let readings_a = body_a["solid_soundness"]
        .as_array()
        .expect("solid_soundness must be an array");
    assert!(
        readings_a
            .iter()
            .any(|r| r["reading"] == "sound" && r["solid_id"] == sid_a),
        "ground truth: read under the TRUE owner's own header must report \
         `sound` for solid {sid_a}; body_a = {body_a}"
    );

    // THE LIE: read under B's header.
    let (status_b, body_b) = dispatch(
        &state,
        get_with_header(
            format!("/api/drawings/{drawing_id}/certificate"),
            PART_HEADER,
            &part_b.to_string(),
        ),
    )
    .await;

    eprintln!(
        "RED EVIDENCE — drawing {drawing_id} (owner: part A, solid {sid_a}, \
         SOUND) read under X-Roshera-Part-Id naming part B (solid {sid_b}, \
         UNSOUND):\n  read under A's own header -> status {status_a}, body = {body_a}\n  \
         read under B's header     -> status {status_b}, body = {body_b}"
    );

    assert_eq!(
        status_a, status_b,
        "reading the SAME drawing_id must not depend on which part header \
         the caller happens to send; status_a = {status_a} body_a = {body_a}; \
         status_b = {status_b} body_b = {body_b}"
    );
    let readings_b = body_b["solid_soundness"]
        .as_array()
        .expect("solid_soundness must be an array");
    assert!(
        readings_b
            .iter()
            .any(|r| r["reading"] == "sound" && r["solid_id"] == sid_a),
        "THE LIE (pre-fix): reading part A's OWN, sound drawing while the \
         caller's X-Roshera-Part-Id names an unrelated part (B) must still \
         report `sound` for solid {sid_a} — the drawing's OWNER, not the \
         caller's header, must decide which model certifies it. \
         body_b = {body_b}"
    );
    assert!(
        !readings_b
            .iter()
            .any(|r| r["reading"] == "unsound" && r["solid_id"] == sid_a),
        "THE LIE (pre-fix): must never report `unsound` for A's drawing \
         merely because B's UNRELATED solid happens to share the same \
         numeric id; body_b = {body_b}"
    );
}

// =====================================================================
// 2. THE LEGACY/DOCUMENT LEG — activate() destroys drawings outright
// =====================================================================

async fn register_document(state: &AppState, name: &str) -> String {
    let (status, body) = dispatch(state, post("/api/documents", json!({ "name": name }))).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "document register must 200; body = {body}"
    );
    body["id"].as_str().expect("document id string").to_string()
}

async fn open_document(state: &AppState, id: &str) {
    let (status, body) =
        dispatch(state, post(&format!("/api/documents/{id}/open"), json!({}))).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "document open must 200; body = {body}"
    );
}

/// A state whose recorder writes through to a real `DatabaseEventSink`
/// (in-memory sqlite is fine here — this test never tears the `AppState`
/// down and rebuilds it, so the same connection pool stays live for the
/// whole test; `durability_boot_tests`'s file-backed variant exists only
/// because THAT module simulates a process restart, which this test does
/// not). Needed because `documents::activate` unconditionally resets
/// `state.model` to empty and REPLAYS a document's persisted log — with
/// `make_test_state()`'s default (no sink), nothing is ever persisted, so
/// switching away and back would empty the model regardless of this fix,
/// which would test durability wiring, not drawing ownership.
async fn make_durable_test_state() -> AppState {
    use session_manager::{DatabaseConfig, DatabasePersistence, DatabaseType, SqliteDatabase};
    let db_config = DatabaseConfig {
        db_type: DatabaseType::SQLite,
        url: "sqlite::memory:".to_string(),
        max_connections: 4,
        connect_timeout: 5,
        run_migrations: true,
    };
    let database: std::sync::Arc<dyn DatabasePersistence + Send + Sync> = std::sync::Arc::new(
        SqliteDatabase::new(&db_config)
            .await
            .expect("sqlite::memory: must initialise"),
    );
    let active_document = std::sync::Arc::new(tokio::sync::RwLock::new(
        crate::durability::DURABILITY_SESSION_ID.to_string(),
    ));
    let sink: std::sync::Arc<dyn timeline_engine::EventSink> = std::sync::Arc::new(
        crate::durability::DatabaseEventSink::new(database.clone(), active_document.clone()),
    );
    crate::router_integration_tests::make_test_state_with_database(
        database,
        Some(sink),
        Some(active_document),
    )
    .await
}

/// THE RED this fix hangs on (legacy/document leg). A drawing registered
/// (headerless — the legacy singleton model) under document 1 must survive
/// switching to document 2, per the design ruling: "switching documents
/// stops destroying drawings." Reading it while document 2 is active must
/// disclose a stated absence (`/certificate` is read-only and never
/// refuses), never a silent wrong answer and never the destructive
/// 404-by-erasure `documents::activate`'s `state.drawings.clear()`
/// produces today; EXPORTING it must be the new typed refusal instead.
/// Reactivating document 1 must make the drawing fully measurable again,
/// content and geometry both — this uses a durably-wired state (see
/// `make_durable_test_state`) so that claim is actually exercised, not
/// just the registry survival half.
#[tokio::test]
async fn red_legacy_drawing_survives_a_document_switch_instead_of_being_erased() {
    let state = make_durable_test_state().await;
    let doc1 = register_document(&state, "Doc 1").await;
    let doc2 = register_document(&state, "Doc 2").await;

    open_document(&state, &doc1).await;
    let (status, body) = dispatch(
        &state,
        post(
            "/api/geometry/box",
            json!({ "width": 10.0, "depth": 10.0, "height": 10.0 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "box create must 200; body = {body}");
    let solid_id = body["solid_id"]
        .as_u64()
        .expect("solid_id must be a number") as u32;

    let drawing_id = {
        let (status, body) = dispatch(
            &state,
            post(&format!("/api/parts/{solid_id}/drawing"), json!({})),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "part drawing create must 200; body = {body}"
        );
        Uuid::parse_str(body["id"].as_str().expect("drawing id string"))
            .expect("drawing id must parse")
    };

    // Switch away. Pre-fix: `documents::activate`'s `state.drawings.clear()`
    // erases the drawing outright — a data-loss mitigation, not a fix.
    open_document(&state, &doc2).await;

    // `/certificate` is a READ-ONLY inspection surface and DISCLOSES rather
    // than refuses on an unresolvable owner (the same L2 disclose-don't-
    // refuse ruling already applied to a single unsound solid) — so the
    // right post-fix answer here is 200 with `certificate: null` and a
    // stated `unavailable_reason`, never a 404 (erasure) and never a
    // refused status either.
    let (status_away, body_away) = dispatch(
        &state,
        get(&format!("/api/drawings/{drawing_id}/certificate")),
    )
    .await;
    eprintln!(
        "RED EVIDENCE — drawing {drawing_id} registered under document \
         {doc1}, read while document {doc2} is active: status = \
         {status_away}, body = {body_away}"
    );
    assert_ne!(
        status_away,
        StatusCode::NOT_FOUND,
        "the drawing must SURVIVE a document switch (documents::activate's \
         drawings.clear() is the destructive mitigation this fix replaces) \
         — a read-only inspection surface must disclose a stated absence, \
         not be erased outright; body_away = {body_away}"
    );
    assert_eq!(
        status_away,
        StatusCode::OK,
        "a read-only inspection surface never refuses — disclose, not \
         refuse; body_away = {body_away}"
    );
    assert!(
        body_away.get("certificate").is_none() || body_away["certificate"].is_null(),
        "the certificate must be a STATED ABSENCE while the owning \
         document is not active, never a default/fabricated value; \
         body_away = {body_away}"
    );
    assert!(
        body_away["unavailable_reason"].is_string(),
        "the absence must carry a stated reason; body_away = {body_away}"
    );
    assert_eq!(
        body_away["owner"]["document_id"].as_str(),
        Some(doc1.as_str()),
        "the disclosed owner must still name document 1, regardless of \
         which document is currently active; body_away = {body_away}"
    );

    // The EXPORT surface makes the OPPOSITE choice — it produces an
    // artifact, so an unresolvable owner is refused, fail closed, with the
    // NEW typed error code distinct from drawing-not-found.
    let (status_export, body_export) =
        dispatch(&state, get(&format!("/api/drawings/{drawing_id}/pdf"))).await;
    assert_eq!(
        body_export["error_code"].as_str(),
        Some("drawing_owner_unresolvable"),
        "exporting a document-1-owned drawing while document 2 is active \
         must be the NEW typed refusal, distinct from drawing-not-found — \
         the drawing exists, its owning document does not resolve; \
         status = {status_export}, body_export = {body_export}"
    );

    // Switch back. The design ruling: "reactivating a document makes its
    // drawings measurable again."
    open_document(&state, &doc1).await;
    let (status_back, body_back) = dispatch(
        &state,
        get(&format!("/api/drawings/{drawing_id}/certificate")),
    )
    .await;
    assert_eq!(
        status_back,
        StatusCode::OK,
        "reactivating the owning document must make the drawing \
         measurable again; body_back = {body_back}"
    );
    assert_eq!(
        body_back["sound"].as_bool(),
        Some(true),
        "body_back = {body_back}"
    );
}

// =====================================================================
// 3. THE add_view OWNER-CONSISTENCY GATE
// =====================================================================
//
// A hole the ownership fix would otherwise have INTRODUCED (spotted in
// review): `create_drawing` stamps an owner ONCE, at creation, from
// whatever `ActiveModel` context was ambient THEN. `add_view`'s own
// `ViewSource::Part.part_id` is a real `PartManager` id by contract,
// resolved directly — never through `ActiveModel` — so on its own it
// carries no aliasing risk. But if a view sourced from a DIFFERENT part
// than the drawing's owner were allowed to attach, every LATER owner-
// scoped read (certify, /semantic, /certificate, the export gates) would
// measure that one view against the WRONG model forever — a fresh,
// deterministic lie strictly worse than the pre-fix bug (today's caller
// could at least reach truth by sending the right header at READ time;
// post-fix nothing they send at read time matters at all).

fn empty_drawing_request(name: &str) -> serde_json::Value {
    json!({ "name": name, "sheet_size": "A4" })
}

fn add_view_request(part_id: Uuid, solid_id: u32) -> serde_json::Value {
    json!({
        "name": "Front",
        "source": { "kind": "part", "part_id": part_id, "solid_id": solid_id },
        "projection": { "kind": "front" },
    })
}

/// A view sourced from a part OTHER than a `Part`-owned drawing's own
/// owner must be refused, or the drawing would silently start measuring
/// against the wrong part's geometry on every future read.
#[tokio::test]
async fn add_view_refuses_a_view_sourced_from_a_different_part_than_the_drawings_owner() {
    let state = make_test_state().await;
    let part_a = create_part(&state, "A").await;
    let part_b = create_part(&state, "B").await;
    let sid_a = sound_box_in_part(&state, part_a, 10.0).await;

    let (status, body) = dispatch(
        &state,
        post_with_header(
            "/api/drawings".to_string(),
            PART_HEADER,
            &part_a.to_string(),
            empty_drawing_request("A's sheet"),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "drawing create must 200; body = {body}"
    );
    let drawing_id = body["id"].as_str().expect("drawing id string").to_string();

    // Add a view sourced from B — a DIFFERENT part than the drawing's
    // owner (A).
    let (status, body) = dispatch(
        &state,
        post(
            &format!("/api/drawings/{drawing_id}/views"),
            add_view_request(part_b, sid_a),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a view sourced from a part OTHER than the drawing's owner must be \
         refused; body = {body}"
    );
    assert_eq!(body["error_code"].as_str(), Some("invalid_parameter"));
}

/// The SAME part as the drawing's owner must be accepted — the gate must
/// not refuse the legitimate case.
#[tokio::test]
async fn add_view_accepts_a_view_sourced_from_the_drawings_own_owner_part() {
    let state = make_test_state().await;
    let part_a = create_part(&state, "A").await;
    let sid_a = sound_box_in_part(&state, part_a, 10.0).await;

    let (status, body) = dispatch(
        &state,
        post_with_header(
            "/api/drawings".to_string(),
            PART_HEADER,
            &part_a.to_string(),
            empty_drawing_request("A's sheet"),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "drawing create must 200; body = {body}"
    );
    let drawing_id = body["id"].as_str().expect("drawing id string").to_string();

    let (status, body) = dispatch(
        &state,
        post(
            &format!("/api/drawings/{drawing_id}/views"),
            add_view_request(part_a, sid_a),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a view sourced from the drawing's OWN owner part must succeed; \
         body = {body}"
    );
}

/// A `Legacy`-owned drawing (created with no `X-Roshera-Part-Id` header)
/// has no part-tab of its own at all — a part-sourced view can never be
/// added to it without introducing exactly the aliasing risk this fix
/// closes elsewhere.
#[tokio::test]
async fn add_view_refuses_a_part_sourced_view_on_a_legacy_owned_drawing() {
    let state = make_test_state().await;
    let part_a = create_part(&state, "A").await;
    let sid_a = sound_box_in_part(&state, part_a, 10.0).await;

    // Headerless create ⇒ Legacy-owned.
    let (status, body) = dispatch(
        &state,
        post("/api/drawings", empty_drawing_request("Legacy sheet")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "drawing create must 200; body = {body}"
    );
    let drawing_id = body["id"].as_str().expect("drawing id string").to_string();

    let (status, body) = dispatch(
        &state,
        post(
            &format!("/api/drawings/{drawing_id}/views"),
            add_view_request(part_a, sid_a),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a part-sourced view must be refused on a legacy-owned drawing; \
         body = {body}"
    );
    assert_eq!(body["error_code"].as_str(), Some("invalid_parameter"));
}
