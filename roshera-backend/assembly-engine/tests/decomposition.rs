//! Slice-3 decomposition pipeline REDs (kinematic-assembly campaign,
//! spec §3.4): seated-fastened condensation → connected components →
//! recursive-assembly DR-plan (Extend + loop clusters) → dense fallback,
//! plus the structural-vs-numeric dual DOF report.
//!
//! Pre-implementation signature (captured 2026-07-17, post-91f3c588
//! tree + analytic-Jacobian slice): `Assembly::solve_decomposed`,
//! `Assembly::solved_poses_with_stats`, `Assembly::dual_dof_report` and
//! `MateKind::structural_rank` did not exist — the only solve path was
//! the one-big-system Gauss-Newton — so this file failed to compile
//! (E0599). Every behavioural pin below is on quantities the dense path
//! cannot produce (step counts, body counts, Kutzbach-vs-rank
//! disagreement), so none of them can pass vacuously.

use assembly_engine::{Assembly, FeatureRef, Instance, InstanceId, Mate, MateKind, Mesh};

fn part(id: u32) -> Instance {
    Instance::new(InstanceId(id), format!("part_{id}"), Mesh::default())
}

fn frame(origin: [f64; 3], z: [f64; 3], x: [f64; 3]) -> FeatureRef {
    FeatureRef::Frame {
        origin,
        z_axis: z,
        x_axis: x,
    }
}

fn mate(kind: MateKind, a: u32, fa: FeatureRef, b: u32, fb: FeatureRef) -> Mate {
    Mate {
        kind,
        a: InstanceId(a),
        feature_a: fa,
        b: InstanceId(b),
        feature_b: fb,
    }
}

