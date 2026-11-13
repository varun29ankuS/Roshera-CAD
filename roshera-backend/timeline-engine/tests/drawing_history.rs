// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A sheet is its history: created from a part, then renamed, its title block
//! filled in, views added and removed. Every one of those edits is recorded;
//! until now only the creation (and a projection switch) was replayed, so a
//! server restart lost every sheet and a dimension edit -- which rebuilds the
//! part and re-derives its sheets -- silently reset the title block (an
//! engineer's approval, the material) and dropped any added view.

use chrono::Utc;
use geometry_engine::drawing::types::{stable_view_id, TitleBlock};
use serde_json::json;
use timeline_engine::drawing_history::rebuild_drawings;
use timeline_engine::frames::FrameCache;
use timeline_engine::mould::mould_operation;
use timeline_engine::types::{Author, EventId, EventMetadata, Operation, TimelineEvent};
use uuid::Uuid;

fn generic(kind: &str, seq: u64, params: serde_json::Value) -> TimelineEvent {
    TimelineEvent {
        id: EventId(Uuid::new_v4()),
        sequence_number: seq,
        timestamp: Utc::now(),
        author: Author::System,
        operation: Operation::Generic {
            command_type: kind.to_string(),
            parameters: params,
        },
        inputs: Default::default(),
        outputs: Default::default(),
        metadata: EventMetadata::default(),
    }
}

fn box20(seq: u64, out: u64) -> TimelineEvent {
    generic(
        "create_box_3d",
        seq,
        json!({
            "params": { "Create3D": {
                "primitive_type": "box",
                "parameters": { "width": 20.0, "height": 20.0, "depth": 20.0 },
                "timestamp": 0
            }},
            "inputs": [], "outputs": [format!("solid:{out}")]
        }),
    )
}

fn drill(seq: u64, out: u64, radius: f64) -> TimelineEvent {
    generic(
        "create_cylinder_3d",
        seq,
        json!({
            "params": { "Create3D": {
                "primitive_type": "cylinder",
                "parameters": {
                    "base_x": 0.0, "base_y": 0.0, "base_z": -20.0,
                    "axis_x": 0.0, "axis_y": 0.0, "axis_z": 1.0,
                    "radius": radius, "height": 40.0
                },
                "timestamp": 0
            }},
            "inputs": [], "outputs": [format!("solid:{out}")]
        }),
    )
}

fn difference(seq: u64, a: u64, b: u64, out: u64) -> TimelineEvent {
    generic(
        "boolean_difference",
        seq,
        json!({
            "params": { "solid_a": a, "solid_b": b },
            "inputs": [format!("solid:{a}"), format!("solid:{b}")],
            "outputs": [format!("solid:{out}")]
        }),
    )
}

fn mould(target_seq: u64, param: &str, value: f64, own_seq: u64) -> TimelineEvent {
    let mut e = generic("param.mould", own_seq, json!(null));
    e.operation = mould_operation(target_seq, None, param, value);
    e
}

/// The live api-server's recorded shapes (drawing_mgr.rs).
fn from_part(seq: u64, solid: u64, did: Uuid) -> TimelineEvent {
    generic(
        "drawing.create_from_part",
        seq,
        json!({
            "params": { "solid_id": solid, "part_uuid": Uuid::nil().to_string(), "sheet_size": "A3" },
            "inputs": [format!("solid:{solid}")],
            "outputs": [format!("drawing:{did}")]
        }),
    )
}

fn title_block(seq: u64, did: Uuid, tb: &TitleBlock) -> TimelineEvent {
    generic(
        "drawing.title_block.update",
        seq,
        json!({
            "params": { "title_block": tb },
            "inputs": [format!("drawing:{did}")],
            "outputs": [format!("drawing:{did}")]
        }),
    )
}

fn rename(seq: u64, did: Uuid, name: &str) -> TimelineEvent {
    generic(
        "drawing.rename",
        seq,
        json!({
            "params": { "old_name": "x", "new_name": name },
            "inputs": [format!("drawing:{did}")],
            "outputs": [format!("drawing:{did}")]
        }),
    )
}

