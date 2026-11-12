// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Hidden-line-removal harness (#22).
//!
//! End-to-end: `standard_drawing_hlr` must produce an opaque drawing — occluded
//! edges dashed, visible edges solid — through the full render pipeline, every
//! verdict an exact ray↔surface test (sound, never a wireframe that lies). And
//! the plain wireframe `standard_drawing` must stay see-through (no hidden set),
//! so the two paths are distinct and #20 is untouched.

use geometry_engine::drawing::types::{ProjectionType, SheetSize};
use geometry_engine::drawing::{
    is_point_hidden, project_solid_edges_visibility, render_drawing_svg, standard_drawing,
    standard_drawing_hlr,
};
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

#[test]
fn hlr_drawing_has_dashed_hidden_edges_wireframe_does_not() {
    let mut m = BRepModel::new();
    let b = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(30.0, 20.0, 16.0)
        .expect("box"));

    // HLR drawing: back edges classify hidden → dashed lines appear.
    let hlr = standard_drawing_hlr(&m, b, uuid::Uuid::nil(), SheetSize::A3, 1.0).expect("hlr");
    assert!(
        hlr.views.iter().any(|v| !v.hidden_polylines.is_empty()),
        "HLR drawing carries hidden edges"
    );
    let svg = render_drawing_svg(&hlr);
    assert!(
        svg.contains("polyline class=\"hidden\""),
        "HLR SVG renders dashed hidden polylines"
    );
    // ISO 128-2 line type 02.1 "dashed narrow" (hidden edges):
    // dash 4 mm, gap 2 mm at 0.25 mm mid-tier.
    // Pinned to c489a85 (Task 8 ISO 128 weight hierarchy); the new values are
    // standard drafting practice and correct per the 2:1 visible/hidden/thin hierarchy.
    assert!(
        svg.contains("stroke-dasharray: 4 2"),
        "hidden lines use ISO 128 type 02.1 dashed narrow pattern (4 2 at 0.25 mm)"
    );

    // Plain wireframe drawing: no hidden set, no dashed hidden elements.
    let wire = standard_drawing(&m, b, uuid::Uuid::nil(), SheetSize::A3, 1.0).expect("wire");
    assert!(
        wire.views.iter().all(|v| v.hidden_polylines.is_empty()),
        "wireframe drawing has no hidden set"
    );
    let wsvg = render_drawing_svg(&wire);
    assert!(
        !wsvg.contains("polyline class=\"hidden\""),
        "wireframe SVG has no hidden polylines"
    );
}

#[test]
fn hlr_visible_plus_hidden_covers_the_wireframe() {
    // Visibility is a PARTITION: every projected edge sub-span is either visible
    // or hidden — none dropped, none duplicated into both. Check the total
    // polyline count is positive and both buckets are non-empty for an opaque
    // box seen in isometric (where front and back edges are distinct on-page).
    let mut m = BRepModel::new();
    let b = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(24.0, 24.0, 24.0)
        .expect("box"));
    let e = project_solid_edges_visibility(&m, b, ProjectionType::Isometric, 8).expect("vis");
    assert!(!e.visible.is_empty(), "iso box has visible edges");
    assert!(
        !e.hidden.is_empty(),
        "iso box has hidden edges (the far corner)"
    );
    // The iso view direction is now the standard-engineering-iso camera shared
    // with the shaded raster: w = dir = (−1,−1,−1)/√3, camera on the (1,1,1)
    // octant (drawing-iso-mismatch fix — the arm previously hand-rolled
    // w=(1,1,−1)/√3, an OPPOSITE octant that disagreed with the shaded raster).
    // So the +X face is now NEAR (visible) and the −X face is FAR (hidden) —
    // the two assertions below flip accordingly.
    assert!(
        !is_point_hidden(
            &m,
            b,
            ProjectionType::Isometric,
            Point3::new(12.0, 6.0, 6.0)
        ),
        "the near (+X) face is visible in the standard iso (camera at (1,1,1))"
    );
    assert!(
        is_point_hidden(
            &m,
            b,
            ProjectionType::Isometric,
            Point3::new(-12.0, -6.0, -6.0)
        ),
        "the far (−X) face is hidden in the standard iso (camera at (1,1,1))"
    );
}

