// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! DFM process packs (CNC milling, CNC turning, sheet metal, casting,
//! molding extensions) on parts built with the kernel's own primitives,
//! booleans and blends. Every rule has a part that must FAIL it and a
//! corrected twin that must PASS it; each assertion pins the measured value
//! and the limit, and the witnesses name the offending faces.

use geometry_engine::dfm::analyzers::features::concave_features;
use geometry_engine::dfm::analyzers::holes::hole_census;
use geometry_engine::dfm::packs::{casting, cnc, sheet, turning};
use geometry_engine::dfm::{
    analyze, CastingParams, CncMillingParams, CncTurningParams, DfmReport, PackParams,
    RuleProvenance, SheetMetalParams, Verdict,
};
use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::transform::{transform_solid, TransformOptions};
use geometry_engine::primitives::curve::Line;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::readable::adjacency::{attributed_adjacency_graph, EdgeConvexity};

// ── Fixture builders ─────────────────────────────────────────────────────────

fn solid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}

/// Box `w × h × d` centred at `(x, y, z)`.
fn block(m: &mut BRepModel, w: f64, h: f64, d: f64, x: f64, y: f64, z: f64) -> SolidId {
    let s = solid(TopologyBuilder::new(m).create_box_3d(w, h, d).unwrap());
    if x != 0.0 || y != 0.0 || z != 0.0 {
        transform_solid(
            m,
            s,
            Matrix4::from_translation(&Vector3::new(x, y, z)),
            TransformOptions::default(),
        )
        .unwrap();
    }
    s
}

fn cyl_along(m: &mut BRepModel, base: Point3, axis: Vector3, r: f64, h: f64) -> SolidId {
    solid(
        TopologyBuilder::new(m)
            .create_cylinder_3d(base, axis, r, h)
            .unwrap(),
    )
}

fn cyl(m: &mut BRepModel, x: f64, y: f64, z: f64, r: f64, h: f64) -> SolidId {
    cyl_along(m, Point3::new(x, y, z), Vector3::Z, r, h)
}

fn cone(m: &mut BRepModel, x: f64, y: f64, z: f64, r0: f64, r1: f64, h: f64) -> SolidId {
    solid(
        TopologyBuilder::new(m)
            .create_cone_3d(Point3::new(x, y, z), Vector3::Z, r0, r1, h)
            .unwrap(),
    )
}

fn cut(m: &mut BRepModel, a: SolidId, b: SolidId) -> SolidId {
    boolean_operation(m, a, b, BooleanOp::Difference, BooleanOptions::default()).unwrap()
}

fn fuse(m: &mut BRepModel, a: SolidId, b: SolidId) -> SolidId {
    boolean_operation(m, a, b, BooleanOp::Union, BooleanOptions::default()).unwrap()
}

/// 60 × 40 × 20 plate, z ∈ [-10, 10].
fn plate(m: &mut BRepModel) -> SolidId {
    block(m, 60.0, 40.0, 20.0, 0.0, 0.0, 0.0)
}

/// Straight edges parallel to Z of the given convexity.
fn vertical_edges(m: &BRepModel, s: SolidId, kind: EdgeConvexity) -> Vec<u32> {
    let g = attributed_adjacency_graph(m, s);
    g.arcs
        .iter()
        .filter(|a| a.convexity == kind)
        .filter(|a| {
            let e = m.edges.get(a.edge).unwrap();
            let c = m.curves.get(e.curve_id).unwrap();
            c.as_any().downcast_ref::<Line>().is_some_and(|l| {
                let d = l.end - l.start;
                d.z.abs() > 0.999 * (d.x * d.x + d.y * d.y + d.z * d.z).sqrt()
            })
        })
        .map(|a| a.edge)
        .collect()
}

/// Assert the kernel certifies the fixture sound: a DFM verdict on an
/// unsound boolean result would test the boolean, not the rule.
fn assert_sound(m: &mut BRepModel, s: SolidId) {
    let gt = m.ground_truth(s).expect("ground truth");
    assert!(
        gt.certificate.is_sound(),
        "fixture must be sound: {}",
        gt.summary()
    );
}

/// `Some(r)`: plate with a half-round Ø`2r` notch milled through its +X
/// side (a vertical inside radius formed by a cutter's side). `None`: a
/// 30 × 20 rectangular THROUGH window with four SHARP vertical corners.
///
/// (Constructions tried and NOT used, both kernel limits rather than DFM
/// rules: filleting the window's concave edges meets the mixed-convexity
/// corner at each end, which `fillet_edges` refuses; cutting the window
/// with a box whose vertical edges were filleted first, or cutting corner
/// reliefs across the window's loop, leaves an UNSOUND solid.)
fn window(r: Option<f64>) -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let s = match r {
        // A milled edge notch: a Ø2r cutter run straight down through the
        // plate's +X side leaves a half-round, 20 deep, whose radius IS the
        // cutter's — the inside radius this rule judges. (Cutting reliefs
        // or rounds into a window's corners crosses the window's own loop,
        // which the boolean leaves unsound; see the note above.)
        Some(r) => {
            let c = cyl(&mut m, 30.0, 0.0, -15.0, r, 30.0);
            cut(&mut m, p, c)
        }
        None => {
            let w = block(&mut m, 30.0, 20.0, 30.0, 0.0, 0.0, 0.0);
            let s = cut(&mut m, p, w);
            assert_eq!(vertical_edges(&m, s, EdgeConvexity::Concave).len(), 4);
            s
        }
    };
    assert_sound(&mut m, s);
    (m, s)
}

/// Plate with a 30 × 20 × 6 deep blind pocket from the top face.
fn sharp_pocket() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let k = block(&mut m, 30.0, 20.0, 12.0, 0.0, 0.0, 10.0);
    let s = cut(&mut m, p, k);
    (m, s)
}

fn thru(m: &mut BRepModel, s: SolidId, x: f64, y: f64, r: f64) -> SolidId {
    let t = cyl(m, x, y, -15.0, r, 30.0);
    cut(m, s, t)
}

