//! Bore-rim (inner-hole) fillet + chamfer regression (#26).
//!
//! `cylinder_rim_fillet` / `create_closed_edge_chamfer` originally only
//! handled the OUTER rim of a cap: they searched the cap face's *outer*
//! loop for the rim edge and assumed the cap circle shrinks to R−r. A
//! tube / washer / flange has an ANNULAR cap whose bore rim lives in an
//! *inner* loop, where the hole instead grows to R+r and the blend sits
//! on the torus inner equator (fillet) / a cone that opens the other way
//! (chamfer). The bore case therefore failed with
//! `InvalidGeometry("Rim edge not found in cap loop")`.
//!
//! These tests pin the fix: filleting and chamfering the bore rim of a
//! revolved tube succeeds, adds exactly one analytic blend face of the
//! right kind (Torus / Cone), and leaves the solid B-Rep-valid AND mesh-
//! watertight. Outer-rim coverage stays in `fillet_closed_edge.rs`.
//!
//! Cone-walled rims (Plane–Cone) are now supported too (task #89,
//! `cone_rim_fillet`): `cone_walled_rim_fillet_succeeds` pins that filleting a
//! frustum-tube's outer-top rim yields a sound, watertight torus blend.

use std::f64::consts::TAU;

use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::chamfer::{chamfer_edges, ChamferOptions};
use geometry_engine::operations::fillet::{FilletType, PropagationMode};
use geometry_engine::operations::revolve::{revolve_profile, RevolveOptions};
use geometry_engine::operations::{fillet_edges, FilletOptions, OperationError};
use geometry_engine::primitives::curve::{Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::SurfaceType;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};

/// Revolve a closed (r, z) profile a full turn about +Z.
fn revolve_tube(m: &mut BRepModel, pts: &[(f64, f64)]) -> SolidId {
    let verts: Vec<_> = pts
        .iter()
        .map(|(r, z)| m.vertices.add(*r, 0.0, *z))
        .collect();
    let mut edges = Vec::new();
    for i in 0..pts.len() {
        let j = (i + 1) % pts.len();
        let line = Line::new(
            Point3::new(pts[i].0, 0.0, pts[i].1),
            Point3::new(pts[j].0, 0.0, pts[j].1),
        );
        let cid = m.curves.add(Box::new(line));
        edges.push(m.edges.add(Edge::new(
            0,
            verts[i],
            verts[j],
            cid,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )));
    }
    let opts = RevolveOptions {
        axis_origin: Point3::ZERO,
        axis_direction: Vector3::Z,
        angle: TAU,
        segments: 64,
        ..Default::default()
    };
    revolve_profile(m, edges, opts).expect("tube revolve")
}

/// Closed rim edge whose seam vertex sits at radius `r_want`, height `z_want`.
fn rim_at(m: &BRepModel, r_want: f64, z_want: f64) -> Option<EdgeId> {
    m.edges.iter().find_map(|(id, e)| {
        if !e.is_loop() {
            return None;
        }
        let p = m.vertices.get_position(e.start_vertex)?;
        let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
        if (r - r_want).abs() < 0.5 && (p[2] - z_want).abs() < 0.5 {
            Some(id)
        } else {
            None
        }
    })
}

fn assert_valid_watertight(m: &mut BRepModel, s: SolidId, what: &str) {
    let v = validate_solid_scoped(m, s, Tolerance::default(), ValidationLevel::Standard);
    assert!(v.is_valid, "{what}: B-Rep invalid: {:?}", v.errors);
    assert!(
        geometry_engine::harness::watertight::is_watertight(m, s, 0.25, 1e-3),
        "{what}: mesh not watertight"
    );
}

fn count_surface(m: &BRepModel, s: SolidId, want: SurfaceType) -> usize {
    let solid = m.solids.get(s).expect("solid");
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    let mut n = 0;
    for sh in shells {
        if let Some(shell) = m.shells.get(sh) {
            for &fid in &shell.faces {
                if let Some(f) = m.faces.get(fid) {
                    if let Some(surf) = m.surfaces.get(f.surface_id) {
                        if surf.surface_type() == want {
                            n += 1;
                        }
                    }
                }
            }
        }
    }
    n
}

// Tube: outer wall R10, bore R6, z 0..20. Outer wall + bore are
// cylinders; top + bottom caps are annular planes. The bore-top rim
// (r≈6, z≈20) is the inner loop of the top cap.
const TUBE: &[(f64, f64)] = &[(10.0, 0.0), (10.0, 20.0), (6.0, 20.0), (6.0, 0.0)];

