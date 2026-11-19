// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The sheet never lies about completeness.
//!
//! The automatic sheet used to thin a busy view (it kept five diameters,
//! eight dimensions, no angles when there were more than two) and the
//! certificate walked only bores and GD&T, so a stepped shaft whose middle
//! journals were dropped certified exactly like one fully dimensioned. The
//! thinning is gone -- every journal is dimensioned -- and these pin that any
//! model dimension no view states (it had no room, it was taken out of the
//! stored sheet) is a typed fact on the certificate, that it makes the sheet
//! not complete, and that a dimension a person takes off the sheet is a
//! WAIVED omission naming them.

use geometry_engine::drawing::dimension_edit::{DimensionEdit, Waiver};
use geometry_engine::drawing::dimensioning::{auto_dimensions, standard_drawing_auto};
use geometry_engine::drawing::sheet_certificate::{certify_drawing, SheetFactKind, SheetVerdict};
use geometry_engine::drawing::svg::render_drawing_svg;
use geometry_engine::drawing::types::ProjectionType;
use geometry_engine::drawing::OmissionReason;
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

/// A shaft of seven journals, each 10 long: Ø40, Ø36, Ø32, Ø28, Ø24, Ø20, Ø16.
fn stepped_shaft() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let mut shaft: Option<SolidId> = None;
    for (i, d) in [40.0, 36.0, 32.0, 28.0, 24.0, 20.0, 16.0]
        .iter()
        .enumerate()
    {
        // Each journal is its own recorded step, as a real history is.
        m.set_event_key(Some(format!("journal-{i}")));
        let journal = solid(
            TopologyBuilder::new(&mut m)
                .create_cylinder_3d(
                    Point3::new(0.0, 0.0, 10.0 * i as f64),
                    Vector3::Z,
                    0.5 * d,
                    10.0,
                )
                .unwrap(),
        );
        m.set_event_key(Some(format!("join-{i}")));
        shaft = Some(match shaft {
            None => journal,
            Some(s) => boolean_operation(
                &mut m,
                s,
                journal,
                BooleanOp::Union,
                BooleanOptions::default(),
            )
            .unwrap(),
        });
    }
    m.set_event_key(None);
    (m, shaft.unwrap())
}

fn block(x: f64, y: f64, z: f64) -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    m.set_event_key(Some("block".to_string()));
    let s = solid(TopologyBuilder::new(&mut m).create_box_3d(x, y, z).unwrap());
    m.set_event_key(None);
    (m, s)
}

/// Every diameter the model's dimension table carries, as the sheet prints
/// it: the ground truth, independent of what the sheet chose to draw.
fn model_diameters(m: &BRepModel, s: SolidId) -> Vec<String> {
    let mut diameters: Vec<String> = auto_dimensions(m, s, ProjectionType::Front)
        .into_iter()
        .filter(|x| x.kind == "diameter")
        .map(|x| x.label)
        .collect();
    diameters.sort();
    diameters.dedup();
    diameters
}

/// No dimension is thinned away: all seven journals of the stepped shaft are
/// inked, and the certificate lists no diameter as missing. (The view used
/// to keep the three largest and two smallest diameters and at most eight
/// dimensions; the two middle journals were on no view.)
///
/// Mutation: cap `visible_dimensions` at five diameters per view again (the
/// old diameter limit) and a journal goes missing. (Capping at eight
/// dimensions alone does not: the seven diameters come first.)
#[test]
fn every_journal_of_a_stepped_shaft_is_dimensioned() {
    let (m, s) = stepped_shaft();
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let svg = render_drawing_svg(&d);
    let diameters = model_diameters(&m, s);
    assert_eq!(
        diameters.len(),
        7,
        "fixture: seven journals give seven diameters, got {diameters:?}"
    );
    for label in &diameters {
        assert!(
            svg.contains(&format!(">{label}<")),
            "{label} is not inked; the sheet must dimension every journal"
        );
    }
    let cert = certify_drawing(&m, &d);
    let missing: Vec<_> = cert
        .omissions
        .iter()
        .filter(|o| o.kind == "diameter")
        .collect();
    assert!(missing.is_empty(), "no journal is omitted: {missing:?}");
    assert_eq!(cert.counts.stale, 0);
    assert_eq!(cert.counts.dangling, 0);
}

