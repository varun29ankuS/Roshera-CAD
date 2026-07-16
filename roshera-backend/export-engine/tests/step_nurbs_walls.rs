// Reason: integration-test crate -- panicking (unwrap/expect/assert/index) is
// the test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::indexing_slicing)]

//! SKETCH-DCM #45 — Wave C follow-ups (item 2): exactly-swept ruled
//! NURBS lateral walls export as proper `B_SPLINE_SURFACE_WITH_KNOTS`
//! entities (rational complex form for rational rails), and ROUND-TRIP.
//!
//! Wave B left NURBS/oblique walls as generic `RuledSurface`s, which the
//! STEP writer flattened to a degree-(1,1) 11×11 SAMPLED grid — a chord
//! approximation labeled as the wall. An exactly-swept ruled wall (both
//! rails share one NURBS basis; top rail = bottom rail + constant sweep
//! vector) IS a degree-(p, 1) NURBS surface with the rail control net
//! swept along the direction and weights preserved per row — so the
//! writer now emits that exact surface instead.
//!
//! Fixtures: the Slice-7/B closed cubic BLOB (non-rational), the exact
//! rational-quadratic ELLIPSE (weights √2/2 — P&T §7.5), and the blob
//! under an OBLIQUE extrude direction. Gate: entities parse back
//! (import), and the re-imported wall geometry matches the live wall
//! surface pointwise to tolerance.

use export_engine::formats::step::export_brep_to_step;
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::extrude::{extrude_profile_regions, ProfileLoop, ProfileRegion};
use geometry_engine::primitives::surface::{GeneralNurbsSurface, RuledSurface, Surface};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::sketch2d::sketch_topology::ProfileEdge;
use std::f64::consts::FRAC_1_SQRT_2;
use tempfile::TempDir;

const EXTRUDE_H: f64 = 5.0;

/// The Slice-7/B closed cubic blob as a typed profile edge.
fn blob_edge() -> ProfileEdge {
    ProfileEdge::Nurbs {
        degree: 3,
        control_points: vec![
            [10.0, 0.0],
            [14.0, 9.0],
            [-2.0, 12.0],
            [-8.0, 2.0],
            [2.0, -7.0],
            [10.0, 0.0],
        ],
        weights: None,
        knots: vec![0.0, 0.0, 0.0, 0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0, 1.0, 1.0, 1.0],
    }
}

/// Exact rational-quadratic ellipse (semi-axes a × b): the affine image
/// of the unit circle's canonical 9-CP degree-2 net (mid-weights √2/2,
/// double knots at ¼/½/¾ — Piegl & Tiller §7.5).
fn ellipse_edge(a: f64, b: f64) -> ProfileEdge {
    let w = FRAC_1_SQRT_2;
    ProfileEdge::Nurbs {
        degree: 2,
        control_points: vec![
            [a, 0.0],
            [a, b],
            [0.0, b],
            [-a, b],
            [-a, 0.0],
            [-a, -b],
            [0.0, -b],
            [a, -b],
            [a, 0.0],
        ],
        weights: Some(vec![1.0, w, 1.0, w, 1.0, w, 1.0, w, 1.0]),
        knots: vec![
            0.0, 0.0, 0.0, 0.25, 0.25, 0.5, 0.5, 0.75, 0.75, 1.0, 1.0, 1.0,
        ],
    }
}

