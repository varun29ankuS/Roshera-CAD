// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Soundness gate for the shell operation (`offset_solid`) as a MULTI-SHELL
//! B-Rep producer.
//!
//! A shelled box has two boundary surfaces: the original exterior and the new
//! inner cavity. The generalized Euler–Poincaré characteristic
//!
//!     V − E + F − R = 2 (S − G)
//!
//! (Mäntylä 1988; ISO 10303-42) counts S = number of boundary shells (1
//! peripheral + one per void). A fully-enclosed hollow box is therefore a
//! TWO-shell solid (S = 2), genus 0: V−E+F−R = 4. Filing the cavity faces into
//! the single outer shell made the count read S = 1, forcing genus = 1 − 4/2 =
//! −1 and a (correct) validator rejection — the defect these tests pin.
//!
//! `offset_solid` now emits the cavity of a fully-closed hollow as its own
//! INNER (void) shell (`Solid::inner_shells`), so the count reads S = 2, genus
//! 0, and the body validates. An OPEN shell (a face removed) is instead one
//! connected 2-manifold (a tray: S = 1, χ = 2, genus 0) and stays a single
//! shell. Both cases are gated here for validity, correct enclosed volume, and
//! self-consistency (watertightness witness), plus a typed refusal for a shell
//! too thick to admit a non-self-intersecting cavity, and determinism.

use geometry_engine::math::Tolerance;
use geometry_engine::operations::offset::OffsetType;
use geometry_engine::operations::{offset_solid, CommonOptions, OffsetOptions};
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};

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
    let shell = model.shells.get(solid.outer_shell).expect("shell").clone();
    let mut top = None;
    for &face_id in &shell.faces {
        let face = model.faces.get(face_id).expect("face");
        let surface = model.surfaces.get(face.surface_id).expect("surface");
        let n = surface.normal_at(0.5, 0.5).expect("normal");
        if (n.z - 1.0).abs() < 1e-9 && n.x.abs() < 1e-9 && n.y.abs() < 1e-9 {
            top = Some(face_id);
            break;
        }
    }
    (model, solid_id, top.expect("+Z face"))
}

fn opts(thickness: f64) -> OffsetOptions {
    OffsetOptions {
        // Post-op validation stays ON here — this gate asserts the op emits a
        // solid the B-Rep validator accepts.
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        offset_type: OffsetType::Distance(thickness),
        ..Default::default()
    }
}

fn is_sound(model: &BRepModel, sid: SolidId) -> bool {
    validate_solid_scoped(model, sid, Tolerance::default(), ValidationLevel::Standard).is_valid
}

fn rel_close(a: f64, b: f64, tol: f64) -> bool {
    if b.abs() < 1e-9 {
        a.abs() <= tol
    } else {
        ((a - b) / b).abs() <= tol
    }
}

// ---------------------------------------------------------------------------
// Fully-closed hollow: the two-shell (void) case — the reproduced defect.
// ---------------------------------------------------------------------------

#[test]
fn fully_hollow_box_is_sound_two_shell_solid() {
    let (mut model, solid_id, _top) = box_with_top_face(10.0, 10.0, 10.0);
    let hollow = offset_solid(&mut model, solid_id, 1.0, vec![], opts(1.0))
        .expect("fully-hollow shell of a plain box must succeed and validate");

    // Structure: one peripheral shell (6 exterior faces) + one void shell.
    let solid = model.solids.get(hollow).expect("hollow solid");
    assert_eq!(
        solid.inner_shells.len(),
        1,
        "a box with one cavity must have exactly one inner (void) shell, got {:?}",
        solid.inner_shells
    );
    let outer_faces = model
        .shells
        .get(solid.outer_shell)
        .expect("outer shell")
        .faces
        .len();
    assert_eq!(
        outer_faces, 6,
        "peripheral shell must carry the 6 box faces"
    );
    let void_faces = model
        .shells
        .get(solid.inner_shells[0])
        .expect("void shell")
        .faces
        .len();
    assert_eq!(void_faces, 6, "the cavity is a 6-faced inner box");

    // Validity: S=2, genus 0 — the Euler–Poincaré check must pass.
    assert!(
        is_sound(&model, hollow),
        "fully-hollow box must validate as a sound two-shell solid"
    );

    // Volume: outer 1000 − inner cavity (8×8×8 = 512) = 488 of wall material.
    let mp = model
        .mass_properties_for(hollow)
        .expect("hollow mass properties");
    assert!(
        rel_close(mp.volume, 488.0, 1e-3),
        "fully-hollow 10³ t=1 wall volume must be 488 (1000−512), got {}",
        mp.volume
    );
}

