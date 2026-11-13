// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! An engineer edits a view's display: hidden lines, name, scale. A bad
//! field refuses the whole edit and leaves the view untouched.

use geometry_engine::drawing::standard_drawing_hlr;
use geometry_engine::drawing::types::{SheetSize, ViewEdit, ViewEditError};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn sheet() -> geometry_engine::drawing::types::Drawing {
    let mut m = BRepModel::new();
    let b = match TopologyBuilder::new(&mut m)
        .create_box_3d(30.0, 20.0, 16.0)
        .expect("box")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    };
    standard_drawing_hlr(&m, b, uuid::Uuid::nil(), SheetSize::A3, 1.0).expect("hlr")
}

#[test]
fn a_valid_edit_changes_only_what_it_names() {
    let mut d = sheet();
    let id = d.views[0].id;
    let before = d.views[0].clone();
    d.edit_view(
        id,
        &ViewEdit {
            show_hidden: Some(false),
            ..ViewEdit::default()
        },
    )
    .expect("hidden off");
    assert!(!d.views[0].show_hidden);
    assert_eq!(d.views[0].name, before.name);
    assert_eq!(d.views[0].scale, before.scale);
    d.edit_view(
        id,
        &ViewEdit {
            name: Some("  DETAIL B ".into()),
            scale: Some(2.0),
            ..ViewEdit::default()
        },
    )
    .expect("rename + rescale");
    assert_eq!(d.views[0].name, "DETAIL B", "the name is trimmed");
    assert_eq!(d.views[0].scale, 2.0);
}

#[test]
fn a_bad_field_refuses_the_whole_edit() {
    let mut d = sheet();
    let id = d.views[0].id;
    let other = d.views[1].name.clone();
    let before = d.views[0].clone();
    let cases = [
        (ViewEdit::default(), "empty"),
        (
            ViewEdit {
                show_hidden: Some(false),
                scale: Some(0.0),
                ..ViewEdit::default()
            },
            "scale 0",
        ),
        (
            ViewEdit {
                scale: Some(f64::NAN),
                ..ViewEdit::default()
            },
            "scale NaN",
        ),
        (
            ViewEdit {
                scale: Some(500.0),
                ..ViewEdit::default()
            },
            "scale 500",
        ),
        (
            ViewEdit {
                show_hidden: Some(false),
                name: Some(other.to_lowercase()),
                ..ViewEdit::default()
            },
            "name taken",
        ),
        (
            ViewEdit {
                name: Some("   ".into()),
                ..ViewEdit::default()
            },
            "blank name",
        ),
        (
            ViewEdit {
                name: Some("x".repeat(41)),
                ..ViewEdit::default()
            },
            "long name",
        ),
    ];
    for (edit, what) in cases {
        assert!(d.edit_view(id, &edit).is_err(), "{what} is refused");
        assert_eq!(
            d.views[0].show_hidden, before.show_hidden,
            "{what}: unchanged"
        );
        assert_eq!(d.views[0].name, before.name, "{what}: unchanged");
        assert_eq!(d.views[0].scale, before.scale, "{what}: unchanged");
    }
    let missing = geometry_engine::drawing::types::ProjectedViewId::new();
    assert!(matches!(
        d.edit_view(
            missing,
            &ViewEdit {
                show_hidden: Some(true),
                ..ViewEdit::default()
            }
        ),
        Err(ViewEditError::NoSuchView(_))
    ));
}

// ── Dragging a view (Varun 2026-11-13: drag like established CAD) ────────────
//
// The standard HLR sheet of the 30 x 20 x 16 box on A3, third angle:
//   FRONT geometry [186..216] x [133.5..149.5]   (sheet mm, y down), 2 dims
//   TOP   geometry [186..216] x [ 81.5..101.5],  1 dim  (above FRONT)
//   RIGHT geometry [246..266] x [133.5..149.5],  no dims (right of FRONT)
//   frame [20..410] x [10..287]; title block [225..410] x [239..287]
// A dimensioned view's footprint runs 22 mm left of and below its geometry
// (the band `verify_drawing` polices), so FRONT's is [164..216] x [133.5..171.5].

