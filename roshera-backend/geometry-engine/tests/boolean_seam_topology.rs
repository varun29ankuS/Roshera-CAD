// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! **A boolean leaves the cylinder's seam edge inside a removed window.**
//!
//! Cylinder r=10 h=20 minus a box that pierces the wall at `u = 0` (+X, the
//! parametric seam). The removed window straddles the seam, so the seam
//! segment `x=10, y=0, z in [6,14]` lies wholly inside removed material. The
//! kernel kept it: the wall face's outer loop walked it TWICE (once at each
//! end of the period) and the window came back as an inner loop touching it at
//! both ends.
//!
//! ## What "correct" is, and why it is not "the window stays an inner loop"
//!
//! A full-wrap cylinder lateral is an annulus; the B-Rep makes it a disc by
//! carrying the parametric seam as a REAL edge that the outer loop walks
//! twice, once per period end. A hole that stays clear of the seam is an
//! ordinary inner loop. A hole that STRADDLES the seam cannot be: the seam is
//! pinned at `u = 0` by the rim vertices it shares with the cap faces, and the
//! part of it inside the hole does not exist. The only correct resolution that
//! leaves the cap faces alone is the DETOUR -- the outer loop climbs the seam,
//! turns into the window's seam-negative half-chain, comes back to the seam
//! above it, and on the descent takes the seam-positive half-chain. The
//! in-window seam segment then drops out, and the window is no longer an inner
//! loop because it has become part of the outer boundary.
//!
//! The kernel ALREADY does exactly this for a cross bore whose breakout
//! straddles the seam ([`seam_crossing_cross_bore_breakout_is_a_detour`],
//! `split_cylinder_lateral_by_interior_ovals`). It did not for the box-cut
//! window, because `split_cylinder_lateral_by_window` handed the complement
//! fragment the ORIGINAL lateral boundary verbatim, without ever asking
//! whether the window crossed the seam.

use geometry_engine::harness::brep_integrity::brep_integrity;
use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::{
    boolean_operation, transform_solid, BooleanOp, BooleanOptions, TransformOptions,
};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use std::collections::BTreeMap;

const CYL_R: f64 = 10.0;
const CYL_H: f64 = 20.0;
const WIN_HALF: f64 = 3.0;
const WIN_Z_LO: f64 = 6.0;
const WIN_Z_HI: f64 = 14.0;

const BLANK_R: f64 = 43.0;
const BLANK_H: f64 = 70.0;
const BORE_R: f64 = 12.0;
const BORE_Z: f64 = 30.0;

// --- fixtures ---------------------------------------------------------------

fn cylinder(model: &mut BRepModel, base: Point3, axis: Vector3, r: f64, h: f64) -> SolidId {
    match TopologyBuilder::new(model).create_cylinder_3d(base, axis, r, h) {
        Ok(GeometryId::Solid(id)) => id,
        other => panic!("expected a solid; got {other:?}"),
    }
}

/// The box tool. `seam` puts it on +X (across the seam); otherwise +Y.
fn window_tool(model: &mut BRepModel, seam: bool) -> SolidId {
    let (sx, sy) = if seam {
        (30.0, 2.0 * WIN_HALF)
    } else {
        (2.0 * WIN_HALF, 30.0)
    };
    let id = match TopologyBuilder::new(model).create_box_3d(sx, sy, WIN_Z_HI - WIN_Z_LO) {
        Ok(GeometryId::Solid(id)) => id,
        other => panic!("expected a solid; got {other:?}"),
    };
    let t = if seam {
        Vector3::new(15.0, 0.0, 0.5 * (WIN_Z_LO + WIN_Z_HI))
    } else {
        Vector3::new(0.0, 15.0, 0.5 * (WIN_Z_LO + WIN_Z_HI))
    };
    transform_solid(
        model,
        id,
        Matrix4::from_translation(&t),
        TransformOptions::default(),
    )
    .expect("translate the window tool");
    id
}

