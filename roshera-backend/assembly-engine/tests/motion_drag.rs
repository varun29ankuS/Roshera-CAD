//! Slice-5 gate — kinematic drag: driven joint parameters, limits,
//! cluster-scoped re-solve, multi-turn winding, singular-stroke honesty
//! (spec §3.4 "Driven vs driving" + §3.8 Slice 5; premises #1/#5 of the
//! slice-3/4 report).
//!
//! # Pre-implementation RED signatures (recorded 2026-07-17)
//!
//! `cargo test -p assembly-engine --test motion_drag` failed to COMPILE:
//! E0432 (unresolved imports `assembly_engine::{DriveParam, DriveRefusal}`)
//! + E0599 (`no method named `drag` found for struct `Assembly``) — no
//! drag surface existed at HEAD d6bc74ed. The behavioural holes pinned
//! below (winding, limits, scope, singular strokes) had NO mechanism to
//! even express them.

mod common;

use assembly_engine::{Assembly, DriveParam, DriveRefusal, InstanceId, MateKind};
use common::{frame, mate, part, revolute_at};
use std::f64::consts::{PI, TAU};

/// A revolute about world z through the origin between ground 0 and arm 1
/// (frames coincident and aligned at the identity poses: θ0 = 0).
fn hinge(kind: MateKind) -> Assembly {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    assembly.add_instance(part(1));
    assembly.add_mate(mate(
        kind,
        0,
        frame([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        1,
        frame([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ));
    assembly
}

fn joint_of(assembly: &Assembly, mate_index: u32) -> (f64, f64) {
    assembly
        .joint_parameters_of(mate_index)
        .unwrap_or((f64::NAN, f64::NAN))
}

// ── Driving each parameter of the taxonomy ──────────────────────────────

#[test]
fn revolute_drag_rotates_the_arm_to_theta() {
    let mut assembly = hinge(MateKind::Revolute { limits: None });
    let outcome = assembly.drag(0, DriveParam::Rotation, 0.7);
    let Ok(outcome) = outcome else {
        assert!(false, "revolute rotation is driveable: {outcome:?}");
        return;
    };
    assert!(outcome.report.converged, "{outcome:?}");
    assert!((outcome.applied - 0.7).abs() < 1e-9, "{outcome:?}");
    assert!(outcome.limit.is_none(), "no limits declared");
    let (theta, s) = joint_of(&assembly, 0);
    assert!((theta - 0.7).abs() < 1e-6, "θ driven to 0.7, got {theta}");
    assert!(s.abs() < 1e-6, "slide untouched, got {s}");
    // The arm's quaternion is a rotation about z by 0.7.
    let r = assembly
        .instance(InstanceId(1))
        .map(|i| i.rotation)
        .unwrap_or([f64::NAN; 4]);
    let angle = 2.0 * r[3].clamp(-1.0, 1.0).acos();
    assert!(
        (angle - 0.7).abs() < 1e-6 && r[0].abs() < 1e-9 && r[1].abs() < 1e-9,
        "arm rotated 0.7 about z, got {r:?}"
    );
    // The driven mate itself still HOLDS at the new pose (the drive rides
    // the joint's own free motion — never off-manifold).
    let violation = assembly
        .mates
        .first()
        .map(|m| assembly.mate_violation(m))
        .unwrap_or(f64::NAN);
    assert!(violation < 1e-8, "revolute still holds: {violation}");
}

#[test]
fn slider_drag_translates_along_z() {
    let mut assembly = hinge(MateKind::Slider { limits: None });
    let outcome = assembly.drag(0, DriveParam::Translation, 3.5);
    let Ok(outcome) = outcome else {
        assert!(false, "slider translation is driveable: {outcome:?}");
        return;
    };
    assert!(outcome.report.converged, "{outcome:?}");
    let (theta, s) = joint_of(&assembly, 0);
    assert!((s - 3.5).abs() < 1e-6, "s driven to 3.5, got {s}");
    assert!(theta.abs() < 1e-6, "spin untouched, got {theta}");
    let t = assembly
        .instance(InstanceId(1))
        .map(|i| i.translation)
        .unwrap_or([f64::NAN; 3]);
    assert!(
        t[0].abs() < 1e-6 && t[1].abs() < 1e-6 && (t[2] - 3.5).abs() < 1e-6,
        "arm slid to z=3.5, got {t:?}"
    );
}

#[test]
fn cylindrical_drag_drives_each_parameter_independently() {
    let mut assembly = hinge(MateKind::Cylindrical {
        rot_limits: None,
        trans_limits: None,
    });
    let spin = assembly.drag(0, DriveParam::Rotation, 0.5);
    assert!(spin.map(|o| o.report.converged).unwrap_or(false));
    let (theta, s) = joint_of(&assembly, 0);
    assert!((theta - 0.5).abs() < 1e-6, "θ=0.5, got {theta}");
    assert!(s.abs() < 1e-6, "slide FREE and untouched, got {s}");

    let slide = assembly.drag(0, DriveParam::Translation, 2.0);
    assert!(slide.map(|o| o.report.converged).unwrap_or(false));
    let (theta, s) = joint_of(&assembly, 0);
    assert!((s - 2.0).abs() < 1e-6, "s=2.0, got {s}");
    assert!(
        (theta - 0.5).abs() < 1e-6,
        "spin FREE and kept at 0.5, got {theta}"
    );
}

// ── Limits ──────────────────────────────────────────────────────────────

#[test]
fn limits_clamp_the_target_and_report_the_hit() {
    let mut assembly = hinge(MateKind::Revolute {
        limits: Some((-0.5, 0.5)),
    });
    let outcome = assembly.drag(0, DriveParam::Rotation, 1.2);
    let Ok(outcome) = outcome else {
        assert!(
            false,
            "a beyond-limit target CLAMPS, never errors: {outcome:?}"
        );
        return;
    };
    assert!((outcome.applied - 0.5).abs() < 1e-9, "{outcome:?}");
    let Some(limit) = outcome.limit else {
        assert!(false, "the at-limit fact must be reported: {outcome:?}");
        return;
    };
    assert_eq!(limit.requested, 1.2);
    assert_eq!((limit.min, limit.max), (-0.5, 0.5));
    let (theta, _) = joint_of(&assembly, 0);
    assert!((theta - 0.5).abs() < 1e-6, "clamped at max, got {theta}");

    // Inside the band: no fact.
    let inside = assembly.drag(0, DriveParam::Rotation, -0.25);
    assert!(inside.map(|o| o.limit.is_none()).unwrap_or(false));
}

// ── Typed refusals ──────────────────────────────────────────────────────

#[test]
fn undriveable_requests_refuse_typed() {
    // A Planar mate has no scalar joint parameter to drive.
    let mut planar = hinge(MateKind::Planar);
    assert!(matches!(
        planar.drag(0, DriveParam::Rotation, 0.3),
        Err(DriveRefusal::NotDriveable { .. })
    ));
    // A Revolute exposes no translational parameter.
    let mut rev = hinge(MateKind::Revolute { limits: None });
    assert!(matches!(
        rev.drag(0, DriveParam::Translation, 1.0),
        Err(DriveRefusal::NotDriveable { .. })
    ));
    // The honest-refuse set is not enforced, hence not driveable.
    let mut cam = hinge(MateKind::Cam);
    assert!(matches!(
        cam.drag(0, DriveParam::Rotation, 0.3),
        Err(DriveRefusal::NotEnforced { .. })
    ));
    // Unknown mate index.
    assert!(matches!(
        rev.drag(7, DriveParam::Rotation, 0.3),
        Err(DriveRefusal::UnknownMate { .. })
    ));
    // A coupling relates OTHER mates' parameters — drive the base joint.
    let mut geared = hinge(MateKind::Revolute { limits: None });
    geared.add_instance(part(2));
    geared.add_mate(mate(
        MateKind::Revolute { limits: None },
        0,
        frame([5.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        2,
        frame([5.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ));
    geared.add_mate(mate(
        MateKind::GearRatio {
            ratio: 1.0,
            at: [0.0, 0.0],
            couples: [0, 1],
        },
        0,
        frame([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        2,
        frame([5.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ));
    assert!(matches!(
        geared.drag(2, DriveParam::Rotation, 0.3),
        Err(DriveRefusal::NotDriveable { .. })
    ));
}

// ── Cluster-scoped re-solve (the sketch drag-scoping precedent) ─────────

#[test]
fn drag_resolve_is_scoped_to_the_affected_component() {
    // Component A: arm 1 revolute to ground. Component B: part 2 fastened
    // to ground (seated) and part 3 fastened to 2 at a DELIBERATELY WRONG
    // pose (violation planted). A scoped drag of component A must (a)
    // instrument exactly {1} as its scope, (b) leave component B's poses
    // byte-identical, and (c) leave B's planted violation UNREPAIRED — a
    // whole-assembly re-solve would seat part 3 (the mutation this pins).
    let mut assembly = hinge(MateKind::Revolute { limits: None });
    let mut p2 = part(2);
    p2.translation = [20.0, 0.0, 0.0];
    assembly.add_instance(p2);
    let mut p3 = part(3);
    p3.translation = [20.0, 0.0, 4.0]; // WRONG: fastened wants z = 1
    assembly.add_instance(p3);
    assembly.add_mate(mate(
        MateKind::Fastened,
        0,
        frame([20.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        2,
        frame([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ));
    assembly.add_mate(mate(
        MateKind::Fastened,
        2,
        frame([0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        3,
        frame([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ));
    let planted = assembly
        .mates
        .get(2)
        .map(|m| assembly.mate_violation(m))
        .unwrap_or(0.0);
    assert!(planted > 1.0, "the plant is live: {planted}");

    let before: Vec<([f64; 3], [f64; 4])> = assembly
        .instances
        .iter()
        .filter(|i| i.id.0 >= 2)
        .map(|i| (i.translation, i.rotation))
        .collect();
    let outcome = assembly.drag(0, DriveParam::Rotation, 0.4);
    let Ok(outcome) = outcome else {
        assert!(false, "drag runs: {outcome:?}");
        return;
    };
    assert!(outcome.report.converged);
    assert_eq!(
        outcome.scope.instances,
        vec![InstanceId(1)],
        "scope = the driven component only: {:?}",
        outcome.scope
    );
    assert_eq!(
        outcome.scope.mates,
        vec![0],
        "scoped mates: {:?}",
        outcome.scope
    );
    let after: Vec<([f64; 3], [f64; 4])> = assembly
        .instances
        .iter()
        .filter(|i| i.id.0 >= 2)
        .map(|i| (i.translation, i.rotation))
        .collect();
    assert_eq!(before, after, "component B untouched — byte-identical");
    let still_planted = assembly
        .mates
        .get(2)
        .map(|m| assembly.mate_violation(m))
        .unwrap_or(0.0);
    assert!(
        (still_planted - planted).abs() < 1e-12,
        "the planted violation must NOT be repaired by a scoped drag"
    );
}

// ── Multi-turn winding (premise #5: coupling θ wraps at ±π) ─────────────

#[test]
fn screw_coupling_survives_multi_turn_winding() {
    // A cylindrical joint with a Screw coupling (lead 2.0): two full turns
    // must advance the nut by 2 leads. WITHOUT winding handling the wrapped
    // θ ∈ (−π, π] snaps the coupling target back every half-turn and the
    // nut ends near s ≈ 0 — the premise-#5 corruption this pins.
    let mut assembly = hinge(MateKind::Cylindrical {
        rot_limits: None,
        trans_limits: None,
    });
    assembly.add_mate(mate(
        MateKind::Screw {
            lead: 2.0,
            at: [0.0, 0.0],
            couples: 0,
        },
        0,
        frame([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        1,
        frame([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ));

    let outcome = assembly.drag(0, DriveParam::Rotation, 2.0 * TAU);
    let Ok(outcome) = outcome else {
        assert!(false, "screw drive runs: {outcome:?}");
        return;
    };
    assert!(outcome.report.converged, "{outcome:?}");
    let (_, s) = joint_of(&assembly, 0);
    assert!(
        (s - 4.0).abs() < 1e-5,
        "two turns × lead 2.0 ⇒ s = 4.0, got {s} (wrapped-θ corruption)"
    );
    // The winding is REPORTED (the document layer persists the rebased
    // coupling reference so the state stays consistent across calls).
    assert!(
        outcome
            .windings
            .iter()
            .any(|w| w.mate_index == 1 && w.turns != 0),
        "winding fact for the screw coupling: {:?}",
        outcome.windings
    );
    // The coupling mate itself is SATISFIED at the wound pose — the state
    // the drag leaves behind is consistent, not a wrapped-θ time bomb.
    let coupling_violation = assembly
        .mates
        .get(1)
        .map(|m| assembly.mate_violation(m))
        .unwrap_or(f64::NAN);
    assert!(
        coupling_violation < 1e-6,
        "coupling holds at the wound pose: {coupling_violation}"
    );

    // And back down: unwinding returns the nut home.
    let back = assembly.drag(0, DriveParam::Rotation, 0.0);
    assert!(back.map(|o| o.report.converged).unwrap_or(false));
    let (_, s) = joint_of(&assembly, 0);
    assert!(s.abs() < 1e-5, "unwound home, got s = {s}");
}

// ── Configuration-sensitive rank (premise #1) ───────────────────────────

/// The 10×10 parallelogram four-bar of `tests/decomposition.rs`, poses
/// satisfied: link 1 at the origin, link 2 at (0,10), link 3 at (10,10).
fn parallelogram() -> Assembly {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    assembly.add_instance(part(1));
    let mut p2 = part(2);
    p2.translation = [0.0, 10.0, 0.0];
    assembly.add_instance(p2);
    let mut p3 = part(3);
    p3.translation = [10.0, 10.0, 0.0];
    assembly.add_instance(p3);
    assembly.add_mate(revolute_at(0, [0.0, 0.0, 0.0], 1, [0.0, 0.0, 0.0]));
    assembly.add_mate(revolute_at(1, [0.0, 10.0, 0.0], 2, [0.0, 0.0, 0.0]));
    assembly.add_mate(revolute_at(2, [10.0, 0.0, 0.0], 3, [0.0, 0.0, 0.0]));
    assembly.add_mate(revolute_at(3, [0.0, -10.0, 0.0], 0, [10.0, 0.0, 0.0]));
    assembly
}

#[test]
fn stroke_into_a_singular_pose_is_detected_and_reported() {
    // Driving the crank +90° STRETCHES the four-bar collinear — the
    // configuration whose instantaneous numeric DOF is 2, not the schematic
    // 1 (the slice-3 fixture lesson, premise #1). The stroke must complete
    // WITHOUT silent corruption and must REPORT the rank change.
    let mut assembly = parallelogram();
    let outcome = assembly.drag(0, DriveParam::Rotation, PI / 2.0);
    let Ok(outcome) = outcome else {
        assert!(false, "the stretched pose is reachable: {outcome:?}");
        return;
    };
    assert!(outcome.report.converged, "{outcome:?}");
    // Every mate still holds at the stretched pose (no corruption).
    for (idx, m) in assembly.mates.iter().enumerate() {
        let v = assembly.mate_violation(m);
        assert!(v < 1e-6, "mate {idx} holds at the singular pose: {v}");
    }
    assert!(
        !outcome.rank_transitions.is_empty(),
        "the rank change along the stroke must be REPORTED: {outcome:?}"
    );
    let last = outcome
        .rank_transitions
        .last()
        .copied()
        .unwrap_or_else(|| unreachable!("non-empty checked above"));
    assert!(
        last.dof_after > last.dof_before,
        "stretch GAINS an instantaneous DOF (1 → 2): {last:?}"
    );
    assert!(
        (last.param - PI / 2.0).abs() < 0.6,
        "the transition is stamped near the stretch: {last:?}"
    );

    // A generic (non-singular) stroke reports NO transitions.
    let mut generic = parallelogram();
    let quiet = generic.drag(0, DriveParam::Rotation, 0.4);
    assert!(
        quiet
            .map(|o| o.rank_transitions.is_empty())
            .unwrap_or(false),
        "a generic stroke has nothing to report"
    );
}

// ── Honest failure: poses restored ──────────────────────────────────────

#[test]
fn an_unreachable_drag_restores_the_poses() {
    // The arm is BOTH fastened and revolute to ground (both hold at θ=0).
    // Driving θ to 0.5 fights the fastened lock — the drive is unreachable.
    // The drag must report the failure AND leave every pose exactly as it
    // found them (never a silently corrupted half-stroke).
    let mut assembly = hinge(MateKind::Revolute { limits: None });
    assembly.add_mate(mate(
        MateKind::Fastened,
        0,
        frame([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        1,
        frame([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ));
    let before: Vec<([f64; 3], [f64; 4])> = assembly
        .instances
        .iter()
        .map(|i| (i.translation, i.rotation))
        .collect();
    let outcome = assembly.drag(0, DriveParam::Rotation, 0.5);
    let Ok(outcome) = outcome else {
        assert!(
            false,
            "an unreachable drive REPORTS, not errors: {outcome:?}"
        );
        return;
    };
    assert!(!outcome.report.converged, "unreachable: {outcome:?}");
    assert!(outcome.report.final_residual_norm > 1e-3);
    let after: Vec<([f64; 3], [f64; 4])> = assembly
        .instances
        .iter()
        .map(|i| (i.translation, i.rotation))
        .collect();
    assert_eq!(before, after, "a failed drag restores every pose");
    // FeatureRef untouched: the drive substitution never leaks into the
    // declared mates.
    assert!(matches!(
        assembly.mates.first().map(|m| m.kind),
        Some(MateKind::Revolute { .. })
    ));
}
