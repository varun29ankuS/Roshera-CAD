//! Boolean broad-phase ablation — the harness applied beyond CD (HARNESS-β).
//!
//! A boolean (union / intersection / difference) only has to intersect the
//! face-pairs whose bounding boxes overlap; the rest of the `n_a × n_b` raw
//! pairs are spatially disjoint and contribute nothing. This study measures that
//! broad-phase funnel — raw face-pairs → AABB-overlapping pairs — so the
//! culling's contribution is a number, exactly as [`crate::harness::cd`] does for
//! contact determination.
//!
//! The cull is *sound by construction*: a face's AABB contains the face, so two
//! faces that share any point necessarily have overlapping AABBs — the cull can
//! never drop a pair the boolean needs. The harness verifies this on a known
//! coincident pair rather than asserting it.

use crate::harness::{AblationReport, StageMetric};
use crate::math::bbox::BBox;
use crate::math::vector3::Vector3;
use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;

/// Measure the boolean broad-phase between two solids: how many of the raw
/// face-pairs survive AABB-overlap culling (the pairs a boolean must actually
/// intersect). `retained_witness`, when `Some((fa, fb))`, is a face-pair known to
/// be coincident; the report is marked verified iff that pair survives the cull
/// (soundness — the broad phase never drops a real intersection).
pub fn boolean_broad_phase_ablation(
    model: &BRepModel,
    solid_a: SolidId,
    solid_b: SolidId,
    retained_witness: Option<(FaceId, FaceId)>,
) -> AblationReport {
    let faces_a = solid_face_ids(model, solid_a);
    let faces_b = solid_face_ids(model, solid_b);
    let aabbs_a: Vec<Option<BBox>> = faces_a.iter().map(|&f| face_aabb(model, f)).collect();
    let aabbs_b: Vec<Option<BBox>> = faces_b.iter().map(|&f| face_aabb(model, f)).collect();

    let raw = faces_a.len() * faces_b.len();
    let mut overlapping = 0usize;
    let mut cost = 0u64;
    for ba in &aabbs_a {
        for bb in &aabbs_b {
            cost += 1;
            if let (Some(x), Some(y)) = (ba, bb) {
                if x.intersects(y) {
                    overlapping += 1;
                }
            }
        }
    }

    let mut report = AblationReport::new("boolean broad-phase (face-pair AABB cull)")
        .stage(StageMetric::new("raw_face_pairs", raw, raw, 0))
        .stage(StageMetric::new("aabb_overlap", raw, overlapping, cost));

    if let Some((fa, fb)) = retained_witness {
        let retained = match (face_aabb(model, fa), face_aabb(model, fb)) {
            (Some(x), Some(y)) => x.intersects(&y),
            _ => false,
        };
        report = report.verified(retained);
    }
    report
}

fn face_aabb(model: &BRepModel, face_id: FaceId) -> Option<BBox> {
    let face = model.faces.get(face_id)?;
    let mut pts = Vec::new();
    let mut loop_ids = vec![face.outer_loop];
    loop_ids.extend(face.inner_loops.iter().copied());
    for lid in loop_ids {
        if let Some(lp) = model.loops.get(lid) {
            for &eid in &lp.edges {
                if let Some(edge) = model.edges.get(eid) {
                    for vid in [edge.start_vertex, edge.end_vertex] {
                        if let Some(v) = model.vertices.get(vid) {
                            pts.push(Vector3::new(v.position[0], v.position[1], v.position[2]));
                        }
                    }
                }
            }
        }
    }
    BBox::from_points(&pts)
}

