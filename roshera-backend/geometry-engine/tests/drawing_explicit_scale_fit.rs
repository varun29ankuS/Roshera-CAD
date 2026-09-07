// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Task 40 -- the EXPLICIT-SCALE drawing route must place its views from the
//! SCALED extents, or refuse.
//!
//! `standard_drawing_hlr` is production: `api-server/src/drawing_mgr.rs:1046`
//! sends every request that supplies a `scale` here. It stamped three HARDCODED
//! sheet positions (`[80,110] / [80,210] / [210,110]`) chosen for a 1:1 sheet
//! and never looked at what the requested scale did to the views' footprints,
//! so the sheet was laid out as if the part were drawn 1:1 whatever scale was
//! asked for.
//!
//! Measured on the 40x40x20 bored plate below, on A3, BEFORE the fix -- the
//! positions never moved and the Error-severity issues `verify_drawing` raised
//! were:
//!
//! ```text
//! 0.5 [] | 1.0 [] | 1.5 []
//! 2.0 [ViewOutsideFrame x2]
//! 2.5 [ViewOutsideFrame x2]
//! 3.0 [ViewOutsideFrame x2, ViewOverlap x3]
//! 4.0 [ViewOutsideFrame x2, ViewOverlapsTitleBlock, ViewOverlap x3]
//! ```
//!
//! The fix reuses the AUTOMATIC route's placement machinery
//! (`place_four_view`, the same function `standard_drawing_auto` reaches
//! through `layout_four_view`) at the REQUESTED scale, and refuses with
//! [`ProjectionError::ScaleDoesNotFitSheet`] -- naming the scale, the sheet and
//! the overflow in mm -- when the arrangement cannot fit. Never an off-sheet
//! view; never a silently substituted scale the title block would then
//! misreport as the scale that was asked for.
//!
//! Fix round 1 added the other half of "is this scale usable at all":
//! [`ProjectionError::InvalidScale`] for a `scale` that is NaN, infinite, zero
//! or negative. Those were the quieter defect -- NaN, `-inf` and `0` each
//! produced a sheet `verify_drawing` certified CLEAN, because every comparison
//! against NaN is false and a zero-area footprint is inside every frame. A
//! drawing of nothing, with a clean bill of health.

use geometry_engine::drawing::{
    standard_drawing_auto, standard_drawing_hlr, verify_drawing, Drawing, DrawingIssue,
    DrawingIssueKind, ProjectionError, Severity, SheetSize,
};
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// The fixture: a 40x40x20 plate with one Ø10 through bore -- the same part
/// Task 17's `explicit_scale_hlr_sheet_tables_its_bores_and_certifies_sound`
/// measured the off-sheet overflow on, so the numbers here are comparable to
/// that finding.
fn bored_plate() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    m.set_event_key(Some("explicit-scale-plate".to_string()));
    let plate = match TopologyBuilder::new(&mut m).create_box_3d(40.0, 40.0, 20.0) {
        Ok(GeometryId::Solid(s)) => s,
        o => panic!("expected solid, got {o:?}"),
    };
    let bore = match TopologyBuilder::new(&mut m).create_cylinder_3d(
        Point3::new(0.0, 0.0, -20.0),
        Vector3::Z,
        5.0,
        80.0,
    ) {
        Ok(GeometryId::Solid(s)) => s,
        o => panic!("expected solid, got {o:?}"),
    };
    let part = boolean_operation(
        &mut m,
        plate,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("difference");
    m.set_event_key(None);
    (m, part)
}

/// Error-severity issues only. A Warning is style; an Error is a sheet a shop
/// reader cannot use.
fn errors(d: &Drawing) -> Vec<DrawingIssue> {
    verify_drawing(d)
        .issues
        .into_iter()
        .filter(|i| i.severity == Severity::Error)
        .collect()
}

