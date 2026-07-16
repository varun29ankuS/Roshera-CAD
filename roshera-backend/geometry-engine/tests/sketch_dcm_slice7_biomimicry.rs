// Reason: integration-test crate -- panicking (unwrap/expect/assert/index) is
// the test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::indexing_slicing)]

//! SKETCH-DCM #45 Slice 7 — biomimicry: curve-complete, certified
//! organic sketching (founder directive 2026-07-15; spec §3.6's
//! no-fairing-objective non-goal stands — control points are plain
//! solver DOFs).
//!
//! Stage A — splines as first-class SOLVER citizens: control points
//! are shared `Point2dId`s (draggable, constrainable, zero phantom
//! DOF — the Slice-1 shared-variable model extended to splines).
//!
//! Stage B — continuity constraints that cannot lie:
//!   - `SmoothTangent` (G1) = tangent DIRECTION continuity at the
//!     join. One row. It must NOT demand equal tangent magnitudes
//!     (the old evaluator's `(|t1|−|t2|)·0.1` hack was C1, not G1).
//!   - `CurvatureContinuity` (G2) = tangent row + TRAVERSAL-SIGNED
//!     curvature match. Unsigned |κ| comparison calls an S-join
//!     (inflection) "continuous" — a lie this file pins away.
//!   - `Curvature(κ)` at a curve point (`[curve, point]` arity) and
//!     `CurvatureExtremum` (`[spline, point]`: ∂κ/∂u = 0 at the
//!     point's foot) gain real residuals; unsupported shapes keep
//!     the irreducible refuse residual.
//!
//! Stage C — the certificate measures continuity: per-join facts
//! with numeric tangent/curvature deviations ("the kernel cannot
//! lie" extended to organic curves).

use geometry_engine::sketch2d::{
    analyze_dofs, certify_sketch, Constraint, ConstraintPriority, ConstraintSolver, ContinuityKind,
    DimensionalConstraint, DofStatus, DragTarget, EntityRef, EntityState, GeometricConstraint,
    Point2d, Point2dId, Sketch, SketchAnchor, Spline2d,
};

fn fresh(name: &str) -> Sketch {
    Sketch::new(name.to_string(), SketchAnchor::xy())
}

fn fix_point(sketch: &Sketch, id: &Point2dId) {
    sketch
        .points()
        .get_mut(id)
        .expect("point present")
        .value_mut()
        .fix();
}

fn high_geo(gc: GeometricConstraint, entities: Vec<EntityRef>) -> Constraint {
    Constraint::new_geometric(gc, entities, ConstraintPriority::High)
}

fn high_dim(dc: DimensionalConstraint, entities: Vec<EntityRef>) -> Constraint {
    Constraint::new_dimensional(dc, entities, ConstraintPriority::High)
}

/// Evaluate a `Spline2d` position at `u`.
fn eval(spline: &Spline2d, u: f64) -> Point2d {
    spline.evaluate(u).expect("spline evaluates")
}

/// Traversal-signed curvature of a `Spline2d` at parameter `u`,
/// measured by central differences on the evaluated curve — an
/// implementation-independent oracle (κ = (x'y'' − y'x'') / |r'|³).
fn measured_curvature(spline: &Spline2d, u: f64) -> f64 {
    let (u_min, u_max) = spline.parameter_range();
    let h = (u_max - u_min) * 1e-5;
    let u = u.clamp(u_min + 2.0 * h, u_max - 2.0 * h);
    let p_plus = eval(spline, u + h);
    let p_minus = eval(spline, u - h);
    let p0 = eval(spline, u);
    let dx = (p_plus.x - p_minus.x) / (2.0 * h);
    let dy = (p_plus.y - p_minus.y) / (2.0 * h);
    let ddx = (p_plus.x - 2.0 * p0.x + p_minus.x) / (h * h);
    let ddy = (p_plus.y - 2.0 * p0.y + p_minus.y) / (h * h);
    (dx * ddy - dy * ddx) / (dx * dx + dy * dy).powf(1.5)
}

