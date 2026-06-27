//! Roshera assembly module — **kinematic**.
//!
//! An assembly is not a static arrangement of positioned parts; it is a
//! kinematic system whose mechanisms (gimbal, actuators, shafts) are degrees of
//! freedom in a mate graph. "Sound assembly" means interference-free across
//! every reachable configuration of those DOF — not just the current pose.
//!
//! Design: `Roshera-vault/Development-Journal/assembly-module-design.md`.
//!
//! Layering (built slice by slice via `ASSEMBLY_LOOP.md`):
//!   * data model — instances + mates on named features                (this slice)
//!   * grounding — the no-float check                                   (this slice)
//!   * Parry — broad/narrow-phase interference, then CCD swept clearance (S2+)
//!   * mate solver — SE(3) geometric constraint solve + DOF analysis     (S4+)
//!   * assembly certificate — grounded · dof-as-designed · clear         (S8)
//!
//! The mate-solve and the certificate are OURS (the moat); Parry is the
//! commodity collision/CCD engine underneath.

pub mod certificate;
pub mod grounding;
pub mod interference;
pub mod joint;
pub mod mate_residual;
pub mod report;
pub mod solver;
pub mod sweep;
pub mod types;

pub use certificate::{AssemblyCertificate, Mechanism};
pub use grounding::GroundingReport;
pub use interference::{InterferencePair, InterferenceReport};
pub use joint::Joint;
pub use report::AssemblyReport;
pub use solver::{DofReport, Mobility, SolveReport};
pub use sweep::{swept_clearance, SweptClearance};
pub use types::{Assembly, FeatureRef, Instance, InstanceId, Mate, MateKind, Mesh};
