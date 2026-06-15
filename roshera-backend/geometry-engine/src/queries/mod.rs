//! Read-only geometric queries over a [`crate::primitives::topology_builder::BRepModel`].
//!
//! Queries never mutate the model — they interrogate it. The first inhabitant
//! is [`cd`], the contact-determination bridge that lifts a solid's boundary
//! into the polyhedral-cone algebra.

pub mod bvh;
pub mod cd;
pub mod erosion;
pub mod features;
pub mod kinematics;
pub mod lmd;
pub mod newton;
pub mod raycast;
pub mod trim;

pub use raycast::{raycast_solid, RayHit};
