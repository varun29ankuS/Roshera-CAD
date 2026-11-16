// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! After an operation the agent must be told WHAT CHANGED, by persistent id:
//! which faces and solids the operation created, which it consumed, which it
//! left alone. Before, it got the new solid and had to re-query and guess.

use geometry_engine::lens::changes::{changes, snapshot};
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::surface::Plane;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn sid(g: GeometryId) -> u32 {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

#[test]
fn a_bore_reports_the_faces_it_made_and_the_solids_it_consumed() {
    let mut m = BRepModel::new();
    m.set_event_key(Some("blk".into()));
    let block = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(40.0, 40.0, 20.0)
        .expect("block"));
    m.set_event_key(Some("cut".into()));
    let cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -30.0), Vector3::Z, 5.0, 60.0)
        .expect("cyl"));
    m.set_event_key(None);
    // The block's +X side: a central Z bore does not touch it.
    let side = m
        .faces
        .iter()
        .find(|(_, f)| {
            m.surfaces
                .get(f.surface_id)
                .and_then(|s| s.as_any().downcast_ref::<Plane>())
                .is_some_and(|p| p.normal.x > 0.99 && (p.origin.x - 20.0).abs() < 1e-6)
        })
        .map(|(id, _)| id)
        .expect("+X side");
    let side_pid = m.face_pid(side).expect("side pid");
    let (block_pid, cyl_pid) = (
        m.solid_pid(block).expect("pid"),
        m.solid_pid(cyl).expect("pid"),
    );

    let before = snapshot(&m);
    let result = boolean_operation(
        &mut m,
        block,
        cyl,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("difference");
    let after = snapshot(&m);
    let delta = changes(Some(&before), &after);

    assert!(delta.baseline, "a before-snapshot was given");
    // The cutter is consumed; the block carries on as the same body (its
    // persistent id survives, as an untouched face's does) and is reported
    // as modified, not as deleted-and-recreated.
    assert_eq!(
        delta
            .solids_deleted
            .iter()
            .map(|s| s.pid)
            .collect::<Vec<_>>(),
        vec![cyl_pid],
        "the cutter was consumed"
    );
    assert!(
        delta.solids_created.is_empty(),
        "no new body: {:?}",
        delta.solids_created
    );
    let result_pid = m.solid_pid(result).expect("result pid");
    assert_eq!(result_pid, block_pid, "the result is the block, still");
    assert_eq!(
        delta
            .solids_modified
            .iter()
            .map(|s| s.pid)
            .collect::<Vec<_>>(),
        vec![block_pid],
        "the block was modified"
    );
    assert!(
        delta
            .faces_created
            .iter()
            .any(|f| f.surface == "Cylinder" && f.solid == result),
        "the bore wall is a created face: {:?}",
        delta.faces_created
    );
    assert!(
        delta.faces_deleted.iter().all(|f| f.pid != side_pid),
        "the untouched side was not deleted"
    );
    assert!(
        delta.faces_created.iter().all(|f| f.pid != side_pid)
            && delta.faces_modified.iter().all(|f| f.pid != side_pid),
        "the untouched side is neither created nor modified"
    );
    // Nothing happened: nothing is reported.
    let again = changes(Some(&after), &snapshot(&m));
    assert!(again.is_empty(), "no operation, no change: {again:?}");
}

#[test]
fn without_a_baseline_the_change_set_says_so_instead_of_claiming_everything_is_new() {
    let mut m = BRepModel::new();
    TopologyBuilder::new(&mut m)
        .create_box_3d(10.0, 10.0, 10.0)
        .expect("box");
    let delta = changes(None, &snapshot(&m));
    assert!(!delta.baseline);
    assert!(
        delta.faces_created.is_empty() && delta.solids_created.is_empty(),
        "no baseline, no claims: {delta:?}"
    );
}

/// A fillet on one box edge makes one roll face, trims the two faces it joins
/// and rounds a corner of each end face: those four keep their persistent ids
/// (an agent's "the top face" is still the top face) and are reported as
/// MODIFIED, the roll face as CREATED. The two faces away from the edge are
/// untouched and not reported.
#[test]
fn a_fillet_reports_its_roll_face_as_new_and_the_trimmed_faces_as_modified() {
    use geometry_engine::operations::fillet::{FilletType, PropagationMode};
    use geometry_engine::operations::{fillet_edges, FilletOptions};
    let mut m = BRepModel::new();
    m.set_event_key(Some("box".into()));
    let solid = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(10.0, 10.0, 10.0)
        .expect("box"));
    let edge = m.edges.iter().map(|(id, _)| id).next().expect("an edge");
    let before = snapshot(&m);
    let roll = fillet_edges(
        &mut m,
        solid,
        vec![edge],
        FilletOptions {
            fillet_type: FilletType::Constant(2.0),
            radius: 2.0,
            propagation: PropagationMode::None,
            ..Default::default()
        },
    )
    .expect("fillet");
    let delta = changes(Some(&before), &snapshot(&m));
    let roll_pid = m.face_pid(roll[0]).expect("roll pid");
    assert_eq!(
        delta
            .faces_created
            .iter()
            .map(|f| f.pid)
            .collect::<Vec<_>>(),
        vec![roll_pid],
        "the roll face is the one new face: {delta:?}"
    );
    assert_eq!(
        delta.faces_modified.len(),
        4,
        "the two joined faces and the two end faces changed: {delta:?}"
    );
    assert!(
        delta.faces_modified.iter().all(|f| f.surface == "Plane"),
        "the modified faces are the box's planes: {delta:?}"
    );
    assert!(delta.faces_deleted.is_empty(), "nothing deleted: {delta:?}");
}
