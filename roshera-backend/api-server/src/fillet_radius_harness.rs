//! F3-ε.2 Slice E — cross-layer wire-shape harness for variable-radius
//! fillets.
//!
//! The per-layer unit suites already exist:
//!
//! - **Slice A** — kernel `FilletType` variant pins
//!   (`geometry_engine::operations::fillet` tests).
//! - **Slice B** — `BlendRadiusDto` serde tests in
//!   `timeline_engine::types`.
//! - **Slice C** — `fillet_payload::tests` (28 unit tests covering
//!   every legal wire shape + every negative path).
//!
//! This harness is the integration glue that pins the **whole pipeline**
//! in a single test per profile: the exact JSON shape the frontend
//! emits flows through the REST parser, becomes a
//! [`BlendRadiusDto`](timeline_engine::BlendRadiusDto), translates to
//! a kernel [`FilletType`](geometry_engine::operations::fillet::FilletType),
//! and drives a real `fillet_edges` call on real B-Rep geometry. If any
//! layer drifts (a renamed `kind` tag, a parser variant the kernel
//! doesn't accept, a DTO Serialize the parser can't re-parse), exactly
//! one of these tests fails with a clean stack pointing at the broken
//! seam.
//!
//! # What we deliberately do *not* test here
//!
//! - HTTP routing / extractors. The api-server's `Router::new()` is
//!   still inlined in `main()` (Diagnostics-α Phase-3, Task #52). Once
//!   that's factored into `build_router(state) -> Router`, a follow-up
//!   slice can wrap these in a full `axum::Server` test. Until then
//!   the harness covers everything *south* of the router.
//! - Frontend rendering. The TypeScript tsc pass on `ModifyDialog.tsx`
//!   + the explicit JSON `body:` literal in `sendDirectFilletLinear` /
//!   `sendDirectFilletStations` is the frontend's own contract — we
//!   pin its emitted shape here by replaying it as fixtures.
//! - Cross-edge propagation correctness. Each test runs against a
//!   single isolated box edge so the kernel's own variable-radius
//!   solver is the only moving part.

#![cfg(test)]

use crate::fillet_payload::{parse_fillet_radii, FilletRadii};
use geometry_engine::operations::fillet::{
    fillet_edges, FilletOptions, FilletType, PropagationMode,
};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use serde_json::{json, Value};

// =====================================================================
// Geometry fixtures
// =====================================================================

/// Build an axis-aligned box + return `(model, solid_id, first_edge)`.
/// First edge is a non-loop edge picked deterministically by the
/// underlying store iteration order; sufficient for tests that only
/// care that the wire shape lands the right kernel dispatch — not
/// the geometric outcome of a specific edge.
fn box_first_edge(w: f64, h: f64, d: f64) -> (BRepModel, SolidId, EdgeId) {
    let mut model = BRepModel::new();
    let solid_id = {
        let mut builder = TopologyBuilder::new(&mut model);
        match builder
            .create_box_3d(w, h, d)
            .expect("box primitive must build for positive dims")
        {
            GeometryId::Solid(id) => id,
            other => panic!("expected solid, got {other:?}"),
        }
    };
    let edge = model
        .edges
        .iter()
        .find_map(|(id, e)| if !e.is_loop() { Some(id) } else { None })
        .expect("box always carries at least one open edge");
    (model, solid_id, edge)
}

// =====================================================================
// Pipeline helpers
// =====================================================================

/// Parse a complete frontend-shaped fillet payload (the JSON body the
/// browser POSTs) into a [`FilletRadii`]. The `edges` array is
/// embedded in the payload exactly as the frontend ships it; we pass
/// its length through so the parser's `radii` length-matching path
/// fires identically to the live handler.
fn parse_frontend_payload(payload: &Value) -> FilletRadii {
    let edge_count = payload
        .get("edges")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .expect("test fixtures must carry an `edges` array");
    parse_fillet_radii(payload, edge_count).expect("test fixtures must parse cleanly")
}

/// Take a parsed `FilletRadii`, translate every profile to the kernel
/// `FilletType` dispatch shape, and apply each one as a single-edge
/// fillet on the supplied edge. Returns the *post-op* face count of
/// the affected solid — the cheapest "the operation actually
/// happened" check available without re-tessellating.
fn drive_kernel(
    model: &mut BRepModel,
    solid_id: SolidId,
    edge: EdgeId,
    radii: &FilletRadii,
) -> usize {
    for i in 0..radii.profiles.len() {
        let opts = FilletOptions {
            fillet_type: radii.to_fillet_type(i),
            propagation: PropagationMode::None,
            ..FilletOptions::default()
        };
        fillet_edges(model, solid_id, vec![edge], opts)
            .expect("kernel must accept the wire-derived FilletType for harness fixtures");
    }
    let solid = model.solids.get(solid_id).expect("solid must survive op");
    let shell = model
        .shells
        .get(solid.outer_shell)
        .expect("shell must survive op");
    shell.faces.len()
}

