// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A rigid transform moves EVERY shell of a solid, not only the outer one.
//!
//! `transform_solid` collected the entities to move from `solid.outer_shell`
//! alone, so a solid's voids (`inner_shells`) and disjoint peer bodies
//! (`peer_shells`) stayed where they were while the outer hull moved — a torn
//! solid returned as `Ok`. The same walk feeds `translate` / `rotate` /
//! `mirror`, datum anchoring and timeline replay.
//!
//! The solids here are built the way production builds them: a peer body is
//! the second lump of a union of two disjoint boxes (`reconstruct_topology`
//! files it into `peer_shells`), and a void is the cavity of a fully-closed
//! `offset_solid` hollow (filed into `inner_shells`).

use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::offset::OffsetType;
use geometry_engine::operations::{
    boolean_operation, mirror, offset_solid, transform_solid, BooleanOp, BooleanOptions,
    CommonOptions, OffsetOptions, TransformOptions,
};
use geometry_engine::primitives::shell::ShellId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    match TopologyBuilder::new(model).create_box_3d(w, h, d) {
        Ok(GeometryId::Solid(id)) => id,
        other => panic!("expected solid; got {other:?}"),
    }
}

/// Every distinct vertex position reachable from a shell's faces (all loops).
fn shell_positions(model: &BRepModel, shell_id: ShellId) -> Vec<[f64; 3]> {
    let shell = model.shells.get(shell_id).expect("shell exists");
    let mut ids = std::collections::BTreeSet::new();
    for &fid in &shell.faces {
        let face = model.faces.get(fid).expect("face exists");
        let mut loops = vec![face.outer_loop];
        loops.extend(face.inner_loops.iter().copied());
        for lid in loops {
            let lp = model.loops.get(lid).expect("loop exists");
            for &eid in &lp.edges {
                let e = model.edges.get(eid).expect("edge exists");
                ids.insert(e.start_vertex);
                ids.insert(e.end_vertex);
            }
        }
    }
    ids.into_iter()
        .map(|v| model.vertices.get_position(v).expect("vertex exists"))
        .collect()
}

fn bbox(points: &[[f64; 3]]) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

/// Every point of `expected` has a partner in `actual` (and the counts agree).
fn same_point_set(actual: &[[f64; 3]], expected: &[[f64; 3]], tol: f64) -> bool {
    actual.len() == expected.len()
        && expected.iter().all(|q| {
            actual
                .iter()
                .any(|p| (0..3).all(|k| (p[k] - q[k]).abs() <= tol))
        })
}

fn apply(m: &Matrix4, p: [f64; 3]) -> [f64; 3] {
    let q = m.transform_point(&Point3::new(p[0], p[1], p[2]));
    [q.x, q.y, q.z]
}

/// A single solid carrying two disjoint 10³ bodies: the outer shell around the
/// origin and one peer shell centred at x = 30.
fn two_body_solid(model: &mut BRepModel) -> SolidId {
    let a = make_box(model, 10.0, 10.0, 10.0);
    let b = make_box(model, 10.0, 10.0, 10.0);
    transform_solid(
        model,
        b,
        Matrix4::from_translation(&Vector3::new(30.0, 0.0, 0.0)),
        TransformOptions::default(),
    )
    .expect("translating a plain box must succeed");
    let id = boolean_operation(model, a, b, BooleanOp::Union, BooleanOptions::default())
        .expect("union of two disjoint boxes must succeed");
    let solid = model.solids.get(id).expect("union solid");
    assert_eq!(
        solid.peer_shells.len(),
        1,
        "fixture: a disjoint union must carry exactly one peer body"
    );
    assert!(
        solid.inner_shells.is_empty(),
        "fixture: a disjoint union has no voids"
    );
    id
}

/// A 10³ box hollowed to a closed 1mm wall: one outer shell, one void shell.
fn hollow_box(model: &mut BRepModel) -> SolidId {
    let solid_id = make_box(model, 10.0, 10.0, 10.0);
    let opts = OffsetOptions {
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        offset_type: OffsetType::Distance(1.0),
        ..Default::default()
    };
    let hollow = offset_solid(model, solid_id, 1.0, vec![], opts)
        .expect("fully-hollow shell of a plain box must succeed");
    let solid = model.solids.get(hollow).expect("hollow solid");
    assert_eq!(
        solid.inner_shells.len(),
        1,
        "fixture: a closed hollow box carries exactly one void shell"
    );
    hollow
}

fn shells_of(model: &BRepModel, id: SolidId) -> Vec<ShellId> {
    model.solids.get(id).expect("solid").all_shells()
}

// ---------------------------------------------------------------------------
// (a) translate a two-body solid: BOTH bodies move by exactly the vector
// ---------------------------------------------------------------------------

