//! Property-based tests for the boolean operation pipeline.
//!
//! Complements the seeded `prop_tier1_*` / `prop_tier2_*` / `prop_tier3_*`
//! suite that lives inside `operations::boolean::tests`. Both suites
//! exercise the same invariants, but they answer different questions:
//!
//! - The in-module seeded suite gives **deterministic regression coverage**
//!   over a fixed set of pairs (50 box/box, 50 box/sphere, 50 box/cyl, …).
//!   When CI fails at "iter 47" the failing inputs are reproducible from
//!   the seed but not minimal — the human still has to bisect.
//!
//! - This integration suite uses the `proptest` crate so failures
//!   **shrink to a minimal failing primitive pair** and persist into
//!   `proptest-regressions/` for replay. That makes the ceiling
//!   regressions actionable instead of "a 17×4×9 box union with a
//!   13×11×6 box at seed `0xB001…` fails".
//!
//! ## Numerical-rigor ceiling
//!
//! The boolean engine is not yet rigorously volume-correct. The in-module
//! `assert_tier1` helper accepts every `Err(..)` outcome other than
//! `OperationError::NotImplemented` precisely because tighter assertions
//! would flag known robustness gaps as test failures rather than as the
//! engineering work they are. This file mirrors that ceiling exactly:
//!
//! - **What is asserted:** the pipeline returns either `Ok(solid_id)`
//!   pointing at a real solid in the model, or any typed error other
//!   than `NotImplemented`. Union and Intersection additionally satisfy
//!   *success-parity* under operand swap (set-theoretic commutativity).
//!
//! - **What is NOT asserted:** exact volumes, exact face/edge counts,
//!   manifoldness, Euler characteristic, bbox containment beyond the
//!   solid-existence check. Tightening any of these belongs in a future
//!   tier as the kernel hardens.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::{
    boolean_operation, BooleanOp, BooleanOptions, OperationError,
};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

use proptest::prelude::*;

// -----------------------------------------------------------------------
// Strategies
//
// Ranges are deliberately the same as the seeded harness's `gen_range`
// bounds so the coverage envelope matches. Shrinking will pull each
// dimension toward its lower bound on failure.
// -----------------------------------------------------------------------

/// (width, height, depth) in the box dimension envelope used by the
/// seeded harness's `make_random_box`.
fn arb_box_dims() -> impl Strategy<Value = (f64, f64, f64)> {
    (2.0_f64..20.0, 2.0_f64..20.0, 2.0_f64..20.0)
}

/// Sphere radius envelope used by `prop_tier1_box_sphere_all_ops`.
fn arb_sphere_radius() -> impl Strategy<Value = f64> {
    1.0_f64..10.0
}

/// (radius, height) envelope used by `prop_tier1_box_cylinder_all_ops`.
fn arb_cylinder_params() -> impl Strategy<Value = (f64, f64)> {
    (1.0_f64..8.0, 5.0_f64..25.0)
}

fn arb_op() -> impl Strategy<Value = BooleanOp> {
    prop_oneof![
        Just(BooleanOp::Union),
        Just(BooleanOp::Intersection),
        Just(BooleanOp::Difference),
    ]
}

// -----------------------------------------------------------------------
// Primitive constructors
//
// All primitives are placed at the world origin to maximise pair overlap
// and exercise the face-face intersection / classification pipeline,
// matching the seeded harness's behavior.
// -----------------------------------------------------------------------

fn make_box(model: &mut BRepModel, dims: (f64, f64, f64)) -> SolidId {
    let (w, h, d) = dims;
    let geom = TopologyBuilder::new(model)
        .create_box_3d(w, h, d)
        .expect("strategy bounds guarantee positive dimensions");
    expect_solid(geom)
}

fn make_sphere(model: &mut BRepModel, radius: f64) -> SolidId {
    let geom = TopologyBuilder::new(model)
        .create_sphere_3d(Point3::ORIGIN, radius)
        .expect("strategy bounds guarantee positive radius");
    expect_solid(geom)
}

fn make_cylinder(model: &mut BRepModel, params: (f64, f64)) -> SolidId {
    let (radius, height) = params;
    let geom = TopologyBuilder::new(model)
        .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, radius, height)
        .expect("strategy bounds guarantee positive radius and height");
    expect_solid(geom)
}

fn expect_solid(geom: GeometryId) -> SolidId {
    match geom {
        GeometryId::Solid(id) => id,
        other => panic!("primitive constructor returned non-solid geometry: {other:?}"),
    }
}

// -----------------------------------------------------------------------
// Tier-1 invariants (mirror of the in-module `assert_tier1`)
//
// Returning `TestCaseError` instead of panicking lets proptest record the
// failure, run its shrinker, and persist a regression seed.
// -----------------------------------------------------------------------

fn check_tier1(
    result: &Result<SolidId, OperationError>,
    model: &BRepModel,
    op: BooleanOp,
) -> Result<(), TestCaseError> {
    if let Err(OperationError::NotImplemented(msg)) = result {
        return Err(TestCaseError::fail(format!(
            "{op:?} returned NotImplemented('{msg}') — regression",
        )));
    }
    if let Ok(solid_id) = result {
        if model.solids.get(*solid_id).is_none() {
            return Err(TestCaseError::fail(format!(
                "{op:?} returned Ok({solid_id}) but the solid is missing from the model",
            )));
        }
    }
    // Any other typed `Err(..)` is an accepted Tier-1 outcome until the
    // numerical-robustness ceiling rises (see module docs).
    Ok(())
}

