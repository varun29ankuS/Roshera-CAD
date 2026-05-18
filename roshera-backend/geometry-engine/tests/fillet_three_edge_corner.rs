//! F5-α — three-edge convex equal-radius ball corner integration tests.
//!
//! Architecture pin: F5-α follows the OCCT
//! (`BRepFilletAPI_MakeFillet`) / Parasolid (`PK_BODY_blend_fcs` +
//! `PK_blend_overlap_apex_sphere_c`) inline pattern. A single
//! `fillet_edges` call accepts the three corner edges as one
//! selection; the dispatcher emits per-edge cylindrical fillets,
//! retracts each spine via the BlendGraph setbacks so the cap arcs
//! end on the apex circle, then `create_fillet_transitions` walks
//! the BlendGraph and emits one spherical face per
//! `ConvexCorner { degree: 3 }` corner. No separate
//! `fillet_vertices` API.
//!
//! These tests pin:
//!
//!   * **Watertightness** — the resulting outer shell satisfies the
//!     Euler-Poincaré relation V − E + F = 2 (genus-zero, no
//!     boundary). The headline end-to-end check: if the surgery is
//!     wrong, the solid has a boundary triangle and this test
//!     fails.
//!   * **Sphere placement** — the new face's surface downcasts to
//!     [`Sphere`] with the analytic centre (5−r, 5−r, 5−r) for the
//!     (+x, +y, +z) corner of a 10×10×10 box centred on the origin,
//!     and the radius is stored bit-exactly.
//!   * **Interior-edge invariant** — the three edges of the new
//!     sphere face's loop are exactly the three V-side cap arcs of
//!     the cylindrical fillet faces; after the surgery each of
//!     those edges is used by exactly two faces (sphere face + one
//!     cylindrical fillet), confirming the corner is closed.
//!
//! ## F5-α slice map
//!
//! * **F5-α.1** (commit `feat(fillet): F5-α — infrastructure …`)
//!   landed everything *up to* the splice: lifecycle gate
//!   relaxation, BlendGraph wiring, the
//!   `create_fillet_transitions` dispatcher, the
//!   `apply_apex_sphere_corner` helper (with the surgery body), and
//!   the `MixedRadii` diagnostic.
//!
//! * **F5-α.2** (this slice — corner-aware splice) threads a
//!   per-endpoint `corner_shared` flag into `BlendEdgeSurgery` and
//!   makes `splice_blend_edge` skip the V-side
//!   `find_third_face_at_vertex` / cap insertion / vertex removal /
//!   pred-succ rewire when that endpoint is the shared corner.
//!   With F5-α.2, three corner-sharing per-edge fillets all
//!   referencing the same `original_v0` no longer abort
//!   `validate_surgery` on the second splice.
//!
//! * **F5-α.3** (next slice — apex-aware setbacks) refines
//!   `blend_graph::compute_setbacks` so that, for
//!   `BlendVertexKind::ConvexCorner { degree: 3 }` (and the
//!   concurrent-axes case generally), the per-edge setback retracts
//!   the spine to the apex sphere centre instead of the Hoffmann
//!   smooth-closure point `r·cos(θ_min/2)`. Without this, each
//!   cylinder fillet's V-side cap arc lands at `r·(1 − cos(θ/2))`
//!   short of the apex (e.g. `0.293·r` for a 90° corner), and
//!   `apply_apex_sphere_corner` rejects with
//!   `VertexBlendUnsupportedReason::NonManifoldNeighbourhood`
//!   because `find_cap_arc_edge_at_vertex` looks for centre
//!   coincidence with the apex sphere centre. F5-α.2 is necessary
//!   but not sufficient.
//!
//! Each `#[ignore]` below points at F5-α.3 so that
//! `cargo test --test fillet_three_edge_corner` is silent in the
//! interim and `cargo test -- --ignored fillet_three_edge_corner`
//! re-arms the contract for the next slice.

use std::collections::HashSet;

