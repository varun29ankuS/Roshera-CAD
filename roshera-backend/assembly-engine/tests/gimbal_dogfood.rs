//! Slice-5 DOGFOOD — the two-ring gimbal through ±6° (spec §3.8 Slice 5:
//! "gimbal ±6° dogfood (the original rocket-engine trigger) certifies
//! end-to-end").
//!
//! This is not a unit fixture. It is a real 2-DOF mechanism built from
//! real annulus geometry, and it asks the question the whole campaign
//! exists to answer: **can an agent trust a certificate about a thing that
//! MOVES?**
//!
//! The gimbal:
//!
//! ```text
//!   ground  = a bearing block under the outer ring's −X rim
//!   outer   = annulus r 4.2–5.0, revolute to ground about world X, ±6°
//!   inner   = annulus r 3.2–4.0, revolute to outer about world Y,  ±6°
//!   stop    = an L-bracket fastened to the outer ring, reaching under the
//!             inner ring — the hard stop the ±6° limits exist to respect
//! ```
//!
//! Everything touches something (block under the rim at 0.2, ring inside
//! ring at 0.2, bracket on the ring at 0.05) — so this exercises the
//! anchoring and contact dimensions for real, not just the motion ones.
//! The KINEMATICS live in the mates, not in the shapes: the block does not
//! have to wrap the ring for the revolute to be about world X, and keeping
//! every ground shape convex keeps the fixture about Slice 5 rather than
//! about VHACD's behaviour on a dumbbell.
//!
//! # What is pinned
//!
//! 1. The gimbal certifies SOUND at ±6° — every dimension, including the
//!    swept one, over joints DERIVED from its own mates.
//! 2. Both revolutes are derived as joints and swept over their limit
//!    bands by continuous TOI — nothing is authored.
//! 3. Dragging either ring through its full band stays ON the manifold at
//!    every step, with no rank transitions (a gimbal at ±6° is generic).
//! 4. **The limits are LOAD-BEARING.** Widen them to ±30° and the inner
//!    ring reaches the stop bracket: the swept gate catches it and stamps
//!    the angle. This is what makes claim 1 mean something — without it,
//!    "certifies clear" could be true because the sweep never looked.

mod common;

use assembly_engine::{
    Assembly, DriveParam, EpsilonSpec, Instance, InstanceId, MateKind, Mesh, SweepMethod,
    SweepSource,
};
use common::{frame, mate};

/// ε for the fixture: a deviation bound comfortably below every designed
/// gap (the smallest is the 0.05 bracket seat), so a failure is a real
/// collision and never the margin.
const EPS: EpsilonSpec = EpsilonSpec {
    kernel_floor: 0.01,
    requested: None,
};

const SIX_DEG: f64 = 6.0 * std::f64::consts::PI / 180.0;
const THIRTY_DEG: f64 = 30.0 * std::f64::consts::PI / 180.0;

/// An annulus (a ring) in the local XY plane, centred at the local origin:
/// `segments` quads around the inner and outer walls plus the top and
/// bottom faces. A closed, watertight triangle soup — the kernel
/// tessellation an instance would really carry.
fn ring(inner_r: f64, outer_r: f64, half_thickness: f64, segments: usize) -> Mesh {
    let mut vertices = Vec::new();
    let mut triangles = Vec::new();
    for s in 0..segments {
        let a = std::f64::consts::TAU * (s as f64) / (segments as f64);
        let (c, sn) = (a.cos(), a.sin());
        // 4 vertices per station: inner-bottom, outer-bottom, outer-top, inner-top.
        vertices.push([inner_r * c, inner_r * sn, -half_thickness]);
        vertices.push([outer_r * c, outer_r * sn, -half_thickness]);
        vertices.push([outer_r * c, outer_r * sn, half_thickness]);
        vertices.push([inner_r * c, inner_r * sn, half_thickness]);
    }
    let n = u32::try_from(segments).unwrap_or(1);
    for s in 0..n {
        let b = 4 * s;
        let nb = 4 * ((s + 1) % n);
        // bottom, outer wall, top, inner wall — each a quad of two triangles.
        for (p, q, r, t) in [
            (b, nb, nb + 1, b + 1),         // bottom face
            (b + 1, nb + 1, nb + 2, b + 2), // outer wall
            (b + 2, nb + 2, nb + 3, b + 3), // top face
            (b + 3, nb + 3, nb, b),         // inner wall
        ] {
            triangles.push([p, q, r]);
            triangles.push([p, r, t]);
        }
    }
    Mesh {
        vertices,
        triangles,
    }
}

