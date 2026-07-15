// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Oracle-free volume + watertightness invariants for the shell (hollow)
//! operation (`offset_solid` with faces removed).
//!
//! Shelling an `a×b×c` box with wall thickness `t` and the +Z (top) face
//! removed produces an open box: a floor of thickness `t` and four side walls
//! of thickness `t`, with the top left open. The remaining material volume is
//!
//!     a·b·c − (a−2t)·(b−2t)·(c−t)
//!
//! (outer box minus the open cavity, whose footprint is inset by `t` on each
//! of the four sides and whose height is `c−t` because the floor — but not a
//! top — eats `t`).
//!
//! Existing shell tests check only wall *structure* (positions / counts) with
//! result validation disabled, so the actual hollow geometry's volume and
//! watertightness were never asserted — the kind of gap that hid the revolve
//! and loft bugs. These tests assert the geometry directly: the tessellated
//! divergence volume must equal both the analytic shell volume AND the kernel's
//! reported mass-properties volume (the watertightness witness).
//!
//! STATUS: these caught a real, pre-existing shell bug on first run (the inner
//! cavity faces were untrimmed copies of the outer faces — a 10×10×10 box
//! shelled at t=1 enclosed 1640 instead of 424) and now GUARD the fix. The fix
//! (offset.rs): `compute_vertex_insets` places each inner vertex at the
//! intersection of the inward-offset planes meeting there (so the cavity is
//! correctly inset), and the interior faces are orientation-flipped to face the
//! void (so the divergence subtracts the cavity). 10×10×10 now gives exactly
//! 424, watertight.

use geometry_engine::operations::{offset_solid, CommonOptions, OffsetOptions};
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams, TriangleMesh};

fn rel_close(a: f64, b: f64, tol: f64) -> bool {
    if b.abs() < 1e-9 {
        a.abs() <= tol
    } else {
        ((a - b) / b).abs() <= tol
    }
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

/// Build an `a×b×c` box (centred at origin) and return it with the FaceId of
/// its +Z (top) face, located by surface normal rather than face ordering.
fn box_with_top_face(a: f64, b: f64, c: f64) -> (BRepModel, SolidId, FaceId) {
    let mut model = BRepModel::new();
    let solid_id = match TopologyBuilder::new(&mut model)
        .create_box_3d(a, b, c)
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
    let mut top = None;
    for &face_id in &shell.faces {
        let face = model.faces.get(face_id).expect("face");
        let surface = model.surfaces.get(face.surface_id).expect("surface");
        let n = surface.normal_at(0.5, 0.5).expect("planar normal");
        if (n.z - 1.0).abs() < 1e-9 && n.x.abs() < 1e-9 && n.y.abs() < 1e-9 {
            top = Some(face_id);
            break;
        }
    }
    let top_face_id = top.expect("box must have a +Z face");
    (model, solid_id, top_face_id)
}

/// Shell an `a×b×c` box (top removed) with wall thickness `t`. Returns the
/// hollow solid's (tessellated divergence volume, reported mass-props volume).
fn shelled_open_box(a: f64, b: f64, c: f64, t: f64) -> (f64, f64) {
    let (mut model, solid_id, top_face_id) = box_with_top_face(a, b, c);
    let opts = OffsetOptions {
        // The full B-Rep validator has documented open work for shells; we
        // assert the hollow GEOMETRY (volume + watertightness) directly, which
        // does not depend on that validator, so skip it here.
        common: CommonOptions {
            validate_result: false,
            ..Default::default()
        },
        ..Default::default()
    };
    let hollow = offset_solid(&mut model, solid_id, t, vec![top_face_id], opts)
        .expect("shell of a known-good box (top removed) must succeed");
    let mp = model
        .mass_properties_for(hollow)
        .expect("shelled solid mass properties");
    let solid = model.solids.get(hollow).expect("hollow solid");
    let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
    (mesh_volume(&mesh), mp.volume)
}

/// Analytic open-top shell volume: outer box minus the open cavity.
fn open_shell_volume(a: f64, b: f64, c: f64, t: f64) -> f64 {
    a * b * c - (a - 2.0 * t) * (b - 2.0 * t) * (c - t)
}

macro_rules! shell_volume_test {
    ($name:ident, $a:expr, $b:expr, $c:expr, $t:expr) => {
        #[test]
        fn $name() {
            let (tess_vol, mass_vol) = shelled_open_box($a, $b, $c, $t);
            let expected = open_shell_volume($a, $b, $c, $t);
            // Analytic shell oracle.
            assert!(
                rel_close(tess_vol, expected, 0.03),
                "shell {}x{}x{} t={}: tess volume {} vs analytic {}",
                $a,
                $b,
                $c,
                $t,
                tess_vol,
                expected
            );
            // Watertightness witness: divergence volume = reported volume.
            assert!(
                rel_close(tess_vol, mass_vol, 0.03),
                "shell {}x{}x{} t={}: tess {} vs mass-props {} (non-watertight?)",
                $a,
                $b,
                $c,
                $t,
                tess_vol,
                mass_vol
            );
        }
    };
}

shell_volume_test!(shell_cube_10_t1, 10.0, 10.0, 10.0, 1.0);
shell_volume_test!(shell_8_6_10_t1, 8.0, 6.0, 10.0, 1.0);
shell_volume_test!(shell_cube_6_t1, 6.0, 6.0, 6.0, 1.0);
shell_volume_test!(shell_12_12_4_t1, 12.0, 12.0, 4.0, 1.0);

// BUG REPRO (documented, not yet fixed) — discovered 2026-06-02 by this very
// invariant. The shell (`offset_solid` with a face removed) produces a result
// that is watertight and self-consistent (mass-props volume == tessellated
// divergence volume) but GEOMETRICALLY WRONG: for a 10×10×10 box, t=1, top
// removed, it encloses 1640 — MORE than the solid box itself (1000), which is
// impossible for a hollow shell (analytic answer: 424).
//
// Root cause (per-face dissection of the 14-face result): the inner cavity
// faces are untrimmed, un-reoriented copies of the outer faces shifted inward
// by t. Concretely — the inner floor sits at the correct z=-4 but keeps the
// FULL 10×10 footprint instead of the inset 8×8; the inner side walls sit at
// the correct inset planes (±4) but keep full extent (z spanning [-5,5]) and
// carry the SAME normals as the outer walls instead of facing the cavity. So
// `create_interior_offset_faces` offsets each face along its normal but never
// trims the offset faces to their mutual intersections (the inset cavity
// footprint). The four top-rim faces are correct. The existing shell tests
// (offset.rs) missed this because they only check wall planes/counts with
// `validate_result:false`, never the enclosed volume. Tracked for a focused
// shell-trim fix; these tests pin the exact failing numbers.
#[test]
fn shell_material_less_than_solid_box() {
    let (tess_vol, _) = shelled_open_box(10.0, 10.0, 10.0, 1.0);
    assert!(
        tess_vol < 1000.0,
        "a hollow box must hold less material than the solid box: {tess_vol} !< 1000"
    );
    assert!(tess_vol > 0.0, "shell volume must be positive: {tess_vol}");
}