fn add_view(seq: u64, did: Uuid, vid: Uuid, solid: u64, name: &str, kind: &str) -> TimelineEvent {
    generic(
        "drawing.add_view",
        seq,
        json!({
            "params": {
                "name": name,
                "source": { "kind": "part", "part_id": Uuid::nil(), "solid_id": solid },
                "projection": { "kind": kind },
                "position_mm": [60.0, 40.0],
                "scale": 1.0,
                "polyline_count": 0
            },
            "inputs": [format!("drawing:{did}")],
            "outputs": [format!("view:{vid}")]
        }),
    )
}

fn remove_view(seq: u64, did: Uuid, vid: Uuid) -> TimelineEvent {
    generic(
        "drawing.remove_view",
        seq,
        json!({
            "params": { "view_id": vid },
            "inputs": [format!("drawing:{did}")],
            "outputs": []
        }),
    )
}

fn delete(seq: u64, did: Uuid) -> TimelineEvent {
    generic(
        "drawing.delete",
        seq,
        json!({ "params": {}, "inputs": [format!("drawing:{did}")], "outputs": [] }),
    )
}

fn plate() -> Vec<TimelineEvent> {
    vec![box20(0, 1), drill(1, 2, 3.0), difference(2, 1, 2, 3)]
}

#[test]
fn sheet_edits_survive_a_rebuild_and_a_dimension_change() {
    let did = Uuid::new_v4();
    let left = Uuid::new_v4();
    let tb = TitleBlock {
        material: "S275".into(),
        approved_by: "V. Sharma".into(),
        ..TitleBlock::default()
    };
    let mut events = plate();
    events.extend([
        from_part(3, 3, did),
        title_block(4, did, &tb),
        rename(5, did, "Bored plate"),
        add_view(6, did, left, 3, "LEFT", "left"),
        mould(1, "radius", 4.0, 7),
    ]);
    let built = rebuild_drawings(&events, &FrameCache::new(64));
    let d = built
        .drawings
        .get(&did)
        .expect("the sheet is rebuilt from its history");
    assert_eq!(d.name, "Bored plate");
    assert_eq!(d.title_block.material, "S275");
    assert_eq!(d.title_block.approved_by, "V. Sharma");
    assert!(
        d.views.iter().any(|v| v.id.0 == left && v.name == "LEFT"),
        "the added view keeps its identity"
    );
    // The sheet follows the part: the bore reads the moulded Ø8.
    let reads_8 = d
        .hole_sites
        .iter()
        .any(|h| (h.diameter_mm - 8.0).abs() < 1e-6)
        || d.views
            .iter()
            .flat_map(|v| &v.dimensions)
            .any(|dm| (dm.value - 8.0).abs() < 1e-6);
    assert!(reads_8, "the rebuilt sheet shows the edited bore");
}

#[test]
fn a_removed_standard_view_stays_removed() {
    let did = Uuid::new_v4();
    let mut events = plate();
    events.extend([
        from_part(3, 3, did),
        remove_view(4, did, stable_view_id(did, "ISOMETRIC")),
    ]);
    let built = rebuild_drawings(&events, &FrameCache::new(64));
    let d = built.drawings.get(&did).expect("rebuilt");
    assert!(
        !d.views.iter().any(|v| v.name == "ISOMETRIC"),
        "views: {:?}",
        d.views.iter().map(|v| &v.name).collect::<Vec<_>>()
    );
    assert!(
        d.views.iter().any(|v| v.name == "FRONT"),
        "the other views remain"
    );
}

#[test]
fn a_deleted_drawing_stays_deleted() {
    let did = Uuid::new_v4();
    let mut events = plate();
    events.extend([from_part(3, 3, did), delete(4, did)]);
    let built = rebuild_drawings(&events, &FrameCache::new(64));
    assert!(built.drawings.get(&did).is_none());
}

