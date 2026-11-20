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
use geometry_engine::drawing::dimensioning::{
    standard_drawing_auto, standard_drawing_auto_with_projection, Dimension2d,
};
use geometry_engine::drawing::frame_for_drawing;
use geometry_engine::drawing::layout::compute_layout;
use geometry_engine::drawing::sheet_certificate::{certify_drawing, SheetFactKind, SheetVerdict};
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
/// pipe, the weld end 48 from the raised face prepared as ASME B16.25 / ISO
/// 9692-1 specify: a 37.5 deg bevel measured from the END FACE (the plane
/// square to the pipe axis) down to a 1.6 mm root face -- the bevel falls
/// `dr * tan 37.5` along the axis over its `dr` of wall, so its cone's
/// half-angle to the axis is 52.5 deg -- the Ø54.5 bore through; 4 x Ø18 on
/// Ø125. Datum A the raised face, datum B a bolt hole, a Ø0.5 position frame
/// on every bolt hole.
fn en1092_weld_neck_with_gdt() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    m.set_event_key(Some("flange".to_string()));
    let bevel_dr = 30.15 - (27.25 + 1.6);
    let bevel_dz = bevel_dr * 37.5f64.to_radians().tan();
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
    // Its weld end is a 37.5 deg preparation, and the sheet states it as one:
    // 37.5° from the end face, not an included angle on a leader.
    let angles: Vec<&Dimension2d> = d
        .views
        .iter()
        .flat_map(|v| v.dimensions.iter())
        .filter(|x| x.kind == "angle")
        .collect();
    let labels: Vec<&str> = angles.iter().map(|x| x.label.as_str()).collect();
    assert!(
        angles
            .iter()
            .any(|x| x.label == "37.5\u{00B0}"
                && x.datum.as_ref().is_some_and(|r| r.kind == "end_face")),
        "the weld bevel reads 37.5° from the end face: {labels:?}"
    );
    assert!(
        !labels
            .iter()
            .any(|l| l.contains("75") && !l.contains("37.5")),
        "no included angle on the weld end: {labels:?}"
    );
}

/// A 300 mm NACA 2412 rib with its datums, position frames and ordinate
/// table belongs on A2 or smaller; it came out on A1 at 1:1 filling 9 % of
/// the paper. On A3 at 1:2 its 116 x 136 mm ordinate table has no free paper
/// beside the views (it was printed over TOP), and the search did not know:
/// it built that sheet, found it dirty and kept A1. On A2 at 1:1 the
/// full-scale isometric cell is 2.4 mm too wide beside the 300 mm views. The
/// search now counts the tables' paper, skips A3 at 1:2, and draws the rib
/// full size on A2 with its pictorial at 1:2 (stated in its title, as ISO
/// 128 allows).
#[test]
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

/// The rib's Ø16 and Ø12 bores are 0.18 mm apart in Y: two rows of its hole
/// table, Y 17.9 and Y 18.08. Their location dimensions are not inked (the
/// table states them), so the sheet's check does not report them as one
/// interval stated twice -- it did on the A3 1:2 sheet, where they were
/// 0.09 mm apart on paper. The check still reports the same two dimensions
/// when they ARE inked (no table), where their extension lines would
/// print on each other.
///
/// Mutation: let `verify::check_redundant_dimensions`' same-interval pass
/// count the dimensions the table states again -> RED.
#[test]
fn two_tabled_holes_close_in_y_are_two_rows_not_one_interval() {
    use geometry_engine::drawing::verify::DrawingIssueKind;
    let (mut m, s) = naca2412_rib_with_gdt();
    let _ = frame_for_drawing(&mut m, s);
    let d = standard_drawing_auto_with_projection(&m, s, uuid::Uuid::nil(), ProjectionAngle::First)
        .expect("sheet");
    let row = |y: &str| {
        d.hole_sites
            .iter()
            .find(|h| h.y_label == y)
            .unwrap_or_else(|| panic!("a row at Y {y}"))
            .clone()
    };
    let (b1, a1) = (row("17.9"), row("18.08"));
    // Both locations stated in TOP, as the A3 1:2 sheet had them.
    let mut doctored = d.clone();
    let mine = |x: &Dimension2d| {
        x.kind == "position"
            && x.entities
                .iter()
                .any(|e| b1.face_entities.contains(e) || a1.face_entities.contains(e))
    };
    let ti = doctored
        .views
        .iter()
        .position(|v| v.name == "TOP")
        .expect("TOP");
    let template = doctored.views[ti]
        .dimensions
        .iter()
        .find(|x| x.kind == "position" && (x.a[0] - x.b[0]).abs() < 1e-9)
        .expect("a Y location in TOP")
        .clone();
    for v in &mut doctored.views {
        v.dimensions.retain(|x| !mine(x));
    }
    let min_x = doctored.views[ti].extent.min_x;
    for (k, site) in [&b1, &a1].into_iter().enumerate() {
        let mut x = template.clone();
        x.id = format!("probe-y-{k}");
        x.key = None;
        x.entities = site.face_entities.clone();
        x.value = site.y_mm;
        x.label = site.y_label.clone();
        x.a = [min_x + site.x_mm, template.a[1]];
        x.b = [min_x + site.x_mm, template.a[1] + site.y_mm];
        doctored.views[ti].dimensions.push(x);
    }
    let redundant = |d: &Drawing| {
        verify_drawing(d)
            .issues
            .into_iter()
            .filter(|i| i.kind == DrawingIssueKind::RedundantDimension)
            .map(|i| i.message)
            .collect::<Vec<_>>()
    };
    assert!(
        redundant(&doctored).is_empty(),
        "two table rows read as one interval: {:?}",
        redundant(&doctored)
    );
    // Inked, they would print on each other's lines: still reported.
    let mut inked = doctored.clone();
    inked.hole_sites.clear();
    assert!(
        redundant(&inked)
            .iter()
            .any(|msg| msg.contains("17.9") && msg.contains("18.08")),
        "{:?}",
        redundant(&inked)
    );
}

