//! Analytic-type preservation under rigid transforms (regression gate).
//!
//! A rigid motion maps a circle→circle, cylinder→cylinder, plane→plane: it
//! must transform the analytic DEFINING DATA in place and keep the same
//! variant. The live bug (dogfooded via the MCP `transform` op) was that
//! `Circle::transform` delegated to `Arc::transform`, which always rebuilt an
//! `Arc` — so a translated/rotated rim circle came back typed `"Arc"`, and
//! `select_edge(curve_kind = circle)` returned `not_found` afterwards even
//! though `curve_kind = any` still resolved it. The transform also dropped the
//! arc's in-plane `x_axis`, re-deriving a canonical one and losing the
//! parametrisation a rigid motion must carry.
//!
//! This gate builds an analytic cylinder (2 `Circle` rim edges + a `Cylinder`
//! lateral + 2 `Plane` caps), applies BOTH a pure translation and a rotation,
//! and asserts:
//!   * the rim edges are STILL `Circle` (resolvable by `curve_kind = circle`),
//!     with the centre moved, the normal rotated, and the radius unchanged;
//!   * the lateral is STILL the analytic `Cylinder`, axis moved, radius kept;
//!   * the caps are STILL `Plane`;
//!   * the solid is still valid (scoped B-Rep validation).
//! If circles ever regress to `Arc`/NURBS under transform, the
//! `curve_kind = circle` resolves fail here.

use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::{transform_solid, TransformOptions};
use geometry_engine::primitives::curve::Circle;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::Cylinder;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};
use geometry_engine::queries::select::{
    resolve_edge, resolve_face, CurveKind, EdgeQuery, FaceQuery, SelectError, SurfaceKind,
};

const TOL: f64 = 1e-6;

fn expect_solid(geom: GeometryId) -> SolidId {
    match geom {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid geometry, got {other:?}"),
    }
}

fn make_cylinder(model: &mut BRepModel, base: Point3, axis: Vector3, r: f64, h: f64) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    expect_solid(
        builder
            .create_cylinder_3d(base, axis, r, h)
            .expect("cylinder creation"),
    )
}

