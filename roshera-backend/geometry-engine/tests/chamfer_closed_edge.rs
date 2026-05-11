//! Closed-edge chamfer regression tests (Task #89 — chamfer half).
//!
//! Mirror of `tests/fillet_closed_edge.rs` for the chamfer pipeline.
//! Cylinders, holes, and cones produce closed circular edges
//! (`Edge::is_loop()` — start_vertex == end_vertex). The default
//! open-edge chamfer template assumes V0 != V1 and would fail at the
//! cap-construction step with `Edge axis normalize failed:
//! DivisionByZero`. The chamfer closed-edge dispatch routes the
//! cylinder-rim case through `create_closed_edge_chamfer`, which
//! synthesises a cone-frustum blend in-line: the cap circle shrinks
//! from R to (R − d_cap), the lateral cylinder shortens by d_lat on
//! the rim side, and a new `Cone` surface (`u ∈ [0, 2π],
//! v ∈ [h_a, h_a + d_lat]` where `h_a = d_lat·(R − d_cap)/d_cap`)
//! joins them through a seamed-rectangle loop.
//!
//! These tests pin the contract for the cylinder-rim chamfer case:
//!   - the operation succeeds where the open-edge path used to
//!     surface `DivisionByZero`;
//!   - the open-edge seam (V0 != V1) keeps using the open-edge path;
//!   - distance bounds (`d_cap < R` and `d_lat < height`) reject
//!     pathological inputs cleanly;
//!   - the new blend face is a `Cone`-typed surface;
//!   - the post-chamfer shell remains watertight (every face-loop
//!     edge in exactly two loops);
//!   - the post-chamfer solid volume matches the analytical
//!     cylinder-minus-frustum result within mesh-divergence tolerance.

use std::collections::HashMap;
use std::f64::consts::PI;

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::chamfer::{
    chamfer_edges, ChamferOptions, ChamferType, PropagationMode,
};
use geometry_engine::operations::OperationError;
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

fn equal_chamfer_opts(d: f64) -> ChamferOptions {
    ChamferOptions {
        chamfer_type: ChamferType::EqualDistance(d),
        distance1: d,
        distance2: d,
        propagation: PropagationMode::None,
        ..Default::default()
    }
}

#[test]
fn cylinder_top_rim_chamfer_succeeds_with_cone_blend() {
    // Headline contract: chamfering a cylinder rim must succeed (where
    // the open-edge path surfaced DivisionByZero), and the resulting
    // solid must be intact — the original 3-face cylinder gains
    // exactly one new face (the cone-frustum blend) and the rim edge
    // is gone from the model.
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

    let result = chamfer_edges(&mut model, solid, vec![rim], equal_chamfer_opts(1.0));
    if let Err(err) = &result {
        if let OperationError::NumericalError(msg) = err {
            assert!(
                !msg.contains("DivisionByZero"),
                "regression: closed-edge chamfer leaked the raw DivisionByZero error \
                 from the open-edge cap construction. The closed-edge dispatch in \
                 create_edge_chamfer must intercept this case. Message: {msg}"
            );
        }
        panic!("cylinder rim chamfer should succeed; got {err:?}");
    }

    // Exactly one new face (the cone blend) should have been added.
    assert_eq!(
        model.faces.len(),
        face_count_before + 1,
        "cylinder rim chamfer must add exactly one new face (the cone blend)"
    );

    // The original rim edge should no longer be present in the model
    // — it was retired in step 11 of create_closed_edge_chamfer.
    //
    // `EdgeStore::remove` marks the slot as deleted (rewrites it to a
    // sentinel with `id == INVALID_EDGE_ID`) rather than freeing the
    // backing Vec slot, so `edges.get(rim)` would still return
    // `Some(&sentinel)`. The iterator at `EdgeStore::iter()` filters
    // these sentinel slots out, which is the semantically correct
    // "live edges only" view.
    assert!(
        !model.edges.iter().any(|(id, _)| id == rim),
        "rim edge {rim} should be retired from the live-edge iterator after the chamfer"
    );
}