/// Fastened mate welding the TOP of body `a` (local z = +0.5) to the
/// BOTTOM of body `b` (local z = −0.5): satisfied exactly when body b
/// sits one unit above body a (aligned frames).
fn stacked_fastened(a: u32, b: u32) -> Mate {
    mate(
        MateKind::Fastened,
        a,
        frame([0.0, 0.0, 0.5], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        b,
        frame([0.0, 0.0, -0.5], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    )
}

/// The four revolutes of a 10x10 PARALLELOGRAM four-bar: ground joints
/// at (0,0) and (10,0), crank L1 up to (0,10), coupler L2 across to
/// (10,10), rocker L3 back down. Away from any stretch singularity: the
/// numeric mobility is the linkage's true 1 DOF (a first fixture used a
/// fully-STRETCHED collinear 4-bar and measured its extra instantaneous
/// singular DOF — a real lesson: the numeric layer reports the
/// configuration, not the schematic).
fn add_parallelogram_revolutes(assembly: &mut Assembly) {
    assembly.add_mate(revolute_at(0, [0.0, 0.0, 0.0], 1, [0.0, 0.0, 0.0]));
    assembly.add_mate(revolute_at(1, [0.0, 10.0, 0.0], 2, [0.0, 0.0, 0.0]));
    assembly.add_mate(revolute_at(2, [10.0, 0.0, 0.0], 3, [0.0, 0.0, 0.0]));
    assembly.add_mate(revolute_at(3, [0.0, -10.0, 0.0], 0, [10.0, 0.0, 0.0]));
}

/// A revolute about the world z axis through `p` (frames coincident there).
fn revolute_at(a: u32, pa: [f64; 3], b: u32, pb: [f64; 3]) -> Mate {
    mate(
        MateKind::Revolute { limits: None },
        a,
        frame(pa, [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        b,
        frame(pb, [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    )
}

// ── Condensation ────────────────────────────────────────────────────────

#[test]
fn seated_bolted_stack_condenses_into_one_rigid_body() {
    // Ground + 5 cubes fastened in a seated chain (body k at z = k):
    // the whole stack is ONE rigid body after condensation — zero
    // numeric work remains and the poses come back untouched.
    let mut assembly = Assembly::new(InstanceId(0));
    for k in 0..6u32 {
        let mut p = part(k);
        p.translation = [0.0, 0.0, f64::from(k)];
        assembly.add_instance(p);
    }
    for k in 0..5u32 {
        assembly.add_mate(stacked_fastened(k, k + 1));
    }
    let before: Vec<[f64; 3]> = assembly.instances.iter().map(|i| i.translation).collect();

    let (report, stats) = assembly.solve_decomposed();
    assert!(report.converged, "{report:?}");
    assert_eq!(
        stats.condensed_bodies, 1,
        "the seated stack is one rigid body: {stats:?}"
    );
    assert_eq!(stats.condensation_merges, 5);
    assert_eq!(stats.components, 0, "nothing left to solve");
    assert_eq!(report.iterations, 0, "no numeric work on a seated stack");
    let after: Vec<[f64; 3]> = assembly.instances.iter().map(|i| i.translation).collect();
    assert_eq!(before, after, "seated poses must come back untouched");
}

// ── Recursive-assembly Extend steps ─────────────────────────────────────

#[test]
fn perturbed_fastened_chain_extends_body_by_body() {
    // A 10-body fastened chain dropped at WRONG poses: the DR-plan
    // seats it as 10 sequential 6-DOF Extend solves (Kramer's recursive
    // assembly), never as one 60-column dense system.
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0)); // ground at origin
    for k in 1..11u32 {
        let mut p = part(k);
        p.translation = [0.7 * f64::from(k), -0.4 * f64::from(k), f64::from(k) + 0.3];
        assembly.add_instance(p);
    }
    for k in 0..10u32 {
        assembly.add_mate(stacked_fastened(k, k + 1));
    }

    let (report, stats) = assembly.solve_decomposed();
    assert!(report.converged, "{report:?}");
    assert_eq!(stats.components, 1);
    assert_eq!(stats.extend_steps, 10, "each body extends alone: {stats:?}");
    assert_eq!(stats.loop_clusters, 0);
    assert_eq!(stats.fallbacks, 0);
    for k in 1..11u32 {
        let t = assembly
            .instance(InstanceId(k))
            .map(|i| i.translation)
            .unwrap_or([f64::NAN; 3]);
        assert!(
            t[0].abs() < 1e-6 && t[1].abs() < 1e-6 && (t[2] - f64::from(k)).abs() < 1e-6,
            "body {k} seated at z={k}, got {t:?}"
        );
    }
}

// ── Connected components ────────────────────────────────────────────────

#[test]
fn disjoint_subassemblies_solve_as_separate_components() {
    // Two independent groups both bolted to ground: the datum does NOT
    // merge them — each solves alone (the sketch decompose.rs rule, one
    // dimension up).
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0)); // ground
    let mut a = part(1);
    a.translation = [5.0, 5.0, 3.0]; // perturbed off its seat
    assembly.add_instance(a);
    let mut b = part(2);
    b.translation = [20.0, 0.0, 1.0];
    assembly.add_instance(b);
    let mut c = part(3);
    c.translation = [20.0, 0.0, 2.4];
    assembly.add_instance(c);
    // Group 1: instance 1 fastened to ground.
    assembly.add_mate(stacked_fastened(0, 1));
    // Group 2: 2 fastened to ground (seated at z=... not seated; generic),
    // 3 revolute on 2.
    assembly.add_mate(mate(
        MateKind::Fastened,
        0,
        frame([20.0, 0.0, 1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        2,
        frame([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ));
    assembly.add_mate(revolute_at(2, [0.0, 0.0, 1.0], 3, [0.0, 0.0, 0.0]));

    let (report, stats) = assembly.solve_decomposed();
    assert!(report.converged, "{report:?}");
    assert_eq!(stats.components, 2, "{stats:?}");
}

// ── Loop clusters ───────────────────────────────────────────────────────

#[test]
fn planar_four_bar_solves_as_one_loop_cluster() {
    // Ground + 3 links closed by 4 parallel-axis revolutes — the classic
    // loop no extension step can place (every link keeps its spin DOF).
    // The remainder solves as ONE coupled cluster.
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    let mut l1 = part(1);
    l1.translation = [0.1, -0.1, 0.05];
    assembly.add_instance(l1);
    let mut l2 = part(2);
    l2.translation = [0.2, 10.1, -0.04];
    assembly.add_instance(l2);
    let mut l3 = part(3);
    l3.translation = [9.9, 10.05, 0.08];
    assembly.add_instance(l3);
    add_parallelogram_revolutes(&mut assembly);

    let (report, stats) = assembly.solve_decomposed();
    assert!(report.converged, "{report:?}");
    assert_eq!(
        stats.extend_steps, 0,
        "no link is placeable alone: {stats:?}"
    );
    assert_eq!(stats.loop_clusters, 1, "{stats:?}");
    assert_eq!(stats.fallbacks, 0, "{stats:?}");
}

// ── Fallback honesty ────────────────────────────────────────────────────

#[test]
fn noop_pipeline_is_byte_identical_to_dense() {
    // A component the planner cannot shrink (planar chain — rank-3 mates,
    // no fastened, no loop split): the cluster path degenerates to the
    // EXACT dense system (same blocks, same mate order) — poses and
    // report byte-identical to `solve()`. The planner can only SHRINK
    // what Newton sees; when it can't, behaviour is unchanged.
    let build = || {
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(part(0));
        let mut b1 = part(1);
        b1.translation = [1.0, 2.0, 3.0];
        b1.rotation = [0.1, 0.0, 0.05, 0.99366];
        assembly.add_instance(b1);
        let mut b2 = part(2);
        b2.translation = [-2.0, 1.0, 4.0];
        assembly.add_instance(b2);
        assembly.add_mate(mate(
            MateKind::Planar,
            0,
            frame([0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
            1,
            frame([0.0, 0.0, -1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        ));
        assembly.add_mate(mate(
            MateKind::Planar,
            1,
            frame([0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
            2,
            frame([0.0, 0.0, -1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        ));
        assembly
    };
    let mut dense = build();
    let dense_report = dense.solve();
    let mut decomposed = build();
    let (dec_report, stats) = decomposed.solve_decomposed();

    assert_eq!(stats.extend_steps, 0);
    assert_eq!(stats.loop_clusters, 1);
    assert_eq!(dense_report, dec_report, "identical solve reports");
    for (d, p) in dense.instances.iter().zip(decomposed.instances.iter()) {
        assert_eq!(d.translation, p.translation, "byte-identical translation");
        assert_eq!(d.rotation, p.rotation, "byte-identical rotation");
    }
}

#[test]
fn conflicting_mates_keep_dense_verdict_semantics() {
    // Two contradictory Fastened between the same pair: no plan can
    // satisfy them; the decomposed path must surface the same honest
    // verdict as dense — converged == false with the residual stuck high.
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    assembly.add_instance(part(1));
    assembly.add_mate(stacked_fastened(0, 1)); // seat at z = 1
    assembly.add_mate(mate(
        MateKind::Fastened,
        0,
        frame([0.0, 0.0, 5.5], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        1,
        frame([0.0, 0.0, -0.5], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    )); // and at z = 6 — impossible
    let mut dense = assembly.clone();
    let dense_report = dense.solve();
    let (report, stats) = assembly.solve_decomposed();
    assert!(!report.converged, "{report:?}");
    assert!(!dense_report.converged);
    assert!(report.final_residual_norm > 0.5);
    assert!(
        stats.fallbacks >= 1,
        "a planned miss must re-run dense from the original poses: {stats:?}"
    );
}

#[test]
fn decomposed_solve_is_deterministic() {
    let build = || {
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(part(0));
        for k in 1..8u32 {
            let mut p = part(k);
            p.translation = [1.3 * f64::from(k), -0.9, f64::from(k)];
            assembly.add_instance(p);
        }
        for k in 0..7u32 {
            assembly.add_mate(stacked_fastened(k, k + 1));
        }
        assembly.add_mate(revolute_at(3, [2.0, 0.0, 0.0], 7, [0.0, 0.0, -2.0]));
        assembly
    };
    let mut a = build();
    let (ra, sa) = a.solve_decomposed();
    let mut b = build();
    let (rb, sb) = b.solve_decomposed();
    assert_eq!(ra, rb);
    assert_eq!(sa, sb);
    for (x, y) in a.instances.iter().zip(b.instances.iter()) {
        assert_eq!(x.translation, y.translation);
        assert_eq!(x.rotation, y.rotation);
    }
}

// ── solved_poses routes through the decomposition ───────────────────────

#[test]
fn solved_poses_carries_decomposition_stats() {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    let mut p = part(1);
    p.translation = [3.0, 3.0, 3.0];
    assembly.add_instance(p);
    assembly.add_mate(stacked_fastened(0, 1));

    let (report, stats, poses) = assembly.solved_poses_with_stats();
    assert!(report.converged);
    assert_eq!(stats.extend_steps, 1, "{stats:?}");
    let (plain_report, plain_poses) = assembly.solved_poses();
    assert_eq!(report, plain_report, "solved_poses IS the decomposed path");
    assert_eq!(poses, plain_poses);
}

// ── Structural vs numeric DOF, dual-reported ────────────────────────────

#[test]
fn structural_rank_table_matches_the_dof_table() {
    assert_eq!(MateKind::Fastened.structural_rank(), 6);
    assert_eq!(MateKind::Fixed.structural_rank(), 6);
    assert_eq!(MateKind::Revolute { limits: None }.structural_rank(), 5);
    assert_eq!(MateKind::Slider { limits: None }.structural_rank(), 5);
    assert_eq!(
        MateKind::Cylindrical {
            rot_limits: None,
            trans_limits: None
        }
        .structural_rank(),
        4
    );
    assert_eq!(
        MateKind::PinSlot {
            slot_dir_x: true,
            limits: None
        }
        .structural_rank(),
        4
    );
    assert_eq!(MateKind::Planar.structural_rank(), 3);
    assert_eq!(MateKind::Ball.structural_rank(), 3);
    assert_eq!(MateKind::Coincident.structural_rank(), 3);
    assert_eq!(MateKind::Concentric.structural_rank(), 4);
    assert_eq!(MateKind::Parallel.structural_rank(), 2);
    assert_eq!(MateKind::Distance { value: 1.0 }.structural_rank(), 1);
    assert_eq!(MateKind::Angle { value: 1.0 }.structural_rank(), 1);
    assert_eq!(MateKind::Tangent { radius: 1.0 }.structural_rank(), 1);
    assert_eq!(
        MateKind::Screw {
            lead: 1.0,
            at: [0.0, 0.0],
            couples: 0
        }
        .structural_rank(),
        1
    );
    // The refuse set counts ZERO structural rank — a refused mate must
    // never look like a constraint in either layer (#19 contract).
    assert_eq!(MateKind::Cam.structural_rank(), 0);
    assert_eq!(MateKind::Path.structural_rank(), 0);
    assert_eq!(MateKind::Symmetric.structural_rank(), 0);
}

#[test]
fn generic_mechanism_reports_agreeing_layers() {
    // A single revolute pair: Kutzbach says 6 − 5 = 1, the Jacobian rank
    // says 1 — the layers agree, no special-geometry flag.
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    assembly.add_instance(part(1));
    assembly.add_mate(revolute_at(0, [0.0, 0.0, 0.0], 1, [0.0, 0.0, 0.0]));
    let dual = assembly.dual_dof_report();
    assert_eq!(dual.config_dim, 6);
    assert_eq!(dual.structural_dof, 1);
    assert_eq!(dual.numeric_dof, 1);
    assert!(!dual.special_geometry, "{dual:?}");
}

#[test]
fn planar_four_bar_disagreement_is_flagged_as_special_geometry() {
    // The Grübler-Kutzbach failure case (spec §1.2): a planar 4-bar in
    // 3D counts M = 6·3 − 4·5 = −2, yet the parallel-axis geometry
    // leaves a real 1-DOF motion. Counting is GENERIC — the numeric
    // rank is the executor's verdict, and the DISAGREEMENT ITSELF is a
    // reported fact.
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    assembly.add_instance(part(1));
    let mut l2 = part(2);
    l2.translation = [0.0, 10.0, 0.0];
    assembly.add_instance(l2);
    let mut l3 = part(3);
    l3.translation = [10.0, 10.0, 0.0];
    assembly.add_instance(l3);
    add_parallelogram_revolutes(&mut assembly);

    let dual = assembly.dual_dof_report();
    assert_eq!(dual.config_dim, 18);
    assert_eq!(
        dual.structural_dof, -2,
        "Kutzbach counting on the 4-bar: {dual:?}"
    );
    assert_eq!(dual.numeric_dof, 1, "the crank really turns: {dual:?}");
    assert!(
        dual.special_geometry,
        "structural≠numeric must surface as a fact: {dual:?}"
    );
}

#[test]
fn refused_mates_count_in_neither_layer() {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    assembly.add_instance(part(1));
    assembly.add_mate(mate(
        MateKind::Cam,
        0,
        frame([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        1,
        frame([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ));
    let dual = assembly.dual_dof_report();
    assert_eq!(dual.structural_dof, 6);
    assert_eq!(dual.numeric_dof, 6);
    assert!(!dual.special_geometry);
}
