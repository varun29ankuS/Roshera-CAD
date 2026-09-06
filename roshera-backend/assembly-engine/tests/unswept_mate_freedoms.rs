//! A mate freedom the derived sweep never drives is NAMED, never skipped
//! (audit 2026-09-03, task 41 — the second half of task 18's "an unrun
//! check is not a pass").
//!
//! Task 18 closed the refusal the sweep DID reach: an unbounded slider now
//! lands in `unverified_sweeps` instead of riding a silent pass. This file
//! pins the hole underneath it. `derived_sweeps` looped over the two
//! frame-pair joint parameters (θ, s) and `continue`d whenever the mate
//! exposed neither — so a `Ball`, a `Planar` and a `PinSlot` mate produced
//! NO swept fact at all, and `swept_clearance_ok` read `true` about three
//! freedoms no check had ever run on. Same shape as task 18, bigger blast
//! radius: the refusal was not even reached to be recorded.
//!
//! The contract these tests pin:
//!
//! * a mate kind that GRANTS a relative freedom the (θ, s) drive cannot
//!   express (`Planar`, `Ball`, `PinSlot`) produces an `UnverifiedSweep`
//!   naming the mate and the kind — and blocks soundness;
//! * a RIGID mate (`Fastened`, legacy `Fixed`) grants no freedom, so it
//!   produces no entry at all: closing this hole must not make every
//!   bolted assembly unsound;
//! * a drive the sweep asks for and the drag surface REFUSES lands in
//!   `unverified_sweeps` too, so the certificate never depends on the
//!   coincidence that a refusing mate happens to fail another dimension;
//! * a dimensional overlay (`Distance`, `Angle`, `Parallel`, `Tangent`)
//!   is CONDITIONAL. It grants no motion, so an overlay riding a joint
//!   reports nothing — but nothing requires a joint to be there. An
//!   overlay is enforced over Frame/Frame features on its own, so a lone
//!   `Distance` is enforced, consumes rank 1, leaves 5 DOF and is swept
//!   by nothing. Held alone it is named; riding an enforced joint it is
//!   not; riding an UNENFORCED one it is named again, because a mate that
//!   contributes no residual rows holds nothing.
//!
//! Pre-fix signatures, all captured on this fixture at HEAD ac476969
//! (2026-09-07) with the pre-fix kernel:
//!   ball_mate_freedom_is_named:      is_sound=true swept_clearance_ok=true sweeps=0 unverified=0
//!   planar_mate_freedom_is_named:    is_sound=true swept_clearance_ok=true sweeps=0 unverified=0
//!   pin_slot_mate_freedom_is_named:  is_sound=true swept_clearance_ok=true sweeps=0 unverified=0
//!   a_refused_drive_is_named:        unverified=0, and swept_clearance_ok=true DESPITE
//!                                    mates_enforced=false — the coincidence never covered
//!                                    the swept dimension itself
//!   a_lone_distance_overlay_is_named: is_sound=true swept_clearance_ok=true unverified=0 dof=5
//!   a_lone_angle_overlay_is_named:    is_sound=true swept_clearance_ok=true unverified=0 dof=6
//!   a_lone_parallel_overlay_is_named: is_sound=true swept_clearance_ok=true unverified=0 dof=4
//!   a_lone_tangent_overlay_is_named:  swept_clearance_ok=true unverified=0 (is_sound was
//!                                    already false on this rig for an unrelated dimension —
//!                                    the swept lie was there all the same)
//!   an_overlay_riding_a_joint_adds_no_entry:  sound BEFORE and AFTER (the control)

#[allow(dead_code)]
mod common;

use assembly_engine::{
    Assembly, AssemblyCertificate, DriveParam, DriveRefusal, EpsilonSpec, FeatureRef, Instance,
    InstanceId, Mate, MateKind, Mesh, SweepRefusal, SweepSource,
};
use common::{frame, mate};

/// An axis-aligned cuboid with the given half-extents, centred at the
/// local origin (the `sweep_toi` fixture's body).
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

/// A ground pad (top face at z = 1) with a block SEATED on it (bottom face
/// at z = 1) — the parts touch, nothing overlaps, and the block is
/// grounded through whatever mate the test declares.
///
/// Every certificate dimension other than the swept one is deliberately
/// clean, so the verdict can only turn on the dimension under test.
fn pad_and_block() -> Assembly {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(instance_at(0, cuboid(10.0, 1.0, 1.0), [0.0, 0.0, 0.0]));
    assembly.add_instance(instance_at(1, cuboid(1.0, 1.0, 1.0), [0.0, 0.0, 2.0]));
    assembly
}