/// Take one journal's diameter out of the stored sheet, as an edited or
/// older record would have it.
fn drop_one_journal(d: &mut geometry_engine::drawing::Drawing) -> String {
    for v in &mut d.views {
        if let Some(i) = v.dimensions.iter().position(|x| x.kind == "diameter") {
            return v.dimensions.remove(i).label;
        }
    }
    panic!("the sheet carries no diameter");
}

/// THE defect: a dimension the model has and no view states must be a fact
/// on the certificate, and the sheet must not read as complete -- whatever
/// the reason it is missing. The certificate walks the live model, so it
/// does not depend on the sheet remembering what it left out.
///
/// Mutation: remove the omission walk from `certify_drawing` (or make
/// `dimension_omissions` return nothing) and this goes red.
#[test]
fn a_dimension_no_view_states_is_reported_and_the_sheet_is_not_complete() {
    let (m, s) = stepped_shaft();
    let mut d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let label = drop_one_journal(&mut d);
    assert!(
        !render_drawing_svg(&d).contains(&format!(">{label}<")),
        "fixture: {label} is no longer inked"
    );

    let cert = certify_drawing(&m, &d);
    let fact = cert.facts.iter().find(|f| {
        f.kind == SheetFactKind::Omitted
            && f.live.verdict == SheetVerdict::NotDimensioned
            && f.label.contains(label.as_str())
    });
    assert!(
        fact.is_some(),
        "{label} is on no view and the certificate does not say so; facts: {:?}",
        cert.facts
            .iter()
            .map(|f| (&f.label, f.live.verdict))
            .collect::<Vec<_>>()
    );
    assert!(
        !cert.complete,
        "a sheet that omits a journal is not complete"
    );
    assert_eq!(cert.counts.not_dimensioned, cert.omissions.len());
    assert!(
        cert.omissions
            .iter()
            .any(|o| o.reason == OmissionReason::NotOnSheet && o.label == label),
        "the reason says it is on no view: {:?}",
        cert.omissions
    );
    assert!(
        cert.omission_summary
            .iter()
            .any(|l| l.contains("not dimensioned on")),
        "a person reads it in one line: {:?}",
        cert.omission_summary
    );
    // Nothing shown is wrong: an omission is incompleteness, not a lie.
    assert_eq!(cert.counts.stale, 0);
    assert_eq!(cert.counts.dangling, 0);
}

/// The other direction: a sheet that states everything is complete. A block's
/// three sizes are all inked, so nothing is reported.
#[test]
fn a_sheet_that_states_every_dimension_is_complete() {
    let (m, s) = block(120.0, 60.0, 20.0);
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let cert = certify_drawing(&m, &d);
    assert!(
        cert.omissions.is_empty(),
        "nothing is left out of a block's sheet: {:?}",
        cert.omissions
    );
    assert!(cert.complete && cert.sound);
}

/// A cube's width, depth and height are three statements with one value. The
/// selection merged equal values in a view regardless of direction; the
/// height then stood on no view at all.
#[test]
fn a_cubes_three_equal_sizes_are_all_stated() {
    let (m, s) = block(40.0, 40.0, 40.0);
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let cert = certify_drawing(&m, &d);
    assert!(
        cert.complete,
        "a cube's sheet states all three sizes: {:?}",
        cert.omissions
    );
    let inked = render_drawing_svg(&d).matches(">40<").count();
    assert_eq!(inked, 3, "width, depth and height, once each");
}