// =====================================================================
// Cross-layer pipeline tests
// =====================================================================

/// Bare-number `radius: r` — the legacy wire shape every existing
/// client uses. Must continue to parse, translate to
/// `FilletType::Constant`, and run a successful fillet. This is the
/// "F3-ε.2 cannot regress F0" pin.
#[tokio::test]
async fn legacy_bare_number_round_trips_to_kernel_constant() {
    let (mut model, solid_id, edge) = box_first_edge(10.0, 10.0, 10.0);
    let face_count_before = model
        .shells
        .get(model.solids.get(solid_id).unwrap().outer_shell)
        .unwrap()
        .faces
        .len();

    let payload = json!({ "object": "deadbeef", "edges": [42], "radius": 1.5 });
    let radii = parse_frontend_payload(&payload);

    assert!(
        radii.uniform_constant,
        "bare-number radius must set the kernel fast path"
    );
    match radii.to_fillet_type(0) {
        FilletType::Constant(r) => assert!((r - 1.5).abs() < 1e-12),
        other => panic!("expected Constant(1.5), got {other:?}"),
    }

    let face_count_after = drive_kernel(&mut model, solid_id, edge, &radii);
    assert!(
        face_count_after > face_count_before,
        "fillet must add at least one face (the blend surface); before={face_count_before}, after={face_count_after}"
    );
}

/// Tagged-`Constant` wire shape — what the canonical Serialize emits
/// and what an agent that wants explicit profile typing should send.
/// Must produce kernel-identical behaviour to the bare number above.
#[tokio::test]
async fn tagged_constant_round_trips_to_kernel_constant() {
    let (mut model, solid_id, edge) = box_first_edge(10.0, 10.0, 10.0);

    let payload = json!({
        "object": "deadbeef",
        "edges": [42],
        "radius": { "kind": "constant", "value": 1.5 },
    });
    let radii = parse_frontend_payload(&payload);

    assert!(radii.uniform_constant);
    match radii.to_fillet_type(0) {
        FilletType::Constant(r) => assert!((r - 1.5).abs() < 1e-12),
        other => panic!("expected Constant(1.5), got {other:?}"),
    }

    drive_kernel(&mut model, solid_id, edge, &radii);
}

/// Linear profile — exactly the JSON `sendDirectFilletLinear` ships.
/// Pinning this body verbatim guarantees the wire never drifts from
/// what the frontend emits.
#[tokio::test]
async fn linear_profile_drives_kernel_variable_endpoints() {
    let (mut model, solid_id, edge) = box_first_edge(20.0, 20.0, 20.0);

    // Byte-for-byte the shape `sendDirectFilletLinear` POSTs.
    let payload = json!({
        "object": "deadbeef",
        "edges": [42],
        "radius": { "kind": "linear", "start": 1.0, "end": 3.0 },
    });
    let radii = parse_frontend_payload(&payload);

    assert!(
        !radii.uniform_constant,
        "Linear profile must NOT engage the constant fast path"
    );
    match radii.to_fillet_type(0) {
        FilletType::Variable(s, e) => {
            assert!((s - 1.0).abs() < 1e-12, "start must thread untouched");
            assert!((e - 3.0).abs() < 1e-12, "end must thread untouched");
        }
        other => panic!("expected Variable(1.0, 3.0), got {other:?}"),
    }

    drive_kernel(&mut model, solid_id, edge, &radii);
}

/// Variable-station profile — exactly the JSON
/// `sendDirectFilletStations` ships. A box edge gives the 3-valent
/// corners the variable-radius surgery path requires; the cylinder
/// rim's seam vertex is 2-valent and would (correctly) reject the op.
#[tokio::test]
async fn variable_profile_drives_kernel_variable_stations() {
    let (mut model, solid_id, edge) = box_first_edge(20.0, 20.0, 20.0);

    // Byte-for-byte the shape `sendDirectFilletStations` POSTs.
    let payload = json!({
        "object": "deadbeef",
        "edges": [42],
        "radius": {
            "kind": "variable",
            "samples": [[0.0, 0.5], [0.5, 1.0], [1.0, 0.5]],
        },
    });
    let radii = parse_frontend_payload(&payload);

    assert!(!radii.uniform_constant);
    match radii.to_fillet_type(0) {
        FilletType::VariableStations(samples) => {
            assert_eq!(samples.len(), 3, "every station must thread through");
            assert!((samples[0].0 - 0.0).abs() < 1e-12);
            assert!((samples[0].1 - 0.5).abs() < 1e-12);
            assert!((samples[1].0 - 0.5).abs() < 1e-12);
            assert!((samples[1].1 - 1.0).abs() < 1e-12);
            assert!((samples[2].0 - 1.0).abs() < 1e-12);
            assert!((samples[2].1 - 0.5).abs() < 1e-12);
        }
        other => panic!("expected VariableStations, got {other:?}"),
    }

    drive_kernel(&mut model, solid_id, edge, &radii);
}

