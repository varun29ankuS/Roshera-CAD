//! The full assembly certificate — the non-fakeable "does this physically go
//! together and move without collision?" verdict.
//!
//! Seven dimensions, fused at the SOLVED configuration (we solve a clone first so
//! the static and swept checks run at the pose the mates actually produce):
//!   * `mates_consistent`     — the constraint system is satisfiable (the solve converges)
//!   * `fully_grounded`       — every part reaches ground (nothing floats)
//!   * `dof` / `mobility`     — the assembly's residual freedom
//!   * `no_static_interference` — no two parts overlap at the solved pose
//!   * `swept_clearance_ok`   — every mechanism stays clear across its full motion
//!   * `mates_anchored`       — every mate's features sit on real geometry (no fabricated joint)
//!   * `mates_in_contact`     — every mated pair actually touches (no paper joint)
//!
//! The kernel cannot return a `sound` assembly that doesn't assemble, self-
//! collides through its motion, or is held together by joints that aren't there.

use crate::joint::Joint;
use crate::solver::Mobility;
use crate::sweep::swept_clearance;
use crate::types::{Assembly, InstanceId};
use serde::{Deserialize, Serialize};

/// A mechanism to verify swept clearance for during certification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mechanism {
    pub moving: InstanceId,
    pub joint: Joint,
    pub base_translation: [f64; 3],
    pub base_rotation: [f64; 4],
    pub range: (f64, f64),
    pub samples: usize,
}

/// The full assembly certificate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssemblyCertificate {
    /// The mate system is satisfiable (the Gauss-Newton solve converges).
    pub mates_consistent: bool,
    /// Every part reaches ground (nothing floats).
    pub fully_grounded: bool,
    /// Free degrees of freedom at the solved pose.
    pub dof: usize,
    pub mobility: Mobility,
    /// No two parts overlap at the solved pose.
    pub no_static_interference: bool,
    /// Every supplied mechanism stays clear across its full range of motion.
    pub swept_clearance_ok: bool,
    /// Every mate's features sit on their parts' real geometry — no part is
    /// grounded through a constraint declared against an invented coordinate.
    pub mates_anchored: bool,
    /// Every mated pair actually touches — no part is joined to another only on
    /// paper, sitting coaxial-but-floating with a gap between them.
    pub mates_in_contact: bool,
    /// Every mate is NUMERICALLY ENFORCED by the solver. A typed-but-
    /// unenforced mate (the honest-refuse set: Cam/Path/Symmetric, feature
    /// mismatches, broken coupling references) contributes zero residual
    /// rows — it must block soundness, never silently ride a `sound`
    /// verdict. Serde-defaults to `true` so pre-Slice-2 payloads parse.
    #[serde(default = "default_true")]
    pub mates_enforced: bool,
}

fn default_true() -> bool {
    true
}

impl AssemblyCertificate {
    /// The single verdict: assembles, and moves without collision — through
    /// joints that actually exist AND actually connect.
    pub fn is_sound(&self) -> bool {
        self.mates_consistent
            && self.fully_grounded
            && self.no_static_interference
            && self.swept_clearance_ok
            && self.mates_anchored
            && self.mates_in_contact
            && self.mates_enforced
    }
}

