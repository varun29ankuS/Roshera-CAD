//! End-to-end regression tests for the geometry kernel.
//!
//! These cover the **public-API workflows** the rest of the stack
//! (api-server, AI executors, export-engine, frontend) actually depends
//! on. Where the in-module unit tests verify individual functions in
//! isolation, this file pins the cross-module behaviour: a primitive
//! gets created, queries against it return sensible values, and
//! follow-up operations (transforms, booleans, anchoring, recording)
//! preserve invariants.
//!
//! Numerical-rigor ceiling matches `boolean_proptest.rs`: boolean
//! operations are allowed to return any typed `Err(..)` other than
//! `NotImplemented` until the kernel hardens further. Volume / bbox
//! checks use a generous 5% relative tolerance to absorb tessellation
//! and mesh-driven surface-area / volume integration error.

use std::sync::{Arc, Mutex};

use geometry_engine::math::{BBox, Matrix4, Point3, Tolerance, Vector3};
use geometry_engine::operations::recorder::{
    OperationRecorder, RecordedOperation, RecorderError,
};
use geometry_engine::operations::{
    boolean_operation, transform_solid, BooleanOp, BooleanOptions, OperationError,
    TransformOptions,
};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

// ---------------------------------------------------------------------
// Recorder used by the operation-chain tests.
// ---------------------------------------------------------------------

#[derive(Debug, Default)]
struct CaptureRecorder {
    events: Mutex<Vec<RecordedOperation>>,
}

impl CaptureRecorder {
    fn snapshot(&self) -> Vec<RecordedOperation> {
        self.events
            .lock()
            .expect("CaptureRecorder mutex poisoned")
            .clone()
    }
}

impl OperationRecorder for CaptureRecorder {
    fn record(&self, operation: RecordedOperation) -> Result<(), RecorderError> {
        self.events
            .lock()
            .expect("CaptureRecorder mutex poisoned")
            .push(operation);
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Primitive helpers.
// ---------------------------------------------------------------------

fn expect_solid(geom: GeometryId) -> SolidId {
    match geom {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid geometry, got {other:?}"),
    }
}

fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    expect_solid(
        builder
            .create_box_3d(w, h, d)
            .expect("box creation succeeds"),
    )
}

fn make_sphere(model: &mut BRepModel, center: Point3, radius: f64) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    expect_solid(
        builder
            .create_sphere_3d(center, radius)
            .expect("sphere creation succeeds"),
    )
}

fn make_cylinder(
    model: &mut BRepModel,
    center: Point3,
    axis: Vector3,
    radius: f64,
    height: f64,
) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    expect_solid(
        builder
            .create_cylinder_3d(center, axis, radius, height)
            .expect("cylinder creation succeeds"),
    )
}

fn make_cone(
    model: &mut BRepModel,
    base_center: Point3,
    axis: Vector3,
    base_radius: f64,
    height: f64,
) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    expect_solid(
        builder
            .create_cone_3d(base_center, axis, base_radius, 0.0, height)
            .expect("cone creation succeeds"),
    )
}

fn relative_close(actual: f64, expected: f64, rel_tol: f64) -> bool {
    if expected == 0.0 {
        return actual.abs() <= rel_tol;
    }
    ((actual - expected) / expected).abs() <= rel_tol
}

fn bbox_extent(bbox: &BBox) -> (f64, f64, f64) {
    (
        bbox.max.x - bbox.min.x,
        bbox.max.y - bbox.min.y,
        bbox.max.z - bbox.min.z,
    )
}

// ---------------------------------------------------------------------
// Primitive workflows
// ---------------------------------------------------------------------

#[test]
fn default_brep_model_seeds_seven_canonical_datums() {
    // Slice 3a invariant: every newly-constructed model has Origin +
    // 3 reference planes + 3 reference axes available without any
    // explicit setup. AI executors and datum queries depend on this.
    let model = BRepModel::new();
    assert_eq!(
        model.datums.len(),
        7,
        "fresh BRepModel must seed exactly 7 default datums (Origin + 3 planes + 3 axes)"
    );
    assert!(
        model.datums.get(0).is_some(),
        "datum id 0 must be the world Origin"
    );
}

#[test]
fn fresh_model_has_no_solids() {
    let model = BRepModel::new();
    assert_eq!(model.solids.len(), 0);
}

