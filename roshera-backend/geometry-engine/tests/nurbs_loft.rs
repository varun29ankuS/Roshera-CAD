//! NURBS-LOFT gate — `operations::nurbs_loft` must build a watertight, valid
//! solid whose lateral wall is a genuine NURBS surface, skinned (interpolated)
//! through the cross-section rings and G2 along the loft at the default degree 3.
//!
//! This is the kernel's first NURBS-surface-as-a-B-Rep-face path, so the gate
//! checks both the topology (valid + watertight at export density) AND that the
//! lateral really is a `NurbsSurface` (not silently degraded to a ruled/planar
//! approximation).

use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Point3, Tolerance};
use geometry_engine::operations::nurbs_loft::{nurbs_loft, NurbsLoftOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};

/// Sample a circle of `n` points (NOT closed — `nurbs_loft` closes the ring)
/// at height `z`, radius `r`, centred on the Z axis.
fn ring(n: usize, r: f64, z: f64) -> Vec<Point3> {
    (0..n)
        .map(|i| {
            let a = i as f64 * std::f64::consts::TAU / n as f64;
            Point3::new(r * a.cos(), r * a.sin(), z)
        })
        .collect()
}

fn assert_nurbs_solid_sound(m: &BRepModel, s: SolidId, label: &str) {
    let v = validate_solid_scoped(m, s, Tolerance::default(), ValidationLevel::Standard);
    assert!(v.is_valid, "{label}: B-Rep invalid: {:?}", v.errors);

    for defl in [0.5_f64, 0.1] {
        let rep = manifold_report(m, s, defl, 1e-6)
            .unwrap_or_else(|| panic!("{label}: manifold_report none @defl {defl}"));
        assert_eq!(
            (rep.boundary_edges, rep.nonmanifold_edges),
            (0, 0),
            "{label}: not watertight @defl {defl} (open={}, nm={})",
            rep.boundary_edges,
            rep.nonmanifold_edges
        );
    }

    // The lateral wall must be a real NURBS surface.
    let solid = m.solids.get(s).expect("solid");
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    let mut has_nurbs = false;
    for sh in shells {
        for &fid in &m.shells.get(sh).expect("shell").faces {
            let face = m.faces.get(fid).expect("face");
            if m.surfaces.get(face.surface_id).expect("surf").type_name() == "NurbsSurface" {
                has_nurbs = true;
            }
        }
    }
    assert!(
        has_nurbs,
        "{label}: no NURBS lateral face — skin degraded to a non-NURBS surface"
    );
}

/// A barrel/vase: circular rings whose radius bulges then necks → a genuinely
/// skinned (non-extruded, non-conical) NURBS lateral.
#[test]
fn nurbs_loft_barrel_is_watertight() {
    let mut m = BRepModel::new();
    let sections = vec![
        ring(20, 2.0, 0.0),
        ring(20, 3.0, 1.5),
        ring(20, 3.5, 3.0),
        ring(20, 3.0, 4.5),
        ring(20, 2.0, 6.0),
    ];
    let s = nurbs_loft(&mut m, sections, NurbsLoftOptions::default()).expect("nurbs_loft barrel");
    assert_nurbs_solid_sound(&m, s, "barrel");

    // Envelope: Ø7 at the bulge, 6 tall.
    let b = m.solid_world_bbox(s).expect("bbox");
    let sz = b.size();
    assert!(
        (sz.x - 7.0).abs() < 0.4 && (sz.y - 7.0).abs() < 0.4 && (sz.z - 6.0).abs() < 0.01,
        "barrel envelope wrong: {sz:?}"
    );
}

/// A straight circular tube (constant radius) — the skin must still close
/// watertight and stay NURBS (regression on the degenerate equal-section case).
#[test]
fn nurbs_loft_straight_tube_is_watertight() {
    let mut m = BRepModel::new();
    let sections = vec![
        ring(16, 4.0, 0.0),
        ring(16, 4.0, 2.0),
        ring(16, 4.0, 4.0),
        ring(16, 4.0, 6.0),
    ];
    let s = nurbs_loft(&mut m, sections, NurbsLoftOptions::default()).expect("nurbs_loft tube");
    assert_nurbs_solid_sound(&m, s, "tube");
}

/// A non-circular (super-ellipse-ish) freeform section lofted with a twist of
/// radius — exercises the skin on a genuinely freeform U profile.
#[test]
fn nurbs_loft_freeform_section_is_watertight() {
    let mut m = BRepModel::new();
    let blob = |scale: f64, z: f64| -> Vec<Point3> {
        (0..24)
            .map(|i| {
                let a = i as f64 * std::f64::consts::TAU / 24.0;
                // A lobed radius so the section is clearly non-circular.
                let r = scale * (2.0 + 0.4 * (3.0 * a).cos());
                Point3::new(r * a.cos(), r * a.sin(), z)
            })
            .collect()
    };
    let sections = vec![
        blob(1.0, 0.0),
        blob(1.3, 2.0),
        blob(1.1, 4.0),
        blob(0.9, 6.0),
    ];
    let s = nurbs_loft(&mut m, sections, NurbsLoftOptions::default()).expect("nurbs_loft freeform");
    assert_nurbs_solid_sound(&m, s, "freeform");
}
