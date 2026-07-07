//! D-1 dogfood fixtures — the sequential-adjacent blend-scar honesty chain.
//!
//! Authority: `.superpowers/sdd/dogfood-diag-api-blend.md` (BUG 1). Live
//! dogfooding followed the kernel's OWN Task-#82 refusal guidance ("apply
//! each edge in a separate fillet/chamfer call") and silently corrupted a
//! 30-box: the second single-edge fillet at a corner already carrying a
//! fillet scar re-trims the host faces assuming pristine planar boundaries,
//! producing a combinatorially-valid but geometrically-open solid (mesh
//! χ = 0, hundreds of boundary chords) that `validate_result: true`
//! accepted and the certificate shipped with `sound=false, errors: []`.
//!
//! These fixtures pin the four-part fix:
//!
//! 1. The Task-#82 shared-corner refusal names the SUPPORTED path
//!    (`partial_corner_vertices` + the corner vertex id) and warns that
//!    sequential separate single-edge calls at a shared corner are
//!    unsupported.
//! 2. A single-edge fillet/chamfer whose edge endpoint carries an
//!    existing SAME-kind blend scar is REFUSED pre-flight (typed
//!    `BlendFailure::AdjacentSameKindBlendScar`) instead of corrupting.
//! 3. The blend post-flight includes a geometric-closure (coarse-chord
//!    mesh watertightness) check, so a combinatorially-valid-but-open
//!    result can never be accepted again.
//! 4. An unsound certificate never ships an empty `errors` list — the
//!    failing dimensions are named.
//!
//! The supported two-call mixed protocol (all same-kind corner edges in
//! ONE call with the opt-in, then the opposite kind) must keep working —
//! pinned end-to-end here at the kernel layer (the HTTP twin lives in
//! `api-server/src/router_integration_tests.rs`).

use geometry_engine::operations::chamfer::ChamferType;
use geometry_engine::operations::fillet::{FilletType, PropagationMode};
use geometry_engine::operations::{
    chamfer_edges, fillet_edges, ChamferOptions, FilletOptions, OperationError,
};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::vertex::VertexId;

// `create_box_3d(w, h, d)` builds a box centred at the origin, so the
// 30-box (the dogfood repro dimension) has its top face at z = +15.
const BOX: f64 = 30.0;
const Z_TOP: f64 = 15.0;
const EPS: f64 = 1e-9;
/// The dogfood blend displacement (fillet radius / chamfer distance).
const R: f64 = 4.0;

fn make_box(model: &mut BRepModel) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    match builder
        .create_box_3d(BOX, BOX, BOX)
        .expect("box creation succeeds")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {:?}", other),
    }
}

/// Top edges of the canonical box: both endpoints at z == Z_TOP.
/// Returns `(edge_id, midpoint_x, midpoint_y)`.
fn top_edges(model: &BRepModel) -> Vec<(EdgeId, f64, f64)> {
    let mut found = Vec::new();
    for (eid, edge) in model.edges.iter() {
        if edge.is_loop() {
            continue;
        }
        let Some(v0) = model.vertices.get(edge.start_vertex) else {
            continue;
        };
        let Some(v1) = model.vertices.get(edge.end_vertex) else {
            continue;
        };
        if (v0.position[2] - Z_TOP).abs() < EPS && (v1.position[2] - Z_TOP).abs() < EPS {
            found.push((
                eid,
                0.5 * (v0.position[0] + v1.position[0]),
                0.5 * (v0.position[1] + v1.position[1]),
            ));
        }
    }
    found
}

/// Two adjacent top edges (sharing exactly one vertex) plus the shared
/// corner vertex — the dogfood corner.
fn adjacent_top_pair(model: &BRepModel) -> (EdgeId, EdgeId, VertexId) {
    let edges = top_edges(model);
    assert_eq!(edges.len(), 4, "box must have exactly 4 top edges");
    let (first, _, _) = edges[0];
    let first_edge = model.edges.get(first).expect("first edge stored");
    let v_first = [first_edge.start_vertex, first_edge.end_vertex];
    for &(other, _, _) in &edges[1..] {
        let e = model.edges.get(other).expect("edge stored");
        for shared in [e.start_vertex, e.end_vertex] {
            if v_first.contains(&shared) {
                return (first, other, shared);
            }
        }
    }
    panic!("box top edges must contain an adjacent pair");
}

/// Re-locate a surviving top edge near world-space (x, y) after a blend
/// shifted edge ids (surgery destroys the requested edge and re-trims
/// its neighbours; midpoints move by at most the setback).
fn find_top_edge_near(model: &BRepModel, x: f64, y: f64, tol: f64) -> Option<EdgeId> {
    top_edges(model)
        .into_iter()
        .find(|&(_, mx, my)| (mx - x).hypot(my - y) < tol)
        .map(|(eid, _, _)| eid)
}

fn fillet_opts(radius: f64, partial: Vec<VertexId>) -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(radius),
        radius,
        propagation: PropagationMode::None,
        partial_corner_vertices: partial,
        ..Default::default()
    }
}

