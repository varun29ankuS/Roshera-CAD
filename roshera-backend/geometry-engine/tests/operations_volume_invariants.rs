// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Volume invariants for solid-producing operations (extrude, boolean).
//!
//! Extrude is the robust core: extruding a planar rectangle of area `w·h`
//! along `d` produces a `w×h×d` box, so volume must equal `w·h·d` and surface
//! area `2(wh+hd+wd)` — both integrate near-exactly (planar faces), asserted
//! at 3 %.
//!
//! Boolean volume relations (inclusion–exclusion, difference, idempotence) are
//! checked on axis-aligned boxes. These are the common, known-correct case, so
//! the op MUST succeed for these fixed inputs: each case `.expect()`s a result
//! (a `None`/`Err` is a kernel regression that fails the test) rather than
//! silently passing when the op fails — which would be a vacuous pass that
//! hides exactly the kind of regression these tests exist to catch. (The
//! "Err ⇒ skip" allowance is reserved for genuinely hard, non-axis-aligned or
//! curved inputs elsewhere; see operation_composition_invariants.rs.)

use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::{
    boolean_operation, extrude_profile, transform_solid, BooleanOp, BooleanOptions, ExtrudeOptions,
    TransformOptions,
};
use geometry_engine::primitives::curve::Line;
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use proptest::prelude::*;

fn rel_close(actual: f64, expected: f64, rel_tol: f64) -> bool {
    if expected == 0.0 {
        return actual.abs() <= rel_tol;
    }
    ((actual - expected) / expected).abs() <= rel_tol
}

// ---------------------------------------------------------------------
// Extrude helpers (mirroring operations/extrude.rs test fixtures).
// ---------------------------------------------------------------------

fn add_line_edge(model: &mut BRepModel, v_start: u32, v_end: u32) -> EdgeId {
    let s = model.vertices.get(v_start).expect("start vertex").position;
    let e = model.vertices.get(v_end).expect("end vertex").position;
    let line = Line::new(Point3::from(s), Point3::from(e));
    let curve_id = model.curves.add(Box::new(line));
    let edge = Edge::new_auto_range(0, v_start, v_end, curve_id, EdgeOrientation::Forward);
    model.edges.add(edge)
}

fn make_rectangle(model: &mut BRepModel, width: f64, height: f64) -> Vec<EdgeId> {
    let v0 = model.vertices.add(0.0, 0.0, 0.0);
    let v1 = model.vertices.add(width, 0.0, 0.0);
    let v2 = model.vertices.add(width, height, 0.0);
    let v3 = model.vertices.add(0.0, height, 0.0);
    vec![
        add_line_edge(model, v0, v1),
        add_line_edge(model, v1, v2),
        add_line_edge(model, v2, v3),
        add_line_edge(model, v3, v0),
    ]
}

/// Extrude a `w×h` rectangle by `d`; return (volume, surface_area).
fn extruded_box_props(w: f64, h: f64, d: f64) -> (f64, f64) {
    let mut model = BRepModel::new();
    let edges = make_rectangle(&mut model, w, h);
    let opts = ExtrudeOptions {
        distance: d,
        ..ExtrudeOptions::default()
    };
    let solid = extrude_profile(&mut model, edges, opts).expect("extrude profile");
    let mp = model
        .mass_properties_for(solid)
        .expect("extruded solid mass props");
    (mp.volume, mp.surface_area)
}

// ---------------------------------------------------------------------
// Extrude: volume = w·h·d, surface area = 2(wh + hd + wd).
// ---------------------------------------------------------------------

macro_rules! extrude_volume_test {
    ($name:ident, $w:expr, $h:expr, $d:expr) => {
        #[test]
        fn $name() {
            let (vol, area) = extruded_box_props($w, $h, $d);
            let (w, h, d) = ($w as f64, $h as f64, $d as f64);
            assert!(
                rel_close(vol, w * h * d, 0.03),
                "extrude {}x{} by {}: volume {} vs {}",
                $w,
                $h,
                $d,
                vol,
                w * h * d
            );
            let expected_area = 2.0 * (w * h + h * d + w * d);
            assert!(
                rel_close(area, expected_area, 0.03),
                "extrude {}x{} by {}: area {} vs {}",
                $w,
                $h,
                $d,
                area,
                expected_area
            );
        }
    };
}

extrude_volume_test!(extrude_unit, 1.0, 1.0, 1.0);
extrude_volume_test!(extrude_2_3_4, 2.0, 3.0, 4.0);
extrude_volume_test!(extrude_thin, 10.0, 0.5, 5.0);
extrude_volume_test!(extrude_tall, 1.0, 1.0, 20.0);
extrude_volume_test!(extrude_5_5_2, 5.0, 5.0, 2.0);
extrude_volume_test!(extrude_frac, 1.5, 2.5, 3.5);
extrude_volume_test!(extrude_wide, 12.0, 8.0, 1.0);
extrude_volume_test!(extrude_3_7_6, 3.0, 7.0, 6.0);
extrude_volume_test!(extrude_small, 0.5, 0.5, 0.5);
extrude_volume_test!(extrude_9_2_4, 9.0, 2.0, 4.0);
extrude_volume_test!(extrude_6_6_6, 6.0, 6.0, 6.0);
extrude_volume_test!(extrude_2_10_3, 2.0, 10.0, 3.0);

