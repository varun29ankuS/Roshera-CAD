//! Regression tests for the second-fillet-on-box bug.
//!
//! ## User report (2026-05-14)
//!
//! On a freshly extruded rectangle (box), filleting a single top edge
//! succeeds and the tessellation looks clean. Attempting to fillet a
//! second top edge produces visible "stretched triangles, holes" in
//! the rendered mesh — i.e. a non-manifold tessellation — rather
//! than a clean second blend.
//!
//! ## Diagnosis pinned by this file
//!
//! Two layers are exercised independently:
//!
//! 1. **Kernel topology** (`assert_solid_valid`). After two sequential
//!    fillets on a box, the kernel's `fillet_edges` produces valid
//!    B-Rep: every shell edge is shared by ≥ 2 faces, the validator
//!    reports no errors. Pinned by the unignored tests below — these
//!    pass today.
//!
//! 2. **Tessellation manifoldness** (`assert_mesh_manifold`). The
//!    tessellated `TriangleMesh` after the second fillet is *not*
//!    closed-2-manifold: hundreds of triangle edges appear in only
//!    one triangle, exactly matching the user's "stretched triangles,
//!    holes" observation. Pinned by the `#[ignore]`-tagged tests
//!    suffixed `_tessellation_manifold` — these fail today and are
//!    the regression net for the eventual tessellation fix.
//!
//! Diagnosis (2026-05-14 session): the kernel-level fillet is doing
//! its job. The visible corruption originates in the per-face
//! tessellator, which after sequential fillets emits boundary samples
//! whose 3D positions do not collapse under `weld_mesh_watertight_
//! range`, leaving T-junctions / mismatched interior triangulations
//! between the trimmed top face and its two blend faces.
//!
//! ## Geometry choice
//!
//! The two filleted edges are deliberately non-adjacent (opposite
//! sides of the top face) so that the first fillet's vertex
//! modifications cannot reach the second edge's endpoints — the
//! second edge survives the first fillet intact, and the only
//! interaction between the two operations is via the shared top
//! face.

use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::extrude::{extrude_profile, ExtrudeOptions};
use geometry_engine::operations::fillet::{FilletType, PropagationMode};
use geometry_engine::operations::{fillet_edges, FilletOptions};
use geometry_engine::primitives::curve::Line;
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::validation::{ParallelValidator, ValidationLevel};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

// `create_box_3d(w, h, d)` builds a box **centred at the origin**:
// vertices at ±w/2, ±h/2, ±d/2. Top face is therefore at z = +d/2.
const Z_TOP: f64 = 2.5;
const EPS: f64 = 1e-9;

fn make_box(model: &mut BRepModel) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    match builder
        .create_box_3d(5.0, 5.0, 5.0)
        .expect("box creation succeeds")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {:?}", other),
    }
}

/// Top edges of the canonical box are those whose two vertex
/// positions both sit at `z == Z_TOP`. Returns `(edge_id, midpoint_x,
/// midpoint_y)` for each top edge in the order the EdgeStore yields
/// them.
fn top_edges(model: &BRepModel) -> Vec<(EdgeId, f64, f64)> {
    let mut found = Vec::new();
    for (eid, edge) in model.edges.iter() {
        if edge.is_loop() {
            continue;
        }
        let Some(v0) = model.vertices.get(edge.start_vertex) else {
            continue;
        };
        let Some(v1) = model.vertices.get(edge.end_vertex) else {
            continue;
        };
        let p0 = v0.position;
        let p1 = v1.position;
        if (p0[2] - Z_TOP).abs() < EPS && (p1[2] - Z_TOP).abs() < EPS {
            let mx = 0.5 * (p0[0] + p1[0]);
            let my = 0.5 * (p0[1] + p1[1]);
            found.push((eid, mx, my));
        }
    }
    found
}

/// Find a surviving top edge whose midpoint is close to the given
/// world-space (x, y). Used after the first fillet to re-locate the
/// "opposite top edge" without depending on EdgeId stability.
fn find_top_edge_near(model: &BRepModel, target_x: f64, target_y: f64, tol: f64) -> Option<EdgeId> {
    for (eid, mx, my) in top_edges(model) {
        if (mx - target_x).hypot(my - target_y) < tol {
            return Some(eid);
        }
    }
    None
}