/// Cylinder minus the box window. Returns (model, solid, wall face id).
fn cut_window(seam: bool) -> (BRepModel, SolidId, u32) {
    let mut model = BRepModel::new();
    let cyl = cylinder(
        &mut model,
        Point3::ORIGIN,
        Vector3::new(0.0, 0.0, 1.0),
        CYL_R,
        CYL_H,
    );
    let tool = window_tool(&mut model, seam);
    let cut = boolean_operation(
        &mut model,
        cyl,
        tool,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("the window difference must not error");
    let wall = sole_wall_face(&model, cut, CYL_R);
    (model, cut, wall)
}

/// Blank minus a through cross bore along `axis`. Returns (model, solid, wall).
fn cut_cross_bore(axis: Vector3, base: Point3) -> (BRepModel, SolidId, u32) {
    let mut model = BRepModel::new();
    let blank = cylinder(
        &mut model,
        Point3::ORIGIN,
        Vector3::new(0.0, 0.0, 1.0),
        BLANK_R,
        BLANK_H,
    );
    let tool = cylinder(&mut model, base, axis, BORE_R, 160.0);
    let bored = boolean_operation(
        &mut model,
        blank,
        tool,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("the cross-bore difference must not error");
    let wall = sole_wall_face(&model, bored, BLANK_R);
    (model, bored, wall)
}

// --- topology readers -------------------------------------------------------

fn face_ids(model: &BRepModel, solid: SolidId) -> Vec<u32> {
    let s = model.solids.get(solid).expect("solid");
    let mut out = Vec::new();
    for sh in [s.outer_shell]
        .into_iter()
        .chain(s.inner_shells.iter().copied())
    {
        if let Some(shell) = model.shells.get(sh) {
            out.extend(shell.faces.iter().copied());
        }
    }
    out
}

/// The one cylindrical face whose radius is `r` -- the outer wall. (A cross
/// bore contributes a second cylindrical face, the bore barrel, at its own
/// radius; keying on the radius keeps the two apart without guessing ids.)
fn sole_wall_face(model: &BRepModel, solid: SolidId, r: f64) -> u32 {
    let walls: Vec<u32> = face_ids(model, solid)
        .into_iter()
        .filter(|&fid| {
            model
                .faces
                .get(fid)
                .and_then(|f| model.surfaces.get(f.surface_id))
                .and_then(|s| {
                    s.as_any()
                        .downcast_ref::<geometry_engine::primitives::surface::Cylinder>()
                        .map(|c| (c.radius - r).abs() <= 1.0e-9)
                })
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(
        walls.len(),
        1,
        "expected exactly one wall face at r={r}, got {walls:?}"
    );
    walls[0]
}

fn loop_edges(model: &BRepModel, lid: u32) -> Vec<(u32, bool)> {
    let lp = model.loops.get(lid).expect("loop");
    lp.edges
        .iter()
        .copied()
        .zip(lp.orientations.iter().copied())
        .collect()
}

fn outer_edges(model: &BRepModel, fid: u32) -> Vec<(u32, bool)> {
    loop_edges(model, model.faces.get(fid).expect("face").outer_loop)
}

fn inner_loop_ids(model: &BRepModel, fid: u32) -> Vec<u32> {
    model.faces.get(fid).expect("face").inner_loops.clone()
}

/// Every edge id referenced by the face (outer + inner) with its use count.
fn edge_use_counts(model: &BRepModel, fid: u32) -> BTreeMap<u32, usize> {
    let f = model.faces.get(fid).expect("face");
    let mut counts: BTreeMap<u32, usize> = BTreeMap::new();
    for lid in [f.outer_loop]
        .into_iter()
        .chain(f.inner_loops.iter().copied())
    {
        for (e, _) in loop_edges(model, lid) {
            *counts.entry(e).or_insert(0) += 1;
        }
    }
    counts
}

fn doubled_edges(model: &BRepModel, fid: u32) -> Vec<u32> {
    edge_use_counts(model, fid)
        .into_iter()
        .filter(|&(_, n)| n > 1)
        .map(|(e, _)| e)
        .collect()
}

/// Sample points strictly inside an edge (endpoints excluded), in 3D.
fn edge_interior_samples(model: &BRepModel, eid: u32) -> Vec<Point3> {
    let e = model.edges.get(eid).expect("edge");
    let curve = model.curves.get(e.curve_id).expect("curve");
    let (t0, t1) = (e.param_range.start, e.param_range.end);
    (1..8)
        .filter_map(|k| curve.point_at(t0 + (t1 - t0) * (k as f64) / 8.0).ok())
        .collect()
}

/// Edges of the face whose INTERIOR lies in material the cut removed.
fn edges_with_interior_in_removed_material(
    model: &BRepModel,
    fid: u32,
    inside_tool: &dyn Fn(Point3) -> bool,
) -> Vec<u32> {
    edge_use_counts(model, fid)
        .into_keys()
        .filter(|&eid| {
            let s = edge_interior_samples(model, eid);
            !s.is_empty() && s.iter().all(|&p| inside_tool(p))
        })
        .collect()
}

/// Strictly inside the box tool (its own faces excluded by a margin).
fn inside_window_box(p: Point3) -> bool {
    let m = 1.0e-6;
    p.x > m
        && p.x < 30.0 - m
        && p.y.abs() < WIN_HALF - m
        && p.z > WIN_Z_LO + m
        && p.z < WIN_Z_HI - m
}

/// Strictly inside the +X cross-bore tool.
fn inside_cross_bore(p: Point3) -> bool {
    let m = 1.0e-6;
    (p.y * p.y + (p.z - BORE_Z) * (p.z - BORE_Z)).sqrt() < BORE_R - m
}

fn describe(model: &BRepModel, edges: &[u32]) -> Vec<String> {
    edges
        .iter()
        .map(|&e| {
            let s = edge_interior_samples(model, e);
            let mid = s.get(s.len() / 2).copied().unwrap_or(Point3::ORIGIN);
            format!("e{e}@({:.3},{:.3},{:.3})", mid.x, mid.y, mid.z)
        })
        .collect()
}

// --- the defect -------------------------------------------------------------

/// RED. The seam segment `x=10, y=0, z in [6,14]` is inside the window; no
/// loop of the wall face may reference it.
#[test]
fn no_wall_edge_lies_inside_the_removed_window() {
    let (model, _cut, wall) = cut_window(true);
    let offenders = edges_with_interior_in_removed_material(&model, wall, &inside_window_box);
    assert!(
        offenders.is_empty(),
        "the wall face still walks {} edge(s) whose interior lies inside the removed window: {:?}",
        offenders.len(),
        describe(&model, &offenders)
    );
}

/// RED. With the in-window seam segment gone, the window's own boundary is
/// spliced into the outer loop and the face has NO inner loop.
#[test]
fn a_seam_straddling_window_is_not_an_inner_loop() {
    let (model, _cut, wall) = cut_window(true);
    let inners = inner_loop_ids(&model, wall);
    let sizes: Vec<usize> = inners
        .iter()
        .map(|&l| loop_edges(&model, l).len())
        .collect();
    assert!(
        inners.is_empty(),
        "a window that straddles the seam cannot be an inner loop -- the seam edge would cross it; got {} inner loop(s) with {sizes:?} edges",
        inners.len()
    );
}

/// RED. The detoured outer loop: 3 bottom rim arcs + (seam-lower, 3 window
/// edges, seam-upper) + 3 top rim arcs + (seam-upper, 3 window edges,
/// seam-lower) = 16 edges.
#[test]
fn the_detoured_outer_loop_has_the_expected_edge_count() {
    let (model, _cut, wall) = cut_window(true);
    let outer = outer_edges(&model, wall);
    assert_eq!(
        outer.len(),
        16,
        "detoured outer loop: expected 16 edges (3 bottom rim + seam ascent through the window's negative half-chain + 3 top rim + seam descent through the positive half-chain); got {outer:?}"
    );
}

/// RED. A full-wrap lateral legitimately walks its seam twice, so "no edge
/// walked twice" is not the contract -- "the doubled set is EXACTLY the
/// surviving seam segments" is. Pre-fix that set is three edges, one of them
/// inside the window.
#[test]
fn the_only_doubled_wall_edges_are_the_surviving_seam_segments() {
    let (model, _cut, wall) = cut_window(true);
    let doubled = doubled_edges(&model, wall);
    assert_eq!(
        doubled.len(),
        2,
        "expected exactly 2 doubled edges (the seam below and above the window); got {:?}",
        describe(&model, &doubled)
    );
    for &e in &doubled {
        let s = edge_interior_samples(&model, e);
        assert!(
            !s.iter().any(|&p| inside_window_box(p)),
            "doubled edge e{e} has interior inside the removed window"
        );
        assert!(
            s.iter()
                .all(|p| (p.x - CYL_R).abs() <= 1.0e-6 && p.y.abs() <= 1.0e-6),
            "doubled edge e{e} is not the seam (x=r, y=0)"
        );
    }
}

/// Structural gate: the solid stays a clean closed manifold. Every window edge
/// must end up used exactly twice overall (once by the wall, once by a planar
/// face of the cut), so the splice must not orphan or duplicate a use.
#[test]
fn the_cut_solid_stays_a_clean_closed_manifold() {
    let (model, cut, _wall) = cut_window(true);
    let r = brep_integrity(&model, cut, 1.0e-6);
    assert!(
        r.is_clean(),
        "the seam-window cut is not structurally clean:\n{}",
        r.render(&model)
    );
    assert!(
        r.orientation_inconsistent_edges.is_empty(),
        "orientation flips across shared edges: {:?}",
        r.orientation_inconsistent_edges
    );
}

// --- controls: these pass BEFORE and AFTER, and that is their job -----------

/// CONTROL. The same window a quarter turn away never meets the seam, so it
/// stays an ordinary inner loop and the outer loop keeps its whole seam. If the
/// fix widened past the straddling case this moves.
#[test]
fn an_off_seam_window_stays_a_clean_inner_loop() {
    let (model, cut, wall) = cut_window(false);
    let inners = inner_loop_ids(&model, wall);
    assert_eq!(inners.len(), 1, "off-seam window must be ONE inner loop");
    assert_eq!(
        loop_edges(&model, inners[0]).len(),
        4,
        "the off-seam window loop is 2 cap arcs + 2 wall lines"
    );
    assert_eq!(
        outer_edges(&model, wall).len(),
        8,
        "off-seam: 3 bottom rim arcs + seam + 3 top rim arcs + seam"
    );
    assert_eq!(
        doubled_edges(&model, wall).len(),
        1,
        "off-seam: the one unbroken seam edge is the only doubled edge"
    );
    let r = brep_integrity(&model, cut, 1.0e-6);
    assert!(r.is_clean(), "off-seam control:\n{}", r.render(&model));
}

/// PREMISE CORRECTION, and the guard on the shared detour builder.
///
/// Task 33 recorded that a +X cross bore's seam-side breakout is "SPLICED INTO
/// THE OUTER LOOP (eats the seam edge)" and Task 34's brief inherited that as a
/// defect to fix. It is not a defect: it is the correct B-Rep, and it is the
/// very construction this task transplants onto the box-cut window. Measured on
/// the live kernel, before any change here: the +X wall's outer loop is 14
/// edges, the seam segment between the two breakout crossings (z in [18,42]) is
/// absent, and the two doubled edges are the seam BELOW and ABOVE the bore.
///
/// This test passed before the fix and must pass after it -- it is what makes
/// extracting the detour builder out of
/// `split_cylinder_lateral_by_interior_ovals` a measured move rather than a
/// hopeful one.
#[test]
fn seam_crossing_cross_bore_breakout_is_a_detour() {
    let (model, _bored, wall) =
        cut_cross_bore(Vector3::new(1.0, 0.0, 0.0), Point3::new(-80.0, 0.0, BORE_Z));
    assert_eq!(
        outer_edges(&model, wall).len(),
        14,
        "the +X wall's outer loop detours around the seam-side breakout"
    );
    assert_eq!(
        inner_loop_ids(&model, wall).len(),
        1,
        "only the FAR (u = pi) breakout remains an inner loop"
    );
    let offenders = edges_with_interior_in_removed_material(&model, wall, &inside_cross_bore);
    assert!(
        offenders.is_empty(),
        "the +X wall walks edge(s) whose interior lies inside the bore: {:?}",
        describe(&model, &offenders)
    );
    let doubled = doubled_edges(&model, wall);
    assert_eq!(
        doubled.len(),
        2,
        "the seam below and above the breakout are the only doubled edges; got {:?}",
        describe(&model, &doubled)
    );
}

/// CONTROL for the above: a +Y cross bore breaks out at u = pi/2 and 3pi/2,
/// nowhere near the seam, so BOTH breakouts are inner loops and the seam is one
/// unbroken doubled edge.
#[test]
fn off_seam_cross_bore_breakouts_are_inner_loops() {
    let (model, _bored, wall) =
        cut_cross_bore(Vector3::new(0.0, 1.0, 0.0), Point3::new(0.0, -80.0, BORE_Z));
    assert_eq!(outer_edges(&model, wall).len(), 8);
    assert_eq!(inner_loop_ids(&model, wall).len(), 2);
    assert_eq!(doubled_edges(&model, wall).len(), 1);
}
