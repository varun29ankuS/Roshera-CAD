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

use geometry_engine::math::{Matrix4, Point3, Tolerance, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::transform::{transform_solid, TransformOptions};
use geometry_engine::primitives::curve::{Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
use geometry_engine::primitives::face::{Face, FaceId, FaceOrientation};
use geometry_engine::primitives::r#loop::{Loop, LoopType};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::Cylinder;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::validation::{
    validate_faces_scoped, validate_model_enhanced, ValidationLevel,
};

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

/// #29/#39 GUARD: scoped validation must IGNORE defects on solids the op
/// did not touch. A valid box A and an open (invalid) box B coexist;
/// validating scoped to A's faces passes even though whole-model
/// validation fails on B. This is exactly what lets a blend / pattern /
/// op on A succeed despite an unrelated broken solid elsewhere in the
/// model (without it, every op fails as soon as any solid is malformed).
#[test]
fn scoped_validation_ignores_unrelated_invalid_solid() {
    let mut model = BRepModel::new();
    let a = box_solid(&mut model, 10.0, 10.0, 10.0);
    let b = box_solid(&mut model, 4.0, 4.0, 4.0);

    // Break B: remove one face → open shell, invalid.
    let b_shell = model.solids.get(b).expect("b in store").outer_shell;
    let bad_face = *model
        .shells
        .get(b_shell)
        .expect("b shell")
        .faces
        .first()
        .expect("b has faces");
    model
        .shells
        .get_mut(b_shell)
        .expect("b shell")
        .remove_face(bad_face);
    assert!(
        !is_valid(&model),
        "precondition: whole-model validation must see B's defect"
    );

    // Scoped to A's faces, B's defect must be invisible.
    let a_shell = model.solids.get(a).expect("a in store").outer_shell;
    let a_faces: Vec<FaceId> = model.shells.get(a_shell).expect("a shell").faces.clone();
    let scoped = validate_faces_scoped(
        &model,
        &a_faces,
        Tolerance::default(),
        ValidationLevel::Standard,
    );
    assert!(
        scoped.is_valid,
        "validation scoped to A's faces must ignore the unrelated defect on B; \
         errors: {:?}",
        scoped.errors
    );
}

/// MUTATION-PROOF GUARD for the 1c torus-commutator exemption (Task 1B).
///
/// The torus-commutator fix exempts a single-vertex closed loop ONLY when
/// the surface is doubly-closed (is_closed_u AND is_closed_v). This test
/// pins the narrowness of that gate: a degenerate zero-span loop on a
/// CYLINDER (closed in u, NOT in v) must still be flagged by check-1c.
///
/// Mutation transcript (narrowness proof):
///   Widen the exemption → remove the `is_closed_v()` requirement so that
///   ANY single-vertex loop on a closed-in-u surface escapes.
///   Under that widened predicate this test goes RED because the cylinder
///   face is not flagged. Restore the `is_closed_v()` gate → GREEN.
///
/// This confirms the exemption cannot silently absorb genuine degenerate
/// faces on singly-closed or open surfaces.
#[test]
fn check_1c_still_flags_degenerate_loop_on_singly_closed_surface() {
    // Build a minimal model: one cylinder surface (is_closed_u=true,
    // is_closed_v=false) with a face whose outer loop has three edges all
    // sharing the same single vertex — zero spatial span, genuine defect.
    let mut model = BRepModel::new();

    // Cylinder: origin=origin, axis=Z, radius=5, no height limits.
    let cyl = Cylinder::new(Point3::ORIGIN, Vector3::Z, 5.0).expect("cylinder surface");
    let surf_id = model.surfaces.add(Box::new(cyl));

    // Single seam vertex at origin.
    let vid = model.vertices.add(0.0, 0.0, 0.0);

    // Degenerate point-curve: start == end, zero length.
    let degenerate_line = Line::new(Point3::ORIGIN, Point3::ORIGIN);

    // Three degenerate edges, all start_vertex = end_vertex = vid.
    let curve_a = model.curves.add(Box::new(degenerate_line.clone()));
    let edge_a = model.edges.add(Edge::new(
        0,
        vid,
        vid,
        curve_a,
        EdgeOrientation::Forward,
        ParameterRange::unit(),
    ));

    let curve_b = model.curves.add(Box::new(degenerate_line.clone()));
    let edge_b = model.edges.add(Edge::new(
        0,
        vid,
        vid,
        curve_b,
        EdgeOrientation::Forward,
        ParameterRange::unit(),
    ));

    let curve_c = model.curves.add(Box::new(degenerate_line));
    let edge_c = model.edges.add(Edge::new(
        0,
        vid,
        vid,
        curve_c,
        EdgeOrientation::Forward,
        ParameterRange::unit(),
    ));

    // Outer loop with those three edges.
    let mut outer = Loop::new(0, LoopType::Outer);
    outer.add_edge(edge_a, true);
    outer.add_edge(edge_b, true);
    outer.add_edge(edge_c, true);
    let loop_id = model.loops.add(outer);

    // Face on the cylinder surface.
    model
        .faces
        .add(Face::new(0, surf_id, loop_id, FaceOrientation::Forward));

    // check-1c must fire: the loop has one distinct vertex (all at origin),
    // span = 0, but the cylinder is NOT doubly-closed, so the torus-commutator
    // exemption does NOT apply.
    let result = validate_model_enhanced(&model, Tolerance::default(), ValidationLevel::Standard);
    let has_degenerate_error = result.errors.iter().any(|e| {
        matches!(e,
            geometry_engine::primitives::validation::ValidationError::GeometryError { message, .. }
            if message.contains("degenerate")
        )
    });
    assert!(
        has_degenerate_error,
        "check-1c must flag a zero-span loop on a cylinder (singly-closed surface); \
         errors: {:?}",
        result.errors
    );
}
