// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! SKETCH-DCM campaign #45, Slice 4 — certificate v2: the certified-sketch
//! surface (spec §3.2 / §3.5 Slice 4).
//!
//! RED contract (written before the implementation):
//!
//! 1. **Conflict witness** — a sketch carrying two mutually-exclusive
//!    `Distance` constraints on the same point pair, among otherwise
//!    satisfiable constraints, must yield a witness naming EXACTLY that
//!    pair (QuickXplain-minimal, Junker 2004), with per-constraint
//!    residuals, flagged `minimal == true`. Mutation proof: making the
//!    minimiser return the full candidate set while keeping the flag
//!    must fail the exact-pair assertion.
//! 2. **Per-entity constrainment** — a half-constrained slot (two of
//!    four points pinned, one of two arc radii dimensioned) must report
//!    the exact hand-counted per-entity split: pinned points + the arc
//!    and line whose parents are placed = fully constrained; the loose
//!    points carry 2 free DOFs each, the undimensioned arc 1, the
//!    derived entities on loose parents 0 (they move with their
//!    parents but own no private DOF).
//! 3. **Cluster localisation** — a component the DR-planner solves via
//!    a `PlaceCluster` step must label exactly the cluster's entities
//!    with that cluster id.
//! 4. **Honesty for refuse-kinds** — an honest-refuse constraint
//!    (`MomentOfInertia`) must surface as a minimal singleton witness,
//!    not vanish into a generic "diverged".

#![allow(clippy::float_cmp)]
// Reason for `#![allow(clippy::expect_used)]` / `unwrap_used` /
// `panic` — test-only file: failing loudly at the fixture site is the
// desired failure mode; the workspace deny lints target production
// code.
#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]

use geometry_engine::sketch2d::constraints::{
    Constraint, ConstraintId, ConstraintPriority, DimensionalConstraint, EntityRef,
    GeometricConstraint,
};
use geometry_engine::sketch2d::sketch::{Sketch, SketchAnchor};
use geometry_engine::sketch2d::sketch_certificate::{
    certify_sketch, EntityConstrainment, SketchConstrainedness, SolverVerdict, WitnessKind,
};
use geometry_engine::sketch2d::{Point2d, Point2dId};

fn fresh(name: &str) -> Sketch {
    Sketch::new(name.to_string(), SketchAnchor::xy())
}

fn required_dim(dc: DimensionalConstraint, entities: Vec<EntityRef>) -> Constraint {
    Constraint::new_dimensional(dc, entities, ConstraintPriority::Required)
}

/// Pin a point with Required X + Y coordinate dimensions.
fn anchor(sketch: &Sketch, p: Point2dId, x: f64, y: f64) {
    sketch.add_constraint(required_dim(
        DimensionalConstraint::XCoordinate(x),
        vec![EntityRef::Point(p)],
    ));
    sketch.add_constraint(required_dim(
        DimensionalConstraint::YCoordinate(y),
        vec![EntityRef::Point(p)],
    ));
}

fn sorted_ids(mut ids: Vec<ConstraintId>) -> Vec<ConstraintId> {
    ids.sort_by_key(|id| id.0);
    ids
}

// ── RED 1: minimal conflict witness (QuickXplain) ───────────────────

/// Fixture: fixed anchor at the origin; p1 pinned to the X axis and
/// pulled by TWO contradictory Distance dimensions (10 and 20) to the
/// anchor; a bystander point p2 that is fully and satisfiably pinned
/// in its own component. The only minimal inconsistent subset is the
/// distance pair — each constraint alone (and each pairing with the
/// Y pin) is satisfiable.
fn conflicting_pair_sketch() -> (Sketch, ConstraintId, ConstraintId, Point2dId, Point2dId) {
    let sketch = fresh("slice4_conflict_pair");
    let anchor_pt = sketch.add_point(Point2d::new(0.0, 0.0));
    sketch
        .points()
        .get_mut(&anchor_pt)
        .expect("anchor present")
        .value_mut()
        .fix();
    let p1 = sketch.add_point(Point2d::new(12.0, 0.0));
    sketch.add_constraint(required_dim(
        DimensionalConstraint::YCoordinate(0.0),
        vec![EntityRef::Point(p1)],
    ));
    let d10 = sketch.add_constraint(required_dim(
        DimensionalConstraint::Distance(10.0),
        vec![EntityRef::Point(anchor_pt), EntityRef::Point(p1)],
    ));
    let d20 = sketch.add_constraint(required_dim(
        DimensionalConstraint::Distance(20.0),
        vec![EntityRef::Point(anchor_pt), EntityRef::Point(p1)],
    ));
    // Satisfiable bystander component: p2 exactly pinned.
    let p2 = sketch.add_point(Point2d::new(3.0, 4.0));
    anchor(&sketch, p2, 3.0, 4.0);
    (sketch, d10, d20, p1, p2)
}

