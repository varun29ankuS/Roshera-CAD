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
pub mod point;
pub mod raycast;
pub mod raytrace;
pub mod trim;

pub use point::{classify_point, nearest_on_solid, PointClass};
pub use raycast::{raycast_all, raycast_solid, RayHit};
pub use raytrace::{raytrace_ortho, RaytraceFrame};
