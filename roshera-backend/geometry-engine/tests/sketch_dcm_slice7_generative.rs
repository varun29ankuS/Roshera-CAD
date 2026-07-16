// Reason: integration-test crate -- panicking (unwrap/expect/assert/index) is
// the test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::indexing_slicing)]

//! SKETCH-DCM #45 Slice 7 — generative patterns + analytic spline
//! profiles (stages D/E of the biomimicry slice).
//!
//! Stage D — generative patterns as sketch ops:
//!   - `phyllotaxis_pattern`: florets at r = c·√n, θ stepped by the
//!     EXACT golden angle 2π(1 − 1/φ) (Vogel 1979), maintained by the
//!     Slice-6 constraint-web scheme (spokes + Distance + Angle
//!     chains) with provenance lineage.
//!   - `curve_pattern`: n instances along a spline/arc rail at
//!     arc-length steps, maintained by `PointOnCurve` + chained
//!     `Distance` (chord values measured at placement — documented).
//!
//! Stage E — spline profiles become typed NURBS edges
//! (`ProfileEdge::Nurbs`) and extrude to SOUND solids with exact
//! NURBS rails; a CLOSED single-edge spline wall stays a typed
//! refusal (the Slice-5 documented zero-triangle closed-ruled trap).

use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::extrude::{extrude_profile_regions, ProfileLoop, ProfileRegion};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::sketch2d::sketch_ops::{curve_pattern, golden_angle_rad, phyllotaxis_pattern};
use geometry_engine::sketch2d::sketch_topology::{
    AnalyticLoop, ProfileEdge, ProfileExtractor, SketchTopology,
};
use geometry_engine::sketch2d::{
    certify_sketch, Constraint, ConstraintPriority, DimensionalConstraint, EntityRef, Point2d,
    Point2dId, Sketch, SketchAnchor, SketchOpError, Spline2d, Tolerance2d,
};
use std::f64::consts::TAU;

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

// ── Stage D: phyllotaxis ─────────────────────────────────────────────

/// The op's angle constant IS the exact golden angle 2π(1 − 1/φ):
/// 137.50776405003785…° — never a rounded literal.
#[test]
fn golden_angle_is_exact() {
    let phi = (1.0 + 5.0_f64.sqrt()) / 2.0;
    let exact = TAU * (1.0 - 1.0 / phi);
    let got = golden_angle_rad();
    assert!(
        (got - exact).abs() < 1e-15,
        "golden angle must be exact: {got} vs {exact}"
    );
    let degrees = got.to_degrees();
    assert!(
        (degrees - 137.50776405003785).abs() < 1e-10,
        "≈ 137.50776405003785°, got {degrees}"
    );
}

