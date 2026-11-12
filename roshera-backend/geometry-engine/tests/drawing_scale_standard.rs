// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A sheet states its scale, and the statement is the truth: views are laid
//! out at ISO 5455 scales only, a part that fits full size on a larger sheet
//! is drawn full size, and every printed scale is the scale drawn. Found
//! live: a Ø80 hub went on A4 at 0.75 while its views and title block read
//! "1:1" (the label rounded 1:1.33 to 1:1).

use geometry_engine::drawing::render_drawing_svg;
use geometry_engine::drawing::standard_drawing_auto;
use geometry_engine::drawing::types::ProjectionType;
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

/// ISO 5455 recommended scales (enlargement, full size, reduction).
const ISO_5455: [f64; 13] = [
    50.0, 20.0, 10.0, 5.0, 2.0, 1.0, 0.5, 0.2, 0.1, 0.05, 0.02, 0.01, 0.005,
];

fn label(scale: f64) -> String {
    if scale >= 1.0 {
        format!("{}:1", scale)
    } else {
        format!("1:{}", (1.0 / scale).round())
    }
}

#[test]
fn a_sheet_states_the_scale_it_draws_at_and_uses_iso_5455_scales() {
    let mut m = BRepModel::new();
    let flange = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 40.0, 12.0)
        .expect("flange"));
    let hub = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 12.0), Vector3::Z, 22.5, 20.0)
        .expect("hub"));
    let part = boolean_operation(
        &mut m,
        flange,
        hub,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("union");

    let d = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("sheet");
    for v in &d.views {
        assert!(
            ISO_5455.iter().any(|s| (s - v.scale).abs() < 1e-9),
            "{} is drawn at {}, not an ISO 5455 scale",
            v.name,
            v.scale
        );
    }
    let front = d
        .views
        .iter()
        .find(|v| matches!(v.projection, ProjectionType::Front))
        .expect("front");
    assert_eq!(
        front.scale, 1.0,
        "an 80 mm part fits full size on a standard sheet"
    );

    // Every printed scale is the scale drawn.
    let svg = render_drawing_svg(&d);
    for v in &d.views {
        let expect = format!("{} ({})", v.name, label(v.scale));
        assert!(
            svg.contains(&expect),
            "the sheet labels {} as drawn: {expect}",
            v.name
        );
    }
}
