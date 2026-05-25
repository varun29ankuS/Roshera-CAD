//! CF-γ.3 — G1 mixed-kind corner integration suite (post-backout).
//!
//! CF-γ.2 landed a single-degenerate-bicubic NURBS cap synthesizer
//! (`operations::mixed_kind_corner_cap_g1::synthesize_mixed_kind_corner_cap_g1`).
//! Field testing showed that a 4×4 control net with one collapsed
//! apex column is rank-limited against the 3 rims × K=5 stations × 1
//! normal-direction = 15 G1 constraints it must satisfy at a mixed
//! chamfer × fillet box corner: it clears `G1_TOLERANCE = 1e-4` rad
//! only for some 1C2F orderings and fails 2C1F by 3–4 orders of
//! magnitude (asymmetric across construction order).
//!
//! Per the CF-γ plan §"Backout plan", the dispatcher arms in
//! `chamfer.rs::handle_chamfer_vertices` and the two
//! `fillet.rs::create_fillet_transitions` mirrors are reverted to
//! the CF-γ.1 sentinel: every `SeamContinuity::G1` cap request
//! surfaces the typed
//! [`BlendFailure::SeamContinuityUnreachable`] reject. The
//! synthesizer module stays in tree for a follow-up reformulation
//! (Gregory patch or 3-sub-patch split — both explicitly rejected
//! by the original plan, so resurrection is a planning decision,
//! not an implementation detail).
//!
//! This file pins the post-backout contract end-to-end:
//!
//! * Each of the 4 mixed-kind topologies the kernel supports today
//!   on a box corner (1C2F × {chamfer-first, fillet-first} and
//!   2C1F × {chamfer-first, fillet-first}) lands the first call
//!   under the partial-mixed opt-in and surfaces
//!   `BlendFailure::SeamContinuityUnreachable` on the second call
//!   (the one that would synthesize the cap).
//! * The C0 default path is unchanged — same fixture with
//!   `SeamContinuity::C0` (the [`Default`] impl) still produces a
//!   `Plane`-backed N-gon cap. Catches an accidental flip of the
//!   default that would silently change CF-β semantics.

#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

#[path = "blend_fixtures/mod.rs"]
mod blend_fixtures;

use blend_fixtures::*;

use geometry_engine::operations::chamfer::{
    chamfer_edges, ChamferOptions, ChamferType, PropagationMode as ChamferProp,
};
use geometry_engine::operations::diagnostics::BlendFailure;
use geometry_engine::operations::fillet::{FilletType, PropagationMode as FilletProp};
use geometry_engine::operations::mixed_kind_corner_cap::SeamContinuity;
use geometry_engine::operations::{fillet_edges, CommonOptions, FilletOptions, OperationError};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::surface::{GeneralNurbsSurface, Plane};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::vertex::VertexId;

const BOX_SIZE: f64 = 10.0;
const HALF_BOX: f64 = BOX_SIZE / 2.0;
const D: f64 = 1.0;

// ---------------------------------------------------------------------------
// Option helpers
// ---------------------------------------------------------------------------

fn fillet_g1_opts(radius: f64) -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(radius),
        radius,
        propagation: FilletProp::None,
        seam_continuity: SeamContinuity::G1,
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn fillet_g1_opts_with_partial(radius: f64, partial: Vec<VertexId>) -> FilletOptions {
    FilletOptions {
        partial_corner_vertices: partial,
        ..fillet_g1_opts(radius)
    }
}

fn chamfer_g1_opts(distance: f64) -> ChamferOptions {
    ChamferOptions {
        chamfer_type: ChamferType::EqualDistance(distance),
        distance1: distance,
        distance2: distance,
        symmetric: true,
        propagation: ChamferProp::None,
        seam_continuity: SeamContinuity::G1,
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn chamfer_g1_opts_with_partial(distance: f64, partial: Vec<VertexId>) -> ChamferOptions {
    ChamferOptions {
        partial_corner_vertices: partial,
        ..chamfer_g1_opts(distance)
    }
}

/// C0 default opts — used by `c0_default_still_produces_planar_cap`
/// to pin the CF-β regression boundary.
fn chamfer_c0_opts_with_partial(distance: f64, partial: Vec<VertexId>) -> ChamferOptions {
    ChamferOptions {
        seam_continuity: SeamContinuity::C0,
        ..chamfer_g1_opts_with_partial(distance, partial)
    }
}

fn fillet_c0_opts(radius: f64) -> FilletOptions {
    FilletOptions {
        seam_continuity: SeamContinuity::C0,
        ..fillet_g1_opts(radius)
    }
}

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

/// Assert that `err` is the typed
/// [`BlendFailure::SeamContinuityUnreachable`] payload — the only
/// shape the G1 dispatcher arms produce after the CF-γ backout.
fn assert_seam_continuity_unreachable(err: OperationError, label: &str) {
    match err {
        OperationError::BlendFailed(failure) => match *failure {
            BlendFailure::SeamContinuityUnreachable { .. } => {}
            other => panic!(
                "{label}: expected BlendFailure::SeamContinuityUnreachable, got {:?}",
                other
            ),
        },
        other => panic!(
            "{label}: expected OperationError::BlendFailed, got {:?}",
            other
        ),
    }
}

// ---------------------------------------------------------------------------
// Headline tests — every G1 cap request routes to the typed reject
// ---------------------------------------------------------------------------
//
// In each test the first call (chamfer or fillet) opens the corner
// under the `partial_corner_vertices` opt-in — no cap is synthesized
// yet because the opt-in pins the corner vertex open — so the first
// call succeeds regardless of `seam_continuity`. The second call
// finalizes the mixed-kind corner and is the one routed through the
// G1 dispatcher arm; with the backout in place it surfaces the
// typed reject.

#[test]
fn g1_1c2f_chamfer_first_routes_to_seam_continuity_unreachable() {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let edges = corner_edges(&model);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);
    chamfer_edges(
        &mut model,
        solid_id,
        vec![edges[0]],
        chamfer_g1_opts_with_partial(D, vec![corner]),
    )
    .expect("1C2F chamfer-first: first chamfer with partial-corner opt-in succeeds");
    let err = fillet_edges(
        &mut model,
        solid_id,
        vec![edges[1], edges[2]],
        fillet_g1_opts(D),
    )
    .expect_err(
        "1C2F chamfer-first: second fillet must route the G1 cap request to the typed reject",
    );
    assert_seam_continuity_unreachable(err, "1C2F chamfer-first");
}

#[test]
fn g1_1c2f_fillet_first_routes_to_seam_continuity_unreachable() {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let edges = corner_edges(&model);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);
    fillet_edges(
        &mut model,
        solid_id,
        vec![edges[1], edges[2]],
        fillet_g1_opts_with_partial(D, vec![corner]),
    )
    .expect("1C2F fillet-first: first fillet with partial-corner opt-in succeeds");
    let err = chamfer_edges(
        &mut model,
        solid_id,
        vec![edges[0]],
        chamfer_g1_opts(D),
    )
    .expect_err(
        "1C2F fillet-first: second chamfer must route the G1 cap request to the typed reject",
    );
    assert_seam_continuity_unreachable(err, "1C2F fillet-first");
}

