// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Task #33: `section_view` mis-reconstructs RE-ENTRANT section boundaries on a
//! cross-drilled manifold (two orthogonal Ø16 through-bores crossing inside a
//! 60×40×40 block). The solid itself is SOUND (χ=-4, Steinmetz volume to 6 sig
//! figs) — the failure is section-side loop assembly.
//!
//! Live evidence (2026-07-18):
//!   * plane ⊥ one bore, away from the crossing: EXACT (0.04%);
//!   * plane parallel to both axes, offset through the crossing: void
//!     under-subtracted + spurious diagonal chords across the void;
//!   * plane exactly THROUGH both bore axes: full uncut rectangle reported
//!     (as if no material removed) + phantom diagonal edge;
//!   * rectangle-in-rectangle annulus (box − box): perfect.
//!
//! Fixture geometry (all analytic):
//!   block:  x∈[-30,30], y∈[-20,20], z∈[-20,20]
//!   bore A: axis +Z through (0,0), r=8, full through
//!   bore B: axis +X through (0,0), r=8, full through
//!
//! Section normal +Y at height y=c (|c|<8) cuts each bore wall in two straight
//! generator lines at distance w = sqrt(64 − c²) from the bore's axis plane, so
//! the section is the outer 60×40 rectangle minus a full-span cruciform:
//!   void  = 2w·40 (bore A strip) + 2w·60 (bore B strip) − 4w² (overlap square)
//!   area  = 2400 − 200w + 4w²    (4 disjoint corner rectangles)
//! At c=0 (plane contains BOTH axes, the degenerate tangency case): w=8,
//! area = 2400 − 1600 + 256 = 1056. At c=5: w=√39, area = 2556 − 200√39 ≈ 1307.0.

use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::section::{section_solid_by_plane, SectionCap};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