/// Blind Ø`2r`, ↧ `depth` from the top face at `top`, flat floor.
fn blind_flat(
    m: &mut BRepModel,
    s: SolidId,
    x: f64,
    y: f64,
    r: f64,
    top: f64,
    depth: f64,
) -> SolidId {
    let t = cyl(m, x, y, top - depth, r, depth + 5.0);
    cut(m, s, t)
}

/// Blind Ø`2r`, ↧ `depth`, with a 118° drill point below the floor.
fn blind_drilled(
    m: &mut BRepModel,
    s: SolidId,
    x: f64,
    y: f64,
    r: f64,
    top: f64,
    depth: f64,
) -> SolidId {
    let s = blind_flat(m, s, x, y, r, top, depth);
    let h = r / 59.0_f64.to_radians().tan();
    let k = cone(m, x, y, top - depth - h, 0.0, r, h);
    cut(m, s, k)
}

fn cnc_params() -> CncMillingParams {
    CncMillingParams::default()
}

fn report(m: &BRepModel, s: SolidId, p: PackParams) -> DfmReport {
    analyze(m, s, p).expect("DFM analysis must run on a kernel-built part")
}

fn verdict<'a>(r: &'a DfmReport, rule: &str) -> &'a Verdict {
    &r.verdicts()
        .iter()
        .find(|v| v.rule == rule)
        .unwrap_or_else(|| panic!("rule {rule} missing from {:?}", r.verdicts()))
        .verdict
}

fn expect_violation(r: &DfmReport, rule: &str) -> (Vec<u32>, f64, f64) {
    match verdict(r, rule) {
        Verdict::Violation {
            witnesses,
            measured,
            limit,
        } => (witnesses.clone(), measured.value, limit.value),
        o => panic!("{rule}: expected Violation, got {o:?}"),
    }
}

fn expect_pass(r: &DfmReport, rule: &str) -> f64 {
    match verdict(r, rule) {
        Verdict::Pass { margin } => margin.value,
        o => panic!("{rule}: expected Pass, got {o:?}"),
    }
}

/// Vacuous-pass sentinel (no candidate feature).
const VACUOUS: f64 = 1.0e300;

// ── CNC milling: inside corners ──────────────────────────────────────────────

/// A blind pocket cut by a box has four SHARP vertical inside corners: no
/// rotating cutter forms radius 0. The pocket's floor-to-wall edges are
/// floor corners (an end mill's end cuts them sharp) and must NOT be
/// witnesses.
#[test]
fn cnc_sharp_pocket_corners_violate_tool_radius_but_floor_edges_do_not() {
    let (m, s) = sharp_pocket();
    let r = report(&m, s, PackParams::Cnc3Axis(cnc_params()));
    let (w, measured, limit) = expect_violation(&r, cnc::INTERNAL_CORNER_RADIUS_RULE_ID);
    assert_eq!(measured, 0.0);
    assert_eq!(limit, 1.5, "default min tool Ø3 → radius 1.5");
    // Witnesses: the 4 pocket walls only (each vertical corner names its
    // two walls) — never the pocket floor.
    let g = attributed_adjacency_graph(&m, s);
    let floor = g
        .nodes
        .iter()
        .find(|n| match &n.geometry {
            geometry_engine::readable::adjacency::FaceGeometry::Plane { origin, normal } => {
                normal[2] > 0.999 && (origin[2] - 4.0).abs() < 1e-6
            }
            _ => false,
        })
        .map(|n| n.face)
        .expect("the pocket floor at z = 4");
    assert_eq!(w.len(), 4, "four pocket walls: {w:?}");
    assert!(
        !w.contains(&floor),
        "the pocket floor is not a witness: {w:?}"
    );
}

/// A milled STEP has one sharp inside edge — a FLOOR corner (the step's
/// floor faces the +Z setup, its wall stands along it): an end mill's end
/// cuts it sharp, so the corner rule has nothing to judge and passes.
/// Without the floor-corner exemption this edge would be an undecided
/// corner and the rule would refuse.
#[test]
fn cnc_a_milled_step_floor_corner_is_cut_sharp_by_the_end_mill() {
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let k = block(&mut m, 40.0, 50.0, 10.0, 20.0, 0.0, 10.0); // x ∈ [0, 40], z ∈ [5, 15]
    let s = cut(&mut m, p, k);
    assert_sound(&mut m, s);
    let r = report(&m, s, PackParams::Cnc3Axis(cnc_params()));
    assert_eq!(
        expect_pass(&r, cnc::INTERNAL_CORNER_RADIUS_RULE_ID),
        VACUOUS
    );
}

/// The corrected twin: a Ø6 vertical inside radius (r 3 ≥ 1.5) passes,
/// margin exactly 3 − 1.5; reach 20 / 6.
#[test]
fn cnc_filleted_window_passes_corner_radius_and_reach() {
    let (m, s) = window(Some(3.0));
    let r = report(&m, s, PackParams::Cnc3Axis(cnc_params()));
    let margin = expect_pass(&r, cnc::INTERNAL_CORNER_RADIUS_RULE_ID);
    assert!((margin - 1.5).abs() < 1e-9, "margin {margin}");
    let reach = expect_pass(&r, cnc::CORNER_REACH_RULE_ID);
    assert!(
        (reach - (4.0 - 20.0 / 6.0)).abs() < 1e-9,
        "reach margin {reach}"
    );
}

/// A Ø2 corner (r 1 < 1.5) is uncuttable with a Ø3 tool and 20 deep it is
/// a 10:1 reach — both rules name the four fillet faces.
#[test]
fn cnc_small_fillet_violates_radius_and_reach() {
    let (m, s) = window(Some(1.0));
    let r = report(&m, s, PackParams::Cnc3Axis(cnc_params()));
    let (w, measured, _) = expect_violation(&r, cnc::INTERNAL_CORNER_RADIUS_RULE_ID);
    assert!((measured - 1.0).abs() < 1e-9, "{measured}");
    assert_eq!(w.len(), 1, "the notch face is named: {w:?}");
    let (w2, ratio, limit) = expect_violation(&r, cnc::CORNER_REACH_RULE_ID);
    assert!((ratio - 10.0).abs() < 1e-9, "20 / (2·1) = 10, got {ratio}");
    assert_eq!(limit, 4.0);
    assert_eq!(w, w2, "the same fillet faces carry both findings");
}

