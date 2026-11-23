//! Roshera simulation.
//!
//! An agent (or an engineer) sets up a study on a certified part -- where it
//! is held, what loads it, what it is made of, what it must achieve -- and
//! this crate checks the set-up before anything is solved, meshes and solves
//! it through engines, and returns a verdict with its evidence. Design:
//! `docs/research/2026-11-23-fea-cfd-requirements.md`.
//!
//! Modules:
//! - [`study`]: the study a request carries.
//! - [`rigid`]: can the part still move under its supports? Pure geometry,
//!   no solver: the rank of the supports' constraints against the six rigid
//!   motions, and the motions left free, in words.
//! - [`loads`]: a stated force on faces as consistent nodal forces on the
//!   quadratic mesh, evenly or as a pin bearing, summing to the stated force.
//! - [`engines`]: the mesher (Gmsh, interim) and solver (CalculiX), found
//!   from configuration and run without a shell, under a time limit.
//! - [`mesh`]: quadratic tetrahedra from the part's exact geometry, every
//!   boundary triangle tied back to its kernel face.
//! - [`ccx`]: the solver deck and its results.
//! - [`run`]: the study end to end, and a verdict only the evidence supports.

pub mod ccx;
pub mod engines;
pub mod field;
pub mod loads;
pub mod mesh;
pub mod rigid;
pub mod run;
pub mod study;
