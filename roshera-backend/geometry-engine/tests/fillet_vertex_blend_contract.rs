//! Contract-pinning regression tests for `fillet_vertices` (Task #82
//! / Task #89 slice C).
//!
//! A complete vertex blend (spherical corner fillet) requires:
//!
//!   1. A sphere of the requested radius centered on the vertex.
//!   2. Surface/surface intersection of that sphere against each
//!      adjacent edge-fillet surface (cylinder for constant-radius
//!      edge fillets, torus for variable-radius), producing the
//!      trimming curves on the sphere.
//!   3. Re-trimming of each adjacent edge-fillet face against the
//!      sphere so the seams meet.
//!   4. Multi-way topology surgery to stitch the new spherical patch
//!      into the shell without leaving boundary edges.
//!
//! That is a substantial multi-slice piece of kernel work, tracked
//! separately as Task #82. Slice C of Task #89 does NOT implement it.
//!
//! What slice C DOES is pin the contract of the current
//! `fillet_vertices` entry point so that any future regression which
//! silently "succeeds" by producing a face with no trimming loop —
//! topologically invalid, downstream-corrupting — is caught
//! immediately. The contract pinned here:
//!
//!   * Valid input (existing vertex, positive radius) returns
//!     `OperationError::NotImplemented`. The model is **not** mutated:
//!     no faces, edges, or vertices are added or removed before the
//!     error is returned.
//!   * Zero / negative radius is rejected by
//!     `validate_vertex_fillet_inputs` with `InvalidRadius` *before*
//!     reaching `create_vertex_blend` — so the error channel for bad
//!     radius stays distinct from the not-implemented channel.
//!   * Unknown vertex IDs are rejected by the same validator with
//!     `InvalidGeometry`.
//!   * An empty vertex list is a no-op that returns
//!     `Ok(vec![])` — the for loop in `fillet_vertices` never enters,
//!     so the NotImplemented branch is never reached.
//!
//! When Task #82 lands the first three of these tests will need to be
//! revisited; the last (empty-list no-op) is a permanent invariant of
//! the public API.

use geometry_engine::operations::fillet::{FilletType, PropagationMode};
use geometry_engine::operations::{fillet_vertices, FilletOptions, OperationError};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::vertex::VertexId;

fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    match builder
        .create_box_3d(w, h, d)
        .expect("box creation succeeds")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {:?}", other),
    }
}

fn first_vertex(model: &BRepModel) -> VertexId {
    model
        .vertices
        .iter()
        .map(|(id, _)| id)
        .next()
        .expect("box must have at least one vertex")
}

fn default_opts() -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(0.5),
        radius: 0.5,
        propagation: PropagationMode::None,
        ..Default::default()
    }
}

/// Snapshot of the topology counts that vertex blend must not perturb
/// when it returns an error. If `fillet_vertices` ever starts
/// partially mutating the model before failing — e.g. by inserting a
/// half-trimmed sphere face — these counts will diverge and the
/// `assert_eq` below will catch it.
#[derive(Debug, PartialEq, Eq)]
struct TopologyCensus {
    vertices: usize,
    edges: usize,
    faces: usize,
    loops: usize,
    solids: usize,
}

fn census(model: &BRepModel) -> TopologyCensus {
    TopologyCensus {
        vertices: model.vertices.len(),
        edges: model.edges.len(),
        faces: model.faces.len(),
        loops: model.loops.len(),
        solids: model.solids.len(),
    }
}

#[test]
fn fillet_vertices_rejects_vertex_without_pre_filleted_edges() {
    // Slice 1 of Task #82 (Task #104) changed the contract for a
    // raw vertex on an un-filleted polyhedron. The vertex-blend
    // pipeline now requires that the 3 (or more) edges meeting at
    // the corner have already been rounded by `fillet_edges` — the
    // sphere is tangent to the resulting cylindrical fillet faces,
    // so there must be cylindrical fillet faces to be tangent *to*.
    //
    // A bare box vertex has 3 incident edges, all adjacent to two
    // planar faces (no cylinder). Slice-1 classification finds 0
    // filleted incidents and rejects with InvalidGeometry whose
    // message names the precondition. The model must not be
    // mutated.
    let mut model = BRepModel::new();
    let solid = make_box(&mut model, 4.0, 4.0, 4.0);
    let vertex = first_vertex(&model);
    let before = census(&model);

    let result = fillet_vertices(&mut model, solid, vec![vertex], 0.5, default_opts());

    let err = result.expect_err(
        "bare box vertex (no prior edge fillets) must fail slice-1 classification",
    );
    let msg = match &err {
        OperationError::InvalidGeometry(m) => m.clone(),
        other => panic!("expected InvalidGeometry; got {other:?}"),
    };
    assert!(
        msg.contains("filleted incident edge"),
        "expected diagnostic to name the missing-fillet precondition; got: {msg}"
    );

    // The model must not be partially mutated when classification
    // fails. This is the critical invariant — a half-applied vertex
    // blend would leave the caller with a corrupt B-Rep and no way
    // to recover.
    let after = census(&model);
    assert_eq!(
        before, after,
        "fillet_vertices must not mutate topology when classification fails; \
         before = {before:?}, after = {after:?}"
    );
}