/// RED before the fix: `[ViewOutsideFrame, ViewOutsideFrame]`.
///
/// The assertion is ZERO Error-severity issues, not merely "no
/// `ViewOutsideFrame`" -- an overflow that reappears as `ViewOverlap` or
/// `ViewOverlapsTitleBlock` is the same defect wearing a different label, and a
/// test that only names one kind would call that fixed.
#[test]
fn explicit_scale_two_to_one_places_every_view_inside_the_frame() {
    let (m, part) = bored_plate();
    let d = standard_drawing_hlr(&m, part, uuid::Uuid::nil(), SheetSize::A3, 2.0)
        .expect("2:1 on A3 fits a 40 mm plate; the route must not refuse it");

    let errs = errors(&d);
    assert!(
        errs.is_empty(),
        "2:1 on A3 must produce a clean sheet; got {:?}",
        errs.iter().map(|e| e.kind).collect::<Vec<_>>()
    );

    // The scale that was ASKED for is the scale that was DRAWN -- a route that
    // fits by quietly reducing the scale is lying in the title block.
    assert_eq!(d.views.len(), 3, "third-angle sheet is three views");
    for v in &d.views {
        assert_eq!(
            v.scale, 2.0,
            "view '{}' must be drawn at the requested 2.0, not a substituted scale",
            v.name
        );
    }

    // Placement must have MOVED off the old hardcoded triple; had it not, the
    // clean verdict would be an accident of some other change.
    let pos: Vec<[f64; 2]> = d.views.iter().map(|v| v.position_mm).collect();
    assert!(
        pos != vec![[80.0, 110.0], [80.0, 210.0], [210.0, 110.0]],
        "the hardcoded 1:1 positions are still in place: {pos:?}"
    );

    // Task 17's hole table survives the new placement path.
    assert!(
        !d.hole_sites.is_empty(),
        "the explicit-scale route must still table the bore it drew"
    );
}

/// 1:1 is unchanged IN KIND: still `Ok`, still clean, still exactly the scale
/// asked for, still tabling its bore. It is NOT unchanged in POSITION -- the
/// binding design forbids a second placement algorithm, so 1:1 is now centred
/// by the same machinery as every other scale rather than stamped at
/// `[80,110]`.
#[test]
fn explicit_scale_one_to_one_is_unchanged_in_kind() {
    let (m, part) = bored_plate();
    let d =
        standard_drawing_hlr(&m, part, uuid::Uuid::nil(), SheetSize::A3, 1.0).expect("1:1 sheet");

    let errs = errors(&d);
    assert!(
        errs.is_empty(),
        "1:1 on A3 was clean before and must stay clean; got {:?}",
        errs.iter().map(|e| e.kind).collect::<Vec<_>>()
    );
    assert_eq!(d.views.len(), 3);
    for v in &d.views {
        assert_eq!(v.scale, 1.0, "view '{}' drawn at the requested 1.0", v.name);
    }
    assert!(!d.hole_sites.is_empty(), "1:1 sheet still tables its bore");
}

/// A scale no arrangement fits is REFUSED, and the refusal says why in the
/// units a draughtsman works in.
///
/// The overflow figures are pinned, not merely required to be positive: they
/// are what proves the number was measured from the SCALED extents of THIS
/// part on THIS sheet. On A3 the usable drawing area is 368 x 211 mm (frame
/// margins, the 22/18 mm dimension pads and the 48 mm title-block band
/// deducted); the group must clear its HEIGHT by a further
/// `BOTTOM_DIM_SHORTFALL` = 2*(22 - 18) = 8 mm, so the fit height is 203 mm.
/// The plate's unit spans are left 40, right 40, top 40, bottom 20, so at 10:1
/// the group is 10*80 + 30 = 830 wide and 10*60 + 32 = 632 tall -- 830 - 368 =
/// 462.0 mm and 632 - 203 = 429.0 mm of overrun.
#[test]
fn a_scale_that_fits_no_arrangement_is_refused_by_name() {
    let (m, part) = bored_plate();
    let err = standard_drawing_hlr(&m, part, uuid::Uuid::nil(), SheetSize::A3, 10.0)
        .expect_err("10:1 cannot fit a 40 mm plate on A3; the route must refuse");

    match &err {
        ProjectionError::ScaleDoesNotFitSheet {
            scale,
            sheet,
            overflow_x_mm,
            overflow_y_mm,
        } => {
            assert_eq!(
                *scale, 10.0,
                "the refusal names the scale that was asked for"
            );
            assert_eq!(sheet, "A3", "the refusal names the sheet it was asked for");
            assert_eq!(*overflow_x_mm, 462.0, "horizontal overrun, mm");
            assert_eq!(*overflow_y_mm, 429.0, "vertical overrun, mm");
        }
        other => panic!("expected ScaleDoesNotFitSheet, got {other:?}"),
    }

    let msg = err.to_string();
    for needle in ["10", "A3", "462.0", "429.0", "mm"] {
        assert!(
            msg.contains(needle),
            "refusal must name {needle:?}; message was {msg:?}"
        );
    }
    assert!(
        !msg.contains("  "),
        "refusal message carries a run of absorbed indentation: {msg:?}"
    );
}

