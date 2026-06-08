//! DIAGNOSTIC (not a pass/fail gate) for #82: box ∩ sphere face-straddle.
//!
//! Sphere r=1 at (1,0,0) is exactly half inside the box [-1,1]³, so the true
//! intersection is a hemisphere, V = (2/3)π ≈ 2.094. The kernel returns a stable
//! 3.070 — a deterministic *topological* error. This dumps the result's volume,
//! tessellated watertightness, and B-Rep seam structure so the failure mode is
//! visible (open seam? coincident-but-distinct edges on the cut great circle?
//! mismatched arc counts on the two sides of the shared circle?).
//!
//! Run: `cargo test -p geometry-engine --test diag_sphere_poke_82 -- --ignored --nocapture`
//!
//! ROOT-CAUSE MAP (#86 exact-tangent predicates) — two distinct mechanisms,
//! both in the PLANAR face arrangement of the box face cut by the great circle,
//! confirmed by the per-fragment classification trace (ROSHERA_BOOL_TRACE=1):
//!
//!   * NEAR-TANGENT band, r ∈ [0.95, 0.995] (circle inside the face, gap ≤ 0.05
//!     to the box edges) → **FIXED** (commit after `4c900e3`). Was: boolean
//!     ERRORED ("component 0 has only 1 planar face"). The arrangement is in fact
//!     correct (square + interior circle); the bug was in
//!     `compute_split_face_interior_points` — it approximated the cut circle by
//!     its few arc *endpoints* (a coarse inscribed polygon), so the
//!     point-in-polygon containment mis-read the circle-minus-polygon gap as
//!     "outside the hole" and placed the annular face's interior point inside its
//!     own hole → the annulus mis-classified Inside → two coplanar Inside planar
//!     faces → invalid shell. Fix: densify the containment polygons (sample each
//!     arc) + small-first nudge fractions so a THIN annulus still yields an
//!     interior point near the outer boundary. Now exact + watertight; gated in
//!     `curved_boolean_poke_envelope.rs`.
//!
//!   * EXACT-TANGENT, r = 1 (great circle radius = box half-width ⇒ tangent to
//!     all 4 box edges at their midpoints) → wrong volume 3.070, open seam. At
//!     each tangent point the circle's tangent line equals the edge's tangent
//!     line (a degenerate arrangement vertex with collinear incident edges), so
//!     the cell walk keeps a CORNER SLIVER (bounded by 2 box edges + 2 arcs,
//!     incl. a box corner OUTSIDE the sphere) in place of the disk cap. The kept
//!     planar fragment then carries 4 arcs vs the hemisphere's 5 → 5 unweldable
//!     seam edges → non-watertight.
//!
//! Both need robust handling in the planar DCEL arrangement near tangency
//! (degenerate/near-degenerate vertices). General (non-tangent) curved poke is
//! correct + watertight — gated by `tests/curved_boolean_poke_envelope.rs`.

use geometry_engine::harness::brep_integrity::brep_integrity;
use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn build_r(model: &mut BRepModel, r: f64) -> SolidId {
    let bx = match TopologyBuilder::new(model)
        .create_box_3d(2.0, 2.0, 2.0)
        .expect("box")
    {
        GeometryId::Solid(id) => id,
        other => panic!("box: {other:?}"),
    };
    let sp = match TopologyBuilder::new(model)
        .create_sphere_3d(Point3::new(1.0, 0.0, 0.0), r)
        .expect("sphere")
    {
        GeometryId::Solid(id) => id,
        other => panic!("sphere: {other:?}"),
    };
    match boolean_operation(
        model,
        bx,
        sp,
        BooleanOp::Intersection,
        BooleanOptions::default(),
    ) {
        Ok(id) => id,
        Err(e) => panic!("boolean failed: {e:?}"),
    }
}

fn build(model: &mut BRepModel) -> SolidId {
    build_r(model, 1.0)
}

/// Scope: does the failure depend on the degenerate edge-tangency (r=1 ⇒ great
/// circle tangent to all 4 box edges), or do non-tangent radii also fail? The
/// sphere centre sits ON the box face x=1, so exactly half is inside regardless
/// of r: truth = (2/3)π·r³.
#[test]
#[ignore = "diagnostic, not a gate"]
fn diag_box_sphere_poke_radius_sweep() {
    println!("\n=== #82 radius sweep (sphere centre on box face x=1) ===");
    for &r in &[0.9_f64, 0.95, 0.97, 0.98, 0.99, 0.995, 1.0, 1.005, 1.05] {
        let mut model = BRepModel::new();
        let bx = match TopologyBuilder::new(&mut model)
            .create_box_3d(2.0, 2.0, 2.0)
            .expect("box")
        {
            GeometryId::Solid(id) => id,
            o => panic!("{o:?}"),
        };
        let sp = match TopologyBuilder::new(&mut model)
            .create_sphere_3d(Point3::new(1.0, 0.0, 0.0), r)
            .expect("sphere")
        {
            GeometryId::Solid(id) => id,
            o => panic!("{o:?}"),
        };
        let truth = 2.0 / 3.0 * std::f64::consts::PI * r * r * r;
        match boolean_operation(
            &mut model,
            bx,
            sp,
            BooleanOp::Intersection,
            BooleanOptions::default(),
        ) {
            Ok(result) => {
                let vol = model.calculate_solid_volume(result).unwrap_or(f64::NAN);
                let wt = manifold_report(&model, result, 0.05, 1e-6)
                    .map(|m| format!("boundary_e={} closed={}", m.boundary_edges, m.closed))
                    .unwrap_or_else(|| "<empty>".into());
                println!(
                    "  r={r:.3}: vol={vol:.4} truth={truth:.4} err={:+.1}%  {wt}",
                    100.0 * (vol - truth) / truth
                );
            }
            Err(e) => println!("  r={r:.3}: ERROR {e:?}  (truth {truth:.4})"),
        }
    }
    println!("=== end sweep ===\n");
}

