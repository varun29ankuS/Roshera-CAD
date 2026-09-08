// Reason: integration-test crate -- panicking (unwrap/expect/assert/index) is
// the test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::indexing_slicing)]

//! The solver reads the SAME parameter columns in every process.
//!
//! Task 38 keyed the Jacobian's ROWS on the constraints' insertion
//! sequence. Its COLUMNS were left on the entity map's hash order over
//! random v4 entity uuids, and columns are not a listing key:
//!
//! - the rank-revealing Gram-Schmidt in `scan_blocks` projects a row
//!   onto the accumulated basis with a dot product summed IN COLUMN
//!   ORDER, and floating-point addition is not associative, so a
//!   permuted column order gives a different residual norm in the last
//!   bits -- enough to move a near-threshold dependency verdict;
//! - the Newton step solves `JT.J dx = -JT.e` by Gaussian elimination
//!   with partial pivoting, and `JT.J` under a column permutation `P`
//!   is `P^T (JT.J) P`, whose pivot sequence -- and therefore whose
//!   rounding -- is a different one.
//!
//! Both make the same sketch, built the same way, land on different
//! bits in the next process. These tests pin the bits.
//!
//! Every fixture is deliberately ILL-CONDITIONED (a nearly-straightened
//! four-bar chain, two nearly-parallel lines): a well-conditioned
//! system rounds to the same answer whatever order it is summed in, so
//! a well-conditioned fixture would pass with the defect present and
//! prove nothing.
//!
//! The assertions are BITWISE and POSITIONAL. Each round builds a fresh
//! sketch that mints fresh uuids, so the ids are incomparable across
//! rounds; what must match is the `f64::to_bits` of each solved
//! coordinate at each INSERTION POSITION. Running `ROUNDS` fresh draws
//! in one process is the same experiment two processes run -- the uuid
//! draw is the only thing a new process changes.

use geometry_engine::sketch2d::{
    Constraint, ConstraintPriority, DimensionalConstraint, EntityRef, GeometricConstraint, Point2d,
    Point2dId, Sketch, SketchAnchor,
};

/// Independent sketches per test. A column permutation of six points
/// agrees with insertion order once in 720 draws, so 24 rounds make an
/// accidental pass unreachable.
const ROUNDS: usize = 24;

/// The solved coordinates of `points`, in the order given, as raw
/// IEEE-754 bits.
///
/// Bits, not an epsilon comparison: this file exists because the last
/// bits move, and a tolerance would hide exactly the defect under test.
fn solved_bits(sketch: &Sketch, points: &[Point2dId]) -> Vec<u64> {
    let mut bits = Vec::with_capacity(points.len() * 2);
    for id in points {
        let p = sketch
            .get_point(id)
            .expect("every fixture point survives the solve");
        bits.push(p.x.to_bits());
        bits.push(p.y.to_bits());
    }
    bits
}

/// A four-bar chain stretched to within 0.5 permille of straight.
///
/// Five points in a line-ish arrangement: four unit-ish `Distance`
/// links plus a span dimension across the whole chain that is only
/// just satisfiable. Near the straightened configuration the four link
/// gradients are nearly parallel, so `JT.J` is nearly singular and the
/// Newton iterate is dominated by the rounding of its own elimination
/// -- which is what makes the column order observable in the answer.
///
/// The first point is pinned by an X and a Y dimension rather than by
/// `is_fixed`, so it still owns two columns and cannot quietly drop out
/// of the permutation under test.
fn straightening_chain(name: &str) -> (Sketch, Vec<Point2dId>) {
    let sketch = Sketch::new(name.to_string(), SketchAnchor::xy());
    let coords = [
        (0.0, 0.0),
        (5.0, 0.30),
        (10.0, 0.41),
        (15.0, 0.22),
        (20.0, 0.05),
    ];
    let points: Vec<Point2dId> = coords
        .iter()
        .map(|(x, y)| sketch.add_point(Point2d::new(*x, *y)))
        .collect();
    let link = |a: usize, b: usize, d: f64| {
        sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::Distance(d),
            vec![EntityRef::Point(points[a]), EntityRef::Point(points[b])],
            ConstraintPriority::Required,
        ));
    };
    link(0, 1, 5.0);
    link(1, 2, 5.0);
    link(2, 3, 5.0);
    link(3, 4, 5.0);
    // 0.5 permille under the straightened span of 20: reachable, but
    // only just, so the chain converges into the ill-conditioned
    // neighbourhood rather than sitting comfortably inside it.
    link(0, 4, 19.99);
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::XCoordinate(0.0),
        vec![EntityRef::Point(points[0])],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::YCoordinate(0.0),
        vec![EntityRef::Point(points[0])],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::YCoordinate(0.0),
        vec![EntityRef::Point(points[4])],
        ConstraintPriority::Required,
    ));
    (sketch, points)
}

