//! Meta-harness: pins that the B-Rep validator itself is CORRECT.
//!
//! A validation oracle is only useful if it both ACCEPTS topologically
//! valid solids and REJECTS genuinely broken ones. KNOWN_BUGS #37 was a
//! WRONG oracle: `validate_euler_characteristic_for_solid` used the naive
//! `V - E + F = 2`, which only holds when every face is a disk — so it
//! falsely rejected every solid with a face-hole (a bore, a counterbore, a
//! box pierced by another box: the everyday output of booleans), which in
//! turn blocked downstream chamfer/fillet. Nothing tested the oracle
//! against known-valid solids, so the wrong check went unnoticed for a long
//! time (the union harness only checked MESH watertightness, never the
//! B-Rep Euler check).
//!
//! These tests gate the generalized Euler–Poincaré identity
//! `V - E + F - R = 2(S - G)` from the validator's side: face-hole solids
//! must validate, and a genuinely-open shell must not.

use geometry_engine::math::{Matrix4, Tolerance, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::transform::{transform_solid, TransformOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::validation::{validate_model_enhanced, ValidationLevel};

fn box_solid(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    match TopologyBuilder::new(model)
        .create_box_3d(w, h, d)
        .expect("box creation succeeds")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn is_valid(model: &BRepModel) -> bool {
    validate_model_enhanced(model, Tolerance::default(), ValidationLevel::Standard).is_valid
}

/// A plain box is the trivial valid case — V-E+F = 2, R = 0.
#[test]
fn plain_box_validates() {
    let mut model = BRepModel::new();
    box_solid(&mut model, 10.0, 10.0, 10.0);
    assert!(is_valid(&model), "a plain box must validate");
}

/// #37 GUARD: a box pierced by another box (the second box pokes out
/// through one face → that face becomes an annulus, one inner loop) is a
/// genus-0 solid with R = 1, so `V - E + F = 3 ≠ 2`. The naive check
/// rejected it; the generalized identity accepts it. This is the exact
/// case that blocked chamfer-on-union before the fix.
#[test]
fn pierced_face_solid_validates() {
    let mut model = BRepModel::new();
    let a = box_solid(&mut model, 80.0, 80.0, 40.0); // z ∈ [-20, 20]
    let b = box_solid(&mut model, 30.0, 30.0, 60.0); // z ∈ [-30, 30]
    transform_solid(
        &mut model,
        b,
        Matrix4::from_translation(&Vector3::new(0.0, 0.0, 25.0)),
        TransformOptions::default(),
    )
    .expect("translate B to pierce A's top");
    boolean_operation(
        &mut model,
        a,
        b,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("pierce union must succeed");

    assert!(
        is_valid(&model),
        "a pierced-face solid (face with a hole, R=1) must validate under \
         Euler–Poincaré; the naive V-E+F=2 wrongly rejected it (#37)"
    );
}

/// The oracle must still REJECT a genuinely open shell. Removing one face
/// from a box leaves an open boundary: `V - E + F - R` becomes odd, which
/// is impossible for a closed orientable solid. If this ever passes, the
/// oracle has gone permissive and is no longer trustworthy.
#[test]
fn open_box_is_rejected() {
    let mut model = BRepModel::new();
    let solid = box_solid(&mut model, 10.0, 10.0, 10.0);
    assert!(is_valid(&model), "precondition: the intact box validates");

    let shell_id = model.solids.get(solid).expect("solid in store").outer_shell;
    let face_to_remove = *model
        .shells
        .get(shell_id)
        .expect("shell in store")
        .faces
        .first()
        .expect("box shell has faces");
    let removed = model
        .shells
        .get_mut(shell_id)
        .expect("shell in store")
        .remove_face(face_to_remove);
    assert!(removed, "face removal must succeed");

    assert!(
        !is_valid(&model),
        "a box with a face removed is an open shell and must FAIL validation \
         — the oracle must not accept genuine defects"
    );
}
