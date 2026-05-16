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

pub mod projection;
pub mod svg;
pub mod types;

pub use projection::{
    project_solid_edges, project_solid_view, view_matrix_for_projection, ProjectionError,
};
pub use svg::render_drawing_svg;
pub use types::{
    Drawing, DrawingId, Polyline2d, ProjectedView, ProjectedViewId, ProjectionType, SheetSize,
    ViewExtent,
};

#[cfg(test)]
mod tests;
