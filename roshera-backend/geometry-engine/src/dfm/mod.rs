//! Design-for-Manufacturability (DFM) subsystem — the kernel learning to
//! answer "can this exist?" with the same self-certifying honesty as "is
//! this sound?" (spec: `Roshera-vault/Research/2026-07-28-dfm-subsystem-spec.md`).
//!
//! Today `ValidationResult::manufacturing_valid` is a hardcoded `bool`
//! (`primitives/validation.rs`) — the exact "kernel can lie" shape this
//! subsystem exists to kill. DFM turns the question into honest capability:
//! every reported number carries its derivation, and every refusal is a
//! typed value in the report, never a fabricated pass.
//!
//! ## Slice S1: types only
//!
//! `report` ships the report/verdict/fact TYPES and the summary honesty
//! fold — no analyzers, no rule packs, no geometry, no B-Rep traversal.
//! `analyze(model, solid, pack) -> DfmReport` (spec §3) lands once the
//! process-agnostic analyzers exist to feed it. Adding a stub `analyze()`
//! now — one that always passes, or always refuses — would itself be the
//! "kernel can lie" defect this subsystem is meant to remove.
//!
//! ## Slice S2: the first analyzer
//!
//! `analyzers::face_orientation_field` — per-face angle range vs a
//! reference direction, exact-or-refuse over the face's TRIMMED parameter
//! domain (spec §3.1).
//!
//! ## Slice S2 (rule packs)
//!
//! `packs::fdm::evaluate_overhang` (`fdm.overhang`) and
//! `packs::molding::evaluate_draft` (`mold.draft`) — the first two rules,
//! both riding `face_orientation_field` against a different reference
//! direction with a different threshold and violation sense, proving the
//! "one analyzer, many rules/packs" architecture (spec §3.2). `analyze()`
//! proper (a generic engine over every pack) is separate, independently-
//! tracked work; see `packs/mod.rs` module docs for why.
//!
//! ## Slice S3 (this addition)
//!
//! `analyzers::pair_thickness` — wall thickness between provably opposing
//! face pairs (parallel planes / coaxial cylinders), exact-or-refuse, and
//! `packs::fdm::evaluate_min_wall` (`fdm.min_wall`: wall thickness ≥ 2×
//! nozzle diameter) riding it. `packs::fdm::evaluate` now runs BOTH FDM
//! rules (`fdm.overhang`, `fdm.min_wall`) and folds across them.
//!
//! Planned layout (spec §3), populated incrementally:
//! ```text
//! dfm/
//!   mod.rs        — public surface: analyze(...) -> DfmReport      (later)
//!   analyzers/    — face_orientation_field (S2), pair_thickness (S3);
//!                   3 more analyzers                               (later)
//!   packs/        — fdm.rs, molding.rs (S2/S3, here); cnc.rs, sheet.rs (later)
//!   report.rs     — DfmReport, RuleVerdict, DfmValue, DfmSummary,
//!                   DfmFact                                         (S1)
//! ```

pub mod analyzers;
pub mod packs;
pub mod provenance;
pub mod report;

pub use analyzers::{
    face_orientation_field, pair_thickness, FacePair, OrientationOutcome, PairThicknessOutcome,
    UnpairedRegion,
};
pub use packs::{Rule, RulePack};
pub use provenance::{RuleProvenance, StandardBody};
pub use report::{
    Derivation, DfmError, DfmFact, DfmReport, DfmSummary, DfmValue, FaceRef, PackParams,
    RulePackId, RuleVerdict, SurfaceKind, UnverifiableReason, Verdict,
};
