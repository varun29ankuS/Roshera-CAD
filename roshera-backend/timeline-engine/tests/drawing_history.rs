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
