//! Tier-2 volume-invariant property tests for the boolean pipeline
//! (Task #106).
//!
//! `boolean_proptest.rs` (Tier 1) pins termination + commutativity-
//! parity but explicitly opts out of volume checks because "Tightening
//! any of these belongs in a future tier as the kernel hardens." This
//! file *is* that future tier, restricted to scenarios where the
//! analytical answer is unambiguous:
//!
//!   1. **Containment** — A box `small` strictly inside a box `big`
//!      (both at the origin, `small.dim ≤ big.dim` componentwise with
//!      a healthy margin). The analytical answers are:
//!        * `V(big ∪ small) = V(big)`
//!        * `V(big ∩ small) = V(small)`
//!        * `V(big − small) = V(big) − V(small)`
//!      Any deviation > 5% is a real regression — the strictly-
//!      containing case is the easiest non-trivial boolean and any
//!      kernel that fails it cannot be trusted for harder cases.
//!
//!   2. **Identity** — `V(A ∪ A)` and `V(A ∩ A)` both equal `V(A)`.
//!      A trivial sanity check; the engine should resolve "self-
//!      overlap" without exploding face counts or producing a
//!      different volume.
//!
//!   3. **Disjoint union** — Two boxes translated far enough apart
//!      that they do not overlap (bounding boxes separated by > 5
//!      units of slack). `V(A ∪ B) = V(A) + V(B)`. Catches the
//!      classic disjoint-classifier bug where the union accidentally
//!      stitches an empty intersection curve.
//!
//!   4. **Inclusion-exclusion** — For any two boxes the soft invariant
//!      `V(A ∪ B) + V(A ∩ B) ≈ V(A) + V(B)` must hold within 10%.
//!      This is the only property here that uses a relative tolerance
//!      rather than a hard analytical equality; it's a Tier-2 *fitness
//!      function* — the gap measures the current numerical-robustness
//!      ceiling of the boolean engine.
//!
//! ## Tolerance strategy
//!
//! - Hard analytical cases (containment, identity, disjoint) get a
//!   5% relative tolerance — the kernel uses mesh-based volume
//!   (`TessellationParams::fine()`, ~0.5% intrinsic error) for curved
//!   primitives, so 5% gives ~10× headroom over the floor. Box-only
//!   tests should run well under 1% in practice.
//!
//! - The inclusion-exclusion soft invariant uses 10% — overlapping
//!   booleans are the hard case and the kernel is known to be less
//!   accurate there.
//!
//! - Any case where `boolean_operation` returns `Err(..)` is
//!   skipped via `prop_assume!`, NOT failed. The Tier-1 file already
//!   catches `NotImplemented` regressions; this file's job is
//!   asserting volumes on the success path.

use geometry_engine::math::{Matrix4, Vector3};
use geometry_engine::operations::{
    boolean_operation, transform_solid, BooleanOp, BooleanOptions, OperationError,
    TransformOptions,
};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

use proptest::prelude::*;

// -----------------------------------------------------------------------
// Construction helpers
// -----------------------------------------------------------------------

fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    let geom = TopologyBuilder::new(model)
        .create_box_3d(w, h, d)
        .expect("strategy bounds guarantee positive dimensions");
    match geom {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid; got {other:?}"),
    }
}

fn translate(model: &mut BRepModel, solid: SolidId, delta: Vector3) {
    let mat = Matrix4::from_translation(&delta);
    transform_solid(model, solid, mat, TransformOptions::default())
        .expect("translation of a valid solid must succeed");
}

fn volume(model: &mut BRepModel, solid: SolidId) -> Option<f64> {
    model.calculate_solid_volume(solid)
}

/// Relative-difference check: returns true if `|a − b| / max(|a|, |b|, 1.0) ≤ tol`.
/// The `max(_, 1.0)` floor prevents tiny-volume false alarms.
fn within_rel(a: f64, b: f64, tol: f64) -> bool {
    let scale = a.abs().max(b.abs()).max(1.0);
    (a - b).abs() / scale <= tol
}

