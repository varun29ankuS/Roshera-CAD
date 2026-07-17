// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! CYL∘SPHERE boolean campaign (task #7 — analytic SSI arms: cyl∘sphere).
//!
//! Surfaced by a live dogfood: "subtract a sphere from a cylinder of the same
//! radius". Originally there was NO analytic cylinder×sphere surface-surface
//! intersection at all; the ON-AXIS circle arm landed first (transverse-wall
//! and enclosed-void gates below), and the GENERAL-POSITION quartic arm
//! (off-axis sphere centre — one bite loop or two pierce ovals, the exact
//! Levin-pencil `QsicCurve` on the cylinder carrier) landed 2026-07-17 with
//! the `diff_cyl_offaxis_sphere_bite_7` / `diff_drilled_ball_offaxis_7` /
//! `diff_box_boss_offaxis_sphere_7` gates below (RED signatures recorded in
//! `cylinder_sphere_offaxis_ssi_returns_analytic_quartic_7`'s doc, boolean.rs).
//!
//! STILL OPEN (honest residual): the SAME-radius coaxial case — the sphere
//! tangent to the cylinder wall along the whole equator. The intersection
//! degenerates to a tangent circle (a genuine #86-class tangency, `Δ ≡ 0` on
//! the contact ring); the analytic arms refuse it typed and the marcher
//! cannot trace it → ~200 OPEN edges, not watertight, invalid. Its gate
//! (`cyl_minus_sphere_same_radius_7`) stays `#[ignore]`d until tangential
//! contact handling exists. Run the live signature:
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

// ---------------------------------------------------------------------------
// GENERAL-POSITION cyl∘sphere (the bool7 residual): OFF-AXIS sphere centre.
// The intersection is a genuine spatial quartic (one bite loop or two pierce
// ovals) — no circle special case applies. These gates are RED until the
// analytic cyl–sphere QSIC producer + sphere-wall splitting/tessellation land.
// ---------------------------------------------------------------------------

/// EXACT volume oracle for `cylinder ∩ sphere` when the sphere's axial span
/// lies strictly inside the cylinder's axial extent (no axial clipping):
///
///   V = ∫∫_{D} 2·√(r_s² − ρ²) dA,
///
/// over the region `D` = (cylinder cross-section disk) as seen from the
/// SPHERE's planar centre, `ρ` = planar distance from the sphere centre and
/// `d` = planar distance between the sphere centre and the cylinder axis.
/// In polar coordinates about the sphere centre the ρ-integral is CLOSED
/// FORM (`∫ 2√(r_s²−ρ²)·ρ dρ = −(2/3)(r_s²−ρ²)^{3/2}`), leaving a smooth 1-D
/// φ-integral evaluated by dense Simpson — accuracy far below 1e-8 relative,
/// suitable for a 1e-4 acceptance bar. Handles both `d < r_c` (sphere centre
/// planar-inside the bore disk; drilled-ball case) and `d > r_c` (bite case:
/// each ray hits the disk in an interval `[ρ_1, ρ_2]`).
fn cyl_sphere_overlap_volume_oracle(rc: f64, d: f64, rs: f64) -> f64 {
    let f = |rho: f64| -> f64 {
        // −(2/3)(r_s²−ρ²)^{3/2}, clamped past the sphere rim.
        let s = (rs * rs - rho * rho).max(0.0);
        -(2.0 / 3.0) * s * s.sqrt()
    };
    let integrand = |phi: f64| -> f64 {
        // Ray from the sphere planar centre, direction (cos φ, sin φ); the
        // cylinder axis sits at planar distance d along +x. Points at ρ are
        // inside the disk when ρ² − 2ρ·d·cosφ + d² − r_c² ≤ 0.
        let b = d * phi.cos();
        let disc = b * b - (d * d - rc * rc);
        if disc <= 0.0 {
            return 0.0;
        }
        let sq = disc.sqrt();
        let lo = (b - sq).max(0.0);
        let hi = (b + sq).min(rs).max(0.0);
        if hi <= lo {
            return 0.0;
        }
        f(hi) - f(lo)
    };
    // Composite Simpson over φ ∈ [0, 2π], n even and dense.
    let n = 200_000usize;
    let h = 2.0 * PI / n as f64;
    let mut acc = integrand(0.0) + integrand(2.0 * PI);
    for i in 1..n {
        let w = if i % 2 == 1 { 4.0 } else { 2.0 };
        acc += w * integrand(h * i as f64);
    }
    acc * h / 3.0
}

