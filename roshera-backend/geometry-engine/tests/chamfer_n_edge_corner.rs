//! Chamfer-β — degree ≥ 4 convex corner planar cap integration tests.
//!
//! Extends `chamfer_three_edge_corner.rs` (Chamfer-α) to polyhedral
//! apex vertices. The fixture is a symmetric square pyramid: 5
//! vertices (4 base corners + 1 apex), 8 edges (4 base + 4 sloped),
//! 5 faces (1 square base + 4 triangular sloped sides). Chamfering
//! the 4 sloped edges feeds the `handle_chamfer_vertices` pass a
//! degree-4 apex; the N-gon cap-emit path closes the apex with a
//! single planar quad cap.
//!
//! These tests pin:
//!
//!   * **Watertightness** — the resulting outer shell satisfies
//!     Euler-Poincaré V − E + F = 2.
//!   * **Cap-face shape** — the new face's surface downcasts to
//!     [`Plane`], its outer loop has exactly 4 edges, each
//!     underlying curve is a [`Line`].
//!   * **Interior-edge invariant** — each of the four V-side cap
//!     edges is referenced by exactly two faces (the cap + one
//!     chamfer face).
//!   * **3-of-4 subset canary** — chamfering only 3 of the 4 sloped
//!     edges leaves the apex with edge_indices.len() = 3 but
//!     adjacent_set.len() = 4 (the three chamfered edges'
//!     four-face neighbour union still includes the un-chamfered
//!     fourth edge's neighbours), failing the adjacency-consistency
//!     gate and skipping cap synthesis. The apex remains a vertex
//!     (still incident to the un-chamfered sloped edge) and the
//!     three V-side cap edges form an open hole.
//!
//! The α regression suite in `chamfer_three_edge_corner.rs` covers
//! N=3, which is the identity case of `verify_cap_edges_form_closed_polygon`'s
//! chain walk.

use std::collections::HashSet;

