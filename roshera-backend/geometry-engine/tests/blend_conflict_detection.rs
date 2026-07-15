// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! CF-α — chamfer × fillet conflict detection.
//!
//! Pins the contract introduced by
//! `operations::lifecycle::validate_blend_conflict`: when a caller
//! invokes `fillet_edges` (or `chamfer_edges`) on an edge that has
//! already participated in a blend on the host `Solid`, the kernel
//! must surface a typed
//! [`BlendFailure::ConflictingBlendKind`] *pre-flight* — before
//! `check_edges_exist` would otherwise reject the call with the
//! generic `"edge not found in model"` string.
//!
//! The per-`Solid` blend registry (`blended_edges` +
//! `blended_vertices`) is the storage backing this gate; the
//! `BlendKind::{Fillet,Chamfer}` tag carries the kind that the gate
//! reports through the `ConflictingBlendKind` payload.
//!
//! These tests pin:
//!
//! 1. **Chamfer → Fillet same edge** — the second call returns
//!    `OperationError::InvalidInput` whose `received` carries the
//!    typed signal (kind names + edge id).
//! 2. **Fillet → Chamfer same edge** — mirror.
//! 3. **Chamfer → Chamfer same edge** — same-kind same-edge is also
//!    a conflict (the edge no longer exists).
//! 4. **Disjoint-edge re-blend on the same solid** — a second blend
//!    on a *different* edge that does not share any vertex with the
//!    previously-blended edge proceeds without a false-positive
//!    rejection.

#[path = "blend_fixtures/mod.rs"]
mod blend_fixtures;

use blend_fixtures::{make_cube, vertex_at};

use geometry_engine::operations::chamfer::{
    chamfer_edges, ChamferOptions, ChamferType, PropagationMode as ChamferProp,
};
use geometry_engine::operations::fillet::{FilletType, PropagationMode as FilletProp};
use geometry_engine::operations::{fillet_edges, CommonOptions, FilletOptions, OperationError};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::BlendKind;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::vertex::VertexId;

const BOX_SIZE: f64 = 10.0;
const HALF_BOX: f64 = BOX_SIZE / 2.0;
const RADIUS: f64 = 0.5;
const OFFSET: f64 = 0.5;

/// Pick one edge incident to `vertex`. Used to grab a single edge to
/// chamfer/fillet without grabbing the whole 3-edge corner — the
/// CF-α gate is a single-edge contract and we do not want to drag the
/// F2-γ.1 corner-compatibility gate into the tests.
fn pick_one_edge_at_vertex(model: &BRepModel, vertex: VertexId) -> EdgeId {
    model
        .edges
        .iter()
        .find_map(|(id, edge)| {
            if edge.start_vertex == vertex || edge.end_vertex == vertex {
                Some(id)
            } else {
                None
            }
        })
        .expect("at least one edge incident to vertex")
}