/// The name a sheet was created with comes back with it. The creation event
/// did not record it, so every rebuilt sheet came back as "Auto Drawing".
#[test]
fn a_rebuilt_sheet_keeps_the_name_it_was_created_with() {
    let did = Uuid::new_v4();
    let mut created = from_part(3, 3, did);
    if let Operation::Generic { parameters, .. } = &mut created.operation {
        parameters["params"]["name"] = json!("Mounting plate 120 x 80 x 15");
    }
    let mut events = plate();
    events.push(created);
    let built = rebuild_drawings(&events, &FrameCache::new(64));
    assert_eq!(
        built.drawings.get(&did).expect("rebuilt").name,
        "Mounting plate 120 x 80 x 15"
    );
}

fn view_update(seq: u64, did: Uuid, vid: Uuid, change: serde_json::Value) -> TimelineEvent {
    let mut params = change;
    params["view_id"] = json!(vid);
    generic(
        "drawing.view.update",
        seq,
        json!({
            "params": params,
            "inputs": [format!("drawing:{did}")],
            "outputs": []
        }),
    )
}

/// A view's display edits (hidden lines off, renamed, rescaled) are history:
/// a rebuild -- restart, undo, a dimension edit -- keeps them.
#[test]
fn a_view_edit_survives_a_rebuild() {
    let did = Uuid::new_v4();
    let front = stable_view_id(did, "FRONT");
    let top = stable_view_id(did, "TOP");
    let mut events = plate();
    events.extend([
        from_part(3, 3, did),
        view_update(4, did, front, json!({ "show_hidden": false })),
        view_update(5, did, top, json!({ "name": "PLAN", "scale": 0.5 })),
        mould(1, "radius", 4.0, 6),
    ]);
    let built = rebuild_drawings(&events, &FrameCache::new(64));
    assert!(built.issues.is_empty(), "{:?}", built.issues);
    let d = built.drawings.get(&did).expect("rebuilt");
    let f = d.views.iter().find(|v| v.id.0 == front).expect("FRONT");
    assert!(!f.show_hidden, "FRONT keeps hidden lines off");
    let t = d
        .views
        .iter()
        .find(|v| v.id.0 == top)
        .expect("the renamed view");
    assert_eq!(t.name, "PLAN");
    assert!((t.scale - 0.5).abs() < 1e-12);
    assert!(
        d.views
            .iter()
            .filter(|v| v.id.0 != front)
            .all(|v| v.show_hidden),
        "only FRONT changed"
    );
}

/// Undo, redo and every sheet edit rebuild the sheets from history. The
/// derived sheet (hidden-line projection, dimensions) depends only on the
/// geometry it was derived from, so an edit-only change (a view edit, a
/// rename) reuses it and re-applies the cheap edits -- 5.5 s of a 6.5 s undo
/// was re-deriving unchanged sheets. A geometry change (a dimension edit)
/// derives it again. Reused or derived, the sheet is the same.
#[test]
fn an_edit_only_change_reuses_the_derived_sheet() {
    let frames = FrameCache::new(64);
    let did = Uuid::new_v4();
    let left = Uuid::new_v4();
    let mut events = plate();
    events.extend([
        from_part(3, 3, did),
        add_view(4, did, left, 3, "LEFT", "left"),
    ]);
    let first = rebuild_drawings(&events, &frames);
    assert_eq!(
        (first.stats.sheets_derived, first.stats.views_projected),
        (1, 1)
    );

    events.push(view_update(
        5,
        did,
        stable_view_id(did, "FRONT"),
        json!({ "show_hidden": false }),
    ));
    let second = rebuild_drawings(&events, &frames);
    assert_eq!(
        (second.stats.sheets_derived, second.stats.views_projected),
        (0, 0),
        "nothing re-derived for an edit"
    );
    assert_eq!(
        (second.stats.sheets_reused, second.stats.views_reused),
        (1, 1)
    );
    let fresh = rebuild_drawings(&events, &FrameCache::new(64));
    let a = serde_json::to_value(second.drawings.get(&did).expect("sheet")).expect("json");
    let b = serde_json::to_value(fresh.drawings.get(&did).expect("sheet")).expect("json");
    assert_eq!(
        first_difference(&a, &b, String::new()),
        None,
        "a reused sheet is the sheet a fresh rebuild makes"
    );

    // A cached sheet is never edited in place: rebuilding again still
    // starts from the derived sheet, not from the edited one.
    events.pop();
    let back = rebuild_drawings(&events, &frames);
    assert!(
        back.drawings
            .get(&did)
            .expect("sheet")
            .views
            .iter()
            .all(|v| v.show_hidden),
        "undoing the edit shows hidden lines again"
    );

    events.push(mould(1, "radius", 4.0, 5));
    let third = rebuild_drawings(&events, &frames);
    assert_eq!(
        (third.stats.sheets_derived, third.stats.views_projected),
        (1, 1),
        "a geometry change derives the sheet again"
    );
}