#[test]
#[ignore = "trace, run with ROSHERA_BOOL_TRACE=1"]
fn diag_trace_minus_z() {
    let mut model = BRepModel::new();
    let bx = match TopologyBuilder::new(&mut model)
        .create_box_3d(2.0, 2.0, 2.0)
        .expect("box")
    {
        GeometryId::Solid(id) => id,
        o => panic!("{o:?}"),
    };
    let sp = match TopologyBuilder::new(&mut model)
        .create_sphere_3d(Point3::new(0.0, 0.0, -1.0), 0.8)
        .expect("sphere")
    {
        GeometryId::Solid(id) => id,
        o => panic!("{o:?}"),
    };
    let r = boolean_operation(
        &mut model,
        bx,
        sp,
        BooleanOp::Intersection,
        BooleanOptions::default(),
    );
    println!(
        "MINUS-Z RESULT: {:?}",
        r.map(|id| model.calculate_solid_volume(id))
    );
}

#[test]
#[ignore = "trace, run with ROSHERA_BOOL_TRACE=1"]
fn diag_trace_near_tangent_097() {
    // Near-tangent: circle radius 0.97 is entirely inside the box face (gap 0.03
    // to the edges), topologically identical to r=0.8 (which works) — yet errors.
    let mut model = BRepModel::new();
    let bx = match TopologyBuilder::new(&mut model)
        .create_box_3d(2.0, 2.0, 2.0)
        .expect("box")
    {
        GeometryId::Solid(id) => id,
        o => panic!("{o:?}"),
    };
    let sp = match TopologyBuilder::new(&mut model)
        .create_sphere_3d(Point3::new(1.0, 0.0, 0.0), 0.97)
        .expect("sphere")
    {
        GeometryId::Solid(id) => id,
        o => panic!("{o:?}"),
    };
    let r = boolean_operation(
        &mut model,
        bx,
        sp,
        BooleanOp::Intersection,
        BooleanOptions::default(),
    );
    println!(
        "R=0.97 RESULT: {:?}",
        r.map(|id| model.calculate_solid_volume(id))
    );
}

#[test]
#[ignore = "diagnostic, not a gate"]
fn diag_face_axis_sweep() {
    println!("\n=== #85 per-face poke at r=0.8 (non-tangent) ===");
    let faces: [([f64; 3], &str); 6] = [
        ([1.0, 0.0, 0.0], "+x"),
        ([-1.0, 0.0, 0.0], "-x"),
        ([0.0, 1.0, 0.0], "+y"),
        ([0.0, -1.0, 0.0], "-y"),
        ([0.0, 0.0, 1.0], "+z"),
        ([0.0, 0.0, -1.0], "-z"),
    ];
    let truth = 2.0 / 3.0 * std::f64::consts::PI * 0.8 * 0.8 * 0.8;
    for (c, name) in faces {
        let mut model = BRepModel::new();
        let bx = match TopologyBuilder::new(&mut model)
            .create_box_3d(2.0, 2.0, 2.0)
            .expect("box")
        {
            GeometryId::Solid(id) => id,
            o => panic!("{o:?}"),
        };
        let sp = match TopologyBuilder::new(&mut model)
            .create_sphere_3d(Point3::new(c[0], c[1], c[2]), 0.8)
            .expect("sphere")
        {
            GeometryId::Solid(id) => id,
            o => panic!("{o:?}"),
        };
        match boolean_operation(
            &mut model,
            bx,
            sp,
            BooleanOp::Intersection,
            BooleanOptions::default(),
        ) {
            Ok(result) => {
                let vol = model.calculate_solid_volume(result).unwrap_or(f64::NAN);
                let wt = manifold_report(&model, result, 0.05, 1e-6)
                    .map(|m| format!("boundary_e={} closed={}", m.boundary_edges, m.closed))
                    .unwrap_or_else(|| "<empty>".into());
                println!(
                    "  {name} face: vol={vol:.4} truth={truth:.4} err={:+.1}% {wt}",
                    100.0 * (vol - truth) / truth
                );
            }
            Err(e) => println!("  {name} face: ERROR {e:?}"),
        }
    }
    println!("=== end ===\n");
}