#[test]
fn box_creation_yields_one_solid_anchored_to_world_origin() {
    // Slice 3a regression: every solid carries an anchor; primitives
    // created via TopologyBuilder default to SolidAnchor::world_origin
    // (datum_id = 0, identity local transform).
    let mut model = BRepModel::new();
    let id = make_box(&mut model, 1.0, 1.0, 1.0);

    assert_eq!(model.solids.len(), 1);
    let solid = model.solids.get(id).expect("solid exists");
    assert_eq!(solid.anchor.datum_id, 0, "default anchor is world Origin");
    assert!(
        solid
            .anchor
            .local_transform
            .is_identity(Tolerance::default()),
        "default local transform is identity"
    );
}

#[test]
fn box_volume_matches_dimensions() {
    let mut model = BRepModel::new();
    let id = make_box(&mut model, 2.0, 3.0, 4.0);
    let vol = model
        .calculate_solid_volume(id)
        .expect("volume query succeeds");
    assert!(
        relative_close(vol, 24.0, 0.05),
        "box(2,3,4) volume {vol} should be ≈ 24.0 (5% rel tol)"
    );
}

#[test]
fn box_world_bbox_extent_matches_dimensions() {
    let mut model = BRepModel::new();
    let id = make_box(&mut model, 5.0, 7.0, 11.0);
    let bbox = model.solid_world_bbox(id).expect("world bbox available");
    let (ex, ey, ez) = bbox_extent(&bbox);
    // Box can be centred at origin or at +half — we only check extents.
    assert!(relative_close(ex, 5.0, 0.05), "x extent {ex} ≈ 5.0");
    assert!(relative_close(ey, 7.0, 0.05), "y extent {ey} ≈ 7.0");
    assert!(relative_close(ez, 11.0, 0.05), "z extent {ez} ≈ 11.0");
}

// ---------------------------------------------------------------------
// Curved-primitive volume gap (regression contract).
//
// `BRepModel::calculate_solid_volume` delegates to
// `Solid::compute_mass_properties`, which iterates the shell's faces
// and runs the divergence-theorem face-area integral via
// `Shell::compute_mass_properties`. That path requires every loop to
// have ≥3 unique vertex IDs because `Loop::compute_stats` uses the
// shoelace formula on the loop's vertex polygon.
//
// Curved primitives created by the kernel (sphere, cylinder caps,
// cone apex) ship with degenerate edge-loops — a cylinder cap has a
// single circular edge connecting one start/end vertex; a sphere
// patch has a similarly compressed seam. Until the kernel grows
// surface-parameter-space integration of `r⃗ · n̂ dA` for curved
// faces (or auto-densifies degenerate loops to a polygonal
// approximation), `calculate_solid_volume` will return `None` for
// these primitives.
//
// The three tests below pin that contract: they assert the kernel
// honestly returns `None` rather than fabricating a wrong number.
// The day someone adds curved-surface integration, these tests will
// flip to `Some(_)` and need to be tightened to assert the analytical
// value (4/3·π·r³, π·r²·h, π·r²·h/3) within tessellation tolerance.
// ---------------------------------------------------------------------

#[test]
fn sphere_volume_currently_unsupported_returns_none() {
    let mut model = BRepModel::new();
    let id = make_sphere(&mut model, Point3::ORIGIN, 3.0);
    assert!(
        model.calculate_solid_volume(id).is_none(),
        "kernel does not yet integrate curved-surface volumes; expected None"
    );
}

#[test]
fn cylinder_volume_currently_unsupported_returns_none() {
    let mut model = BRepModel::new();
    let id = make_cylinder(&mut model, Point3::ORIGIN, Vector3::Z, 2.0, 5.0);
    assert!(
        model.calculate_solid_volume(id).is_none(),
        "kernel does not yet integrate curved-surface volumes; expected None"
    );
}

#[test]
fn cone_volume_currently_unsupported_returns_none() {
    let mut model = BRepModel::new();
    let id = make_cone(&mut model, Point3::ORIGIN, Vector3::Z, 2.0, 6.0);
    assert!(
        model.calculate_solid_volume(id).is_none(),
        "kernel does not yet integrate curved-surface volumes; expected None"
    );
}

#[test]
fn two_independent_boxes_yield_distinct_solid_ids() {
    let mut model = BRepModel::new();
    let a = make_box(&mut model, 1.0, 1.0, 1.0);
    let b = make_box(&mut model, 2.0, 2.0, 2.0);
    assert_ne!(a, b, "second box must get a fresh solid id");
    assert_eq!(model.solids.len(), 2);
}

#[test]
fn solid_distance_to_self_is_zero() {
    let mut model = BRepModel::new();
    let id = make_box(&mut model, 1.0, 1.0, 1.0);
    let d = model
        .solid_distance(id, id)
        .expect("self-distance available");
    assert!(d.abs() < 1e-9, "self-distance must be 0, got {d}");
}