/// The JSON path of the first place two values differ, with both sides.
fn first_difference(a: &serde_json::Value, b: &serde_json::Value, path: String) -> Option<String> {
    use serde_json::Value;
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for k in x.keys().chain(y.keys()) {
                let (va, vb) = (
                    x.get(k).unwrap_or(&Value::Null),
                    y.get(k).unwrap_or(&Value::Null),
                );
                if let Some(d) = first_difference(va, vb, format!("{path}.{k}")) {
                    return Some(d);
                }
            }
            None
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => x
            .iter()
            .zip(y)
            .enumerate()
            .find_map(|(i, (va, vb))| first_difference(va, vb, format!("{path}[{i}]"))),
        _ if a == b => None,
        _ => {
            let show = |v: &Value| v.to_string().chars().take(120).collect::<String>();
            Some(format!("{path}: {} vs {}", show(a), show(b)))
        }
    }
}

/// One history, rebuilt twice from scratch, gives the same sheet bit for bit
/// -- the sheet digest a certificate states must not change between runs.
/// An added view's line work came out in the model's (run-dependent) edge
/// order.
#[test]
fn two_rebuilds_of_one_history_are_identical() {
    let did = Uuid::new_v4();
    let left = Uuid::new_v4();
    let mut events = plate();
    events.extend([
        from_part(3, 3, did),
        add_view(4, did, left, 3, "LEFT", "left"),
    ]);
    let a = rebuild_drawings(&events, &FrameCache::new(64));
    let b = rebuild_drawings(&events, &FrameCache::new(64));
    let a = serde_json::to_value(a.drawings.get(&did).expect("a")).expect("json");
    let b = serde_json::to_value(b.drawings.get(&did).expect("b")).expect("json");
    assert_eq!(first_difference(&a, &b, String::new()), None);
}

/// A dimension edit first certifies the edited history (which derives its
/// sheets) and then rebuilds the live sheets from that same history. The
/// second must reuse the first's sheets, not derive them all again -- a
/// dimension edit paid for every sheet twice.
#[test]
fn certifying_a_history_leaves_its_sheets_for_the_rebuild() {
    let frames = FrameCache::new(64);
    let did = Uuid::new_v4();
    let mut events = plate();
    events.extend([from_part(3, 3, did), mould(1, "radius", 4.0, 4)]);
    let (_model, cert) = timeline_engine::rebuild_certificate::certify_rebuild_with_frames(
        &events,
        Some(1),
        &frames,
    );
    assert!(cert.is_sound());
    let rebuilt = rebuild_drawings(&events, &frames);
    assert_eq!(
        (rebuilt.stats.sheets_derived, rebuilt.stats.sheets_reused),
        (0, 1),
        "the certified sheet is reused"
    );
    let fresh = rebuild_drawings(&events, &FrameCache::new(64));
    let a = serde_json::to_value(rebuilt.drawings.get(&did).expect("sheet")).expect("json");
    let b = serde_json::to_value(fresh.drawings.get(&did).expect("sheet")).expect("json");
    assert_eq!(
        first_difference(&a, &b, String::new()),
        None,
        "and it is the same sheet"
    );
}

