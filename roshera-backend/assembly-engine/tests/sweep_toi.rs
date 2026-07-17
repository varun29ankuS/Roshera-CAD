//! Slice-5 gate — motion-certified sweeps: nonlinear TOI (no tunneling),
//! joints DERIVED from mates, the constraint-manifold re-check that closes
//! the spec §2.2 wrong-axis hole, and motion-stamped interference facts
//! (spec §3.6 + §3.8 Slice 5).
//!
//! # Pre-implementation RED signatures (recorded 2026-07-17)
//!
//! `cargo test -p assembly-engine --test sweep_toi` failed to COMPILE:
//! E0432/E0599 — no `sweep_driven`, no `sweep_mechanism_checked`, no
//! `SweptFact`, and `AssemblyCertificate` had no `sweeps` field. The two
//! defects pinned behaviourally below existed at HEAD d6bc74ed:
//!
//! * **Tunneling** — `swept_clearance` (dense sampling, `sweep.rs:15-16`)
//!   certified the thin-blade fixture CLEAR through a wall it passes
//!   through between samples (the legacy pin
//!   `legacy_dense_sampler_misses_the_tunneling_blade` keeps that defect
//!   measured).
//! * **Wrong axis (§2.2)** — `certify_v2` swept an AUTHORED mechanism
//!   without ever re-checking the mates: a Revolute declared about a wrong
//!   axis certified `swept_clearance_ok: true` while the motion tore the
//!   assembly off its constraint manifold.

#[allow(dead_code)]
mod common;

use assembly_engine::{
    Assembly, DriveParam, EpsilonSpec, Instance, InstanceId, Joint, MateKind, Mechanism, Mesh,
    SweepMethod, SweepSource,
};
use common::{frame, mate};
use std::f64::consts::TAU;

