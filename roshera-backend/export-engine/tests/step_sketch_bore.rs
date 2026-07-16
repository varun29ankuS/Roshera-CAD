// Reason: integration-test crate -- panicking (unwrap/expect/assert/index) is
// the test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::indexing_slicing)]

//! SKETCH-DCM #45 Slice 5 — the STEP half of the gate (spec §3.5):
//! "csketch circle-in-rectangle extrude produces a solid whose bore
//! face count and STEP output match the `create_cylinder`-drilled
//! equivalent."
//!
//! The sketch-extruded bore lateral must export as the same
//! `CYLINDRICAL_SURFACE` entity class the primitive-drilled bore does
//! (the f44e6f1-hardened mapping) — pre-slice it exported as 64
//! planar side strips with zero `CYLINDRICAL_SURFACE` entities.

use export_engine::formats::step::export_brep_to_step;
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::extrude::{extrude_profile_regions, ProfileLoop, ProfileRegion};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::sketch2d::sketch_topology::{AnalyticLoop, ProfileExtractor, SketchTopology};
use geometry_engine::sketch2d::{Point2d, Sketch, SketchAnchor, Tolerance2d};
use tempfile::TempDir;

const RECT_W: f64 = 40.0;
const RECT_H: f64 = 30.0;
const BORE_R: f64 = 6.0;
const EXTRUDE_H: f64 = 10.0;

fn as_solid(g: GeometryId) -> u32 {
    match g {
        GeometryId::Solid(id) => id,
        other => panic!("expected a solid, got {other:?}"),
    }
}

/// Sketch-built gate solid: rectangle + circle hole, extracted as
/// typed analytic edges and extruded through the shared kernel entry.
fn build_sketch_bore() -> BRepModel {
    let sketch = Sketch::new("step-gate".to_string(), SketchAnchor::xy());
    sketch
        .add_rectangle(Point2d::new(0.0, 0.0), Point2d::new(RECT_W, RECT_H))
        .expect("rectangle");
    sketch
        .add_circle(Point2d::new(RECT_W / 2.0, RECT_H / 2.0), BORE_R)
        .expect("circle");
    let topo = SketchTopology::analyze(&sketch, &Tolerance2d::default()).expect("topology");
    let profiles = ProfileExtractor::extract_for_extrusion(&topo).expect("profiles");
    assert_eq!(profiles.len(), 1);
    let to_edges =
        |lp| match ProfileExtractor::analytic_loop_edges(&sketch, &topo, lp).expect("extraction") {
            AnalyticLoop::Edges(edges) => edges,
            other => panic!("gate loops must extract analytically, got {other:?}"),
        };
    let region = ProfileRegion {
        outer: ProfileLoop::Edges(to_edges(&profiles[0].outer_boundary)),
        holes: vec![ProfileLoop::Edges(to_edges(&profiles[0].holes[0]))],
    };
    let mut model = BRepModel::new();
    extrude_profile_regions(
        &mut model,
        Point3::new(-RECT_W / 2.0, -RECT_H / 2.0, -EXTRUDE_H / 2.0),
        Vector3::X,
        Vector3::Y,
        &[region],
        EXTRUDE_H,
        None,
        Tolerance::default(),
    )
    .expect("analytic extrude");
    model
}

