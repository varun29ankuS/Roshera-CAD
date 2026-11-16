//! Correctness of the two speed fixes on the open and snapshot paths:
//!
//! - a document open replays its history ONCE and installs the certified
//!   model — it must be the model a full replay builds, and the live model
//!   must keep recording afterwards;
//! - the scene snapshot serves each part's cached display mesh — it must be
//!   the mesh of the part as it stands, never one from before a change.

#![cfg(test)]

use crate::durability_boot_tests::{build_state, dispatch, get, open_db, post, temp_db_path};
use crate::{durability, AppState};

use axum::http::StatusCode;
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
use serde_json::{json, Value};

/// Every solid's id, name and default tessellation, sorted by id.
async fn model_portrait(state: &AppState) -> Vec<(u32, Option<String>, Vec<f64>, usize)> {
    let model = state.model.read().await;
    portrait_of(&model)
}

fn portrait_of(
    model: &geometry_engine::primitives::topology_builder::BRepModel,
) -> Vec<(u32, Option<String>, Vec<f64>, usize)> {
    let params = TessellationParams::default();
    let mut out: Vec<_> = model
        .solids
        .iter()
        .map(|(id, solid)| {
            let mesh = tessellate_solid(solid, model, &params);
            let coords: Vec<f64> = mesh
                .vertices
                .iter()
                .flat_map(|v| [v.position.x, v.position.y, v.position.z])
                .collect();
            (id, solid.name.clone(), coords, mesh.triangles.len())
        })
        .collect();
    out.sort_by_key(|p| p.0);
    out
}

async fn build(state: &AppState) {
    let (s, body) = dispatch(
        state,
        post(
            "/api/geometry/box",
            json!({ "width": 6.0, "depth": 4.0, "height": 3.0, "name": "Block" }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "box: {body}");
    let block = body["object"]["id"].as_str().expect("box id").to_string();
    let (s, body) = dispatch(
        state,
        post(
            "/api/geometry/cylinder",
            json!({ "radius": 1.0, "height": 8.0, "name": "Pin" }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "cylinder: {body}");
    let pin = body["object"]["id"]
        .as_str()
        .expect("cylinder id")
        .to_string();
    let (s, body) = dispatch(
        state,
        post(
            "/api/geometry/boolean",
            json!({ "operation": "difference", "object_a": block, "object_b": pin }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "boolean: {body}");
    let (s, body) = dispatch(
        state,
        post(
            "/api/geometry/box",
            json!({ "width": 1.0, "depth": 1.0, "height": 1.0, "name": "Cube" }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "cube: {body}");
    state
        .timeline_recorder
        .flush()
        .await
        .expect("recorder flush must succeed");
}

/// The re-open installs the model the certify pass built: it must be the
/// model a full `rebuild_model_from_events` of the same history builds —
/// ids, names, geometry — and the live model must still record afterwards
/// (a model installed without the recorder would silently persist nothing).
#[tokio::test]
async fn reopen_installs_the_full_replay_model_and_keeps_recording() {
    let state = build_state(open_db(&temp_db_path()).await, true).await;
    build(&state).await;

    let rows = state
        .database
        .load_all_timeline_events(durability::DURABILITY_SESSION_ID)
        .await
        .expect("event log must read");
    let events: Vec<timeline_engine::TimelineEvent> = rows
        .iter()
        .filter_map(|r| serde_json::from_value(r.data.clone()).ok())
        .collect();
    let mut reference = geometry_engine::primitives::topology_builder::BRepModel::new();
    let outcome = timeline_engine::rebuild_model_from_events(&mut reference, &events);
    assert_eq!(
        outcome.events_skipped, 0,
        "precondition: the history replays"
    );

    let status = crate::documents::activate(&state, durability::DURABILITY_SESSION_ID).await;
    assert!(
        matches!(status, durability::DurabilityStatus::Active { .. }),
        "status = {status:?}"
    );
    assert_eq!(
        model_portrait(&state).await,
        portrait_of(&reference),
        "the installed model is the full-replay model"
    );

    let before = rows.len();
    let (s, body) = dispatch(
        &state,
        post(
            "/api/geometry/box",
            json!({ "width": 2.0, "depth": 2.0, "height": 2.0 }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "box after re-open: {body}");
    state
        .timeline_recorder
        .flush()
        .await
        .expect("recorder flush must succeed");
    let after = state
        .database
        .get_event_count(durability::DURABILITY_SESSION_ID)
        .await
        .expect("event count must query");
    assert!(
        after as usize > before,
        "an operation after the re-open must be recorded and persisted ({before} → {after})"
    );
}

/// The object of `name` in a snapshot body.
fn snapshot_object<'a>(body: &'a Value, name: &str) -> &'a Value {
    body["objects"]
        .as_array()
        .and_then(|o| o.iter().find(|o| o["name"] == name))
        .unwrap_or_else(|| panic!("snapshot has no object named {name}: {body}"))
}

/// The snapshot serves the CURRENT shape: a part moved after a snapshot comes
/// back moved (its cached mesh went with the change), equal to a fresh
/// tessellation; an untouched repeat read is byte-identical.
#[tokio::test]
async fn the_snapshot_mesh_is_the_part_as_it_stands() {
    let state = build_state(open_db(&temp_db_path()).await, true).await;
    let (s, body) = dispatch(
        &state,
        post(
            "/api/geometry/box",
            json!({ "width": 3.0, "depth": 2.0, "height": 1.0, "name": "Mover" }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "box: {body}");
    let uuid = body["object"]["id"].as_str().expect("box id").to_string();

    let (s, first) = dispatch(&state, get("/api/scene/snapshot")).await;
    assert_eq!(s, StatusCode::OK);
    let before = snapshot_object(&first, "Mover")["mesh"]["vertices"].clone();

    let (s, body) = dispatch(
        &state,
        post(
            "/api/geometry/transform",
            json!({ "object": uuid, "translation": [5.0, 0.0, 0.0] }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "transform: {body}");

    let (s, second) = dispatch(&state, get("/api/scene/snapshot")).await;
    assert_eq!(s, StatusCode::OK);
    let after = snapshot_object(&second, "Mover")["mesh"]["vertices"].clone();
    assert_ne!(
        after, before,
        "the moved part must not be served at its old place"
    );

    let fresh: Vec<f32> = {
        let model = state.model.read().await;
        let (id, solid) = model
            .solids
            .iter()
            .find(|(_, s)| s.name.as_deref() == Some("Mover"))
            .expect("the part");
        let _ = id;
        tessellate_solid(solid, &model, &TessellationParams::default())
            .to_threejs()
            .positions
    };
    assert_eq!(
        after,
        json!(fresh),
        "the served mesh is a fresh tessellation"
    );

    let (_, third) = dispatch(&state, get("/api/scene/snapshot")).await;
    assert_eq!(third, second, "an untouched repeat read is identical");
}
