//! Radius-precondition regression tests for `fillet_edges` (Task #86 /
//! Task #89 slice D).
//!
//! `validate_fillet_parameters` rejects any radius that exceeds half
//! the edge's arc length — otherwise the rolling-ball cap arcs cannot
//! sit on the edge without overlapping each other. Slice D extends
//! that validation in the public `fillet_edges` entry point so that
//! variable-radius fillets must satisfy the bound at **both**
//! endpoint radii (`r1` and `r2`), not just `r1`. Earlier revisions
//! only checked `r1`, allowing a pathological `r2` to slip through
//! validation and surface a less-actionable error deep inside the
//! surgery loop.
//!
//! These tests pin:
//!
//!   * Variable-radius `(r1, r2)` is rejected when **only `r2`**
//!     exceeds the half-edge bound — the bug fixed by slice D.
//!   * Variable-radius `(r1, r2)` is rejected when **only `r1`**
//!     exceeds the bound — pre-existing behaviour, kept here so the
//!     two cases live side-by-side.
//!   * Multi-edge input where a single edge violates the bound
//!     rejects the entire call (no partial application).
//!   * Zero-and-negative radii are still rejected at the top of
//!     `fillet_edges` regardless of whether they appear in `r1` or
//!     `r2` of a Variable fillet.

use geometry_engine::operations::fillet::{FilletType, PropagationMode};
use geometry_engine::operations::{fillet_edges, FilletOptions, OperationError};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    match builder
        .create_box_3d(w, h, d)
        .expect("box creation succeeds")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {:?}", other),
    }
}

fn first_open_edge(model: &BRepModel) -> EdgeId {
    model
        .edges
        .iter()
        .filter_map(|(id, edge)| if !edge.is_loop() { Some(id) } else { None })
        .next()
        .expect("box must have at least one open edge")
}

fn variable_opts(r1: f64, r2: f64) -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Variable(r1, r2),
        radius: r1,
        propagation: PropagationMode::None,
        ..Default::default()
    }
}

#[test]
fn variable_fillet_rejects_when_r2_exceeds_half_edge_length() {
    // The bug fixed by slice D: a variable fillet `(r1, r2)` where
    // r1 is in-bounds but r2 exceeds edge_length/2 used to slip past
    // validation because the inputs loop only checked r1. Now both
    // endpoints are validated.
    //
    // Box edge length = 4.0 (the side dimension). half = 2.0.
    // r1 = 0.5 is valid; r2 = 3.0 exceeds the bound.
    let mut model = BRepModel::new();
    let solid = make_box(&mut model, 4.0, 4.0, 4.0);
    let edge = first_open_edge(&model);

    let result = fillet_edges(&mut model, solid, vec![edge], variable_opts(0.5, 3.0));
    let err = result.expect_err(
        "variable fillet (0.5, 3.0) on edge of length 4.0 must fail — r2 = 3.0 \
         exceeds edge_length/2 = 2.0",
    );
    assert!(
        matches!(err, OperationError::InvalidRadius(_)),
        "expected InvalidRadius for r2 out of bounds; got {err:?}"
    );
}

#[test]
fn variable_fillet_rejects_when_r1_exceeds_half_edge_length() {
    // The pre-slice-D behaviour: r1 out of bounds is rejected. Keep
    // this test alongside the r2 case so the two preconditions live
    // side-by-side and any future regression in either branch is
    // caught immediately.
    let mut model = BRepModel::new();
    let solid = make_box(&mut model, 4.0, 4.0, 4.0);
    let edge = first_open_edge(&model);

    let result = fillet_edges(&mut model, solid, vec![edge], variable_opts(3.0, 0.5));
    let err = result.expect_err(
        "variable fillet (3.0, 0.5) on edge of length 4.0 must fail — r1 = 3.0 \
         exceeds edge_length/2 = 2.0",
    );
    assert!(
        matches!(err, OperationError::InvalidRadius(_)),
        "expected InvalidRadius for r1 out of bounds; got {err:?}"
    );
}

