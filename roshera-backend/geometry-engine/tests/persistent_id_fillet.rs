// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Persistent-id fillet lineage harness (#11 slice 40-E, fillet).
//!
//! A fillet must PROPAGATE persistent-id lineage, not drop it:
//!   * DURABILITY — a PID minted on a face BEFORE the fillet still resolves to
//!     the geometrically-corresponding face AFTER (the adjacent faces are
//!     re-trimmed in place, keeping their FaceId, so their identity survives);
//!   * COVERAGE — every newly synthesized fillet roll face carries a distinct,
//!     round-trippable PID (an agent's "the fillet on edge e" is addressable);
//!   * REAL derivation — a roll face's PID equals the independently-recomputed
//!     `FilletRoll` derivation from its source edge's canonical identity, so it
//!     is real lineage and not a positional afterthought;
//!   * MOULD stability — re-filleting the SAME edge at a DIFFERENT radius under
//!     the same event key yields the SAME roll-face PID (identity derives from
//!     the source edge, not the geometry).

use geometry_engine::operations::fillet::{FilletType, PropagationMode as FilletPropagation};
use geometry_engine::operations::{fillet_edges, FilletOptions};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::persistent_id::{PersistentId, Role};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn expect_solid(geom: GeometryId) -> SolidId {
    match geom {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid, got {o:?}"),
    }
}

/// Build a 10×10×10 box under `key` and fillet its first edge at `radius`.
/// Returns the model, solid, the filleted edge id, and the emitted blend faces.
fn fillet_first_edge(key: &str, radius: f64) -> (BRepModel, SolidId, EdgeId, Vec<FaceId>) {
    let mut model = BRepModel::new();
    model.set_event_key(Some(key.to_string()));
    let solid_id = expect_solid(
        TopologyBuilder::new(&mut model)
            .create_box_3d(10.0, 10.0, 10.0)
            .expect("box"),
    );
    let edge = model
        .edges
        .iter()
        .map(|(id, _)| id)
        .next()
        .expect("box has edges");
    let opts = FilletOptions {
        fillet_type: FilletType::Constant(radius),
        radius,
        propagation: FilletPropagation::None,
        ..Default::default()
    };
    let blend = fillet_edges(&mut model, solid_id, vec![edge], opts).expect("single-edge fillet");
    (model, solid_id, edge, blend)
}

#[test]
fn every_fillet_roll_face_has_a_distinct_pid() {
    let (m, _s, _e, blend) = fillet_first_edge("fil-cov", 2.0);
    assert_eq!(blend.len(), 1, "single-edge constant fillet = 1 roll face");
    let mut pids = Vec::new();
    for f in &blend {
        let p = m
            .face_pid(*f)
            .unwrap_or_else(|| panic!("fillet roll face {f} has no PID"));
        assert_eq!(m.face_by_pid(p), Some(*f), "id<->pid round-trip");
        pids.push(p);
    }
    let n = pids.len();
    pids.sort();
    pids.dedup();
    assert_eq!(pids.len(), n, "all roll-face PIDs distinct");
}

