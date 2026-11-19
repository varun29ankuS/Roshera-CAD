// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The automatic sheet is chosen by SEARCH over (sheet, ISO 5455 scale)
//! pairs, and a revolved weld-neck flange is drawn the way a drafter draws
//! it: full size on a small sheet, with its hub and raised face dimensioned.
//!
//! Found rehearsing a demo: a DN50 weld-neck flange (Ø165 x 18 plate, Ø102 x 2
//! raised face, hub Ø75 tapering to Ø60.3, 45 mm from the back of the raised
//! face to the weld end, Ø54.5 bore, 4 x Ø18 on Ø125) was drawn at 1:2 on A2
//! with its views filling 8% of the paper, the hub and the raised-face height
//! not dimensioned at all.

use geometry_engine::drawing::dimensioning::standard_drawing_auto;
use geometry_engine::drawing::layout::compute_layout;
use geometry_engine::drawing::types::{Drawing, SheetSize};
use geometry_engine::drawing::verify::{verify_drawing, Severity};
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::revolve::{revolve_meridian, RevolveOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// The weld-neck meridian `(r, z)`: the raised face Ø102 x 2 below the plate
/// (z -2..0), the Ø165 x 18 plate (z 0..18), the hub from Ø75 at the plate to
/// Ø60.3 at the weld end (z 45), the Ø54.5 bore all through. `revolve_meridian`
/// closes the bore wall itself.
const WELD_NECK_MERIDIAN: [(f64, f64); 8] = [
    (27.25, -2.0),
    (51.0, -2.0),
    (51.0, 0.0),
    (82.5, 0.0),
    (82.5, 18.0),
    (37.5, 18.0),
    (30.15, 45.0),
    (27.25, 45.0),
];

fn weld_neck_flange() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let opts = RevolveOptions {
        axis_origin: Point3::new(0.0, 0.0, 0.0),
        axis_direction: Vector3::new(0.0, 0.0, 1.0),
        angle: std::f64::consts::TAU,
        segments: 96,
        ..Default::default()
    };
    let mut s = revolve_meridian(&mut m, &WELD_NECK_MERIDIAN, opts).expect("revolve weld neck");
    for k in 0..4 {
        let a = (45.0 + 90.0 * k as f64).to_radians();
        let hole = match TopologyBuilder::new(&mut m)
            .create_cylinder_3d(
                Point3::new(62.5 * a.cos(), 62.5 * a.sin(), -1.0),
                Vector3::Z,
                9.0,
                20.0,
            )
            .unwrap()
        {
            GeometryId::Solid(h) => h,
            o => panic!("{o:?}"),
        };
        s = boolean_operation(
            &mut m,
            s,
            hole,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("drill bolt hole");
    }
    (m, s)
}

fn box_part(x: f64, y: f64, z: f64) -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let s = match TopologyBuilder::new(&mut m).create_box_3d(x, y, z).unwrap() {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };
    (m, s)
}

/// The labels the layout actually INKS (a dimension carried but not placed
/// is not on the sheet).
fn inked_labels(d: &Drawing) -> Vec<String> {
    compute_layout(d)
        .dimensions
        .iter()
        .map(|pd| pd.label.clone())
        .collect()
}

fn describe(d: &Drawing) -> String {
    let report = verify_drawing(d);
    let errors: Vec<String> = report
        .issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .map(|i| format!("{:?}: {}", i.kind, i.message))
        .collect();
    format!(
        "sheet {} views {:?} utilization {:.3} errors {errors:#?} inked {:?}",
        d.sheet_size.label(),
        d.views
            .iter()
            .map(|v| (v.name.clone(), v.scale))
            .collect::<Vec<_>>(),
        report.sheet_utilization,
        inked_labels(d)
    )
}

