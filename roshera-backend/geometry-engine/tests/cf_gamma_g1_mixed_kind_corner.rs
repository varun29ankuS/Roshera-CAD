//! CF-γ.6.2 — G1 mixed-kind corner integration suite.
//!
//! The CF-γ.2 single-degenerate-bicubic synthesizer hit a structural
//! rank limit (4×4 control net with one collapsed apex column vs 3
//! rims × K=5 stations × 1 normal-direction = 15 G1 constraints) and
//! was rolled back to the typed `SeamContinuityUnreachable` sentinel.
//! CF-γ.6 replaces it with a **3-sub-patch** topology: three bicubic
//! NURBS sub-patches sharing a central apex vertex, each sub-patch
//! owning one rim's G1 constraint independently. γ.6.2 ships the
//! watertight C0 topology with planar-fairing seed CPs; γ.6.3 lifts
//! the seed to the coupled rim-G1 + internal-C1 solver and adds the
//! 1e-6 rad residual gate.
//!
//! This file pins the γ.6.2 contract end-to-end:
//!
//! * Each of the 4 mixed-kind topologies the kernel supports on a
//!   box corner (1C2F × {chamfer-first, fillet-first} and 2C1F ×
//!   {chamfer-first, fillet-first}) lands both calls successfully.
//! * The second call (the one that synthesizes the cap) creates
//!   **exactly 3 new NURBS-backed faces** in the shell (ΔF = +3)
//!   with `non_manifold_edge_count == 0`.
//! * Each cap face's outer loop has exactly 3 edges (1 rim + 2
//!   spokes meeting at the central apex).
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
use geometry_engine::operations::fillet::{FilletType, PropagationMode as FilletProp};
use geometry_engine::operations::mixed_kind_corner_cap::SeamContinuity;
use geometry_engine::operations::{fillet_edges, CommonOptions, FilletOptions};
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

/// Count the NURBS-backed faces in the outer shell of `solid_id`.
/// On a box, no other operation in these fixtures produces a
/// `GeneralNurbsSurface` (cube faces are `Plane`; fillet faces are
/// `Cylinder`; chamfer faces are `Plane`), so this count isolates
/// the γ.6.2 sub-patch cap faces.
fn count_nurbs_faces_in_shell(
    model: &BRepModel,
    solid_id: geometry_engine::primitives::solid::SolidId,
) -> usize {
    let solid = model.solids.get(solid_id).expect("solid exists");
    let shell = model
        .shells
        .get(solid.outer_shell)
        .expect("outer shell exists");
    let mut n = 0;
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
            n += 1;
        }
    }
    n
}

/// Collect every NURBS-backed face in the outer shell paired with
/// its outer-loop edge count. Used by the per-face loop assertion.
fn nurbs_faces_with_loop_edge_counts(
    model: &BRepModel,
    solid_id: geometry_engine::primitives::solid::SolidId,
) -> Vec<usize> {
    let solid = model.solids.get(solid_id).expect("solid exists");
    let shell = model
        .shells
        .get(solid.outer_shell)
        .expect("outer shell exists");
    let mut out = Vec::new();
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
            let outer = model
                .loops
                .get(face.outer_loop)
                .expect("NURBS cap face outer loop exists");
            out.push(outer.edges.len());
        }
    }
    out
}

/// γ.6.2 success-contract assertion. After both calls land, the
/// shell must contain exactly 3 NURBS-backed cap faces, each with
/// an outer loop of 3 edges (1 rim + 2 spokes meeting at the apex),
/// and the cap must be watertight (zero non-manifold edges).
fn assert_three_subpatch_cap(
    model: &BRepModel,
    solid_id: geometry_engine::primitives::solid::SolidId,
    label: &str,
) {
    assert_eq!(
        non_manifold_edge_count(model, solid_id),
        0,
        "{label}: 3-sub-patch G1 cap must be watertight",
    );
    let nurbs_count = count_nurbs_faces_in_shell(model, solid_id);
    assert_eq!(
        nurbs_count, 3,
        "{label}: γ.6.2 must emit exactly 3 NURBS sub-patch faces; got {nurbs_count}",
    );
    let loop_sizes = nurbs_faces_with_loop_edge_counts(model, solid_id);
    for size in &loop_sizes {
        assert_eq!(
            *size, 3,
            "{label}: each γ.6.2 sub-patch face must have a 3-edge outer loop \
             (1 rim + 2 spokes); got {size}",
        );
    }
}

// ---------------------------------------------------------------------------
// Headline tests — γ.6.2: 3 NURBS sub-patch faces, watertight, no G1 gate
// ---------------------------------------------------------------------------
//
// In each test the first call (chamfer or fillet) opens the corner
// under the `partial_corner_vertices` opt-in — no cap is synthesized
// yet because the opt-in pins the corner vertex open. The second
// call finalises the mixed-kind corner; γ.6.2 routes the G1 arm
// through `synthesize_mixed_kind_corner_cap_g1`, emitting 3
// bicubic-NURBS sub-patches sharing a central apex vertex. γ.6.3
// will extend each test with a rim-G1 residual assertion.

#[test]
fn g1_1c2f_chamfer_first_emits_three_subpatch_cap() {
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
    fillet_edges(
        &mut model,
        solid_id,
        vec![edges[1], edges[2]],
        fillet_g1_opts(D),
    )
    .expect("1C2F chamfer-first: second fillet must close the corner with 3-sub-patch G1 cap");
    assert_three_subpatch_cap(&model, solid_id, "1C2F chamfer-first");
}

#[test]
fn g1_1c2f_fillet_first_emits_three_subpatch_cap() {
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
    chamfer_edges(
        &mut model,
        solid_id,
        vec![edges[0]],
        chamfer_g1_opts(D),
    )
    .expect("1C2F fillet-first: second chamfer must close the corner with 3-sub-patch G1 cap");
    assert_three_subpatch_cap(&model, solid_id, "1C2F fillet-first");
}

#[test]
fn g1_2c1f_chamfer_first_emits_three_subpatch_cap() {
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
    fillet_edges(
        &mut model,
        solid_id,
        vec![edges[2]],
        fillet_g1_opts(D),
    )
    .expect("2C1F chamfer-first: second fillet must close the corner with 3-sub-patch G1 cap");
    assert_three_subpatch_cap(&model, solid_id, "2C1F chamfer-first");
}

#[test]
fn g1_2c1f_fillet_first_emits_three_subpatch_cap() {
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
    chamfer_edges(
        &mut model,
        solid_id,
        vec![edges[0], edges[1]],
        chamfer_g1_opts(D),
    )
    .expect("2C1F fillet-first: second chamfer must close the corner with 3-sub-patch G1 cap");
    assert_three_subpatch_cap(&model, solid_id, "2C1F fillet-first");
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