/// Oracle self-check: at d=0 with r_c ≥ r_s the overlap is the whole sphere;
/// with r_s ≥ √(r_c² + (h/2)²)… (axial clipping is out of contract) — pin the
/// enclosed-sphere limit and the additivity of two complementary bites.
#[test]
fn cyl_sphere_overlap_oracle_pins_7() {
    let v_sphere = (4.0 / 3.0) * PI * 4.0_f64.powi(3);
    let rel = (cyl_sphere_overlap_volume_oracle(5.0, 0.0, 4.0) - v_sphere).abs() / v_sphere;
    assert!(rel < 1e-9, "enclosed-sphere limit off by {rel:.3e}");
    // Symmetric split: a sphere centred ON the wall (d = r_c) of a huge
    // cylinder loses exactly half its volume to the outside as r_c → ∞.
    let half = 0.5 * (4.0 / 3.0) * PI * 2.0_f64.powi(3);
    let v = cyl_sphere_overlap_volume_oracle(4000.0, 4000.0, 2.0);
    let rel2 = (v - half).abs() / half;
    assert!(
        rel2 < 1e-3,
        "wall-centred half-sphere limit off by {rel2:.3e}"
    );
}

/// Sound-build assertion body shared by the general-position fixtures:
/// watertight + manifold + closed + oriented + valid B-Rep, dense-mesh volume
/// against the analytic oracle at `dense_bar`, live mass-props at `live_bar`
/// (fine()'s 200-segment cap precedent from #35 Slice 2).
fn assert_sound_with_volume(
    label: &str,
    m: &mut BRepModel,
    res: geometry_engine::primitives::solid::SolidId,
    v_oracle: f64,
    dense_bar: f64,
    live_bar: f64,
) {
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    let vol = m.calculate_solid_volume(res).unwrap_or(f64::NAN);
    let ultra = TessellationParams {
        max_angle_deviation: 0.005,
        chord_tolerance: 2e-5,
        min_segments: 16,
        max_segments: 800,
        ..TessellationParams::fine()
    };
    let vol_dense = m
        .solids
        .get(res)
        .map(|s| tessellate_solid(s, m, &ultra))
        .map(|mesh| {
            let mut six_v = 0.0;
            for tri in &mesh.triangles {
                let p0 = mesh.vertices[tri[0] as usize].position;
                let p1 = mesh.vertices[tri[1] as usize].position;
                let p2 = mesh.vertices[tri[2] as usize].position;
                six_v += p0.dot(&p1.cross(&p2));
            }
            (six_v / 6.0).abs()
        })
        .expect("result solid must tessellate");
    let rep = manifold_report(m, res, 0.5, 1e-6).expect("manifold report");
    let v = validate_solid_scoped(m, res, Tolerance::default(), ValidationLevel::Standard);
    eprintln!(
        "[{label}] open={} nm={} euler={} manifold={} closed={} oriented={} brep_valid={} \
         vol={vol:.4} vol_dense={vol_dense:.4} oracle={v_oracle:.4} rel={:.3e} rel_dense={:.3e}",
        rep.boundary_edges,
        rep.nonmanifold_edges,
        rep.euler_characteristic,
        rep.manifold,
        rep.closed,
        rep.oriented,
        v.is_valid,
        ((vol - v_oracle) / v_oracle).abs(),
        ((vol_dense - v_oracle) / v_oracle).abs(),
    );
    assert!(
        rep.boundary_edges == 0
            && rep.nonmanifold_edges == 0
            && rep.manifold
            && rep.closed
            && rep.oriented,
        "{label}: result must be watertight+manifold+closed+oriented \
         (open={}, nm={}, manifold={}, closed={}, oriented={})",
        rep.boundary_edges,
        rep.nonmanifold_edges,
        rep.manifold,
        rep.closed,
        rep.oriented
    );
    assert!(
        v.is_valid,
        "{label}: validate_solid_scoped must pass ({} errors: {:?})",
        v.errors.len(),
        v.errors.iter().take(4).collect::<Vec<_>>()
    );
    let rel_dense = ((vol_dense - v_oracle) / v_oracle).abs();
    assert!(
        rel_dense <= dense_bar,
        "{label}: dense-mesh volume {vol_dense:.6} must match the analytic oracle \
         {v_oracle:.6} to ≤{dense_bar:.0e} relative (rel={rel_dense:.3e})"
    );
    let rel_live = ((vol - v_oracle) / v_oracle).abs();
    assert!(
        rel_live <= live_bar,
        "{label}: live mass-props volume {vol:.6} must stay within {live_bar:.0e} of \
         the oracle {v_oracle:.6} (rel={rel_live:.3e})"
    );
}

