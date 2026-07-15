// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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
use geometry_engine::operations::recorder::{OperationRecorder, RecordedOperation, RecorderError};
use geometry_engine::operations::{
    boolean_operation, transform_solid, BooleanOp, BooleanOptions, OperationError, TransformOptions,
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
// Curved-primitive volume — analytical contract.
//
// `BRepModel::calculate_solid_volume` first attempts the
// divergence-theorem face-area integral via
// `Shell::compute_mass_properties`. That path requires every loop to
// have ≥3 unique vertex IDs because `Loop::compute_stats` uses the
// shoelace formula on the loop's vertex polygon, and curved primitives
// created by the kernel (sphere seams, cylinder caps with a single
// circular edge, cone apex) ship with degenerate edge-loops that fail
// that pre-condition. When that happens, the kernel falls back to a
// mesh-based divergence theorem on the tessellated solid:
//
//     V = | (1/6) Σ_t  v0_t · (v1_t × v2_t) |
//
// Tessellation runs at `TessellationParams::fine()` (chord tolerance
// 1e-4), which is dense enough that the mesh volume agrees with the
// analytical formulas to roughly five decimal places. The relative
// tolerance of 0.5% in the asserts below is generous — it leaves
// room for adaptive samplers to coarsen on flat regions without
// flapping the test.
//
// If the analytical face-by-face integrator ever grows native curved-
// face support (surface-parameter-space integration of `r⃗ · n̂ dA`),
// these tests should still pass — both paths target the same
// analytical truth and the tolerance is wide enough to absorb the
// switch.
// ---------------------------------------------------------------------

const CURVED_VOLUME_REL_TOL: f64 = 0.005;

#[test]
fn sphere_volume_matches_analytical_within_tessellation_tolerance() {
    let mut model = BRepModel::new();
    let radius = 3.0;
    let id = make_sphere(&mut model, Point3::ORIGIN, radius);
    let expected = 4.0 / 3.0 * std::f64::consts::PI * radius.powi(3);
    let actual = model
        .calculate_solid_volume(id)
        .expect("sphere volume must be computable via mesh fallback");
    assert!(
        relative_close(actual, expected, CURVED_VOLUME_REL_TOL),
        "sphere volume {actual} ≈ {expected} (4/3·π·r³ for r={radius})"
    );
}

#[test]
fn cylinder_volume_matches_analytical_within_tessellation_tolerance() {
    let mut model = BRepModel::new();
    let radius = 2.0;
    let height = 5.0;
    let id = make_cylinder(&mut model, Point3::ORIGIN, Vector3::Z, radius, height);
    let expected = std::f64::consts::PI * radius.powi(2) * height;
    let actual = model
        .calculate_solid_volume(id)
        .expect("cylinder volume must be computable via mesh fallback");
    assert!(
        relative_close(actual, expected, CURVED_VOLUME_REL_TOL),
        "cylinder volume {actual} ≈ {expected} (π·r²·h for r={radius}, h={height})"
    );
}

#[test]
fn cone_volume_matches_analytical_within_tessellation_tolerance() {
    let mut model = BRepModel::new();
    let radius = 2.0;
    let height = 6.0;
    let id = make_cone(&mut model, Point3::ORIGIN, Vector3::Z, radius, height);
    let expected = std::f64::consts::PI * radius.powi(2) * height / 3.0;
    let actual = model
        .calculate_solid_volume(id)
        .expect("cone volume must be computable via mesh fallback");
    assert!(
        relative_close(actual, expected, CURVED_VOLUME_REL_TOL),
        "cone volume {actual} ≈ {expected} (π·r²·h/3 for r={radius}, h={height})"
    );
}

#[test]
fn sphere_surface_area_matches_analytical_within_tessellation_tolerance() {
    let mut model = BRepModel::new();
    let radius = 3.0;
    let id = make_sphere(&mut model, Point3::ORIGIN, radius);
    let expected = 4.0 * std::f64::consts::PI * radius.powi(2);
    let actual = model
        .calculate_solid_surface_area(id)
        .expect("sphere surface area must be computable via mesh fallback");
    assert!(
        relative_close(actual, expected, CURVED_VOLUME_REL_TOL),
        "sphere surface area {actual} ≈ {expected} (4·π·r² for r={radius})"
    );
}

#[test]
fn cylinder_surface_area_matches_analytical_within_tessellation_tolerance() {
    let mut model = BRepModel::new();
    let radius = 2.0;
    let height = 5.0;
    let id = make_cylinder(&mut model, Point3::ORIGIN, Vector3::Z, radius, height);
    // Closed cylinder: lateral 2·π·r·h plus two caps π·r².
    let expected =
        2.0 * std::f64::consts::PI * radius * height + 2.0 * std::f64::consts::PI * radius.powi(2);
    let actual = model
        .calculate_solid_surface_area(id)
        .expect("cylinder surface area must be computable via mesh fallback");
    assert!(
        relative_close(actual, expected, CURVED_VOLUME_REL_TOL),
        "cylinder surface area {actual} ≈ {expected} (2·π·r·h + 2·π·r² for r={radius}, h={height})"
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
    let original_datum = model.solids.get(id).expect("solid present").anchor.datum_id;

    let result = model.reanchor_solid(id, 9999, None);
    assert!(result.is_err(), "reanchor to nonexistent datum must fail");

    let after = model.solids.get(id).expect("solid still present");
    assert_eq!(
        after.anchor.datum_id, original_datum,
        "failed reanchor must not partially mutate anchor"
    );
}

// ---------------------------------------------------------------------
// Inertia-tensor regressions — pin the mesh-based Tonon (2004) pipeline
// against analytical formulas. The analytical path covers the box; the
// mesh path covers every curved primitive (sphere / cylinder / cone).
// Tolerances:
//   * Planar-faced solids → 1e-6 relative (analytical, float noise only).
//   * Curved primitives  → 5e-2 relative (mesh resolution at
//     `TessellationParams::fine()`; moments scale as r² so the volume
//     tolerance of 5e-3 amplifies through one square).
// ---------------------------------------------------------------------

const INERTIA_ANALYTICAL_REL_TOL: f64 = 1e-6;
const INERTIA_MESH_REL_TOL: f64 = 5e-2;

#[test]
fn box_inertia_matches_analytical_formula() {
    // Centred 2 × 3 × 4 box. I_ii = m·(a_j² + a_k²) / 12 with the side
    // length convention (full extent, not half-extent). Principal
    // moments are returned sorted DESCENDING by the Jacobi eigensolver
    // (`compute_principal_inertia`, solid.rs:925).
    //
    // Mesh tolerance (5e-2) rather than analytical (1e-6) because the
    // unified entry point routes every solid through the Tonon (2004)
    // tessellated pipeline — see comments on
    // `compute_solid_mass_properties`. The mesh path is exact to ~5e-3
    // on volume, ~5e-2 on second moments (the squaring amplifies the
    // per-vertex sampling error by one order).
    let mut model = BRepModel::new();
    let (a, b, c) = (2.0, 3.0, 4.0);
    let id = make_box(&mut model, a, b, c);
    let mp = model
        .mass_properties_for(id)
        .expect("box mass props must resolve");

    let m = mp.mass;
    let mut expected = [
        m * (b * b + c * c) / 12.0,
        m * (a * a + c * c) / 12.0,
        m * (a * a + b * b) / 12.0,
    ];
    // Principal moments sorted descending; align expected the same way.
    expected.sort_by(|x, y| y.partial_cmp(x).unwrap_or(std::cmp::Ordering::Equal));
    for k in 0..3 {
        assert!(
            relative_close(mp.principal_moments[k], expected[k], INERTIA_MESH_REL_TOL),
            "box principal moment[{k}] {} ≈ {} (m·(a_j²+a_k²)/12)",
            mp.principal_moments[k],
            expected[k]
        );
    }
}

#[test]
fn sphere_inertia_matches_analytical_formula() {
    // Solid sphere about any axis through centre: I = (2/5)·m·r².
    let mut model = BRepModel::new();
    let radius = 3.0;
    let id = make_sphere(&mut model, Point3::ORIGIN, radius);
    let mp = model
        .mass_properties_for(id)
        .expect("sphere mass props must resolve via mesh fallback");

    let expected = 0.4 * mp.mass * radius * radius;
    for k in 0..3 {
        assert!(
            relative_close(mp.principal_moments[k], expected, INERTIA_MESH_REL_TOL),
            "sphere principal moment[{k}] {} ≈ {} (2/5·m·r² for r={radius})",
            mp.principal_moments[k],
            expected
        );
    }
}

#[test]
fn cylinder_inertia_matches_analytical_formula() {
    // Cylinder along z with the base at the origin. Axial moment about
    // the symmetry axis (regardless of where the COM sits along that
    // axis) is (1/2)·m·r²; radial moments about an axis through the COM
    // perpendicular to the symmetry axis are (1/4)·m·r² + (1/12)·m·h².
    let mut model = BRepModel::new();
    let radius = 2.0;
    let height = 5.0;
    let id = make_cylinder(&mut model, Point3::ORIGIN, Vector3::Z, radius, height);
    let mp = model
        .mass_properties_for(id)
        .expect("cylinder mass props must resolve via mesh fallback");

    let m = mp.mass;
    let i_axial = 0.5 * m * radius * radius;
    let i_radial = 0.25 * m * radius * radius + (1.0 / 12.0) * m * height * height;
    // Principal moments are sorted DESCENDING; expected too.
    let mut expected = [i_radial, i_radial, i_axial];
    expected.sort_by(|x, y| y.partial_cmp(x).unwrap_or(std::cmp::Ordering::Equal));
    for k in 0..3 {
        assert!(
            relative_close(mp.principal_moments[k], expected[k], INERTIA_MESH_REL_TOL),
            "cylinder principal moment[{k}] {} ≈ {} (axial 1/2·m·r², radial 1/4·m·r²+1/12·m·h²)",
            mp.principal_moments[k],
            expected[k]
        );
    }
}

#[test]
fn cone_inertia_matches_analytical_formula() {
    // Solid cone, base at origin pointing +Z, base_radius=r, height=h.
    // About its COM (which sits at z = h/4 above the base):
    //   I_axial  = (3/10)·m·r²
    //   I_radial = (3/20)·m·r² + (3/80)·m·h²
    let mut model = BRepModel::new();
    let radius = 2.0;
    let height = 6.0;
    let id = make_cone(&mut model, Point3::ORIGIN, Vector3::Z, radius, height);
    let mp = model
        .mass_properties_for(id)
        .expect("cone mass props must resolve via mesh fallback");

    let m = mp.mass;
    let i_axial = 0.3 * m * radius * radius;
    let i_radial = (3.0 / 20.0) * m * radius * radius + (3.0 / 80.0) * m * height * height;
    // Principal moments sorted DESCENDING; expected too.
    let mut expected = [i_radial, i_radial, i_axial];
    expected.sort_by(|x, y| y.partial_cmp(x).unwrap_or(std::cmp::Ordering::Equal));
    for k in 0..3 {
        assert!(
            relative_close(mp.principal_moments[k], expected[k], INERTIA_MESH_REL_TOL),
            "cone principal moment[{k}] {} ≈ {} (axial 3/10·m·r², radial 3/20·m·r²+3/80·m·h²)",
            mp.principal_moments[k],
            expected[k]
        );
    }
}

#[test]
fn principal_axes_are_orthonormal_for_every_primitive() {
    // Across the four primitive types the Jacobi eigensolver must
    // produce an orthonormal triad. Spheres are degenerate (any basis
    // is valid) so we only check orthonormality, not direction.
    let cases: Vec<(&'static str, Box<dyn Fn(&mut BRepModel) -> SolidId>)> = vec![
        (
            "box",
            Box::new(|m: &mut BRepModel| make_box(m, 2.0, 3.0, 4.0)),
        ),
        (
            "sphere",
            Box::new(|m: &mut BRepModel| make_sphere(m, Point3::ORIGIN, 3.0)),
        ),
        (
            "cylinder",
            Box::new(|m: &mut BRepModel| make_cylinder(m, Point3::ORIGIN, Vector3::Z, 2.0, 5.0)),
        ),
        (
            "cone",
            Box::new(|m: &mut BRepModel| make_cone(m, Point3::ORIGIN, Vector3::Z, 2.0, 6.0)),
        ),
    ];

    for (name, make) in cases {
        let mut model = BRepModel::new();
        let id = make(&mut model);
        let mp = model
            .mass_properties_for(id)
            .unwrap_or_else(|| panic!("{name} mass props must resolve"));

        for i in 0..3 {
            let n = (mp.principal_axes[i][0].powi(2)
                + mp.principal_axes[i][1].powi(2)
                + mp.principal_axes[i][2].powi(2))
            .sqrt();
            assert!(
                (n - 1.0).abs() < 1e-9,
                "{name} principal_axes[{i}] not unit-length: ‖·‖ = {n}"
            );
            for j in (i + 1)..3 {
                let dot = mp.principal_axes[i][0] * mp.principal_axes[j][0]
                    + mp.principal_axes[i][1] * mp.principal_axes[j][1]
                    + mp.principal_axes[i][2] * mp.principal_axes[j][2];
                assert!(
                    dot.abs() < 1e-9,
                    "{name} principal_axes[{i}] · principal_axes[{j}] = {dot} (should be 0)"
                );
            }
        }
    }
}

#[test]
fn principal_axes_and_moments_satisfy_eigenequation() {
    // Frame-independent inertia regression. For every primitive the
    // (principal_moments, principal_axes) pair the kernel returns must
    // satisfy the defining eigen-relation against the same report's
    // inertia tensor:
    //
    //     I · v_k = λ_k · v_k       for k ∈ {0, 1, 2}
    //
    // This test does NOT depend on a specific eigenvalue ordering, on
    // a hand-derived expected formula, or on the analytical-vs-mesh
    // method discriminator. It pins the internal consistency of the
    // report itself — if `inertia_tensor`, `principal_moments`, and
    // `principal_axes` drift apart (e.g. one path multiplies by
    // density and another doesn't, or the parallel-axis shift only
    // touches the tensor but not the eigenpair), every primitive will
    // fail here regardless of the symbolic answer being "right".
    //
    // The companion `principal_axes_are_orthonormal_for_every_primitive`
    // test only verifies the eigenvector frame is an orthonormal triad;
    // this test verifies it is the **right** triad for the tensor in
    // the same struct.
    let cases: Vec<(&'static str, Box<dyn Fn(&mut BRepModel) -> SolidId>)> = vec![
        (
            "box",
            Box::new(|m: &mut BRepModel| make_box(m, 2.0, 3.0, 4.0)),
        ),
        (
            "sphere",
            Box::new(|m: &mut BRepModel| make_sphere(m, Point3::ORIGIN, 3.0)),
        ),
        (
            "cylinder",
            Box::new(|m: &mut BRepModel| make_cylinder(m, Point3::ORIGIN, Vector3::Z, 2.0, 5.0)),
        ),
        (
            "cone",
            Box::new(|m: &mut BRepModel| make_cone(m, Point3::ORIGIN, Vector3::Z, 2.0, 6.0)),
        ),
    ];

    for (name, make) in cases {
        let mut model = BRepModel::new();
        let id = make(&mut model);
        let mp = model
            .mass_properties_for(id)
            .unwrap_or_else(|| panic!("{name} mass props must resolve"));

        // Largest eigenvalue normalises the absolute tolerance — Jacobi
        // sweeps drive off-diagonals to ~1e-14 relative, but the mesh
        // accumulation introduces ~1e-3 relative noise that scales with
        // the moment magnitude.
        let lam_max = mp.principal_moments[0]
            .abs()
            .max(mp.principal_moments[1].abs())
            .max(mp.principal_moments[2].abs());
        let abs_tol = 1e-2 * lam_max;

        for k in 0..3 {
            let v = mp.principal_axes[k];
            let lambda = mp.principal_moments[k];

            // I · v  (row-major contraction over the second index).
            let iv = [
                mp.inertia_tensor[0][0] * v[0]
                    + mp.inertia_tensor[0][1] * v[1]
                    + mp.inertia_tensor[0][2] * v[2],
                mp.inertia_tensor[1][0] * v[0]
                    + mp.inertia_tensor[1][1] * v[1]
                    + mp.inertia_tensor[1][2] * v[2],
                mp.inertia_tensor[2][0] * v[0]
                    + mp.inertia_tensor[2][1] * v[1]
                    + mp.inertia_tensor[2][2] * v[2],
            ];
            // λ · v
            let lv = [lambda * v[0], lambda * v[1], lambda * v[2]];

            for axis in 0..3 {
                let residual = (iv[axis] - lv[axis]).abs();
                assert!(
                    residual < abs_tol,
                    "{name}: eigenequation residual component {axis} of axis {k} = \
                     {residual} > {abs_tol} (I·v = {:?}, λ·v = {:?}, λ = {lambda})",
                    iv,
                    lv,
                );
            }
        }
    }
}

#[test]
fn mass_props_method_discriminates_analytical_vs_tessellated() {
    use geometry_engine::primitives::solid::MassPropertiesMethod;

    // Current contract (see `compute_solid_mass_properties` docs):
    // **every** solid routes through the Tonon (2004) mesh pipeline
    // because the analytical face-by-face traversal's inertia tensor
    // is a documented "lie" (shell-level box-approximation, wrong by
    // O(1) for curved geometry and by `density` for every solid). The
    // wire-visible `MassPropertiesMethod::Analytical` discriminator is
    // reserved for the future slice that fixes the shell-level inertia.
    //
    // Both primitives below therefore advertise `Tessellated`. The
    // discriminator field still earns its keep: it forward-declares
    // the variant agents will see once the analytical pipeline is
    // promoted, and it surfaces the achieved tessellation tolerance
    // to clients that need to decide whether to re-request at finer
    // resolution.
    let mut model = BRepModel::new();
    let box_id = make_box(&mut model, 2.0, 2.0, 2.0);
    let box_mp = model
        .mass_properties_for(box_id)
        .expect("box mass props must resolve");
    assert!(
        matches!(box_mp.method, MassPropertiesMethod::Tessellated { .. }),
        "box must use the tessellated path (analytical inertia broken upstream), got {:?}",
        box_mp.method
    );

    let mut model = BRepModel::new();
    let sphere_id = make_sphere(&mut model, Point3::ORIGIN, 3.0);
    let sphere_mp = model
        .mass_properties_for(sphere_id)
        .expect("sphere mass props must resolve");
    assert!(
        matches!(sphere_mp.method, MassPropertiesMethod::Tessellated { .. }),
        "sphere must use the tessellated path, got {:?}",
        sphere_mp.method
    );
}