// ── Cone angles: each stated by the convention for its kind ─────────────────

/// The EN 1092-1 DN50 weld neck of [`en1092_weld_neck_with_gdt`] with its
/// weld end prepared the way ASME B16.25 and ISO 9692-1 specify it: the
/// bevel `bevel_deg` from the END FACE (the plane square to the pipe axis),
/// down to the 1.6 mm root face, so the bevel cone's half-angle to the axis
/// is `90 - bevel_deg` (at 37.5 deg it is that fixture's weld end, without
/// its GD&T). Returns the bevel face,
/// which carries a persistent id as a face the app's history minted does.
fn weld_neck_with_weld_prep(bevel_deg: f64) -> (BRepModel, SolidId, u32) {
    use geometry_engine::primitives::surface::Cone;
    let mut m = BRepModel::new();
    m.set_event_key(Some("weld-neck".to_string()));
    let bevel_dr = 30.15 - (27.25 + 1.6);
    let bevel_dz = bevel_dr * bevel_deg.to_radians().tan();
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
    let half = (90.0 - bevel_deg).to_radians();
    let bevel = solid_faces(&m, s)
        .into_iter()
        .find(|&f| {
            let face = m.faces.get(f).expect("face");
            m.surfaces
                .get(face.surface_id)
                .and_then(|surf| surf.as_any().downcast_ref::<Cone>())
                .is_some_and(|c| (c.half_angle - half).abs() < 1e-6)
        })
        .expect("the bevel cone");
    ensure_pid(&mut m, bevel, "bevel");
    (m, s, bevel)
}

/// The cone-angle dimensions of the sheet that name one of `faces`.
fn cone_angles(d: &Drawing, faces: &[u32]) -> Vec<Dimension2d> {
    d.views
        .iter()
        .flat_map(|v| v.dimensions.iter())
        .filter(|x| x.kind == "angle" && x.entities.iter().any(|e| faces.contains(e)))
        .cloned()
        .collect()
}

fn all_labels(d: &Drawing) -> Vec<String> {
    d.views
        .iter()
        .flat_map(|v| v.dimensions.iter().map(|x| x.label.clone()))
        .collect()
}

/// A weld bevel is stated the way its preparation is specified: 37.5 deg
/// from the end face, not its 52.5 deg half-angle to the axis, and with no
/// `∠` in front of it (ISO 129-1 writes an angle as a bare number and its
/// degree sign; `∠` is the angularity symbol). The dimension names the end
/// face as what the number is measured from.
///
/// Mutation: state every cone by its half-angle again (make
/// `cone_angles::apply_cone_angle_conventions` return the records
/// unchanged) -> the sheet reads `∠ 52.5°` -> RED.
#[test]
fn a_weld_bevel_reads_its_angle_from_the_end_face() {
    let (m, s, bevel) = weld_neck_with_weld_prep(37.5);
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let all = all_labels(&d);
    let angles = cone_angles(&d, &[bevel]);
    assert_eq!(angles.len(), 1, "the bevel's angle, once: {all:?}");
    let a = &angles[0];
    assert_eq!(a.label, "37.5\u{00B0}", "{all:?}");
    assert!((a.value - 37.5).abs() < 1e-9, "{all:?}");
    assert!(
        !all.iter()
            .any(|l| l.contains("52.5") || l.contains('\u{2220}')),
        "no half-angle and no angle sign on the sheet: {all:?}"
    );
    let reference = a.datum.as_ref().expect("the angle names its reference");
    assert_eq!(reference.kind, "end_face", "{reference:?}");
    // The reference is the weld end's plane, z = 46.
    assert!((reference.origin[2] - 46.0).abs() < 1e-6, "{reference:?}");
}

