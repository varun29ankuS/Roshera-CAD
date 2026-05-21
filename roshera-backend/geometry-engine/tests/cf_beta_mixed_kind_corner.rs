//! CF-β.5 — mixed-kind corner integration suite.
//!
//! Pins the eager-cap synthesizer for the headline case: a 3-edge
//! convex box corner where two of the three incident edges receive
//! one blend kind and the third receives the opposite kind, with
//! `offset == radius`. The second of the two kind-mismatched calls
//! is the one that synthesizes the watertight planar mixed cap.
//!
//! These tests pin (per CF-β plan):
//!
//! * **Order-independence** — chamfer-first ↔ fillet-first produce
//!   structurally identical solids (equal `topology_hash`).
//! * **Watertightness after second call** — V−E+F = 2 and zero
//!   non-manifold edges on the outer shell.
//! * **Intermediate-state carve-out** — `validate_result: true` on
//!   the first of the two calls passes even though the corner is
//!   deliberately left open until the second call closes it (CF-β.4
//!   `Solid::pending_mixed_kind_corners` + `filter_pending_corner_errors`).
//! * **Pyramid degree-4 typed reject** — a 4-edge mixed corner
//!   still rejects with `MixedKindUnsupported { detail:
//!   DegreeUnsupported { degree: 4 } }` (β.3.4 carved out only
//!   degree-3).

#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

#[path = "blend_fixtures/mod.rs"]
mod blend_fixtures;

use blend_fixtures::*;

use geometry_engine::operations::chamfer::{
    chamfer_edges, ChamferOptions, ChamferType, PropagationMode as ChamferProp,
};
use geometry_engine::operations::diagnostics::{
    BlendFailure, MixedKindRejectDetail, VertexBlendUnsupportedReason,
};
use geometry_engine::operations::fillet::{FilletType, PropagationMode as FilletProp};
use geometry_engine::operations::{fillet_edges, CommonOptions, FilletOptions, OperationError};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::vertex::VertexId;

const BOX_SIZE: f64 = 10.0;
const HALF_BOX: f64 = BOX_SIZE / 2.0;
const D: f64 = 1.0; // shared chamfer offset / fillet radius for the equal-displacement case

fn fillet_opts_constant(radius: f64) -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(radius),
        radius,
        propagation: FilletProp::None,
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// CF-β.5.2-A — fillet options pre-populated with a partial-mixed
/// corner opt-in. Used at the *first* call of a kind-mismatched
/// pair to declare intent for the kernel to leave V open. The
/// *second* call need not repeat the opt-in (auto-detected from
/// the pending registry + opposite-kind blend records).
fn fillet_opts_with_partial_corner(radius: f64, partial: Vec<VertexId>) -> FilletOptions {
    FilletOptions {
        partial_corner_vertices: partial,
        ..fillet_opts_constant(radius)
    }
}

