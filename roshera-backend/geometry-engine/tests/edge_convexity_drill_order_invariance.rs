//! Classifier-lie regression (audit 2026-07-14, drill-order SURVIVOR): the
//! convex rims of a boolean-drilled bolt pattern must ALL classify convex,
//! regardless of the order in which the tool blanks were created and
//! differenced.
//!
//! ## The lie
//! `drill_pattern` (and any batch-drill flow) creates ALL bore cylinders FIRST,
//! then differences them sequentially. This makes `find_adjacent_faces` return
//! the two rim faces in a DIFFERENT order for the last-drilled bore — its
//! CYLINDER wall becomes `face1` instead of the box PLANE. The edge-convexity
//! classifier derived the tangent handedness from a single-face membership
//! probe on `face1`; on the cylinder wall that probe is AXIAL, so both sample
//! points sit ON the curved surface and the trimmed winding test cannot resolve
//! the rim boundary — it reports both interior probes as OUTSIDE. The membership
//! test then falls back to the stored loop-winding flag, which a boolean tool
//! face leaves inconsistent with its (correct) flipped outward normal, so the
//! WHOLE last bore's rims flip to a false CONCAVE while its geometrically
//! identical siblings read CONVEX. A co-drilled bore with the opposite
//! convexity of its siblings is a self-certifying lie that poisons
//! `select_edge`'s convexity filter.
//!
//! ## The invariant
//! All co-circular / symmetric rims of a drilled pattern classify IDENTICALLY,
//! and a through-bore rim (plane∩cylinder, a 90° material wedge) is CONVEX.
//! Fixed by deriving the sign from whichever face's membership test resolves
//! (try `face1`; if indeterminate, try `face2` and negate — order-independent).