use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::chamfer::{ChamferOptions, ChamferType, PropagationMode};
use geometry_engine::operations::{chamfer_edges, CommonOptions};
use geometry_engine::primitives::curve::{Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::face::{Face, FaceId, FaceOrientation};
use geometry_engine::primitives::r#loop::{Loop, LoopType};
use geometry_engine::primitives::shell::{Shell, ShellType};
use geometry_engine::primitives::solid::{Solid, SolidId};
use geometry_engine::primitives::surface::Plane;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::vertex::VertexId;

const PYRAMID_BASE: f64 = 10.0;
const PYRAMID_HEIGHT: f64 = 10.0;
const CHAMFER_OFFSET: f64 = 0.5;

/// Build a symmetric square-base pyramid centred on the z-axis with
/// the base on z=0 and the apex at (0, 0, height).
///
/// Vertex layout (`hb = base / 2`):
///   v0=(-hb,-hb,0) v1=(hb,-hb,0) v2=(hb,hb,0) v3=(-hb,hb,0)   base
///   v4=(0,0,h)                                                 apex
///
/// Edge layout (all stored start→end):
///   e0:v0→v1   e1:v1→v2   e2:v2→v3   e3:v3→v0   (base, CCW)
///   e4:v0→v4   e5:v1→v4   e6:v2→v4   e7:v3→v4   (sloped to apex)
///
/// Face layout (outward normals point away from the solid centroid):
///   f_base:  base square at z=0, outward −Z, loop v0→v3→v2→v1
///   f_front: triangle v0,v1,v4 — outward (0,−h,hb) direction
///   f_right: triangle v1,v2,v4 — outward (h,0,hb) direction
///   f_back:  triangle v2,v3,v4 — outward (0,h,hb) direction
///   f_left:  triangle v3,v0,v4 — outward (−h,0,hb) direction
fn make_square_pyramid_solid(model: &mut BRepModel, base: f64, height: f64) -> SolidId {
    let tol = Tolerance::default().distance();
    let hb = base / 2.0;

    // 5 vertices
    let v0 = model.vertices.add_or_find(-hb, -hb, 0.0, tol);
    let v1 = model.vertices.add_or_find(hb, -hb, 0.0, tol);
    let v2 = model.vertices.add_or_find(hb, hb, 0.0, tol);
    let v3 = model.vertices.add_or_find(-hb, hb, 0.0, tol);
    let v4 = model.vertices.add_or_find(0.0, 0.0, height, tol);

    let p0 = Point3::new(-hb, -hb, 0.0);
    let p1 = Point3::new(hb, -hb, 0.0);
    let p2 = Point3::new(hb, hb, 0.0);
    let p3 = Point3::new(-hb, hb, 0.0);
    let p4 = Point3::new(0.0, 0.0, height);

    // 8 edges
    let edges = [
        // Base CCW square at z=0
        (v0, v1, p0, p1),
        (v1, v2, p1, p2),
        (v2, v3, p2, p3),
        (v3, v0, p3, p0),
        // Sloped edges base→apex
        (v0, v4, p0, p4),
        (v1, v4, p1, p4),
        (v2, v4, p2, p4),
        (v3, v4, p3, p4),
    ];
    let mut edge_ids: [EdgeId; 8] = [0; 8];
    for (i, &(sv, ev, sp, ep)) in edges.iter().enumerate() {
        let curve_id = model.curves.add(Box::new(Line::new(sp, ep)));
        let edge = Edge::new(
            0,
            sv,
            ev,
            curve_id,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        );
        edge_ids[i] = model.edges.add(edge);
    }

    // Face data: (loop edge indices, per-edge forward flags, plane
    // origin, plane outward normal).
    //
    // Base (z=0, outward −Z): traversal v0→v3→v2→v1→v0
    //   v0→v3 = e3 reversed, v3→v2 = e2 reversed,
    //   v2→v1 = e1 reversed, v1→v0 = e0 reversed.
    //
    // Each sloped face's traversal is CCW when viewed from outside
    // (away from the solid centroid at z≈h/4), so the right-hand-rule
    // normal of the loop matches the analytical outward normal
    // supplied below.
    let face_data: [(Vec<usize>, Vec<bool>, Point3, Vector3); 5] = [
        // Base
        (
            vec![3, 2, 1, 0],
            vec![false, false, false, false],
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, -1.0),
        ),
        // Front (-Y side): v0→v1→v4
        (
            vec![0, 5, 4],
            vec![true, true, false],
            p0,
            Vector3::new(0.0, -height, hb),
        ),
        // Right (+X side): v1→v2→v4
        (
            vec![1, 6, 5],
            vec![true, true, false],
            p1,
            Vector3::new(height, 0.0, hb),
        ),
        // Back (+Y side): v2→v3→v4
        (
            vec![2, 7, 6],
            vec![true, true, false],
            p2,
            Vector3::new(0.0, height, hb),
        ),
        // Left (-X side): v3→v0→v4
        (
            vec![3, 4, 7],
            vec![true, true, false],
            p3,
            Vector3::new(-height, 0.0, hb),
        ),
    ];

    let mut face_ids: [FaceId; 5] = [0; 5];
    for (face_idx, (edge_indices, orientations, point, normal)) in face_data.iter().enumerate() {
        let plane = Plane::from_point_normal(*point, *normal).expect("pyramid plane construction");
        let surface_id = model.surfaces.add(Box::new(plane));

        let mut loop_obj = Loop::new(0, LoopType::Outer);
        for (i, &edge_idx) in edge_indices.iter().enumerate() {
            loop_obj.add_edge(edge_ids[edge_idx], orientations[i]);
        }
        let loop_id = model.loops.add(loop_obj);

        let mut face = Face::new(0, surface_id, loop_id, FaceOrientation::Forward);
        face.outer_loop = loop_id;
        face_ids[face_idx] = model.faces.add(face);
    }

    let mut shell = Shell::new(0, ShellType::Closed);
    for &fid in &face_ids {
        shell.add_face(fid);
    }
    let shell_id = model.shells.add(shell);
    model.solids.add(Solid::new(0, shell_id))
}

fn vertex_at(model: &BRepModel, x: f64, y: f64, z: f64) -> VertexId {
    for (id, vertex) in model.vertices.iter() {
        let p = vertex.position;
        if (p[0] - x).abs() < 1.0e-9 && (p[1] - y).abs() < 1.0e-9 && (p[2] - z).abs() < 1.0e-9 {
            return id;
        }
    }
    panic!("no vertex at ({}, {}, {})", x, y, z);
}