#[test]
fn cylinder_all_rims_succeed_with_cone_blend() {
    // A cylinder has two closed rim edges (top and bottom). Both must
    // succeed independently — the rims have opposite outward
    // orientations and the closed-edge dispatch is sign-sensitive
    // (the rim-sign factor flips between top and bottom). Each fresh
    // model isolates the test case.
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

        let result = chamfer_edges(
            &mut working_model,
            working_solid,
            vec![target],
            equal_chamfer_opts(0.5),
        );
        assert!(
            result.is_ok(),
            "rim edge {target} (index {which}) should succeed; got {:?}",
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
fn cylinder_rim_chamfer_distance_too_large_rejected_cleanly() {
    // d_cap >= R collapses the cap circle to a point (or worse,
    // inverts it); d_lat >= height collapses the lateral surface.
    // Both should surface an InvalidGeometry error rather than panic
    // or silently succeed.
    let mut model = BRepModel::new();
    let solid = make_cylinder(&mut model, Point3::ORIGIN, Vector3::Z, 2.0, 5.0);
    let rim = closed_edges(&model)[0];

    // d = 2.0 equals R = 2.0 — cap shrinks to zero radius. Must be
    // rejected at the geometric-precondition gate.
    let opts_too_wide = equal_chamfer_opts(2.0);
    let err = chamfer_edges(&mut model, solid, vec![rim], opts_too_wide)
        .expect_err("over-cap-distance chamfer must error, not silently succeed or panic");
    assert!(
        matches!(err, OperationError::InvalidGeometry(_)),
        "expected InvalidGeometry for over-cap-distance chamfer; got {err:?}"
    );

    // d_lat = height collapses the lateral surface. Same error class.
    let mut model2 = BRepModel::new();
    let solid2 = make_cylinder(&mut model2, Point3::ORIGIN, Vector3::Z, 5.0, 1.0);
    let rim2 = closed_edges(&model2)[0];
    let opts_too_tall = equal_chamfer_opts(1.0);
    let err2 = chamfer_edges(&mut model2, solid2, vec![rim2], opts_too_tall)
        .expect_err("chamfer >= height must error cleanly");
    assert!(
        matches!(err2, OperationError::InvalidGeometry(_)),
        "expected InvalidGeometry for chamfer >= height; got {err2:?}"
    );
}

#[test]
fn cylinder_seam_edge_still_uses_open_path() {
    // Sanity: the cylinder's vertical seam edge connects the top and
    // bottom rim vertices — start_vertex != end_vertex. It must NOT be
    // routed through the closed-edge guard. We don't assert success
    // of the seam chamfer here (it has its own preconditions), only
    // that we don't observe an error message indicating the
    // closed-edge guard mis-fired.
    let mut model = BRepModel::new();
    let solid = make_cylinder(&mut model, Point3::ORIGIN, Vector3::Z, 5.0, 10.0);

    let open_edges: Vec<EdgeId> = model
        .edges
        .iter()
        .filter_map(|(id, edge)| if !edge.is_loop() { Some(id) } else { None })
        .collect();

    if open_edges.is_empty() {
        // Seamless cylinder topology — nothing to assert.
        return;
    }

    let seam = open_edges[0];
    let result = chamfer_edges(&mut model, solid, vec![seam], equal_chamfer_opts(0.5));
    if let Err(OperationError::NotImplemented(msg)) = &result {
        assert!(
            !(msg.contains("closed") || msg.contains("rim") || msg.contains("loop")),
            "seam edge {seam} (start != end) must NOT be routed through the closed-edge \
             guard. Got: {msg}",
        );
    }
}

/// Comprehensive regression net for `create_closed_edge_chamfer`. Pins
/// the invariants the topology surgery must hold after a successful
/// run:
///
///   1. **Surface type** — the new face's surface is a `Cone`
///      (downcast via `SurfaceType::Cone`). Catches a regression
///      where the blend face is built on a Plane / Cylinder / NURBS
///      placeholder, or a Torus (the fillet shape).
///   2. **Edge curve types in the blend loop** — the four loop slots
///      are: `lat_trim_edge` arc-circle, `cone_seam_edge` line,
///      `cap_trim_edge` arc-circle, `cone_seam_edge` again backward.
///      So the blend loop holds exactly **2 arcs + 2 line slots**
///      (the same line referenced twice). The chamfer's straight
///      seam is the geometric difference from the fillet's
///      quarter-arc seam — pin it here so any regression that
///      substitutes an arc trips this test.
///   3. **Watertight shell** — every edge referenced from a face
///      loop appears in exactly two loops. Same contract enforced
///      by `tests/blend_topology_regression.rs::assert_no_boundary_edges`,
///      reproduced inline here so this regression net stays
///      self-contained.
///   4. **Analytical volume** — chamfering one rim removes a
///      cylindrical ring of height `d_lat` and adds back a
///      cone-frustum of the same height with radii `R` (at the
///      original cap height) and `R − d_cap` (at the new cap
///      height). Net removed:
///
///          V_ring    = π · R² · d_lat
///          V_frustum = (π · d_lat / 3) · (R² + R·(R−d_cap) + (R−d_cap)²)
///          V_removed = V_ring − V_frustum
///          V_expected = π · R² · H − V_removed
///
///      The 5 % relative tolerance matches the fillet test's
///      tolerance — same `TessellationParams::fine()` mesh-divergence
///      fallback applies once the shell carries a cone surface that
///      the analytical loop traversal can't close.
#[test]
fn chamfer_closed_edge_cone_blend() {
    let mut model = BRepModel::new();
    let big_r: f64 = 5.0;
    let height: f64 = 10.0;
    let d: f64 = 1.0;
    let solid = make_cylinder(&mut model, Point3::ORIGIN, Vector3::Z, big_r, height);

    let rim = closed_edges(&model)[0];
    let face_count_before = model.faces.len();

    chamfer_edges(&mut model, solid, vec![rim], equal_chamfer_opts(d))
        .expect("rim chamfer succeeds");

    // Exactly one new face was added.
    assert_eq!(
        model.faces.len(),
        face_count_before + 1,
        "chamfer must add exactly one new (blend) face"
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
        SurfaceType::Cone,
        "blend face surface must be a Cone, got {:?}",
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
                "edge {eid} in blend loop has unsupported curve type for a cone blend"
            );
        }
    }
    assert_eq!(
        arc_refs, 2,
        "blend loop must reference exactly 2 arcs (lat_trim_edge, cap_trim_edge); got {arc_refs}"
    );
    assert_eq!(
        line_refs, 2,
        "blend loop must reference the cone seam line exactly 2× (forward + backward); \
         got {line_refs}. The chamfer's straight seam is what distinguishes a cone \
         blend from the fillet's quarter-arc torus seam."
    );

    // ---- (3) watertight shell ----
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
        "post-chamfer shell must remain watertight (every face-loop edge in \
         exactly 2 loops); boundary/over-shared edges: {bad:?}"
    );

    // ---- (4) analytical volume check ----
    let v0 = PI * big_r * big_r * height;
    let r_small = big_r - d;
    let v_ring = PI * big_r * big_r * d;
    let v_frustum =
        (PI * d / 3.0) * (big_r * big_r + big_r * r_small + r_small * r_small);
    let v_removed = v_ring - v_frustum;
    let expected = v0 - v_removed;
    let actual = model
        .calculate_solid_volume(solid)
        .expect("solid volume must be computable post-chamfer (mesh fallback)");
    let rel_err = ((actual - expected) / expected).abs();
    assert!(
        rel_err < 0.05,
        "post-chamfer cylinder volume {actual} should be ≈ {expected} \
         (V0 = {v0:.4}, V_removed ≈ {v_removed:.4}); rel_err = {rel_err:.4}"
    );
}