/// Primitive-drilled equivalent: box minus through-cylinder.
fn build_drilled_equivalent() -> BRepModel {
    let mut model = BRepModel::new();
    let box_id = as_solid(
        TopologyBuilder::new(&mut model)
            .create_box_3d(RECT_W, RECT_H, EXTRUDE_H)
            .expect("box"),
    );
    let cyl_id = as_solid(
        TopologyBuilder::new(&mut model)
            .create_cylinder_3d(
                Point3::new(0.0, 0.0, -EXTRUDE_H / 2.0 - 1.0),
                Vector3::Z,
                BORE_R,
                EXTRUDE_H + 2.0,
            )
            .expect("cylinder"),
    );
    boolean_operation(
        &mut model,
        box_id,
        cyl_id,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("box minus cylinder");
    model
}

async fn step_text(model: &BRepModel, name: &str) -> String {
    let temp = TempDir::new().expect("tmp dir");
    let path = temp.path().join(format!("{name}.step"));
    export_brep_to_step(model, &path)
        .await
        .expect("STEP export must succeed");
    std::fs::read_to_string(&path).expect("read STEP file")
}

/// Entity ids of every `CYLINDRICAL_SURFACE` in the STEP text
/// (`#id=CYLINDRICAL_SURFACE(...)` lines).
fn cylindrical_surface_ids(step: &str) -> Vec<String> {
    step.lines()
        .filter_map(|l| {
            let l = l.trim();
            let (id, rest) = l.split_once('=')?;
            rest.starts_with("CYLINDRICAL_SURFACE")
                .then(|| id.trim().to_string())
        })
        .collect()
}

/// The surface id bound by each `ADVANCED_FACE`
/// (`#id=ADVANCED_FACE('',(...),#surface,.T.);` — surface is the
/// second-to-last field, before the same_sense flag).
fn advanced_face_surface_ids(step: &str) -> Vec<String> {
    step.lines()
        .filter(|l| l.contains("=ADVANCED_FACE"))
        .filter_map(|l| {
            let (head, _sense) = l.rsplit_once(",.")?;
            let id = head.rsplit(',').next()?.trim().to_string();
            id.starts_with('#').then_some(id)
        })
        .collect()
}

/// `CYLINDRICAL_SURFACE` entities actually BOUND by a face — the
/// shape-bearing count. (The raw entity count can include orphan
/// surfaces: the boolean's operand prune tombstones faces but leaves
/// the operand's surfaces live in the store, and the STEP writer dumps
/// every store surface — a pre-existing #43a-class artifact pinned
/// below, NOT part of the solid's shape.)
fn face_bound_cylinder_count(step: &str) -> usize {
    let cyl_ids: std::collections::HashSet<String> =
        cylindrical_surface_ids(step).into_iter().collect();
    advanced_face_surface_ids(step)
        .iter()
        .filter(|id| cyl_ids.contains(*id))
        .count()
}

/// GATE (STEP half): the sketch bore exports the SAME count of
/// `CYLINDRICAL_SURFACE` entities as the drilled equivalent (≥ 1),
/// with the exact radius in the entity payload. Pre-slice: 0.
#[tokio::test]
async fn gate_step_output_matches_drilled_equivalent() {
    let sketch_model = build_sketch_bore();
    let drilled_model = build_drilled_equivalent();

    let sketch_step = step_text(&sketch_model, "sketch_bore").await;
    let drilled_step = step_text(&drilled_model, "drilled_bore").await;

    let sketch_cyls = face_bound_cylinder_count(&sketch_step);
    let drilled_cyls = face_bound_cylinder_count(&drilled_step);
    assert!(
        drilled_cyls >= 1,
        "drilled reference must export a face-bound CYLINDRICAL_SURFACE (got {drilled_cyls})"
    );
    assert_eq!(
        sketch_cyls, drilled_cyls,
        "sketch bore must export the same face-bound CYLINDRICAL_SURFACE count \
         as the create_cylinder-drilled equivalent (pre-slice: 0 vs {drilled_cyls})"
    );

    // The sketch-built export is CLEAN: every CYLINDRICAL_SURFACE
    // entity is face-bound (no orphan geometry).
    assert_eq!(
        cylindrical_surface_ids(&sketch_step).len(),
        sketch_cyls,
        "sketch export must carry no orphan cylindrical surfaces"
    );

    // Pin the pre-existing #43a-class artifact honestly instead of
    // hiding it: the drilled model's boolean prune leaves the pruned
    // cylinder OPERAND's lateral surface live in the surface store, and
    // the STEP writer dumps every store surface — so the drilled export
    // carries exactly one orphan (never face-bound, shape-inert)
    // CYLINDRICAL_SURFACE entity. If this assertion starts failing with
    // 0 orphans, the writer artifact was fixed — delete this pin and
    // tighten the gate to raw entity counts.
    let drilled_raw = cylindrical_surface_ids(&drilled_step).len();
    assert_eq!(
        drilled_raw,
        drilled_cyls + 1,
        "expected exactly one orphan operand CYLINDRICAL_SURFACE in the drilled \
         export (raw {drilled_raw} vs face-bound {drilled_cyls}) — see comment"
    );

    // The bore radius rides in the CYLINDRICAL_SURFACE payload
    // (`CYLINDRICAL_SURFACE('',#n,6);`) — assert it appears with the
    // exact dimension, not a chord-fit value.
    let has_exact_radius = sketch_step
        .lines()
        .filter(|l| l.contains("CYLINDRICAL_SURFACE"))
        .any(|l| l.contains(",6);"));
    assert!(
        has_exact_radius,
        "sketch bore CYLINDRICAL_SURFACE must carry radius 6 exactly; lines: {:?}",
        sketch_step
            .lines()
            .filter(|l| l.contains("CYLINDRICAL_SURFACE"))
            .collect::<Vec<_>>()
    );
}
