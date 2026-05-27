//! CF-β.5.3 — property-test sweep for the mixed-kind corner cap.
//!
//! Complements [`cf_beta_mixed_kind_corner`] with proptest-driven
//! coverage of two contract dimensions the integration tests pin only
//! at a fixed displacement:
//!
//! * **Order invariance** under displacement `d ∈ [0.5, 2.0]`:
//!   chamfer-first and fillet-first must produce structurally
//!   identical solids (equal [`topology_hash`]) for every sampled `d`.
//!   This is the kernel-level half of the CF-β order-independence
//!   contract; the integration test pins it at the single point
//!   `d = 1.0`.
//!
//! * **Watertightness** under the same displacement sweep: every
//!   sampled `d` must produce a closed shell (V−E+F = 2, zero
//!   non-manifold edges). The integration tests pin watertightness
//!   only at `d = 1.0`; this proptest extends the guarantee across
//!   the open interval and acts as the fitness floor for any future
//!   widening of the displacement domain.
//!
//! * **Typed degree-4 rejection invariance** under the same
//!   displacement sweep on a pyramid apex: the
//!   `MixedKindUnsupported { detail: DegreeUnsupported { degree: 4 } }`
//!   contract must fire regardless of the chamfer offset. Pins that
//!   the degree-only guard is genuinely degree-driven and does not
//!   accidentally short-circuit on a particular displacement.
//!
//! ## Scope notes
//!
//! - The CF-β plan also lists an `unequal_displacements_typed_reject`
//!   property targeting [`MixedKindRejectDetail::MixedDisplacements`].
//!   The kernel today does not emit `MixedDisplacements` proactively
//!   in the feasibility pre-flight: at degree-3 the three cap
//!   vertices are *always* coplanar (any 3 points define a plane),
//!   so an unequal-displacement cap synthesises a valid (if
//!   geometrically skewed) triangular cap rather than rejecting. A
//!   proactive `MixedDisplacements` pre-flight requires the
//!   per-edge displacement registry — out of CF-β.5 scope and
//!   deferred to CF-δ together with the degree-≥4 widening.
//!
//! - The displacement range `[0.5, 2.0]` keeps every sampled `d`
//!   well inside the box's `5.0`-unit half-extent so the kernel's
//!   F2-γ.1 setback gate never trips for unrelated geometric
//!   reasons. The cap-synthesizer never sees a degenerate corner.
//!
//! - `cases = 24` keeps the suite under the geometry-engine test
//!   runtime budget while sampling the interval finely enough to
//!   surface a regression at the 0.0625-step level on average.

// AUDIT-H13: Reason for `#![allow(clippy::expect_used)]` — test-only file.
// `expect(...)` on fixture/scaffolding code surfaces invariant violations
// with a clear message at the failure site, which is the desired failure
// mode in tests. The workspace `expect_used = "deny"` lint targets
// production panic-freedom; test scaffolding is exempt by design.
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

use proptest::prelude::*;

const BOX_SIZE: f64 = 10.0;
const HALF_BOX: f64 = BOX_SIZE / 2.0;

// ---------------------------------------------------------------------
// Per-test option builders — equal-displacement chamfer / fillet.
// ---------------------------------------------------------------------

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

fn chamfer_opts_with_partial_corner(distance: f64, partial: Vec<VertexId>) -> ChamferOptions {
    ChamferOptions {
        partial_corner_vertices: partial,
        ..chamfer_opts_equal(distance)
    }
}

/// Return the three incident edges at the (+x, +y, +z) corner of a
/// `BOX_SIZE`-side cube centred at the origin, sorted by `EdgeId` so
/// the kind assignment per index is stable across the chamfer-first
/// and fillet-first build paths.
fn corner_edges(model: &BRepModel) -> Vec<EdgeId> {
    let corner = vertex_at(model, HALF_BOX, HALF_BOX, HALF_BOX);
    let mut edges = edges_at_vertex(model, corner);
    edges.sort_unstable();
    assert_eq!(edges.len(), 3, "box corner must have exactly 3 incident edges");
    edges
}