/// A person takes a dimension off the sheet: it leaves the ink, it is listed
/// as a WAIVED omission naming them, and the sheet stays complete because the
/// gap is signed for. Clearing the suppression brings the dimension back.
///
/// Mutation: stop the layout skipping suppressed dimensions and the first
/// assertion goes red; drop the waiver from the omission and the second does.
#[test]
fn a_suppressed_dimension_is_a_waived_omission_naming_who_suppressed_it() {
    let (m, s) = block(120.0, 60.0, 20.0);
    let mut d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    assert!(render_drawing_svg(&d).contains(">20<"));
    let id = d
        .views
        .iter()
        .flat_map(|v| v.dimensions.iter())
        .find(|x| x.label == "20")
        .expect("the 20 mm size is dimensioned")
        .id
        .clone();

    let key = d
        .set_dimension_edit(
            &id,
            DimensionEdit {
                suppressed: Some(Waiver {
                    by: "varun".to_string(),
                    reason: "stock thickness, see material note".to_string(),
                }),
                ..DimensionEdit::default()
            },
            false,
        )
        .expect("suppress");

    assert!(
        !render_drawing_svg(&d).contains(">20<"),
        "a suppressed dimension is not inked"
    );
    let cert = certify_drawing(&m, &d);
    let waived: Vec<_> = cert
        .facts
        .iter()
        .filter(|f| f.live.verdict == SheetVerdict::Waived)
        .collect();
    assert_eq!(waived.len(), 1, "{:?}", cert.omissions);
    assert!(waived[0].label.contains("20") && waived[0].label.contains("varun"));
    assert!(waived[0]
        .live
        .detail
        .as_deref()
        .is_some_and(|t| t.contains("stock thickness")));
    assert_eq!(cert.counts.waived, 1);
    assert_eq!(cert.counts.not_dimensioned, 0);
    assert!(cert.complete, "every gap is signed for");
    assert!(cert.sound);
    assert_eq!(
        cert.omissions[0].waiver.as_ref().map(|w| w.by.as_str()),
        Some("varun")
    );

    // Un-suppress: the record is cleared and the dimension is back.
    d.set_dimension_edit(&key, DimensionEdit::default(), false)
        .expect("clear");
    assert!(d.dimension_edits.is_empty());
    assert!(render_drawing_svg(&d).contains(">20<"));
    assert!(certify_drawing(&m, &d).omissions.is_empty());

    // A suppression must be signed.
    let unsigned = d.set_dimension_edit(
        &id,
        DimensionEdit {
            suppressed: Some(Waiver {
                by: String::new(),
                reason: String::new(),
            }),
            ..DimensionEdit::default()
        },
        false,
    );
    assert!(unsigned.is_err(), "nobody suppressed it: refused");
}

/// A dimension that is already off the sheet can be waived by its key, and
/// only waived: it then counts as signed for.
#[test]
fn an_omitted_dimension_can_be_waived_by_its_key() {
    let (m, s) = stepped_shaft();
    let mut d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let label = drop_one_journal(&mut d);
    let before = certify_drawing(&m, &d);
    let first = before
        .omissions
        .iter()
        .find(|o| o.label == label)
        .expect("the journal taken off the sheet is an omission")
        .clone();
    let key = first.key.clone().expect("the journal has a durable key");
    d.set_dimension_edit(
        &key,
        DimensionEdit {
            suppressed: Some(Waiver {
                by: "checker".to_string(),
                reason: String::new(),
            }),
            ..DimensionEdit::default()
        },
        true,
    )
    .expect("waive");
    let after = certify_drawing(&m, &d);
    assert_eq!(after.counts.waived, 1);
    assert_eq!(
        after.counts.not_dimensioned + 1,
        before.counts.not_dimensioned
    );
    // An unknown key is not a dimension.
    assert!(d
        .set_dimension_edit("no-such-dimension", DimensionEdit::default(), false)
        .is_err());
}
