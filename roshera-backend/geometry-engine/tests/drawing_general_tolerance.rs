// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A general tolerance is ONE claim, stated once and answered consistently.
//!
//! The sheet printed "GENERAL TOLERANCES: LINEAR ±0.1 MM, ANGULAR ±0.5°,
//! ISO 2768-m." -- two different claims: ISO 2768-1 class m is ±0.1 only up
//! to 6 mm; it is ±0.2 over 6 up to 30, ±0.3 over 30 up to 120, ±0.5 over 120
//! up to 400 (ISO 2768-1:1989 Table 1, the same table `roshera-mcp`'s KB
//! answers from). A readback of an untoleranced Ø30 bore answered ±0.1.

use geometry_engine::drawing::dimensioning::standard_drawing_auto;
use geometry_engine::drawing::query::{answer_query, DrawingAnswer, DrawingQuery};
use geometry_engine::drawing::sheet_certificate::certify_drawing;
use geometry_engine::drawing::svg::render_drawing_svg;
use geometry_engine::drawing::types::GeneralTolerance;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::units::LengthUnit;

fn solid(g: GeometryId) -> u32 {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}

#[test]
fn iso_2768_m_is_a_table_not_a_number() {
    let t = GeneralTolerance::for_unit(LengthUnit::Millimetre);
    // ISO 2768-1:1989 Table 1, class m: "over X up to Y".
    for (nominal, pm) in [
        (0.5, 0.1),
        (6.0, 0.1),
        (6.01, 0.2),
        (30.0, 0.2),
        (30.01, 0.3),
        (120.0, 0.3),
        (400.0, 0.5),
        (1000.0, 0.8),
        (2000.0, 1.2),
        (4000.0, 2.0),
    ] {
        assert_eq!(t.linear_for(nominal), Some(pm), "class m at {nominal} mm");
    }
    // The standard's own gaps: below 0.5 mm and above 4000 mm it assigns none.
    assert_eq!(t.linear_for(0.3), None);
    assert_eq!(t.linear_for(4500.0), None);
    // Imperial carries no ISO class: one flat value for every size.
    let imp = GeneralTolerance::for_unit(LengthUnit::Inch);
    assert_eq!(imp.linear_for(3.0), Some(0.004 * 25.4));
    assert_eq!(imp.linear_for(300.0), Some(0.004 * 25.4));
}

#[test]
fn the_sheet_states_the_class_once_and_reads_back_by_size() {
    let mut m = BRepModel::new();
    let plate = solid(
        TopologyBuilder::new(&mut m)
            .create_box_3d(80.0, 60.0, 20.0)
            .unwrap(),
    );
    let bore = solid(
        TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(0.0, 0.0, -11.0), Vector3::Z, 15.0, 22.0)
            .unwrap(),
    );
    let s = boolean_operation(
        &mut m,
        plate,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .unwrap();
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");

    // The note names the class and no flat value beside it.
    assert!(
        d.tolerance_note.contains("ISO 2768-m"),
        "{}",
        d.tolerance_note
    );
    assert!(
        !d.tolerance_note.contains("\u{00B1}"),
        "a class and a flat ± value are two claims: {}",
        d.tolerance_note
    );
    let svg = render_drawing_svg(&d);
    assert!(
        !svg.contains("LINEAR \u{00B1}0.1 MM"),
        "sheet still prints the flat value"
    );

    // The untoleranced Ø30 bore reads back at the class-m value for 30 mm.
    let cert = certify_drawing(&m, &d);
    let ans = answer_query(
        &d,
        &cert,
        &DrawingQuery::TolerancedDiameter {
            tag: Some("A1".to_string()),
            face_id: None,
            pid: None,
        },
    );
    match ans {
        DrawingAnswer::TolerancedDiameter(a) => {
            assert_eq!(a.tolerance_source, "general");
            assert!((a.value - 30.0).abs() < 1e-6, "value {}", a.value);
            assert_eq!(a.general_pm_mm, Some(0.2), "ISO 2768-m over 6 up to 30 mm");
            assert_eq!(a.general_standard.as_deref(), Some("ISO 2768-m"));
        }
        other => panic!("expected a toleranced diameter, got {other:?}"),
    }
}