// -----------------------------------------------------------------------
// Properties
//
// `cases: 64` keeps each property under a couple of seconds wall-clock —
// enough to drive shrinking on real failures without dominating CI. The
// seeded harness gives complementary determinism; this suite gives
// shrinking and persisted regressions.
// -----------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        // Each case runs the full boolean pipeline; cap the per-case
        // generation budget so the shrinker doesn't loop indefinitely
        // on a degenerate strategy.
        max_global_rejects: 1024,
        ..ProptestConfig::default()
    })]

    /// Box ⊕ Box across all three operations. The pipeline must terminate
    /// with a typed outcome (no panic, no `NotImplemented`) and any `Ok`
    /// result must reference a solid that exists in the model.
    #[test]
    fn box_box_all_ops_terminate(
        a in arb_box_dims(),
        b in arb_box_dims(),
        op in arb_op(),
    ) {
        let mut model = BRepModel::new();
        let solid_a = make_box(&mut model, a);
        let solid_b = make_box(&mut model, b);
        let result = boolean_operation(
            &mut model, solid_a, solid_b, op, BooleanOptions::default(),
        );
        check_tier1(&result, &model, op)?;
    }

    /// Box ⊕ Sphere — exercises the plane/sphere classification pairing,
    /// a distinct analytical code path from plane/plane.
    #[test]
    fn box_sphere_all_ops_terminate(
        box_dims in arb_box_dims(),
        radius in arb_sphere_radius(),
        op in arb_op(),
    ) {
        let mut model = BRepModel::new();
        let solid_a = make_box(&mut model, box_dims);
        let solid_b = make_sphere(&mut model, radius);
        let result = boolean_operation(
            &mut model, solid_a, solid_b, op, BooleanOptions::default(),
        );
        check_tier1(&result, &model, op)?;
    }

    /// Box ⊕ Cylinder — exercises the plane/cylinder classification
    /// pairing, again a distinct analytical code path.
    #[test]
    fn box_cylinder_all_ops_terminate(
        box_dims in arb_box_dims(),
        cyl in arb_cylinder_params(),
        op in arb_op(),
    ) {
        let mut model = BRepModel::new();
        let solid_a = make_box(&mut model, box_dims);
        let solid_b = make_cylinder(&mut model, cyl);
        let result = boolean_operation(
            &mut model, solid_a, solid_b, op, BooleanOptions::default(),
        );
        check_tier1(&result, &model, op)?;
    }

    /// Union is set-theoretically commutative — A ∪ B and B ∪ A must
    /// have identical success/failure parity. Different outcomes mean the
    /// classification pipeline diverged on operand order, which is a real
    /// regression even at the current Tier-1 ceiling.
    #[test]
    fn union_commutativity_parity(
        a in arb_box_dims(),
        b in arb_box_dims(),
    ) {
        let mut model = BRepModel::new();
        let solid_a = make_box(&mut model, a);
        let solid_b = make_box(&mut model, b);
        let r_ab = boolean_operation(
            &mut model, solid_a, solid_b, BooleanOp::Union, BooleanOptions::default(),
        );
        let r_ba = boolean_operation(
            &mut model, solid_b, solid_a, BooleanOp::Union, BooleanOptions::default(),
        );
        check_tier1(&r_ab, &model, BooleanOp::Union)?;
        check_tier1(&r_ba, &model, BooleanOp::Union)?;
        prop_assert_eq!(
            r_ab.is_ok(),
            r_ba.is_ok(),
            "A ∪ B success-parity ({}) != B ∪ A success-parity ({}) — asymmetric classification regression",
            r_ab.is_ok(),
            r_ba.is_ok(),
        );
    }

    /// Intersection is set-theoretically commutative; same parity rule
    /// as union.
    #[test]
    fn intersection_commutativity_parity(
        a in arb_box_dims(),
        b in arb_box_dims(),
    ) {
        let mut model = BRepModel::new();
        let solid_a = make_box(&mut model, a);
        let solid_b = make_box(&mut model, b);
        let r_ab = boolean_operation(
            &mut model, solid_a, solid_b, BooleanOp::Intersection, BooleanOptions::default(),
        );
        let r_ba = boolean_operation(
            &mut model, solid_b, solid_a, BooleanOp::Intersection, BooleanOptions::default(),
        );
        check_tier1(&r_ab, &model, BooleanOp::Intersection)?;
        check_tier1(&r_ba, &model, BooleanOp::Intersection)?;
        prop_assert_eq!(
            r_ab.is_ok(),
            r_ba.is_ok(),
            "A ∩ B success-parity ({}) != B ∩ A success-parity ({}) — asymmetric classification regression",
            r_ab.is_ok(),
            r_ba.is_ok(),
        );
    }

    /// Difference is NOT commutative (A − B ≠ B − A in general), so we
    /// only assert that both orderings independently satisfy Tier-1.
    /// This catches operand-direction-specific regressions in the
    /// difference pipeline that would otherwise hide behind the
    /// commutative operations above.
    #[test]
    fn difference_both_orderings_terminate(
        a in arb_box_dims(),
        b in arb_box_dims(),
    ) {
        let mut model = BRepModel::new();
        let solid_a = make_box(&mut model, a);
        let solid_b = make_box(&mut model, b);
        let r_ab = boolean_operation(
            &mut model, solid_a, solid_b, BooleanOp::Difference, BooleanOptions::default(),
        );
        let r_ba = boolean_operation(
            &mut model, solid_b, solid_a, BooleanOp::Difference, BooleanOptions::default(),
        );
        check_tier1(&r_ab, &model, BooleanOp::Difference)?;
        check_tier1(&r_ba, &model, BooleanOp::Difference)?;
    }
}
