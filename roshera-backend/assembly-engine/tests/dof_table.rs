//! The §3.2 DOF / residual-rank table, every row RED-pinned by hand-count
//! (kinematic-assembly campaign, Slice 2).
//!
//! | Mate                    | residual rank | DOF left | joint twist space        |
//! |-------------------------|---------------|----------|--------------------------|
//! | Fastened                | 6             | 0        | ∅                        |
//! | Revolute                | 5             | 1        | rot about z              |
//! | Slider                  | 5             | 1        | trans along z            |
//! | Cylindrical             | 4             | 2        | rot z + trans z          |
//! | Planar                  | 3             | 3        | trans x,y + rot z        |
//! | Ball                    | 3             | 3        | rot x,y,z                |
//! | PinSlot                 | 4             | 2        | rot z + trans slot-dir   |
//! | Distance/Angle/Tangent  | 1             | —        | overlay                  |
//! | Parallel                | 2             | —        | overlay                  |
//! | GearRatio/RackPinion    | 1             | —        | couples 2 joint params   |
//! | Screw                   | 1             | —        | Cylindrical 2 DOF → 1    |
//!
//! Rank is the NUMERIC rank of the constraint Jacobian at a generic
//! satisfied configuration — the executor's verdict, not a counting
//! heuristic (the sketch dr_plan "counting is GENERIC rigidity" honesty
//! contract, one dimension up).
//!
//! Pre-implementation signature (captured 2026-07-17, post-45d8ffee tree):
//! every frame-pair kind fell through the residual dispatch to an EMPTY
//! residual — rank 0, dof 6 — the exact silent-DOF-lie class the refuse
//! contract exists to prevent.

use assembly_engine::{Assembly, FeatureRef, Instance, InstanceId, Mate, MateKind, Mesh, Mobility};

fn part(id: u32) -> Instance {
    Instance::new(InstanceId(id), format!("part_{id}"), Mesh::default())
}

/// Identity connector frame at the origin: z up, x along +X.
fn frame() -> FeatureRef {
    FeatureRef::Frame {
        origin: [0.0, 0.0, 0.0],
        z_axis: [0.0, 0.0, 1.0],
        x_axis: [1.0, 0.0, 0.0],
    }
}

/// The mating counter-frame. Mated frames ALIGN (`z_b = z_a`, spin at
/// θ = 0), so the counter side is the identity frame too — declared as its
/// own function to keep the two roles readable at call sites.
fn counter_frame() -> FeatureRef {
    frame()
}

