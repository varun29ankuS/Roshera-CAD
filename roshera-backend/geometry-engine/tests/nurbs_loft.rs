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
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

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

/// A SLENDER, high-aspect (7.5:1 fineness) missile body — a tangent-ogive nose
/// blending into a constant-radius cylindrical body and tapering to a boattail,
/// lofted through 13 unevenly-spaced circular rings (the body rings sit 17 units
/// apart at z=16→33→50). This is the dogfooded shape whose lateral skin used to
/// under-tessellate (the v-sampling between the widely-spaced body rings collapsed
/// the silhouette to less than the true radius) and crack at the cap↔lateral rim.
///
/// The gate pins BOTH coupled defects shut, durably and across the whole display
/// preset matrix (not just the cert's `default()` chord): the mesh must be
/// watertight (zero boundary edges) at every preset a viewport/export path can
/// pick, AND every mesh must resolve the true Ø8 body — a v/u under-sample would
/// shrink the meshed silhouette below the real radius. The cert (`ground_truth`)
/// must report `sound`.
#[test]
fn nurbs_loft_slender_missile_is_watertight() {
    // (z, radius) — tangent-ogive nose → cylindrical body (r=4) → boattail.
    let profile = [
        (0.0, 0.10),
        (2.0, 0.984),
        (4.0, 1.812),
        (6.0, 2.496),
        (8.0, 3.045),
        (10.0, 3.466),
        (12.0, 3.764),
        (14.0, 3.941),
        (16.0, 4.0),
        (33.0, 4.0),
        (50.0, 4.0),
        (55.0, 3.5),
        (60.0, 3.0),
    ];
    let sections: Vec<Vec<Point3>> = profile.iter().map(|&(z, r)| ring(16, r, z)).collect();

    let mut m = BRepModel::new();
    let s = nurbs_loft(&mut m, sections, NurbsLoftOptions::default()).expect("nurbs_loft missile");

    // B-Rep + cert: a valid, watertight, sound NURBS solid.
    assert_nurbs_solid_sound(&m, s, "missile");
    let gt = m.ground_truth(s).expect("missile ground truth");
    assert!(
        gt.certificate.is_sound(),
        "missile: cert NOT sound — {}",
        gt.summary()
    );

    // Body envelope: Ø8 (radius 4) × 60 tall. The whole point of the bug — the
    // mesh must reach the full body radius, not a collapsed Ø6.
    let bbox = m.solid_world_bbox(s).expect("bbox");
    let sz = bbox.size();
    assert!(
        (sz.x - 8.0).abs() < 0.3 && (sz.y - 8.0).abs() < 0.3 && (sz.z - 60.0).abs() < 0.01,
        "missile envelope wrong (expected ~Ø8 × 60): {sz:?}"
    );

    // Display-preset matrix: every quality a viewport / export / preview path can
    // select must tessellate the slender skin watertight AND resolve the true Ø8
    // body. A v/u under-sample (the original defect) shows up here as boundary
    // edges (cap↔lateral cracks) or a meshed diameter short of Ø8.
    let presets: [(&str, TessellationParams); 5] = [
        ("default", TessellationParams::default()),
        ("coarse", TessellationParams::coarse()),
        ("fine", TessellationParams::fine()),
        ("display", TessellationParams::display()),
        ("realtime", TessellationParams::realtime()),
    ];
    for (name, params) in &presets {
        let solid_ref = m.solids.get(s).expect("solid");
        let mesh = tessellate_solid(solid_ref, &m, params);
        assert!(
            !mesh.triangles.is_empty(),
            "missile {name}: empty tessellation"
        );

        // Weld by quantised position and count undirected-edge multiplicities.
        let eps = 1e-6_f64;
        let key = |p: &Point3| {
            (
                (p.x / eps).round() as i64,
                (p.y / eps).round() as i64,
                (p.z / eps).round() as i64,
            )
        };
        let mut weld: std::collections::HashMap<(i64, i64, i64), u32> =
            std::collections::HashMap::new();
        let mut widx: Vec<u32> = Vec::with_capacity(mesh.vertices.len());
        for vtx in &mesh.vertices {
            let k = key(&vtx.position);
            let next = weld.len() as u32;
            widx.push(*weld.entry(k).or_insert(next));
        }
        let mut undirected: std::collections::HashMap<(u32, u32), u32> =
            std::collections::HashMap::new();
        let mut dia = (0.0_f64, 0.0_f64); // (x-extent, y-extent)
        let mut lo = [f64::INFINITY; 2];
        let mut hi = [f64::NEG_INFINITY; 2];
        for vtx in &mesh.vertices {
            lo[0] = lo[0].min(vtx.position.x);
            hi[0] = hi[0].max(vtx.position.x);
            lo[1] = lo[1].min(vtx.position.y);
            hi[1] = hi[1].max(vtx.position.y);
        }
        dia.0 = hi[0] - lo[0];
        dia.1 = hi[1] - lo[1];
        for tri in &mesh.triangles {
            let (a, b, c) = (
                widx[tri[0] as usize],
                widx[tri[1] as usize],
                widx[tri[2] as usize],
            );
            if a == b || b == c || c == a {
                continue;
            }
            for (x, y) in [(a, b), (b, c), (c, a)] {
                let e = if x < y { (x, y) } else { (y, x) };
                *undirected.entry(e).or_insert(0) += 1;
            }
        }
        let boundary = undirected.values().filter(|&&n| n == 1).count();
        let nonmanifold = undirected.values().filter(|&&n| n >= 3).count();
        assert_eq!(
            (boundary, nonmanifold),
            (0, 0),
            "missile {name}: not watertight (boundary_edges={boundary}, nonmanifold={nonmanifold})"
        );
        let meshed_dia = dia.0.max(dia.1);
        assert!(
            meshed_dia >= 7.6,
            "missile {name}: meshed silhouette Ø{meshed_dia:.3} < Ø8 body — skin under-sampled"
        );
    }
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