/// The bevel's angle is drawn as an angular dimension (ISO 129-1): an arc
/// about the corner where the bevel meets the end face, between an extension
/// line continuing the end face's line and the bevel's own side line, the arc
/// sweeping the 37.5 deg its number states. The reference is on the paper,
/// not only in the data.
///
/// Mutation: place the bevel's angle on a plain leader again (make
/// `layout::angular_dimension` return `None`) -> no arc -> RED.
#[test]
fn a_weld_bevel_is_drawn_as_an_arc_from_the_end_face() {
    let (m, s, bevel) = weld_neck_with_weld_prep(37.5);
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let (vi, dim) = d
        .views
        .iter()
        .enumerate()
        .find_map(|(i, v)| {
            v.dimensions
                .iter()
                .find(|x| x.kind == "angle" && x.entities.contains(&bevel))
                .map(|x| (i, x.clone()))
        })
        .expect("the bevel angle");
    let layout = compute_layout(&d);
    let pd = layout
        .dimensions
        .iter()
        .find(|p| p.dim_id == dim.id && p.owner_view == vi)
        .expect("the bevel angle is placed");
    assert_eq!(pd.label, "37.5\u{00B0}");
    let arc = pd.arc.as_ref().expect("an arc, not a leader");
    let c = arc.centre;
    assert!(arc.points.len() >= 3, "{arc:?}");
    for p in &arc.points {
        let r = (p[0] - c[0]).hypot(p[1] - c[1]);
        assert!(
            (r - arc.radius).abs() < 1e-6,
            "the arc is a circle about the corner"
        );
    }
    let dir = |p: [f64; 2]| {
        let n = (p[0] - c[0]).hypot(p[1] - c[1]);
        [(p[0] - c[0]) / n, (p[1] - c[1]) / n]
    };
    let (u, g) = (dir(arc.points[0]), dir(*arc.points.last().expect("arc")));
    let sweep = (u[0] * g[0] + u[1] * g[1])
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees();
    assert!((sweep - 37.5).abs() < 0.05, "the arc sweeps {sweep} deg");
    // Its arrowheads touch the arc's ends.
    assert_eq!(pd.arrows[0].tip, arc.points[0]);
    assert_eq!(pd.arrows[1].tip, *arc.points.last().expect("arc"));
    // One extension line continues the end face: square to the pipe axis,
    // which FRONT shows vertical, so it is horizontal on the sheet, level
    // with the corner.
    assert!(
        pd.ext.iter().any(|e| (e[0][1] - c[1]).abs() < 1e-6
            && (e[1][1] - c[1]).abs() < 1e-6
            && (e[0][0] - e[1][0]).abs() > 1.0),
        "an extension line on the end face: {:?}",
        pd.ext
    );
    // The corner is the root face's rim, Ø57.7, at the end face z 46.
    let view = &d.views[vi];
    let sheet_h = d.sheet_size.height();
    let end_y = (sheet_h - view.position_mm[1]) - dim.a[1] * view.scale;
    assert!((c[1] - end_y).abs() < 1e-6, "corner {c:?} on the end face");
    assert!(
        ((c[0] - view.position_mm[0]).abs() - 28.85 * view.scale).abs() < 1e-6,
        "corner {c:?} on the root face's rim (view at {:?}, 1:{})",
        view.position_mm,
        1.0 / view.scale
    );
    // The SVG inks it: the number, and the arc's chords.
    let svg = geometry_engine::drawing::svg::render_drawing_svg(&d);
    assert!(svg.contains(">37.5\u{00B0}<"), "the number is inked");
    // Set ROSHERA_DUMP_SHEET_DIR=<dir> to LOOK at the sheet.
    if let Ok(dir) = std::env::var("ROSHERA_DUMP_SHEET_DIR") {
        let pdf = geometry_engine::drawing::pdf::render_drawing_pdf(&d).expect("pdf");
        std::fs::write(std::path::Path::new(&dir).join("weld_neck_bevel.pdf"), pdf)
            .expect("write pdf");
    }
    // And the sheet stays clean with it: no number on another's, no ink off
    // the frame, nothing left unplaced.
    let what = describe(&m, &d);
    assert_eq!(verify_drawing(&d).error_count(), 0, "{what}");
    let outside = find_ink_outside_frame(&layout, frame_rect(&d.sheet_size));
    assert!(outside.is_empty(), "{outside:?}: {what}");
    assert!(
        !dimension_omissions(&m, &d)
            .iter()
            .any(|o| o.reason == OmissionReason::NotPlaced),
        "{what}"
    );
}