/// Topology census of `solid_id`'s outer shell as `(V, E, F)`. The
/// Euler-Poincaré relation V − E + F = 2 is the genus-zero
/// closed-surface invariant.
fn shell_census(model: &BRepModel, solid_id: SolidId) -> (usize, usize, usize) {
    let solid = model.solids.get(solid_id).expect("solid exists");
    let shell = model.shells.get(solid.outer_shell).expect("shell exists");
    let mut vertices: HashSet<VertexId> = HashSet::new();
    let mut edges: HashSet<EdgeId> = HashSet::new();
    for &face_id in &shell.faces {
        let face = model.faces.get(face_id).expect("face exists");
        for loop_id in face.all_loops() {
            let lp = model.loops.get(loop_id).expect("loop exists");
            for &edge_id in &lp.edges {
                edges.insert(edge_id);
                if let Some(edge) = model.edges.get(edge_id) {
                    vertices.insert(edge.start_vertex);
                    vertices.insert(edge.end_vertex);
                }
            }
        }
    }
    (vertices.len(), edges.len(), shell.faces.len())
}

fn faces_referencing_edge(model: &BRepModel, solid_id: SolidId, edge_id: EdgeId) -> usize {
    let solid = model.solids.get(solid_id).expect("solid exists");
    let shell = model.shells.get(solid.outer_shell).expect("shell exists");
    let mut count = 0;
    for &face_id in &shell.faces {
        let face = model.faces.get(face_id).expect("face exists");
        for loop_id in face.all_loops() {
            let lp = model.loops.get(loop_id).expect("loop exists");
            if lp.edges.iter().any(|&e| e == edge_id) {
                count += 1;
                break;
            }
        }
    }
    count
}

/// Count edges of `solid_id`'s outer shell whose reference count
/// across face loops is not exactly 2. A watertight closed manifold
/// has zero such edges.
fn non_manifold_edge_count(model: &BRepModel, solid_id: SolidId) -> usize {
    let solid = model.solids.get(solid_id).expect("solid exists");
    let shell = model.shells.get(solid.outer_shell).expect("shell exists");
    let mut all_edges: HashSet<EdgeId> = HashSet::new();
    for &face_id in &shell.faces {
        let face = model.faces.get(face_id).expect("face exists");
        for loop_id in face.all_loops() {
            let lp = model.loops.get(loop_id).expect("loop exists");
            for &edge_id in &lp.edges {
                all_edges.insert(edge_id);
            }
        }
    }
    all_edges
        .into_iter()
        .filter(|&edge_id| faces_referencing_edge(model, solid_id, edge_id) != 2)
        .count()
}

/// Locate the unique planar cap face among `face_ids` whose outer
/// loop has exactly `expected_edge_count` edges. Panics if there
/// isn't exactly one.
fn find_planar_cap_face(
    model: &BRepModel,
    face_ids: &[FaceId],
    expected_edge_count: usize,
) -> FaceId {
    let mut found: Option<FaceId> = None;
    for &fid in face_ids {
        let face = model.faces.get(fid).expect("face exists");
        let surface = model.surfaces.get(face.surface_id).expect("surface exists");
        if surface.as_any().downcast_ref::<Plane>().is_some() {
            let outer = model
                .loops
                .get(face.outer_loop)
                .expect("planar face outer loop");
            if outer.edges.len() == expected_edge_count {
                assert!(
                    found.is_none(),
                    "more than one planar face with {} outer-loop edges produced",
                    expected_edge_count
                );
                found = Some(fid);
            }
        }
    }
    found.unwrap_or_else(|| {
        panic!(
            "no planar cap face with {} outer-loop edges among returned chamfer faces",
            expected_edge_count
        )
    })
}

/// Collect the four sloped edges (base→apex) on the fixture pyramid.
fn sloped_edges(model: &BRepModel, base: f64, height: f64) -> Vec<EdgeId> {
    let hb = base / 2.0;
    let apex = vertex_at(model, 0.0, 0.0, height);
    let base_corners = [
        vertex_at(model, -hb, -hb, 0.0),
        vertex_at(model, hb, -hb, 0.0),
        vertex_at(model, hb, hb, 0.0),
        vertex_at(model, -hb, hb, 0.0),
    ];

    let mut result = Vec::with_capacity(4);
    for (edge_id, edge) in model.edges.iter() {
        let endpoints = (edge.start_vertex, edge.end_vertex);
        for &bv in &base_corners {
            if (endpoints.0 == bv && endpoints.1 == apex)
                || (endpoints.1 == bv && endpoints.0 == apex)
            {
                result.push(edge_id);
            }
        }
    }
    result
}

