//! Unit tests for the drawing module.
//!
//! Coverage:
//! * Pure projection math (`view_matrix_for_projection`, `project_point`)
//!   pinning the page-coordinate convention for every preset.
//! * `project_solid_edges` end-to-end against a real BRep box and a
//!   cylinder, asserting edge counts + bounds.
//! * `render_drawing_svg` shape contract (well-formed XML envelope,
//!   per-view groups, polyline payload preservation).

use super::projection::{
    project_point, project_solid_edges, project_solid_view, view_matrix_for_projection,
    DEFAULT_CURVE_SAMPLES,
};
use super::svg::render_drawing_svg;
use super::types::{Drawing, Polyline2d, ProjectionType, SheetSize, ViewExtent, ViewSource};
use uuid::Uuid;

/// Pin a deterministic, all-zero part_id for in-test
/// `project_solid_view` calls — the in-memory model resolver doesn't
/// look at part_id, so any UUID does.
fn nil_view_source(solid_id: crate::primitives::solid::SolidId) -> ViewSource {
    ViewSource::Part {
        part_id: Uuid::nil(),
        solid_id,
    }
}

use crate::math::{Point3, Vector3};
use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

// ============================================================
// Projection matrix presets
// ============================================================

/// Front view: world X → page X, world Z → page Y, world Y collapses.
#[test]
fn front_projection_drops_y() {
    let p = project_point(ProjectionType::Front, Point3::new(3.0, 99.0, 5.0));
    assert!(
        (p[0] - 3.0).abs() < 1e-12,
        "X should pass through; got {p:?}"
    );
    assert!(
        (p[1] - 5.0).abs() < 1e-12,
        "Z should map to page Y; got {p:?}"
    );
}

/// Top view: world X → page X, world Y → page Y, world Z collapses.
#[test]
fn top_projection_drops_z() {
    let p = project_point(ProjectionType::Top, Point3::new(3.0, 7.0, 99.0));
    assert!((p[0] - 3.0).abs() < 1e-12);
    assert!((p[1] - 7.0).abs() < 1e-12);
}

/// Right view: looking from +X, page X = -Y, page Y = Z.
#[test]
fn right_projection_drops_x() {
    let p = project_point(ProjectionType::Right, Point3::new(99.0, 4.0, 5.0));
    assert!((p[0] - (-4.0)).abs() < 1e-12);
    assert!((p[1] - 5.0).abs() < 1e-12);
}

/// Bottom view = upside-down top.
#[test]
fn bottom_projection_flips_y_relative_to_top() {
    let p = project_point(ProjectionType::Bottom, Point3::new(3.0, 7.0, 99.0));
    assert!((p[0] - 3.0).abs() < 1e-12);
    assert!((p[1] - (-7.0)).abs() < 1e-12);
}

/// Left view = mirror of right.
#[test]
fn left_projection_flips_y_relative_to_right() {
    let p = project_point(ProjectionType::Left, Point3::new(99.0, 4.0, 5.0));
    assert!((p[0] - 4.0).abs() < 1e-12);
    assert!((p[1] - 5.0).abs() < 1e-12);
}

/// Isometric projection maps origin to origin and behaves symmetrically
/// for axis-aligned points. Concrete numbers come from the standard
/// (1,1,1)/√3 camera convention.
#[test]
fn isometric_origin_is_origin() {
    let p = project_point(ProjectionType::Isometric, Point3::new(0.0, 0.0, 0.0));
    assert!(p[0].abs() < 1e-12 && p[1].abs() < 1e-12);
}

#[test]
fn isometric_maps_x_and_y_symmetrically_on_page() {
    let px = project_point(ProjectionType::Isometric, Point3::new(1.0, 0.0, 0.0));
    let py = project_point(ProjectionType::Isometric, Point3::new(0.0, 1.0, 0.0));
    // X-only and Y-only world axes both contribute equally to page Y
    // in the iso preset; their page-Y components must match.
    assert!(
        (px[1] - py[1]).abs() < 1e-12,
        "iso: page-Y for +X and +Y should match; got {px:?} vs {py:?}"
    );
}