/// The rig joined by a FRAME-pair mate: the connector frames are
/// world-coincident and aligned at z = 1, so the mate is satisfied exactly
/// where the parts already sit (the solve moves nothing).
fn framed(kind: MateKind) -> Assembly {
    let mut assembly = pad_and_block();
    assembly.add_mate(mate(
        kind,
        0,
        frame([0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        1,
        frame([0.0, 0.0, -1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ));
    assembly
}

/// The same rig joined by a legacy FACE-pair mate (flush = antiparallel
/// normals).
fn faced(kind: MateKind) -> Assembly {
    let mut assembly = pad_and_block();
    assembly.add_mate(Mate {
        kind,
        a: InstanceId(0),
        feature_a: FeatureRef::Face {
            point: [0.0, 0.0, 1.0],
            normal: [0.0, 0.0, 1.0],
        },
        b: InstanceId(1),
        feature_b: FeatureRef::Face {
            point: [0.0, 0.0, -1.0],
            normal: [0.0, 0.0, -1.0],
        },
    });
    assembly
}

fn certify(assembly: &Assembly) -> AssemblyCertificate {
    assembly.certify_v2(
        &[],
        EpsilonSpec {
            kernel_floor: 0.01,
            requested: None,
        },
    )
}

/// Every dimension EXCEPT the swept one is clean on these rigs — asserting
/// them names which dimension moved if a fixture ever drifts.
fn assert_only_the_swept_dimension_is_at_stake(cert: &AssemblyCertificate) {
    assert!(cert.mates_consistent, "{cert:?}");
    assert!(cert.fully_grounded, "{cert:?}");
    assert!(cert.no_static_interference, "{cert:?}");
    assert!(cert.mates_in_contact, "{cert:?}");
    assert!(cert.mates_anchored, "{cert:?}");
    assert!(cert.mates_enforced, "{cert:?}");
}

/// The shared body of the three undriveable-freedom cases: the freedom is
/// named, it carries the kind, and it blocks the verdict.
fn assert_freedom_is_named(kind: MateKind) {
    let assembly = framed(kind);
    let cert = certify(&assembly);
    assert_only_the_swept_dimension_is_at_stake(&cert);

    let named = cert.unverified_sweeps.iter().any(|u| {
        u.source == SweepSource::MateFreedom { mate_index: 0 }
            && u.refusal
                == SweepRefusal::NotDriveable {
                    mate_index: 0,
                    kind,
                }
    });
    assert!(
        named,
        "the certificate must NAME the {kind:?} freedom it never swept: {:?}",
        cert.unverified_sweeps
    );
    // The fact stays in the motion table too — `unverified_sweeps` is a
    // projection of `sweeps`, not a move (the task-18 contract).
    let listed = cert
        .sweeps
        .iter()
        .any(|s| s.source == SweepSource::MateFreedom { mate_index: 0 } && !s.clear);
    assert!(
        listed,
        "an unswept freedom is not a clear one: {:?}",
        cert.sweeps
    );
    assert!(
        !cert.swept_clearance_ok,
        "a freedom no check ran on cannot certify swept clearance: {cert:?}"
    );
    assert!(!cert.is_sound(), "{cert:?}");
}

// ── The freedoms the (θ, s) drive cannot express ────────────────────────

#[test]
fn ball_mate_freedom_is_named() {
    // A ball mate's freedom is 3 rotations — no single frame parameter
    // spans them, so the sweep cannot drive it. Silence was the defect.
    assert_freedom_is_named(MateKind::Ball);
}

#[test]
fn planar_mate_freedom_is_named() {
    // 2 in-plane translations + spin: driving θ alone would leave the
    // assembly under-determined, so the drive refuses — and the refusal
    // must be RECORDED, not swallowed.
    assert_freedom_is_named(MateKind::Planar);
}

#[test]
fn pin_slot_mate_freedom_is_named() {
    // The pin spins and the block travels along the SLOT direction on
    // frame A, which is deliberately not the frame z axis the (θ, s)
    // parameters read. Declared limits do not make it driveable.
    assert_freedom_is_named(MateKind::PinSlot {
        slot_dir_x: true,
        limits: Some((-1.0, 1.0)),
    });
}

// ── The overlays: CONDITIONAL, on whether a joint holds the pair ────────

/// A dimensional overlay declared ALONE on a pair — no joint holds it, so
/// the pair's remaining freedom is the whole picture and nothing sweeps it.
fn assert_lone_overlay_is_named(kind: MateKind) {
    let cert = certify(&framed(kind));
    let named = cert.unverified_sweeps.iter().any(|u| {
        u.source == SweepSource::MateFreedom { mate_index: 0 }
            && u.refusal
                == SweepRefusal::OverlayWithoutJoint {
                    mate_index: 0,
                    kind,
                }
    });
    assert!(
        named,
        "a lone {kind:?} overlay holds a pair no joint holds — the freedom \
         it leaves must be NAMED: {:?}",
        cert.unverified_sweeps
    );
    assert!(
        !cert.swept_clearance_ok,
        "and an unswept pair cannot certify swept clearance: {cert:?}"
    );
    assert!(!cert.is_sound(), "{cert:?}");
}

#[test]
fn a_lone_distance_overlay_is_named() {
    // The reviewer's case, measured: `features_match_kind` puts the
    // overlays in its `_` arm, so a `Distance` over Frame/Frame is
    // ENFORCED on its own. It consumes rank 1 and leaves 5 DOF that
    // nothing sweeps — and it is agent-reachable (`assembly_mates.rs`
    // maps `DocMateKind::Distance` straight through).
    assert_lone_overlay_is_named(MateKind::Distance { value: 0.0 });
}

#[test]
fn a_lone_angle_overlay_is_named() {
    assert_lone_overlay_is_named(MateKind::Angle { value: 0.0 });
}

#[test]
fn a_lone_parallel_overlay_is_named() {
    assert_lone_overlay_is_named(MateKind::Parallel);
}

#[test]
fn a_lone_tangent_overlay_is_named() {
    assert_lone_overlay_is_named(MateKind::Tangent { radius: 1.0 });
}

#[test]
fn an_overlay_riding_a_joint_adds_no_entry() {
    // The CONTROL that keeps the condition conditional. The same
    // `Distance` overlay on a pair a `Revolute` holds: the joint's freedom
    // is swept, so the overlay has nothing left to report and the
    // assembly stays sound. Without this, every dimensioned joint in the
    // product would certify unsound.
    let mut assembly = framed(MateKind::Revolute {
        limits: Some((-0.5, 0.5)),
    });
    assembly.add_mate(mate(
        MateKind::Distance { value: 0.0 },
        0,
        frame([0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        1,
        frame([0.0, 0.0, -1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ));
    let cert = certify(&assembly);
    assert_only_the_swept_dimension_is_at_stake(&cert);
    assert!(
        cert.unverified_sweeps.is_empty(),
        "an overlay riding a swept joint reports nothing: {:?}",
        cert.unverified_sweeps
    );
    assert!(cert.swept_clearance_ok, "{cert:?}");
    assert!(cert.is_sound(), "{cert:?}");
}

#[test]
fn an_overlay_riding_an_unenforced_joint_is_still_named() {
    // The condition requires the joint to be ENFORCED. A `Revolute`
    // declared over FACE features contributes no residual rows, so it
    // holds nothing — the overlay's pair is still unswept, and treating
    // an unenforced neighbour as cover would be exactly the "a predicate
    // happens to pass" reasoning this task exists to remove.
    let mut assembly = faced(MateKind::Revolute {
        limits: Some((-0.5, 0.5)),
    });
    assembly.add_mate(mate(
        MateKind::Distance { value: 0.0 },
        0,
        frame([0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        1,
        frame([0.0, 0.0, -1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ));
    let cert = certify(&assembly);
    assert!(!cert.mates_enforced, "the fixture's premise: {cert:?}");
    let named = cert.unverified_sweeps.iter().any(|u| {
        u.refusal
            == SweepRefusal::OverlayWithoutJoint {
                mate_index: 1,
                kind: MateKind::Distance { value: 0.0 },
            }
    });
    assert!(
        named,
        "an unenforced joint holds nothing, so the overlay still refuses: {:?}",
        cert.unverified_sweeps
    );
}

// ── The control: a rigid mate has NO freedom ────────────────────────────

#[test]
fn a_fastened_mate_grants_no_freedom_and_stays_sound() {
    // The load-bearing control. `Fastened` is 0 DOF: there is nothing to
    // sweep, so there is nothing to refuse. If closing the hole above
    // emitted an entry here, every bolted assembly in the product would
    // certify unsound.
    let cert = certify(&framed(MateKind::Fastened));
    assert_only_the_swept_dimension_is_at_stake(&cert);
    assert!(
        cert.unverified_sweeps.is_empty(),
        "a rigid mate has no freedom to leave unswept: {:?}",
        cert.unverified_sweeps
    );
    assert!(
        cert.sweeps.is_empty(),
        "and no motion to report at all: {:?}",
        cert.sweeps
    );
    assert!(cert.swept_clearance_ok, "{cert:?}");
    assert!(cert.is_sound(), "{cert:?}");
}

#[test]
fn a_legacy_fixed_mate_grants_no_freedom_and_stays_sound() {
    // The same control over the legacy Face-pair `Fixed` kind (rank 6
    // since the Slice-1 fix — `tests/fixed_mate_rigid.rs`).
    let cert = certify(&faced(MateKind::Fixed));
    assert_only_the_swept_dimension_is_at_stake(&cert);
    assert!(
        cert.unverified_sweeps.is_empty(),
        "a bolt pattern has no freedom to leave unswept: {:?}",
        cert.unverified_sweeps
    );
    assert!(cert.swept_clearance_ok, "{cert:?}");
    assert!(cert.is_sound(), "{cert:?}");
}

// ── The refusals survive the WIRE ───────────────────────────────────────

#[test]
fn the_named_freedom_survives_a_round_trip() {
    // `NotDriveable` carries a `MateKind` and `DriveRefused` carries a
    // whole `DriveRefusal` — both are enums nested inside an internally
    // tagged one, the shape serde is fussiest about. A refusal the agent
    // cannot read is a refusal that did not happen, so the wire is pinned
    // here rather than trusted.
    //
    // Every nesting is exercised, because they are different serde shapes
    // and only the first is trivial:
    //   * `Ball`     — a UNIT variant of `MateKind`, a bare string;
    //   * `PinSlot`  — a STRUCT variant carrying an `Option<(f64, f64)>`;
    //   * `Distance` — the `OverlayWithoutJoint` path, a struct variant
    //     carrying a bare `f64`;
    //   * `Revolute` over Face features — the `DriveRefused` path, whose
    //     `DriveRefusal` is ITSELF internally tagged, on a key of the same
    //     name (`refusal`) as the `SweepRefusal` that contains it.
    //
    // `SweepMethod::NotRun` rides every one of them: each of these certs
    // carries at least one refused fact, and a method serde could not read
    // back would fail the equality below.
    let rigs = [
        framed(MateKind::Ball),
        framed(MateKind::PinSlot {
            slot_dir_x: false,
            limits: Some((-1.0, 1.0)),
        }),
        framed(MateKind::Distance { value: 2.5 }),
        faced(MateKind::Revolute {
            limits: Some((-0.5, 0.5)),
        }),
    ];
    for assembly in &rigs {
        let cert = certify(assembly);
        assert!(
            !cert.unverified_sweeps.is_empty(),
            "the rig's premise: it HAS a refusal to carry: {cert:?}"
        );
        let json = serde_json::to_string(&cert);
        let Ok(json) = json else {
            assert!(false, "the certificate serialises: {json:?}");
            return;
        };
        let parsed: Result<AssemblyCertificate, _> = serde_json::from_str(&json);
        let Ok(parsed) = parsed else {
            assert!(false, "and parses back: {parsed:?}");
            return;
        };
        assert_eq!(
            parsed.unverified_sweeps, cert.unverified_sweeps,
            "the named refusal must cross the wire intact"
        );
        // The refused FACTS travel too — that is where the method rides,
        // and `NotRun` is the value a stale reader is likeliest to choke on.
        assert_eq!(
            parsed.sweeps, cert.sweeps,
            "and so does the motion table that carries them"
        );
        assert!(
            !parsed.is_sound(),
            "and still block the verdict: {parsed:?}"
        );
    }
}

// ── A refused DRIVE is recorded, not dropped ────────────────────────────

#[test]
fn a_refused_drive_is_named() {
    // `derived_sweeps` used to drop `sweep_driven`'s `Err` on the floor,
    // defended by a COINCIDENCE: every mate whose drive refuses also fails
    // `mates_enforced`, so the certificate was unsound anyway. A verdict
    // must not rest on two predicates happening to agree — the refusal is
    // now recorded on its own dimension.
    //
    // The cheapest honest trigger: a Revolute declared over FACE features.
    // The kind owns θ (so the sweep asks for the drive) but the mate is
    // not numerically enforced (feature kinds do not match), so
    // `prepare_drive` refuses with `NotEnforced`.
    let assembly = faced(MateKind::Revolute {
        limits: Some((-0.5, 0.5)),
    });
    let cert = certify(&assembly);
    assert!(
        !cert.mates_enforced,
        "the fixture's premise: this mate is NOT enforced: {cert:?}"
    );

    let named = cert.unverified_sweeps.iter().any(|u| {
        u.source
            == SweepSource::DrivenMate {
                mate_index: 0,
                param: DriveParam::Rotation,
            }
            && matches!(
                &u.refusal,
                SweepRefusal::DriveRefused {
                    mate_index: 0,
                    param: DriveParam::Rotation,
                    drive_refusal: DriveRefusal::NotEnforced { mate_index: 0, .. },
                }
            )
    });
    assert!(
        named,
        "a drive the sweep asked for and the drag refused must be RECORDED: {:?}",
        cert.unverified_sweeps
    );
    assert!(
        !cert.swept_clearance_ok,
        "a motion that was never driven was never swept: {cert:?}"
    );
    assert!(!cert.is_sound(), "{cert:?}");
}