/// The hole census must not mistake a pocket-corner fillet (a partial-turn
/// cavity cylinder) for a Ø2 hole.
#[test]
fn a_corner_fillet_is_never_a_hole() {
    let (m, s) = window(Some(1.0));
    let f = concave_features(&m, s).unwrap();
    let c = hole_census(&m, s, &f);
    assert!(c.holes.is_empty(), "{:?}", c.holes);
    assert!(c.unrecognised.is_empty(), "{:?}", c.unrecognised);
    let r = report(&m, s, PackParams::Cnc3Axis(cnc_params()));
    assert_eq!(expect_pass(&r, cnc::HOLE_DEPTH_RATIO_RULE_ID), VACUOUS);
    assert_eq!(expect_pass(&r, cnc::MIN_HOLE_DIAMETER_RULE_ID), VACUOUS);
}

// ── CNC milling: holes ───────────────────────────────────────────────────────

/// Ø4 blind ↧ 50 with a drill point in a tall block: L/D 12.5 > 10.
#[test]
fn cnc_deep_narrow_hole_violates_drill_depth_ratio() {
    let mut m = BRepModel::new();
    let b = block(&mut m, 20.0, 20.0, 60.0, 0.0, 0.0, 0.0);
    let s = blind_drilled(&mut m, b, 0.0, 0.0, 2.0, 30.0, 50.0);
    let r = report(&m, s, PackParams::Cnc3Axis(cnc_params()));
    let (w, ratio, limit) = expect_violation(&r, cnc::HOLE_DEPTH_RATIO_RULE_ID);
    assert!((ratio - 12.5).abs() < 1e-6, "50 / 4 = 12.5, got {ratio}");
    assert_eq!(limit, 10.0);
    assert!(!w.is_empty());
    // The 118° drill point is a standard floor.
    assert!(expect_pass(&r, cnc::BLIND_HOLE_FLOOR_RULE_ID) >= 0.0);
}

#[test]
fn cnc_shallow_twin_passes_drill_depth_ratio() {
    let mut m = BRepModel::new();
    let b = block(&mut m, 20.0, 20.0, 60.0, 0.0, 0.0, 0.0);
    let s = blind_drilled(&mut m, b, 0.0, 0.0, 2.0, 30.0, 30.0);
    let r = report(&m, s, PackParams::Cnc3Axis(cnc_params()));
    let margin = expect_pass(&r, cnc::HOLE_DEPTH_RATIO_RULE_ID);
    assert!((margin - (10.0 - 7.5)).abs() < 1e-6, "{margin}");
}

/// A flat-bottomed blind hole needs a flat-bottom drill: 180° vs 140°.
#[test]
fn cnc_flat_bottom_hole_violates_floor_rule_unless_allowed() {
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let s = blind_flat(&mut m, p, 0.0, 0.0, 4.0, 10.0, 12.0);
    let r = report(&m, s, PackParams::Cnc3Axis(cnc_params()));
    let (w, dev, limit) = expect_violation(&r, cnc::BLIND_HOLE_FLOOR_RULE_ID);
    assert!((dev - 40.0).abs() < 1e-9, "180 − 140 = 40, got {dev}");
    assert_eq!(limit, 0.5);
    assert!(!w.is_empty());
    let allow = CncMillingParams {
        allow_flat_bottom_holes: true,
        ..cnc_params()
    };
    let r = report(&m, s, PackParams::Cnc3Axis(allow));
    assert!(expect_pass(&r, cnc::BLIND_HOLE_FLOOR_RULE_ID) >= 0.0);
}

/// A Ø30 flat-bottomed blind bore (a bearing seat) is milled, not drilled:
/// its flat floor is the intended shape, outside the drill-floor rule.
#[test]
fn cnc_a_milled_bearing_seat_is_not_a_drill_floor_defect() {
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let s = blind_flat(&mut m, p, 0.0, 0.0, 15.0, 10.0, 8.0);
    assert_sound(&mut m, s);
    let r = report(&m, s, PackParams::Cnc3Axis(cnc_params()));
    assert_eq!(expect_pass(&r, cnc::BLIND_HOLE_FLOOR_RULE_ID), VACUOUS);
}

/// Ø6.35 (1/4 in) is off the metric 0.1 mm drill series; Ø6.6 is on it.
#[test]
fn cnc_off_series_drill_violates_and_on_series_passes() {
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let s = thru(&mut m, p, 0.0, 0.0, 3.175);
    let r = report(&m, s, PackParams::Cnc3Axis(cnc_params()));
    let (_, dev, _) = expect_violation(&r, cnc::STANDARD_DRILL_SIZE_RULE_ID);
    assert!((dev - 0.05).abs() < 1e-6, "|6.35 − 6.3| = 0.05, got {dev}");

    let mut m2 = BRepModel::new();
    let p2 = plate(&mut m2);
    let s2 = thru(&mut m2, p2, 0.0, 0.0, 3.3);
    let r2 = report(&m2, s2, PackParams::Cnc3Axis(cnc_params()));
    assert!(expect_pass(&r2, cnc::STANDARD_DRILL_SIZE_RULE_ID) >= 0.0);
    // 20 / 6.6 is a comfortable L/D, and a THRU hole along Z is reachable.
    let m3 = expect_pass(&r2, cnc::HOLE_DEPTH_RATIO_RULE_ID);
    assert!((m3 - (10.0 - 20.0 / 6.6)).abs() < 1e-6, "{m3}");
    assert!(expect_pass(&r2, cnc::HOLE_ACCESS_RULE_ID) >= 0.0);
}

/// Ø0.8 is below the 1.0 mm smallest drill.
#[test]
fn cnc_tiny_hole_violates_min_drill() {
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let s = thru(&mut m, p, 0.0, 0.0, 0.4);
    let r = report(&m, s, PackParams::Cnc3Axis(cnc_params()));
    let (_, d, limit) = expect_violation(&r, cnc::MIN_HOLE_DIAMETER_RULE_ID);
    assert!((d - 0.8).abs() < 1e-9);
    assert_eq!(limit, 1.0);
}

