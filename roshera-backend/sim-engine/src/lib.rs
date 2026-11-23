//! Roshera simulation.
//!
//! An agent (or an engineer) sets up a study on a certified part -- where it
//! is held, what loads it, what it is made of, what it must achieve -- and
//! this crate checks the set-up before anything is solved, meshes and solves
//! it through an engine, and returns a verdict with its evidence. Design:
//! `docs/research/2026-11-23-fea-cfd-requirements.md`.
//!
//! Supports and loads name faces by their PERSISTENT id, so a study survives
//! edits to the part; a face that no longer resolves is refused by name.
//!
//! Modules:
//! - [`rigid`]: can the part still move under its supports? Pure geometry,
//!   no solver: the rank of the supports' constraints against the six rigid
//!   motions, and the motions left free, in words.
//! - [`loads`]: a stated force on faces as consistent nodal forces on the
//!   quadratic mesh, evenly or as a pin bearing, summing to the stated force.

pub mod loads;
pub mod rigid;