#[test]
fn red_conflicting_distance_pair_witness_names_exactly_the_pair() {
    let (sketch, d10, d20, _p1, _p2) = conflicting_pair_sketch();
    let cert = certify_sketch(&sketch);

    assert!(!cert.is_sound(), "conflicting sketch must be unsound");
    assert!(
        matches!(
            cert.constrainedness,
            SketchConstrainedness::Conflicting { .. }
        ),
        "constrainedness must be Conflicting: {:?}",
        cert.constrainedness
    );
    assert!(
        matches!(cert.solver, SolverVerdict::Conflicting { .. }),
        "solver verdict must be Conflicting: {:?}",
        cert.solver
    );

    // Exactly one witness (one conflicted component, no static pairs).
    assert_eq!(
        cert.witnesses.len(),
        1,
        "exactly one conflict witness expected: {:?}",
        cert.witnesses
    );
    let witness = &cert.witnesses[0];
    assert_eq!(witness.kind, WitnessKind::NumericConflict);
    assert!(
        witness.minimal,
        "QuickXplain must certify minimality for a 3-candidate component"
    );
    let named = sorted_ids(witness.constraints.iter().map(|w| w.id).collect());
    let expected = sorted_ids(vec![d10, d20]);
    assert_eq!(
        named, expected,
        "the witness must name EXACTLY the contradictory Distance pair \
         (not the Y pin, not the full candidate set): {witness:?}"
    );
    // Per-constraint residuals ride in the witness. The least-squares
    // compromise lands p1 near x = 15, so both distances miss by ~5.
    for w in &witness.constraints {
        assert!(
            w.residual > 0.1,
            "witness residual must reflect the post-solve miss: {w:?}"
        );
    }
}

#[test]
fn conflict_membership_marks_touched_entities_over_constrained() {
    let (sketch, d10, d20, p1, p2) = conflicting_pair_sketch();
    let cert = certify_sketch(&sketch);

    let status_of = |e: EntityRef| {
        cert.entity_statuses
            .iter()
            .find(|s| s.entity == e)
            .unwrap_or_else(|| panic!("entity {e:?} missing from statuses"))
    };

    match &status_of(EntityRef::Point(p1)).constrainment {
        EntityConstrainment::OverConstrained { via } => {
            assert_eq!(
                sorted_ids(via.clone()),
                sorted_ids(vec![d10, d20]),
                "p1's `via` must be the witness pair"
            );
        }
        other => panic!("p1 must be over-constrained, got {other:?}"),
    }
    // The satisfiable bystander is untouched by the conflict.
    assert_eq!(
        status_of(EntityRef::Point(p2)).constrainment,
        EntityConstrainment::FullyConstrained,
        "p2 is exactly pinned in its own component"
    );
}

// ── RED 2: per-entity split on a half-constrained slot ──────────────

struct Slot {
    sketch: Sketch,
    a: Point2dId,
    b: Point2dId,
    c: Point2dId,
    d: Point2dId,
    ab: geometry_engine::sketch2d::Line2dId,
    dc: geometry_engine::sketch2d::Line2dId,
    right: geometry_engine::sketch2d::Arc2dId,
    left: geometry_engine::sketch2d::Arc2dId,
}

/// The Slice-1 slot: 4 shared points, 2 derived lines, 2
/// endpoint-derived arcs (1 private DOF each — the chord offset).
fn build_slot() -> Slot {
    let sketch = fresh("slice4_slot");
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(20.0, 0.0));
    let c = sketch.add_point(Point2d::new(20.0, 10.0));
    let d = sketch.add_point(Point2d::new(0.0, 10.0));
    let ab = sketch.add_line(a, b).expect("line ab");
    let dc = sketch.add_line(d, c).expect("line dc");
    let right = sketch.add_arc(b, c, 7.0, true, false).expect("right arc");
    let left = sketch.add_arc(d, a, 7.0, true, false).expect("left arc");
    Slot {
        sketch,
        a,
        b,
        c,
        d,
        ab,
        dc,
        right,
        left,
    }
}