#[test]
fn pre_fillet_face_pids_survive_the_fillet() {
    // Every face PID minted at box-creation must still resolve to a live face
    // after the fillet (adjacent faces re-trimmed in place keep their FaceId;
    // untouched faces are wholly unaffected).
    let mut model = BRepModel::new();
    model.set_event_key(Some("fil-durab".to_string()));
    let solid_id = expect_solid(
        TopologyBuilder::new(&mut model)
            .create_box_3d(10.0, 10.0, 10.0)
            .expect("box"),
    );
    // Snapshot (pid -> face) for all box faces before the fillet.
    let before: Vec<(PersistentId, FaceId)> = model
        .solids
        .get(solid_id)
        .map(|s| s.outer_shell)
        .and_then(|sh| model.shells.get(sh))
        .map(|shell| {
            shell
                .faces
                .iter()
                .filter_map(|&f| model.face_pid(f).map(|p| (p, f)))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(before.len(), 6, "clean box carries 6 face PIDs");

    let edge = model
        .edges
        .iter()
        .map(|(id, _)| id)
        .next()
        .expect("box has edges");
    let opts = FilletOptions {
        fillet_type: FilletType::Constant(2.0),
        radius: 2.0,
        propagation: FilletPropagation::None,
        ..Default::default()
    };
    fillet_edges(&mut model, solid_id, vec![edge], opts).expect("fillet");

    for (pid, orig_face) in before {
        let resolved = model
            .face_by_pid(pid)
            .unwrap_or_else(|| panic!("pre-fillet face PID {pid:?} lost after fillet"));
        assert_eq!(
            resolved, orig_face,
            "pre-fillet face PID must still resolve to the SAME face"
        );
    }
}

#[test]
fn roll_face_pid_is_the_principled_fillet_roll_derivation() {
    // Deep: the roll face's PID must equal the independently recomputed
    // FilletRoll derivation from its source edge's canonical identity. The box
    // edge carries no upstream PID, so its identity is the documented mint:
    // derive([min(fa,fb), max(fa,fb)], "blend_edge", Generic{ min, "shared" }).
    //
    // The filleted edge is consumed by surgery, so the two neighbour face PIDs
    // are captured BEFORE the fillet (the faces themselves survive in place).
    let mut model = BRepModel::new();
    model.set_event_key(Some("fil-deep".to_string()));
    let solid_id = expect_solid(
        TopologyBuilder::new(&mut model)
            .create_box_3d(10.0, 10.0, 10.0)
            .expect("box"),
    );
    let edge = model
        .edges
        .iter()
        .map(|(id, _)| id)
        .next()
        .expect("box has edges");
    let (fa, fb) = adjacent_face_pids(&model, solid_id, edge);

    let opts = FilletOptions {
        fillet_type: FilletType::Constant(2.0),
        radius: 2.0,
        propagation: FilletPropagation::None,
        ..Default::default()
    };
    let blend = fillet_edges(&mut model, solid_id, vec![edge], opts).expect("fillet");
    let roll = blend[0];
    let got = model.face_pid(roll).expect("roll pid");

    let (lo, hi) = if fa.as_u128() <= fb.as_u128() {
        (fa, fb)
    } else {
        (fb, fa)
    };
    let edge_pid = PersistentId::derive(
        &[lo, hi],
        "blend_edge",
        &Role::Generic {
            source_pid: lo,
            label: "shared".to_string(),
        },
    );
    let expected = PersistentId::derive(
        &[edge_pid],
        "fillet_edges",
        &Role::FilletRoll {
            source_edge_pid: edge_pid,
        },
    );
    assert_eq!(
        got, expected,
        "roll PID is the principled FilletRoll derivation from its source edge"
    );
}

#[test]
fn roll_face_pid_is_stable_across_a_radius_mould() {
    let (m1, _s1, _e1, b1) = fillet_first_edge("fil-mould", 2.0);
    let (m2, _s2, _e2, b2) = fillet_first_edge("fil-mould", 4.0);
    assert_eq!(b1.len(), 1);
    assert_eq!(b2.len(), 1);
    let p1 = m1.face_pid(b1[0]).expect("roll r=2 pid");
    let p2 = m2.face_pid(b2[0]).expect("roll r=4 pid");
    assert_eq!(
        p1, p2,
        "roll face over the same edge keeps its PID across a radius mould"
    );
}

/// Resolve the persistent ids of the two faces adjacent to `edge` on `solid`,
/// mirroring how the fillet derives the source-edge identity.
fn adjacent_face_pids(m: &BRepModel, solid: SolidId, edge: EdgeId) -> (PersistentId, PersistentId) {
    let shell = m
        .solids
        .get(solid)
        .and_then(|s| m.shells.get(s.outer_shell))
        .expect("shell");
    let mut adj = Vec::new();
    for &f in &shell.faces {
        let Some(face) = m.faces.get(f) else { continue };
        let on_face = std::iter::once(face.outer_loop)
            .chain(face.inner_loops.iter().copied())
            .filter_map(|lid| m.loops.get(lid))
            .any(|l| l.edges.contains(&edge));
        if on_face {
            if let Some(p) = m.face_pid(f) {
                adj.push(p);
            }
        }
    }
    assert!(adj.len() >= 2, "edge should border two PID'd faces");
    (adj[0], adj[1])
}
