// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! P1 enforcement RED→GREEN gate.
//!
//! `roshera-mcp` documented the verification cadence ("ALWAYS call
//! verify_part after a boolean or multi-feature build") as a STRING in a
//! tool description — steering the agent can ignore, and does exactly when
//! the task gets hard. This pins the constraint that replaces it:
//! `BRepModel::soundness_reading` must report `Stale` for a solid mutated
//! since its last full verification — never the previous `Sound` verdict,
//! and never a silently-recomputed fresh one that lets a caller skip the
//! explicit `verify_part` step and still get a passing answer.
//!
//! Three properties, one test each:
//! 1. `soundness_reading_goes_stale_after_a_mutating_blend` — the RED case:
//!    verify, mutate (fillet — a real blend, same solid_id in place), read.
//! 2. `stale_reading_never_satisfies_a_pass_check` — a `Stale` reading is
//!    structurally distinct from `Sound`/`Unsound`; a caller that only
//!    checks `is_sound()` (the shape of every real gate in this codebase)
//!    must see `false`.
//! 3. `reverifying_clears_staleness` — the remedy (`certify_solid`, what
//!    `verify_part` calls) actually heals the reading back to current truth.

#[path = "blend_fixtures/mod.rs"]
mod blend_fixtures;

use blend_fixtures::{edges_at_vertex, make_cube, vertex_at};

/// Pick one edge incident to `vertex` — a single-edge blend selection,
/// mirroring the same helper in `blend_conflict_detection.rs`.
fn pick_one_edge_at_vertex(
    model: &BRepModel,
    vertex: geometry_engine::primitives::vertex::VertexId,
) -> geometry_engine::primitives::edge::EdgeId {
    *edges_at_vertex(model, vertex)
        .first()
        .expect("at least one edge incident to vertex")
}

use geometry_engine::operations::fillet::{FilletType, PropagationMode as FilletProp};
use geometry_engine::operations::{fillet_edges, CommonOptions, FilletOptions};
use geometry_engine::primitives::provenance::SoundnessReading;
use geometry_engine::primitives::topology_builder::BRepModel;

const BOX_SIZE: f64 = 10.0;
const HALF_BOX: f64 = BOX_SIZE / 2.0;
const RADIUS: f64 = 0.5;

fn fillet_opts() -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(RADIUS),
        radius: RADIUS,
        propagation: FilletProp::None,
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// The shape of every real soundness gate in this codebase (DFM precondition,
/// export refusal): "may I trust this part right now?"
fn passes_gate(reading: &SoundnessReading) -> bool {
    reading.is_sound()
}

#[test]
fn soundness_reading_goes_stale_after_a_mutating_blend() {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);

    // Explicit full verification — mirrors the agent's `verify_part` call.
    let verified = model.certify_solid(solid_id);
    assert!(verified.is_sound(), "as-built cube must verify sound");

    // Freshly verified: the read-only reading must be `Sound`, matching the
    // certificate just computed — no recompute happens inside the accessor.
    let reading = model.soundness_reading(solid_id).expect("solid exists");
    assert_eq!(
        reading,
        SoundnessReading::Sound(verified.clone()),
        "immediately after an explicit verify, the reading must be Sound"
    );
    assert!(passes_gate(&reading));

    // Mutate via a real blend (fillet), IN PLACE — same solid_id, exactly
    // the "boolean or multi-feature build" scenario the old tool-description
    // string warned about.
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);
    let edge = pick_one_edge_at_vertex(&model, corner);
    fillet_edges(&mut model, solid_id, vec![edge], fillet_opts())
        .expect("fillet on a fresh single edge succeeds");

    let after = model
        .soundness_reading(solid_id)
        .expect("solid still exists after fillet");
    assert!(
        after.is_stale(),
        "a solid mutated since its last full verification must read as \
         Stale, got {after:?}"
    );
    // Structurally distinct: the stale reading can never equal the earlier
    // Sound verdict, so even a naive `==` comparison cannot mistake it for
    // a pass.
    assert_ne!(after, SoundnessReading::Sound(verified));
}

#[test]
fn stale_reading_never_satisfies_a_pass_check() {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);

    // Never verified at all — the other route into `Stale` (never
    // certified, not just "certified then mutated").
    let never_verified = model.soundness_reading(solid_id).expect("solid exists");
    assert!(never_verified.is_stale());
    assert!(
        !passes_gate(&never_verified),
        "a Stale reading must never satisfy a pass check anywhere \
         (never-verified case)"
    );

    // Verify, mutate, and confirm the post-mutation reading fails the same
    // gate a genuinely unsound part would fail.
    model.certify_solid(solid_id);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);
    let edge = pick_one_edge_at_vertex(&model, corner);
    fillet_edges(&mut model, solid_id, vec![edge], fillet_opts())
        .expect("fillet on a fresh single edge succeeds");

    let stale = model.soundness_reading(solid_id).expect("solid exists");
    assert!(stale.is_stale());
    assert!(
        !passes_gate(&stale),
        "a Stale reading must never satisfy a pass check anywhere \
         (mutated-since-verify case)"
    );
}

#[test]
fn reverifying_clears_staleness() {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    model.certify_solid(solid_id);

    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);
    let edge = pick_one_edge_at_vertex(&model, corner);
    fillet_edges(&mut model, solid_id, vec![edge], fillet_opts())
        .expect("fillet on a fresh single edge succeeds");

    assert!(
        model
            .soundness_reading(solid_id)
            .expect("solid exists")
            .is_stale(),
        "precondition: the fillet must have left the part stale"
    );

    // The remedy — the same call `verify_part` makes.
    let reverified = model.certify_solid(solid_id);
    let healed = model.soundness_reading(solid_id).expect("solid exists");
    assert!(!healed.is_stale(), "re-verifying must clear staleness");
    assert_eq!(
        healed,
        SoundnessReading::Sound(reverified),
        "the healed reading must reflect the CURRENT (post-fillet) \
         certificate, not a fabricated or stale one"
    );
    assert!(passes_gate(&healed));
}