/// Certifying through the frame cache states exactly the certificate the full
/// from-scratch replay states -- every per-feature verdict, the dirty set,
/// soundness -- cold or warm, sound history or broken.
#[test]
fn the_frame_certificate_is_the_full_replay_certificate() {
    use timeline_engine::rebuild_certificate::{certify_rebuild, certify_rebuild_with_frames};
    let did = Uuid::new_v4();
    let mut sound = plate();
    sound.extend([
        from_part(3, 3, did),
        rename(4, did, "Plate"),
        mould(1, "radius", 4.0, 5),
    ]);
    // Broken: the difference names a solid that was never made, and a sheet
    // of the (missing) result follows.
    let broken = vec![
        box20(0, 1),
        drill(1, 2, 3.0),
        difference(2, 1, 99, 3),
        from_part(3, 3, did),
        mould(1, "radius", 2.5, 4),
    ];
    for (name, events) in [("sound", &sound), ("broken", &broken)] {
        let (full_model, full) = certify_rebuild(events, Some(1));
        let frames = FrameCache::new(64);
        for pass in ["cold", "warm"] {
            let (model, cert) = certify_rebuild_with_frames(events, Some(1), &frames);
            assert_eq!(
                serde_json::to_value(&cert).expect("json"),
                serde_json::to_value(&full).expect("json"),
                "{name}/{pass}: same certificate"
            );
            assert_eq!(model.solids.len(), full_model.solids.len(), "{name}/{pass}");
        }
        // The live reconcile replays the same history through the same
        // frames: everything certification shot is reused.
        let (_m, _o, stats) = frames.replay(events);
        assert_eq!(
            stats.events_reused,
            events.len(),
            "{name}: the reconcile re-executes nothing"
        );
    }
}

fn drill_at(seq: u64, out: u64, x: f64, y: f64, radius: f64) -> TimelineEvent {
    generic(
        "create_cylinder_3d",
        seq,
        json!({
            "params": { "Create3D": {
                "primitive_type": "cylinder",
                "parameters": {
                    "base_x": x, "base_y": y, "base_z": -20.0,
                    "axis_x": 0.0, "axis_y": 0.0, "axis_z": 1.0,
                    "radius": radius, "height": 40.0
                },
                "timestamp": 0
            }},
            "inputs": [], "outputs": [format!("solid:{out}")]
        }),
    )
}

/// A drawing follows its part to the part's CURRENT state. Found live
/// 2026-11-12: fillets and chamfers added after a hub's drawing never reached
/// the sheet (it was re-derived from the part as it stood when the sheet was
/// made), so the sheet stayed stale and the export refused it for good. Here
/// a second hole is cut after the drawing: the rebuilt sheet shows both.
#[test]
fn a_drawing_follows_its_part_through_later_features() {
    let did = Uuid::new_v4();
    let mut events = plate();
    events.extend([
        from_part(3, 3, did),
        drill_at(4, 4, 5.0, 5.0, 2.0),
        difference(5, 3, 4, 5),
    ]);
    let built = rebuild_drawings(&events, &FrameCache::new(64));
    assert!(built.issues.is_empty(), "{:?}", built.issues);
    let d = built.drawings.get(&did).expect("the sheet is rebuilt");
    let diameters: Vec<f64> = d.hole_sites.iter().map(|h| h.diameter_mm).collect();
    assert_eq!(
        d.hole_sites.len(),
        2,
        "both holes are on the sheet: {diameters:?}"
    );
    assert!(
        diameters.iter().any(|v| (v - 4.0).abs() < 1e-6),
        "the later Ø4 hole: {diameters:?}"
    );

    // Certification states the same: the sheet is rebuilt, not dangling.
    let (_m, cert) = timeline_engine::rebuild_certificate::certify_rebuild(&events, None);
    let sheet = cert
        .verdicts
        .iter()
        .find(|v| v.sequence == 3)
        .expect("the sheet's verdict");
    assert!(!sheet.status.is_break(), "{:?}", sheet.status);
}

