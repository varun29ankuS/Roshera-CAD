//! Read-only geometric queries over a [`crate::primitives::topology_builder::BRepModel`].
//!
//! Queries never mutate the model — they interrogate it. The first inhabitant
//! is [`cd`], the contact-determination bridge that lifts a solid's boundary
//! into the polyhedral-cone algebra.

pub mod cd;
pub mod lmd;
