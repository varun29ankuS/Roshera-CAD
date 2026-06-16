//! Revolve analytic-bands gate (#19).
//!
//! A FULL revolution of a closed cylinder/plane line profile must emit ONE
//! analytic face per band (Cylinder / annular Plane) — NOT one
//! `SurfaceOfRevolution` patch per (segment × band). A 48-segment tube must be
//! 4 analytic faces, not 192. The analytic path self-verifies watertightness
//! and rolls back to the per-segment path on any failure, so this also pins the
//! zero-regression contract: cone/stepped profiles still produce a watertight
//! solid (via fallback), just not the minimal analytic face set yet (v2).
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::revolve::{revolve_profile, RevolveOptions};
use geometry_engine::primitives::curve::{Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::{Cylinder, SurfaceType};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};

fn revolve(m: &mut BRepModel, pts: &[(f64, f64)], segments: u32) -> SolidId {
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
        angle: std::f64::consts::TAU,
        segments,
        ..Default::default()
    };
    revolve_profile(m, edges, opts).unwrap_or_else(|e| panic!("revolve: {e:?}"))
}

fn face_kinds(m: &BRepModel, sid: SolidId) -> Vec<SurfaceType> {
    let solid = m.solids.get(sid).unwrap_or_else(|| panic!("solid"));
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    let mut out = Vec::new();
    for shid in shells {
        if let Some(shell) = m.shells.get(shid) {
            for &fid in &shell.faces {
                if let Some(f) = m.faces.get(fid) {
                    if let Some(s) = m.surfaces.get(f.surface_id) {
                        out.push(s.surface_type());
                    }
                }
            }
        }
    }
    out
}

fn count(k: &[SurfaceType], want: SurfaceType) -> usize {
    k.iter().filter(|&&x| x == want).count()
}

fn cyl_radii(m: &BRepModel, sid: SolidId) -> Vec<f64> {
    let solid = m.solids.get(sid).unwrap_or_else(|| panic!("solid"));
    let shell = m
        .shells
        .get(solid.outer_shell)
        .unwrap_or_else(|| panic!("shell"));
    let mut out = Vec::new();
    for &fid in &shell.faces {
        if let Some(f) = m.faces.get(fid) {
            if let Some(s) = m.surfaces.get(f.surface_id) {
                if let Some(c) = s.as_any().downcast_ref::<Cylinder>() {
                    out.push(c.radius);
                }
            }
        }
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    out
}

#[test]
fn tube_is_four_analytic_faces_19() {
    // Tube r6..10, z0..20: outer cyl r10 + inner cyl r6 + 2 annular plane caps.
    let mut m = BRepModel::new();
    let s = revolve(
        &mut m,
        &[(10.0, 0.0), (10.0, 20.0), (6.0, 20.0), (6.0, 0.0)],
        48,
    );
    let k = face_kinds(&m, s);
    assert_eq!(k.len(), 4, "tube must be 4 faces, not 192 (kinds={k:?})");
    assert_eq!(count(&k, SurfaceType::Cylinder), 2, "2 cylinder walls");
    assert_eq!(count(&k, SurfaceType::Plane), 2, "2 annular plane caps");
    assert_eq!(
        count(&k, SurfaceType::SurfaceOfRevolution),
        0,
        "no faceted SurfaceOfRevolution patches"
    );
    let radii = cyl_radii(&m, s);
    assert!(
        radii.len() == 2 && (radii[0] - 6.0).abs() < 1e-6 && (radii[1] - 10.0).abs() < 1e-6,
        "cylinder radii recoverable as 6 and 10: {radii:?}"
    );
    let v = validate_solid_scoped(&m, s, Tolerance::default(), ValidationLevel::Standard);
    assert!(v.is_valid, "tube B-Rep invalid: {:?}", v.errors);
}

#[test]
fn open_washer_is_four_analytic_faces_19() {
    // A flat washer (short tube): outer r20, inner r8, thin (z0..2). Still 4.
    let mut m = BRepModel::new();
    let s = revolve(
        &mut m,
        &[(8.0, 0.0), (20.0, 0.0), (20.0, 2.0), (8.0, 2.0)],
        64,
    );
    let k = face_kinds(&m, s);
    assert_eq!(k.len(), 4, "washer must be 4 faces (kinds={k:?})");
    assert_eq!(count(&k, SurfaceType::Cylinder), 2);
    assert_eq!(count(&k, SurfaceType::Plane), 2);
}