#[test]
fn the_newton_iterate_lands_on_the_same_bits_on_every_build() {
    let mut divergent = 0usize;
    let mut first: Option<Vec<u64>> = None;
    let mut distinct: Vec<Vec<u64>> = Vec::new();
    for _ in 0..ROUNDS {
        let (sketch, points) = straightening_chain("chain_bits");
        sketch
            .solve_constraints()
            .expect("the chain fixture solves");
        let bits = solved_bits(&sketch, &points);
        match &first {
            None => first = Some(bits.clone()),
            Some(f) => {
                if *f != bits {
                    divergent += 1;
                }
            }
        }
        if !distinct.contains(&bits) {
            distinct.push(bits);
        }
    }
    assert_eq!(
        divergent,
        0,
        concat!(
            "the same sketch, built the same way, must land on the SAME BITS in ",
            "every process: the Newton step's elimination pivots on JT.J, and a ",
            "permuted column order permutes JT.J symmetrically, so the pivot ",
            "sequence and its rounding change. {} distinct answers over the rounds"
        ),
        distinct.len()
    );
}

/// Two nearly-parallel lines carrying a `Parallel` constraint plus a
/// distance across them.
///
/// `Parallel`'s residual is the cross product of the two directions;
/// at 0.3 milliradians of misalignment its gradient is within a few
/// ulps of the gradient of the `Horizontal` rows next to it, so the
/// Gram-Schmidt dependency test runs close to the rank threshold --
/// the configuration the brief names, where the verdict is decided in
/// the bits the column-summation order controls.
fn nearly_parallel_lines(name: &str) -> (Sketch, Vec<Point2dId>) {
    let sketch = Sketch::new(name.to_string(), SketchAnchor::xy());
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(10.0, 0.003));
    let c = sketch.add_point(Point2d::new(0.0, 4.0));
    let d = sketch.add_point(Point2d::new(10.0, 4.001));
    let l1 = sketch.add_line(a, b).expect("segment a-b");
    let l2 = sketch.add_line(c, d).expect("segment c-d");
    sketch.add_constraint(Constraint::new_geometric(
        GeometricConstraint::Parallel,
        vec![EntityRef::Line(l1), EntityRef::Line(l2)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_geometric(
        GeometricConstraint::Horizontal,
        vec![EntityRef::Line(l1)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_geometric(
        GeometricConstraint::Horizontal,
        vec![EntityRef::Line(l2)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::Distance(10.0),
        vec![EntityRef::Point(a), EntityRef::Point(b)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::Distance(10.0),
        vec![EntityRef::Point(c), EntityRef::Point(d)],
        ConstraintPriority::Required,
    ));
    (sketch, vec![a, b, c, d])
}

#[test]
fn the_rank_scan_reaches_the_same_verdict_on_every_build() {
    let mut divergent = 0usize;
    let mut first: Option<(usize, usize, usize, usize)> = None;
    let mut answers: Vec<(usize, usize, usize, usize)> = Vec::new();
    for _ in 0..ROUNDS {
        let (sketch, _) = nearly_parallel_lines("parallel_rank");
        let report = sketch.analyze_dofs();
        // Everything the rank pass decides, as counts that survive a
        // fresh uuid draw: how much freedom is left, how many rows the
        // pass called dependent-and-satisfied, how many
        // dependent-and-violated, and how many components it split into.
        let answer = (
            report.total_free_dofs,
            report.redundant.len(),
            report.conflicts.len(),
            report.components.len(),
        );
        match first {
            None => first = Some(answer),
            Some(f) => {
                if f != answer {
                    divergent += 1;
                }
            }
        }
        if !answers.contains(&answer) {
            answers.push(answer);
        }
    }
    assert_eq!(
        divergent, 0,
        concat!(
            "the rank pass must reach the same verdict in every process. Gram-Schmidt ",
            "sums its projection over the COLUMNS, so a permuted column order changes ",
            "the residual norm in its last bits and can move a near-threshold row ",
            "across the rank tolerance. Distinct verdicts: {:?}"
        ),
        answers
    );
}

#[test]
fn the_nearly_parallel_lines_land_on_the_same_bits_on_every_build() {
    let mut divergent = 0usize;
    let mut first: Option<Vec<u64>> = None;
    let mut distinct: Vec<Vec<u64>> = Vec::new();
    for _ in 0..ROUNDS {
        let (sketch, points) = nearly_parallel_lines("parallel_bits");
        sketch
            .solve_constraints()
            .expect("the nearly-parallel fixture solves");
        let bits = solved_bits(&sketch, &points);
        match &first {
            None => first = Some(bits.clone()),
            Some(f) => {
                if *f != bits {
                    divergent += 1;
                }
            }
        }
        if !distinct.contains(&bits) {
            distinct.push(bits);
        }
    }
    assert_eq!(
        divergent,
        0,
        concat!(
            "a nearly-parallel pair must solve to the SAME BITS in every process; ",
            "{} distinct answers over the rounds"
        ),
        distinct.len()
    );
}
