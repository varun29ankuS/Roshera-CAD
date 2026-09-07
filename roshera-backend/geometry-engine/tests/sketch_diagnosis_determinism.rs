// Reason: integration-test crate -- panicking (unwrap/expect/assert/index) is
// the test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::indexing_slicing)]

//! The diagnosis names the SAME constraints in every process.
//!
//! Rank and status were always stable; the WITNESS was not. Which
//! member of a linearly dependent set is called "redundant" is decided
//! by row order -- Gram-Schmidt keeps the first row of a dependent set
//! and flags the rest -- and the row order followed the constraints'
//! random v4 uuids, because `diagnose_constraints`, `build_diagnostic_
//! solver` and the certificate all sorted by `id.0`. Sorting by a
//! random key is deterministic WITHIN one process and arbitrary
//! BETWEEN two: the same sketch, built the same way, named different
//! redundant constraints on the next run. A certificate whose witness
//! changes between runs is not a certificate.
//!
//! The stable key is the constraint's INSERTION SEQUENCE within its
//! sketch -- a per-store counter assigned at `add_constraint`. Two
//! processes that build the same sketch in the same order then agree,
//! which id order can never promise.
//!
//! Every fixture here is deliberately TINY and single-entity or
//! single-pair, so the only thing that can vary between rounds is the
//! row order this file exists to pin. The assertions are POSITIONAL --
//! each round's sketch draws fresh uuids, so the ids themselves are
//! incomparable across rounds; what must match is which INSERTION
//! POSITION the diagnosis blames.
//!
//! Each test builds `ROUNDS` independent sketches in one process. That
//! is the same experiment two processes run, because the id draw is
//! the only thing a new process changes: if the answer is a function
//! of the uuids, `ROUNDS` fresh draws expose it here.

use geometry_engine::sketch2d::sketch_certificate::{certify_sketch, WitnessKind};
use geometry_engine::sketch2d::{
    Constraint, ConstraintId, ConstraintPriority, DimensionalConstraint, EntityRef,
    GeometricConstraint, Point2d, Sketch, SketchAnchor,
};

/// Independent sketches per test.
///
/// The pre-fix answer is a function of a 3-way random draw, so a
/// single round agrees with the fixed answer about one time in three.
/// 24 rounds make an accidental pass a `(1/3)^24` event.
const ROUNDS: usize = 24;

/// Map a diagnosis list back onto the INSERTION POSITIONS of the
/// constraints it names. Ids differ between rounds; positions do not.
fn positions(inserted: &[ConstraintId], named: &[ConstraintId]) -> Vec<usize> {
    named
        .iter()
        .map(|id| {
            inserted
                .iter()
                .position(|i| i == id)
                .expect("the diagnosis may only name constraints this sketch inserted")
        })
        .collect()
}

/// Same, for the pairs the static conflict detector returns: each pair
/// maps to the two insertion positions it joins.
fn pair_positions(
    inserted: &[ConstraintId],
    pairs: &[(ConstraintId, ConstraintId)],
) -> Vec<(usize, usize)> {
    pairs
        .iter()
        .map(|(a, b)| {
            let pa = positions(inserted, &[*a]);
            let pb = positions(inserted, &[*b]);
            (pa[0], pb[0])
        })
        .collect()
}

/// A free point carrying `count` `XCoordinate` constraints, one per
/// value. Returns the sketch and the ids IN INSERTION ORDER.
fn point_with_x_constraints(name: &str, values: &[f64]) -> (Sketch, Vec<ConstraintId>) {
    let sketch = Sketch::new(name.to_string(), SketchAnchor::xy());
    let p = sketch.add_point(Point2d::new(0.0, 0.0));
    let ids = values
        .iter()
        .map(|v| {
            sketch.add_constraint(Constraint::new_dimensional(
                DimensionalConstraint::XCoordinate(*v),
                vec![EntityRef::Point(p)],
                ConstraintPriority::Required,
            ))
        })
        .collect();
    (sketch, ids)
}

#[test]
fn the_redundant_list_names_the_same_constraints_on_every_build() {
    let mut divergent = 0usize;
    let mut answers: Vec<Vec<usize>> = Vec::new();
    for _ in 0..ROUNDS {
        let (sketch, ids) = point_with_x_constraints("dup_x", &[0.0, 0.0, 0.0]);
        let report = sketch.analyze_dofs();
        assert!(
            report.conflicts.is_empty(),
            "three IDENTICAL x-coordinates contradict nothing: {report:?}"
        );
        assert_eq!(
            report.redundant.len(),
            2,
            "one row pins x; the other two are dependent and satisfied: {report:?}"
        );
        let got = positions(&ids, &report.redundant);
        if got != vec![1usize, 2] {
            divergent += 1;
        }
        answers.push(got);
    }
    assert_eq!(
        divergent, 0,
        concat!(
            "the redundant list must name the SAME insertion positions on every build: ",
            "the FIRST row of a dependent set is the essential one, so the redundant ",
            "pair is always positions [1, 2]; got {:?}"
        ),
        answers
    );
}

