// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! An arc is drawn as an arc and dimensioned as a radius.
//!
//! Found by looking at the benchmark contact sheets (parts 110 and 124):
//! every corner round and half-round end was drawn as its WHOLE circle, so a
//! plate with filleted corners carried phantom rings around its outline, and
//! every concave arc was tabled as a drilled hole of twice its radius.

use geometry_engine::drawing::dimensioning::standard_drawing_auto;
use geometry_engine::drawing::types::{Drawing, ProjectionType};
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

fn op(m: &mut BRepModel, a: SolidId, b: SolidId, kind: BooleanOp) -> SolidId {
    boolean_operation(m, a, b, kind, BooleanOptions::default()).unwrap()
}

/// A 60 x 40 x 10 plate with a half-round notch (R8) cut into the middle of
/// one long edge, and one drilled hole (Ø6) through it.
fn notched_plate() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    m.set_event_key(Some("plate".to_string()));
    let plate = solid(
        TopologyBuilder::new(&mut m)
            .create_box_3d(60.0, 40.0, 10.0)
            .unwrap(),
    );
    let bb = geometry_engine::readable::extract_dimensions(&m, plate);
    assert!(bb.iter().any(|d| d.kind == "extent"));
    // The box is centred on the origin in X and Y (asserted below through the
    // drawn outline), so the notch sits on the y = -20 edge.
    m.set_event_key(Some("notch-tool".to_string()));
    let notch = solid(
        TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(0.0, -20.0, -10.0), Vector3::Z, 8.0, 30.0)
            .unwrap(),
    );
    m.set_event_key(Some("notch".to_string()));
    let plate = op(&mut m, plate, notch, BooleanOp::Difference);
    m.set_event_key(Some("drill-tool".to_string()));
    let drill = solid(
        TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(15.0, 5.0, -10.0), Vector3::Z, 3.0, 30.0)
            .unwrap(),
    );
    m.set_event_key(Some("drill".to_string()));
    let plate = op(&mut m, plate, drill, BooleanOp::Difference);
    m.set_event_key(None);
    (m, plate)
}

fn top(d: &Drawing) -> &geometry_engine::drawing::types::ProjectedView {
    d.views
        .iter()
        .find(|v| v.projection == ProjectionType::Top)
        .expect("TOP view")
}

/// The notch is half a circle: TOP must not draw a whole R8 circle (half of
/// it would lie outside the part), while the drilled hole stays a circle.
///
/// Mutation: drop the `closed` test where the rims are flushed in
/// `project_solid_edges_visibility_occ` and the R8 ring comes back.
#[test]
fn a_half_round_notch_is_drawn_as_an_arc_not_a_whole_circle() {
    let (m, s) = notched_plate();
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let v = top(&d);
    let rings: Vec<f64> = v
        .circles
        .iter()
        .chain(v.hidden_circles.iter())
        .map(|c| c.r)
        .collect();
    assert!(
        rings.iter().any(|r| (r - 3.0).abs() < 1e-6),
        "fixture: the drilled hole is drawn as a circle; circles: {rings:?}"
    );
    assert!(
        !rings.iter().any(|r| (r - 8.0).abs() < 1e-6),
        "the R8 notch is an arc, not a circle; circles: {rings:?}"
    );
    // The arc itself is still drawn: some polyline point lies on the R8 arc
    // inside the plate (y > -20), none of the view's ink lies below the edge.
    let on_arc = v
        .polylines
        .iter()
        .flat_map(|p| p.points.iter())
        .filter(|p| ((p[0]).hypot(p[1] + 20.0) - 8.0).abs() < 1e-3 && p[1] > -19.0)
        .count();
    assert!(
        on_arc >= 3,
        "the notch's arc is drawn ({on_arc} points on it)"
    );
    let lowest = v
        .polylines
        .iter()
        .chain(v.hidden_polylines.iter())
        .flat_map(|p| p.points.iter())
        .map(|p| p[1])
        .fold(f64::INFINITY, f64::min);
    assert!(
        lowest > -20.0 - 1e-6,
        "no ink below the plate's edge (lowest y = {lowest})"
    );
}

