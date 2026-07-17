//! Analytic-vs-FD Jacobian agreement gate (kinematic-assembly campaign,
//! Slice 3 — spec §3.4).
//!
//! Every residual row in the mate taxonomy gets a closed-form se(3)
//! tangent-space derivative (`jacobian.rs`); central finite differences
//! are RETAINED as the debug oracle. This gate pins, for every mate kind
//! at GENERIC configurations (translated + rotated off-axis, satisfied
//! AND violated), that the two Jacobians agree entrywise to ≤ 1e-6 —
//! the spec's Slice-3 agreement bound.
//!
//! Pre-implementation signature (captured 2026-07-17, post-91f3c588
//! tree): `Assembly::jacobian_probe` did not exist — the solver had ONLY
//! the central-difference Jacobian (12 residual-stack evaluations per
//! instance per iteration, `solver.rs:118-134`), so this file failed to
//! compile (E0599). The gate is therefore a genuine RED: it can only
//! pass once the analytic derivation exists AND matches the oracle.

use assembly_engine::{Assembly, FeatureRef, Instance, InstanceId, Mate, MateKind, Mesh};

/// The spec's Slice-3 agreement bound. Central differences at EPS=1e-6
/// carry O(EPS²)·‖∂³g‖ truncation error ≈ 1e-12 on unit-scale residuals
/// and O(machine_eps/EPS) ≈ 1e-10 rounding error; 1e-6 leaves four
/// orders of headroom while still catching any wrong/missing term
/// (a dropped lever arm or sign flip shows up at O(1)).
const GATE: f64 = 1e-6;

fn part(id: u32) -> Instance {
    Instance::new(InstanceId(id), format!("part_{id}"), Mesh::default())
}

/// Unit quaternion `[x,y,z,w]` for a rotation of `angle_deg` about `axis`.
fn quat(axis: [f64; 3], angle_deg: f64) -> [f64; 4] {
    let n = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    let half = angle_deg.to_radians() / 2.0;
    let s = half.sin() / n;
    [axis[0] * s, axis[1] * s, axis[2] * s, half.cos()]
}

fn frame(origin: [f64; 3], z: [f64; 3], x: [f64; 3]) -> FeatureRef {
    FeatureRef::Frame {
        origin,
        z_axis: z,
        x_axis: x,
    }
}

/// A GENERIC connector frame: off-origin (so every lever-arm term
/// `r = p − t` is nonzero) and off-axis (so no cross-product row
/// degenerates). Orthonormality of (z, x) is exact by construction.
fn id_frame() -> FeatureRef {
    frame([1.7, -0.9, 2.3], [0.0, 0.6, 0.8], [1.0, 0.0, 0.0])
}