fn fillet_opts(radius: f64) -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(radius),
        radius,
        propagation: PropagationMode::None,
        ..Default::default()
    }
}

/// Assert that the tessellated mesh of `solid` is closed-2-manifold
/// (every triangle edge appears in exactly two triangles, ignoring
/// orientation). Catches the case where the kernel topology is valid
/// but the tessellator emits stretched / overlapping triangles — the
/// visual corruption the user reported.
fn assert_mesh_manifold(model: &BRepModel, solid: SolidId, label: &str) {
    assert_mesh_manifold_with_params(model, solid, label, &TessellationParams::default());
}

/// Same closed-2-manifold check, but parameterised over
/// `TessellationParams`. Used by the cross-tolerance matrix tests to
/// pin the canonical-edge-sample cache across the regimes where
/// sample counts diverge most.
fn assert_mesh_manifold_with_params(
    model: &BRepModel,
    solid: SolidId,
    label: &str,
    params: &TessellationParams,
) {
    let solid_ref = model.solids.get(solid).expect("solid stored");
    let mesh = tessellate_solid(solid_ref, model, params);
    assert!(
        !mesh.triangles.is_empty(),
        "{label}: tessellation must produce at least one triangle"
    );

    // Build half-edge usage map. Each undirected edge {a, b} (a<b)
    // must appear in exactly two triangles.
    let mut edge_count: std::collections::HashMap<(u32, u32), usize> =
        std::collections::HashMap::new();
    for tri in &mesh.triangles {
        let a = tri[0];
        let b = tri[1];
        let c = tri[2];
        for (u, v) in [(a, b), (b, c), (c, a)] {
            let key = if u < v { (u, v) } else { (v, u) };
            *edge_count.entry(key).or_insert(0) += 1;
        }
    }
    let boundary: Vec<_> = edge_count
        .iter()
        .filter(|(_, &c)| c != 2)
        .map(|(k, c)| (*k, *c))
        .collect();
    let max_boundary_report = 8usize.min(boundary.len());
    assert!(
        boundary.is_empty(),
        "{label}: tessellated mesh must be closed 2-manifold; \
         {} non-manifold edges (sample: {:?})",
        boundary.len(),
        &boundary[..max_boundary_report],
    );

    // No degenerate / NaN / inf vertices.
    for (idx, v) in mesh.vertices.iter().enumerate() {
        assert!(
            v.position[0].is_finite() && v.position[1].is_finite() && v.position[2].is_finite(),
            "{label}: vertex {idx} has non-finite position {:?}",
            v.position
        );
    }
}

fn assert_solid_valid(model: &BRepModel, solid: SolidId, label: &str) {
    let validator = ParallelValidator::new();
    let report = validator.validate_model(model, Tolerance::default(), ValidationLevel::Standard);
    assert!(
        report.errors.is_empty(),
        "{label}: validation must report no errors; got {} errors: {:?}",
        report.errors.len(),
        report.errors,
    );
    let solid_ref = model.solids.get(solid).expect("solid still in store");
    let shell_id = solid_ref.outer_shell;
    let shell = model.shells.get(shell_id).expect("outer shell stored");
    let mut edge_face_count: std::collections::HashMap<EdgeId, usize> =
        std::collections::HashMap::new();
    for &face_id in &shell.faces {
        let face = model.faces.get(face_id).expect("face stored");
        let mut loops = vec![face.outer_loop];
        loops.extend(face.inner_loops.iter().copied());
        for loop_id in loops {
            let lp = model.loops.get(loop_id).expect("loop stored");
            for &edge_id in &lp.edges {
                *edge_face_count.entry(edge_id).or_insert(0) += 1;
            }
        }
    }
    let boundary: Vec<_> = edge_face_count
        .iter()
        .filter(|(_, &c)| c < 2)
        .map(|(e, c)| (*e, *c))
        .collect();
    assert!(
        boundary.is_empty(),
        "{label}: every edge in the outer shell must be used by ≥2 faces; \
         boundary edges: {boundary:?}"
    );
}