/// Build a fresh box and apply the chamfer-first-then-fillet ordering
/// at the (+x,+y,+z) corner with `d = r = displacement`. Returns the
/// resulting `(model, solid_id)` pair so callers can hash or census.
fn build_chamfer_first(displacement: f64) -> (BRepModel, geometry_engine::primitives::solid::SolidId) {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let edges = corner_edges(&model);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);

    chamfer_edges(
        &mut model,
        solid_id,
        vec![edges[0]],
        chamfer_opts_with_partial_corner(displacement, vec![corner]),
    )
    .expect("chamfer-first opt-in on a single corner edge succeeds");

    fillet_edges(
        &mut model,
        solid_id,
        vec![edges[1], edges[2]],
        fillet_opts_constant(displacement),
    )
    .expect("second-call fillet on the remaining corner edges closes the cap");

    (model, solid_id)
}

/// Mirror of [`build_chamfer_first`] for the fillet-first ordering.
fn build_fillet_first(displacement: f64) -> (BRepModel, geometry_engine::primitives::solid::SolidId) {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let edges = corner_edges(&model);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);

    fillet_edges(
        &mut model,
        solid_id,
        vec![edges[1], edges[2]],
        fillet_opts_with_partial_corner(displacement, vec![corner]),
    )
    .expect("fillet-first opt-in on two corner edges succeeds");

    chamfer_edges(
        &mut model,
        solid_id,
        vec![edges[0]],
        chamfer_opts_equal(displacement),
    )
    .expect("second-call chamfer on the remaining corner edge closes the cap");

    (model, solid_id)
}

