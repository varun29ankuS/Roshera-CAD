//! HARNESS GATE: the sketch-validity certificate — the "can't lie" moat extended
//! to 2D sketches. The certificate is only as strong as the defect classes it
//! catches, so this harness constructs a sketch for EACH class and asserts the
//! kernel's verdict:
//!
//!   1. a clean closed profile        → SOUND, closed, under-constrained
//!   2. an open profile               → SOUND but reported open (NOT failed)
//!   3. a self-intersecting profile   → UNSOUND (self_intersection_free == false)
//!   4. mutually-inconsistent dims    → UNSOUND, Conflicting (the core moat)
//!   5. calibration                   → DOF freedom is REPORTED, never failed
//!
//! Principle (cf. verification-comprehensiveness-gap): every missed defect class
//! is a way for the kernel to lie. A failure here is a real finding, not noise.

use geometry_engine::sketch2d::{
    certify_sketch, Constraint, ConstraintPriority, DimensionalConstraint, EntityRef,
    GeometricConstraint, Point2d, Sketch, SketchAnchor, SketchConstrainedness,
};

/// A clean, unconstrained, CLOSED triangle (three points, three shared-endpoint
/// edges). It is geometrically valid and does not self-intersect, so the kernel
/// must certify it SOUND — closed (extrude-ready) and under-constrained (the
/// vertices are still free). DOF freedom is reported, not failed.
#[test]
fn clean_closed_triangle_is_sound_closed_and_under_constrained() {
    let sketch = Sketch::new("triangle".to_string(), SketchAnchor::xy());
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(20.0, 0.0));
    let c = sketch.add_point(Point2d::new(10.0, 15.0));
    sketch.add_line(a, b).expect("edge a-b");
    sketch.add_line(b, c).expect("edge b-c");
    sketch.add_line(c, a).expect("edge c-a");

    let cert = certify_sketch(&sketch);

    assert!(cert.is_sound(), "a clean triangle must be sound: {cert:?}");
    assert!(cert.constraint_consistent, "no constraints, so consistent");
    assert!(
        cert.self_intersection_free,
        "a triangle does not self-intersect"
    );
    assert!(cert.entities_valid, "all three edges are valid");
    assert!(
        cert.closed_profile,
        "three shared-endpoint edges form a CLOSED loop: profile={}",
        cert.profile
    );
    assert!(
        !cert.constrainedness.is_fully_constrained(),
        "an unconstrained triangle is not fully constrained: {:?}",
        cert.constrainedness
    );
}

/// A single open edge. A legal sketch — just not a closed region. The kernel
/// must REPORT it open (closed_profile == false) without calling it unsound:
/// open-ness is an extrude-readiness fact, not a validity defect.
#[test]
fn open_profile_is_reported_not_failed() {
    let sketch = Sketch::new("open".to_string(), SketchAnchor::xy());
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(10.0, 0.0));
    sketch.add_line(a, b).expect("single edge");

    let cert = certify_sketch(&sketch);

    assert!(cert.is_sound(), "an open edge is a valid sketch: {cert:?}");
    assert!(
        !cert.closed_profile,
        "a single edge is NOT a closed profile: {}",
        cert.profile
    );
}

/// A self-intersecting closed polyline (a bow-tie: the diagonals cross). The
/// geometry crosses itself, so the kernel must mark it UNSOUND with
/// `self_intersection_free == false` — a profile that crosses itself cannot be
/// a faithful region boundary.
#[test]
fn self_intersecting_profile_is_unsound() {
    let sketch = Sketch::new("bowtie".to_string(), SketchAnchor::xy());
    // Vertex order (0,0)->(10,10)->(10,0)->(0,10)->close makes the two long
    // edges cross at (5,5): a classic bow-tie self-intersection.
    let _poly = sketch
        .add_polyline(
            vec![
                Point2d::new(0.0, 0.0),
                Point2d::new(10.0, 10.0),
                Point2d::new(10.0, 0.0),
                Point2d::new(0.0, 10.0),
            ],
            true,
        )
        .expect("bow-tie polyline");

    let cert = certify_sketch(&sketch);

    assert!(
        !cert.self_intersection_free,
        "a bow-tie polyline self-intersects — the cert must catch it: {cert:?}"
    );
    assert!(
        !cert.is_sound(),
        "a self-intersecting sketch cannot be sound: {cert:?}"
    );
}

