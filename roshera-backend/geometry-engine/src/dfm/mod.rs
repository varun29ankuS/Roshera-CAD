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
//! ## Slice S2 (this addition): the first analyzer
//!
//! `analyzers::face_orientation_field` — per-face angle range vs a
//! reference direction, exact-or-refuse over the face's TRIMMED parameter
//! domain (spec §3.1). Rule packs and `analyze()` are separate,
//! independently-tracked work; this slice ships the analyzer only.
//!
//! Planned layout (spec §3), populated incrementally:
//! ```text
//! dfm/
//!   mod.rs        — public surface: analyze(...) -> DfmReport      (later)
//!   analyzers/    — face_orientation_field (S2); 4 more analyzers  (later)
//!   packs/        — rule-pack definitions (fdm.rs, molding.rs, ...) (later)
//!   report.rs     — DfmReport, RuleVerdict, DfmValue, DfmSummary,
//!                   DfmFact                                         (S1, here)
//! ```

pub mod analyzers;
pub mod provenance;
pub mod report;

pub use analyzers::{face_orientation_field, OrientationOutcome};
pub use provenance::{RuleProvenance, StandardBody};
pub use report::{
    Derivation, DfmError, DfmFact, DfmReport, DfmSummary, DfmValue, FaceRef, PackParams,
    RulePackId, RuleVerdict, SurfaceKind, UnverifiableReason, Verdict,
};