#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::edge_classification::classify_edge;
use geometry_engine::operations::revolve::{revolve_profile, RevolveOptions};
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::primitives::curve::{Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn solid_edges(model: &BRepModel, solid: SolidId) -> Vec<EdgeId> {
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

fn assert_sound(m: &mut BRepModel, s: SolidId, what: &str) {
    let cert = m.certify_solid(s);
    assert!(
        cert.brep_valid && cert.watertight && cert.manifold,
        "{what} must be sound; brep_valid={} watertight={} manifold={} errors={:?}",
        cert.brep_valid,
        cert.watertight,
        cert.manifold,
        cert.errors,
    );
}

/// Convexity of every CIRCLE edge whose midpoint sits at radial distance
/// `~rim_radius` from one of `centres` — i.e. the bore/bolt rim arcs — grouped
/// by which pattern hole they belong to. Classified via `classify_edge`, the
/// same read-only path `select_edge`'s convexity filter uses.
fn rim_convexities(
    m: &BRepModel,
    s: SolidId,
    centres: &[(f64, f64)],
    rim_radius: f64,
) -> Vec<(usize, EdgeId, i8, f64)> {
    let mut out = Vec::new();
    for e in solid_edges(m, s) {
        let Some(edge) = m.edges.get(e) else { continue };
        let is_circle = m
            .curves
            .get(edge.curve_id)
            .map(|c| c.type_name() == "Circle")
            .unwrap_or(false);
        if !is_circle {
            continue;
        }
        let Ok(mid) = edge.evaluate(0.5, &m.curves) else {
            continue;
        };
        let (hole, dist) = centres
            .iter()
            .enumerate()
            .map(|(i, (cx, cy))| (i, (mid.x - cx).hypot(mid.y - cy)))
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap();
        // Only rim arcs (distance from the hole axis ≈ the bore radius); skips
        // the hub bore, flange OD, and pocket-edge circles.
        if (dist - rim_radius).abs() > 0.4 {
            continue;
        }
        let c = classify_edge(m, e).expect("classify rim arc");
        out.push((hole, e, c.convexity, c.dihedral_angle.unwrap_or(f64::NAN)));
    }
    out
}

fn assert_all_convex_identical(rims: &[(usize, EdgeId, i8, f64)], holes: usize, what: &str) {
    assert!(
        rims.len() >= holes * 2,
        "{what}: expected ≥ {} rim arcs (2 rims/hole), found {}",
        holes * 2,
        rims.len()
    );
    // Every rim arc must be convex (+1) with a positive dihedral — and thus all
    // identical. A single sibling reading concave is the lie.
    for &(hole, e, conv, dih) in rims {
        assert_eq!(
            conv, 1,
            "{what}: rim arc {e} of hole {hole} classified {conv} (dih {dih:.4}); \
             a through-bore rim is CONVEX, and a co-drilled sibling with opposite \
             convexity is a self-certifying lie",
        );
        assert!(
            dih > 0.0,
            "{what}: rim arc {e} of hole {hole} dihedral must be positive; got {dih:.4}",
        );
    }
    // Explicit uniformity check across holes (redundant with the per-arc assert,
    // but names the property under test).
    let signs: std::collections::HashSet<i8> = rims.iter().map(|r| r.2).collect();
    assert_eq!(
        signs.len(),
        1,
        "{what}: co-circular rims of a symmetric pattern must classify identically; \
         saw mixed signs {signs:?}",
    );
}

/// Part (a): 60×40×30 box (base-centre [0,0,0]) − a 40×20×20 pocket
/// (base-centre [0,0,20]) − 4 through-bores r2.5 on ring_r10 at 45/135/225/315°.
/// DRILL-ORDER: all 4 bore cylinders created up front, THEN differenced — the
/// exact `drill_pattern` sequence that surfaces the last-bore face-order flip.
#[test]
fn drill_order_pocketed_box_bore_rims_all_convex() {
    let mut m = BRepModel::new();
    let base = match TopologyBuilder::new(&mut m)
        .create_box_3d(60.0, 40.0, 30.0)
        .expect("box")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid, got {o:?}"),
    };
    translate(
        &mut m,
        vec![base],
        Vector3::Z,
        15.0,
        TransformOptions::default(),
    )
    .expect("seat box");
    let pocket = match TopologyBuilder::new(&mut m)
        .create_box_3d(40.0, 20.0, 20.0)
        .expect("pocket")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid, got {o:?}"),
    };
    translate(
        &mut m,
        vec![pocket],
        Vector3::Z,
        30.0,
        TransformOptions::default(),
    )
    .expect("raise");
    let mut part = boolean_operation(
        &mut m,
        base,
        pocket,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("cut pocket");
    assert_sound(&mut m, part, "pocketed box");

    let ring_r = 10.0;
    let d = ring_r * std::f64::consts::FRAC_1_SQRT_2;
    let centres = [(d, d), (-d, d), (-d, -d), (d, -d)];
    let mut bores = Vec::new();
    for (cx, cy) in centres {
        let bore = match TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(cx, cy, -1.0), Vector3::Z, 2.5, 32.0)
            .expect("bore")
        {
            GeometryId::Solid(s) => s,
            o => panic!("expected Solid, got {o:?}"),
        };
        bores.push(bore);
    }
    for bore in bores {
        part = boolean_operation(
            &mut m,
            part,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("drill bore");
    }
    assert_sound(&mut m, part, "drilled pocketed box");

    let rims = rim_convexities(&m, part, &centres, 2.5);
    assert_all_convex_identical(&rims, 4, "pocketed-box 4-bore pattern");
}

/// Part (b): a revolved hub-flange − 6 bolt holes r2 on ring_r21 through the
/// 6 mm flange. Same DRILL-ORDER (all cylinders first, then differenced). The
/// revolve seam makes the bolt-rim splits asymmetric, so the last-drilled hole
/// takes the cylinder-first face order that triggered the flip.
#[test]
fn drill_order_revolve_flange_bolt_rims_all_convex() {
    let mut m = BRepModel::new();
    let profile = [
        (6.0, 0.0),
        (30.0, 0.0),
        (30.0, 6.0),
        (12.0, 6.0),
        (12.0, 20.0),
        (6.0, 20.0),
    ];
    let verts: Vec<_> = profile
        .iter()
        .map(|(r, z)| m.vertices.add(*r, 0.0, *z))
        .collect();
    let mut edges = Vec::new();
    for i in 0..profile.len() {
        let j = (i + 1) % profile.len();
        let cid = m.curves.add(Box::new(Line::new(
            Point3::new(profile[i].0, 0.0, profile[i].1),
            Point3::new(profile[j].0, 0.0, profile[j].1),
        )));
        edges.push(m.edges.add(Edge::new(
            0,
            verts[i],
            verts[j],
            cid,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )));
    }
    let mut part = revolve_profile(
        &mut m,
        edges,
        RevolveOptions {
            axis_origin: Point3::ZERO,
            axis_direction: Vector3::Z,
            angle: std::f64::consts::TAU,
            segments: 96,
            ..Default::default()
        },
    )
    .expect("revolve hub-flange");
    assert_sound(&mut m, part, "hub-flange");

    let ring_r = 21.0;
    let mut centres = Vec::new();
    let mut bores = Vec::new();
    for k in 0..6 {
        let th = std::f64::consts::TAU * (k as f64) / 6.0;
        let (cx, cy) = (ring_r * th.cos(), ring_r * th.sin());
        centres.push((cx, cy));
        let bore = match TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(cx, cy, -2.0), Vector3::Z, 2.0, 10.0)
            .expect("bolt bore")
        {
            GeometryId::Solid(s) => s,
            o => panic!("expected Solid, got {o:?}"),
        };
        bores.push(bore);
    }
    for bore in bores {
        part = boolean_operation(
            &mut m,
            part,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("drill bolt hole");
    }
    assert_sound(&mut m, part, "bolted flange");

    let rims = rim_convexities(&m, part, &centres, 2.0);
    assert_all_convex_identical(&rims, 6, "revolve-flange 6-bolt pattern");
}
