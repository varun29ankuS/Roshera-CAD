// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A REVOLVED part reaches the drawing pipeline with its bore intact.
//!
//! ## Why this fixture exists
//!
//! Every fixture in `drawing_visual_harness.rs` is BOOLEAN-built (a disc
//! cylinder minus a bore cylinder minus bolt-hole cylinders), and so is every
//! drawing fixture in this crate. A part built by REVOLVING an `[r, z]`
//! meridian -- which is how the demo catalog builds every flange, and how
//! `roshera-eval`'s `buildHubFlange` builds its part -- had never been drawn
//! by any test in this repo.
//!
//! That gap hid a live defect. `roshera-eval` scenario 15 builds the flange
//! WITH six boolean-drilled bolt holes and gets a section view; scenario 19
//! builds the same revolved flange with NO drilled holes and got four views
//! instead of five -- no SECTION at all.
//!
//! The cause was NOT the revolve, and not the boolean. Both parts carry the
//! same Ø12 bore, and in BOTH the bore's own `x_mm`/`y_mm` are the fabricated
//! `0.0` that pairs with an "--" label (measured; see
//! `dimensioning::tests::section_plane_lands_on_the_measured_bore_axis_not_the_datum_corner`).
//! `choose_section_plane` added those zeros to the datum corner, so the cut
//! plane landed at [-30, -30, 0] and missed the solid. The drilled part
//! escaped only because its six bolt-hole offsets average onto the axis --
//! an accident of symmetry. A plain bored flange had no such rescue.
//!
//! The meridian below is `buildHubFlange`'s verbatim: a Ø60 x 6 flange plate
//! with a Ø24 x 14 hub on top, bored Ø12 all the way through. The bore is the
//! most important dimension on the part, and the reason a section exists.

use geometry_engine::drawing::dimensioning::standard_drawing_auto;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::revolve::{revolve_meridian, RevolveOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::readable::bore_face_ids;

/// `roshera-eval/lib/builders.mjs::buildHubFlange`'s profile, verbatim.
/// `revolve_meridian` closes the loop itself, so the final `(6, 20) -> (6, 0)`
/// segment -- the Ø12 bore wall, 20 mm long -- is implicit.
const HUB_FLANGE_MERIDIAN: [(f64, f64); 6] = [
    (6.0, 0.0),
    (30.0, 0.0),
    (30.0, 6.0),
    (12.0, 6.0),
    (12.0, 20.0),
    (6.0, 20.0),
];

fn revolved_hub_flange() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let opts = RevolveOptions {
        axis_origin: Point3::new(0.0, 0.0, 0.0),
        axis_direction: Vector3::new(0.0, 0.0, 1.0),
        angle: std::f64::consts::TAU,
        segments: 96,
        ..Default::default()
    };
    let solid = revolve_meridian(&mut m, &HUB_FLANGE_MERIDIAN, opts).expect("revolve hub flange");
    (m, solid)
}

/// The bore wall is a CAVITY, and the kernel must classify it as one.
///
/// `bore_face_ids` is the material-side discriminator the hole table depends
/// on: a cylindrical face whose material-out normal points TOWARD the axis.
/// The Ø12 wall qualifies on any honest reading -- material surrounds it on
/// every side. If this is empty, the part has no recognised internal feature
/// and everything downstream (hole table, bore diameter callout, section
/// view) is silently absent rather than wrong, which is the worse failure.
#[test]
fn revolved_bore_is_classified_as_a_cavity() {
    let (m, solid) = revolved_hub_flange();
    let bores = bore_face_ids(&m, solid);
    assert!(
        !bores.is_empty(),
        "a revolved Ø12 through-bore is a cavity and must be classified as one; \
         bore_face_ids returned NOTHING, so the hole table, the bore's diameter \
         callout and the section view are all suppressed on every revolved part"
    );
}

/// The bore reaches the hole table.
///
/// Classification alone is not enough -- `attach_hole_table_from_dims` also
/// needs a diameter record keyed to that face. This asserts the property the
/// sheet actually consumes.
#[test]
fn revolved_bore_reaches_the_hole_table() {
    let (m, solid) = revolved_hub_flange();
    let drawing = standard_drawing_auto(&m, solid, uuid::Uuid::nil()).expect("sheet");
    assert!(
        !drawing.hole_sites.is_empty(),
        "the Ø12 bore must appear as a hole site on a revolved flange's sheet; \
         hole_sites is empty, which is what suppresses SECTION A-A"
    );
}

