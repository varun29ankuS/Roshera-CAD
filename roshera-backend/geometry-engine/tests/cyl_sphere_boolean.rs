//! CYL∘SPHERE boolean campaign (task #7 — analytic SSI arms: cyl∘sphere).
//!
//! Surfaced by a live dogfood: "subtract a sphere from a cylinder of the same
//! radius". There is NO analytic cylinder×sphere surface-surface intersection in
//! the boolean dispatcher (`surface_surface_intersection` routes Cylinder–Sphere
//! to the generic MARCHING fallback), so the result is unreliable:
//!
//!   * SAME radius (sphere tangent to the cylinder wall along the whole
//!     equator): the intersection curve degenerates to a tangent circle the
//!     marcher cannot trace → ~200 OPEN edges, not watertight, invalid.
//!   * SMALLER, fully-enclosed sphere (clean spherical void): the mesh closes
//!     (watertight) but the B-Rep still validates INVALID — the internal void
//!     shell is not formed/validated cleanly.
//!
//! Both are a z-axis cylinder centred at the origin (radius `rc`, height 10,
//! z∈[-5,5]) minus a sphere at the origin (radius `rs`). For `rs ≤ rc` and
//! `rs ≤ 5` the sphere is fully enclosed, so the correct result is the cylinder
//! carrying a spherical cavity: volume = π·rc²·10 − (4/3)π·rs³, watertight, and
//! a valid 2-shell solid.
//!
//! These GATES assert that correct outcome. They FAIL today, so they are
//! #[ignore]'d — flip on when the analytic cyl∘sphere SSI (+ tangency handling
//! + void-shell validity) lands. Run the live signature:
//!   `cargo test -p geometry-engine --test cyl_sphere_boolean -- --ignored --nocapture`

use std::f64::consts::PI;

use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};

fn sid(g: GeometryId) -> geometry_engine::primitives::solid::SolidId {
    match g {
        GeometryId::Solid(id) => id,
        o => panic!("expected Solid, got {o:?}"),
    }
}

/// cylinder(radius=rc, h=10, centred at origin) ∖ sphere(radius=rs, at origin).
/// Returns (open_edges, nonmanifold_edges, valid, volume) of the result, or an
/// error string if the boolean itself failed.
fn cyl_minus_sphere(rc: f64, rs: f64) -> Result<(usize, usize, bool, f64), String> {
    let mut m = BRepModel::new();
    let cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -5.0), Vector3::Z, rc, 10.0)
        .expect("cylinder"));
    let sph = sid(TopologyBuilder::new(&mut m)
        .create_sphere_3d(Point3::ORIGIN, rs)
        .expect("sphere"));
    let res = boolean_operation(
        &mut m,
        cyl,
        sph,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .map_err(|e| format!("boolean errored: {e:?}"))?;
    let rep = manifold_report(&m, res, 0.5, 1e-6).ok_or("no mesh")?;
    let v = validate_solid_scoped(&m, res, Tolerance::default(), ValidationLevel::Standard);
    if !v.is_valid {
        for e in v.errors.iter().take(6) {
            eprintln!("    [validity] {e:?}");
        }
    }
    let vol = m.calculate_solid_volume(res).unwrap_or(f64::NAN);
    Ok((rep.boundary_edges, rep.nonmanifold_edges, v.is_valid, vol))
}

fn assert_clean_void(rc: f64, rs: f64) {
    let (open, nm, valid, vol) =
        cyl_minus_sphere(rc, rs).unwrap_or_else(|e| panic!("cyl(r{rc}) ∖ sphere(r{rs}): {e}"));
    let expected = PI * rc * rc * 10.0 - (4.0 / 3.0) * PI * rs * rs * rs;
    let rel = (vol - expected).abs() / expected;
    eprintln!(
        "[cyl∖sphere] rc={rc} rs={rs}: open={open} nm={nm} valid={valid} vol={vol:.2} expected={expected:.2} ({:+.1}%)",
        100.0 * (vol - expected) / expected
    );
    assert_eq!(
        (open, nm),
        (0, 0),
        "cyl(r{rc}) ∖ sphere(r{rs}): not watertight (open={open} nm={nm})"
    );
    assert!(valid, "cyl(r{rc}) ∖ sphere(r{rs}): invalid B-Rep");
    assert!(
        rel < 0.03,
        "cyl(r{rc}) ∖ sphere(r{rs}): volume {vol:.2} vs expected {expected:.2} ({:+.1}%)",
        100.0 * (vol - expected) / expected
    );
}

/// SAME radius — sphere tangent to the cylinder wall along the whole equator.
/// The degenerate tangency case the marching cyl∘sphere SSI cannot trace.
#[test]
#[ignore = "task #7: cyl∘sphere analytic SSI not implemented — flip on when it lands"]
fn cyl_minus_sphere_same_radius_7() {
    assert_clean_void(5.0, 5.0);
}

/// SMALLER enclosed sphere — a clean interior spherical void; no wall tangency.
/// The mesh closes but the void-shell B-Rep validates invalid today.
#[test]
#[ignore = "task #7: cyl∘sphere enclosed-void B-Rep invalid — flip on when it lands"]
fn cyl_minus_sphere_enclosed_void_7() {
    assert_clean_void(5.0, 4.0);
}