// ---------------------------------------------------------------------------
// Open-top shell: the single connected-manifold (tray) case.
// ---------------------------------------------------------------------------

#[test]
fn open_top_box_is_sound_single_shell_solid() {
    let (mut model, solid_id, top) = box_with_top_face(10.0, 10.0, 10.0);
    let hollow = offset_solid(&mut model, solid_id, 1.0, vec![top], opts(1.0))
        .expect("open-top shell of a plain box must succeed and validate");

    // Structure: one connected shell, no void.
    let solid = model.solids.get(hollow).expect("hollow solid");
    assert!(
        solid.inner_shells.is_empty(),
        "an open (tray) shell is one connected manifold — no void shell, got {:?}",
        solid.inner_shells
    );

    assert!(
        is_sound(&model, hollow),
        "open-top box shell must validate (S=1, χ=2, genus 0)"
    );

    // Volume: 1000 − (8×8×9) open cavity = 1000 − 576 = 424 wall material.
    let mp = model
        .mass_properties_for(hollow)
        .expect("hollow mass properties");
    assert!(
        rel_close(mp.volume, 424.0, 3e-2),
        "open-top 10³ t=1 wall volume must be ~424 (1000−576), got {}",
        mp.volume
    );
}

// ---------------------------------------------------------------------------
// Honest refusal: a wall thicker than half the smallest extent is impossible.
// ---------------------------------------------------------------------------

#[test]
fn too_thick_shell_refuses_typed_fully_hollow() {
    // 10³ box, t = 5: 2·t = 10 ≥ 10 → the inner cavity would collapse to zero
    // extent (or invert). The op must refuse with a typed error, not emit torn
    // geometry.
    let (mut model, solid_id, _top) = box_with_top_face(10.0, 10.0, 10.0);
    let err = offset_solid(&mut model, solid_id, 5.0, vec![], opts(5.0))
        .expect_err("t ≥ half the smallest extent must refuse");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("too large") && msg.contains("self-intersect"),
        "refusal must name the too-thick / self-intersection cause, got: {msg}"
    );
}

#[test]
fn too_thick_shell_refuses_typed_open_top() {
    // A wall of t = 6 on a 10³ box laterals to 10 − 12 < 0 even with the top
    // open — impossible. Refused up-front by the smallest-extent guard.
    let (mut model, solid_id, top) = box_with_top_face(10.0, 10.0, 10.0);
    let err = offset_solid(&mut model, solid_id, 6.0, vec![top], opts(6.0))
        .expect_err("t larger than half the lateral extent must refuse");
    assert!(
        format!("{err:?}").contains("too large"),
        "open-top too-thick shell must refuse typed"
    );
}

#[test]
fn admissible_thickness_below_half_extent_succeeds() {
    // t = 4 on a 10³ box: 2·t = 8 < 10 → inner cavity 2×2×2, valid.
    let (mut model, solid_id, _top) = box_with_top_face(10.0, 10.0, 10.0);
    let hollow = offset_solid(&mut model, solid_id, 4.0, vec![], opts(4.0))
        .expect("t just below half the extent must succeed");
    assert!(is_sound(&model, hollow), "thin-cavity hollow must validate");
    let mp = model.mass_properties_for(hollow).expect("mass properties");
    // 1000 − 2³ = 992.
    assert!(
        rel_close(mp.volume, 992.0, 1e-3),
        "10³ t=4 wall volume must be 992 (1000−8), got {}",
        mp.volume
    );
}

// ---------------------------------------------------------------------------
// Determinism: identical inputs produce identical shell structure + volume.
// ---------------------------------------------------------------------------

#[test]
fn fully_hollow_shell_is_deterministic() {
    let run = || -> (usize, usize, f64) {
        let (mut model, solid_id, _top) = box_with_top_face(10.0, 10.0, 10.0);
        let hollow =
            offset_solid(&mut model, solid_id, 1.0, vec![], opts(1.0)).expect("shell must succeed");
        let solid = model.solids.get(hollow).expect("solid");
        let inner = solid.inner_shells.len();
        let outer = model
            .shells
            .get(solid.outer_shell)
            .expect("outer")
            .faces
            .len();
        let vol = model
            .mass_properties_for(hollow)
            .expect("mass props")
            .volume;
        (outer, inner, vol)
    };
    let a = run();
    let b = run();
    assert_eq!(a.0, b.0, "outer face count must be deterministic");
    assert_eq!(a.1, b.1, "inner shell count must be deterministic");
    assert!(
        (a.2 - b.2).abs() < 1e-9,
        "volume must be bit-stable across runs: {} vs {}",
        a.2,
        b.2
    );
}
