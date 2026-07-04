//! Read-only geometric queries over a [`crate::primitives::topology_builder::BRepModel`].
//!
//! Queries never mutate the model — they interrogate it. The first inhabitant
//! is [`cd`], the contact-determination bridge that lifts a solid's boundary
//! into the polyhedral-cone algebra.

pub mod bvh;
pub mod cd;
pub mod erosion;
pub mod features;
pub mod field;
pub mod kinematics;
pub mod lmd;
pub mod measure;
pub mod newton;
pub mod occupancy;
pub mod point;
pub mod raycast;
pub mod raytrace;
pub mod region;
pub mod relational;
pub mod select;
pub mod trim;

pub use field::{sample_field_adaptive, signed_distance, AdaptiveField, FieldCell};
pub use measure::{measure, MeasureError, MeasureResult, MeasureSubject};
pub use occupancy::{occupancy_grid, to_slice_stack, OccupancyGrid};
pub use point::{classify_point, nearest_on_solid, PointClass};
pub use raycast::{raycast_all, raycast_solid, RayHit};
pub use raytrace::{raytrace_ortho, RaytraceFrame};
pub use region::{face_world_box, faces_in_box, faces_in_sphere, WorldBox};
pub use relational::{
    are_coaxial, are_parallel, are_perpendicular, axis_relation, coaxial_clusters, face_axis,
    AxisKind, AxisRelation, FaceAxis,
};