fn solid_face_ids(model: &BRepModel, solid_id: SolidId) -> Vec<FaceId> {
    let mut out = Vec::new();
    let Some(solid) = model.solids.get(solid_id) else {
        return out;
    };
    let mut shell_ids = vec![solid.outer_shell];
    shell_ids.extend(solid.inner_shells.iter().copied());
    for sid in shell_ids {
        if let Some(shell) = model.shells.get(sid) {
            out.extend(shell.faces.iter().copied());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Boolean CORRECTNESS harness — the algebraic invariants every boolean must
// satisfy, checked by volume. This is the iron-clad core: a violation is a hard
// over/under-inclusion bug, not a performance question.
// ---------------------------------------------------------------------------

/// Operand and result volumes of a boolean scene, plus a verdict on each
/// algebraic law. A correct boolean satisfies all of them for *any* two solids.
#[derive(Debug, Clone)]
pub struct BooleanInvariants {
    pub vol_a: Option<f64>,
    pub vol_b: Option<f64>,
    pub vol_union: Option<f64>,
    pub vol_intersection: Option<f64>,
    pub vol_difference: Option<f64>,
    /// `V(A∪B) + V(A∩B) == V(A) + V(B)` — the master inclusion–exclusion law.
    pub inclusion_exclusion: bool,
    /// `V(A∖B) == V(A) − V(A∩B)`.
    pub difference_consistent: bool,
    /// `max(V(A),V(B)) ≤ V(A∪B) ≤ V(A) + V(B)`.
    pub union_bounded: bool,
    /// `0 ≤ V(A∩B) ≤ min(V(A),V(B))`.
    pub intersection_bounded: bool,
    /// Every invariant held and every volume was computable.
    pub all_hold: bool,
}

/// Check the boolean algebraic invariants for a scene. `build` constructs the two
/// operand solids in a fresh model; it is called once per measurement so a
/// boolean that mutates its inputs cannot corrupt a later one. `rel_tol` is the
/// relative volume tolerance.
pub fn check_boolean_invariants<F>(build: F, rel_tol: f64) -> BooleanInvariants
where
    F: Fn(&mut BRepModel) -> Option<(SolidId, SolidId)>,
{
    let operand = |take_a: bool| -> Option<f64> {
        let mut model = BRepModel::new();
        let (a, b) = build(&mut model)?;
        model.calculate_solid_volume(if take_a { a } else { b })
    };
    let run = |op: BooleanOp| -> Option<f64> {
        let mut model = BRepModel::new();
        let (a, b) = build(&mut model)?;
        let result = boolean_operation(&mut model, a, b, op, BooleanOptions::default()).ok()?;
        model.calculate_solid_volume(result)
    };

    let vol_a = operand(true);
    let vol_b = operand(false);
    let vol_union = run(BooleanOp::Union);
    let vol_intersection = run(BooleanOp::Intersection);
    let vol_difference = run(BooleanOp::Difference);

    let inclusion_exclusion = match (vol_a, vol_b, vol_union, vol_intersection) {
        (Some(a), Some(b), Some(u), Some(i)) => within_rel(u + i, a + b, rel_tol),
        _ => false,
    };
    let difference_consistent = match (vol_a, vol_intersection, vol_difference) {
        (Some(a), Some(i), Some(d)) => within_rel(d, a - i, rel_tol),
        _ => false,
    };
    let union_bounded = match (vol_a, vol_b, vol_union) {
        (Some(a), Some(b), Some(u)) => {
            u >= a.max(b) * (1.0 - rel_tol) - 1e-9 && u <= (a + b) * (1.0 + rel_tol) + 1e-9
        }
        _ => false,
    };
    let intersection_bounded = match (vol_a, vol_b, vol_intersection) {
        (Some(a), Some(b), Some(i)) => i >= -1e-9 && i <= a.min(b) * (1.0 + rel_tol) + 1e-9,
        _ => false,
    };
    let all_hold =
        inclusion_exclusion && difference_consistent && union_bounded && intersection_bounded;

    BooleanInvariants {
        vol_a,
        vol_b,
        vol_union,
        vol_intersection,
        vol_difference,
        inclusion_exclusion,
        difference_consistent,
        union_bounded,
        intersection_bounded,
        all_hold,
    }
}

/// Relative-difference check with a `max(_, 1.0)` floor against tiny-volume false
/// alarms.
fn within_rel(a: f64, b: f64, tol: f64) -> bool {
    let scale = a.abs().max(b.abs()).max(1.0);
    (a - b).abs() / scale <= tol
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::surface::Plane;
    use crate::primitives::topology_builder::TopologyBuilder;

    const X: Vector3 = Vector3::X;

    // ---- independent Monte-Carlo volume oracle (diagnostic) --------------
    // Tests a boolean RESULT against the ORIGINAL operands by point sampling:
    // p ∈ A∩B ⟺ in(A) ∧ in(B), etc. This is independent of mass-props AND of
    // the result B-Rep, so it catches over/under-inclusion that the all-mass-
    // props inclusion-exclusion identity can mask.

    /// Analytic MC truth from explicit in-A / in-B predicates (independent of
    /// the kernel entirely). Returns (V_union, V_intersection, V_difference).
    fn mc_truth_analytic<A, B>(in_a: A, in_b: B, half: f64, n: usize) -> (f64, f64, f64)
    where
        A: Fn(f64, f64, f64) -> bool,
        B: Fn(f64, f64, f64) -> bool,
    {
        let lo = -half;
        let span = 2.0 * half;
        let cell = span / n as f64;
        let off = cell * 0.4365; // off the face planes
        let (mut u, mut i, mut d) = (0usize, 0usize, 0usize);
        for ix in 0..n {
            for iy in 0..n {
                for iz in 0..n {
                    let (x, y, z) = (
                        lo + off + ix as f64 * cell,
                        lo + off + iy as f64 * cell,
                        lo + off + iz as f64 * cell,
                    );
                    let (ina, inb) = (in_a(x, y, z), in_b(x, y, z));
                    if ina || inb {
                        u += 1;
                    }
                    if ina && inb {
                        i += 1;
                    }
                    if ina && !inb {
                        d += 1;
                    }
                }
            }
        }
        let cellv = cell * cell * cell;
        (u as f64 * cellv, i as f64 * cellv, d as f64 * cellv)
    }

    /// Run one boolean and return the result's mass-properties volume.
    fn kernel_vol<F>(build: &F, op: BooleanOp) -> Option<f64>
    where
        F: Fn(&mut BRepModel) -> (SolidId, SolidId),
    {
        let mut m = BRepModel::new();
        let (a, b) = build(&mut m);
        boolean_operation(&mut m, a, b, op, BooleanOptions::default())
            .ok()
            .and_then(|r| m.calculate_solid_volume(r))
    }

    fn mkbox(m: &mut BRepModel, sz: f64) -> SolidId {
        TopologyBuilder::new(m)
            .create_box_3d(sz, sz, sz)
            .expect("box");
        m.solids.iter().last().map(|(id, _)| id).expect("s")
    }

    const IN_UNIT4: fn(f64, f64, f64) -> bool =
        |x, y, z| x.abs() <= 2.0 && y.abs() <= 2.0 && z.abs() <= 2.0;

    /// MC over a larger box (operands may extend past ±2).
    fn mc_truth_wide<A, B>(in_a: A, in_b: B, half: f64, n: usize) -> (f64, f64, f64)
    where
        A: Fn(f64, f64, f64) -> bool,
        B: Fn(f64, f64, f64) -> bool,
    {
        mc_truth_analytic(in_a, in_b, half, n)
    }

    /// A cylinder fully inside the box (no poke-through) is a clean curved
    /// boolean — ∪/∩/∖ all match the MC truth. Regression guard.
    #[test]
    fn cylinder_inside_box_all_ops_match_mc() {
        let build = |m: &mut BRepModel| {
            let a = mkbox(m, 4.0);
            TopologyBuilder::new(m)
                .create_cylinder_3d(Vector3::new(0.0, 0.0, -1.0), Vector3::Z, 1.5, 2.0)
                .expect("cyl");
            (a, m.solids.iter().last().map(|(id, _)| id).expect("b"))
        };
        let in_cyl = |x: f64, y: f64, z: f64| x * x + y * y <= 1.5 * 1.5 && z.abs() <= 1.0;
        assert_mc(build, in_cyl, 4e-2);
    }

    /// A sphere fully inside the box: ∪/∩/∖ all match the MC truth. The ∖ case
    /// (box with a spherical void) is the regression guard for the spherical
    /// tessellation winding fix — the sphere void must SUBTRACT (was added:
    /// 78 vs 49.85, because a Forward sphere wound inward and the Backward void
    /// wound outward).
    #[test]
    fn sphere_inside_box_all_ops_match_mc() {
        let build = |m: &mut BRepModel| {
            let a = mkbox(m, 4.0);
            TopologyBuilder::new(m)
                .create_sphere_3d(Vector3::ZERO, 1.5)
                .expect("sph");
            (a, m.solids.iter().last().map(|(id, _)| id).expect("b"))
        };
        let in_sph = |x: f64, y: f64, z: f64| x * x + y * y + z * z <= 1.5 * 1.5;
        assert_mc(build, in_sph, 4e-2);
    }

    /// Curved-boolean frontier map (BOOL-CURVED-* tracking). Runs many hard
    /// configs through the independent analytic MC oracle and prints which the
    /// kernel still gets wrong — sphere cavities and curved poke-throughs are
    /// the open frontier. Run with `--ignored --nocapture`.
    #[test]
    #[ignore = "frontier map (run with --nocapture to see curved-boolean status)"]
    fn diag_curved_boolean_stress() {
        use crate::operations::transform::translate;
        let box4 = |m: &mut BRepModel| mkbox(m, 4.0);
        let report = |name: &str,
                      build: &dyn Fn(&mut BRepModel) -> (SolidId, SolidId),
                      in_a: &dyn Fn(f64, f64, f64) -> bool,
                      in_b: &dyn Fn(f64, f64, f64) -> bool,
                      half: f64| {
            let (mu, mi, md) = mc_truth_wide(in_a, in_b, half, 100);
            let run = |op: BooleanOp, t: f64| -> String {
                let mut m = BRepModel::new();
                let (a, b) = build(&mut m);
                match boolean_operation(&mut m, a, b, op, BooleanOptions::default()) {
                    Ok(r) => match m.calculate_solid_volume(r) {
                        Some(v) if (v - t).abs() <= 0.04 * t.max(1.0) => format!("OK({v:.2})"),
                        Some(v) => format!("**{v:.2}/{t:.2}**"),
                        None => "**novol**".to_string(),
                    },
                    Err(e) => format!("**ERR:{e:?}**"),
                }
            };
            eprintln!(
                "{name:>24}:\n   ∪ {}\n   ∩ {}\n   ∖ {}",
                run(BooleanOp::Union, mu),
                run(BooleanOp::Intersection, mi),
                run(BooleanOp::Difference, md),
            );
        };

        // sphere fully inside box (r=1.5)
        report(
            "sphere1.5 in box",
            &|m| {
                let a = box4(m);
                TopologyBuilder::new(m)
                    .create_sphere_3d(Vector3::ZERO, 1.5)
                    .expect("sph");
                (a, m.solids.iter().last().map(|(id, _)| id).expect("b"))
            },
            &IN_UNIT4,
            &|x, y, z| x * x + y * y + z * z <= 1.5 * 1.5,
            4.0,
        );
        // sphere poking out of box faces (r=2.5; box half 2)
        report(
            "sphere2.5 thru box",
            &|m| {
                let a = box4(m);
                TopologyBuilder::new(m)
                    .create_sphere_3d(Vector3::ZERO, 2.5)
                    .expect("sph");
                (a, m.solids.iter().last().map(|(id, _)| id).expect("b"))
            },
            &IN_UNIT4,
            &|x, y, z| x * x + y * y + z * z <= 2.5 * 2.5,
            4.0,
        );
        // cylinder fully inside box (no poke-through): z∈[-1,1]
        report(
            "cyl inside box",
            &|m| {
                let a = box4(m);
                TopologyBuilder::new(m)
                    .create_cylinder_3d(Vector3::new(0.0, 0.0, -1.0), Vector3::Z, 1.5, 2.0)
                    .expect("cyl");
                (a, m.solids.iter().last().map(|(id, _)| id).expect("b"))
            },
            &IN_UNIT4,
            &|x, y, z| x * x + y * y <= 1.5 * 1.5 && z.abs() <= 1.0,
            4.0,
        );
        // off-axis cylinder: shifted +X so it pokes the +X box wall AND ±Z caps
        report(
            "cyl off-axis +x",
            &|m| {
                let a = box4(m);
                TopologyBuilder::new(m)
                    .create_cylinder_3d(Vector3::new(0.0, 0.0, -3.0), Vector3::Z, 1.0, 6.0)
                    .expect("cyl");
                let b = m.solids.iter().last().map(|(id, _)| id).expect("b");
                translate(m, vec![b], Vector3::X, 1.5, Default::default()).unwrap();
                (a, b)
            },
            &IN_UNIT4,
            &|x, y, z| (x - 1.5) * (x - 1.5) + y * y <= 1.0 && z.abs() <= 3.0,
            4.0,
        );
        // horizontal cylinder (axis +X) through two box side walls
        report(
            "cyl horizontal X",
            &|m| {
                let a = box4(m);
                TopologyBuilder::new(m)
                    .create_cylinder_3d(Vector3::new(-3.0, 0.0, 0.0), Vector3::X, 1.0, 6.0)
                    .expect("cyl");
                (a, m.solids.iter().last().map(|(id, _)| id).expect("b"))
            },
            &IN_UNIT4,
            &|x, y, z| y * y + z * z <= 1.0 && x.abs() <= 3.0,
            4.0,
        );
    }

    /// Harsh EXACT-analytic judge for a B operand fully CONTAINED in the 4³ box:
    /// ∩ = V(shape), ∪ = 64, ∖ = 64 − V(shape). No Monte-Carlo noise, so the
    /// only slack is the kernel's own faceting.
    fn assert_contained<F>(build: &F, v_shape: f64, tol: f64)
    where
        F: Fn(&mut BRepModel) -> (SolidId, SolidId),
    {
        let box_vol = 64.0;
        // Fast result volume via a coarse tessellation (divergence sum). For a
        // contained operand each result is a single solid possibly with one
        // void, so |signed mesh volume| is the magnitude (∪=box, ∩=shape,
        // ∖=box−shape) — ~10× cheaper than the fine-tessellation mass-props.
        let vol = |op: BooleanOp| -> Option<f64> {
            let mut m = BRepModel::new();
            let (a, b) = build(&mut m);
            let r = boolean_operation(&mut m, a, b, op, BooleanOptions::default()).ok()?;
            crate::harness::watertight::mesh_volume(&m, r, 0.02)
        };
        let close = |k: Option<f64>, truth: f64, what: &str| {
            let v = k.unwrap_or_else(|| panic!("{what} returned None (op failed)"));
            assert!(
                (v - truth).abs() <= tol * truth.max(1.0),
                "{what}: kernel {v:.4} vs analytic truth {truth:.4} (V_shape={v_shape:.4})"
            );
        };
        close(vol(BooleanOp::Union), box_vol, "union");
        close(vol(BooleanOp::Intersection), v_shape, "intersection");
        close(vol(BooleanOp::Difference), box_vol - v_shape, "difference");
    }

    /// Assert all three booleans match the independent analytic MC truth.
    fn assert_mc<F, B>(build: F, in_b: B, tol: f64)
    where
        F: Fn(&mut BRepModel) -> (SolidId, SolidId),
        B: Fn(f64, f64, f64) -> bool,
    {
        let (mu, mi, md) = mc_truth_analytic(IN_UNIT4, in_b, 4.0, 120);
        let close = |k: Option<f64>, truth: f64, what: &str| {
            let v = k.unwrap_or_else(|| panic!("{what} returned None (op failed)"));
            assert!(
                (v - truth).abs() <= tol * truth.max(1.0),
                "{what}: kernel {v:.3} vs MC truth {truth:.3}"
            );
        };
        close(kernel_vol(&build, BooleanOp::Union), mu, "union");
        close(
            kernel_vol(&build, BooleanOp::Intersection),
            mi,
            "intersection",
        );
        close(kernel_vol(&build, BooleanOp::Difference), md, "difference");
    }

    /// Independent-oracle check: rotating one box about Z gives ∩/∪/∖ matching
    /// the analytic Monte-Carlo truth (not just the inclusion-exclusion identity,
    /// which can mask compensating errors). Confirms the rotated-box
    /// over-inclusion bug (task #34) is genuinely gone.
    #[test]
    fn rotated_box_booleans_match_mc_truth() {
        let ang = 0.5236; // 30°
        let build = move |m: &mut BRepModel| {
            let a = mkbox(m, 4.0);
            let b = mkbox(m, 4.0);
            rotate(
                m,
                vec![b],
                Vector3::ZERO,
                Vector3::Z,
                ang,
                Default::default(),
            )
            .unwrap();
            (a, b)
        };
        let in_b = move |x: f64, y: f64, z: f64| {
            let (c, s) = ((-ang).cos(), (-ang).sin());
            IN_UNIT4(c * x - s * y, s * x + c * y, z)
        };
        assert_mc(build, in_b, 2e-2);
    }

    /// Tilted-axis (fully 3D) rotation matches the analytic MC truth.
    #[test]
    fn tilted_box_booleans_match_mc_truth() {
        let ang = 0.5236;
        let ax = Vector3::new(1.0, 1.0, 1.0).normalize().unwrap();
        let build = move |m: &mut BRepModel| {
            let a = mkbox(m, 4.0);
            let b = mkbox(m, 4.0);
            rotate(m, vec![b], Vector3::ZERO, ax, ang, Default::default()).unwrap();
            (a, b)
        };
        let in_b = move |x: f64, y: f64, z: f64| {
            let p = crate::math::vector3::Point3::new(x, y, z);
            let q = crate::math::matrix4::Matrix4::from_axis_angle(&ax, -ang)
                .unwrap()
                .transform_point(&p);
            IN_UNIT4(q.x, q.y, q.z)
        };
        assert_mc(build, in_b, 2e-2);
    }

    fn cyl_through_box(m: &mut BRepModel) -> (SolidId, SolidId) {
        let a = mkbox(m, 4.0);
        TopologyBuilder::new(m)
            .create_cylinder_3d(Vector3::new(0.0, 0.0, -3.0), Vector3::Z, 1.5, 6.0)
            .expect("cyl");
        let b = m.solids.iter().last().map(|(id, _)| id).expect("b");
        (a, b)
    }
    fn in_cyl_through(x: f64, y: f64, z: f64) -> bool {
        x * x + y * y <= 1.5 * 1.5 && z.abs() <= 3.0
    }

    /// A cylinder poking ALL THE WAY THROUGH a box's ±Z caps — ∪/∩/∖ all match
    /// the independent MC truth. The intersection/difference were fixed by the
    /// periodic-face `is_point_in_face` bug (a point on a full cylinder lateral
    /// was judged outside, mis-classifying every cap fragment); the UNION (box
    /// body + 2 cylinder stubs + 2 end caps that must stitch via the shared
    /// ±Z=2/±Z=3 circles) was fixed by the circular-edge adjacency pass (a full
    /// circle on the box cap groups with its arc decomposition on the cylinder).
    /// Full regression guard for BOOL-CURVED-STITCH (#50).
    #[test]
    fn cylinder_through_box_all_ops_match_mc_truth() {
        assert_mc(cyl_through_box, in_cyl_through, 3e-2);
    }

    fn box_solid(model: &mut BRepModel) -> SolidId {
        TopologyBuilder::new(model)
            .create_box_3d(2.0, 2.0, 2.0)
            .expect("box");
        model.solids.iter().last().map(|(id, _)| id).expect("solid")
    }

    fn plane_face(model: &BRepModel, want_pos_x: bool) -> FaceId {
        model
            .faces
            .iter()
            .find(|(_, face)| {
                model
                    .surfaces
                    .get(face.surface_id)
                    .and_then(|s| s.as_any().downcast_ref::<Plane>())
                    .map(|p| {
                        p.normal.dot(&X).abs() > 0.99
                            && if want_pos_x {
                                p.origin.dot(&X) > 0.5
                            } else {
                                p.origin.dot(&X) < -0.5
                            }
                    })
                    .unwrap_or(false)
            })
            .map(|(id, _)| id)
            .expect("axis face")
    }

    #[test]
    fn touching_boxes_broad_phase_culls_and_retains_the_contact() {
        let mut model = BRepModel::new();
        let a = box_solid(&mut model);
        let b = box_solid(&mut model);
        crate::operations::transform::translate(&mut model, vec![b], X, 2.0, Default::default())
            .expect("translate");

        // The coincident pair: A's +X face and B's −X face both at x = 1.
        let a_plus_x = plane_face(&model, true);
        // B's faces include a −X face now at x = 1 (B spans [1,3]); find it among
        // B's faces specifically by re-scanning for the one near x = 1.
        let b_minus_x = model
            .faces
            .iter()
            .filter(|(id, _)| *id != a_plus_x)
            .find(|(_, face)| {
                model
                    .surfaces
                    .get(face.surface_id)
                    .and_then(|s| s.as_any().downcast_ref::<Plane>())
                    .map(|p| p.normal.dot(&X).abs() > 0.99 && (p.origin.dot(&X) - 1.0).abs() < 1e-6)
                    .unwrap_or(false)
            })
            .map(|(id, _)| id)
            .expect("B's face at x=1");

        let report = boolean_broad_phase_ablation(&model, a, b, Some((a_plus_x, b_minus_x)));
        let raw = report.stages[0].output;
        let overlap = report.stages[1].output;
        assert_eq!(raw, 36, "6×6 face-pairs");
        assert!(overlap < raw, "AABB culling must prune some pairs");
        assert!(overlap > 0, "the touching region is retained");
        assert_eq!(
            report.correct,
            Some(true),
            "coincident pair survives the cull"
        );
        assert!(report.render().contains("aabb_overlap"));
    }

    #[test]
    fn separated_boxes_broad_phase_culls_everything() {
        let mut model = BRepModel::new();
        let a = box_solid(&mut model);
        let b = box_solid(&mut model);
        crate::operations::transform::translate(&mut model, vec![b], X, 20.0, Default::default())
            .expect("translate");
        let report = boolean_broad_phase_ablation(&model, a, b, None);
        assert_eq!(report.stages[0].output, 36);
        assert_eq!(
            report.stages[1].output, 0,
            "far apart → no overlapping face-pairs"
        );
    }

    // -- boolean correctness invariants (iron-clad) ------------------------

    use crate::operations::transform::{rotate, translate};

    fn make_box(model: &mut BRepModel, size: f64) -> Option<SolidId> {
        TopologyBuilder::new(model)
            .create_box_3d(size, size, size)
            .ok()?;
        model.solids.iter().last().map(|(id, _)| id)
    }

    /// Two `size³` boxes centred at the origin; B shifted `dx` along +X.
    fn shifted_boxes(size: f64, dx: f64) -> impl Fn(&mut BRepModel) -> Option<(SolidId, SolidId)> {
        move |m| {
            let a = make_box(m, size)?;
            let b = make_box(m, size)?;
            if dx.abs() > 1e-9 {
                translate(m, vec![b], X, dx, Default::default()).ok()?;
            }
            Some((a, b))
        }
    }

    /// Two `size³` boxes centred at the origin; B rotated `angle` about +Z (still
    /// concentric, so they overlap heavily — a rotated-intersection stress case).
    fn rotated_boxes(
        size: f64,
        angle: f64,
    ) -> impl Fn(&mut BRepModel) -> Option<(SolidId, SolidId)> {
        move |m| {
            let a = make_box(m, size)?;
            let b = make_box(m, size)?;
            rotate(
                m,
                vec![b],
                Vector3::ZERO,
                Vector3::Z,
                angle,
                Default::default(),
            )
            .ok()?;
            Some((a, b))
        }
    }

    #[test]
    fn axis_aligned_partial_overlap_satisfies_all_invariants() {
        // 4³ boxes, B shifted 2 along x → exactly half overlap.
        let inv = check_boolean_invariants(shifted_boxes(4.0, 2.0), 1e-3);
        assert!(inv.all_hold, "invariants violated: {inv:?}");
        // Ground truth: V(A)=V(B)=64, overlap = 2·4·4 = 32, union = 96.
        assert!(
            (inv.vol_intersection.unwrap() - 32.0).abs() < 0.1,
            "{inv:?}"
        );
        assert!((inv.vol_union.unwrap() - 96.0).abs() < 0.1, "{inv:?}");
        assert!((inv.vol_difference.unwrap() - 32.0).abs() < 0.1, "{inv:?}");
    }

    #[test]
    fn containment_satisfies_invariants() {
        // Small box fully inside a big one: ∪ = big, ∩ = small.
        let build = |m: &mut BRepModel| -> Option<(SolidId, SolidId)> {
            let big = make_box(m, 10.0)?;
            let small = make_box(m, 4.0)?;
            Some((big, small))
        };
        let inv = check_boolean_invariants(build, 1e-3);
        assert!(inv.all_hold, "{inv:?}");
        assert!((inv.vol_union.unwrap() - 1000.0).abs() < 0.5, "{inv:?}");
        assert!(
            (inv.vol_intersection.unwrap() - 64.0).abs() < 0.5,
            "{inv:?}"
        );
    }

    /// B rotated about Z **and** shifted along +X so the overlap is a small,
    /// non-axis-aligned sliver — the regime where intersection over-inclusion was
    /// historically observed.
    fn rotated_shifted_boxes(
        size: f64,
        angle: f64,
        dx: f64,
    ) -> impl Fn(&mut BRepModel) -> Option<(SolidId, SolidId)> {
        move |m| {
            let a = make_box(m, size)?;
            let b = make_box(m, size)?;
            rotate(
                m,
                vec![b],
                Vector3::ZERO,
                Vector3::Z,
                angle,
                Default::default(),
            )
            .ok()?;
            translate(m, vec![b], X, dx, Default::default()).ok()?;
            Some((a, b))
        }
    }

    /// B rotated about a tilted (1,1,1) axis — fully 3D, no face stays
    /// axis-aligned. The hardest face-split regime for the boolean.
    fn tilted_rotated_boxes(
        size: f64,
        angle: f64,
    ) -> impl Fn(&mut BRepModel) -> Option<(SolidId, SolidId)> {
        move |m| {
            let a = make_box(m, size)?;
            let b = make_box(m, size)?;
            let axis = Vector3::new(1.0, 1.0, 1.0).normalize().ok()?;
            rotate(m, vec![b], Vector3::ZERO, axis, angle, Default::default()).ok()?;
            Some((a, b))
        }
    }

    #[test]
    fn rotated_overlap_satisfies_inclusion_exclusion() {
        // Two concentric 4³ boxes, B rotated 30° about Z. Whatever the
        // intersection's exact shape, V(∪)+V(∩) must equal V(A)+V(B)=128.
        let inv = check_boolean_invariants(rotated_boxes(4.0, std::f64::consts::PI / 6.0), 1e-2);
        assert!(
            inv.inclusion_exclusion,
            "rotated inclusion-exclusion violated: {inv:?}"
        );
        assert!(inv.intersection_bounded, "rotated ∩ out of bounds: {inv:?}");
    }

    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

        /// Inclusion–exclusion holds for any axis-aligned partial overlap.
        #[test]
        fn pp_axis_aligned_inclusion_exclusion(dx in 0.5f64..3.5) {
            let inv = check_boolean_invariants(shifted_boxes(4.0, dx), 1e-2);
            prop_assert!(inv.inclusion_exclusion, "dx={dx}: {inv:?}");
            prop_assert!(inv.difference_consistent, "dx={dx}: {inv:?}");
        }

        /// Inclusion–exclusion holds for any rotation of one operand. This is the
        /// adversarial case that exercises non-axis-aligned face splits.
        #[test]
        fn pp_rotated_inclusion_exclusion(angle in 0.15f64..1.4) {
            let inv = check_boolean_invariants(rotated_boxes(4.0, angle), 2e-2);
            prop_assert!(inv.inclusion_exclusion, "angle={angle}: {inv:?}");
        }

        /// The hardest regime: B rotated AND shifted so the overlap is a small,
        /// non-axis-aligned region. `dx ≤ 2` keeps the operands genuinely
        /// overlapping (the rotated box's half-diagonal is ~2.83). This is the
        /// configuration historically associated with intersection over-inclusion.
        #[test]
        fn pp_rotated_shifted_inclusion_exclusion(angle in 0.2f64..1.2, dx in 1.0f64..2.0) {
            let inv = check_boolean_invariants(rotated_shifted_boxes(4.0, angle, dx), 3e-2);
            prop_assert!(inv.inclusion_exclusion, "angle={angle} dx={dx}: {inv:?}");
            prop_assert!(inv.intersection_bounded, "angle={angle} dx={dx}: {inv:?}");
        }

        /// Tilted-axis (fully 3D) rotation — no face stays axis-aligned.
        #[test]
        fn pp_tilted_rotation_inclusion_exclusion(angle in 0.2f64..1.2) {
            let inv = check_boolean_invariants(tilted_rotated_boxes(4.0, angle), 3e-2);
            prop_assert!(inv.inclusion_exclusion, "angle={angle}: {inv:?}");
        }

        // ---- HARSH curved-boolean judgement (independent MC oracle) --------
        // Nothing less than the analytic truth: ALL THREE of ∪/∩/∖ must match
        // the Monte-Carlo ground truth (which owes nothing to the kernel) to a
        // few percent, for any contained sphere/cylinder across the parameter
        // space. These exercise the now-fixed periodic-classification and
        // spherical-winding paths over random configs, not just one example.

        /// A sphere fully inside the box at ANY offset/radius. For a CONTAINED
        /// operand the truth is EXACT and analytic — no Monte-Carlo noise:
        /// ∩ = V(sphere), ∪ = V(box), ∖ = V(box) − V(sphere). Judged hard
        /// (2.5%, faceting only).
        #[test]
        fn pp_contained_sphere_all_ops(
            cx in -0.6f64..0.6, cy in -0.6f64..0.6, cz in -0.6f64..0.6,
            r in 0.5f64..1.2,
        ) {
            let build = move |m: &mut BRepModel| {
                let a = mkbox(m, 4.0);
                TopologyBuilder::new(m)
                    .create_sphere_3d(Vector3::new(cx, cy, cz), r)
                    .expect("sph");
                (a, m.solids.iter().last().map(|(id, _)| id).expect("b"))
            };
            let v_shape = 4.0 / 3.0 * std::f64::consts::PI * r * r * r;
            assert_contained(&build, v_shape, 2.5e-2);
        }

        /// A cylinder fully inside the box at ANY offset/radius/height. Exact
        /// analytic truth (∩ = πr²h, ∪ = 64, ∖ = 64 − πr²h), judged hard.
        #[test]
        fn pp_contained_cylinder_all_ops(
            cx in -0.5f64..0.5, cy in -0.5f64..0.5,
            r in 0.5f64..1.0, zc in -0.5f64..0.5, hh in 0.5f64..1.2,
        ) {
            let (z0, z1) = (zc - hh, zc + hh);
            let build = move |m: &mut BRepModel| {
                let a = mkbox(m, 4.0);
                TopologyBuilder::new(m)
                    .create_cylinder_3d(Vector3::new(cx, cy, z0), Vector3::Z, r, z1 - z0)
                    .expect("cyl");
                (a, m.solids.iter().last().map(|(id, _)| id).expect("b"))
            };
            let v_shape = std::f64::consts::PI * r * r * (z1 - z0);
            assert_contained(&build, v_shape, 2.5e-2);
        }
    }
}