#[test]
#[ignore = "diagnostic, not a gate"]
fn diag_box_sphere_poke() {
    let mut model = BRepModel::new();
    let result = build(&mut model);

    let vol = model.calculate_solid_volume(result).unwrap_or(f64::NAN);
    let truth = 2.0 / 3.0 * std::f64::consts::PI;
    println!("\n=== #82 box ∩ sphere(1,0,0; r=1) ===");
    println!(
        "volume   = {vol:.5}   truth = {truth:.5}   (whole sphere = {:.5})",
        4.0 / 3.0 * std::f64::consts::PI
    );

    // Tessellated watertightness.
    if let Some(r) = manifold_report(&model, result, 0.05, 1e-6) {
        println!(
            "mesh: tris={} welded_v={} undirected_e={} boundary_e={} nonmanifold_e={} inconsistent_dir={} components={} euler={} closed={} manifold={} oriented={}",
            r.triangles,
            r.welded_vertices,
            r.undirected_edges,
            r.boundary_edges,
            r.nonmanifold_edges,
            r.inconsistent_directed_edges,
            r.components,
            r.euler_characteristic,
            r.closed,
            r.manifold,
            r.oriented,
        );
    } else {
        println!("mesh: <empty tessellation>");
    }

    // B-Rep structure: where is the seam malformed?
    let rep = brep_integrity(&model, result, 1e-6);
    println!("\n{}", rep.render(&model));

    // Per-face boundary-edge count — exposes a great-circle subdivided into a
    // different arc count on each adjacent face.
    if let Some(solid) = model.solids.get(result) {
        let mut shells = vec![solid.outer_shell];
        shells.extend(solid.inner_shells.iter().copied());
        println!("--- per-face boundary edge counts ---");
        for sid in shells {
            if let Some(shell) = model.shells.get(sid) {
                for &fid in &shell.faces {
                    if let Some(face) = model.faces.get(fid) {
                        let stype = model
                            .surfaces
                            .get(face.surface_id)
                            .map(|s| s.surface_type())
                            .map(|t| format!("{t:?}"))
                            .unwrap_or_else(|| "?".into());
                        let outer = model
                            .loops
                            .get(face.outer_loop)
                            .map(|l| l.edges.len())
                            .unwrap_or(0);
                        let inner: usize = face
                            .inner_loops
                            .iter()
                            .filter_map(|&lid| model.loops.get(lid))
                            .map(|l| l.edges.len())
                            .sum();
                        println!(
                            "  face {fid:?} [{stype}] outer_edges={outer} inner_edges={inner}"
                        );
                        if let Some(lp) = model.loops.get(face.outer_loop) {
                            for &eid in &lp.edges {
                                if let Some(e) = model.edges.get(eid) {
                                    let sp = model.vertices.get_position(e.start_vertex);
                                    let ep = model.vertices.get_position(e.end_vertex);
                                    let f = |o: Option<[f64; 3]>| {
                                        o.map(|p| {
                                            format!("({:+.3},{:+.3},{:+.3})", p[0], p[1], p[2])
                                        })
                                        .unwrap_or_else(|| "?".into())
                                    };
                                    println!("      e{eid:?} {} → {}", f(sp), f(ep));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    // Vertex positions + angle around the cut axis (1,0,0) in the x=1 plane,
    // and how far each vertex sits off the *ideal* great circle (center (1,0,0),
    // radius 1, in plane x=1). A nonzero off-circle gap on a "shared" seam vertex
    // is the coincident-but-distinct-curve failure.
    println!("--- seam vertices (angle θ=atan2(z,y), r=√(y²+z²), x-offset) ---");
    {
        use std::collections::BTreeSet;
        let mut seen: BTreeSet<String> = BTreeSet::new();
        if let Some(solid) = model.solids.get(result) {
            let mut shells = vec![solid.outer_shell];
            shells.extend(solid.inner_shells.iter().copied());
            for sid in shells {
                if let Some(shell) = model.shells.get(sid) {
                    for &fid in &shell.faces {
                        if let Some(face) = model.faces.get(fid) {
                            let mut loops = vec![face.outer_loop];
                            loops.extend(face.inner_loops.iter().copied());
                            for lid in loops {
                                if let Some(lp) = model.loops.get(lid) {
                                    for &eid in &lp.edges {
                                        if let Some(e) = model.edges.get(eid) {
                                            for v in [e.start_vertex, e.end_vertex] {
                                                if let Some(p) = model.vertices.get_position(v) {
                                                    let theta = p[2].atan2(p[1]).to_degrees();
                                                    let r = (p[1] * p[1] + p[2] * p[2]).sqrt();
                                                    let line = format!(
                                                        "  v{v:?} pos=({:+.6},{:+.6},{:+.6}) θ={:+7.2}° r={:.6} x-off={:+.6}",
                                                        p[0], p[1], p[2], theta, r, p[0] - 1.0
                                                    );
                                                    if seen.insert(line.clone()) {
                                                        println!("{line}");
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    println!("=== end ===\n");
    let _ = Vector3::Z;
}