/// Closest parameter of `p` on a spline by dense scan + refinement —
/// oracle-grade, independent of the kernel's closest_point.
fn foot_parameter(spline: &Spline2d, p: &Point2d) -> f64 {
    let (u_min, u_max) = spline.parameter_range();
    let mut best_u = u_min;
    let mut best_d = f64::INFINITY;
    for i in 0..=2000 {
        let u = u_min + (u_max - u_min) * (i as f64) / 2000.0;
        let q = eval(spline, u);
        let d = (q.x - p.x).powi(2) + (q.y - p.y).powi(2);
        if d < best_d {
            best_d = d;
            best_u = u;
        }
    }
    best_u
}

// ── Stage A: shared-control-point splines ───────────────────────────

/// RED (compile + assert): a spline whose control points are shared
/// point entities contributes ZERO private DOF — its geometry IS its
/// points, exactly like derived segments (A.2) and endpoint-derived
/// arcs (Slice 1). Hand count: 4 points × 2 = 8 free; spline = 0.
/// Pre-slice a 4-CP spline carried 8 phantom DOF of its own, so a
/// fully-dimensioned organic profile could never certify
/// FullyConstrained.
#[test]
fn red_shared_cp_spline_dofs_match_hand_count() {
    let s = fresh("slice7_shared_cp_dof");
    let p0 = s.add_point(Point2d::new(0.0, 0.0));
    let p1 = s.add_point(Point2d::new(10.0, 12.0));
    let p2 = s.add_point(Point2d::new(20.0, 12.0));
    let p3 = s.add_point(Point2d::new(30.0, 0.0));
    let spline = s
        .add_bspline_with_control_points(3, &[p0, p1, p2, p3])
        .expect("shared-CP spline");

    let report = analyze_dofs(&s);
    assert_eq!(
        report.total_free_dofs, 8,
        "phantom spline DOFs: 4 shared CPs must contribute 8 total (spline itself 0), got {}",
        report.total_free_dofs
    );

    // Pin every CP with coordinate dimensions → FullyConstrained.
    for id in [p0, p1, p2, p3] {
        let pos = s.get_point(&id).expect("cp");
        s.add_constraint(high_dim(
            DimensionalConstraint::XCoordinate(pos.x),
            vec![EntityRef::Point(id)],
        ));
        s.add_constraint(high_dim(
            DimensionalConstraint::YCoordinate(pos.y),
            vec![EntityRef::Point(id)],
        ));
    }
    let report = analyze_dofs(&s);
    assert_eq!(
        report.status,
        DofStatus::FullyConstrained,
        "a coordinate-pinned shared-CP spline sketch must be fully constrained: {report:?}"
    );

    // The stored geometry interpolates the shared points (clamped)
    // and the sketch can echo the linkage.
    let ids = s
        .spline_control_point_ids(&spline)
        .expect("control-point linkage recorded");
    assert_eq!(ids, vec![p0, p1, p2, p3]);
}

/// RED (compile + assert): dragging a shared control point reshapes
/// the spline — the post-solve stored geometry's control point IS the
/// point's solved position (single-writer sync, same contract as
/// derived segments).
#[test]
fn red_dragging_a_control_point_reshapes_the_spline() {
    let s = fresh("slice7_drag_cp");
    let p0 = s.add_point(Point2d::new(0.0, 0.0));
    let p1 = s.add_point(Point2d::new(10.0, 12.0));
    let p2 = s.add_point(Point2d::new(20.0, 12.0));
    let p3 = s.add_point(Point2d::new(30.0, 0.0));
    fix_point(&s, &p0);
    fix_point(&s, &p3);
    let spline = s
        .add_bspline_with_control_points(3, &[p0, p1, p2, p3])
        .expect("shared-CP spline");

    let before_mid = {
        let entry = s.splines().get(&spline).expect("spline");
        eval(&entry.value().spline, 0.5)
    };

    let target = Point2d::new(10.0, 20.0);
    let report = s
        .solve_drag(EntityRef::Point(p1), DragTarget::Point(target))
        .expect("drag solve");
    assert!(
        report.violations.is_empty(),
        "unconstrained CP drag must be violation-free: {:?}",
        report.violations
    );

    let dragged = s.get_point(&p1).expect("p1");
    assert!(
        (dragged.x - target.x).abs() < 1e-6 && (dragged.y - target.y).abs() < 1e-6,
        "drag must pull the CP onto the cursor, got {dragged:?}"
    );

    let entry = s.splines().get(&spline).expect("spline");
    let cps = match &entry.value().spline {
        Spline2d::BSpline(bs) => bs.control_points.clone(),
        Spline2d::Nurbs(n) => n.control_points.clone(),
    };
    assert_eq!(
        (cps[1].x, cps[1].y),
        (dragged.x, dragged.y),
        "stored spline CP must be synced bitwise from the shared point"
    );
    let after_mid = eval(&entry.value().spline, 0.5);
    assert!(
        (after_mid.y - before_mid.y).abs() > 1.0,
        "spline midpoint must move with the dragged CP: {before_mid:?} -> {after_mid:?}"
    );
}