/// On this route the FIT PREDICATE and the QUALITY VERIFIER agree, ACROSS THE
/// RANGE MEASURED HERE: two fixtures, scale 0.25 to 10.00 in 0.25 steps.
///
/// Every sheet the route AGREES to draw is clean (no Error-severity issue of
/// any kind), and every scale it REFUSES reports a strictly positive overrun.
/// Neither half may be vacuous, so the sweep also asserts it saw both outcomes
/// per fixture -- a predicate that refused everything, or one that accepted
/// everything, would otherwise pass.
///
/// # The claim is scoped, and the scope is not decorative
///
/// This is NOT "the two never disagree at any scale". Below 0.05 they DO
/// disagree, and in the verifier's direction: a bored plate at 0.001..=0.01 is
/// drawn (correctly -- it fits with room to spare) while `verify_drawing`
/// reports `RedundantDimension`. Measured, and diagnosed rather than assumed:
/// the redundancy check quantizes SHEET-space positions, so at a small enough
/// scale two genuinely distinct dimensions land on the same quantized interval
/// and read as one repeated callout. That is a defect in the VERIFIER, not in
/// the placement this test guards, and it is out of scope for Task 40 -- but
/// leaving the doc claiming a universal it does not hold would be the more
/// expensive kind of wrong. The sweep therefore starts at 0.25.
///
/// # Why two fixtures
///
/// Each is swept PAST its own refusal boundary, and one was not enough. The
/// first cut of the fit predicate compared the group against the raw drawing
/// area, and the bored plate (which refuses from 3:1, well below the
/// interesting region) swept clean -- while a 10 mm CUBE at 8.75:1, whose
/// square silhouette pushes the bottom row down to the area's floor, was DRAWN
/// with its FRONT/RIGHT dimension band lying on the title block
/// (`ViewOverlapsTitleBlock`). `BOTTOM_DIM_SHORTFALL` closed it; this sweep is
/// what found it and what keeps it closed.
#[test]
fn the_fit_refusal_and_the_quality_verifier_never_disagree() {
    let (plate_model, plate) = bored_plate();
    let mut cube_model = BRepModel::new();
    cube_model.set_event_key(Some("explicit-scale-cube".to_string()));
    let cube = match TopologyBuilder::new(&mut cube_model).create_box_3d(10.0, 10.0, 10.0) {
        Ok(GeometryId::Solid(s)) => s,
        o => panic!("expected solid, got {o:?}"),
    };
    cube_model.set_event_key(None);

    for (tag, model, sid) in [
        ("bored plate 40x40x20", &plate_model, plate),
        ("cube 10x10x10", &cube_model, cube),
    ] {
        let mut drawn = 0usize;
        let mut refused = 0usize;
        for step in 1..=40u32 {
            let s = f64::from(step) * 0.25;
            match standard_drawing_hlr(model, sid, uuid::Uuid::nil(), SheetSize::A3, s) {
                Ok(d) => {
                    drawn += 1;
                    let errs = errors(&d);
                    assert!(
                        errs.is_empty(),
                        "{tag} scale {s}: the route agreed to draw this sheet, so it must be clean; got {:?}",
                        errs.iter().map(|e| e.kind).collect::<Vec<_>>()
                    );
                    assert!(
                        !verify_drawing(&d).has(DrawingIssueKind::ViewOutsideFrame),
                        "{tag} scale {s}: a drawn sheet must never carry a view off the frame"
                    );
                }
                Err(ProjectionError::ScaleDoesNotFitSheet {
                    overflow_x_mm,
                    overflow_y_mm,
                    ..
                }) => {
                    refused += 1;
                    assert!(
                        overflow_x_mm > 0.0 || overflow_y_mm > 0.0,
                        "{tag} scale {s}: a refusal must report a real overrun, not zeros"
                    );
                }
                Err(other) => panic!("{tag} scale {s}: unexpected error {other:?}"),
            }
        }
        assert!(
            drawn > 0 && refused > 0,
            "{tag}: the sweep must exercise both outcomes; drawn={drawn} refused={refused}"
        );
    }
}

