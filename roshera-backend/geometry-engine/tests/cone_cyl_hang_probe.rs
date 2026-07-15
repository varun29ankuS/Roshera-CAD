// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Probe: cone ∪ cylinder must TERMINATE (no hang) after the march step cap.
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
fn sid(g: GeometryId) -> geometry_engine::primitives::solid::SolidId {
    match g {
        GeometryId::Solid(id) => id,
        o => panic!("{o:?}"),
    }
}
#[test]
fn cone_union_cylinder_terminates() {
    let cases = [
        ("coaxial-frustum", 5.0, 2.0, 10.0, 0.0),
        ("coaxial-apex", 5.0, 0.0, 10.0, 0.0),
        ("wider-cone", 8.0, 0.0, 12.0, -2.0),
    ];
    for (label, br, tr, h, bz) in cases {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut m = BRepModel::new();
            let cyl = sid(TopologyBuilder::new(&mut m)
                .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 5.0, 10.0)
                .expect("cyl"));
            let cone = sid(TopologyBuilder::new(&mut m)
                .create_cone_3d(Point3::new(0.0, 0.0, bz), Vector3::Z, br, tr, h)
                .expect("cone"));
            let t = Instant::now();
            let r = boolean_operation(
                &mut m,
                cyl,
                cone,
                BooleanOp::Union,
                BooleanOptions::default(),
            );
            let _ = tx.send((r.is_ok(), t.elapsed()));
        });
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok((ok, dt)) => eprintln!("[cone-u-cyl] {label}: returned ok={ok} in {:?}", dt),
            Err(_) => panic!("[cone-u-cyl] {label}: STILL HANGS (>30s)"),
        }
    }
}