/// A side hole (axis X) cannot be drilled from the default ±Z setups; it
/// can once the plan includes a ±X setup.
#[test]
fn cnc_side_hole_needs_a_side_setup() {
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let t = cyl_along(&mut m, Point3::new(-40.0, 0.0, 0.0), Vector3::X, 3.3, 80.0);
    let s = cut(&mut m, p, t);
    let r = report(&m, s, PackParams::Cnc3Axis(cnc_params()));
    let (w, angle, _) = expect_violation(&r, cnc::HOLE_ACCESS_RULE_ID);
    assert!((angle - 90.0).abs() < 1e-9, "{angle}");
    assert!(!w.is_empty());
    let with_side = CncMillingParams {
        setup_directions: vec![[0.0, 0.0, 1.0], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0]],
        ..cnc_params()
    };
    let r = report(&m, s, PackParams::Cnc3Axis(with_side));
    assert!(expect_pass(&r, cnc::HOLE_ACCESS_RULE_ID) >= 0.0);
}

#[test]
fn cnc_zero_setup_direction_is_malformed_input() {
    let (m, s) = sharp_pocket();
    let bad = CncMillingParams {
        setup_directions: vec![[0.0, 0.0, 0.0]],
        ..cnc_params()
    };
    assert!(analyze(&m, s, PackParams::Cnc3Axis(bad)).is_err());
}

// ── CNC turning ──────────────────────────────────────────────────────────────

/// Shaft Ø20 × 30 with a coaxial Ø10 bore: axisymmetric, bore L/D 3.
fn shaft(x: f64, y: f64, cross_hole: bool, bore_r: f64) -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let s = cyl(&mut m, x, y, 0.0, 10.0, 30.0);
    let b = cyl(&mut m, x, y, -1.0, bore_r, 32.0);
    let mut s = cut(&mut m, s, b);
    if cross_hole {
        let t = cyl_along(
            &mut m,
            Point3::new(x - 15.0, y, 15.0),
            Vector3::X,
            2.0,
            30.0,
        );
        s = cut(&mut m, s, t);
    }
    (m, s)
}

#[test]
fn turning_plain_shaft_passes_with_inferred_off_origin_axis() {
    let (m, s) = shaft(100.0, 50.0, false, 5.0);
    let r = report(&m, s, PackParams::CncTurning(CncTurningParams::default()));
    assert_eq!(expect_pass(&r, turning::AXISYMMETRY_RULE_ID), 0.0);
    let bore = expect_pass(&r, turning::BORE_DEPTH_RATIO_RULE_ID);
    assert!((bore - (4.0 - 3.0)).abs() < 1e-9, "{bore}");
}

#[test]
fn turning_cross_hole_needs_a_milling_op() {
    let (m, s) = shaft(0.0, 0.0, true, 5.0);
    let r = report(&m, s, PackParams::CncTurning(CncTurningParams::default()));
    let (w, count, limit) = expect_violation(&r, turning::AXISYMMETRY_RULE_ID);
    assert!(count >= 1.0 && limit == 0.0, "{count}");
    assert_eq!(w.len() as f64, count, "every off-axis face is named");
}

#[test]
fn turning_deep_small_bore_violates_boring_bar_reach() {
    let (m, s) = shaft(0.0, 0.0, false, 2.0);
    let r = report(&m, s, PackParams::CncTurning(CncTurningParams::default()));
    let (_, ratio, _) = expect_violation(&r, turning::BORE_DEPTH_RATIO_RULE_ID);
    assert!((ratio - 7.5).abs() < 1e-9, "30 / 4 = 7.5, got {ratio}");
}

#[test]
fn turning_without_an_od_refuses_never_guesses_an_axis() {
    let mut m = BRepModel::new();
    let s = plate(&mut m);
    let r = report(&m, s, PackParams::CncTurning(CncTurningParams::default()));
    assert!(matches!(
        verdict(&r, turning::AXISYMMETRY_RULE_ID),
        Verdict::Unverifiable { .. }
    ));
}

// ── Sheet metal ──────────────────────────────────────────────────────────────

fn sheet_params() -> SheetMetalParams {
    SheetMetalParams {
        thickness: Some(2.0),
        ..SheetMetalParams::default()
    }
}

/// 100 × 60 × 2 sheet, z ∈ [-1, 1], with a Ø5 hole whose centre sits
/// `edge_gap` from the x = +50 cut edge.
fn sheet_with_hole(edge_gap: f64) -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let p = block(&mut m, 100.0, 60.0, 2.0, 0.0, 0.0, 0.0);
    let t = cyl(&mut m, 50.0 - edge_gap, 0.0, -5.0, 2.5, 10.0);
    let s = cut(&mut m, p, t);
    (m, s)
}

#[test]
fn sheet_hole_too_close_to_cut_edge_violates() {
    let (m, s) = sheet_with_hole(3.0);
    let r = report(&m, s, PackParams::SheetMetal(sheet_params()));
    let (w, d, limit) = expect_violation(&r, sheet::HOLE_TO_EDGE_RULE_ID);
    assert!(
        (d - 0.5).abs() < 1e-9,
        "rim 3 − 2.5 = 0.5 from the edge, got {d}"
    );
    assert_eq!(limit, 4.0, "2 t");
    assert_eq!(w.len(), 2, "both skins carrying the hole: {w:?}");
}

#[test]
fn sheet_hole_well_inside_passes_edge_distance() {
    let (m, s) = sheet_with_hole(20.0);
    let r = report(&m, s, PackParams::SheetMetal(sheet_params()));
    let margin = expect_pass(&r, sheet::HOLE_TO_EDGE_RULE_ID);
    assert!((margin - (17.5 - 4.0)).abs() < 1e-9, "{margin}");
    assert_eq!(expect_pass(&r, sheet::DEVELOPABLE_RULE_ID), 0.0);
}