/// Per-edge `radii` array — the existing per-edge-constant wire shape
/// (`fillet-variable` mode). The frontend ships bare numbers in the
/// array; the parser produces N parallel `Constant` profiles. This
/// test pins the per-edge dispatch path against the same kernel
/// execution other tests cover.
#[tokio::test]
async fn per_edge_radii_array_round_trips_to_kernel_constants() {
    let (mut model, solid_id, edge) = box_first_edge(15.0, 15.0, 15.0);

    // Single edge but `radii` shape — proves the parser handles an
    // array of length 1 without flipping to the `radius`-scalar
    // branch. Length-matching is part of the contract.
    let payload = json!({
        "object": "deadbeef",
        "edges": [42],
        "radii": [1.25],
    });
    let radii = parse_frontend_payload(&payload);

    assert_eq!(radii.profiles.len(), 1);
    assert!(
        radii.uniform_constant,
        "single-element array of equal Constants is still uniform"
    );
    match radii.to_fillet_type(0) {
        FilletType::Constant(r) => assert!((r - 1.25).abs() < 1e-12),
        other => panic!("expected Constant(1.25), got {other:?}"),
    }

    drive_kernel(&mut model, solid_id, edge, &radii);
}

/// Canonical round-trip: serialize the DTO output back to JSON, then
/// feed *that* through the parser. The result must be the same DTO.
/// This pins serde Serialize ↔ Deserialize symmetry, which is the
/// invariant the broadcast frame relies on (the api-server echoes
/// `canonical_per_edge` to subscribers; the next replay parses it
/// back).
#[tokio::test]
async fn canonical_serialize_parses_back_to_identical_dto() {
    let originals = vec![
        json!({ "edges": [1], "radius": 2.0 }),
        json!({ "edges": [1], "radius": { "kind": "constant", "value": 2.0 } }),
        json!({ "edges": [1], "radius": { "kind": "linear", "start": 1.5, "end": 2.5 } }),
        json!({
            "edges": [1],
            "radius": { "kind": "variable", "samples": [[0.0, 1.0], [0.5, 2.0], [1.0, 1.0]] },
        }),
    ];

    for orig in originals {
        let first = parse_frontend_payload(&orig);
        // Echo back the canonical form as if it were a fresh request.
        let echoed = json!({
            "edges": [1],
            "radius": first.canonical_per_edge[0].clone(),
        });
        let second = parse_frontend_payload(&echoed);

        // The two DTOs must be byte-identical when re-serialized —
        // we go through JSON Value rather than `PartialEq` because
        // `BlendRadiusDto::Variable(samples)` is `Vec<(f64, f64)>`
        // which has the float-equality concern; comparing JSON gives
        // us exact textual equality for the canonical-shape pin.
        let canon1 = &first.canonical_per_edge[0];
        let canon2 = &second.canonical_per_edge[0];
        assert_eq!(
            canon1, canon2,
            "canonical wire shape must be a fixed point of the parser"
        );
    }
}

/// Negative path pin: a wire shape with a `kind` value the parser
/// doesn't recognise must be rejected at the REST edge, *not* by
/// the kernel. This is the contract that prevents agent-emitted
/// typos from reaching the geometry layer.
#[tokio::test]
async fn unknown_kind_rejected_at_parser_not_kernel() {
    let payload = json!({
        "edges": [42],
        "radius": { "kind": "bogus", "value": 1.0 },
    });
    let result = parse_fillet_radii(&payload, 1);
    assert!(
        result.is_err(),
        "unknown kind must reject before reaching the kernel"
    );
    let err = result.unwrap_err();
    assert_eq!(err.code, crate::error_catalog::ErrorCode::InvalidParameter);
    assert!(
        err.error.contains("unknown kind"),
        "rejection message must point at the discriminant, got: {}",
        err.error
    );
}

/// Frontend symmetry pin: every `kind` tag the frontend emits
/// (`constant`, `linear`, `variable`) must round-trip through the
/// parser. If somebody changes the DTO's canonical Serialize tag
/// (e.g., to PascalCase) without updating the frontend, this test
/// catches the asymmetry at build time.
#[tokio::test]
async fn frontend_emitted_kind_tags_are_all_parser_known() {
    for kind in ["constant", "linear", "variable"] {
        let payload = match kind {
            "constant" => json!({ "edges": [1], "radius": { "kind": kind, "value": 1.0 } }),
            "linear" => {
                json!({ "edges": [1], "radius": { "kind": kind, "start": 1.0, "end": 2.0 } })
            }
            "variable" => json!({
                "edges": [1],
                "radius": { "kind": kind, "samples": [[0.0, 1.0], [1.0, 1.0]] },
            }),
            _ => unreachable!(),
        };
        let parsed = parse_fillet_radii(&payload, 1);
        assert!(
            parsed.is_ok(),
            "frontend tag '{kind}' must parse; got {:?}",
            parsed.err()
        );
    }
}