/// Extrude a single-edge closed profile on the XY frame.
fn extrude_profile(edge: ProfileEdge, direction: Option<Vector3>) -> BRepModel {
    let mut model = BRepModel::new();
    extrude_profile_regions(
        &mut model,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::X,
        Vector3::Y,
        &[ProfileRegion {
            outer: ProfileLoop::Edges(vec![edge]),
            holes: vec![],
        }],
        EXTRUDE_H,
        direction,
        Tolerance::default(),
    )
    .expect("profile extrudes");
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

/// The surface id bound by each `ADVANCED_FACE` (second-to-last field).
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

/// Ids of simple-form `B_SPLINE_SURFACE_WITH_KNOTS` entities with the
/// given (u, v) degrees.
fn bspline_surface_ids(step: &str, deg_u: u32, deg_v: u32) -> Vec<String> {
    let prefix = format!("B_SPLINE_SURFACE_WITH_KNOTS('',{deg_u},{deg_v},");
    step.lines()
        .filter_map(|l| {
            let l = l.trim();
            let (id, rest) = l.split_once('=')?;
            rest.starts_with(&prefix).then(|| id.trim().to_string())
        })
        .collect()
}

/// Ids of RATIONAL complex-entity surface instances with the given
/// (u, v) degrees (`( BOUNDED_SURFACE() B_SPLINE_SURFACE(u,v,…) …
/// RATIONAL_B_SPLINE_SURFACE(…) … )`).
fn rational_bspline_surface_ids(step: &str, deg_u: u32, deg_v: u32) -> Vec<String> {
    let sig = format!("B_SPLINE_SURFACE({deg_u},{deg_v},");
    step.lines()
        .filter_map(|l| {
            let l = l.trim();
            let (id, rest) = l.split_once('=')?;
            (rest.contains(&sig) && rest.contains("RATIONAL_B_SPLINE_SURFACE"))
                .then(|| id.trim().to_string())
        })
        .collect()
}

/// Count how many of `ids` are bound by an `ADVANCED_FACE`.
fn face_bound_count(step: &str, ids: &[String]) -> usize {
    let bound: std::collections::HashSet<String> =
        advanced_face_surface_ids(step).into_iter().collect();
    ids.iter().filter(|id| bound.contains(*id)).count()
}

/// Every `RuledSurface` wall in the live model, cloned out of the store.
fn ruled_walls(model: &BRepModel) -> Vec<RuledSurface> {
    let mut out = Vec::new();
    for (_fid, face) in model.faces.iter() {
        if let Some(surface) = model.surfaces.get(face.surface_id) {
            if let Some(r) = surface.as_any().downcast_ref::<RuledSurface>() {
                out.push(r.clone());
            }
        }
    }
    out
}

/// Max pointwise distance between an imported surface and a live wall,
/// sampled on the SAME (u, v) grid — valid because the exact-sweep
/// mapping preserves the rail parameterisation (knot domain [0, 1])
/// and v ∈ [0, 1] verbatim.
fn max_grid_deviation(imported: &dyn Surface, wall: &RuledSurface) -> f64 {
    let mut worst: f64 = 0.0;
    for i in 0..=8 {
        let u = i as f64 / 8.0;
        for j in 0..=2 {
            let v = j as f64 / 2.0;
            let a = imported.point_at(u, v).expect("imported surface eval");
            let b = wall.point_at(u, v).expect("wall eval");
            worst = worst.max(a.distance(&b));
        }
    }
    worst
}

/// Re-import `step` text and return every imported NURBS surface with
/// the given (u, v) degrees.
async fn imported_nurbs_surfaces(
    step_path: &std::path::Path,
    deg_u: usize,
    deg_v: usize,
) -> Vec<GeneralNurbsSurface> {
    let (imported, report) =
        export_engine::formats::step::import_step_to_brep_with_report(step_path)
            .await
            .expect("re-import parses");
    assert!(
        !imported.solids.is_empty(),
        "re-import must resolve at least one solid; report: {report:?}"
    );
    let mut out = Vec::new();
    for sid in 0..imported.surfaces.len() as u32 {
        if let Some(surface) = imported.surfaces.get(sid) {
            if let Some(n) = surface.as_any().downcast_ref::<GeneralNurbsSurface>() {
                if n.nurbs.degree_u == deg_u && n.nurbs.degree_v == deg_v {
                    out.push(GeneralNurbsSurface {
                        nurbs: n.nurbs.clone(),
                    });
                }
            }
        }
    }
    out
}

/// Shared gate body: extrude `edge` along `direction`, assert the walls
/// export as exact face-bound B-spline surfaces of the rail degree
/// (pre-fix: the sampled degree-(1,1) grid — zero such entities), and
/// that the re-imported wall geometry matches the LIVE wall surfaces
/// pointwise to 1e-6.
async fn gate_walls(
    edge: ProfileEdge,
    direction: Option<Vector3>,
    rail_degree: u32,
    rational: bool,
    name: &str,
) {
    let model = extrude_profile(edge, direction);
    let walls = ruled_walls(&model);
    assert_eq!(
        walls.len(),
        2,
        "{name}: seam-split closed profile must carry exactly 2 ruled walls"
    );

    let temp = TempDir::new().expect("tmp dir");
    let path = temp.path().join(format!("{name}.step"));
    export_brep_to_step(&model, &path)
        .await
        .expect("STEP export must succeed");
    let step = std::fs::read_to_string(&path).expect("read STEP file");

    let ids = if rational {
        rational_bspline_surface_ids(&step, rail_degree, 1)
    } else {
        bspline_surface_ids(&step, rail_degree, 1)
    };
    assert_eq!(
        ids.len(),
        2,
        "{name}: both walls must export as exact degree-({rail_degree},1) \
         B-spline surfaces (pre-fix: 0 — the sampled degree-(1,1) grid)"
    );
    assert_eq!(
        face_bound_count(&step, &ids),
        2,
        "{name}: both exact wall surfaces must be face-bound"
    );
    // No sampled-grid wall left behind.
    assert_eq!(
        bspline_surface_ids(&step, 1, 1).len(),
        0,
        "{name}: no wall may remain a degree-(1,1) sampled grid"
    );

    // Round-trip: entities parse back and the imported walls match the
    // live walls pointwise.
    let imported = imported_nurbs_surfaces(&path, rail_degree as usize, 1).await;
    assert_eq!(
        imported.len(),
        2,
        "{name}: both exact wall surfaces must re-import as NURBS surfaces"
    );
    for (k, surf) in imported.iter().enumerate() {
        let best = walls
            .iter()
            .map(|w| max_grid_deviation(surf, w))
            .fold(f64::INFINITY, f64::min);
        assert!(
            best < 1e-6,
            "{name}: imported wall {k} must match a live wall pointwise \
             (best max-deviation {best:.3e})"
        );
    }
}

/// GATE: the closed cubic blob's two seam-split walls export exact.
#[tokio::test]
async fn blob_walls_export_exact_bspline_and_roundtrip() {
    gate_walls(blob_edge(), None, 3, false, "blob").await;
}

/// GATE: the exact rational ellipse's walls export as RATIONAL complex
/// entities with the √2/2 mid-weights preserved in the payload.
#[tokio::test]
async fn ellipse_walls_export_rational_bspline_and_roundtrip() {
    let model = extrude_profile(ellipse_edge(8.0, 5.0), None);
    let step = step_text(&model, "ellipse_pin").await;
    // Weight payload pin: √2/2 written into the rational weight grid.
    let has_weight = step
        .lines()
        .filter(|l| l.contains("RATIONAL_B_SPLINE_SURFACE"))
        .any(|l| l.contains("0.707106781187"));
    assert!(
        has_weight,
        "ellipse walls must preserve the √2/2 rational weights in STEP"
    );

    gate_walls(ellipse_edge(8.0, 5.0), None, 2, true, "ellipse").await;
}

/// GATE: an OBLIQUE extrude direction is still an exact translational
/// sweep — the oblique blob walls export exact and round-trip.
#[tokio::test]
async fn oblique_blob_walls_export_exact_bspline_and_roundtrip() {
    let dir = Vector3::new(0.35, 0.2, 1.0).normalize().expect("direction");
    gate_walls(blob_edge(), Some(dir), 3, false, "oblique_blob").await;
}

/// HONEST-SCOPE PIN: an oblique CIRCLE extrude's walls are ARC-railed
/// ruled surfaces (B6 seam-splits the circle into two half arcs). An
/// arc's `to_nurbs()` form RE-PARAMETERISES (the rational-Bézier
/// parameter is angle-nonlinear), so mapping those walls onto the NURBS
/// basis would emit a surface whose (u, v) frame silently disagrees
/// with the live `RuledSurface` — distorting every projected pcurve.
/// The parameterisation-identity guard therefore keeps Arc-railed walls
/// on the sampled-grid path; this pin is the guard's teeth (mutation:
/// dropping the guard exports them as degree-(2,1) surfaces and fails
/// here).
#[tokio::test]
async fn oblique_circle_walls_keep_sampled_fallback() {
    let dir = Vector3::new(0.35, 0.2, 1.0).normalize().expect("direction");
    let model = extrude_profile(
        ProfileEdge::Circle {
            center: [0.0, 0.0],
            radius: 6.0,
        },
        Some(dir),
    );
    let walls = ruled_walls(&model);
    assert_eq!(
        walls.len(),
        2,
        "seam-split oblique circle: 2 arc-railed walls"
    );
    let step = step_text(&model, "oblique_circle").await;
    assert_eq!(
        bspline_surface_ids(&step, 2, 1).len() + rational_bspline_surface_ids(&step, 2, 1).len(),
        0,
        "arc-railed walls must NOT map onto the re-parameterised NURBS basis"
    );
    assert!(
        !bspline_surface_ids(&step, 1, 1).is_empty(),
        "arc-railed walls stay on the explicit sampled-grid fallback"
    );
}
