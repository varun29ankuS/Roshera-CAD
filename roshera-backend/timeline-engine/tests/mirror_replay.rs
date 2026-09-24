// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A mirror is one recorded operation, and it replays the right way out.
//!
//! The kernel `mirror` is a reflection (determinant −1) followed by a per-face
//! orientation restore. It used to record only the inner `transform_solid`
//! event (the bare reflection matrix, certified in its pre-restore state), so
//! replay re-ran a different operation from the live one. Measured at BASE the
//! two disagreed in both directions: the live mirror flipped every face flag
//! and returned a planar block inside-out (signed volume −1048), while the
//! replayed bare reflection kept the faces outward but left every planar loop
//! wound clockwise about its own plane.
//!
//! These tests drive the production path end to end: a real `BRepModel` with a
//! `TimelineRecorder` attached, the recorded events collected from the
//! timeline, then `rebuild_model_from_events` into a fresh model (the path
//! server boot, undo/redo and scrub all use).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::recorder::OperationRecorder;
use geometry_engine::operations::transform::{mirror, transform_solid, TransformOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
use timeline_engine::{
    rebuild_model_from_events, Author, BranchId, Operation, Timeline, TimelineConfig,
    TimelineEvent, TimelineRecorder,
};
use tokio::sync::RwLock;

async fn drain_to_at_least(
    timeline: &Arc<RwLock<Timeline>>,
    expected: usize,
) -> Vec<TimelineEvent> {
    for _ in 0..400 {
        let count = timeline
            .read()
            .await
            .get_branch_events(&BranchId::main(), None, None)
            .map(|v| v.len())
            .unwrap_or(0);
        if count >= expected {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    timeline
        .read()
        .await
        .get_branch_events(&BranchId::main(), None, None)
        .expect("branch events readable")
}

fn kind_of(event: &TimelineEvent) -> String {
    match &event.operation {
        Operation::Generic { command_type, .. } => command_type.clone(),
        other => format!("{other:?}"),
    }
}

fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    match TopologyBuilder::new(model).create_box_3d(w, h, d) {
        Ok(GeometryId::Solid(id)) => id,
        other => panic!("expected a solid; got {other:?}"),
    }
}

/// A 10³ block with a 4³ boss sticking out of its +x face — asymmetric about
/// the mirror plane used below, so a reflection that did not happen (or
/// happened twice) is visible in the vertex set.
fn block_with_boss(model: &mut BRepModel) -> SolidId {
    let block = make_box(model, 10.0, 10.0, 10.0);
    let boss = make_box(model, 4.0, 4.0, 4.0);
    transform_solid(
        model,
        boss,
        Matrix4::from_translation(&Vector3::new(6.0, 0.0, 0.0)),
        TransformOptions::default(),
    )
    .expect("translating the boss must succeed");
    boolean_operation(
        model,
        block,
        boss,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("union of block and overlapping boss must succeed")
}

/// Signed volume enclosed by each shell of `id`, from the solid's own
/// tessellation (triangles grouped by the face they came from). Positive for a
/// shell whose faces point out of the material it bounds.
fn per_shell_signed_volume(model: &BRepModel, id: SolidId) -> Vec<f64> {
    let solid = model.solids.get(id).expect("solid exists");
    let mesh = tessellate_solid(&solid, model, &TessellationParams::default());
    solid
        .all_shells()
        .into_iter()
        .map(|sh| {
            let faces: HashSet<u32> = model
                .shells
                .get(sh)
                .expect("shell exists")
                .faces
                .iter()
                .copied()
                .collect();
            let mut six = 0.0;
            for (t, tri) in mesh.triangles.iter().enumerate() {
                if !faces.contains(&mesh.face_map[t]) {
                    continue;
                }
                let p0 = mesh.vertices[tri[0] as usize].position.to_vec();
                let p1 = mesh.vertices[tri[1] as usize].position.to_vec();
                let p2 = mesh.vertices[tri[2] as usize].position.to_vec();
                six += p0.dot(&p1.cross(&p2));
            }
            six / 6.0
        })
        .collect()
}

/// Sorted, rounded vertex positions reachable from the solid's faces.
fn solid_points(model: &BRepModel, id: SolidId) -> Vec<[i64; 3]> {
    let solid = model.solids.get(id).expect("solid exists");
    let mut vids = std::collections::BTreeSet::new();
    for sh in solid.all_shells() {
        for &fid in &model.shells.get(sh).expect("shell").faces {
            let face = model.faces.get(fid).expect("face");
            let mut loops = vec![face.outer_loop];
            loops.extend(face.inner_loops.iter().copied());
            for lid in loops {
                for &eid in &model.loops.get(lid).expect("loop").edges {
                    let e = model.edges.get(eid).expect("edge");
                    vids.insert(e.start_vertex);
                    vids.insert(e.end_vertex);
                }
            }
        }
    }
    let mut pts: Vec<[i64; 3]> = vids
        .into_iter()
        .map(|v| {
            let p = model.vertices.get_position(v).expect("vertex");
            [
                (p[0] * 1e6).round() as i64,
                (p[1] * 1e6).round() as i64,
                (p[2] * 1e6).round() as i64,
            ]
        })
        .collect();
    pts.sort_unstable();
    pts
}

/// Planar faces whose outer loop's STORED walk is not counter-clockwise about
/// the plane's own normal — the kernel's loop convention (see
/// `geometry-engine/tests/coedge_orientation_invariant.rs`). A bare reflection
/// leaves every such loop wound backwards even where the face still points out.
fn planar_loops_wound_backwards(model: &BRepModel, id: SolidId) -> Vec<u32> {
    use geometry_engine::math::Tolerance;
    use geometry_engine::primitives::surface::SurfaceType;
    let solid = model.solids.get(id).expect("solid exists");
    let mut bad = Vec::new();
    for sh in solid.all_shells() {
        for &face in &model.shells.get(sh).expect("shell").faces {
            let f = model.faces.get(face).expect("face");
            let surface = model.surfaces.get(f.surface_id).expect("surface");
            if surface.surface_type() != SurfaceType::Plane {
                continue;
            }
            let lp = model.loops.get(f.outer_loop).expect("loop");
            let mut pts = Vec::new();
            for (i, &e) in lp.edges.iter().enumerate() {
                let edge = model.edges.get(e).expect("edge");
                let v = if lp.orientations[i] {
                    edge.start_vertex
                } else {
                    edge.end_vertex
                };
                let p = model.vertices.get_position(v).expect("vertex");
                pts.push(Vector3::new(p[0], p[1], p[2]));
            }
            if pts.len() < 3 {
                continue;
            }
            let mut newell = Vector3::ZERO;
            for i in 0..pts.len() {
                let a = pts[i];
                let b = pts[(i + 1) % pts.len()];
                newell = newell
                    + Vector3::new(
                        (a.y - b.y) * (a.z + b.z),
                        (a.z - b.z) * (a.x + b.x),
                        (a.x - b.x) * (a.y + b.y),
                    );
            }
            let p0 = Point3::new(pts[0].x, pts[0].y, pts[0].z);
            let (u, v) = surface
                .closest_point(&p0, Tolerance::default())
                .expect("closest point");
            let n = surface.normal_at(u, v).expect("normal");
            if newell.dot(&n) <= 0.0 {
                bad.push(face);
            }
        }
    }
    bad
}

fn resolve(outcome_remap: &std::collections::HashMap<u64, u64>, live: SolidId) -> SolidId {
    outcome_remap
        .get(&(live as u64))
        .map(|&r| r as SolidId)
        .unwrap_or(live)
}

/// Record `build` against a fresh model with a `TimelineRecorder` attached;
/// return the live model, the solid `build` returned, and the recorded events.
async fn record<F>(expected_events: usize, build: F) -> (BRepModel, SolidId, Vec<TimelineEvent>)
where
    F: FnOnce(&mut BRepModel) -> SolidId,
{
    let timeline: Arc<RwLock<Timeline>> =
        Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
    let recorder = TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());
    let recorder_arc: Arc<dyn OperationRecorder> = Arc::new(recorder);
    let mut model = BRepModel::new();
    let _ = model.attach_recorder(Some(Arc::clone(&recorder_arc)));
    let id = build(&mut model);
    let _ = model.attach_recorder(None);
    drop(recorder_arc);
    let events = drain_to_at_least(&timeline, expected_events).await;
    (model, id, events)
}

// ---------------------------------------------------------------------------
// (a) a recorded mirror replays orientation-correct and equal to the live solid
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recorded_mirror_replays_the_right_way_out() {
    let (mut live, id, events) = record(5, |model| {
        let id = block_with_boss(model);
        mirror(
            model,
            vec![id],
            Point3::new(20.0, 0.0, 0.0),
            Vector3::X,
            TransformOptions::default(),
        )
        .expect("mirroring a sound block must succeed");
        id
    })
    .await;

    let live_signed = per_shell_signed_volume(&live, id);
    assert!(
        live_signed.iter().all(|&v| v > 0.0),
        "the LIVE mirror must be outward-oriented: per-shell signed volume {live_signed:?}"
    );
    assert!(
        live.certify_solid(id).is_sound(),
        "the live mirrored block must certify sound"
    );

    let mut replayed = BRepModel::new();
    let outcome = rebuild_model_from_events(&mut replayed, &events);
    let kinds: Vec<String> = events.iter().map(kind_of).collect();
    assert_eq!(
        outcome.events_skipped, 0,
        "every recorded event must replay; kinds {kinds:?}, first failure {:?}",
        outcome.first_failure
    );
    let rid = resolve(&outcome.id_remap, id);

    let replay_signed = per_shell_signed_volume(&replayed, rid);
    assert!(
        replay_signed.iter().all(|&v| v > 0.0),
        "the REPLAYED mirror came back inside-out: per-shell signed volume {replay_signed:?} (live {live_signed:?}); events {kinds:?}"
    );
    assert_eq!(
        solid_points(&replayed, rid),
        solid_points(&live, id),
        "the replayed mirror must land where the live one did; events {kinds:?}"
    );
    assert_eq!(
        planar_loops_wound_backwards(&replayed, rid),
        Vec::<u32>::new(),
        "every replayed planar loop must run counter-clockwise about its plane; events {kinds:?}"
    );
    assert!(
        replayed.certify_solid(rid).is_sound(),
        "the replayed mirrored block must certify sound; events {kinds:?}"
    );
}

// ---------------------------------------------------------------------------
// (b) one mirror is one recorded event, of kind "mirror"
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_mirror_records_exactly_one_mirror_event() {
    let (_live, _id, events) = record(2, |model| {
        let id = make_box(model, 10.0, 6.0, 4.0);
        mirror(
            model,
            vec![id],
            Point3::new(3.0, 0.0, 0.0),
            Vector3::X,
            TransformOptions::default(),
        )
        .expect("mirroring a box must succeed");
        id
    })
    .await;
    let kinds: Vec<String> = events.iter().map(kind_of).collect();
    assert_eq!(
        kinds,
        vec!["create_box_3d".to_string(), "mirror".to_string()],
        "a box then one mirror must record exactly [create_box_3d, mirror]"
    );
}

// ---------------------------------------------------------------------------
// (c) a legacy mirror — a "transform_solid" carrying a reflection matrix —
//     replays orientation-correct
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_legacy_reflection_transform_replays_the_right_way_out() {
    // Before the fix `mirror` persisted exactly this: the bare reflection as a
    // "transform_solid" event, with the orientation fix unrecorded. Recording
    // the bare transform reproduces those persisted bytes.
    let reflection = Matrix4::mirror(Point3::new(20.0, 0.0, 0.0), Vector3::X).expect("mirror");
    let (_live, id, events) = record(5, |model| {
        let id = block_with_boss(model);
        transform_solid(model, id, reflection, TransformOptions::default())
            .expect("a bare reflection transform runs");
        id
    })
    .await;
    let kinds: Vec<String> = events.iter().map(kind_of).collect();
    assert_eq!(
        kinds.last().map(String::as_str),
        Some("transform_solid"),
        "fixture: the legacy stream ends in the reflection transform; kinds {kinds:?}"
    );

    // The reference: the same block mirrored through the kernel `mirror`.
    let mut reference = BRepModel::new();
    let ref_id = block_with_boss(&mut reference);
    mirror(
        &mut reference,
        vec![ref_id],
        Point3::new(20.0, 0.0, 0.0),
        Vector3::X,
        TransformOptions::default(),
    )
    .expect("reference mirror");

    let mut replayed = BRepModel::new();
    let outcome = rebuild_model_from_events(&mut replayed, &events);
    assert_eq!(
        outcome.events_skipped, 0,
        "every legacy event must replay; kinds {kinds:?}, first failure {:?}",
        outcome.first_failure
    );
    let rid = resolve(&outcome.id_remap, id);
    let signed = per_shell_signed_volume(&replayed, rid);
    assert!(
        signed.iter().all(|&v| v > 0.0),
        "a legacy reflection event replayed inside-out: per-shell signed volume {signed:?}"
    );
    assert_eq!(
        solid_points(&replayed, rid),
        solid_points(&reference, ref_id),
        "the legacy replay must land where the kernel mirror does"
    );
    assert_eq!(
        planar_loops_wound_backwards(&replayed, rid),
        Vec::<u32>::new(),
        "every legacy-replayed planar loop must run counter-clockwise about its plane"
    );
    assert!(
        replayed.certify_solid(rid).is_sound(),
        "the legacy-replayed mirror must certify sound"
    );
}
