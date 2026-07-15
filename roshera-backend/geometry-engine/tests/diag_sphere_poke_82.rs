// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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
//!     all 4 box edges at their midpoints) → **FIXED**. Was: wrong volume 3.070,
//!     open seam. `find_curve_curve_intersections` reported each tangent TOUCH
//!     point (distance 0) as an intersection, so the circle was split at the 4
//!     touch points and each box edge at its midpoint, fracturing the clean
//!     interior-loop arrangement into degenerate vertices (circle tangent line =
//!     edge tangent line). Fix: reject TANGENTIAL contacts in
//!     `compute_edge_intersections` — a tangency does not separate a face into
//!     cells, so only transversal crossings split. The circle then behaves as the
//!     clean interior loop that r just-under-1 already handles. Now exact +
//!     watertight; gated to r=1.0 in `curved_boolean_poke_envelope.rs`.
//!
//! REMAINING (open): BEYOND-tangent r > 1 (circle radius > box half-width ⇒ it
//! genuinely CROSSES the box edges, and the sphere is clipped by the adjacent
//! box faces). This is a multi-face transversal clip, NOT a tangency — a
//! different, more general problem. General curved poke r ∈ [0..1] is correct +
//! watertight, gated by `tests/curved_boolean_poke_envelope.rs`.

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

/// Grid-integrated truth for box[-1,1]³ ∩ sphere(centre, r): count cell centres
/// that lie inside BOTH. Because the integration domain IS the box, box
/// membership is implicit and EVERY box face clips the sphere correctly — so
/// this is valid for r > box-half-width (the sphere bulging past the host face's
/// edges, clipped by the adjacent faces), where the closed-form (2/3)π·r³ is an
/// over-estimate. N=240 cells/axis ⇒ ~1.4e7 samples, ~0.1% accurate.
fn box_sphere_grid_truth(centre: [f64; 3], r: f64) -> f64 {
    const N: usize = 240;
    let cell = 2.0 / N as f64;
    let cell_vol = cell * cell * cell;
    let r2 = r * r;
    let mut count = 0u64;
    for i in 0..N {
        let x = -1.0 + (i as f64 + 0.5) * cell;
        let dx = x - centre[0];
        for j in 0..N {
            let y = -1.0 + (j as f64 + 0.5) * cell;
            let dy = y - centre[1];
            for k in 0..N {
                let z = -1.0 + (k as f64 + 0.5) * cell;
                let dz = z - centre[2];
                if dx * dx + dy * dy + dz * dz <= r2 {
                    count += 1;
                }
            }
        }
    }
    count as f64 * cell_vol
}

/// Scope sweep: sphere centre sits ON the box face x=1. Truth is the GRID
/// oracle (valid for r>1, where the sphere bulges past the face edges and is
/// clipped by the adjacent box faces — the closed-form (2/3)π·r³ over-estimates
/// there). #82/#86 (r ≤ 1) are now fixed; this characterises the open #88
/// (r > 1 multi-face clip).
#[test]
#[ignore = "diagnostic, not a gate"]
fn diag_box_sphere_poke_radius_sweep() {
    println!("\n=== box∩sphere radius sweep (centre on box face x=1) ===");
    for &r in &[0.9_f64, 0.95, 1.0, 1.02, 1.05, 1.1, 1.2, 1.3, 1.41] {
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
        let truth = box_sphere_grid_truth([1.0, 0.0, 0.0], r);
        let analytic = 2.0 / 3.0 * std::f64::consts::PI * r * r * r;
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
                    "  r={r:.3}: vol={vol:.4} grid_truth={truth:.4} (½sphere={analytic:.4}) err={:+.1}%  {wt}",
                    100.0 * (vol - truth) / truth
                );
            }
            Err(e) => println!("  r={r:.3}: ERROR {e:?}  (grid_truth {truth:.4})"),
        }
    }
    println!("=== end sweep ===\n");
}

