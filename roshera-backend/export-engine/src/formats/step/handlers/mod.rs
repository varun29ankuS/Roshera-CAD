//! Entity handler registrations.
//!
//! Each tier of STEP coverage lands as a submodule here, plus a call
//! from [`register_all`] to bind its handlers into the dispatch table.
//!
//! - **`tier1`** (IMP2): planar + cylindrical solids. Covers ~60–70%
//!   of mechanical-CAD STEP files (brackets, plates, blocks, simple
//!   housings). Includes unit handlers, geometry handlers
//!   (CARTESIAN_POINT through CYLINDRICAL_SURFACE), topology
//!   handlers (EDGE_CURVE through MANIFOLD_SOLID_BREP), and root
//!   handlers (SHAPE_REPRESENTATION, ADVANCED_BREP_SHAPE_REPRESENTATION).
//! - **`tier2`** (IMP3, pending): NURBS curves and surfaces.
//! - **`tier3`** (IMP4, pending): voids, open shells, assemblies.
//!
//! See `plans/step-import-universal.md` for the coverage roadmap.

use crate::formats::step::dispatch::EntityDispatch;

pub mod tier1;
pub mod tier2;

/// Register every available entity handler into `dispatch`.
///
/// Called once per import, before [`EntityDispatch::run_all`].
/// Coverage grows by adding new tierN submodules and chaining their
/// `register` calls here.
pub fn register_all(dispatch: &mut EntityDispatch) {
    tier1::register(dispatch);
    tier2::register(dispatch);
    // IMP4: tier3::register(dispatch);
}