#[test]
fn solid_bbox_in_world_frame_matches_world_bbox() {
    // Slice 5 regression: the datum-frame bbox query against the world
    // Origin (datum 0) must agree with the cached world bbox.
    let mut model = BRepModel::new();
    let id = make_box(&mut model, 4.0, 4.0, 4.0);

    let world = model.solid_world_bbox(id).expect("world bbox");
    let in_origin = model
        .solid_bbox_in_frame(id, 0)
        .expect("frame bbox in datum 0");

    let (wx, wy, wz) = bbox_extent(&world);
    let (ox, oy, oz) = bbox_extent(&in_origin);
    assert!(
        relative_close(wx, ox, 1e-9),
        "x extent diverges: world={wx} in-origin={ox}"
    );
    assert!(
        relative_close(wy, oy, 1e-9),
        "y extent diverges: world={wy} in-origin={oy}"
    );
    assert!(
        relative_close(wz, oz, 1e-9),
        "z extent diverges: world={wz} in-origin={oz}"
    );
}

// ---------------------------------------------------------------------
// Transform pipeline
// ---------------------------------------------------------------------

#[test]
fn transform_translates_solid_world_bbox() {
    let mut model = BRepModel::new();
    let id = make_box(&mut model, 2.0, 2.0, 2.0);
    let before = model
        .solid_world_bbox(id)
        .expect("pre-transform bbox available");

    let shift = Vector3::new(10.0, 0.0, 0.0);
    transform_solid(
        &mut model,
        id,
        Matrix4::from_translation(&shift),
        TransformOptions::default(),
    )
    .expect("translate succeeds");

    let after = model
        .solid_world_bbox(id)
        .expect("post-transform bbox available");

    let dx = after.min.x - before.min.x;
    let dy = after.min.y - before.min.y;
    let dz = after.min.z - before.min.z;
    assert!(
        (dx - 10.0).abs() < 1e-6,
        "min.x should translate by 10, got dx={dx}"
    );
    assert!(dy.abs() < 1e-6, "y should not move, got dy={dy}");
    assert!(dz.abs() < 1e-6, "z should not move, got dz={dz}");
}

#[test]
fn transform_preserves_solid_count() {
    let mut model = BRepModel::new();
    let id = make_box(&mut model, 1.0, 1.0, 1.0);
    let before = model.solids.len();
    transform_solid(
        &mut model,
        id,
        Matrix4::from_translation(&Vector3::new(1.0, 2.0, 3.0)),
        TransformOptions::default(),
    )
    .expect("translate succeeds");
    assert_eq!(
        model.solids.len(),
        before,
        "transform must mutate in place — solid count must not change"
    );
}

// ---------------------------------------------------------------------
// Boolean smoke regression
//
// The kernel's boolean engine is not yet rigorously volume-correct; we
// only check that the pipeline terminates with a typed outcome and any
// `Ok` result references a real solid in the model. Mirrors the
// numerical ceiling pinned in `boolean_proptest.rs`.
// ---------------------------------------------------------------------

fn assert_typed_or_solid(
    result: &Result<SolidId, OperationError>,
    model: &BRepModel,
    op: BooleanOp,
) {
    if let Err(OperationError::NotImplemented(msg)) = result {
        panic!("{op:?} returned NotImplemented({msg})");
    }
    if let Ok(solid_id) = result {
        assert!(
            model.solids.get(*solid_id).is_some(),
            "{op:?} returned Ok({solid_id}) but the solid is missing from the model",
        );
    }
}

#[test]
fn boolean_union_of_overlapping_boxes_returns_solid_or_typed_error() {
    let mut model = BRepModel::new();
    let a = make_box(&mut model, 4.0, 4.0, 4.0);
    let b = make_box(&mut model, 3.0, 3.0, 3.0);
    let result = boolean_operation(
        &mut model,
        a,
        b,
        BooleanOp::Union,
        BooleanOptions::default(),
    );
    assert_typed_or_solid(&result, &model, BooleanOp::Union);
}

#[test]
fn boolean_difference_of_overlapping_boxes_returns_solid_or_typed_error() {
    let mut model = BRepModel::new();
    let a = make_box(&mut model, 4.0, 4.0, 4.0);
    let b = make_box(&mut model, 2.0, 2.0, 2.0);
    let result = boolean_operation(
        &mut model,
        a,
        b,
        BooleanOp::Difference,
        BooleanOptions::default(),
    );
    assert_typed_or_solid(&result, &model, BooleanOp::Difference);
}