/// Every element of `items`, serialized, in sorted order: view line work
/// compared as a set (the HLR circle de-clutter groups through a hash map, so
/// its order is not part of the claim).
fn bag<T: serde::Serialize>(items: &[T]) -> Vec<String> {
    let mut out: Vec<String> = items
        .iter()
        .map(|i| serde_json::to_string(i).expect("serializes"))
        .collect();
    out.sort();
    out
}

/// A view added to a sheet is drawn the way the sheet's own views are:
/// hidden-line removed, occluded edges dashed, centre lines on its circular
/// features. It was a bare wireframe -- every edge solid, no hidden set, no
/// centre lines -- because the replay (and the live handler) projected it
/// with the plain edge projector instead of the HLR pipeline. Pinned on the
/// rebuilt sheet, where every export now reads it: an added RIGHT view of the
/// bored plate must classify its edges exactly as the sheet's own RIGHT does.
#[test]
fn an_added_view_gets_the_same_hidden_line_treatment_as_the_sheets_own() {
    let did = Uuid::new_v4();
    let side = Uuid::new_v4();
    let left = Uuid::new_v4();
    let mut events = plate();
    events.extend([
        from_part(3, 3, did),
        add_view(4, did, side, 3, "SIDE", "right"),
        add_view(5, did, left, 3, "LEFT", "left"),
    ]);
    let built = rebuild_drawings(&events, &FrameCache::new(64));
    assert!(built.issues.is_empty(), "{:?}", built.issues);
    let d = built.drawings.get(&did).expect("rebuilt");
    let own = d
        .views
        .iter()
        .find(|v| v.name == "RIGHT")
        .expect("the sheet's RIGHT");
    let added = d.views.iter().find(|v| v.id.0 == side).expect("SIDE");
    // The bore is behind the plate's side wall: hidden, dashed.
    assert!(
        !own.hidden_polylines.is_empty() && !own.centerlines.is_empty(),
        "fixture: the sheet's RIGHT shows the bore hidden, with its axis"
    );
    assert_eq!(bag(&added.polylines), bag(&own.polylines), "visible edges");
    assert_eq!(
        bag(&added.polyline_sources),
        bag(&own.polyline_sources),
        "visible edge lineage"
    );
    assert_eq!(
        bag(&added.hidden_polylines),
        bag(&own.hidden_polylines),
        "hidden edges"
    );
    assert_eq!(bag(&added.circles), bag(&own.circles), "circles");
    assert_eq!(
        bag(&added.hidden_circles),
        bag(&own.hidden_circles),
        "hidden circles"
    );
    assert_eq!(bag(&added.ellipses), bag(&own.ellipses), "ellipses");
    assert_eq!(
        bag(&added.hidden_ellipses),
        bag(&own.hidden_ellipses),
        "hidden ellipses"
    );
    assert_eq!(
        bag(&added.centerlines),
        bag(&own.centerlines),
        "centre lines"
    );
    // A projection the sheet does not have gets the same treatment.
    let lv = d.views.iter().find(|v| v.id.0 == left).expect("LEFT");
    assert!(
        !lv.hidden_polylines.is_empty(),
        "LEFT shows the bore hidden"
    );
    assert!(!lv.centerlines.is_empty(), "LEFT draws the bore's axis");
    // An added view states no dimensions of its own: the sheet's are
    // deduplicated across its views when it is made.
    assert!(added.dimensions.is_empty() && lv.dimensions.is_empty());
}

// ── Dragged views (2026-11-13) ───────────────────────────────────────────────

