//! Slice A integration tests — multi-loop extrusion in
//! `create_fresh_extrusion`.
//!
//! These cover the failing-case workflow uncovered by Task #18 F:
//! extruding a 2D face that already carries inner loops (holes) should
//! produce a watertight solid whose bottom and top caps each have the
//! same outer-loop + inner-loop topology, with one ruled side face per
//! edge on every loop. The kernel previously walked only the outer loop;
//! Slice A wires the inner loops through the same shared-topology
//! pipeline. See `roshera-backend/geometry-engine/src/operations/extrude.rs`
//! for the production code.

// AUDIT-H13: Reason for `#![allow(clippy::expect_used)]` — test-only file.
// `expect(...)` on fixture/scaffolding code surfaces invariant violations
// with a clear message at the failure site, which is the desired failure
// mode in tests. The workspace `expect_used = "deny"` lint targets
// production panic-freedom; test scaffolding is exempt by design.
#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::{extrude_face, ExtrudeOptions, OperationError};
use geometry_engine::primitives::{
    curve::Line,
    edge::{Edge, EdgeOrientation, EdgeId},
    face::{Face, FaceOrientation},
    r#loop::{Loop, LoopType, LoopId},
    surface::Plane,
    topology_builder::BRepModel,
    vertex::VertexId,
};

/// Add a straight-line edge between two existing vertices.
fn add_line_edge(model: &mut BRepModel, v_start: VertexId, v_end: VertexId) -> EdgeId {
    let s = model.vertices.get(v_start).expect("start vertex");
    let e = model.vertices.get(v_end).expect("end vertex");
    let line = Line::new(Point3::from(s.position), Point3::from(e.position));
    let curve_id = model.curves.add(Box::new(line));
    let edge = Edge::new_auto_range(0, v_start, v_end, curve_id, EdgeOrientation::Forward);
    model.edges.add(edge)
}

/// Build a CCW rectangle loop in the XY plane with corners (x0,y0)-(x1,y1).
fn add_outer_rect_loop(model: &mut BRepModel, x0: f64, y0: f64, x1: f64, y1: f64) -> LoopId {
    let v0 = model.vertices.add(x0, y0, 0.0);
    let v1 = model.vertices.add(x1, y0, 0.0);
    let v2 = model.vertices.add(x1, y1, 0.0);
    let v3 = model.vertices.add(x0, y1, 0.0);
    let e0 = add_line_edge(model, v0, v1);
    let e1 = add_line_edge(model, v1, v2);
    let e2 = add_line_edge(model, v2, v3);
    let e3 = add_line_edge(model, v3, v0);
    let mut l = Loop::new(0, LoopType::Outer);
    l.add_edge(e0, true);
    l.add_edge(e1, true);
    l.add_edge(e2, true);
    l.add_edge(e3, true);
    model.loops.add(l)
}

/// Build a CW (hole-winding) rectangle loop in the XY plane with corners
/// (x0,y0)-(x1,y1). Walks (x0,y0) → (x0,y1) → (x1,y1) → (x1,y0) → (x0,y0),
/// which is the inverse of the outer rectangle's CCW walk and gives the
/// side-face outward normals the correct "into the hole" direction.
fn add_inner_rect_loop(model: &mut BRepModel, x0: f64, y0: f64, x1: f64, y1: f64) -> LoopId {
    let v0 = model.vertices.add(x0, y0, 0.0);
    let v1 = model.vertices.add(x0, y1, 0.0);
    let v2 = model.vertices.add(x1, y1, 0.0);
    let v3 = model.vertices.add(x1, y0, 0.0);
    let e0 = add_line_edge(model, v0, v1);
    let e1 = add_line_edge(model, v1, v2);
    let e2 = add_line_edge(model, v2, v3);
    let e3 = add_line_edge(model, v3, v0);
    let mut l = Loop::new(0, LoopType::Inner);
    l.add_edge(e0, true);
    l.add_edge(e1, true);
    l.add_edge(e2, true);
    l.add_edge(e3, true);
    model.loops.add(l)
}

/// Build a planar XY face with the given outer + inner loops.
fn build_xy_face(model: &mut BRepModel, outer: LoopId, inners: &[LoopId]) -> u32 {
    let plane = Plane::from_point_normal(Point3::ZERO, Vector3::Z).expect("XY plane");
    let surface_id = model.surfaces.add(Box::new(plane));
    let mut face = Face::new(0, surface_id, outer, FaceOrientation::Forward);
    for &inner in inners {
        face.add_inner_loop(inner);
    }
    model.faces.add(face)
}

// ---------------------------------------------------------------------
// Happy paths
// ---------------------------------------------------------------------

#[test]
fn extrudes_rectangle_with_one_square_hole() {
    let mut model = BRepModel::new();
    // Outer 10×10 rectangle, inner 4×4 square at the center.
    let outer = add_outer_rect_loop(&mut model, 0.0, 0.0, 10.0, 10.0);
    let inner = add_inner_rect_loop(&mut model, 3.0, 3.0, 7.0, 7.0);
    let face_id = build_xy_face(&mut model, outer, &[inner]);

    let opts = ExtrudeOptions {
        distance: 5.0,
        common: geometry_engine::operations::CommonOptions {
            validate_result: false,
            ..Default::default()
        },
        ..Default::default()
    };

    let solid_id = extrude_face(&mut model, face_id, opts).expect("multi-loop extrude succeeds");
    let solid = model.solids.get(solid_id).expect("solid stored");
    let shell = model.shells.get(solid.outer_shell).expect("shell stored");

    // 4 outer-side walls + 4 inner-side walls + bottom cap + top cap = 10 faces.
    assert_eq!(
        shell.faces.len(),
        10,
        "rectangle-with-square-hole extrusion should produce 10 faces"
    );

    // Top cap must carry the inner loop too — same topology as the bottom.
    let top_face_id = *shell.faces.last().expect("top cap pushed last");
    let top_face = model.faces.get(top_face_id).expect("top face stored");
    assert_eq!(
        top_face.inner_loops.len(),
        1,
        "top cap must have one inner loop matching the base"
    );
}

