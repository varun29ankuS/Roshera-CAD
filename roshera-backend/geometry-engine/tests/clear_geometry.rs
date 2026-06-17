//! clear_geometry gate (#10).
//!
//! Deleting solids one-by-one leaves ORPHANED entities behind when an upstream
//! op materialised geometry then failed (e.g. a sketch lifted into edges/curves
//! followed by a revolve that failed validation — the revolve's own
//! with_rollback undoes ITS additions, but the pre-lifted sketch entities
//! linger). Those orphans poison later validation with phantom connectivity
//! errors, which is why `clear_parts` (delete solids) used to need a full
//! `clear_timeline` to recover. `BRepModel::clear_geometry` sweeps them so
//! "clear" is a true reset — while preserving the seeded datums.
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::primitives::curve::{Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::Plane;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

/// Add orphan entities that mimic a failed op's leak: lifted sketch
/// vertices/curve/edge + a stray surface, none folded into a solid.
fn add_orphans(m: &mut BRepModel) {
    let v0 = m.vertices.add(1.0, 2.0, 3.0);
    let v1 = m.vertices.add(4.0, 5.0, 6.0);
    let cid = m.curves.add(Box::new(Line::new(
        Point3::new(1.0, 2.0, 3.0),
        Point3::new(4.0, 5.0, 6.0),
    )));
    m.edges.add(Edge::new(
        0,
        v0,
        v1,
        cid,
        EdgeOrientation::Forward,
        ParameterRange::new(0.0, 1.0),
    ));
    m.surfaces.add(Box::new(
        Plane::from_point_normal(Point3::ZERO, Vector3::Z).expect("plane"),
    ));
}

#[test]
fn clear_geometry_sweeps_orphans_keeps_datums() {
    let mut m = BRepModel::new();
    let datums_before = m.datums.len();
    assert!(datums_before >= 7, "model seeds the canonical datums");

    let _b = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(10.0, 10.0, 10.0)
        .expect("box"));
    add_orphans(&mut m);
    assert!(m.solids.len() == 1 && m.vertices.len() > 0 && m.edges.len() > 0);

    m.clear_geometry();

    assert_eq!(m.solids.len(), 0, "solids swept");
    assert_eq!(m.vertices.len(), 0, "vertices swept");
    assert_eq!(m.edges.len(), 0, "edges swept");
    assert_eq!(m.curves.len(), 0, "curves swept");
    assert_eq!(m.surfaces.len(), 0, "surfaces swept");
    assert_eq!(m.faces.len(), 0, "faces swept");
    assert_eq!(m.loops.len(), 0, "loops swept");
    assert_eq!(m.shells.len(), 0, "shells swept");
    assert_eq!(m.datums.len(), datums_before, "datums preserved");
}

#[test]
fn op_after_clear_is_not_poisoned() {
    let mut m = BRepModel::new();
    // A solid + orphans, then clear, then a FRESH op — it must validate clean
    // (the orphans must not surface as phantom connectivity errors).
    let _ = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(5.0, 5.0, 5.0)
        .expect("box1"));
    add_orphans(&mut m);
    m.clear_geometry();

    let b = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(8.0, 8.0, 8.0)
        .expect("box2"));
    let v = validate_solid_scoped(&m, b, Tolerance::default(), ValidationLevel::Standard);
    assert!(v.is_valid, "post-clear op must be clean: {:?}", v.errors);
    assert_eq!(m.solids.len(), 1, "only the fresh solid exists");
}

#[test]
fn clear_geometry_is_idempotent_on_empty() {
    let mut m = BRepModel::new();
    let d = m.datums.len();
    m.clear_geometry();
    m.clear_geometry();
    assert_eq!(m.solids.len(), 0);
    assert_eq!(m.vertices.len(), 0);
    assert_eq!(
        m.datums.len(),
        d,
        "datums still seeded after repeated clears"
    );
    // and a build still works afterward
    let b = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(3.0, 3.0, 3.0)
        .expect("box"));
    assert!(validate_solid_scoped(&m, b, Tolerance::default(), ValidationLevel::Standard).is_valid);
}
