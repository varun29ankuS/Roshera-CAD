// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The automatic FIRST-ANGLE sheet of a part the agent built, with its GD&T,
//! is as clean and as full as the third-angle sheet of the bare part.
//!
//! Found on the sheets the agent drew through the app on 2026-11-19, both
//! first angle: an EN 1092-1 DN50 PN16 weld-neck flange came out on A2 at 1:2
//! filling 8 % of the paper, its face view missing Ø102 and the PCD (the
//! certificate said INCOMPLETE (2)), and a 300 mm NACA 2412 wing rib on A1
//! at 1:1 filling 9 %. The third-angle weld-neck fixture of
//! `drawing_sheet_search` comes out full size on A2 and clean.
//!
//! The flange: the gaps between the views were sized for the third-angle
//! arrangement whatever the angle. First angle puts the face view (TOP) in
//! the right-hand column beside the isometric, so its stacked diameters had
//! the 30 mm column gap instead of the sheet's left margin, and three of them
//! found no room on any sheet the search tried.

use geometry_engine::drawing::completeness::{dimension_omissions, OmissionReason};
use geometry_engine::drawing::dimensioning::standard_drawing_auto_with_projection;
use geometry_engine::drawing::frame_for_drawing;
use geometry_engine::drawing::layout::compute_layout;
use geometry_engine::drawing::svg::frame_rect;
use geometry_engine::drawing::types::{Drawing, ProjectionAngle, SheetSize};
use geometry_engine::drawing::verify::{find_ink_outside_frame, verify_drawing, Severity};
use geometry_engine::gdt::model::{
    Annotation, DimensionalTolerance, FeatureControlFrame, GeometricCharacteristic,
};
use geometry_engine::gdt::FeatureTarget;
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::extrude::{extrude_polygon, ExtrudeOptions};
use geometry_engine::operations::revolve::{revolve_meridian, RevolveOptions};
use geometry_engine::primitives::persistent_id::PersistentId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::{Cylinder, Plane};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// The quality check's under-utilization threshold
/// (`verify::MIN_UTILIZATION`).
const MIN_UTILIZATION: f64 = 0.10;

fn solid_faces(m: &BRepModel, s: SolidId) -> Vec<u32> {
    let solid = m.solids.get(s).expect("solid");
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    shells.extend_from_slice(&solid.peer_shells);
    let mut out = Vec::new();
    for sh in shells {
        if let Some(shell) = m.shells.get(sh) {
            out.extend(shell.faces.iter().copied());
        }
    }
    out
}

/// The face of `s` whose plane faces `normal` through a point at height `at`
/// along it.
fn planar_face(m: &BRepModel, s: SolidId, normal: Vector3, at: f64) -> u32 {
    solid_faces(m, s)
        .into_iter()
        .find(|&f| {
            let face = m.faces.get(f).expect("face");
            m.surfaces
                .get(face.surface_id)
                .and_then(|surf| surf.as_any().downcast_ref::<Plane>())
                .is_some_and(|pl| {
                    let n = pl.normal * face.orientation.sign();
                    n.dot(&normal) > 0.999 && (pl.origin.dot(&normal) - at).abs() < 1e-6
                })
        })
        .expect("planar face")
}

/// The cylindrical faces of `s` of radius `r`.
fn cylinder_faces(m: &BRepModel, s: SolidId, r: f64) -> Vec<u32> {
    solid_faces(m, s)
        .into_iter()
        .filter(|&f| {
            let face = m.faces.get(f).expect("face");
            m.surfaces
                .get(face.surface_id)
                .and_then(|surf| surf.as_any().downcast_ref::<Cylinder>())
                .is_some_and(|c| (c.radius - r).abs() < 1e-6)
        })
        .collect()
}

/// The face carries a persistent id, as a face the app's history minted does.
fn ensure_pid(m: &mut BRepModel, face: u32, seed: &str) {
    if m.face_pids.get(&face).is_none() {
        let p = PersistentId::root(seed.as_bytes());
        m.face_pids.insert(face, p);
        m.pid_to_face.insert(p, face);
    }
}