use geometry_engine::operations::fillet::{FilletType, PropagationMode};
use geometry_engine::operations::{fillet_edges, CommonOptions, FilletOptions};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::Sphere;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::vertex::VertexId;

const BOX_SIZE: f64 = 10.0;
const HALF_BOX: f64 = BOX_SIZE / 2.0;
const FILLET_RADIUS: f64 = 1.0;

/// Build a `size×size×size` box centred on the origin (corners at
/// `(±size/2, ±size/2, ±size/2)`).
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

/// Find the vertex whose world position matches `(x, y, z)` within
/// `1e-9`.
fn vertex_at(model: &BRepModel, x: f64, y: f64, z: f64) -> VertexId {
    for (id, vertex) in model.vertices.iter() {
        let p = vertex.position;
        if (p[0] - x).abs() < 1.0e-9 && (p[1] - y).abs() < 1.0e-9 && (p[2] - z).abs() < 1.0e-9 {
            return id;
        }
    }
    panic!("no vertex at ({}, {}, {})", x, y, z);
}

/// Edges currently incident to `vertex` (start or end).
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
/// closed-surface invariant — the F5-α watertightness pin.
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

/// How many faces of `solid_id`'s outer shell reference `edge_id`
/// in any of their loops. For a watertight closed manifold this is
/// 2 for every interior edge.
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

/// Find the unique sphere face among `face_ids`. Panics if there
/// isn't exactly one — F5-α emits one spherical patch per
/// degree-3 convex corner.
fn find_sphere_face(model: &BRepModel, face_ids: &[FaceId]) -> FaceId {
    let mut found: Option<FaceId> = None;
    for &fid in face_ids {
        let face = model.faces.get(fid).expect("face exists");
        let surface = model.surfaces.get(face.surface_id).expect("surface exists");
        if surface.as_any().downcast_ref::<Sphere>().is_some() {
            assert!(found.is_none(), "more than one sphere face produced");
            found = Some(fid);
        }
    }
    found.expect("no sphere face among returned fillet faces")
}

