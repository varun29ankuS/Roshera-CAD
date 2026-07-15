// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Blend (fillet / chamfer) winding & orientation invariants across dihedrals.
//!
//! Two harness-found bugs (FILLET-MULTIEDGE-VOLUME #51, CHAMFER-MULTIEDGE-VOLUME
//! #52) shared a root cause class: a blend face whose tessellated mesh normal
//! pointed *inward* for some edge geometries, so the mesh-based mass-properties
//! (the kernel's only volume source — `compute_solid_mass_properties` always
//! routes through the mesh) silently mis-reported the removed volume.
//!
//! * **Fillet.** `tessellate_fillet_face` wound the grid purely off
//!   `face.orientation`, ignoring the parametric chart handedness. A
//!   `CylindricalFillet` sign-flips its frame to keep the blend arc minor, so
//!   `du×dv` points opposite the outward normal for ~half the edges of a box —
//!   those tessellated inward, cancelling their divergence-volume contribution
//!   (all-12-edge box fillet read 16 instead of 55).
//! * **Chamfer.** The bevel `RuledSurface` face was stamped a hardcoded
//!   `FaceOrientation::Forward`. Its intrinsic normal points inward for non-π/2
//!   dihedrals (e.g. a 108° pentagon edge), so the bevel tessellated inverted and
//!   the mesh over-reported removed volume ~22×.
//!
//! These tests pin the fix with two independent oracles per case:
//!   1. **Manifold orientation** — the result must be a closed, *oriented*,
//!      genus-0 2-manifold (`manifold_report().is_valid_solid()` + `χ = 2`). A
//!      single inward-wound face flips a directed edge and fails `oriented`.
//!   2. **Analytic removed volume** — the mesh ΔV must match the closed-form
//!      cross-section a constant-radius fillet / equal-distance chamfer removes
//!      from a straight dihedral edge, to a fraction of that removed amount.
//!
//! The single-edge path is the load-bearing one for the winding/orientation
//! reconciliation; multi-edge corner synthesis (the spherical-octant corner of
//! a fillet, the triangular corner cap of a chamfer) is tracked separately under
//! #51 / #52 and is not asserted here.

use geometry_engine::harness::watertight::{manifold_report, mesh_volume};
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::chamfer::{chamfer_edges, ChamferOptions, ChamferType};
use geometry_engine::operations::extrude::{extrude_profile, ExtrudeOptions};
use geometry_engine::operations::fillet::{fillet_edges, FilletOptions, FilletType};
use geometry_engine::primitives::curve::Line;
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, TopologyBuilder};
use std::f64::consts::PI;

const CHORD: f64 = 0.01;

/// Extrude a closed CCW polygon `ring` along +Z by `height`, returning the solid.
fn extrude_regular(model: &mut BRepModel, ring: &[(f64, f64)], height: f64) -> SolidId {
    let verts: Vec<_> = ring
        .iter()
        .map(|&(x, y)| model.vertices.add(x, y, 0.0))
        .collect();
    let n = verts.len();
    let mut edges = Vec::with_capacity(n);
    for i in 0..n {
        let a = verts[i];
        let b = verts[(i + 1) % n];
        let pa = model.vertices.get(a).expect("va").position;
        let pb = model.vertices.get(b).expect("vb").position;
        let line = Line::new(
            Point3::new(pa[0], pa[1], pa[2]),
            Point3::new(pb[0], pb[1], pb[2]),
        );
        let cid = model.curves.add(Box::new(line));
        edges.push(
            model
                .edges
                .add(Edge::new_auto_range(0, a, b, cid, EdgeOrientation::Forward)),
        );
    }
    extrude_profile(
        model,
        edges,
        ExtrudeOptions {
            direction: Vector3::Z,
            distance: height,
            cap_ends: true,
            ..Default::default()
        },
    )
    .expect("extrusion")
}