/// Full size on the smallest sheet that holds the views at 1:1, clean, and
/// not mostly empty paper.
///
/// The smallest such sheet is A2, not the A3 a drafter would use: the
/// automatic sheet always draws FRONT, TOP, RIGHT, the isometric and SECTION
/// A-A, and at 1:1 that column is 47 (SECTION) + 165 (TOP) + 47 (FRONT) mm of
/// views plus their gaps, about 304 mm, where A3's drawing area is 203 mm
/// tall. A3 needs a round-part layout (the face view beside SECTION A-A, the
/// RIGHT view -- which repeats FRONT on a solid of revolution -- left out),
/// which the sheet does not have yet.
#[test]
fn a_weld_neck_flange_is_drawn_full_size_on_a_small_sheet() {
    let (m, s) = weld_neck_flange();
    let t = std::time::Instant::now();
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    eprintln!(
        "weld-neck sheet built in {:.0} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    if let Ok(path) = std::env::var("ROSHERA_DUMP_SHEET_WN") {
        std::fs::write(path, geometry_engine::drawing::svg::render_drawing_svg(&d))
            .expect("write sheet");
    }
    let what = describe(&d);
    eprintln!("{what}");
    assert!(
        matches!(d.sheet_size, SheetSize::A4 | SheetSize::A3 | SheetSize::A2),
        "a Ø165 flange belongs on A2 or smaller: {what}"
    );
    for v in d.views.iter().filter(|v| v.name != "ISOMETRIC") {
        assert!(
            (v.scale - 1.0).abs() < 1e-9,
            "{} is not full size: {what}",
            v.name
        );
    }
    let report = verify_drawing(&d);
    assert_eq!(report.error_count(), 0, "layout errors: {what}");
    assert!(
        report.sheet_utilization >= 0.10,
        "the views fill too little of the sheet: {what}"
    );
}

/// The hub and the raised face are dimensioned: the raised-face height, the
/// plate thickness, the hub's length and its diameters at both ends.
#[test]
fn a_weld_neck_flange_states_its_hub_and_raised_face() {
    let (m, s) = weld_neck_flange();
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let what = describe(&d);
    let inked = inked_labels(&d);
    for want in [
        "\u{00D8}165",
        "\u{00D8}102",
        "\u{00D8}75",
        "\u{00D8}60.3",
        "\u{00D8}54.5",
        "18",
        "2",
    ] {
        assert!(
            inked.iter().any(|l| l == want),
            "the sheet does not ink {want:?}: {what}"
        );
    }
    // The hub's length (27, plate to weld end) or the overall height (47) --
    // either states where the weld end is.
    assert!(
        inked.iter().any(|l| l == "27" || l == "47" || l == "45"),
        "the hub length is not stated: {what}"
    );
}

/// A cone's diameter is measured across its circle: the record's anchor is on
/// the rim, its direction radial -- so `anchor -> anchor - direction * value`
/// spans the circle. Both ends of the hub are stated, each by its own
/// persistent id. (The anchor was on the axis with a fixed +X direction: the
/// Ø75 was drawn from the centre line to a point 75 mm off it.)
#[test]
fn a_frustums_diameters_span_its_circles() {
    let (m, s) = weld_neck_flange();
    let dims = geometry_engine::readable::extract_dimensions(&m, s);
    let cone_diameters: Vec<_> = dims
        .iter()
        .filter(|d| {
            d.kind == "diameter" && (d.value - 75.0).abs() < 1e-6
                || d.kind == "diameter" && (d.value - 60.3).abs() < 1e-6
        })
        .collect();
    assert_eq!(
        cone_diameters.len(),
        2,
        "the hub's two end diameters: {:?}",
        dims.iter()
            .filter(|d| d.kind == "diameter")
            .map(|d| d.value)
            .collect::<Vec<_>>()
    );
    for d in &cone_diameters {
        let [x, y, _] = d.anchor;
        let radial = x.hypot(y);
        assert!(
            (radial - 0.5 * d.value).abs() < 1e-6,
            "{}: the anchor is {radial:.3} mm off the axis, not on the rim",
            d.label
        );
        assert!(
            d.direction[2].abs() < 1e-9,
            "{}: the direction {:?} is not radial",
            d.label,
            d.direction
        );
    }
    if let (Some(a), Some(b)) = (&cone_diameters[0].pid, &cone_diameters[1].pid) {
        assert_ne!(a, b, "the two ends are two dimensions, keyed apart");
    }
}

/// The search is never worse than the old ladder's pick: a plain plate
/// comes out clean, at a standard scale, not under-filled.
#[test]
fn a_plain_plate_sheet_is_clean_and_filled() {
    for (x, y, z) in [(80.0, 60.0, 10.0), (199.0, 74.0, 40.0), (30.0, 20.0, 5.0)] {
        let (m, s) = box_part(x, y, z);
        let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
        let what = describe(&d);
        eprintln!("box {x}x{y}x{z}: {what}");
        let report = verify_drawing(&d);
        assert_eq!(report.error_count(), 0, "box {x}x{y}x{z}: {what}");
        assert!(
            report.sheet_utilization >= 0.10,
            "box {x}x{y}x{z} under-fills its sheet: {what}"
        );
    }
}