use geometry_engine::drawing::layout::view_geometry_rect;
use geometry_engine::drawing::types::{Drawing, ProjectionType};

fn box_model() -> (BRepModel, u32) {
    let mut m = BRepModel::new();
    let b = match TopologyBuilder::new(&mut m)
        .create_box_3d(30.0, 20.0, 16.0)
        .expect("box")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    };
    (m, b)
}

/// The sheet plus an ISOMETRIC view (no dimensions, no projection line: it
/// moves freely), centred at sheet (80, 230) -- clear of everything.
fn sheet_with_iso() -> Drawing {
    let (m, b) = box_model();
    let mut d = standard_drawing_hlr(&m, b, uuid::Uuid::nil(), SheetSize::A3, 1.0).expect("hlr");
    let like = d.views[0].clone();
    let mut iso = geometry_engine::drawing::projection::project_solid_view(
        &m,
        like.source,
        ProjectionType::Isometric,
        "ISO",
        [0.0, 0.0],
        1.0,
    )
    .expect("iso");
    let c = centre(&d, &iso);
    iso.position_mm = shifted([0.0, 0.0], c, [80.0, 230.0]);
    d.add_view(iso);
    d
}

/// Sheet-space (y-down) centre of a view's geometry.
fn centre(d: &Drawing, v: &geometry_engine::drawing::types::ProjectedView) -> [f64; 2] {
    let r = view_geometry_rect(v, d.sheet_size.height()).expect("geometry");
    [0.5 * (r.x0 + r.x1), 0.5 * (r.y0 + r.y1)]
}

/// The `position_mm` that moves a view whose centre is `c` (at `pos`) to put
/// its centre at sheet point `to` (`position_mm` y runs UP the sheet).
fn shifted(pos: [f64; 2], c: [f64; 2], to: [f64; 2]) -> [f64; 2] {
    [pos[0] + (to[0] - c[0]), pos[1] - (to[1] - c[1])]
}

fn idx(d: &Drawing, name: &str) -> usize {
    d.views
        .iter()
        .position(|v| v.name == name)
        .unwrap_or_else(|| panic!("no view {name}"))
}

fn positions(d: &Drawing) -> Vec<(String, [f64; 2])> {
    d.views
        .iter()
        .map(|v| (v.name.clone(), v.position_mm))
        .collect()
}

fn drag(d: &mut Drawing, name: &str, to: [f64; 2]) -> Result<(), ViewEditError> {
    let id = d.views[idx(d, name)].id;
    d.edit_view(
        id,
        &ViewEdit {
            position_mm: Some(to),
            ..ViewEdit::default()
        },
    )
    .map(|_| ())
}

/// Dropped on top of FRONT: refused, and nothing on the sheet moves. Before,
/// any finite position was accepted.
#[test]
fn a_drop_onto_another_view_is_refused_and_changes_nothing() {
    let mut d = sheet_with_iso();
    let before = positions(&d);
    let iso = d.views[idx(&d, "ISO")].clone();
    let front = d.views[idx(&d, "FRONT")].clone();
    let to = shifted(iso.position_mm, centre(&d, &iso), centre(&d, &front));
    let r = drag(&mut d, "ISO", to);
    assert!(
        matches!(&r, Err(ViewEditError::Overlaps { view, other }) if view == "ISO" && other == "FRONT"),
        "a drop onto FRONT is refused as an overlap, got {r:?}"
    );
    assert_eq!(positions(&d), before, "a refused drop moves nothing");
}

/// Dropped past the frame, or onto the title block: refused.
#[test]
fn a_drop_off_the_sheet_or_on_the_title_block_is_refused() {
    let mut d = sheet_with_iso();
    let before = positions(&d);
    let iso = d.views[idx(&d, "ISO")].clone();
    let c = centre(&d, &iso);
    let off = drag(&mut d, "ISO", shifted(iso.position_mm, c, [440.0, 230.0]));
    assert!(
        matches!(&off, Err(ViewEditError::OffSheet { view }) if view == "ISO"),
        "past the frame is refused, got {off:?}"
    );
    let tb = drag(&mut d, "ISO", shifted(iso.position_mm, c, [317.0, 263.0]));
    assert!(
        matches!(&tb, Err(ViewEditError::OverlapsTitleBlock { view }) if view == "ISO"),
        "on the title block is refused, got {tb:?}"
    );
    assert_eq!(positions(&d), before, "a refused drop moves nothing");
}