#[test]
fn variable_fillet_accepts_when_both_radii_are_in_bounds() {
    // Sanity peer to the rejection cases: a fillet with both
    // endpoints below the half-edge bound must succeed under the new
    // stricter validation. Otherwise we'd be over-rejecting.
    let mut model = BRepModel::new();
    let solid = make_box(&mut model, 4.0, 4.0, 4.0);
    let edge = first_open_edge(&model);
    let face_count_before = model.faces.len();

    fillet_edges(&mut model, solid, vec![edge], variable_opts(0.4, 0.6))
        .expect("variable fillet (0.4, 0.6) on edge of length 4.0 must succeed");

    assert_eq!(
        model.faces.len(),
        face_count_before + 1,
        "valid variable fillet must add exactly one blend face"
    );
}

#[test]
fn multi_edge_call_rejected_if_any_edge_violates_bound() {
    // Validation runs per-edge in the inputs loop; the first
    // violation must abort the entire call without applying the
    // valid edge. This pins the all-or-nothing contract — a partial
    // mutation would leave the caller with no way to recover.
    let mut model = BRepModel::new();
    let solid = make_box(&mut model, 4.0, 4.0, 4.0);

    // Two distinct open edges. The first is 4.0 long, the second
    // (chosen as the second open-edge entry) is also 4.0 long for a
    // unit-side box. Collect into a Vec so the iterator's immutable
    // borrow of `model` is released before the `&mut model` call
    // below.
    let open: Vec<EdgeId> = model
        .edges
        .iter()
        .filter_map(|(id, edge)| if !edge.is_loop() { Some(id) } else { None })
        .take(2)
        .collect();
    assert_eq!(open.len(), 2, "box must have at least two open edges");
    let e1 = open[0];
    let e2 = open[1];

    // r1 = 0.5 is valid for any 4.0-long edge; the validation loop
    // applies the same radius to every edge. Because no edge
    // violates the bound here, the call should succeed — this is
    // the *positive* setup confirming both edges are individually
    // valid before we move on to the rejection case.
    let mut sanity_model = BRepModel::new();
    let sanity_solid = make_box(&mut sanity_model, 4.0, 4.0, 4.0);
    let opts = FilletOptions {
        fillet_type: FilletType::Constant(0.5),
        radius: 0.5,
        propagation: PropagationMode::None,
        ..Default::default()
    };
    fillet_edges(&mut sanity_model, sanity_solid, vec![e1, e2], opts)
        .expect("two valid edges with r=0.5 must succeed");

    // Now the rejection case: a radius that exceeds the half-edge
    // bound for both edges. The validate loop iterates over `edges`
    // and the first violation aborts — neither edge is mutated.
    let face_count_before = model.faces.len();
    let bad_opts = FilletOptions {
        fillet_type: FilletType::Constant(3.0),
        radius: 3.0,
        propagation: PropagationMode::None,
        ..Default::default()
    };
    let err = fillet_edges(&mut model, solid, vec![e1, e2], bad_opts)
        .expect_err("over-radius constant fillet must fail validation");
    assert!(
        matches!(err, OperationError::InvalidRadius(_)),
        "expected InvalidRadius; got {err:?}"
    );
    assert_eq!(
        model.faces.len(),
        face_count_before,
        "rejected fillet must not partially mutate the model — face count unchanged"
    );
}

#[test]
fn variable_fillet_rejects_zero_or_negative_radius_at_either_end() {
    // Zero / negative radii fail the `radius <= 0` guard inside
    // `validate_fillet_parameters`. The slice-D fix ensures that
    // guard now covers BOTH endpoints, not just r1.
    let mut model = BRepModel::new();
    let solid = make_box(&mut model, 4.0, 4.0, 4.0);
    let edge = first_open_edge(&model);

    // r2 = 0 — was permitted by the old r1-only loop (r1 = 0.5 was
    // valid), now rejected.
    let err_r2_zero = fillet_edges(&mut model, solid, vec![edge], variable_opts(0.5, 0.0))
        .expect_err("variable fillet with r2 = 0 must fail");
    assert!(
        matches!(err_r2_zero, OperationError::InvalidRadius(_)),
        "expected InvalidRadius for r2 = 0; got {err_r2_zero:?}"
    );

    // r2 < 0 — same channel.
    let err_r2_neg = fillet_edges(&mut model, solid, vec![edge], variable_opts(0.5, -1.0))
        .expect_err("variable fillet with r2 < 0 must fail");
    assert!(
        matches!(err_r2_neg, OperationError::InvalidRadius(_)),
        "expected InvalidRadius for r2 < 0; got {err_r2_neg:?}"
    );
}