/// An axis-aligned box given by its local min/max corners, appended into
/// `mesh` — so one INSTANCE can carry a multi-box shape (the fork, the
/// L-bracket) exactly as a real kernel tessellation would.
fn push_box(mesh: &mut Mesh, min: [f64; 3], max: [f64; 3]) {
    let base = u32::try_from(mesh.vertices.len()).unwrap_or(0);
    for &(i, j, k) in &[
        (0, 0, 0),
        (1, 0, 0),
        (1, 1, 0),
        (0, 1, 0),
        (0, 0, 1),
        (1, 0, 1),
        (1, 1, 1),
        (0, 1, 1),
    ] {
        mesh.vertices.push([
            if i == 0 { min[0] } else { max[0] },
            if j == 0 { min[1] } else { max[1] },
            if k == 0 { min[2] } else { max[2] },
        ]);
    }
    for tri in [
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
    ] {
        triangles_push(mesh, base, tri);
    }
}

fn triangles_push(mesh: &mut Mesh, base: u32, tri: [u32; 3]) {
    mesh.triangles
        .push([base + tri[0], base + tri[1], base + tri[2]]);
}

fn instance_at(id: u32, name: &str, mesh: Mesh, pos: [f64; 3]) -> Instance {
    let mut instance = Instance::new(InstanceId(id), name, mesh);
    instance.translation = pos;
    instance
}

