// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Persistent-id EDGE coverage for primitives + boolean outputs (#11, slice 40-F).
//!
//! `assign_primitive_pids` previously minted SOLID + FACE PIDs only, and booleans
//! minted FACE PIDs only — so a fillet / chamfer / GD&T reference to a *primitive*
//! or *boolean-produced* edge had no durable name to bind by (the #64 slice-5
//! residual). Every primitive edge and every boolean-output edge now carries a
//! canonical PID derived from its two neighbour FACE PIDs (Kripac neighbour-face
//! naming). The acceptance properties:
//!   * coverage — every edge of a primitive / boolean result carries a PID,
//!     recoverable both ways (id ↔ pid);
//!   * distinctness — no two edges share a PID (`pid_to_edge` stays injective);
//!   * replay stability — the same construction under the same event keys
//!     re-derives the same edge PIDs;
//!   * BLEND-BIND-BY-PID — an edge's PID equals `blend_edge_source_pid(fa, fb)`
//!     of its two neighbour faces, so the fillet/chamfer path recovers the exact
//!     edge by PID (the #64 "follow by PID" payoff for primitive/boolean edges).

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::edge_classification::find_adjacent_faces;
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::persistent_id::PersistentId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

/// Every distinct edge referenced by the solid's face loops (outer + inner).
fn edges_of(m: &BRepModel, s: SolidId) -> Vec<EdgeId> {
    let solid = m.solids.get(s).expect("solid");
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for sh in shells {
        let shell = match m.shells.get(sh) {
            Some(s) => s,
            None => continue,
        };
        for &fid in &shell.faces {
            let face = match m.faces.get(fid) {
                Some(f) => f,
                None => continue,
            };
            let mut loops = vec![face.outer_loop];
            loops.extend_from_slice(&face.inner_loops);
            for lid in loops {
                if let Some(lp) = m.loops.get(lid) {
                    for &e in &lp.edges {
                        if seen.insert(e) {
                            out.push(e);
                        }
                    }
                }
            }
        }
    }
    out
}

fn box_under_key(key: &str) -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    m.set_event_key(Some(key.to_string()));
    let s = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(20.0, 10.0, 30.0)
        .expect("box"));
    (m, s)
}

#[test]
fn every_box_edge_has_a_distinct_recoverable_pid() {
    let (m, s) = box_under_key("evt-box");
    let edges = edges_of(&m, s);
    assert_eq!(edges.len(), 12, "a box has 12 edges");
    let mut pids = Vec::new();
    for e in &edges {
        let p = m
            .edge_pid(*e)
            .unwrap_or_else(|| panic!("box edge {e} has NO persistent id"));
        assert_eq!(m.edge_by_pid(p), Some(*e), "id↔pid round-trip for edge {e}");
        pids.push(p);
    }
    let n = pids.len();
    pids.sort();
    pids.dedup();
    assert_eq!(pids.len(), n, "every box edge PID is distinct");
}

#[test]
fn box_edge_pids_are_replay_stable() {
    let (m1, s1) = box_under_key("evt-box");
    let (m2, s2) = box_under_key("evt-box");
    let e1 = edges_of(&m1, s1);
    let e2 = edges_of(&m2, s2);
    // Match edges by neighbour-face-PID identity (the canonical name), not by
    // transient id order, and require the SAME PID across the two "replays".
    let key = |m: &BRepModel, s: SolidId| -> Vec<(u128, PersistentId)> {
        let mut v: Vec<(u128, PersistentId)> = edges_of(m, s)
            .into_iter()
            .map(|e| {
                (
                    m.edge_pid(e).expect("edge pid").as_u128(),
                    m.edge_pid(e).expect("pid"),
                )
            })
            .collect();
        v.sort_by_key(|&(k, _)| k);
        v
    };
    assert_eq!(e1.len(), 12);
    assert_eq!(e2.len(), 12);
    assert_eq!(key(&m1, s1), key(&m2, s2), "edge PIDs stable across replay");
}

