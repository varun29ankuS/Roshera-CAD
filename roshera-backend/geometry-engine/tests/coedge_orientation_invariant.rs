// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Co-edge orientation invariant — in a closed 2-manifold shell every edge is
//! used by exactly two co-edges, and those two co-edges must traverse it in
//! OPPOSITE stored senses (face A walks `v8 -> v9`, face B walks `v9 -> v8`).
//!
//! `validate_shell_closure` counts edge USES only — a same-sense pairing has
//! use-count 2 and passes. `check_face_orientations` judges orientation
//! GEOMETRICALLY (outward normal x tangent), deliberately ignoring the stored
//! loop senses. Neither sees a stored same-sense pairing. This file measures
//! that gap across the kernel's producers AND pins it closed where the
//! producer now guarantees it (boolean output — calibrated at the loop-
//! minting seam by `orient_minted_walk_material_left` in
//! `operations/boolean.rs`; nurbs loft — cap ring senses fixed to
//! Newell-forward; primitives and extrude/revolve/chamfer already
//! conformed).
//!
//! ## Which sense the invariant is stated in
//!
//! The kernel's convention (measured, 2026-08-05, see the survey below) is
//! that the STORED walk is CCW about the SURFACE normal, and
//! `FaceOrientation` maps the surface normal to the outward normal. The
//! manifold invariant therefore holds in the EFFECTIVE sense — stored sense
//! XOR `FaceOrientation::Backward` — and that is what these tests assert.
//! (An extrude's bottom cap stores forward walks with a `Backward` face; the
//! raw stored flags alone would false-positive it.)
//!
//! ## Known, measured gap: the fillet blend face (NOT hidden by these tests)
//!
//! The fillet blend face's loop senses do NOT conform, and — MEASURED, not
//! taken from the comment (2026-08-05, `fillet_blend_walk_is_weld_load_
//! bearing`) — they CANNOT: the blend tessellation welds seams by sample
//! order derived from the loop walk, so forcing opposition by whole-loop
//! reversal tears the verified mesh (boundary=4, inconsistent_directed=79,
//! euler 2→0 on every nonconforming edge). The `check_face_orientations`
//! doc's claim is therefore TRUE for fillet. It was FALSE for the nurbs
//! loft: the same reversal there left every mesh oracle and the volume
//! unchanged, so the loft producer was FIXED instead (both cap rings now
//! store forward — CCW about their Newell plane normal by construction).
//!
//! The correct statement of the invariant, one every face can satisfy:
//! two manifold-adjacent faces traverse a shared edge in opposite
//! directions of their OUTWARD-oriented boundary. For every producer whose
//! stored walks encode orientation (primitives, extrude, revolve, chamfer,
//! boolean output, nurbs loft) that surfaces as stored-effective co-edge
//! opposition — asserted here as a producer contract. For the fillet blend
//! face the stored walk encodes the weld contour instead; its orientation
//! truth lives in `FaceOrientation` + the geometric outward-walk arm of
//! `check_face_orientations` + the welded-mesh directed-edge oracle.
//! Consequently the stored-sense conjunct is NOT wired into the validity
//! certificate / `is_sound()` — doing so would red every filleted solid,
//! and scoping it by producer would be special-casing. The tripwire test
//! re-measures the blocker on every run and goes red the day the blocker
//! is gone, naming the follow-up: calibrate fillet, then wire the
//! certificate.
//!
//! ## Survey semantics (measurement, not assertion)
//!
//! `blast_radius_survey` classifies every edge of every fixture shell into:
//!   * `opposed`     — two co-edges, opposite stored senses (the invariant),
//!   * `same_sense`  — two co-edges, SAME stored sense (the defect),
//!   * `boundary`    — one co-edge (open/unmerged in stored topology),
//!   * `overused`    — more than two co-edges,
//!   * `seam_ok` / `seam_bad` — both co-edges inside one loop (periodic seam).
//!
//! It prints a per-fixture table and PASSES — the measurement itself must not
//! turn the suite red while the fillet/loft producers remain nonconforming
//! (`KNOWN_REDS.md` is empty; a red here would block every merge without
//! teaching anything the table does not).

use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::chamfer::{chamfer_edges, ChamferOptions};
use geometry_engine::operations::extrude::{extrude_profile, ExtrudeOptions};
use geometry_engine::operations::fillet::{fillet_edges, FilletOptions};
use geometry_engine::operations::nurbs_loft::{nurbs_loft, NurbsLoftOptions};
use geometry_engine::operations::revolve::{revolve_profile, RevolveOptions};
use geometry_engine::operations::{
    boolean_operation, transform_solid, BooleanOp, BooleanOptions, TransformOptions,
};
use geometry_engine::primitives::curve::Line;
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use std::f64::consts::PI;

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

/// One edge's co-edge pairing verdict within a shell.
#[derive(Debug, Clone, PartialEq, Eq)]
enum EdgePairing {
    /// Two co-edges from (usually) two faces, opposite stored senses.
    Opposed,
    /// Two co-edges, SAME stored sense — the malformation under test.
    SameSense { faces: (FaceId, FaceId) },
    /// A single co-edge: boundary in stored topology.
    Boundary,
    /// More than two co-edges.
    Overused { count: usize },
    /// Both co-edges inside ONE loop (periodic self-seam), opposite senses.
    SeamOk,
    /// Both co-edges inside ONE loop, same sense.
    SeamBad,
}

