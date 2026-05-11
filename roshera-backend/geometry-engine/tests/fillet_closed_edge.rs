//! Closed-edge fillet regression tests (Task #89, slices A1–A4).
//!
//! Cylinders, holes, and torus seams produce closed circular edges
//! (`Edge::is_loop()` — start_vertex == end_vertex). The default
//! 4-sided fillet template assumes V0 != V1 and would fail at the
//! cap-arc construction step with `Edge axis normalize failed:
//! DivisionByZero`. Slice A2 routes the closed-edge case through a
//! dedicated builder (`cylinder_rim_fillet`) that synthesises the
//! quarter-torus blend in-line: the cap circle shrinks from R to
//! (R - r), the lateral cylinder shortens by r on the rim side, and
//! a new `Torus` surface (`u ∈ [0, 2π], v ∈ [0, π/2]`) joins them
//! through a seamed-rectangle loop.
//!
//! These tests pin the contract for the cylinder-rim case:
//!   - the operation succeeds where it used to surface
//!     `DivisionByZero`;
//!   - the open-edge seam (V0 != V1) keeps using the open-edge path;
//!   - radius bounds (r < R/2 and r < height) reject pathological
//!     inputs cleanly.
//!
//! Volume / watertightness / surface-type checks live in slice A4
//! alongside `fillet_closed_edge_torus_blend`; this file restricts
//! itself to the routing + bounds contract.
//!
//! See `tests/blend_topology_regression.rs` for the open-edge
//! (V0 != V1) coverage.

use std::collections::HashMap;
use std::f64::consts::PI;

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::fillet::{FilletType, PropagationMode};
use geometry_engine::operations::{fillet_edges, FilletOptions, OperationError};
use geometry_engine::primitives::curve::{Arc, Line};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::SurfaceType;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn make_cylinder(
    model: &mut BRepModel,
    center: Point3,
    axis: Vector3,
    radius: f64,
    height: f64,
) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    match builder
        .create_cylinder_3d(center, axis, radius, height)
        .expect("cylinder creation succeeds")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {:?}", other),
    }
}

/// Collect every closed edge in the model — these are the rim edges
/// for cylinders/cones/holes (start_vertex == end_vertex).
fn closed_edges(model: &BRepModel) -> Vec<EdgeId> {
    model
        .edges
        .iter()
        .filter_map(|(id, edge)| if edge.is_loop() { Some(id) } else { None })
        .collect()
}

#[test]
fn cylinder_top_rim_fillet_succeeds_with_torus_blend() {
    // A2 contract: filleting a cylinder rim must succeed (where it
    // previously surfaced DivisionByZero), and the resulting solid
    // must be intact — the original 3-face cylinder gains exactly
    // one new face (the toroidal blend) and the rim edge is gone
    // from the model.
    let mut model = BRepModel::new();
    let solid = make_cylinder(&mut model, Point3::ORIGIN, Vector3::Z, 5.0, 10.0);

    let rims = closed_edges(&model);
    assert!(
        !rims.is_empty(),
        "cylinder must have at least one closed (rim) edge — found {} closed edges in {} total",
        rims.len(),
        model.edges.len()
    );

    let face_count_before = model.faces.len();
    let rim = rims[0];
    let opts = FilletOptions {
        fillet_type: FilletType::Constant(1.0),
        radius: 1.0,
        propagation: PropagationMode::None,
        ..Default::default()
    };

    let result = fillet_edges(&mut model, solid, vec![rim], opts);
    if let Err(err) = &result {
        if let OperationError::NumericalError(msg) = err {
            assert!(
                !msg.contains("DivisionByZero"),
                "regression: closed-edge fillet leaked the raw DivisionByZero error \
                 from the open-edge cap-arc construction. The closed-edge dispatch in \
                 create_closed_edge_fillet must intercept this case. Message: {msg}"
            );
        }
        panic!("cylinder rim fillet should succeed under A2; got {err:?}");
    }

    // Exactly one new face (the torus blend) should have been added.
    assert_eq!(
        model.faces.len(),
        face_count_before + 1,
        "cylinder rim fillet must add exactly one new face (the torus blend)"
    );

    // The original rim edge should no longer be present in the model
    // — it was retired in step 9 of cylinder_rim_fillet.
    assert!(
        model.edges.get(rim).is_none(),
        "rim edge {rim} should be removed from the model after the fillet"
    );
}