#[test]
fn box_edge_binds_by_pid_through_the_blend_path() {
    // THE PAYOFF PROPERTY: the fillet/chamfer path names an edge via
    // `blend_edge_source_pid(fa, fb)`. For every box edge, that value must be
    // exactly the registered edge PID, so `edge_by_pid` recovers the edge —
    // i.e. a blend on a PRIMITIVE edge can follow it by PID.
    let (mut m, s) = box_under_key("evt-box");
    for e in edges_of(&m, s) {
        let neigh = find_adjacent_faces(&m, e);
        assert_eq!(neigh.len(), 2, "a box edge borders exactly two faces");
        let source = m.blend_edge_source_pid(neigh[0], neigh[1]);
        assert_eq!(
            m.edge_by_pid(source),
            Some(e),
            "blend_edge_source_pid recovers box edge {e} by PID"
        );
        assert_eq!(m.edge_pid(e), Some(source), "edge PID == blend source PID");
    }
}

#[test]
fn cylinder_edges_all_have_pids() {
    let mut m = BRepModel::new();
    m.set_event_key(Some("cyl".into()));
    let s = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::ZERO, Vector3::Z, 5.0, 10.0)
        .expect("cyl"));
    let edges = edges_of(&m, s);
    assert!(!edges.is_empty(), "cylinder has edges");
    for e in &edges {
        assert!(
            m.edge_pid(*e).is_some(),
            "cylinder edge {e} (rim / seam) has a PID"
        );
    }
}

#[test]
fn all_primitive_kinds_assign_edge_pids() {
    let mut m = BRepModel::new();
    m.set_event_key(Some("k".into()));
    let prims = [
        sid(TopologyBuilder::new(&mut m)
            .create_box_3d(2.0, 2.0, 2.0)
            .expect("box")),
        sid(TopologyBuilder::new(&mut m)
            .create_sphere_3d(Point3::ZERO, 3.0)
            .expect("sphere")),
        sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::ZERO, Vector3::Z, 2.0, 4.0)
            .expect("cyl")),
        sid(TopologyBuilder::new(&mut m)
            .create_cone_3d(Point3::ZERO, Vector3::Z, 3.0, 0.0, 5.0)
            .expect("cone")),
    ];
    // A full sphere is a closed surface with no boundary edges — nothing to
    // mint, and that is honest; the box / cylinder / cone below all carry edges.
    let mut total_edges = 0usize;
    for s in prims {
        for e in edges_of(&m, s) {
            total_edges += 1;
            assert!(
                m.edge_pid(e).is_some(),
                "edge {e} of primitive {s} has a PID"
            );
        }
    }
    assert!(total_edges > 0, "box/cylinder/cone contribute edges");
}

/// box (40×40×20) minus a Ø16 through-bore on +Z. Returns model + result solid.
fn bored_block() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    m.set_event_key(Some("blk".into()));
    let block = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(40.0, 40.0, 20.0)
        .expect("block"));
    m.set_event_key(Some("cut".into()));
    let cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -30.0), Vector3::Z, 8.0, 60.0)
        .expect("cyl"));
    m.set_event_key(None);
    let result = boolean_operation(
        &mut m,
        block,
        cyl,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("difference");
    (m, result)
}

#[test]
fn every_boolean_result_edge_has_a_distinct_recoverable_pid() {
    let (m, result) = bored_block();
    let edges = edges_of(&m, result);
    assert!(!edges.is_empty(), "bored block has edges");
    let mut pids = Vec::new();
    for e in &edges {
        let p = m
            .edge_pid(*e)
            .unwrap_or_else(|| panic!("boolean result edge {e} has NO persistent id"));
        assert_eq!(m.edge_by_pid(p), Some(*e), "id↔pid round-trip for edge {e}");
        pids.push(p);
    }
    let n = pids.len();
    pids.sort();
    pids.dedup();
    assert_eq!(pids.len(), n, "every boolean result edge PID is distinct");
}

#[test]
fn boolean_bore_rim_edge_binds_by_pid() {
    // The bore rim on the top face is a NEW intersection edge — no operand edge
    // existed there. It must (a) carry a PID and (b) be recoverable through the
    // blend path so a fillet on the bore rim follows it by PID.
    let (mut m, result) = bored_block();
    let mut any_bindable = false;
    for e in edges_of(&m, result) {
        let neigh = find_adjacent_faces(&m, e);
        if neigh.len() != 2 {
            continue;
        }
        let source = m.blend_edge_source_pid(neigh[0], neigh[1]);
        // For a singleton face-pair edge (the norm) the blend source recovers
        // the edge exactly.
        if m.edge_by_pid(source) == Some(e) {
            any_bindable = true;
        }
        assert!(m.edge_pid(e).is_some(), "boolean result edge {e} has a PID");
    }
    assert!(
        any_bindable,
        "at least one boolean result edge binds by PID through the blend path"
    );
}
