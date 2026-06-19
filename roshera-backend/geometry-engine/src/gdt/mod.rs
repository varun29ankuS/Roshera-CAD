//! Geometric dimensioning & tolerancing (GD&T) — a tolerance data model plus the
//! differentiator: **kernel-verified conformance**.
//!
//! Every CAD system can *annotate* a drawing with tolerances. The thing that
//! cannot lie is a kernel that takes the ACTUAL geometry it built, measures it
//! against the declared tolerance zone, and reports pass/fail from the B-Rep
//! itself. That is what this module delivers for the datum-free families
//! (dimensional size + form: flatness, circularity, cylindricity, straightness),
//! and what it honestly declines (`NotYetVerified`) for the datum-referenced
//! families (orientation, location, runout, profile) until a datum reference
//! frame is wired.
//!
//! ## Layout
//!
//! * [`model`] — the tolerance vocabulary: dimensional tolerances, the fourteen
//!   geometric characteristics, material modifiers, datum refs, the feature
//!   control frame, and the per-feature [`model::Annotation`].
//! * [`sidecar`] — the [`sidecar::GdtSidecar`] that binds annotations to features
//!   by [`crate::primitives::persistent_id::PersistentId`], stored beside the
//!   topology stores (clear/snapshot-safe, mirroring the PID/provenance sidecar).
//! * [`fit`] — least-squares geometry fitting (plane/line/circle/cylinder) via
//!   the kernel SVD; the numerical engine behind form verification.
//! * [`verify`] — the conformance verifier proper, returning a tri-state
//!   [`verify::Conformance`] verdict with the measured actual value.
//!
//! ## Honesty
//!
//! No path in this module fabricates an in-spec verdict. An unimplemented
//! characteristic or an unresolved fit class returns
//! [`verify::Conformance::NotYetVerified`], never a silent pass — the property
//! that makes a verifier worth trusting.
//!
//! ## References
//! - ASME Y14.5-2018, *Dimensioning and Tolerancing*.
//! - ISO 1101:2017, *Geometrical tolerancing*.

pub mod fit;
pub mod model;
pub mod sidecar;
pub mod verify;

pub use model::{
    Annotation, Datum, DatumKind, DatumRef, DimensionalTolerance, FeatureControlFrame, FitClass,
    GeometricCharacteristic, MaterialModifier, ToleranceBound,
};
pub use sidecar::GdtSidecar;
pub use verify::{Conformance, ConformanceResult};
