//! GD&T edits are recorded design events (founder decision 2026-11-13:
//! "Datums and GD&T should be changeable by the user too, and drawing should
//! adapt to it").
//!
//! Before this, datum features and feature control frames lived only in the
//! live model's sidecars (`"persistence": "session"`): no undo, lost on
//! restart, and a drawing saw them only when it was first built. These tests
//! drive the real router and prove each behaviour end to end:
//!
//! * a datum + frame survive a restart (durable replay);
//! * undo takes back the last GD&T edit and redo restores it;
//! * an edited tolerance reaches the next sheet export, and the old sheet
//!   certifies STALE against the edited model;
//! * a datum face consumed by a later boolean is reported DANGLING (typed),
//!   on the part the boolean made — the GD&T follows the part;
//! * every validation refusal changes nothing (model and history).

#![cfg(test)]

use crate::durability_boot_tests::{
    build_state, del, dispatch, get, open_db, patch, post, temp_db_path,
};
use crate::router_integration_tests::make_test_state;
use crate::AppState;

use axum::http::StatusCode;
use serde_json::{json, Value};

/// A 10 × 10 × 10 box through the live route: base on z = 0, top at z = 10.
/// Returns `(solid_id, object uuid)`.
async fn make_box(state: &AppState, center: [f64; 3], w: f64, d: f64, h: f64) -> (u32, String) {
    let (s, body) = dispatch(
        state,
        post(
            "/api/geometry/box",
            json!({ "center": center, "width": w, "depth": d, "height": h }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "box create; body = {body}");
    let id = body["solid_id"].as_u64().expect("solid_id") as u32;
    let uuid = body["object"]["id"].as_str().expect("uuid").to_string();
    (id, uuid)
}

fn face_along(z: f64) -> Value {
    json!({ "kind": "planar", "normal_dir": [0.0, 0.0, z] })
}

async fn gdt_of(state: &AppState, part: u32) -> Value {
    let (s, body) = dispatch(state, get(&format!("/api/agent/parts/{part}/gdt"))).await;
    assert_eq!(s, StatusCode::OK, "GET gdt; body = {body}");
    body
}

/// Number of recorded events on the main branch, after the recorder settles.
async fn history_len(state: &AppState) -> usize {
    let _ = state.timeline_recorder.settle().await;
    let timeline = state.timeline.read().await;
    timeline
        .get_branch_events(&timeline_engine::BranchId::main(), None, None)
        .map(|e| e.len())
        .unwrap_or(0)
}

/// `(label, persistent_id, status)` of every datum, sorted by label.
fn datums_of(gdt: &Value) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = gdt["datums"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|d| {
                    (
                        d["label"].as_str().unwrap_or_default().to_string(),
                        d["persistent_id"].as_str().unwrap_or_default().to_string(),
                        d["resolution"]["status"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// `(annotation_id, characteristic, tolerance_mm, datum labels)` of every
/// annotation, sorted by id.
fn frames_of(gdt: &Value) -> Vec<(String, String, f64, Vec<String>)> {
    let mut out: Vec<(String, String, f64, Vec<String>)> = gdt["annotations"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|f| {
                    (
                        f["annotation_id"].as_str().unwrap_or_default().to_string(),
                        f["verdict"]["characteristic"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        f["verdict"]["tolerance_mm"].as_f64().unwrap_or(f64::NAN),
                        f["verdict"]["datum_statuses"]
                            .as_array()
                            .map(|d| {
                                d.iter()
                                    .map(|x| x["label"].as_str().unwrap_or_default().to_string())
                                    .collect()
                            })
                            .unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Datum A on the bottom face, a parallelism frame to A on the top face.
/// Returns the frame's annotation id.
async fn datum_and_frame(state: &AppState, part: u32, tolerance: f64) -> String {
    let (s, body) = dispatch(
        state,
        post(
            &format!("/api/agent/parts/{part}/datums"),
            json!({ "label": "A", "selector": face_along(-1.0) }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "datum A; body = {body}");
    let (s, body) = dispatch(
        state,
        post(
            &format!("/api/agent/parts/{part}/fcf"),
            json!({
                "characteristic": "parallelism",
                "tolerance_mm": tolerance,
                "datum_refs": ["A"],
                "selector": face_along(1.0),
            }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "frame; body = {body}");
    body["annotation_id"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_default()
}

#[tokio::test]
async fn a_datum_and_a_frame_survive_a_restart() {
    let path = temp_db_path();
    let (part, before) = {
        let state = build_state(open_db(&path).await, true).await;
        let (part, _) = make_box(&state, [0.0, 0.0, 0.0], 10.0, 10.0, 10.0).await;
        let id = datum_and_frame(&state, part, 0.05).await;
        let before = gdt_of(&state, part).await;
        assert_eq!(frames_of(&before)[0].0, id);
        state
            .timeline_recorder
            .flush()
            .await
            .expect("recorder flush must succeed");
        (part, before)
    };
    let state2 = build_state(open_db(&path).await, true).await;
    let (s, status) = dispatch(&state2, get("/api/durability/status")).await;
    assert_eq!(s, StatusCode::OK, "body = {status}");
    assert_eq!(status["quarantined"], false, "body = {status}");
    let after = gdt_of(&state2, part).await;
    assert_eq!(
        datums_of(&after),
        datums_of(&before),
        "the rebuilt part carries the same datum, on the same face"
    );
    assert_eq!(
        frames_of(&after),
        frames_of(&before),
        "the rebuilt part carries the same frame under the same id"
    );
    assert!(!datums_of(&after).is_empty(), "fixture: a datum was made");
    assert_eq!(after["persistence"], "timeline", "body = {after}");
}

#[tokio::test]
async fn undo_takes_back_the_last_gdt_edit_and_redo_restores_it() {
    let state = make_test_state().await;
    let (part, _) = make_box(&state, [0.0, 0.0, 0.0], 10.0, 10.0, 10.0).await;
    let id = datum_and_frame(&state, part, 0.05).await;
    let (s, body) = dispatch(
        &state,
        patch(
            &format!("/api/agent/parts/{part}/fcf/{id}"),
            json!({ "tolerance_mm": 0.02 }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "frame edit; body = {body}");
    assert_eq!(frames_of(&gdt_of(&state, part).await)[0].2, 0.02);

    let _ = state.timeline_recorder.settle().await;
    let session = crate::handlers::timeline::live_session_id(&timeline_engine::BranchId::main());
    let (s, undone) = dispatch(
        &state,
        post(
            "/api/timeline/undo",
            json!({ "session_id": session.to_string() }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{undone}");
    assert_ne!(undone["success"], false, "{undone}");
    let after_undo = gdt_of(&state, part).await;
    assert_eq!(
        frames_of(&after_undo)
            .iter()
            .map(|f| (f.0.clone(), f.2))
            .collect::<Vec<_>>(),
        vec![(id.clone(), 0.05)],
        "undo puts the frame back at its earlier tolerance; body = {after_undo}"
    );
    assert_eq!(datums_of(&after_undo).len(), 1, "the datum is untouched");

    let (s, redone) = dispatch(
        &state,
        post(
            "/api/timeline/redo",
            json!({ "session_id": session.to_string() }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{redone}");
    assert_ne!(redone["success"], false, "{redone}");
    assert_eq!(
        frames_of(&gdt_of(&state, part).await)
            .iter()
            .map(|f| (f.0.clone(), f.2))
            .collect::<Vec<_>>(),
        vec![(id, 0.02)],
        "redo restores the edit"
    );
}

/// The FCF blocks of a registered drawing, as `(tolerance_text, datums)`.
async fn sheet_frames(state: &AppState, drawing: &str) -> Vec<(String, Vec<String>)> {
    let (s, sheet) = dispatch(state, get(&format!("/api/drawings/{drawing}"))).await;
    assert_eq!(s, StatusCode::OK, "GET drawing; body = {sheet}");
    sheet["fcf_blocks"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|b| {
                    (
                        b["tolerance_text"].as_str().unwrap_or_default().to_string(),
                        b["datum_labels"]
                            .as_array()
                            .map(|d| {
                                d.iter()
                                    .map(|x| x.as_str().unwrap_or_default().to_string())
                                    .collect()
                            })
                            .unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn export_svg(state: &AppState, drawing: &str) -> String {
    let (s, body) = dispatch(state, get(&format!("/api/drawings/{drawing}/svg"))).await;
    assert_eq!(s, StatusCode::OK, "svg export; body = {body}");
    body.to_string()
}

#[tokio::test]
async fn an_edited_tolerance_reaches_the_sheet_and_the_old_sheet_is_stale() {
    let state = make_test_state().await;
    let (part, _) = make_box(&state, [0.0, 0.0, 0.0], 10.0, 10.0, 10.0).await;
    let id = datum_and_frame(&state, part, 0.05).await;
    let (s, created) = dispatch(
        &state,
        post(&format!("/api/parts/{part}/drawing"), json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "drawing; body = {created}");
    let drawing = created["id"].as_str().expect("drawing id").to_string();
    assert_eq!(
        sheet_frames(&state, &drawing).await,
        vec![("0.05".to_string(), vec!["A".to_string()])],
        "the sheet inks the frame as authored"
    );
    // Warm the sheet cache: the export rebuilds the sheet from history.
    let _ = export_svg(&state, &drawing).await;

    let (s, body) = dispatch(
        &state,
        patch(
            &format!("/api/agent/parts/{part}/fcf/{id}"),
            json!({ "tolerance_mm": 0.02 }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "frame edit; body = {body}");

    // The sheet as it stands still says 0.05: against the edited model it is
    // stale, and says which frame.
    let (s, cert) = dispatch(&state, get(&format!("/api/drawings/{drawing}/certificate"))).await;
    assert_eq!(s, StatusCode::OK, "certificate; body = {cert}");
    let fcf_verdicts: Vec<String> = cert["facts"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|f| f["kind"] == "fcf")
                .map(|f| {
                    f["live"]["verdict"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        fcf_verdicts,
        vec!["stale".to_string()],
        "the old sheet's frame is stale against the edited tolerance; cert = {cert}"
    );
    assert_eq!(cert["sound"], false, "cert = {cert}");

    // The next export rebuilds the sheet from history: it shows 0.02.
    let _ = export_svg(&state, &drawing).await;
    assert_eq!(
        sheet_frames(&state, &drawing).await,
        vec![("0.02".to_string(), vec!["A".to_string()])],
        "the exported sheet carries the edited tolerance"
    );
    let (_, cert) = dispatch(&state, get(&format!("/api/drawings/{drawing}/certificate"))).await;
    assert_eq!(cert["sound"], true, "cert = {cert}");

    // Deleting the frame takes it off the next sheet, and the sheet that
    // still shows it is stale.
    let (s, body) = dispatch(&state, del(&format!("/api/agent/parts/{part}/fcf/{id}"))).await;
    assert_eq!(s, StatusCode::OK, "frame delete; body = {body}");
    let (_, cert) = dispatch(&state, get(&format!("/api/drawings/{drawing}/certificate"))).await;
    assert_eq!(cert["sound"], false, "cert = {cert}");
    let _ = export_svg(&state, &drawing).await;
    assert!(
        sheet_frames(&state, &drawing).await.is_empty(),
        "the deleted frame is gone from the rebuilt sheet"
    );
}

#[tokio::test]
async fn a_datum_face_consumed_by_a_later_boolean_is_dangling_on_the_new_part() {
    let state = make_test_state().await;
    let (part, part_uuid) = make_box(&state, [0.0, 0.0, 0.0], 10.0, 10.0, 10.0).await;
    let id = datum_and_frame(&state, part, 0.05).await;
    let (s, body) = dispatch(
        &state,
        post(
            &format!("/api/agent/parts/{part}/datums"),
            json!({ "label": "B", "selector": face_along(1.0) }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "datum B on the top face; body = {body}");

    // A tool wider than the part takes the top 2 mm off: the top face (datum
    // B, and the frame's feature) is consumed; the bottom face (datum A)
    // survives.
    let (_, tool_uuid) = make_box(&state, [0.0, 0.0, 8.0], 20.0, 20.0, 10.0).await;
    let (s, body) = dispatch(
        &state,
        post(
            "/api/geometry/boolean",
            json!({ "operation": "difference", "object_a": part_uuid, "object_b": tool_uuid }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "boolean; body = {body}");
    let result = body["solid_id"].as_u64().expect("result solid") as u32;
    assert_ne!(result, part, "fixture: the boolean made a new solid");

    let gdt = gdt_of(&state, result).await;
    let datums = datums_of(&gdt);
    assert_eq!(
        datums
            .iter()
            .map(|d| (d.0.clone(), d.2.clone()))
            .collect::<Vec<_>>(),
        vec![
            ("A".to_string(), "live".to_string()),
            ("B".to_string(), "dangling".to_string()),
        ],
        "the datums followed the part; B's face is gone; body = {gdt}"
    );
    let frames = gdt["annotations"].as_array().cloned().unwrap_or_default();
    assert_eq!(frames.len(), 1, "the frame followed the part; body = {gdt}");
    assert_eq!(frames[0]["annotation_id"], id.as_str());
    assert_eq!(frames[0]["status"], "dangling", "body = {gdt}");
    assert_eq!(frames[0]["verdict"]["conforms"], "not_evaluable");

    // A sheet of the new part reports both, typed, and is not sound.
    let (s, created) = dispatch(
        &state,
        post(&format!("/api/parts/{result}/drawing"), json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "drawing; body = {created}");
    let drawing = created["id"].as_str().expect("drawing id").to_string();
    let (_, cert) = dispatch(&state, get(&format!("/api/drawings/{drawing}/certificate"))).await;
    let dangling: Vec<(String, String)> = cert["facts"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|f| f["live"]["verdict"] == "dangling")
                .map(|f| {
                    (
                        f["kind"].as_str().unwrap_or_default().to_string(),
                        f["label"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    assert!(
        dangling
            .iter()
            .any(|(k, l)| k == "datum_symbol" && l == "datum B"),
        "datum B is reported dangling on the sheet; cert = {cert}"
    );
    assert!(
        dangling.iter().any(|(k, _)| k == "fcf"),
        "the frame is reported dangling on the sheet; cert = {cert}"
    );
    assert_eq!(cert["sound"], false, "cert = {cert}");
}

#[tokio::test]
async fn every_gdt_refusal_changes_nothing() {
    let state = make_test_state().await;
    let (part, _) = make_box(&state, [0.0, 0.0, 0.0], 10.0, 10.0, 10.0).await;
    let id = datum_and_frame(&state, part, 0.05).await;
    let before = gdt_of(&state, part).await;
    let events = history_len(&state).await;

    let fcf = format!("/api/agent/parts/{part}/fcf");
    let refusals: Vec<(axum::http::Request<axum::body::Body>, StatusCode, &str)> = vec![
        (
            post(
                &format!("/api/agent/parts/{part}/datums"),
                json!({ "label": "A", "selector": face_along(1.0) }),
            ),
            StatusCode::CONFLICT,
            "duplicate_label",
        ),
        (
            post(
                &fcf,
                json!({ "characteristic": "parallelism", "tolerance_mm": 0.05,
                        "datum_refs": ["C"], "selector": face_along(1.0) }),
            ),
            StatusCode::UNPROCESSABLE_ENTITY,
            "datum_label_not_in_drf",
        ),
        (
            post(
                &fcf,
                json!({ "characteristic": "flatness", "tolerance_mm": 0.0,
                        "selector": face_along(1.0) }),
            ),
            StatusCode::UNPROCESSABLE_ENTITY,
            "non_positive_tolerance",
        ),
        (
            post(
                &fcf,
                json!({ "characteristic": "flatness", "tolerance_mm": -0.1,
                        "selector": face_along(1.0) }),
            ),
            StatusCode::UNPROCESSABLE_ENTITY,
            "non_positive_tolerance",
        ),
        (
            post(
                &fcf,
                json!({ "characteristic": "flatness", "tolerance_mm": 0.05,
                        "datum_refs": ["A"], "selector": face_along(1.0) }),
            ),
            StatusCode::UNPROCESSABLE_ENTITY,
            "datum_refs_not_allowed",
        ),
        (
            post(
                &fcf,
                json!({ "characteristic": "perpendicularity", "tolerance_mm": 0.05,
                        "selector": face_along(1.0) }),
            ),
            StatusCode::UNPROCESSABLE_ENTITY,
            "datum_refs_required",
        ),
        (
            post(
                &fcf,
                json!({ "characteristic": "parallelism", "tolerance_mm": 0.05,
                        "datum_refs": ["A", "A"], "selector": face_along(1.0) }),
            ),
            StatusCode::UNPROCESSABLE_ENTITY,
            "repeated_datum_ref",
        ),
        (
            patch(&format!("{fcf}/{id}"), json!({ "tolerance_mm": 0.0 })),
            StatusCode::UNPROCESSABLE_ENTITY,
            "non_positive_tolerance",
        ),
        (
            patch(&format!("{fcf}/{id}"), json!({ "datum_refs": ["Z"] })),
            StatusCode::UNPROCESSABLE_ENTITY,
            "datum_label_not_in_drf",
        ),
        (
            patch(
                &format!("{fcf}/no-such-frame"),
                json!({ "tolerance_mm": 0.1 }),
            ),
            StatusCode::NOT_FOUND,
            "annotation_not_found",
        ),
        (
            del(&format!("{fcf}/no-such-frame")),
            StatusCode::NOT_FOUND,
            "annotation_not_found",
        ),
        (
            // Datum A is referenced by the frame: deleting it would leave
            // the frame naming a datum the part no longer has.
            del(&format!("/api/agent/parts/{part}/datums/A")),
            StatusCode::CONFLICT,
            "datum_referenced",
        ),
        (
            patch(
                &format!("/api/agent/parts/{part}/datums/A"),
                json!({ "label": "D" }),
            ),
            StatusCode::CONFLICT,
            "datum_referenced",
        ),
        (
            del(&format!("/api/agent/parts/{part}/datums/Q")),
            StatusCode::NOT_FOUND,
            "datum_not_found",
        ),
    ];
    for (request, status, code) in refusals {
        let what = format!("{} {}", request.method(), request.uri());
        let (s, body) = dispatch(&state, request).await;
        assert_eq!(s, status, "{what}: body = {body}");
        assert_eq!(body["error"], code, "{what}: body = {body}");
        assert!(
            body["message"].as_str().is_some_and(|m| !m.is_empty()),
            "{what}: a refusal says why in words; body = {body}"
        );
        let now = gdt_of(&state, part).await;
        assert_eq!(
            datums_of(&now),
            datums_of(&before),
            "{what} changed a datum"
        );
        assert_eq!(
            frames_of(&now),
            frames_of(&before),
            "{what} changed a frame"
        );
        assert_eq!(
            history_len(&state).await,
            events,
            "{what} recorded an event"
        );
    }
}