/// Sheet-space (y-down) centre of a view's geometry.
fn centre(
    d: &geometry_engine::drawing::types::Drawing,
    v: &geometry_engine::drawing::types::ProjectedView,
) -> [f64; 2] {
    let r = geometry_engine::drawing::layout::view_geometry_rect(v, d.sheet_size.height())
        .expect("geometry");
    [0.5 * (r.x0 + r.x1), 0.5 * (r.y0 + r.y1)]
}

fn positions(d: &geometry_engine::drawing::types::Drawing) -> Vec<(String, [f64; 2])> {
    d.views
        .iter()
        .map(|v| (v.name.clone(), v.position_mm))
        .collect()
}

/// A drag of FRONT recorded the way the live server records it: the kernel
/// resolves the drag on the sheet as it stands, and the event carries the
/// resolved place of the dragged view and of every view it carried.
fn recorded_front_drag(
    seq: u64,
    did: Uuid,
    sheet: &geometry_engine::drawing::types::Drawing,
    delta: [f64; 2],
) -> (
    TimelineEvent,
    Vec<geometry_engine::drawing::types::ViewMove>,
) {
    use geometry_engine::drawing::types::ViewEdit;
    let front = sheet
        .views
        .iter()
        .find(|v| v.name == "FRONT")
        .expect("FRONT");
    let mut live = sheet.clone();
    let moves = live
        .edit_view(
            front.id,
            &ViewEdit {
                position_mm: Some([
                    front.position_mm[0] + delta[0],
                    front.position_mm[1] + delta[1],
                ]),
                ..ViewEdit::default()
            },
        )
        .expect("the drag fits on the live sheet");
    let event = view_update(
        seq,
        did,
        front.id.0,
        json!({ "position_mm": moves[0].position_mm, "moves": moves }),
    );
    (event, moves)
}

/// A drag of FRONT moves the views projected from it in ONE recorded event;
/// a rebuild (restart, undo, a later edit) puts every view exactly where the
/// drag did, and two rebuilds agree bit for bit.
#[test]
fn a_front_drag_with_its_views_survives_a_rebuild_identically() {
    let did = Uuid::new_v4();
    let mut events = plate();
    events.push(from_part(3, 3, did));
    let derived = rebuild_drawings(&events, &FrameCache::new(64));
    let sheet = derived.drawings.get(&did).expect("derived sheet").clone();
    let (drag, moves) = recorded_front_drag(4, did, &sheet, [-10.0, -5.0]);
    assert!(
        moves.len() >= 3,
        "FRONT carries TOP and RIGHT: {} moves",
        moves.len()
    );
    events.push(drag);

    let built = rebuild_drawings(&events, &FrameCache::new(64));
    assert!(built.issues.is_empty(), "{:?}", built.issues);
    let d = built.drawings.get(&did).expect("rebuilt");
    for m in &moves {
        let v = d
            .views
            .iter()
            .find(|v| v.id.0 == m.view_id)
            .expect("moved view");
        assert_eq!(
            v.position_mm, m.position_mm,
            "{} is where the drag put it",
            v.name
        );
    }
    for v in &sheet.views {
        if !moves.iter().any(|m| m.view_id == v.id.0) {
            let now = d.views.iter().find(|w| w.id == v.id).expect("view");
            assert_eq!(now.position_mm, v.position_mm, "{} did not move", v.name);
        }
    }
    let again = rebuild_drawings(&events, &FrameCache::new(64));
    let a = serde_json::to_value(d).expect("json");
    let b = serde_json::to_value(again.drawings.get(&did).expect("again")).expect("json");
    assert_eq!(first_difference(&a, &b, String::new()), None);
}