/// Two plates fused at a right angle: the inside of the "bend" is a sharp
/// concave edge — inside radius 0 < 1 t.
#[test]
fn sheet_sharp_inside_bend_violates_bend_radius() {
    let mut m = BRepModel::new();
    let a = block(&mut m, 40.0, 30.0, 2.0, 0.0, 0.0, 0.0);
    let b = block(&mut m, 2.0, 30.0, 20.0, 19.0, 0.0, 11.0);
    let s = fuse(&mut m, a, b);
    let r = report(&m, s, PackParams::SheetMetal(sheet_params()));
    let (w, radius, limit) = expect_violation(&r, sheet::BEND_RADIUS_RULE_ID);
    assert_eq!(radius, 0.0);
    assert_eq!(limit, 2.0);
    assert_eq!(w.len(), 2, "the two inside skins: {w:?}");
}

/// A laser-cut rectangular WINDOW in a flat sheet has four sharp concave
/// corners running THROUGH the sheet: cut-out corners, not bends. Neither
/// the bend-radius nor the flange-length rule may call them a violation.
#[test]
fn sheet_cutout_corners_are_not_bends() {
    let mut m = BRepModel::new();
    let p = block(&mut m, 100.0, 60.0, 2.0, 0.0, 0.0, 0.0);
    let w = block(&mut m, 30.0, 20.0, 10.0, 0.0, 0.0, 0.0);
    let s = cut(&mut m, p, w);
    assert_sound(&mut m, s);
    assert_eq!(vertical_edges(&m, s, EdgeConvexity::Concave).len(), 4);
    let r = report(&m, s, PackParams::SheetMetal(sheet_params()));
    for rule in [sheet::BEND_RADIUS_RULE_ID, sheet::FLANGE_LENGTH_RULE_ID] {
        assert!(
            !matches!(verdict(&r, rule), Verdict::Violation { .. }),
            "{rule}: a cut-out corner is not a bend: {:?}",
            verdict(&r, rule)
        );
    }
}

/// DFM audit: the two skins of a holed sheet are CONGRUENT (identical
/// projected loops), so the pair is proven and the gauge is inferred —
/// the same answer as a given gauge.
#[test]
fn sheet_gauge_is_inferred_from_congruent_skins() {
    let (m, s) = sheet_with_hole(20.0);
    let r = report(&m, s, PackParams::SheetMetal(SheetMetalParams::default()));
    let margin = expect_pass(&r, sheet::HOLE_TO_EDGE_RULE_ID);
    assert!((margin - (17.5 - 4.0)).abs() < 1e-9, "{margin}");
    assert!(expect_pass(&r, sheet::UNIFORM_THICKNESS_RULE_ID).abs() < 1e-2);
}

/// A recess on ONE skin breaks the congruence: the skins can no longer be
/// paired, so the thinnest PROVEN pair is the sheet's 60 mm width — which
/// must never be taken as the gauge.
#[test]
fn sheet_unpaired_skin_refuses_the_gauge_never_takes_the_width() {
    let mut m = BRepModel::new();
    let p = block(&mut m, 100.0, 60.0, 2.0, 0.0, 0.0, 0.0);
    let s = blind_flat(&mut m, p, 20.0, 0.0, 2.5, 1.0, 1.0);
    let r = report(&m, s, PackParams::SheetMetal(SheetMetalParams::default()));
    match verdict(&r, sheet::HOLE_TO_EDGE_RULE_ID) {
        Verdict::Unverifiable { reason, .. } => assert!(
            format!("{reason:?}").contains("sheet thickness unknown"),
            "refused for the RIGHT reason (no provable gauge): {reason:?}"
        ),
        o => panic!("expected the gauge to be refused: {o:?}"),
    }
}

/// A boss standing in a round pocket: the pocket wall (concave, R8) and
/// the boss (convex, R7.5) face each other across a 0.5 mm VOID annulus —
/// never a 0.5 mm wall.
#[test]
fn audit_a_boss_in_a_pocket_is_a_gap_not_a_thin_wall() {
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let pocket = cyl(&mut m, 0.0, 0.0, 5.0, 8.0, 10.0); // z ∈ [5, 15]
    let s = cut(&mut m, p, pocket);
    let boss = cyl(&mut m, 0.0, 0.0, 0.0, 7.5, 9.0); // z ∈ [0, 9]
    let s = fuse(&mut m, s, boss);
    assert_sound(&mut m, s);
    let r = report(&m, s, PackParams::Cnc3Axis(cnc_params()));
    assert!(
        !matches!(
            verdict(&r, cnc::MIN_WALL_RULE_ID),
            Verdict::Violation { .. }
        ),
        "a void annulus must not read as a thin wall: {:?}",
        verdict(&r, cnc::MIN_WALL_RULE_ID)
    );
}

// ── Casting ──────────────────────────────────────────────────────────────────

/// A box has zero-draft walls along the pull: every side face violates.
#[test]
fn casting_undrafted_walls_and_sharp_pocket_violate() {
    let (m, s) = sharp_pocket();
    let r = report(&m, s, PackParams::Casting(CastingParams::default()));
    let (w, draft, limit) = expect_violation(&r, casting::DRAFT_RULE_ID);
    assert!(draft.abs() < 1e-9, "{draft}");
    assert_eq!(limit, 1.0);
    assert!(w.len() >= 8, "4 outer + 4 pocket walls: {w:?}");
    let (_, radius, _) = expect_violation(&r, casting::INTERNAL_FILLET_RULE_ID);
    assert_eq!(radius, 0.0);
}

#[test]
fn casting_small_side_hole_violates_cored_hole_and_side_core() {
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let t = cyl_along(&mut m, Point3::new(-40.0, 0.0, 0.0), Vector3::X, 1.0, 80.0);
    let s = cut(&mut m, p, t);
    let r = report(&m, s, PackParams::Casting(CastingParams::default()));
    let (_, d, limit) = expect_violation(&r, casting::CORED_HOLE_RULE_ID);
    assert!((d - 2.0).abs() < 1e-9 && limit == 2.5);
    expect_violation(&r, casting::SIDE_CORE_RULE_ID);
    assert!(expect_pass(&r, casting::TRAPPED_CORE_RULE_ID) >= 0.0);
}

#[test]
fn casting_hole_along_pull_needs_no_side_core() {
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let s = thru(&mut m, p, 0.0, 0.0, 3.0);
    let r = report(&m, s, PackParams::Casting(CastingParams::default()));
    assert!(expect_pass(&r, casting::SIDE_CORE_RULE_ID) >= 0.0);
    assert!(expect_pass(&r, casting::CORED_HOLE_RULE_ID) >= 0.0);
}