/// Re-dropping every view where it is, and a 1 mm nudge into free space, are
/// accepted: the refusal rule is the sheet's own, not a stricter one.
#[test]
fn a_drop_in_place_and_a_small_nudge_are_accepted() {
    let mut d = sheet_with_iso();
    for name in ["FRONT", "TOP", "RIGHT", "ISO"] {
        let p = d.views[idx(&d, name)].position_mm;
        drag(&mut d, name, p).unwrap_or_else(|e| panic!("{name} re-dropped in place: {e}"));
        let q = d.views[idx(&d, name)].position_mm;
        assert!(
            (q[0] - p[0]).abs() < 1e-9 && (q[1] - p[1]).abs() < 1e-9,
            "{name} stays where it was: {p:?} -> {q:?}"
        );
    }
    let p = d.views[idx(&d, "ISO")].position_mm;
    drag(&mut d, "ISO", [p[0] + 1.0, p[1] - 1.0]).expect("a 1 mm nudge into free space");
    assert_eq!(
        d.views[idx(&d, "ISO")].position_mm,
        [p[0] + 1.0, p[1] - 1.0]
    );
}

/// TOP keeps FRONT's column and moves only up/down; RIGHT keeps FRONT's row
/// and moves only left/right. The off-axis part of the drag is ignored, as in
/// established CAD.
#[test]
fn a_projected_view_stays_on_its_projection_line() {
    let mut d = sheet_with_iso();
    let fc = centre(&d, &d.views[idx(&d, "FRONT")].clone());

    let top = d.views[idx(&d, "TOP")].position_mm;
    drag(&mut d, "TOP", [top[0] + 15.0, top[1] + 10.0]).expect("TOP moves up");
    let t = d.views[idx(&d, "TOP")].clone();
    assert!(
        (centre(&d, &t)[0] - fc[0]).abs() < 1e-9,
        "TOP stays in FRONT's column: centre {:?} vs FRONT {fc:?}",
        centre(&d, &t)
    );
    assert!(
        (t.position_mm[1] - (top[1] + 10.0)).abs() < 1e-9,
        "TOP moved up 10"
    );

    let right = d.views[idx(&d, "RIGHT")].position_mm;
    drag(&mut d, "RIGHT", [right[0] + 10.0, right[1] + 12.0]).expect("RIGHT moves right");
    let r = d.views[idx(&d, "RIGHT")].clone();
    assert!(
        (centre(&d, &r)[1] - fc[1]).abs() < 1e-9,
        "RIGHT stays on FRONT's row: centre {:?} vs FRONT {fc:?}",
        centre(&d, &r)
    );
    assert!(
        (r.position_mm[0] - (right[0] + 10.0)).abs() < 1e-9,
        "RIGHT moved right 10"
    );
}

/// A projected view dragged along its line PAST FRONT, to the other side, is
/// refused: third angle puts TOP above FRONT -- below it, it reads as BOTTOM.
#[test]
fn a_projected_view_cannot_cross_to_the_other_side_of_front() {
    let mut d = sheet_with_iso();
    let before = positions(&d);
    let t = d.views[idx(&d, "TOP")].clone();
    let c = centre(&d, &t);
    let r = drag(&mut d, "TOP", shifted(t.position_mm, c, [c[0], 201.5]));
    assert!(
        matches!(&r, Err(ViewEditError::WrongSide { view, side: "above", .. }) if view == "TOP"),
        "TOP below FRONT is refused, got {r:?}"
    );
    assert_eq!(positions(&d), before);
}