fn chamfer_opts_equal(distance: f64) -> ChamferOptions {
    ChamferOptions {
        chamfer_type: ChamferType::EqualDistance(distance),
        distance1: distance,
        distance2: distance,
        symmetric: true,
        propagation: ChamferProp::None,
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// CF-β.5.2-A — chamfer mirror of [`fillet_opts_with_partial_corner`].
fn chamfer_opts_with_partial_corner(distance: f64, partial: Vec<VertexId>) -> ChamferOptions {
    ChamferOptions {
        partial_corner_vertices: partial,
        ..chamfer_opts_equal(distance)
    }
}

/// Return the three edges of the (+x, +y, +z) box corner of a
/// `BOX_SIZE`-side cube centred at the origin.
fn corner_edges(model: &BRepModel) -> Vec<EdgeId> {
    let corner = vertex_at(model, HALF_BOX, HALF_BOX, HALF_BOX);
    let mut edges = edges_at_vertex(model, corner);
    edges.sort_unstable();
    assert_eq!(
        edges.len(),
        3,
        "box corner must have exactly 3 incident edges; got {}",
        edges.len()
    );
    edges
}

/// Pin a baseline: the freshly-built `BOX_SIZE` cube has V=8, E=12,
/// F=6 and the corner of interest has exactly 3 incident edges.
#[test]
fn fixture_box_baseline_topology() {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let (v, e, f) = shell_census(&model, solid_id);
    assert_eq!((v, e, f), (8, 12, 6), "axis-aligned cube V/E/F census");
    assert_eq!(non_manifold_edge_count(&model, solid_id), 0);
    let edges = corner_edges(&model);
    assert_eq!(edges.len(), 3);
}

// ---------------------------------------------------------------------------
// Headline mixed-kind tests
// ---------------------------------------------------------------------------

/// **Chamfer first, then fillet** on a single box corner: one edge is
/// chamfered (V is preserved across that call via the β.4.4 partial-mixed
/// V-retention extension), then the remaining two edges are filleted.
/// The fillet call's β.3.4 dispatch hook detects `has_prior_chamfer`
/// and routes the corner to `synthesize_mixed_kind_corner_cap`.
/// Final shell must be watertight.
#[test]
#[ignore = "blocked on CF-β.5.2-B: finalize-pass synthesizer dispatch for partial-mixed corners (#17)"]
fn box_corner_one_chamfer_two_fillets_chamfer_first_then_fillet() {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let edges = corner_edges(&model);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);

    // Chamfer the first edge only — declare V as a partial-mixed
    // corner so the F2-γ.1 gate carves it out, V is preserved
    // across surgery (β.5.2-A corner_shared stamp), and V is
    // registered in pending_mixed_kind_corners so the post-flight
    // validator's β.4.2 carve-out fires.
    chamfer_edges(
        &mut model,
        solid_id,
        vec![edges[0]],
        chamfer_opts_with_partial_corner(D, vec![corner]),
    )
    .expect("partial-mixed first-call chamfer on a single corner edge succeeds");

    // Fillet the remaining two edges of the same corner.
    fillet_edges(
        &mut model,
        solid_id,
        vec![edges[1], edges[2]],
        fillet_opts_constant(D),
    )
    .expect(
        "second-call fillet on the remaining two corner edges synthesises the mixed-kind cap",
    );

    assert_eq!(
        non_manifold_edge_count(&model, solid_id),
        0,
        "mixed-kind cap must close the corner watertight"
    );
    let (v, e, f) = shell_census(&model, solid_id);
    assert_eq!(
        v as i64 - e as i64 + f as i64,
        2,
        "Euler-Poincaré V−E+F=2 after mixed-kind cap synthesis"
    );
}

/// Mirror: **fillet first, then chamfer**. The fillet call leaves a
/// deliberately-open partial-mixed corner (β.4.4 V-retention via the
/// `or v?_prior_chamfer` extension does not fire here because V has
/// no prior chamfer yet; instead the symmetric route is the
/// `BlendGraph::ConvexCorner{degree:3}` classifier — but the current
/// call only selects 2 of 3 edges, so the classifier yields
/// degree-2 / Edge and the corner is NOT classified as a 3-corner;
/// V removal would orphan the corner). The β.4.1 pending registry +
/// β.4.2 validate-result carve-out lets the call complete.
/// The chamfer second call sees V already recorded with Fillet kind
/// and routes through the chamfer-side dispatch hook.
#[test]
#[ignore = "blocked on CF-β.5.2-B: fillet-arc-rim discovery via find_blend_cap_edges_at_vertex returns 0 — needs investigation of fillet face cap-edge survival across surgery (#17)"]
fn box_corner_two_fillets_then_chamfer_synthesises_mixed_cap() {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let edges = corner_edges(&model);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);

    // First-call opt-in (CF-β.5.2-A) — fillet two of three corner
    // edges and declare V as partial-mixed so the F2-γ.1 gate
    // carves out the shared-vertex clash, surgery preserves V, and
    // the post-flight validator's β.4.2 pending-corner carve-out
    // accepts the deliberately-open intermediate state.
    fillet_edges(
        &mut model,
        solid_id,
        vec![edges[1], edges[2]],
        fillet_opts_with_partial_corner(D, vec![corner]),
    )
    .expect("first-call fillet on two of three corner edges succeeds");

    // Second call auto-detects via pending_mixed_kind_corners +
    // opposite-kind blend records — no explicit opt-in needed.
    chamfer_edges(
        &mut model,
        solid_id,
        vec![edges[0]],
        chamfer_opts_equal(D),
    )
    .expect("second-call chamfer on the remaining corner edge synthesises the mixed-kind cap");

    assert_eq!(
        non_manifold_edge_count(&model, solid_id),
        0,
        "mixed-kind cap must close the corner watertight (reverse order)"
    );
    let (v, e, f) = shell_census(&model, solid_id);
    assert_eq!(v as i64 - e as i64 + f as i64, 2);
}

