//! DOF analysis at scale: the rank pass runs on every `/dof` request,
//! so its cost is a product property, not a benchmark curiosity.
//!
//! `analyze_dofs` adjudicates its structural DOF tally with the
//! Jacobian's numerical rank, because a tally cannot tell "two DOFs
//! removed" from "one DOF removed twice". Ranking the whole Jacobian
//! as one dense matrix, after an unconditional Newton solve, cost
//! 682 ms on the 300-constraint plate below (measured, unoptimised
//! build) against 0.60 ms for the tally alone. Three structural
//! changes brought that back:
//!
//! 1. **Block-diagonal rank scan.** Disconnected components share no
//!    parameter, so a constraint's central-difference row is exactly
//!    zero in every foreign column and the blocks can be scanned
//!    independently for a bit-identical verdict (proved in
//!    `constraint_solver.rs`'s `blocked_scan_is_bitwise_identical_to_
//!    the_whole_matrix_scan`).
//! 2. **Scoped differentiation.** Perturbing one parameter can only
//!    move its own component's residuals, so only those are evaluated.
//! 3. **No solve when there is nothing to classify.** The residual
//!    split into redundant-vs-conflicting only applies to dependent
//!    rows; a sketch whose rank equals its row count has none, and the
//!    Newton solve that made those residuals accurate is pure cost.
//!
//! The budgets below are wall-clock tripwires, not benchmarks. Each is
//! set well above the measured time and well below the regression it
//! guards, so a loaded machine does not fail the suite but a return to
//! the whole-matrix path does.
//!
//! They are calibrated for the DEBUG profile these tests run under --
//! `--release` is several times faster and would pass them trivially,
//! so they are a regression tripwire for this profile and not a
//! performance claim about shipped builds. Wall clock is shared
//! hardware: on a loaded CI box they can go noisy, and a failure here
//! with the structural assertions in
//! `constraint_solver.rs` (`rank_scan_runs_one_block_per_disconnected_component`)
//! still green means the machine, not the code. Those structural
//! assertions are the primary guard; these are the second, independent
//! one.

#![allow(clippy::float_cmp)]
// Reason for `#![allow(clippy::expect_used)]` / `unwrap_used` /
// `panic` — test-only file: failing loudly at the fixture site is the
// desired failure mode; the workspace deny lints target production
// code.
#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]

mod common;

use common::{generate_plate, PlateSpec};
use geometry_engine::sketch2d::sketch_certificate::certify_sketch;
use geometry_engine::sketch2d::sketch_solver::DofStatus;
use std::time::Instant;

/// Wall-clock ceiling for `analyze_dofs` on the 300-constraint plate.
///
/// Measured: 16.8 ms. Whole-matrix scan + unconditional solve: 682 ms.
/// 150 ms is ~9x the measurement and ~4.5x under the regression.
const DOF_BUDGET_MS: f64 = 150.0;

/// Wall-clock ceiling for `certify_sketch` on the same plate.
///
/// Measured: 363 ms. The same certificate with a second, independent
/// rank scan for its DOF snapshot: 1677 ms. 900 ms is ~2.5x the
/// measurement and ~1.9x under the regression.
const CERTIFY_BUDGET_MS: f64 = 900.0;

/// Best of `runs`, so one scheduling hiccup cannot fail the gate.
fn best_ms(runs: usize, mut f: impl FnMut()) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..runs {
        let t = Instant::now();
        f();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        if ms < best {
            best = ms;
        }
    }
    best
}

#[test]
fn analyze_dofs_on_the_300_constraint_plate_stays_within_budget() {
    let plate = generate_plate(&PlateSpec::LARGE);
    // Warm-up: the first call pays for lazily-built solver state.
    let first = plate.sketch.analyze_dofs();
    assert_eq!(
        first.status,
        DofStatus::FullyConstrained,
        "the generated plate is fully dimensioned: {first:?}"
    );
    assert!(
        first.redundant.is_empty() && first.conflicts.is_empty(),
        "nothing to classify on a clean plate: {first:?}"
    );
    assert!(
        !first.singular_configuration,
        "a jittered plate does not stand on a singular configuration: {first:?}"
    );

    let ms = best_ms(3, || {
        let _ = plate.sketch.analyze_dofs();
    });
    assert!(
        ms < DOF_BUDGET_MS,
        concat!(
            "analyze_dofs on {} constraints took {:.1} ms, budget {} ms ",
            "(measured regressions on this fixture: ~305 ms with a whole-matrix ",
            "rank scan, ~403 ms with an unconditional Newton solve, ~682 ms with both)"
        ),
        plate.constraint_count,
        ms,
        DOF_BUDGET_MS
    );
    println!(
        "analyze_dofs({} constraints, {} components) = {ms:.3} ms",
        plate.constraint_count,
        first.components.len()
    );
}

#[test]
fn certify_sketch_pays_for_one_rank_scan_not_two() {
    let plate = generate_plate(&PlateSpec::LARGE);
    let cert = certify_sketch(&plate.sketch);
    assert!(
        cert.constrainedness.is_fully_constrained(),
        "the generated plate is fully dimensioned: {:?}",
        cert.constrainedness
    );

    let ms = best_ms(2, || {
        let _ = certify_sketch(&plate.sketch);
    });
    assert!(
        ms < CERTIFY_BUDGET_MS,
        concat!(
            "certify_sketch on {} constraints took {:.1} ms, budget {} ms ",
            "(running the rank scan twice costs ~1677 ms here)"
        ),
        plate.constraint_count,
        ms,
        CERTIFY_BUDGET_MS
    );
    println!(
        "certify_sketch({} constraints) = {ms:.3} ms",
        plate.constraint_count
    );
}

/// The certificate's DOF snapshot must be the SAME analysis a caller
/// gets from `analyze_dofs` — one diagnosis, two readings. At plate
/// scale, where a second scan would have every opportunity to land
/// somewhere else.
#[test]
fn certificate_dof_snapshot_agrees_with_analyze_dofs_at_scale() {
    for spec in [PlateSpec::SMALL, PlateSpec::MEDIUM] {
        let plate = generate_plate(&spec);
        let standalone = plate.sketch.analyze_dofs();
        let cert = certify_sketch(&plate.sketch);
        assert_eq!(
            cert.dof.status, standalone.status,
            "{} constraints",
            plate.constraint_count
        );
        assert_eq!(
            cert.dof.components, standalone.components,
            "{} constraints",
            plate.constraint_count
        );
        assert_eq!(
            cert.dof.singular_configuration, standalone.singular_configuration,
            "{} constraints",
            plate.constraint_count
        );
        assert_eq!(
            cert.redundant_constraints,
            standalone.redundant.len(),
            "{} constraints",
            plate.constraint_count
        );
    }
}