fn chamfer_opts(distance: f64) -> ChamferOptions {
    ChamferOptions {
        chamfer_type: ChamferType::EqualDistance(distance),
        distance1: distance,
        distance2: distance,
        ..Default::default()
    }
}

/// One-line cert summary for failure messages.
fn cert_summary(model: &mut BRepModel, solid: SolidId) -> String {
    let c = model.certify_solid(solid);
    format!(
        "sound={} watertight={} euler={} boundary={} nonmanifold={} selfint_free={} brep_valid={} errors={}",
        c.is_sound(),
        c.watertight,
        c.euler_characteristic,
        c.boundary_edges,
        c.nonmanifold_edges,
        c.self_intersection_free,
        c.brep_valid,
        c.errors.len(),
    )
}

// =====================================================================
// Item 2 — the dogfood corrupting sequence must refuse typed at call 2.
// =====================================================================

/// The exact dogfood sequence (BUG-1 evidence row C, steps 1-2): fillet
/// one top edge, then fillet the ADJACENT top edge in a separate call
/// with no opt-in. Pre-fix the second call is ACCEPTED and silently
/// corrupts (cert watertight=false, euler=0, hundreds of boundary
/// chords, `errors: []`). Post-fix it must refuse with the typed
/// `AdjacentSameKindBlendScar` payload and roll back, leaving the
/// post-first-fillet solid intact and certifiably closed.
#[test]
fn sequential_adjacent_same_kind_fillet_refused_typed_and_rolled_back() {
    let mut model = BRepModel::new();
    let solid = make_box(&mut model);
    let (first, second_orig, _corner) = adjacent_top_pair(&model);
    let (_, sx, sy) = top_edges(&model)
        .into_iter()
        .find(|&(eid, _, _)| eid == second_orig)
        .expect("second edge is a top edge");

    fillet_edges(&mut model, solid, vec![first], fillet_opts(R, Vec::new()))
        .expect("first single-edge fillet must succeed");
    let after_first = cert_summary(&mut model, solid);

    // The adjacent edge survives (shortened); re-locate it by midpoint.
    let second = find_top_edge_near(&model, sx, sy, R)
        .expect("adjacent top edge must survive the first fillet");

    let result = fillet_edges(&mut model, solid, vec![second], fillet_opts(R, Vec::new()));

    let err = match result {
        Err(e) => e,
        Ok(faces) => {
            let after_second = cert_summary(&mut model, solid);
            panic!(
                "RED: second adjacent same-kind fillet was ACCEPTED ({} faces) — \
                 silent corruption. cert after call 1: [{after_first}]; \
                 cert after call 2: [{after_second}]",
                faces.len()
            );
        }
    };

    // Typed refusal: BlendFailed carrying AdjacentSameKindBlendScar,
    // whose guidance (the Display surface agents read) names the
    // supported opt-in path.
    let type_ok = matches!(err, OperationError::BlendFailed(_));
    let dbg = format!("{err:?}");
    assert!(
        type_ok && dbg.contains("AdjacentSameKindBlendScar"),
        "refusal must be the typed BlendFailed(AdjacentSameKindBlendScar); got {err:?}"
    );
    assert!(
        err.to_string().contains("partial_corner_vertices"),
        "refusal guidance must name the partial_corner_vertices opt-in; got {err}"
    );

    // Rollback: the model is byte-restored to the post-first-fillet
    // state, which certifies closed and self-intersection-free.
    let cert = model.certify_solid(solid);
    assert!(
        cert.watertight && cert.self_intersection_free,
        "post-refusal model must be the intact post-first-fillet solid; cert: [{}]",
        cert_summary(&mut model, solid)
    );
}

/// Chamfer mirror of the corrupting sequence — the same-kind scar gate
/// covers both blend kinds.
#[test]
fn sequential_adjacent_same_kind_chamfer_refused_typed_and_rolled_back() {
    let mut model = BRepModel::new();
    let solid = make_box(&mut model);
    let (first, second_orig, _corner) = adjacent_top_pair(&model);
    let (_, sx, sy) = top_edges(&model)
        .into_iter()
        .find(|&(eid, _, _)| eid == second_orig)
        .expect("second edge is a top edge");

    chamfer_edges(&mut model, solid, vec![first], chamfer_opts(R))
        .expect("first single-edge chamfer must succeed");
    let after_first = cert_summary(&mut model, solid);

    let second = find_top_edge_near(&model, sx, sy, R)
        .expect("adjacent top edge must survive the first chamfer");

    let result = chamfer_edges(&mut model, solid, vec![second], chamfer_opts(R));

    let err = match result {
        Err(e) => e,
        Ok(faces) => {
            let after_second = cert_summary(&mut model, solid);
            panic!(
                "RED: second adjacent same-kind chamfer was ACCEPTED ({} faces) — \
                 silent corruption. cert after call 1: [{after_first}]; \
                 cert after call 2: [{after_second}]",
                faces.len()
            );
        }
    };

    let type_ok = matches!(err, OperationError::BlendFailed(_));
    let dbg = format!("{err:?}");
    assert!(
        type_ok && dbg.contains("AdjacentSameKindBlendScar"),
        "refusal must be the typed BlendFailed(AdjacentSameKindBlendScar); got {err:?}"
    );

    let cert = model.certify_solid(solid);
    assert!(
        cert.watertight && cert.self_intersection_free,
        "post-refusal model must be the intact post-first-chamfer solid; cert: [{}]",
        cert_summary(&mut model, solid)
    );
}