/// THE CORE MOAT: two Distance constraints on the same point pair demand 10 AND
/// 20 simultaneously — mutually inconsistent. The kernel's DOF diagnosis must
/// surface a conflict; the certificate must then be UNSOUND and Conflicting.
/// If this regresses, the kernel is lying about a contradictory sketch.
#[test]
fn mutually_inconsistent_dimensions_are_caught() {
    let sketch = Sketch::new("conflict".to_string(), SketchAnchor::xy());
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(10.0, 0.0));
    sketch.add_line(a, b).expect("edge a-b");

    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::Distance(10.0),
        vec![EntityRef::Point(a), EntityRef::Point(b)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::Distance(20.0),
        vec![EntityRef::Point(a), EntityRef::Point(b)],
        ConstraintPriority::Required,
    ));

    let cert = certify_sketch(&sketch);

    assert!(
        !cert.constraint_consistent,
        "distance=10 AND distance=20 cannot both hold — must be inconsistent: {cert:?}"
    );
    assert!(
        matches!(
            cert.constrainedness,
            SketchConstrainedness::Conflicting { .. }
        ),
        "the verdict must be Conflicting: {:?}",
        cert.constrainedness
    );
    assert!(
        !cert.is_sound(),
        "a sketch with contradictory dimensions is NOT sound: {cert:?}"
    );
}

/// CALIBRATION: soundness must gate ONLY on real defects (inconsistency,
/// degeneracy, self-intersection) — never on DOF freedom. A clean,
/// under-constrained sketch (the common case while drawing) must stay SOUND, or
/// the certificate would false-fail almost every real in-progress sketch.
#[test]
fn soundness_does_not_false_fail_dof_freedom() {
    let sketch = Sketch::new("free".to_string(), SketchAnchor::xy());
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(10.0, 0.0));
    let c = sketch.add_point(Point2d::new(10.0, 10.0));
    let d = sketch.add_point(Point2d::new(0.0, 10.0));
    sketch.add_line(a, b).expect("a-b");
    sketch.add_line(b, c).expect("b-c");
    sketch.add_line(c, d).expect("c-d");
    sketch.add_line(d, a).expect("d-a");

    let cert = certify_sketch(&sketch);

    assert!(
        cert.is_sound(),
        "an unconstrained but clean square must NOT be failed for having free DOFs: {cert:?}"
    );
    assert!(
        matches!(
            cert.constrainedness,
            SketchConstrainedness::UnderConstrained { .. }
        ),
        "an unconstrained square is under-constrained: {:?}",
        cert.constrainedness
    );
}

/// A lone circle: a valid, non-self-intersecting closed entity. The kernel must
/// certify it SOUND — exercises a NON-polyline entity through the cert so the
/// validity path isn't polyline-only.
#[test]
fn circle_is_sound() {
    let sketch = Sketch::new("circle".to_string(), SketchAnchor::xy());
    sketch
        .add_circle(Point2d::new(0.0, 0.0), 5.0)
        .expect("unit circle");

    let cert = certify_sketch(&sketch);

    assert!(cert.is_sound(), "a clean circle must be sound: {cert:?}");
    assert!(
        cert.self_intersection_free,
        "a circle does not self-intersect"
    );
    assert!(cert.entities_valid, "a positive-radius circle is valid");
}

/// CONSISTENT over-specification: the SAME Horizontal constraint twice. The
/// second is REDUNDANT, not conflicting — the system still has a solution. The
/// kernel must keep it `constraint_consistent` and SOUND (redundancy is reported,
/// never failed), distinguishing it from the inconsistent-dimensions case above.
#[test]
fn redundant_constraint_is_consistent_and_sound() {
    let sketch = Sketch::new("redundant".to_string(), SketchAnchor::xy());
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(10.0, 0.0));
    let line = sketch.add_line(a, b).expect("edge a-b");

    // The same geometric fact asserted twice.
    sketch.add_constraint(Constraint::new_geometric(
        GeometricConstraint::Horizontal,
        vec![EntityRef::Line(line)],
        ConstraintPriority::High,
    ));
    sketch.add_constraint(Constraint::new_geometric(
        GeometricConstraint::Horizontal,
        vec![EntityRef::Line(line)],
        ConstraintPriority::High,
    ));

    let cert = certify_sketch(&sketch);

    assert!(
        cert.constraint_consistent,
        "duplicate (consistent) constraints are NOT a conflict: {cert:?}"
    );
    assert!(
        cert.is_sound(),
        "consistent over-specification must stay sound: {cert:?}"
    );
    assert!(
        !cert.constrainedness.is_conflicting(),
        "a duplicate constraint is redundant, not conflicting: {:?}",
        cert.constrainedness
    );
}

// ───────────────────────────────────────────────────────────────────────────
// ADVERSARIAL TIER — deliberately hard cases that stress the validator, the
// solver's conflict diagnosis, and the DOF accounting. Each is a place the
// kernel could quietly lie; passing all of them is the quality bar.
// ───────────────────────────────────────────────────────────────────────────

