//! COMPREHENSIVE round-trip fidelity harness for the Roshera native `.ros`
//! format — the gate every future format change is verified against.
//!
//! ## Why this file exists
//!
//! The pre-existing `ros_format_tests.rs` only ever asserts vertex *counts*
//! (`imported.vertices.len() == model.vertices.len()`). A format that drops
//! every geometric coordinate, every curve parameter, every semantic sidecar
//! would pass those tests so long as the entity *count* survived. Two prior
//! audits established the real gaps:
//!
//!   1. The live export (`ExportEngine::export_ros` → `engine.rs`) writes an
//!      EMPTY timeline + EMPTY provenance (`history: None, aipr: None`).
//!   2. The GEOM snapshot (`BRepSnapshot`) drops every semantic sidecar:
//!      persistent ids, labels, datums, GD&T, solid provenance, construction
//!      geometry, sketch planes, materials, assemblies.
//!   3. Edge `param_range`s are reconstructed as `ParameterRange::unit()`.
//!   4. Pcurves are dropped.
//!   5. Surfaces/curves the analytic downcasts don't recognise are refit to
//!      degree-1 polyline B-splines (lossy).
//!
//! This harness builds REAL artifacts — a box, a cylinder, a box−cylinder
//! bore (boolean), and a NURBS loft — round-trips each through
//! `export_brep_to_ros` → `import_ros`, and asserts FULL fidelity. Each
//! property is its own named test so a regression pinpoints itself.
//!
//! ## Convention for known gaps
//!
//! Assertions that CANNOT pass today (because the format genuinely drops the
//! data) are marked `#[ignore = "GAP: …"]` with a one-line reason naming the
//! campaign slice that will close them. The assertion body is written exactly
//! as it should read once the slice lands, so enabling it is removing the one
//! `#[ignore]` line. Everything that CAN pass today is a live test and MUST
//! pass.
//!
//! Run: `cargo test -p export-engine --test ros_roundtrip_harness -- --nocapture`

use export_engine::formats::ros::{
    export_brep_to_ros, import_ros, HistData, RosExportOptions, RosExportPayload,
};
use export_engine::formats::timeline_chunk::BranchManifest;

use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::nurbs_loft::{nurbs_loft, NurbsLoftOptions};
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::curve::NurbsCurve;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::GeneralNurbsSurface;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

use ros_format::{AICommandTracker, CommandType, PrivacySettings, TrackingLevel};

use std::collections::HashMap;
use tempfile::TempDir;
use timeline_engine::{
    Author, BranchId, BranchMetadata, BranchPurpose, BranchState, EventId, EventMetadata,
    Operation, OperationInputs, OperationOutputs, TimelineEvent,
};

// ───────────────────────────────────────────────────────────────────────────
// Tolerances and small helpers
// ───────────────────────────────────────────────────────────────────────────

/// Geometric position tolerance for "the coordinate survived the round trip".
/// MessagePack stores f64 verbatim, so survival should be bit-exact; we use a
/// tiny epsilon to stay robust to any future quantization in the codec.
const POS_EPS: f64 = 1e-9;

/// Chord tolerance for the watertight tessellation oracle. Coarse enough to be
/// fast, fine enough to expose real leaks on these unit-to-tens-of-mm solids.
const CHORD: f64 = 0.05;

/// Vertex-weld epsilon recommended by `manifold_report` for solids in the
/// unit-to-ten-unit size range.
const WELD_EPS: f64 = 1e-6;

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(id) => id,
        other => panic!("expected a Solid geometry id, got {other:?}"),
    }
}

/// Write `model` to a `.ros` file (GEOM cache on) and read it straight back
/// through the snapshot path. Returns the rebuilt `BRepModel`.
async fn roundtrip_geometry(model: &BRepModel) -> BRepModel {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("rt.ros");
    export_brep_to_ros(
        RosExportPayload {
            model,
            history: None,
            aipr: None,
        },
        &path,
        RosExportOptions::default(),
    )
    .await
    .expect("export should succeed");

    let imported = import_ros(&path, None)
        .await
        .expect("import should succeed");
    imported
        .snapshot
        .expect("GEOM snapshot must be present (include_snapshot default = true)")
        .to_model()
}

/// Sort a slice of `[f64;3]` lexicographically so two unordered vertex sets can
/// be compared positionally without depending on store insertion order.
fn sorted_positions(model: &BRepModel) -> Vec<[f64; 3]> {
    let mut v: Vec<[f64; 3]> = model.vertices.iter().map(|(_, vx)| vx.position).collect();
    v.sort_by(|a, b| {
        a.partial_cmp(b)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a[1].partial_cmp(&b[1]).unwrap_or(std::cmp::Ordering::Equal))
    });
    // The closure above is a fallback; do a full lexicographic sort properly.
    v.sort_by(|a, b| {
        for k in 0..3 {
            match a[k].partial_cmp(&b[k]) {
                Some(std::cmp::Ordering::Equal) | None => continue,
                Some(o) => return o,
            }
        }
        std::cmp::Ordering::Equal
    });
    v
}

fn approx_pos(a: [f64; 3], b: [f64; 3], eps: f64) -> bool {
    (a[0] - b[0]).abs() <= eps && (a[1] - b[1]).abs() <= eps && (a[2] - b[2]).abs() <= eps
}