#[test]
fn boolean_intersection_of_overlapping_boxes_returns_solid_or_typed_error() {
    let mut model = BRepModel::new();
    let a = make_box(&mut model, 4.0, 4.0, 4.0);
    let b = make_box(&mut model, 3.0, 3.0, 3.0);
    let result = boolean_operation(
        &mut model,
        a,
        b,
        BooleanOp::Intersection,
        BooleanOptions::default(),
    );
    assert_typed_or_solid(&result, &model, BooleanOp::Intersection);
}

#[test]
fn boolean_with_unknown_solid_id_returns_typed_error() {
    let mut model = BRepModel::new();
    let a = make_box(&mut model, 1.0, 1.0, 1.0);
    let bogus: SolidId = 9999;
    let result = boolean_operation(
        &mut model,
        a,
        bogus,
        BooleanOp::Union,
        BooleanOptions::default(),
    );
    // Whatever error type comes back, it must be typed (no panic, no
    // NotImplemented).
    if let Err(OperationError::NotImplemented(msg)) = &result {
        panic!("Union with bogus operand returned NotImplemented({msg})");
    }
    assert!(
        result.is_err(),
        "boolean against a missing solid must surface as Err"
    );
}

// ---------------------------------------------------------------------
// Recorder integration across a multi-step workflow
// ---------------------------------------------------------------------

#[test]
fn operation_recorder_captures_full_primitive_chain() {
    // A realistic AI-executor workflow: create a box, then a cylinder,
    // then translate the box. The recorder must observe all three
    // events in order with non-null parameter payloads.
    let mut model = BRepModel::new();
    let capture = Arc::new(CaptureRecorder::default());
    model.attach_recorder(Some(capture.clone() as Arc<dyn OperationRecorder>));

    let box_id = make_box(&mut model, 2.0, 2.0, 2.0);
    let _cyl_id = make_cylinder(&mut model, Point3::ORIGIN, Vector3::Z, 1.0, 3.0);
    transform_solid(
        &mut model,
        box_id,
        Matrix4::from_translation(&Vector3::new(5.0, 0.0, 0.0)),
        TransformOptions::default(),
    )
    .expect("translate succeeds");

    let events = capture.snapshot();
    assert!(
        events.len() >= 3,
        "expected at least 3 recorded events (box, cylinder, transform), got {}",
        events.len()
    );

    // The two creations are first; ordering is a hard invariant.
    assert_eq!(events[0].kind, "create_box_3d");
    assert_eq!(events[1].kind, "create_cylinder_3d");

    // Some kind tagged with "transform" should appear after the
    // creations. We don't pin the exact kind string because the
    // transform module records under a shared name.
    let post = &events[2..];
    assert!(
        post.iter().any(|e| e.kind.contains("transform")),
        "expected a transform-flavored event after creations, got kinds: {:?}",
        post.iter().map(|e| &e.kind).collect::<Vec<_>>()
    );

    // Every event must carry a non-null parameter payload so timeline
    // replay can reconstruct it.
    for e in &events {
        assert!(
            !e.parameters.is_null(),
            "event {} has null parameters",
            e.kind
        );
    }
}

// ---------------------------------------------------------------------
// Anchoring & datum-relative queries
// ---------------------------------------------------------------------

#[test]
fn reanchor_solid_updates_anchor_metadata() {
    // Slice 5 regression: reanchor mutates the solid's anchor metadata
    // and the change is observable via the public `solids` store.
    let mut model = BRepModel::new();
    let id = make_box(&mut model, 1.0, 1.0, 1.0);

    // Datum id 0 is Origin; pick a different default datum (1) to
    // re-anchor to. The seven defaults are id 0..=6.
    let target_datum = 1u32;
    model
        .reanchor_solid(id, target_datum, None)
        .expect("reanchor to a seeded default datum succeeds");

    let solid = model.solids.get(id).expect("solid still present");
    assert_eq!(
        solid.anchor.datum_id, target_datum,
        "anchor.datum_id should reflect the new target"
    );
}

#[test]
fn reanchor_to_unknown_datum_returns_error_and_preserves_anchor() {
    let mut model = BRepModel::new();
    let id = make_box(&mut model, 1.0, 1.0, 1.0);
    let original_datum = model
        .solids
        .get(id)
        .expect("solid present")
        .anchor
        .datum_id;

    let result = model.reanchor_solid(id, 9999, None);
    assert!(result.is_err(), "reanchor to nonexistent datum must fail");

    let after = model.solids.get(id).expect("solid still present");
    assert_eq!(
        after.anchor.datum_id, original_datum,
        "failed reanchor must not partially mutate anchor"
    );
}