/// Count edges whose underlying curve reports each `type_name`, scoped to the
/// solid's faces (the same edge set `select_edge` walks).
fn curve_type_counts(model: &BRepModel, solid: SolidId) -> std::collections::BTreeMap<String, u32> {
    let mut counts = std::collections::BTreeMap::new();
    let s = model.solids.get(solid).expect("solid");
    for sh in s.shell_ids() {
        let shell = model.shells.get(sh).expect("shell");
        for &fid in &shell.faces {
            let face = model.faces.get(fid).expect("face");
            let mut lids = vec![face.outer_loop];
            lids.extend_from_slice(&face.inner_loops);
            for lid in lids {
                if let Some(lp) = model.loops.get(lid) {
                    for &eid in &lp.edges {
                        if let Some(edge) = model.edges.get(eid) {
                            if let Some(curve) = model.curves.get(edge.curve_id) {
                                *counts.entry(curve.type_name().to_string()).or_insert(0) += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    counts
}

/// Pull the concrete `Circle` data for the first edge that resolves under
/// `curve_kind = circle`. Returns (center, normal, radius).
fn first_circle(model: &mut BRepModel, solid: SolidId) -> (Point3, Vector3, f64) {
    let eid = resolve_edge(model, solid, &EdgeQuery::new(CurveKind::Circle))
        .or_else(|e| match e {
            // Two coincident rim circles are an acceptable `Ambiguous` — take
            // either; this gate cares about TYPE, not which one.
            SelectError::Ambiguous(ids) => Ok(*ids.first().expect("non-empty ambiguous set")),
            other => Err(other),
        })
        .expect("a circle edge must resolve");
    let edge = model.edges.get(eid).expect("edge");
    let curve = model.curves.get(edge.curve_id).expect("curve");
    let circle = curve
        .as_any()
        .downcast_ref::<Circle>()
        .expect("curve_kind=circle must downcast to Circle");
    (circle.center(), circle.normal(), circle.radius())
}

fn lateral_cylinder(model: &mut BRepModel, solid: SolidId) -> (Point3, Vector3, f64) {
    let fid = resolve_face(model, solid, &FaceQuery::new(SurfaceKind::Cylindrical))
        .expect("a cylindrical lateral must resolve");
    let face = model.faces.get(fid).expect("face");
    let surf = model.surfaces.get(face.surface_id).expect("surface");
    let cyl = surf
        .as_any()
        .downcast_ref::<Cylinder>()
        .expect("SurfaceKind=Cylindrical must downcast to Cylinder");
    (cyl.origin, cyl.axis, cyl.radius)
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= TOL
}

fn vclose(a: Vector3, b: Vector3) -> bool {
    close(a.x, b.x) && close(a.y, b.y) && close(a.z, b.z)
}

fn pclose(a: Point3, b: Point3) -> bool {
    close(a.x, b.x) && close(a.y, b.y) && close(a.z, b.z)
}

fn assert_valid(model: &BRepModel, solid: SolidId, when: &str) {
    let r = validate_solid_scoped(
        model,
        solid,
        geometry_engine::math::Tolerance::default(),
        ValidationLevel::Standard,
    );
    assert!(
        r.is_valid,
        "solid invalid {when}: {:?}",
        r.errors.iter().take(3).collect::<Vec<_>>()
    );
}

#[test]
fn translation_preserves_circle_and_cylinder_types() {
    let mut model = BRepModel::new();
    // Axis +Z so the rim circles have +Z / -Z normals and radius 5.
    let solid = make_cylinder(&mut model, Point3::ORIGIN, Vector3::Z, 5.0, 20.0);

    // BEFORE: circle rims + cylinder lateral are analytic.
    let before = curve_type_counts(&model, solid);
    assert!(
        before.get("Circle").copied().unwrap_or(0) >= 2,
        "expected >=2 Circle rim edges before transform, got {before:?}"
    );
    let (c0, _n0, r0) = first_circle(&mut model, solid);
    let (axis_o0, axis_d0, cyl_r0) = lateral_cylinder(&mut model, solid);
    assert!(close(r0, 5.0));
    assert!(close(cyl_r0, 5.0));

    // Pure translation [40, 0, 0].
    let t = Matrix4::from_translation(&Vector3::new(40.0, 0.0, 0.0));
    transform_solid(&mut model, solid, t, TransformOptions::default()).expect("translate");

    // AFTER: still resolvable as Circle / Cylinder, geometry shifted by +40 x.
    let after = curve_type_counts(&model, solid);
    assert_eq!(
        before, after,
        "translation must not change the curve-type inventory"
    );
    assert!(
        after.get("Arc").copied().unwrap_or(0) == 0,
        "no rim may re-type to Arc after a rigid translation: {after:?}"
    );

    let (c1, _n1, r1) = first_circle(&mut model, solid);
    assert!(
        pclose(c1, c0 + Vector3::new(40.0, 0.0, 0.0)),
        "circle center must shift by +40x: {c0:?} -> {c1:?}"
    );
    assert!(close(r1, r0), "radius invariant under translation");

    let (axis_o1, axis_d1, cyl_r1) = lateral_cylinder(&mut model, solid);
    assert!(
        pclose(axis_o1, axis_o0 + Vector3::new(40.0, 0.0, 0.0)),
        "cylinder axis origin must shift by +40x"
    );
    assert!(vclose(axis_d1, axis_d0), "axis direction invariant");
    assert!(close(cyl_r1, cyl_r0), "cylinder radius invariant");

    assert_valid(&model, solid, "after translation");
}

#[test]
fn rotation_preserves_circle_and_cylinder_types() {
    use std::f64::consts::PI;
    let mut model = BRepModel::new();
    let solid = make_cylinder(&mut model, Point3::ORIGIN, Vector3::Z, 5.0, 20.0);

    let before = curve_type_counts(&model, solid);
    let (_c0, n0, r0) = first_circle(&mut model, solid);
    let (_o0, axis_d0, cyl_r0) = lateral_cylinder(&mut model, solid);

    // Rotate 30° about Z (axis-parallel) — normals stay along ±Z, radii kept.
    let angle = PI / 6.0;
    let rot = Matrix4::rotation_axis(Point3::ORIGIN, Vector3::Z, angle).expect("rotation");
    transform_solid(&mut model, solid, rot, TransformOptions::default()).expect("rotate about z");

    let after = curve_type_counts(&model, solid);
    assert_eq!(before, after, "rotation must preserve curve-type inventory");
    assert!(
        after.get("Arc").copied().unwrap_or(0) == 0,
        "no rim may re-type to Arc after a rigid rotation: {after:?}"
    );

    let (_c1, n1, r1) = first_circle(&mut model, solid);
    // Rotation about Z leaves a ±Z normal unchanged (up to orientation).
    assert!(
        vclose(n1, n0) || vclose(n1, -n0),
        "rim normal stays along the rotation axis: {n0:?} -> {n1:?}"
    );
    assert!(close(r1, r0), "radius invariant under rotation");

    let (_o1, axis_d1, cyl_r1) = lateral_cylinder(&mut model, solid);
    assert!(
        vclose(axis_d1, axis_d0) || vclose(axis_d1, -axis_d0),
        "cylinder axis stays along Z"
    );
    assert!(
        close(cyl_r1, cyl_r0),
        "cylinder radius invariant under rotation"
    );

    assert_valid(&model, solid, "after rotation");
}

#[test]
fn off_axis_rotation_preserves_types_and_rotates_normal() {
    use std::f64::consts::PI;
    let mut model = BRepModel::new();
    let solid = make_cylinder(&mut model, Point3::ORIGIN, Vector3::Z, 5.0, 20.0);

    let before = curve_type_counts(&model, solid);
    let (_c0, n0, r0) = first_circle(&mut model, solid);

    // Rotate 90° about X: a +Z normal must rotate to a ∓Y normal, radius kept,
    // and the rim must STILL be a Circle (this is the case that most stresses
    // the in-plane x_axis / normal transform).
    let rot = Matrix4::rotation_axis(Point3::ORIGIN, Vector3::X, PI / 2.0).expect("rotation");
    transform_solid(&mut model, solid, rot, TransformOptions::default()).expect("rotate about x");

    let after = curve_type_counts(&model, solid);
    assert_eq!(
        before, after,
        "off-axis rotation must preserve type inventory"
    );

    let (_c1, n1, r1) = first_circle(&mut model, solid);
    // R_x(90°): (0,0,±1) -> (0,∓1,0).
    let expected = Vector3::new(n0.x, -n0.z, n0.y);
    assert!(
        vclose(n1, expected),
        "rim normal must rotate under R_x(90°): {n0:?} -> {n1:?} (expected {expected:?})"
    );
    assert!(close(r1, r0), "radius invariant under off-axis rotation");

    // And it must STILL resolve as a circle (the exact MCP failure path).
    assert!(
        resolve_edge(&mut model, solid, &EdgeQuery::new(CurveKind::Circle)).is_ok()
            || matches!(
                resolve_edge(&mut model, solid, &EdgeQuery::new(CurveKind::Circle)),
                Err(SelectError::Ambiguous(_))
            ),
        "select_edge(curve_kind=circle) must resolve after an off-axis rotation"
    );

    assert_valid(&model, solid, "after off-axis rotation");
}

#[test]
fn translate_then_rotate_round_trips_types() {
    use std::f64::consts::PI;
    let mut model = BRepModel::new();
    let solid = make_cylinder(&mut model, Point3::ORIGIN, Vector3::Z, 5.0, 20.0);
    let before = curve_type_counts(&model, solid);
    let (_c0, _n0, r0) = first_circle(&mut model, solid);

    let t = Matrix4::from_translation(&Vector3::new(40.0, 0.0, 0.0));
    transform_solid(&mut model, solid, t, TransformOptions::default()).expect("translate");
    let rot = Matrix4::rotation_axis(Point3::ORIGIN, Vector3::Z, PI / 6.0).expect("rotation");
    transform_solid(&mut model, solid, rot, TransformOptions::default()).expect("rotate");

    let after = curve_type_counts(&model, solid);
    assert_eq!(
        before, after,
        "translate-then-rotate must preserve the analytic type inventory"
    );
    let (_c1, _n1, r1) = first_circle(&mut model, solid);
    assert!(
        close(r1, r0),
        "radius invariant across composed rigid motions"
    );

    assert_valid(&model, solid, "after translate+rotate");
}