/// First solid id in the model (these models each build exactly one final
/// solid; the box−cyl bore leaves the operand solids behind too, so we take
/// the last-added solid as the result).
fn last_solid(model: &BRepModel) -> SolidId {
    let mut max = None;
    for (id, _) in model.solids.iter() {
        max = Some(max.map_or(id, |m: SolidId| m.max(id)));
    }
    max.expect("model has at least one solid")
}

// ───────────────────────────────────────────────────────────────────────────
// Artifact builders — REAL geometry through the public kernel API
// ───────────────────────────────────────────────────────────────────────────

/// A 40×40×20 box centred at the origin (8v / 12e / 6f / 1 shell / 1 solid).
fn build_box() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let s = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(40.0, 40.0, 20.0)
        .expect("box"));
    (m, s)
}

/// A cylinder: radius 8, height 30, axis +Z, base at z = −15.
fn build_cylinder() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let s = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -15.0), Vector3::Z, 8.0, 30.0)
        .expect("cylinder"));
    (m, s)
}

/// A box with a cylindrical through-bore: (40×40×20 box) − (r=8 cylinder that
/// fully pierces it). Returns the model and the result solid id.
fn build_box_minus_cylinder_bore() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let box_s = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(40.0, 40.0, 20.0)
        .expect("box"));
    let cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -15.0), Vector3::Z, 8.0, 30.0)
        .expect("cyl"));
    let res = boolean_operation(
        &mut m,
        box_s,
        cyl,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("box − cyl bore");
    (m, res)
}

/// Ordered ring of `n` points on a circle of radius `r` at height `z`.
fn ring(r: f64, z: f64, n: usize) -> Vec<Point3> {
    (0..n)
        .map(|i| {
            let t = 2.0 * std::f64::consts::PI * (i as f64) / (n as f64);
            Point3::new(r * t.cos(), r * t.sin(), z)
        })
        .collect()
}

/// A NURBS-lofted barrel: three circular sections of differing radius stacked
/// along +Z, skinned by `nurbs_loft` (degree 3 in both directions). Yields a
/// watertight solid with one `GeneralNurbsSurface` lateral face + two planar
/// caps.
fn build_nurbs_loft() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let n = 16;
    let sections = vec![ring(10.0, 0.0, n), ring(14.0, 12.0, n), ring(10.0, 24.0, n)];
    let s = nurbs_loft(&mut m, sections, NurbsLoftOptions::default()).expect("nurbs loft");
    (m, s)
}

// ───────────────────────────────────────────────────────────────────────────
// GEOMETRY — vertex positions
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn box_vertex_positions_survive() {
    let (model, _) = build_box();
    let rebuilt = roundtrip_geometry(&model).await;

    let before = sorted_positions(&model);
    let after = sorted_positions(&rebuilt);
    assert_eq!(
        before.len(),
        after.len(),
        "vertex count changed: {} -> {}",
        before.len(),
        after.len()
    );
    for (a, b) in before.iter().zip(after.iter()) {
        assert!(
            approx_pos(*a, *b, POS_EPS),
            "vertex position drifted: {a:?} -> {b:?}"
        );
    }
}

#[tokio::test]
async fn cylinder_vertex_positions_survive() {
    let (model, _) = build_cylinder();
    let rebuilt = roundtrip_geometry(&model).await;

    let before = sorted_positions(&model);
    let after = sorted_positions(&rebuilt);
    assert_eq!(before.len(), after.len(), "vertex count changed");
    for (a, b) in before.iter().zip(after.iter()) {
        assert!(approx_pos(*a, *b, POS_EPS), "vertex drift {a:?} -> {b:?}");
    }
}

#[tokio::test]
async fn bore_vertex_positions_survive() {
    let (model, _) = build_box_minus_cylinder_bore();
    let rebuilt = roundtrip_geometry(&model).await;

    let before = sorted_positions(&model);
    let after = sorted_positions(&rebuilt);
    assert_eq!(
        before.len(),
        after.len(),
        "bore vertex count changed: {} -> {}",
        before.len(),
        after.len()
    );
    for (a, b) in before.iter().zip(after.iter()) {
        assert!(
            approx_pos(*a, *b, POS_EPS),
            "bore vertex drift {a:?} -> {b:?}"
        );
    }
}

// ───────────────────────────────────────────────────────────────────────────
// GEOMETRY — topology counts (vertices / edges / loops / faces / shells / solids)
// ───────────────────────────────────────────────────────────────────────────

// Count LIVE topology, not raw store capacity. The kernel stores are
// generational-index Vecs: `remove()` tombstones a slot (stamps it
// INVALID_*_ID and leaves it in place to keep ids stable) rather than
// compacting, so `store.len()` reports the allocation high-water mark
// including dead slots, while `store.iter()` skips tombstones and yields
// only live entities. Since 2026-07-11 (commit d5633bc, #82 Slice 2) a
// Difference prunes its operand husks via
// `prune_boolean_orphan_topology`, which tombstones the operand-only
// faces/loops/edges/shells. The ROS snapshot serializes via `.iter()`
// (live only), so a faithful round trip legitimately drops the dead
// slots and comes back with `len() == live`. Fidelity is a property of
// the LIVE topology, so we measure it with `.iter().count()` on both
// sides — comparing raw `len()` would fail purely on the pre-round-trip
// operand-husk tombstones the format is correct to omit.
fn topo_counts(m: &BRepModel) -> (usize, usize, usize, usize, usize, usize) {
    (
        m.vertices.iter().count(),
        m.edges.iter().count(),
        m.loops.iter().count(),
        m.faces.iter().count(),
        m.shells.iter().count(),
        m.solids.iter().count(),
    )
}

