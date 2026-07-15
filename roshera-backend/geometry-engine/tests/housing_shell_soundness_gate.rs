// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! PER-OP SOUNDNESS-VERDICT GATE — the kernel certificate must FLAG the
//! self-intersecting / non-watertight "housing" part as UNSOUND, and it must do
//! so on the path the live per-op verdict runs (`certify_solid` → `is_sound()`).
//!
//! THE BUG THIS PINS: an earlier latency fix pulled the O(n²) self-intersection
//! pass and the full certificate OFF the per-op hot path, so the ambient verdict
//! reported `sound: true` from only the cheap O(n) checks (brep_valid +
//! display-mesh watertight). A self-intersecting / non-B-Rep-watertight solid
//! was therefore reported sound on every build op — a verdict claiming soundness
//! it never checked.
//!
//! THE REPRO (confirmed live): box(20×16×12) → boolean-difference a Ø10 (r=5)
//! cylinder bored through it → shell(thickness 2, remove the top cap). The full
//! certificate reports `watertight: false`, `self_intersection_free: false`,
//! hence `sound: false`. This test asserts the kernel's own `certify_solid`
//! verdict — the EXACT computation the per-op `certified_response` path now runs
//! after the fix — returns `is_sound() == false` for that part. If the per-op
//! path ever again reports a shallow verdict, it would no longer match this full
//! `is_sound()` answer; the verification gap is closed by construction because
//! the per-op path now CALLS `certify_solid`.
//!
//! It also pins the other half of the contract: a CLEAN part (plain box) still
//! certifies SOUND, and the certificate computes quickly (the spatial-grid
//! acceleration on the self-intersection scan keeps the full check sub-second).

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::offset::offset_solid;
use geometry_engine::operations::{CommonOptions, OffsetOptions};
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

fn shell_options() -> OffsetOptions {
    OffsetOptions {
        common: CommonOptions {
            // Mirror the API shell endpoint: do not pre-reject, so the op
            // produces the same geometry the live per-op verdict certifies.
            validate_result: false,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// The +Z–facing planar cap faces whose surface sample sits ABOVE the mid-plane
/// (z > 0) — i.e. the "top cap" of the part (a bore fragments the top into more
/// than one coplanar piece, so we collect all of them, mirroring the API's
/// coplanar-opening expansion).
fn top_caps(model: &BRepModel, solid: SolidId) -> Vec<FaceId> {
    let mut caps = Vec::new();
    let s = match model.solids.get(solid) {
        Some(s) => s.clone(),
        None => return caps,
    };
    let shell = match model.shells.get(s.outer_shell) {
        Some(sh) => sh.clone(),
        None => return caps,
    };
    for &fid in &shell.faces {
        if let Some(face) = model.faces.get(fid) {
            if let Some(surf) = model.surfaces.get(face.surface_id) {
                let normal_up = surf
                    .normal_at(0.5, 0.5)
                    .ok()
                    .and_then(|n| n.normalize().ok())
                    .map(|nn| nn.dot(&Vector3::Z).abs() > 1.0 - 1e-6)
                    .unwrap_or(false);
                let above = surf.point_at(0.5, 0.5).map(|p| p.z > 0.0).unwrap_or(false);
                if normal_up && above {
                    caps.push(fid);
                }
            }
        }
    }
    caps
}

/// Build the live housing repro: box(20×16×12) ∖ Ø10 bore (through +Z), then
/// shell t=2 with the top cap removed.
fn build_housing() -> (BRepModel, SolidId) {
    let mut model = BRepModel::new();

    // Box centred at the origin: x∈[-10,10], y∈[-8,8], z∈[-6,6].
    let block = sid(TopologyBuilder::new(&mut model)
        .create_box_3d(20.0, 16.0, 12.0)
        .expect("create box"));

    // Ø10 (r=5) cylinder, axis +Z, tall enough to pierce the block fully.
    let bore = sid(TopologyBuilder::new(&mut model)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -10.0), Vector3::Z, 5.0, 20.0)
        .expect("create bore cylinder"));

    let bored = boolean_operation(
        &mut model,
        block,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("boolean difference (bore)");

    let caps = top_caps(&model, bored);
    assert!(
        !caps.is_empty(),
        "bored block must expose at least one +Z top-cap face to remove"
    );
    let hollow = offset_solid(&mut model, bored, 2.0, caps, shell_options())
        .expect("shell (remove top cap)");
    (model, hollow)
}

/// THE GATE: the housing part is flagged UNSOUND by the kernel certificate —
/// the exact `certify_solid` verdict the per-op `certified_response` path runs.
/// The original verification gap reported this part `sound: true`; the full
/// certificate (now restored to the per-op path, spatial-grid-accelerated)
/// reports `sound: false`, and the failure is a REAL geometric defect
/// (not-watertight and/or self-intersecting), not a B-Rep-validity artifact.
#[test]
fn housing_shell_is_flagged_unsound_by_the_certificate() {
    let (mut model, hollow) = build_housing();

    let cert = model.certify_solid(hollow);

    assert!(
        !cert.is_sound(),
        "the housing shell must be flagged UNSOUND by the full certificate — \
         reporting it sound is the verification gap this gate closes. cert: \
         watertight={} self_intersection_free={} brep_valid={} manifold={}",
        cert.watertight,
        cert.self_intersection_free,
        cert.brep_valid,
        cert.manifold,
    );

    // The unsoundness must be a real GEOMETRIC defect — the live observation is
    // not-watertight AND self-intersecting. At least one of those exact flags
    // must be the cause (so the gate cannot be satisfied by an unrelated B-Rep
    // bookkeeping failure).
    assert!(
        !cert.watertight || !cert.self_intersection_free,
        "the housing's unsoundness must come from the watertight / \
         self-intersection checks (the gap-defeating geometric defects), \
         got watertight={} self_intersection_free={}",
        cert.watertight,
        cert.self_intersection_free,
    );
}

/// THE OTHER HALF OF THE CONTRACT: a CLEAN part still certifies SOUND on the
/// same full-certificate path, and computing it is cheap. A plain box must be
/// `is_sound() == true`, and the full certificate (including the spatial-grid
/// self-intersection scan) must complete well under a second.
#[test]
fn clean_box_certifies_sound_and_fast() {
    use std::time::Instant;

    let mut model = BRepModel::new();
    let block = sid(TopologyBuilder::new(&mut model)
        .create_box_3d(20.0, 16.0, 12.0)
        .expect("create box"));

    let t0 = Instant::now();
    let cert = model.certify_solid(block);
    let elapsed = t0.elapsed();

    assert!(
        cert.is_sound(),
        "a plain box must certify SOUND — watertight={} self_intersection_free={} \
         brep_valid={} manifold={}",
        cert.watertight,
        cert.self_intersection_free,
        cert.brep_valid,
        cert.manifold,
    );
    assert!(
        elapsed.as_millis() < 1000,
        "the full certificate on a clean box must be sub-second (it runs on the \
         per-op hot path); took {} ms",
        elapsed.as_millis()
    );
}