/// A steep cone that does NOT end the part is not a weld bevel: a stepped
/// shaft's 68 deg shoulder cone meets the shoulder face (square to the axis,
/// facing out, a convex edge -- everything a bevel's root face is), but the
/// pin carries on beyond that face. It is stated by its included angle.
///
/// Mutation: drop the open-end check in `cone_angles::bevel_end` (no point
/// of the part "beyond" the end plane) -> the shoulder reads 21.8° from a
/// face that is not an end -> RED.
#[test]
fn a_steep_cone_short_of_the_part_end_is_not_a_bevel() {
    use geometry_engine::primitives::surface::Cone;
    let mut m = BRepModel::new();
    m.set_event_key(Some("stepped-shaft".to_string()));
    let meridian = [
        (4.0, 0.0),
        (20.0, 0.0),
        (20.0, 10.0),
        (15.0, 12.0),
        (8.0, 12.0),
        (8.0, 30.0),
        (4.0, 30.0),
    ];
    let opts = RevolveOptions {
        axis_origin: Point3::new(0.0, 0.0, 0.0),
        axis_direction: Vector3::new(0.0, 0.0, 1.0),
        angle: std::f64::consts::TAU,
        segments: 96,
        ..Default::default()
    };
    let s = revolve_meridian(&mut m, &meridian, opts).expect("revolve shaft");
    m.set_event_key(None);
    let cones: Vec<u32> = solid_faces(&m, s)
        .into_iter()
        .filter(|&f| {
            let face = m.faces.get(f).expect("face");
            m.surfaces
                .get(face.surface_id)
                .and_then(|surf| surf.as_any().downcast_ref::<Cone>())
                .is_some()
        })
        .collect();
    assert!(!cones.is_empty(), "fixture: the shoulder is a cone");
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let all = all_labels(&d);
    let angles = cone_angles(&d, &cones);
    assert_eq!(angles.len(), 1, "the shoulder's angle, once: {all:?}");
    let included = 2.0 * (5.0f64 / 2.0).atan().to_degrees();
    assert!(
        (angles[0].value - included).abs() < 1e-6,
        "included {included}: {all:?}"
    );
    assert!(angles[0].datum.is_none(), "{:?}", angles[0].datum);
}

/// The certificate re-measures the number the sheet prints: the bevel's
/// 37.5 deg is checked against the live solid measured the same way, from
/// its end face. A stored value that was tampered with, a label whose number
/// no longer states the stored value, and the same sheet against the part
/// re-cut to a 30 deg preparation are each caught.
///
/// Mutation: re-measure the live bevel by its half-angle (the sheet's 37.5
/// against the solid's 52.5) -> the honest sheet is stale -> RED; drop the
/// label-number check -> the relabelled sheet certifies -> RED.
#[test]
fn the_printed_bevel_angle_is_re_measured_on_the_solid() {
    let (m, s, bevel) = weld_neck_with_weld_prep(37.5);
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let fact_of = |m: &BRepModel, d: &Drawing| {
        let cert = certify_drawing(m, d);
        cert.facts
            .iter()
            .find(|f| {
                f.kind == SheetFactKind::Dimension && f.unit == "deg" && f.face_ids.contains(&bevel)
            })
            .map(|f| (f.label.clone(), f.live.verdict, f.live.measured))
            .expect("the bevel's angle is a sheet fact")
    };
    let (label, verdict, measured) = fact_of(&m, &d);
    assert_eq!(label, "37.5\u{00B0}");
    assert_eq!(verdict, SheetVerdict::Consistent, "{label} {measured:?}");
    assert!((measured.expect("re-measured") - 37.5).abs() < 1e-6);

    let edit = |d: &Drawing, f: &dyn Fn(&mut Dimension2d)| -> Drawing {
        let mut d = d.clone();
        for v in &mut d.views {
            for x in &mut v.dimensions {
                if x.kind == "angle" && x.entities.contains(&bevel) {
                    f(x);
                }
            }
        }
        d
    };
    // The half-angle stored as if it were the bevel's angle.
    let wrong = edit(&d, &|x| {
        x.value = 52.5;
        x.label = "52.5\u{00B0}".to_string();
    });
    assert_eq!(fact_of(&m, &wrong).1, SheetVerdict::Stale);
    // The label alone says 52.5 over a stored 37.5.
    let relabelled = edit(&d, &|x| x.label = "52.5\u{00B0}".to_string());
    assert_eq!(fact_of(&m, &relabelled).1, SheetVerdict::Stale);

    // The same sheet against the part re-cut to a 30 deg preparation.
    let (m30, _, bevel30) = weld_neck_with_weld_prep(30.0);
    assert_eq!(bevel30, bevel, "fixture: the re-cut bevel is the same face");
    let (_, verdict, measured) = fact_of(&m30, &d);
    assert_eq!(verdict, SheetVerdict::Stale, "{measured:?}");
    assert!((measured.expect("re-measured") - 30.0).abs() < 1e-6);
}

