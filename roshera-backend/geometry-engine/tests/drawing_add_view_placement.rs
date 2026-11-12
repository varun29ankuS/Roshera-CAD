// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! "Add view ▸ Left" places itself. A person picks WHICH view; the kernel
//! finds WHERE: a free spot on the sheet, clear of every view, label,
//! dimension, table and the title block, as close as possible to the view it
//! was taken from. Before, the add-view form asked for X/Y in millimetres
//! and put the view at (0, 0) when none was typed.

use geometry_engine::drawing::dimensioning::standard_drawing_auto;
use geometry_engine::drawing::layout::{compute_layout, free_view_position, SheetItemKind};
use geometry_engine::drawing::projection::project_solid_view;
use geometry_engine::drawing::svg::frame_rect;
use geometry_engine::drawing::types::ProjectionType;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn solid(g: GeometryId) -> u32 {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}

#[test]
fn an_added_view_lands_on_free_sheet_inside_the_frame() {
    let mut m = BRepModel::new();
    let s = solid(
        TopologyBuilder::new(&mut m)
            .create_box_3d(60.0, 40.0, 20.0)
            .unwrap(),
    );
    let mut d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let like = &d.views[0];
    let mut left = project_solid_view(
        &m,
        like.source,
        ProjectionType::Left,
        "LEFT",
        [0.0, 0.0],
        like.scale,
    )
    .expect("left view");

    let pos = free_view_position(&d, &left, Some(0)).expect("a free spot exists");
    left.position_mm = pos;
    let before = compute_layout(&d);
    d.add_view(left);
    let after = compute_layout(&d);

    let new_rect = after
        .items
        .iter()
        .filter(|i| i.kind == SheetItemKind::ViewGeometry)
        .last()
        .expect("the new view's geometry")
        .bbox;
    let frame = frame_rect(&d.sheet_size);
    assert!(
        new_rect.x0 >= frame.x0
            && new_rect.x1 <= frame.x1
            && new_rect.y0 >= frame.y0
            && new_rect.y1 <= frame.y1,
        "inside the frame: {new_rect:?} vs {frame:?}"
    );
    for it in before
        .items
        .iter()
        .filter(|i| i.kind != SheetItemKind::ZoneRef)
    {
        assert!(
            !new_rect.intersects(&it.bbox, 0.0),
            "the new view overlaps {:?} {:?}",
            it.kind,
            it.text
        );
    }
}

#[test]
fn a_full_sheet_refuses_instead_of_overlapping() {
    let mut m = BRepModel::new();
    let s = solid(
        TopologyBuilder::new(&mut m)
            .create_box_3d(60.0, 40.0, 20.0)
            .unwrap(),
    );
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let like = &d.views[0];
    // A view far larger than the sheet: no free spot can hold it.
    let huge = project_solid_view(
        &m,
        like.source,
        ProjectionType::Left,
        "LEFT",
        [0.0, 0.0],
        50.0,
    )
    .expect("view");
    assert_eq!(free_view_position(&d, &huge, Some(0)), None);
}