/// The SECTION OP itself cuts a revolved solid.
///
/// This is the isolated link the live bisect indicted. Against the running
/// backend (debug build at main @ 2b9eef47) this part's sheet reports
/// `hole_sites: 1` -- the Ø12 bore, tagged A1, THRU, correctly classified --
/// and yet `section: null` with four views. So classification and the hole
/// table are both fine; `attach_section_view` clears its gate and then gets
/// `None` back from `section_view`, whose only two `None` paths are "the
/// section op returned no caps" and "the caps carried no triangles".
///
/// `attach_section_view` attributes that `None` to "plane missed the solid".
/// A plane through the axis of a solid of revolution cannot miss it, so if
/// this assertion fails, that comment is a FALSE STATED REASON as well as a
/// missing feature -- the sheet silently drops a view and misreports why.
#[test]
fn the_section_op_cuts_a_revolved_solid() {
    use geometry_engine::math::Tolerance;
    use geometry_engine::operations::section::section_solid_by_plane;

    let (m, solid) = revolved_hub_flange();
    // The ZX plane through the axis: normal +Y, origin on the axis. This cuts
    // the full meridian -- flange plate, hub and bore wall.
    let caps = section_solid_by_plane(
        &m,
        solid,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
        Tolerance::default(),
    );
    let caps = match caps {
        Ok(c) => c,
        Err(e) => panic!(
            "sectioning a revolved solid on a plane through its own axis must not \
             error -- the section op refused with: {e:?}"
        ),
    };
    assert!(
        !caps.is_empty(),
        "a plane through a revolved flange's axis cuts its plate, hub and bore \
         wall, so the section op must return cut caps; it returned NONE, which is \
         what leaves SECTION A-A off every revolved part's sheet"
    );
}

/// The drawing-level section builder works on a revolved solid -- given the
/// RIGHT plane. This isolates the failure to the plane CHOICE, not the
/// section machinery.
///
/// Measured (2026-08-17), all four combinations of the two candidate normals
/// with an on-axis vs. corner origin:
///
/// ```text
///   X normal @ origin on axis      -> SOME (18 polylines, extent 60.0x20.0)
///   X normal @ origin at -30 corner -> NONE
///   Y normal @ origin on axis      -> SOME (24 polylines, extent 60.0x20.0)
///   Y normal @ origin at -30 corner -> NONE
/// ```
///
/// So HLR, capping, hatching and outlining all handle revolve-built analytic
/// bands correctly. The only thing that decides whether a revolved flange
/// gets SECTION A-A is where `choose_section_plane` puts the origin.
#[test]
fn the_section_builder_accepts_an_on_axis_plane_and_only_misses_off_axis() {
    use geometry_engine::drawing::section_view::section_view;

    let (m, solid) = revolved_hub_flange();
    let cut = |o: Point3, n: Vector3| {
        section_view(
            &m,
            solid,
            uuid::Uuid::nil(),
            o,
            n,
            "SECTION A-A",
            [0.0, 0.0],
            1.0,
        )
    };

    for (label, n) in [
        ("X", Vector3::new(1.0, 0.0, 0.0)),
        ("Y", Vector3::new(0.0, 1.0, 0.0)),
    ] {
        let on_axis = cut(Point3::new(0.0, 0.0, 0.0), n);
        assert!(
            on_axis.is_some_and(|v| !v.polylines.is_empty()),
            "{label}-normal plane through the axis must section the revolved flange"
        );
        // The part spans x,y in [-30, 30]; a plane at the -30 corner is
        // tangent at best. This is the origin `choose_section_plane` actually
        // hands over today, and it is why the sheet has four views.
        assert!(
            cut(Point3::new(-30.0, -30.0, 0.0), n).is_none(),
            "{label}-normal plane at the part's corner cannot section it -- \
             this pins WHY a wrong origin silently drops the view"
        );
    }
}

/// The sheet carries a SECTION view.
///
/// This is the end-to-end property `roshera-eval` scenario 19 scores and
/// currently fails: four views where five belong. A flange whose bore is its
/// defining feature is not a complete drawing without a section through it.
#[test]
fn revolved_flange_sheet_carries_a_section_view() {
    let (m, solid) = revolved_hub_flange();
    let drawing = standard_drawing_auto(&m, solid, uuid::Uuid::nil()).expect("sheet");
    let names: Vec<&str> = drawing.views.iter().map(|v| v.name.as_str()).collect();
    assert!(
        names.iter().any(|n| n.to_uppercase().contains("SECTION")),
        "a revolved flange with a through-bore must be sectioned; views were {names:?}"
    );
}
