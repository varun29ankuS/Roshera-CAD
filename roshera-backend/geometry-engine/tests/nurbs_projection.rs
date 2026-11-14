#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Projecting a 3D point onto a NURBS surface (the footpoint, `closest_point`)
//! must find the TRUE footpoint, to tolerance, or say it could not.
//!
//! The mesher builds every curved face's outline by projecting its edge
//! samples onto the surface. Each projection started from scratch -- an
//! 11 × 11 grid search, then 20 half-strength Newton steps with a stopping
//! test that cannot fire for a point off the surface -- and returned `Ok`
//! whatever it reached. On the GrabCAD bench vice that made outlines that
//! crossed themselves (the face then meshed to nothing) and cost seconds a
//! face. A footpoint is a property of the geometry: these tests pin it.

use geometry_engine::math::{Point3, Tolerance};
use geometry_engine::operations::nurbs_loft::{nurbs_loft, NurbsLoftOptions};
use geometry_engine::primitives::surface::Surface;
use geometry_engine::primitives::topology_builder::BRepModel;

fn ring(n: usize, r: f64, z: f64) -> Vec<Point3> {
    (0..n)
        .map(|i| {
            let a = i as f64 * std::f64::consts::TAU / n as f64;
            Point3::new(r * a.cos(), r * a.sin(), z)
        })
        .collect()
}

/// The NURBS wall of a vice-sized barrel (Ø40–Ø70, 60 mm tall).
fn barrel_wall() -> (BRepModel, u32) {
    let mut m = BRepModel::new();
    let sections = vec![
        ring(20, 20.0, 0.0),
        ring(20, 30.0, 15.0),
        ring(20, 35.0, 30.0),
        ring(20, 30.0, 45.0),
        ring(20, 20.0, 60.0),
    ];
    let s = nurbs_loft(&mut m, sections, NurbsLoftOptions::default()).expect("barrel");
    let solid = m.solids.get(s).expect("solid");
    let shell = m.shells.get(solid.outer_shell).expect("shell");
    let surface_id = shell
        .faces
        .iter()
        .map(|&f| m.faces.get(f).expect("face").surface_id)
        .find(|&sid| m.surfaces.get(sid).expect("surface").type_name() == "NurbsSurface")
        .expect("a NURBS wall");
    (m, surface_id)
}

/// Interior parameter samples (away from the domain border).
fn interior_samples(surface: &dyn Surface) -> Vec<(f64, f64)> {
    let ((u0, u1), (v0, v1)) = surface.parameter_bounds();
    let mut out = Vec::new();
    for i in 1..12 {
        for j in 1..12 {
            out.push((
                u0 + (u1 - u0) * i as f64 / 12.0,
                v0 + (v1 - v0) * j as f64 / 12.0,
            ));
        }
    }
    out
}

#[test]
fn a_point_on_a_nurbs_surface_projects_onto_itself() {
    let (m, sid) = barrel_wall();
    let surface = m.surfaces.get(sid).expect("surface");
    let mut worst = 0.0_f64;
    for (u, v) in interior_samples(surface) {
        let p = surface.point_at(u, v).expect("point");
        let (fu, fv) = surface
            .closest_point(&p, Tolerance::default())
            .expect("footpoint");
        let q = surface.point_at(fu, fv).expect("point");
        worst = worst.max(p.distance(&q));
    }
    assert!(
        worst <= 1e-7,
        "a point ON the surface must project onto itself; worst miss {worst:.3e} mm"
    );
}

#[test]
fn a_point_off_a_nurbs_surface_projects_to_its_true_footpoint() {
    let (m, sid) = barrel_wall();
    let surface = m.surfaces.get(sid).expect("surface");
    let mut worst = 0.0_f64;
    for (u, v) in interior_samples(surface) {
        let foot = surface.point_at(u, v).expect("point");
        let n = surface.normal_at(u, v).expect("normal");
        // 0.2 mm off, along the normal: well inside every radius of curvature
        // here, so `foot` is the unique nearest point.
        for d in [0.2, -0.2] {
            let p = foot + n * d;
            let (fu, fv) = surface
                .closest_point(&p, Tolerance::default())
                .expect("footpoint");
            let q = surface.point_at(fu, fv).expect("point");
            worst = worst.max(foot.distance(&q));
        }
    }
    assert!(
        worst <= 1e-6,
        "a point 0.2 mm off the surface must project to its footpoint; worst miss {worst:.3e} mm"
    );
}
