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
