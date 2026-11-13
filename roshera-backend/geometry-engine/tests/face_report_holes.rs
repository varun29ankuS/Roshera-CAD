//! The face report of a face with a hole: its area excludes the hole, its
//! inner loops are reported, and its neighbours are the faces it shares an
//! edge with. Found live 2026-11-12: a holed plate's top face reported the
//! full square's area, no hole, and no neighbours.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::surface::Plane;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn sid(g: GeometryId) -> u32 {
    match g {
        GeometryId::Solid(s) => s,
        other => panic!("expected a solid, got {other:?}"),
    }
}

/// A 20 x 20 x 5 plate with a Ø6 through hole at its centre.
fn holed_plate() -> (BRepModel, u32) {
    let mut m = BRepModel::new();
    let plate = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(20.0, 20.0, 5.0)
        .expect("plate"));
    let tool = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -10.0), Vector3::Z, 3.0, 30.0)
        .expect("tool"));
    let part = boolean_operation(
        &mut m,
        plate,
        tool,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("drill");
    (m, part)
}

fn top_face(m: &BRepModel) -> u32 {
    m.faces
        .iter()
        .filter_map(|(id, f)| {
            let p = m
                .surfaces
                .get(f.surface_id)?
                .as_any()
                .downcast_ref::<Plane>()?;
            (p.normal.z.abs() > 1.0 - 1e-9).then_some((id, p.origin.z))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
        .expect("the plate's top face")
}

#[test]
fn a_holed_face_reports_its_net_area() {
    let (mut m, _) = holed_plate();
    let top = top_face(&m);
    let report = m.query_face(top).expect("report");
    let want = 400.0 - 9.0 * std::f64::consts::PI;
    let area = report.area.expect("area");
    assert!((area - want).abs() < 1e-3, "area {area}, expected {want}");
}

#[test]
fn a_holed_face_reports_its_hole() {
    let (mut m, _) = holed_plate();
    let top = top_face(&m);
    let report = m.query_face(top).expect("report");
    assert_eq!(report.inner_loop_edge_ids.len(), 1, "{report:?}");
    assert!(!report.inner_loop_edge_ids[0].is_empty(), "{report:?}");
}

#[test]
fn a_face_reports_the_faces_it_shares_an_edge_with() {
    let (mut m, part) = holed_plate();
    let top = top_face(&m);
    let report = m.query_face(top).expect("report");
    let mut got = report.neighbour_face_ids.clone();
    got.sort_unstable();
    // Every face of the part other than the top and the bottom.
    let solid = m.solids.get(part).expect("part");
    let shell = m.shells.get(solid.outer_shell).expect("shell");
    let bottom: Vec<u32> = shell
        .faces
        .iter()
        .copied()
        .filter(|&f| f != top)
        .filter(|&f| {
            m.faces
                .get(f)
                .and_then(|face| m.surfaces.get(face.surface_id))
                .and_then(|s| s.as_any().downcast_ref::<Plane>())
                .is_some_and(|p| p.normal.z.abs() > 1.0 - 1e-9)
        })
        .collect();
    let mut want: Vec<u32> = shell
        .faces
        .iter()
        .copied()
        .filter(|&f| f != top && !bottom.contains(&f))
        .collect();
    want.sort_unstable();
    assert_eq!(want.len(), 5, "four sides and the bore");
    assert_eq!(got, want);
}