#[test]
fn first_fillet_on_top_edge_produces_valid_solid() {
    let mut model = BRepModel::new();
    let solid = make_box(&mut model);
    let edges = top_edges(&model);
    assert_eq!(edges.len(), 4, "box must have exactly 4 top edges");
    let (first_edge, _, _) = edges[0];

    fillet_edges(&mut model, solid, vec![first_edge], fillet_opts(1.0))
        .expect("first fillet on a top edge must succeed");

    assert_solid_valid(&model, solid, "after first fillet");
}

#[test]
fn second_fillet_on_opposite_top_edge_produces_valid_solid() {
    // Reproducer for the 2026-05-14 user report. Two non-adjacent
    // top edges of a box; the second fillet must not corrupt the
    // shell. Either it succeeds with valid topology, or it returns
    // an `Err` and `with_rollback` keeps the post-first-fillet
    // model intact.
    let mut model = BRepModel::new();
    let solid = make_box(&mut model);
    let edges = top_edges(&model);
    assert_eq!(edges.len(), 4, "box must have exactly 4 top edges");

    // Pick edge #0 and its opposite (the top edge whose midpoint is
    // furthest from edge #0's midpoint). For a 5×5 top face this is
    // unambiguously the parallel edge on the far side.
    let (first_edge, fx, fy) = edges[0];
    let opposite = edges[1..]
        .iter()
        .max_by(|a, b| {
            let da = (a.1 - fx).hypot(a.2 - fy);
            let db = (b.1 - fx).hypot(b.2 - fy);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .copied()
        .expect("3 other top edges exist");
    let (_, ox, oy) = opposite;

    fillet_edges(&mut model, solid, vec![first_edge], fillet_opts(1.0))
        .expect("first fillet on a top edge must succeed");
    assert_solid_valid(&model, solid, "after first fillet");

    // EdgeIds are not guaranteed stable across fillet operations.
    // Re-locate the opposite edge by its (still-valid) midpoint.
    let second_edge = find_top_edge_near(&model, ox, oy, 0.25).expect(
        "opposite top edge must survive the first fillet \
         (its endpoints don't share vertices with the first edge)",
    );

    let second_result = fillet_edges(&mut model, solid, vec![second_edge], fillet_opts(1.0));

    match second_result {
        Ok(_) => assert_solid_valid(&model, solid, "after second fillet (Ok)"),
        Err(e) => assert_solid_valid(
            &model,
            solid,
            &format!("after second fillet rolled back ({:?})", e),
        ),
    }
}

#[test]
fn second_fillet_on_adjacent_top_edge_does_not_corrupt() {
    // Stricter variant: two adjacent top edges sharing a vertex.
    // This is the closer match to the user's screenshot where they
    // likely picked the next edge on the same face. The shared
    // vertex is a known sharp point of the fillet algebra (corner
    // ball / setback territory) — current kernel may refuse this
    // (Task #82 vertex-blend gap), in which case the rollback
    // contract must keep the post-first-fillet state intact.
    let mut model = BRepModel::new();
    let solid = make_box(&mut model);
    let edges = top_edges(&model);
    assert_eq!(edges.len(), 4, "box must have exactly 4 top edges");
    let (first_edge, _fx, _fy) = edges[0];

    // Find a top edge that shares exactly one vertex with the first
    // (adjacent top edge). For the canonical box, two of the three
    // other top edges are adjacent; either works.
    let first_edge_obj = model.edges.get(first_edge).expect("first edge stored");
    let v_first: [_; 2] = [first_edge_obj.start_vertex, first_edge_obj.end_vertex];

    let adjacent_edge_id = edges[1..]
        .iter()
        .find(|(eid, _, _)| {
            let e = model.edges.get(*eid).expect("edge stored");
            v_first.contains(&e.start_vertex) ^ v_first.contains(&e.end_vertex)
        })
        .copied()
        .expect("two of the other three top edges are adjacent");
    let (_, ax, ay) = adjacent_edge_id;

    fillet_edges(&mut model, solid, vec![first_edge], fillet_opts(1.0))
        .expect("first fillet on a top edge must succeed");
    assert_solid_valid(&model, solid, "after first fillet");

    let second_edge = find_top_edge_near(&model, ax, ay, 1.5);
    let Some(second_edge) = second_edge else {
        // First fillet consumed / replaced the adjacent edge; the
        // user's "second fillet" use case doesn't apply here. Test
        // is vacuously satisfied — but assert the model is still
        // valid.
        assert_solid_valid(&model, solid, "after first fillet, adjacent edge consumed");
        return;
    };

    let second_result = fillet_edges(
        &mut model,
        solid,
        vec![second_edge],
        fillet_opts(0.5), // smaller radius; adjacency makes a corner ball
    );

    match second_result {
        Ok(_) => assert_solid_valid(&model, solid, "after adjacent-edge fillet (Ok)"),
        Err(e) => assert_solid_valid(
            &model,
            solid,
            &format!("after adjacent-edge fillet rolled back ({:?})", e),
        ),
    }
}

#[test]
fn second_fillet_with_different_radius_does_not_corrupt() {
    // The user's screenshot suggested different radii on the two
    // fillets. Pin that case explicitly: fillet#1 at r=1.0,
    // fillet#2 at r=2.0 on the opposite top edge.
    let mut model = BRepModel::new();
    let solid = make_box(&mut model);
    let edges = top_edges(&model);
    let (first_edge, fx, fy) = edges[0];
    let (_, ox, oy) = edges[1..]
        .iter()
        .max_by(|a, b| {
            let da = (a.1 - fx).hypot(a.2 - fy);
            let db = (b.1 - fx).hypot(b.2 - fy);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .copied()
        .expect("3 other top edges exist");

    fillet_edges(&mut model, solid, vec![first_edge], fillet_opts(1.0))
        .expect("first fillet must succeed");
    assert_solid_valid(&model, solid, "after first fillet r=1.0");

    let second_edge = find_top_edge_near(&model, ox, oy, 0.25)
        .expect("opposite top edge survives the first fillet");
    let result = fillet_edges(&mut model, solid, vec![second_edge], fillet_opts(2.0));
    match result {
        Ok(_) => assert_solid_valid(&model, solid, "after second fillet r=2.0 (Ok)"),
        Err(e) => assert_solid_valid(
            &model,
            solid,
            &format!("after second fillet r=2.0 rolled back ({:?})", e),
        ),
    }
}

// ---------------------------------------------------------------------
// Extruded-rectangle reproducer
//
// The user-facing flow that triggered the bug builds the solid via
// `extrude_profile` over a rectangular sketch, not via
// `create_box_3d`. The two paths produce topologically equivalent
// boxes but use different construction primitives — `extrude_profile`
// generates faces, loops and edges through the extrusion-loop-
// topology helpers, whereas `create_box_3d` stamps them directly.
// We exercise both to be sure the bug isn't sensitive to the
// construction route.
// ---------------------------------------------------------------------

/// Build a 5×5×5 box by extruding a CCW rectangle from z=0 to z=5.
/// Matches the topology emitted by `extrude_sketch` on a single-shape
/// rectangle sketch.
fn make_extruded_box(model: &mut BRepModel) -> SolidId {
    // CCW rectangle on the XY plane.
    let v0 = model.vertices.add(0.0, 0.0, 0.0);
    let v1 = model.vertices.add(5.0, 0.0, 0.0);
    let v2 = model.vertices.add(5.0, 5.0, 0.0);
    let v3 = model.vertices.add(0.0, 5.0, 0.0);
    let positions = [
        (v0, v1, [0.0, 0.0, 0.0], [5.0, 0.0, 0.0]),
        (v1, v2, [5.0, 0.0, 0.0], [5.0, 5.0, 0.0]),
        (v2, v3, [5.0, 5.0, 0.0], [0.0, 5.0, 0.0]),
        (v3, v0, [0.0, 5.0, 0.0], [0.0, 0.0, 0.0]),
    ];
    let mut edges = Vec::with_capacity(4);
    for (va, vb, pa, pb) in positions {
        let line = Line::new(
            Point3::new(pa[0], pa[1], pa[2]),
            Point3::new(pb[0], pb[1], pb[2]),
        );
        let curve_id = model.curves.add(Box::new(line));
        let edge = Edge::new_auto_range(0, va, vb, curve_id, EdgeOrientation::Forward);
        edges.push(model.edges.add(edge));
    }
    let opts = ExtrudeOptions {
        direction: Vector3::Z,
        distance: 5.0,
        cap_ends: true,
        ..ExtrudeOptions::default()
    };
    extrude_profile(model, edges, opts).expect("rectangle extrusion must succeed")
}

#[test]
fn first_fillet_on_extruded_rect_top_edge_produces_valid_solid() {
    // For an extruded rectangle, z = 5.0 (z_top = distance).
    let mut model = BRepModel::new();
    let solid = make_extruded_box(&mut model);

    let edges: Vec<(EdgeId, f64, f64)> = {
        let z_top = 5.0;
        let mut found = Vec::new();
        for (eid, edge) in model.edges.iter() {
            if edge.is_loop() {
                continue;
            }
            let v0 = model.vertices.get(edge.start_vertex).expect("v0");
            let v1 = model.vertices.get(edge.end_vertex).expect("v1");
            if (v0.position[2] - z_top).abs() < EPS && (v1.position[2] - z_top).abs() < EPS {
                found.push((
                    eid,
                    0.5 * (v0.position[0] + v1.position[0]),
                    0.5 * (v0.position[1] + v1.position[1]),
                ));
            }
        }
        found
    };
    assert_eq!(edges.len(), 4, "extruded rect must have 4 top edges");

    let (first, _, _) = edges[0];
    fillet_edges(&mut model, solid, vec![first], fillet_opts(1.0))
        .expect("first fillet on extruded-rect top edge must succeed");

    assert_solid_valid(&model, solid, "after first fillet on extruded rect");
}

#[test]
fn second_fillet_on_extruded_rect_top_edge_does_not_corrupt() {
    // Direct reproducer of the 2026-05-14 user report: build the
    // solid via extrude_profile (as the sketch path does), fillet one
    // top edge, then fillet the opposite top edge. After both
    // operations the shell must be valid.
    let mut model = BRepModel::new();
    let solid = make_extruded_box(&mut model);

    let z_top = 5.0;
    let collect_top_edges = |m: &BRepModel| -> Vec<(EdgeId, f64, f64)> {
        let mut found = Vec::new();
        for (eid, edge) in m.edges.iter() {
            if edge.is_loop() {
                continue;
            }
            let Some(v0) = m.vertices.get(edge.start_vertex) else {
                continue;
            };
            let Some(v1) = m.vertices.get(edge.end_vertex) else {
                continue;
            };
            if (v0.position[2] - z_top).abs() < EPS && (v1.position[2] - z_top).abs() < EPS {
                found.push((
                    eid,
                    0.5 * (v0.position[0] + v1.position[0]),
                    0.5 * (v0.position[1] + v1.position[1]),
                ));
            }
        }
        found
    };

    let initial_top = collect_top_edges(&model);
    assert_eq!(initial_top.len(), 4, "extruded rect must have 4 top edges");

    let (first, fx, fy) = initial_top[0];
    let (_, ox, oy) = initial_top[1..]
        .iter()
        .max_by(|a, b| {
            let da = (a.1 - fx).hypot(a.2 - fy);
            let db = (b.1 - fx).hypot(b.2 - fy);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .copied()
        .expect("3 other top edges exist");

    fillet_edges(&mut model, solid, vec![first], fillet_opts(1.0))
        .expect("first fillet on extruded rect top edge must succeed");
    assert_solid_valid(&model, solid, "after first fillet on extruded rect");

    // Re-locate the opposite top edge after the first fillet
    // (EdgeIds may have shifted but the geometry of the opposite
    // edge is untouched).
    let post_top = collect_top_edges(&model);
    let second = post_top
        .iter()
        .find(|(_, mx, my)| (mx - ox).hypot(my - oy) < 0.25)
        .map(|(id, _, _)| *id)
        .expect("opposite top edge of the extruded rect must survive the first fillet");

    let result = fillet_edges(&mut model, solid, vec![second], fillet_opts(1.0));
    match result {
        Ok(_) => assert_solid_valid(
            &model,
            solid,
            "after second fillet on extruded rect opposite top edge (Ok)",
        ),
        Err(e) => assert_solid_valid(
            &model,
            solid,
            &format!("after second fillet on extruded rect rolled back ({:?})", e),
        ),
    }
}

#[test]
fn second_fillet_on_extruded_rect_adjacent_top_edge_does_not_corrupt() {
    // Variant: adjacent top edge (sharing a vertex with the first
    // filleted edge). This exercises the corner-blend territory.
    // The result must either be valid or roll back to the post-
    // first-fillet state — never produce malformed B-Rep.
    let mut model = BRepModel::new();
    let solid = make_extruded_box(&mut model);

    let z_top = 5.0;
    let mut initial_top: Vec<(EdgeId, f64, f64)> = Vec::new();
    for (eid, edge) in model.edges.iter() {
        if edge.is_loop() {
            continue;
        }
        let v0 = model.vertices.get(edge.start_vertex).expect("v0");
        let v1 = model.vertices.get(edge.end_vertex).expect("v1");
        if (v0.position[2] - z_top).abs() < EPS && (v1.position[2] - z_top).abs() < EPS {
            initial_top.push((
                eid,
                0.5 * (v0.position[0] + v1.position[0]),
                0.5 * (v0.position[1] + v1.position[1]),
            ));
        }
    }
    assert_eq!(initial_top.len(), 4);

    let (first, _fx, _fy) = initial_top[0];
    let first_edge_obj = model.edges.get(first).expect("first edge stored");
    let v_first = [first_edge_obj.start_vertex, first_edge_obj.end_vertex];
    let adjacent = initial_top[1..]
        .iter()
        .find(|(eid, _, _)| {
            let e = model.edges.get(*eid).expect("edge stored");
            v_first.contains(&e.start_vertex) ^ v_first.contains(&e.end_vertex)
        })
        .copied()
        .expect("two of the other three top edges are adjacent");
    let (_, ax, ay) = adjacent;

    fillet_edges(&mut model, solid, vec![first], fillet_opts(1.0))
        .expect("first fillet must succeed");
    assert_solid_valid(&model, solid, "after first fillet on extruded rect");

    // The adjacent edge's vertex shared with the first edge was
    // consumed by fillet#1; the remaining portion of the adjacent
    // edge may have been replaced by a trimmed-back successor.
    // Search by midpoint with a generous tolerance.
    let mut survived: Option<EdgeId> = None;
    for (eid, edge) in model.edges.iter() {
        if edge.is_loop() {
            continue;
        }
        let v0 = model.vertices.get(edge.start_vertex).expect("v0");
        let v1 = model.vertices.get(edge.end_vertex).expect("v1");
        if (v0.position[2] - z_top).abs() < EPS && (v1.position[2] - z_top).abs() < EPS {
            let mx = 0.5 * (v0.position[0] + v1.position[0]);
            let my = 0.5 * (v0.position[1] + v1.position[1]);
            if (mx - ax).hypot(my - ay) < 1.5 {
                survived = Some(eid);
                break;
            }
        }
    }
    let Some(second) = survived else {
        // Adjacent edge fully consumed — vacuously satisfied.
        return;
    };

    let result = fillet_edges(&mut model, solid, vec![second], fillet_opts(0.5));
    match result {
        Ok(_) => assert_solid_valid(
            &model,
            solid,
            "after second fillet on adjacent extruded-rect top edge (Ok)",
        ),
        Err(e) => assert_solid_valid(
            &model,
            solid,
            &format!(
                "after second fillet on adjacent extruded-rect top edge rolled back ({:?})",
                e
            ),
        ),
    }
}

// =====================================================================
// Tessellation-manifold regression net
//
// Pins the canonical-edge-sample cache (`tessellation/edge_cache.rs`).
// After a fillet, the trimmed top face and the blend faces share an
// edge; the cache ensures both faces resolve that edge to the same
// 3D point sequence at tessellation time, collapsing the seam under
// `weld_mesh_watertight_range`. Pre-fix observed counts (2026-05-14,
// `TessellationParams::default()`):
//
//   - single fillet on `create_box_3d`:           168 non-manifold edges
//   - single fillet on `extrude_profile` rect:    0   non-manifold edges
//   - two fillets on either construction route:   336 non-manifold edges
//
// Post-fix: 0 non-manifold edges across all three scenarios.
// =====================================================================

#[test]
fn first_fillet_on_top_edge_tessellation_manifold() {
    let mut model = BRepModel::new();
    let solid = make_box(&mut model);
    let edges = top_edges(&model);
    let (first_edge, _, _) = edges[0];
    fillet_edges(&mut model, solid, vec![first_edge], fillet_opts(1.0))
        .expect("first fillet on a top edge must succeed");
    assert_mesh_manifold(&model, solid, "after first fillet — mesh");
}

#[test]
fn second_fillet_on_opposite_top_edge_tessellation_manifold() {
    let mut model = BRepModel::new();
    let solid = make_box(&mut model);
    let edges = top_edges(&model);
    let (first_edge, fx, fy) = edges[0];
    let opposite = edges[1..]
        .iter()
        .max_by(|a, b| {
            let da = (a.1 - fx).hypot(a.2 - fy);
            let db = (b.1 - fx).hypot(b.2 - fy);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .copied()
        .expect("3 other top edges exist");
    let (_, ox, oy) = opposite;
    fillet_edges(&mut model, solid, vec![first_edge], fillet_opts(1.0))
        .expect("first fillet must succeed");
    let second_edge = find_top_edge_near(&model, ox, oy, 0.25)
        .expect("opposite top edge must survive the first fillet");
    let _ = fillet_edges(&mut model, solid, vec![second_edge], fillet_opts(1.0));
    assert_mesh_manifold(&model, solid, "after second fillet — mesh");
}

#[test]
fn second_fillet_on_extruded_rect_top_edge_tessellation_manifold() {
    let mut model = BRepModel::new();
    let solid = make_extruded_box(&mut model);

    let z_top = 5.0;
    let collect_top_edges = |m: &BRepModel| -> Vec<(EdgeId, f64, f64)> {
        let mut found = Vec::new();
        for (eid, edge) in m.edges.iter() {
            if edge.is_loop() {
                continue;
            }
            let Some(v0) = m.vertices.get(edge.start_vertex) else {
                continue;
            };
            let Some(v1) = m.vertices.get(edge.end_vertex) else {
                continue;
            };
            if (v0.position[2] - z_top).abs() < EPS && (v1.position[2] - z_top).abs() < EPS {
                found.push((
                    eid,
                    0.5 * (v0.position[0] + v1.position[0]),
                    0.5 * (v0.position[1] + v1.position[1]),
                ));
            }
        }
        found
    };

    let initial_top = collect_top_edges(&model);
    let (first, fx, fy) = initial_top[0];
    let (_, ox, oy) = initial_top[1..]
        .iter()
        .max_by(|a, b| {
            let da = (a.1 - fx).hypot(a.2 - fy);
            let db = (b.1 - fx).hypot(b.2 - fy);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .copied()
        .expect("3 other top edges exist");

    fillet_edges(&mut model, solid, vec![first], fillet_opts(1.0))
        .expect("first fillet must succeed");
    let post_top = collect_top_edges(&model);
    let second = post_top
        .iter()
        .find(|(_, mx, my)| (mx - ox).hypot(my - oy) < 0.25)
        .map(|(id, _, _)| *id)
        .expect("opposite top edge survives the first fillet");
    let _ = fillet_edges(&mut model, solid, vec![second], fillet_opts(1.0));
    assert_mesh_manifold(&model, solid, "after second fillet on extruded rect — mesh");
}

// =====================================================================
// Tolerance-matrix variants
//
// The canonical-edge-sample cache picks per-edge sample counts from
// the cache's `compute_curve_sample_count`, which is driven by
// `TessellationParams::{chord_tolerance, max_edge_length,
// max_angle_deviation}`. The matrix below pins the manifoldness
// invariant across `coarse` (loose), `default`, and `fine` (tight)
// regimes — where the two trim caches are most likely to disagree
// in length and the local resampling path in
// `tessellate_fillet_face` is exercised.
// =====================================================================

fn tolerance_matrix() -> [(&'static str, TessellationParams); 3] {
    [
        ("coarse", TessellationParams::coarse()),
        ("default", TessellationParams::default()),
        ("fine", TessellationParams::fine()),
    ]
}

#[test]
fn first_fillet_on_top_edge_tessellation_manifold_across_tolerances() {
    for (regime, params) in tolerance_matrix() {
        let mut model = BRepModel::new();
        let solid = make_box(&mut model);
        let edges = top_edges(&model);
        let (first_edge, _, _) = edges[0];
        fillet_edges(&mut model, solid, vec![first_edge], fillet_opts(1.0))
            .expect("first fillet on a top edge must succeed");
        assert_mesh_manifold_with_params(
            &model,
            solid,
            &format!("first fillet — {regime}"),
            &params,
        );
    }
}

#[test]
fn second_fillet_on_opposite_top_edge_tessellation_manifold_across_tolerances() {
    for (regime, params) in tolerance_matrix() {
        let mut model = BRepModel::new();
        let solid = make_box(&mut model);
        let edges = top_edges(&model);
        let (first_edge, fx, fy) = edges[0];
        let opposite = edges[1..]
            .iter()
            .max_by(|a, b| {
                let da = (a.1 - fx).hypot(a.2 - fy);
                let db = (b.1 - fx).hypot(b.2 - fy);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .copied()
            .expect("3 other top edges exist");
        let (_, ox, oy) = opposite;
        fillet_edges(&mut model, solid, vec![first_edge], fillet_opts(1.0))
            .expect("first fillet must succeed");
        let second_edge = find_top_edge_near(&model, ox, oy, 0.25)
            .expect("opposite top edge must survive the first fillet");
        let _ = fillet_edges(&mut model, solid, vec![second_edge], fillet_opts(1.0));
        assert_mesh_manifold_with_params(
            &model,
            solid,
            &format!("second fillet (opposite) — {regime}"),
            &params,
        );
    }
}

#[test]
fn second_fillet_on_extruded_rect_top_edge_tessellation_manifold_across_tolerances() {
    let collect_top_edges = |m: &BRepModel| -> Vec<(EdgeId, f64, f64)> {
        let z_top = 5.0;
        let mut found = Vec::new();
        for (eid, edge) in m.edges.iter() {
            if edge.is_loop() {
                continue;
            }
            let Some(v0) = m.vertices.get(edge.start_vertex) else {
                continue;
            };
            let Some(v1) = m.vertices.get(edge.end_vertex) else {
                continue;
            };
            if (v0.position[2] - z_top).abs() < EPS && (v1.position[2] - z_top).abs() < EPS {
                found.push((
                    eid,
                    0.5 * (v0.position[0] + v1.position[0]),
                    0.5 * (v0.position[1] + v1.position[1]),
                ));
            }
        }
        found
    };

    for (regime, params) in tolerance_matrix() {
        let mut model = BRepModel::new();
        let solid = make_extruded_box(&mut model);
        let initial_top = collect_top_edges(&model);
        let (first, fx, fy) = initial_top[0];
        let (_, ox, oy) = initial_top[1..]
            .iter()
            .max_by(|a, b| {
                let da = (a.1 - fx).hypot(a.2 - fy);
                let db = (b.1 - fx).hypot(b.2 - fy);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .copied()
            .expect("3 other top edges exist");
        fillet_edges(&mut model, solid, vec![first], fillet_opts(1.0))
            .expect("first fillet must succeed");
        let post_top = collect_top_edges(&model);
        let second = post_top
            .iter()
            .find(|(_, mx, my)| (mx - ox).hypot(my - oy) < 0.25)
            .map(|(id, _, _)| *id)
            .expect("opposite top edge survives the first fillet");
        let _ = fillet_edges(&mut model, solid, vec![second], fillet_opts(1.0));
        assert_mesh_manifold_with_params(
            &model,
            solid,
            &format!("second fillet (extruded rect) — {regime}"),
            &params,
        );
    }
}