#[test]
fn red_half_constrained_slot_reports_exact_per_entity_split() {
    let slot = build_slot();
    // Pin A and B (2 × 2 DOF) and dimension the LEFT arc's radius
    // (its 1 private DOF — but its endpoint D stays loose, so the arc
    // is dimensioned yet not placed). Hand count:
    //   total free = 4 points × 2 + 2 arcs × 1        = 10
    //   removed    = X_a + Y_a + X_b + Y_b + R_left   =  5
    //   residual   =                                     5
    // Per entity: a, b fully constrained; line ab fully constrained
    // (both parents placed); c, d keep 2 each; right arc keeps its 1
    // (no dimension at all); left arc keeps 0 (its private DOF is
    // consumed by the Radius dimension) but is NOT fully constrained
    // (parent d is loose); line dc likewise moves with loose parents
    // and owns 0 private DOFs.
    anchor(&slot.sketch, slot.a, 0.0, 0.0);
    anchor(&slot.sketch, slot.b, 20.0, 0.0);
    slot.sketch.add_constraint(required_dim(
        DimensionalConstraint::Radius(6.0),
        vec![EntityRef::Arc(slot.left)],
    ));

    let cert = certify_sketch(&slot.sketch);

    assert_eq!(
        cert.constrainedness,
        SketchConstrainedness::UnderConstrained { free_dofs: 5 },
        "hand-counted residual freedom is 5"
    );

    let status_of = |e: EntityRef| {
        cert.entity_statuses
            .iter()
            .find(|s| s.entity == e)
            .unwrap_or_else(|| panic!("entity {e:?} missing from statuses"))
            .constrainment
            .clone()
    };

    assert_eq!(
        status_of(EntityRef::Point(slot.a)),
        EntityConstrainment::FullyConstrained
    );
    assert_eq!(
        status_of(EntityRef::Point(slot.b)),
        EntityConstrainment::FullyConstrained
    );
    assert_eq!(
        status_of(EntityRef::Line(slot.ab)),
        EntityConstrainment::FullyConstrained,
        "derived segment on two placed parents is fully constrained"
    );
    assert_eq!(
        status_of(EntityRef::Point(slot.c)),
        EntityConstrainment::UnderConstrained { free_dofs: 2 }
    );
    assert_eq!(
        status_of(EntityRef::Point(slot.d)),
        EntityConstrainment::UnderConstrained { free_dofs: 2 }
    );
    assert_eq!(
        status_of(EntityRef::Arc(slot.right)),
        EntityConstrainment::UnderConstrained { free_dofs: 1 },
        "undimensioned arc keeps its private chord-offset DOF"
    );
    assert_eq!(
        status_of(EntityRef::Arc(slot.left)),
        EntityConstrainment::UnderConstrained { free_dofs: 0 },
        "radius-dimensioned arc on a loose endpoint: 0 private DOFs \
         left, but NOT fully constrained (it moves with d)"
    );
    assert_eq!(
        status_of(EntityRef::Line(slot.dc)),
        EntityConstrainment::UnderConstrained { free_dofs: 0 },
        "derived segment on loose parents moves with them"
    );

    // The per-entity freedom must sum to the component residual.
    let sum: usize = cert
        .entity_statuses
        .iter()
        .filter_map(|s| match &s.constrainment {
            EntityConstrainment::UnderConstrained { free_dofs } => Some(*free_dofs),
            _ => None,
        })
        .sum();
    assert_eq!(sum, 5, "per-entity attribution must sum to the residual");
}

#[test]
fn fully_dimensioned_slot_reports_every_entity_fully_constrained() {
    let slot = build_slot();
    for (p, x, y) in [
        (slot.a, 0.0, 0.0),
        (slot.b, 20.0, 0.0),
        (slot.c, 20.0, 10.0),
        (slot.d, 0.0, 10.0),
    ] {
        anchor(&slot.sketch, p, x, y);
    }
    for arc in [slot.right, slot.left] {
        slot.sketch.add_constraint(required_dim(
            DimensionalConstraint::Radius(6.0),
            vec![EntityRef::Arc(arc)],
        ));
    }

    let cert = certify_sketch(&slot.sketch);
    assert!(cert.is_sound(), "dimensioned slot is sound: {cert:?}");
    assert_eq!(
        cert.constrainedness,
        SketchConstrainedness::FullyConstrained
    );
    assert!(
        matches!(cert.solver, SolverVerdict::Converged { .. }),
        "verdict: {:?}",
        cert.solver
    );
    for status in &cert.entity_statuses {
        assert_eq!(
            status.constrainment,
            EntityConstrainment::FullyConstrained,
            "every slot entity is placed: {status:?}"
        );
    }
    // One connected component, plannable end-to-end, no clusters
    // needed (pure sequential extension).
    assert_eq!(cert.decomposition.components, 1);
    assert_eq!(cert.decomposition.planned_components, 1);
    assert_eq!(cert.decomposition.dense_components, 0);
    // Per-constraint facts: everything satisfied and independent.
    assert_eq!(cert.constraint_facts.len(), 10);
    for fact in &cert.constraint_facts {
        assert!(fact.satisfied, "{fact:?}");
        assert!(fact.residual < 1e-6, "{fact:?}");
    }
}