#[derive(Debug, Default)]
struct CoedgeReport {
    opposed: usize,
    same_sense: Vec<(EdgeId, FaceId, FaceId)>,
    /// Same pairing judged on the EFFECTIVE walk: stored sense XOR
    /// (`FaceOrientation::Reversed` on the owning face). This is the classic
    /// B-Rep convention (loop CCW wrt the SURFACE normal; the face flag maps
    /// surface normal to outward normal, and with it the walk direction).
    eff_opposed: usize,
    eff_same_sense: Vec<(EdgeId, FaceId, FaceId)>,
    reversed_faces: usize,
    boundary: Vec<EdgeId>,
    overused: Vec<(EdgeId, usize)>,
    seam_ok: usize,
    seam_bad: Vec<EdgeId>,
    faces: usize,
    degenerate_loops: usize,
}

impl CoedgeReport {
    fn clean(&self) -> bool {
        self.same_sense.is_empty() && self.seam_bad.is_empty() && self.overused.is_empty()
    }
    fn eff_clean(&self) -> bool {
        self.eff_same_sense.is_empty() && self.seam_bad.is_empty() && self.overused.is_empty()
    }
    fn one_line(&self) -> String {
        format!(
            "faces={:3}(rev {:2})  opposed={:4}  SAME_SENSE={:3}  EFF_SAME={:3}  boundary={:3}  overused={:2}  seam_ok={:2}  seam_bad={:2}  degen={}",
            self.faces,
            self.reversed_faces,
            self.opposed,
            self.same_sense.len(),
            self.eff_same_sense.len(),
            self.boundary.len(),
            self.overused.len(),
            self.seam_ok,
            self.seam_bad.len(),
            self.degenerate_loops,
        )
    }
}

/// Classify every edge of every shell of `solid_id` by its stored co-edge
/// senses. Pure read of the stored topology — no geometry, no tessellation.
fn coedge_report(model: &BRepModel, solid_id: SolidId) -> CoedgeReport {
    let mut report = CoedgeReport::default();
    let Some(solid) = model.solids.get(solid_id) else {
        return report;
    };
    let mut shells = vec![solid.outer_shell];
    shells.extend(solid.inner_shells.iter().copied());

    for shell_id in shells {
        let Some(shell) = model.shells.get(shell_id) else {
            continue;
        };
        // edge -> occurrences of (face, loop, stored sense, effective sense)
        let mut uses: std::collections::BTreeMap<
            EdgeId,
            Vec<(
                FaceId,
                geometry_engine::primitives::r#loop::LoopId,
                bool,
                bool,
            )>,
        > = std::collections::BTreeMap::new();

        for &face_id in &shell.faces {
            let Some(face) = model.faces.get(face_id) else {
                continue;
            };
            report.faces += 1;
            let face_reversed = matches!(
                face.orientation,
                geometry_engine::primitives::face::FaceOrientation::Backward
            );
            if face_reversed {
                report.reversed_faces += 1;
            }
            let mut loops = vec![face.outer_loop];
            loops.extend(face.inner_loops.iter().copied());
            for loop_id in loops {
                let Some(loop_data) = model.loops.get(loop_id) else {
                    continue;
                };
                if loop_data.edges.is_empty() {
                    report.degenerate_loops += 1;
                    continue;
                }
                for (i, &edge_id) in loop_data.edges.iter().enumerate() {
                    let sense = loop_data.orientations.get(i).copied().unwrap_or(true);
                    uses.entry(edge_id).or_default().push((
                        face_id,
                        loop_id,
                        sense,
                        sense ^ face_reversed,
                    ));
                }
            }
        }

        for (edge_id, occ) in uses {
            let verdict = match occ.len() {
                1 => EdgePairing::Boundary,
                2 => {
                    let (f1, l1, s1, _) = occ[0];
                    let (f2, l2, s2, _) = occ[1];
                    let same_loop = l1 == l2;
                    match (same_loop, s1 == s2) {
                        (true, false) => EdgePairing::SeamOk,
                        (true, true) => EdgePairing::SeamBad,
                        (false, false) => EdgePairing::Opposed,
                        (false, true) => EdgePairing::SameSense { faces: (f1, f2) },
                    }
                }
                n => EdgePairing::Overused { count: n },
            };
            // Effective-walk pairing (only meaningful for 2-use, cross-loop edges).
            if occ.len() == 2 && occ[0].1 != occ[1].1 {
                if occ[0].3 == occ[1].3 {
                    report.eff_same_sense.push((edge_id, occ[0].0, occ[1].0));
                } else {
                    report.eff_opposed += 1;
                }
            }
            match verdict {
                EdgePairing::Opposed => report.opposed += 1,
                EdgePairing::SameSense { faces } => {
                    report.same_sense.push((edge_id, faces.0, faces.1))
                }
                EdgePairing::Boundary => report.boundary.push(edge_id),
                EdgePairing::Overused { count } => report.overused.push((edge_id, count)),
                EdgePairing::SeamOk => report.seam_ok += 1,
                EdgePairing::SeamBad => report.seam_bad.push(edge_id),
            }
        }
    }
    report
}

// ---------------------------------------------------------------------------
// Fixture builders
// ---------------------------------------------------------------------------

/// Closed CCW polygon ring extruded along +Z.
fn extrude_ring(model: &mut BRepModel, ring: &[(f64, f64)], height: f64) -> SolidId {
    let verts: Vec<_> = ring
        .iter()
        .map(|&(x, y)| model.vertices.add(x, y, 0.0))
        .collect();
    let n = verts.len();
    let mut edges = Vec::with_capacity(n);
    for i in 0..n {
        let a = verts[i];
        let b = verts[(i + 1) % n];
        let pa = model.vertices.get(a).expect("va").position;
        let pb = model.vertices.get(b).expect("vb").position;
        let line = Line::new(
            Point3::new(pa[0], pa[1], pa[2]),
            Point3::new(pb[0], pb[1], pb[2]),
        );
        let cid = model.curves.add(Box::new(line));
        edges.push(
            model
                .edges
                .add(Edge::new_auto_range(0, a, b, cid, EdgeOrientation::Forward)),
        );
    }
    extrude_profile(
        model,
        edges,
        ExtrudeOptions {
            direction: Vector3::Z,
            distance: height,
            cap_ends: true,
            ..Default::default()
        },
    )
    .expect("extrusion")
}

