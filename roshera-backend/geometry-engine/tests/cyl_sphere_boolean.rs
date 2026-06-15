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

/// DIAGNOSTIC (task #7): localize the enclosed-void invalidity. Validate a LONE
/// sphere, a LONE cylinder, and the cyl∖sphere result; print per-solid shell
/// count + the Euler residual message. Tells us whether the sphere PRIMITIVE's
/// B-Rep is the odd-Euler source or whether the difference's void-shell drops
/// the sphere seam/poles.
#[test]
#[ignore = "diagnostic — run with --ignored --nocapture"]
fn diag_cyl_sphere_validity_structure_7() {
    let report = |label: &str, m: &BRepModel, s: geometry_engine::primitives::solid::SolidId| {
        let v = validate_solid_scoped(m, s, Tolerance::default(), ValidationLevel::Standard);
        let solid = m.solids.get(s);
        let n_shells = solid.map(|sd| 1 + sd.inner_shells.len()).unwrap_or(0);
        eprintln!("[{label}] valid={} shells={n_shells}", v.is_valid);
        for e in v.errors.iter().take(4) {
            eprintln!("    {e:?}");
        }
    };
    // Lone sphere.
    let mut ms = BRepModel::new();
    let sp = sid(TopologyBuilder::new(&mut ms)
        .create_sphere_3d(Point3::ORIGIN, 4.0)
        .expect("sphere"));
    report("lone-sphere-r4", &ms, sp);
    // Lone cylinder.
    let mut mc = BRepModel::new();
    let cy = sid(TopologyBuilder::new(&mut mc)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -5.0), Vector3::Z, 5.0, 10.0)
        .expect("cyl"));
    report("lone-cyl-r5", &mc, cy);
    // The difference.
    let mut m = BRepModel::new();
    let cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -5.0), Vector3::Z, 5.0, 10.0)
        .expect("cylinder"));
    let sph = sid(TopologyBuilder::new(&mut m)
        .create_sphere_3d(Point3::ORIGIN, 4.0)
        .expect("sphere"));
    let res = boolean_operation(
        &mut m,
        cyl,
        sph,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("difference");
    report("cyl-minus-sphere", &m, res);

    // Per-shell unique edge/vertex/loop counts (the Euler decomposition).
    let dump_shell = |label: &str,
                      mm: &BRepModel,
                      sh_id: geometry_engine::primitives::shell::ShellId| {
        use std::collections::HashSet;
        let mut edges = HashSet::new();
        let mut verts = HashSet::new();
        let mut n_loops = 0usize;
        let mut n_faces = 0usize;
        if let Some(sh) = mm.shells.get(sh_id) {
            n_faces = sh.faces.len();
            for &fid in &sh.faces {
                if let Some(f) = mm.faces.get(fid) {
                    for lid in std::iter::once(f.outer_loop).chain(f.inner_loops.iter().copied()) {
                        if let Some(lp) = mm.loops.get(lid) {
                            n_loops += 1;
                            for &eid in &lp.edges {
                                edges.insert(eid);
                                if let Some(e) = mm.edges.get(eid) {
                                    verts.insert(e.start_vertex);
                                    verts.insert(e.end_vertex);
                                }
                            }
                        }
                    }
                }
            }
        }
        let (vn, en, fnn) = (verts.len() as i64, edges.len() as i64, n_faces as i64);
        eprintln!(
            "    [{label}] shell {sh_id:?}: F={n_faces} loops={n_loops} E={} V={} chi={}",
            edges.len(),
            verts.len(),
            vn - en + fnn
        );
    };
    // Lone sphere structure for comparison.
    if let Some(solid) = ms.solids.get(sp) {
        dump_shell("lone-sphere", &ms, solid.outer_shell);
    }
    if let Some(solid) = m.solids.get(res) {
        eprintln!(
            "    outer_shell={:?} inner_shells={:?}",
            solid.outer_shell, solid.inner_shells
        );
        dump_shell("result-outer", &m, solid.outer_shell);
        for &is in &solid.inner_shells {
            dump_shell("result-void", &m, is);
        }
    }
}

/// TRANSVERSE gate (BOOL #7): a sphere that pokes THROUGH the cylinder wall
/// (rs > rc), fully within the cylinder's height so it cuts ONLY the lateral
/// (two circles at z = ±√(rs²−rc²)). This is the case the analytic cyl×sphere
/// SSI arm handles (exact circles, vs the old marching hang/garbage). The
/// end-to-end difference must be watertight + valid. cyl(r5, z∈[-10,10]) ∖
/// sphere(r6 @ origin) → a cylinder with a spherical side-scoop open at the wall.
#[test]
fn cyl_minus_sphere_transverse_wall_7() {
    let mut m = BRepModel::new();
    let cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -10.0), Vector3::Z, 5.0, 20.0)
        .expect("cyl"));
    let sph = sid(TopologyBuilder::new(&mut m)
        .create_sphere_3d(Point3::ORIGIN, 6.0)
        .expect("sphere"));
    let res = boolean_operation(
        &mut m,
        cyl,
        sph,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("transverse cyl∖sphere must succeed");
    let rep = manifold_report(&m, res, 0.5, 1e-6).expect("mesh");
    let v = validate_solid_scoped(&m, res, Tolerance::default(), ValidationLevel::Standard);
    let vol = m.calculate_solid_volume(res).unwrap_or(f64::NAN);
    eprintln!(
        "[transverse] open={} nm={} valid={} vol={vol:.2}",
        rep.boundary_edges, rep.nonmanifold_edges, v.is_valid
    );
    assert_eq!(
        (rep.boundary_edges, rep.nonmanifold_edges),
        (0, 0),
        "transverse cyl∖sphere not watertight"
    );
    assert!(
        v.is_valid,
        "transverse cyl∖sphere invalid B-Rep: {:?}",
        v.errors
    );
    // Sanity bounds: removed volume is positive and less than the cylinder.
    let cyl_vol = std::f64::consts::PI * 25.0 * 20.0;
    assert!(
        vol > 0.0 && vol < cyl_vol,
        "transverse cyl∖sphere volume {vol:.2} out of (0, {cyl_vol:.2})"
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
/// FIXED (validator): the spherical void shell is a seamless closed face (χ=2),
/// which the Euler–Poincaré check now accounts for in a mixed (seamed outer +
/// seamless void) solid. Geometry was always correct (watertight, exact volume);
/// only the validity check was wrong. This is now a passing GATE.
#[test]
fn cyl_minus_sphere_enclosed_void_7() {
    assert_clean_void(5.0, 4.0);
}