fn cert(m: &BRepModel, d: &Drawing) -> geometry_engine::drawing::SheetReadbackCertificate {
    geometry_engine::drawing::certify_drawing(m, d)
}

/// The notch is an arc: the sheet states it as `R8` on TOP, where it is drawn
/// as an arc, and does NOT table it as a drilled hole of diameter 16. The
/// drilled hole keeps its row. The certificate re-measures the radius as a
/// radius (consistent), and the sheet is sound.
///
/// Mutation: make `arcs::CylinderWall::is_radius` always false and the notch
/// is tabled as `Ø16` again and no `R8` is inked.
#[test]
fn a_notch_is_stated_by_its_radius_and_is_not_a_hole() {
    let (m, s) = notched_plate();
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let sizes: Vec<f64> = d.hole_sites.iter().map(|h| h.diameter_mm).collect();
    assert_eq!(
        sizes,
        vec![6.0],
        "the hole table holds the drilled hole only, not the notch"
    );
    let radii: Vec<&geometry_engine::drawing::Dimension2d> = d
        .views
        .iter()
        .flat_map(|v| v.dimensions.iter())
        .filter(|x| x.kind == "radius")
        .collect();
    assert_eq!(radii.len(), 1, "one radius on the sheet: {radii:?}");
    assert_eq!(radii[0].label, "R8");
    assert!((radii[0].value - 8.0).abs() < 1e-9);
    assert!(
        top(&d).dimensions.iter().any(|x| x.kind == "radius"),
        "the radius is on TOP, the view that draws the arc as an arc"
    );
    // A notch is not tangent to the edge it is cut into: its centre is still
    // located (two offsets from the part's corner).
    let located = d
        .views
        .iter()
        .flat_map(|v| v.dimensions.iter())
        .filter(|x| x.kind == "position" && x.entities == radii[0].entities)
        .count();
    assert!(located >= 1, "the notch's centre is located");

    let c = cert(&m, &d);
    let fact = c
        .facts
        .iter()
        .find(|f| f.label == "R8")
        .expect("the radius is a fact on the certificate");
    assert_eq!(
        fact.live.verdict,
        geometry_engine::drawing::SheetVerdict::Consistent,
        "{fact:?}"
    );
    assert_eq!(fact.live.measured, Some(8.0));
    assert!(
        c.sound,
        "unsound facts: {:?}",
        c.unsound_facts().collect::<Vec<_>>()
    );
}

/// A plate with a half-round notch R8 in each long edge and no hole.
fn twice_notched_plate() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    m.set_event_key(Some("plate".to_string()));
    let mut plate = solid(
        TopologyBuilder::new(&mut m)
            .create_box_3d(60.0, 40.0, 10.0)
            .unwrap(),
    );
    for (i, (x, y)) in [(-10.0, -20.0), (10.0, 20.0)].iter().enumerate() {
        m.set_event_key(Some(format!("notch-tool-{i}")));
        let tool = solid(
            TopologyBuilder::new(&mut m)
                .create_cylinder_3d(Point3::new(*x, *y, -10.0), Vector3::Z, 8.0, 30.0)
                .unwrap(),
        );
        m.set_event_key(Some(format!("notch-{i}")));
        plate = op(&mut m, plate, tool, BooleanOp::Difference);
    }
    m.set_event_key(None);
    (m, plate)
}

/// Two equal notches are ONE callout that says so -- `2× R8` -- and the
/// second notch is stated by it, not omitted: the certificate lists no
/// radius as missing, and re-measures both.
///
/// Mutation: remove the `count_equal_features` call from
/// `standard_drawing_auto` and two separate `R8` leaders are inked.
#[test]
fn equal_arcs_are_one_counted_callout_and_none_is_omitted() {
    let (m, s) = twice_notched_plate();
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let radii: Vec<&geometry_engine::drawing::Dimension2d> = d
        .views
        .iter()
        .flat_map(|v| v.dimensions.iter())
        .filter(|x| x.kind == "radius")
        .collect();
    assert_eq!(radii.len(), 1, "one callout for the two notches: {radii:?}");
    assert_eq!(radii[0].label, "2\u{00D7} R8");
    assert_eq!(radii[0].members.len(), 2, "it names both notches");
    // And where each notch is in the view: its leader may end on either.
    let pts = &radii[0].member_points;
    assert_eq!(pts.len(), 2, "both notches' points: {pts:?}");
    assert_eq!(pts[0], radii[0].a, "the head's point first");
    assert!(
        (pts[0][0] - pts[1][0]).hypot(pts[0][1] - pts[1][1]) > 1.0,
        "two notches, two places: {pts:?}"
    );

    let c = cert(&m, &d);
    let missing: Vec<_> = c.omissions.iter().filter(|o| o.kind == "radius").collect();
    assert!(
        missing.is_empty(),
        "a counted notch is stated, not omitted: {missing:?}"
    );
    let fact = c
        .facts
        .iter()
        .find(|f| f.label == "2\u{00D7} R8")
        .expect("the counted callout is a fact");
    assert_eq!(
        fact.live.verdict,
        geometry_engine::drawing::SheetVerdict::Consistent,
        "{fact:?}"
    );
}

