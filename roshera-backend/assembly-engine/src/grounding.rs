//! Grounding analysis — the **no-float check**.
//!
//! An instance is GROUNDED iff a path of mates connects it to the assembly's
//! ground instance. Any instance with no such path is FLOATING — the literal
//! defect behind "parts hanging in the air" (the rocket-engine massing: a part
//! placed by raw coordinate, joined to nothing).
//!
//! This is purely topological (the mate graph), so it needs no geometry and is
//! O(instances + mates).

use crate::types::{Assembly, InstanceId};
use std::collections::{HashMap, HashSet, VecDeque};

/// The result of grounding analysis: which instances reach ground, which float.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroundingReport {
    pub grounded: Vec<InstanceId>,
    pub floating: Vec<InstanceId>,
}

impl GroundingReport {
    /// True when every instance is connected to ground — nothing floats.
    pub fn fully_grounded(&self) -> bool {
        self.floating.is_empty()
    }
}

impl Assembly {
    /// Breadth-first reachability over the (undirected) mate graph from the
    /// ground instance. Instances reached are grounded; the rest float.
    pub fn grounding_report(&self) -> GroundingReport {
        // Build undirected adjacency from the mates. Every instance gets an
        // entry so an isolated (un-mated) instance is represented.
        let mut adjacency: HashMap<InstanceId, Vec<InstanceId>> = HashMap::new();
        for instance in &self.instances {
            adjacency.entry(instance.id).or_default();
        }
        for mate in &self.mates {
            adjacency.entry(mate.a).or_default().push(mate.b);
            adjacency.entry(mate.b).or_default().push(mate.a);
        }

        // BFS from ground (only if ground is a real instance).
        let mut reached: HashSet<InstanceId> = HashSet::new();
        let mut frontier: VecDeque<InstanceId> = VecDeque::new();
        if self.instance(self.ground).is_some() {
            reached.insert(self.ground);
            frontier.push_back(self.ground);
        }
        while let Some(current) = frontier.pop_front() {
            if let Some(neighbours) = adjacency.get(&current) {
                for &neighbour in neighbours {
                    if reached.insert(neighbour) {
                        frontier.push_back(neighbour);
                    }
                }
            }
        }

        // Partition in stable instance order.
        let mut grounded = Vec::new();
        let mut floating = Vec::new();
        for instance in &self.instances {
            if reached.contains(&instance.id) {
                grounded.push(instance.id);
            } else {
                floating.push(instance.id);
            }
        }
        GroundingReport { grounded, floating }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FeatureRef, Instance, Mate, MateKind, Mesh};

    fn part(id: u32) -> Instance {
        Instance::new(InstanceId(id), format!("part_{id}"), Mesh::default())
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
    fn a_chain_of_mates_to_ground_is_fully_grounded() {
        let mut assembly = Assembly::new(InstanceId(0));
        for id in 0..4 {
            assembly.add_instance(part(id));
        }
        assembly.add_mate(concentric(0, 1));
        assembly.add_mate(concentric(1, 2));
        assembly.add_mate(concentric(2, 3));

        let report = assembly.grounding_report();
        assert!(report.fully_grounded(), "a chain to ground must not float");
        assert!(report.floating.is_empty());
        assert_eq!(report.grounded.len(), 4);
    }

    #[test]
    fn an_unmated_instance_is_flagged_floating() {
        // The rocket-engine defect, distilled: a part placed by coordinate with
        // no mate to ground. The module MUST flag it — a shaded render and a
        // per-part "SOUND" verdict never could.
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(part(0)); // ground
        assembly.add_instance(part(1)); // mated to ground
        assembly.add_instance(part(2)); // FLOATING — no mate at all
        assembly.add_mate(concentric(0, 1));

        let report = assembly.grounding_report();
        assert!(!report.fully_grounded());
        assert_eq!(report.floating, vec![InstanceId(2)]);
        assert_eq!(report.grounded, vec![InstanceId(0), InstanceId(1)]);
    }

    #[test]
    fn no_instance_is_both_grounded_and_floating() {
        // Invariant (VERIFY loop): the partition is exhaustive and disjoint.
        let mut assembly = Assembly::new(InstanceId(0));
        for id in 0..5 {
            assembly.add_instance(part(id));
        }
        assembly.add_mate(concentric(0, 1));
        assembly.add_mate(concentric(0, 3));
        // 2 and 4 left floating.

        let report = assembly.grounding_report();
        let grounded: HashSet<_> = report.grounded.iter().copied().collect();
        let floating: HashSet<_> = report.floating.iter().copied().collect();
        assert!(grounded.is_disjoint(&floating), "disjoint partition");
        assert_eq!(
            report.grounded.len() + report.floating.len(),
            assembly.instances.len(),
            "exhaustive partition"
        );
    }
}