/// A conical recess -- a countersink with no hole under it, which the hole
/// recogniser does not claim -- states its INCLUDED angle, 90 deg, the way a
/// countersink is specified; its 45 deg half-angle is not printed. The
/// countersunk hole beside it keeps its callout's `× 90°`.
///
/// Mutation: state a cavity cone by its half-angle (drop the included-angle
/// branch of `cone_angles`) -> `45°` -> RED.
#[test]
fn a_countersink_reads_its_included_angle() {
    use geometry_engine::primitives::surface::Cone;
    let solid = |g: GeometryId| match g {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };
    let cut = |m: &mut BRepModel, key: &str, a: SolidId, b: SolidId| {
        m.set_event_key(Some(key.to_string()));
        let out = boolean_operation(m, a, b, BooleanOp::Difference, BooleanOptions::default())
            .expect(key);
        m.set_event_key(None);
        out
    };
    let mut m = BRepModel::new();
    m.set_event_key(Some("plate".to_string()));
    let mut s = solid(
        TopologyBuilder::new(&mut m)
            .create_box_3d(80.0, 60.0, 20.0)
            .unwrap(),
    );
    m.set_event_key(None);
    // A Ø9 THRU hole countersunk Ø18 × 90° from the top face, at x = -20.
    let bore = solid(
        TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(-20.0, 0.0, -15.0), Vector3::Z, 4.5, 30.0)
            .unwrap(),
    );
    s = cut(&mut m, "bore", s, bore);
    let csk = solid(
        TopologyBuilder::new(&mut m)
            .create_cone_3d(Point3::new(-20.0, 0.0, 5.5), Vector3::Z, 4.5, 9.0, 4.5)
            .unwrap(),
    );
    s = cut(&mut m, "countersink", s, csk);
    // A Ø16 × 90° conical recess from the top face, its point 8 deep.
    let recess = solid(
        TopologyBuilder::new(&mut m)
            .create_cone_3d(Point3::new(20.0, 0.0, 2.0), Vector3::Z, 0.0, 8.0, 8.0)
            .unwrap(),
    );
    s = cut(&mut m, "recess", s, recess);
    let cones: Vec<u32> = solid_faces(&m, s)
        .into_iter()
        .filter(|&f| {
            let face = m.faces.get(f).expect("face");
            m.surfaces
                .get(face.surface_id)
                .and_then(|surf| surf.as_any().downcast_ref::<Cone>())
                .is_some()
        })
        .collect();
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let all = all_labels(&d);
    let angles = cone_angles(&d, &cones);
    assert_eq!(angles.len(), 1, "the recess's angle, once: {all:?}");
    assert_eq!(angles[0].label, "90\u{00B0}", "{all:?}");
    assert!((angles[0].value - 90.0).abs() < 1e-6, "{all:?}");
    let stacks = &d
        .hole_features
        .as_ref()
        .expect("the hole is a stack")
        .stacks;
    assert!(
        stacks
            .iter()
            .any(|st| st.label().ends_with("\u{00D8}18.00 \u{00D7} 90\u{00B0}")),
        "the countersunk hole's callout keeps its included angle: {:?}",
        stacks.iter().map(|st| st.label()).collect::<Vec<_>>()
    );
    assert!(
        !all.iter()
            .any(|l| l.contains('\u{2220}') || l.contains("45\u{00B0}")),
        "{all:?}"
    );
}

/// The space map of a real sheet: every placed dimension is held by exactly
/// one space (none unassigned, none counted twice), every space hangs from a
/// space that exists, the grade is the worst space's score, and the same part
/// laid out twice gives the same ids, boxes and scores. The flange and the
/// rib, drawn first angle as the agent drew them (the rib carries its
/// ordinate table).
#[test]
fn the_space_map_holds_every_placed_dimension_once_with_stable_ids() {
    use geometry_engine::drawing::spaces::{space_map, SpaceKind};
    type Build = fn() -> (BRepModel, SolidId);
    let parts: [(&str, Build); 2] = [
        ("flange", en1092_weld_neck_with_gdt),
        ("rib", naca2412_rib_with_gdt),
    ];
    for (what, build) in parts {
        let sheet = || {
            let (mut m, s) = build();
            let _ = frame_for_drawing(&mut m, s);
            standard_drawing_auto_with_projection(&m, s, uuid::Uuid::nil(), ProjectionAngle::First)
                .expect("sheet")
        };
        let d1 = sheet();
        let l1 = compute_layout(&d1);
        let map = space_map(&d1, &l1);
        assert!(!l1.dimensions.is_empty(), "{what}: a dimensioned sheet");
        let mut held = vec![0usize; l1.dimensions.len()];
        for s in &map.spaces {
            assert_eq!(
                s.contents.placements.len(),
                s.contents.dimensions.len(),
                "{what}: {}",
                s.id
            );
            for &k in &s.contents.placements {
                held[k] += 1;
            }
            if let Some(p) = &s.parent {
                assert!(map.space(p).is_some(), "{what}: {} hangs from {p}", s.id);
            }
        }
        let wrong: Vec<(usize, usize, String)> = held
            .iter()
            .enumerate()
            .filter(|(_, n)| **n != 1)
            .map(|(k, n)| (k, *n, l1.dimensions[k].label.clone()))
            .collect();
        assert!(wrong.is_empty(), "{what}: held != once: {wrong:?}");
        let worst = map
            .spaces
            .iter()
            .map(|s| s.score)
            .fold(f64::INFINITY, f64::min);
        assert_eq!(map.grade, worst, "{what}: the grade is the worst space");
        for kind in [
            SpaceKind::Frame,
            SpaceKind::TitleBlock,
            SpaceKind::NotesStrip,
            SpaceKind::ViewCell,
            SpaceKind::Lane,
        ] {
            assert!(
                map.spaces.iter().any(|s| s.kind == kind),
                "{what}: no {kind:?} space"
            );
        }
        if what == "rib" {
            assert!(
                map.space("table:profile").is_some(),
                "the rib's ordinate table"
            );
        }

        let d2 = sheet();
        let l2 = compute_layout(&d2);
        let item_diff: Vec<String> = l1
            .items
            .iter()
            .zip(&l2.items)
            .filter(|(a, b)| a.bbox != b.bbox || a.text != b.text)
            .map(|(a, b)| format!("{:?} {:?} {:?} vs {:?}", a.kind, a.text, a.bbox, b.bbox))
            .chain(
                l1.dimensions
                    .iter()
                    .zip(&l2.dimensions)
                    .filter(|(a, b)| {
                        a.line != b.line || a.text_anchor != b.text_anchor || a.ext != b.ext
                    })
                    .map(|(a, b)| format!("dim {} {:?} vs {:?}", a.label, a.ext, b.ext)),
            )
            .collect();
        assert!(
            item_diff.is_empty(),
            "{what}: two builds laid out differently: {item_diff:#?}"
        );
        let again = space_map(&d2, &l2);
        let key = |m: &geometry_engine::drawing::spaces::SpaceMap| -> Vec<String> {
            m.spaces
                .iter()
                .map(|s| {
                    format!(
                        "{} {:.6} {:.6} {:.6} {:.6} {:.9} {:?}",
                        s.id,
                        s.rect.x0,
                        s.rect.y0,
                        s.rect.x1,
                        s.rect.y1,
                        s.score,
                        s.contents.dimensions
                    )
                })
                .collect()
        };
        assert_eq!(
            key(&map),
            key(&again),
            "{what}: the same sheet, the same map"
        );
        eprintln!(
            "{what}: {} grade {:.3} below_bar {} of {} spaces, by rule {:?}",
            d1.sheet_size.label(),
            map.grade,
            map.below_bar,
            map.spaces.len(),
            map.by_rule
        );
    }
}

