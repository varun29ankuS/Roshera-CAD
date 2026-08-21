// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! **A solid says what produced it.**
//!
//! The model tree cannot tell a part from its scaffolding, because every row
//! is a bare solid and no solid carries a kind. `Cylinder 93` (a cutter body
//! nobody consumed), `bore 1/4` (the result of a difference) and `flange` (a
//! part) render with an identical bullet, identical indent and identical
//! weight. That is not a styling problem — there is no field to style on.
//!
//! The cheapest honest kind is the one the kernel already knows without being
//! told: WHAT MADE THIS SOLID. A boolean consumes its operands, so anything
//! still alive that came straight from a primitive constructor was never
//! combined into anything — loose stock, a cutter that missed, an abandoned
//! attempt. Anything that came out of a boolean is a result someone built.
//!
//! This is deliberately NOT a caller-declared label. A `role: "tool"` argument
//! only works when an agent remembers to pass it, and a model that forgets is
//! indistinguishable from one that meant it. Deriving the role from what
//! actually happened makes the mess inexpressible rather than discouraged —
//! the house rule is `constraint-beats-steering`.
//!
//! Scope, stated so the next reader does not over-read these two tests: this
//! separates PRIMITIVE from DERIVED. It does not yet say "assembly", "part" or
//! "feature of a part" — those need the assembly hierarchy, which is a
//! separate and larger piece of work.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::{SolidId, SolidRole};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn cylinder(model: &mut BRepModel, base: Point3, axis: Vector3, r: f64, h: f64) -> SolidId {
    match TopologyBuilder::new(model).create_cylinder_3d(base, axis, r, h) {
        Ok(GeometryId::Solid(id)) => id,
        other => panic!("expected a solid; got {other:?}"),
    }
}

fn role_of(model: &BRepModel, id: SolidId) -> SolidRole {
    model
        .solids
        .get(id)
        .expect("the solid must still exist to have a role")
        .attributes
        .role
}

/// A solid straight from a constructor has been combined with nothing.
#[test]
fn a_freshly_built_primitive_is_marked_primitive() {
    let mut model = BRepModel::new();
    let blank = cylinder(
        &mut model,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
        20.0,
        40.0,
    );
    assert_eq!(
        role_of(&model, blank),
        SolidRole::Primitive,
        "a cylinder that has been through no boolean is loose stock, not a result"
    );
}

/// The result of a boolean was built out of something, and says so.
///
/// An AXIAL bore is used deliberately: the cross-bore case returns a
/// non-manifold solid (see `cross_bore_manifold.rs`), and a role test must not
/// be able to fail for a reason that has nothing to do with roles.
#[test]
fn the_result_of_a_boolean_is_marked_derived() {
    let mut model = BRepModel::new();
    let blank = cylinder(
        &mut model,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
        20.0,
        40.0,
    );
    let tool = cylinder(
        &mut model,
        Point3::new(0.0, 0.0, -5.0),
        Vector3::new(0.0, 0.0, 1.0),
        6.0,
        50.0,
    );
    assert_eq!(
        role_of(&model, blank),
        SolidRole::Primitive,
        "precondition: the blank starts out primitive"
    );

    let bored = boolean_operation(
        &mut model,
        blank,
        tool,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("an axial through-bore must succeed");

    assert_eq!(
        role_of(&model, bored),
        SolidRole::Derived,
        "a solid that came out of a boolean was built from operands, and the \
         tree needs to be able to say so without being told"
    );
}