#[tokio::test]
async fn box_topology_counts_survive() {
    let (model, _) = build_box();
    let rebuilt = roundtrip_geometry(&model).await;
    assert_eq!(
        topo_counts(&model),
        topo_counts(&rebuilt),
        "box topology (v,e,l,f,sh,so) changed across round trip"
    );
}

#[tokio::test]
async fn cylinder_topology_counts_survive() {
    let (model, _) = build_cylinder();
    let rebuilt = roundtrip_geometry(&model).await;
    assert_eq!(
        topo_counts(&model),
        topo_counts(&rebuilt),
        "cylinder topology changed across round trip"
    );
}

#[tokio::test]
async fn bore_topology_counts_survive() {
    let (model, _) = build_box_minus_cylinder_bore();
    let rebuilt = roundtrip_geometry(&model).await;
    assert_eq!(
        topo_counts(&model),
        topo_counts(&rebuilt),
        "bore topology changed across round trip"
    );
}

#[tokio::test]
async fn loft_topology_counts_survive() {
    let (model, _) = build_nurbs_loft();
    let rebuilt = roundtrip_geometry(&model).await;
    assert_eq!(
        topo_counts(&model),
        topo_counts(&rebuilt),
        "NURBS loft topology changed across round trip"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// GEOMETRY — surface TYPE + exact analytic params
// ───────────────────────────────────────────────────────────────────────────

/// Count surfaces by their kind tag so we can assert the analytic TYPE survived
/// (a plane must come back a plane, a cylinder a cylinder, …) rather than being
/// refit to a degree-1 B-spline.
fn surface_kind_histogram(m: &BRepModel) -> HashMap<&'static str, usize> {
    let mut h: HashMap<&'static str, usize> = HashMap::new();
    for (_, s) in m.surfaces.iter() {
        let any = s.as_any();
        let tag = if any
            .downcast_ref::<geometry_engine::primitives::surface::Plane>()
            .is_some()
        {
            "plane"
        } else if any
            .downcast_ref::<geometry_engine::primitives::surface::Cylinder>()
            .is_some()
        {
            "cylinder"
        } else if any
            .downcast_ref::<geometry_engine::primitives::surface::Sphere>()
            .is_some()
        {
            "sphere"
        } else if any
            .downcast_ref::<geometry_engine::primitives::surface::Cone>()
            .is_some()
        {
            "cone"
        } else if any
            .downcast_ref::<geometry_engine::primitives::surface::Torus>()
            .is_some()
        {
            "torus"
        } else if any.downcast_ref::<GeneralNurbsSurface>().is_some() {
            "nurbs"
        } else {
            "other"
        };
        *h.entry(tag).or_default() += 1;
    }
    h
}

#[tokio::test]
async fn box_surfaces_stay_planar() {
    let (model, _) = build_box();
    let rebuilt = roundtrip_geometry(&model).await;
    assert_eq!(
        surface_kind_histogram(&model),
        surface_kind_histogram(&rebuilt),
        "box surface kinds changed (planes refit to something else?)"
    );
    // Concretely: a box has 6 planar faces and nothing else.
    let h = surface_kind_histogram(&rebuilt);
    assert_eq!(h.get("plane").copied().unwrap_or(0), 6, "expected 6 planes");
}

#[tokio::test]
async fn cylinder_keeps_cylinder_and_plane_surfaces() {
    let (model, _) = build_cylinder();
    let rebuilt = roundtrip_geometry(&model).await;
    assert_eq!(
        surface_kind_histogram(&model),
        surface_kind_histogram(&rebuilt),
        "cylinder surface kinds changed across round trip"
    );
    let h = surface_kind_histogram(&rebuilt);
    assert_eq!(
        h.get("cylinder").copied().unwrap_or(0),
        1,
        "expected exactly one cylindrical surface"
    );
    assert_eq!(
        h.get("plane").copied().unwrap_or(0),
        2,
        "expected two planar caps"
    );
}

#[tokio::test]
async fn cylinder_surface_exact_params_survive() {
    let (model, _) = build_cylinder();
    let rebuilt = roundtrip_geometry(&model).await;

    let find_cyl = |m: &BRepModel| -> (Point3, Vector3, f64) {
        for (_, s) in m.surfaces.iter() {
            if let Some(c) = s
                .as_any()
                .downcast_ref::<geometry_engine::primitives::surface::Cylinder>()
            {
                return (c.origin, c.axis, c.radius);
            }
        }
        panic!("no cylinder surface found");
    };

    let (o0, a0, r0) = find_cyl(&model);
    let (o1, a1, r1) = find_cyl(&rebuilt);
    assert!(
        (r0 - r1).abs() <= POS_EPS,
        "cylinder radius drift {r0} -> {r1}"
    );
    assert!(
        approx_pos([o0.x, o0.y, o0.z], [o1.x, o1.y, o1.z], POS_EPS),
        "cylinder origin drift"
    );
    // Axis must remain parallel (sign/normalisation aside, the direction is
    // the geometric claim).
    let dot = (a0.x * a1.x + a0.y * a1.y + a0.z * a1.z).abs();
    let mag = (a0.x * a0.x + a0.y * a0.y + a0.z * a0.z).sqrt()
        * (a1.x * a1.x + a1.y * a1.y + a1.z * a1.z).sqrt();
    assert!(
        (dot / mag - 1.0).abs() <= 1e-9,
        "cylinder axis direction drift"
    );
}

#[tokio::test]
async fn bore_surfaces_keep_analytic_types() {
    let (model, _) = build_box_minus_cylinder_bore();
    let rebuilt = roundtrip_geometry(&model).await;
    // The bore wall is the (split) cylinder; the box walls stay planar. The
    // exact face count after a boolean is kernel-version-dependent, so we
    // assert the histogram is *identical* across the round trip and that NO
    // analytic face degraded to "other" (the degree-1 refit fallback).
    assert_eq!(
        surface_kind_histogram(&model),
        surface_kind_histogram(&rebuilt),
        "bore surface kinds changed across round trip"
    );
    let h = surface_kind_histogram(&rebuilt);
    assert_eq!(
        h.get("other").copied().unwrap_or(0),
        0,
        "an analytic bore face was refit to a degree-1 B-spline (lossy fallback)"
    );
    assert!(
        h.get("cylinder").copied().unwrap_or(0) >= 1,
        "the bore wall cylinder was lost"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// GEOMETRY — NURBS surface exact params (control points, knots, weights, degree)
// ───────────────────────────────────────────────────────────────────────────

fn first_nurbs_surface(m: &BRepModel) -> Option<GeneralNurbsSurface> {
    for (_, s) in m.surfaces.iter() {
        if let Some(n) = s.as_any().downcast_ref::<GeneralNurbsSurface>() {
            return Some(n.clone());
        }
    }
    None
}

#[tokio::test]
async fn loft_nurbs_surface_exact_params_survive() {
    let (model, _) = build_nurbs_loft();
    let rebuilt = roundtrip_geometry(&model).await;

    let before = first_nurbs_surface(&model).expect("loft built a NURBS surface");
    let after = first_nurbs_surface(&rebuilt)
        .expect("NURBS surface must survive as a NURBS surface, not a refit B-spline");

    assert_eq!(
        before.nurbs.degree_u, after.nurbs.degree_u,
        "degree_u changed"
    );
    assert_eq!(
        before.nurbs.degree_v, after.nurbs.degree_v,
        "degree_v changed"
    );

    let ku0 = before.nurbs.knots_u.values().to_vec();
    let ku1 = after.nurbs.knots_u.values().to_vec();
    assert_eq!(ku0.len(), ku1.len(), "knots_u length changed");
    for (a, b) in ku0.iter().zip(ku1.iter()) {
        assert!((a - b).abs() <= POS_EPS, "knots_u value drift {a} -> {b}");
    }

    let kv0 = before.nurbs.knots_v.values().to_vec();
    let kv1 = after.nurbs.knots_v.values().to_vec();
    assert_eq!(kv0.len(), kv1.len(), "knots_v length changed");
    for (a, b) in kv0.iter().zip(kv1.iter()) {
        assert!((a - b).abs() <= POS_EPS, "knots_v value drift {a} -> {b}");
    }

    // Control-point grid: same shape, same coordinates.
    assert_eq!(
        before.nurbs.control_points.len(),
        after.nurbs.control_points.len(),
        "control-point row count changed"
    );
    for (rb, ra) in before
        .nurbs
        .control_points
        .iter()
        .zip(after.nurbs.control_points.iter())
    {
        assert_eq!(rb.len(), ra.len(), "control-point row width changed");
        for (pb, pa) in rb.iter().zip(ra.iter()) {
            assert!(
                approx_pos([pb.x, pb.y, pb.z], [pa.x, pa.y, pa.z], POS_EPS),
                "control point drift {:?} -> {:?}",
                [pb.x, pb.y, pb.z],
                [pa.x, pa.y, pa.z]
            );
        }
    }

    // Weights: identical grid of values.
    assert_eq!(
        before.nurbs.weights.len(),
        after.nurbs.weights.len(),
        "weight row count changed"
    );
    for (rb, ra) in before.nurbs.weights.iter().zip(after.nurbs.weights.iter()) {
        assert_eq!(rb.len(), ra.len(), "weight row width changed");
        for (wb, wa) in rb.iter().zip(ra.iter()) {
            assert!((wb - wa).abs() <= POS_EPS, "weight drift {wb} -> {wa}");
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// GEOMETRY — curve TYPE + exact params (box edges are lines; bore has circles)
// ───────────────────────────────────────────────────────────────────────────

fn curve_kind_histogram(m: &BRepModel) -> HashMap<&'static str, usize> {
    use geometry_engine::primitives::curve::{Arc, Circle, Line};
    let mut h: HashMap<&'static str, usize> = HashMap::new();
    for cid in 0..m.curves.len() as u32 {
        let Some(c) = m.curves.get(cid) else { continue };
        let any = c.as_any();
        let tag = if any.downcast_ref::<Line>().is_some() {
            "line"
        } else if any.downcast_ref::<Circle>().is_some() {
            "circle"
        } else if any.downcast_ref::<Arc>().is_some() {
            "arc"
        } else if any.downcast_ref::<NurbsCurve>().is_some() {
            "nurbs"
        } else {
            "other"
        };
        *h.entry(tag).or_default() += 1;
    }
    h
}

#[tokio::test]
async fn box_edges_stay_lines() {
    let (model, _) = build_box();
    let rebuilt = roundtrip_geometry(&model).await;
    assert_eq!(
        curve_kind_histogram(&model),
        curve_kind_histogram(&rebuilt),
        "box edge curve kinds changed across round trip"
    );
    let h = curve_kind_histogram(&rebuilt);
    assert_eq!(
        h.get("line").copied().unwrap_or(0),
        12,
        "expected 12 line edges"
    );
}

#[tokio::test]
async fn cylinder_circles_survive_as_circles() {
    // A cylinder's rim edges are full circles. The snapshot has a dedicated
    // `Circle` arm, so the curve TYPE must survive (not degrade to a sampled
    // polyline B-spline). NOTE: the writer stores Circle/Arc by center/normal/
    // radius; a closed full circle has start==end, so the *count* of distinct
    // curves can collapse — we assert the kind histogram is preserved and that
    // no curve became an "other".
    let (model, _) = build_cylinder();
    let rebuilt = roundtrip_geometry(&model).await;
    assert_eq!(
        curve_kind_histogram(&model),
        curve_kind_histogram(&rebuilt),
        "cylinder curve kinds changed across round trip"
    );
    let h = curve_kind_histogram(&rebuilt);
    assert_eq!(
        h.get("other").copied().unwrap_or(0),
        0,
        "a cylinder curve degraded to a polyline fallback"
    );
}

#[tokio::test]
async fn box_edge_endpoints_survive() {
    // Each edge's endpoint *positions* (resolved through its start/end vertex)
    // must be unchanged. This proves edge→vertex wiring survived, independent
    // of vertex id renumbering.
    let (model, _) = build_box();
    let rebuilt = roundtrip_geometry(&model).await;

    let endpoint_set = |m: &BRepModel| -> Vec<([f64; 3], [f64; 3])> {
        let mut out = Vec::new();
        for (_, e) in m.edges.iter() {
            let a = m.vertices.get(e.start_vertex).map(|v| v.position);
            let b = m.vertices.get(e.end_vertex).map(|v| v.position);
            if let (Some(a), Some(b)) = (a, b) {
                // Canonicalise endpoint order so a forward/backward flip does
                // not register as a difference.
                let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                out.push((lo, hi));
            }
        }
        out.sort_by(|x, y| {
            x.0.partial_cmp(&y.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| x.1.partial_cmp(&y.1).unwrap_or(std::cmp::Ordering::Equal))
        });
        out
    };

    let before = endpoint_set(&model);
    let after = endpoint_set(&rebuilt);
    assert_eq!(before.len(), after.len(), "edge count changed");
    for ((a0, b0), (a1, b1)) in before.iter().zip(after.iter()) {
        assert!(
            approx_pos(*a0, *a1, POS_EPS) && approx_pos(*b0, *b1, POS_EPS),
            "edge endpoint drift ({a0:?},{b0:?}) -> ({a1:?},{b1:?})"
        );
    }
}

// ───────────────────────────────────────────────────────────────────────────
// GEOMETRY — edge parameter ranges (KNOWN GAP)
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "GAP: snapshot reconstructs every edge with ParameterRange::unit() \
            (ros_snapshot.rs:549); the writer never stores Edge::param_range. \
            Slice: add param_range to EdgeData + round-trip it."]
async fn edge_param_ranges_survive() {
    let (model, _) = build_cylinder();
    let rebuilt = roundtrip_geometry(&model).await;

    // Collect (start,end) param-range pairs keyed by canonicalised endpoint
    // positions so the comparison is order-independent.
    let ranges = |m: &BRepModel| -> Vec<(f64, f64)> {
        let mut out: Vec<(f64, f64)> = m
            .edges
            .iter()
            .map(|(_, e)| (e.param_range.start, e.param_range.end))
            .collect();
        out.sort_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        });
        out
    };

    let before = ranges(&model);
    let after = ranges(&rebuilt);
    assert_eq!(before.len(), after.len(), "edge count changed");
    for ((s0, e0), (s1, e1)) in before.iter().zip(after.iter()) {
        assert!(
            (s0 - s1).abs() <= POS_EPS && (e0 - e1).abs() <= POS_EPS,
            "param range drift ({s0},{e0}) -> ({s1},{e1})"
        );
    }
}

// ───────────────────────────────────────────────────────────────────────────
// GEOMETRY — watertightness after round trip
// ───────────────────────────────────────────────────────────────────────────

fn assert_watertight(model: &BRepModel, solid: SolidId, ctx: &str) {
    let r = manifold_report(model, solid, CHORD, WELD_EPS)
        .unwrap_or_else(|| panic!("{ctx}: solid {solid} produced no mesh"));
    assert_eq!(
        r.boundary_edges, 0,
        "{ctx}: {} open (boundary) edges — not watertight ({r:?})",
        r.boundary_edges
    );
    assert_eq!(
        r.nonmanifold_edges, 0,
        "{ctx}: {} non-manifold edges ({r:?})",
        r.nonmanifold_edges
    );
}

#[tokio::test]
async fn box_watertight_after_roundtrip() {
    let (model, solid) = build_box();
    assert_watertight(&model, solid, "box (pre-roundtrip baseline)");
    let rebuilt = roundtrip_geometry(&model).await;
    assert_watertight(&rebuilt, last_solid(&rebuilt), "box (post-roundtrip)");
}

#[tokio::test]
async fn cylinder_watertight_after_roundtrip() {
    let (model, solid) = build_cylinder();
    assert_watertight(&model, solid, "cylinder (pre-roundtrip baseline)");
    let rebuilt = roundtrip_geometry(&model).await;
    assert_watertight(&rebuilt, last_solid(&rebuilt), "cylinder (post-roundtrip)");
}

#[tokio::test]
async fn bore_is_watertight_when_built() {
    // Baseline (live): the kernel builds a watertight box−cylinder bore. This
    // is the precondition for the round-trip gap below to be meaningful — the
    // tear is introduced by the format, not by the boolean.
    let (model, solid) = build_box_minus_cylinder_bore();
    assert_watertight(&model, solid, "bore (as built)");
}

#[tokio::test]
#[ignore = "GAP: a watertight box−cylinder bore re-imports with ~208 open edges \
            (two disconnected mesh components). The snapshot drops pcurves and \
            rebuilds every edge with ParameterRange::unit(), so the boolean-split \
            cylinder-wall faces lose the shared-edge wiring that made the bore \
            watertight. Slice: round-trip edge param_range + pcurves so split \
            faces re-weld. (See edge_param_ranges_survive — same root cause.)"]
async fn bore_watertight_after_roundtrip() {
    let (model, solid) = build_box_minus_cylinder_bore();
    assert_watertight(&model, solid, "bore (pre-roundtrip baseline)");
    let rebuilt = roundtrip_geometry(&model).await;
    assert_watertight(&rebuilt, last_solid(&rebuilt), "bore (post-roundtrip)");
}

#[tokio::test]
async fn loft_watertight_after_roundtrip() {
    let (model, solid) = build_nurbs_loft();
    assert_watertight(&model, solid, "loft (pre-roundtrip baseline)");
    let rebuilt = roundtrip_geometry(&model).await;
    assert_watertight(&rebuilt, last_solid(&rebuilt), "loft (post-roundtrip)");
}

// ───────────────────────────────────────────────────────────────────────────
// TIMELINE / PROVENANCE
// ───────────────────────────────────────────────────────────────────────────

fn synth_event(branch: BranchId, seq: u64) -> TimelineEvent {
    TimelineEvent {
        id: EventId::new(),
        sequence_number: seq,
        timestamp: chrono::Utc::now(),
        author: Author::System,
        operation: Operation::Generic {
            command_type: "create_box_3d".to_string(),
            // Shape mirrors what the recorder bridge emits and `replay::
            // dispatch_generic` expects: params.Create3D.parameters.{w,h,d}.
            parameters: serde_json::json!({
                "params": {
                    "Create3D": {
                        "primitive_type": "box",
                        "parameters": { "width": 40.0, "height": 40.0, "depth": 20.0 },
                        "timestamp": 0
                    }
                },
                "inputs": [],
                "outputs": [1]
            }),
        },
        inputs: OperationInputs::default(),
        outputs: OperationOutputs::default(),
        metadata: EventMetadata {
            description: Some(format!("synthetic event {seq}")),
            branch_id: branch,
            tags: vec!["harness".to_string()],
            properties: Default::default(),
        },
    }
}

fn synth_branch(id: BranchId, name: &str) -> BranchManifest {
    BranchManifest {
        id,
        name: name.to_string(),
        parent: None,
        fork_point: timeline_engine::ForkPoint {
            branch_id: id,
            event_index: 0,
            timestamp: chrono::Utc::now(),
        },
        state: BranchState::Active,
        metadata: BranchMetadata {
            created_by: Author::System,
            created_at: chrono::Utc::now(),
            purpose: BranchPurpose::UserExploration {
                description: "harness branch".to_string(),
            },
            ai_context: None,
            checkpoints: vec![],
        },
        protected: id == BranchId::main(),
        hidden: false,
    }
}

#[tokio::test]
async fn timeline_events_and_branches_survive() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("hist.ros");

    let main = BranchId::main();
    let alt = BranchId::new();
    let events = vec![
        synth_event(main, 0),
        synth_event(main, 1),
        synth_event(alt, 0),
    ];
    let branches = vec![
        synth_branch(main, "main"),
        synth_branch(alt, "experimental"),
    ];
    let history = HistData::new(branches, events.clone());

    let (model, _) = build_box();
    export_brep_to_ros(
        RosExportPayload {
            model: &model,
            history: Some(history),
            aipr: None,
        },
        &path,
        RosExportOptions::default(),
    )
    .await
    .expect("export with timeline");

    let imported = import_ros(&path, None).await.expect("import");

    assert_eq!(imported.timeline.len(), 3, "event count changed");
    assert_eq!(imported.branches.len(), 2, "branch count changed");

    // Event ids + sequence numbers survive exactly.
    for original in &events {
        let found = imported
            .timeline
            .iter()
            .find(|e| e.id == original.id)
            .unwrap_or_else(|| panic!("event {} missing after round trip", original.id));
        assert_eq!(
            found.sequence_number, original.sequence_number,
            "event sequence number changed"
        );
        assert_eq!(
            found.metadata.branch_id, original.metadata.branch_id,
            "event branch_id changed"
        );
    }

    // Branch identities survive.
    assert!(imported
        .branches
        .iter()
        .any(|b| b.id == main && b.name == "main" && b.protected));
    assert!(imported
        .branches
        .iter()
        .any(|b| b.id == alt && b.name == "experimental"));
}

#[tokio::test]
async fn replay_of_imported_timeline_reconstructs_model() {
    // Build a box via the timeline-equivalent Generic event, export the events
    // (NO geom snapshot), re-import, and replay — the rebuilt model must carry
    // the same vertex/face/solid counts a directly-built box has.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("replay.ros");

    let main = BranchId::main();
    let events = vec![synth_event(main, 0)];
    let history = HistData::new(vec![synth_branch(main, "main")], events);

    let empty = BRepModel::new();
    export_brep_to_ros(
        RosExportPayload {
            model: &empty,
            history: Some(history),
            aipr: None,
        },
        &path,
        RosExportOptions {
            include_snapshot: false,
            ..RosExportOptions::default()
        },
    )
    .await
    .expect("export events-only");

    let imported = import_ros(&path, None).await.expect("import");
    assert!(
        imported.snapshot.is_none(),
        "events-only file must omit GEOM"
    );

    let mut rebuilt = BRepModel::new();
    let outcome = timeline_engine::rebuild_model_from_events(&mut rebuilt, &imported.timeline);
    assert_eq!(
        outcome.events_skipped, 0,
        "replay skipped events: {outcome:?}"
    );
    assert_eq!(
        outcome.events_applied, 1,
        "replay applied wrong event count"
    );

    // A box: 8 vertices, 6 faces, 1 solid.
    let (direct, _) = build_box();
    assert_eq!(
        rebuilt.vertices.len(),
        direct.vertices.len(),
        "replayed box vertex count != directly built box"
    );
    assert_eq!(
        rebuilt.faces.len(),
        direct.faces.len(),
        "replayed box face count != directly built box"
    );
    assert_eq!(
        rebuilt.solids.len(),
        direct.solids.len(),
        "replayed box solid count != directly built box"
    );
}

#[tokio::test]
async fn ai_provenance_commands_survive() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("prov.ros");

    let mut tracker =
        AICommandTracker::new(TrackingLevel::Forensic, PrivacySettings::default(), None);
    tracker.start_session(Some("harness-user".to_string()));
    tracker
        .track_command(
            CommandType::Create,
            [0u8; 32],
            1,
            "create a 40mm cube",
            "created box solid #0",
            &["solid:0".to_string()],
            0.97,
            12,
            None,
        )
        .expect("track command");

    let (model, _) = build_box();
    export_brep_to_ros(
        RosExportPayload {
            model: &model,
            history: None,
            aipr: Some(tracker),
        },
        &path,
        RosExportOptions {
            tracking_level: TrackingLevel::Forensic,
            ..RosExportOptions::default()
        },
    )
    .await
    .expect("export with provenance");

    let imported = import_ros(&path, None).await.expect("import");
    assert_eq!(
        imported.aipr.tracking_level,
        TrackingLevel::Forensic,
        "tracking level changed"
    );
    assert_eq!(
        imported.aipr.commands.len(),
        1,
        "AI command count changed across round trip"
    );
    let cmd = &imported.aipr.commands[0];
    assert_eq!(
        cmd.affected_objects,
        vec!["solid:0".to_string()],
        "affected_objects changed"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// SEMANTIC SIDECARS (KNOWN GAPS — the snapshot drops every sidecar)
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "GAP: BRepSnapshot has no field for `labels`; the LabelSidecar is \
            dropped on export (ros_snapshot.rs BRepSnapshot struct). Slice: add \
            a LABELS sub-chunk (or snapshot field) carrying LabelSidecar."]
async fn labels_survive() {
    use geometry_engine::labels::{Fingerprint, Label, LabelAssertion, LabelKind, LabelTarget};
    use geometry_engine::primitives::persistent_id::PersistentId;

    let (mut model, _) = build_box();

    // Pin a label onto a face by a (stable) fingerprint assertion.
    let pid = PersistentId::root(b"harness-face-seed");
    let label = Label {
        target: LabelTarget::Entity {
            kind: LabelKind::Face,
            pid,
        },
        assertion: Some(LabelAssertion::Fingerprint(Fingerprint {
            kind: LabelKind::Face,
            position: [0.0, 0.0, 10.0],
            normal: Some([0.0, 0.0, 1.0]),
            radius: None,
            size: Some(1600.0),
        })),
        description: Some("top face".to_string()),
    };
    model
        .labels
        .attach("top_face", label)
        .expect("attach label");
    assert_eq!(model.labels.len(), 1);

    let rebuilt = roundtrip_geometry(&model).await;
    assert_eq!(
        rebuilt.labels.len(),
        1,
        "label was dropped across the round trip"
    );
    assert!(
        rebuilt.labels.get("top_face").is_some(),
        "named label 'top_face' missing after round trip"
    );
}

#[tokio::test]
#[ignore = "GAP: BRepSnapshot drops solid_provenance; export never serializes \
            the SolidProvenance sidecar. Slice: carry provenance in GEOM (or a \
            PROV-adjacent sub-chunk) keyed by solid."]
async fn solid_provenance_survives() {
    use geometry_engine::primitives::provenance::{OperationKind, SolidProvenance};

    let (mut model, solid) = build_box_minus_cylinder_bore();
    model
        .solid_provenance
        .insert(solid, SolidProvenance::new(OperationKind::Boolean, vec![]));
    assert!(model.solid_provenance.contains_key(&solid));

    let rebuilt = roundtrip_geometry(&model).await;
    // The result solid is renumbered on import; assert SOME solid carries the
    // Boolean provenance.
    assert!(
        rebuilt
            .solid_provenance
            .values()
            .any(|p| p.created_by == OperationKind::Boolean),
        "solid provenance (Boolean) was dropped across the round trip"
    );
}

#[tokio::test]
#[ignore = "GAP: BRepSnapshot drops the datum store; the seeded canonical datums \
            and any user datums are not serialized. Slice: serialize DatumStore \
            into GEOM (or a DATM sub-chunk)."]
async fn datums_survive() {
    // A fresh model is seeded with the canonical seven datums. They must
    // survive a round trip (today the rebuilt model re-seeds its own, so a
    // count check alone is insufficient — assert the seeded datums are present
    // AND that a user-authored datum survives once the slice lands).
    let (model, _) = build_box();
    let before = model.datums.len();
    assert!(before >= 7, "expected the canonical seven datums seeded");

    let rebuilt = roundtrip_geometry(&model).await;
    // Once datums round-trip, the COUNT must match exactly (re-seeding +
    // serialized datums would double-count, so the slice must replace, not add).
    assert_eq!(
        rebuilt.datums.len(),
        before,
        "datum count changed across the round trip"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// DETERMINISM
// ───────────────────────────────────────────────────────────────────────────

/// Read a `.ros` file and return only its chunk *payload* bytes (everything
/// between the 128-byte header and the chunk index), stripping the header and
/// trailing index whose timestamps/offsets are wall-clock dependent. The HIST
/// chunk also embeds a `written_at` timestamp and the META chunk a `created`
/// field, so a true byte-for-byte determinism check is only meaningful over a
/// payload with NO embedded clocks — which is the geometry-only GEOM chunk.
async fn geom_chunk_bytes(path: &std::path::Path) -> Vec<u8> {
    use ros_format::{ChunkType, FileHeader};
    use std::io::Cursor;
    use tokio::io::AsyncReadExt;

    let mut bytes = Vec::new();
    tokio::fs::File::open(path)
        .await
        .unwrap()
        .read_to_end(&mut bytes)
        .await
        .unwrap();
    let mut cursor = Cursor::new(bytes.clone());
    let header = FileHeader::read_from(&mut cursor).expect("header");
    let table = ros_format::chunk::ChunkTable::read_from(
        &mut cursor,
        header.index_offset,
        header.index_entry_count,
    )
    .expect("chunk table");
    let entry = table.find_by_type(ChunkType::GEOM).expect("GEOM present");
    let start = entry.offset as usize;
    let end = start + entry.uncompressed_size as usize;
    bytes[start..end].to_vec()
}

#[tokio::test]
async fn geom_chunk_is_deterministic() {
    // The GEOM (geometry snapshot) chunk carries no wall-clock fields except
    // `BRepMetadata.created_at/modified_at`, which DO use current_time_ms().
    // Those two timestamps are the only non-determinism in the geometry chunk;
    // every coordinate, id mapping and topology reference is content-derived.
    //
    // We therefore write the same model twice and assert the GEOM payloads are
    // identical EXCEPT in the metadata timestamp region. Concretely: the byte
    // lengths must match (structure is identical) and the payloads must be
    // equal once we account for the timestamp drift being a fixed-width field.
    let (model, _) = build_box();

    let dir = TempDir::new().unwrap();
    let p1 = dir.path().join("a.ros");
    let p2 = dir.path().join("b.ros");

    for p in [&p1, &p2] {
        export_brep_to_ros(
            RosExportPayload {
                model: &model,
                history: None,
                aipr: None,
            },
            p,
            RosExportOptions::default(),
        )
        .await
        .expect("export");
    }

    let g1 = geom_chunk_bytes(&p1).await;
    let g2 = geom_chunk_bytes(&p2).await;

    // Structural determinism: identical serialized length every time.
    assert_eq!(
        g1.len(),
        g2.len(),
        "GEOM chunk length differs between two writes of the same model"
    );

    // Content determinism: the number of differing bytes must be small and
    // bounded — only the two u64 metadata timestamps (created_at, modified_at)
    // may differ. 16 bytes is the hard upper bound; in practice writes within
    // the same millisecond differ in zero bytes.
    let differing = g1.iter().zip(g2.iter()).filter(|(a, b)| a != b).count();
    assert!(
        differing <= 16,
        "GEOM chunk differs in {differing} bytes between two identical writes \
         (only the 2×u64 metadata timestamps may differ); the snapshot is not \
         content-deterministic"
    );
}

#[tokio::test]
#[ignore = "GAP: the writer has no fixed-timestamp option — META.created and \
            BRepMetadata.created_at/modified_at use current_time_ms(), and HIST \
            embeds written_at via Utc::now(). A byte-for-byte determinism gate \
            needs a RosExportOptions::fixed_timestamp (or a clock injection \
            seam). Slice: thread a deterministic clock through the writer."]
async fn whole_file_is_byte_identical() {
    let (model, _) = build_box();

    let dir = TempDir::new().unwrap();
    let p1 = dir.path().join("a.ros");
    let p2 = dir.path().join("b.ros");

    for p in [&p1, &p2] {
        export_brep_to_ros(
            RosExportPayload {
                model: &model,
                history: None,
                aipr: None,
            },
            p,
            RosExportOptions::default(),
        )
        .await
        .expect("export");
    }

    let b1 = tokio::fs::read(&p1).await.unwrap();
    let b2 = tokio::fs::read(&p2).await.unwrap();
    assert_eq!(
        b1, b2,
        "two writes of the same model are not byte-identical"
    );
}