/// Self-intersection in an OPEN polyline (the closed-only fix must generalise).
/// v0(0,0)->v1(10,0)->v2(5,-5)->v3(5,5): the vertical seg2 crosses the
/// horizontal seg0 at (5,0). seg0 and seg2 are non-adjacent.
#[test]
fn adversarial_open_polyline_self_intersection_is_caught() {
    let sketch = Sketch::new("open-x".to_string(), SketchAnchor::xy());
    sketch
        .add_polyline(
            vec![
                Point2d::new(0.0, 0.0),
                Point2d::new(10.0, 0.0),
                Point2d::new(5.0, -5.0),
                Point2d::new(5.0, 5.0),
            ],
            false,
        )
        .expect("open self-crossing polyline");
    let cert = certify_sketch(&sketch);
    assert!(
        !cert.self_intersection_free,
        "an OPEN polyline that crosses itself must be caught: {cert:?}"
    );
    assert!(!cert.is_sound(), "a self-crossing open profile is unsound");
}

/// NEAR-MISS: two segments that come close but never cross must NOT be flagged.
/// A false positive here would make the cert lie the other way (calling clean
/// sketches unsound). v0(0,0)->v1(10,0)->v2(0,1)->v3(10,1): two parallel
/// horizontals one unit apart — no crossing.
#[test]
fn adversarial_near_miss_is_not_false_flagged() {
    let sketch = Sketch::new("near-miss".to_string(), SketchAnchor::xy());
    sketch
        .add_polyline(
            vec![
                Point2d::new(0.0, 0.0),
                Point2d::new(10.0, 0.0),
                Point2d::new(0.0, 1.0),
                Point2d::new(10.0, 1.0),
            ],
            false,
        )
        .expect("near-miss polyline");
    let cert = certify_sketch(&sketch);
    assert!(
        cert.self_intersection_free,
        "non-crossing segments must NOT be flagged as self-intersecting: {cert:?}"
    );
    assert!(cert.is_sound(), "a non-self-crossing sketch is sound");
}

/// Self-intersection buried in the MIDDLE of a long polyline (not first/last).
/// 6 vertices; seg1 (v1-v2) crosses seg4 (v4-v5).
#[test]
fn adversarial_mid_polyline_self_intersection_is_caught() {
    let sketch = Sketch::new("long-x".to_string(), SketchAnchor::xy());
    sketch
        .add_polyline(
            vec![
                Point2d::new(0.0, 0.0),  // v0
                Point2d::new(2.0, 0.0),  // v1
                Point2d::new(2.0, 10.0), // v2  (seg1 = v1->v2, vertical x=2)
                Point2d::new(8.0, 10.0), // v3
                Point2d::new(8.0, 5.0),  // v4
                Point2d::new(0.0, 5.0),  // v5  (seg4 = v4->v5, horizontal y=5 crosses x=2)
            ],
            false,
        )
        .expect("long self-crossing polyline");
    let cert = certify_sketch(&sketch);
    assert!(
        !cert.self_intersection_free,
        "a crossing deep in a long polyline must be caught: {cert:?}"
    );
}

/// A valid convex pentagon (closed) must NOT be false-flagged — calibration that
/// the all-pairs scan stays correct on a larger clean loop.
#[test]
fn adversarial_convex_pentagon_is_clean() {
    let sketch = Sketch::new("pentagon".to_string(), SketchAnchor::xy());
    sketch
        .add_polyline(
            vec![
                Point2d::new(0.0, 0.0),
                Point2d::new(10.0, 0.0),
                Point2d::new(13.0, 8.0),
                Point2d::new(5.0, 14.0),
                Point2d::new(-3.0, 8.0),
            ],
            true,
        )
        .expect("convex pentagon");
    let cert = certify_sketch(&sketch);
    assert!(
        cert.self_intersection_free,
        "a convex pentagon does not self-intersect: {cert:?}"
    );
    assert!(cert.is_sound(), "a convex pentagon is sound");
}

