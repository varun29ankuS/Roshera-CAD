// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Whether an edge lies ON its faces is judged at the EDGE'S OWN tolerance,
//! as in every exchange kernel: a STEP file declares its model uncertainty
//! (CADGenBench parts declare 1e-5 and 2e-5 mm) and the importer stamps it
//! on each edge. The validator used one global 1e-6 instead, so edges a few
//! microns off their surfaces -- inside the file's own declared accuracy --
//! failed the solid. Edges the kernel builds keep the default tolerance and
//! are judged exactly as strictly as before.

use geometry_engine::math::{Point3, Tolerance};
use geometry_engine::primitives::curve::Line;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};

/// A 10 x 10 x 10 box with one of its edges moved `offset` off both of its
/// faces (outward, along the bisector) and given tolerance `edge_tol`.
/// Returns the validator's "off face" messages.
fn off_face_errors(offset: f64, edge_tol: f64) -> Vec<String> {
    let mut model = BRepModel::new();
    let sid = match TopologyBuilder::new(&mut model)
        .create_box_3d(10.0, 10.0, 10.0)
        .expect("box")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected a solid, got {other:?}"),
    };
    // The box's centre, from its vertices.
    let pts: Vec<[f64; 3]> = model.vertices.iter().map(|(_, v)| v.position).collect();
    let n = pts.len() as f64;
    let c = Point3::new(
        pts.iter().map(|p| p[0]).sum::<f64>() / n,
        pts.iter().map(|p| p[1]).sum::<f64>() / n,
        pts.iter().map(|p| p[2]).sum::<f64>() / n,
    );
    let (eid, a, b) = {
        let (eid, e) = model.edges.iter().next().expect("an edge");
        let pa = model.vertices.get(e.start_vertex).expect("start").position;
        let pb = model.vertices.get(e.end_vertex).expect("end").position;
        (
            eid,
            Point3::new(pa[0], pa[1], pa[2]),
            Point3::new(pb[0], pb[1], pb[2]),
        )
    };
    let along = (b - a).normalize().expect("edge direction");
    let m = a + (b - a) * 0.5;
    let out = (m - c) - along * (m - c).dot(&along);
    let out = out.normalize().expect("outward");
    let moved = model
        .curves
        .add(Box::new(Line::new(a + out * offset, b + out * offset)));
    let edge = model.edges.get_mut(eid).expect("edge");
    edge.curve_id = moved;
    edge.tolerance = edge_tol;

    validate_solid_scoped(&model, sid, Tolerance::default(), ValidationLevel::Standard)
        .errors
        .iter()
        .map(|e| format!("{e:?}"))
        .filter(|e| e.contains("off face"))
        .collect()
}

#[test]
fn an_edge_within_its_own_tolerance_is_on_its_faces() {
    // 5e-6 off, edge tolerance 1e-5 (a file declaring 1e-5 mm uncertainty).
    let errors = off_face_errors(5e-6, 1e-5);
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn an_edge_beyond_its_own_tolerance_is_still_off_its_faces() {
    // The same 5e-6 off at the kernel's default 1e-6 edge tolerance.
    let errors = off_face_errors(5e-6, 1e-6);
    assert!(
        !errors.is_empty(),
        "the check still catches an edge off its faces"
    );
}