// ── Eyes (agent eyes slice 2): the sheet, or one space of it, as a PNG ────

/// The flange's or the rib's first-angle sheet as the agent drew it, laid out
/// and mapped.
fn mapped_sheet(
    build: fn() -> (BRepModel, SolidId),
) -> (
    Drawing,
    geometry_engine::drawing::layout::SheetLayout,
    geometry_engine::drawing::spaces::SpaceMap,
) {
    let (mut m, s) = build();
    let _ = frame_for_drawing(&mut m, s);
    let d = standard_drawing_auto_with_projection(&m, s, uuid::Uuid::nil(), ProjectionAngle::First)
        .expect("sheet");
    let layout = compute_layout(&d);
    let map = geometry_engine::drawing::spaces::space_map(&d, &layout);
    (d, layout, map)
}

const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

fn decode_png(png: &[u8]) -> tiny_skia::Pixmap {
    assert!(png.starts_with(&PNG_MAGIC), "PNG magic bytes");
    tiny_skia::Pixmap::decode_png(png).expect("a decodable PNG")
}

/// Dark ink on opaque paper -- black, grey, or black under the translucent
/// finding mark -- never the outline's colour.
fn is_ink(p: tiny_skia::PremultipliedColorU8) -> bool {
    let lum =
        0.299 * f64::from(p.red()) + 0.587 * f64::from(p.green()) + 0.114 * f64::from(p.blue());
    p.alpha() == 255 && lum < 110.0 && !is_outline(p)
}

fn is_white(p: tiny_skia::PremultipliedColorU8) -> bool {
    p.alpha() == 255 && p.red() > 240 && p.green() > 240 && p.blue() > 240
}

fn is_outline(p: tiny_skia::PremultipliedColorU8) -> bool {
    use geometry_engine::drawing::raster::OUTLINE_RGB;
    let near = |a: u8, b: u8| a.abs_diff(b) < 50;
    p.alpha() == 255
        && near(p.red(), OUTLINE_RGB[0])
        && near(p.green(), OUTLINE_RGB[1])
        && near(p.blue(), OUTLINE_RGB[2])
}

/// The share of the pixels of paper box `r` (mm) that pass `test`, in an
/// image of paper `crop` at `s` px per mm.
fn share_in(
    pm: &tiny_skia::Pixmap,
    crop: &geometry_engine::drawing::layout::Rect2,
    s: f64,
    r: &geometry_engine::drawing::layout::Rect2,
    test: fn(tiny_skia::PremultipliedColorU8) -> bool,
) -> f64 {
    let px = |mm: f64, o: f64, n: u32| (((mm - o) * s).round().max(0.0) as u32).min(n);
    let (x0, x1) = (px(r.x0, crop.x0, pm.width()), px(r.x1, crop.x0, pm.width()));
    let (y0, y1) = (
        px(r.y0, crop.y0, pm.height()),
        px(r.y1, crop.y0, pm.height()),
    );
    let (mut hit, mut all) = (0usize, 0usize);
    for y in y0..y1 {
        for x in x0..x1 {
            if let Some(p) = pm.pixel(x, y) {
                all += 1;
                if test(p) {
                    hit += 1;
                }
            }
        }
    }
    assert!(all > 0, "the box {r:?} lies inside the image");
    hit as f64 / all as f64
}