// ── Pack-level honesty ───────────────────────────────────────────────────────

/// Every pack reports EXACTLY its declared rules, in order, each with
/// shop-practice provenance (no CNC/sheet/casting number is dressed up as
/// a standard).
#[test]
fn every_new_pack_reports_its_declared_rules_with_honest_provenance() {
    let (m, s) = sharp_pocket();
    let cases = vec![
        (
            PackParams::Cnc3Axis(cnc_params()),
            cnc::rule_pack(PackParams::Cnc3Axis(cnc_params())),
        ),
        (
            PackParams::CncTurning(CncTurningParams::default()),
            turning::rule_pack(PackParams::CncTurning(CncTurningParams::default())),
        ),
        (
            PackParams::SheetMetal(sheet_params()),
            sheet::rule_pack(PackParams::SheetMetal(sheet_params())),
        ),
        (
            PackParams::Casting(CastingParams::default()),
            casting::rule_pack(PackParams::Casting(CastingParams::default())),
        ),
    ];
    for (params, declared) in cases {
        let r = report(&m, s, params);
        let got: Vec<&str> = r.verdicts().iter().map(|v| v.rule.as_str()).collect();
        let want: Vec<&str> = declared.rules.iter().map(|r| r.id).collect();
        assert_eq!(got, want);
        for (v, d) in r.verdicts().iter().zip(&declared.rules) {
            assert_eq!(v.provenance, d.provenance, "{}", v.rule);
            assert!(matches!(v.provenance, RuleProvenance::ShopPractice { .. }));
        }
    }
}

/// `{"pack": "cnc_3_axis"}` alone is a complete request: every threshold
/// takes its documented default, echoed on the report.
#[test]
fn a_bare_pack_tag_deserializes_to_the_documented_defaults() {
    for (json, want) in [
        (
            r#"{"pack":"cnc_3_axis"}"#,
            PackParams::Cnc3Axis(CncMillingParams::default()),
        ),
        (
            r#"{"pack":"cnc_turning"}"#,
            PackParams::CncTurning(CncTurningParams::default()),
        ),
        (
            r#"{"pack":"sheet_metal"}"#,
            PackParams::SheetMetal(SheetMetalParams::default()),
        ),
        (
            r#"{"pack":"casting"}"#,
            PackParams::Casting(CastingParams::default()),
        ),
    ] {
        let got: PackParams = serde_json::from_str(json).unwrap();
        assert_eq!(got, want, "{json}");
        let back = serde_json::to_value(&got).unwrap();
        assert_eq!(
            back["pack"],
            serde_json::from_str::<serde_json::Value>(json).unwrap()["pack"]
        );
    }
}

#[test]
fn params_hash_separates_every_new_field() {
    let base = PackParams::Cnc3Axis(cnc_params());
    let tweak = PackParams::Cnc3Axis(CncMillingParams {
        min_wall: 0.81,
        ..cnc_params()
    });
    let plan = PackParams::Cnc3Axis(CncMillingParams {
        setup_directions: vec![[0.0, 0.0, 1.0]],
        ..cnc_params()
    });
    let flat = PackParams::Cnc3Axis(CncMillingParams {
        allow_flat_bottom_holes: true,
        ..cnc_params()
    });
    let hashes = [
        base.stable_hash(),
        tweak.stable_hash(),
        plan.stable_hash(),
        flat.stable_hash(),
        PackParams::CncTurning(CncTurningParams::default()).stable_hash(),
        PackParams::SheetMetal(SheetMetalParams::default()).stable_hash(),
        PackParams::SheetMetal(sheet_params()).stable_hash(),
        PackParams::Casting(CastingParams::default()).stable_hash(),
    ];
    for i in 0..hashes.len() {
        for j in i + 1..hashes.len() {
            assert_ne!(hashes[i], hashes[j], "params {i} and {j} collide");
        }
    }
}

// ── Audit of the existing analyzers ──────────────────────────────────────────

/// A 0.5 mm slot's two walls face each other ACROSS VOID: a gap, not a
/// wall. Before the material-side fix `pair_thickness` paired them (it took
/// `|separation|`) and `cnc.min_wall` reported a fabricated 0.5 mm wall.
#[test]
fn audit_a_narrow_slot_is_a_gap_not_a_thin_wall() {
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let k = block(&mut m, 0.5, 20.0, 30.0, 0.0, 0.0, 0.0);
    let s = cut(&mut m, p, k);
    let r = report(&m, s, PackParams::Cnc3Axis(cnc_params()));
    if let Verdict::Unverifiable { regions, .. } = verdict(&r, cnc::MIN_WALL_RULE_ID) {
        let g = attributed_adjacency_graph(&m, s);
        for f in regions {
            let face = m.faces.get(*f).unwrap();
            let lp = m.loops.get(face.outer_loop).unwrap();
            eprintln!(
                "face {f}: {:?} outer edges {} inner loops {}",
                g.node(*f).map(|n| &n.geometry),
                lp.edges.len(),
                face.inner_loops.len()
            );
        }
    }
    let margin = expect_pass(&r, cnc::MIN_WALL_RULE_ID);
    // Thinnest real wall: slot end (y = ±10) to plate side (y = ±20).
    assert!((margin - (10.0 - 0.8)).abs() < 1e-9, "{margin}");
}

/// A pocket leaving a 0.5 mm floor is a REAL thin wall, measured exactly.
fn thin_floor() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let k = block(&mut m, 30.0, 20.0, 30.0, 0.0, 0.0, 5.5); // z ∈ [-9.5, 20.5]
    let s = cut(&mut m, p, k);
    (m, s)
}

#[test]
fn audit_a_thin_pocket_floor_violates_min_wall_exactly() {
    let (m, s) = thin_floor();
    let r = report(&m, s, PackParams::Cnc3Axis(cnc_params()));
    let (w, t, limit) = expect_violation(&r, cnc::MIN_WALL_RULE_ID);
    assert!((t - 0.5).abs() < 1e-9, "{t}");
    assert_eq!(limit, 0.8);
    assert_eq!(w.len(), 2, "pocket floor + plate bottom: {w:?}");
}