// ── Stage B: continuity constraints ─────────────────────────────────

/// RED (assert, pre-existing surface): G1 is tangent DIRECTION
/// continuity. Two collinear derived segments of DIFFERENT lengths
/// sharing an endpoint are perfectly G1 — the old evaluator's
/// `(|t1| − |t2|) · 0.1` magnitude row reported a phantom violation.
#[test]
fn red_g1_between_segments_does_not_demand_equal_lengths() {
    let s = fresh("slice7_g1_lengths");
    let a = s.add_point(Point2d::new(0.0, 0.0));
    let j = s.add_point(Point2d::new(10.0, 0.0));
    let b = s.add_point(Point2d::new(30.0, 0.0));
    fix_point(&s, &a);
    fix_point(&s, &j);
    fix_point(&s, &b);
    let l1 = s.add_line(a, j).expect("l1");
    let l2 = s.add_line(j, b).expect("l2");

    let cid = s.add_constraint(high_geo(
        GeometricConstraint::SmoothTangent,
        vec![EntityRef::Line(l1), EntityRef::Line(l2)],
    ));
    let report = s.solve_constraints().expect("solve");
    assert!(
        !report.violations.iter().any(|(id, _)| *id == cid),
        "collinear segments of different lengths ARE G1-continuous; \
         the magnitude-matching hack must be gone: {:?}",
        report.violations
    );
}

/// RED (assert, pre-existing surface): a G1 constraint touching a
/// spline must never SILENTLY pass. The old evaluator returned
/// `None → [0, 0]` for spline tangents — a kinked line-spline join
/// reported zero residual. Both the 2-entity and the 3-entity
/// (`[c1, c2, connection_point]`) arities are pinned.
#[test]
fn red_g1_with_spline_is_never_a_silent_zero() {
    let mut solver = ConstraintSolver::new();
    let line = EntityRef::Line(geometry_engine::sketch2d::Line2dId::new());
    let spline = EntityRef::Spline(geometry_engine::sketch2d::Spline2dId::new());
    let joint = EntityRef::Point(Point2dId::new());
    // Fixed horizontal line ending at the origin...
    solver.add_entity(
        line,
        EntityState::line(
            Point2d::new(-10.0, 0.0),
            geometry_engine::sketch2d::Vector2d::new(1.0, 0.0),
            true,
            true,
        ),
    );
    // ...meeting a fixed cubic that leaves the origin STEEPLY upward
    // (tangent (1, 2) — a visible kink).
    let cps = vec![
        Point2d::new(0.0, 0.0),
        Point2d::new(1.0, 2.0),
        Point2d::new(2.0, 3.0),
        Point2d::new(3.0, 3.0),
    ];
    let knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
    solver.add_entity(spline, EntityState::spline_bspline(3, cps, knots, true));
    solver.add_entity(joint, EntityState::point(Point2d::new(0.0, 0.0), true));

    for entities in [vec![line, spline], vec![line, spline, joint]] {
        let con = high_geo(GeometricConstraint::SmoothTangent, entities.clone());
        solver.set_constraints(vec![con.clone()]);
        let _ = solver.solve();
        let residuals = solver.residuals_by_constraint();
        let (_, r) = residuals.first().expect("one constraint");
        assert!(
            *r > 1e-3,
            "a kinked line-spline G1 join (arity {}) must surface a real residual, got {r}",
            entities.len()
        );
    }
}