// =====================================================================
// Items 1 + 4 + protocol preservation — the SUPPORTED two-call path.
// =====================================================================

/// Item 1: the F2-γ.1 shared-corner refusal (Task #82) must name the
/// supported route — `partial_corner_vertices` plus the concrete corner
/// vertex id — and must NOT steer callers onto the corrupting
/// "separate calls" protocol.
#[test]
fn shared_corner_refusal_names_partial_corner_vertices_opt_in() {
    let mut model = BRepModel::new();
    let solid = make_box(&mut model);
    let (e1, e2, corner) = adjacent_top_pair(&model);

    let err = fillet_edges(&mut model, solid, vec![e1, e2], fillet_opts(R, Vec::new()))
        .expect_err("two same-kind edges sharing a corner without opt-in must refuse");
    let msg = format!("{err:?}");

    assert!(
        msg.contains("partial_corner_vertices"),
        "refusal must name the partial_corner_vertices opt-in; got: {msg}"
    );
    assert!(
        msg.contains(&format!("{corner}")),
        "refusal must name the corner vertex id {corner}; got: {msg}"
    );
    assert!(
        !msg.contains("separate fillet/chamfer call"),
        "refusal must no longer advise the corrupting separate-call protocol; got: {msg}"
    );
    assert!(
        msg.to_lowercase().contains("unsupported")
            || msg.to_lowercase().contains("do not apply")
            || msg.to_lowercase().contains("corrupt"),
        "refusal must warn that sequential separate single-edge calls at a shared \
         corner are unsupported; got: {msg}"
    );
}

/// The fixture protocol (diagnosis evidence row B) end-to-end at the
/// kernel layer:
///
/// * call 1 — fillet BOTH same-kind corner edges in one call with the
///   `partial_corner_vertices` opt-in. The intermediate state is
///   deliberately open at the corner; the certificate must report that
///   HONESTLY (`watertight=false`) and — item 4 — must NAME the failing
///   watertight dimension in `errors` instead of shipping an empty list.
/// * call 2 — chamfer the third corner edge. The finalize machinery
///   synthesizes the mixed cap; the result must certify geometrically
///   CLOSED (watertight, χ = 2, self-intersection-free).
///
/// This also pins the scar gate's scoping: the legitimate second call
/// must NOT be refused (cross-kind at a pending corner), and the
/// geometric-closure post-flight must exempt the pending intermediate
/// while still gating the final state.
#[test]
fn mixed_protocol_two_call_supported_path_still_passes() {
    let mut model = BRepModel::new();
    let solid = make_box(&mut model);
    let (e1, e2, corner) = adjacent_top_pair(&model);

    // Call 1: both same-kind corner edges + opt-in, ONE call.
    fillet_edges(
        &mut model,
        solid,
        vec![e1, e2],
        fillet_opts(R, vec![corner]),
    )
    .expect("opt-in two-edge fillet (protocol call 1) must succeed");

    let mid_cert = model.certify_solid(solid);
    assert!(
        !mid_cert.watertight,
        "protocol intermediate must be honestly open at the corner; cert: [{}]",
        cert_summary(&mut model, solid)
    );
    // Item 4 — an unsound cert must name its failing dimension(s).
    assert!(
        !mid_cert.errors.is_empty(),
        "unsound intermediate cert must not ship empty errors (item 4)"
    );
    assert!(
        mid_cert.errors.iter().any(|e| e.contains("watertight")),
        "unsound intermediate cert errors must name the failing watertight \
         dimension; got: {:?}",
        mid_cert.errors
    );

    // Call 2: the third corner edge, opposite kind — the finalize.
    // The corner vertex survived call 1 (opt-in preserved it); the
    // remaining incident edge is the vertical corner edge.
    let third: EdgeId = {
        let mut found: Vec<EdgeId> = Vec::new();
        for (eid, edge) in model.edges.iter() {
            if edge.start_vertex == corner || edge.end_vertex == corner {
                found.push(eid);
            }
        }
        assert_eq!(
            found.len(),
            1,
            "after the opt-in first call exactly the vertical corner edge must \
             remain incident to V={corner}; got {found:?}"
        );
        found[0]
    };

    chamfer_edges(&mut model, solid, vec![third], chamfer_opts(R))
        .expect("opposite-kind finalize (protocol call 2) must succeed");

    let final_cert = model.certify_solid(solid);
    assert!(
        final_cert.watertight
            && final_cert.euler_characteristic == 2
            && final_cert.self_intersection_free,
        "protocol final state must certify geometrically closed; cert: [{}]",
        cert_summary(&mut model, solid)
    );
}
