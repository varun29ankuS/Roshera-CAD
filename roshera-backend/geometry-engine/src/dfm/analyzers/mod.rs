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
//! ## Slice S4
//!
//! [`bore::bore_metrics`] — per-bore diameter, trimmed axial depth,
//! through-vs-blind, and aspect ratio, reusing
//! [`crate::readable::bore_face_ids`] verbatim as its concave-cylinder
//! candidate filter (spec §3.1). Unlike every other S2/S3 analyzer, its
//! contract is `(model, solid_id)` rather than `faces: &[FaceId]` + bare
//! stores — see `bore.rs`'s module docs for why (through-vs-blind needs
//! the SOLID's own extent along the bore axis, not just one face's trim).
//!
//! ## Slice S5 (this addition)
//!
//! [`blend_radius::blend_radius`] — internal (concave) corner radii:
//! toroidal blend minor radius, cylindrical fillet radius on a concave
//! edge, and explicit sharp-edge (radius 0) detection. Concavity reuses the
//! kernel's own `face.orientation == FaceOrientation::Backward` convention
//! (`readable::bore_face_ids`, `thickness.rs`'s `Classified::Cylinder`
//! doc); a genuine blend is additionally gated on
//! [`crate::operations::edge_classification::classify_edge`]'s
//! `DihedralClass::G1Smooth` (tangency to a neighbour), which is what
//! distinguishes a fillet from a bore/pocket wall — see `blend_radius.rs`'s
//! module docs for the full derivation (including the Torus extension,
//! hand-verified from `Torus::evaluate_full`).
//!
//! [`internal_voids::internal_voids`] — fully-enclosed cavities from
//! `Solid.inner_shells`, with enclosure PROVEN from a derived per-edge
//! two-face-use count over each shell's own topology (never assumed from
//! the caller-supplied `ShellType::Closed` label) — see
//! `internal_voids.rs`'s module docs. Contract is `(model, solid_id)`,
//! matching `bore_metrics`'s precedent for a solid-scoped analyzer.

pub mod blend_radius;
pub mod bore;
pub mod internal_voids;
pub mod orientation;
pub mod thickness;

pub use blend_radius::{
    blend_radius, BlendRadiusOutcome, BlendRecord, SharpCorner, UnverifiableBlend,
};
pub use bore::{bore_metrics, BoreMetricsOutcome, BoreRecord, UnverifiableBore};
pub use internal_voids::{internal_voids, InternalVoidsOutcome, UnverifiableVoid, VoidRecord};
pub use orientation::{face_orientation_field, OrientationOutcome};
pub use thickness::{pair_thickness, FacePair, PairThicknessOutcome, UnpairedRegion};