/// Moving FRONT carries the views projected from it by the same amount, so
/// they stay on their lines. The isometric stays where it is.
#[test]
fn front_carries_its_projected_views() {
    let mut d = sheet_with_iso();
    let before = d.clone();
    let f = before.views[idx(&before, "FRONT")].position_mm;
    let delta = [-20.0, -10.0];
    drag(&mut d, "FRONT", [f[0] + delta[0], f[1] + delta[1]]).expect("FRONT moves");
    for name in ["FRONT", "TOP", "RIGHT"] {
        let was = before.views[idx(&before, name)].position_mm;
        let now = d.views[idx(&d, name)].position_mm;
        assert_eq!(
            now,
            [was[0] + delta[0], was[1] + delta[1]],
            "{name} moves with FRONT"
        );
    }
    assert_eq!(
        d.views[idx(&d, "ISO")].position_mm,
        before.views[idx(&before, "ISO")].position_mm,
        "the isometric is not projected from FRONT: it stays"
    );
}

/// FRONT itself fits 75 mm higher, but TOP would leave the sheet: the whole
/// move is refused and no view moves.
#[test]
fn a_carry_that_would_leave_the_sheet_refuses_the_whole_move() {
    let mut d = sheet_with_iso();
    let before = positions(&d);
    let f = d.views[idx(&d, "FRONT")].position_mm;
    let r = drag(&mut d, "FRONT", [f[0], f[1] + 75.0]);
    assert!(
        matches!(&r, Err(ViewEditError::OffSheet { view }) if view == "TOP"),
        "TOP would leave the frame: refused, got {r:?}"
    );
    assert_eq!(positions(&d), before, "nothing moved");
}

/// The sheet's own check of the projection arrangement covers LEFT and
/// BOTTOM, not only TOP and RIGHT.
#[test]
fn the_sheet_check_sees_a_left_or_bottom_view_off_its_line() {
    use geometry_engine::drawing::verify::{verify_drawing, DrawingIssueKind};
    let (m, b) = box_model();
    let base = standard_drawing_hlr(&m, b, uuid::Uuid::nil(), SheetSize::A3, 1.0).expect("hlr");
    let fc = centre(&base, &base.views[idx(&base, "FRONT")].clone());
    let misaligned = |d: &Drawing, word: &str| {
        verify_drawing(d)
            .issues
            .iter()
            .any(|i| i.kind == DrawingIssueKind::ProjectionMisaligned && i.message.contains(word))
    };
    for (kind, name, on_line, off_line) in [
        (
            ProjectionType::Left,
            "LEFT",
            [100.0, fc[1]],
            [100.0, fc[1] + 30.0],
        ),
        (
            ProjectionType::Bottom,
            "BOTTOM",
            [fc[0], 205.0],
            [fc[0] + 40.0, 205.0],
        ),
    ] {
        for (at, expect) in [(on_line, false), (off_line, true)] {
            let mut d = base.clone();
            let mut v = geometry_engine::drawing::projection::project_solid_view(
                &m,
                d.views[0].source,
                kind,
                name,
                [0.0, 0.0],
                1.0,
            )
            .expect("view");
            let c = centre(&d, &v);
            v.position_mm = shifted([0.0, 0.0], c, at);
            d.add_view(v);
            let word = if kind == ProjectionType::Left {
                "Left"
            } else {
                "Bottom"
            };
            assert_eq!(
                misaligned(&d, word),
                expect,
                "{name} centred at {at:?}: misaligned expected {expect}; issues {:?}",
                verify_drawing(&d)
                    .issues
                    .iter()
                    .map(|i| &i.message)
                    .collect::<Vec<_>>()
            );
        }
    }
}

