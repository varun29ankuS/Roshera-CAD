//! Sketch-validity certificate — the 2D analogue of the solid
//! [`crate::primitives::provenance::ValidityCertificate`].
//!
//! This is the "can't lie" moat extended from solids to sketches. Every field
//! is **computed** from the constraint solver's DOF analysis
//! ([`super::sketch_solver::analyze_dofs`]), the validation engine
//! ([`super::sketch_validation::SketchValidator`]), and the topology analyser
//! ([`super::sketch_topology::SketchTopology`]) — never asserted. Because the
//! sketch entity and its solver live in the kernel, the kernel can certify a
//! sketch the way it certifies a solid: it refuses to call an internally
//! inconsistent sketch sound.
//!
//! Soundness vs. reporting: an **under-constrained** sketch (free DOFs remain)
//! or an **open** profile is a perfectly legal sketch — just not fully
//! determined / not extrude-ready. The certificate *reports* those; it does not
//! fail them. A sketch is UNSOUND only when its constraints are mutually
//! inconsistent, an entity is geometrically degenerate, or the geometry
//! self-intersects.

use serde::{Deserialize, Serialize};

use super::sketch_solver::DofStatus;
use super::sketch_topology::{ProfileType, SketchTopology};
use super::sketch_validation::{SketchValidator, ValidationIssue};
use super::Sketch;

/// Degree-of-freedom verdict over a sketch's constraint system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SketchConstrainedness {
    /// Exactly determined: zero free DOFs, no excess constraints.
    FullyConstrained,
    /// Free DOFs remain — the geometry can still move.
    UnderConstrained { free_dofs: usize },
    /// More constraints than DOFs, but the surplus is REDUNDANT — the system
    /// is still consistent (it has a solution).
    OverConstrained { redundant: usize },
    /// Over-constrained AND inconsistent: a subset of constraints cannot be
    /// satisfied simultaneously. The kernel refuses to call this sound.
    Conflicting { conflicts: usize },
}

impl SketchConstrainedness {
    /// True when the sketch is exactly determined.
    pub fn is_fully_constrained(&self) -> bool {
        matches!(self, Self::FullyConstrained)
    }
    /// True when constraints are mutually inconsistent.
    pub fn is_conflicting(&self) -> bool {
        matches!(self, Self::Conflicting { .. })
    }
}

/// The kernel's self-certified, can't-lie verdict on a 2D sketch.
///
/// See the module docs for the soundness contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SketchValidityCertificate {
    /// DOF verdict (under / fully / over / conflicting).
    pub constrainedness: SketchConstrainedness,
    /// True when no subset of constraints is mutually inconsistent (the solver
    /// found no conflicting rows in the Jacobian).
    pub constraint_consistent: bool,
    /// Count of redundant (safe-to-remove) constraints.
    pub redundant_constraints: usize,
    /// The profile forms one or more closed regions (extrude/revolve-ready).
    pub closed_profile: bool,
    /// Human description of the profile topology (simple / nested / open / …).
    pub profile: String,
    /// No entity self-intersects.
    pub self_intersection_free: bool,
    /// Every entity is geometrically valid (no zero-length / degenerate / NaN).
    pub entities_valid: bool,
    /// Human-readable issues backing a non-sound verdict.
    pub issues: Vec<String>,
}

impl SketchValidityCertificate {
    /// The single can't-lie predicate: a sketch is SOUND when its constraint
    /// system is satisfiable, its geometry is valid, and it does not
    /// self-intersect. DOF freedom and open profiles are reported, not failed.
    pub fn is_sound(&self) -> bool {
        self.constraint_consistent && self.entities_valid && self.self_intersection_free
    }

    /// One-line human summary for the agent eye / blackboard.
    pub fn summary(&self) -> String {
        let dof = match self.constrainedness {
            SketchConstrainedness::FullyConstrained => "fully-constrained".to_string(),
            SketchConstrainedness::UnderConstrained { free_dofs } => {
                format!("under-constrained ({free_dofs} DOF)")
            }
            SketchConstrainedness::OverConstrained { redundant } => {
                format!("over-constrained ({redundant} redundant)")
            }
            SketchConstrainedness::Conflicting { conflicts } => {
                format!("CONFLICTING ({conflicts})")
            }
        };
        format!(
            "sketch {} · {dof} · {} · {}",
            if self.is_sound() { "SOUND" } else { "UNSOUND" },
            if self.closed_profile {
                "closed"
            } else {
                "open"
            },
            self.profile,
        )
    }
}