/// The gimbal, with the two revolutes limited to `band` (radians, ±).
fn gimbal(band: f64) -> Assembly {
    let mut assembly = Assembly::new(InstanceId(0));

    // Ground: a single CONVEX bearing block seated 0.2 under the outer
    // ring's −X rim (the +X side is where the stop bracket hangs).
    let mut block = Mesh::default();
    push_box(&mut block, [-5.0, -0.4, -0.8], [-4.2, 0.4, -0.5]);
    assembly.add_instance(instance_at(0, "bearing_block", block, [0.0, 0.0, 0.0]));

    // Outer ring: revolute to ground about world X.
    assembly.add_instance(instance_at(
        1,
        "outer_ring",
        ring(4.2, 5.0, 0.3, 16),
        [0.0; 3],
    ));
    // Inner ring: revolute to the outer ring about world Y (0.2 radial gap).
    assembly.add_instance(instance_at(
        2,
        "inner_ring",
        ring(3.2, 4.0, 0.3, 16),
        [0.0; 3],
    ));

    // The stop: an L-bracket hanging off the outer ring's +X rim and
    // reaching inward UNDER the inner ring. Local origin at its seat.
    let mut bracket = Mesh::default();
    // Vertical leg: seats 0.05 under the outer ring's underside (world
    // z −1.3 … −0.35). The two boxes meet on the z = −0.325 plane and do
    // not overlap — a self-intersecting soup would give the convexity gate
    // a doubled volume and VHACD a degenerate input.
    push_box(&mut bracket, [-0.3, -0.4, -0.325], [0.3, 0.4, 0.625]);
    // Horizontal foot: the stop, reaching in to radius 3.4 (world z −1.6 … −1.3).
    push_box(&mut bracket, [-1.2, -0.4, -0.625], [0.3, 0.4, -0.325]);
    assembly.add_instance(instance_at(3, "stop_bracket", bracket, [4.6, 0.0, -0.975]));

    // Mate 0 — outer ring on the fork: revolute about world X.
    assembly.add_mate(mate(
        MateKind::Revolute {
            limits: Some((-band, band)),
        },
        0,
        frame([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        1,
        frame([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ));
    // Mate 1 — inner ring in the outer ring: revolute about world Y.
    assembly.add_mate(mate(
        MateKind::Revolute {
            limits: Some((-band, band)),
        },
        1,
        frame([0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
        2,
        frame([0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
    ));
    // Mate 2 — the stop bracket, fastened to the outer ring.
    assembly.add_mate(mate(
        MateKind::Fastened,
        1,
        frame([4.6, 0.0, -0.975], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        3,
        frame([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ));
    assembly
}

fn driven_fact(
    cert: &assembly_engine::AssemblyCertificate,
    mate_index: u32,
) -> Option<&assembly_engine::SweptFact> {
    cert.sweeps.iter().find(
        |s| matches!(s.source, SweepSource::DrivenMate { mate_index: m, param: DriveParam::Rotation } if m == mate_index),
    )
}

#[test]
fn gimbal_certifies_sound_through_its_six_degree_band() {
    let assembly = gimbal(SIX_DEG);
    let cert = assembly.certify_v2(&[], EPS);

    assert!(
        cert.is_sound(),
        "the ±6° gimbal must certify SOUND end-to-end: {cert:?}"
    );
    // Named, so a future regression says WHICH dimension broke.
    assert!(cert.mates_consistent, "the mates solve");
    assert!(cert.fully_grounded, "every ring reaches the block");
    assert!(cert.mates_anchored, "no fabricated joint");
    assert!(cert.mates_in_contact, "no paper joint — rings really touch");
    assert!(cert.no_static_interference, "nothing overlaps at rest");
    assert!(cert.swept_clearance_ok, "and nothing collides through ±6°");
    assert!(cert.mates_enforced, "every mate is numerically enforced");
    // A gimbal is a MECHANISM: 2 DOF is the design, not a defect.
    assert_eq!(cert.dof, 2, "two rings, two axes: {cert:?}");
}

#[test]
fn both_revolutes_are_derived_as_joints_and_swept_by_toi() {
    // Nothing is authored: `certify` is handed NO mechanisms, and the
    // gimbal's own mates supply both joints and both ranges.
    let assembly = gimbal(SIX_DEG);
    let cert = assembly.certify_v2(&[], EPS);

    for mate_index in [0, 1] {
        let Some(fact) = driven_fact(&cert, mate_index) else {
            assert!(
                false,
                "mate {mate_index}'s joint must be DERIVED, not authored: {:?}",
                cert.sweeps
            );
            return;
        };
        assert!(
            matches!(fact.method, SweepMethod::NonlinearToi { .. }),
            "the swept gate is continuous, not sampled: {fact:?}"
        );
        assert!(
            (fact.range.0 - (-SIX_DEG)).abs() < 1e-12 && (fact.range.1 - SIX_DEG).abs() < 1e-12,
            "the derived range IS the declared limit band: {fact:?}"
        );
        assert!(fact.clear, "clear through the band: {fact:?}");
        assert!(fact.refusal.is_none(), "a bounded band is certifiable");
        assert!(fact.manifold_violation.is_none(), "on-manifold throughout");
        assert!(fact.first_contact.is_none() && fact.interference.is_empty());
        assert_eq!(fact.epsilon, 0.01, "ε recorded on every fact");
    }
    // The fastened bracket exposes no joint — it must not invent one.
    assert!(
        driven_fact(&cert, 2).is_none(),
        "a Fastened mate has no freedom to sweep: {:?}",
        cert.sweeps
    );
}

#[test]
fn dragging_each_ring_across_its_band_stays_on_the_manifold() {
    for (mate_index, other) in [(0u32, 1u32), (1, 0)] {
        for &target in &[SIX_DEG, -SIX_DEG, 0.0] {
            let mut assembly = gimbal(SIX_DEG);
            let outcome = assembly.drag(mate_index, DriveParam::Rotation, target);
            let Ok(outcome) = outcome else {
                assert!(false, "a limited revolute drives: {outcome:?}");
                return;
            };
            assert!(
                outcome.report.converged,
                "mate {mate_index} → {target}: {outcome:?}"
            );
            assert!(
                outcome.limit.is_none(),
                "the band's own endpoints are INSIDE it: {outcome:?}"
            );
            // Every mate still holds — the drag rode the joint's own free
            // motion and never tore the gimbal off its manifold.
            for (idx, m) in assembly.mates.iter().enumerate() {
                let violation = assembly.mate_violation(m);
                assert!(
                    violation < 1e-6,
                    "mate {idx} holds after driving {mate_index} to {target}: {violation}"
                );
            }
            // The driven angle landed; the OTHER ring stayed put.
            let (theta, _) = assembly
                .joint_parameters_of(mate_index)
                .unwrap_or((f64::NAN, f64::NAN));
            assert!(
                (theta - target).abs() < 1e-6,
                "driven to {target}, got {theta}"
            );
            let (idle, _) = assembly
                .joint_parameters_of(other)
                .unwrap_or((f64::NAN, f64::NAN));
            assert!(
                idle.abs() < 1e-6,
                "driving one axis must not swing the other: {idle}"
            );
            // A ±6° gimbal is generic — no singular pose in the band.
            assert!(
                outcome.rank_transitions.is_empty(),
                "no rank change in a generic band: {:?}",
                outcome.rank_transitions
            );
        }
    }
}

#[test]
fn beyond_the_limits_the_inner_ring_reaches_the_stop_bracket() {
    // THE PROOF THAT ±6° MEANS SOMETHING. Same geometry, limits widened to
    // ±30°: the inner ring now swings far enough to reach the stop bracket
    // fastened under it, and the swept gate catches it and stamps WHERE.
    // Without this, "certifies clear at ±6°" could be true merely because
    // the sweep never looked far enough to find anything.
    let wide = gimbal(THIRTY_DEG);
    let cert = wide.certify_v2(&[], EPS);

    assert!(
        !cert.swept_clearance_ok,
        "±30° drives the inner ring into the stop: {:?}",
        cert.sweeps
    );
    assert!(!cert.is_sound(), "and that must break soundness");

    let Some(fact) = driven_fact(&cert, 1) else {
        assert!(false, "the inner ring's derived sweep: {:?}", cert.sweeps);
        return;
    };
    assert!(
        !fact.clear,
        "the inner ring's own sweep is what fails: {fact:?}"
    );
    let stamp = fact
        .first_contact
        .map(|c| c.param)
        .or_else(|| fact.interference.first().map(|i| i.at.param));
    let Some(stamp) = stamp else {
        assert!(false, "the hit must be MOTION-STAMPED: {fact:?}");
        return;
    };
    // Geometry: the inner rim dips z = −4·sin θ and its underside meets the
    // bracket foot (top at z = −1.3) near 14.6°. Well outside ±6°, well
    // inside ±30° — the band is what was protecting it.
    assert!(
        stamp.abs() > SIX_DEG && stamp.abs() < THIRTY_DEG,
        "contact stamped between the old band and the new one, got {stamp}"
    );

    // The OUTER axis fails at ±30° too — and this one is worth recording,
    // because the fixture's author expected it to stay clear and was
    // WRONG. Swinging the outer ring 30° drives its rim down into the
    // bearing block it is mated to: a point at local y ≈ −0.6 rotates to
    // z ≈ −0.56 while landing at y ≈ −0.37, i.e. INSIDE the block's ±0.4
    // window. A by-hand check of the rim at y = −0.4 alone says "clear by
    // 0.04" and misses it entirely, because rotation sweeps a whole BAND
    // of y into that window.
    //
    // This is the campaign's thesis in one fixture: the certificate found
    // an interference a careful hand analysis of the same geometry got
    // wrong, and stamped the angle it happens at.
    let Some(outer) = driven_fact(&cert, 0) else {
        assert!(false, "the outer ring's derived sweep: {:?}", cert.sweeps);
        return;
    };
    assert!(
        !outer.clear,
        "±30° also drives the outer rim into its bearing block: {outer:?}"
    );
    let Some(rim) = outer.interference.first() else {
        assert!(false, "the rim/block hit is motion-stamped: {outer:?}");
        return;
    };
    assert!(
        rim.depth > 0.0 && rim.at.param.abs() > SIX_DEG,
        "the rim digs in only well beyond the ±6° band: {rim:?}"
    );
}

#[test]
fn the_limit_band_clamps_a_beyond_stop_drive_and_reports_it() {
    // An agent that asks for 30° on the ±6° gimbal gets 6° and is TOLD —
    // the joint bottomed out, which is information, not a failure.
    let mut assembly = gimbal(SIX_DEG);
    let outcome = assembly.drag(1, DriveParam::Rotation, THIRTY_DEG);
    let Ok(outcome) = outcome else {
        assert!(
            false,
            "a beyond-limit target clamps, never errors: {outcome:?}"
        );
        return;
    };
    assert!(outcome.report.converged);
    assert!((outcome.applied - SIX_DEG).abs() < 1e-12, "{outcome:?}");
    let Some(limit) = outcome.limit else {
        assert!(false, "the at-limit fact must be reported: {outcome:?}");
        return;
    };
    assert_eq!(limit.requested, THIRTY_DEG);
    assert!((limit.max - SIX_DEG).abs() < 1e-12);
    // And the clamped pose is still sound — the stop was never reached.
    assert!(
        assembly.certify_v2(&[], EPS).no_static_interference,
        "clamped at 6°, the inner ring is nowhere near the bracket"
    );
}