fn datum(m: &mut BRepModel, s: SolidId, label: &str, face: u32) {
    ensure_pid(m, face, &format!("datum-{label}"));
    m.gdt_designate_datum(s, label, FeatureTarget::Face(face))
        .expect("designate datum");
}

fn position(m: &mut BRepModel, s: SolidId, id: &str, face: u32, tol: f64, basic: [f64; 2]) {
    ensure_pid(m, face, id);
    m.gdt_create_annotation(
        s,
        id,
        FeatureTarget::Face(face),
        Annotation::Geometric(FeatureControlFrame::position(tol, ["A", "B"], basic)),
    )
    .expect("position frame");
}

/// Drill a Z-axis hole, under its own history event key (as each app
/// operation is).
fn drill(m: &mut BRepModel, s: SolidId, x: f64, y: f64, z0: f64, r: f64, h: f64) -> SolidId {
    m.set_event_key(Some(format!("drill-{x:.3}-{y:.3}")));
    let hole = match TopologyBuilder::new(m)
        .create_cylinder_3d(Point3::new(x, y, z0), Vector3::Z, r, h)
        .unwrap()
    {
        GeometryId::Solid(h) => h,
        o => panic!("{o:?}"),
    };
    let out = boolean_operation(m, s, hole, BooleanOp::Difference, BooleanOptions::default())
        .expect("drill");
    m.set_event_key(None);
    out
}

/// EN 1092-1 DN50 PN16 type 11: the raised face Ø102 x 2 (z -2..0), the
/// Ø165 x 18 plate, the hub from Ø74 at the plate tapering to the Ø60.3 x 2.9
/// pipe, the weld end 48 from the raised face with a 37.5 deg bevel down to a
/// 1.6 mm root face, the Ø54.5 bore through; 4 x Ø18 on Ø125. Datum A the
/// raised face, datum B the bore, a Ø0.5 position frame on every bolt hole.
fn en1092_weld_neck_with_gdt() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    m.set_event_key(Some("flange".to_string()));
    let bevel_dr = 30.15 - (27.25 + 1.6);
    let bevel_dz = bevel_dr / 37.5f64.to_radians().tan();
    let meridian = [
        (27.25, -2.0),
        (51.0, -2.0),
        (51.0, 0.0),
        (82.5, 0.0),
        (82.5, 18.0),
        (37.0, 18.0),
        (30.15, 46.0 - bevel_dz),
        (27.25 + 1.6, 46.0),
        (27.25, 46.0),
    ];
    let opts = RevolveOptions {
        axis_origin: Point3::new(0.0, 0.0, 0.0),
        axis_direction: Vector3::new(0.0, 0.0, 1.0),
        angle: std::f64::consts::TAU,
        segments: 96,
        ..Default::default()
    };
    let mut s = revolve_meridian(&mut m, &meridian, opts).expect("revolve weld neck");
    m.set_event_key(None);
    for k in 0..4 {
        let a = (45.0 + 90.0 * k as f64).to_radians();
        s = drill(&mut m, s, 62.5 * a.cos(), 62.5 * a.sin(), -1.0, 9.0, 20.0);
    }
    // The flange playbook's DRF: A the seating (raised) face, B one bolt
    // hole, the full bolt pattern positioned to A|B; flatness on the seating
    // face and the bore's size tolerance besides.
    let raised = planar_face(&m, s, -Vector3::Z, 2.0);
    datum(&mut m, s, "A", raised);
    let holes = cylinder_faces(&m, s, 9.0);
    assert_eq!(holes.len(), 4, "four bolt holes");
    datum(&mut m, s, "B", holes[0]);
    for (k, &f) in holes.iter().enumerate() {
        let a = (45.0 + 90.0 * k as f64).to_radians();
        position(
            &mut m,
            s,
            &format!("pos-bolt-{k}"),
            f,
            0.5,
            [62.5 * a.cos(), 62.5 * a.sin()],
        );
    }
    m.gdt_create_annotation(
        s,
        "flat-A",
        FeatureTarget::Face(raised),
        Annotation::Geometric(FeatureControlFrame::form(
            GeometricCharacteristic::Flatness,
            0.05,
        )),
    )
    .expect("flatness");
    let bore = *cylinder_faces(&m, s, 27.25).first().expect("bore");
    ensure_pid(&mut m, bore, "bore");
    m.gdt_create_annotation(
        s,
        "size-bore",
        FeatureTarget::Face(bore),
        Annotation::Dimensional(DimensionalTolerance::symmetric(54.5, 0.2)),
    )
    .expect("bore size");
    (m, s)
}