/// #88: dump the box∩sphere(r=1.05) result face structure. Expected for r>1:
/// the +x cap is a squircle (great circle clipped by the 4 box edges), the
/// sphere pokes through the 4 SIDE faces (±y,±z) in a small circle each → 4 disk
/// caps, plus the clipped spherical surface.
///
/// FINDING (2026-06-08): all 6 expected faces ARE produced — 4 side-face disk
/// caps (±y,±z), the +x squircle cap (10 edges), and the spherical surface
/// (11 edges). The great-circle arcs on +x DO weld (sphere shares 6 arcs with
/// the squircle). Non-watertight (boundary_e≈149, euler=-2; 9 B-Rep edges used
/// once) from TWO distinct sub-bugs, both in the SPHERE multi-circle split:
///
///   A. POLAR CAP NOT CLIPPED. The sphere's +y pole (1, 1.05, 0) is OUTSIDE the
///      box, yet sphere edges run out to it (…→(1,1.05,0)→…), so the sphere face
///      keeps the bulge poking past the y=1 face instead of being clipped to it.
///      The other 3 sides (z=±1, y=-1) ARE clipped (seam arcs sit on the box
///      faces). `split_sphere_face_by_circles` mishandles the cut circle that
///      encloses a parametric pole — the polar cap escapes the side-face clip.
///   B. COINCIDENT-BUT-DISTINCT SEAM ARCS. Where the sphere IS clipped, its seam
///      arc and the matching side-cap arc are SEPARATE edges on the same circle
///      (sphere e69↔cap e12 on -z, e66↔e16 on +z, e76↔e20 on -y) — never welded,
///      so each is used once. The cross-face weld (canonicalise/heal) doesn't
///      unify a planar-cap arc with the coincident sphere-surface arc.
///
/// Fix order: A first (correct the sphere's clipped boundary so all 4 side seams
/// are real), then B (weld the sphere↔cap coincident arcs). Substantial,
/// regression-sensitive (touches `split_sphere_face_by_circles` + the weld stage,
/// both used by every sphere Boolean) — budget the full boolean-suite verify.
#[test]
#[ignore = "diagnostic, not a gate"]
fn diag_dump_r105() {
    let mut model = BRepModel::new();
    let result = build_r(&mut model, 1.05);
    let truth = box_sphere_grid_truth([1.0, 0.0, 0.0], 1.05);
    let vol = model.calculate_solid_volume(result).unwrap_or(f64::NAN);
    println!("\n=== #88 box∩sphere r=1.05 ===");
    println!(
        "vol={vol:.4} grid_truth={truth:.4} err={:+.1}%",
        100.0 * (vol - truth) / truth
    );
    if let Some(m) = manifold_report(&model, result, 0.04, 1e-6) {
        println!(
            "mesh: boundary_e={} nonmanifold_e={} closed={} euler={}",
            m.boundary_edges, m.nonmanifold_edges, m.closed, m.euler_characteristic
        );
    }
    if let Some(solid) = model.solids.get(result) {
        let mut shells = vec![solid.outer_shell];
        shells.extend(solid.inner_shells.iter().copied());
        let mut by_type: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for sid in shells {
            if let Some(shell) = model.shells.get(sid) {
                for &fid in &shell.faces {
                    if let Some(face) = model.faces.get(fid) {
                        let stype = model
                            .surfaces
                            .get(face.surface_id)
                            .map(|s| format!("{:?}", s.surface_type()))
                            .unwrap_or_else(|| "?".into());
                        let outer = model
                            .loops
                            .get(face.outer_loop)
                            .map(|l| l.edges.len())
                            .unwrap_or(0);
                        // Centroid-ish: average of edge start vertices.
                        let c = face_center(&model, fid);
                        println!("  face {fid:?} [{stype}] outer_edges={outer} center=({:+.2},{:+.2},{:+.2})", c[0], c[1], c[2]);
                        if let Some(lp) = model.loops.get(face.outer_loop) {
                            for &eid in &lp.edges {
                                if let Some(e) = model.edges.get(eid) {
                                    let s = model.vertices.get_position(e.start_vertex);
                                    let en = model.vertices.get_position(e.end_vertex);
                                    let f = |o: Option<[f64; 3]>| {
                                        o.map(|p| {
                                            format!("({:+.3},{:+.3},{:+.3})", p[0], p[1], p[2])
                                        })
                                        .unwrap_or_else(|| "?".into())
                                    };
                                    println!("      e{eid:?} {} → {}", f(s), f(en));
                                }
                            }
                        }
                        *by_type.entry(stype).or_default() += 1;
                    }
                }
            }
        }
        println!("face counts by type: {by_type:?}");
    }
    println!("--- brep integrity ---");
    println!("{}", brep_integrity(&model, result, 1e-6).render(&model));
    println!("=== end ===\n");
}