fn square_ring(half: f64) -> Vec<(f64, f64)> {
    vec![(-half, -half), (half, -half), (half, half), (-half, half)]
}

/// A vertical edge (endpoints differing in z) of the model, chosen
/// DETERMINISTICALLY: the one whose midpoint is lexicographically smallest
/// in (x, y). Store iteration order is not a fixture parameter.
fn first_vertical_edge(model: &BRepModel) -> EdgeId {
    let mut best: Option<(f64, f64, EdgeId)> = None;
    for (edge_id, edge) in model.edges.iter() {
        let (Some(a), Some(b)) = (
            model.vertices.get(edge.start_vertex),
            model.vertices.get(edge.end_vertex),
        ) else {
            continue;
        };
        if (a.position[2] - b.position[2]).abs() > 1e-9 {
            let mx = (a.position[0] + b.position[0]) * 0.5;
            let my = (a.position[1] + b.position[1]) * 0.5;
            let better = match best {
                None => true,
                Some((bx, by, _)) => (mx, my) < (bx, by),
            };
            if better {
                best = Some((mx, my, edge_id));
            }
        }
    }
    best.map(|(_, _, e)| e).expect("no vertical edge found")
}

fn boxes_overlapping(model: &mut BRepModel) -> (SolidId, SolidId) {
    let a = sid(TopologyBuilder::new(model)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("box a"));
    let b = sid(TopologyBuilder::new(model)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("box b"));
    transform_solid(
        model,
        b,
        Matrix4::translation(10.0, 10.0, 10.0),
        TransformOptions::default(),
    )
    .expect("translate box b");
    (a, b)
}

/// Sphere r=10 with centre translated to `centre`, and a 20^3 box at origin.
fn sphere_and_box(model: &mut BRepModel, centre: Point3) -> (SolidId, SolidId) {
    let box_id = sid(TopologyBuilder::new(model)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("box"));
    let sphere_id = sid(TopologyBuilder::new(model)
        .create_sphere_3d(Point3::ZERO, 10.0)
        .expect("sphere"));
    transform_solid(
        model,
        sphere_id,
        Matrix4::translation(centre.x, centre.y, centre.z),
        TransformOptions::default(),
    )
    .expect("translate sphere");
    (box_id, sphere_id)
}

// ---------------------------------------------------------------------------
// The invariant, asserted
// ---------------------------------------------------------------------------

/// Assert the co-edge orientation invariant on `solid`: every manifold edge's
/// two co-edges traverse it in OPPOSITE effective senses (stored sense XOR
/// `FaceOrientation::Backward`), periodic self-seams oppose within their
/// loop, and no edge carries more than two co-edges. Violations are named
/// edge-by-edge with both offending faces.
fn assert_coedge_opposition(model: &BRepModel, solid: SolidId, label: &str) {
    let report = coedge_report(model, solid);
    let problems = coedge_problems(&report);
    assert!(
        problems.is_empty(),
        "{label}: stored co-edge orientation malformed:\n  {}",
        problems.join("\n  ")
    );
}

/// Human-readable violation list for a report (used both by the assertion
/// and by the mutation test, which needs to inspect the message itself).
fn coedge_problems(report: &CoedgeReport) -> Vec<String> {
    let mut problems: Vec<String> = Vec::new();
    for (e, f1, f2) in &report.eff_same_sense {
        problems.push(format!(
            "edge {e}: faces {f1} and {f2} traverse it in the SAME effective sense \
             (a closed 2-manifold requires opposite co-edge directions)"
        ));
    }
    for e in &report.seam_bad {
        problems.push(format!(
            "edge {e}: periodic self-seam traversed twice in the same sense"
        ));
    }
    for (e, n) in &report.overused {
        problems.push(format!(
            "edge {e}: used by {n} co-edges (manifold max is 2)"
        ));
    }
    problems
}

/// The reported defect, RED before the producer fix: off-centre sphere ∩ box
/// yields a 2-face shell whose three shared circle arcs were walked in the
/// SAME sense by both faces (`v8→v9→v10→v8`, flags `[true,true,true]` on
/// both sides).
#[test]
fn boolean_sphere_box_offcentre_intersection_coedges_opposed() {
    let mut model = BRepModel::new();
    let (box_id, sphere_id) = sphere_and_box(&mut model, Point3::new(14.0, 0.0, 0.0));
    let r = boolean_operation(
        &mut model,
        box_id,
        sphere_id,
        BooleanOp::Intersection,
        BooleanOptions::default(),
    )
    .expect("sphere ∩ box");
    assert_coedge_opposition(&model, r, "sphere(14,0,0) ∩ box");
}