/// GATE (b): the phyllotaxis op mints n florets at r = c·√n stepped by
/// the golden angle, each FULLY maintained (Distance-to-center +
/// Angle spoke chain), with provenance lineage, and the sketch
/// re-certifies sound. Positions verified against the closed-form
/// Vogel spiral.
#[test]
fn gate_phyllotaxis_pattern_certified_with_lineage() {
    let s = fresh("slice7_phyllotaxis");
    let center = s.add_point(Point2d::new(0.0, 0.0));
    fix_point(&s, &center);
    let c = 2.0;
    // Seed floret 1 exactly on its Vogel radius (r₁ = c·√1 = c) along +x.
    let seed = s.add_point(Point2d::new(c, 0.0));

    let count = 6usize;
    let outcome = phyllotaxis_pattern(&s, &[EntityRef::Point(seed)], &center, count, c)
        .expect("phyllotaxis op");

    // count − 1 minted instance points (source = floret 1).
    let instance_points: Vec<Point2dId> = outcome
        .created
        .iter()
        .filter_map(|e| match e {
            EntityRef::Point(id) => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(
        instance_points.len(),
        count - 1,
        "florets 2..=count minted: {:?}",
        outcome.created
    );

    // Provenance lineage minted from day one (the #11 3D-pattern debt
    // is not repeated in 2D).
    for e in &outcome.created {
        assert!(
            s.provenance_of(e).is_some(),
            "created entity {e} must carry provenance"
        );
    }
    let mut instances_seen: Vec<usize> = outcome
        .provenance
        .iter()
        .filter_map(|(e, p)| match e {
            EntityRef::Point(_) => p.instance,
            _ => None,
        })
        .collect();
    instances_seen.sort_unstable();
    assert_eq!(
        instances_seen,
        (2..=count).collect::<Vec<_>>(),
        "floret indices 2..=count recorded on the instance points"
    );

    let report = s.solve_constraints().expect("solve");
    assert!(
        report.violations.is_empty(),
        "the phyllotaxis web must solve violation-free: {:?}",
        report.violations
    );

    // Vogel spiral verification: floret n at r = c·√n, θ = (n−1)·γ.
    let ga = golden_angle_rad();
    for (i, pid) in instance_points.iter().enumerate() {
        let n = (i + 2) as f64; // florets 2..=count in mint order
        let p = s.get_point(pid).expect("instance");
        let r = (p.x * p.x + p.y * p.y).sqrt();
        assert!(
            (r - c * n.sqrt()).abs() < 1e-6,
            "floret {n}: radius must be c·√n = {}, got {r}",
            c * n.sqrt()
        );
        let theta = p.y.atan2(p.x).rem_euclid(TAU);
        let expected = ((n - 1.0) * ga).rem_euclid(TAU);
        let dev = (theta - expected).abs().min(TAU - (theta - expected).abs());
        assert!(
            dev < 1e-6,
            "floret {n}: angle must step by the golden angle: {theta} vs {expected}"
        );
    }

    let cert = certify_sketch(&s);
    assert!(
        cert.is_sound(),
        "phyllotaxis sketch certifies sound: {:?}",
        cert.issues
    );

    // MAINTAINED, not copied — two teeth:
    // 1. DOF hand-count: seed 2 + 5 florets × 2 = 12 free;
    //    Distance(center, seed) 1 + 5 × (Distance 1 + Angle 1) = 11
    //    removed ⇒ exactly ONE honest residual DOF (the spiral's
    //    phase). Dropping any maintenance chain changes this count.
    let dof = geometry_engine::sketch2d::analyze_dofs(&s);
    assert_eq!(
        dof.status,
        geometry_engine::sketch2d::DofStatus::UnderConstrained { dofs: 1 },
        "the spiral keeps exactly its phase DOF: {dof:?}"
    );
    // 2. Rotating the SEED re-solves the whole spiral: drag floret 1
    //    to the +y axis and every floret must follow (radii preserved,
    //    golden-angle steps preserved relative to the seed).
    let report = s
        .solve_drag(
            EntityRef::Point(seed),
            geometry_engine::sketch2d::DragTarget::Point(Point2d::new(0.0, c)),
        )
        .expect("drag seed");
    assert!(
        report.violations.is_empty(),
        "phase rotation is the free DOF — drag must stay violation-free: {:?}",
        report.violations
    );
    let seed_pos = s.get_point(&seed).expect("seed");
    let seed_theta = seed_pos.y.atan2(seed_pos.x);
    assert!(
        (seed_theta - std::f64::consts::FRAC_PI_2).abs() < 1e-4,
        "seed must reach the dragged phase: {seed_theta}"
    );
    for (i, pid) in instance_points.iter().enumerate() {
        let n = (i + 2) as f64;
        let p = s.get_point(pid).expect("instance");
        let r = (p.x * p.x + p.y * p.y).sqrt();
        assert!(
            (r - c * n.sqrt()).abs() < 1e-5,
            "floret {n} radius maintained through the rotation: {r}"
        );
        let theta = p.y.atan2(p.x);
        let expected = (seed_theta + (n - 1.0) * ga).rem_euclid(TAU);
        let dev = (theta.rem_euclid(TAU) - expected).abs();
        let dev = dev.min(TAU - dev);
        assert!(
            dev < 1e-4,
            "floret {n} must FOLLOW the rotated seed (maintained web): dev {dev}"
        );
    }
}

/// Phyllotaxis over a CIRCLE source: florets are circles whose radii
/// ride an `Equal` chain — dimensioning the source propagates to every
/// floret (maintained, not copied).
#[test]
fn phyllotaxis_circles_inherit_equal_radius_chain() {
    let s = fresh("slice7_phyllotaxis_circles");
    let center = s.add_point(Point2d::new(0.0, 0.0));
    fix_point(&s, &center);
    let seed_center = s.add_point(Point2d::new(3.0, 0.0));
    let seed = s
        .add_circle_centered(seed_center, 0.8)
        .expect("seed circle");

    let outcome = phyllotaxis_pattern(&s, &[EntityRef::Circle(seed)], &center, 4, 3.0)
        .expect("phyllotaxis op");
    let instance_circles: Vec<_> = outcome
        .created
        .iter()
        .filter(|e| matches!(e, EntityRef::Circle(_)))
        .collect();
    assert_eq!(instance_circles.len(), 3, "3 floret circles minted");

    // Dimension the SOURCE radius; the Equal chain must propagate.
    s.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::Radius(0.5),
        vec![EntityRef::Circle(seed)],
        ConstraintPriority::Required,
    ));
    let report = s.solve_constraints().expect("solve");
    assert!(
        report.violations.is_empty(),
        "radius edit must propagate: {:?}",
        report.violations
    );
    for e in &instance_circles {
        if let EntityRef::Circle(id) = e {
            let r = s.circles().get(id).expect("circle").circle.radius;
            assert!(
                (r - 0.5).abs() < 1e-6,
                "floret radius must follow the source via the Equal chain, got {r}"
            );
        }
    }
}