/// A drag reports where every moved view went, the dragged view first. Those
/// recorded moves, applied to the sheet as it was, put every view in exactly
/// the same place -- that is what the history replays.
#[test]
fn a_drag_reports_every_moved_view_and_its_record_replays_exactly() {
    let fresh = sheet_with_iso();
    let mut live = fresh.clone();
    let f = fresh.views[idx(&fresh, "FRONT")].clone();
    let edit = ViewEdit {
        position_mm: Some([f.position_mm[0] - 20.0, f.position_mm[1] - 10.0]),
        ..ViewEdit::default()
    };
    let moves = live.edit_view(f.id, &edit).expect("FRONT moves");
    let names: Vec<String> = moves
        .iter()
        .map(|m| {
            live.views
                .iter()
                .find(|v| v.id.0 == m.view_id)
                .expect("a moved view is on the sheet")
                .name
                .clone()
        })
        .collect();
    assert_eq!(
        names,
        ["FRONT", "TOP", "RIGHT"],
        "FRONT first, then what it carried"
    );
    for m in &moves {
        let v = live
            .views
            .iter()
            .find(|v| v.id.0 == m.view_id)
            .expect("view");
        assert_eq!(
            v.position_mm, m.position_mm,
            "{} is where its move says",
            v.name
        );
    }

    let mut replayed = fresh.clone();
    replayed
        .apply_view_edit(f.id, &ViewEdit::default(), Some(&moves))
        .expect("the recorded move applies");
    assert_eq!(positions(&replayed), positions(&live));

    // A recorded move is checked as a live one: one that no longer fits is
    // refused whole and the sheet stays as it was.
    let mut grown = fresh.clone();
    let iso = grown.views[idx(&grown, "ISO")].clone();
    let onto_front = shifted(iso.position_mm, centre(&grown, &iso), centre(&grown, &f));
    let bad = [geometry_engine::drawing::types::ViewMove {
        view_id: iso.id.0,
        position_mm: onto_front,
    }];
    let before = positions(&grown);
    assert!(matches!(
        grown.apply_view_edit(iso.id, &ViewEdit::default(), Some(&bad)),
        Err(ViewEditError::Overlaps { .. })
    ));
    assert_eq!(positions(&grown), before);
}

/// An edit recorded before drags were resolved (no `moves`) applies exactly
/// as it always did -- even a placement the live drag now refuses -- so old
/// histories rebuild unchanged.
#[test]
fn an_edit_recorded_without_moves_applies_as_it_always_did() {
    let mut d = sheet_with_iso();
    let iso = d.views[idx(&d, "ISO")].clone();
    let front = d.views[idx(&d, "FRONT")].clone();
    let onto_front = shifted(iso.position_mm, centre(&d, &iso), centre(&d, &front));
    d.apply_view_edit(
        iso.id,
        &ViewEdit {
            position_mm: Some(onto_front),
            ..ViewEdit::default()
        },
        None,
    )
    .expect("a legacy edit applies verbatim");
    assert_eq!(d.views[idx(&d, "ISO")].position_mm, onto_front);
    // ...and a legacy FRONT move carries nothing.
    let top = d.views[idx(&d, "TOP")].position_mm;
    d.apply_view_edit(
        front.id,
        &ViewEdit {
            position_mm: Some([front.position_mm[0] - 20.0, front.position_mm[1]]),
            ..ViewEdit::default()
        },
        None,
    )
    .expect("legacy FRONT move");
    assert_eq!(d.views[idx(&d, "TOP")].position_mm, top, "nothing carried");
}

/// `view_alignment` measures: TOP on FRONT's column is aligned, TOP pushed
/// off it is not, FRONT reports its views, the isometric has no line.
#[test]
fn alignment_is_measured_not_claimed() {
    use geometry_engine::drawing::layout::view_alignment;
    let mut d = sheet_with_iso();
    let (fi, ti, ii) = (idx(&d, "FRONT"), idx(&d, "TOP"), idx(&d, "ISO"));
    assert_eq!(view_alignment(&d, ti), Some(true));
    assert_eq!(view_alignment(&d, fi), Some(true));
    assert_eq!(view_alignment(&d, ii), None, "an isometric has no line");
    d.views[ti].position_mm[0] += 5.0;
    assert_eq!(view_alignment(&d, ti), Some(false));
    assert_eq!(
        view_alignment(&d, fi),
        Some(false),
        "FRONT: TOP is off its line"
    );
    // A view already off its line moves where it is dropped.
    let p = d.views[ti].position_mm;
    drag(&mut d, "TOP", [p[0] + 3.0, p[1] + 4.0]).expect("free move");
    assert_eq!(d.views[ti].position_mm, [p[0] + 3.0, p[1] + 4.0]);
}