fn face_center(model: &BRepModel, fid: geometry_engine::primitives::face::FaceId) -> [f64; 3] {
    let (mut sx, mut sy, mut sz, mut n) = (0.0, 0.0, 0.0, 0.0);
    if let Some(face) = model.faces.get(fid) {
        if let Some(lp) = model.loops.get(face.outer_loop) {
            for &eid in &lp.edges {
                if let Some(e) = model.edges.get(eid) {
                    if let Some(p) = model.vertices.get_position(e.start_vertex) {
                        sx += p[0];
                        sy += p[1];
                        sz += p[2];
                        n += 1.0;
                    }
                }
            }
        }
    }
    if n == 0.0 {
        [f64::NAN; 3]
    } else {
        [sx / n, sy / n, sz / n]
    }
}

/// #89: box ∪ sphere at a corner. Survey says kernel=7.80 < box(8.0) — union is
/// REMOVING the box's corner cut-bits but NOT ADDING the sphere's external bulge.
/// Dump the result faces + (with ROSHERA_BOOL_TRACE=1) the classify/select
/// stages to confirm the sphere's outside-box fragments are dropped.
#[test]
#[ignore = "diagnostic, run with ROSHERA_BOOL_TRACE=1 --nocapture"]
fn diag_union_corner() {
    let mut model = BRepModel::new();
    let bx = match TopologyBuilder::new(&mut model)
        .create_box_3d(2.0, 2.0, 2.0)
        .expect("box")
    {
        GeometryId::Solid(id) => id,
        o => panic!("{o:?}"),
    };
    let sp = match TopologyBuilder::new(&mut model)
        .create_sphere_3d(Point3::new(1.0, 1.0, 1.0), 0.5)
        .expect("sphere")
    {
        GeometryId::Solid(id) => id,
        o => panic!("{o:?}"),
    };
    let res = boolean_operation(
        &mut model,
        bx,
        sp,
        BooleanOp::Union,
        BooleanOptions::default(),
    );
    println!("\n=== #89 box ∪ sphere(1,1,1; r=0.5) ===");
    match res {
        Ok(result) => {
            let vol = model.calculate_solid_volume(result).unwrap_or(f64::NAN);
            println!("vol={vol:.4}  (box=8.0, truth≈8.48 = box + protruding octant ≈0.48)");
            if let Some(solid) = model.solids.get(result) {
                let mut shells = vec![solid.outer_shell];
                shells.extend(solid.inner_shells.iter().copied());
                let mut by_type: std::collections::BTreeMap<String, usize> =
                    std::collections::BTreeMap::new();
                for sid in shells {
                    if let Some(shell) = model.shells.get(sid) {
                        for &fid in &shell.faces {
                            if let Some(face) = model.faces.get(fid) {
                                let st = model
                                    .surfaces
                                    .get(face.surface_id)
                                    .map(|s| format!("{:?}", s.surface_type()))
                                    .unwrap_or_else(|| "?".into());
                                let c = face_center(&model, fid);
                                println!(
                                    "  face {fid:?} [{st}] center=({:+.2},{:+.2},{:+.2})",
                                    c[0], c[1], c[2]
                                );
                                *by_type.entry(st).or_default() += 1;
                            }
                        }
                    }
                }
                println!("face counts by type: {by_type:?}  (expect 6 Plane + 1 Sphere for a corner bulge)");
            }
        }
        Err(e) => println!("ERROR {e:?}"),
    }
    println!("=== end ===\n");
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
