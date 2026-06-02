//! Watertightness + validity invariants for the draft operation.
//!
//! Drafting tilts a face about a neutral plane by a pull angle. The exact
//! volume change is convention-dependent (neutral plane, pull direction), but
//! two things must always hold for a valid result: it must be watertight
//! (tessellated divergence volume == reported mass-properties volume) and the
//! volume must stay finite and positive. Draft currently has NO behavioural
//! test coverage (the in-module test module is commented out), so even these
//! basic invariants are net-new.

use geometry_engine::operations::{apply_draft, CommonOptions, DraftOptions};
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams, TriangleMesh};

fn mesh_volume(mesh: &TriangleMesh) -> f64 {
    let mut v = 0.0;
    for t in &mesh.triangles {
        let a = mesh.vertices[t[0] as usize].position;
        let b = mesh.vertices[t[1] as usize].position;
        let c = mesh.vertices[t[2] as usize].position;
        v += (a.x * (b.y * c.z - b.z * c.y) - a.y * (b.x * c.z - b.z * c.x)
            + a.z * (b.x * c.y - b.y * c.x))
            / 6.0;
    }
    v.abs()
}

/// Build a 10×10×10 box and return it with the FaceId of its +X face.
fn box_with_plus_x_face() -> (BRepModel, SolidId, FaceId) {
    let mut model = BRepModel::new();
    let solid_id = match TopologyBuilder::new(&mut model)
        .create_box_3d(10.0, 10.0, 10.0)
        .expect("create_box_3d")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    };
    let solid = model.solids.get(solid_id).expect("solid").clone();
    let shell = model
        .shells
        .get(solid.outer_shell)
        .expect("outer shell")
        .clone();
    let mut target = None;
    for &face_id in &shell.faces {
        let face = model.faces.get(face_id).expect("face");
        let surface = model.surfaces.get(face.surface_id).expect("surface");
        let n = surface.normal_at(0.5, 0.5).expect("planar normal");
        if (n.x - 1.0).abs() < 1e-9 && n.y.abs() < 1e-9 && n.z.abs() < 1e-9 {
            target = Some(face_id);
            break;
        }
    }
    (model, solid_id, target.expect("box must have a +X face"))
}

fn rel_close(a: f64, b: f64, tol: f64) -> bool {
    if b.abs() < 1e-9 {
        a.abs() <= tol
    } else {
        ((a - b) / b).abs() <= tol
    }
}

/// Drafting one side face of a symmetric box about its OWN mid-plane (z = 0,
/// the default neutral plane) tilts the face about the line where it meets
/// z = 0: the wedge of material added above z = 0 is congruent to the wedge
/// removed below, so the operation is exactly volume-neutral. The result must
/// therefore keep the original 1000 volume AND be watertight (tessellated
/// divergence == reported mass-properties).
#[test]
fn draft_about_midplane_preserves_volume_and_is_watertight() {
    let (mut model, solid_id, face) = box_with_plus_x_face();
    let opts = DraftOptions {
        common: CommonOptions {
            // NOTE: validate_result is OFF here because draft currently leaves
            // boundary edges on the drafted face (see the #[ignore]d
            // `draft_produces_a_valid_manifold_brep` below for the tracked
            // B-Rep bug). The GEOMETRY it produces is nonetheless mesh-
            // watertight and volume-neutral, which is what this test pins;
            // those checks are independent of the B-Rep validator.
            validate_result: false,
            ..Default::default()
        },
        ..Default::default()
    };
    apply_draft(&mut model, solid_id, vec![face], opts)
        .expect("drafting a box side face about its mid-plane must succeed");

    let mass_vol = model
        .mass_properties_for(solid_id)
        .expect("drafted solid mass properties")
        .volume;
    let solid = model.solids.get(solid_id).expect("drafted solid");
    let tess_vol = mesh_volume(&tessellate_solid(
        solid,
        &model,
        &TessellationParams::default(),
    ));

    // Volume-neutral about the mid-plane.
    assert!(
        rel_close(mass_vol, 1000.0, 0.01),
        "mid-plane draft of a symmetric box must preserve volume: {mass_vol} vs 1000"
    );
    // Watertightness witness.
    assert!(
        rel_close(tess_vol, mass_vol, 0.02),
        "drafted box not watertight: tess {tess_vol} vs mass-props {mass_vol}"
    );
}

/// Watertightness must survive drafting regardless of the exact volume: the
/// result's tessellated divergence volume must match its reported volume and
/// stay finite and positive. (A leaky or self-intersecting draft — the shell-
/// class failure — fails here even when the volume oracle is unavailable.)
#[test]
fn draft_result_is_watertight_and_finite() {
    let (mut model, solid_id, face) = box_with_plus_x_face();
    let opts = DraftOptions {
        common: CommonOptions {
            validate_result: false,
            ..Default::default()
        },
        ..Default::default()
    };
    apply_draft(&mut model, solid_id, vec![face], opts).expect("draft must succeed");

    let mass_vol = model
        .mass_properties_for(solid_id)
        .expect("mass props")
        .volume;
    let solid = model.solids.get(solid_id).expect("solid");
    let tess_vol = mesh_volume(&tessellate_solid(
        solid,
        &model,
        &TessellationParams::default(),
    ));
    assert!(
        mass_vol.is_finite() && mass_vol > 0.0,
        "drafted volume must be finite and positive: {mass_vol}"
    );
    assert!(
        rel_close(tess_vol, mass_vol, 0.02),
        "drafted solid not watertight: tess {tess_vol} vs mass-props {mass_vol}"
    );
}

// BUG REPRO (documented, not yet fixed) — discovered 2026-06-02 by this very
// invariant. Draft had NO behavioural test coverage (its in-module test module
// is commented out). Running it with `validate_result: true` shows that
// `apply_draft` leaves the drafted solid non-manifold: the Standard validator
// reports three BOUNDARY EDGES (single-use edges 13/14/15) on the drafted
// face's loop. So draft's topology surgery tilts the face but never re-stitches
// its new boundary edges to the adjacent faces — the adjacent faces still
// reference the OLD edges, leaving the drafted face's new edges used only once.
// This is the same single-use-edge / boundary-edge class as the torus, revolve
// and loft bugs fixed earlier. The tessellated MESH is still closed (volume
// neutral, watertight — see the two passing tests above), which is why nothing
// caught it before. Tracked for a focused draft-restitch fix; un-ignore when
// the drafted face's edges are shared with their neighbours.
#[test]
#[ignore = "draft leaves boundary edges on the drafted face (non-manifold B-Rep) — documented bug repro"]
fn draft_produces_a_valid_manifold_brep() {
    let (mut model, solid_id, face) = box_with_plus_x_face();
    let opts = DraftOptions {
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        ..Default::default()
    };
    apply_draft(&mut model, solid_id, vec![face], opts)
        .expect("drafted box must be a valid manifold B-Rep (no boundary edges)");
}