// ── RED 3: cluster localisation ─────────────────────────────────────

#[test]
fn red_triangle_cluster_entities_carry_their_cluster_id() {
    // The Slice-3 triangle topology: two anchors, a distance triangle
    // whose vertices each carry exactly one link to placed geometry
    // (so sequential extension stalls and the Fudos-Hoffmann cluster
    // fires), plus a frame Y closing the 3 placement DOF.
    let sketch = fresh("slice4_triangle");
    let g0 = sketch.add_point(Point2d::new(0.1, -0.1));
    anchor(&sketch, g0, 0.0, 0.0);
    let g1 = sketch.add_point(Point2d::new(59.9, 0.2));
    anchor(&sketch, g1, 60.0, 0.0);

    let t0t = (20.0, 10.0);
    let t1t = (40.0, 25.0);
    let t2t = (15.0, 35.0);
    let t0 = sketch.add_point(Point2d::new(t0t.0 + 0.3, t0t.1 - 0.2));
    let t1 = sketch.add_point(Point2d::new(t1t.0 - 0.4, t1t.1 + 0.1));
    let t2 = sketch.add_point(Point2d::new(t2t.0 + 0.2, t2t.1 + 0.3));
    let dist = |a: (f64, f64), b: (f64, f64)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
    let add_distance = |a: Point2dId, at: (f64, f64), b: Point2dId, bt: (f64, f64)| {
        sketch.add_constraint(required_dim(
            DimensionalConstraint::Distance(dist(at, bt)),
            vec![EntityRef::Point(a), EntityRef::Point(b)],
        ));
    };
    add_distance(t0, t0t, t1, t1t);
    add_distance(t1, t1t, t2, t2t);
    add_distance(t0, t0t, t2, t2t);
    add_distance(g0, (0.0, 0.0), t0, t0t);
    add_distance(g1, (60.0, 0.0), t1, t1t);
    sketch.add_constraint(required_dim(
        DimensionalConstraint::YCoordinate(t2t.1),
        vec![EntityRef::Point(t2)],
    ));

    let cert = certify_sketch(&sketch);
    assert_eq!(
        cert.constrainedness,
        SketchConstrainedness::FullyConstrained
    );
    assert!(cert.decomposition.clusters >= 1, "{:?}", cert.decomposition);

    let cluster_of = |e: EntityRef| {
        cert.entity_statuses
            .iter()
            .find(|s| s.entity == e)
            .unwrap_or_else(|| panic!("entity {e:?} missing"))
            .cluster
    };
    let c0 = cluster_of(EntityRef::Point(t0));
    assert!(c0.is_some(), "triangle vertex must carry a cluster id");
    assert_eq!(cluster_of(EntityRef::Point(t1)), c0);
    assert_eq!(cluster_of(EntityRef::Point(t2)), c0);
    // The anchors extend sequentially — no cluster label.
    assert_eq!(cluster_of(EntityRef::Point(g0)), None);
    assert_eq!(cluster_of(EntityRef::Point(g1)), None);
}

// ── RED 4: honest-refuse constraints surface as singleton witnesses ─

#[test]
fn red_unenforced_constraint_yields_minimal_singleton_witness() {
    let sketch = fresh("slice4_refuse");
    let p = sketch.add_point(Point2d::new(1.0, 2.0));
    anchor(&sketch, p, 1.0, 2.0);
    let moi = sketch.add_constraint(required_dim(
        DimensionalConstraint::MomentOfInertia(0.5),
        vec![EntityRef::Point(p)],
    ));

    let cert = certify_sketch(&sketch);
    assert!(
        !cert.is_sound(),
        "an honest-refuse constraint must keep the sketch unsound"
    );
    let witness = cert
        .witnesses
        .iter()
        .find(|w| w.constraints.iter().any(|c| c.id == moi))
        .expect("the refused constraint must appear in a witness");
    assert_eq!(
        witness.constraints.len(),
        1,
        "the refused constraint is inconsistent ALONE (irreducible \
         residual) — the witness must be the minimal singleton: {witness:?}"
    );
    assert!(witness.minimal);
    assert!(
        (witness.constraints[0].residual - 1.0).abs() < 1e-9,
        "the irreducible refuse residual (1.0) must ride in the witness"
    );
}

