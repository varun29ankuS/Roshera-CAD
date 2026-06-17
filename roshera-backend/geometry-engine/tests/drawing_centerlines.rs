//! Centerline drawing harness (#22).
//!
//! End-to-end: a part with circular features must carry chain-line centerlines
//! through the full `standard_drawing → render_drawing_svg` pipeline, every
//! centerline must name a real B-Rep face (recoverable), and a part with NO
//! circular features must emit none (no phantom centerlines).

use geometry_engine::drawing::types::{ProjectionType, SheetSize};
use geometry_engine::drawing::{centerlines, render_drawing_svg, standard_drawing};
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

fn real_faces(m: &BRepModel, s: SolidId) -> std::collections::HashSet<u32> {
    let solid = m.solids.get(s).expect("solid");
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    let mut out = std::collections::HashSet::new();
    for sh in shells {
        if let Some(shell) = m.shells.get(sh) {
            out.extend(shell.faces.iter().copied());
        }
    }
    out
}

#[test]
fn bored_plate_drawing_carries_recoverable_centerlines() {
    let mut m = BRepModel::new();
    let plate = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(60.0, 40.0, 16.0)
        .expect("plate"));
    let bore = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -20.0), Vector3::Z, 9.0, 80.0)
        .expect("bore"));
    let part = boolean_operation(
        &mut m,
        plate,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("bore");

    // Every view's centerlines name a real face.
    let faces = real_faces(&m, part);
    for proj in [
        ProjectionType::Front,
        ProjectionType::Top,
        ProjectionType::Right,
    ] {
        for cl in centerlines(&m, part, proj) {
            assert!(!cl.entities.is_empty(), "centerline names a face");
            for e in &cl.entities {
                assert!(faces.contains(e), "centerline face {e} is real");
            }
        }
    }

    // Top view: the bore reads end-on → a centre mark exists.
    let top = centerlines(&m, part, ProjectionType::Top);
    assert!(
        top.iter().any(|c| c.kind == "center_mark"),
        "Top view has the bore centre mark"
    );
    // Front view: the bore reads side-on → an axis chain line exists.
    let front = centerlines(&m, part, ProjectionType::Front);
    assert!(
        front.iter().any(|c| c.kind == "axis"),
        "Front view has the bore axis line"
    );

    // Full pipeline: the rendered SVG carries the chain-line class.
    let dwg = standard_drawing(&m, part, uuid::Uuid::nil(), SheetSize::A3, 1.0).expect("drawing");
    assert!(
        dwg.views.iter().any(|v| !v.centerlines.is_empty()),
        "at least one view carries centerlines"
    );
    let svg = render_drawing_svg(&dwg);
    assert!(
        svg.contains("class=\"centerline\""),
        "SVG renders chain-line centerlines"
    );
    assert!(
        svg.contains("stroke-dasharray: 4 1 1 1"),
        "centerline uses the ISO 128 dash-dot pattern"
    );
}

#[test]
fn plain_box_has_no_centerlines() {
    // A box has no circular feature → no centerlines anywhere, no phantom lines.
    let mut m = BRepModel::new();
    let b = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(40.0, 30.0, 20.0)
        .expect("box"));
    for proj in [
        ProjectionType::Front,
        ProjectionType::Top,
        ProjectionType::Right,
        ProjectionType::Isometric,
    ] {
        assert!(
            centerlines(&m, b, proj).is_empty(),
            "box has no centerlines in {proj:?}"
        );
    }
    let dwg = standard_drawing(&m, b, uuid::Uuid::nil(), SheetSize::A3, 1.0).expect("drawing");
    let svg = render_drawing_svg(&dwg);
    // The .centerline CSS class is always defined in the stylesheet, but NO
    // <line class="centerline"> element should be emitted.
    assert!(
        !svg.contains("<line class=\"centerline\""),
        "no centerline elements drawn for a box"
    );
}

#[test]
fn coaxial_counterbore_centerlines_deduped() {
    // A counterbore = two coaxial cylindrical faces. In Top view both read
    // end-on at the same axis point; the centre marks must be deduplicated to a
    // single cross, not stacked.
    let mut m = BRepModel::new();
    let plate = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(50.0, 50.0, 20.0)
        .expect("plate"));
    let through = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -30.0), Vector3::Z, 5.0, 120.0)
        .expect("through"));
    let part = boolean_operation(
        &mut m,
        plate,
        through,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("through-bore");
    let counter = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 4.0), Vector3::Z, 9.0, 30.0)
        .expect("counter"));
    let part = boolean_operation(
        &mut m,
        part,
        counter,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("counterbore");

    let top = centerlines(&m, part, ProjectionType::Top);
    let marks = top.iter().filter(|c| c.kind == "center_mark").count();
    // Both coaxial bores share the axis → at most one centre mark after dedup.
    assert!(
        marks <= 1,
        "coaxial counterbore centre marks deduped to ≤1, got {marks}"
    );
}
