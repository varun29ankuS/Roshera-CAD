//! Tier-2 STEP entity coverage — NURBS curves and surfaces, plus the
//! remaining analytic surface family (`SPHERICAL_SURFACE`,
//! `TOROIDAL_SURFACE`, `CONICAL_SURFACE`).
//!
//! Tier-2 lifts the importer from "60–70% of mechanical CAD files"
//! (planar + cylindrical, covered by tier-1) toward parts whose faces
//! carry free-form geometry: blended fillets, lofted surfaces, swept
//! profiles. The handlers here all live in `Phase::Geometry` —
//! topology in tier-1 consumes their kernel ids by the same
//! `caches.curves` / `caches.surfaces` resolve paths that already
//! work for `LINE` / `CIRCLE` / `PLANE`.
//!
//! Submodule layout:
//!
//! - [`bspline`] — `B_SPLINE_CURVE_WITH_KNOTS` and
//!   `B_SPLINE_SURFACE_WITH_KNOTS` in their non-rational simple
//!   forms. Knot vectors are expanded from `(knots, multiplicities)`
//!   into the flat form the kernel expects, then handed to
//!   `primitives::curve::NurbsCurve::from_bspline` /
//!   `math::nurbs::NurbsSurface::new` (wrapped in
//!   `GeneralNurbsSurface`).
//! - [`analytic`] — `SPHERICAL_SURFACE`, `TOROIDAL_SURFACE`,
//!   `CONICAL_SURFACE`. Same handler shape as tier-1's `PLANE` /
//!   `CYLINDRICAL_SURFACE`: resolve placement + length-scale the
//!   radius / angle parameters, call the kernel constructor, cache
//!   the resulting `SurfaceId`.
//!
//! ## Scope of this slice (IMP3.1–3.3)
//!
//! - Non-rational simple-form B-spline curves and surfaces only.
//!   The rational variants (`RATIONAL_B_SPLINE_CURVE` /
//!   `RATIONAL_B_SPLINE_SURFACE`) arrive as STEP complex entities
//!   (a `&SCOPE`-style record list whose constituents include
//!   `BOUNDED_CURVE` / `B_SPLINE_CURVE` / `B_SPLINE_CURVE_WITH_KNOTS`
//!   / `RATIONAL_B_SPLINE_CURVE`); they are routed through the
//!   dispatcher's `Complex` path but are deferred to a follow-up
//!   slice. Until then they surface as `Unsupported` exactly as they
//!   did before tier-2 landed.
//! - `SURFACE_OF_REVOLUTION` and `SURFACE_OF_LINEAR_EXTRUSION` are
//!   tier-2 by ISO 10303-42 but require profile-curve resolution
//!   into a kernel sweep surface that does not yet exist in
//!   `primitives::surface`; they are deferred.

use crate::formats::step::dispatch::EntityDispatch;

pub mod analytic;
pub mod bspline;
pub mod complex;
pub mod swept;

/// Register every tier-2 entity handler into `dispatch`.
pub fn register(dispatch: &mut EntityDispatch) {
    bspline::register(dispatch);
    analytic::register(dispatch);
    swept::register(dispatch);
    // `complex` has no name-keyed handler: complex (sub-super) instances
    // are routed directly by the dispatcher / lazy resolver via
    // `complex::try_build_complex`, not through the name table.
}