/// A second generic frame, differently placed and oriented.
fn id_frame_b() -> FeatureRef {
    frame([-0.8, 1.4, 0.6], [0.8, 0.0, 0.6], [0.6, 0.0, -0.8])
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

/// TWO MOVING bodies at GENERIC poses (translated, rotated about skew
/// axes — nothing axis-aligned, nothing satisfied), mated to each other,
/// grounded on a SEPARATE datum instance. Both sides of the mate are
/// non-ground, so BOTH sides' gradient blocks enter the Jacobian — a
/// ground-side fixture would silently discard one side's derivation
/// (caught by mutation proofing: the first fixture generation did
/// exactly that and let a dropped lever-arm term survive).
fn generic_two_body(kind: MateKind, fa: FeatureRef, fb: FeatureRef) -> Assembly {
    let mut assembly = Assembly::new(InstanceId(9));
    let mut g = part(0);
    g.translation = [0.3, -0.7, 1.1];
    g.rotation = quat([1.0, 2.0, 3.0], 17.0);
    assembly.add_instance(g);
    let mut p = part(1);
    p.translation = [4.2, -1.3, 2.6];
    p.rotation = quat([-2.0, 1.0, 1.5], 31.0);
    assembly.add_instance(p);
    assembly.add_instance(part(9)); // datum ground — not in the mate
    assembly.add_mate(mate(kind, 0, fa, 1, fb));
    assembly
}

/// Assert the analytic and central-difference Jacobians agree entrywise.
fn assert_gate(assembly: &Assembly, label: &str) {
    let probe = assembly.jacobian_probe();
    assert!(
        probe.rows > 0
            || !assembly
                .mates
                .iter()
                .any(|m| m.kind.is_numerically_enforced()),
        "{label}: an enforced mate must contribute rows"
    );
    assert!(
        probe.max_abs_disagreement <= GATE,
        "{label}: analytic vs FD disagreement {} > {GATE} ({}x{} Jacobian)",
        probe.max_abs_disagreement,
        probe.rows,
        probe.cols
    );
}

// ── Frame-pair joint kinds at generic poses ─────────────────────────────

#[test]
fn fastened_agrees() {
    let a = generic_two_body(MateKind::Fastened, id_frame(), id_frame_b());
    assert_gate(&a, "Fastened");
}

#[test]
fn revolute_agrees() {
    let a = generic_two_body(
        MateKind::Revolute { limits: None },
        frame([1.0, 0.5, -0.2], [0.0, 0.6, 0.8], [1.0, 0.0, 0.0]),
        id_frame(),
    );
    assert_gate(&a, "Revolute");
}

#[test]
fn slider_agrees() {
    let a = generic_two_body(
        MateKind::Slider { limits: None },
        id_frame(),
        frame([0.4, 0.0, 2.0], [0.6, 0.0, 0.8], [0.8, 0.0, -0.6]),
    );
    assert_gate(&a, "Slider");
}

#[test]
fn cylindrical_agrees() {
    let a = generic_two_body(
        MateKind::Cylindrical {
            rot_limits: None,
            trans_limits: None,
        },
        id_frame(),
        id_frame_b(),
    );
    assert_gate(&a, "Cylindrical");
}

#[test]
fn planar_agrees() {
    let a = generic_two_body(MateKind::Planar, id_frame(), id_frame_b());
    assert_gate(&a, "Planar");
}

#[test]
fn ball_agrees() {
    let a = generic_two_body(MateKind::Ball, id_frame(), id_frame_b());
    assert_gate(&a, "Ball");
}

#[test]
fn pinslot_agrees_both_slot_directions() {
    for slot_dir_x in [true, false] {
        let a = generic_two_body(
            MateKind::PinSlot {
                slot_dir_x,
                limits: None,
            },
            id_frame(),
            id_frame_b(),
        );
        assert_gate(&a, &format!("PinSlot(slot_dir_x={slot_dir_x})"));
    }
}

// ── Dimensional overlays ────────────────────────────────────────────────

#[test]
fn distance_agrees() {
    let a = generic_two_body(MateKind::Distance { value: 2.5 }, id_frame(), id_frame_b());
    assert_gate(&a, "Distance");
}

#[test]
fn angle_agrees() {
    let a = generic_two_body(
        MateKind::Angle {
            value: std::f64::consts::FRAC_PI_3,
        },
        id_frame(),
        frame([0.0, 0.0, 0.0], [0.0, 0.6, 0.8], [0.0, 0.8, -0.6]),
    );
    assert_gate(&a, "Angle");
}

#[test]
fn parallel_agrees() {
    let a = generic_two_body(
        MateKind::Parallel,
        id_frame(),
        frame([0.0, 0.0, 0.0], [0.28, 0.0, 0.96], [0.96, 0.0, -0.28]),
    );
    assert_gate(&a, "Parallel");
}

#[test]
fn tangent_agrees_away_from_crossing() {
    // Generic pose keeps |d·z| well away from 0 — the smooth branch.
    let a = generic_two_body(MateKind::Tangent { radius: 3.0 }, id_frame(), id_frame_b());
    assert_gate(&a, "Tangent (smooth branch)");
}

#[test]
fn tangent_at_exact_crossing_uses_the_central_subgradient() {
    // |d·z_a| is non-smooth exactly at the plane crossing (slice-1/2
    // report residual #4). The analytic row takes the central
    // subgradient σ(0) = 0 — the same value central differencing
    // measures — so the gate holds even AT the crossing, and the choice
    // is documented rather than accidental.
    let mut assembly = Assembly::new(InstanceId(9));
    assembly.add_instance(part(0));
    assembly.add_instance(part(1)); // identity pose: d·z = 0 exactly
    assembly.add_instance(part(9)); // datum ground — both mate sides move
    assembly.add_mate(mate(
        MateKind::Tangent { radius: 3.0 },
        0,
        id_frame(),
        1,
        id_frame(),
    ));
    assert_gate(&assembly, "Tangent (at crossing)");
}

// ── Legacy Face/Axis kinds ──────────────────────────────────────────────

#[test]
fn concentric_agrees() {
    let a = generic_two_body(
        MateKind::Concentric,
        FeatureRef::Axis {
            origin: [0.5, -0.5, 0.0],
            direction: [0.0, 0.0, 2.0], // non-unit on purpose: to_world normalizes
        },
        FeatureRef::Axis {
            origin: [0.0, 0.0, 0.0],
            direction: [0.0, 0.5, 1.0],
        },
    );
    assert_gate(&a, "Concentric");
}

#[test]
fn coincident_agrees() {
    let a = generic_two_body(
        MateKind::Coincident,
        FeatureRef::Face {
            point: [0.0, 0.0, 1.0],
            normal: [0.0, 0.0, 1.0],
        },
        FeatureRef::Face {
            point: [0.2, 0.4, 0.0],
            normal: [0.0, 0.6, -0.8],
        },
    );
    assert_gate(&a, "Coincident");
}

#[test]
fn fixed_agrees() {
    let a = generic_two_body(
        MateKind::Fixed,
        FeatureRef::Face {
            point: [0.0, 0.0, 1.0],
            normal: [0.0, 0.0, 1.0],
        },
        FeatureRef::Face {
            point: [0.0, 0.0, -1.0],
            normal: [0.6, 0.0, -0.8],
        },
    );
    assert_gate(&a, "Fixed");
}

// ── Couplings (rows over the COUPLED mates' four/two instances) ─────────

/// Two revolutes to ground on parallel axes + a generic third-body pose.
fn geared_assembly() -> Assembly {
    let mut assembly = Assembly::new(InstanceId(9));
    let mut carrier = part(0);
    carrier.translation = [0.5, 0.8, -0.4];
    carrier.rotation = quat([1.0, -1.0, 2.0], 11.0);
    assembly.add_instance(carrier);
    assembly.add_instance(part(9)); // datum ground
    let mut g1 = part(1);
    g1.translation = [0.2, -0.1, 0.3];
    g1.rotation = quat([0.0, 0.0, 1.0], 23.0);
    assembly.add_instance(g1);
    let mut g2 = part(2);
    g2.translation = [29.0, 0.4, -0.2];
    g2.rotation = quat([0.0, 0.0, 1.0], -12.0);
    assembly.add_instance(g2);
    assembly.add_mate(mate(
        MateKind::Revolute { limits: None },
        0,
        id_frame(),
        1,
        id_frame(),
    ));
    assembly.add_mate(mate(
        MateKind::Revolute { limits: None },
        0,
        frame([30.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        2,
        id_frame(),
    ));
    assembly
}

#[test]
fn gear_ratio_agrees() {
    let mut a = geared_assembly();
    a.add_mate(mate(
        MateKind::GearRatio {
            ratio: 2.0,
            at: [0.1, -0.05],
            couples: [0, 1],
        },
        1,
        id_frame(),
        2,
        id_frame(),
    ));
    assert_gate(&a, "GearRatio");
}

#[test]
fn rack_pinion_agrees() {
    let mut assembly = Assembly::new(InstanceId(9));
    let mut carrier = part(0);
    carrier.translation = [-0.3, 0.2, 0.9];
    carrier.rotation = quat([2.0, 1.0, -1.0], 7.0);
    assembly.add_instance(carrier);
    assembly.add_instance(part(9)); // datum ground
    let mut pinion = part(1);
    pinion.rotation = quat([0.0, 0.0, 1.0], 9.0);
    assembly.add_instance(pinion);
    let mut rack = part(2);
    rack.translation = [0.7, 10.2, 0.1];
    assembly.add_instance(rack);
    assembly.add_mate(mate(
        MateKind::Revolute { limits: None },
        0,
        id_frame(),
        1,
        id_frame(),
    ));
    assembly.add_mate(mate(
        MateKind::Slider { limits: None },
        0,
        frame([0.0, 10.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        2,
        frame([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ));
    assembly.add_mate(mate(
        MateKind::RackPinion {
            pinion_radius: 5.0,
            at: [0.0, 0.0],
            couples: [0, 1],
        },
        1,
        id_frame(),
        2,
        id_frame(),
    ));
    assert_gate(&assembly, "RackPinion");
}

#[test]
fn screw_agrees() {
    let mut a = generic_two_body(
        MateKind::Cylindrical {
            rot_limits: None,
            trans_limits: None,
        },
        id_frame(),
        id_frame(),
    );
    a.add_mate(mate(
        MateKind::Screw {
            lead: 1.5,
            at: [0.0, 0.0],
            couples: 0,
        },
        0,
        id_frame(),
        1,
        id_frame(),
    ));
    assert_gate(&a, "Screw");
}

// ── Refuse set + satisfied configurations ───────────────────────────────

#[test]
fn refused_kinds_contribute_no_rows() {
    for kind in [MateKind::Cam, MateKind::Path, MateKind::Symmetric] {
        let a = generic_two_body(kind, id_frame(), id_frame());
        let probe = a.jacobian_probe();
        assert_eq!(probe.rows, 0, "{kind:?} must contribute zero rows");
        assert_eq!(probe.max_abs_disagreement, 0.0);
    }
}

#[test]
fn satisfied_configurations_agree_too() {
    // At the SOLVED configuration the Jacobian feeds the rank/DOF verdict —
    // agreement there is what keeps the certificate honest.
    for kind in [
        MateKind::Fastened,
        MateKind::Revolute { limits: None },
        MateKind::Cylindrical {
            rot_limits: None,
            trans_limits: None,
        },
        MateKind::Planar,
        MateKind::Ball,
    ] {
        let mut assembly = Assembly::new(InstanceId(9));
        assembly.add_instance(part(0));
        assembly.add_instance(part(1));
        assembly.add_instance(part(9)); // datum ground — both mate sides move
        assembly.add_mate(mate(kind, 0, id_frame(), 1, id_frame()));
        assert_gate(&assembly, &format!("{kind:?} (satisfied)"));
    }
}

// ── The solver actually RUNS on the analytic Jacobian ───────────────────

#[test]
fn dof_analysis_and_solve_use_the_analytic_jacobian() {
    // Not just agreement — the shipped rank/solve path must consume the
    // analytic Jacobian (FD retires to the debug oracle). The probe
    // reports which path `dof_analysis`/`solve` use.
    let a = generic_two_body(MateKind::Fastened, id_frame(), id_frame_b());
    let probe = a.jacobian_probe();
    assert!(
        probe.solver_uses_analytic,
        "the production solve/DOF path must run on the analytic Jacobian"
    );
}