/// NACA 2412 at x/c: the upper or lower surface point, in chord units.
fn naca2412(xc: f64, upper: bool) -> (f64, f64) {
    let (mc, p, t) = (0.02, 0.4, 0.12);
    let yt = 5.0
        * t
        * (0.2969 * xc.sqrt() - 0.1260 * xc - 0.3516 * xc.powi(2) + 0.2843 * xc.powi(3)
            - 0.1015 * xc.powi(4));
    let (yc, dyc) = if xc < p {
        (
            mc / (p * p) * (2.0 * p * xc - xc * xc),
            2.0 * mc / (p * p) * (p - xc),
        )
    } else {
        (
            mc / ((1.0 - p).powi(2)) * ((1.0 - 2.0 * p) + 2.0 * p * xc - xc * xc),
            2.0 * mc / ((1.0 - p).powi(2)) * (p - xc),
        )
    };
    let th = dyc.atan();
    if upper {
        (xc - yt * th.sin(), yc + yt * th.cos())
    } else {
        (xc + yt * th.sin(), yc - yt * th.cos())
    }
}

/// The agent's wing rib: a 300 mm chord NACA 2412 section 6 thick, the
/// profile a cosine-spaced polyline (as the agent's psketch polyline states
/// it), Ø16, Ø20 and Ø12 bores at 75, 126 and 180 mm from the leading edge.
/// Datum A the rib's face, datum B the Ø12 bore, a Ø0.1 position frame to
/// A|B on each of the others.
fn naca2412_rib_with_gdt() -> (BRepModel, SolidId) {
    const CHORD: f64 = 300.0;
    let n = 80;
    let station = |i: usize| 0.5 * (1.0 - (std::f64::consts::PI * i as f64 / n as f64).cos());
    let mut pts = Vec::new();
    for i in (0..=n).rev() {
        let (x, y) = naca2412(station(i), true);
        pts.push(Point3::new(x * CHORD, y * CHORD, 0.0));
    }
    for i in 1..n {
        let (x, y) = naca2412(station(i), false);
        pts.push(Point3::new(x * CHORD, y * CHORD, 0.0));
    }
    let mut m = BRepModel::new();
    m.set_event_key(Some("rib".to_string()));
    let opts = ExtrudeOptions {
        direction: Vector3::Z,
        distance: 6.0,
        ..ExtrudeOptions::default()
    };
    let mut s = extrude_polygon(&mut m, &pts, opts, Tolerance::default()).expect("rib");
    m.set_event_key(None);
    let bores = [(180.0, 5.37, 6.0), (75.0, 5.19, 8.0), (126.0, 6.03, 10.0)];
    for &(x, y, r) in &bores {
        s = drill(&mut m, s, x, y, -1.0, r, 12.0);
    }
    let face = planar_face(&m, s, Vector3::Z, 6.0);
    datum(&mut m, s, "A", face);
    let spar = cylinder_faces(&m, s, 6.0);
    datum(&mut m, s, "B", *spar.first().expect("spar bore"));
    for (k, &(x, y, r)) in bores.iter().enumerate().skip(1) {
        let f = *cylinder_faces(&m, s, r).first().expect("lightening hole");
        position(
            &mut m,
            s,
            &format!("pos-hole-{k}"),
            f,
            0.1,
            [x - 180.0, y - 5.37],
        );
    }
    (m, s)
}

