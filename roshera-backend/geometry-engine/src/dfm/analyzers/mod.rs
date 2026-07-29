//! Process-agnostic, analytic-or-refuse geometry analyzers (spec §3.1).
//!
//! Each analyzer answers ONE geometric question about a face or solid in
//! closed form on the surface kinds it supports, and refuses (a typed
//! [`crate::dfm::report::Verdict::Unverifiable`], never a bare error or an
//! approximation) everywhere else. Rule packs (`crate::dfm::packs`) are
//! the ONLY consumers — an analyzer never knows which manufacturing
//! process is asking.
//!
//! ## Slice S2 (this module)
//!
//! [`orientation::face_orientation_field`] — per-face angle range against a
//! caller-supplied reference direction, exact on `Plane`/`Cylinder`/`Cone`,
//! exact-if-untrimmed on `Sphere`, and an honest refusal on `Torus` and
//! every non-analytic surface kind (spec §3.1's support table). The
//! remaining four analyzers (`pair_thickness`, `blend_radius`,
//! `internal_voids`, `bore_metrics`) land in S3–S5.

pub mod orientation;

pub use orientation::{face_orientation_field, OrientationOutcome};