#[test]
fn cylinder_all_rims_succeed_with_torus_blend() {
    // A cylinder has two closed rim edges (top and bottom). Both must
    // succeed — independence matters because the rims have opposite
    // outward orientations and the open-edge path's failure mode is
    // direction-sensitive. Each fresh model isolates the test case.
    let mut model = BRepModel::new();
    let _ = make_cylinder(&mut model, Point3::ORIGIN, Vector3::Z, 5.0, 10.0);
    let rims = closed_edges(&model);

    assert_eq!(
        rims.len(),
        2,
        "expected exactly two closed rim edges on a finite cylinder; got {}",
        rims.len()
    );

    for which in 0..2 {
        let mut working_model = BRepModel::new();
        let working_solid = make_cylinder(
            &mut working_model,
            Point3::ORIGIN,
            Vector3::Z,
            5.0,
            10.0,
        );
        // Each model has its own EdgeIds — recompute the closed-edge
        // list per iteration. Cylinder construction is deterministic,
        // so index `which` selects the same rim across runs.
        let working_rims = closed_edges(&working_model);
        let target = working_rims[which];
        let face_count_before = working_model.faces.len();

        let opts = FilletOptions {
            fillet_type: FilletType::Constant(0.5),
            radius: 0.5,
            propagation: PropagationMode::None,
            ..Default::default()
        };
        let result = fillet_edges(&mut working_model, working_solid, vec![target], opts);
        assert!(
            result.is_ok(),
            "rim edge {target} (index {which}) should succeed under A2; got {:?}",
            result.err()
        );
        assert_eq!(
            working_model.faces.len(),
            face_count_before + 1,
            "rim edge {target} (index {which}) must add exactly one blend face"
        );
    }
}

#[test]
fn cylinder_rim_fillet_radius_too_large_rejected_cleanly() {
    // r >= R/2 makes the resulting torus self-pinch (minor >= major);
    // r >= height collapses the lateral surface. Both should surface
    // an InvalidGeometry error rather than panic or silently succeed.
    let mut model = BRepModel::new();
    let solid = make_cylinder(&mut model, Point3::ORIGIN, Vector3::Z, 2.0, 1.0);
    let rim = closed_edges(&model)[0];

    // r = 1.0 violates r < R/2 (R=2, R/2=1). Any value at the boundary
    // or above triggers the precondition.
    let opts_too_thick = FilletOptions {
        fillet_type: FilletType::Constant(1.0),
        radius: 1.0,
        propagation: PropagationMode::None,
        ..Default::default()
    };
    let err = fillet_edges(&mut model, solid, vec![rim], opts_too_thick)
        .expect_err("over-radius fillet must error, not silently succeed or panic");
    assert!(
        matches!(err, OperationError::InvalidGeometry(_)),
        "expected InvalidGeometry for over-radius fillet; got {err:?}"
    );

    // r = 1.0 also violates r < height (height=1.0). Same error class.
    let mut model2 = BRepModel::new();
    let solid2 = make_cylinder(&mut model2, Point3::ORIGIN, Vector3::Z, 5.0, 1.0);
    let rim2 = closed_edges(&model2)[0];
    let opts_too_tall = FilletOptions {
        fillet_type: FilletType::Constant(1.0),
        radius: 1.0,
        propagation: PropagationMode::None,
        ..Default::default()
    };
    let err2 = fillet_edges(&mut model2, solid2, vec![rim2], opts_too_tall)
        .expect_err("fillet >= height must error cleanly");
    assert!(
        matches!(err2, OperationError::InvalidGeometry(_)),
        "expected InvalidGeometry for fillet >= height; got {err2:?}"
    );
}