#[test]
fn the_conflict_list_names_the_same_constraints_on_every_build() {
    let mut divergent = 0usize;
    let mut answers: Vec<Vec<usize>> = Vec::new();
    for _ in 0..ROUNDS {
        let (sketch, ids) = point_with_x_constraints("clash_x", &[3.0, 7.0, 9.0]);
        let report = sketch.analyze_dofs();
        assert!(
            report.redundant.is_empty(),
            "no dependent row here is satisfiable: {report:?}"
        );
        assert_eq!(
            report.conflicts.len(),
            2,
            "one row pins x; the other two are dependent and violated: {report:?}"
        );
        let got = positions(&ids, &report.conflicts);
        if got != vec![1usize, 2] {
            divergent += 1;
        }
        answers.push(got);
    }
    assert_eq!(
        divergent, 0,
        concat!(
            "the conflict list must name the SAME insertion positions on every build; ",
            "expected [1, 2] every round, got {:?}"
        ),
        answers
    );
}

/// Two independent contradictory pairs on two lines, inserted 0,1 then
/// 2,3: `Parallel` vs `Perpendicular` on the pair, then `Horizontal`
/// vs `Vertical` on the first line. Both are CONFIGURATION-INDEPENDENT
/// contradictions, so the static detector finds exactly these two.
fn two_static_conflict_pairs(name: &str) -> (Sketch, Vec<ConstraintId>) {
    let sketch = Sketch::new(name.to_string(), SketchAnchor::xy());
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(10.0, 0.0));
    let c = sketch.add_point(Point2d::new(0.0, 5.0));
    let d = sketch.add_point(Point2d::new(10.0, 5.0));
    let l1 = sketch.add_line(a, b).expect("segment a-b");
    let l2 = sketch.add_line(c, d).expect("segment c-d");
    let ids = vec![
        sketch.add_constraint(Constraint::new_geometric(
            GeometricConstraint::Parallel,
            vec![EntityRef::Line(l1), EntityRef::Line(l2)],
            ConstraintPriority::Required,
        )),
        sketch.add_constraint(Constraint::new_geometric(
            GeometricConstraint::Perpendicular,
            vec![EntityRef::Line(l1), EntityRef::Line(l2)],
            ConstraintPriority::Required,
        )),
        sketch.add_constraint(Constraint::new_geometric(
            GeometricConstraint::Horizontal,
            vec![EntityRef::Line(l1)],
            ConstraintPriority::Required,
        )),
        sketch.add_constraint(Constraint::new_geometric(
            GeometricConstraint::Vertical,
            vec![EntityRef::Line(l1)],
            ConstraintPriority::Required,
        )),
    ];
    (sketch, ids)
}

#[test]
fn the_static_conflict_pairs_are_listed_in_one_order_on_every_build() {
    let mut divergent = 0usize;
    let mut answers: Vec<Vec<(usize, usize)>> = Vec::new();
    let expected = vec![(0usize, 1usize), (2usize, 3usize)];
    for _ in 0..ROUNDS {
        let (sketch, ids) = two_static_conflict_pairs("static_pairs");
        let pairs = sketch.find_constraint_conflicts();
        assert_eq!(
            pairs.len(),
            2,
            "parallel-vs-perpendicular and horizontal-vs-vertical: {pairs:?}"
        );
        let got = pair_positions(&ids, &pairs);
        if got != expected {
            divergent += 1;
        }
        answers.push(got);
    }
    assert_eq!(
        divergent, 0,
        concat!(
            "the static conflict pairs must be listed in ONE order, and each pair must ",
            "name its earlier-inserted member first; expected [(0, 1), (2, 3)] every ",
            "round, got {:?}"
        ),
        answers
    );
}

#[test]
fn the_certificate_lists_its_constraint_facts_in_insertion_order_on_every_build() {
    let mut divergent = 0usize;
    let mut answers: Vec<Vec<usize>> = Vec::new();
    for _ in 0..ROUNDS {
        let (sketch, ids) = point_with_x_constraints("cert_dup_x", &[0.0, 0.0, 0.0]);
        let cert = certify_sketch(&sketch);
        assert_eq!(
            cert.constraint_facts.len(),
            3,
            "one fact per constraint: {:?}",
            cert.constraint_facts
        );
        let listed: Vec<ConstraintId> = cert.constraint_facts.iter().map(|f| f.id).collect();
        let got = positions(&ids, &listed);
        if got != vec![0usize, 1, 2] {
            divergent += 1;
        }
        answers.push(got);
    }
    assert_eq!(
        divergent, 0,
        concat!(
            "the certificate's per-constraint facts must be listed in the order the ",
            "constraints were added, not in the order their random uuids happen to ",
            "sort; expected [0, 1, 2] every round, got {:?}"
        ),
        answers
    );
}