/// Regular `n`-gon prism, circumradius `r`, height `h`, centred on the z-axis.
/// Interior (material) dihedral at every vertical edge is `(n−2)·π/n`.
fn regular_prism(model: &mut BRepModel, n: usize, circumradius: f64, height: f64) -> SolidId {
    let ring: Vec<(f64, f64)> = (0..n)
        .map(|i| {
            let t = 2.0 * PI * (i as f64) / (n as f64);
            (circumradius * t.cos(), circumradius * t.sin())
        })
        .collect();
    extrude_regular(model, &ring, height)
}

/// First vertical edge (endpoints differ in z) of `solid`.
fn first_vertical_edge(model: &BRepModel) -> EdgeId {
    model
        .edges
        .iter()
        .find(|(_, e)| {
            let za = model.vertices.get(e.start_vertex).map(|v| v.position[2]);
            let zb = model.vertices.get(e.end_vertex).map(|v| v.position[2]);
            matches!((za, zb), (Some(a), Some(b)) if (a - b).abs() > 1.0)
        })
        .map(|(id, _)| id)
        .expect("vertical edge")
}

/// Closed-form removed cross-section of a constant-radius `r` fillet at a convex
/// straight edge with interior dihedral `theta`, times the edge length `len`:
/// kite `r²/tan(θ/2)` minus the circular sector `½r²(π−θ)`.
fn fillet_removed(theta: f64, r: f64, len: f64) -> f64 {
    (r * r / (theta * 0.5).tan() - 0.5 * r * r * (PI - theta)) * len
}

/// Closed-form removed cross-section of an equal-distance `d` chamfer at a convex
/// straight edge with interior dihedral `theta`, times the edge length: the
/// triangle `½d²·sin(θ)`.
fn chamfer_removed(theta: f64, d: f64, len: f64) -> f64 {
    0.5 * d * d * theta.sin() * len
}

fn analytic_volume(model: &mut BRepModel, solid: SolidId) -> f64 {
    model.calculate_solid_volume(solid).expect("volume")
}

/// Assert the solid is a closed, oriented, genus-0 2-manifold. A single
/// inward-wound blend face flips a directed edge and fails `oriented`.
fn assert_valid_oriented(model: &BRepModel, solid: SolidId, label: &str) {
    let r = manifold_report(model, solid, 0.05, 1e-6).unwrap_or_else(|| panic!("{label}: no mesh"));
    assert!(
        r.is_valid_solid(),
        "{label}: not a closed/oriented manifold: {r:?}"
    );
    assert_eq!(r.euler_characteristic, 2, "{label}: χ≠2: {r:?}");
    assert_eq!(r.components, 1, "{label}: components≠1: {r:?}");
}

// ── Fillet winding across the 12 box edges ──────────────────────────────────