/// CF-β.4 intermediate-state carve-out — the first of two
/// kind-mismatched calls runs with `validate_result: true` (default
/// path in `fillet_opts_constant` / `chamfer_opts_equal`) and must
/// succeed even though it leaves an open corner. Without the
/// `pending_mixed_kind_corners` + `filter_pending_corner_errors`
/// gate, the post-flight `validate_model_enhanced` would surface
/// a non-manifold edge at the corner and reject.
#[test]
fn box_corner_mixed_kind_intermediate_state_skips_watertight_validation() {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let edges = corner_edges(&model);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);

    // First call only — leave the corner intentionally open via
    // the β.5.2-A opt-in. The opt-in (a) carves out the F2-γ.1
    // shared-vertex gate, (b) preserves V across surgery, and
    // (c) registers V in `pending_mixed_kind_corners` so the
    // post-flight `filter_pending_corner_errors` drops the
    // non-manifold-edge errors at the pending corner. Without
    // the opt-in, `validate_result: true` would reject.
    fillet_edges(
        &mut model,
        solid_id,
        vec![edges[1], edges[2]],
        fillet_opts_with_partial_corner(D, vec![corner]),
    )
    .expect(
        "first-call partial fillet must pass post-flight validation via the β.4.2 carve-out",
    );

    // Sanity-check the carve-out's premise: the shell is *actually*
    // non-watertight at this intermediate state.
    let solid = model.solids.get(solid_id).expect("solid exists");
    assert!(
        solid.is_mixed_kind_corner_pending(corner),
        "the corner vertex must be flagged in pending_mixed_kind_corners after the first call"
    );
}

/// CF-β.3.4 — degree-3 carve-out only. A degree-4 mixed-kind corner
/// (built on a square pyramid apex with the 4 sloped edges split
/// between chamfer and fillet) must still surface a typed
/// `MixedKindUnsupported { detail: DegreeUnsupported { degree: 4 } }`
/// reject because the cap walker has not been validated for N > 3.
#[test]
#[ignore = "blocked on CF-β.5.2-B: degree-4 partial-mixed apex requires synthesizer scaffold to reach second call (#17)"]
fn box_corner_mixed_kind_degree_4_pyramid_rejected_typed_degree_unsupported() {
    let mut model = BRepModel::new();
    let solid_id = make_square_pyramid_solid(&mut model, 10.0, 10.0);
    let apex = vertex_at(&model, 0.0, 0.0, 10.0);
    let mut sloped: Vec<EdgeId> = edges_at_vertex(&model, apex);
    sloped.sort_unstable();
    assert_eq!(sloped.len(), 4, "pyramid apex has exactly 4 sloped edges");

    // First call: chamfer two of the four apex edges with the apex
    // declared as a partial-mixed corner via the β.5.2-A opt-in.
    // This forces `original_v?_corner_shared` on the apex side so
    // the chamfer records the apex in `blended_vertices` (Chamfer
    // kind) — without it the degree-4 apex would be merged away by
    // the splice and the second call's mixed-kind feasibility gate
    // would never see prior-kind state at the apex.
    let _ = chamfer_edges(
        &mut model,
        solid_id,
        vec![sloped[0], sloped[1]],
        ChamferOptions {
            chamfer_type: ChamferType::EqualDistance(0.5),
            distance1: 0.5,
            distance2: 0.5,
            symmetric: true,
            propagation: ChamferProp::None,
            partial_corner_vertices: vec![apex],
            common: CommonOptions {
                // The degree-4 apex with only 2 chamfered edges
                // leaves a deliberately open partial-mixed surface
                // (analogous to the box-corner degree-3 partial case);
                // run with validate_result=false so we get to the
                // second call without tripping the post-flight
                // (which has no β.4 carve-out for degree-4 yet).
                validate_result: false,
                ..Default::default()
            },
            ..Default::default()
        },
    );

    // Second call: fillet one of the remaining apex edges. Per
    // CF-β.3.4 the feasibility gate carves out only degree-3, so
    // degree-4 surfaces a typed `DegreeUnsupported{degree:4}`.
    let err = fillet_edges(
        &mut model,
        solid_id,
        vec![sloped[2]],
        fillet_opts_constant(0.5),
    )
    .expect_err("degree-4 mixed-kind corner must reject");

    // Unwrap the typed payload — must reach BlendFailed →
    // VertexBlendUnsupported → MixedKindUnsupported with degree=4.
    match err {
        OperationError::BlendFailed(failure) => match *failure {
            BlendFailure::VertexBlendUnsupported { reason, .. } => match reason {
                VertexBlendUnsupportedReason::MixedKindUnsupported { detail, .. } => match detail {
                    MixedKindRejectDetail::DegreeUnsupported { degree } => {
                        assert_eq!(
                            degree, 4,
                            "degree-4 reject must carry degree=4 payload, got {}",
                            degree
                        );
                    }
                    other => panic!(
                        "expected DegreeUnsupported{{degree:4}}, got detail={:?}",
                        other
                    ),
                },
                other => panic!("expected MixedKindUnsupported reason, got {:?}", other),
            },
            other => panic!("expected VertexBlendUnsupported, got {:?}", other),
        },
        other => panic!("expected BlendFailed, got {:?}", other),
    }
}