#[test]
fn bore_rim_fillet_succeeds_watertight_with_torus() {
    let mut m = BRepModel::new();
    let s = revolve_tube(&mut m, TUBE);
    let rim = rim_at(&m, 6.0, 20.0).expect("bore-top rim is a closed edge");

    let tori_before = count_surface(&m, s, SurfaceType::Torus);
    let opts = FilletOptions {
        fillet_type: FilletType::Constant(1.0),
        radius: 1.0,
        propagation: PropagationMode::None,
        ..Default::default()
    };
    fillet_edges(&mut m, s, vec![rim], opts).expect("bore rim fillet must succeed");

    assert_eq!(
        count_surface(&m, s, SurfaceType::Torus),
        tori_before + 1,
        "bore fillet must add exactly one torus blend face"
    );
    assert!(
        m.edges.get(rim).is_none(),
        "original bore rim edge must be retired"
    );
    assert_valid_watertight(&mut m, s, "bore-rim fillet");
}

#[test]
fn bore_rim_chamfer_succeeds_watertight_with_cone() {
    let mut m = BRepModel::new();
    let s = revolve_tube(&mut m, TUBE);
    let rim = rim_at(&m, 6.0, 20.0).expect("bore-top rim is a closed edge");

    let cones_before = count_surface(&m, s, SurfaceType::Cone);
    let opts = ChamferOptions::default(); // symmetric 1.0
    chamfer_edges(&mut m, s, vec![rim], opts).expect("bore rim chamfer must succeed");

    assert_eq!(
        count_surface(&m, s, SurfaceType::Cone),
        cones_before + 1,
        "bore chamfer must add exactly one cone blend face"
    );
    assert_valid_watertight(&mut m, s, "bore-rim chamfer");
}

#[test]
fn outer_rim_of_annular_cap_still_works() {
    // Regression guard: the OUTER rim of the same annular cap must keep
    // working after the bore-rim generalization (radial_out = +1 path).
    let mut m = BRepModel::new();
    let s = revolve_tube(&mut m, TUBE);
    let rim = rim_at(&m, 10.0, 20.0).expect("outer-top rim");
    let opts = FilletOptions {
        fillet_type: FilletType::Constant(1.0),
        radius: 1.0,
        propagation: PropagationMode::None,
        ..Default::default()
    };
    fillet_edges(&mut m, s, vec![rim], opts).expect("outer rim fillet still works");
    assert_valid_watertight(&mut m, s, "outer-rim fillet (annular cap)");
}

#[test]
fn bore_rim_fillet_radius_too_large_rejected_cleanly() {
    // The rounded bore (R+r) must not reach the cap's outer edge (R_outer
    // = 10). r = 3.5 → 6 + 3.5 = 9.5 < 10 is fine; r = 4.5 → 10.5 > 10
    // must be rejected with an actionable message, not a panic.
    let mut m = BRepModel::new();
    let s = revolve_tube(&mut m, TUBE);
    let rim = rim_at(&m, 6.0, 20.0).expect("bore-top rim");
    let opts = FilletOptions {
        fillet_type: FilletType::Constant(4.5),
        radius: 4.5,
        propagation: PropagationMode::None,
        ..Default::default()
    };
    let err =
        fillet_edges(&mut m, s, vec![rim], opts).expect_err("over-large bore fillet rejected");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("bore") || msg.contains("outer edge"),
        "expected a bore-width rejection, got: {msg}"
    );
}

#[test]
fn cone_walled_rim_fillet_succeeds() {
    // A cone-frustum tube: the outer-top rim is Plane+Cone. Closed-edge fillet
    // now supports this (#89 — `cone_rim_fillet`, an analytic torus carrier), so
    // it must produce a SOUND, watertight solid with the torus blend — never a
    // corrupt solid and never NotImplemented.
    let cone: &[(f64, f64)] = &[(10.0, 0.0), (6.0, 20.0), (4.0, 20.0), (8.0, 0.0)];
    let mut m = BRepModel::new();
    let s = revolve_tube(&mut m, cone);
    let rim = rim_at(&m, 6.0, 20.0).expect("cone outer-top rim");
    let opts = FilletOptions {
        fillet_type: FilletType::Constant(1.0),
        radius: 1.0,
        propagation: PropagationMode::None,
        ..Default::default()
    };
    let blend = fillet_edges(&mut m, s, vec![rim], opts).expect("cone-walled rim fillet (#89)");
    assert_eq!(
        blend.len(),
        1,
        "expected one torus blend face, got {}",
        blend.len()
    );
    assert_valid_watertight(&mut m, s, "cone-walled outer-top rim fillet");
}