/// Certify a sketch: run DOF analysis, validation, and topology, then package
/// the kernel's verdict. Pure (`&Sketch`) — no mutation, no solve side effect.
pub fn certify_sketch(sketch: &Sketch) -> SketchValidityCertificate {
    let dof = sketch.analyze_dofs();
    let validation = SketchValidator::new().validate(sketch);
    let profile = SketchTopology::analyze(sketch, &sketch.tolerance)
        .map(|t| t.profile_type())
        .unwrap_or(ProfileType::Open);

    let numeric_conflicts = dof.conflicts.len();
    let redundant = dof.redundant.len();
    // Two independent conflict detectors, unioned — a contradiction caught by
    // EITHER makes the sketch inconsistent:
    //  - numerical: the solver's Jacobian rank/residual diagnosis (catches
    //    e.g. distance=10 AND distance=20);
    //  - static: configuration-independent contradictory pairs
    //    (Parallel+Perpendicular, Horizontal+Vertical, Coincident vs Distance),
    //    which the numerical pass can MISS when geometry degenerates (a line
    //    direction collapsing to zero vacuously satisfies both Parallel and
    //    Perpendicular). DCM/OCCT-grade solvers need both; so do we.
    let static_conflicts = sketch.find_constraint_conflicts();
    let conflict_count = numeric_conflicts + static_conflicts.len();
    let constraint_consistent = conflict_count == 0;

    // A truly inconsistent constraint subset trumps the structural DOF count:
    // the sketch cannot be solved as posed.
    let constrainedness = if conflict_count > 0 {
        SketchConstrainedness::Conflicting {
            conflicts: conflict_count,
        }
    } else {
        match dof.status {
            DofStatus::FullyConstrained => SketchConstrainedness::FullyConstrained,
            DofStatus::UnderConstrained { dofs } => {
                SketchConstrainedness::UnderConstrained { free_dofs: dofs }
            }
            // Over-constrained with no conflicts == the surplus is purely
            // redundant (consistent). Report it with the redundant count.
            DofStatus::OverConstrained { .. } => {
                SketchConstrainedness::OverConstrained { redundant }
            }
        }
    };

    let closed_profile = matches!(
        profile,
        ProfileType::Simple | ProfileType::Nested | ProfileType::Disjoint
    );
    let profile_desc = match profile {
        ProfileType::Simple => "simple closed region",
        ProfileType::Nested => "nested (with holes)",
        ProfileType::Disjoint => "disjoint closed regions",
        ProfileType::Open => "open curves",
        ProfileType::Mixed => "mixed open/closed",
    }
    .to_string();

    let mut self_intersection_free = true;
    let mut entities_valid = true;
    let mut issues = Vec::new();

    for issue in &validation.issues {
        match issue {
            ValidationIssue::SelfIntersection { .. } => {
                self_intersection_free = false;
                issues.push("self-intersecting entity".to_string());
            }
            ValidationIssue::ZeroLengthLine { .. } => {
                entities_valid = false;
                issues.push("zero-length line".to_string());
            }
            ValidationIssue::DegenerateArc { reason, .. } => {
                entities_valid = false;
                issues.push(format!("degenerate arc: {reason}"));
            }
            ValidationIssue::InvalidEntity { reason, .. } => {
                entities_valid = false;
                issues.push(format!("invalid entity: {reason}"));
            }
            ValidationIssue::NumericalPrecision { .. } => {
                entities_valid = false;
                issues.push("numerical-precision defect".to_string());
            }
            _ => {}
        }
    }

    if numeric_conflicts > 0 {
        issues.push(format!(
            "{numeric_conflicts} numerically-inconsistent constraint(s)"
        ));
    }
    for (a, b) in &static_conflicts {
        issues.push(format!("contradictory constraint pair ({a} \u{2194} {b})"));
    }

    SketchValidityCertificate {
        constrainedness,
        constraint_consistent,
        redundant_constraints: redundant,
        closed_profile,
        profile: profile_desc,
        self_intersection_free,
        entities_valid,
        issues,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sketch2d::{Point2d, Sketch, SketchAnchor};

    /// A clean, unconstrained triangle: three points joined by three lines.
    /// It is geometrically valid and does not self-intersect, so the kernel
    /// must certify it SOUND — but it is under-constrained (the points are
    /// free to move), which the certificate must REPORT, not fail.
    #[test]
    fn clean_triangle_is_sound_and_under_constrained() {
        let sketch = Sketch::new("triangle".to_string(), SketchAnchor::xy());
        let a = sketch.add_point(Point2d::new(0.0, 0.0));
        let b = sketch.add_point(Point2d::new(10.0, 0.0));
        let c = sketch.add_point(Point2d::new(5.0, 8.0));
        sketch.add_line(a, b).expect("edge a-b");
        sketch.add_line(b, c).expect("edge b-c");
        sketch.add_line(c, a).expect("edge c-a");

        let cert = certify_sketch(&sketch);

        // The moat: a valid, non-self-intersecting sketch is SOUND.
        assert!(cert.is_sound(), "clean triangle must be sound: {cert:?}");
        assert!(cert.constraint_consistent, "no conflicting constraints");
        assert!(
            cert.self_intersection_free,
            "triangle does not self-intersect"
        );
        assert!(cert.entities_valid, "all three edges are valid");
        // With no dimensional/geometric constraints the points are free —
        // the certificate must NOT claim it is fully constrained.
        assert!(
            !cert.constrainedness.is_fully_constrained(),
            "an unconstrained triangle is not fully constrained: {:?}",
            cert.constrainedness
        );
        assert!(
            !cert.constrainedness.is_conflicting(),
            "an unconstrained triangle has no conflicts"
        );
        // summary() must not panic and must mark it sound.
        assert!(cert.summary().contains("SOUND"));
    }
}
