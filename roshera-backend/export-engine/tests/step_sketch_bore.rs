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
/// shape-bearing count.
fn face_bound_cylinder_count(step: &str) -> usize {
    let cyl_ids: std::collections::HashSet<String> =
        cylindrical_surface_ids(step).into_iter().collect();
    advanced_face_surface_ids(step)
        .iter()
        .filter(|id| cyl_ids.contains(*id))
        .count()
}

/// Every 3D surface entity of the listed simple-form types
/// (`#id=KEYWORD(...)`), as `(id, keyword)` pairs.
fn surface_entity_ids(step: &str) -> Vec<(String, &'static str)> {
    const SURFACE_KEYWORDS: [&str; 7] = [
        "PLANE",
        "CYLINDRICAL_SURFACE",
        "SPHERICAL_SURFACE",
        "CONICAL_SURFACE",
        "TOROIDAL_SURFACE",
        "SURFACE_OF_REVOLUTION",
        "B_SPLINE_SURFACE_WITH_KNOTS",
    ];
    step.lines()
        .filter_map(|l| {
            let l = l.trim();
            let (id, rest) = l.split_once('=')?;
            let kw = SURFACE_KEYWORDS
                .iter()
                .find(|kw| rest.starts_with(**kw) && rest[kw.len()..].starts_with('('))?;
            Some((id.trim().to_string(), *kw))
        })
        .collect()
}

/// Assert the export carries NO orphan geometry: every 3D surface
/// entity is bound by an `ADVANCED_FACE`, and every `CIRCLE` / `LINE`
/// curve entity is referenced by at least one other entity. The
/// kernel's boolean operand prune tombstones faces/edges but the store
/// keeps SURFACES and CURVES alive (`EntityType` has no Surface/Curve
/// variants), so a writer that dumps the raw store exports the pruned
/// operand's geometry as shape-inert orphans (the #43a dead-slot class
/// made visible in the exchange file).
fn assert_orphan_free(step: &str, label: &str) {
    let face_bound: std::collections::HashSet<String> =
        advanced_face_surface_ids(step).into_iter().collect();
    for (id, kw) in surface_entity_ids(step) {
        assert!(
            face_bound.contains(&id),
            "{label}: orphan {kw} {id} — surface entity bound by no ADVANCED_FACE"
        );
    }

    // Curve entities: `CIRCLE` and `LINE` definitions must each be
    // referenced somewhere else in the file (EDGE_CURVE edge_geometry,
    // SURFACE_CURVE/SEAM_CURVE 3D carrier, or a SURFACE_OF_REVOLUTION
    // profile). An unreferenced one is a pruned operand's leftover.
    for kw in ["CIRCLE", "LINE"] {
        let ids: Vec<String> = step
            .lines()
            .filter_map(|l| {
                let l = l.trim();
                let (id, rest) = l.split_once('=')?;
                (rest.starts_with(kw) && rest[kw.len()..].starts_with('('))
                    .then(|| id.trim().to_string())
            })
            .collect();
        for id in ids {
            let referenced = step.lines().any(|l| {
                let l = l.trim();
                if l.starts_with(&format!("{id}=")) {
                    return false; // its own definition
                }
                // Reference with a non-digit delimiter after the id so
                // #12 does not match #120.
                l.match_indices(&id).any(|(pos, _)| {
                    let after = &l[pos + id.len()..];
                    !after.starts_with(|c: char| c.is_ascii_digit())
                })
            });
            assert!(
                referenced,
                "{label}: orphan {kw} {id} — curve entity referenced by nothing"
            );
        }
    }
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

    // BOTH exports are CLEAN — the raw entity count IS the face-bound
    // count (follow-ups C item 1: the writer now emits only
    // face-referenced geometry, so the boolean-heavy drilled fixture's
    // pruned-operand surfaces no longer ride into the file; the
    // Slice-5 one-orphan pin was flipped per its delete-me note).
    assert_eq!(
        cylindrical_surface_ids(&sketch_step).len(),
        sketch_cyls,
        "sketch export must carry no orphan cylindrical surfaces"
    );
    assert_eq!(
        cylindrical_surface_ids(&drilled_step).len(),
        drilled_cyls,
        "drilled export must carry no orphan cylindrical surfaces \
         (pre-fix: the pruned cutter operand's lateral rode along as +1)"
    );

    // Full orphan sweep on both fixtures: every surface entity
    // face-bound, every CIRCLE/LINE curve entity referenced.
    assert_orphan_free(&sketch_step, "sketch bore export");
    assert_orphan_free(&drilled_step, "drilled (boolean-heavy) export");

    // The bore wall's seam edge must export as a SEAM_CURVE carrying
    // both parameter-space branches (the seam-ambiguity machinery).
    // Follow-ups C item 3 (Slice-5 residual 9): pre-fix, the sketch
    // wall's Cylinder kept the default `axis.perpendicular()` ref_dir,
    // π/2 out of phase with the seam vertex — the pcurve builder's
    // iso-u seam lines then lifted 2·r·sin(π/4) ≈ 8.5 off the actual
    // seam curve and were silently DROPPED (zero SEAM_CURVE entities),
    // so a reader fell back to the ambiguous seam reprojection this
    // machinery exists to remove.
    let seam_curves = sketch_step
        .lines()
        .filter(|l| l.contains("=SEAM_CURVE"))
        .count();
    assert!(
        seam_curves >= 1,
        "sketch bore wall must export its seam edge as a SEAM_CURVE \
         (pre-fix: 0 — misaligned ref_dir made the pcurve lift check drop it)"
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