/// RED (assert, pre-existing surface): G2 must compare
/// TRAVERSAL-SIGNED curvatures. Two equal-radius arcs joined in an
/// S (inflection) have κ = +1/r and −1/r along the walk — NOT
/// curvature-continuous. The old unsigned |κ| comparison called the
/// S-join satisfied. The C-join (same bending side) IS G2 and must
/// stay satisfied.
#[test]
fn red_g2_distinguishes_bending_side_at_arc_joins() {
    // C-join: two arcs of the SAME circle (center origin, r = 6),
    // traversed continuously a(180°) → j(90°) → b(0°), i.e. clockwise
    // (`ccw = false`). Tangent AND traversal-signed curvature agree at
    // the join (κ = −1/6 both) — genuinely G2, must NOT be violated.
    {
        let s = fresh("slice7_g2_c_join");
        let a = s.add_point(Point2d::new(-6.0, 0.0));
        let j = s.add_point(Point2d::new(0.0, 6.0));
        let b = s.add_point(Point2d::new(6.0, 0.0));
        fix_point(&s, &a);
        fix_point(&s, &j);
        fix_point(&s, &b);
        let arc1 = s.add_arc(a, j, 6.0, false, false).expect("arc1");
        let arc2 = s.add_arc(j, b, 6.0, false, false).expect("arc2");
        for arc in [arc1, arc2] {
            s.add_constraint(Constraint::new_dimensional(
                DimensionalConstraint::Radius(6.0),
                vec![EntityRef::Arc(arc)],
                ConstraintPriority::Required,
            ));
        }
        let cid = s.add_constraint(high_geo(
            GeometricConstraint::CurvatureContinuity,
            vec![EntityRef::Arc(arc1), EntityRef::Arc(arc2)],
        ));
        let report = s.solve_constraints().expect("solve");
        assert!(
            !report.violations.iter().any(|(id, _)| *id == cid),
            "two arcs of one circle traversed continuously ARE G2: {:?}",
            report.violations
        );
    }
    // S-join (inflection): collinear chords along the x-axis with
    // OPPOSITE orientation bits — tangents are continuous at j but the
    // traversal-signed curvatures are +1/6 and −1/6 (the classic
    // ogee/inflection). The pre-slice unsigned |κ| comparison called
    // this satisfied — the lie this pin kills. Geometry is fully
    // pinned, so the constraint cannot be "solved away": it must be
    // REPORTED violated with the 2/r curvature jump.
    {
        let s = fresh("slice7_g2_s_join");
        let a = s.add_point(Point2d::new(0.0, 0.0));
        let j = s.add_point(Point2d::new(10.0, 0.0));
        let b = s.add_point(Point2d::new(20.0, 0.0));
        fix_point(&s, &a);
        fix_point(&s, &j);
        fix_point(&s, &b);
        let arc1 = s.add_arc(a, j, 6.0, true, false).expect("arc1");
        let arc2 = s.add_arc(j, b, 6.0, false, false).expect("arc2");
        for arc in [arc1, arc2] {
            s.add_constraint(Constraint::new_dimensional(
                DimensionalConstraint::Radius(6.0),
                vec![EntityRef::Arc(arc)],
                ConstraintPriority::Required,
            ));
        }
        let cid = s.add_constraint(high_geo(
            GeometricConstraint::CurvatureContinuity,
            vec![EntityRef::Arc(arc1), EntityRef::Arc(arc2)],
        ));
        // Certificate measurement on the EXACT pinned geometry: the
        // inflection's curvature jump is 2/r bit-for-bit (no solver
        // compromise in the way).
        let cert = certify_sketch(&s);
        let fact = cert
            .continuity
            .iter()
            .find(|f| f.constraint == cid)
            .expect("G2 join must surface a continuity fact");
        assert!(fact.measured);
        assert!(
            !fact.satisfied,
            "an S-join must be REPORTED curvature-discontinuous (the unsigned-|κ| lie): {fact:?}"
        );
        let tangent_dev = fact.tangent_deviation_rad.expect("tangent measured");
        assert!(
            tangent_dev < 1e-9,
            "the inflection IS tangent-continuous (G1): {tangent_dev}"
        );
        let jump = fact.curvature_deviation.expect("curvature measured");
        assert!(
            (jump - 2.0 / 6.0).abs() < 1e-9,
            "the measured jump must be the actual 2/r inflection step, got {jump}"
        );
        // And the solver reports it violated rather than claiming a
        // solve (the residual cannot reach zero — geometry is pinned
        // up to least-squares slack).
        let report = s.solve_constraints().expect("solve");
        let violation = report
            .violations
            .iter()
            .find(|(id, _)| *id == cid)
            .map(|(_, r)| *r)
            .expect("S-join stays violated after the solve attempt");
        assert!(
            violation > 0.1,
            "the violation must be the curvature-jump class, got {violation}"
        );
    }
}