/// PARTIAL-BITE builder: cyl(r5, z∈[0,10] at the origin) ∖ sphere(r4 at
/// (6.5, 1, 5)). Radial offset d = √(6.5²+1²) ≈ 6.5765 → |r_c−d| ≈ 1.576 <
/// r_s = 4 < r_c+d ≈ 11.58: ONE closed bite loop, a window on the cylinder
/// wall (angular half-width ≈ 37°, axial extent ≈ [1.32, 8.68] — strictly
/// inside the rims, away from the u-seam) and a lens cap on the sphere.
fn build_bite_7(m: &mut BRepModel) -> Result<geometry_engine::primitives::solid::SolidId, String> {
    let cyl = sid(TopologyBuilder::new(m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 5.0, 10.0)
        .expect("cyl"));
    let sph = sid(TopologyBuilder::new(m)
        .create_sphere_3d(Point3::new(6.5, 1.0, 5.0), 4.0)
        .expect("sphere"));
    boolean_operation(
        m,
        cyl,
        sph,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .map_err(|e| format!("bite difference errored: {e:?}"))
}

/// GENERAL-POSITION gate (bool7 residual, partial bite): an off-axis sphere
/// takes a side bite out of a cylinder. RED until the cyl–sphere QSIC lands
/// (recorded marching baseline in the fn doc of the SSI white-box test).
#[test]
fn diff_cyl_offaxis_sphere_bite_7() {
    let mut m = BRepModel::new();
    let res = build_bite_7(&mut m).expect("bite difference must build");
    let d = (6.5_f64 * 6.5 + 1.0).sqrt();
    let v_cyl = PI * 25.0 * 10.0;
    let v_bite = cyl_sphere_overlap_volume_oracle(5.0, d, 4.0);
    assert_sound_with_volume("bite-7", &mut m, res, v_cyl - v_bite, 1e-4, 5e-4);
}

/// Determinism 5× for the bite fixture (fingerprint: quantised volume +
/// face/tri/edge/vertex counts).
#[test]
fn bite_7_is_deterministic() {
    let mut fps: Vec<(u64, usize, usize, usize, usize)> = Vec::with_capacity(5);
    for run in 0..5 {
        let mut m = BRepModel::new();
        let res = build_bite_7(&mut m).unwrap_or_else(|e| panic!("run {run}: {e}"));
        let vol = m.calculate_solid_volume(res).unwrap_or(f64::NAN);
        let vq = (vol * 1e6).round() as u64;
        let faces = m
            .solids
            .get(res)
            .map(|s| {
                std::iter::once(s.outer_shell)
                    .chain(s.inner_shells.iter().copied())
                    .filter_map(|sh| m.shells.get(sh))
                    .map(|sh| sh.faces.len())
                    .sum::<usize>()
            })
            .unwrap_or(0);
        let report = manifold_report(&m, res, 0.08, 1e-6);
        let tris = report.as_ref().map(|r| r.triangles).unwrap_or(0);
        let edges = report.as_ref().map(|r| r.undirected_edges).unwrap_or(0);
        let verts = report.as_ref().map(|r| r.welded_vertices).unwrap_or(0);
        fps.push((vq, faces, tris, edges, verts));
    }
    let first = fps[0];
    for (i, fp) in fps.iter().enumerate() {
        assert_eq!(
            *fp, first,
            "bite-7 NONDETERMINISTIC: run {i} fingerprint {fp:?} != run0 {first:?}; \
             full set = {fps:?}"
        );
    }
}

/// FULL-PIERCE builder: sphere(r8 at the origin) ∖ cyl(r3 along Z through
/// (2, 0), z∈[−12, 12]) — the drilled ball. r_s = 8 > r_c + d = 5 with 3 mm
/// clearance: the bore fully pierces the sphere → TWO closed QSIC ovals
/// (entry + exit), axial bands on the tool wall, two caps + a two-hole
/// barrel on the sphere.
fn build_drilled_ball_7(
    m: &mut BRepModel,
) -> Result<geometry_engine::primitives::solid::SolidId, String> {
    let sph = sid(TopologyBuilder::new(m)
        .create_sphere_3d(Point3::ORIGIN, 8.0)
        .expect("sphere"));
    let cyl = sid(TopologyBuilder::new(m)
        .create_cylinder_3d(Point3::new(2.0, 0.0, -12.0), Vector3::Z, 3.0, 24.0)
        .expect("cyl"));
    boolean_operation(
        m,
        sph,
        cyl,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .map_err(|e| format!("drilled-ball difference errored: {e:?}"))
}

/// GENERAL-POSITION gate (bool7 residual, full pierce): an off-axis bore
/// drilled through a ball. RED until the cyl–sphere QSIC lands.
#[test]
fn diff_drilled_ball_offaxis_7() {
    let mut m = BRepModel::new();
    let res = build_drilled_ball_7(&mut m).expect("drilled-ball difference must build");
    let v_sphere = (4.0 / 3.0) * PI * 512.0;
    let v_bore = cyl_sphere_overlap_volume_oracle(3.0, 2.0, 8.0);
    assert_sound_with_volume("drilled-ball-7", &mut m, res, v_sphere - v_bore, 1e-4, 5e-4);
}

/// Determinism 5× for the drilled ball.
#[test]
fn drilled_ball_7_is_deterministic() {
    let mut fps: Vec<(u64, usize, usize, usize, usize)> = Vec::with_capacity(5);
    for run in 0..5 {
        let mut m = BRepModel::new();
        let res = build_drilled_ball_7(&mut m).unwrap_or_else(|e| panic!("run {run}: {e}"));
        let vol = m.calculate_solid_volume(res).unwrap_or(f64::NAN);
        let vq = (vol * 1e6).round() as u64;
        let faces = m
            .solids
            .get(res)
            .map(|s| {
                std::iter::once(s.outer_shell)
                    .chain(s.inner_shells.iter().copied())
                    .filter_map(|sh| m.shells.get(sh))
                    .map(|sh| sh.faces.len())
                    .sum::<usize>()
            })
            .unwrap_or(0);
        let report = manifold_report(&m, res, 0.08, 1e-6);
        let tris = report.as_ref().map(|r| r.triangles).unwrap_or(0);
        let edges = report.as_ref().map(|r| r.undirected_edges).unwrap_or(0);
        let verts = report.as_ref().map(|r| r.welded_vertices).unwrap_or(0);
        fps.push((vq, faces, tris, edges, verts));
    }
    let first = fps[0];
    for (i, fp) in fps.iter().enumerate() {
        assert_eq!(
            *fp, first,
            "drilled-ball-7 NONDETERMINISTIC: run {i} fingerprint {fp:?} != run0 {first:?}; \
             full set = {fps:?}"
        );
    }
}

/// SEAM-LENS bite gate: same regime as `diff_cyl_offaxis_sphere_bite_7` but
/// with the sphere on the −X side of the bore, so the kept sphere
/// complement's rim (and the dropped lens) STRADDLE the sphere's own u=0
/// parameterisation seam (ref_dir = +X: the lens faces the cylinder axis,
/// i.e. the +X direction from the sphere centre). UV-projection paths (DCEL
/// walk, winding membership) degenerate across the seam; the chart-free QSIC
/// sphere splitter + slerp-ring cap tessellation must hold this one.
#[test]
fn diff_cyl_offaxis_sphere_bite_seam_7() {
    let mut m = BRepModel::new();
    let cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 5.0, 10.0)
        .expect("cyl"));
    let sph = sid(TopologyBuilder::new(&mut m)
        .create_sphere_3d(Point3::new(-6.5, 1.0, 5.0), 4.0)
        .expect("sphere"));
    let res = boolean_operation(
        &mut m,
        cyl,
        sph,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("seam-lens bite difference must build");
    let d = (6.5_f64 * 6.5 + 1.0).sqrt();
    let v_cyl = PI * 25.0 * 10.0;
    let v_bite = cyl_sphere_overlap_volume_oracle(5.0, d, 4.0);
    assert_sound_with_volume("bite-seam-7", &mut m, res, v_cyl - v_bite, 1e-4, 5e-4);
}

/// BOSS-BITE gate — the bool7-class dogfood shape: a box with a cylindrical
/// BOSS standing on its top face (coincident-coplanar union, the #44/#32
/// cap-merge path), minus an off-axis sphere biting the boss lateral (same
/// bite geometry as `diff_cyl_offaxis_sphere_bite_7`, in composite context:
/// the sphere face carries ONLY the QSIC loop, the boss wall carries the
/// window, and the box is untouched).
#[test]
fn diff_box_boss_offaxis_sphere_7() {
    use geometry_engine::operations::extrude::{extrude_polygon_regions, PolygonRegion};
    let mut m = BRepModel::new();
    let tol = Tolerance::default();
    let block = extrude_polygon_regions(
        &mut m,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::X,
        Vector3::Y,
        &[PolygonRegion {
            outer: vec![[0.0, 0.0], [40.0, 0.0], [40.0, 40.0], [0.0, 40.0]],
            holes: vec![],
        }],
        10.0,
        None,
        tol,
    )
    .expect("block");
    let boss = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(20.0, 20.0, 10.0), Vector3::Z, 5.0, 20.0)
        .expect("boss"));
    let composite = boolean_operation(
        &mut m,
        block,
        boss,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("box∪boss union must build");
    let sph = sid(TopologyBuilder::new(&mut m)
        .create_sphere_3d(Point3::new(26.5, 21.0, 20.0), 4.0)
        .expect("sphere"));
    let res = boolean_operation(
        &mut m,
        composite,
        sph,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("boss-bite difference must build");
    let d = (6.5_f64 * 6.5 + 1.0).sqrt();
    let v_oracle =
        40.0 * 40.0 * 10.0 + PI * 25.0 * 20.0 - cyl_sphere_overlap_volume_oracle(5.0, d, 4.0);
    assert_sound_with_volume("boss-bite-7", &mut m, res, v_oracle, 1e-4, 5e-4);
}

/// BALL-END ROD gate (bool7 companion, ON-AXIS): rod(r5, z∈[0,20]) ∪
/// sphere(r6 at (0,0,20)). The sphere centre is on the axis, so the lateral
/// cut is the analytic circle path (one circle at z = 20 − √(r_s²−r_c²) ≈
/// 16.683; the exit circle falls beyond the rod). Exact union volume =
/// π·r_c²·z_c + spherical-zone volume above z_c.
#[test]
fn union_ball_end_rod_7() {
    let mut m = BRepModel::new();
    let rod = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 5.0, 20.0)
        .expect("rod"));
    let ball = sid(TopologyBuilder::new(&mut m)
        .create_sphere_3d(Point3::new(0.0, 0.0, 20.0), 6.0)
        .expect("ball"));
    let res = boolean_operation(
        &mut m,
        rod,
        ball,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("ball-end-rod union must build");
    // V = V_rod + V_sphere − V_overlap; overlap = sphere slice z∈[14, z_c]
    // (cross-sections smaller than the rod) + rod core z∈[z_c, 20].
    // ∫ π(r_s²−u²) du (u = z−20, from −6 to u_c) + π r_c² (20−z_c).
    let (rc, rs) = (5.0_f64, 6.0_f64);
    let u_c = -(rs * rs - rc * rc).sqrt(); // ≈ −3.3166
    let z_c = 20.0 + u_c;
    let slice =
        PI * ((rs * rs * u_c - u_c.powi(3) / 3.0) - (rs * rs * (-rs) - (-rs).powi(3) / 3.0));
    let v_overlap = slice + PI * rc * rc * (20.0 - z_c);
    let v_oracle = PI * rc * rc * 20.0 + (4.0 / 3.0) * PI * rs.powi(3) - v_overlap;
    assert_sound_with_volume("ball-end-rod-7", &mut m, res, v_oracle, 1e-4, 5e-4);
}
