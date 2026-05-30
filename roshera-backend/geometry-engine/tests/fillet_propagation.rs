//! End-to-end regression tests for fillet edge-selection propagation
//! (Task #87 / Task #89 slice B).
//!
//! `propagate_tangent_edges` and `propagate_smooth_edges` are exercised
//! by `fillet_edges` whenever `FilletOptions::propagation` is set to
//! `Tangent`/`Smooth` (the default is `Tangent`). These tests pin the
//! observable invariants the public entry point must hold across
//! propagation modes:
//!
//!   * `Tangent` mode on a box (every edge meets its neighbours at 90°)
//!     must NOT expand the selection — the angle threshold in
//!     `are_edges_tangent` (≈ 5.7°) excludes perpendicular adjacents,
//!     so the resulting fillet must be identical to running with
//!     `None` mode on the same edge.
//!   * `None` mode is a strict passthrough — exactly one blend face
//!     per requested edge.
//!   * Propagation does not duplicate edges — picking the same edge
//!     under any mode produces a fillet equal to picking it once.
//!   * Propagation does not crash on a closed (rim) edge — the
//!     closed-edge dispatch in `create_closed_edge_fillet` must run
//!     identically regardless of propagation mode (no tangent
//!     neighbours exist for a self-loop with start_vertex ==
//!     end_vertex).
//!
//! The positive case (a chain of actually-tangent edges getting
//! propagated as one fillet) requires a B-Rep with smoothly joined
//! edges, which the kernel's primitives do not currently produce.
//! That positive coverage will land alongside the smooth-extrude /
//! polyline-with-tangent-arcs work (#87 follow-up); this file only
//! pins the negative + passthrough cases that the existing
//! infrastructure must already honour.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::fillet::{FilletType, PropagationMode};
use geometry_engine::operations::{fillet_edges, FilletOptions};
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

/// Pick an arbitrary open (V0 != V1) edge from the model. Box edges
/// are all open; this just returns the first one reported by the
/// EdgeStore iterator. Using "the first open edge" rather than
/// hard-coding an EdgeId keeps the test resilient to changes in the
/// box-construction order (the EdgeStore is append-only, so ID
/// stability across kernel revisions is not guaranteed).
fn first_open_edge(model: &BRepModel) -> EdgeId {
    model
        .edges
        .iter()
        .filter_map(|(id, edge)| if !edge.is_loop() { Some(id) } else { None })
        .next()
        .expect("box must have at least one open edge")
}

fn make_opts(radius: f64, propagation: PropagationMode) -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(radius),
        radius,
        propagation,
        ..Default::default()
    }
}

#[test]
fn none_propagation_adds_exactly_one_face_per_requested_edge() {
    // Baseline contract: with propagation explicitly disabled, exactly
    // one blend face is produced per requested edge. Pinned so that
    // any future change to `propagate_edge_selection` that accidentally
    // expands the `None` branch (e.g. by fall-through) is caught.
    let mut model = BRepModel::new();
    let solid = make_box(&mut model, 4.0, 4.0, 4.0);
    let edge = first_open_edge(&model);
    let face_count_before = model.faces.len();

    fillet_edges(
        &mut model,
        solid,
        vec![edge],
        make_opts(0.4, PropagationMode::None),
    )
    .expect("fillet with None propagation must succeed on a box edge");

    assert_eq!(
        model.faces.len(),
        face_count_before + 1,
        "PropagationMode::None on a single edge must produce exactly one new face"
    );
}

#[test]
fn tangent_propagation_on_box_edge_does_not_expand_selection() {
    // Every box edge meets its neighbours at 90°. The tangent-angle
    // threshold inside `are_edges_tangent` is ~5.7° (0.1 rad); 90°
    // sits an order of magnitude outside that band, so propagation
    // must be a no-op and the resulting fillet must be identical to
    // `PropagationMode::None`. This is the regression net for the
    // angle threshold — flipping a sign or comparing to π/2 instead
    // of 0 would silently include every adjacent box edge and the
    // assertion below would fail with `+12 faces` (every edge
    // filleted).
    let mut none_model = BRepModel::new();
    let none_solid = make_box(&mut none_model, 4.0, 4.0, 4.0);
    let none_edge = first_open_edge(&none_model);
    let none_face_count_before = none_model.faces.len();
    fillet_edges(
        &mut none_model,
        none_solid,
        vec![none_edge],
        make_opts(0.4, PropagationMode::None),
    )
    .expect("None-mode fillet must succeed on box edge");
    let none_added = none_model.faces.len() - none_face_count_before;

    let mut tan_model = BRepModel::new();
    let tan_solid = make_box(&mut tan_model, 4.0, 4.0, 4.0);
    let tan_edge = first_open_edge(&tan_model);
    let tan_face_count_before = tan_model.faces.len();
    fillet_edges(
        &mut tan_model,
        tan_solid,
        vec![tan_edge],
        make_opts(0.4, PropagationMode::Tangent),
    )
    .expect("Tangent-mode fillet must succeed on box edge");
    let tan_added = tan_model.faces.len() - tan_face_count_before;

    assert_eq!(
        tan_added, none_added,
        "Tangent propagation must not expand the selection on a box \
         (all neighbours at 90°); None added {none_added} face(s) but \
         Tangent added {tan_added}"
    );
}

#[test]
fn duplicate_input_edges_dedup_under_propagation() {
    // Picking the same edge twice should be idempotent — the
    // propagation step uses a `HashSet<EdgeId>` for the closure so the
    // second mention can never produce a second blend face. This pins
    // the dedup guarantee that `group_edges_into_chains` and
    // `create_fillet_chain` rely on (they assume each edge appears
    // once per chain).
    let mut model = BRepModel::new();
    let solid = make_box(&mut model, 4.0, 4.0, 4.0);
    let edge = first_open_edge(&model);
    let face_count_before = model.faces.len();

    fillet_edges(
        &mut model,
        solid,
        vec![edge, edge, edge],
        make_opts(0.4, PropagationMode::Tangent),
    )
    .expect("duplicate-edge fillet must succeed (dedup'd in propagation)");

    assert_eq!(
        model.faces.len(),
        face_count_before + 1,
        "duplicate edge mentions must produce exactly one blend face"
    );
}

#[test]
fn tangent_propagation_on_closed_rim_does_not_crash() {
    // Closed (rim) edges have start_vertex == end_vertex, so the only
    // "neighbour" the propagation walk can find through
    // `find_edges_at_vertex` is the edge itself. The `result.contains`
    // check in `propagate_tangent_edges` filters that out, leaving the
    // selection unchanged. This regression test pins that the closed-
    // edge dispatch in `create_closed_edge_fillet` runs identically
    // regardless of propagation mode — a defensive guard against any
    // future change that special-cases closed edges in propagation.
    let mut model = BRepModel::new();
    let solid = make_cylinder(&mut model, Point3::ORIGIN, Vector3::Z, 5.0, 10.0);
    let rim = model
        .edges
        .iter()
        .filter_map(|(id, edge)| if edge.is_loop() { Some(id) } else { None })
        .next()
        .expect("cylinder must have a rim edge");
    let face_count_before = model.faces.len();

    fillet_edges(
        &mut model,
        solid,
        vec![rim],
        make_opts(1.0, PropagationMode::Tangent),
    )
    .expect("Tangent-mode fillet on cylinder rim must succeed (closed-edge dispatch)");

    assert_eq!(
        model.faces.len(),
        face_count_before + 1,
        "Tangent-mode rim fillet must add exactly one torus blend face"
    );
}