fn fillet_opts() -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(RADIUS),
        radius: RADIUS,
        propagation: FilletProp::None,
        common: CommonOptions {
            // Keep the per-call validator on — the registry write
            // sits before `record_operation` so any post-op
            // validation rejection would also block the registry
            // entry, and we want the registry to survive into the
            // second call.
            validate_result: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn chamfer_opts() -> ChamferOptions {
    ChamferOptions {
        chamfer_type: ChamferType::EqualDistance(OFFSET),
        distance1: OFFSET,
        distance2: OFFSET,
        symmetric: true,
        propagation: ChamferProp::None,
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Helper — assert that `err` is an `InvalidInput` whose `received`
/// string carries the CF-α ConflictingBlendKind signal for the given
/// edge and (existing, requested) kinds. The mapping path is
/// `BlendFailure::ConflictingBlendKind` →
/// `OperationError::InvalidInput { parameter = "blend", received =
/// failure.to_string() }` (see `operations::diagnostics::From`).
fn assert_conflict(err: OperationError, edge: EdgeId, existing: BlendKind, requested: BlendKind) {
    match err {
        OperationError::InvalidInput {
            parameter,
            received,
            ..
        } => {
            assert_eq!(
                parameter, "blend",
                "ConflictingBlendKind must map onto the `blend` parameter slot"
            );
            assert!(
                received.contains(&format!("edge {}", edge)),
                "received payload missing the offending edge id; got: {}",
                received
            );
            assert!(
                received.contains(&format!("existing kind {}", existing)),
                "received payload missing existing kind tag {}; got: {}",
                existing,
                received
            );
            assert!(
                received.contains(&format!("a {}", requested)),
                "received payload missing requested kind tag {}; got: {}",
                requested,
                received
            );
        }
        other => panic!(
            "expected InvalidInput carrying ConflictingBlendKind, got {:?}",
            other
        ),
    }
}

#[test]
fn chamfer_then_fillet_same_edge_returns_typed_conflict() {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);
    let edge = pick_one_edge_at_vertex(&model, corner);

    chamfer_edges(&mut model, solid_id, vec![edge], chamfer_opts())
        .expect("first chamfer on a fresh single edge succeeds");

    // Second call asks for the same (now-destroyed) edge. Without
    // CF-α this would surface as `edge not found in model`. With
    // CF-α the validate_blend_conflict gate fires first and produces
    // the typed signal.
    let err = fillet_edges(&mut model, solid_id, vec![edge], fillet_opts())
        .expect_err("fillet on a chamfered edge must fail");

    assert_conflict(err, edge, BlendKind::Chamfer, BlendKind::Fillet);
}

#[test]
fn fillet_then_chamfer_same_edge_returns_typed_conflict() {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);
    let edge = pick_one_edge_at_vertex(&model, corner);

    fillet_edges(&mut model, solid_id, vec![edge], fillet_opts())
        .expect("first fillet on a fresh single edge succeeds");

    let err = chamfer_edges(&mut model, solid_id, vec![edge], chamfer_opts())
        .expect_err("chamfer on a filleted edge must fail");

    assert_conflict(err, edge, BlendKind::Fillet, BlendKind::Chamfer);
}

#[test]
fn chamfer_then_chamfer_same_edge_returns_typed_conflict() {
    // Same-kind same-edge is also a conflict — the edge no longer
    // exists in the model, and CF-α deliberately routes that case
    // through the typed signal too (rather than falling back to the
    // legacy `edge not found` shape).
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);
    let edge = pick_one_edge_at_vertex(&model, corner);

    chamfer_edges(&mut model, solid_id, vec![edge], chamfer_opts())
        .expect("first chamfer on a fresh single edge succeeds");

    let err = chamfer_edges(&mut model, solid_id, vec![edge], chamfer_opts())
        .expect_err("second chamfer on the same edge must fail");

    assert_conflict(err, edge, BlendKind::Chamfer, BlendKind::Chamfer);
}

#[test]
fn chamfer_disjoint_edges_does_not_falsely_trigger_conflict() {
    // Two edges on opposite corners of the box share no vertex, so
    // a second blend on the second edge after the first edge's
    // surgery must NOT trip the CF-α gate. This pins the
    // false-positive boundary: the gate fires on the registry hits
    // only, not on every subsequent blend.
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);

    let corner_a = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);
    let edge_a = pick_one_edge_at_vertex(&model, corner_a);

    let corner_b = vertex_at(&model, -HALF_BOX, -HALF_BOX, -HALF_BOX);
    let edge_b = pick_one_edge_at_vertex(&model, corner_b);
    assert_ne!(
        edge_a, edge_b,
        "edges at opposite box corners must be distinct"
    );

    chamfer_edges(&mut model, solid_id, vec![edge_a], chamfer_opts())
        .expect("first chamfer on corner A succeeds");
    chamfer_edges(&mut model, solid_id, vec![edge_b], chamfer_opts())
        .expect("second chamfer on a disjoint corner B succeeds — no false-positive CF-α trip");
}