#[test]
fn the_certificate_lists_its_static_pair_witnesses_in_one_order_on_every_build() {
    let mut divergent = 0usize;
    let mut answers: Vec<Vec<Vec<usize>>> = Vec::new();
    let expected: Vec<Vec<usize>> = vec![vec![0, 1], vec![2, 3]];
    for _ in 0..ROUNDS {
        let (sketch, ids) = two_static_conflict_pairs("cert_static_pairs");
        let cert = certify_sketch(&sketch);
        let got: Vec<Vec<usize>> = cert
            .witnesses
            .iter()
            .filter(|w| w.kind == WitnessKind::StaticPair)
            .map(|w| {
                positions(
                    &ids,
                    &w.constraints.iter().map(|c| c.id).collect::<Vec<_>>(),
                )
            })
            .collect();
        if got != expected {
            divergent += 1;
        }
        answers.push(got);
    }
    assert_eq!(
        divergent, 0,
        concat!(
            "the certificate's static-pair witnesses must be listed in ONE order, each ",
            "witness naming its members in insertion order; expected [[0, 1], [2, 3]] ",
            "every round, got {:?}"
        ),
        answers
    );
}

#[test]
fn the_certificate_reports_its_contradictory_pairs_in_one_order_on_every_build() {
    let mut divergent = 0usize;
    let mut answers: Vec<Vec<(usize, usize)>> = Vec::new();
    let expected = vec![(0usize, 1usize), (2usize, 3usize)];
    for _ in 0..ROUNDS {
        let (sketch, ids) = two_static_conflict_pairs("cert_issue_pairs");
        let cert = certify_sketch(&sketch);
        // `issues` names each pair by `ConstraintId`'s Display form, so
        // the ids are recoverable from the prose and mappable back onto
        // insertion positions.
        let marks: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
        let got: Vec<(usize, usize)> = cert
            .issues
            .iter()
            .filter(|line| line.starts_with("contradictory constraint pair"))
            .map(|line| {
                let mut hits: Vec<(usize, usize)> = marks
                    .iter()
                    .enumerate()
                    .filter_map(|(index, mark)| line.find(mark.as_str()).map(|at| (at, index)))
                    .collect();
                hits.sort_unstable();
                assert_eq!(
                    hits.len(),
                    2,
                    "a contradictory-pair line names exactly two constraints: {line}"
                );
                (hits[0].1, hits[1].1)
            })
            .collect();
        if got != expected {
            divergent += 1;
        }
        answers.push(got);
    }
    assert_eq!(
        divergent, 0,
        concat!(
            "the certificate's contradictory-pair issues must be reported in ONE order, ",
            "each naming its earlier-inserted member first; expected [(0, 1), (2, 3)] ",
            "every round, got {:?}"
        ),
        answers
    );
}

#[test]
fn quickxplain_returns_the_same_minimal_core_on_every_build() {
    // Three mutually incompatible x-coordinates on ONE point: EVERY
    // pair among them is already inconsistent, so every pair is a
    // minimal conflict and QuickXplain's answer is decided by nothing
    // but the CANDIDATE ORDER it is handed. That makes this fixture a
    // direct probe of the candidate sort -- the one ordering key in
    // the certificate whose regression corrupts witness CONTENT
    // (which constraints the witness blames) rather than merely the
    // order a list is printed in.
    //
    // The assertion is SELF-CONSISTENCY, not a hardcoded pair: which
    // pair a preference order singles out is QuickXplain's business,
    // but it must be the SAME pair every time.
    let mut answers: Vec<Vec<Vec<usize>>> = Vec::new();
    for _ in 0..ROUNDS {
        let (sketch, ids) = point_with_x_constraints("qx_clash_x", &[3.0, 7.0, 9.0]);
        let cert = certify_sketch(&sketch);
        let cores: Vec<Vec<usize>> = cert
            .witnesses
            .iter()
            .filter(|w| w.kind == WitnessKind::NumericConflict)
            .map(|w| {
                assert!(
                    w.minimal,
                    "every pair here is already a conflict, so the core is minimal: {w:?}"
                );
                let core = positions(
                    &ids,
                    &w.constraints.iter().map(|c| c.id).collect::<Vec<_>>(),
                );
                assert_eq!(
                    core.len(),
                    2,
                    "a minimal core among mutually incompatible coordinates is a PAIR: {core:?}"
                );
                core
            })
            .collect();
        assert_eq!(
            cores.len(),
            1,
            "one component, one numeric-conflict witness: {:?}",
            cert.witnesses
        );
        answers.push(cores);
    }
    let first = answers.first().expect("ROUNDS is non-zero").clone();
    let divergent = answers.iter().filter(|a| **a != first).count();
    assert_eq!(
        divergent, 0,
        concat!(
            "QuickXplain must blame the SAME constraints on every build -- the candidate ",
            "order is its preference order, so a uuid-keyed sort hands back a different ",
            "(still minimal) core each process; first round said {:?}, all rounds {:?}"
        ),
        first, answers
    );
}