#[test]
fn bored_plate_hlr_dashes_the_bore_behind_the_face() {
    // A blind pocket: the far wall of a Ø20 through-bore is hidden behind the
    // plate's front face in Front view → dashed in the HLR drawing.
    let mut m = BRepModel::new();
    let plate = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(50.0, 30.0, 16.0)
        .expect("plate"));
    let bore = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -20.0), Vector3::Z, 10.0, 80.0)
        .expect("bore"));
    let part = boolean_operation(
        &mut m,
        plate,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("bore");
    let dwg = standard_drawing_hlr(&m, part, uuid::Uuid::nil(), SheetSize::A3, 1.0).expect("hlr");
    // Front view is the first in the layout.
    let front = &dwg.views[0];
    assert!(
        !front.hidden_polylines.is_empty(),
        "the bored plate's Front view has hidden edges (the far bore wall)"
    );
}

#[test]
fn hlr_is_deterministic() {
    let mut m = BRepModel::new();
    let b = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("box"));
    let a = standard_drawing_hlr(&m, b, uuid::Uuid::nil(), SheetSize::A3, 1.0).expect("a");
    let c = standard_drawing_hlr(&m, b, uuid::Uuid::nil(), SheetSize::A3, 1.0).expect("c");
    for (va, vc) in a.views.iter().zip(c.views.iter()) {
        assert_eq!(
            va.polylines.len(),
            vc.polylines.len(),
            "visible count stable"
        );
        assert_eq!(
            va.hidden_polylines.len(),
            vc.hidden_polylines.len(),
            "hidden count stable"
        );
    }
}

/// An engineer can switch hidden lines off on one view ("hidden lines
/// removed" display): that view renders no dashed edges; the other views keep
/// theirs. The hidden set is kept on the view, so switching back needs no
/// re-projection.
#[test]
fn a_view_with_hidden_lines_off_renders_none_and_others_keep_theirs() {
    let mut m = BRepModel::new();
    let b = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(30.0, 20.0, 16.0)
        .expect("box"));
    let mut d = standard_drawing_hlr(&m, b, uuid::Uuid::nil(), SheetSize::A3, 1.0).expect("hlr");
    let with_hidden: Vec<usize> = d
        .views
        .iter()
        .enumerate()
        .filter(|(_, v)| !v.hidden_polylines.is_empty())
        .map(|(i, _)| i)
        .collect();
    assert!(with_hidden.len() >= 2, "need two views with hidden edges");
    assert!(
        d.views.iter().all(|v| v.show_hidden),
        "hidden lines show by default"
    );

    let off = with_hidden[0];
    d.views[off].show_hidden = false;
    let hidden_kept = d.views[off].hidden_polylines.len();
    let svg = render_drawing_svg(&d);

    // Count dashed polylines per view group: the switched-off view has none.
    let dashed_total = svg.matches("polyline class=\"hidden\"").count();
    let expected: usize = d
        .views
        .iter()
        .filter(|v| v.show_hidden)
        .map(|v| {
            v.hidden_polylines
                .iter()
                .filter(|p| p.points.len() >= 2)
                .count()
        })
        .sum();
    assert_eq!(
        dashed_total, expected,
        "only views showing hidden lines draw them"
    );
    assert!(expected > 0, "the other views still draw theirs");
    assert_eq!(
        d.views[off].hidden_polylines.len(),
        hidden_kept,
        "the hidden set is kept"
    );

    // Older saved drawings (no field) read as "show hidden".
    let mut json = serde_json::to_value(&d.views[off]).expect("ser");
    json.as_object_mut().expect("obj").remove("show_hidden");
    let back: geometry_engine::drawing::types::ProjectedView =
        serde_json::from_value(json).expect("de");
    assert!(back.show_hidden);
}

