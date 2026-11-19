// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A sheet whose part is not in the model says THAT, in one field.
//!
//! Found 2026-11-19 rehearsing the aerospace video: the wing rib's document
//! reopened without its part, and the sheet certificate reported 19 dangling
//! dimensions and 4 stale ones -- the app read it as "Not sound: 23 to fix",
//! while the agent had certified the same sheet SOUND a minute earlier. The
//! facts were honest, but the one thing to know was buried: the part the
//! sheet was drawn from is gone, and no dimension can be fixed on the sheet.

use geometry_engine::drawing::certify_drawing;
use geometry_engine::drawing::dimensioning::standard_drawing_auto;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn plate() -> (BRepModel, u32) {
    let mut m = BRepModel::new();
    m.set_event_key(Some("plate".to_string()));
    let s = match TopologyBuilder::new(&mut m)
        .create_box_3d(60.0, 40.0, 10.0)
        .unwrap()
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };
    m.set_event_key(None);
    (m, s)
}

#[test]
fn a_sheet_of_a_present_part_names_no_missing_part() {
    let (m, s) = plate();
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let c = certify_drawing(&m, &d);
    assert!(c.sound);
    assert_eq!(c.missing_part, None);
}

#[test]
fn a_sheet_whose_part_is_gone_says_so() {
    let (m, s) = plate();
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");

    // The document reopened without the part: an empty model.
    let empty = BRepModel::new();
    let c = certify_drawing(&empty, &d);

    assert!(!c.sound, "nothing on the sheet can be verified");
    let why = c
        .missing_part
        .as_deref()
        .expect("the certificate names the missing part");
    assert!(
        why.contains("not in the model"),
        "a reader is told the part is gone, not that dimensions need fixing: {why}"
    );
}