impl Assembly {
    /// Certify the assembly. The mate solve runs on a clone, and the static +
    /// swept checks then run at that solved configuration — the pose the mates
    /// actually produce, not the raw authored coordinates.
    pub fn certify(&self, mechanisms: &[Mechanism], epsilon: f64) -> AssemblyCertificate {
        // A mate feature may float at most this far off its part before the
        // joint is judged fabricated (a constraint to a coordinate, not a part).
        const MATE_ANCHOR_TOL: f64 = 0.5;
        // A mated pair must close to within this gap or the joint is judged a
        // paper joint (parts coaxial-but-floating, not actually touching).
        const MATE_CONTACT_TOL: f64 = 0.25;

        let fully_grounded = self.grounding_report().fully_grounded();
        // Anchoring is pose-independent (features are local), so it reads the
        // assembly as declared — before the solve can paper over a fake joint.
        let mates_anchored = self.mate_anchor_report(MATE_ANCHOR_TOL).all_anchored();
        // Enforcement is declaration-level too: a refused mate contributes no
        // residual rows, so it is judged before any solve can hide it.
        let mates_enforced = self.mate_enforcement_report().all_enforced();

        // Slice 3: the certificate's internal solve rides the decomposed
        // pipeline (condensation / DR-plan / verified dense fallback) —
        // same verdict contract, near-linear work on tree-like assemblies.
        let mut solved = self.clone();
        let (solve_report, _decomposition) = solved.solve_decomposed();
        let mates_consistent = solve_report.converged;

        let dof_report = solved.dof_analysis();
        let no_static_interference = solved.interference_report().no_static_interference();
        // Contact is judged at the SOLVED pose — the configuration the mates
        // actually produce — so a mate that the solve pulls into contact passes.
        let mates_in_contact = solved
            .mate_contact_report(MATE_CONTACT_TOL)
            .all_in_contact();

        let swept_clearance_ok = mechanisms.iter().all(|m| {
            !swept_clearance(
                &solved,
                m.moving,
                &m.joint,
                &m.base_translation,
                &m.base_rotation,
                m.range,
                m.samples,
                epsilon,
            )
            .collides
        });

        AssemblyCertificate {
            mates_consistent,
            fully_grounded,
            dof: dof_report.dof,
            mobility: dof_report.mobility,
            no_static_interference,
            swept_clearance_ok,
            mates_anchored,
            mates_in_contact,
            mates_enforced,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FeatureRef, Instance, Mate, MateKind, Mesh};
    use std::f64::consts::PI;

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

    fn cube_at(id: u32, pos: [f64; 3]) -> Instance {
        let mut instance = Instance::new(InstanceId(id), format!("cube_{id}"), cube(1.0));
        instance.translation = pos;
        instance
    }

    /// Concentric: instance `b` (placed at `axis_origin`) to ground's z-axis
    /// through `axis_origin` — satisfied where `b` already sits, so it grounds
    /// `b` without moving it.
    fn concentric_to(b: u32, axis_origin: [f64; 3]) -> Mate {
        Mate {
            kind: MateKind::Concentric,
            a: InstanceId(0),
            feature_a: FeatureRef::Axis {
                origin: axis_origin,
                direction: [0.0, 0.0, 1.0],
            },
            b: InstanceId(b),
            feature_b: FeatureRef::Axis {
                origin: [0.0, 0.0, 0.0],
                direction: [0.0, 0.0, 1.0],
            },
        }
    }

    fn plane_to(b: u32, a_point: [f64; 3]) -> Mate {
        Mate {
            kind: MateKind::Coincident,
            a: InstanceId(0),
            feature_a: FeatureRef::Face {
                point: a_point,
                normal: [0.0, 0.0, 1.0],
            },
            b: InstanceId(b),
            feature_b: FeatureRef::Face {
                point: [0.0, 0.0, 0.0],
                normal: [0.0, 0.0, -1.0],
            },
        }
    }

    #[test]
    fn clean_assembly_is_sound() {
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(cube_at(0, [0.0, 0.0, 0.0])); // ground, z in [-1, 1]
        assembly.add_instance(cube_at(1, [0.0, 0.0, 2.0])); // seated on top, touching at z=1
        assembly.add_mate(concentric_to(1, [0.0, 0.0, 0.0]));

        let cert = assembly.certify(&[], 0.01);
        assert!(cert.is_sound(), "{cert:?}");
        assert!(cert.mates_consistent && cert.fully_grounded);
        assert!(cert.no_static_interference && cert.swept_clearance_ok);
        assert!(cert.mates_anchored && cert.mates_in_contact);
    }

    #[test]
    fn a_floating_part_is_not_sound() {
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(cube_at(0, [0.0, 0.0, 0.0]));
        assembly.add_instance(cube_at(1, [20.0, 0.0, 0.0])); // no mate → floats
        let cert = assembly.certify(&[], 0.01);
        assert!(!cert.is_sound());
        assert!(!cert.fully_grounded);
    }

    #[test]
    fn conflicting_mates_are_not_sound() {
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(cube_at(0, [0.0, 0.0, 0.0]));
        assembly.add_instance(cube_at(1, [0.0, 0.0, 0.0]));
        assembly.add_mate(plane_to(1, [0.0, 0.0, 0.0])); // face at z=0
        assembly.add_mate(plane_to(1, [0.0, 0.0, 5.0])); // and at z=5 — impossible
        let cert = assembly.certify(&[], 0.01);
        assert!(!cert.is_sound());
        assert!(!cert.mates_consistent);
    }

    #[test]
    fn an_interfering_pair_is_not_sound() {
        // Grounded and consistent, but part 1 is driven deep into the ground part
        // (overlapping by 1.2, not merely touching) — only the interference
        // dimension fails.
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(cube_at(0, [0.0, 0.0, 0.0])); // [-1, 1]^3
        assembly.add_instance(cube_at(1, [0.8, 0.0, 0.0])); // overlaps by 1.2 in x
        assembly.add_mate(concentric_to(1, [0.8, 0.0, 0.0]));
        let cert = assembly.certify(&[], 0.01);
        assert!(!cert.is_sound());
        assert!(cert.mates_consistent && cert.fully_grounded);
        assert!(!cert.no_static_interference);
    }

    #[test]
    fn a_colliding_mechanism_is_not_sound() {
        // Static layout is clean (grounded, consistent, no overlap), but a
        // mechanism sweeps part 1 through part 2.
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(cube_at(0, [0.0, 0.0, 0.0])); // ground hub
        assembly.add_instance(cube_at(1, [10.0, 0.0, 0.0])); // swinging arm
        assembly.add_instance(cube_at(2, [0.0, 10.0, 0.0])); // on the swing circle
        assembly.add_mate(concentric_to(1, [10.0, 0.0, 0.0]));
        assembly.add_mate(concentric_to(2, [0.0, 10.0, 0.0]));

        let mechanism = Mechanism {
            moving: InstanceId(1),
            joint: Joint::Revolute {
                axis_origin: [0.0, 0.0, 0.0],
                axis_dir: [0.0, 0.0, 1.0],
            },
            base_translation: [10.0, 0.0, 0.0],
            base_rotation: [0.0, 0.0, 0.0, 1.0],
            range: (0.0, 2.0 * PI),
            samples: 73,
        };
        let cert = assembly.certify(&[mechanism], 0.01);
        assert!(cert.fully_grounded && cert.mates_consistent && cert.no_static_interference);
        assert!(!cert.swept_clearance_ok, "the arm sweeps through part 2");
        assert!(!cert.is_sound());
    }

    #[test]
    fn sound_implies_every_dimension_holds() {
        // VERIFY/HARNESS invariant: a sound certificate never carries a failing
        // dimension. Check across the sound case and several broken ones.
        let mut sound = Assembly::new(InstanceId(0));
        sound.add_instance(cube_at(0, [0.0, 0.0, 0.0]));
        sound.add_instance(cube_at(1, [0.0, 0.0, 2.0])); // seated on top, touching
        sound.add_mate(concentric_to(1, [0.0, 0.0, 0.0]));

        let mut floating = Assembly::new(InstanceId(0));
        floating.add_instance(cube_at(0, [0.0, 0.0, 0.0]));
        floating.add_instance(cube_at(1, [20.0, 0.0, 0.0]));

        for assembly in [&sound, &floating] {
            let cert = assembly.certify(&[], 0.01);
            if cert.is_sound() {
                assert!(cert.mates_consistent);
                assert!(cert.fully_grounded);
                assert!(cert.no_static_interference);
                assert!(cert.swept_clearance_ok);
                assert!(cert.mates_anchored);
            }
        }
    }
}