/// The DXF carries hidden edges as dashed line work (line type HIDDEN, ISO
/// 128 type 02.1: 4 mm dash, 2 mm gap) on each view's layer -- and none for a
/// view the engineer switched hidden lines off on. Before, DXF dropped hidden
/// edges entirely, so an exported sheet lost every hidden bore.
#[test]
fn dxf_carries_hidden_edges_dashed_and_honours_show_hidden() {
    let mut m = BRepModel::new();
    let b = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(30.0, 20.0, 16.0)
        .expect("box"));
    let mut d = standard_drawing_hlr(&m, b, uuid::Uuid::nil(), SheetSize::A3, 1.0).expect("hlr");
    let hidden_count = |d: &geometry_engine::drawing::types::Drawing| -> usize {
        d.views
            .iter()
            .filter(|v| v.show_hidden)
            .map(|v| {
                v.hidden_polylines
                    .iter()
                    .filter(|p| p.points.len() >= 2)
                    .count()
                    + v.hidden_circles.len()
                    + v.hidden_ellipses.len()
            })
            .sum()
    };
    let read = |d: &geometry_engine::drawing::types::Drawing| {
        let bytes = geometry_engine::drawing::render_drawing_dxf(d).expect("dxf");
        dxf::Drawing::load(&mut std::io::Cursor::new(bytes)).expect("readable DXF")
    };

    let all = hidden_count(&d);
    assert!(all > 0, "fixture has hidden edges");
    let file = read(&d);
    let hidden_type = file
        .line_types()
        .find(|lt| lt.name == "HIDDEN")
        .expect("a HIDDEN line type is defined");
    assert_eq!(hidden_type.dash_dot_space_lengths, vec![4.0, -2.0]);
    let dashed = file
        .entities()
        .filter(|e| e.common.line_type_name == "HIDDEN")
        .count();
    assert_eq!(dashed, all, "every hidden edge is written dashed");

    let off = d
        .views
        .iter()
        .position(|v| !v.hidden_polylines.is_empty())
        .expect("a view with hidden edges");
    d.views[off].show_hidden = false;
    let fewer = hidden_count(&d);
    assert!(fewer < all);
    let dashed = read(&d)
        .entities()
        .filter(|e| e.common.line_type_name == "HIDDEN")
        .count();
    assert_eq!(dashed, fewer, "a view with hidden lines off writes none");
}

/// A flanged hub (Ø80 x 12 flange, Ø45 x 20 hub, Ø30 bore) seen from the
/// front: the near edges of the flange top face and the hub top face are in
/// plain sight, so they are drawn solid. They were classified hidden -- the
/// line of sight to them runs exactly in the plane of the face they bound,
/// and that face was counted as blocking it. Found live: the FRONT and RIGHT
/// views had no flange top line and a broken hub top line.
#[test]
fn edges_seen_along_their_own_face_plane_are_visible() {
    let mut m = BRepModel::new();
    let flange = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 40.0, 12.0)
        .expect("flange"));
    let hub = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 12.0), Vector3::Z, 22.5, 20.0)
        .expect("hub"));
    let body = boolean_operation(
        &mut m,
        flange,
        hub,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("union");
    let bore = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -4.0), Vector3::Z, 15.0, 40.0)
        .expect("bore"));
    let part = boolean_operation(
        &mut m,
        body,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("bore");

    // Front: camera on +Y, so the near half of each rim has y > 0.
    let near = |r: f64, x: f64, z: f64| Point3::new(x, (r * r - x * x).sqrt(), z);
    for (what, p) in [
        ("flange top rim, left of the hub", near(40.0, -31.0, 12.0)),
        ("flange top rim, right of the hub", near(40.0, 33.0, 12.0)),
        ("hub top rim, near its end", near(22.5, -20.0, 32.0)),
        ("hub top rim, middle", near(22.5, 2.0, 32.0)),
        ("flange bottom rim", near(40.0, -10.0, 0.0)),
    ] {
        assert!(
            !is_point_hidden(&m, part, ProjectionType::Front, p),
            "{what} at {p:?} is in plain sight"
        );
    }
    // And the far half of the flange rim is behind the part.
    let far = Point3::new(-31.0, -(40.0f64 * 40.0 - 31.0 * 31.0).sqrt(), 12.0);
    assert!(is_point_hidden(&m, part, ProjectionType::Front, far));

    // What the sheet draws: a visible line along the flange top (view y = 12)
    // either side of the hub, and along the hub top (view y = 32).
    let dwg = standard_drawing_hlr(&m, part, uuid::Uuid::nil(), SheetSize::A3, 1.0).expect("hlr");
    let front = dwg
        .views
        .iter()
        .find(|v| matches!(v.projection, ProjectionType::Front))
        .expect("front view");
    let covered = |y: f64, x: f64| {
        front.polylines.iter().any(|pl| {
            pl.points.windows(2).any(|w| {
                let (a, b) = (w[0], w[1]);
                (a[1] - y).abs() < 1e-6
                    && (b[1] - y).abs() < 1e-6
                    && x >= a[0].min(b[0]) - 1e-6
                    && x <= a[0].max(b[0]) + 1e-6
            })
        })
    };
    for (what, y, x) in [
        ("flange top, left", 12.0, -31.0),
        ("flange top, right", 12.0, 33.0),
        ("hub top, near its end", 32.0, -20.0),
        ("hub top, middle", 32.0, 2.0),
    ] {
        assert!(covered(y, x), "{what}: no visible line at view ({x}, {y})");
    }
}