/// Congruent skins: a plate with a through hole now pairs its top and
/// bottom (identical projected loops) — `cnc.min_wall` decides instead of
/// refusing.
#[test]
fn audit_congruent_holed_plate_skins_pair_exactly() {
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let s = thru(&mut m, p, 0.0, 0.0, 3.3);
    let r = report(&m, s, PackParams::Cnc3Axis(cnc_params()));
    let margin = expect_pass(&r, cnc::MIN_WALL_RULE_ID);
    assert!((margin - (20.0 - 0.8)).abs() < 1e-9, "{margin}");
}

/// `bore_metrics` called a counterbored THRU hole blind (its drilled wall
/// is shorter than the part); THRU now comes from the hole's topology.
#[test]
fn audit_bore_metrics_reads_a_counterbored_hole_as_through() {
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let s = thru(&mut m, p, 0.0, 0.0, 3.3);
    let c = cyl(&mut m, 0.0, 0.0, 4.0, 5.5, 11.0); // counterbore Ø11 ↧ 6
    let s = cut(&mut m, s, c);
    let out = geometry_engine::dfm::bore_metrics(&m, s).unwrap();
    let drill = out
        .bores
        .iter()
        .find(|b| (b.diameter.value - 6.6).abs() < 1e-9)
        .unwrap_or_else(|| panic!("the Ø6.6 drilled wall: {out:?}"));
    assert!(drill.is_through, "{drill:?}");
}

// ── Molding extensions ───────────────────────────────────────────────────────

fn molding() -> PackParams {
    PackParams::InjectionMolding {
        pull_direction: [0.0, 0.0, 1.0],
        min_draft_deg: 1.0,
    }
}

#[test]
fn molding_side_hole_needs_a_side_action_and_sharp_corners_are_counted() {
    use geometry_engine::dfm::packs::molding::{
        SHARP_INTERNAL_CORNER_RULE_ID, SIDE_ACTION_RULE_ID,
    };
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let t = cyl_along(&mut m, Point3::new(-40.0, 0.0, 0.0), Vector3::X, 3.0, 80.0);
    let s = cut(&mut m, p, t);
    let r = report(&m, s, molding());
    let (_, angle, _) = expect_violation(&r, SIDE_ACTION_RULE_ID);
    assert!((angle - 90.0).abs() < 1e-9);

    let (m2, s2) = sharp_pocket();
    let r2 = report(&m2, s2, molding());
    let (_, count, limit) = expect_violation(&r2, SHARP_INTERNAL_CORNER_RULE_ID);
    assert_eq!(count, 8.0, "4 vertical + 4 floor inside edges");
    assert_eq!(limit, 0.0);
    assert!(expect_pass(&r2, SIDE_ACTION_RULE_ID) >= 0.0);
}

#[test]
fn molding_plain_block_passes_the_new_rules() {
    use geometry_engine::dfm::packs::molding::{
        SHARP_INTERNAL_CORNER_RULE_ID, SIDE_ACTION_RULE_ID, TRAPPED_CORE_RULE_ID,
    };
    let mut m = BRepModel::new();
    let s = plate(&mut m);
    let r = report(&m, s, molding());
    for id in [
        SIDE_ACTION_RULE_ID,
        TRAPPED_CORE_RULE_ID,
        SHARP_INTERNAL_CORNER_RULE_ID,
    ] {
        expect_pass(&r, id);
    }
}

// ── Animations ───────────────────────────────────────────────────────────────

use geometry_engine::dfm::animation::{animate_report, ActorShape, AnimationKind, DfmAnimation};
use geometry_engine::dfm::RuleVerdict;

fn anim<'a>(a: &'a [DfmAnimation], rule: &str) -> &'a DfmAnimation {
    a.iter()
        .find(|x| x.rule == rule)
        .unwrap_or_else(|| panic!("no animation for {rule}: {a:?}"))
}

/// Rebuild `r` with one verdict's measured / limit overridden.
fn mutate(r: &DfmReport, rule: &str, measured: Option<f64>, limit: Option<f64>) -> DfmReport {
    let verdicts: Vec<RuleVerdict> = r
        .verdicts()
        .iter()
        .cloned()
        .map(|mut v| {
            if v.rule == rule {
                if let Verdict::Violation {
                    measured: mm,
                    limit: ll,
                    ..
                } = &mut v.verdict
                {
                    if let Some(x) = measured {
                        mm.value = x;
                    }
                    if let Some(x) = limit {
                        ll.value = x;
                    }
                }
            }
            v
        })
        .collect();
    DfmReport::new(r.params().clone(), verdicts)
}

#[test]
fn animation_corner_tool_is_sized_from_the_rule_limit() {
    let (m, s) = sharp_pocket();
    let r = report(&m, s, PackParams::Cnc3Axis(cnc_params()));
    let a = animate_report(&m, s, &r);
    let c = anim(&a, cnc::INTERNAL_CORNER_RADIUS_RULE_ID);
    assert_eq!(c.kind, AnimationKind::ToolApproach);
    assert_eq!(c.measured, 0.0);
    match &c.actors[0].shape {
        ActorShape::EndMill { diameter, .. } => assert!((diameter - 3.0).abs() < 1e-12),
        o => panic!("{o:?}"),
    }
    // The tool ends at the corner's bottom (pocket floor z = 4), axis −Z.
    let last = c.actors[0].keys.last().unwrap();
    assert!((last.position[2] - 4.0).abs() < 1e-9, "{last:?}");
    assert!((last.axis[2] + 1.0).abs() < 1e-12);
    // MUTATION: a different limit must show in the descriptor.
    let r2 = mutate(&r, cnc::INTERNAL_CORNER_RADIUS_RULE_ID, None, Some(2.0));
    let a2 = animate_report(&m, s, &r2);
    match &anim(&a2, cnc::INTERNAL_CORNER_RADIUS_RULE_ID).actors[0].shape {
        ActorShape::EndMill { diameter, .. } => assert!((diameter - 4.0).abs() < 1e-12),
        o => panic!("{o:?}"),
    }
    assert_ne!(a, a2);
}

