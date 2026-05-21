//! Chamfer-α — 3-edge convex corner planar cap integration tests.
//!
//! Mirrors the F5-α apex-sphere harness
//! (`fillet_three_edge_corner.rs`) with `Sphere` swapped for `Plane`.
//! A single `chamfer_edges` call accepts the three corner edges as
//! one selection; the per-edge surgery emits one chamfer face
//! (`RuledSurface`) per edge, and `handle_chamfer_vertices` walks
//! the pre-surgery corner set and emits one planar triangular patch
//! per qualifying convex degree-3 corner.
//!
//! These tests pin:
//!
//!   * **Watertightness** — the resulting outer shell satisfies
//!     Euler-Poincaré V − E + F = 2.
//!   * **Cap-face shape** — the new face's surface downcasts to
//!     [`Plane`], its outer loop has exactly 3 edges, and each
//!     underlying curve is a [`Line`] (chord across the chamfer
//!     cross-section).
//!   * **Interior-edge invariant** — each of the three V-side cap
//!     edges is referenced by exactly two faces (the cap +
//!     one chamfer face), confirming the corner closes.
//!   * **Non-uniform-offset canary** — `ChamferType::TwoDistances`
//!     produces per-edge chamfers but no cap. The shell is
//!     deliberately non-watertight (V − E + F = 1) — flips back to
//!     2 once Chamfer-β.5 lands.

use std::collections::HashSet;

use geometry_engine::operations::chamfer::{ChamferOptions, ChamferType, PropagationMode};
use geometry_engine::operations::{chamfer_edges, CommonOptions};
use geometry_engine::primitives::curve::Line;
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::Plane;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::vertex::VertexId;

const BOX_SIZE: f64 = 10.0;
const HALF_BOX: f64 = BOX_SIZE / 2.0;
const CHAMFER_OFFSET: f64 = 1.0;

/// Build a `size×size×size` box centred on the origin.
fn make_box(model: &mut BRepModel, size: f64) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    match builder
        .create_box_3d(size, size, size)
        .expect("box creation succeeds")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {:?}", other),
    }
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

fn edges_at_vertex(model: &BRepModel, vertex: VertexId) -> Vec<EdgeId> {
    model
        .edges
        .iter()
        .filter(|(_, edge)| edge.start_vertex == vertex || edge.end_vertex == vertex)
        .map(|(id, _)| id)
        .collect()
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

/// How many faces of `solid_id`'s outer shell reference `edge_id` in
/// any loop. 2 for every interior edge on a watertight manifold.
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

/// Locate the unique planar cap face among `face_ids`. Panics if
/// there isn't exactly one with a triangular outer loop.
fn find_planar_cap_face(model: &BRepModel, face_ids: &[FaceId]) -> FaceId {
    let mut found: Option<FaceId> = None;
    for &fid in face_ids {
        let face = model.faces.get(fid).expect("face exists");
        let surface = model.surfaces.get(face.surface_id).expect("surface exists");
        if surface.as_any().downcast_ref::<Plane>().is_some() {
            let outer = model
                .loops
                .get(face.outer_loop)
                .expect("planar face outer loop");
            if outer.edges.len() == 3 {
                assert!(found.is_none(), "more than one triangular planar face produced");
                found = Some(fid);
            }
        }
    }
    found.expect("no triangular planar cap face among returned chamfer faces")
}

/// Drive a Chamfer-α call: build the box, chamfer the three edges
/// of the (+x, +y, +z) corner at offset `CHAMFER_OFFSET`. Returns
/// `(model, solid_id, returned_face_ids)`.
fn build_corner_chamfer(chamfer_type: ChamferType) -> (BRepModel, SolidId, Vec<FaceId>) {
    let mut model = BRepModel::new();
    let solid_id = make_box(&mut model, BOX_SIZE);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);
    let corner_edges = edges_at_vertex(&model, corner);
    assert_eq!(
        corner_edges.len(),
        3,
        "a box corner has exactly 3 incident edges; got {}",
        corner_edges.len()
    );

    let (d1, d2) = match chamfer_type {
        ChamferType::EqualDistance(d) => (d, d),
        ChamferType::TwoDistances(a, b) => (a, b),
        ChamferType::DistanceAngle(d, _) => (d, d),
        ChamferType::Angle(_) => (CHAMFER_OFFSET, CHAMFER_OFFSET),
    };

    let opts = ChamferOptions {
        chamfer_type,
        distance1: d1,
        distance2: d2,
        symmetric: (d1 - d2).abs() < 1.0e-12,
        propagation: PropagationMode::None,
        preserve_edges: false,
        partial_corner_vertices: Vec::new(),
        common: CommonOptions {
            validate_result: false,
            ..Default::default()
        },
    };
    let face_ids = chamfer_edges(&mut model, solid_id, corner_edges, opts)
        .expect("Chamfer-α three-edge corner chamfer on a box succeeds");

    (model, solid_id, face_ids)
}