fn describe(m: &BRepModel, d: &Drawing) -> String {
    let report = verify_drawing(d);
    let omitted: Vec<String> = dimension_omissions(m, d)
        .into_iter()
        .map(|o| format!("{} {} ({:?})", o.view, o.label, o.reason))
        .collect();
    let errors: Vec<String> = report
        .issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .map(|i| format!("{:?}: {}", i.kind, i.message))
        .collect();
    format!(
        "sheet {} views {:?} utilization {:.3} errors {errors:#?} omitted {omitted:?}",
        d.sheet_size.label(),
        d.views
            .iter()
            .map(|v| (v.name.clone(), v.scale))
            .collect::<Vec<_>>(),
        report.sheet_utilization,
    )
}

/// The sheet is clean -- no layout error, no ink over the frame, every
/// carried dimension drawn -- and its views are not mostly empty paper.
fn assert_clean_and_filled(m: &BRepModel, d: &Drawing, what: &str) {
    let report = verify_drawing(d);
    assert_eq!(report.error_count(), 0, "layout errors: {what}");
    let layout = compute_layout(d);
    let outside = find_ink_outside_frame(&layout, frame_rect(&d.sheet_size));
    assert!(
        outside.is_empty(),
        "ink outside the frame {outside:?}: {what}"
    );
    let unplaced: Vec<String> = dimension_omissions(m, d)
        .into_iter()
        .filter(|o| o.reason == OmissionReason::NotPlaced)
        .map(|o| format!("{} {}", o.view, o.label))
        .collect();
    assert!(
        unplaced.is_empty(),
        "unplaced dimensions {unplaced:?}: {what}"
    );
    assert!(
        report.sheet_utilization >= MIN_UTILIZATION,
        "the views fill too little of the sheet: {what}"
    );
}

/// A Ø165 weld-neck flange with its datums and bolt-hole position frames,
/// drawn first angle, is full size on A2 or smaller with every dimension
/// placed -- the face view's Ø102, Ø74 and PCD Ø125 included -- and its views
/// fill the paper.
///
/// A3, where a drafter would put it, needs a round-part layout (the face
/// view beside SECTION A-A, the RIGHT view that repeats FRONT left out): at
/// 1:1 the five views stack about 330 mm tall where A3 has 267.
#[test]
fn a_first_angle_toleranced_weld_neck_flange_is_full_size_and_complete() {
    let (mut m, s) = en1092_weld_neck_with_gdt();
    let _ = frame_for_drawing(&mut m, s);
    let d = standard_drawing_auto_with_projection(&m, s, uuid::Uuid::nil(), ProjectionAngle::First)
        .expect("sheet");
    let what = describe(&m, &d);
    eprintln!("flange: {what}");
    assert_clean_and_filled(&m, &d, &what);
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
}

/// A 300 mm NACA 2412 rib with its datums, position frames and ordinate
/// table belongs on A3 at 1:2 or A2 at 1:1. It still comes out on A1 at 1:1
/// filling 9 % of the paper: A2 at 1:1 is 2.4 mm too narrow for the
/// full-scale isometric cell beside the 300 mm views, and on A3 at 1:2 the
/// 145 mm ordinate table has no free paper beside the centred views (it is
/// printed over TOP, and two hole locations 0.18 mm apart read as one
/// interval at 1:2). Making either clean needs the sheet search to reserve
/// room for the tables, or a reduced-scale pictorial.
#[test]
#[ignore = "open: the sheet search does not reserve paper for the ordinate and hole tables"]
fn a_toleranced_aerofoil_rib_goes_on_a2_or_smaller() {
    let (mut m, s) = naca2412_rib_with_gdt();
    let _ = frame_for_drawing(&mut m, s);
    let d = standard_drawing_auto_with_projection(&m, s, uuid::Uuid::nil(), ProjectionAngle::First)
        .expect("sheet");
    let what = describe(&m, &d);
    eprintln!("rib: {what}");
    assert!(
        d.profile_table.is_some(),
        "the rib carries its ordinates: {what}"
    );
    assert!(
        matches!(d.sheet_size, SheetSize::A4 | SheetSize::A3 | SheetSize::A2),
        "a 300 mm rib belongs on A2 or smaller: {what}"
    );
    assert_clean_and_filled(&m, &d, &what);
}