/// Custom projection passes the rotation through verbatim.
#[test]
fn custom_projection_uses_supplied_rotation() {
    // 90° rotation about Z: x ↦ y, y ↦ -x.
    let rotation = [0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
    let p = project_point(
        ProjectionType::Custom { rotation },
        Point3::new(1.0, 0.0, 0.0),
    );
    assert!((p[0] - 0.0).abs() < 1e-12);
    assert!((p[1] - (-1.0)).abs() < 1e-12);
}

/// The 3×3 sub-matrix of every preset must be orthonormal (rotation).
#[test]
fn every_preset_rotation_is_orthonormal() {
    for pt in [
        ProjectionType::Front,
        ProjectionType::Top,
        ProjectionType::Right,
        ProjectionType::Bottom,
        ProjectionType::Left,
        ProjectionType::Isometric,
    ] {
        let m = view_matrix_for_projection(pt);
        // Pull rows as Vector3.
        let row = |r: usize| Vector3::new(m.get(r, 0), m.get(r, 1), m.get(r, 2));
        let r0 = row(0);
        let r1 = row(1);
        let r2 = row(2);
        let dot = |a: Vector3, b: Vector3| a.x * b.x + a.y * b.y + a.z * b.z;
        // Unit length.
        assert!((dot(r0, r0) - 1.0).abs() < 1e-12, "{:?}: row0 not unit", pt);
        assert!((dot(r1, r1) - 1.0).abs() < 1e-12, "{:?}: row1 not unit", pt);
        assert!((dot(r2, r2) - 1.0).abs() < 1e-12, "{:?}: row2 not unit", pt);
        // Mutually orthogonal.
        assert!(dot(r0, r1).abs() < 1e-12, "{:?}: r0·r1 ≠ 0", pt);
        assert!(dot(r0, r2).abs() < 1e-12, "{:?}: r0·r2 ≠ 0", pt);
        assert!(dot(r1, r2).abs() < 1e-12, "{:?}: r1·r2 ≠ 0", pt);
    }
}

// ============================================================
// BRep-driven projection
// ============================================================

fn build_box(w: f64, h: f64, d: f64) -> (BRepModel, crate::primitives::solid::SolidId) {
    let mut model = BRepModel::new();
    let solid_id = {
        let mut builder = TopologyBuilder::new(&mut model);
        match builder
            .create_box_3d(w, h, d)
            .expect("box primitive must build")
        {
            GeometryId::Solid(id) => id,
            other => panic!("expected solid, got {other:?}"),
        }
    };
    (model, solid_id)
}

/// A box has 12 topological edges. In the *front* view, eight edges
/// (the top + bottom + left + right rectangles in X–Z) project to
/// non-degenerate segments; the four edges along Y collapse to single
/// points and are dropped. We therefore expect 8 polylines.
#[test]
fn box_front_view_projects_to_eight_polylines() {
    let (model, solid) = build_box(10.0, 20.0, 30.0);
    let polylines =
        project_solid_edges(&model, solid, ProjectionType::Front, DEFAULT_CURVE_SAMPLES)
            .expect("box projection must succeed");
    assert_eq!(
        polylines.len(),
        8,
        "front view must drop the 4 edges parallel to the view direction"
    );
}

/// Same box in top view: edges along Z collapse, leaving 8 polylines.
#[test]
fn box_top_view_projects_to_eight_polylines() {
    let (model, solid) = build_box(10.0, 20.0, 30.0);
    let polylines = project_solid_edges(&model, solid, ProjectionType::Top, DEFAULT_CURVE_SAMPLES)
        .expect("box projection must succeed");
    assert_eq!(polylines.len(), 8);
}

/// Box edges are linear; the polyline sampler must emit exactly 2
/// points per surviving linear edge regardless of `samples_per_curve`.
#[test]
fn box_linear_edges_use_two_samples() {
    let (model, solid) = build_box(10.0, 20.0, 30.0);
    let polylines = project_solid_edges(&model, solid, ProjectionType::Top, 64).unwrap();
    for pl in &polylines {
        assert_eq!(
            pl.points.len(),
            2,
            "linear edge must sample at endpoints only; got {} points",
            pl.points.len()
        );
    }
}

/// Front view of a 10×20×30 box must span exactly its X-Z extent.
#[test]
fn box_front_view_extent_matches_geometry() {
    let (model, solid) = build_box(10.0, 20.0, 30.0);
    let view = project_solid_view(
        &model,
        crate::drawing::types::ViewSource::Part {
            part_id: uuid::Uuid::nil(),
            solid_id: solid,
        },
        ProjectionType::Front,
        "Front",
        [0.0, 0.0],
        1.0,
    )
    .unwrap();
    let w = view.extent.width();
    let h = view.extent.height();
    assert!(
        (w - 10.0).abs() < 1e-6,
        "front view width should be box X={}; got {w}",
        10.0
    );
    assert!(
        (h - 30.0).abs() < 1e-6,
        "front view height should be box Z={}; got {h}",
        30.0
    );
}

/// Top view of the same box must span its X-Y extent (depth disappears).
#[test]
fn box_top_view_extent_matches_geometry() {
    let (model, solid) = build_box(10.0, 20.0, 30.0);
    let view = project_solid_view(
        &model,
        crate::drawing::types::ViewSource::Part {
            part_id: uuid::Uuid::nil(),
            solid_id: solid,
        },
        ProjectionType::Top,
        "Top",
        [0.0, 0.0],
        1.0,
    )
    .unwrap();
    assert!((view.extent.width() - 10.0).abs() < 1e-6);
    assert!((view.extent.height() - 20.0).abs() < 1e-6);
}

/// Cylinder produces non-linear (circular) edges so the sample count
/// budget must engage. The two caps each contribute a closed circle
/// (sampled into a polyline) and the seam contributes a linear edge.
#[test]
fn cylinder_projects_with_curve_samples() {
    let mut model = BRepModel::new();
    let solid_id = {
        let mut b = TopologyBuilder::new(&mut model);
        match b
            .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, 5.0, 12.0)
            .unwrap()
        {
            GeometryId::Solid(id) => id,
            o => panic!("{o:?}"),
        }
    };
    let polylines = project_solid_edges(
        &model,
        solid_id,
        ProjectionType::Front,
        DEFAULT_CURVE_SAMPLES,
    )
    .unwrap();
    assert!(
        !polylines.is_empty(),
        "cylinder should project at least one polyline"
    );
    // At least one polyline must have > 2 points (the rim circles).
    let curved_count = polylines.iter().filter(|p| p.points.len() > 2).count();
    assert!(
        curved_count >= 1,
        "cylinder must yield at least one multi-segment polyline (the rim)"
    );
}