#[test]
fn translate_moves_the_peer_body_with_the_outer_body() {
    let mut model = BRepModel::new();
    let id = two_body_solid(&mut model);
    let shells = shells_of(&model, id);
    let before: Vec<([f64; 3], [f64; 3])> = shells
        .iter()
        .map(|&s| bbox(&shell_positions(&model, s)))
        .collect();

    transform_solid(
        &mut model,
        id,
        Matrix4::from_translation(&Vector3::new(100.0, 0.0, 0.0)),
        TransformOptions::default(),
    )
    .expect("translating a sound two-body solid must succeed");

    for (i, &s) in shells.iter().enumerate() {
        let (lo0, hi0) = before[i];
        let (lo1, hi1) = bbox(&shell_positions(&model, s));
        let role = if i == 0 { "outer" } else { "peer" };
        for k in 0..3 {
            let shift = if k == 0 { 100.0 } else { 0.0 };
            assert!(
                (lo1[k] - lo0[k] - shift).abs() < 1e-9 && (hi1[k] - hi0[k] - shift).abs() < 1e-9,
                "{role} shell {s} axis {k}: bbox {lo0:?}..{hi0:?} -> {lo1:?}..{hi1:?}, expected a shift of {shift}",
            );
        }
    }

    let cert = model.certify_solid(id);
    assert!(
        cert.is_sound(),
        "a translated two-body solid is still correct geometry: {:?}",
        cert.errors
    );
    assert_eq!(cert.peer_count, 1, "the peer body must survive the move");
    let v = model
        .calculate_solid_volume(id)
        .expect("a two-body solid has a volume");
    assert!(
        (v - 2000.0).abs() < 20.0,
        "translation preserves volume: V = {v}, expected 2000"
    );
}

// ---------------------------------------------------------------------------
// (b) rigid motion of a hollow: the void rides with the outer hull
// ---------------------------------------------------------------------------

#[test]
fn rigid_motion_carries_the_void_with_the_outer_hull() {
    let mut model = BRepModel::new();
    let id = hollow_box(&mut model);
    let shells = shells_of(&model, id);
    let before: Vec<Vec<[f64; 3]>> = shells.iter().map(|&s| shell_positions(&model, s)).collect();

    let axis = Vector3::new(1.0, 2.0, 3.0).normalize_or_zero();
    let rot = Matrix4::from_axis_angle(&axis, 0.7).expect("axis-angle");
    let m = Matrix4::from_translation(&Vector3::new(40.0, -15.0, 7.5)) * rot;

    transform_solid(&mut model, id, m, TransformOptions::default())
        .expect("a rigid motion of a sound hollow box must succeed");

    for (i, &s) in shells.iter().enumerate() {
        let expected: Vec<[f64; 3]> = before[i].iter().map(|&p| apply(&m, p)).collect();
        let actual = shell_positions(&model, s);
        let role = if i == 0 { "outer" } else { "void" };
        assert!(
            same_point_set(&actual, &expected, 1e-9),
            "{role} shell {s} did not undergo the rigid motion:\n  expected {expected:?}\n  actual   {actual:?}",
        );
    }

    // The void keeps its offset from the hull: its bbox centre is carried by
    // the same map as the hull's (both boxes are concentric at the origin).
    let (olo, ohi) = bbox(&shell_positions(&model, shells[0]));
    let (vlo, vhi) = bbox(&shell_positions(&model, shells[1]));
    for k in 0..3 {
        let oc = 0.5 * (olo[k] + ohi[k]);
        let vc = 0.5 * (vlo[k] + vhi[k]);
        assert!(
            (oc - vc).abs() < 1e-9,
            "axis {k}: void centre {vc} drifted from hull centre {oc}"
        );
    }

    let cert = model.certify_solid(id);
    assert!(
        cert.is_sound(),
        "a rigidly moved hollow box is still correct geometry: {:?}",
        cert.errors
    );
}

// ---------------------------------------------------------------------------
// (c) mirror a two-body solid: both bodies mirrored AND orientation-fixed
// ---------------------------------------------------------------------------

#[test]
fn mirror_reflects_and_reorients_the_peer_body() {
    let mut model = BRepModel::new();
    let id = two_body_solid(&mut model);
    let shells = shells_of(&model, id);
    let before: Vec<Vec<[f64; 3]>> = shells.iter().map(|&s| shell_positions(&model, s)).collect();

    // Reflect through the plane x = 50: the outer body lands near x = 100,
    // the peer near x = 70.
    let origin = Point3::new(50.0, 0.0, 0.0);
    let m = Matrix4::mirror(origin, Vector3::X).expect("mirror matrix");
    mirror(
        &mut model,
        vec![id],
        origin,
        Vector3::X,
        TransformOptions::default(),
    )
    .expect("mirroring a sound two-body solid must succeed");

    for (i, &s) in shells.iter().enumerate() {
        let expected: Vec<[f64; 3]> = before[i].iter().map(|&p| apply(&m, p)).collect();
        let actual = shell_positions(&model, s);
        let role = if i == 0 { "outer" } else { "peer" };
        assert!(
            same_point_set(&actual, &expected, 1e-9),
            "{role} shell {s} was not mirrored:\n  expected {expected:?}\n  actual   {actual:?}",
        );
    }

    let cert = model.certify_solid(id);
    assert!(
        cert.is_sound(),
        "a mirrored two-body solid must be outward-oriented and sound: {:?}",
        cert.errors
    );
    assert_eq!(cert.peer_count, 1, "the peer body must survive the mirror");
    let v = model
        .calculate_solid_volume(id)
        .expect("a two-body solid has a volume");
    assert!(
        (v - 2000.0).abs() < 20.0,
        "a mirror preserves volume and both bodies must be outward-oriented: V = {v}, expected 2000",
    );
}
