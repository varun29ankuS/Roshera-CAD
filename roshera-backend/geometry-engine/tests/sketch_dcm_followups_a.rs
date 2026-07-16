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

// ── 2. Arc extend (Slice-6 residual 4) ──────────────────────────────

use geometry_engine::sketch2d::sketch_ops::{self, LineEnd, SketchOpError, SketchOpKind};
use geometry_engine::sketch2d::{
    Constraint, ConstraintPriority, ConstraintType, DimensionalConstraint, EntityRef,
    GeometricConstraint,
};

fn count_constraints_of(sketch: &Sketch, pred: impl Fn(&ConstraintType) -> bool) -> usize {
    sketch
        .all_constraints()
        .iter()
        .filter(|c| pred(&c.constraint_type))
        .count()
}

fn required_dim(dc: DimensionalConstraint, entities: Vec<EntityRef>) -> Constraint {
    Constraint::new_dimensional(dc, entities, ConstraintPriority::Required)
}

#[test]
fn red_extend_arc_end_grows_the_sweep_to_a_forward_boundary() {
    // Quarter arc on the R10 carrier about the origin (0..90 deg,
    // ccw). Extending its END must grow the sweep along the carrier to
    // the nearest forward boundary intersection (the vertical chord at
    // x = -sqrt(50) crosses the carrier at 135 deg within its extent),
    // move the shared endpoint there, re-sync the stored arc, and mint
    // the same PointOnCurve contact the line extend mints.
    let s = fresh("followups_extend_arc_end");
    let e1 = s.add_point(Point2d::new(10.0, 0.0));
    let e2 = s.add_point(Point2d::new(0.0, 10.0));
    let arc = s.add_arc(e1, e2, 10.0, true, false).expect("target arc");
    let x = -(50.0f64).sqrt();
    let b1 = s.add_point(Point2d::new(x, 0.0));
    let b2 = s.add_point(Point2d::new(x, 20.0));
    let boundary = s.add_line(b1, b2).expect("boundary");

    let outcome = sketch_ops::extend(
        &s,
        &EntityRef::Arc(arc),
        LineEnd::End,
        &EntityRef::Line(boundary),
    )
    .expect("arc extend");

    assert_eq!(outcome.op, SketchOpKind::Extend);
    let moved = s.get_point(&e2).expect("moved endpoint");
    assert!(
        (moved.x - x).abs() < 1e-9 && (moved.y + x).abs() < 1e-9,
        "endpoint must land on the boundary at 135 deg: {moved:?}"
    );
    let stored = s.arcs().get(&arc).expect("arc").arc;
    assert!(
        (stored.radius - 10.0).abs() < 1e-9
            && stored.center.x.abs() < 1e-9
            && stored.center.y.abs() < 1e-9,
        "carrier preserved: {stored:?}"
    );
    assert!(
        (stored.sweep_angle() - 3.0 * std::f64::consts::FRAC_PI_4).abs() < 1e-9,
        "sweep must grow to 135 deg, got {}",
        stored.sweep_angle()
    );
    assert!(outcome.modified.contains(&EntityRef::Arc(arc)));
    assert!(outcome.modified.contains(&EntityRef::Point(e2)));
    assert_eq!(
        outcome.constraints_added.len(),
        1,
        "one minted PointOnCurve keeps the extended end on the boundary"
    );
    let poc = count_constraints_of(&s, |t| {
        matches!(
            t,
            ConstraintType::Geometric(GeometricConstraint::PointOnCurve)
        )
    });
    assert_eq!(poc, 1);
    let report = s.solve_constraints().expect("solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);
}

#[test]
fn extend_arc_refuses_when_no_forward_hit_fits_the_remaining_sweep() {
    // Boundary crosses the carrier only INSIDE the current span (at
    // 45 deg): growing the end would have to sweep past the fixed
    // start, degenerating the arc -- typed NoIntersection, sketch
    // untouched.
    let s = fresh("followups_extend_arc_refuse");
    let e1 = s.add_point(Point2d::new(10.0, 0.0));
    let e2 = s.add_point(Point2d::new(0.0, 10.0));
    let arc = s.add_arc(e1, e2, 10.0, true, false).expect("target arc");
    let x = (50.0f64).sqrt();
    let b1 = s.add_point(Point2d::new(x, 0.0));
    let b2 = s.add_point(Point2d::new(x, 20.0));
    let boundary = s.add_line(b1, b2).expect("boundary");
    let err = sketch_ops::extend(
        &s,
        &EntityRef::Arc(arc),
        LineEnd::End,
        &EntityRef::Line(boundary),
    )
    .expect_err("must refuse");
    assert!(matches!(err, SketchOpError::NoIntersection { .. }));
    let pos = s.get_point(&e2).expect("e2");
    assert_eq!((pos.x, pos.y), (0.0, 10.0), "no mutation on refusal");
    assert_eq!(s.all_constraints().len(), 0);
}

#[test]
fn extend_refuses_legacy_arcs_and_spline_shapes_per_shape() {
    let s = fresh("followups_extend_per_shape_refuse");
    // Legacy center-angle arc: no endpoint POINT exists for the
    // contact constraint to bind -- per-shape typed refusal.
    let legacy = s
        .add_arc_center_angles(Point2d::new(0.0, 0.0), 10.0, 0.0, 1.0)
        .expect("legacy arc");
    let b1 = s.add_point(Point2d::new(20.0, -5.0));
    let b2 = s.add_point(Point2d::new(20.0, 5.0));
    let boundary = s.add_line(b1, b2).expect("boundary");
    let err = sketch_ops::extend(
        &s,
        &EntityRef::Arc(legacy),
        LineEnd::End,
        &EntityRef::Line(boundary),
    )
    .expect_err("legacy arc must refuse");
    assert!(
        matches!(&err, SketchOpError::Unsupported { reason, .. } if reason.contains("legacy")),
        "typed per-shape refusal expected, got {err:?}"
    );

    // Spline TARGET: not extendable -- per-shape typed refusal.
    let spline = s
        .add_bspline(
            3,
            vec![
                Point2d::new(0.0, 0.0),
                Point2d::new(3.0, 5.0),
                Point2d::new(7.0, 5.0),
                Point2d::new(10.0, 0.0),
            ],
            vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        )
        .expect("spline");
    let err = sketch_ops::extend(
        &s,
        &EntityRef::Spline(spline),
        LineEnd::End,
        &EntityRef::Line(boundary),
    )
    .expect_err("spline target must refuse");
    assert!(matches!(err, SketchOpError::Unsupported { .. }));

    // Spline BOUNDARY: no analytic carrier intersection -- per-shape
    // typed refusal.
    let a = s.add_point(Point2d::new(0.0, 0.0));
    let b = s.add_point(Point2d::new(5.0, 0.0));
    let line = s.add_line(a, b).expect("line");
    let err = sketch_ops::extend(
        &s,
        &EntityRef::Line(line),
        LineEnd::End,
        &EntityRef::Spline(spline),
    )
    .expect_err("spline boundary must refuse");
    assert!(matches!(err, SketchOpError::Unsupported { .. }));
}

// ── 3. Legacy-arc + shared-CP-spline mirror (Slice-6 residual 5) ────

fn construction_axis_x0(s: &Sketch) -> geometry_engine::sketch2d::Line2dId {
    let a1 = s.add_point(Point2d::new(0.0, -10.0));
    let a2 = s.add_point(Point2d::new(0.0, 10.0));
    for id in [a1, a2] {
        s.points()
            .get_mut(&id)
            .expect("axis point")
            .value_mut()
            .fix();
    }
    let axis = s.add_line(a1, a2).expect("axis");
    s.set_construction(&EntityRef::Line(axis), true)
        .expect("construction");
    axis
}

#[test]
fn red_mirror_legacy_center_angle_arc_is_maintained() {
    // Slice-6 residual 5: legacy (center-angle) arcs refused mirror
    // because there are no endpoint points to bind Symmetric to. The
    // follow-up mints an ARC-PAIR Symmetric (4 rows: reflected center
    // + reflected traversal-normalized angles) + Equal radius -- all 5
    // of the legacy arc's parameters are maintained, exactly like the
    // line/circle paths.
    let s = fresh("followups_mirror_legacy_arc");
    let axis = construction_axis_x0(&s);
    let deg = std::f64::consts::PI / 180.0;
    let src = s
        .add_arc_center_angles(Point2d::new(5.0, 2.0), 2.5, 30.0 * deg, 120.0 * deg)
        .expect("legacy source arc");

    let outcome = sketch_ops::mirror(&s, &[EntityRef::Arc(src)], &axis).expect("legacy-arc mirror");

    let m_arc = outcome
        .created
        .iter()
        .find_map(|e| match e {
            EntityRef::Arc(id) => Some(*id),
            _ => None,
        })
        .expect("mirrored arc created");
    let src_geo = s.arcs().get(&src).expect("src").arc;
    let dst_geo = s.arcs().get(&m_arc).expect("dst").arc;
    assert!(
        (dst_geo.center.x + 5.0).abs() < 1e-9 && (dst_geo.center.y - 2.0).abs() < 1e-9,
        "mirrored center: {:?}",
        dst_geo.center
    );
    assert!((dst_geo.radius - 2.5).abs() < 1e-9);
    // The mirrored arc's midpoint is the exact reflection of the
    // source's (x negated about the x = 0 axis).
    let (sm, dm) = (src_geo.midpoint(), dst_geo.midpoint());
    assert!(
        (dm.x + sm.x).abs() < 1e-9 && (dm.y - sm.y).abs() < 1e-9,
        "arc midpoint must reflect: {sm:?} vs {dm:?}"
    );
    // Maintenance: Symmetric[arc, arc, axis] + Equal radius.
    let symmetric = count_constraints_of(&s, |t| {
        matches!(t, ConstraintType::Geometric(GeometricConstraint::Symmetric))
    });
    let equals = count_constraints_of(&s, |t| {
        matches!(t, ConstraintType::Geometric(GeometricConstraint::Equal))
    });
    assert_eq!(symmetric, 1, "one arc-pair Symmetric");
    assert_eq!(equals, 1, "one Equal radius");

    let report = s.solve_constraints().expect("solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);

    // MAINTAINED, not one-shot: pin the source radius, edit it, and
    // the mirrored arc must follow through the Equal chain while the
    // Symmetric rows keep the reflection exact.
    let pin = s.add_constraint(required_dim(
        DimensionalConstraint::Radius(2.5),
        vec![EntityRef::Arc(src)],
    ));
    s.update_dimensional_value(&pin, 3.0).expect("edit radius");
    let report = s.solve_constraints().expect("re-solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    let r_dst = s.arcs().get(&m_arc).expect("dst").arc.radius;
    assert!(
        (r_dst - 3.0).abs() < 1e-6,
        "mirror must track the source radius edit: {r_dst}"
    );
    let dst_geo = s.arcs().get(&m_arc).expect("dst").arc;
    let src_geo = s.arcs().get(&src).expect("src").arc;
    let (sm, dm) = (src_geo.midpoint(), dst_geo.midpoint());
    assert!(
        (dm.x + sm.x).abs() < 1e-6 && (dm.y - sm.y).abs() < 1e-6,
        "reflection must survive the re-solve: {sm:?} vs {dm:?}"
    );
}

#[test]
fn red_mirror_shared_cp_spline_is_maintained_per_control_point() {
    // Slice-7 entity model: shared-CP splines are point webs -- mirror
    // is NATURAL for them (one Symmetric per control point, identical
    // to the line path). Raw-CP splines refuse per-shape below.
    let s = fresh("followups_mirror_spline");
    let axis = construction_axis_x0(&s);
    let cps: Vec<_> = [
        Point2d::new(2.0, 0.0),
        Point2d::new(3.0, 4.0),
        Point2d::new(6.0, 4.0),
        Point2d::new(8.0, 1.0),
    ]
    .iter()
    .map(|p| s.add_point(*p))
    .collect();
    let spline = s
        .add_bspline_with_control_points(3, &cps)
        .expect("shared-CP spline");

    let outcome =
        sketch_ops::mirror(&s, &[EntityRef::Spline(spline)], &axis).expect("spline mirror");

    let m_spline = outcome
        .created
        .iter()
        .find_map(|e| match e {
            EntityRef::Spline(id) => Some(*id),
            _ => None,
        })
        .expect("mirrored spline created");
    let m_cps = s
        .spline_control_point_ids(&m_spline)
        .expect("mirrored spline is shared-CP");
    assert_eq!(m_cps.len(), 4);
    for (src_id, dst_id) in cps.iter().zip(&m_cps) {
        let (sp, dp) = (
            s.get_point(src_id).expect("src cp"),
            s.get_point(dst_id).expect("dst cp"),
        );
        assert!(
            (dp.x + sp.x).abs() < 1e-9 && (dp.y - sp.y).abs() < 1e-9,
            "control point must reflect exactly: {sp:?} vs {dp:?}"
        );
    }
    let symmetric = count_constraints_of(&s, |t| {
        matches!(t, ConstraintType::Geometric(GeometricConstraint::Symmetric))
    });
    assert_eq!(symmetric, 4, "one Symmetric per control point");
    let report = s.solve_constraints().expect("solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);

    // Raw-CP splines have no points to bind Symmetric to: per-shape
    // typed refusal, sketch untouched.
    let raw = s
        .add_bspline(
            3,
            vec![
                Point2d::new(2.0, 6.0),
                Point2d::new(3.0, 9.0),
                Point2d::new(6.0, 9.0),
                Point2d::new(8.0, 6.0),
            ],
            vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        )
        .expect("raw spline");
    let err = sketch_ops::mirror(&s, &[EntityRef::Spline(raw)], &axis).expect_err("must refuse");
    assert!(
        matches!(&err, SketchOpError::Unsupported { reason, .. } if reason.contains("control")),
        "per-shape typed refusal expected, got {err:?}"
    );
}

#[test]
fn symmetric_arc_pair_drives_the_angles_not_just_the_center() {
    // The 4-row arc-pair Symmetric must pull a mis-rotated image arc
    // back onto the exact reflection — center rows alone would accept
    // ANY angular span at the mirrored center (which is precisely how
    // a zeroed-angle-row mutation would lie).
    let s = fresh("followups_symmetric_arc_pair_angles");
    let axis = construction_axis_x0(&s);
    let deg = std::f64::consts::PI / 180.0;
    let src = s
        .add_arc_center_angles(Point2d::new(5.0, 2.0), 2.5, 30.0 * deg, 120.0 * deg)
        .expect("source arc");
    // Correct mirrored span is [60, 150] deg; start 25 deg rotated.
    let dst = s
        .add_arc_center_angles(Point2d::new(-5.0, 2.0), 2.5, 85.0 * deg, 175.0 * deg)
        .expect("mis-rotated image arc");
    s.add_constraint(Constraint::new_geometric(
        GeometricConstraint::Symmetric,
        vec![
            EntityRef::Arc(src),
            EntityRef::Arc(dst),
            EntityRef::Line(axis),
        ],
        ConstraintPriority::High,
    ));
    s.add_constraint(Constraint::new_geometric(
        GeometricConstraint::Equal,
        vec![EntityRef::Arc(src), EntityRef::Arc(dst)],
        ConstraintPriority::High,
    ));
    let report = s.solve_constraints().expect("solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    let (src_geo, dst_geo) = (
        s.arcs().get(&src).expect("src").arc,
        s.arcs().get(&dst).expect("dst").arc,
    );
    let (sm, dm) = (src_geo.midpoint(), dst_geo.midpoint());
    assert!(
        (dm.x + sm.x).abs() < 1e-6 && (dm.y - sm.y).abs() < 1e-6,
        "the solve must rotate the image span onto the exact reflection: \
         {sm:?} vs {dm:?}"
    );
}