/// RED (assert, pre-existing surface): `Curvature(κ)` with the
/// `[curve, point]` arity is a real residual — the solver reshapes a
/// free spline until the measured curvature at the point's foot hits
/// the target. Pre-slice the 2-entity arity refused irreducibly.
#[test]
fn red_curvature_value_at_spline_point_solves() {
    let s = fresh("slice7_curvature_value");
    let cps = vec![
        Point2d::new(0.0, 0.0),
        Point2d::new(10.0, 8.0),
        Point2d::new(20.0, 8.0),
        Point2d::new(30.0, 0.0),
    ];
    let knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
    let spline = s.add_bspline(3, cps, knots).expect("spline");
    let apex = s.add_point(Point2d::new(15.0, 6.0));
    fix_point(&s, &apex);

    let target_kappa = -0.08; // right-turning apex under increasing-u traversal
    let cid = s.add_constraint(high_dim(
        DimensionalConstraint::Curvature(target_kappa),
        vec![EntityRef::Spline(spline), EntityRef::Point(apex)],
    ));
    let report = s.solve_constraints().expect("solve");
    assert!(
        !report.violations.iter().any(|(id, _)| *id == cid),
        "Curvature-at-point must be ENFORCED, not refused: {:?}",
        report.violations
    );

    let entry = s.splines().get(&spline).expect("spline");
    let geometry = entry.value().spline.clone();
    drop(entry);
    let apex_pos = s.get_point(&apex).expect("apex");
    let u_foot = foot_parameter(&geometry, &apex_pos);
    let kappa = measured_curvature(&geometry, u_foot);
    assert!(
        (kappa - target_kappa).abs() < 5e-3,
        "measured curvature at the point's foot must hit the target: {kappa} vs {target_kappa}"
    );
}

