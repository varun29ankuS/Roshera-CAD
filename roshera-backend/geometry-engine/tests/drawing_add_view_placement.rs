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