/// GEOMETRIC conflict: two lines forced both Parallel AND Perpendicular. There
/// is no angle satisfying both — the solver's diagnosis must surface a conflict
/// (this exercises the GEOMETRIC conflict path, distinct from the dimensional
/// distance conflict above).
#[test]
fn adversarial_parallel_and_perpendicular_conflict() {
    let sketch = Sketch::new("para-perp".to_string(), SketchAnchor::xy());
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(10.0, 0.0));
    let c = sketch.add_point(Point2d::new(0.0, 5.0));
    let d = sketch.add_point(Point2d::new(10.0, 5.0));
    let l1 = sketch.add_line(a, b).expect("line 1");
    let l2 = sketch.add_line(c, d).expect("line 2");
    sketch.add_constraint(Constraint::new_geometric(
        GeometricConstraint::Parallel,
        vec![EntityRef::Line(l1), EntityRef::Line(l2)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_geometric(
        GeometricConstraint::Perpendicular,
        vec![EntityRef::Line(l1), EntityRef::Line(l2)],
        ConstraintPriority::Required,
    ));
    let cert = certify_sketch(&sketch);
    assert!(
        !cert.constraint_consistent,
        "parallel AND perpendicular is unsatisfiable — must be inconsistent: {cert:?}"
    );
    assert!(!cert.is_sound(), "a parallel+perpendicular pair is unsound");
}

/// MIXED conflict: Coincident says the two points are the same; Distance says
/// they are 10 apart. Contradiction. (This is the numerically-degenerate case —
/// the distance gradient at coincident points is delicate — which is exactly
/// why it belongs in the adversarial tier.)
#[test]
fn adversarial_coincident_and_distance_conflict() {
    let sketch = Sketch::new("coinc-dist".to_string(), SketchAnchor::xy());
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(10.0, 0.0));
    sketch.add_constraint(Constraint::new_geometric(
        GeometricConstraint::Coincident,
        vec![EntityRef::Point(a), EntityRef::Point(b)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::Distance(10.0),
        vec![EntityRef::Point(a), EntityRef::Point(b)],
        ConstraintPriority::Required,
    ));
    let cert = certify_sketch(&sketch);
    assert!(
        !cert.is_sound(),
        "coincident AND distance=10 is contradictory — must be unsound: {cert:?}"
    );
}

/// DOF PRECISION: a fully-determined line — point a pinned by X/Y coordinate,
/// the edge length fixed, the edge horizontal — leaves zero free DOF. The
/// verdict must be exactly FullyConstrained (not under, not over).
#[test]
fn adversarial_fully_constrained_sketch_reports_fully_constrained() {
    let sketch = Sketch::new("fully".to_string(), SketchAnchor::xy());
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(10.0, 0.0));
    let line = sketch.add_line(a, b).expect("edge");
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::XCoordinate(0.0),
        vec![EntityRef::Point(a)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::YCoordinate(0.0),
        vec![EntityRef::Point(a)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::Distance(10.0),
        vec![EntityRef::Point(a), EntityRef::Point(b)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_geometric(
        GeometricConstraint::Horizontal,
        vec![EntityRef::Line(line)],
        ConstraintPriority::Required,
    ));
    let cert = certify_sketch(&sketch);
    assert!(
        cert.constraint_consistent,
        "a fully-determined line is consistent: {cert:?}"
    );
    assert!(
        cert.constrainedness.is_fully_constrained(),
        "pinned point + length + horizontal leaves zero DOF: {:?}",
        cert.constrainedness
    );
    assert!(cert.is_sound(), "a fully-constrained valid sketch is sound");
}

/// DETERMINISM: certifying the same sketch twice must yield an identical verdict.
/// A flaky certificate is a non-deterministic solver/diagnosis — a bug class in
/// its own right (cf. the poke-matrix determinism lesson).
#[test]
fn adversarial_certify_is_deterministic() {
    let sketch = Sketch::new("determinism".to_string(), SketchAnchor::xy());
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(10.0, 0.0));
    let c = sketch.add_point(Point2d::new(5.0, 8.0));
    sketch.add_line(a, b).expect("a-b");
    sketch.add_line(b, c).expect("b-c");
    sketch.add_line(c, a).expect("c-a");
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::Distance(10.0),
        vec![EntityRef::Point(a), EntityRef::Point(b)],
        ConstraintPriority::Required,
    ));

    let c1 = certify_sketch(&sketch);
    let c2 = certify_sketch(&sketch);
    assert_eq!(
        c1.is_sound(),
        c2.is_sound(),
        "soundness must be deterministic"
    );
    assert_eq!(
        c1.constrainedness, c2.constrainedness,
        "the DOF verdict must be deterministic: {:?} vs {:?}",
        c1.constrainedness, c2.constrainedness
    );
    assert_eq!(
        c1.closed_profile, c2.closed_profile,
        "closed-profile detection must be deterministic"
    );
}

/// ROBUSTNESS: an empty sketch must certify without panicking and is vacuously
/// sound (no entities to be invalid, no constraints to conflict).
#[test]
fn adversarial_empty_sketch_certifies_without_panic() {
    let sketch = Sketch::new("empty".to_string(), SketchAnchor::xy());
    let cert = certify_sketch(&sketch);
    assert!(
        cert.is_sound(),
        "an empty sketch is vacuously sound: {cert:?}"
    );
    assert!(cert.constraint_consistent, "no constraints, no conflict");
    assert!(cert.entities_valid, "no entities, none invalid");
}
