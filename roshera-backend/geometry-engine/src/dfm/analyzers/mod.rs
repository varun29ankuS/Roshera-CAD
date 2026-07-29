//! Process-agnostic, analytic-or-refuse geometry analyzers (spec §3.1).
//!
//! Each analyzer answers ONE geometric question about a face or solid in
//! closed form on the surface kinds it supports, and refuses (a typed
//! [`crate::dfm::report::Verdict::Unverifiable`], never a bare error or an
//! approximation) everywhere else. Rule packs (`crate::dfm::packs`) are
//! the ONLY consumers — an analyzer never knows which manufacturing
//! process is asking.
//!
//! ## Slice S2
//!
//! [`orientation::face_orientation_field`] — per-face angle range against a
//! caller-supplied reference direction, exact on `Plane`/`Cylinder`/`Cone`,
//! exact-if-untrimmed on `Sphere`, and an honest refusal on `Torus` and
//! every non-analytic surface kind (spec §3.1's support table).
//!
//! ## Slice S3
//!
//! [`thickness::pair_thickness`] — wall thickness between provably
//! opposing face pairs (parallel planes / coaxial cylinders), exact-or-
//! refuse over each pair's TRIMMED domain, never a nearest-face guess
//! (spec §3.1).
//!
//! ## Slice S4 (this addition)
//!
//! [`bore::bore_metrics`] — per-bore diameter, trimmed axial depth,
//! through-vs-blind, and aspect ratio, reusing
//! [`crate::readable::bore_face_ids`] verbatim as its concave-cylinder
//! candidate filter (spec §3.1). Unlike every other analyzer here, its
//! contract is `(model, solid_id)` rather than `faces: &[FaceId]` + bare
//! stores — see `bore.rs`'s module docs for why (through-vs-blind needs
//! the SOLID's own extent along the bore axis, not just one face's trim).
//! The remaining two analyzers (`blend_radius`, `internal_voids`) land in
//! S5.

pub mod bore;
pub mod orientation;
pub mod thickness;

pub use bore::{bore_metrics, BoreMetricsOutcome, BoreRecord, UnverifiableBore};
pub use orientation::{face_orientation_field, OrientationOutcome};
pub use thickness::{pair_thickness, FacePair, PairThicknessOutcome, UnpairedRegion};
