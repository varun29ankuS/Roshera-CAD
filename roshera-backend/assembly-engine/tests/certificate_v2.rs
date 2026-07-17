//! Assembly certificate v2 REDs (kinematic-assembly campaign, Slice 4 —
//! spec §3.5): verdict-with-witness, mirroring sketch certificate v2 one
//! dimension up. QuickXplain conflict witnesses over a re-solve oracle
//! (128-call cap, honest `minimal` flag), per-mate facts with rank
//! roles, per-instance constrainment with twist-decoded motions,
//! structural-vs-numeric dual report, and the kernel-derived ε floor
//! that kills the ε=0 default lie.
//!
//! Pre-implementation signature (captured 2026-07-17, post-c42c7e9c
//! tree): `Assembly::analyze_constrainedness`, `certify_v2`,
//! `static_contradictory_pairs` and every v2 type did not exist — the
//! certificate was 9 ANDed booleans with a bare `dof` int, conflicts
//! were detected only as "residual stayed high", nothing named WHICH
//! mates fight, and `certify` took a bare `epsilon: f64` the callers
//! defaulted to 0.0 — so this file failed to compile (E0599/E0432).

use assembly_engine::constrainedness::{
    AssemblyConstrainedness, EpsilonSpec, InstanceConstrainment, MateRole, SolverVerdict,
    TwistMotion, WitnessKind,
};
use assembly_engine::{Assembly, FeatureRef, Instance, InstanceId, Joint, Mate, MateKind, Mesh};

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