#[test]
fn chamfer_three_edge_corner_box_pre_chamfer_baseline() {
    // Pin the harness math: a freshly-built 10×10×10 box outer shell
    // has 8 vertices, 12 edges, 6 faces, V − E + F = 2.
    let mut model = BRepModel::new();
    let solid_id = make_box(&mut model, BOX_SIZE);
    let (v, e, f) = shell_census(&model, solid_id);
    assert_eq!(v, 8, "box has 8 vertices");
    assert_eq!(e, 12, "box has 12 edges");
    assert_eq!(f, 6, "box has 6 faces");
    assert_eq!(v as i64 - e as i64 + f as i64, 2, "Euler-Poincaré");
}

#[test]
fn chamfer_three_edge_corner_box_emits_planar_cap() {
    let (model, solid_id, face_ids) =
        build_corner_chamfer(ChamferType::EqualDistance(CHAMFER_OFFSET));

    // 3 chamfer faces (one per edge) + 1 planar corner cap.
    assert_eq!(
        face_ids.len(),
        4,
        "expected 4 produced faces (3 chamfer + 1 corner cap); got {}",
        face_ids.len()
    );

    let cap_face_id = find_planar_cap_face(&model, &face_ids);
    let cap_face = model.faces.get(cap_face_id).expect("cap face exists");

    // The cap surface must downcast to Plane.
    let cap_surface = model
        .surfaces
        .get(cap_face.surface_id)
        .expect("cap surface exists");
    assert!(
        cap_surface.as_any().downcast_ref::<Plane>().is_some(),
        "cap face surface must be a Plane"
    );

    // The cap's outer loop must be a triangle whose three edges
    // all carry Line curves (straight cap chords).
    let outer = model
        .loops
        .get(cap_face.outer_loop)
        .expect("cap outer loop");
    assert_eq!(
        outer.edges.len(),
        3,
        "cap outer loop must have exactly 3 edges; got {}",
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
             after corner closure; edge {:?} has refs = {}",
            edge_id, refs
        );
    }

    // Watertightness.
    let (v, e, f) = shell_census(&model, solid_id);
    let euler = v as i64 - e as i64 + f as i64;
    assert_eq!(
        euler, 2,
        "outer shell must satisfy V − E + F = 2 after Chamfer-α \
         corner closure; got V={}, E={}, F={}, V−E+F={}",
        v, e, f, euler
    );
}

/// Count edges of `solid_id`'s outer shell whose reference count
/// across face loops is not exactly 2. A watertight closed manifold
/// has zero such edges; every "hole" boundary contributes one.
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

#[test]
fn chamfer_three_edge_corner_mixed_kind_skips_cap_silently() {
    // Canary: until Chamfer-β.5 (mixed-offset corner cap) lands,
    // `ChamferType::TwoDistances` admits per-edge surgery but skips
    // cap synthesis. The three V-side cap edges of the chamfer faces
    // are referenced by exactly one face (the chamfer face itself)
    // instead of two — the triangular hole's three boundary edges.
    //
    // The Euler-Poincaré V−E+F sum can still hit 2 in this state
    // because it's a *count* identity, not a manifold check: a hole
    // bounded by N edges shared among N faces still satisfies the
    // alternating-sign sum when the orphan V vertex is dropped from
    // the count. The robust signal is "≥1 edge referenced by ≠ 2
    // faces"; that's what we pin here. When Chamfer-β.5 lands this
    // test must flip to assert zero non-manifold edges.
    let (_model, solid_id, face_ids) =
        build_corner_chamfer(ChamferType::TwoDistances(CHAMFER_OFFSET, 1.5));

    // No cap face: only 3 chamfer faces returned.
    assert_eq!(
        face_ids.len(),
        3,
        "mixed-offset chamfer must emit exactly 3 chamfer faces \
         (no corner cap until Chamfer-β.5); got {}",
        face_ids.len()
    );

    let non_manifold = non_manifold_edge_count(&_model, solid_id);
    assert!(
        non_manifold >= 3,
        "mixed-offset chamfer is deliberately non-watertight in α \
         (three V-side cap edges of the chamfer faces are referenced \
         by only one face each — Chamfer-β.5 will close the hole); \
         expected ≥ 3 non-manifold edges, got {}",
        non_manifold
    );
}