/// The AUTOMATIC route must be untouched by this change.
///
/// Its sheet size, its chosen scale and every view position are pinned as
/// EXACT f64 bit patterns captured from the tree immediately before the fix
/// (two identical runs), for both branches of the automatic layout:
///
/// * a 40 mm bored plate -- A4, 1.0, the A4 ReplaceIso arrangement where
///   SECTION A-A displaces the isometric cell and the pictorial is re-attached;
/// * a 150x90x60 block -- A3, 0.75, the plain four-view arrangement.
///
/// Positions, not rendered SVG: `render_drawing_svg` stamps
/// `data-view-id="{uuid}"` from `ProjectedViewId::new()`, so the bytes differ
/// between two runs of an UNCHANGED tree (measured -- same length, different
/// hash). An SVG-hash fixture here would be a coin toss dressed as a guarantee.
#[test]
fn the_automatic_route_is_bit_identical() {
    fn pinned(d: &Drawing, expect_sheet: SheetSize, expect: &[(&str, f64, u64, u64)]) {
        assert_eq!(d.sheet_size, expect_sheet, "sheet size");
        assert_eq!(d.views.len(), expect.len(), "view count");
        for (v, (name, scale, x_bits, y_bits)) in d.views.iter().zip(expect) {
            assert_eq!(&v.name, name, "view order");
            assert_eq!(v.scale, *scale, "view '{name}' scale");
            assert_eq!(
                v.position_mm[0].to_bits(),
                *x_bits,
                "view '{name}' x moved: {} vs pinned {}",
                v.position_mm[0],
                f64::from_bits(*x_bits)
            );
            assert_eq!(
                v.position_mm[1].to_bits(),
                *y_bits,
                "view '{name}' y moved: {} vs pinned {}",
                v.position_mm[1],
                f64::from_bits(*y_bits)
            );
        }
    }

    let (m, part) = bored_plate();
    let auto = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("auto sheet");
    pinned(
        &auto,
        SheetSize::A4,
        &[
            ("FRONT", 1.0, 4638637247447433216, 4636666922610458624),
            ("TOP", 1.0, 4638637247447433216, 4639868700470542336),
            ("RIGHT", 1.0, 4641135337865740288, 4636666922610458624),
            ("SECTION A-A", 1.0, 4641135337865740288, 4639516856749654016),
            ("ISOMETRIC", 1.0, 4643080480235970904, 4640062394047828689),
        ],
    );

    let mut m2 = BRepModel::new();
    m2.set_event_key(Some("explicit-scale-big".to_string()));
    let big = match TopologyBuilder::new(&mut m2).create_box_3d(150.0, 90.0, 60.0) {
        Ok(GeometryId::Solid(s)) => s,
        o => panic!("expected solid, got {o:?}"),
    };
    m2.set_event_key(None);
    let auto2 = standard_drawing_auto(&m2, big, uuid::Uuid::nil()).expect("auto sheet (A3)");
    pinned(
        &auto2,
        SheetSize::A3,
        &[
            ("FRONT", 0.75, 4639388799346361594, 4637468174964069548),
            ("TOP", 0.75, 4639388799346361594, 4641944578423783424),
            ("RIGHT", 0.75, 4643936893493313536, 4637468174964069548),
            ("ISOMETRIC", 0.75, 4643936893493313536, 4641944578423783424),
        ],
    );
}