#[test]
fn chamfer_pyramid_apex_baseline_pre_chamfer() {
    // Pin the fixture math: a freshly-built square pyramid has
    // 5 vertices, 8 edges, 5 faces, V − E + F = 2.
    let mut model = BRepModel::new();
    let solid_id = make_square_pyramid_solid(&mut model, PYRAMID_BASE, PYRAMID_HEIGHT);
    let (v, e, f) = shell_census(&model, solid_id);
    assert_eq!(v, 5, "pyramid has 5 vertices (4 base + apex)");
    assert_eq!(e, 8, "pyramid has 8 edges (4 base + 4 sloped)");
    assert_eq!(f, 5, "pyramid has 5 faces (1 base + 4 sloped)");
    assert_eq!(
        v as i64 - e as i64 + f as i64,
        2,
        "Euler-Poincaré for the pre-chamfer pyramid"
    );
    assert_eq!(
        non_manifold_edge_count(&model, solid_id),
        0,
        "pre-chamfer pyramid must be a closed watertight manifold"
    );
}

#[test]
fn chamfer_pyramid_apex_emits_planar_quad_cap() {
    // Chamfer all 4 sloped edges of the symmetric square pyramid
    // at offset 0.5. The apex is a degree-4 convex uniform-offset
    // planar corner; Chamfer-β must close it with a single quad
    // cap.
    let mut model = BRepModel::new();
    let solid_id = make_square_pyramid_solid(&mut model, PYRAMID_BASE, PYRAMID_HEIGHT);
    let edges = sloped_edges(&model, PYRAMID_BASE, PYRAMID_HEIGHT);
    assert_eq!(edges.len(), 4, "pyramid has exactly 4 sloped edges");

    let opts = ChamferOptions {
        chamfer_type: ChamferType::EqualDistance(CHAMFER_OFFSET),
        distance1: CHAMFER_OFFSET,
        distance2: CHAMFER_OFFSET,
        symmetric: true,
        propagation: PropagationMode::None,
        preserve_edges: false,
        common: CommonOptions {
            validate_result: false,
            ..Default::default()
        },
    };
    let face_ids = chamfer_edges(&mut model, solid_id, edges, opts)
        .expect("Chamfer-β four-edge apex chamfer on a square pyramid succeeds");

    // 4 chamfer faces (one per sloped edge) + 1 planar quad cap.
    assert_eq!(
        face_ids.len(),
        5,
        "expected 5 produced faces (4 chamfer + 1 quad cap); got {}",
        face_ids.len()
    );

    let cap_face_id = find_planar_cap_face(&model, &face_ids, 4);
    let cap_face = model.faces.get(cap_face_id).expect("cap face exists");

    let cap_surface = model
        .surfaces
        .get(cap_face.surface_id)
        .expect("cap surface exists");
    assert!(
        cap_surface.as_any().downcast_ref::<Plane>().is_some(),
        "cap face surface must be a Plane"
    );

    let outer = model
        .loops
        .get(cap_face.outer_loop)
        .expect("cap outer loop");
    assert_eq!(
        outer.edges.len(),
        4,
        "cap outer loop must have exactly 4 edges; got {}",
        outer.edges.len()
    );
    for &edge_id in &outer.edges {
        let edge = model.edges.get(edge_id).expect("cap edge exists");
        let curve = model
            .curves
            .get(edge.curve_id)
            .expect("cap edge curve exists");
        assert!(
            curve.as_any().downcast_ref::<Line>().is_some(),
            "cap edge {:?} curve must be a Line",
            edge_id
        );

        // Each cap edge must be referenced by exactly 2 faces of
        // the outer shell: the planar cap + one chamfer face.
        let refs = faces_referencing_edge(&model, solid_id, edge_id);
        assert_eq!(
            refs, 2,
            "each cap edge must be referenced by exactly 2 faces \
             after Chamfer-β apex closure; edge {:?} has refs = {}",
            edge_id, refs
        );
    }

    // Watertightness — the closed manifold invariant after the
    // degree-4 cap synthesis.
    let (v, e, f) = shell_census(&model, solid_id);
    let euler = v as i64 - e as i64 + f as i64;
    assert_eq!(
        euler, 2,
        "outer shell must satisfy V − E + F = 2 after Chamfer-β \
         apex closure; got V={}, E={}, F={}, V−E+F={}",
        v, e, f, euler
    );
    assert_eq!(
        non_manifold_edge_count(&model, solid_id),
        0,
        "after Chamfer-β apex closure the outer shell must be a \
         closed watertight manifold (zero non-manifold edges)"
    );
}

