//! The Phase-1 assembly verdict.
//!
//! Composes the two Phase-1 checks — grounding (no float) and static
//! interference (no overlap) — into one answer: is this assembly buildable as
//! posed? This is the Phase-1 slice of the assembly certificate; Phase 2 adds
//! `dof_as_designed` and `swept_clearance`.

use crate::grounding::GroundingReport;
use crate::interference::InterferenceReport;
use crate::types::{Assembly, InstanceId};

/// The combined Phase-1 verdict for an assembly.
#[derive(Debug, Clone, PartialEq)]
pub struct AssemblyReport {
    pub grounding: GroundingReport,
    pub interference: InterferenceReport,
}

impl AssemblyReport {
    /// Buildable as posed: every part reaches ground (nothing floats) AND no two
    /// parts overlap. The Phase-1 certificate verdict — the non-fakeable answer
    /// to "does this physically go together as placed?".
    pub fn assemblable_phase1(&self) -> bool {
        self.grounding.fully_grounded() && self.interference.no_static_interference()
    }

    /// Instances that float (no mate path to ground) — empty when sound.
    pub fn floats(&self) -> &[InstanceId] {
        &self.grounding.floating
    }
}

impl Assembly {
    /// Run the Phase-1 checks and return the combined verdict.
    pub fn phase1_report(&self) -> AssemblyReport {
        AssemblyReport {
            grounding: self.grounding_report(),
            interference: self.interference_report(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FeatureRef, Instance, Mate, MateKind, Mesh};

    /// An axis-aligned cube of side `2*h` centred at the origin (local frame).
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

    fn cube_at(id: u32, h: f64, x: f64) -> Instance {
        let mut instance = Instance::new(InstanceId(id), format!("cube_{id}"), cube(h));
        instance.translation = [x, 0.0, 0.0];
        instance
    }

    fn concentric(a: u32, b: u32) -> Mate {
        Mate {
            kind: MateKind::Concentric,
            a: InstanceId(a),
            feature_a: FeatureRef::Axis {
                origin: [0.0; 3],
                direction: [0.0, 0.0, 1.0],
            },
            b: InstanceId(b),
            feature_b: FeatureRef::Axis {
                origin: [0.0; 3],
                direction: [0.0, 0.0, 1.0],
            },
        }
    }

    #[test]
    fn grounded_and_clear_is_assemblable() {
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(cube_at(0, 1.0, 0.0));
        assembly.add_instance(cube_at(1, 1.0, 5.0)); // separated
        assembly.add_mate(concentric(0, 1)); // grounded
        let report = assembly.phase1_report();
        assert!(report.assemblable_phase1());
        assert!(report.floats().is_empty());
    }

    #[test]
    fn a_float_makes_it_not_assemblable() {
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(cube_at(0, 1.0, 0.0));
        assembly.add_instance(cube_at(1, 1.0, 5.0));
        assembly.add_instance(cube_at(2, 1.0, 10.0)); // floats — no mate
        assembly.add_mate(concentric(0, 1));
        let report = assembly.phase1_report();
        assert!(!report.assemblable_phase1());
        assert_eq!(report.floats(), &[InstanceId(2)]);
    }

    #[test]
    fn an_overlap_makes_it_not_assemblable() {
        // Grounded but interfering — the verdict must still be NO.
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(cube_at(0, 1.0, 0.0));
        assembly.add_instance(cube_at(1, 1.0, 0.5)); // overlaps part 0
        assembly.add_mate(concentric(0, 1));
        let report = assembly.phase1_report();
        assert!(report.grounding.fully_grounded());
        assert!(!report.interference.no_static_interference());
        assert!(!report.assemblable_phase1());
    }
}