// -----------------------------------------------------------------------
// Strategies
// -----------------------------------------------------------------------

/// Three-tuple `(w, h, d)` in [2, 10] — the "small" box envelope.
fn arb_small_dims() -> impl Strategy<Value = (f64, f64, f64)> {
    (2.0_f64..10.0, 2.0_f64..10.0, 2.0_f64..10.0)
}

/// `(w, h, d)` strictly larger than `small` by at least 4 units per axis
/// — this guarantees `small ⊂ big` for both anchored at origin, with a
/// 2-unit margin on each axis side that the kernel's coincidence
/// tolerance (1e-8) cannot bridge.
fn arb_strictly_containing(
    small: (f64, f64, f64),
) -> impl Strategy<Value = (f64, f64, f64)> {
    let (sw, sh, sd) = small;
    (
        (sw + 4.0)..(sw + 20.0),
        (sh + 4.0)..(sh + 20.0),
        (sd + 4.0)..(sd + 20.0),
    )
}

/// A translation vector whose component magnitudes all exceed 100 units
/// — far beyond the diameter of any primitive built by `arb_small_dims`
/// (≤ 10 each axis). Guarantees the translated box does not overlap
/// the origin-anchored box.
fn arb_far_translation() -> impl Strategy<Value = Vector3> {
    (100.0_f64..200.0, 100.0_f64..200.0, 100.0_f64..200.0)
        .prop_map(|(x, y, z)| Vector3::new(x, y, z))
}

