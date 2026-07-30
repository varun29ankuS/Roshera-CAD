//! Design-for-Manufacturability (DFM) subsystem — the kernel learning to
//! answer "can this exist?" with the same self-certifying honesty as "is
//! this sound?" (spec: `Roshera-vault/Research/2026-07-28-dfm-subsystem-spec.md`).
//!
//! `ValidationResult::manufacturing_valid` (`primitives/validation.rs`) was
//! a hardcoded `bool` — the exact "kernel can lie" shape this subsystem
//! exists to kill. Spec S6 (audit id H5) closed that: the field is now
//! `Option<DfmSummary>`, `None` until a real DFM pack has actually run
//! against the solid. DFM turns the question into honest capability: every
//! reported number carries its derivation, and every refusal is a typed
//! value in the report, never a fabricated pass.
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
//! ## Slice S3
//!
//! `analyzers::pair_thickness` — wall thickness between provably opposing
//! face pairs (parallel planes / coaxial cylinders), exact-or-refuse, and
//! `packs::fdm::evaluate_min_wall` (`fdm.min_wall`: wall thickness ≥ 2×
//! nozzle diameter) riding it.
//!
//! ## Slice S4 (this addition)
//!
//! `analyzers::bore_metrics` — per-bore diameter, trimmed axial depth,
//! through-vs-blind, and aspect ratio, reusing
//! `crate::readable::bore_face_ids` verbatim as its concave-cylinder
//! candidate filter (never a second, divergent bore-detection rule), and
//! `packs::fdm::evaluate_min_bore` (`fdm.min_bore`: bore diameter ≥ 2×
//! nozzle diameter) riding it. `packs::fdm::evaluate` now runs all THREE
//! FDM rules (`fdm.overhang`, `fdm.min_wall`, `fdm.min_bore`) and folds
//! across them. Because through-vs-blind needs the SOLID's own extent
//! along the bore axis (not just one face's trim), `bore_metrics` — and
//! therefore `packs::fdm::evaluate`/`packs::evaluate` for the `Fdm` arm —
//! takes `(model, solid_id)` rather than `faces: &[FaceId]` + bare stores;
//! see `analyzers/bore.rs`'s module docs for the full reasoning.
//!
//! ## Slice S6: `manufacturing_valid` Option swap (kills H5)
//!
//! `ValidationResult::manufacturing_valid: bool` → `Option<DfmSummary>`
//! (spec §3.3/§6, audit id H5). Both construction sites in
//! `primitives/validation.rs` (`validate_topology_parallel`'s per-solid
//! loop and `combine_results`) now set `None`: `validate_model_enhanced`
//! does not run DFM analysis, so `None` means NOT ASSESSED, never a
//! fabricated pass. A caller that wants a real verdict runs a
//! [`report::DfmReport`]-producing pack and attaches the resulting
//! [`report::DfmSummary`] itself. [`report::DfmSummary`] now derives
//! `Hash` so `Option<DfmSummary>` can be folded into
//! `generate_signature`'s hand-rolled per-field hash the same way the
//! `bool` it replaced was — `ValidationResult` itself derives only
//! `Debug` and has no derived `Hash` impl. The certificate-riding
//! [`report::DfmFact`] wiring (spec §6 step 4) is separate, later work; this
//! slice is the Option swap only.
//! ```text
//! dfm/
//!   mod.rs        — public surface: analyze(...) -> DfmReport      (later)
//!   analyzers/    — face_orientation_field (S2), pair_thickness (S3),
//!                   bore_metrics (S4); 2 more analyzers             (later)
//!   packs/        — fdm.rs, molding.rs (S2-S4, here); cnc.rs, sheet.rs (later)
//!   report.rs     — DfmReport, RuleVerdict, DfmValue, DfmSummary,
//!                   DfmFact                                         (S1)
//! ```

pub mod analyzers;
pub mod packs;
pub mod provenance;
pub mod report;

pub use analyzers::{
    bore_metrics, face_orientation_field, pair_thickness, BoreMetricsOutcome, BoreRecord, FacePair,
    OrientationOutcome, PairThicknessOutcome, UnpairedRegion, UnverifiableBore,
};
pub use packs::{Rule, RulePack};
pub use provenance::{RuleProvenance, StandardBody};
pub use report::{
    Derivation, DfmError, DfmFact, DfmReport, DfmSummary, DfmValue, FaceRef, PackParams,
    RulePackId, RuleVerdict, SurfaceKind, UnverifiableReason, Verdict,
};
