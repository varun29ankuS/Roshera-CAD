// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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

#[path = "blend_fixtures/mod.rs"]
mod blend_fixtures;

use blend_fixtures::{
    faces_referencing_edge, find_planar_cap_face, make_square_pyramid_solid,
    non_manifold_edge_count, shell_census, vertex_at,
};

use geometry_engine::operations::chamfer::{ChamferOptions, ChamferType, PropagationMode};
use geometry_engine::operations::{chamfer_edges, CommonOptions};
use geometry_engine::primitives::curve::Line;
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::surface::Plane;
use geometry_engine::primitives::topology_builder::BRepModel;

const PYRAMID_BASE: f64 = 10.0;
const PYRAMID_HEIGHT: f64 = 10.0;
const CHAMFER_OFFSET: f64 = 0.5;

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
        partial_corner_vertices: Vec::new(),
        common: CommonOptions {
            validate_result: false,
            ..Default::default()
        },
        ..ChamferOptions::default()
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

    let cap_face_id = find_planar_cap_face(&model, &face_ids, Some(4));
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
        partial_corner_vertices: Vec::new(),
        common: CommonOptions {
            validate_result: false,
            ..Default::default()
        },
        ..ChamferOptions::default()
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
    let apex_alive = model.vertices.iter().any(|(_, v)| {
        v.position[2] == PYRAMID_HEIGHT
            && v.position[0].abs() < 1.0e-9
            && v.position[1].abs() < 1.0e-9
    });
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