/// An added view sits where the projection convention puts it: third angle,
/// LEFT to the left of FRONT on the same row, BOTTOM below it in the same
/// column; first angle mirrors both. Before, it went to the nearest free
/// spot (on the live plate: LEFT under FRONT).
#[test]
fn an_added_view_is_aligned_by_the_projection_convention() {
    use geometry_engine::drawing::layout::{aligned_view_position, ViewSide};
    use geometry_engine::drawing::types::ProjectionAngle;
    let mut m = BRepModel::new();
    let s = solid(
        TopologyBuilder::new(&mut m)
            .create_box_3d(30.0, 20.0, 10.0)
            .unwrap(),
    );
    for angle in [ProjectionAngle::Third, ProjectionAngle::First] {
        let mut d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
        d.projection = angle;
        // FRONT alone, so every side has room: this tests the alignment.
        // (A side with no room is refused -- see the case below.)
        d.views.retain(|v| v.projection == ProjectionType::Front);
        d.hole_sites.clear();
        // ...in the middle of a large sheet (on the auto A4 sheet FRONT's own
        // dimensions and the title block crowd every side).
        d.sheet_size = geometry_engine::drawing::types::SheetSize::A2;
        d.views[0].position_mm = [270.0, 220.0];
        let front_idx = d
            .views
            .iter()
            .position(|v| v.projection == ProjectionType::Front)
            .expect("FRONT");
        let sheet_h = d.sheet_size.height();
        let front = compute_layout(&d)
            .items
            .iter()
            .find(|i| i.kind == SheetItemKind::ViewGeometry && i.owner_view == Some(front_idx))
            .expect("front geometry")
            .bbox;
        for (kind, name) in [
            (ProjectionType::Left, "LEFT"),
            (ProjectionType::Bottom, "BOTTOM"),
        ] {
            let like = &d.views[front_idx];
            let mut v = project_solid_view(&m, like.source, kind, name, [0.0, 0.0], like.scale)
                .expect("view");
            let side = ViewSide::for_projection(kind, angle).expect("a side for LEFT/BOTTOM");
            let pos = aligned_view_position(&d, &v, front_idx, side)
                .unwrap_or_else(|| {
                    panic!(
                        "{angle:?} {name}: no room on the {side:?} of FRONT {front:?}; frame {:?}; views {:?}",
                        geometry_engine::drawing::svg::frame_rect(&d.sheet_size),
                        compute_layout(&d)
                            .items
                            .iter()
                            .filter(|i| i.kind == SheetItemKind::ViewGeometry)
                            .map(|i| (i.owner_view, i.bbox))
                            .collect::<Vec<_>>()
                    )
                });
            v.position_mm = pos;
            let mut probe = d.clone();
            probe.add_view(v);
            let r = compute_layout(&probe)
                .items
                .iter()
                .filter(|i| i.kind == SheetItemKind::ViewGeometry)
                .last()
                .expect("new view")
                .bbox;
            let (cx, cy) = (0.5 * (r.x0 + r.x1), 0.5 * (r.y0 + r.y1));
            let (fx, fy) = (0.5 * (front.x0 + front.x1), 0.5 * (front.y0 + front.y1));
            let _ = sheet_h;
            match (angle, kind) {
                (ProjectionAngle::Third, ProjectionType::Left)
                | (ProjectionAngle::First, ProjectionType::Right) => {
                    assert!(r.x1 < front.x0, "{angle:?} {name} left of FRONT");
                    assert!((cy - fy).abs() < 1e-6, "{angle:?} {name} on FRONT's row");
                }
                (ProjectionAngle::First, ProjectionType::Left) => {
                    assert!(r.x0 > front.x1, "{angle:?} LEFT right of FRONT");
                    assert!((cy - fy).abs() < 1e-6, "{angle:?} LEFT on FRONT's row");
                }
                (ProjectionAngle::Third, ProjectionType::Bottom) => {
                    assert!(r.y0 > front.y1, "third-angle BOTTOM below FRONT");
                    assert!((cx - fx).abs() < 1e-6, "BOTTOM in FRONT's column");
                }
                (ProjectionAngle::First, ProjectionType::Bottom) => {
                    assert!(r.y1 < front.y0, "first-angle BOTTOM above FRONT");
                    assert!((cx - fx).abs() < 1e-6, "BOTTOM in FRONT's column");
                }
                _ => {}
            }
        }
    }
}

/// No room on the aligned side: refused (the caller falls back to the
/// nearest free spot and says so) -- never placed on top of the dimensions.
#[test]
fn a_blocked_side_is_refused_not_overlapped() {
    use geometry_engine::drawing::layout::{aligned_view_position, ViewSide};
    let mut m = BRepModel::new();
    let s = solid(
        TopologyBuilder::new(&mut m)
            .create_box_3d(30.0, 20.0, 10.0)
            .unwrap(),
    );
    // The standard A4 sheet: FRONT's dimension stack fills its left side.
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let fi = d
        .views
        .iter()
        .position(|v| v.projection == ProjectionType::Front)
        .expect("FRONT");
    let like = &d.views[fi];
    let left = project_solid_view(
        &m,
        like.source,
        ProjectionType::Left,
        "LEFT",
        [0.0, 0.0],
        like.scale,
    )
    .expect("view");
    assert_eq!(aligned_view_position(&d, &left, fi, ViewSide::Left), None);
}
