// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! An existing drawing is put on other paper, or drawn at another scale:
//! "put this drawing on A2", "A1 at 1:2". The size used to be fixed when the
//! sheet was made (the search picks the smallest that fits); only one view's
//! scale could be changed afterwards.
//!
//! `standard_drawing_on_sheet` draws the automatic sheet on exactly the paper
//! and scale asked for, and REFUSES -- with the overflow in millimetres --
//! when the views at that scale do not fit; it never quietly shrinks them.

use geometry_engine::drawing::sheet_certificate::certify_drawing;
use geometry_engine::drawing::{
    largest_scale_on_sheet, standard_drawing_auto, standard_drawing_on_sheet, verify_drawing,
    Drawing, ProjectionAngle, ProjectionError, Sheet, SheetOrientation, SheetSize,
};
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn solid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}

/// A `w` x `d` x 20 plate with two Ø10 through holes.
fn plate(w: f64, d: f64) -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let mut s = solid(
        TopologyBuilder::new(&mut m)
            .create_box_3d(w, d, 20.0)
            .unwrap(),
    );
    for x in [-0.25 * w, 0.25 * w] {
        let t = solid(
            TopologyBuilder::new(&mut m)
                .create_cylinder_3d(Point3::new(x, 0.0, -15.0), Vector3::Z, 5.0, 30.0)
                .unwrap(),
        );
        s = boolean_operation(
            &mut m,
            s,
            t,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .unwrap();
    }
    (m, s)
}

fn on(m: &BRepModel, s: SolidId, sheet: Sheet, scale: f64) -> Result<Drawing, ProjectionError> {
    standard_drawing_on_sheet(
        m,
        s,
        uuid::Uuid::nil(),
        ProjectionAngle::Third,
        sheet,
        scale,
    )
}

/// What a sheet states, whatever paper it is on: every dimension's kind and
/// label, and every hole-table row.
fn statements(d: &Drawing) -> Vec<String> {
    let mut v: Vec<String> = d
        .views
        .iter()
        .flat_map(|view| view.dimensions.iter())
        .map(|x| format!("{} {}", x.kind, x.label))
        .chain(
            d.hole_sites
                .iter()
                .map(|h| format!("hole {} {} {}", h.tag, h.dia_label, h.depth_label)),
        )
        .collect();
    v.sort();
    v
}

#[test]
fn a_sheet_is_drawn_on_the_paper_and_scale_asked_for() {
    let (m, s) = plate(120.0, 80.0);
    let auto = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("the searched sheet");
    assert_ne!(
        auto.sheet_size,
        SheetSize::A1,
        "fixture: the search does not pick A1"
    );
    for (sheet, scale) in [
        (SheetSize::A2.landscape(), 1.0),
        (Sheet::new(SheetSize::A0, SheetOrientation::Portrait), 2.0),
        (SheetSize::A1.landscape(), 0.5),
    ] {
        let d = on(&m, s, sheet, scale).expect("the sheet fits");
        assert_eq!(d.sheet(), sheet, "drawn on the paper asked for");
        for v in d
            .views
            .iter()
            .filter(|v| matches!(v.name.as_str(), "FRONT" | "TOP" | "RIGHT"))
        {
            assert!(
                (v.scale - scale).abs() < 1e-12,
                "{} is at {}, asked {scale}",
                v.name,
                v.scale
            );
        }
        // The paper changes where things are, never what the sheet states.
        assert_eq!(
            statements(&d),
            statements(&auto),
            "on {} at {scale}",
            sheet.describe()
        );
        let quality = verify_drawing(&d);
        assert_eq!(quality.error_count(), 0, "{:#?}", quality.issues);
        let cert = certify_drawing(&m, &d);
        assert!(cert.sound, "the certificate re-runs sound on the new paper");
        assert_eq!(cert.counts.stale + cert.counts.dangling, 0);
    }
}

/// Mutation: drop the `placement.fits()` refusal in
/// `standard_drawing_on_sheet` -> a sheet with its views off the paper is
/// returned -> RED.
#[test]
fn a_scale_the_paper_cannot_hold_is_refused_with_its_overflow() {
    let (m, s) = plate(300.0, 200.0);
    let a4 = SheetSize::A4.landscape();
    match on(&m, s, a4, 1.0) {
        Err(ProjectionError::ScaleDoesNotFitSheet {
            scale,
            sheet,
            overflow_x_mm,
            overflow_y_mm,
        }) => {
            assert!((scale - 1.0).abs() < 1e-12);
            assert_eq!(sheet, "A4 landscape");
            assert!(
                overflow_x_mm > 100.0 && overflow_y_mm > 100.0,
                "a 300 x 200 plate overruns A4 by hundreds of mm: {overflow_x_mm} x {overflow_y_mm}"
            );
        }
        other => panic!(
            "1:1 on A4 must be refused, got {:?}",
            other.map(|d| d.sheet())
        ),
    }
    // The refusal can be acted on: the largest scale that fits is named, it
    // is accepted, and the next ISO scale up is refused.
    let fits = largest_scale_on_sheet(&m, s, uuid::Uuid::nil(), ProjectionAngle::Third, a4)
        .expect("measured")
        .expect("some ISO scale fits");
    assert!(fits < 1.0, "{fits}");
    let d = on(&m, s, a4, fits).expect("the named scale is accepted");
    assert_eq!(d.sheet(), a4);
    let next_up = [0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0]
        .into_iter()
        .find(|&k| k > fits + 1e-12)
        .expect("a larger ISO scale");
    assert!(
        matches!(
            on(&m, s, a4, next_up),
            Err(ProjectionError::ScaleDoesNotFitSheet { .. })
        ),
        "{next_up} is the next scale up from the largest that fits ({fits})"
    );
}

#[test]
fn a_scale_that_is_not_a_ratio_is_refused() {
    let (m, s) = plate(120.0, 80.0);
    for bad in [0.0, -2.0, f64::NAN, f64::INFINITY] {
        assert!(
            matches!(
                on(&m, s, SheetSize::A2.landscape(), bad),
                Err(ProjectionError::InvalidScale { .. })
            ),
            "{bad} is not a drawing scale"
        );
    }
}