/// The count is a claim, and the certificate checks it: a callout that says
/// `3×` on a model with two equal notches is stale, and says why.
///
/// Mutation: remove the `counted_members_check` call from `certify_drawing`
/// and the false count certifies consistent.
#[test]
fn a_false_count_is_stale_on_the_certificate() {
    let (m, s) = twice_notched_plate();
    let mut d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let callout = d
        .views
        .iter_mut()
        .flat_map(|v| v.dimensions.iter_mut())
        .find(|x| x.kind == "radius")
        .expect("the counted radius");
    callout.members.push("no-such-feature".to_string());
    callout.label = "3\u{00D7} R8".to_string();

    let c = cert(&m, &d);
    let fact = c
        .facts
        .iter()
        .find(|f| f.label == "3\u{00D7} R8")
        .expect("the tampered callout is a fact");
    assert_eq!(
        fact.live.verdict,
        geometry_engine::drawing::SheetVerdict::Stale,
        "{fact:?}"
    );
    let why = fact.live.detail.clone().unwrap_or_default();
    assert!(
        why.contains("3\u{00D7}") && why.contains("2 equal"),
        "the finding names the count and what the model has: {why:?}"
    );
    assert!(!c.sound, "a false count makes the sheet unsound");
}

/// A dimension prints without trailing zeros (ISO 129-1): the 60 x 40 x 10
/// plate is `60`, `40`, `10`, its hole `Ø6` in the table, located at `45`
/// and `25` -- and no number on the sheet ends in a zero after a decimal
/// point.
///
/// Mutation: make `drafting_number_text` return its input and `60.00` is
/// back.
#[test]
fn dimensions_and_the_hole_table_print_no_trailing_zeros() {
    let (m, s) = notched_plate();
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let mut labels: Vec<String> = d
        .views
        .iter()
        .flat_map(|v| v.dimensions.iter())
        .map(|x| x.label.clone())
        .collect();
    for h in &d.hole_sites {
        labels.extend([
            h.x_label.clone(),
            h.y_label.clone(),
            h.dia_label.clone(),
            h.depth_label.clone(),
        ]);
    }
    for want in ["60", "40", "10", "R8"] {
        assert!(
            labels.iter().any(|l| l == want),
            "{want} is on the sheet: {labels:?}"
        );
    }
    assert_eq!(d.hole_sites[0].dia_label, "\u{00D8}6");
    assert_eq!(d.hole_sites[0].x_label, "45");
    assert_eq!(d.hole_sites[0].y_label, "25");
    for l in &labels {
        let chars: Vec<char> = l.chars().collect();
        let mut seen_point = false;
        for (i, ch) in chars.iter().enumerate() {
            if *ch == '.' {
                seen_point = true;
            } else if !ch.is_ascii_digit() {
                seen_point = false;
            }
            let ends_number = chars.get(i + 1).map_or(true, |n| !n.is_ascii_digit());
            assert!(
                !(seen_point && *ch == '0' && ends_number),
                "{l:?} prints a trailing zero"
            );
        }
    }
    // The ink agrees with the labels.
    let svg = geometry_engine::drawing::render_drawing_svg(&d);
    assert!(!svg.contains(">60.00<"), "the SVG still inks 60.00");
    assert!(svg.contains(">60<"));
}
