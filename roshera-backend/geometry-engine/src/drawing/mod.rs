//! Drawing module — 2D orthographic / isometric projections of solids.
//!
//! The drawing subsystem is the kernel-side foundation for 2D engineering
//! drawings: orthographic views (Front / Top / Right / Bottom / Left),
//! isometric views, and the sheet+layout types that group several
//! [`ProjectedView`]s onto a single [`Drawing`].
//!
//! Pipeline:
//! 1. Caller asks for a [`ProjectedView`] of a [`Solid`](crate::primitives::solid::Solid)
//!    using one of the [`ProjectionType`] variants.
//! 2. [`project_solid_edges`] walks every face → loop → edge of the
//!    solid's outer shell, samples each underlying 3D curve, and feeds
//!    the points through the projection matrix to produce a
//!    [`Polyline2d`] per edge.
//! 3. The resulting [`ProjectedView`] is positioned on a [`Drawing`]
//!    sheet at a 2D sheet-coordinate origin (millimetres).
//!
//! Sample density is driven by the existing
//! [`tessellation::TessellationParams`](crate::tessellation::TessellationParams)
//! so the visual fidelity of a drawing matches the viewport tessellation
//! quality the caller already configured.
//!
//! SVG rendering lives in [`svg`]; REST and frontend integration lives
//! in `api-server` and `roshera-app` respectively.

pub mod centerlines;
pub mod dimensioning;
pub mod dxf;
pub mod hole_table;
pub mod layout;
pub mod pdf;
pub mod projection;
pub mod section_comprehension;
pub mod section_view;
pub mod sheet_certificate;
pub mod svg;
pub mod types;
pub mod verify;
pub mod visibility;

pub use centerlines::{centerlines, Centerline};
pub use dimensioning::{
    auto_dimensions, section_slot_rule, standard_drawing, standard_drawing_auto,
    standard_drawing_hlr, visible_dimensions, CuttingPlaneLine, Dimension2d, SectionSlotRule,
};
pub use dxf::{render_drawing_dxf, DxfRenderError};
pub use hole_table::{build_hole_table, qualifies_for_baseline, tag_letter, HoleSite};
pub use pdf::{render_drawing_pdf, PdfRenderError};
pub use projection::{
    project_solid_edges, project_solid_view, view_matrix_for_projection, ProjectionError,
};
pub use section_comprehension::{
    section_cut_through, SectionCut, SectionCutKind, SectionCutThrough,
};
pub use section_view::section_view;
pub use sheet_certificate::{
    certify_drawing, LiveCheck, SheetFact, SheetFactKind, SheetReadbackCertificate, SheetVerdict,
    VerdictCounts,
};
pub use svg::render_drawing_svg;
pub use types::{
    Drawing, DrawingId, GeneralTolerance, PlacedDatumSymbol, PlacedFcfBlock, Polyline2d,
    ProjectedView, ProjectedViewId, ProjectionType, SectionSemantics, ShadedRaster, SheetSize,
    TitleBlock, ToleranceRef, ViewExtent, ViewSource,
};
pub use verify::{verify_drawing, DrawingIssue, DrawingIssueKind, DrawingQualityReport, Severity};
pub use visibility::{is_point_hidden, project_solid_edges_visibility, ViewEdges};

#[cfg(test)]
mod tests;
