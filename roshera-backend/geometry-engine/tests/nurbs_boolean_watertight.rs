//! #17 WATERTIGHT GATE — the brutal corefinement suite. Every boolean involving
//! a NURBS-lateral (lofted) solid must produce a result that is SOUND (valid
//! B-Rep + watertight + manifold + self-intersection-free) AND mesh-leak-free
//! (zero boundary edges at a FINE chord). These are deliberately hard: clean
//! pockets, the periodic SEAM, through-slots, cap crossings, full-height slots,
//! fine features, unions, chained cuts, and off-seam corners.
//!
//! They are RED until the corefinement fix lands (cut edges shared between
//! operands). This file is the definition of "done" for #17 watertight — make
//! them all GREEN without regressing the analytic-boolean core (poke_matrix).
//!
//! Run: `cargo test -p geometry-engine --test nurbs_boolean_watertight`.

use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::nurbs_loft::{nurbs_loft, NurbsLoftOptions};
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// A lofted barrel, closed in u. Seam vertex sits at +X (angle 0): (r, 0, z).
/// So the +X wall is the PERIODIC SEAM (a deliberately hard cut location), and
/// the +Y wall is seam-free.
fn barrel(m: &mut BRepModel) -> SolidId {
    let ring = |r: f64, z: f64| {
        (0..24)
            .map(|i| {
                let a = i as f64 * std::f64::consts::TAU / 24.0;
                Point3::new(r * a.cos(), r * a.sin(), z)
            })
            .collect::<Vec<_>>()
    };
    let sections = vec![
        ring(3.0, 0.0),
        ring(4.0, 2.0),
        ring(4.0, 4.0),
        ring(3.0, 6.0),
    ];
    nurbs_loft(m, sections, NurbsLoftOptions::default()).expect("barrel")
}

/// A box of (w,h,d) centred at origin, translated to (tx,ty,tz).
fn cutter(m: &mut BRepModel, w: f64, h: f64, d: f64, tx: f64, ty: f64, tz: f64) -> SolidId {
    let s = match TopologyBuilder::new(m).create_box_3d(w, h, d).unwrap() {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };
    translate(m, vec![s], Vector3::X, tx, TransformOptions::default()).expect("tx");
    translate(m, vec![s], Vector3::Y, ty, TransformOptions::default()).expect("ty");
    translate(m, vec![s], Vector3::Z, tz, TransformOptions::default()).expect("tz");
    s
}

/// THE GATE: the result must be fully sound AND leak-free at a fine chord.
fn assert_watertight(m: &mut BRepModel, s: SolidId, label: &str) {
    let gt = m
        .ground_truth(s)
        .unwrap_or_else(|| panic!("{label}: no ground truth"));
    let cert = &gt.certificate;
    let mr = manifold_report(m, s, 0.05, 1.0e-5)
        .unwrap_or_else(|| panic!("{label}: no manifold report"));
    assert!(
        cert.is_sound(),
        "{label}: NOT SOUND — {} (boundary_edges={}, non_manifold={})",
        gt.summary(),
        mr.boundary_edges,
        mr.nonmanifold_edges
    );
    assert_eq!(
        mr.boundary_edges, 0,
        "{label}: {} mesh boundary edges = LEAK (not watertight at fine chord)",
        mr.boundary_edges
    );
}

fn diff(m: &mut BRepModel, a: SolidId, b: SolidId) -> SolidId {
    boolean_operation(m, a, b, BooleanOp::Difference, BooleanOptions::default())
        .expect("difference must complete")
}

// ───────────────────── the brutal cases ─────────────────────

/// Baseline: a clean blind pocket into the seam-FREE +Y wall, mid-height.
#[test]
fn w01_clean_blind_pocket() {
    let mut m = BRepModel::new();
    let b = barrel(&mut m);
    let c = cutter(&mut m, 2.0, 2.0, 2.0, 0.0, 4.0, 3.0);
    let r = diff(&mut m, b, c);
    assert_watertight(&mut m, r, "w01 clean blind pocket (+Y wall)");
}