/// RED (assert, pre-existing surface): `CurvatureExtremum` on
/// `[spline, point]` gains its honest residual (∂κ/∂u = 0 at the
/// point's foot — the Slice-6 documented-refuse whose natural home
/// is this slice). The point slides along a frozen asymmetric arch
/// onto its curvature maximum.
#[test]
fn red_curvature_extremum_places_stationary_curvature() {
    let mut solver = ConstraintSolver::new();
    let spline = EntityRef::Spline(geometry_engine::sketch2d::Spline2dId::new());
    let probe = EntityRef::Point(Point2dId::new());
    // Frozen asymmetric arch: curvature peaks off-center.
    let cps = vec![
        Point2d::new(0.0, 0.0),
        Point2d::new(4.0, 9.0),
        Point2d::new(14.0, 3.0),
        Point2d::new(30.0, 0.0),
    ];
    let knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
    solver.add_entity(
        spline,
        EntityState::spline_bspline(3, cps.clone(), knots.clone(), true),
    );
    // Seed near the arch's bend (the curvature extremum sits at
    // u ≈ 0.182, point ≈ (2.77, 3.53) — verified analytically):
    // Newton solves ∂κ/∂u = 0 in the extremum's basin. A seed out on
    // the straightening tail would chase the asymptotically-flattening
    // κ′ off the end of the curve — basin choice by initial placement,
    // the standing sketch-solver convention.
    solver.add_entity(probe, EntityState::point(Point2d::new(2.5, 4.0), false));

    let on_curve = high_geo(GeometricConstraint::PointOnCurve, vec![probe, spline]);
    let extremum = high_geo(GeometricConstraint::CurvatureExtremum, vec![spline, probe]);
    assert_eq!(
        extremum.degrees_of_freedom_removed(),
        1,
        "[spline, point] CurvatureExtremum removes exactly one DOF"
    );
    solver.set_constraints(vec![on_curve, extremum.clone()]);
    let result = solver.solve();
    let residuals = solver.residuals_by_constraint();
    for (id, r) in &residuals {
        assert!(
            *r < 1e-4,
            "constraint {id} must converge (CurvatureExtremum enforced, not refused): {residuals:?}"
        );
    }

    let geometry = Spline2d::BSpline(
        geometry_engine::sketch2d::BSpline2d::new(3, cps, knots).expect("bspline"),
    );
    let solved = match result.entity_updates.get(&probe) {
        Some(geometry_engine::sketch2d::constraint_solver::EntityUpdate::Point(p)) => *p,
        other => panic!("probe must surface a point update, got {other:?}"),
    };
    let u_foot = foot_parameter(&geometry, &solved);
    let k_here = measured_curvature(&geometry, u_foot).abs();
    let k_left = measured_curvature(&geometry, (u_foot - 0.05).max(0.001)).abs();
    let k_right = measured_curvature(&geometry, (u_foot + 0.05).min(0.999)).abs();
    assert!(
        k_here >= k_left && k_here >= k_right,
        "the probe must land on a local curvature extremum: |κ|({u_foot:.3}) = {k_here:.5} \
         vs neighbours {k_left:.5} / {k_right:.5}"
    );
}

/// Unsupported CurvatureExtremum shapes STILL refuse (per-shape
/// refusal, the Offset/MultiTangent precedent): zero DOF removed +
/// irreducible residual.
#[test]
fn curvature_extremum_unsupported_shape_still_refuses() {
    let s = fresh("slice7_extremum_refuse");
    let a = s.add_point(Point2d::new(0.0, 0.0));
    let b = s.add_point(Point2d::new(1.0, 0.0));
    fix_point(&s, &a);
    fix_point(&s, &b);
    let con = high_geo(
        GeometricConstraint::CurvatureExtremum,
        vec![EntityRef::Point(a), EntityRef::Point(b)],
    );
    assert_eq!(con.degrees_of_freedom_removed(), 0);
    let cid = s.add_constraint(con);
    let report = s.solve_constraints().expect("solve");
    assert!(
        report.violations.iter().any(|(id, _)| *id == cid),
        "point-point CurvatureExtremum must refuse with an irreducible violation"
    );
}

// ── Stage A+B+C gate: the organic leaf outline ──────────────────────

