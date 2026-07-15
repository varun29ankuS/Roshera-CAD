// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Regression tests for the fillet / chamfer 4-face topology surgery
//! (`operations::edge_blend_topology::splice_blend_edge`) wired through
//! `update_adjacent_faces` (fillet) and `update_adjacent_faces_for_chamfer`
//! (chamfer).
//!
//! These tests pin the watertight-B-Rep contract that the blend pipeline
//! is required to honour after a successful operation:
//!
//!   * Euler `V - E + F = 2` for the closed genus-0 box,
//!     including the deltas introduced by the surgery
//!     (`+2 V, +3 E, +1 F` per blended edge).
//!   * Every edge appears in **exactly two** loops across the model
//!     (no boundary edges).
//!   * The full `validate_model_enhanced(Standard)` pass succeeds — the
//!     same gate the public `fillet_edges` / `chamfer_edges` entry points
//!     enforce when `validate_result = true`.
//!
//! Earlier kernel revisions left the original edge in a face loop or
//! failed to add the trim/cap edges, producing boundary edges (V-E+F = 4
//! for chamfer, validation failures naming edges 12/13/15 for fillet).
//! These cases are the regression net.

use std::collections::HashMap;

use geometry_engine::math::Tolerance;
use geometry_engine::operations::chamfer::{ChamferType, PropagationMode as ChamferPropagation};
use geometry_engine::operations::fillet::{FilletType, PropagationMode as FilletPropagation};
use geometry_engine::operations::{chamfer_edges, fillet_edges, ChamferOptions, FilletOptions};
use geometry_engine::primitives::curve::Arc;
use geometry_engine::primitives::edge::{EdgeId, EdgeOrientation};
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::validation::{validate_model_enhanced, ValidationLevel};

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