/// Undoing the drag (dropping its event) puts every view back -- FRONT and
/// the views it carried -- in one step.
#[test]
fn undoing_a_front_drag_restores_every_view() {
    let frames = FrameCache::new(64);
    let did = Uuid::new_v4();
    let mut events = plate();
    events.push(from_part(3, 3, did));
    let derived = rebuild_drawings(&events, &frames);
    let sheet = derived.drawings.get(&did).expect("derived sheet").clone();
    let (drag, _moves) = recorded_front_drag(4, did, &sheet, [-10.0, -5.0]);
    events.push(drag);
    let moved = rebuild_drawings(&events, &frames);
    assert_ne!(
        positions(moved.drawings.get(&did).expect("moved")),
        positions(&sheet),
        "the drag moved the sheet"
    );
    events.pop();
    let undone = rebuild_drawings(&events, &frames);
    assert!(undone.issues.is_empty(), "{:?}", undone.issues);
    assert_eq!(
        positions(undone.drawings.get(&did).expect("undone")),
        positions(&sheet),
        "every view is back"
    );
}

/// A move recorded before drags were resolved (only `position_mm`, no
/// `moves`) replays exactly as it always did: the view goes where it was
/// put, nothing is carried, nothing is refused -- even a placement a live
/// drag now refuses.
#[test]
fn a_view_move_recorded_before_drags_were_resolved_replays_unchanged() {
    let did = Uuid::new_v4();
    let mut events = plate();
    events.push(from_part(3, 3, did));
    let derived = rebuild_drawings(&events, &FrameCache::new(64));
    let sheet = derived.drawings.get(&did).expect("derived sheet").clone();
    let iso = sheet
        .views
        .iter()
        .find(|v| v.name == "ISOMETRIC")
        .expect("ISOMETRIC")
        .clone();
    let front = sheet
        .views
        .iter()
        .find(|v| v.name == "FRONT")
        .expect("FRONT")
        .clone();
    let (ci, cf) = (centre(&sheet, &iso), centre(&sheet, &front));
    let onto_front = [
        iso.position_mm[0] + (cf[0] - ci[0]),
        iso.position_mm[1] - (cf[1] - ci[1]),
    ];
    let front_to = [front.position_mm[0] - 10.0, front.position_mm[1]];
    events.extend([
        view_update(4, did, iso.id.0, json!({ "position_mm": onto_front })),
        view_update(5, did, front.id.0, json!({ "position_mm": front_to })),
    ]);
    let built = rebuild_drawings(&events, &FrameCache::new(64));
    assert!(built.issues.is_empty(), "{:?}", built.issues);
    let d = built.drawings.get(&did).expect("rebuilt");
    let at = |name: &str| {
        d.views
            .iter()
            .find(|v| v.name == name)
            .expect("view")
            .position_mm
    };
    assert_eq!(at("ISOMETRIC"), onto_front, "placed exactly as recorded");
    assert_eq!(at("FRONT"), front_to);
    let top_was = sheet
        .views
        .iter()
        .find(|v| v.name == "TOP")
        .expect("TOP")
        .position_mm;
    assert_eq!(at("TOP"), top_was, "an old FRONT move carries nothing");
}

/// A drag, then a dimension edit that re-derives the sheet: the recorded
/// move is applied again to the new sheet, checked as the live drag was.
/// Measured: on this plate (Ø6 -> Ø8 bore) the move still fits and every
/// view the drag placed is where the drag put it.
#[test]
fn a_front_drag_survives_a_later_dimension_edit() {
    let did = Uuid::new_v4();
    let mut events = plate();
    events.push(from_part(3, 3, did));
    let derived = rebuild_drawings(&events, &FrameCache::new(64));
    let sheet = derived.drawings.get(&did).expect("derived sheet").clone();
    let (drag, moves) = recorded_front_drag(4, did, &sheet, [-10.0, -5.0]);
    events.push(drag);
    events.push(mould(1, "radius", 4.0, 5));
    let built = rebuild_drawings(&events, &FrameCache::new(64));
    assert!(built.issues.is_empty(), "{:?}", built.issues);
    let d = built.drawings.get(&did).expect("rebuilt");
    for m in &moves {
        let v = d
            .views
            .iter()
            .find(|v| v.id.0 == m.view_id)
            .expect("moved view");
        assert_eq!(v.position_mm, m.position_mm, "{} keeps its drag", v.name);
    }
}