#[test]
fn extrudes_face_with_two_holes() {
    let mut model = BRepModel::new();
    let outer = add_outer_rect_loop(&mut model, 0.0, 0.0, 20.0, 10.0);
    let hole_a = add_inner_rect_loop(&mut model, 2.0, 2.0, 4.0, 8.0);
    let hole_b = add_inner_rect_loop(&mut model, 12.0, 2.0, 14.0, 8.0);
    let face_id = build_xy_face(&mut model, outer, &[hole_a, hole_b]);

    let opts = ExtrudeOptions {
        distance: 3.0,
        common: geometry_engine::operations::CommonOptions {
            validate_result: false,
            ..Default::default()
        },
        ..Default::default()
    };

    let solid_id = extrude_face(&mut model, face_id, opts).expect("two-hole extrude succeeds");
    let solid = model.solids.get(solid_id).expect("solid stored");
    let shell = model.shells.get(solid.outer_shell).expect("shell stored");

    // 4 outer + 4 + 4 inner walls + bottom + top = 14 faces.
    assert_eq!(
        shell.faces.len(),
        14,
        "rectangle-with-two-holes extrusion should produce 14 faces"
    );

    let top_face = model
        .faces
        .get(*shell.faces.last().expect("top cap"))
        .expect("top face stored");
    assert_eq!(top_face.inner_loops.len(), 2, "top cap has two inner loops");
}

#[test]
fn extrudes_face_without_holes_unchanged_behavior() {
    // Regression: a face with no inner_loops must still produce a six-face
    // box (4 sides + bottom + top), exactly like before Slice A.
    let mut model = BRepModel::new();
    let outer = add_outer_rect_loop(&mut model, 0.0, 0.0, 4.0, 3.0);
    let face_id = build_xy_face(&mut model, outer, &[]);

    let opts = ExtrudeOptions {
        distance: 2.0,
        common: geometry_engine::operations::CommonOptions {
            validate_result: false,
            ..Default::default()
        },
        ..Default::default()
    };

    let solid_id = extrude_face(&mut model, face_id, opts).expect("plain extrude succeeds");
    let solid = model.solids.get(solid_id).expect("solid stored");
    let shell = model.shells.get(solid.outer_shell).expect("shell stored");
    assert_eq!(shell.faces.len(), 6, "plain rectangle extrudes to 6 faces");

    let top_face = model
        .faces
        .get(*shell.faces.last().expect("top cap"))
        .expect("top face stored");
    assert!(top_face.inner_loops.is_empty(), "no holes → no inner loops");
}

// ---------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------

// ---------------------------------------------------------------------
// Slice C — coplanar disjoint extrusions must Union successfully
// ---------------------------------------------------------------------

#[test]
fn coincident_planes_disjoint_extrusions_union_succeeds() {
    // Two disjoint rectangles on the same XY sketch plane. Pre-Slice C
    // their extrusions shared bottom and top sketch planes, so the
    // boolean pipeline's `plane_plane_intersection` short-circuited
    // with `OperationError::CoplanarFaces` and killed the Union. After
    // Slice C, the face-overlap check sees the AABBs are disjoint and
    // lets the intersection pass return an empty curve set — Union
    // proceeds normally.
    let mut model = BRepModel::new();

    let outer_a = add_outer_rect_loop(&mut model, 0.0, 0.0, 2.0, 2.0);
    let face_a = build_xy_face(&mut model, outer_a, &[]);

    let outer_b = add_outer_rect_loop(&mut model, 10.0, 10.0, 12.0, 12.0);
    let face_b = build_xy_face(&mut model, outer_b, &[]);

    let extrude_opts = ExtrudeOptions {
        distance: 5.0,
        common: geometry_engine::operations::CommonOptions {
            validate_result: false,
            ..Default::default()
        },
        ..Default::default()
    };

    let solid_a = extrude_face(&mut model, face_a, extrude_opts.clone())
        .expect("extrude rect A succeeds");
    let solid_b =
        extrude_face(&mut model, face_b, extrude_opts).expect("extrude rect B succeeds");

    let result = boolean_operation(
        &mut model,
        solid_a,
        solid_b,
        BooleanOp::Union,
        BooleanOptions::default(),
    );

    assert!(
        result.is_ok(),
        "Union of two disjoint coplanar extrusions must succeed after Slice C, \
         got error: {:?}",
        result.err(),
    );
}

#[test]
fn rejects_inner_loop_outside_outer() {
    let mut model = BRepModel::new();
    let outer = add_outer_rect_loop(&mut model, 0.0, 0.0, 10.0, 10.0);
    // Inner placed entirely outside the outer rectangle.
    let bad_inner = add_inner_rect_loop(&mut model, 20.0, 20.0, 25.0, 25.0);
    let face_id = build_xy_face(&mut model, outer, &[bad_inner]);

    let opts = ExtrudeOptions {
        distance: 5.0,
        common: geometry_engine::operations::CommonOptions {
            validate_result: false,
            ..Default::default()
        },
        ..Default::default()
    };

    let result = extrude_face(&mut model, face_id, opts);
    match result {
        Err(OperationError::InvalidGeometry(msg)) => {
            assert!(
                msg.contains("not inside the outer loop"),
                "expected outer-containment error, got `{msg}`"
            );
        }
        other => panic!("expected InvalidGeometry rejection, got {other:?}"),
    }
}
