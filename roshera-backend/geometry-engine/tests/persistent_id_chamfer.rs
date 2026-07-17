// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Persistent-id chamfer lineage harness (#11 slice 40-E, chamfer).
//!
//! A chamfer must PROPAGATE persistent-id lineage exactly as a fillet does:
//!   * DURABILITY — a PID minted on a face BEFORE the chamfer still resolves to
//!     the SAME face AFTER (adjacent faces re-trimmed in place keep their id);
//!   * COVERAGE — every synthesized chamfer bevel face carries a distinct,
//!     round-trippable PID;
//!   * REAL derivation — a bevel face's PID equals the independently-recomputed
//!     `ChamferBevel` derivation from its source edge's canonical identity;
//!   * MOULD stability — re-chamfering the SAME edge at a DIFFERENT setback
//!     under the same event key yields the SAME bevel-face PID.

use geometry_engine::operations::chamfer::{ChamferType, PropagationMode as ChamferPropagation};
use geometry_engine::operations::{chamfer_edges, ChamferOptions};
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

fn box_and_first_edge(key: &str) -> (BRepModel, SolidId, EdgeId) {
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
    (model, solid_id, edge)
}

fn chamfer_opts(distance: f64) -> ChamferOptions {
    ChamferOptions {
        chamfer_type: ChamferType::EqualDistance(distance),
        distance1: distance,
        distance2: distance,
        symmetric: true,
        propagation: ChamferPropagation::None,
        ..Default::default()
    }
}

fn chamfer_first_edge(key: &str, distance: f64) -> (BRepModel, SolidId, EdgeId, Vec<FaceId>) {
    let (mut model, solid_id, edge) = box_and_first_edge(key);
    let bevels =
        chamfer_edges(&mut model, solid_id, vec![edge], chamfer_opts(distance)).expect("chamfer");
    (model, solid_id, edge, bevels)
}

#[test]
fn every_chamfer_bevel_face_has_a_distinct_pid() {
    let (m, _s, _e, bevels) = chamfer_first_edge("cha-cov", 1.0);
    assert_eq!(
        bevels.len(),
        1,
        "single-edge equal-distance chamfer = 1 bevel"
    );
    let mut pids = Vec::new();
    for f in &bevels {
        let p = m
            .face_pid(*f)
            .unwrap_or_else(|| panic!("chamfer bevel face {f} has no PID"));
        assert_eq!(m.face_by_pid(p), Some(*f), "id<->pid round-trip");
        pids.push(p);
    }
    let n = pids.len();
    pids.sort();
    pids.dedup();
    assert_eq!(pids.len(), n, "all bevel-face PIDs distinct");
}

#[test]
fn pre_chamfer_face_pids_survive_the_chamfer() {
    let (mut model, solid_id, edge) = box_and_first_edge("cha-durab");
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
    chamfer_edges(&mut model, solid_id, vec![edge], chamfer_opts(1.0)).expect("chamfer");
    for (pid, orig_face) in before {
        let resolved = model
            .face_by_pid(pid)
            .unwrap_or_else(|| panic!("pre-chamfer face PID {pid:?} lost after chamfer"));
        assert_eq!(
            resolved, orig_face,
            "pre-chamfer face PID must still resolve to the SAME face"
        );
    }
}

#[test]
fn bevel_face_pid_is_the_principled_chamfer_bevel_derivation() {
    let (mut model, solid_id, edge) = box_and_first_edge("cha-deep");
    let (fa, fb) = adjacent_face_pids(&model, solid_id, edge);
    let bevels =
        chamfer_edges(&mut model, solid_id, vec![edge], chamfer_opts(1.0)).expect("chamfer");
    let bevel = bevels[0];
    let got = model.face_pid(bevel).expect("bevel pid");

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
        "chamfer_edges",
        &Role::ChamferBevel {
            source_edge_pid: edge_pid,
        },
    );
    assert_eq!(
        got, expected,
        "bevel PID is the principled ChamferBevel derivation from its source edge"
    );
}

#[test]
fn bevel_face_pid_is_stable_across_a_setback_mould() {
    let (m1, _s1, _e1, b1) = chamfer_first_edge("cha-mould", 1.0);
    let (m2, _s2, _e2, b2) = chamfer_first_edge("cha-mould", 2.0);
    assert_eq!(b1.len(), 1);
    assert_eq!(b2.len(), 1);
    let p1 = m1.face_pid(b1[0]).expect("bevel d=1 pid");
    let p2 = m2.face_pid(b2[0]).expect("bevel d=2 pid");
    assert_eq!(
        p1, p2,
        "bevel face over the same edge keeps its PID across a setback mould"
    );
}

/// The two faces adjacent to `edge` on `solid`, by PID (pre-surgery).
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
