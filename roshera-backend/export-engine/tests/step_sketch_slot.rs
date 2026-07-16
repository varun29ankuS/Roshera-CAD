// Reason: integration-test crate -- panicking (unwrap/expect/assert/index) is
// the test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::indexing_slicing)]

//! SKETCH-DCM #45 follow-ups B, item 3 — the STEP half of the
//! partial-arc-wall gate: a slot (stadium) extrude's two semicircular
//! end-cap walls are TRUE trimmed `Cylinder` faces, so STEP maps them
//! as `CYLINDRICAL_SURFACE` (the trim lives in the face bounds,
//! exactly as ISO 10303-42 intends) and the export ROUNDTRIPS: the
//! re-imported model carries the same two cylindrical faces at the
//! exact radius.
//!
//! Pre-fix: the arc walls were generic `RuledSurface`s (Slice-5
//! residual 2) — STEP mapped them as spline surfaces, 0 face-bound
//! `CYLINDRICAL_SURFACE` entities.

use export_engine::formats::step::{export_brep_to_step, import_step_to_brep_with_report};
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::extrude::{extrude_profile_regions, ProfileLoop, ProfileRegion};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::sketch2d::sketch_topology::{AnalyticLoop, ProfileExtractor, SketchTopology};
use geometry_engine::sketch2d::{Point2d, Sketch, SketchAnchor, Tolerance2d};
use std::f64::consts::PI;
use tempfile::TempDir;

const SLOT_L: f64 = 10.0;
const SLOT_R: f64 = 5.0;
const SLOT_H: f64 = 8.0;

fn build_slot_model() -> BRepModel {
    let sketch = Sketch::new("step-slot".to_string(), SketchAnchor::xy());
    let bl = sketch.add_point(Point2d::new(-SLOT_L, -SLOT_R));
    let br = sketch.add_point(Point2d::new(SLOT_L, -SLOT_R));
    let tr = sketch.add_point(Point2d::new(SLOT_L, SLOT_R));
    let tl = sketch.add_point(Point2d::new(-SLOT_L, SLOT_R));
    sketch.add_line(bl, br).expect("bottom line");
    sketch.add_line(tr, tl).expect("top line");
    sketch
        .add_arc_center_angles(Point2d::new(SLOT_L, 0.0), SLOT_R, -PI / 2.0, PI / 2.0)
        .expect("right arc");
    sketch
        .add_arc_center_angles(Point2d::new(-SLOT_L, 0.0), SLOT_R, PI / 2.0, 3.0 * PI / 2.0)
        .expect("left arc");

    let topo = SketchTopology::analyze(&sketch, &Tolerance2d::default()).expect("topology");
    let profiles = ProfileExtractor::extract_for_extrusion(&topo).expect("profiles");
    assert_eq!(profiles.len(), 1);
    let outer =
        match ProfileExtractor::analytic_loop_edges(&sketch, &topo, &profiles[0].outer_boundary)
            .expect("extraction")
        {
            AnalyticLoop::Edges(edges) => edges,
            other => panic!("slot loop must extract analytically, got {other:?}"),
        };
    let mut model = BRepModel::new();
    extrude_profile_regions(
        &mut model,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::X,
        Vector3::Y,
        &[ProfileRegion {
            outer: ProfileLoop::Edges(outer),
            holes: Vec::new(),
        }],
        SLOT_H,
        None,
        Tolerance::default(),
    )
    .expect("slot extrude");
    model
}

/// Entity ids of every `CYLINDRICAL_SURFACE` in the STEP text.
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

/// The surface id bound by each `ADVANCED_FACE`.
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

fn face_bound_cylinder_count(step: &str) -> usize {
    let cyl_ids: std::collections::HashSet<String> =
        cylindrical_surface_ids(step).into_iter().collect();
    advanced_face_surface_ids(step)
        .iter()
        .filter(|id| cyl_ids.contains(*id))
        .count()
}

/// Kernel `Cylinder`-carrier faces of the (single) solid, as radii.
fn cylinder_face_radii(model: &BRepModel) -> Vec<f64> {
    let (_, solid) = model.solids.iter().next().expect("one solid");
    let shell = model.shells.get(solid.outer_shell).expect("shell");
    let mut radii = Vec::new();
    for &fid in &shell.faces {
        let face = model.faces.get(fid).expect("face");
        let surface = model.surfaces.get(face.surface_id).expect("surface");
        if let Some(cyl) = surface
            .as_any()
            .downcast_ref::<geometry_engine::primitives::surface::Cylinder>()
        {
            radii.push(cyl.radius);
        }
    }
    radii
}

/// GATE: the slot exports exactly TWO face-bound `CYLINDRICAL_SURFACE`
/// entities (one per end-cap wall) with the exact radius, no orphans,
/// and re-imports with the same two cylindrical faces.
#[tokio::test]
async fn gate_slot_arc_walls_export_and_roundtrip_as_cylindrical_surfaces() {
    let model = build_slot_model();

    let temp = TempDir::new().expect("tmp dir");
    let path = temp.path().join("sketch_slot.step");
    export_brep_to_step(&model, &path)
        .await
        .expect("STEP export must succeed");
    let step = std::fs::read_to_string(&path).expect("read STEP file");

    let bound = face_bound_cylinder_count(&step);
    assert_eq!(
        bound, 2,
        "both slot end-cap walls must export as face-bound CYLINDRICAL_SURFACE \
         entities (pre-fix: 0, spline-surface mapping)"
    );
    assert_eq!(
        cylindrical_surface_ids(&step).len(),
        bound,
        "sketch export must carry no orphan cylindrical surfaces"
    );
    // Exact radius in the entity payload (`CYLINDRICAL_SURFACE('',#n,5);`).
    let exact_radius_lines = step
        .lines()
        .filter(|l| l.contains("CYLINDRICAL_SURFACE") && l.contains(",5);"))
        .count();
    assert_eq!(
        exact_radius_lines,
        2,
        "both CYLINDRICAL_SURFACE entities must carry radius 5 exactly; lines: {:?}",
        step.lines()
            .filter(|l| l.contains("CYLINDRICAL_SURFACE"))
            .collect::<Vec<_>>()
    );

    // Roundtrip: the re-imported model carries the same two
    // cylindrical faces at the exact radius.
    let (imported, report) = import_step_to_brep_with_report(&path)
        .await
        .expect("STEP import must succeed");
    assert_eq!(
        imported.solids.len(),
        1,
        "slot must re-import as one solid (ok={}, roots_resolved={})",
        report.ok,
        report.roots_resolved
    );
    let radii = cylinder_face_radii(&imported);
    assert_eq!(
        radii.len(),
        2,
        "re-imported slot must carry two cylindrical faces, got {radii:?}"
    );
    for r in radii {
        assert!(
            (r - SLOT_R).abs() < 1e-9,
            "re-imported cylinder radius must be exact: {r}"
        );
    }
}
