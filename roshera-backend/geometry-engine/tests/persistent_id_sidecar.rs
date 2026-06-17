//! Persistent-id sidecar maps on BRepModel (#11, slice 40-A).
//!
//! The PID↔entity sidecar maps must round-trip (set → get → inverse), clear with
//! `clear_geometry`, and survive `with_rollback` snapshot/restore so a failed op
//! cannot leave stale PIDs behind once operations are wired to assign them.

use geometry_engine::primitives::persistent_id::{PersistentId, PrimitiveKind, Role};
use geometry_engine::primitives::snapshot::ModelSnapshot;
use geometry_engine::primitives::topology_builder::BRepModel;

#[test]
fn sidecar_set_get_inverse_round_trip() {
    let mut m = BRepModel::new();
    let fpid = PersistentId::root(b"face-seed");
    let epid = PersistentId::derive(
        &[fpid],
        "fillet_edges",
        &Role::FilletRoll {
            source_edge_pid: fpid,
        },
    );
    let spid = PersistentId::root(b"solid-seed");

    m.set_face_pid(42, fpid);
    m.set_edge_pid(7, epid);
    m.set_solid_pid(3, spid);

    assert_eq!(m.face_pid(42), Some(fpid));
    assert_eq!(m.face_by_pid(fpid), Some(42));
    assert_eq!(m.edge_pid(7), Some(epid));
    assert_eq!(m.edge_by_pid(epid), Some(7));
    assert_eq!(m.solid_pid(3), Some(spid));
    assert_eq!(m.solid_by_pid(spid), Some(3));

    // Unassigned ids / pids resolve to None.
    assert_eq!(m.face_pid(99), None);
    assert_eq!(m.face_by_pid(PersistentId::root(b"nope")), None);
}

#[test]
fn clear_geometry_clears_pids() {
    let mut m = BRepModel::new();
    m.set_face_pid(
        1,
        PersistentId::derive(
            &[],
            "create_box_3d",
            &Role::Root {
                kind: PrimitiveKind::Box,
                key: "evt-1".into(),
            },
        ),
    );
    assert!(!m.face_pids.is_empty());
    m.clear_geometry();
    assert!(m.face_pids.is_empty(), "clear_geometry clears face pids");
    assert!(
        m.pid_to_face.is_empty(),
        "clear_geometry clears inverse map"
    );
}

#[test]
fn snapshot_rolls_back_pids() {
    let mut m = BRepModel::new();
    let kept = PersistentId::root(b"kept");
    m.set_face_pid(5, kept);

    let snap = ModelSnapshot::take(&m);
    // Mutate after the snapshot.
    m.set_face_pid(6, PersistentId::root(b"transient"));
    m.set_edge_pid(2, PersistentId::root(b"transient-edge"));
    assert_eq!(m.face_pids.len(), 2);

    snap.restore(&mut m);
    // Restore must reset to snapshot time: only the kept face pid remains.
    assert_eq!(
        m.face_pids.len(),
        1,
        "restore rolls back post-snapshot pids"
    );
    assert_eq!(m.face_pid(5), Some(kept));
    assert_eq!(m.face_by_pid(kept), Some(5));
    assert_eq!(m.face_pid(6), None, "post-snapshot face pid rolled back");
    assert_eq!(m.edge_pids.len(), 0, "post-snapshot edge pid rolled back");
}