/// Build the leaf fixture: base line A→B on the x-axis, right spline
/// B→T (tip), left spline T→A — both splines shared-CP with the
/// profile vertices as literal endpoint CPs. G2 constraints at all
/// three joins + the dimensions of the hand-count.
fn leaf_sketch() -> (
    Sketch,
    geometry_engine::sketch2d::Spline2dId,
    geometry_engine::sketch2d::Spline2dId,
    Vec<geometry_engine::sketch2d::ConstraintId>,
) {
    let s = fresh("slice7_leaf");
    let a = s.add_point(Point2d::new(0.0, 0.0));
    let b = s.add_point(Point2d::new(40.0, 0.0));
    let t = s.add_point(Point2d::new(20.0, 18.0));
    // Interior CPs seeded NEAR the smooth solution (tangent-parallel
    // at every join by construction; Newton tightens the G2 rows to
    // machine precision inside this basin). 5-CP cubics: the extra
    // interior point per side keeps κ = 0 at the base joins from
    // fighting the ascent to the tip.
    let c1 = s.add_point(Point2d::new(45.0, 0.0));
    let c2 = s.add_point(Point2d::new(46.0, 10.0));
    let c3 = s.add_point(Point2d::new(30.0, 18.0));
    let d1 = s.add_point(Point2d::new(10.0, 18.0));
    let d2 = s.add_point(Point2d::new(-6.0, 10.0));
    let d3 = s.add_point(Point2d::new(-5.0, 0.0));

    let base = s.add_line(a, b).expect("base line");
    let right = s
        .add_bspline_with_control_points(3, &[b, c1, c2, c3, t])
        .expect("right spline");
    let left = s
        .add_bspline_with_control_points(3, &[t, d1, d2, d3, a])
        .expect("left spline");

    // Vertex pins (6 rows).
    for (id, x, y) in [(a, 0.0, 0.0), (b, 40.0, 0.0), (t, 20.0, 18.0)] {
        s.add_constraint(high_dim(
            DimensionalConstraint::XCoordinate(x),
            vec![EntityRef::Point(id)],
        ));
        s.add_constraint(high_dim(
            DimensionalConstraint::YCoordinate(y),
            vec![EntityRef::Point(id)],
        ));
    }
    // G2 joins (3 × 2 rows): line→spline at B, spline→spline at T,
    // spline→line at A (walk order base, right, left).
    let g2_b = s.add_constraint(high_geo(
        GeometricConstraint::CurvatureContinuity,
        vec![EntityRef::Line(base), EntityRef::Spline(right)],
    ));
    let g2_t = s.add_constraint(high_geo(
        GeometricConstraint::CurvatureContinuity,
        vec![EntityRef::Spline(right), EntityRef::Spline(left)],
    ));
    let g2_a = s.add_constraint(high_geo(
        GeometricConstraint::CurvatureContinuity,
        vec![EntityRef::Spline(left), EntityRef::Line(base)],
    ));
    // Leading-CP reach dimensions (2 rows). Hand count: 9 points × 2
    // = 18 free; 6 pins + 6 G2 + 2 distances = 14 removed ⇒ the sketch
    // is HONESTLY UnderConstrained{4} (the organic shape keeps 4
    // styling DOFs) — exactly what the certificate must report.
    s.add_constraint(high_dim(
        DimensionalConstraint::Distance(5.0),
        vec![EntityRef::Point(b), EntityRef::Point(c1)],
    ));
    s.add_constraint(high_dim(
        DimensionalConstraint::Distance(5.0),
        vec![EntityRef::Point(a), EntityRef::Point(d3)],
    ));
    (s, right, left, vec![g2_b, g2_t, g2_a])
}

/// GATE (a): the organic leaf outline — a closed profile of two
/// spline segments joined G2 to a line segment — SOLVES, and the
/// certificate carries measured continuity facts (deviations ≈ 0)
/// with an honest constrainedness verdict.
#[test]
fn gate_leaf_profile_solves_and_certifies_with_continuity_facts() {
    let (s, right, left, g2_ids) = leaf_sketch();

    let report = s.solve_constraints().expect("solve");
    assert!(
        report.violations.is_empty(),
        "the leaf's G2 web must converge violation-free: {:?}",
        report.violations
    );

    let cert = certify_sketch(&s);
    assert!(
        cert.is_sound(),
        "leaf must certify SOUND: {:?}",
        cert.issues
    );
    assert!(
        cert.closed_profile,
        "line + two splines must close: {}",
        cert.profile
    );
    assert!(
        matches!(
            cert.constrainedness,
            geometry_engine::sketch2d::SketchConstrainedness::FullyConstrained
                | geometry_engine::sketch2d::SketchConstrainedness::UnderConstrained { .. }
        ),
        "constrainedness must be honest (fully or under, never a fake verdict): {:?}",
        cert.constrainedness
    );

    // Stage C: continuity facts with MEASURED deviations.
    assert_eq!(
        cert.continuity.len(),
        3,
        "three G2 joins → three continuity facts: {:?}",
        cert.continuity
    );
    for fact in &cert.continuity {
        assert!(
            g2_ids.contains(&fact.constraint),
            "fact must cite its constraint: {fact:?}"
        );
        assert_eq!(fact.kind, ContinuityKind::G2Curvature);
        assert!(fact.measured, "supported joins are MEASURED: {fact:?}");
        let tangent_dev = fact
            .tangent_deviation_rad
            .expect("G2 fact carries tangent deviation");
        let curvature_dev = fact
            .curvature_deviation
            .expect("G2 fact carries curvature deviation");
        assert!(
            tangent_dev < 1e-6,
            "solved join must have ~zero tangent deviation: {fact:?}"
        );
        assert!(
            curvature_dev < 1e-5,
            "solved join must have ~zero curvature deviation: {fact:?}"
        );
        assert!(
            fact.satisfied,
            "solved join fact must be satisfied: {fact:?}"
        );
    }

    // The joins are STRUCTURAL: the splines' endpoint CPs are the
    // shared profile vertices bit-for-bit.
    let right_geo = s
        .splines()
        .get(&right)
        .expect("right")
        .value()
        .spline
        .clone();
    let left_geo = s.splines().get(&left).expect("left").value().spline.clone();
    let tip_from_right = eval(&right_geo, 1.0);
    let tip_from_left = eval(&left_geo, 0.0);
    assert!(
        (tip_from_right.x - tip_from_left.x).abs() < 1e-9
            && (tip_from_right.y - tip_from_left.y).abs() < 1e-9,
        "spline-spline join must be welded: {tip_from_right:?} vs {tip_from_left:?}"
    );
}