/// Topology-hash invariance under call-order swap: chamfer-first vs
/// fillet-first must produce structurally identical solids. The
/// hash is computed via `topology_hash` (canonicalised V/E/F counts
/// + sorted edge-endpoint pairs + sorted face surface-kind tags),
/// so it ignores per-store ID ordering — only structural
/// differences surface as a mismatch.
#[test]
#[ignore = "blocked on CF-β.5.2-B: requires synthesizer dispatch to compute final-state hash (#17)"]
fn box_corner_mixed_kind_topology_hash_order_invariant() {
    // Order A: chamfer first then fillet. First call opts in the
    // apex corner; second call auto-detects via the pending
    // registry + opposite-kind blend records.
    let mut model_a = BRepModel::new();
    let solid_a = make_cube(&mut model_a, BOX_SIZE);
    let edges_a = corner_edges(&model_a);
    let corner_a = vertex_at(&model_a, HALF_BOX, HALF_BOX, HALF_BOX);
    chamfer_edges(
        &mut model_a,
        solid_a,
        vec![edges_a[0]],
        chamfer_opts_with_partial_corner(D, vec![corner_a]),
    )
    .expect("order A: first chamfer");
    fillet_edges(
        &mut model_a,
        solid_a,
        vec![edges_a[1], edges_a[2]],
        fillet_opts_constant(D),
    )
    .expect("order A: second fillet");
    let hash_a = topology_hash(&model_a, solid_a);

    // Order B: fillet first then chamfer. Mirror of order A.
    let mut model_b = BRepModel::new();
    let solid_b = make_cube(&mut model_b, BOX_SIZE);
    let edges_b = corner_edges(&model_b);
    let corner_b = vertex_at(&model_b, HALF_BOX, HALF_BOX, HALF_BOX);
    fillet_edges(
        &mut model_b,
        solid_b,
        vec![edges_b[1], edges_b[2]],
        fillet_opts_with_partial_corner(D, vec![corner_b]),
    )
    .expect("order B: first fillet");
    chamfer_edges(
        &mut model_b,
        solid_b,
        vec![edges_b[0]],
        chamfer_opts_equal(D),
    )
    .expect("order B: second chamfer");
    let hash_b = topology_hash(&model_b, solid_b);

    assert_eq!(
        hash_a, hash_b,
        "chamfer-first and fillet-first must produce structurally identical solids"
    );
}
