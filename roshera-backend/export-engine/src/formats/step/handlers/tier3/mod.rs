//! Tier-3 STEP coverage: voids, open shells, and assembly instancing.
//!
//! Tier-1/2 cover a single watertight `MANIFOLD_SOLID_BREP` per root.
//! Real-world parts add:
//!
//! | STEP                          | Effect                                                                 |
//! |-------------------------------|------------------------------------------------------------------------|
//! | `OPEN_SHELL`                  | Allocates a kernel `Shell` (`ShellType::Open`) — non-volume-bounding.  |
//! | `BREP_WITH_VOIDS`             | A solid with internal cavities: outer closed shell + void shells.      |
//! | `MANIFOLD_SURFACE_SHELL_..`   | Surface model: each shell materialised; no solid (open bodies).        |
//! | `MAPPED_ITEM`                 | Assembly instance: the mapped representation's solids, re-placed.      |
//!
//! ## BREP_WITH_VOIDS
//!
//! `BREP_WITH_VOIDS('label', #outer_closed_shell, (#void_shell, …))`.
//! The outer shell bounds the material; each void shell is an internal
//! cavity (a hollow). The kernel `Solid` models this directly:
//! `Solid::new(0, outer)` then [`Solid::add_inner_shell`] per void. STEP
//! orients void shells so their face normals point *into* the void (away
//! from material); the kernel's inner-shell list carries them as-is.
//!
//! ## MAPPED_ITEM (assembly instancing)
//!
//! `MAPPED_ITEM('label', #representation_map, #target_placement)` where
//! `REPRESENTATION_MAP(#mapping_origin, #mapped_representation)`. We
//! resolve the mapped representation's solids and apply the rigid
//! placement transform (`mapping_origin → target`) so each instance
//! lands in world space. A single mapped representation referenced by N
//! mapped items yields N independent transformed solids — true
//! multi-body assembly instancing. When the transform cannot be derived
//! (non-`AXIS2_PLACEMENT_3D` operators), the untransformed solids are
//! still surfaced and the gap is logged honestly.

use crate::formats::step::dispatch::EntityDispatch;

pub mod assembly;
pub mod shells;
pub mod solid;

/// Register every tier-3 entity handler into `dispatch`.
pub fn register(dispatch: &mut EntityDispatch) {
    shells::register(dispatch);
    solid::register(dispatch);
    assembly::register(dispatch);
}