/// Stage C honesty: a VIOLATED continuity join reports its measured
/// deviation and `satisfied: false` — the certificate cannot paint a
/// kink as smooth. A G1 fact on a frozen kinked join carries the
/// actual corner angle.
#[test]
fn continuity_fact_reports_violated_join_with_measured_deviation() {
    let s = fresh("slice7_kink_fact");
    let a = s.add_point(Point2d::new(0.0, 0.0));
    let j = s.add_point(Point2d::new(10.0, 0.0));
    let b = s.add_point(Point2d::new(20.0, 10.0)); // 45° kink at j
    fix_point(&s, &a);
    fix_point(&s, &j);
    fix_point(&s, &b);
    let l1 = s.add_line(a, j).expect("l1");
    let l2 = s.add_line(j, b).expect("l2");
    let cid = s.add_constraint(high_geo(
        GeometricConstraint::SmoothTangent,
        vec![EntityRef::Line(l1), EntityRef::Line(l2)],
    ));

    let cert = certify_sketch(&s);
    let fact = cert
        .continuity
        .iter()
        .find(|f| f.constraint == cid)
        .expect("kinked G1 join must produce a continuity fact");
    assert_eq!(fact.kind, ContinuityKind::G1Tangent);
    assert!(fact.measured);
    assert!(!fact.satisfied, "a 45° kink is not smooth: {fact:?}");
    let dev = fact.tangent_deviation_rad.expect("measured angle");
    assert!(
        (dev - std::f64::consts::FRAC_PI_4).abs() < 1e-9,
        "measured tangent deviation must be the actual 45° corner: {dev}"
    );
}

/// Continuity facts stay HONEST for unsupported pairings: the fact is
/// flagged unmeasured (refusal), never fabricated.
#[test]
fn continuity_fact_flags_unsupported_pairing_as_unmeasured() {
    let s = fresh("slice7_unmeasured_fact");
    let a = s.add_point(Point2d::new(0.0, 0.0));
    let b = s.add_point(Point2d::new(1.0, 0.0));
    fix_point(&s, &a);
    fix_point(&s, &b);
    let cid = s.add_constraint(high_geo(
        GeometricConstraint::SmoothTangent,
        vec![EntityRef::Point(a), EntityRef::Point(b)],
    ));
    let cert = certify_sketch(&s);
    let fact = cert
        .continuity
        .iter()
        .find(|f| f.constraint == cid)
        .expect("even a refused join surfaces a fact");
    assert!(
        !fact.measured,
        "point-point G1 cannot be measured: {fact:?}"
    );
    assert!(!fact.satisfied, "unmeasured facts are never satisfied");
    assert!(fact.tangent_deviation_rad.is_none());
    assert!(fact.curvature_deviation.is_none());
}