#[test]
fn chamfer_pyramid_apex_three_of_four_edges_subset() {
    // Subset case: chamfer only 3 of the 4 sloped edges. At the
    // apex `edge_indices.len() = 3`, but the adjacency union of
    // those three chamfered edges covers all 4 sloped triangular
    // faces (each chamfered edge is shared by two sloped faces;
    // the three chamfered edges' face neighbours fully cover the
    // pyramid's slope), so `adjacent_set.len() = 4`. The cap-emit
    // consistency gate (`adjacent_set.len() != edge_indices.len()`)
    // fires and cap synthesis is skipped — correctly, because the
    // un-chamfered fourth sloped edge still meets the apex and a
    // triangular cap would leave a non-manifold edge.
    //
    // The apex vertex is retained (it's still an endpoint of the
    // un-chamfered fourth sloped edge), and the three V-side cap
    // edges form an open boundary.
    let mut model = BRepModel::new();
    let solid_id = make_square_pyramid_solid(&mut model, PYRAMID_BASE, PYRAMID_HEIGHT);
    let mut edges = sloped_edges(&model, PYRAMID_BASE, PYRAMID_HEIGHT);
    edges.pop(); // drop the last sloped edge — chamfer 3 of 4
    assert_eq!(edges.len(), 3);

    let opts = ChamferOptions {
        chamfer_type: ChamferType::EqualDistance(CHAMFER_OFFSET),
        distance1: CHAMFER_OFFSET,
        distance2: CHAMFER_OFFSET,
        symmetric: true,
        propagation: PropagationMode::None,
        preserve_edges: false,
        common: CommonOptions {
            validate_result: false,
            ..Default::default()
        },
    };
    let face_ids = chamfer_edges(&mut model, solid_id, edges, opts)
        .expect("Chamfer-β three-of-four-edge apex chamfer on a square pyramid succeeds");

    // Exactly 3 chamfer faces returned; no cap face emitted (the
    // consistency gate rejects the inconsistent-adjacency apex).
    assert_eq!(
        face_ids.len(),
        3,
        "expected 3 produced faces (3 chamfer, no cap); got {}",
        face_ids.len()
    );
    for &fid in &face_ids {
        let face = model.faces.get(fid).expect("face exists");
        let outer = model.loops.get(face.outer_loop).expect("face outer loop");
        assert_ne!(
            outer.edges.len(),
            3,
            "no triangular cap face must be emitted at an \
             inconsistent-adjacency apex (face {:?} has {} \
             outer-loop edges; chamfer faces are 4-sided)",
            fid,
            outer.edges.len()
        );
    }

    // The apex vertex stays alive because the un-chamfered fourth
    // sloped edge still references it.
    let apex_alive = model
        .vertices
        .iter()
        .any(|(_, v)| v.position[2] == PYRAMID_HEIGHT && v.position[0].abs() < 1.0e-9 && v.position[1].abs() < 1.0e-9);
    assert!(
        apex_alive,
        "apex vertex must remain in the model — it's still incident \
         to the un-chamfered fourth sloped edge"
    );

    // The deliberate open boundary at the apex: at least 3
    // non-manifold edges (the three V-side cap edges of the
    // chamfer faces) bound the un-closed apex hole.
    let non_manifold = non_manifold_edge_count(&model, solid_id);
    assert!(
        non_manifold >= 3,
        "three-of-four-sloped chamfer is deliberately non-watertight \
         in Chamfer-β (consistency gate rejects the apex; the three \
         V-side cap edges form an open hole); expected ≥ 3 \
         non-manifold edges, got {}",
        non_manifold
    );
}