/// Every one of a box's 12 edges, filleted individually, must produce a valid
/// oriented manifold whose removed volume matches the 90° round-over
/// `(1−π/4)r²L`. Before the `tessellate_fillet_face` chart-sign reconciliation,
/// 6 of the 12 (the left-handed-chart edges) tessellated inward — `oriented`
/// false and the divergence volume wrong-signed.
#[test]
fn every_box_edge_single_fillet_is_oriented_and_correct_volume() {
    let (side, r) = (4.0_f64, 0.6_f64);
    let removed = fillet_removed(PI / 2.0, r, side); // 90° edge, length = side
    for ei in 0..12usize {
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_box_3d(side, side, side)
            .expect("box");
        let solid = model.solids.iter().last().map(|(id, _)| id).expect("s");
        let v0 = analytic_volume(&mut model, solid);
        let edge = model.edges.iter().map(|(id, _)| id).nth(ei).expect("edge");
        fillet_edges(
            &mut model,
            solid,
            vec![edge],
            FilletOptions {
                fillet_type: FilletType::Constant(r),
                radius: r,
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("edge {ei}: fillet failed: {e:?}"));
        assert_valid_oriented(&model, solid, &format!("box edge {ei} fillet"));
        let dv = v0 - analytic_volume(&mut model, solid);
        assert!(
            (dv - removed).abs() < 0.15 * removed,
            "box edge {ei}: removed {dv:.5} vs analytic {removed:.5}"
        );
    }
}

// ── Fillet & chamfer removed volume across dihedral angles ──────────────────

/// Single-edge fillet on regular prisms (triangle 60°, square 90°, pentagon
/// 108°, hexagon 120°) must remove the closed-form round-over volume and stay an
/// oriented manifold. Pins the fillet winding reconciliation across the full
/// dihedral range (the chart handedness flips with the dihedral).
#[test]
fn single_edge_fillet_removed_volume_across_dihedrals() {
    let (circumradius, height, r) = (2.0_f64, 4.0_f64, 0.2_f64);
    for &n in &[3usize, 4, 5, 6] {
        let theta = (n as f64 - 2.0) * PI / (n as f64);
        let mut model = BRepModel::new();
        let solid = regular_prism(&mut model, n, circumradius, height);
        let v0 = analytic_volume(&mut model, solid);
        let edge = first_vertical_edge(&model);
        fillet_edges(
            &mut model,
            solid,
            vec![edge],
            FilletOptions {
                fillet_type: FilletType::Constant(r),
                radius: r,
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{n}-gon fillet failed: {e:?}"));
        assert_valid_oriented(&model, solid, &format!("{n}-gon fillet"));
        let dv = v0 - analytic_volume(&mut model, solid);
        let expected = fillet_removed(theta, r, height);
        assert!(
            (dv - expected).abs() < 0.15 * expected,
            "{n}-gon (θ={:.1}°) fillet removed {dv:.5} vs analytic {expected:.5}",
            theta.to_degrees()
        );
    }
}

/// Single-edge equal-distance chamfer on the same prisms must remove the
/// closed-form bevel volume and stay an oriented manifold. Pins the chamfer
/// bevel orientation fix (`orient_face_for_outward(n1+n2)`): with the old
/// hardcoded `Forward`, the 108° / 120° bevels tessellated inward and the mesh
/// over-reported removed volume by more than an order of magnitude.
#[test]
fn single_edge_chamfer_removed_volume_across_dihedrals() {
    let (circumradius, height, d) = (2.0_f64, 4.0_f64, 0.2_f64);
    for &n in &[3usize, 4, 5, 6] {
        let theta = (n as f64 - 2.0) * PI / (n as f64);
        let mut model = BRepModel::new();
        let solid = regular_prism(&mut model, n, circumradius, height);
        let v0 = analytic_volume(&mut model, solid);
        let edge = first_vertical_edge(&model);
        chamfer_edges(
            &mut model,
            solid,
            vec![edge],
            ChamferOptions {
                chamfer_type: ChamferType::EqualDistance(d),
                distance1: d,
                distance2: d,
                symmetric: true,
                preserve_edges: false,
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{n}-gon chamfer failed: {e:?}"));
        assert_valid_oriented(&model, solid, &format!("{n}-gon chamfer"));
        let dv = v0 - analytic_volume(&mut model, solid);
        let expected = chamfer_removed(theta, d, height);
        assert!(
            (dv - expected).abs() < 0.15 * expected,
            "{n}-gon (θ={:.1}°) chamfer removed {dv:.5} vs analytic {expected:.5}",
            theta.to_degrees()
        );
    }
}

/// Cross-check the two mesh-volume oracles agree: the divergence-theorem
/// `mesh_volume` and the Tonon `calculate_solid_volume` must match on a filleted
/// prism (both must see the same correctly-oriented mesh).
#[test]
fn mesh_volume_oracles_agree_on_filleted_prism() {
    let mut model = BRepModel::new();
    let solid = regular_prism(&mut model, 5, 2.0, 4.0);
    let edge = first_vertical_edge(&model);
    fillet_edges(
        &mut model,
        solid,
        vec![edge],
        FilletOptions {
            fillet_type: FilletType::Constant(0.2),
            radius: 0.2,
            ..Default::default()
        },
    )
    .expect("fillet");
    let tonon = analytic_volume(&mut model, solid);
    let divergence = mesh_volume(&model, solid, CHORD).expect("mesh vol");
    assert!(
        (tonon - divergence).abs() < 1e-2 * tonon,
        "Tonon {tonon:.5} vs divergence {divergence:.5} disagree — a face is mis-wound"
    );
}