/// Drive the F5-α single-call workflow: build a box, fillet the
/// three edges of the (+x, +y, +z) corner with one `fillet_edges`
/// call at constant radius `FILLET_RADIUS`. Returns
/// `(model, solid_id, returned_face_ids)`.
fn build_corner_blend() -> (BRepModel, SolidId, Vec<FaceId>) {
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

    let opts = FilletOptions {
        fillet_type: FilletType::Constant(FILLET_RADIUS),
        radius: FILLET_RADIUS,
        propagation: PropagationMode::None,
        common: CommonOptions {
            // The per-call Euler validation runs after corner
            // closure now, so it is safe to keep validate_result
            // = true. Leaving the default explicit for clarity.
            validate_result: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let face_ids = fillet_edges(&mut model, solid_id, corner_edges, opts)
        .expect("F5-α single-call three-edge corner fillet on a box succeeds");

    (model, solid_id, face_ids)
}

#[test]
fn box_corner_three_edge_fillet_produces_watertight_solid() {
    let (model, solid_id, face_ids) = build_corner_blend();

    // 3 cylindrical fillet faces (one per edge) + 1 spherical
    // corner patch.
    assert_eq!(
        face_ids.len(),
        4,
        "expected 4 produced faces (3 cylindrical fillets + 1 corner sphere); got {}",
        face_ids.len()
    );

    let (v, e, f) = shell_census(&model, solid_id);
    let euler = v as i64 - e as i64 + f as i64;
    assert_eq!(
        euler, 2,
        "outer shell must satisfy V − E + F = 2 after F5-α corner closure; \
         got V={}, E={}, F={}, V−E+F={}. A non-2 value means the sphere \
         face did not stitch correctly into the shell — typically a \
         missing cap arc on one fillet face or a cycle that didn't close.",
        v, e, f, euler
    );
}

#[test]
fn vertex_blend_sphere_face_carries_correct_centre_and_radius() {
    let (model, _solid_id, face_ids) = build_corner_blend();
    let sphere_face = find_sphere_face(&model, &face_ids);
    let face = model.faces.get(sphere_face).expect("sphere face exists");
    let surface = model
        .surfaces
        .get(face.surface_id)
        .expect("sphere face's surface exists");
    let sphere = surface
        .as_any()
        .downcast_ref::<Sphere>()
        .expect("vertex-blend face surface must be a Sphere");

    // Radius is stored bit-exactly: F5-α writes the caller-supplied
    // radius straight into `Sphere::new`.
    assert_eq!(
        sphere.radius, FILLET_RADIUS,
        "sphere radius must equal the requested fillet radius bit-exactly"
    );

    // For a 10×10×10 box centred on the origin, the (+x, +y, +z)
    // corner sits at (HALF_BOX, HALF_BOX, HALF_BOX). With three
    // equal-radius edge fillets the rolling-ball centre sits at
    // (HALF_BOX − r, HALF_BOX − r, HALF_BOX − r). The concurrent-
    // axes solver is least-squares but for a rectilinear corner the
    // residual is zero, so the centre lands within numerical noise
    // of the analytic value.
    let expected = (
        HALF_BOX - FILLET_RADIUS,
        HALF_BOX - FILLET_RADIUS,
        HALF_BOX - FILLET_RADIUS,
    );
    let dx = sphere.center.x - expected.0;
    let dy = sphere.center.y - expected.1;
    let dz = sphere.center.z - expected.2;
    let err_sq = dx * dx + dy * dy + dz * dz;
    assert!(
        err_sq < 1.0e-18,
        "sphere centre {:?} differs from analytic {:?} by √{:.3e} > 1e-9",
        sphere.center,
        expected,
        err_sq.sqrt()
    );
}

#[test]
fn vertex_blend_sphere_face_shares_three_cap_arcs_with_cylindrical_fillets() {
    let (model, solid_id, face_ids) = build_corner_blend();
    let sphere_face = find_sphere_face(&model, &face_ids);
    let face = model.faces.get(sphere_face).expect("sphere face exists");
    let outer_loop = model
        .loops
        .get(face.outer_loop)
        .expect("sphere face has outer loop");

    assert_eq!(
        outer_loop.edges.len(),
        3,
        "sphere-face outer loop must have exactly 3 edges (one cap arc \
         from each of the three incident cylindrical fillets); got {}",
        outer_loop.edges.len()
    );

    for &edge_id in &outer_loop.edges {
        let refs = faces_referencing_edge(&model, solid_id, edge_id);
        assert_eq!(
            refs, 2,
            "each cap-arc edge of the sphere face must be referenced by \
             exactly 2 faces of the outer shell after the corner closes \
             (sphere face + one cylindrical fillet face); edge {:?} has \
             refs = {}",
            edge_id, refs
        );
    }
}

#[test]
#[ignore = "Concave-corner fixtures require hand-built non-convex topology \
            (L-shape extrude); deferred to F5-δ alongside the actual concave \
            corner patch. The kernel-side rejection arm in \
            lifecycle::validate_corner_compatibility is exercised today via \
            its own unit tests."]
fn fillet_edges_rejects_concave_three_edge_corner() {
    // F5-δ: build an L-shape (e.g. union of two boxes offset by half
    // the side length) whose re-entrant corner has three incident
    // edges with concave dihedrals. Pass the three edges to
    // `fillet_edges` and assert:
    //
    //   matches!(
    //       err,
    //       OperationError::NotImplemented(_)
    //         | OperationError::BlendFailed(box BlendFailure::VertexBlendUnsupported {
    //               reason: VertexBlendUnsupportedReason::ConcaveCorner, ..
    //           })
    //   )
    //
    // The exact error variant depends on whether F5-δ adds the
    // `ConcaveCorner` reason variant; today the lifecycle gate maps
    // concave corners through the generic `NotImplemented` arm.
}