/// An axis-aligned cuboid soup with the given half-extents, centred at the
/// local origin.
fn cuboid(hx: f64, hy: f64, hz: f64) -> Mesh {
    Mesh {
        vertices: vec![
            [-hx, -hy, -hz],
            [hx, -hy, -hz],
            [hx, hy, -hz],
            [-hx, hy, -hz],
            [-hx, -hy, hz],
            [hx, -hy, hz],
            [hx, hy, hz],
            [-hx, hy, hz],
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

fn instance_at(id: u32, mesh: Mesh, pos: [f64; 3]) -> Instance {
    let mut instance = Instance::new(InstanceId(id), format!("part_{id}"), mesh);
    instance.translation = pos;
    instance
}

/// The tunneling fixture: a small blade (half 0.2) swinging on a radius-10
/// circle about world z, driven by a REVOLUTE MATE (ground hub ↔ blade,
/// frames at the origin), plus a wall THIN in the tangential direction
/// parked at 92.5° ON the swing circle — squarely BETWEEN the 90° and 95°
/// samples of a 73-sample full-turn sweep.
fn blade_fixture() -> Assembly {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(instance_at(0, cuboid(0.5, 0.5, 0.5), [0.0, 0.0, 0.0]));
    assembly.add_instance(instance_at(1, cuboid(0.2, 0.2, 0.2), [10.0, 0.0, 0.0]));
    let phi = 92.5_f64.to_radians();
    assembly.add_instance(instance_at(
        2,
        cuboid(0.05, 0.5, 0.5),
        [10.0 * phi.cos(), 10.0 * phi.sin(), 0.0],
    ));
    // The blade's revolute: ground frame at the origin, blade frame at its
    // LOCAL [-10,0,0] — world-coincident, aligned, θ0 = 0.
    assembly.add_mate(mate(
        MateKind::Revolute { limits: None },
        0,
        frame([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        1,
        frame([-10.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ));
    assembly
}

// ── The tunneling RED ───────────────────────────────────────────────────

#[test]
fn legacy_dense_sampler_misses_the_tunneling_blade() {
    // THE DEFECT, measured (kept as the fixture's calibration pin): the
    // pre-slice-5 dense sampler reads every sample clear because the blade
    // passes THROUGH the wall strictly between the 90° and 95° samples.
    let assembly = blade_fixture();
    let legacy = assembly_engine::swept_clearance(
        &assembly,
        InstanceId(1),
        &Joint::Revolute {
            axis_origin: [0.0, 0.0, 0.0],
            axis_dir: [0.0, 0.0, 1.0],
        },
        &[10.0, 0.0, 0.0],
        &[0.0, 0.0, 0.0, 1.0],
        (0.0, TAU),
        73,
        0.02,
    );
    assert!(
        !legacy.collides,
        "the dense sampler MISSES the blade-through-wall pass \
         (raw_min = {}) — the defect TOI exists to kill",
        legacy.raw_min_clearance
    );
    assert!(legacy.raw_min_clearance > 0.05, "{legacy:?}");
}

#[test]
fn nonlinear_toi_catches_the_tunneling_blade() {
    let assembly = blade_fixture();
    let fact = assembly.sweep_driven(0, DriveParam::Rotation, (0.0, TAU), 73, 0.02);
    let Ok(fact) = fact else {
        assert!(false, "a revolute mate sweeps: {fact:?}");
        return;
    };
    assert!(matches!(fact.method, SweepMethod::NonlinearToi { .. }));
    assert!(!fact.clear, "TOI must catch the pass-through: {fact:?}");
    let Some(hit) = fact.first_contact else {
        assert!(false, "first contact is MOTION-STAMPED: {fact:?}");
        return;
    };
    // Contact happens as the blade enters the wall near 92.5° (± the
    // combined angular half-widths).
    assert!(
        hit.param > 1.55 && hit.param < 1.67,
        "contact stamped near 92.5° = 1.614 rad, got {}",
        hit.param
    );
    assert!(
        fact.manifold_violation.is_none(),
        "the motion is ON-manifold"
    );
    assert_eq!(fact.epsilon, 0.02, "ε recorded on the fact");
}

#[test]
fn certified_mechanism_sweep_also_catches_the_tunnel() {
    // The same catch through the certificate's authored-mechanism path
    // (correct axis, so the ONLY failure is the tunnel).
    let assembly = blade_fixture();
    let cert = assembly.certify_v2(
        &[Mechanism {
            moving: InstanceId(1),
            joint: Joint::Revolute {
                axis_origin: [0.0, 0.0, 0.0],
                axis_dir: [0.0, 0.0, 1.0],
            },
            base_translation: [10.0, 0.0, 0.0],
            base_rotation: [0.0, 0.0, 0.0, 1.0],
            range: (0.0, TAU),
            samples: 73,
        }],
        EpsilonSpec {
            kernel_floor: 0.02,
            requested: None,
        },
    );
    assert!(!cert.swept_clearance_ok, "TOI closes the tunnel in certify");
    assert!(
        cert.sweeps.iter().any(
            |s| matches!(s.source, SweepSource::Mechanism { moving } if moving == InstanceId(1))
                && s.first_contact.is_some()
        ),
        "the certificate carries the motion-stamped sweep fact: {:?}",
        cert.sweeps
    );
}

// ── The §2.2 wrong-axis RED ─────────────────────────────────────────────

#[test]
fn wrong_axis_mechanism_is_refused_not_certified() {
    // Pre-slice-5 signature: an AUTHORED Revolute about [5,0,0] — an axis
    // the revolute MATE (about the origin) forbids — swept with
    // `swept_clearance_ok: true` because nothing ever re-checked the mates
    // (spec §2.2). Now the manifold re-check refuses it, typed and stamped.
    let mut assembly = blade_fixture();
    // Park the wall far away so the ONLY verdict in play is the manifold.
    if let Some(wall) = assembly
        .instances
        .iter_mut()
        .find(|i| i.id == InstanceId(2))
    {
        wall.translation = [40.0, 0.0, 0.0];
    }
    let cert = assembly.certify_v2(
        &[Mechanism {
            moving: InstanceId(1),
            joint: Joint::Revolute {
                axis_origin: [5.0, 0.0, 0.0], // WRONG axis — mates forbid it
                axis_dir: [0.0, 0.0, 1.0],
            },
            base_translation: [10.0, 0.0, 0.0],
            base_rotation: [0.0, 0.0, 0.0, 1.0],
            range: (0.0, 0.6),
            samples: 21,
        }],
        EpsilonSpec {
            kernel_floor: 0.02,
            requested: None,
        },
    );
    assert!(
        !cert.swept_clearance_ok,
        "a motion the mates forbid must never certify clear (§2.2)"
    );
    let violated = cert.sweeps.iter().find_map(|s| s.manifold_violation);
    let Some(violation) = violated else {
        assert!(false, "the refusal is TYPED and stamped: {:?}", cert.sweeps);
        return;
    };
    assert!(violation.param > 0.0 && violation.param <= 0.6);
    assert!(
        violation.violation > 0.1,
        "the off-manifold tear is measured: {violation:?}"
    );

    // The CORRECT axis on the same fixture stays certified clear.
    let good = assembly.certify_v2(
        &[Mechanism {
            moving: InstanceId(1),
            joint: Joint::Revolute {
                axis_origin: [0.0, 0.0, 0.0],
                axis_dir: [0.0, 0.0, 1.0],
            },
            base_translation: [10.0, 0.0, 0.0],
            base_rotation: [0.0, 0.0, 0.0, 1.0],
            range: (0.0, 0.6),
            samples: 21,
        }],
        EpsilonSpec {
            kernel_floor: 0.02,
            requested: None,
        },
    );
    assert!(good.swept_clearance_ok, "{:?}", good.sweeps);
}

// ── Joints DERIVED from mates (certify sweeps them automatically) ───────

#[test]
fn derived_sweeps_respect_limits_and_stamp_interference() {
    // A revolute LIMITED to ±0.3 rad with an obstacle parked ON the swing
    // circle at +0.15 rad (inside the band): certify must derive the joint
    // FROM the mate (no authored mechanism), sweep the limit range, and
    // stamp the hit. The same obstacle at +1.57 rad (outside the band) is
    // unreachable and the assembly sweeps clear.
    let build = |obstacle_angle: f64| {
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(instance_at(0, cuboid(0.5, 0.5, 0.5), [0.0, 0.0, 0.0]));
        assembly.add_instance(instance_at(1, cuboid(0.2, 0.2, 0.2), [10.0, 0.0, 0.0]));
        assembly.add_instance(instance_at(
            2,
            cuboid(0.3, 0.3, 0.3),
            [
                10.0 * obstacle_angle.cos(),
                10.0 * obstacle_angle.sin(),
                0.0,
            ],
        ));
        assembly.add_mate(mate(
            MateKind::Revolute {
                limits: Some((-0.3, 0.3)),
            },
            0,
            frame([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
            1,
            frame([-10.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        ));
        assembly
    };
    let eps = EpsilonSpec {
        kernel_floor: 0.01,
        requested: None,
    };

    let blocked = build(0.15).certify_v2(&[], eps);
    let fact = blocked.sweeps.iter().find(|s| {
        matches!(
            s.source,
            SweepSource::DrivenMate {
                mate_index: 0,
                param: DriveParam::Rotation
            }
        )
    });
    let Some(fact) = fact else {
        assert!(
            false,
            "certify derives the joint FROM the mate: {:?}",
            blocked.sweeps
        );
        return;
    };
    assert!(
        (fact.range.0 - (-0.3)).abs() < 1e-9 && (fact.range.1 - 0.3).abs() < 1e-9,
        "the derived range IS the limit band: {fact:?}"
    );
    assert!(!fact.clear && !blocked.swept_clearance_ok);
    let stamp = fact
        .interference
        .first()
        .map(|i| i.at.param)
        .or(fact.first_contact.map(|c| c.param));
    let Some(stamp) = stamp else {
        assert!(false, "the hit is motion-stamped: {fact:?}");
        return;
    };
    assert!(
        stamp > 0.05 && stamp < 0.25,
        "'interpenetrates at θ≈0.15' — got θ = {stamp}"
    );

    let clear = build(1.57).certify_v2(&[], eps);
    assert!(
        clear.swept_clearance_ok,
        "outside the limit band the obstacle is unreachable: {:?}",
        clear.sweeps
    );
}

#[test]
fn unbounded_slider_travel_refuses_the_sweep_honestly() {
    // A slider with NO limits has unbounded travel — there is no finite
    // range to certify. The derived sweep must surface a TYPED refusal
    // (never an invented range, never a silent skip) and the refusal does
    // not fail soundness (mirrors mobility-reported-not-failed).
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(instance_at(0, cuboid(1.0, 1.0, 1.0), [0.0, 0.0, 0.0]));
    assembly.add_instance(instance_at(1, cuboid(1.0, 1.0, 1.0), [0.0, 0.0, 2.0]));
    assembly.add_mate(mate(
        MateKind::Slider { limits: None },
        0,
        frame([0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        1,
        frame([0.0, 0.0, -1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ));
    let cert = assembly.certify_v2(
        &[],
        EpsilonSpec {
            kernel_floor: 0.01,
            requested: None,
        },
    );
    let fact = cert
        .sweeps
        .iter()
        .find(|s| matches!(s.source, SweepSource::DrivenMate { mate_index: 0, .. }));
    let Some(fact) = fact else {
        assert!(false, "the unswept motion is VISIBLE: {:?}", cert.sweeps);
        return;
    };
    assert!(
        fact.refusal.is_some(),
        "unbounded travel refuses typed: {fact:?}"
    );
    assert!(
        cert.swept_clearance_ok,
        "an honest refusal is reported, not failed (mobility precedent)"
    );
}

// ── ε stays load-bearing; contact stays allowed ─────────────────────────

#[test]
fn epsilon_margin_still_fails_a_sub_epsilon_sweep() {
    // Blade on the radius-10 circle, obstacle OFF the circle with a true
    // ~2.2 gap at closest approach: ε below the gap certifies clear, ε
    // above it fails the margin — with NO contact and NO penetration (the
    // conservative `certified = distance − ε` contract, slice-4 pin kept).
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(instance_at(0, cuboid(0.5, 0.5, 0.5), [0.0, 0.0, 0.0]));
    assembly.add_instance(instance_at(1, cuboid(0.2, 0.2, 0.2), [10.0, 0.0, 0.0]));
    assembly.add_instance(instance_at(2, cuboid(0.5, 0.5, 0.5), [13.0, 0.0, 0.0]));
    assembly.add_mate(mate(
        MateKind::Revolute { limits: None },
        0,
        frame([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        1,
        frame([-10.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ));

    let clear = assembly.sweep_driven(0, DriveParam::Rotation, (0.0, TAU), 49, 0.02);
    let Ok(clear) = clear else {
        assert!(false, "{clear:?}");
        return;
    };
    assert!(clear.clear, "{clear:?}");
    let Some(margin) = clear.min_certified_clearance else {
        assert!(false, "separated pairs carry a certified margin: {clear:?}");
        return;
    };
    assert!(margin > 1.5, "true gap ≈ 2.2 − ε: {margin}");

    let smothered = assembly.sweep_driven(0, DriveParam::Rotation, (0.0, TAU), 49, 3.0);
    let Ok(smothered) = smothered else {
        assert!(false, "{smothered:?}");
        return;
    };
    assert!(
        !smothered.clear && smothered.first_contact.is_none() && smothered.interference.is_empty(),
        "ε above the gap fails the MARGIN, not a contact: {smothered:?}"
    );
}

#[test]
fn touching_hinge_pair_sweeps_clear() {
    // A lid seated flush on a base, revolute about their shared normal:
    // the pair is in CONTACT by design. Spinning the lid keeps flush
    // contact the whole turn — contact is mating, not collision, and the
    // sweep must not false-positive it.
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(instance_at(0, cuboid(1.0, 1.0, 1.0), [0.0, 0.0, 0.0]));
    assembly.add_instance(instance_at(1, cuboid(1.0, 1.0, 1.0), [0.0, 0.0, 2.0]));
    assembly.add_mate(mate(
        MateKind::Revolute { limits: None },
        0,
        frame([0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        1,
        frame([0.0, 0.0, -1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ));
    let fact = assembly.sweep_driven(0, DriveParam::Rotation, (0.0, TAU), 25, 0.01);
    let Ok(fact) = fact else {
        assert!(false, "{fact:?}");
        return;
    };
    assert!(
        fact.clear,
        "flush mating contact through the motion is NOT a collision: {fact:?}"
    );
    assert!(fact.first_contact.is_none() && fact.interference.is_empty());
}

// ── Manifold re-check on the driven path ────────────────────────────────

#[test]
fn driven_sweep_refuses_when_the_mechanism_cannot_follow() {
    // The arm is fastened AND revolute to ground: driving θ is unreachable
    // (the fastened lock fights the drive). The driven sweep must refuse
    // with a manifold violation, never certify a motion it could not make.
    let mut assembly = blade_fixture();
    assembly.add_mate(mate(
        MateKind::Fastened,
        0,
        frame([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        1,
        frame([-10.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ));
    let fact = assembly.sweep_driven(0, DriveParam::Rotation, (0.0, 1.0), 11, 0.02);
    let Ok(fact) = fact else {
        assert!(false, "{fact:?}");
        return;
    };
    assert!(!fact.clear, "{fact:?}");
    assert!(
        fact.manifold_violation.is_some(),
        "the stuck mechanism is a TYPED manifold refusal: {fact:?}"
    );
}