#[test]
fn fillet_vertices_rejects_zero_radius_before_not_implemented() {
    // Zero radius is rejected by `validate_vertex_fillet_inputs`
    // before `create_vertex_blend` is reached. This keeps the error
    // channels separated — a user passing radius=0 must see
    // InvalidRadius, not NotImplemented (which would be a misleading
    // diagnostic).
    let mut model = BRepModel::new();
    let solid = make_box(&mut model, 4.0, 4.0, 4.0);
    let vertex = first_vertex(&model);
    let before = census(&model);

    let result = fillet_vertices(&mut model, solid, vec![vertex], 0.0, default_opts());
    let err = result.expect_err("zero radius must fail validation");
    assert!(
        matches!(err, OperationError::InvalidRadius(r) if r == 0.0),
        "expected InvalidRadius(0.0); got {err:?}"
    );

    let after = census(&model);
    assert_eq!(before, after, "validation failure must not mutate topology");
}

#[test]
fn fillet_vertices_rejects_negative_radius_before_not_implemented() {
    // Mirror of the zero-radius case for the strictly-negative branch
    // of the `radius <= 0.0` guard in `validate_vertex_fillet_inputs`.
    let mut model = BRepModel::new();
    let solid = make_box(&mut model, 4.0, 4.0, 4.0);
    let vertex = first_vertex(&model);
    let before = census(&model);

    let result = fillet_vertices(&mut model, solid, vec![vertex], -1.5, default_opts());
    let err = result.expect_err("negative radius must fail validation");
    assert!(
        matches!(err, OperationError::InvalidRadius(r) if r == -1.5),
        "expected InvalidRadius(-1.5); got {err:?}"
    );

    let after = census(&model);
    assert_eq!(before, after, "validation failure must not mutate topology");
}

#[test]
fn fillet_vertices_rejects_unknown_vertex_id() {
    // An unknown vertex id is rejected with InvalidGeometry, not
    // NotImplemented — confirming the validator runs first. The
    // chosen id (u32::MAX) is overwhelmingly unlikely to collide
    // with a real id assigned by VertexStore.
    let mut model = BRepModel::new();
    let solid = make_box(&mut model, 4.0, 4.0, 4.0);
    let before = census(&model);

    let bogus: VertexId = u32::MAX;
    let result = fillet_vertices(&mut model, solid, vec![bogus], 0.5, default_opts());
    let err = result.expect_err("unknown vertex must fail validation");
    assert!(
        matches!(err, OperationError::InvalidGeometry(_)),
        "expected InvalidGeometry for unknown vertex; got {err:?}"
    );

    let after = census(&model);
    assert_eq!(before, after, "validation failure must not mutate topology");
}

#[test]
fn fillet_vertices_empty_list_is_noop_success() {
    // An empty vertex list is a no-op: validation passes (the for
    // loops in `validate_vertex_fillet_inputs` are empty), the main
    // loop in `fillet_vertices` is empty, and `create_vertex_blend`
    // is never reached. The function returns Ok(vec![]) and the
    // model is untouched. This is a permanent invariant of the API
    // — it will still hold once Task #82 implements vertex blend.
    let mut model = BRepModel::new();
    let solid = make_box(&mut model, 4.0, 4.0, 4.0);
    let before = census(&model);

    let faces = fillet_vertices(&mut model, solid, vec![], 0.5, default_opts())
        .expect("empty vertex list must succeed as a no-op");
    assert!(
        faces.is_empty(),
        "empty vertex list must yield an empty face vec; got {} face(s)",
        faces.len()
    );

    let after = census(&model);
    assert_eq!(before, after, "no-op call must not mutate topology");
}

#[test]
fn fillet_vertices_rejects_unknown_solid_id() {
    // Unknown solid id is rejected by the validator before any vertex
    // lookups happen. Same channel as unknown vertex.
    let mut model = BRepModel::new();
    let _solid = make_box(&mut model, 4.0, 4.0, 4.0);
    let vertex = first_vertex(&model);
    let before = census(&model);

    let bogus_solid: SolidId = u32::MAX;
    let result = fillet_vertices(&mut model, bogus_solid, vec![vertex], 0.5, default_opts());
    let err = result.expect_err("unknown solid must fail validation");
    assert!(
        matches!(err, OperationError::InvalidGeometry(_)),
        "expected InvalidGeometry for unknown solid; got {err:?}"
    );

    let after = census(&model);
    assert_eq!(before, after, "validation failure must not mutate topology");
}
