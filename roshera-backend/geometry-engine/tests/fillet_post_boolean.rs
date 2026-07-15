// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! POST-BOOLEAN FILLET — `fillet all edges` on a part that has been through a
//! boolean. Pinned from a live MCP dogfood: a drilled-hole part refused
//! `fillet_edges` (all edges) though the part was SOUND. It was a SIX-layer
//! failure, each fix exposing the next; all are now fixed (the kernel never
//! shipped broken geometry — it over-refused or hit an un-handled topology):
//!
//!   1. SEAM edges not filletable (blend_graph::build) — dropped, not a Cliff.
//!   2. RESILIENT pre-flight (fillet::fillet_edges) — drop infeasible edges,
//!      blend the rest, instead of aborting the whole op.
//!   3. COLLINEAR pass-through (blend_graph::compute_setbacks) — setback ≈ 0.
//!   4. Faces resolved UP FRONT into a map so one chain's blend can't strand a
//!      later chain's edge.
//!   5. Orphaned consumed-operand edges (propagation) dropped as not-on-solid.
//!   6. ★ OVER-SPLIT RIM: a boolean splits a hole's rim CIRCLE into co-curve
//!      arcs joined at 2-valent smooth vertices; the per-edge blend surgery
//!      can't coordinate that same-face arc chain. `fillet_edges` now COALESCES
//!      the arcs back into the single canonical (closed) rim edge first
//!      (`coalesce_smooth_cocurve_chains`), and the proven closed-rim blend
//!      handles it.
//!
//! This gate: a block with one through-hole, fillet all edges → must be SOUND.

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
/// boolean produces, and the one whose over-split rim defeated `fillet all`.
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
fn fillet_all_on_holed_block_is_sound() {
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
    let faces = fillet_edges(
        &mut m,
        block,
        edges,
        FilletOptions {
            fillet_type: FilletType::Constant(2.0),
            radius: 2.0,
            ..Default::default()
        },
    )
    .expect("fillet all edges must complete (rim coalesced, the rest blended)");

    let gt = m.ground_truth(block).expect("filleted gt");
    eprintln!(
        "[fillet-all] {} faces | sound={} | {}",
        faces.len(),
        gt.certificate.is_sound(),
        gt.summary()
    );
    assert!(
        gt.certificate.is_sound(),
        "filleted holed block must be SOUND — {}",
        gt.summary()
    );
}
