//! Volume + watertightness + validity invariants for the draft operation.
//!
//! Drafting tilts a face about a neutral plane by a pull angle. For the
//! prismatic-planar case the kernel now drafts IN PLACE (shears the face's
//! off-neutral vertices along the face normal by `distance·tanθ`), so:
//!   * the result is a VALID MANIFOLD B-Rep (no boundary edges),
//!   * it is watertight (tessellated divergence == reported mass-properties),
//!   * drafting about the face's mid-plane is volume-neutral (the wedge added
//!     above the neutral plane equals the wedge removed below), and
//!   * drafting about an off-centre neutral plane changes the volume by the
//!     wedge `tanθ · W · H²/2` (a clean analytic oracle).
//!
//! Draft had NO behavioural coverage before this (its in-module test module is
//! commented out) — and the first version of these tests is what surfaced the
//! original non-manifold bug (now fixed). The volume checks also guard against
//! the operation silently no-op'ing.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::draft::{DraftType, NeutralElement};
use geometry_engine::operations::{apply_draft, CommonOptions, DraftOptions};
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams, TriangleMesh};

/// Largest x-coordinate over all vertices of the solid's outer shell — used to
/// witness that draft actually deformed the geometry (not a silent no-op).
fn max_x(model: &BRepModel, solid_id: SolidId) -> f64 {
    let solid = model.solids.get(solid_id).expect("solid");
    let shell = model.shells.get(solid.outer_shell).expect("shell");
    let mut mx = f64::MIN;
    for &face_id in &shell.faces {
        let face = model.faces.get(face_id).expect("face");
        let lp = model.loops.get(face.outer_loop).expect("loop");
        for &eid in &lp.edges {
            let e = model.edges.get(eid).expect("edge");
            for vid in [e.start_vertex, e.end_vertex] {
                mx = mx.max(model.vertices.get(vid).expect("v").position[0]);
            }
        }
    }
    mx
}

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
    box_with_plus_x_face_dims(10.0, 10.0, 10.0)
}

/// Build a `w×h×d` box (centred at origin) and return it with the FaceId of its
/// +X face, located by surface normal rather than face ordering.
fn box_with_plus_x_face_dims(w: f64, h: f64, d: f64) -> (BRepModel, SolidId, FaceId) {
    let mut model = BRepModel::new();
    let solid_id = match TopologyBuilder::new(&mut model)
        .create_box_3d(w, h, d)
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
    let mx_before = max_x(&model, solid_id);
    let opts = DraftOptions {
        common: CommonOptions {
            // The in-place prismatic draft produces a valid manifold B-Rep, so
            // full result validation is ON.
            validate_result: true,
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

    // The draft must actually deform the solid — the +X face tilts, so its top
    // corners push past x = 5 (guards against a silent no-op).
    assert!(
        max_x(&model, solid_id) > mx_before + 0.1,
        "draft must deform the solid: max-x stayed at {mx_before}"
    );
    // Volume-neutral about the mid-plane (wedge added above = wedge removed below).
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

/// Drafting the +X face about a neutral plane at the BOTTOM (z = -5) tilts the
/// whole face one way, so the volume changes by the analytic wedge
/// `tanθ · W · H²/2` (W = 10 face width in y, H = 10 height). For the default
/// 5° angle that is `tan5° · 10 · 100/2 ≈ 43.7`.
#[test]
fn draft_off_midplane_changes_volume_by_wedge_oracle() {
    let (mut model, solid_id, face) = box_with_plus_x_face();
    let angle = 5.0_f64.to_radians();
    let opts = DraftOptions {
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        neutral: NeutralElement::Plane(Point3::new(0.0, 0.0, -5.0), Vector3::Z),
        pull_direction: Vector3::Z,
        ..Default::default()
    };
    apply_draft(&mut model, solid_id, vec![face], opts).expect("off-centre draft must succeed");

    let vol = model
        .mass_properties_for(solid_id)
        .expect("mass props")
        .volume;
    let wedge = angle.tan() * 10.0 * 100.0 / 2.0; // tanθ · W · H²/2
    let expected = 1000.0 + wedge; // +X face shears outward ⇒ volume grows
    assert!(
        rel_close(vol, expected, 0.03),
        "off-centre draft volume {vol} vs wedge oracle {expected} (wedge {wedge})"
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

// REGRESSION GUARD (was a bug, now FIXED 2026-06-02). The first version of this
// test surfaced that `apply_draft` left the drafted solid non-manifold — three
// boundary edges (single-use edges) on the drafted face, because the legacy
// path minted orphan geometry and never re-stitched neighbours (the explicit
// `update_adjacent_faces_for_draft` stub). The fix drafts prismatic-planar
// faces IN PLACE (shear the existing off-neutral vertices), so all shared
// topology is preserved by construction — manifold with no boundary edges.
// This test now PASSES; keep it as the guard that the in-place path stays valid.
#[test]
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

use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(40))]

    /// Off-centre draft (neutral plane at the box bottom, z = -d/2) over a
    /// randomized range of angles and box sizes. The +X face shears outward,
    /// so the volume grows by the analytic wedge `tanθ · W · H²/2`, where for
    /// `create_box_3d(w, h, d)` the +X face has width `W = h` (its y-extent)
    /// and shear height `H = d` (its z-extent, the full distance from the
    /// bottom neutral plane). Every case is validated (manifold) at
    /// construction.
    #[test]
    fn prop_draft_offcentre_wedge_volume(
        angle_deg in 1.0f64..12.0,
        w in 4.0f64..12.0,
        h in 4.0f64..12.0,
        d in 4.0f64..12.0,
    ) {
        let (mut model, solid_id, face) = box_with_plus_x_face_dims(w, h, d);
        let angle = angle_deg.to_radians();
        let opts = DraftOptions {
            common: CommonOptions {
                validate_result: true,
                ..Default::default()
            },
            draft_type: DraftType::Angle(angle),
            neutral: NeutralElement::Plane(Point3::new(0.0, 0.0, -d / 2.0), Vector3::Z),
            pull_direction: Vector3::Z,
            ..Default::default()
        };
        apply_draft(&mut model, solid_id, vec![face], opts)
            .expect("off-centre prismatic draft must produce a valid manifold solid");

        let vol = model
            .mass_properties_for(solid_id)
            .expect("mass props")
            .volume;
        let expected = w * h * d + angle.tan() * h * d * d / 2.0;
        prop_assert!(
            rel_close(vol, expected, 0.02),
            "draft {w}x{h}x{d} @ {angle_deg}°: volume {vol} vs wedge oracle {expected}"
        );
    }
}