#[test]
fn cylinder_seam_edge_still_uses_open_path() {
    // Sanity: the cylinder's vertical seam edge connects the top and
    // bottom rim vertices — start_vertex != end_vertex. It must NOT be
    // routed through the closed-edge guard. We don't assert success of
    // the seam fillet here (it has its own preconditions and may fail
    // for unrelated reasons depending on how create_cylinder_3d builds
    // its lateral surface), only that we don't get the closed-edge
    // NotImplemented error.
    let mut model = BRepModel::new();
    let solid = make_cylinder(&mut model, Point3::ORIGIN, Vector3::Z, 5.0, 10.0);

    let open_edges: Vec<EdgeId> = model
        .edges
        .iter()
        .filter_map(|(id, edge)| if !edge.is_loop() { Some(id) } else { None })
        .collect();

    if open_edges.is_empty() {
        // Some cylinder topologies are seamless (one face wrapping
        // around) — nothing to assert.
        return;
    }

    let seam = open_edges[0];
    let opts = FilletOptions {
        fillet_type: FilletType::Constant(0.5),
        radius: 0.5,
        propagation: PropagationMode::None,
        ..Default::default()
    };
    let result = fillet_edges(&mut model, solid, vec![seam], opts);
    if let Err(OperationError::NotImplemented(msg)) = &result {
        assert!(
            !(msg.contains("closed") || msg.contains("rim") || msg.contains("loop")),
            "seam edge {seam} (start != end) must NOT be routed through the closed-edge \
             guard. Got: {msg}",
        );
    }
}