/// A `scale` that is not a RATIO is refused by name, before anything is drawn.
///
/// Measured on the unguarded route, 40 mm plate on A3 -- what each value used
/// to produce, all returned as `Ok`:
///
/// ```text
/// NaN   3 views, every position [NaN, NaN], verify_drawing errors = []
/// -inf  3 views, every position [NaN, NaN], verify_drawing errors = []
/// 0.0   3 views collapsed to a point,       verify_drawing errors = []
/// -4.0  3 views MIRRORED and laid right-to-left, errors = [ViewOverlapsTitleBlock, ViewOverlap x3]
/// inf   refused, but as a FIT failure reporting "inf mm" of overflow
/// ```
///
/// The first three are why this is a guard and not a nicety: `verify_drawing`
/// certified all three CLEAN. Every comparison against NaN is false, so no NaN
/// rect is ever "outside" a frame; a zero-area footprint is inside every frame.
/// Nothing downstream could have caught them -- the quality gate, the sheet
/// certificate and the export gate all read a sheet that renders nothing and
/// find nothing wrong with it.
///
/// `inf` is included even though it was already refused: it was refused as
/// `ScaleDoesNotFitSheet` with an "inf mm" overflow, which is a measurement
/// nobody made. It belongs in the same class as NaN, not in the class of
/// "3:1 overruns A3 by 9 mm".
#[test]
fn a_scale_that_is_not_a_ratio_is_refused_before_anything_is_drawn() {
    let (m, part) = bored_plate();
    for s in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        0.0_f64,
        -0.0_f64,
        -4.0,
        -0.25,
        // Finite and positive, and STILL not usable: 1e308 * 80 is `inf`, so
        // the arrangement has no measurable size. Refused here rather than as
        // a fit failure reporting "inf mm" (which serialises to JSON `null`).
        1e308,
    ] {
        let err = standard_drawing_hlr(&m, part, uuid::Uuid::nil(), SheetSize::A3, s)
            .expect_err("a scale that is not a finite positive ratio must be refused");
        match &err {
            ProjectionError::InvalidScale { scale, reason } => {
                assert!(
                    !reason.is_empty(),
                    "scale {s}: the refusal must say WHICH way the scale is unusable"
                );
                // NaN != NaN, so compare the CLASSIFICATION, not the value.
                assert_eq!(
                    scale.is_nan(),
                    s.is_nan(),
                    "the refusal must carry back the scale it was given, not a substitute"
                );
                if !s.is_nan() {
                    assert_eq!(*scale, s, "the refusal names the scale that was asked for");
                }
            }
            other => panic!(
                "scale {s}: expected InvalidScale (a non-ratio has no overflow to measure), got {other:?}"
            ),
        }
        let msg = err.to_string();
        assert!(
            !msg.contains("  "),
            "scale {s}: refusal carries absorbed indentation: {msg:?}"
        );
        assert!(
            !msg.contains("overrun") && !msg.contains("mm"),
            "scale {s}: a non-ratio must NOT be reported as a fit failure with a fabricated overflow: {msg:?}"
        );
    }
}

/// A solid the model does not contain is named as MISSING, not turned into a
/// sheet of degenerate extents.
///
/// This is a plain regression on the projection error path, and nothing more.
/// An earlier revision of this doc claimed it exercised `overrun`'s
/// NaN-handling "through the public route with a finite scale" — it does not:
/// `project_solid_view` fails on the unknown id in pass 1 and `?` propagates
/// before `place_four_view` is ever called, so no extent and no overrun are
/// computed. `overrun` and `fits()` are unreachable from any integration test
/// (the scale guard sits in front of them), and are pinned directly in
/// `dimensioning.rs`'s `mod tests` instead:
/// `overrun_does_not_launder_a_nan_into_a_fit` and
/// `a_placement_with_a_non_finite_overrun_neither_fits_nor_measures`.
#[test]
fn a_solid_that_projects_to_nothing_does_not_read_as_a_fitting_sheet() {
    // An empty model: the solid id resolves to nothing, so the route must fail
    // at projection rather than produce a sheet of any kind.
    let m = BRepModel::new();
    let err = standard_drawing_hlr(
        &m,
        geometry_engine::primitives::solid::INVALID_SOLID_ID,
        uuid::Uuid::nil(),
        SheetSize::A3,
        1.0,
    )
    .expect_err("a solid the model does not contain cannot yield a sheet");
    assert!(
        matches!(err, ProjectionError::SolidNotFound(_)),
        "a missing solid must be named as missing, not laundered into a fit verdict; got {err:?}"
    );
}