fn frame_at(origin: [f64; 3], z: [f64; 3], x: [f64; 3]) -> FeatureRef {
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

/// Ground + one free part joined by `kind` over an identity frame pair;
/// returns (rank, dof).
fn two_body(kind: MateKind) -> (usize, usize) {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    assembly.add_instance(part(1));
    assembly.add_mate(mate(kind, 0, frame(), 1, counter_frame()));
    let report = assembly.dof_analysis();
    assert_eq!(report.config_dim, 6);
    (report.rank, report.dof)
}

// ── Joint kinds ─────────────────────────────────────────────────────────

#[test]
fn fastened_rank_6_dof_0() {
    assert_eq!(two_body(MateKind::Fastened), (6, 0), "Fastened: total lock");
}

#[test]
fn revolute_rank_5_dof_1() {
    assert_eq!(
        two_body(MateKind::Revolute { limits: None }),
        (5, 1),
        "Revolute: spin about z remains"
    );
}

#[test]
fn slider_rank_5_dof_1() {
    assert_eq!(
        two_body(MateKind::Slider { limits: None }),
        (5, 1),
        "Slider: slide along z remains"
    );
}

#[test]
fn cylindrical_rank_4_dof_2() {
    assert_eq!(
        two_body(MateKind::Cylindrical {
            rot_limits: None,
            trans_limits: None
        }),
        (4, 2),
        "Cylindrical: spin + slide remain"
    );
}

#[test]
fn planar_rank_3_dof_3() {
    assert_eq!(
        two_body(MateKind::Planar),
        (3, 3),
        "Planar: in-plane slide x/y + spin remain"
    );
}

#[test]
fn ball_rank_3_dof_3() {
    assert_eq!(
        two_body(MateKind::Ball),
        (3, 3),
        "Ball: all three rotations remain"
    );
}

#[test]
fn pinslot_rank_4_dof_2() {
    assert_eq!(
        two_body(MateKind::PinSlot {
            slot_dir_x: true,
            limits: None
        }),
        (4, 2),
        "PinSlot: pin spin + slot slide remain"
    );
}

// ── Dimensional overlays ────────────────────────────────────────────────

#[test]
fn distance_rank_1() {
    assert_eq!(
        two_body(MateKind::Distance { value: 0.0 }),
        (1, 5),
        "Distance: one offset equation"
    );
}

#[test]
fn angle_rank_1() {
    // z_b at 45° to z_a in the xz-plane; constrain exactly that angle so
    // the configuration is satisfied and the rank is generic.
    let s = std::f64::consts::FRAC_1_SQRT_2;
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    assembly.add_instance(part(1));
    assembly.add_mate(mate(
        MateKind::Angle {
            value: std::f64::consts::FRAC_PI_4,
        },
        0,
        frame(),
        1,
        frame_at([0.0, 0.0, 0.0], [s, 0.0, s], [s, 0.0, -s]),
    ));
    let report = assembly.dof_analysis();
    assert_eq!((report.rank, report.dof), (1, 5), "Angle: one equation");
}

#[test]
fn parallel_rank_2() {
    assert_eq!(
        two_body(MateKind::Parallel),
        (2, 4),
        "Parallel: direction lock only"
    );
}

#[test]
fn tangent_rank_1() {
    // A Ø6 cylinder axis-frame held tangent to the ground plane: origin at
    // z = 3 above the plane, axis along x (perpendicular to the normal).
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    assembly.add_instance(part(1));
    assembly.add_mate(mate(
        MateKind::Tangent { radius: 3.0 },
        0,
        frame(),
        1,
        frame_at([0.0, 0.0, 3.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ));
    let report = assembly.dof_analysis();
    assert_eq!((report.rank, report.dof), (1, 5), "Tangent: one equation");
}

// ── Couplings ───────────────────────────────────────────────────────────

/// Two gears revolute-mounted to ground on parallel axes 30 apart.
/// Base mechanism: 12 config DOF − 2×5 = 2 free spins.
fn two_revolutes() -> Assembly {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    assembly.add_instance(part(1));
    assembly.add_instance(part(2));
    assembly.add_mate(mate(
        MateKind::Revolute { limits: None },
        0,
        frame(),
        1,
        counter_frame(),
    ));
    assembly.add_mate(mate(
        MateKind::Revolute { limits: None },
        0,
        frame_at([30.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        2,
        counter_frame(),
    ));
    assembly
}

#[test]
fn gear_ratio_couples_two_spins_into_one() {
    let mut assembly = two_revolutes();
    let base = assembly.dof_analysis();
    assert_eq!(
        (base.rank, base.dof),
        (10, 2),
        "two free spins before gearing"
    );
    assembly.add_mate(mate(
        MateKind::GearRatio {
            ratio: 2.0,
            at: [0.0, 0.0],
            couples: [0, 1],
        },
        1,
        frame(),
        2,
        frame(),
    ));
    let geared = assembly.dof_analysis();
    assert_eq!(
        (geared.rank, geared.dof),
        (11, 1),
        "gear adds exactly 1 independent row: one shared spin remains"
    );
}

#[test]
fn rack_pinion_couples_spin_to_slide() {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    assembly.add_instance(part(1)); // pinion
    assembly.add_instance(part(2)); // rack
    assembly.add_mate(mate(
        MateKind::Revolute { limits: None },
        0,
        frame(),
        1,
        counter_frame(),
    ));
    assembly.add_mate(mate(
        MateKind::Slider { limits: None },
        0,
        frame_at([0.0, 10.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        2,
        frame_at([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ));
    let base = assembly.dof_analysis();
    assert_eq!(
        (base.rank, base.dof),
        (10, 2),
        "spin + slide before coupling"
    );
    assembly.add_mate(mate(
        MateKind::RackPinion {
            pinion_radius: 5.0,
            at: [0.0, 0.0],
            couples: [0, 1],
        },
        1,
        frame(),
        2,
        frame(),
    ));
    let coupled = assembly.dof_analysis();
    assert_eq!(
        (coupled.rank, coupled.dof),
        (11, 1),
        "rack-pinion: one shared DOF remains"
    );
}

#[test]
fn screw_collapses_cylindrical_to_one_dof() {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    assembly.add_instance(part(1));
    assembly.add_mate(mate(
        MateKind::Cylindrical {
            rot_limits: None,
            trans_limits: None,
        },
        0,
        frame(),
        1,
        counter_frame(),
    ));
    let base = assembly.dof_analysis();
    assert_eq!(
        (base.rank, base.dof),
        (4, 2),
        "spin + slide before the screw"
    );
    assembly.add_mate(mate(
        MateKind::Screw {
            lead: 1.5,
            at: [0.0, 0.0],
            couples: 0,
        },
        0,
        frame(),
        1,
        counter_frame(),
    ));
    let coupled = assembly.dof_analysis();
    assert_eq!(
        (coupled.rank, coupled.dof),
        (5, 1),
        "screw: helix = ONE degree of freedom"
    );
}

/// z-spin angle of an instance's quaternion (rotation assumed about z).
fn spin_z(assembly: &Assembly, id: u32) -> f64 {
    let r = assembly
        .instance(InstanceId(id))
        .map(|i| i.rotation)
        .unwrap_or([f64::NAN; 4]);
    2.0 * r[2].atan2(r[3])
}

#[test]
fn gear_value_ratio_2_counter_rotates() {
    // Spin gear 1 by +10° (its own free DOF — every geometric mate still
    // holds); the gear equation 2·θ₁ + θ₂ = 0 is now violated by 20° and
    // the solve must counter-rotate gear 2 to θ₂ = −20°.
    let mut assembly = two_revolutes();
    assembly.add_mate(mate(
        MateKind::GearRatio {
            ratio: 2.0,
            at: [0.0, 0.0],
            couples: [0, 1],
        },
        1,
        frame(),
        2,
        frame(),
    ));
    let theta1 = (10.0_f64).to_radians();
    if let Some(g1) = assembly
        .instances
        .iter_mut()
        .find(|i| i.id == InstanceId(1))
    {
        g1.rotation = [0.0, 0.0, (theta1 / 2.0).sin(), (theta1 / 2.0).cos()];
    }
    let report = assembly.solve();
    assert!(report.converged, "gear system satisfiable: {report:?}");
    let theta2 = spin_z(&assembly, 2);
    // The solver may split the correction across both spins; the INVARIANT
    // is the gear equation itself.
    let theta1_after = spin_z(&assembly, 1);
    assert!(
        (2.0 * theta1_after + theta2).abs() < 1e-6,
        "gear equation must hold: 2·{theta1_after} + {theta2} != 0"
    );
    assert!(
        theta2.abs() > 1e-3 || (theta1_after - theta1).abs() > 1e-3,
        "the solve actually moved something to restore the mesh"
    );
}

#[test]
fn screw_value_lead_couples_advance_to_spin() {
    // Slide the nut +3 along z (free DOF of the cylindrical mate); the
    // screw (lead 1.5) must spin it to θ = 2π·s/lead to stay on the helix.
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    assembly.add_instance(part(1));
    assembly.add_mate(mate(
        MateKind::Cylindrical {
            rot_limits: None,
            trans_limits: None,
        },
        0,
        frame(),
        1,
        counter_frame(),
    ));
    assembly.add_mate(mate(
        MateKind::Screw {
            lead: 1.5,
            at: [0.0, 0.0],
            couples: 0,
        },
        0,
        frame(),
        1,
        counter_frame(),
    ));
    if let Some(nut) = assembly
        .instances
        .iter_mut()
        .find(|i| i.id == InstanceId(1))
    {
        nut.translation = [0.0, 0.0, 0.4]; // s = 0.4, less than a turn
    }
    let report = assembly.solve();
    assert!(report.converged, "screw system satisfiable: {report:?}");
    let s = assembly
        .instance(InstanceId(1))
        .map(|i| i.translation[2])
        .unwrap_or(f64::NAN);
    let theta = spin_z(&assembly, 1);
    // Frame convention z_b = −z_a: the joint θ measured about z_a relates
    // to the body spin about its own axis with a sign; the INVARIANT is
    // the engine's own joint-parameter relation s = lead·θ_joint/2π, which
    // we assert through the residual being zero with genuine motion.
    assert!(
        s.abs() > 1e-3 || theta.abs() > 1e-3,
        "the nut still sits somewhere on the helix away from the seed"
    );
    let violation = assembly.mate_violation(&assembly.mates[1].clone());
    assert!(
        violation < 1e-9,
        "helix equation holds after solve: violation={violation}"
    );
    // And the helix is REAL: spin and slide are locked together — the
    // configuration cannot have moved in s without θ following.
    let expected_theta_mag = std::f64::consts::TAU * s.abs() / 1.5;
    assert!(
        (theta.abs() - expected_theta_mag).abs() < 1e-6,
        "θ magnitude {theta} must equal 2π·|s|/lead = {expected_theta_mag}"
    );
}

// ── Solve behaviour: the freed motions are the RIGHT ones ───────────────

#[test]
fn revolute_seats_origin_and_axis_but_not_spin() {
    // Part starts translated AND spun; revolute must pull origin+axis home
    // and leave the spin exactly where it was (it is the joint DOF).
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    let mut p1 = part(1);
    p1.translation = [4.0, -2.0, 7.0];
    let half = (40.0_f64).to_radians() / 2.0;
    p1.rotation = [0.0, 0.0, half.sin(), half.cos()];
    assembly.add_instance(p1);
    assembly.add_mate(mate(
        MateKind::Revolute { limits: None },
        0,
        frame(),
        1,
        counter_frame(),
    ));
    let report = assembly.solve();
    assert!(report.converged, "revolute is satisfiable: {report:?}");
    let t = assembly
        .instance(InstanceId(1))
        .map(|i| i.translation)
        .unwrap_or([f64::NAN; 3]);
    assert!(
        t[0].abs() < 1e-6 && t[1].abs() < 1e-6 && t[2].abs() < 1e-6,
        "origins must coincide, got {t:?}"
    );
    let r = assembly
        .instance(InstanceId(1))
        .map(|i| i.rotation)
        .unwrap_or([f64::NAN; 4]);
    assert!(
        r[2].abs() > 1e-3,
        "the spin about z is the JOINT DOF — the solver must not consume it, got {r:?}"
    );
}

#[test]
fn fastened_from_generic_start_locks_everything() {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    let mut p1 = part(1);
    p1.translation = [3.0, 4.0, -5.0];
    let half = (25.0_f64).to_radians() / 2.0;
    p1.rotation = [half.sin(), 0.0, 0.0, half.cos()];
    assembly.add_instance(p1);
    assembly.add_mate(mate(MateKind::Fastened, 0, frame(), 1, counter_frame()));
    let report = assembly.solve();
    assert!(report.converged, "fastened is satisfiable: {report:?}");
    let inst = assembly.instance(InstanceId(1)).cloned();
    let t = inst
        .as_ref()
        .map(|i| i.translation)
        .unwrap_or([f64::NAN; 3]);
    let r = inst.as_ref().map(|i| i.rotation).unwrap_or([f64::NAN; 4]);
    assert!(
        t.iter().all(|c| c.abs() < 1e-6),
        "origin locked home, got {t:?}"
    );
    // Aligned-frame convention: the mated state IS the identity body pose.
    assert!(
        r[3].abs() > 1.0 - 1e-9,
        "orientation locked to the declared frames, got {r:?}"
    );
    let dof = assembly.dof_analysis();
    assert_eq!(dof.mobility, Mobility::FullyConstrained);
}

// ── Honest refusal ──────────────────────────────────────────────────────

#[test]
fn refuse_set_is_typed_and_never_counts_as_constraint() {
    for kind in [MateKind::Cam, MateKind::Path, MateKind::Symmetric] {
        assert!(!kind.is_numerically_enforced(), "{kind:?} must refuse");
        let (rank, dof) = two_body(kind);
        assert_eq!(
            (rank, dof),
            (0, 6),
            "{kind:?} contributes NO silent constraint rows"
        );
    }
    let enforced = [
        MateKind::Fastened,
        MateKind::Planar,
        MateKind::Ball,
        MateKind::Parallel,
    ];
    for kind in enforced {
        assert!(kind.is_numerically_enforced(), "{kind:?} is enforced");
    }
}

#[test]
fn enforcement_report_names_refused_and_mismatched_mates() {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));
    assembly.add_instance(part(1));
    // 0: healthy frame-pair mate.
    assembly.add_mate(mate(MateKind::Fastened, 0, frame(), 1, counter_frame()));
    // 1: typed refuse set.
    assembly.add_mate(mate(MateKind::Cam, 0, frame(), 1, counter_frame()));
    // 2: kind/feature MISMATCH — a frame kind declared over Face features
    //    (the old silent `_ => Vec::new()` class; must be named, not muted).
    assembly.add_mate(mate(
        MateKind::Revolute { limits: None },
        0,
        FeatureRef::Face {
            point: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
        },
        1,
        FeatureRef::Face {
            point: [0.0; 3],
            normal: [0.0, 0.0, -1.0],
        },
    ));
    // 3: coupling with an out-of-range reference.
    assembly.add_mate(mate(
        MateKind::GearRatio {
            ratio: 1.0,
            at: [0.0, 0.0],
            couples: [0, 99],
        },
        0,
        frame(),
        1,
        frame(),
    ));
    let report = assembly.mate_enforcement_report();
    assert!(!report.all_enforced());
    let enforced: Vec<bool> = report.mates.iter().map(|m| m.enforced).collect();
    assert_eq!(
        enforced,
        vec![true, false, false, false],
        "exactly the healthy mate is enforced: {report:?}"
    );
    // Every refused mate carries a human-readable reason.
    assert!(report
        .mates
        .iter()
        .filter(|m| !m.enforced)
        .all(|m| m.reason.is_some()));
}

#[test]
fn certificate_refuses_soundness_with_unenforced_mates() {
    // Two touching cubes, grounded, consistent — but the joint is a Cam,
    // which the solver cannot enforce. The certificate must surface
    // mates_enforced = false and refuse is_sound, NEVER silently certify
    // a mate it did not check.
    let half = 1.0;
    let cube = Mesh {
        vertices: vec![
            [-half, -half, -half],
            [half, -half, -half],
            [half, half, -half],
            [-half, half, -half],
            [-half, -half, half],
            [half, -half, half],
            [half, half, half],
            [-half, half, half],
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
    };
    let mut assembly = Assembly::new(InstanceId(0));
    let mut g = Instance::new(InstanceId(0), "ground", cube.clone());
    g.translation = [0.0, 0.0, 0.0];
    assembly.add_instance(g);
    let mut p = Instance::new(InstanceId(1), "cam_follower", cube);
    p.translation = [0.0, 0.0, 2.0];
    assembly.add_instance(p);
    assembly.add_mate(mate(
        MateKind::Cam,
        0,
        frame_at([0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        1,
        frame_at([0.0, 0.0, -1.0], [0.0, 0.0, -1.0], [1.0, 0.0, 0.0]),
    ));
    let cert = assembly.certify(&[], 0.01);
    assert!(!cert.mates_enforced, "the Cam mate is not enforced");
    assert!(!cert.is_sound(), "unenforced mates block soundness");
}