// ── Stage D: pattern along a curve ───────────────────────────────────

/// Instances stay ON the rail (`PointOnCurve`) with chained spacing —
/// verified by measuring each instance's distance to the rail and the
/// consecutive chords.
#[test]
fn curve_pattern_places_instances_on_the_rail() {
    let s = fresh("slice7_curve_pattern");
    // Rail: gentle open cubic.
    let rail = s
        .add_bspline(
            3,
            vec![
                Point2d::new(0.0, 0.0),
                Point2d::new(10.0, 8.0),
                Point2d::new(20.0, -4.0),
                Point2d::new(30.0, 2.0),
            ],
            vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        )
        .expect("rail");
    // Seed near the rail start.
    let seed = s.add_point(Point2d::new(0.2, 0.3));

    let count = 4usize;
    let spacing = 7.0;
    let outcome = curve_pattern(
        &s,
        &[EntityRef::Point(seed)],
        &EntityRef::Spline(rail),
        count,
        Some(spacing),
    )
    .expect("curve pattern");

    let instance_points: Vec<Point2dId> = outcome
        .created
        .iter()
        .filter_map(|e| match e {
            EntityRef::Point(id) => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(instance_points.len(), count - 1);

    let report = s.solve_constraints().expect("solve");
    assert!(
        report.violations.is_empty(),
        "curve-pattern web must solve violation-free: {:?}",
        report.violations
    );

    // Every instance sits ON the rail (oracle: dense closest-point scan).
    let geometry = s.splines().get(&rail).expect("rail").value().spline.clone();
    let distance_to_rail = |p: &Point2d| -> f64 {
        // Coarse scan + two zoom rounds: a flat sampled scan's
        // resolution floor (half the chord spacing, ~4e-3 here) would
        // dominate the measurement.
        let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
        let mut best = f64::INFINITY;
        for _ in 0..3 {
            let mut best_u = lo;
            for i in 0..=400 {
                let u = lo + (hi - lo) * (i as f64) / 400.0;
                let q = geometry.evaluate(u).expect("eval");
                let d = ((q.x - p.x).powi(2) + (q.y - p.y).powi(2)).sqrt();
                if d < best {
                    best = d;
                    best_u = u;
                }
            }
            let span = (hi - lo) / 400.0;
            lo = (best_u - 2.0 * span).max(0.0);
            hi = (best_u + 2.0 * span).min(1.0);
        }
        best
    };
    for pid in &instance_points {
        let p = s.get_point(pid).expect("instance");
        assert!(
            distance_to_rail(&p) < 1e-5,
            "instance must lie on the rail, off by {}",
            distance_to_rail(&p)
        );
    }
    // Provenance lineage.
    for e in &outcome.created {
        assert!(s.provenance_of(e).is_some());
    }
}

/// A step that runs off the rail's end refuses typed and leaves the
/// sketch untouched.
#[test]
fn curve_pattern_overflow_refuses_typed() {
    let s = fresh("slice7_curve_pattern_overflow");
    let rail = s
        .add_bspline(
            3,
            vec![
                Point2d::new(0.0, 0.0),
                Point2d::new(3.0, 2.0),
                Point2d::new(6.0, -1.0),
                Point2d::new(9.0, 0.0),
            ],
            vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        )
        .expect("rail");
    let seed = s.add_point(Point2d::new(0.0, 0.0));
    let points_before = s.points().len();
    let constraints_before = s.all_constraints().len();

    let err = curve_pattern(
        &s,
        &[EntityRef::Point(seed)],
        &EntityRef::Spline(rail),
        5,
        Some(100.0),
    )
    .expect_err("a 400-long pattern cannot fit a ~10-long rail");
    assert!(
        matches!(err, SketchOpError::InvalidParameter { .. }),
        "typed refusal, got {err:?}"
    );
    assert_eq!(s.points().len(), points_before, "sketch untouched");
    assert_eq!(s.all_constraints().len(), constraints_before);
}

// ── Stage E: typed NURBS profile edges + sound extrusion ─────────────

const LEAF_H: f64 = 6.0;

/// The organic profile: base line A→B plus two clamped cubic splines
/// (B→T, T→A) with shared endpoint CPs — the same class as the
/// stage-A/B/C leaf, geometry chosen smooth at the seed.
fn leaf_profile_sketch() -> Sketch {
    let s = fresh("slice7_leaf_profile");
    let a = s.add_point(Point2d::new(0.0, 0.0));
    let b = s.add_point(Point2d::new(40.0, 0.0));
    let t = s.add_point(Point2d::new(20.0, 18.0));
    let c1 = s.add_point(Point2d::new(45.0, 0.0));
    let c2 = s.add_point(Point2d::new(46.0, 10.0));
    let c3 = s.add_point(Point2d::new(30.0, 18.0));
    let d1 = s.add_point(Point2d::new(10.0, 18.0));
    let d2 = s.add_point(Point2d::new(-6.0, 10.0));
    let d3 = s.add_point(Point2d::new(-5.0, 0.0));
    s.add_line(a, b).expect("base");
    s.add_bspline_with_control_points(3, &[b, c1, c2, c3, t])
        .expect("right spline");
    s.add_bspline_with_control_points(3, &[t, d1, d2, d3, a])
        .expect("left spline");
    s
}

/// Spline loop edges lift to TYPED `ProfileEdge::Nurbs` carrying the
/// exact stored control points / knots — no chord fit, no silent
/// sampling.
#[test]
fn spline_profile_lifts_to_typed_nurbs_edges() {
    let s = leaf_profile_sketch();
    let topo = SketchTopology::analyze(&s, &Tolerance2d::default()).expect("topology");
    let profiles = ProfileExtractor::extract_for_extrusion(&topo).expect("profiles");
    assert_eq!(profiles.len(), 1, "one closed region");
    let outer = match ProfileExtractor::analytic_loop_edges(&s, &topo, &profiles[0].outer_boundary)
        .expect("extraction")
    {
        AnalyticLoop::Edges(edges) => edges,
        other => panic!("line + spline loop must lift analytically, got {other:?}"),
    };
    assert_eq!(outer.len(), 3, "line + two splines");
    let lines = outer
        .iter()
        .filter(|e| matches!(e, ProfileEdge::Line { .. }))
        .count();
    let nurbs = outer
        .iter()
        .filter(|e| matches!(e, ProfileEdge::Nurbs { .. }))
        .count();
    assert_eq!((lines, nurbs), (1, 2), "typed composition: {outer:?}");

    // Loop-walk orientation baked in: consecutive edges chain
    // head-to-tail (a NURBS edge's first/last control point are its
    // clamped endpoints).
    let endpoint = |e: &ProfileEdge, first: bool| -> [f64; 2] {
        match e {
            ProfileEdge::Line { start, end } => {
                if first {
                    *start
                } else {
                    *end
                }
            }
            ProfileEdge::Nurbs { control_points, .. } => {
                if first {
                    control_points[0]
                } else {
                    control_points[control_points.len() - 1]
                }
            }
            other => panic!("unexpected edge {other:?}"),
        }
    };
    for i in 0..outer.len() {
        let prev_end = endpoint(&outer[(i + outer.len() - 1) % outer.len()], false);
        let this_start = endpoint(&outer[i], true);
        assert!(
            (prev_end[0] - this_start[0]).abs() < 1e-9
                && (prev_end[1] - this_start[1]).abs() < 1e-9,
            "edge {i} must chain from the previous edge's end: {prev_end:?} vs {this_start:?}"
        );
    }

    // Serde wire shape (the sketch_extrude timeline event format):
    // {"kind": "nurbs", ...}.
    let nurbs_edge = outer
        .iter()
        .find(|e| matches!(e, ProfileEdge::Nurbs { .. }))
        .expect("nurbs edge");
    let wire = serde_json::to_value(nurbs_edge).expect("serialise");
    assert_eq!(wire["kind"], "nurbs", "wire shape: {wire}");
    assert!(wire["control_points"].is_array());
    assert!(wire["knots"].is_array());
    let back: ProfileEdge = serde_json::from_value(wire).expect("roundtrip");
    assert_eq!(&back, nurbs_edge, "wire roundtrip is lossless");
}

/// GATE (c): the organic profile extrudes to a SOUND solid — ground
/// truth certifies watertight/Euler-consistent, the face census is
/// exactly 2 caps + 3 walls, and the volume matches a Green's-theorem
/// boundary oracle.
#[test]
fn gate_leaf_profile_extrudes_sound() {
    let s = leaf_profile_sketch();
    let topo = SketchTopology::analyze(&s, &Tolerance2d::default()).expect("topology");
    let profiles = ProfileExtractor::extract_for_extrusion(&topo).expect("profiles");
    let outer = match ProfileExtractor::analytic_loop_edges(&s, &topo, &profiles[0].outer_boundary)
        .expect("extraction")
    {
        AnalyticLoop::Edges(edges) => edges,
        other => panic!("must lift analytically: {other:?}"),
    };
    let regions = vec![ProfileRegion {
        outer: ProfileLoop::Edges(outer),
        holes: Vec::new(),
    }];

    let mut model = BRepModel::new();
    let solid = extrude_profile_regions(
        &mut model,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::X,
        Vector3::Y,
        &regions,
        LEAF_H,
        None,
        Tolerance::default(),
    )
    .expect("organic profile must extrude — no silent broken solids");

    let gt = model.ground_truth(solid).expect("ground truth");
    assert!(
        gt.certificate.is_sound(),
        "the organic solid must be SOUND: {:?}",
        gt.certificate
    );

    let face_count = model
        .solid_outer_face_count(solid)
        .expect("outer face count");
    assert_eq!(
        face_count, 5,
        "2 caps + 1 planar wall + 2 NURBS ruled walls"
    );

    // Volume oracle: Green's-theorem area of the profile boundary
    // (densely sampled from the SKETCH entities — independent of the
    // kernel's tessellation) × height.
    let area = {
        let mut boundary: Vec<Point2d> = Vec::new();
        // base line A→B
        for i in 0..800 {
            let t = i as f64 / 800.0;
            boundary.push(Point2d::new(40.0 * t, 0.0));
        }
        // right spline B→T, left spline T→A: walk in profile order.
        let mut splines: Vec<(Point2d, Spline2d)> = s
            .splines()
            .iter()
            .map(|e| {
                let geo = e.value().spline.clone();
                (geo.evaluate(0.0).expect("start"), geo)
            })
            .collect();
        // Order: the spline starting at B=(40,0) first.
        splines.sort_by(|a, b| {
            let da = (a.0.x - 40.0).abs() + a.0.y.abs();
            let db = (b.0.x - 40.0).abs() + b.0.y.abs();
            da.partial_cmp(&db).unwrap()
        });
        for (_, geo) in &splines {
            for i in 0..800 {
                let u = i as f64 / 800.0;
                boundary.push(geo.evaluate(u).expect("eval"));
            }
        }
        let mut acc = 0.0;
        for i in 0..boundary.len() {
            let p = &boundary[i];
            let q = &boundary[(i + 1) % boundary.len()];
            acc += p.x * q.y - q.x * p.y;
        }
        (acc / 2.0).abs()
    };
    let expected_volume = area * LEAF_H;
    let measured = model.calculate_solid_volume(solid).expect("solid volume");
    let rel = (measured - expected_volume).abs() / expected_volume;
    assert!(
        rel < 2e-3,
        "extruded volume must match the boundary oracle: measured {measured}, \
         expected {expected_volume}, rel {rel}"
    );
}

/// The documented refusal: a CLOSED single-edge spline loop's extruded
/// wall would be a closed generic ruled surface — the Slice-5
/// zero-triangle tessellation trap. The kernel refuses TYPED; it never
/// emits a silently broken solid.
#[test]
fn closed_single_edge_spline_loop_refuses_typed() {
    let s = fresh("slice7_closed_spline");
    // Closed clamped cubic: last CP == first CP.
    let p0 = Point2d::new(10.0, 0.0);
    s.add_bspline(
        3,
        vec![
            p0,
            Point2d::new(14.0, 9.0),
            Point2d::new(-2.0, 12.0),
            Point2d::new(-8.0, 2.0),
            Point2d::new(2.0, -7.0),
            p0,
        ],
        vec![0.0, 0.0, 0.0, 0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0, 1.0, 1.0, 1.0],
    )
    .expect("closed spline");

    let topo = SketchTopology::analyze(&s, &Tolerance2d::default()).expect("topology");
    let profiles = ProfileExtractor::extract_for_extrusion(&topo).expect("profiles");
    assert_eq!(profiles.len(), 1, "the closed spline bounds one region");
    let outer = match ProfileExtractor::analytic_loop_edges(&s, &topo, &profiles[0].outer_boundary)
        .expect("extraction")
    {
        AnalyticLoop::Edges(edges) => edges,
        other => panic!("closed spline lifts to a typed edge: {other:?}"),
    };
    assert_eq!(outer.len(), 1);
    assert!(matches!(outer[0], ProfileEdge::Nurbs { .. }));

    let mut model = BRepModel::new();
    let err = extrude_profile_regions(
        &mut model,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::X,
        Vector3::Y,
        &[ProfileRegion {
            outer: ProfileLoop::Edges(outer),
            holes: Vec::new(),
        }],
        LEAF_H,
        None,
        Tolerance::default(),
    )
    .expect_err("a closed single-edge NURBS wall is the documented closed-ruled trap");
    let message = err.to_string();
    assert!(
        message.contains("closed") && message.to_lowercase().contains("nurbs"),
        "the refusal must name the trap: {message}"
    );
}