// ---------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig {
        // 24 cases keeps the wall-clock time under ~10 s on a warm
        // build; every case rebuilds two cubes from scratch and runs
        // four blend ops, so the per-case cost is non-trivial.
        cases: 24,
        max_global_rejects: 256,
        ..ProptestConfig::default()
    })]

    /// Order invariance under `d ∈ [0.5, 2.0]`: chamfer-first and
    /// fillet-first must produce structurally identical solids
    /// (equal `topology_hash`). The hash is an isomorphism-invariant
    /// fingerprint via Weisfeiler-Lehman colour refinement, so a
    /// mismatch always points at a real structural difference (not
    /// a store-ID ordering artefact).
    #[test]
    fn prop_mixed_kind_corner_topology_order_invariant(
        displacement in 0.5_f64..2.0,
    ) {
        let (model_a, solid_a) = build_chamfer_first(displacement);
        let (model_b, solid_b) = build_fillet_first(displacement);

        let hash_a = topology_hash(&model_a, solid_a);
        let hash_b = topology_hash(&model_b, solid_b);

        prop_assert_eq!(
            hash_a, hash_b,
            "chamfer-first and fillet-first must produce structurally identical \
             solids at d = {}; hash_a = {}, hash_b = {}",
            displacement, hash_a, hash_b,
        );
    }

    /// Watertightness sweep under `d ∈ [0.5, 2.0]`: every sampled
    /// displacement must produce a closed outer shell after the
    /// second-call cap synthesis. Asserts V−E+F = 2 (genus-0) and
    /// zero non-manifold edges. Exercised through the chamfer-first
    /// ordering — fillet-first watertightness is implied by the
    /// topology-hash equality above (a structural duplicate of a
    /// closed shell is itself closed).
    #[test]
    fn prop_mixed_kind_corner_displacement_sweep_watertight(
        displacement in 0.5_f64..2.0,
    ) {
        let (model, solid_id) = build_chamfer_first(displacement);
        let nm = non_manifold_edge_count(&model, solid_id);
        prop_assert_eq!(
            nm, 0,
            "mixed-kind cap must close the corner watertight at d = {}; \
             non-manifold edge count = {}",
            displacement, nm,
        );
        let (v, e, f) = shell_census(&model, solid_id);
        let chi = v as i64 - e as i64 + f as i64;
        prop_assert_eq!(
            chi, 2,
            "Euler-Poincaré V−E+F=2 required at d = {}; got V={} E={} F={}, χ = {}",
            displacement, v, e, f, chi,
        );
    }

    /// Typed degree-4 reject invariance under `d ∈ [0.1, 1.5]`: a
    /// degree-4 pyramid-apex mixed-kind corner must surface a
    /// `MixedKindUnsupported { detail: DegreeUnsupported { degree: 4 } }`
    /// regardless of the chamfer offset / fillet radius. The upper
    /// bound `1.5` keeps every sampled `d` well inside the
    /// pyramid's geometric envelope so the rejection comes from
    /// the degree-only guard rather than an unrelated F2-γ.1 trip.
    /// The first call runs with `validate_result: false` (the
    /// degree-4 partial-mixed state has no β.4 carve-out yet); the
    /// second call's typed reject is the assertion target.
    #[test]
    fn prop_mixed_kind_corner_degree_four_pyramid_rejects_under_d_sweep(
        d in 0.1_f64..1.5,
    ) {
        let mut model = BRepModel::new();
        let solid_id = make_square_pyramid_solid(&mut model, 10.0, 10.0);
        let apex = vertex_at(&model, 0.0, 0.0, 10.0);
        let mut sloped: Vec<EdgeId> = edges_at_vertex(&model, apex);
        sloped.sort_unstable();
        prop_assert_eq!(sloped.len(), 4, "pyramid apex has exactly 4 sloped edges");

        // First call: chamfer 2 of 4 apex edges with the apex opted
        // in. `validate_result: false` so the deliberately-open
        // degree-4 partial state is not gated by the post-flight
        // (which has no β.4 carve-out for degree ≥ 4 today).
        let _ = chamfer_edges(
            &mut model,
            solid_id,
            vec![sloped[0], sloped[1]],
            ChamferOptions {
                chamfer_type: ChamferType::EqualDistance(d),
                distance1: d,
                distance2: d,
                symmetric: true,
                propagation: ChamferProp::None,
                partial_corner_vertices: vec![apex],
                common: CommonOptions {
                    validate_result: false,
                    ..Default::default()
                },
                ..Default::default()
            },
        );

        // Second call: fillet a third apex edge. Per CF-β.3.4 the
        // degree carve-out is degree-3 only; degree-4 must reject
        // with the typed `DegreeUnsupported{degree:4}`.
        let err = fillet_edges(
            &mut model,
            solid_id,
            vec![sloped[2]],
            fillet_opts_constant(d),
        )
        .expect_err("degree-4 mixed-kind corner must still reject");

        match err {
            OperationError::BlendFailed(failure) => match *failure {
                BlendFailure::VertexBlendUnsupported { reason, .. } => match reason {
                    VertexBlendUnsupportedReason::MixedKindUnsupported { detail, .. } => {
                        match detail {
                            MixedKindRejectDetail::DegreeUnsupported { degree } => {
                                prop_assert_eq!(
                                    degree, 4,
                                    "degree-4 reject must carry degree=4 payload at d = {}; got {}",
                                    d, degree
                                );
                            }
                            other => prop_assert!(
                                false,
                                "expected DegreeUnsupported{{degree:4}} at d = {}, got {:?}",
                                d, other,
                            ),
                        }
                    }
                    other => prop_assert!(
                        false,
                        "expected MixedKindUnsupported reason at d = {}, got {:?}",
                        d, other,
                    ),
                },
                other => prop_assert!(
                    false,
                    "expected VertexBlendUnsupported at d = {}, got {:?}",
                    d, other,
                ),
            },
            other => prop_assert!(
                false,
                "expected BlendFailed at d = {}, got {:?}",
                d, other,
            ),
        }
    }
}
