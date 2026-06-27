//! POST-BOOLEAN FILLET ROBUSTNESS — `fillet all edges` on a part that has been
//! through a boolean. Pinned from a live MCP dogfood: a mounting bracket with
//! drilled holes refused `fillet_edges` (all edges) with a CLIFF /
//! rank-deficient-setback error even though the part was SOUND. The kernel
//! never shipped broken geometry — it OVER-REFUSED. Three independent
//! all-or-nothing refusals were fixed so the blend rounds everything it can:
//!
//!   1. SEAM edges (blend_graph::build) — a drilled hole's cylindrical wall
//!      closes on a seam edge (same face both sides → cached `Boundary`, no
//!      dihedral). A seam is not a corner; it is dropped from the blend set
//!      instead of flagging its endpoints as Cliffs and aborting.
//!   2. RESILIENT pre-flight (fillet::fillet_edges) — an edge whose radius
//!      overruns its corner room is DROPPED from the selection, not aborted.
//!   3. COLLINEAR pass-through (blend_graph::compute_setbacks) — a straight
//!      edge a boolean split into two collinear segments is a pass-through
//!      (setback ≈ 0), no longer refused as "rank-deficient".
//!
//! This gate covers the common case: a block with one through-hole. The
//! L-bracket-with-bolt-holes variant (a union producing merged L-faces) gets
//! PAST all three refusals and then hits a deeper SEQUENTIAL-SURGERY layer
//! ("edge not found in any face": an earlier edge's blend consumes the faces a
//! later edge references) — tracked separately as the next fillet target.
//!
//! Run: `cargo test -p geometry-engine --test fillet_post_boolean -- --nocapture`.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::fillet::{fillet_edges, FilletOptions, FilletType};
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn box_at(m: &mut BRepModel, w: f64, h: f64, d: f64, tx: f64, ty: f64, tz: f64) -> SolidId {
    let s = match TopologyBuilder::new(m).create_box_3d(w, h, d).unwrap() {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };
    if tx != 0.0 {
        translate(m, vec![s], Vector3::X, tx, TransformOptions::default()).expect("tx");
    }
    if ty != 0.0 {
        translate(m, vec![s], Vector3::Y, ty, TransformOptions::default()).expect("ty");
    }
    if tz != 0.0 {
        translate(m, vec![s], Vector3::Z, tz, TransformOptions::default()).expect("tz");
    }
    s
}

fn cylinder(m: &mut BRepModel, base: Point3, axis: Vector3, radius: f64, height: f64) -> SolidId {
    match TopologyBuilder::new(m)
        .create_cylinder_3d(base, axis, radius, height)
        .unwrap()
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}

fn all_edges(model: &BRepModel, solid: SolidId) -> Vec<EdgeId> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let Some(s) = model.solids.get(solid) else {
        return out;
    };
    let mut shells = vec![s.outer_shell];
    shells.extend_from_slice(&s.inner_shells);
    for sh in shells {
        let Some(shell) = model.shells.get(sh) else {
            continue;
        };
        for &fid in &shell.faces {
            let Some(face) = model.faces.get(fid) else {
                continue;
            };
            for lid in face.all_loops() {
                let Some(lp) = model.loops.get(lid) else {
                    continue;
                };
                for &eid in &lp.edges {
                    if seen.insert(eid) {
                        out.push(eid);
                    }
                }
            }
        }
    }
    out
}

/// A plain block with ONE drilled through-hole — the most common shape a
/// boolean produces. The hole's cylindrical wall carries a seam edge that
/// pre-seam-filter `fillet all edges` refused on.
fn build_holed_block(m: &mut BRepModel) -> SolidId {
    let block = box_at(m, 40.0, 40.0, 10.0, 0.0, 0.0, 5.0); // x[-20,20] y[-20,20] z[0,10]
    let hole = cylinder(m, Point3::new(0.0, 0.0, -1.0), Vector3::Z, 6.0, 12.0); // r6 through Z
    boolean_operation(
        m,
        block,
        hole,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("through-hole difference")
}

#[test]
fn fillet_all_on_holed_block_clears_the_refusal_layers() {
    // PROGRESS PIN. The three all-or-nothing REFUSALS are fixed: `fillet all
    // edges` on a drilled-hole part no longer aborts at a seam Cliff, an
    // infeasible-radius edge, or a collinear pass-through — it now reaches the
    // blend SURGERY. A deeper layer remains: `create_fillet_chain` mutates a
    // face while building one edge's blend, so a later edge in the same pass
    // finds its (now-replaced) face missing ("edge not found in any face").
    // Fixing that is a geometry-pass rework (resolve all blends against the
    // ORIGINAL topology, or re-resolve per edge) — the next fillet target.
    //
    // This pin asserts the three refusals STAY fixed, and upgrades to a full
    // soundness gate the moment the surgery layer is fixed too.
    let mut m = BRepModel::new();
    let block = build_holed_block(&mut m);
    assert!(
        m.ground_truth(block)
            .expect("block gt")
            .certificate
            .is_sound(),
        "holed block must be sound BEFORE filleting"
    );

    let edges = all_edges(&m, block);
    let n = edges.len();

    match fillet_edges(
        &mut m,
        block,
        edges,
        FilletOptions {
            fillet_type: FilletType::Constant(2.0),
            radius: 2.0,
            ..Default::default()
        },
    ) {
        Ok(faces) => {
            // Surgery layer fixed too — full end-to-end win; demand soundness.
            let gt = m.ground_truth(block).expect("filleted gt");
            assert!(
                gt.certificate.is_sound(),
                "filleted holed block ({} faces, {} edges) must be SOUND — {}",
                faces.len(),
                n,
                gt.summary()
            );
        }
        Err(e) => {
            let msg = format!("{e:?}");
            assert!(
                !msg.contains("rank-deficient")
                    && !msg.contains("CLIFF")
                    && !msg.to_lowercase().contains("invalidradius"),
                "a FIXED fillet refusal layer regressed (must be the surgery layer only): {msg}"
            );
            eprintln!("[fillet pin] refusals cleared; remaining surgery-layer error: {msg}");
        }
    }
}