/// 60×40×40 block, two orthogonal Ø16 through-bores crossing at the origin.
fn cross_drilled_block() -> (BRepModel, SolidId) {
    let mut model = BRepModel::new();
    let block = sid(TopologyBuilder::new(&mut model)
        .create_box_3d(60.0, 40.0, 40.0)
        .expect("block"));
    let bore_z = sid(TopologyBuilder::new(&mut model)
        .create_cylinder_3d(
            Point3::new(0.0, 0.0, -40.0),
            Vector3::new(0.0, 0.0, 1.0),
            8.0,
            80.0,
        )
        .expect("bore z"));
    let after_z = boolean_operation(
        &mut model,
        block,
        bore_z,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("block − bore Z");
    let bore_x = sid(TopologyBuilder::new(&mut model)
        .create_cylinder_3d(
            Point3::new(-40.0, 0.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
            8.0,
            80.0,
        )
        .expect("bore x"));
    let s = boolean_operation(
        &mut model,
        after_z,
        bore_x,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("− bore X");
    (model, s)
}

/// Total area of all caps (sum of triangle areas).
fn caps_area(caps: &[SectionCap]) -> f64 {
    let mut area = 0.0;
    for cap in caps {
        for tri in &cap.indices {
            let a = cap.vertices[tri[0] as usize];
            let b = cap.vertices[tri[1] as usize];
            let c = cap.vertices[tri[2] as usize];
            area += (b - a).cross(&(c - a)).magnitude() * 0.5;
        }
    }
    area
}

/// Cruciform analytic area at plane height c (normal +Y, |c| < r): 4 corner
/// rectangles of the 60×40 outer minus the full-span cross of half-width
/// w = sqrt(r² − c²).
fn cruciform_area(c: f64) -> f64 {
    let w = (64.0 - c * c).sqrt();
    2400.0 - 200.0 * w + 4.0 * w * w
}

/// RED (a): section plane EXACTLY THROUGH BOTH bore axes (y=0). The plane
/// contains each cylinder's axis — the tangency-degenerate case where each
/// wall is cut in two straight generator LINES. Live failure: reported the
/// FULL uncut 2400 mm² rectangle plus a phantom diagonal edge.
#[test]
fn section_through_both_bore_axes_subtracts_cruciform() {
    let (model, s) = cross_drilled_block();
    let caps = section_solid_by_plane(
        &model,
        s,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
        Tolerance::default(),
    )
    .expect("axis-plane section must not error");
    assert!(!caps.is_empty(), "axis-plane section returned EMPTY caps");
    let area = caps_area(&caps);
    let expected = cruciform_area(0.0); // 1056: four 22×12 corner rectangles
    let rel = (area - expected).abs() / expected;
    assert!(
        rel < 0.01,
        "axis-plane cruciform area {area:.2} vs analytic {expected:.2} \
         (rel {rel:.4}); caps={} — full-rect 2400 means the void was not \
         subtracted at all",
        caps.len()
    );
    assert_eq!(
        caps.len(),
        4,
        "axis-plane section must be 4 disjoint corner pieces, got {}",
        caps.len()
    );
}

/// RED (b): offset plane (y=5) through the crossing region — parallel to both
/// axes, inside both bores. Live failure: void under-subtracted, spurious
/// diagonal chords across the void.
#[test]
fn section_offset_plane_through_crossing_subtracts_cruciform() {
    let (model, s) = cross_drilled_block();
    let caps = section_solid_by_plane(
        &model,
        s,
        Point3::new(0.0, 5.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
        Tolerance::default(),
    )
    .expect("offset-plane section must not error");
    assert!(!caps.is_empty(), "offset-plane section returned EMPTY caps");
    let area = caps_area(&caps);
    let expected = cruciform_area(5.0); // ≈ 1307.0
    let rel = (area - expected).abs() / expected;
    assert!(
        rel < 0.01,
        "offset-plane cruciform area {area:.2} vs analytic {expected:.2} \
         (rel {rel:.4}); caps={} — over-area means the void was \
         under-subtracted (diagonal-chord mis-join)",
        caps.len()
    );
    assert_eq!(
        caps.len(),
        4,
        "offset-plane section must be 4 disjoint corner pieces, got {}",
        caps.len()
    );
}

/// Regression pin (c): plane ⊥ bore X, away from the crossing (x=20 > r) —
/// the outer 40×40 rectangle minus bore X's full circle. EXACT live (0.04%);
/// must stay exact.
#[test]
fn section_single_bore_circle_stays_exact() {
    let (model, s) = cross_drilled_block();
    let caps = section_solid_by_plane(
        &model,
        s,
        Point3::new(20.0, 0.0, 0.0),
        Vector3::new(1.0, 0.0, 0.0),
        Tolerance::default(),
    )
    .expect("single-bore section must not error");
    assert_eq!(
        caps.len(),
        1,
        "single-bore cut must be one cap with one hole, got {}",
        caps.len()
    );
    let area = caps_area(&caps);
    let expected = 40.0 * 40.0 - std::f64::consts::PI * 64.0; // 1398.94
    let rel = (area - expected).abs() / expected;
    assert!(
        rel < 0.005,
        "single-bore area {area:.2} vs analytic {expected:.2} (rel {rel:.4})"
    );
}

/// Regression pin (d): rectangle-in-rectangle annulus (box − through-box) —
/// the case the live cross-check showed reconstructing perfectly.
#[test]
fn section_box_in_box_annulus_stays_exact() {
    let mut model = BRepModel::new();
    let outer = sid(TopologyBuilder::new(&mut model)
        .create_box_3d(40.0, 40.0, 20.0)
        .expect("outer box"));
    let inner = sid(TopologyBuilder::new(&mut model)
        .create_box_3d(20.0, 20.0, 40.0)
        .expect("inner box"));
    let s = boolean_operation(
        &mut model,
        outer,
        inner,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("box − box");
    let caps = section_solid_by_plane(
        &model,
        s,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
        Tolerance::default(),
    )
    .expect("annulus section must not error");
    assert_eq!(caps.len(), 1, "annulus must be one cap, got {}", caps.len());
    let area = caps_area(&caps);
    let expected = 40.0 * 40.0 - 20.0 * 20.0; // 1200
    let rel = (area - expected).abs() / expected;
    assert!(
        rel < 0.005,
        "annulus area {area:.2} vs analytic {expected:.2} (rel {rel:.4})"
    );
}