// ── Static contradictory pairs are their own (already minimal) witness

#[test]
fn static_contradiction_yields_a_minimal_static_pair_witness() {
    let sketch = fresh("slice4_static_pair");
    let p = sketch.add_point(Point2d::new(0.0, 0.0));
    let q = sketch.add_point(Point2d::new(10.0, 3.0));
    let line = sketch.add_line(p, q).expect("line");
    let h = sketch.add_constraint(Constraint::new_geometric(
        GeometricConstraint::Horizontal,
        vec![EntityRef::Line(line)],
        ConstraintPriority::Required,
    ));
    let v = sketch.add_constraint(Constraint::new_geometric(
        GeometricConstraint::Vertical,
        vec![EntityRef::Line(line)],
        ConstraintPriority::Required,
    ));

    let cert = certify_sketch(&sketch);
    assert!(!cert.is_sound());
    // The pair must surface as a MINIMAL witness. Kind depends on
    // which detector proves it first for this geometry (the numeric
    // pass may also catch it; identical sets are deduped with numeric
    // provenance preferred) — the contract is the set + minimality.
    let witness = cert
        .witnesses
        .iter()
        .find(|w| {
            sorted_ids(w.constraints.iter().map(|c| c.id).collect()) == sorted_ids(vec![h, v])
        })
        .expect("the contradictory pair must surface as a witness");
    assert!(witness.minimal, "a pair is minimal by construction");
    assert!(
        witness.kind == WitnessKind::StaticPair || witness.kind == WitnessKind::NumericConflict
    );
    // No duplicate witness carries the same set.
    assert_eq!(
        cert.witnesses
            .iter()
            .filter(|w| {
                sorted_ids(w.constraints.iter().map(|c| c.id).collect()) == sorted_ids(vec![h, v])
            })
            .count(),
        1,
        "identical witness sets must be deduped: {:?}",
        cert.witnesses
    );
}

// ── Wire shape, determinism, compact summary ────────────────────────

#[test]
fn certificate_serializes_and_orders_deterministically() {
    let (sketch, ..) = conflicting_pair_sketch();
    let first = serde_json::to_value(certify_sketch(&sketch)).expect("serialise");
    let second = serde_json::to_value(certify_sketch(&sketch)).expect("serialise");
    assert_eq!(first, second, "certify must be deterministic");

    // Deterministic ordering contracts: entity statuses ascend by
    // entity, constraint facts ascend by constraint id.
    let cert = certify_sketch(&sketch);
    for pair in cert.entity_statuses.windows(2) {
        assert!(pair[0].entity < pair[1].entity, "statuses must ascend");
    }
    for pair in cert.constraint_facts.windows(2) {
        assert!(pair[0].id.0 < pair[1].id.0, "facts must ascend by id");
    }
    // The wire payload carries the v2 sections.
    assert!(first.get("entity_statuses").is_some());
    assert!(first.get("witnesses").is_some());
    assert!(first.get("solver").is_some());
    assert!(first.get("dof").is_some());
    assert!(first.get("decomposition").is_some());
    assert!(first.get("constraint_facts").is_some());
}

#[test]
fn compact_summary_reflects_the_full_certificate() {
    let (sketch, d10, d20, ..) = conflicting_pair_sketch();
    let cert = certify_sketch(&sketch);
    let compact = cert.compact();

    assert!(!compact.sound);
    assert_eq!(compact.constrainedness, cert.constrainedness);
    assert_eq!(compact.witnesses.len(), 1);
    assert_eq!(
        sorted_ids(compact.witnesses[0].constraints.clone()),
        sorted_ids(vec![d10, d20])
    );
    assert!(compact.witnesses[0].minimal);
    assert!(compact.over_constrained_entities >= 1);

    // A clean fully-dimensioned slot summarises as sound/complete.
    let slot = build_slot();
    for (p, x, y) in [
        (slot.a, 0.0, 0.0),
        (slot.b, 20.0, 0.0),
        (slot.c, 20.0, 10.0),
        (slot.d, 0.0, 10.0),
    ] {
        anchor(&slot.sketch, p, x, y);
    }
    for arc in [slot.right, slot.left] {
        slot.sketch.add_constraint(required_dim(
            DimensionalConstraint::Radius(6.0),
            vec![EntityRef::Arc(arc)],
        ));
    }
    let clean = certify_sketch(&slot.sketch).compact();
    assert!(clean.sound);
    assert!(clean.witnesses.is_empty());
    assert_eq!(clean.under_constrained_entities, 0);
    assert_eq!(clean.over_constrained_entities, 0);
    assert_eq!(clean.fully_constrained_entities, 8);
}