// ============================================================
// Polyline2d behaviour
// ============================================================

#[test]
fn polyline_dedupes_consecutive_duplicates() {
    let pl = Polyline2d::from_points(vec![[0.0, 0.0], [0.0, 0.0], [1.0, 0.0], [1.0, 0.0]]);
    assert_eq!(pl.points.len(), 2);
}

#[test]
fn view_extent_starts_empty_and_grows() {
    let mut e = ViewExtent::empty();
    assert!(e.is_empty());
    e.include([1.0, 2.0]);
    e.include([-3.0, 4.0]);
    assert!(!e.is_empty());
    assert!((e.min_x - (-3.0)).abs() < 1e-12);
    assert!((e.max_x - 1.0).abs() < 1e-12);
    assert!((e.min_y - 2.0).abs() < 1e-12);
    assert!((e.max_y - 4.0).abs() < 1e-12);
}

// ============================================================
// SVG render
// ============================================================

#[test]
fn svg_envelope_contains_sheet_size_and_views() {
    let (model, solid) = build_box(10.0, 20.0, 30.0);
    let mut drawing = Drawing::new("box-test", SheetSize::A4);
    let view = project_solid_view(
        &model,
        nil_view_source(solid),
        ProjectionType::Front,
        "Front",
        [50.0, 100.0],
        2.0,
    )
    .unwrap();
    drawing.add_view(view);

    let svg = render_drawing_svg(&drawing);
    assert!(svg.starts_with("<?xml"), "must be a well-formed XML doc");
    assert!(svg.contains("xmlns=\"http://www.w3.org/2000/svg\""));
    assert!(
        svg.contains("width=\"297mm\""),
        "A4 width should drive width attribute"
    );
    assert!(
        svg.contains("height=\"210mm\""),
        "A4 height should drive height attribute"
    );
    assert!(svg.contains("box-test"), "drawing title should be rendered");
    assert!(
        svg.contains("class=\"view\""),
        "every view should emit a class=\"view\" group"
    );
    assert!(
        svg.contains("<polyline"),
        "non-empty view must emit polylines"
    );
}

#[test]
fn svg_escapes_special_chars_in_title() {
    let mut drawing = Drawing::new("<safe & sound>", SheetSize::A4);
    // Empty drawing renders without crashing.
    drawing.views.clear();
    let svg = render_drawing_svg(&drawing);
    assert!(svg.contains("&lt;safe &amp; sound&gt;"));
    assert!(!svg.contains("<safe & sound>"));
}

#[test]
fn drawing_add_remove_view_round_trips() {
    let (model, solid) = build_box(10.0, 20.0, 30.0);
    let mut drawing = Drawing::new("rt", SheetSize::A3);
    let view = project_solid_view(
        &model,
        nil_view_source(solid),
        ProjectionType::Front,
        "Front",
        [0.0, 0.0],
        1.0,
    )
    .unwrap();
    let view_id = view.id;
    let returned_id = drawing.add_view(view);
    assert_eq!(returned_id, view_id);
    assert!(drawing.view(view_id).is_some());
    assert!(drawing.remove_view(view_id));
    assert!(drawing.view(view_id).is_none());
}

#[test]
fn sheet_size_dimensions_are_correct() {
    assert_eq!(SheetSize::A4.width(), 297.0);
    assert_eq!(SheetSize::A4.height(), 210.0);
    assert_eq!(SheetSize::A3.width(), 420.0);
    assert_eq!(SheetSize::A0.height(), 841.0);
    let c = SheetSize::Custom {
        width: 500.0,
        height: 400.0,
    };
    assert_eq!(c.width(), 500.0);
    assert_eq!(c.height(), 400.0);
}
