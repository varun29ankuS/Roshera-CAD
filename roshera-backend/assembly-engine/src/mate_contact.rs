//! Mate-contact verification — mated parts must actually TOUCH.
//!
//! Mate-anchoring proved the feature is real (it sits on the part). This proves
//! the JOINT is real: the two parts a mate connects must be in contact at it.
//! Every mate kind here — concentric, coincident, fixed — is a CONTACT mate (a
//! shaft seats in a bore, a face seats on a face); none holds parts apart at a
//! distance. So two parts joined by a mate yet sitting with a gap are connected
//! only on paper — coaxial-but-floating, the pipe that grazes its port without
//! reaching it. The clearance between every mated pair must be ~0, or the
//! certificate must say the joint isn't really there.

use crate::types::{Assembly, InstanceId};
use serde::{Deserialize, Serialize};

/// A mate whose two parts don't touch — declared, but not physically joined.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisconnectedMate {
    pub mate_index: usize,
    pub a: InstanceId,
    pub b: InstanceId,
    /// The clearance between the parts the mate claims to join (the gap, measured).
    pub gap: f64,
}

/// The contact verdict for an assembly's mates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MateContactReport {
    pub disconnected: Vec<DisconnectedMate>,
}

impl MateContactReport {
    /// True when every mated pair is actually touching — no paper joints.
    pub fn all_in_contact(&self) -> bool {
        self.disconnected.is_empty()
    }
}

impl Assembly {
    /// For every mate, the clearance between the two parts it joins. A contact
    /// mate should hold them touching, so a gap larger than `tol` means the
    /// parts are mated only on paper — joined to nothing.
    pub fn mate_contact_report(&self, tol: f64) -> MateContactReport {
        let mut disconnected = Vec::new();
        for (idx, mate) in self.mates.iter().enumerate() {
            // `clearance` is 0 when the parts touch or overlap (overlap is the
            // interference check's job); a positive value is a real gap.
            if let Some(gap) = self.clearance(mate.a, mate.b) {
                if gap > tol {
                    disconnected.push(DisconnectedMate {
                        mate_index: idx,
                        a: mate.a,
                        b: mate.b,
                        gap,
                    });
                }
            }
        }
        MateContactReport { disconnected }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FeatureRef, Instance, Mate, MateKind, Mesh};

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

    fn cube_at(id: u32, x: f64) -> Instance {
        let mut i = Instance::new(InstanceId(id), format!("cube_{id}"), cube(1.0));
        i.translation = [x, 0.0, 0.0];
        i
    }

    // A face mate between the two parts — the kind doesn't matter to contact,
    // only that a mate joins them.
    fn join() -> Mate {
        Mate {
            kind: MateKind::Coincident,
            a: InstanceId(0),
            feature_a: FeatureRef::Face {
                point: [1.0, 0.0, 0.0],
                normal: [1.0, 0.0, 0.0],
            },
            b: InstanceId(1),
            feature_b: FeatureRef::Face {
                point: [-1.0, 0.0, 0.0],
                normal: [-1.0, 0.0, 0.0],
            },
        }
    }

    #[test]
    fn touching_parts_are_in_contact() {
        // Two cubes face-to-face at x=1 — a real seated joint.
        let mut a = Assembly::new(InstanceId(0));
        a.add_instance(cube_at(0, 0.0)); // x in [-1, 1]
        a.add_instance(cube_at(1, 2.0)); // x in [1, 3], touching at x=1
        a.add_mate(join());
        assert!(a.mate_contact_report(0.25).all_in_contact());
    }

    #[test]
    fn gapped_parts_are_disconnected() {
        // The same mate, but part 1 floats 4 units away — joined on paper only.
        let mut a = Assembly::new(InstanceId(0));
        a.add_instance(cube_at(0, 0.0)); // x in [-1, 1]
        a.add_instance(cube_at(1, 6.0)); // x in [5, 7], a 4-unit gap
        a.add_mate(join());
        let report = a.mate_contact_report(0.25);
        assert!(
            !report.all_in_contact(),
            "a gapped mate is not a real joint"
        );
        assert_eq!(report.disconnected.len(), 1);
        assert!(
            report.disconnected[0].gap > 3.5,
            "the measured gap should be ~4, got {}",
            report.disconnected[0].gap
        );
    }

    #[test]
    fn no_mates_is_vacuously_in_contact() {
        let mut a = Assembly::new(InstanceId(0));
        a.add_instance(cube_at(0, 0.0));
        assert!(a.mate_contact_report(0.25).all_in_contact());
    }
}