// -----------------------------------------------------------------------
// Properties
// -----------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 48,
        max_global_rejects: 1024,
        ..ProptestConfig::default()
    })]

    /// Containment: when `small ⊂ big`, the union must have volume
    /// `V(big)` and the intersection `V(small)`. The kernel must
    /// recognise that one operand fully encloses the other and resolve
    /// the classification without spurious face splits.
    #[test]
    fn prop_containment_union_equals_big(
        small in arb_small_dims(),
    ) {
        let big = arb_strictly_containing(small);
        proptest!(|(b in big)| {
            let mut model = BRepModel::new();
            let big_id = make_box(&mut model, b.0, b.1, b.2);
            let small_id = make_box(&mut model, small.0, small.1, small.2);
            let v_big_expected = b.0 * b.1 * b.2;

            let union = boolean_operation(
                &mut model, big_id, small_id, BooleanOp::Union, BooleanOptions::default(),
            );
            // Skip if the engine couldn't classify this pair; Tier-1
            // covers termination regressions independently.
            let union_id = match union { Ok(id) => id, Err(_) => return Ok(()) };

            let v_union = volume(&mut model, union_id)
                .ok_or_else(|| TestCaseError::fail("union volume calculation returned None"))?;

            prop_assert!(
                within_rel(v_union, v_big_expected, 0.05),
                "V(big ∪ small) = {} expected ≈ {} (V(big) since small ⊂ big); \
                 small={:?} big={:?}",
                v_union, v_big_expected, small, b,
            );
        });
    }

    /// Containment: `V(big ∩ small) = V(small)`. Same setup as above.
    #[test]
    fn prop_containment_intersection_equals_small(
        small in arb_small_dims(),
    ) {
        let big = arb_strictly_containing(small);
        proptest!(|(b in big)| {
            let mut model = BRepModel::new();
            let big_id = make_box(&mut model, b.0, b.1, b.2);
            let small_id = make_box(&mut model, small.0, small.1, small.2);
            let v_small_expected = small.0 * small.1 * small.2;

            let inter = boolean_operation(
                &mut model, big_id, small_id, BooleanOp::Intersection, BooleanOptions::default(),
            );
            let inter_id = match inter { Ok(id) => id, Err(_) => return Ok(()) };

            let v_inter = volume(&mut model, inter_id)
                .ok_or_else(|| TestCaseError::fail("intersection volume calculation returned None"))?;

            prop_assert!(
                within_rel(v_inter, v_small_expected, 0.05),
                "V(big ∩ small) = {} expected ≈ {} (V(small) since small ⊂ big); \
                 small={:?} big={:?}",
                v_inter, v_small_expected, small, b,
            );
        });
    }

    /// Containment: `V(big − small) = V(big) − V(small)`. The hollowed-
    /// out volume must be exactly the algebraic difference. This is
    /// the classic "subtract a hole" case and any kernel that mis-
    /// classifies inner-vs-outer here will produce confused shells in
    /// every other difference call too.
    ///
    /// Enabled after bug #51's classification-pipeline fixes
    /// (commit 3ee1a63): the `is_point_in_face` planar-path fallback
    /// for closed-seam circular boundaries, plus the conditional
    /// ≥4-face manifold check, removed the spurious OnBoundary and
    /// shell-rejection conditions that previously caused the
    /// containing-pair difference to return wrong volumes.
    #[test]
    fn prop_containment_difference_equals_diff(
        small in arb_small_dims(),
    ) {
        let big = arb_strictly_containing(small);
        proptest!(|(b in big)| {
            let mut model = BRepModel::new();
            let big_id = make_box(&mut model, b.0, b.1, b.2);
            let small_id = make_box(&mut model, small.0, small.1, small.2);
            let v_big = b.0 * b.1 * b.2;
            let v_small = small.0 * small.1 * small.2;
            let v_expected = v_big - v_small;

            let diff = boolean_operation(
                &mut model, big_id, small_id, BooleanOp::Difference, BooleanOptions::default(),
            );
            let diff_id = match diff { Ok(id) => id, Err(_) => return Ok(()) };

            let v_diff = volume(&mut model, diff_id)
                .ok_or_else(|| TestCaseError::fail("difference volume calculation returned None"))?;

            prop_assert!(
                within_rel(v_diff, v_expected, 0.05),
                "V(big − small) = {} expected ≈ {} = {} − {}; small={:?} big={:?}",
                v_diff, v_expected, v_big, v_small, small, b,
            );
        });
    }

    /// Identity: `V(A ∪ A) = V(A)`. Self-union must not duplicate the
    /// solid or its volume.
    #[test]
    fn prop_identity_self_union_volume_unchanged(
        dims in arb_small_dims(),
    ) {
        let mut model = BRepModel::new();
        let a = make_box(&mut model, dims.0, dims.1, dims.2);
        let b = make_box(&mut model, dims.0, dims.1, dims.2);
        let v_expected = dims.0 * dims.1 * dims.2;

        let result = boolean_operation(
            &mut model, a, b, BooleanOp::Union, BooleanOptions::default(),
        );
        let result_id = match result { Ok(id) => id, Err(_) => return Ok(()) };

        let v = volume(&mut model, result_id)
            .ok_or_else(|| TestCaseError::fail("self-union volume returned None"))?;

        prop_assert!(
            within_rel(v, v_expected, 0.05),
            "V(A ∪ A) = {} expected ≈ {} = V(A); dims={:?}",
            v, v_expected, dims,
        );
    }

    /// Identity: `V(A ∩ A) = V(A)`.
    #[test]
    fn prop_identity_self_intersection_volume_unchanged(
        dims in arb_small_dims(),
    ) {
        let mut model = BRepModel::new();
        let a = make_box(&mut model, dims.0, dims.1, dims.2);
        let b = make_box(&mut model, dims.0, dims.1, dims.2);
        let v_expected = dims.0 * dims.1 * dims.2;

        let result = boolean_operation(
            &mut model, a, b, BooleanOp::Intersection, BooleanOptions::default(),
        );
        let result_id = match result { Ok(id) => id, Err(_) => return Ok(()) };

        let v = volume(&mut model, result_id)
            .ok_or_else(|| TestCaseError::fail("self-intersection volume returned None"))?;

        prop_assert!(
            within_rel(v, v_expected, 0.05),
            "V(A ∩ A) = {} expected ≈ {} = V(A); dims={:?}",
            v, v_expected, dims,
        );
    }

    /// Disjoint union: when A and B are far apart with no overlap,
    /// `V(A ∪ B) = V(A) + V(B)`. Catches the classifier bug where
    /// the union path stitches an empty intersection curve and either
    /// returns a single solid with wrong volume or fails altogether.
    ///
    /// Note: the Tier-1 ceiling explicitly does not require disjoint
    /// unions to succeed (the engine may classify "no overlap" as
    /// `OperationError::EmptyResult` or similar); we skip those cases
    /// rather than treating them as failures.
    ///
    /// Enabled after bug #51's brute-force bbox pre-prune
    /// (commit 3ee1a63): without it, far-translated box pairs
    /// produced phantom face intersections from
    /// `intersect_surface_plane` evaluated on the unbounded plane
    /// equations, which shredded the participating faces and
    /// diverged the volume sum.
    #[test]
    fn prop_disjoint_union_volume_sums(
        a_dims in arb_small_dims(),
        b_dims in arb_small_dims(),
        delta in arb_far_translation(),
    ) {
        let mut model = BRepModel::new();
        let a = make_box(&mut model, a_dims.0, a_dims.1, a_dims.2);
        let b = make_box(&mut model, b_dims.0, b_dims.1, b_dims.2);
        translate(&mut model, b, delta);
        let v_a = a_dims.0 * a_dims.1 * a_dims.2;
        let v_b = b_dims.0 * b_dims.1 * b_dims.2;
        let v_expected = v_a + v_b;

        let result = boolean_operation(
            &mut model, a, b, BooleanOp::Union, BooleanOptions::default(),
        );
        let result_id = match result { Ok(id) => id, Err(_) => return Ok(()) };

        let v = volume(&mut model, result_id)
            .ok_or_else(|| TestCaseError::fail("disjoint-union volume returned None"))?;

        prop_assert!(
            within_rel(v, v_expected, 0.05),
            "V(A ∪ B) = {} expected ≈ {} = V(A) + V(B) = {} + {}; \
             a_dims={:?} b_dims={:?} delta={:?}",
            v, v_expected, v_a, v_b, a_dims, b_dims, delta,
        );
    }

    /// Inclusion-exclusion (soft): for any two boxes,
    /// `V(A ∪ B) + V(A ∩ B) = V(A) + V(B)` within 10% relative tol.
    /// This is the *fitness function* for the boolean engine — the
    /// invariant holds analytically for *any* pair of solids, so a
    /// failure here means the engine produced inconsistent volumes
    /// on the same input pair.
    ///
    /// Operates on two concentric boxes (both at origin) so we
    /// always have a non-trivial overlap to measure. The 10%
    /// tolerance is intentionally loose — see module-level docs.
    ///
    /// Enabled after bug #51's pipeline fixes (commit 3ee1a63):
    /// once `V(A∪B)` and `V(A∩B)` are individually correct on these
    /// configurations the invariant holds automatically, as
    /// predicted in the original docstring.
    #[test]
    fn prop_inclusion_exclusion_consistency(
        a_dims in arb_small_dims(),
        b_dims in arb_small_dims(),
    ) {
        let mut model = BRepModel::new();
        let a = make_box(&mut model, a_dims.0, a_dims.1, a_dims.2);
        let b = make_box(&mut model, b_dims.0, b_dims.1, b_dims.2);
        let v_a = a_dims.0 * a_dims.1 * a_dims.2;
        let v_b = b_dims.0 * b_dims.1 * b_dims.2;

        let r_union = boolean_operation(
            &mut model, a, b, BooleanOp::Union, BooleanOptions::default(),
        );
        let union_id = match r_union { Ok(id) => id, Err(_) => return Ok(()) };
        let v_union = volume(&mut model, union_id)
            .ok_or_else(|| TestCaseError::fail("union volume None"))?;

        // Rebuild fresh inputs for the intersection — the union call
        // mutates the model, and re-using `a` / `b` after their
        // operands have been consumed by face-splitting would be
        // unsound. A fresh BRepModel is the cleanest reset.
        let mut model2 = BRepModel::new();
        let a2 = make_box(&mut model2, a_dims.0, a_dims.1, a_dims.2);
        let b2 = make_box(&mut model2, b_dims.0, b_dims.1, b_dims.2);
        let r_inter = boolean_operation(
            &mut model2, a2, b2, BooleanOp::Intersection, BooleanOptions::default(),
        );
        let inter_id = match r_inter { Ok(id) => id, Err(_) => return Ok(()) };
        let v_inter = volume(&mut model2, inter_id)
            .ok_or_else(|| TestCaseError::fail("intersection volume None"))?;

        let lhs = v_union + v_inter;
        let rhs = v_a + v_b;
        prop_assert!(
            within_rel(lhs, rhs, 0.10),
            "inclusion-exclusion violated: V(A∪B) + V(A∩B) = {} + {} = {}, \
             expected V(A) + V(B) = {} + {} = {}; a={:?} b={:?}",
            v_union, v_inter, lhs, v_a, v_b, rhs, a_dims, b_dims,
        );
    }
}