fn fastened_at(a: u32, origin_a: [f64; 3], b: u32, origin_b: [f64; 3]) -> Mate {
    mate(
        MateKind::Fastened,
        a,
        frame(origin_a, [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        b,
        frame(origin_b, [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    )
}

// ── The planted conflict yields a MINIMAL 2-mate witness ────────────────

#[test]
fn planted_conflict_yields_minimal_two_mate_witness() {
    // Mate 0 seats body 1 at z=1; mate 1 demands z=6 — impossible
    // together. Mate 2 is a healthy revolute hanging body 2 off body 1:
    // it must NOT be dragged into the witness (QuickXplain minimality).
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    assembly.add_instance(part(1));
    assembly.add_instance(part(2));
    assembly.add_mate(fastened_at(0, [0.0, 0.0, 1.0], 1, [0.0, 0.0, 0.0]));
    assembly.add_mate(fastened_at(0, [0.0, 0.0, 6.0], 1, [0.0, 0.0, 0.0]));
    assembly.add_mate(mate(
        MateKind::Revolute { limits: None },
        1,
        frame([2.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        2,
        frame([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ));

    let analysis = assembly.analyze_constrainedness();
    assert!(
        matches!(
            analysis.constrainedness,
            AssemblyConstrainedness::Conflicting { conflicts } if conflicts >= 1
        ),
        "{:?}",
        analysis.constrainedness
    );
    assert!(
        matches!(analysis.solver, SolverVerdict::Conflicting { .. }),
        "{:?}",
        analysis.solver
    );
    assert_eq!(analysis.witnesses.len(), 1, "{:?}", analysis.witnesses);
    let w = &analysis.witnesses[0];
    let members: Vec<usize> = w.mates.iter().map(|m| m.index).collect();
    assert_eq!(
        members,
        vec![0, 1],
        "the witness is EXACTLY the contradictory pair: {w:?}"
    );
    assert!(w.minimal, "QuickXplain completed under the cap: {w:?}");
    assert!(
        w.oracle_calls <= 128,
        "oracle budget (sketch parity): {w:?}"
    );
    // Roles: the pair is Conflicting; the healthy revolute is Independent.
    assert_eq!(analysis.mate_facts.len(), 3);
    assert_eq!(analysis.mate_facts[0].role, MateRole::Conflicting);
    assert_eq!(analysis.mate_facts[1].role, MateRole::Conflicting);
    assert_eq!(analysis.mate_facts[2].role, MateRole::Independent);
    // The certificate carries the same verdict and refuses soundness.
    let cert = assembly.certify_v2(
        &[],
        EpsilonSpec {
            kernel_floor: 0.01,
            requested: None,
        },
    );
    assert!(!cert.is_sound());
    assert!(!cert.mates_consistent);
    assert_eq!(cert.witnesses.len(), 1);
    assert_eq!(
        cert.constrainedness,
        Some(analysis.constrainedness),
        "certificate and analysis agree"
    );
}

// ── Static contradictory pairs (configuration-independent) ──────────────

#[test]
fn static_detector_names_contradictory_fastened_pairs() {
    // Two Fastened between the SAME pair implying DIFFERENT relative
    // poses: contradictory by declaration, no solve needed.
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    assembly.add_instance(part(1));
    assembly.add_mate(fastened_at(0, [0.0, 0.0, 1.0], 1, [0.0, 0.0, 0.0]));
    assembly.add_mate(fastened_at(0, [0.0, 0.0, 6.0], 1, [0.0, 0.0, 0.0]));
    assert_eq!(assembly.static_contradictory_pairs(), vec![(0, 1)]);

    // The SAME declared frames twice = redundant, NOT contradictory.
    let mut dup = Assembly::new(InstanceId(0));
    dup.add_instance(part(0));
    dup.add_instance(part(1));
    dup.add_mate(fastened_at(0, [0.0, 0.0, 1.0], 1, [0.0, 0.0, 0.0]));
    dup.add_mate(fastened_at(0, [0.0, 0.0, 1.0], 1, [0.0, 0.0, 0.0]));
    assert!(dup.static_contradictory_pairs().is_empty());

    // Distance 2 vs Distance 7 over the same connector frames.
    let mut dd = Assembly::new(InstanceId(0));
    dd.add_instance(part(0));
    dd.add_instance(part(1));
    let fa = frame([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let fb = frame([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    dd.add_mate(mate(
        MateKind::Distance { value: 2.0 },
        0,
        fa.clone(),
        1,
        fb.clone(),
    ));
    dd.add_mate(mate(MateKind::Distance { value: 7.0 }, 0, fa, 1, fb));
    assert_eq!(dd.static_contradictory_pairs(), vec![(0, 1)]);
}

// ── Redundant vs conflicting split ──────────────────────────────────────

#[test]
fn duplicate_fastened_is_redundant_not_conflicting() {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    let mut p = part(1);
    p.translation = [0.5, -0.3, 0.8]; // perturbed; the solve seats it
    assembly.add_instance(p);
    assembly.add_mate(fastened_at(0, [0.0, 0.0, 1.0], 1, [0.0, 0.0, 0.0]));
    assembly.add_mate(fastened_at(0, [0.0, 0.0, 1.0], 1, [0.0, 0.0, 0.0]));

    let analysis = assembly.analyze_constrainedness();
    assert!(
        matches!(
            analysis.constrainedness,
            AssemblyConstrainedness::OverConstrained { redundant: 2 }
        ),
        "each duplicate is individually removable: {:?}",
        analysis.constrainedness
    );
    assert!(
        matches!(
            analysis.solver,
            SolverVerdict::Redundant { redundant: 2, .. }
        ),
        "{:?}",
        analysis.solver
    );
    assert!(
        analysis.witnesses.is_empty(),
        "consistent surplus, no conflict"
    );
    assert!(analysis.mate_facts.iter().all(|f| f.satisfied));
    assert!(analysis
        .mate_facts
        .iter()
        .all(|f| f.role == MateRole::Redundant));
}

// ── Per-instance constrainment with twist-decoded motions ───────────────

#[test]
fn revolute_instance_reports_rotation_about_the_hinge_axis() {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    assembly.add_instance(part(1));
    // Hinge axis = world z through (2, 0, 0).
    assembly.add_mate(mate(
        MateKind::Revolute { limits: None },
        0,
        frame([2.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        1,
        frame([2.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ));
    let analysis = assembly.analyze_constrainedness();
    assert!(matches!(
        analysis.constrainedness,
        AssemblyConstrainedness::Mobile { dof: 1 }
    ));
    let status = analysis
        .instance_statuses
        .iter()
        .find(|s| s.instance == InstanceId(1))
        .cloned();
    let Some(status) = status else {
        assert!(false, "instance 1 must have a status");
        return;
    };
    let InstanceConstrainment::Mobile { dof, motions } = &status.constrainment else {
        assert!(false, "instance 1 is mobile: {status:?}");
        return;
    };
    assert_eq!(*dof, 1);
    assert_eq!(motions.len(), 1);
    let TwistMotion::RotationAbout { point, axis } = &motions[0] else {
        assert!(false, "a revolute frees a pure rotation: {motions:?}");
        return;
    };
    assert!(
        axis[2].abs() > 1.0 - 1e-6,
        "axis is world z (either sign): {axis:?}"
    );
    // The axis passes through (2, 0, z): its point projects onto x=2, y=0.
    assert!(
        (point[0] - 2.0).abs() < 1e-6 && point[1].abs() < 1e-6,
        "axis through the hinge at (2,0,·): {point:?}"
    );
}

#[test]
fn slider_instance_reports_translation_along_z() {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    assembly.add_instance(part(1));
    assembly.add_mate(mate(
        MateKind::Slider { limits: None },
        0,
        frame([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        1,
        frame([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ));
    let analysis = assembly.analyze_constrainedness();
    let status = analysis
        .instance_statuses
        .iter()
        .find(|s| s.instance == InstanceId(1))
        .cloned();
    let Some(status) = status else {
        assert!(false, "instance 1 must have a status");
        return;
    };
    let InstanceConstrainment::Mobile { dof, motions } = &status.constrainment else {
        assert!(false, "slider leaves 1 DOF: {status:?}");
        return;
    };
    assert_eq!(*dof, 1);
    let TwistMotion::TranslationAlong { direction } = &motions[0] else {
        assert!(false, "a slider frees a pure translation: {motions:?}");
        return;
    };
    assert!(
        direction[2].abs() > 1.0 - 1e-6,
        "slides along z: {direction:?}"
    );
}

#[test]
fn fastened_instance_is_fully_constrained_and_conflicted_instances_carry_via() {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    assembly.add_instance(part(1));
    assembly.add_instance(part(2));
    assembly.add_mate(fastened_at(0, [0.0, 0.0, 1.0], 1, [0.0, 0.0, 0.0]));
    // Conflicting pair on instance 2.
    assembly.add_mate(fastened_at(0, [5.0, 0.0, 0.0], 2, [0.0, 0.0, 0.0]));
    assembly.add_mate(fastened_at(0, [9.0, 0.0, 0.0], 2, [0.0, 0.0, 0.0]));

    let analysis = assembly.analyze_constrainedness();
    let of = |id: u32| {
        analysis
            .instance_statuses
            .iter()
            .find(|s| s.instance == InstanceId(id))
            .map(|s| s.constrainment.clone())
    };
    assert_eq!(of(1), Some(InstanceConstrainment::FullyConstrained));
    let Some(InstanceConstrainment::OverConstrained { via }) = of(2) else {
        assert!(
            false,
            "instance 2 is implicated by the witness: {analysis:?}"
        );
        return;
    };
    assert_eq!(via, vec![1, 2], "the witness mates implicate instance 2");
}

// ── Kernel-derived ε: the ε=0 default dies ──────────────────────────────

fn cube(h: f64) -> Mesh {
    Mesh {
        vertices: vec![
            [-h, -h, -h],
            [h, -h, -h],
            [h, h, -h],
            [-h, h, -h],
            [-h, -h, h],
            [h, -h, h],
            [h, h, h],
            [-h, h, h],
        ],
        triangles: vec![
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [2, 3, 7],
            [2, 7, 6],
            [1, 2, 6],
            [1, 6, 5],
            [3, 0, 4],
            [3, 4, 7],
        ],
    }
}

#[test]
fn epsilon_is_floored_by_the_kernel_and_only_raisable() {
    let mut assembly = Assembly::new(InstanceId(0));
    let mut g = Instance::new(InstanceId(0), "ground", cube(1.0));
    g.translation = [0.0, 0.0, 0.0];
    assembly.add_instance(g);
    let mut p = Instance::new(InstanceId(1), "top", cube(1.0));
    p.translation = [0.0, 0.0, 2.0];
    assembly.add_instance(p);
    assembly.add_mate(mate(
        MateKind::Concentric,
        0,
        FeatureRef::Axis {
            origin: [0.0, 0.0, 0.0],
            direction: [0.0, 0.0, 1.0],
        },
        1,
        FeatureRef::Axis {
            origin: [0.0, 0.0, 0.0],
            direction: [0.0, 0.0, 1.0],
        },
    ));

    // A request BELOW the kernel floor is clamped UP and recorded.
    let clamped = assembly.certify_v2(
        &[],
        EpsilonSpec {
            kernel_floor: 0.02,
            requested: Some(0.0), // the old default-0 lie
        },
    );
    let eps = clamped.epsilon.clone();
    let Some(eps) = eps else {
        assert!(false, "certificate v2 must record the ε fact");
        return;
    };
    assert_eq!(eps.effective, 0.02, "floored: {eps:?}");
    assert_eq!(eps.kernel_floor, 0.02);
    assert_eq!(eps.requested, Some(0.0));
    assert!(!eps.raised_by_caller);

    // A request ABOVE the floor is honoured.
    let raised = assembly.certify_v2(
        &[],
        EpsilonSpec {
            kernel_floor: 0.02,
            requested: Some(0.5),
        },
    );
    assert_eq!(raised.epsilon.clone().map(|e| e.effective), Some(0.5));
    assert_eq!(raised.epsilon.map(|e| e.raised_by_caller), Some(true));

    // And ε is LOAD-BEARING: a swept mechanism whose true clearance is
    // ~1.0 passes at the floor but fails once ε exceeds the clearance —
    // the conservative contract `certified = distance − ε`.
    let mechanism = |samples| assembly_engine::Mechanism {
        moving: InstanceId(1),
        joint: Joint::Prismatic {
            axis_origin: [0.0, 0.0, 0.0],
            axis_dir: [0.0, 0.0, 1.0],
        },
        base_translation: [0.0, 0.0, 3.0], // gap 1.0 above the ground cube
        base_rotation: [0.0, 0.0, 0.0, 1.0],
        range: (0.0, 0.5),
        samples,
    };
    let clear = assembly.certify_v2(
        &[mechanism(11)],
        EpsilonSpec {
            kernel_floor: 0.02,
            requested: None,
        },
    );
    assert!(clear.swept_clearance_ok, "1.0 gap clears ε=0.02");
    let smothered = assembly.certify_v2(
        &[mechanism(11)],
        EpsilonSpec {
            kernel_floor: 1.5,
            requested: None,
        },
    );
    assert!(
        !smothered.swept_clearance_ok,
        "ε=1.5 exceeds the 1.0 gap — the conservative bound must fail it"
    );
}

// ── Additive wire change: old payloads parse, new payloads superset ─────

#[test]
fn certificate_wire_change_is_additive() {
    // A pre-Slice-4 certificate JSON (the 9-boolean shape) still parses.
    let old_shape = r#"{
        "mates_consistent": true,
        "fully_grounded": true,
        "dof": 0,
        "mobility": "FullyConstrained",
        "no_static_interference": true,
        "swept_clearance_ok": true,
        "mates_anchored": true,
        "mates_in_contact": true
    }"#;
    let parsed: Result<assembly_engine::AssemblyCertificate, _> = serde_json::from_str(old_shape);
    let Ok(parsed) = parsed else {
        assert!(false, "old wire shape must keep parsing: {parsed:?}");
        return;
    };
    assert!(parsed.is_sound());
    assert!(parsed.constrainedness.is_none(), "v2 fields default empty");
    assert!(parsed.witnesses.is_empty());

    // A new certificate serialises to a SUPERSET of the old field set.
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    assembly.add_instance(part(1));
    assembly.add_mate(fastened_at(0, [0.0, 0.0, 1.0], 1, [0.0, 0.0, 0.0]));
    let cert = assembly.certify_v2(
        &[],
        EpsilonSpec {
            kernel_floor: 0.01,
            requested: None,
        },
    );
    let json = serde_json::to_value(&cert).unwrap_or(serde_json::Value::Null);
    for old_field in [
        "mates_consistent",
        "fully_grounded",
        "dof",
        "mobility",
        "no_static_interference",
        "swept_clearance_ok",
        "mates_anchored",
        "mates_in_contact",
        "mates_enforced",
    ] {
        assert!(json.get(old_field).is_some(), "{old_field} still present");
    }
    for new_field in [
        "constrainedness",
        "solver",
        "mate_facts",
        "instance_statuses",
        "witnesses",
        "structural",
        "decomposition",
        "epsilon",
    ] {
        assert!(json.get(new_field).is_some(), "{new_field} added");
    }
    // And the legacy entry point still stands (kernel_floor = its ε).
    let legacy = assembly.certify(&[], 0.01);
    assert_eq!(legacy.epsilon.map(|e| e.effective), Some(0.01));
}

// ── Structural + decomposition ride the certificate ─────────────────────

#[test]
fn certificate_carries_dual_dof_and_decomposition_stats() {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    let mut p = part(1);
    p.translation = [0.4, 0.2, 1.3];
    assembly.add_instance(p);
    assembly.add_mate(fastened_at(0, [0.0, 0.0, 1.0], 1, [0.0, 0.0, 0.0]));
    let cert = assembly.certify_v2(
        &[],
        EpsilonSpec {
            kernel_floor: 0.01,
            requested: None,
        },
    );
    let Some(structural) = cert.structural else {
        assert!(false, "dual DOF must ride the certificate");
        return;
    };
    assert_eq!(structural.structural_dof, 0);
    assert_eq!(structural.numeric_dof, 0);
    assert!(!structural.special_geometry);
    let Some(decomposition) = cert.decomposition else {
        assert!(false, "decomposition stats must ride the certificate");
        return;
    };
    assert_eq!(decomposition.extend_steps, 1);
    assert_eq!(
        cert.constrainedness,
        Some(AssemblyConstrainedness::FullyConstrained)
    );
    assert!(matches!(cert.solver, Some(SolverVerdict::Converged { .. })));
}

// ── Witness kind for statically-caught pairs ────────────────────────────

#[test]
fn witnesses_report_their_provenance_kind() {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    assembly.add_instance(part(1));
    assembly.add_mate(fastened_at(0, [0.0, 0.0, 1.0], 1, [0.0, 0.0, 0.0]));
    assembly.add_mate(fastened_at(0, [0.0, 0.0, 6.0], 1, [0.0, 0.0, 0.0]));
    let analysis = assembly.analyze_constrainedness();
    assert_eq!(
        analysis.witnesses.len(),
        1,
        "deduped: {:?}",
        analysis.witnesses
    );
    let w = &analysis.witnesses[0];
    assert!(
        matches!(
            w.kind,
            WitnessKind::NumericConflict | WitnessKind::StaticPair
        ),
        "{w:?}"
    );
    assert!(w.minimal);
}
