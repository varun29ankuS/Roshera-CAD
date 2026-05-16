//! Tier-1 STEP entity coverage — planar and cylindrical solids.
//!
//! Tier-1 is the entity set you need to import roughly 60–70% of
//! real mechanical CAD STEP files: brackets, plates, blocks, simple
//! housings. Every entity here is either a pure-geometry primitive
//! (point, direction, axis frame, line, circle, plane, cylinder),
//! a topology element (vertex, edge, loop, face, shell, solid), or
//! a root container (`SHAPE_REPRESENTATION`,
//! `ADVANCED_BREP_SHAPE_REPRESENTATION`).
//!
//! The submodule layout matches the architectural split:
//!
//! - [`params`] — type-safe extractors from `ruststep::ast::Parameter`
//!   with source-location error context (entity name + `#N` + parameter
//!   path). No `.unwrap()` on parameter matches anywhere in the
//!   handlers.
//! - [`resolver`] — lazy cross-phase reference resolution. When a
//!   handler references an entity that hasn't been dispatched yet
//!   (the registry is HashMap-backed and walk order is not source
//!   order), the resolver dispatches that entity *now* and writes
//!   its result into [`super::super::context::ResolutionCaches`].
//!   Cycle-detected via [`super::super::context::ImportContext::resolution_stack`].
//! - [`healing`] — vertex-snap, loop-closure check, axis-frame
//!   degeneracy repair. Each heal emits a structured
//!   [`super::super::diagnostics::Healing`] event.
//! - [`manifold`] — closed-shell validation. Edge-use counting
//!   detects non-manifold edges, dangling edges, and orientation
//!   mismatches; results surfaced as
//!   [`super::super::diagnostics::ManifoldWarning`].
//!
//! IMP2.1 lands these four foundation modules with no entity handlers
//! registered. IMP2.2 onwards adds the handlers themselves.

use crate::formats::step::dispatch::EntityDispatch;

pub mod geometry;
pub mod healing;
pub mod manifold;
pub mod params;
pub mod resolver;
pub mod root;
pub mod topology;
pub mod units;

/// Register every tier-1 entity handler into `dispatch`.
///
/// IMP2.2: unit-phase handlers (SI / conversion-based unit
/// declarations + `GLOBAL_UNIT_ASSIGNED_CONTEXT` /
/// `GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT`).
///
/// IMP2.3: geometry-phase handlers (CARTESIAN_POINT, DIRECTION,
/// VECTOR, AXIS2_PLACEMENT_3D, LINE, CIRCLE, PLANE,
/// CYLINDRICAL_SURFACE, VERTEX_POINT).
///
/// IMP2.4: topology-phase handlers (EDGE_CURVE, ORIENTED_EDGE,
/// EDGE_LOOP, FACE_BOUND, FACE_OUTER_BOUND, ADVANCED_FACE,
/// CLOSED_SHELL, MANIFOLD_SOLID_BREP).
///
/// IMP2.5: root-phase handlers (`SHAPE_REPRESENTATION`,
/// `ADVANCED_BREP_SHAPE_REPRESENTATION`). These populate
/// `ctx.caches.roots`, which drives the `ImportReport::ok` flag
/// surfaced from the importer entry point.
pub fn register(dispatch: &mut EntityDispatch) {
    units::register(dispatch);
    geometry::register(dispatch);
    topology::register(dispatch);
    root::register(dispatch);
}
