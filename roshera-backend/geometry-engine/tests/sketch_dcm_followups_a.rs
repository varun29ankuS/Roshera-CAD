// Reason: integration-test crate -- panicking (unwrap/expect/assert/index) is
// the test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::indexing_slicing)]

//! SKETCH-DCM #45 — Wave A follow-ups (Slice 6/7 residual burndown).
//!
//! Each section pins one follow-up item from the Slice 6/7 reports:
//!
//! 1. `SketchLoop::is_ccw` — the legacy INVERTED sign convention is
//!    fixed at the root: `is_ccw == true` now means geometric
//!    counter-clockwise winding of the walk, exact (predicate-based),
//!    with arc/spline interior witnesses so all-curved loops (whose
//!    chord polygons collapse) classify correctly.
//! 2. Arc extend — arcs grow their sweep to a forward intersection
//!    with a boundary, same `PointOnCurve` contact contract as lines.
//! 3. Legacy-arc mirror — center-angle arcs mirror about a
//!    construction axis with maintained `Symmetric` (4-row arc pair
//!    arm) + `Equal` constraints.
//! 4. Line / arc / spline pattern sources — the Slice-6 Equal-chain +
//!    provenance scheme extended to entity webs (one point-web per
//!    endpoint / control point).
//! 5. All-arc offset loops — per-junction concentric minting closes
//!    the Slice-6 residual-2 freedom (lens gate: FullyConstrained).
//! 6. Offset global self-intersection — distant colliding features
//!    refuse typed (`SelfIntersecting`), never a silent bad loop.
//! 7. Trim constraint re-application — carrier-invariant constraints
//!    survive onto the trimmed survivors; extent-bound constraints
//!    are genuinely dropped and reported.
//! 8. curve_pattern arc rails — maintained arc-length-true spacing
//!    via the `ArcLength [rail, prev, next]` residual.

use geometry_engine::sketch2d::sketch_topology::SketchTopology;
use geometry_engine::sketch2d::{Point2d, Sketch, SketchAnchor, Tolerance2d};

fn fresh(name: &str) -> Sketch {
    Sketch::new(name.to_string(), SketchAnchor::xy())
}

fn analyze(sketch: &Sketch) -> SketchTopology {
    SketchTopology::analyze(sketch, &Tolerance2d::default()).expect("topology analysis")
}

// ── 1. SketchLoop::is_ccw — corrected geometric winding ─────────────

#[test]
fn red_loop_is_ccw_is_true_for_a_ccw_drawn_rectangle() {
    // Four head-to-tail lines drawn counter-clockwise. The walk seeds
    // from the first edge's stored direction, so the loop is traversed
    // CCW — `is_ccw` must say TRUE. The legacy convention mapped
    // `Orientation::Clockwise => true` (preserving an old trapezoid
    // `area > 0.0` decision) and reported exactly the opposite.
    let s = fresh("followups_is_ccw_ccw_rect");
    let p = [
        Point2d::new(0.0, 0.0),
        Point2d::new(10.0, 0.0),
        Point2d::new(10.0, 10.0),
        Point2d::new(0.0, 10.0),
    ];
    let ids: Vec<_> = p.iter().map(|q| s.add_point(*q)).collect();
    for i in 0..4 {
        s.add_line(ids[i], ids[(i + 1) % 4]).expect("outline line");
    }
    let topo = analyze(&s);
    assert_eq!(topo.loops().len(), 1);
    assert!(
        topo.loops()[0].is_ccw,
        "a CCW-drawn rectangle walk must report is_ccw == true"
    );
}