fn expect_solid(geom: GeometryId) -> SolidId {
    match geom {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    expect_solid(
        builder
            .create_box_3d(w, h, d)
            .expect("box creation should succeed"),
    )
}

/// Count how many distinct loops reference each edge across every face
/// in the model. A watertight closed shell has exactly 2 references
/// per edge.
fn edge_loop_usage(model: &BRepModel) -> HashMap<EdgeId, usize> {
    let mut usage: HashMap<EdgeId, usize> = HashMap::new();
    for (_face_id, face) in model.faces.iter() {
        for loop_id in face.all_loops() {
            if let Some(loop_ref) = model.loops.get(loop_id) {
                for &edge_id in &loop_ref.edges {
                    *usage.entry(edge_id).or_insert(0) += 1;
                }
            }
        }
    }
    usage
}

/// Assert: every edge referenced by a face loop appears exactly twice
/// (i.e., the shell is closed — no boundary edges).
fn assert_no_boundary_edges(model: &BRepModel, context: &str) {
    let usage = edge_loop_usage(model);
    let bad: Vec<(EdgeId, usize)> = usage
        .iter()
        .filter(|(_, &count)| count != 2)
        .map(|(&e, &c)| (e, c))
        .collect();
    assert!(
        bad.is_empty(),
        "{}: expected every edge to appear in exactly 2 loops, but found \
         boundary/over-shared edges: {:?}",
        context,
        bad,
    );
}

/// Assert Euler `V - E + F = 2` over the live entities the model's
/// loops actually touch (edges referenced from a face loop count
/// once each; vertices are counted from those edges).
///
/// We compute V/E/F from the *referenced* set rather than the raw
/// store sizes because an in-flight surgery may legitimately leave an
/// orphaned-but-not-yet-removed edge in the EdgeStore for a few
/// instructions. After the public `fillet_edges` / `chamfer_edges`
/// entry points return, no such orphan should remain — but using the
/// referenced set is the strictly stronger statement and the one
/// boundary-edge bugs actually violate.
fn assert_euler_two(model: &BRepModel, context: &str) {
    use std::collections::HashSet;

    let mut edges: HashSet<EdgeId> = HashSet::new();
    let mut faces: HashSet<u32> = HashSet::new();
    for (face_id, face) in model.faces.iter() {
        faces.insert(face_id);
        for loop_id in face.all_loops() {
            if let Some(loop_ref) = model.loops.get(loop_id) {
                for &edge_id in &loop_ref.edges {
                    edges.insert(edge_id);
                }
            }
        }
    }

    let mut vertices: HashSet<u32> = HashSet::new();
    for &edge_id in &edges {
        if let Some(edge) = model.edges.get(edge_id) {
            vertices.insert(edge.start_vertex);
            vertices.insert(edge.end_vertex);
        }
    }

    let v = vertices.len() as i64;
    let e = edges.len() as i64;
    let f = faces.len() as i64;
    let chi = v - e + f;
    assert_eq!(
        chi, 2,
        "{}: expected V - E + F = 2 (closed genus-0 manifold), \
         got V={} E={} F={} → χ={}",
        context, v, e, f, chi,
    );
}

/// Assert: for every edge in the model, the underlying curve, when
/// evaluated at the (trimmed) `param_range` endpoints, lands on the
/// labelled start/end vertex positions to within `geom_tol`.
///
/// This is the geometric counterpart to `assert_no_boundary_edges`:
/// topology can be perfectly stitched (every edge in exactly two
/// loops, χ = 2) while the geometry of an edge still traces from a
/// corner that the topology has already surgically removed. That
/// mismatch is exactly the "original sharp edge still drawn after
/// fillet" pathology — the tessellator samples
/// `curve.evaluate(t) for t ∈ param_range`, not the vertex labels,
/// so an edge whose `param_range` was never re-trimmed against the
/// new vertex still draws the full original segment from corner to
/// corner.
///
/// `EdgeOrientation::Forward` ↔ `start_vertex` at `param_range.start`;
/// `Backward` inverts that mapping.
fn assert_edges_geometrically_coherent(model: &BRepModel, context: &str) {
    let geom_tol = 1.0e-6_f64;
    let mut bad: Vec<String> = Vec::new();

    for (edge_id, edge) in model.edges.iter() {
        let (low_param_vertex, high_param_vertex) = match edge.orientation {
            EdgeOrientation::Forward => (edge.start_vertex, edge.end_vertex),
            EdgeOrientation::Backward => (edge.end_vertex, edge.start_vertex),
        };

        let v_low = match model.vertices.get(low_param_vertex) {
            Some(v) => v.position,
            None => {
                bad.push(format!(
                    "edge {edge_id}: low-param vertex {low_param_vertex} missing from VertexStore",
                ));
                continue;
            }
        };
        let v_high = match model.vertices.get(high_param_vertex) {
            Some(v) => v.position,
            None => {
                bad.push(format!(
                    "edge {edge_id}: high-param vertex {high_param_vertex} missing from VertexStore",
                ));
                continue;
            }
        };

        let curve = match model.curves.get(edge.curve_id) {
            Some(c) => c,
            None => {
                bad.push(format!(
                    "edge {edge_id}: curve {} missing from CurveStore",
                    edge.curve_id,
                ));
                continue;
            }
        };

        let p_low = match curve.point_at(edge.param_range.start) {
            Ok(p) => p,
            Err(err) => {
                bad.push(format!(
                    "edge {edge_id}: curve.point_at(param_range.start = {}) failed: {err:?}",
                    edge.param_range.start,
                ));
                continue;
            }
        };
        let p_high = match curve.point_at(edge.param_range.end) {
            Ok(p) => p,
            Err(err) => {
                bad.push(format!(
                    "edge {edge_id}: curve.point_at(param_range.end = {}) failed: {err:?}",
                    edge.param_range.end,
                ));
                continue;
            }
        };

        let d_low = ((p_low.x - v_low[0]).powi(2)
            + (p_low.y - v_low[1]).powi(2)
            + (p_low.z - v_low[2]).powi(2))
        .sqrt();
        let d_high = ((p_high.x - v_high[0]).powi(2)
            + (p_high.y - v_high[1]).powi(2)
            + (p_high.z - v_high[2]).powi(2))
        .sqrt();
        if d_low > geom_tol {
            bad.push(format!(
                "edge {edge_id}: curve(t={:.6}) = ({:.6}, {:.6}, {:.6}) but \
                 labelled low-param vertex {low_param_vertex} is at \
                 ({:.6}, {:.6}, {:.6}) — Δ = {:.3e}",
                edge.param_range.start,
                p_low.x,
                p_low.y,
                p_low.z,
                v_low[0],
                v_low[1],
                v_low[2],
                d_low,
            ));
        }
        if d_high > geom_tol {
            bad.push(format!(
                "edge {edge_id}: curve(t={:.6}) = ({:.6}, {:.6}, {:.6}) but \
                 labelled high-param vertex {high_param_vertex} is at \
                 ({:.6}, {:.6}, {:.6}) — Δ = {:.3e}",
                edge.param_range.end,
                p_high.x,
                p_high.y,
                p_high.z,
                v_high[0],
                v_high[1],
                v_high[2],
                d_high,
            ));
        }
    }

    assert!(
        bad.is_empty(),
        "{}: {} edge(s) have curve geometry that doesn't terminate at \
         their labelled vertices — the original-edges-not-removed \
         pathology (rewire_edge_vertex must re-trim param_range, not \
         just swap vertex IDs):\n  {}",
        context,
        bad.len(),
        bad.join("\n  "),
    );
}

/// Assert that each fillet blend face has **exactly two** cap edges
/// whose curve is an `Arc`, each with radius and sweep matching the
/// expected fillet cross-section (radius `expected_radius`, sweep
/// `±π/2` for a convex 90° box edge).
///
/// This pins the chord-vs-arc invariant: the original 4-sided fillet
/// pipeline used straight `Line` chords for cap edges, leaving a
/// triangular gap between the loop boundary and the cylinder's
/// natural cross-section that surfaced as see-through faces in the
/// viewport. Replacing the chord with the cylinder cross-section
/// arc closed that gap. A future regression that re-introduces
/// `Line::new(...)` caps would still pass `assert_edges_geometrically_coherent`
/// (the chord endpoints land on the same vertices the arc does), so
/// we need a positive type assertion to catch it.
fn assert_fillet_cap_edges_are_arcs(
    model: &BRepModel,
    blend_faces: &[FaceId],
    expected_radius: f64,
    context: &str,
) {
    use std::f64::consts::PI;
    // Tolerances:
    //   Radius is set exactly by the caller, so a tight 1e-6 catches
    //   any drift introduced by the construction path.
    //   Sweep angle comes from atan2 against rolling-ball trim
    //   endpoints; for an orthogonal box edge the analytical value is
    //   exactly π/2. Allow 1e-3 rad (~0.06°) for floating-point slop
    //   in normal-projection / bisector arithmetic upstream.
    let radius_tol = 1.0e-6_f64;
    let sweep_tol = 1.0e-3_f64;
    let expected_sweep = PI / 2.0;

    for &face_id in blend_faces {
        let face = model
            .faces
            .get(face_id)
            .unwrap_or_else(|| panic!("{}: blend face {} missing", context, face_id));
        let outer_loop = model.loops.get(face.outer_loop).unwrap_or_else(|| {
            panic!(
                "{}: blend face {} outer loop {} missing",
                context, face_id, face.outer_loop
            )
        });

        let mut arc_edges: Vec<(EdgeId, &Arc)> = Vec::new();
        for &edge_id in &outer_loop.edges {
            let edge = model
                .edges
                .get(edge_id)
                .unwrap_or_else(|| panic!("{}: edge {} missing", context, edge_id));
            let curve = model.curves.get(edge.curve_id).unwrap_or_else(|| {
                panic!(
                    "{}: edge {} curve {} missing",
                    context, edge_id, edge.curve_id
                )
            });
            if let Some(arc) = curve.as_any().downcast_ref::<Arc>() {
                arc_edges.push((edge_id, arc));
            }
        }

        assert_eq!(
            arc_edges.len(),
            2,
            "{}: blend face {} should have exactly 2 cap edges typed as Arc \
             (one at each end of the original edge), found {} — chord caps \
             would surface as 0 Arc edges, indicating the fix in fillet.rs \
             has been reverted",
            context,
            face_id,
            arc_edges.len(),
        );

        for (edge_id, arc) in arc_edges {
            assert!(
                (arc.radius - expected_radius).abs() < radius_tol,
                "{}: cap edge {} radius = {:.9}, expected {:.9} (Δ = {:.3e})",
                context,
                edge_id,
                arc.radius,
                expected_radius,
                (arc.radius - expected_radius).abs(),
            );
            let sweep_err = (arc.sweep_angle.abs() - expected_sweep).abs();
            assert!(
                sweep_err < sweep_tol,
                "{}: cap edge {} sweep = {:.6} rad, expected ±{:.6} rad \
                 (Δ from |π/2| = {:.3e})",
                context,
                edge_id,
                arc.sweep_angle,
                expected_sweep,
                sweep_err,
            );
        }
    }
}

/// Assert the kernel's Standard-level validation passes — same gate
/// `fillet_edges` / `chamfer_edges` use internally when
/// `validate_result = true`. Failure surfaces the first three errors.
fn assert_kernel_validation_passes(model: &BRepModel, context: &str) {
    let result = validate_model_enhanced(model, Tolerance::default(), ValidationLevel::Standard);
    if !result.is_valid {
        let summary = result
            .errors
            .iter()
            .take(3)
            .map(|e| format!("{:?}", e))
            .collect::<Vec<_>>()
            .join("; ");
        panic!(
            "{}: validate_model_enhanced(Standard) reported {} error(s): {}",
            context,
            result.errors.len(),
            summary,
        );
    }
}

// ---------------------------------------------------------------------
// Fillet — single edge
// ---------------------------------------------------------------------

#[test]
fn fillet_single_box_edge_preserves_watertight_topology() {
    let mut model = BRepModel::new();
    let solid_id = make_box(&mut model, 10.0, 10.0, 10.0);

    // Pre-surgery baseline: a clean box has 8V / 12E / 6F.
    assert_no_boundary_edges(&model, "box pre-fillet");
    assert_euler_two(&model, "box pre-fillet");

    let edge = model
        .edges
        .iter()
        .map(|(id, _)| id)
        .next()
        .expect("box should have at least one edge");

    let options = FilletOptions {
        fillet_type: FilletType::Constant(2.0),
        radius: 2.0,
        propagation: FilletPropagation::None,
        ..Default::default()
    };

    let blend_faces = fillet_edges(&mut model, solid_id, vec![edge], options)
        .expect("single-edge fillet on a 10x10x10 box should succeed");
    assert_eq!(
        blend_faces.len(),
        1,
        "constant fillet on a single box edge should emit exactly one blend face"
    );

    assert_no_boundary_edges(&model, "box post-fillet");
    assert_euler_two(&model, "box post-fillet");
    assert_edges_geometrically_coherent(&model, "box post-fillet");
    assert_fillet_cap_edges_are_arcs(&model, &blend_faces, 2.0, "box post-fillet");
    assert_kernel_validation_passes(&model, "box post-fillet");
}

// ---------------------------------------------------------------------
// Chamfer — single edge, equal distance
// ---------------------------------------------------------------------

#[test]
fn chamfer_single_box_edge_preserves_watertight_topology() {
    let mut model = BRepModel::new();
    let solid_id = make_box(&mut model, 10.0, 10.0, 10.0);

    assert_no_boundary_edges(&model, "box pre-chamfer");
    assert_euler_two(&model, "box pre-chamfer");

    let edge = model
        .edges
        .iter()
        .map(|(id, _)| id)
        .next()
        .expect("box should have at least one edge");

    let options = ChamferOptions {
        chamfer_type: ChamferType::EqualDistance(1.0),
        distance1: 1.0,
        distance2: 1.0,
        symmetric: true,
        propagation: ChamferPropagation::None,
        ..Default::default()
    };

    let blend_faces = chamfer_edges(&mut model, solid_id, vec![edge], options)
        .expect("single-edge equal-distance chamfer on a 10x10x10 box should succeed");
    assert_eq!(
        blend_faces.len(),
        1,
        "equal-distance chamfer on a single box edge should emit exactly one blend face"
    );

    assert_no_boundary_edges(&model, "box post-chamfer");
    assert_euler_two(&model, "box post-chamfer");
    assert_edges_geometrically_coherent(&model, "box post-chamfer");
    assert_kernel_validation_passes(&model, "box post-chamfer");
}

// ---------------------------------------------------------------------
// Chamfer — TwoDistances asymmetric variant exercises the same surgery
// path with non-trivial offsets on each face (catches sign / orientation
// regressions that EqualDistance can mask).
// ---------------------------------------------------------------------

#[test]
fn chamfer_two_distances_box_edge_preserves_watertight_topology() {
    let mut model = BRepModel::new();
    let solid_id = make_box(&mut model, 10.0, 10.0, 10.0);

    let edge = model
        .edges
        .iter()
        .map(|(id, _)| id)
        .next()
        .expect("box should have at least one edge");

    let options = ChamferOptions {
        chamfer_type: ChamferType::TwoDistances(1.5, 0.75),
        distance1: 1.5,
        distance2: 0.75,
        symmetric: false,
        propagation: ChamferPropagation::None,
        ..Default::default()
    };

    let blend_faces = chamfer_edges(&mut model, solid_id, vec![edge], options)
        .expect("two-distance chamfer on a single box edge should succeed");
    assert_eq!(blend_faces.len(), 1);

    assert_no_boundary_edges(&model, "box post-chamfer (two-distance)");
    assert_euler_two(&model, "box post-chamfer (two-distance)");
    assert_edges_geometrically_coherent(&model, "box post-chamfer (two-distance)");
    assert_kernel_validation_passes(&model, "box post-chamfer (two-distance)");
}