#[test]
fn g1_2c1f_chamfer_first_routes_to_seam_continuity_unreachable() {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let edges = corner_edges(&model);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);
    chamfer_edges(
        &mut model,
        solid_id,
        vec![edges[0], edges[1]],
        chamfer_g1_opts_with_partial(D, vec![corner]),
    )
    .expect("2C1F chamfer-first: first chamfer (two edges) with partial-corner opt-in succeeds");
    let err = fillet_edges(
        &mut model,
        solid_id,
        vec![edges[2]],
        fillet_g1_opts(D),
    )
    .expect_err(
        "2C1F chamfer-first: second fillet must route the G1 cap request to the typed reject",
    );
    assert_seam_continuity_unreachable(err, "2C1F chamfer-first");
}

#[test]
fn g1_2c1f_fillet_first_routes_to_seam_continuity_unreachable() {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let edges = corner_edges(&model);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);
    fillet_edges(
        &mut model,
        solid_id,
        vec![edges[2]],
        fillet_g1_opts_with_partial(D, vec![corner]),
    )
    .expect("2C1F fillet-first: first fillet (one edge) with partial-corner opt-in succeeds");
    let err = chamfer_edges(
        &mut model,
        solid_id,
        vec![edges[0], edges[1]],
        chamfer_g1_opts(D),
    )
    .expect_err(
        "2C1F fillet-first: second chamfer must route the G1 cap request to the typed reject",
    );
    assert_seam_continuity_unreachable(err, "2C1F fillet-first");
}

// ---------------------------------------------------------------------------
// CF-β regression boundary — C0 default still produces a planar cap
// ---------------------------------------------------------------------------

/// Omitting the G1 opt-in (default [`SeamContinuity::C0`]) must
/// still produce the CF-β planar N-gon cap — a `Plane`-backed face,
/// not a NURBS one. Catches an accidental flip of
/// `SeamContinuity::default()` in a future refactor.
#[test]
fn c0_default_still_produces_planar_cap() {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let edges = corner_edges(&model);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);
    chamfer_edges(
        &mut model,
        solid_id,
        vec![edges[0]],
        chamfer_c0_opts_with_partial(D, vec![corner]),
    )
    .expect("C0 1C2F chamfer-first: first chamfer succeeds");
    fillet_edges(
        &mut model,
        solid_id,
        vec![edges[1], edges[2]],
        fillet_c0_opts(D),
    )
    .expect("C0 1C2F chamfer-first: second fillet synthesizes planar cap");

    assert_eq!(non_manifold_edge_count(&model, solid_id), 0);
    let solid = model.solids.get(solid_id).expect("solid exists");
    let shell = model
        .shells
        .get(solid.outer_shell)
        .expect("shell exists");
    let mut nurbs_cap_present = false;
    let mut planar_3gon_cap_present = false;
    for &fid in &shell.faces {
        let face = model.faces.get(fid).expect("face exists");
        let surface = model
            .surfaces
            .get(face.surface_id)
            .expect("surface exists");
        if surface
            .as_any()
            .downcast_ref::<GeneralNurbsSurface>()
            .is_some()
        {
            nurbs_cap_present = true;
        }
        if surface.as_any().downcast_ref::<Plane>().is_some() {
            let outer = model
                .loops
                .get(face.outer_loop)
                .expect("planar face outer loop");
            if outer.edges.len() == 3 {
                planar_3gon_cap_present = true;
            }
        }
    }
    assert!(
        !nurbs_cap_present,
        "C0 default path must not produce a NURBS cap; SeamContinuity::default() flipped?"
    );
    assert!(
        planar_3gon_cap_present,
        "C0 default path must produce a 3-edge planar cap face"
    );
}