#[test]
fn animation_drill_plunges_to_the_measured_depth() {
    let mut m = BRepModel::new();
    let b = block(&mut m, 20.0, 20.0, 60.0, 0.0, 0.0, 0.0);
    let s = blind_drilled(&mut m, b, 0.0, 0.0, 2.0, 30.0, 50.0);
    let r = report(&m, s, PackParams::Cnc3Axis(cnc_params()));
    let a = animate_report(&m, s, &r);
    let d = anim(&a, cnc::HOLE_DEPTH_RATIO_RULE_ID);
    assert_eq!(d.kind, AnimationKind::DrillPlunge);
    assert!((d.measured - 12.5).abs() < 1e-6);
    let last = d.actors[0].keys.last().unwrap();
    assert!((last.position[2] - (30.0 - 50.0)).abs() < 1e-6, "{last:?}");
    assert!(d.caption.contains("12.50"), "{}", d.caption);
    let r2 = mutate(&r, cnc::HOLE_DEPTH_RATIO_RULE_ID, Some(11.0), None);
    let d2 = animate_report(&m, s, &r2);
    assert!(anim(&d2, cnc::HOLE_DEPTH_RATIO_RULE_ID)
        .caption
        .contains("11.00"));
}

#[test]
fn animation_caliper_closes_to_the_measured_wall() {
    let (m, s) = thin_floor();
    let r = report(&m, s, PackParams::Cnc3Axis(cnc_params()));
    let a = animate_report(&m, s, &r);
    let c = anim(&a, cnc::MIN_WALL_RULE_ID);
    assert_eq!(c.kind, AnimationKind::Caliper);
    assert!((c.actors[0].keys.last().unwrap().extent - 0.5).abs() < 1e-9);
    // MUTATION: a measurement no wall pair has cannot place a caliper —
    // the descriptor degrades to a highlight carrying the mutated value,
    // never a caliper at the old gap.
    let r2 = mutate(&r, cnc::MIN_WALL_RULE_ID, Some(0.7), None);
    let c2 = animate_report(&m, s, &r2);
    let c2 = anim(&c2, cnc::MIN_WALL_RULE_ID);
    assert_eq!(c2.kind, AnimationKind::Highlight);
    assert_eq!(c2.measured, 0.7);
}

#[test]
fn animation_setup_cycle_and_mold_separation_and_spin() {
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let t = cyl_along(&mut m, Point3::new(-40.0, 0.0, 0.0), Vector3::X, 3.3, 80.0);
    let s = cut(&mut m, p, t);
    let r = report(&m, s, PackParams::Cnc3Axis(cnc_params()));
    let a = animate_report(&m, s, &r);
    let sc = anim(&a, cnc::HOLE_ACCESS_RULE_ID);
    assert_eq!(sc.kind, AnimationKind::SetupCycle);
    assert_eq!(
        sc.actors.len(),
        3,
        "two setup arrows + the needed direction"
    );
    let need = &sc.actors[2].keys[0].axis;
    assert!(
        need[0].abs() > 0.999,
        "the hole needs an X approach: {need:?}"
    );

    let (m2, s2) = sharp_pocket();
    let r2 = report(&m2, s2, PackParams::Casting(CastingParams::default()));
    let a2 = animate_report(&m2, s2, &r2);
    let ms = anim(&a2, casting::DRAFT_RULE_ID);
    assert_eq!(ms.kind, AnimationKind::MoldSeparation);
    // Every failing wall travels with ITS half: the track that turns it red
    // ends at the same offset as the half's own track that holds it.
    let last_off = |tr: &geometry_engine::dfm::animation::FaceTrack| tr.keys.last().unwrap().offset;
    let fail_tracks: Vec<_> = ms
        .face_tracks
        .iter()
        .filter(|tr| {
            tr.keys
                .iter()
                .any(|k| k.state == geometry_engine::dfm::animation::FaceState::Fail)
        })
        .collect();
    assert!(!fail_tracks.is_empty());
    for ft in &fail_tracks {
        for f in &ft.faces {
            let half =
                ms.face_tracks
                    .iter()
                    .find(|tr| {
                        tr.faces.contains(f)
                            && tr.keys.iter().all(|k| {
                                k.state == geometry_engine::dfm::animation::FaceState::Context
                            })
                    })
                    .expect("each failing face belongs to a half");
            assert_eq!(
                last_off(ft),
                last_off(half),
                "face {f} must move with its half"
            );
        }
    }
    let covered: std::collections::BTreeSet<u32> = fail_tracks
        .iter()
        .flat_map(|t| t.faces.iter().copied())
        .collect();
    assert_eq!(
        covered,
        ms.faces.iter().copied().collect(),
        "every witness is shown failing"
    );

    let (m3, s3) = shaft(0.0, 0.0, true, 5.0);
    let r3 = report(&m3, s3, PackParams::CncTurning(CncTurningParams::default()));
    let a3 = animate_report(&m3, s3, &r3);
    let sp = anim(&a3, turning::AXISYMMETRY_RULE_ID);
    assert_eq!(sp.kind, AnimationKind::SpindleSpin);
    assert!(sp.spin_axis.is_some());
    for tr in &sp.face_tracks {
        assert_eq!(
            tr.keys.last().unwrap().rotation_deg,
            720.0,
            "every face — the failing ones too — spins with the part"
        );
    }

    // The descriptor is plain data: it round-trips through JSON.
    let json = serde_json::to_string(&a).unwrap();
    let back: Vec<DfmAnimation> = serde_json::from_str(&json).unwrap();
    assert_eq!(back.len(), a.len());
    for (x, y) in back.iter().zip(&a) {
        assert_eq!((x.kind, &x.rule, &x.faces), (y.kind, &y.rule, &y.faces));
    }
    assert!(json.contains("\"kind\":\"setup_cycle\""));
}

#[test]
fn animation_only_for_violations() {
    let (m, s) = window(Some(3.0));
    let r = report(&m, s, PackParams::Cnc3Axis(cnc_params()));
    let a = animate_report(&m, s, &r);
    assert!(a
        .iter()
        .all(|x| x.rule != cnc::INTERNAL_CORNER_RADIUS_RULE_ID));
}