// -----------------------------------------------------------------------
// Negative-control regression tests
//
// Single-case versions of the most important properties using fixed
// inputs. These let the test suite double-check the analytical claims
// (and the volume helper) under a stable seed before the property
// tests amplify them.
// -----------------------------------------------------------------------

#[test]
fn neg_control_small_box_volume_is_8() {
    let mut model = BRepModel::new();
    let solid = make_box(&mut model, 2.0, 2.0, 2.0);
    let v = volume(&mut model, solid).expect("volume of a 2³ box must be computable");
    assert!((v - 8.0).abs() < 1e-6, "V(2×2×2) = {}, expected 8.0", v);
}

#[test]
fn neg_control_containment_difference_volume_matches_diff() {
    // Fixed: 10×10×10 big, 2×2×2 small. Difference must have V = 1000 − 8 = 992.
    let mut model = BRepModel::new();
    let big = make_box(&mut model, 10.0, 10.0, 10.0);
    let small = make_box(&mut model, 2.0, 2.0, 2.0);
    let diff = boolean_operation(
        &mut model,
        big,
        small,
        BooleanOp::Difference,
        BooleanOptions::default(),
    );
    match diff {
        Ok(id) => {
            let v = volume(&mut model, id)
                .expect("difference volume calculable");
            assert!(
                within_rel(v, 992.0, 0.05),
                "fixed-case difference: V = {}, expected ≈ 992",
                v,
            );
        }
        Err(e) => panic!("containment difference failed on fixed-case 10-vs-2 box: {e:?}"),
    }
}

#[test]
fn neg_control_far_translation_makes_boxes_disjoint() {
    // Translate a 2×2×2 box by (100, 0, 0). The bounding boxes are
    // [0,2]³ and [100,102]×[0,2]² respectively — separated by 98
    // units. After translation, volume must remain 8.0.
    let mut model = BRepModel::new();
    let solid = make_box(&mut model, 2.0, 2.0, 2.0);
    translate(&mut model, solid, Vector3::new(100.0, 0.0, 0.0));
    let v = volume(&mut model, solid).expect("translated volume calculable");
    // Translation is a rigid motion → volume invariant.
    assert!(
        (v - 8.0).abs() < 1e-6,
        "translation should preserve volume; got V = {}",
        v,
    );
    // Sanity: ignore the unused operand-error if `OperationError`'s
    // variants change. We only assert the volume invariant above.
    let _: Result<SolidId, OperationError> = Ok(solid);
}