#[test]
fn red_loop_is_ccw_is_false_for_a_cw_drawn_rectangle() {
    let s = fresh("followups_is_ccw_cw_rect");
    // Same rectangle, drawn clockwise (up the left side first).
    let p = [
        Point2d::new(0.0, 0.0),
        Point2d::new(0.0, 10.0),
        Point2d::new(10.0, 10.0),
        Point2d::new(10.0, 0.0),
    ];
    let ids: Vec<_> = p.iter().map(|q| s.add_point(*q)).collect();
    for i in 0..4 {
        s.add_line(ids[i], ids[(i + 1) % 4]).expect("outline line");
    }
    let topo = analyze(&s);
    assert_eq!(topo.loops().len(), 1);
    assert!(
        !topo.loops()[0].is_ccw,
        "a CW-drawn rectangle walk must report is_ccw == false"
    );
}

#[test]
fn red_loop_is_ccw_classifies_an_all_arc_lens_by_its_interior_witnesses() {
    // Two shared-endpoint arcs forming a lens. Every chord-based
    // winding measure degenerates here (the two chords cancel exactly)
    // — the corrected classifier carries an interior witness point per
    // curved edge, so the CCW walk is detected geometrically.
    let s = fresh("followups_is_ccw_lens");
    let a = s.add_point(Point2d::new(-6.0, 0.0));
    let b = s.add_point(Point2d::new(6.0, 0.0));
    // Bottom bulge (A -> B through (0, -2)), then top bulge
    // (B -> A through (0, 2)): a CCW walk.
    s.add_arc(a, b, 10.0, true, false).expect("bottom arc");
    s.add_arc(b, a, 10.0, true, false).expect("top arc");
    let topo = analyze(&s);
    assert_eq!(topo.loops().len(), 1, "lens is one closed loop");
    assert!(
        topo.loops()[0].is_ccw,
        "the CCW lens walk must report is_ccw == true (chord shoelace is \
         degenerate here — interior witnesses required)"
    );
}

// Not a RED (the legacy convention happened to say `true` here too via
// its positive-area fallback) — this PINS that the corrected classifier
// keeps the single-edge convention rather than inheriting the trapezoid
// fallback's inverted sign.
#[test]
fn lone_circle_loop_is_ccw_by_kernel_convention() {
    // A full circle is a single-edge closed loop with no walk
    // direction of its own; the kernel parameterises circles CCW, so
    // the loop reports the convention (documented on `find_loops`).
    let s = fresh("followups_is_ccw_circle");
    s.add_circle(Point2d::new(3.0, 4.0), 5.0).expect("circle");
    let topo = analyze(&s);
    assert_eq!(topo.loops().len(), 1);
    assert!(
        topo.loops()[0].is_ccw,
        "single-edge closed loops are CCW by kernel parameterisation"
    );
}

#[test]
fn red_loop_is_ccw_witnesses_defeat_a_misleading_chord_polygon() {
    // A deep arc blob with an interior notch vertex: the walk is
    // geometrically CCW (interior stays left along the arc's bottom),
    // but the chord polygon [A, B, C] winds strictly CLOCKWISE
    // (C sits below the chord AB). Any vertex-only winding measure
    // gives the WRONG nonzero answer here — deterministically, no
    // f64 noise involved — so this pins that the classifier threads
    // the curved edges' interior witnesses into the polygon.
    let s = fresh("followups_is_ccw_blob");
    let a = s.add_point(Point2d::new(0.0, 0.0));
    let b = s.add_point(Point2d::new(2.0, 0.0));
    let c = s.add_point(Point2d::new(1.0, -0.5));
    // Large arc A -> B around the far (bottom) side, bulging to
    // y ~= -11.9 (center (1, -sqrt(35)), radius 6).
    s.add_arc(a, b, 6.0, true, true).expect("blob arc");
    s.add_line(b, c).expect("notch line 1");
    s.add_line(c, a).expect("notch line 2");
    let topo = analyze(&s);
    assert_eq!(topo.loops().len(), 1, "blob is one closed loop");
    assert!(
        topo.loops()[0].is_ccw,
        "the CCW blob walk must report is_ccw == true even though its          chord polygon winds CW"
    );
}
