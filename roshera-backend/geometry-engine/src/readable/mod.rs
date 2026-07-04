//! Agent-readable query surface (slice 6 — the differentiator).
//!
//! This module is THE pillar that makes Roshera an *agent runtime for
//! geometry* rather than a CAD tool with chat bolted on. Every type
//! here is purposed for an LLM (or another agent) reading a model:
//! datum-relative coordinates, named anchors, one-line summaries, and
//! distance breakdowns rather than raw world-space corner triplets.
//!
//! ## Architectural commitment
//! Agents address geometry by **part identity + anchor datum**, never by
//! world coordinates. Even the world-space bbox extents that flow through
//! `PartReport.dimensions_world` are accompanied by `location.anchor_datum_name`
//! and `location.center_in_anchor_frame` so an agent can ask "what is X
//! relative to" without an extra round-trip.
//!
//! Per Project Identity (decided 2026-04-29): *"geometry carries
//! semantic meaning AI can query, reason about, learn from. The
//! `readable/` module is THE differentiator."*
//!
//! ## What lives here
//! - [`PartReport`] — full agent-facing record for a single solid.
//! - [`PartSummary`] — light list-item for `list_parts`.
//! - [`PartProximity`] — solid + distance for `parts_near_datum`.
//! - [`DistanceReport`] — distance breakdown for `part_distance`.
//! - [`format_location_oneliner`] — produces the human-readable summary
//!   string that agents quote back to users.
//!
//! ## What does NOT live here
//! - The kernel methods themselves (`BRepModel::query_part`, etc.) live
//!   in [`query`] and are added directly as `impl BRepModel` blocks so
//!   they share the existing topology-builder data and cache.
//! - The AI tool routing layer lives in `ai-integration::tool_dispatch`
//!   and `ai-integration::executor`; this module is provider-agnostic.

pub mod claim;
pub mod dimensions;
pub mod features;
pub mod part;
pub mod query;

pub use dimensions::{bore_face_ids, extract_dimensions, DatumDescriptor, DimensionRecord};
pub use features::{
    cylindrical_diameters, distance, extract_features, point_to_plane_signed, FeatureDim,
};
pub use part::{
    format_datum_kind, format_datum_subkind, format_location_oneliner, DatumSummary,
    DistanceReport, EdgeReport, FaceReport, HoverReport, ListPartsFilter, MassPropertiesReport,
    MaterialSummary, OrientedBBox, PartProximity, PartReport, PartSummary, TopologyFingerprint,
};
