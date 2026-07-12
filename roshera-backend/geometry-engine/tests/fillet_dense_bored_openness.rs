//! Live-dogfood regression (confirmed live, kernel-repro): graceful ALL-edges
//! `fillet_edges` on a DENSE pocketed + 4-bore part must not 500 at an
//! aggressive radius while succeeding at a small one. The live symptom:
//!   r=1.5 / r=0.8 → 500 "filleted solid N is combinatorially valid but
//!     geometrically OPEN: 404 boundary mesh edge(s) at coarse-chord
//!     tessellation (mesh χ = -3) …"
//!   r=0.4 → SOUND success.
//! The 404 count is radius-independent above a threshold — the fingerprint of
//! a fixed coarse-mesh artefact, not a radius-scaled real gap.
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::fillet::{fillet_edges, FilletOptions, FilletType};
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// A 60×40×30 box with a 40×20×20 top pocket and FOUR through-bores r=2.5 on a
/// ring of radius 10 (the dense dogfood part). The pocket floor sits at z=5;
/// each bore pierces the whole block.
fn dense_pocketed_bored_part(m: &mut BRepModel) -> SolidId {
    let base = match TopologyBuilder::new(m)
        .create_box_3d(60.0, 40.0, 30.0)
        .expect("base box")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid for base box, got {o:?}"),
    };
    let pocket = match TopologyBuilder::new(m)
        .create_box_3d(40.0, 20.0, 20.0)
        .expect("pocket tool")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid for pocket tool, got {o:?}"),
    };
    translate(
        m,
        vec![pocket],
        Vector3::Z,
        15.0,
        TransformOptions::default(),
    )
    .expect("raise pocket");
    let mut part = boolean_operation(
        m,
        base,
        pocket,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("cut pocket");

    // Four bores r=2.5, axis +Z, on a ring of radius 10 placed on the DIAGONALS
    // (±45°) so each bore clears the pocket walls (pocket y∈[-10,10],
    // x∈[-20,20]; a bore on an axis at r=10 would sit exactly on a wall and
    // produce a non-manifold tangency). z ∈ [-25, 20] pierces the whole block.
    let ring_r = 10.0;
    let d = ring_r * std::f64::consts::FRAC_1_SQRT_2; // 7.07
    let centres = [(d, d), (-d, d), (-d, -d), (d, -d)];
    for (cx, cy) in centres {
        let bore = match TopologyBuilder::new(m)
            .create_cylinder_3d(Point3::new(cx, cy, -25.0), Vector3::Z, 2.5, 45.0)
            .expect("bore")
        {
            GeometryId::Solid(s) => s,
            o => panic!("expected Solid for bore, got {o:?}"),
        };
        part = boolean_operation(
            m,
            part,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("drill bore");
    }
    part
}

/// All non-loop edges of the solid (outer + inner shells, deduplicated).
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
                if let Some(lp) = model.loops.get(lid) {
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

fn fillet_opts(r: f64) -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(r),
        radius: r,
        graceful_corner_skip: true,
        ..Default::default()
    }
}

/// THE fix target: graceful all-edges fillet at an aggressive r=1.5 on the dense
/// bored part must SUCCEED with a SOUND result — not 500 on a coarse-mesh
/// openness artefact.
#[test]
fn fillet_all_graceful_r1_5_dense_bored_is_sound() {
    let mut m = BRepModel::new();
    let s = dense_pocketed_bored_part(&mut m);

    let pre = m.certify_solid(s);
    assert!(
        pre.brep_valid && pre.watertight && pre.manifold,
        "dense bored part unsound pre-fillet: {:?}",
        pre.errors,
    );

    let edges = all_edges(&m, s);
    let faces = fillet_edges(&mut m, s, edges, fillet_opts(1.5))
        .expect("graceful all-edges fillet at r=1.5 on the dense bored part must SUCCEED");
    assert!(!faces.is_empty(), "graceful fillet must round some edges");

    let cert = m.certify_solid(s);
    assert!(
        cert.is_sound(),
        "graceful r=1.5 fillet must leave a SOUND solid; watertight={} manifold={} \
         oriented={} brep_valid={} self_int_free={} boundary_edges={} errors={:?}",
        cert.watertight,
        cert.manifold,
        cert.oriented,
        cert.brep_valid,
        cert.self_intersection_free,
        cert.boundary_edges,
        cert.errors,
    );
}

/// The validator MUST NOT be blinded. The bore-rim overrun pre-filter is gated
/// to the graceful ALL-edges path; the EXPLICIT path (`graceful_corner_skip ==
/// false`) is untouched and must still HONEST-REFUSE this same crowded selection
/// rather than silently ship an open solid, rolling back to the intact, sound
/// part. (Explicitly requesting the full edge set on a part whose bore rims
/// overrun cannot yield a sound result at r=1.5 — the graceful "round what it
/// can" contract is the ONLY way to succeed here, and only by dropping the
/// offending rims.)
#[test]
fn explicit_fillet_all_r1_5_still_refused_and_rolls_back() {
    let mut m = BRepModel::new();
    let s = dense_pocketed_bored_part(&mut m);

    let edges = all_edges(&m, s);
    let opts = FilletOptions {
        fillet_type: FilletType::Constant(1.5),
        radius: 1.5,
        graceful_corner_skip: false,
        ..Default::default()
    };
    let res = fillet_edges(&mut m, s, edges, opts);
    assert!(
        res.is_err(),
        "explicit fillet-all on the crowded bored part must be REFUSED, not silently \
         accepted as an open solid"
    );

    // Transactional: the refusal rolled back to the intact, sound part.
    let cert = m.certify_solid(s);
    assert!(
        cert.is_sound(),
        "explicit refusal must roll back to the intact sound part; errors={:?}",
        cert.errors,
    );
}