/// The PERIODIC SEAM: a pocket straddling the barrel's u=0 seam (+X wall).
#[test]
fn w02_seam_pocket() {
    let mut m = BRepModel::new();
    let b = barrel(&mut m);
    let c = cutter(&mut m, 2.0, 2.0, 2.0, 4.0, 0.0, 3.0);
    let r = diff(&mut m, b, c);
    assert_watertight(&mut m, r, "w02 seam pocket (+X seam wall)");
}

/// A through-slot: a long box crossing BOTH walls (two openings), mid-height.
#[test]
fn w03_through_slot() {
    let mut m = BRepModel::new();
    let b = barrel(&mut m);
    let c = cutter(&mut m, 2.0, 12.0, 2.0, 0.0, 0.0, 3.0);
    let r = diff(&mut m, b, c);
    assert_watertight(&mut m, r, "w03 through slot (both walls)");
}

/// A notch crossing the TOP CAP edge (cut interacts with the planar cap).
#[test]
fn w04_cap_edge_notch() {
    let mut m = BRepModel::new();
    let b = barrel(&mut m);
    let c = cutter(&mut m, 2.0, 2.0, 2.0, 0.0, 3.6, 6.0);
    let r = diff(&mut m, b, c);
    assert_watertight(&mut m, r, "w04 cap-edge notch (top)");
}

/// A FULL-HEIGHT slot: the box spans the whole barrel height + past both caps.
#[test]
fn w05_full_height_slot() {
    let mut m = BRepModel::new();
    let b = barrel(&mut m);
    let c = cutter(&mut m, 2.0, 2.0, 10.0, 0.0, 4.0, 3.0);
    let r = diff(&mut m, b, c);
    assert_watertight(&mut m, r, "w05 full-height slot");
}

/// A FINE feature: a small pocket (sub-curvature-scale).
#[test]
fn w06_fine_pocket() {
    let mut m = BRepModel::new();
    let b = barrel(&mut m);
    let c = cutter(&mut m, 0.8, 0.8, 0.8, 0.0, 4.0, 3.0);
    let r = diff(&mut m, b, c);
    assert_watertight(&mut m, r, "w06 fine pocket");
}

/// An off-seam CORNER pocket on the diagonal wall (between seam and +Y).
#[test]
fn w07_diagonal_corner_pocket() {
    let mut m = BRepModel::new();
    let b = barrel(&mut m);
    let c = cutter(&mut m, 2.0, 2.0, 2.0, 2.8, 2.8, 3.0);
    let r = diff(&mut m, b, c);
    assert_watertight(&mut m, r, "w07 diagonal corner pocket");
}

/// A deep pocket reaching toward the axis.
#[test]
fn w08_deep_pocket() {
    let mut m = BRepModel::new();
    let b = barrel(&mut m);
    let c = cutter(&mut m, 2.0, 6.0, 2.0, 0.0, 3.0, 3.0);
    let r = diff(&mut m, b, c);
    assert_watertight(&mut m, r, "w08 deep pocket");
}

/// UNION of the barrel with an overlapping box (watertight union).
#[test]
fn w09_union_box() {
    let mut m = BRepModel::new();
    let b = barrel(&mut m);
    let c = cutter(&mut m, 2.0, 2.0, 2.0, 0.0, 4.0, 3.0);
    let r = boolean_operation(&mut m, b, c, BooleanOp::Union, BooleanOptions::default())
        .expect("union must complete");
    assert_watertight(&mut m, r, "w09 union box");
}

/// CHAINED cuts: two sequential pockets into opposite walls.
#[test]
fn w10_two_chained_pockets() {
    let mut m = BRepModel::new();
    let b = barrel(&mut m);
    let c1 = cutter(&mut m, 2.0, 2.0, 2.0, 0.0, 4.0, 3.0);
    let r1 = diff(&mut m, b, c1);
    let c2 = cutter(&mut m, 2.0, 2.0, 2.0, 0.0, -4.0, 3.0);
    let r2 = diff(&mut m, r1, c2);
    assert_watertight(&mut m, r2, "w10 two chained pockets");
}