/// Same circle-splitter path, union keep-set — also same-sense before the fix.
#[test]
fn boolean_box_union_sphere_coedges_opposed() {
    let mut model = BRepModel::new();
    let (box_id, sphere_id) = sphere_and_box(&mut model, Point3::new(14.0, 0.0, 0.0));
    let r = boolean_operation(
        &mut model,
        box_id,
        sphere_id,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("box ∪ sphere");
    assert_coedge_opposition(&model, r, "box ∪ sphere(14,0,0)");
}

/// Difference across the same seam (effective-sense violation before the fix:
/// the flipped cap face kept an uncalibrated walk).
#[test]
fn boolean_box_difference_sphere_coedges_opposed() {
    let mut model = BRepModel::new();
    let (box_id, sphere_id) = sphere_and_box(&mut model, Point3::new(14.0, 0.0, 0.0));
    let r = boolean_operation(
        &mut model,
        box_id,
        sphere_id,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("box − sphere");
    assert_coedge_opposition(&model, r, "box − sphere(14,0,0)");
}

/// The great-circle pose (sphere centre exactly on the cut plane) — the pose
/// the trim-tangent membership fix must serve. Pinned so the producer stays
/// trustworthy exactly where `spherical_circular_membership`'s centre
/// reference degenerates.
#[test]
fn boolean_sphere_box_great_circle_intersection_coedges_opposed() {
    let mut model = BRepModel::new();
    let (box_id, sphere_id) = sphere_and_box(&mut model, Point3::new(10.0, 0.0, 0.0));
    let r = boolean_operation(
        &mut model,
        box_id,
        sphere_id,
        BooleanOp::Intersection,
        BooleanOptions::default(),
    )
    .expect("sphere ∩ box (great circle)");
    assert_coedge_opposition(&model, r, "sphere(10,0,0) ∩ box");
}

/// Planar box/box booleans across all three ops — already conforming before
/// the fix; pinned so the calibration cannot regress them.
#[test]
fn boolean_box_box_all_ops_coedges_opposed() {
    for op in [
        BooleanOp::Union,
        BooleanOp::Intersection,
        BooleanOp::Difference,
    ] {
        let mut model = BRepModel::new();
        let (a, b) = boxes_overlapping(&mut model);
        let r = boolean_operation(&mut model, a, b, op, BooleanOptions::default())
            .unwrap_or_else(|e| panic!("box {op:?} box failed: {e:?}"));
        assert_coedge_opposition(&model, r, &format!("box {op:?} box"));
    }
}

/// Curved lateral + seam edge through a boolean.
#[test]
fn boolean_cylinder_box_difference_coedges_opposed() {
    let mut model = BRepModel::new();
    let cyl = sid(TopologyBuilder::new(&mut model)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -5.0), Vector3::Z, 6.0, 10.0)
        .expect("cylinder"));
    let b = sid(TopologyBuilder::new(&mut model)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("box"));
    transform_solid(
        &mut model,
        b,
        Matrix4::translation(0.0, 0.0, 8.0),
        TransformOptions::default(),
    )
    .expect("translate box");
    let r = boolean_operation(
        &mut model,
        cyl,
        b,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("cylinder − box");
    assert_coedge_opposition(&model, r, "cylinder − box");
}

/// A primitive passes, so the invariant (and the check) is not
/// boolean-specific.
#[test]
fn primitive_box_coedges_opposed() {
    let mut model = BRepModel::new();
    let s = sid(TopologyBuilder::new(&mut model)
        .create_box_3d(10.0, 10.0, 10.0)
        .expect("box"));
    assert_coedge_opposition(&model, s, "primitive box");
}

/// A blend result passes too (chamfer conforms to the effective-sense
/// convention; fillet's blend face does NOT — that gap is measured and
/// documented in `blast_radius_survey`, not hidden here).
#[test]
fn chamfer_result_coedges_opposed() {
    let mut model = BRepModel::new();
    let s = extrude_ring(&mut model, &square_ring(5.0), 8.0);
    let e = first_vertical_edge(&model);
    chamfer_edges(
        &mut model,
        s,
        vec![e],
        ChamferOptions {
            distance1: 1.0,
            distance2: 1.0,
            symmetric: true,
            ..Default::default()
        },
    )
    .expect("chamfer");
    assert_coedge_opposition(&model, s, "chamfer 1 edge of prism");
}

/// A nurbs loft passes: the cap ring traversed FORWARD is CCW about the cap
/// plane's Newell normal by construction, so both cap loops store `true` and
/// oppose the lateral's effective walk. RED before the `nurbs_loft.rs` fix
/// (the bottom cap hard-coded `false`, colliding with the lateral on the
/// bottom ring for every section winding).
#[test]
fn nurbs_loft_result_coedges_opposed() {
    let mut model = BRepModel::new();
    let section = |z: f64, half: f64| -> Vec<Point3> {
        vec![
            Point3::new(-half, -half, z),
            Point3::new(half, -half, z),
            Point3::new(half, half, z),
            Point3::new(-half, half, z),
        ]
    };
    let s = nurbs_loft(
        &mut model,
        vec![section(0.0, 5.0), section(6.0, 3.0)],
        NurbsLoftOptions::default(),
    )
    .expect("loft");
    assert_coedge_opposition(&model, s, "nurbs loft square->square");

    // The same invariant must hold for the REVERSED section winding: the
    // Newell normals flip with the winding, and the conforming stored sense
    // (forward) is winding-independent.
    let mut model = BRepModel::new();
    let section_cw = |z: f64, half: f64| -> Vec<Point3> {
        vec![
            Point3::new(-half, half, z),
            Point3::new(half, half, z),
            Point3::new(half, -half, z),
            Point3::new(-half, -half, z),
        ]
    };
    let s = nurbs_loft(
        &mut model,
        vec![section_cw(0.0, 5.0), section_cw(6.0, 3.0)],
        NurbsLoftOptions::default(),
    )
    .expect("loft cw");
    assert_coedge_opposition(&model, s, "nurbs loft (clockwise sections)");
}

/// Mutation proof: deliberately flip ONE co-edge's stored sense on a sound
/// box and confirm the check goes red NAMING that edge and both incident
/// faces; restore the sense and confirm the check is clean again.
#[test]
fn mutation_flipped_coedge_sense_is_named() {
    let mut model = BRepModel::new();
    let s = sid(TopologyBuilder::new(&mut model)
        .create_box_3d(10.0, 10.0, 10.0)
        .expect("box"));
    assert_coedge_opposition(&model, s, "pristine box");

    // Pick the first edge of the first face's outer loop and flip its sense.
    let solid = model.solids.get(s).expect("solid");
    let shell = model.shells.get(solid.outer_shell).expect("shell");
    let face_id = *shell.faces.first().expect("box has faces");
    let loop_id = model.faces.get(face_id).expect("face").outer_loop;
    let (edge_id, original_sense) = {
        let lp = model.loops.get(loop_id).expect("loop");
        (lp.edges[0], lp.orientations[0])
    };
    // The box is manifold: this edge has exactly one other incident face.
    let other_face = {
        let mut other = None;
        for &fid in &shell.faces {
            if fid == face_id {
                continue;
            }
            let f = model.faces.get(fid).expect("face");
            let lp = model.loops.get(f.outer_loop).expect("loop");
            if lp.edges.contains(&edge_id) {
                other = Some(fid);
                break;
            }
        }
        other.expect("manifold edge has a second face")
    };

    {
        let lp = model.loops.get_mut(loop_id).expect("loop");
        lp.orientations[0] = !original_sense;
    }
    let report = coedge_report(&model, s);
    let problems = coedge_problems(&report);
    assert!(
        !problems.is_empty(),
        "flipping a co-edge sense must be detected"
    );
    let msg = problems.join("\n");
    println!("mutation RED message:\n{msg}");
    assert!(
        msg.contains(&format!("edge {edge_id}")),
        "violation must name the flipped edge {edge_id}: {msg}"
    );
    assert!(
        msg.contains(&format!("{face_id}")) && msg.contains(&format!("{other_face}")),
        "violation must name both faces {face_id} and {other_face}: {msg}"
    );

    // Restore and confirm clean.
    {
        let lp = model.loops.get_mut(loop_id).expect("loop");
        lp.orientations[0] = original_sense;
    }
    assert_coedge_opposition(&model, s, "box after restoring the flipped sense");
}

// ---------------------------------------------------------------------------
// MEASUREMENT: does forcing co-edge opposition on blend/loft loops actually
// tear the welded tessellation, as the `check_face_orientations` doc claims?
// ---------------------------------------------------------------------------

/// Snapshot of every oracle we can consult around a loop mutation.
struct OracleSnapshot {
    coedge_clean: bool,
    eff_same: usize,
    mesh_valid: bool,
    boundary_edges: usize,
    nonmanifold_edges: usize,
    inconsistent_directed: usize,
    euler: i64,
    components: usize,
    volume: f64,
    orientation_errors: usize,
}

fn snapshot(model: &mut BRepModel, s: SolidId) -> OracleSnapshot {
    use geometry_engine::harness::watertight::manifold_report;
    use geometry_engine::primitives::validation::check_face_orientations;

    let report = coedge_report(model, s);
    let solid = model.solids.get(s).expect("solid");
    let shell = solid.outer_shell;
    let mr = manifold_report(model, s, 0.05, 1e-6).expect("mesh");
    let orientation_errors = check_face_orientations(model, shell).len();
    let volume = model.calculate_solid_volume(s).expect("volume");
    OracleSnapshot {
        coedge_clean: report.eff_clean(),
        eff_same: report.eff_same_sense.len(),
        mesh_valid: mr.is_valid_solid(),
        boundary_edges: mr.boundary_edges,
        nonmanifold_edges: mr.nonmanifold_edges,
        inconsistent_directed: mr.inconsistent_directed_edges,
        euler: mr.euler_characteristic,
        components: mr.components,
        volume,
        orientation_errors,
    }
}

fn print_snapshot(label: &str, s: &OracleSnapshot) {
    println!(
        "{label}: coedge_clean={} (eff_same={}), mesh_valid={}, boundary={}, nonmanifold={}, \
         inconsistent_directed={}, euler={}, components={}, volume={:.6}, orientation_errors={}",
        s.coedge_clean,
        s.eff_same,
        s.mesh_valid,
        s.boundary_edges,
        s.nonmanifold_edges,
        s.inconsistent_directed,
        s.euler,
        s.components,
        s.volume,
        s.orientation_errors
    );
}

/// Greedily reverse whole outer loops (the face most often on a same-sense
/// pairing first) until the co-edge report is clean or no reversal helps.
/// Returns the faces reversed. Whole-loop reversal preserves chain closure,
/// so this is exactly the mutation a conforming producer would have made.
fn force_coedge_opposition(model: &mut BRepModel, s: SolidId) -> Vec<FaceId> {
    let mut reversed: Vec<FaceId> = Vec::new();
    let mut blacklist: Vec<FaceId> = Vec::new();
    for _ in 0..8 {
        let report = coedge_report(model, s);
        if report.eff_same_sense.is_empty() {
            break;
        }
        let mut counts: std::collections::BTreeMap<FaceId, usize> =
            std::collections::BTreeMap::new();
        for &(_, f1, f2) in &report.eff_same_sense {
            *counts.entry(f1).or_default() += 1;
            *counts.entry(f2).or_default() += 1;
        }
        let Some((&face, _)) = counts
            .iter()
            .filter(|(f, _)| !blacklist.contains(f))
            .max_by_key(|(_, &c)| c)
        else {
            break;
        };
        let loop_id = model.faces.get(face).expect("face").outer_loop;
        let before = report.eff_same_sense.len();
        model.loops.get_mut(loop_id).expect("loop").reverse();
        let after = coedge_report(model, s).eff_same_sense.len();
        if after < before {
            reversed.push(face);
        } else {
            // Not an improvement: revert and never try this face again.
            model.loops.get_mut(loop_id).expect("loop").reverse();
            blacklist.push(face);
        }
    }
    reversed
}

/// MEASURED TRIPWIRE: the fillet blend face's stored loop walk is genuinely
/// LOAD-BEARING for the tessellation weld — forcing co-edge opposition by
/// whole-loop reversal makes the stored topology conform but TEARS the
/// welded mesh (measured 2026-08-05: boundary=4, inconsistent_directed=79,
/// euler 2→0 on every nonconforming edge of this prism). This is why the
/// co-edge invariant is NOT wired into the validity certificate: a fillet
/// result cannot satisfy it without reworking the blend contour/weld
/// contract to derive sample order independently of loop sense.
///
/// The claim originates in the `check_face_orientations` doc; this test
/// exists because a documented behaviour is not automatically a correct one
/// — it RE-MEASURES the claim on every run. If it ever goes red, one of two
/// things happened, and both demand action:
///
///   * a dirty edge's baseline mesh stopped being valid (fillet regression), or
///   * reversal stopped tearing the mesh / fillet started conforming — the
///     blocker is gone: calibrate the fillet blend loops and WIRE the
///     co-edge invariant into the certificate (see the module doc).
///
/// (The nurbs-loft half of the same 2026-08-05 measurement went the other
/// way — reversal was weld-safe, every oracle unchanged — so its producer
/// was fixed instead: `nurbs_loft_result_coedges_opposed` pins it.)
#[test]
fn fillet_blend_walk_is_weld_load_bearing() {
    println!();
    println!("=== forcing co-edge opposition on the fillet blend face ===");
    let mut dirty_edges = 0usize;

    // --- fillet each vertical edge of a square prism, separately: the
    // nonconformance is EDGE-DEPENDENT (the survey saw eff_same 0..4 vary
    // with which edge the nondeterministic picker chose), so sweep them all.
    for edge_index in 0..4 {
        let mut model = BRepModel::new();
        let s = extrude_ring(&mut model, &square_ring(5.0), 8.0);
        let mut vertical: Vec<(f64, f64, geometry_engine::primitives::edge::EdgeId)> = Vec::new();
        for (edge_id, edge) in model.edges.iter() {
            let (Some(a), Some(b)) = (
                model.vertices.get(edge.start_vertex),
                model.vertices.get(edge.end_vertex),
            ) else {
                continue;
            };
            if (a.position[2] - b.position[2]).abs() > 1e-9 {
                let mx = (a.position[0] + b.position[0]) * 0.5;
                let my = (a.position[1] + b.position[1]) * 0.5;
                vertical.push((mx, my, edge_id));
            }
        }
        vertical.sort_by(|p, q| (p.0, p.1).partial_cmp(&(q.0, q.1)).expect("finite"));
        let e = vertical[edge_index].2;
        fillet_edges(
            &mut model,
            s,
            vec![e],
            FilletOptions {
                radius: 1.0,
                ..Default::default()
            },
        )
        .expect("fillet");
        let before = snapshot(&mut model, s);
        print_snapshot(&format!("fillet edge#{edge_index} BEFORE"), &before);
        assert!(
            before.mesh_valid,
            "fillet edge#{edge_index}: baseline mesh must be a valid closed oriented manifold"
        );
        if before.eff_same == 0 {
            // This edge's blend conforms natively (the CylindricalFillet
            // frame sign-flip is edge-dependent); nothing to force.
            continue;
        }
        dirty_edges += 1;
        let flipped = force_coedge_opposition(&mut model, s);
        println!("fillet edge#{edge_index} reversed outer loops of faces: {flipped:?}");
        let after = snapshot(&mut model, s);
        print_snapshot(&format!("fillet edge#{edge_index} AFTER "), &after);
        assert!(
            after.coedge_clean,
            "fillet edge#{edge_index}: whole-loop reversal must restore co-edge opposition"
        );
        assert!(
            !after.mesh_valid,
            "fillet edge#{edge_index}: reversal NO LONGER tears the welded mesh — the measured \
             blocker is gone. Calibrate the fillet blend loops and wire the co-edge invariant \
             into the certificate (see module doc)."
        );
    }
    assert!(
        dirty_edges > 0,
        "no fillet edge produced a same-sense pairing — fillet now conforms natively; wire the \
         co-edge invariant into the certificate (see module doc)."
    );

    // --- all four vertical edges at once (blend-blend interactions) ---
    {
        let mut model = BRepModel::new();
        let s = extrude_ring(&mut model, &square_ring(5.0), 8.0);
        let mut es: Vec<geometry_engine::primitives::edge::EdgeId> = Vec::new();
        for (edge_id, edge) in model.edges.iter() {
            let (Some(a), Some(b)) = (
                model.vertices.get(edge.start_vertex),
                model.vertices.get(edge.end_vertex),
            ) else {
                continue;
            };
            if (a.position[2] - b.position[2]).abs() > 1e-9 {
                es.push(edge_id);
            }
        }
        match fillet_edges(
            &mut model,
            s,
            es,
            FilletOptions {
                radius: 1.0,
                ..Default::default()
            },
        ) {
            Ok(_) => {
                let before = snapshot(&mut model, s);
                print_snapshot("fillet 4-edges BEFORE", &before);
                let flipped = force_coedge_opposition(&mut model, s);
                println!("fillet 4-edges reversed outer loops of faces: {flipped:?}");
                let after = snapshot(&mut model, s);
                print_snapshot("fillet 4-edges AFTER ", &after);
            }
            // Pre-existing kernel behaviour, unrelated to this measurement:
            // the all-4-vertical-edge fillet of this prism is REFUSED by
            // fillet_edges' own post-validation. Report, don't fail the sweep.
            Err(e) => println!("fillet 4-edges REFUSED by fillet_edges: {e:?}"),
        }
    }

    println!("=== end measurement ===");
}

// ---------------------------------------------------------------------------
// The survey
// ---------------------------------------------------------------------------

#[test]
fn blast_radius_survey() {
    let mut rows: Vec<(String, CoedgeReport)> = Vec::new();

    // --- primitives ---
    {
        let mut model = BRepModel::new();
        let s = sid(TopologyBuilder::new(&mut model)
            .create_box_3d(10.0, 10.0, 10.0)
            .expect("box"));
        rows.push(("primitive box".into(), coedge_report(&model, s)));
    }
    {
        let mut model = BRepModel::new();
        let s = sid(TopologyBuilder::new(&mut model)
            .create_cylinder_3d(Point3::ZERO, Vector3::Z, 5.0, 10.0)
            .expect("cylinder"));
        rows.push(("primitive cylinder".into(), coedge_report(&model, s)));
    }
    {
        let mut model = BRepModel::new();
        let s = sid(TopologyBuilder::new(&mut model)
            .create_sphere_3d(Point3::ZERO, 5.0)
            .expect("sphere"));
        rows.push(("primitive sphere".into(), coedge_report(&model, s)));
    }

    // --- extrude ---
    {
        let mut model = BRepModel::new();
        let s = extrude_ring(&mut model, &square_ring(5.0), 8.0);
        rows.push(("extrude square prism".into(), coedge_report(&model, s)));
    }
    {
        let mut model = BRepModel::new();
        let ring: Vec<(f64, f64)> = (0..5)
            .map(|i| {
                let t = 2.0 * PI * (i as f64) / 5.0;
                (4.0 * t.cos(), 4.0 * t.sin())
            })
            .collect();
        let s = extrude_ring(&mut model, &ring, 6.0);
        rows.push(("extrude pentagon prism".into(), coedge_report(&model, s)));
    }

    // --- revolve: rectangle in the XZ plane about the Z axis, full turn ---
    {
        let mut model = BRepModel::new();
        let pts = [(2.0, 0.0), (5.0, 0.0), (5.0, 4.0), (2.0, 4.0)];
        let verts: Vec<_> = pts
            .iter()
            .map(|&(x, z)| model.vertices.add(x, 0.0, z))
            .collect();
        let mut edges = Vec::new();
        for i in 0..verts.len() {
            let a = verts[i];
            let b = verts[(i + 1) % verts.len()];
            let pa = model.vertices.get(a).expect("va").position;
            let pb = model.vertices.get(b).expect("vb").position;
            let line = Line::new(
                Point3::new(pa[0], pa[1], pa[2]),
                Point3::new(pb[0], pb[1], pb[2]),
            );
            let cid = model.curves.add(Box::new(line));
            edges.push(model.edges.add(Edge::new_auto_range(
                0,
                a,
                b,
                cid,
                EdgeOrientation::Forward,
            )));
        }
        match revolve_profile(
            &mut model,
            edges,
            RevolveOptions {
                axis_origin: Point3::ZERO,
                axis_direction: Vector3::Z,
                angle: 2.0 * PI,
                ..Default::default()
            },
        ) {
            Ok(s) => rows.push((
                "revolve annulus (full turn)".into(),
                coedge_report(&model, s),
            )),
            Err(e) => rows.push((
                format!("revolve annulus FAILED: {e:?}"),
                CoedgeReport::default(),
            )),
        }
    }

    // --- loft: two squares -> nurbs solid ---
    {
        let mut model = BRepModel::new();
        let section = |z: f64, half: f64| -> Vec<Point3> {
            vec![
                Point3::new(-half, -half, z),
                Point3::new(half, -half, z),
                Point3::new(half, half, z),
                Point3::new(-half, half, z),
            ]
        };
        match nurbs_loft(
            &mut model,
            vec![section(0.0, 5.0), section(6.0, 3.0)],
            NurbsLoftOptions::default(),
        ) {
            Ok(s) => rows.push(("nurbs loft square->square".into(), coedge_report(&model, s))),
            Err(e) => rows.push((format!("nurbs loft FAILED: {e:?}"), CoedgeReport::default())),
        }
    }

    // --- fillet / chamfer on one vertical edge of a square prism ---
    {
        let mut model = BRepModel::new();
        let s = extrude_ring(&mut model, &square_ring(5.0), 8.0);
        let e = first_vertical_edge(&model);
        match fillet_edges(
            &mut model,
            s,
            vec![e],
            FilletOptions {
                radius: 1.0,
                ..Default::default()
            },
        ) {
            Ok(_) => rows.push(("fillet 1 edge of prism".into(), coedge_report(&model, s))),
            Err(e) => rows.push((format!("fillet FAILED: {e:?}"), CoedgeReport::default())),
        }
    }
    {
        let mut model = BRepModel::new();
        let s = extrude_ring(&mut model, &square_ring(5.0), 8.0);
        let e = first_vertical_edge(&model);
        match chamfer_edges(
            &mut model,
            s,
            vec![e],
            ChamferOptions {
                distance1: 1.0,
                distance2: 1.0,
                symmetric: true,
                ..Default::default()
            },
        ) {
            Ok(_) => rows.push(("chamfer 1 edge of prism".into(), coedge_report(&model, s))),
            Err(e) => rows.push((format!("chamfer FAILED: {e:?}"), CoedgeReport::default())),
        }
    }

    // --- booleans: box/box, all three ops ---
    for op in [
        BooleanOp::Union,
        BooleanOp::Intersection,
        BooleanOp::Difference,
    ] {
        let mut model = BRepModel::new();
        let (a, b) = boxes_overlapping(&mut model);
        match boolean_operation(&mut model, a, b, op, BooleanOptions::default()) {
            Ok(r) => rows.push((format!("boolean box {op:?} box"), coedge_report(&model, r))),
            Err(e) => rows.push((
                format!("boolean box {op:?} box FAILED: {e:?}"),
                CoedgeReport::default(),
            )),
        }
    }

    // --- booleans: the observed sphere cases ---
    // Off-centre pose: sphere centre (14,0,0) — the reported 2-face shell.
    {
        let mut model = BRepModel::new();
        let (box_id, sphere_id) = sphere_and_box(&mut model, Point3::new(14.0, 0.0, 0.0));
        match boolean_operation(
            &mut model,
            box_id,
            sphere_id,
            BooleanOp::Intersection,
            BooleanOptions::default(),
        ) {
            Ok(r) => rows.push((
                "boolean sphere(14,0,0) ∩ box".into(),
                coedge_report(&model, r),
            )),
            Err(e) => rows.push((
                format!("sphere∩box off-centre FAILED: {e:?}"),
                CoedgeReport::default(),
            )),
        }
    }
    // Great-circle pose: sphere centre (10,0,0) exactly on the +X face plane.
    {
        let mut model = BRepModel::new();
        let (box_id, sphere_id) = sphere_and_box(&mut model, Point3::new(10.0, 0.0, 0.0));
        match boolean_operation(
            &mut model,
            box_id,
            sphere_id,
            BooleanOp::Intersection,
            BooleanOptions::default(),
        ) {
            Ok(r) => rows.push((
                "boolean sphere(10,0,0) ∩ box".into(),
                coedge_report(&model, r),
            )),
            Err(e) => rows.push((
                format!("sphere∩box great-circle FAILED: {e:?}"),
                CoedgeReport::default(),
            )),
        }
    }
    // Sphere ∪ box and box − sphere (off-centre pose).
    for op in [BooleanOp::Union, BooleanOp::Difference] {
        let mut model = BRepModel::new();
        let (box_id, sphere_id) = sphere_and_box(&mut model, Point3::new(14.0, 0.0, 0.0));
        match boolean_operation(&mut model, box_id, sphere_id, op, BooleanOptions::default()) {
            Ok(r) => rows.push((
                format!("boolean box {op:?} sphere(14,0,0)"),
                coedge_report(&model, r),
            )),
            Err(e) => rows.push((
                format!("box {op:?} sphere FAILED: {e:?}"),
                CoedgeReport::default(),
            )),
        }
    }

    // --- boolean: cylinder − box (curved lateral involved) ---
    {
        let mut model = BRepModel::new();
        let cyl = sid(TopologyBuilder::new(&mut model)
            .create_cylinder_3d(Point3::new(0.0, 0.0, -5.0), Vector3::Z, 6.0, 10.0)
            .expect("cylinder"));
        let b = sid(TopologyBuilder::new(&mut model)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("box"));
        transform_solid(
            &mut model,
            b,
            Matrix4::translation(0.0, 0.0, 8.0),
            TransformOptions::default(),
        )
        .expect("translate box");
        match boolean_operation(
            &mut model,
            cyl,
            b,
            BooleanOp::Difference,
            BooleanOptions::default(),
        ) {
            Ok(r) => rows.push(("boolean cylinder − box".into(), coedge_report(&model, r))),
            Err(e) => rows.push((format!("cyl − box FAILED: {e:?}"), CoedgeReport::default())),
        }
    }

    println!();
    println!("=== co-edge stored-sense pairing survey ===");
    for (name, report) in &rows {
        println!("{name:38} | {}", report.one_line());
        if !report.same_sense.is_empty() {
            for (e, f1, f2) in report.same_sense.iter().take(8) {
                println!("    raw same-sense edge {e} between faces {f1} and {f2}");
            }
            if report.same_sense.len() > 8 {
                println!("    ... and {} more", report.same_sense.len() - 8);
            }
        }
        if !report.eff_same_sense.is_empty() {
            for (e, f1, f2) in report.eff_same_sense.iter().take(8) {
                println!("    EFFECTIVE same-sense edge {e} between faces {f1} and {f2}");
            }
            if report.eff_same_sense.len() > 8 {
                println!("    ... and {} more", report.eff_same_sense.len() - 8);
            }
        }
    }
    println!("=== end survey ===");
    let dirty: Vec<&str> = rows
        .iter()
        .filter(|(_, r)| !r.clean())
        .map(|(n, _)| n.as_str())
        .collect();
    println!(
        "fixtures with RAW stored-sense defects: {}/{}: {:?}",
        dirty.len(),
        rows.len(),
        dirty
    );
    let eff_dirty: Vec<&str> = rows
        .iter()
        .filter(|(_, r)| !r.eff_clean())
        .map(|(n, _)| n.as_str())
        .collect();
    println!(
        "fixtures with EFFECTIVE-walk defects: {}/{}: {:?}",
        eff_dirty.len(),
        rows.len(),
        eff_dirty
    );
}