/// The whole sheet renders as a valid PNG whose long edge is the pixels asked
/// and whose aspect is the paper's, on white paper carrying ink.
#[test]
fn the_whole_sheet_renders_as_a_png_at_the_sheet_aspect() {
    use geometry_engine::drawing::raster::{render_drawing_png, SheetCrop, SHEET_PNG_DEFAULT_PX};
    assert_eq!(SHEET_PNG_DEFAULT_PX, 1600);
    let (d, _, _) = mapped_sheet(en1092_weld_neck_with_gdt);
    let (w, h) = (d.sheet_size.width(), d.sheet_size.height());
    for px in [1600u32, 700] {
        let out = render_drawing_png(&d, &SheetCrop::whole(&d), px).expect("png");
        let pm = decode_png(&out.png);
        assert_eq!((pm.width(), pm.height()), (out.width, out.height));
        let (long, short) = if w >= h {
            (pm.width(), pm.height())
        } else {
            (pm.height(), pm.width())
        };
        assert_eq!(long, px, "the long edge is the pixels asked");
        let want = (f64::from(px) * w.min(h) / w.max(h)).round() as u32;
        assert!(short.abs_diff(want) <= 1, "short edge {short}, want {want}");
        let ink =
            pm.pixels().iter().filter(|p| is_ink(**p)).count() as f64 / pm.pixels().len() as f64;
        let white =
            pm.pixels().iter().filter(|p| is_white(**p)).count() as f64 / pm.pixels().len() as f64;
        assert!(ink > 0.002 && ink < 0.4, "ink share {ink}");
        assert!(white > 0.5, "white paper share {white}");
    }
}

