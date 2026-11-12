// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! An engineer rescales a view. The view keeps its centre when the bigger
//! footprint fits there; otherwise it moves -- along its projection line when
//! it is an orthographic child of FRONT -- and when nothing on the sheet
//! fits, the rescale is refused instead of overlapping another view.

use geometry_engine::drawing::layout::{rescaled_view_position, view_geometry_rect, Rect2};
use geometry_engine::drawing::standard_drawing_hlr;
use geometry_engine::drawing::types::{Drawing, ProjectionType, SheetSize};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn sheet(size: SheetSize) -> Drawing {
    let mut m = BRepModel::new();
    let b = match TopologyBuilder::new(&mut m)
        .create_box_3d(30.0, 20.0, 16.0)
        .expect("box")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    };
    standard_drawing_hlr(&m, b, uuid::Uuid::nil(), size, 1.0).expect("hlr")
}

fn idx(d: &Drawing, p: fn(&ProjectionType) -> bool) -> usize {
    d.views.iter().position(|v| p(&v.projection)).expect("view")
}

fn rect(d: &Drawing, i: usize) -> Rect2 {
    view_geometry_rect(&d.views[i], d.sheet_size.height()).expect("rect")
}

fn centre(r: &Rect2) -> [f64; 2] {
    [0.5 * (r.x0 + r.x1), 0.5 * (r.y0 + r.y1)]
}

fn apply(d: &mut Drawing, i: usize, scale: f64, pos: [f64; 2]) {
    d.views[i].scale = scale;
    d.views[i].position_mm = pos;
}

fn clear_of_other_views(d: &Drawing, i: usize) -> bool {
    let r = rect(d, i);
    (0..d.views.len())
        .filter(|&j| j != i)
        .all(|j| !r.intersects(&rect(d, j), 0.0))
}

#[test]
fn a_small_rescale_with_room_keeps_the_view_centre() {
    let mut d = sheet(SheetSize::A2);
    let iso = idx(&d, |p| matches!(p, ProjectionType::Right));
    let before = centre(&rect(&d, iso));
    let pos = rescaled_view_position(&d, iso, 1.2).expect("room at 1.2:1");
    assert!(!pos.moved);
    let pos = pos.position;
    apply(&mut d, iso, 1.2, pos);
    let after = centre(&rect(&d, iso));
    assert!(
        (after[0] - before[0]).abs() < 1e-6 && (after[1] - before[1]).abs() < 1e-6,
        "centre kept: {before:?} -> {after:?}"
    );
    assert!(clear_of_other_views(&d, iso));
}

#[test]
fn a_rescaled_child_view_that_must_move_stays_aligned_with_front() {
    let mut d = sheet(SheetSize::A2);
    let front = idx(&d, |p| matches!(p, ProjectionType::Front));
    let top = idx(&d, |p| matches!(p, ProjectionType::Top));
    let pos = rescaled_view_position(&d, top, 4.0).expect("A2 has room for TOP at 4:1");
    assert!(pos.moved && pos.aligned, "{pos:?}");
    let pos = pos.position;
    apply(&mut d, top, 4.0, pos);
    assert!(
        clear_of_other_views(&d, top),
        "never on top of another view"
    );
    let (f, t) = (centre(&rect(&d, front)), centre(&rect(&d, top)));
    assert!(
        (f[0] - t[0]).abs() < 1e-6,
        "TOP stays in FRONT's column: {f:?} vs {t:?}"
    );
}

#[test]
fn a_rescale_that_fits_nowhere_is_refused() {
    let d = sheet(SheetSize::A4);
    let front = idx(&d, |p| matches!(p, ProjectionType::Front));
    assert!(rescaled_view_position(&d, front, 50.0).is_none());
}