#[test]
fn extrude_volume_scales_linearly_with_distance() {
    let (v1, _) = extruded_box_props(3.0, 4.0, 2.0);
    let (v2, _) = extruded_box_props(3.0, 4.0, 4.0);
    // Doubling the extrusion distance doubles the volume.
    assert!(rel_close(v2, 2.0 * v1, 0.03), "v1={v1} v2={v2}");
}

// ---------------------------------------------------------------------
// Boolean volume relations on axis-aligned boxes. Err ⇒ skip (kernel
// numerical-rigor contract), Ok ⇒ assert the relation.
// ---------------------------------------------------------------------

fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    match builder.create_box_3d(w, h, d).expect("box") {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn vol(model: &mut BRepModel, id: SolidId) -> Option<f64> {
    model.mass_properties_for(id).map(|mp| mp.volume)
}

/// Axis-aligned box booleans are the common, known-correct case: for these
/// fixed known-good inputs the op MUST succeed. A `None`/`Err` here is a
/// kernel regression, not a hard-input skip — so call sites `.expect()` this
/// rather than silently passing the test when the operation fails to produce a
/// result (the vacuous-pass trap).
const AXIS_BOOL_MUST_SUCCEED: &str =
    "axis-aligned box boolean on known-good input must succeed (Err/None here is a kernel regression)";

/// Build A (2³ box at origin) and B (2³ box shifted +x by `shift`), run
/// `op`, and return (vol_a, vol_b, vol_result) when the op succeeds.
fn boolean_box_case(shift: f64, op: BooleanOp) -> Option<(f64, f64, f64)> {
    let mut model = BRepModel::new();
    let a = make_box(&mut model, 2.0, 2.0, 2.0);
    let b = make_box(&mut model, 2.0, 2.0, 2.0);
    transform_solid(
        &mut model,
        b,
        Matrix4::from_translation(&Vector3::new(shift, 0.0, 0.0)),
        TransformOptions::default(),
    )
    .ok()?;
    let va = vol(&mut model, a)?;
    let vb = vol(&mut model, b)?;
    let result = boolean_operation(&mut model, a, b, op, BooleanOptions::default()).ok()?;
    let vr = vol(&mut model, result)?;
    Some((va, vb, vr))
}

#[test]
fn boolean_union_inclusion_exclusion() {
    // A = [-1,1]³, B shifted +1 in x = [0,2]×[-1,1]². Overlap = 1×2×2 = 4,
    // so vol(A∪B) = 8 + 8 - 4 = 12.
    let (va, vb, vu) = boolean_box_case(1.0, BooleanOp::Union).expect(AXIS_BOOL_MUST_SUCCEED);
    let (_, _, vi) = boolean_box_case(1.0, BooleanOp::Intersection).expect(AXIS_BOOL_MUST_SUCCEED);
    assert!(
        rel_close(vu, va + vb - vi, 0.05),
        "inclusion-exclusion: vol(A∪B)={vu} vs va+vb-vi={}",
        va + vb - vi
    );
}

#[test]
fn boolean_union_between_max_and_sum() {
    let (va, vb, vu) = boolean_box_case(1.0, BooleanOp::Union).expect(AXIS_BOOL_MUST_SUCCEED);
    assert!(vu >= va.max(vb) * 0.95, "union {vu} below max input");
    assert!(vu <= (va + vb) * 1.05, "union {vu} above sum of inputs");
}

#[test]
fn boolean_intersection_at_most_min() {
    let (va, vb, vi) =
        boolean_box_case(1.0, BooleanOp::Intersection).expect(AXIS_BOOL_MUST_SUCCEED);
    assert!(
        vi <= va.min(vb) * 1.05,
        "intersection {vi} exceeds min input"
    );
    assert!(
        vi > 0.0,
        "overlapping boxes must intersect in positive volume"
    );
}

#[test]
fn boolean_difference_at_most_minuend() {
    let (va, _vb, vd) = boolean_box_case(1.0, BooleanOp::Difference).expect(AXIS_BOOL_MUST_SUCCEED);
    assert!(vd <= va * 1.05, "A−B volume {vd} exceeds A {va}");
    assert!(vd > 0.0, "A−B of partially overlapping boxes is non-empty");
}

#[test]
fn boolean_difference_equals_a_minus_intersection() {
    let (va, _, vd) = boolean_box_case(1.0, BooleanOp::Difference).expect(AXIS_BOOL_MUST_SUCCEED);
    let (_, _, vi) = boolean_box_case(1.0, BooleanOp::Intersection).expect(AXIS_BOOL_MUST_SUCCEED);
    assert!(
        rel_close(vd, va - vi, 0.05),
        "vol(A−B)={vd} vs vol(A)-vol(A∩B)={}",
        va - vi
    );
}

#[test]
fn boolean_intersection_smaller_with_less_overlap() {
    // Less overlap (larger shift) ⇒ smaller intersection volume.
    let (_, _, vi_near) =
        boolean_box_case(0.5, BooleanOp::Intersection).expect(AXIS_BOOL_MUST_SUCCEED);
    let (_, _, vi_far) =
        boolean_box_case(1.5, BooleanOp::Intersection).expect(AXIS_BOOL_MUST_SUCCEED);
    assert!(
        vi_far <= vi_near * 1.05,
        "more-separated overlap not smaller: {vi_far} vs {vi_near}"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn prop_extrude_volume_is_area_times_distance(
        w in 0.3f64..15.0, h in 0.3f64..15.0, d in 0.3f64..15.0,
    ) {
        let (vol, _) = extruded_box_props(w, h, d);
        prop_assert!(rel_close(vol, w * h * d, 0.03), "{vol} vs {}", w * h * d);
    }
}