/// A space crop shows the space's box plus its margin -- max(8 mm, 15 % of
/// the longer side), clamped to the paper -- at that box's aspect; the
/// space's outline is drawn ON the space's box; the dimension texts the space
/// holds are inked where the layout puts them; and empty paper stays white.
#[test]
fn a_space_crop_shows_the_space_its_outline_and_its_dimensions() {
    use geometry_engine::drawing::layout::{Rect2, SheetItemKind};
    use geometry_engine::drawing::raster::{render_drawing_png, SheetCrop};
    use geometry_engine::drawing::spaces::SpaceKind;
    let (d, layout, map) = mapped_sheet(en1092_weld_neck_with_gdt);
    let (sw, sh) = (d.sheet_size.width(), d.sheet_size.height());
    let texts_in = |r: &Rect2| -> Vec<Rect2> {
        layout
            .items
            .iter()
            .filter(|i| i.kind == SheetItemKind::DimensionText)
            .map(|i| i.bbox)
            .filter(|b| {
                let (cx, cy) = ((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0);
                cx > r.x0 && cx < r.x1 && cy > r.y0 && cy < r.y1
            })
            .collect()
    };
    let lane = map
        .spaces
        .iter()
        .find(|s| s.kind == SpaceKind::Lane && !texts_in(&s.rect).is_empty())
        .expect("a lane holding dimension text");
    let crop = SheetCrop::of_space(&d, &layout, &map, &lane.id).expect("the lane's crop");
    let out = render_drawing_png(&d, &crop, 1200).expect("png");
    let pm = decode_png(&out.png);
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("look");
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(
        dir.join(format!("flange-{}.png", lane.id.replace([':', '/'], "_"))),
        &out.png,
    )
    .expect("write");

    // The crop, by this test's own arithmetic.
    let r = lane.rect;
    let m = f64::max(8.0, 0.15 * (r.x1 - r.x0).max(r.y1 - r.y0));
    let want = Rect2 {
        x0: (r.x0 - m).max(0.0),
        y0: (r.y0 - m).max(0.0),
        x1: (r.x1 + m).min(sw),
        y1: (r.y1 + m).min(sh),
    };
    let (cw, ch) = (want.x1 - want.x0, want.y1 - want.y0);
    let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
    assert!(
        close(out.crop.x0, want.x0)
            && close(out.crop.y0, want.y0)
            && close(out.crop.x1, want.x1)
            && close(out.crop.y1, want.y1),
        "crop {:?}, want {want:?}",
        out.crop
    );
    assert_eq!(pm.width().max(pm.height()), 1200);
    let aspect = f64::from(pm.width()) / f64::from(pm.height());
    let tol = 1.5 / f64::from(pm.width().min(pm.height()));
    assert!(
        (aspect / (cw / ch) - 1.0).abs() < tol,
        "image aspect {aspect}, crop aspect {}",
        cw / ch
    );

    // The outline sits on the space's box: its four edges are outline-coloured.
    let s = out.px_per_mm;
    let edge = |x0: f64, y0: f64, x1: f64, y1: f64| Rect2 { x0, y0, x1, y1 };
    let band = 1.5 / s;
    for (what, e) in [
        ("top", edge(r.x0, r.y0 - band, r.x1, r.y0 + band)),
        ("bottom", edge(r.x0, r.y1 - band, r.x1, r.y1 + band)),
        ("left", edge(r.x0 - band, r.y0, r.x0 + band, r.y1)),
        ("right", edge(r.x1 - band, r.y0, r.x1 + band, r.y1)),
    ] {
        let on = share_in(&pm, &out.crop, s, &e, is_outline);
        assert!(
            on > 0.5,
            "{}: {what} edge {on:.3} outline-coloured",
            lane.id
        );
    }

    // The space's dimension texts are inked where the layout puts them.
    for b in texts_in(&r) {
        let (dx, dy) = ((b.x1 - b.x0) * 0.1, (b.y1 - b.y0) * 0.1);
        let inner = edge(b.x0 + dx, b.y0 + dy, b.x1 - dx, b.y1 - dy);
        let ink = share_in(&pm, &out.crop, s, &inner, is_ink);
        assert!(ink > 0.03, "{}: text box {b:?} ink {ink:.3}", lane.id);
    }

    // Empty paper is white: a free rectangle's interior.
    let free = map
        .spaces
        .iter()
        .find(|s| s.kind == SpaceKind::Free)
        .expect("free paper on the flange sheet");
    let fc = SheetCrop::of_space(&d, &layout, &map, &free.id).expect("free crop");
    let fo = render_drawing_png(&d, &fc, 800).expect("png");
    let fp = decode_png(&fo.png);
    let f = free.rect;
    let inner = edge(f.x0 + 3.0, f.y0 + 3.0, f.x1 - 3.0, f.y1 - 3.0);
    let white = share_in(&fp, &fo.crop, fo.px_per_mm, &inner, is_white);
    assert!(white > 0.98, "{}: interior white {white:.3}", free.id);
}

/// The long edge is bounded: past the cap or under the floor is refused
/// typed, never silently clamped; an unknown space has no crop; a crop off
/// the paper is refused.
#[test]
fn the_png_size_cap_holds_and_bad_crops_are_refused() {
    use geometry_engine::drawing::layout::Rect2;
    use geometry_engine::drawing::raster::{
        render_drawing_png, RasterError, SheetCrop, SHEET_PNG_MAX_PX,
    };
    assert_eq!(SHEET_PNG_MAX_PX, 4096);
    let (d, layout, map) = mapped_sheet(en1092_weld_neck_with_gdt);
    let whole = SheetCrop::whole(&d);
    for px in [0u32, 63, 4097, u32::MAX] {
        match render_drawing_png(&d, &whole, px) {
            Err(RasterError::PxOutOfRange { requested, max, .. }) => {
                assert_eq!((requested, max), (px, 4096));
            }
            Err(e) => panic!("px {px}: refused, but not as out of range: {e}"),
            Ok(out) => panic!("px {px}: served {} x {}", out.width, out.height),
        }
    }
    let small = render_drawing_png(&d, &whole, 64).expect("the floor is served");
    assert_eq!(small.width.max(small.height), 64);
    assert!(SheetCrop::of_space(&d, &layout, &map, "view:NOPE").is_none());
    let off = SheetCrop {
        rect: Rect2 {
            x0: 5000.0,
            y0: 5000.0,
            x1: 5100.0,
            y1: 5100.0,
        },
        outline: None,
        marks: Vec::new(),
    };
    assert!(matches!(
        render_drawing_png(&d, &off, 800),
        Err(RasterError::EmptyCrop(_))
    ));
}

/// The pictures to LOOK at: the flange's and the rib's whole sheet and their
/// worst space, written under the test target dir.
#[test]
fn the_look_pictures_of_the_flange_and_the_rib() {
    use geometry_engine::drawing::raster::{render_drawing_png, SheetCrop};
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("look");
    std::fs::create_dir_all(&dir).expect("dir");
    type Build = fn() -> (BRepModel, SolidId);
    let parts: [(&str, Build); 2] = [
        ("flange", en1092_weld_neck_with_gdt),
        ("rib", naca2412_rib_with_gdt),
    ];
    for (what, build) in parts {
        let (d, layout, map) = mapped_sheet(build);
        let whole = render_drawing_png(&d, &SheetCrop::whole(&d), 1600).expect("png");
        std::fs::write(dir.join(format!("{what}-sheet.png")), &whole.png).expect("write");
        let worst = map.worst.clone().expect("a worst space");
        let crop = SheetCrop::of_space(&d, &layout, &map, &worst).expect("crop");
        let look = render_drawing_png(&d, &crop, 1600).expect("png");
        let name = worst.replace([':', '/'], "_");
        std::fs::write(dir.join(format!("{what}-{name}.png")), &look.png).expect("write");
        let space = map.space(&worst).expect("space");
        eprintln!(
            "{what}: worst {worst} score {:.3} rect {:?} crop {:?} {}x{} marks {} findings {:?}",
            space.score,
            space.rect,
            look.crop,
            look.width,
            look.height,
            crop.marks.len(),
            space
                .findings
                .iter()
                .map(|f| format!("{:?} {:.2} {}", f.rule, f.measured, f.message))
                .collect::<Vec<_>>()
        );
    }
    eprintln!("look pictures in {}", dir.display());
}
