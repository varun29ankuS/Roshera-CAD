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

// ── 4. Line / arc / spline pattern sources (Slice-6 residual 6) ─────

use geometry_engine::sketch2d::DofStatus;

fn pin_point(s: &Sketch, id: &geometry_engine::sketch2d::Point2dId, x: f64, y: f64) {
    s.add_constraint(required_dim(
        DimensionalConstraint::XCoordinate(x),
        vec![EntityRef::Point(*id)],
    ));
    s.add_constraint(required_dim(
        DimensionalConstraint::YCoordinate(y),
        vec![EntityRef::Point(*id)],
    ));
}

#[test]
fn red_linear_pattern_of_lines_is_fully_maintained() {
    // Slice-6 residual 6: line sources refused ("maintaining a
    // translated line rigidly needs 2 point-webs per instance").
    // The follow-up mints exactly those webs — one guide + Distance
    // chain per ENDPOINT, the Slice-6 scheme extended, not forked.
    let s = fresh("followups_linear_pattern_lines");
    let p1 = s.add_point(Point2d::new(0.0, 0.0));
    let p2 = s.add_point(Point2d::new(0.0, 8.0));
    let src = s.add_line(p1, p2).expect("source line");
    pin_point(&s, &p1, 0.0, 0.0);
    let y_pin = s.add_constraint(required_dim(
        DimensionalConstraint::YCoordinate(8.0),
        vec![EntityRef::Point(p2)],
    ));
    s.add_constraint(required_dim(
        DimensionalConstraint::XCoordinate(0.0),
        vec![EntityRef::Point(p2)],
    ));

    let outcome = sketch_ops::linear_pattern(&s, &[EntityRef::Line(src)], 3, 12.0, 0.0)
        .expect("line-source pattern");

    let instance_lines: Vec<_> = outcome
        .created
        .iter()
        .filter_map(|e| match e {
            EntityRef::Line(id) => Some(*id),
            _ => None,
        })
        .filter(|id| !s.lines().get(id).expect("line").is_construction)
        .collect();
    assert_eq!(instance_lines.len(), 2, "count 3 = source + 2 instances");
    for id in &instance_lines {
        let prov = s
            .provenance_of(&EntityRef::Line(*id))
            .expect("instance line carries provenance");
        assert_eq!(prov.op, SketchOpKind::LinearPattern);
        assert_eq!(prov.source, Some(EntityRef::Line(src)));
        assert!(prov.instance.is_some());
    }
    let guides = outcome
        .created
        .iter()
        .filter_map(|e| match e {
            EntityRef::Line(id) => Some(*id),
            _ => None,
        })
        .filter(|id| s.lines().get(id).expect("line").is_construction)
        .count();
    assert_eq!(guides, 2, "one construction guide per endpoint web");

    let dof = s.analyze_dofs();
    assert_eq!(
        dof.status,
        DofStatus::FullyConstrained,
        "line pattern must be fully maintained: {dof:?}"
    );
    let report = s.solve_constraints().expect("solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);

    // Instance endpoints at x = 12 and 24 (both webs stepped +12).
    let mut xs: Vec<f64> = instance_lines
        .iter()
        .flat_map(|id| {
            let (a, b) = s.lines().get(id).expect("line").endpoints.expect("derived");
            [s.get_point(&a).expect("a").x, s.get_point(&b).expect("b").x]
        })
        .collect();
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert!(
        (xs[0] - 12.0).abs() < 1e-6 && (xs[3] - 24.0).abs() < 1e-6,
        "{xs:?}"
    );

    // MAINTAINED: edit the source's top endpoint (y 8 -> 6) — every
    // instance's top endpoint must follow through its web.
    s.update_dimensional_value(&y_pin, 6.0).expect("edit");
    let report = s.solve_constraints().expect("re-solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    for id in &instance_lines {
        let (a, b) = s.lines().get(id).expect("line").endpoints.expect("derived");
        let top_y = s
            .get_point(&a)
            .expect("a")
            .y
            .max(s.get_point(&b).expect("b").y);
        assert!(
            (top_y - 6.0).abs() < 1e-6,
            "instance top endpoint must track the source edit: {top_y}"
        );
    }
}

#[test]
fn red_circular_pattern_of_arcs_holds_the_ring() {
    let s = fresh("followups_circular_pattern_arcs");
    let hub = s.add_point(Point2d::new(0.0, 0.0));
    s.points().get_mut(&hub).expect("hub").value_mut().fix();
    let e1 = s.add_point(Point2d::new(10.0, 0.0));
    let e2 = s.add_point(Point2d::new(0.0, 10.0));
    let src = s.add_arc(e1, e2, 10.0, true, false).expect("source arc");
    pin_point(&s, &e1, 10.0, 0.0);
    pin_point(&s, &e2, 0.0, 10.0);
    let r_pin = s.add_constraint(required_dim(
        DimensionalConstraint::Radius(10.0),
        vec![EntityRef::Arc(src)],
    ));

    let step = std::f64::consts::FRAC_PI_2;
    let outcome = sketch_ops::circular_pattern(&s, &[EntityRef::Arc(src)], &hub, 3, step)
        .expect("arc-source pattern");

    let instance_arcs: Vec<_> = outcome
        .created
        .iter()
        .filter_map(|e| match e {
            EntityRef::Arc(id) => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(instance_arcs.len(), 2, "count 3 = source + 2 instances");

    let dof = s.analyze_dofs();
    assert_eq!(
        dof.status,
        DofStatus::FullyConstrained,
        "arc ring must be fully maintained: {dof:?}"
    );
    let report = s.solve_constraints().expect("solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);

    // Instance midpoints are the 90/180-degree rotations of the
    // source's.
    let rotated: Vec<(f64, f64)> = instance_arcs
        .iter()
        .map(|id| {
            let m = s.arcs().get(id).expect("arc").arc.midpoint();
            (m.x, m.y)
        })
        .collect();
    let src_mid = s.arcs().get(&src).expect("src").arc.midpoint();
    let expect1 = (-src_mid.y, src_mid.x); // +90 deg
    let expect2 = (-src_mid.x, -src_mid.y); // +180 deg
    for exp in [expect1, expect2] {
        assert!(
            rotated
                .iter()
                .any(|(x, y)| (x - exp.0).abs() < 1e-6 && (y - exp.1).abs() < 1e-6),
            "expected a rotated arc midpoint at {exp:?}, got {rotated:?}"
        );
    }

    // Radius edit propagates through the Equal chain.
    s.update_dimensional_value(&r_pin, 8.0).expect("edit");
    let report = s.solve_constraints().expect("re-solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    for id in &instance_arcs {
        let r = s.arcs().get(id).expect("arc").arc.radius;
        assert!((r - 8.0).abs() < 1e-6, "Equal chain must propagate: {r}");
    }
}

#[test]
fn red_linear_pattern_of_shared_cp_splines() {
    // Shared-CP splines are point webs (Slice 7): a pattern instance
    // is a fresh spline over per-CP webs — clean under the same
    // scheme, so it is IMPLEMENTED (raw-CP splines refuse per-shape).
    let s = fresh("followups_linear_pattern_spline");
    let positions = [
        Point2d::new(0.0, 0.0),
        Point2d::new(2.0, 5.0),
        Point2d::new(6.0, 5.0),
        Point2d::new(8.0, 0.0),
    ];
    let cps: Vec<_> = positions.iter().map(|p| s.add_point(*p)).collect();
    let src = s
        .add_bspline_with_control_points(3, &cps)
        .expect("source spline");
    for (cp, pos) in cps.iter().zip(&positions) {
        pin_point(&s, cp, pos.x, pos.y);
    }

    let outcome = sketch_ops::linear_pattern(&s, &[EntityRef::Spline(src)], 2, 15.0, 0.0)
        .expect("spline-source pattern");

    let instance = outcome
        .created
        .iter()
        .find_map(|e| match e {
            EntityRef::Spline(id) => Some(*id),
            _ => None,
        })
        .expect("instance spline created");
    let inst_cps = s
        .spline_control_point_ids(&instance)
        .expect("instance is shared-CP");
    assert_eq!(inst_cps.len(), 4);
    for (src_cp, dst_cp) in cps.iter().zip(&inst_cps) {
        let (sp, dp) = (
            s.get_point(src_cp).expect("src"),
            s.get_point(dst_cp).expect("dst"),
        );
        assert!(
            (dp.x - sp.x - 15.0).abs() < 1e-9 && (dp.y - sp.y).abs() < 1e-9,
            "instance CP must be the translated source CP: {sp:?} vs {dp:?}"
        );
    }
    let prov = s
        .provenance_of(&EntityRef::Spline(instance))
        .expect("provenance");
    assert_eq!(prov.op, SketchOpKind::LinearPattern);
    assert_eq!(prov.source, Some(EntityRef::Spline(src)));
    assert_eq!(prov.instance, Some(1));

    let dof = s.analyze_dofs();
    assert_eq!(
        dof.status,
        DofStatus::FullyConstrained,
        "spline pattern must be fully maintained: {dof:?}"
    );
    let report = s.solve_constraints().expect("solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);
}

#[test]
fn pattern_refuses_legacy_and_raw_shapes_per_shape() {
    let s = fresh("followups_pattern_per_shape_refuse");
    // Raw-CP spline: no point web to maintain.
    let raw = s
        .add_bspline(
            3,
            vec![
                Point2d::new(0.0, 0.0),
                Point2d::new(2.0, 5.0),
                Point2d::new(6.0, 5.0),
                Point2d::new(8.0, 0.0),
            ],
            vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        )
        .expect("raw spline");
    let err = sketch_ops::linear_pattern(&s, &[EntityRef::Spline(raw)], 2, 10.0, 0.0)
        .expect_err("raw spline must refuse");
    assert!(
        matches!(&err, SketchOpError::Unsupported { reason, .. } if reason.contains("control")),
        "per-shape typed refusal expected, got {err:?}"
    );

    // Legacy center-angle arc: no endpoint web.
    let legacy = s
        .add_arc_center_angles(Point2d::new(3.0, 3.0), 2.0, 0.0, 1.0)
        .expect("legacy arc");
    let err = sketch_ops::linear_pattern(&s, &[EntityRef::Arc(legacy)], 2, 10.0, 0.0)
        .expect_err("legacy arc must refuse");
    assert!(
        matches!(&err, SketchOpError::Unsupported { reason, .. } if reason.contains("legacy")),
        "per-shape typed refusal expected, got {err:?}"
    );

    // Ellipses stay outside every pattern envelope.
    let ell = s
        .add_ellipse(Point2d::new(0.0, 0.0), 4.0, 2.0, 0.0)
        .expect("ellipse");
    let err = sketch_ops::linear_pattern(&s, &[EntityRef::Ellipse(ell)], 2, 10.0, 0.0)
        .expect_err("ellipse must refuse");
    assert!(matches!(err, SketchOpError::Unsupported { .. }));

    // Validate-first: nothing was minted by any refusal.
    assert_eq!(s.all_constraints().len(), 0);
    assert_eq!(s.points().len(), 0);
    assert_eq!(s.lines().len(), 0);

    // curve/phyllotaxis patterns keep the strict point/circle set —
    // entity webs along a rail are not in this follow-up's envelope.
    let p1 = s.add_point(Point2d::new(0.0, 0.0));
    let p2 = s.add_point(Point2d::new(4.0, 0.0));
    let line = s.add_line(p1, p2).expect("line");
    let rail = s
        .add_arc_center_angles(Point2d::new(0.0, -20.0), 25.0, 0.5, 2.0)
        .expect("rail");
    let err = sketch_ops::curve_pattern(
        &s,
        &[EntityRef::Line(line)],
        &EntityRef::Arc(rail),
        3,
        Some(2.0),
    )
    .expect_err("curve_pattern keeps point/circle sources");
    assert!(matches!(err, SketchOpError::Unsupported { .. }));
}

// ── 5. All-arc offset loops (Slice-6 residual 2) ────────────────────

#[test]
fn red_all_arc_offset_loop_is_fully_maintained() {
    // Slice-6 residual 2: arc offsets minted only the radial-gap
    // OffsetDistance, so a loop with arc-arc junctions reported
    // UnderConstrained (honestly). The follow-up mints per-junction:
    // the concentric Offset for source arcs whose junctions have no
    // adjacent offset-line rows to pin them, and a G1 SmoothTangent at
    // every arc-arc junction (radial ties are first-order degenerate
    // there: the carriers are mutually tangent). Lens hand-count:
    // 4 junctions (8) + 4 chords (4) = 12 free;
    // 2x(Offset 2 + OffsetDistance 1) + 2xRadius + 4xSmoothTangent = 12.
    let s = fresh("followups_offset_lens");
    let a = s.add_point(Point2d::new(-6.0, 0.0));
    let b = s.add_point(Point2d::new(6.0, 0.0));
    let arc1 = s.add_arc(a, b, 10.0, true, false).expect("bottom arc");
    let arc2 = s.add_arc(b, a, 10.0, true, false).expect("top arc");
    for id in [a, b] {
        s.points().get_mut(&id).expect("point").value_mut().fix();
    }
    let r1_pin = s.add_constraint(required_dim(
        DimensionalConstraint::Radius(10.0),
        vec![EntityRef::Arc(arc1)],
    ));
    s.add_constraint(required_dim(
        DimensionalConstraint::Radius(10.0),
        vec![EntityRef::Arc(arc2)],
    ));

    let d = 3.0;
    let outcome = sketch_ops::offset(&s, &EntityRef::Arc(arc1), d).expect("lens offset");

    let new_arcs: Vec<_> = outcome
        .created
        .iter()
        .filter_map(|e| match e {
            EntityRef::Arc(id) => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(new_arcs.len(), 4, "two offset arcs + two corner arcs");
    let radii: Vec<f64> = new_arcs
        .iter()
        .map(|id| s.arcs().get(id).expect("arc").arc.radius)
        .collect();
    assert_eq!(
        radii.iter().filter(|r| (**r - 13.0).abs() < 1e-9).count(),
        2,
        "offset arcs grow to r + d: {radii:?}"
    );
    assert_eq!(
        radii.iter().filter(|r| (**r - d).abs() < 1e-9).count(),
        2,
        "corner arcs have radius |d|: {radii:?}"
    );

    // Per-junction maintenance web.
    let minted_kinds = |pred: &dyn Fn(&ConstraintType) -> bool| -> usize {
        outcome
            .constraints_added
            .iter()
            .filter(|id| {
                s.all_constraints()
                    .iter()
                    .any(|c| c.id == **id && pred(&c.constraint_type))
            })
            .count()
    };
    assert_eq!(
        minted_kinds(&|t| matches!(t, ConstraintType::Geometric(GeometricConstraint::Offset))),
        2,
        "one concentric Offset per source arc pair (arc-arc junctions on both sides)"
    );
    assert_eq!(
        minted_kinds(&|t| matches!(
            t,
            ConstraintType::Dimensional(DimensionalConstraint::OffsetDistance(_))
        )),
        2
    );
    assert_eq!(
        minted_kinds(&|t| matches!(
            t,
            ConstraintType::Dimensional(DimensionalConstraint::Radius(_))
        )),
        2,
        "corner arc radius pins"
    );
    assert_eq!(
        minted_kinds(&|t| matches!(
            t,
            ConstraintType::Geometric(GeometricConstraint::SmoothTangent)
        )),
        4,
        "every arc-arc junction carries a G1 SmoothTangent maintenance row"
    );

    let dof = s.analyze_dofs();
    assert_eq!(
        dof.status,
        DofStatus::FullyConstrained,
        "the all-arc offset loop must be fully maintained: {dof:?}"
    );
    let report = s.solve_constraints().expect("solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);

    // MAINTAINED: shrink the bottom source arc's radius 10 -> 9; its
    // offset partner must re-solve to 12 through the radial gap while
    // the concentric rows keep the carriers locked.
    s.update_dimensional_value(&r1_pin, 9.0).expect("edit");
    let report = s.solve_constraints().expect("re-solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    let radii_after: Vec<f64> = new_arcs
        .iter()
        .map(|id| s.arcs().get(id).expect("arc").arc.radius)
        .collect();
    assert!(
        radii_after.iter().any(|r| (r - 12.0).abs() < 1e-6),
        "the offset arc must track the source radius edit: {radii_after:?}"
    );
}

// ── 6. Offset global self-intersection (Slice-6 residual 3) ─────────

#[test]
fn red_offset_refuses_global_self_intersection_typed() {
    // T-slot block: a narrow neck (width 10) opening into a wider
    // cavity. Offsetting the loop OUTWARD by 6 keeps every edge
    // locally valid (no inversion, all closing trims fit), but the
    // two neck-top corner arcs — far apart in the ring — collide.
    // Slice-6 residual 3: this used to solve and certify as a
    // silently self-intersecting loop; it must now refuse typed,
    // naming the colliding segments, leaving the sketch untouched.
    let s = fresh("followups_offset_self_intersect");
    let pts = [
        Point2d::new(0.0, 0.0),
        Point2d::new(50.0, 0.0),
        Point2d::new(50.0, 40.0),
        Point2d::new(30.0, 40.0),
        Point2d::new(30.0, 20.0),
        Point2d::new(45.0, 20.0),
        Point2d::new(45.0, 5.0),
        Point2d::new(5.0, 5.0),
        Point2d::new(5.0, 20.0),
        Point2d::new(20.0, 20.0),
        Point2d::new(20.0, 40.0),
        Point2d::new(0.0, 40.0),
    ];
    let ids: Vec<_> = pts.iter().map(|p| s.add_point(*p)).collect();
    let mut first_line = None;
    for i in 0..ids.len() {
        let lid = s
            .add_line(ids[i], ids[(i + 1) % ids.len()])
            .expect("outline line");
        first_line.get_or_insert(lid);
    }
    let points_before = s.points().len();
    let lines_before = s.lines().len();
    let arcs_before = s.arcs().len();
    let constraints_before = s.all_constraints().len();

    let err = sketch_ops::offset(&s, &EntityRef::Line(first_line.expect("line")), 6.0)
        .expect_err("globally colliding offset must refuse");
    assert!(
        matches!(&err, SketchOpError::SelfIntersecting { first, second }
            if !first.is_empty() && !second.is_empty()),
        "typed SelfIntersecting refusal naming the colliding segments, got {err:?}"
    );

    // Validate-first: the refusal left the sketch byte-identical.
    assert_eq!(s.points().len(), points_before);
    assert_eq!(s.lines().len(), lines_before);
    assert_eq!(s.arcs().len(), arcs_before);
    assert_eq!(s.all_constraints().len(), constraints_before);

    // A smaller offset that does NOT collide still succeeds (the neck
    // walls stop 2 apart at d = 4).
    let outcome = sketch_ops::offset(&s, &EntityRef::Line(first_line.expect("line")), 4.0)
        .expect("non-colliding offset succeeds");
    assert!(!outcome.created.is_empty());
}

// ── 7. Trim constraint re-application (Slice-6 residual 9) ──────────

#[test]
fn red_trim_reapplies_carrier_constraints_and_drops_extent_bound_ones() {
    // D-Cubed-style rule, principled not heuristic: a constraint
    // survives a trim iff it pins the CARRIER (infinite line / full
    // circle), which trim preserves — direction/incidence kinds
    // re-attach to EVERY survivor; extent-bound kinds (Length,
    // Midpoint, ...) are genuinely dropped and reported. PointOnCurve
    // re-attaches to the survivor whose extent contains the point.
    let s = fresh("followups_trim_reapply_line");
    let a = s.add_point(Point2d::new(0.0, 0.0));
    let b = s.add_point(Point2d::new(20.0, 0.0));
    let line = s.add_line(a, b).expect("target line");
    let o1 = s.add_point(Point2d::new(0.0, 7.0));
    let o2 = s.add_point(Point2d::new(20.0, 7.0));
    let other = s.add_line(o1, o2).expect("other line");
    let h_id = s.add_constraint(Constraint::new_geometric(
        GeometricConstraint::Horizontal,
        vec![EntityRef::Line(line)],
        ConstraintPriority::High,
    ));
    let len_id = s.add_constraint(required_dim(
        DimensionalConstraint::Length(20.0),
        vec![EntityRef::Line(line)],
    ));
    let par_id = s.add_constraint(Constraint::new_geometric(
        GeometricConstraint::Parallel,
        vec![EntityRef::Line(line), EntityRef::Line(other)],
        ConstraintPriority::High,
    ));
    let p_keep = s.add_point(Point2d::new(2.0, 0.0));
    let poc_keep = s.add_constraint(Constraint::new_geometric(
        GeometricConstraint::PointOnCurve,
        vec![EntityRef::Point(p_keep), EntityRef::Line(line)],
        ConstraintPriority::High,
    ));
    let p_gone = s.add_point(Point2d::new(10.0, 0.0));
    let poc_gone = s.add_constraint(Constraint::new_geometric(
        GeometricConstraint::PointOnCurve,
        vec![EntityRef::Point(p_gone), EntityRef::Line(line)],
        ConstraintPriority::High,
    ));

    let cutter = s.add_circle(Point2d::new(10.0, 3.0), 5.0).expect("cutter");
    let outcome = sketch_ops::trim(
        &s,
        &EntityRef::Line(line),
        &EntityRef::Circle(cutter),
        Point2d::new(10.0, 0.0),
    )
    .expect("trim");

    let survivors: Vec<_> = outcome
        .created
        .iter()
        .filter_map(|e| match e {
            EntityRef::Line(id) => Some(EntityRef::Line(*id)),
            _ => None,
        })
        .collect();
    assert_eq!(survivors.len(), 2);

    // Re-applied: Horizontal x2 (each survivor is an independent
    // entity), Parallel x2, PointOnCurve(p_keep) x1 (left survivor
    // only — extent-gated).
    let reapplied = &outcome.constraints_reapplied;
    let count_orig = |id| reapplied.iter().filter(|r| r.original == id).count();
    assert_eq!(count_orig(h_id), 2, "Horizontal onto both survivors");
    assert_eq!(count_orig(par_id), 2, "Parallel onto both survivors");
    assert_eq!(
        count_orig(poc_keep),
        1,
        "PointOnCurve re-attaches to the ONE survivor containing the point"
    );
    // The re-applied PointOnCurve targets the survivor whose extent
    // contains x = 2 (the [0, 6] span).
    let poc_record = reapplied
        .iter()
        .find(|r| r.original == poc_keep)
        .expect("poc record");
    let EntityRef::Line(sid) = poc_record.entity else {
        panic!("survivor must be a line");
    };
    let (sa, sb) = s
        .lines()
        .get(&sid)
        .expect("survivor")
        .endpoints
        .expect("derived");
    let xs = [
        s.get_point(&sa).expect("sa").x,
        s.get_point(&sb).expect("sb").x,
    ];
    assert!(
        xs.iter().any(|x| x.abs() < 1e-9) && xs.iter().any(|x| (x - 6.0).abs() < 1e-9),
        "p_keep must ride the [0, 6] survivor: {xs:?}"
    );

    // Genuinely dropped (extent-bound): Length; PointOnCurve whose
    // point rode the removed span.
    assert!(outcome.constraints_removed.contains(&len_id));
    assert!(outcome.constraints_removed.contains(&poc_gone));
    assert!(
        !outcome.constraints_removed.contains(&h_id),
        "re-applied constraints are not reported as dropped"
    );

    // The re-applied web is live: the sketch solves violation-free
    // and the survivors stay horizontal.
    let report = s.solve_constraints().expect("solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    for sref in &survivors {
        let EntityRef::Line(id) = sref else { continue };
        let (sa, sb) = s.lines().get(id).expect("line").endpoints.expect("derived");
        let (pa, pb) = (s.get_point(&sa).expect("sa"), s.get_point(&sb).expect("sb"));
        assert!(
            (pa.y - pb.y).abs() < 1e-6,
            "survivor must stay horizontal: {pa:?} {pb:?}"
        );
    }
}

#[test]
fn red_trim_circle_to_arc_reapplies_radius_and_equal_maintained() {
    // Carrier constraints on a trimmed CIRCLE re-target the surviving
    // ARC: Radius pins and Equal (radius) chains stay maintained —
    // including across the circle->arc kind change (the solver's
    // Equal gains the mixed circle/arc radius arm; a silent-zero row
    // would be a lie).
    let s = fresh("followups_trim_reapply_circle");
    let target = s.add_circle(Point2d::new(0.0, 0.0), 6.0).expect("target");
    let partner = s.add_circle(Point2d::new(20.0, 0.0), 6.0).expect("partner");
    let r_id = s.add_constraint(required_dim(
        DimensionalConstraint::Radius(6.0),
        vec![EntityRef::Circle(target)],
    ));
    let eq_id = s.add_constraint(Constraint::new_geometric(
        GeometricConstraint::Equal,
        vec![EntityRef::Circle(target), EntityRef::Circle(partner)],
        ConstraintPriority::High,
    ));
    let c1 = s.add_point(Point2d::new(3.0, -10.0));
    let c2 = s.add_point(Point2d::new(3.0, 10.0));
    let cutter = s.add_line(c1, c2).expect("cutter");

    let outcome = sketch_ops::trim(
        &s,
        &EntityRef::Circle(target),
        &EntityRef::Line(cutter),
        Point2d::new(6.0, 0.0),
    )
    .expect("trim");

    let arc = outcome
        .created
        .iter()
        .find_map(|e| match e {
            EntityRef::Arc(id) => Some(*id),
            _ => None,
        })
        .expect("surviving arc");
    let reapplied = &outcome.constraints_reapplied;
    let minted_radius = reapplied
        .iter()
        .find(|r| r.original == r_id)
        .expect("Radius re-applied to the surviving arc");
    assert_eq!(minted_radius.entity, EntityRef::Arc(arc));
    assert!(
        reapplied.iter().any(|r| r.original == eq_id),
        "Equal (radius) re-applied across the circle->arc kind change"
    );
    assert!(outcome.constraints_removed.is_empty(), "nothing dropped");

    // MAINTAINED through the kind change: edit the re-minted Radius
    // pin 6 -> 5 and the Equal chain must pull the partner circle too.
    let report = s.solve_constraints().expect("solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    s.update_dimensional_value(&minted_radius.minted, 5.0)
        .expect("edit re-minted radius");
    let report = s.solve_constraints().expect("re-solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    let arc_r = s.arcs().get(&arc).expect("arc").arc.radius;
    let partner_r = s.circles().get(&partner).expect("partner").circle.radius;
    assert!((arc_r - 5.0).abs() < 1e-6, "arc radius follows: {arc_r}");
    assert!(
        (partner_r - 5.0).abs() < 1e-6,
        "Equal must propagate through the mixed arc/circle pair: {partner_r}"
    );
}