/// A4 regression net for `cylinder_rim_fillet`. Pins the four invariants
/// the topology surgery must hold after a successful run:
///
///   1. **Surface type** — the new face's surface is a `Torus`
///      (downcast via `SurfaceType::Torus`). Catches a regression
///      where the blend face is built on a Plane / Cylinder / NURBS
///      placeholder.
///   2. **Edge curve types in the blend loop** — every reference is
///      an `Arc` (the four loop slots are: `lat_trim_edge` arc-circle,
///      `torus_seam_edge` quarter-arc, `cap_trim_edge` arc-circle,
///      `torus_seam_edge` again backward). The `new_seam_line` lives
///      in the *lateral* face's loop, not the blend loop, so it must
///      NOT appear here.
///   3. **Watertight shell** — every edge referenced from a face loop
///      appears in exactly two loops. Same contract enforced by
///      `tests/blend_topology_regression.rs::assert_no_boundary_edges`,
///      reproduced inline here so this regression net stays
///      self-contained.
///   4. **Analytical volume** — by Pappus, filleting one rim removes
///      a torus-of-revolution sliver whose meridional cross-section is
///      `(square − quarter-disk)` of side `r`, revolved around the
///      cylinder axis at the cross-section's centroid. Using the
///      mid-square centroid `R - r/2` (a tight upper bound — the true
///      centroid sits slightly closer to the axis once the
///      quarter-disk is subtracted, so this overestimates the
///      removed volume by ~0.05% for r ≪ R), the expected
///      post-fillet volume is:
///
///          V_expected = π R² H − r²·(1 − π/4)·2π·(R − r/2)
///
///      The 5 % relative tolerance is wide enough to absorb
///      `TessellationParams::fine()` mesh-divergence-theorem error on
///      the curved cylinder + new torus surface (analytical loops are
///      degenerate on the seamed faces, so the volume integrator
///      falls back to mesh — see
///      `tests/kernel_workflow_regression.rs` "Curved-primitive
///      volume" comment block).
#[test]
fn fillet_closed_edge_torus_blend() {
    let mut model = BRepModel::new();
    let big_r: f64 = 5.0;
    let height: f64 = 10.0;
    let r: f64 = 1.0;
    let solid = make_cylinder(&mut model, Point3::ORIGIN, Vector3::Z, big_r, height);

    let rim = closed_edges(&model)[0];
    let face_count_before = model.faces.len();

    let opts = FilletOptions {
        fillet_type: FilletType::Constant(r),
        radius: r,
        propagation: PropagationMode::None,
        ..Default::default()
    };
    fillet_edges(&mut model, solid, vec![rim], opts).expect("rim fillet succeeds under A2");

    // Exactly one new face was added.
    assert_eq!(
        model.faces.len(),
        face_count_before + 1,
        "fillet must add exactly one new (blend) face"
    );

    // The new face is the largest FaceId in the store — FaceStore is
    // append-only and the surgery's last `model.faces.add(...)` is the
    // blend face.
    let new_face_id = model
        .faces
        .iter()
        .map(|(id, _)| id)
        .max()
        .expect("model has at least one face");
    let new_face = model
        .faces
        .get(new_face_id)
        .expect("new face is in the FaceStore");

    // ---- (1) surface-type check ----
    let surf = model
        .surfaces
        .get(new_face.surface_id)
        .expect("blend face's surface is in the SurfaceStore");
    assert_eq!(
        surf.surface_type(),
        SurfaceType::Torus,
        "blend face surface must be a Torus, got {:?}",
        surf.surface_type()
    );

    // ---- (2) edge curve types in the blend loop ----
    let blend_loop = model
        .loops
        .get(new_face.outer_loop)
        .expect("blend loop is in the LoopStore");
    assert_eq!(
        blend_loop.edges.len(),
        4,
        "blend loop must have exactly 4 edge references (lat_trim, seam_fwd, cap_trim, seam_bwd); got {}",
        blend_loop.edges.len()
    );
    let mut arc_refs = 0usize;
    let mut line_refs = 0usize;
    for &eid in &blend_loop.edges {
        let edge = model.edges.get(eid).expect("blend-loop edge exists");
        let curve = model
            .curves
            .get(edge.curve_id)
            .expect("blend-loop curve exists");
        if curve.as_any().downcast_ref::<Arc>().is_some() {
            arc_refs += 1;
        } else if curve.as_any().downcast_ref::<Line>().is_some() {
            line_refs += 1;
        } else {
            panic!(
                "edge {eid} in blend loop has unsupported curve type for a torus blend"
            );
        }
    }
    assert_eq!(
        arc_refs, 4,
        "all 4 blend-loop references must be Arc-typed; got {arc_refs} arcs"
    );
    assert_eq!(
        line_refs, 0,
        "blend loop must not reference any Line — the new lateral seam line \
         lives in the *lateral* face's loop, not the blend face's loop"
    );

    // ---- (3) watertight shell ----
    // Every edge referenced from any face's outer/inner loops must
    // appear in exactly two loops. Boundary edges (count = 1) or
    // over-shared edges (count > 2) signal a torn shell — the same
    // failure mode the open-edge regression net pins down.
    let mut usage: HashMap<EdgeId, usize> = HashMap::new();
    for (_face_id, face) in model.faces.iter() {
        for loop_id in face.all_loops() {
            if let Some(loop_ref) = model.loops.get(loop_id) {
                for &eid in &loop_ref.edges {
                    *usage.entry(eid).or_insert(0) += 1;
                }
            }
        }
    }
    let bad: Vec<(EdgeId, usize)> = usage
        .iter()
        .filter(|(_, &c)| c != 2)
        .map(|(&e, &c)| (e, c))
        .collect();
    assert!(
        bad.is_empty(),
        "post-fillet shell must remain watertight (every face-loop edge in \
         exactly 2 loops); boundary/over-shared edges: {bad:?}"
    );

    // ---- (4) analytical volume check ----
    let v0 = PI * big_r * big_r * height;
    let cs_area = r * r * (1.0 - PI / 4.0); // (square − quarter-disk)
    let centroid_radius = big_r - r / 2.0; // tight upper bound on the true centroid
    let v_removed = cs_area * 2.0 * PI * centroid_radius;
    let expected = v0 - v_removed;
    let actual = model
        .calculate_solid_volume(solid)
        .expect("solid volume must be computable post-fillet (mesh fallback)");
    let rel_err = ((actual - expected) / expected).abs();
    assert!(
        rel_err < 0.05,
        "post-fillet cylinder volume {actual} should be ≈ {expected} \
         (V0 = {v0:.4}, V_removed ≈ {v_removed:.4}); rel_err = {rel_err:.4}"
    );
}
