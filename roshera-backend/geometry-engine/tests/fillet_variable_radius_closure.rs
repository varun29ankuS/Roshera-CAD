// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! CLOSURE coverage for varying-radius fillet schedules (Task #37).
//!
//! `fillet_variable_radius_spine.rs` asserts on the spine solver's
//! per-station rolling-ball CONTACTS. Every one of those assertions
//! passed while no varying schedule could produce a closed solid at
//! all: `fillet_edges` refused `VariableStations`,
//! `PerEdgeProfile(Variable)` and `Function` alike with `InvalidBRep`
//! "geometrically OPEN" on a plain 10 mm cube edge. The spine was
//! right; the shell was open somewhere the spine file never looked.
//!
//! This file looks at the whole solid instead. It asserts what the
//! spine file cannot:
//!
//! * `fillet_edges` RETURNS (the D-1 closure post-flight in
//!   `validate_filleted_solid` does not roll the operation back),
//! * the tessellated shell is CLOSED, manifold and consistently
//!   oriented with mesh Euler characteristic 2 at the coarse chord
//!   `certify_solid` and the closure gate both use,
//! * `certify_solid` reports the solid SOUND, and
//! * the volume matches the closed form for a cube with one edge
//!   rounded by a linearly-varying radius, and lies strictly between
//!   the two constant-radius bounds -- so a silent fall back to
//!   either endpoint radius fails the test rather than passing it.
//!
//! ## The closed form
//!
//! Rounding one edge of an `L`-cube at constant radius `r` removes the
//! prism between the quarter-cylinder and the square corner it sits
//! in: cross-section `r^2 - pi r^2 / 4`, so `V = L^3 - (1 - pi/4) L r^2`.
//! For a radius that ramps linearly `r0 -> r1` along the edge the same
//! cross-section integrates as the mean of `r(t)^2`:
//!
//! ```text
//! (1/L) INTEGRAL r(t)^2 dt = (r0^2 + r0 r1 + r1^2) / 3
//! V = L^3 - (1 - pi/4) * L * (r0^2 + r0 r1 + r1^2) / 3
//! ```
//!
//! At `L = 10`, `r0 = 1`, `r1 = 2` that is `1000 - 5.00738 = 994.99262`,
//! against `997.85398` for a constant 1 mm fillet and `991.41593` for a
//! constant 2 mm one. The 0.1 % band around the varying value excludes
//! both by roughly a factor of three.
//!
//! **The formula is an IDEALISATION, not the exact envelope.** It
//! integrates the constant-radius cross-section along the edge, which
//! assumes the cross-section stays perpendicular to the spine. For a
//! varying radius it does not: the rolling ball's contact circle tilts
//! in proportion to `dr/du` (the same tilt
//! `fillet_variable_cap_arc_validation.rs` exists to pin at the caps).
//! The discrepancy is `O((dr/L)^2)` of the removed volume -- for the
//! 1 -> 2 mm ramp over 10 mm that is a few parts in ten thousand of
//! ~5 mm^3, i.e. microns^3 against a 995 mm^3 solid. It is far below
//! the 0.1 % budget, which is itself dominated by mesh faceting (the
//! measured residual is ~0.07 %, one-sided, and shrinks with chord).
//! The budget absorbs both; neither is a claim of exactness.

use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::operations::blend_graph::{BlendRadius, EdgeFilletProfile};
use geometry_engine::operations::fillet::{FilletType, PropagationMode};
use geometry_engine::operations::{fillet_edges, FilletOptions};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
use std::collections::HashMap;

const SIZE: f64 = 10.0;
/// The chord `validate_blend_geometric_closure` and `certify_solid`'s
/// watertight dimension both tessellate at.
const GATE_CHORD: f64 = 0.1;
/// The DISPLAY chord (`TessellationParams::default`), which
/// `tessellate_solid` floors at `5e-4 * diagonal` -- about 0.0087 on a
/// 10 mm cube. Far finer than the gate's, and the chord the original
/// defect stayed visible at after the gate went quiet: closure at
/// `GATE_CHORD` alone was measured passing while the display mesh still
/// carried 14 boundary edges and lost two whole planar faces. Both
/// chords are asserted; only asserting the gate's would let exactly
/// that state through again.
const DISPLAY_CHORD: f64 = 0.001;
const WELD_EPS: f64 = 1e-6;

/// Triangles the tessellator emits for `face` at `chord`. Pins the
/// SHAPE of the conforming stitch's output, not just its closure.
fn face_triangles(model: &BRepModel, solid: SolidId, face: u32, chord: f64) -> usize {
    let params = TessellationParams {
        chord_tolerance: chord,
        ..TessellationParams::default()
    };
    let solid_ref = model.solids.get(solid).expect("solid exists");
    let mesh = tessellate_solid(solid_ref, model, &params);
    mesh.face_map.iter().filter(|&&f| f == face).count()
}

fn make_box(model: &mut BRepModel) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    match builder
        .create_box_3d(SIZE, SIZE, SIZE)
        .expect("box creation succeeds")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {:?}", other),
    }
}

fn first_open_edge(model: &BRepModel) -> EdgeId {
    model
        .edges
        .iter()
        .filter_map(|(id, edge)| if !edge.is_loop() { Some(id) } else { None })
        .next()
        .expect("box must have at least one open edge")
}

fn options(fillet_type: FilletType) -> FilletOptions {
    FilletOptions {
        fillet_type,
        radius: 1.0,
        propagation: PropagationMode::None,
        ..Default::default()
    }
}

/// `V = L^3 - (1 - pi/4) * L * (r0^2 + r0 r1 + r1^2) / 3` -- see the
/// module header. `r0 == r1` degenerates to the constant-radius form.
fn analytic_volume(r0: f64, r1: f64) -> f64 {
    SIZE.powi(3) - (1.0 - std::f64::consts::PI / 4.0) * SIZE * (r0 * r0 + r0 * r1 + r1 * r1) / 3.0
}

/// Apply `fillet_type` to one cube edge and assert the result is a
/// closed, sound solid whose volume is the varying-radius closed form.
/// Returns the volume so callers can compare two spellings of the same
/// schedule against each other.
fn assert_closed_sound_and_correct(fillet_type: FilletType, r0: f64, r1: f64, label: &str) -> f64 {
    let mut model = BRepModel::new();
    let solid = make_box(&mut model);
    let edge = first_open_edge(&model);

    let faces = match fillet_edges(&mut model, solid, vec![edge], options(fillet_type)) {
        Ok(faces) => faces,
        Err(e) => panic!(
            concat!(
                "{}: a varying radius schedule on a plain cube edge must produce a ",
                "solid; fillet_edges refused with {:?}"
            ),
            label, e
        ),
    };
    assert_eq!(
        faces.len(),
        1,
        "{label}: one filleted edge yields one blend face, got {}",
        faces.len()
    );

    // Closure at BOTH chords. The gate's coarse chord is what
    // `validate_blend_geometric_closure` and `certify_solid` measure;
    // the display chord is finer, is what a render and every
    // mass-property query actually use, and is where a dropped boundary
    // chord shows up after the gate has gone quiet.
    for (chord_name, chord) in [("gate/coarse", GATE_CHORD), ("display", DISPLAY_CHORD)] {
        let report = manifold_report(&model, solid, chord, WELD_EPS).unwrap_or_else(|| {
            panic!("{label} at the {chord_name} chord: filleted solid tessellates to an empty mesh")
        });
        assert_eq!(
            report.boundary_edges, 0,
            concat!(
                "{} at the {} chord: the shell is geometrically OPEN -- {} boundary mesh ",
                "edge(s) (mesh chi = {}). Either the blend face and its retrimmed ",
                "neighbours disagree on the shared cap/rail samples, or a neighbour ",
                "dropped a boundary chord / emitted no triangles at all"
            ),
            label, chord_name, report.boundary_edges, report.euler_characteristic
        );
        assert_eq!(
            report.nonmanifold_edges, 0,
            "{label} at the {chord_name} chord: {} non-manifold mesh edge(s)",
            report.nonmanifold_edges
        );
        assert_eq!(
            report.inconsistent_directed_edges, 0,
            concat!(
                "{} at the {} chord: {} inconsistently-wound mesh edge(s) -- two triangles ",
                "wind the same way across a shared edge, which is what an OVERLAPPING fan ",
                "over a self-intersecting contour produces"
            ),
            label, chord_name, report.inconsistent_directed_edges
        );
        assert!(
            report.closed && report.manifold && report.oriented,
            "{label} at the {chord_name} chord: closed={} manifold={} oriented={}",
            report.closed,
            report.manifold,
            report.oriented
        );
        assert_eq!(
            report.euler_characteristic, 2,
            concat!(
                "{} at the {} chord: a solid bounded by one sphere-topology shell meshes ",
                "to chi = 2, got {}"
            ),
            label, chord_name, report.euler_characteristic
        );
    }

    // `is_sound()` ANDs every dimension, mesh QUALITY (aspect ratio,
    // min angle, boundary conformance) included -- not just closure.
    // The breakdown is spelled out so a failure names the dimension
    // rather than sending the reader back to the certificate struct.
    let cert = model.certify_solid(solid);
    assert!(
        cert.is_sound(),
        concat!(
            "{}: certificate is UNSOUND -- brep_valid={} watertight={} manifold={} ",
            "oriented={} self_intersection_free={} tessellation.clean={} ",
            "mesh_quality.clean={}; errors {:?}"
        ),
        label,
        cert.brep_valid,
        cert.watertight,
        cert.manifold,
        cert.oriented,
        cert.self_intersection_free,
        cert.tessellation.clean,
        cert.mesh_quality.clean,
        cert.errors
    );

    let volume = model
        .calculate_solid_volume(solid)
        .unwrap_or_else(|| panic!("{label}: no volume for the filleted solid"));

    // Strictly between the two constant-radius bounds. This is the
    // assertion that a silent fall back to either endpoint radius --
    // the shape the surgery would take if a varying schedule were
    // quietly collapsed to a constant -- cannot satisfy.
    // The larger radius removes more material, so it is the LOWER volume
    // bound whichever way the ramp runs.
    let (small, large) = (r0.min(r1), r0.max(r1));
    let (lo, hi) = (analytic_volume(large, large), analytic_volume(small, small));
    assert!(
        volume > lo && volume < hi,
        concat!(
            "{}: a {} mm -> {} mm blend removes more than the {} mm constant and less ",
            "than the {} mm constant; volume {:.5} is not strictly inside ({:.5}, {:.5})"
        ),
        label,
        r0,
        r1,
        large,
        small,
        volume,
        lo,
        hi
    );

    let expected = analytic_volume(r0, r1);
    let rel = (volume - expected).abs() / expected;
    assert!(
        rel <= 1.0e-3,
        concat!(
            "{}: volume {:.5} differs from the closed form {:.5} by {:.4} % ",
            "(budget 0.1 %)"
        ),
        label,
        volume,
        expected,
        rel * 100.0
    );
    volume
}

/// The reproduction from the Task 14 report, as an assertion.
/// `VariableStations` has been reachable since the variant existed and
/// refused with `InvalidBRep` on this exact fixture.
#[test]
fn variable_stations_fillet_on_a_cube_edge_closes() {
    assert_closed_sound_and_correct(
        FilletType::VariableStations(vec![(0.0, 1.0), (1.0, 2.0)]),
        1.0,
        2.0,
        "VariableStations([(0,1),(1,2)])",
    );
}

/// The SHAPE of the conforming stitch's output, pinned at both chords.
///
/// Closure alone does not say the mesh is the one intended: the old
/// rectangular grid emitted 124 triangles for this blend face and they
/// were open, and a degenerate-triangle drop would still close while
/// silently losing a chord. Pinning the counts makes any change to the
/// stitch's density or to the apex rule visible in the diff rather than
/// only in a downstream volume drift.
#[test]
fn the_conforming_stitch_emits_the_pinned_mesh() {
    let mut model = BRepModel::new();
    let solid = make_box(&mut model);
    let edge = first_open_edge(&model);
    let faces = fillet_edges(
        &mut model,
        solid,
        vec![edge],
        options(FilletType::VariableStations(vec![(0.0, 1.0), (1.0, 2.0)])),
    )
    .expect("a 1 mm -> 2 mm blend on a cube edge applies");
    let blend = faces[0];

    assert_eq!(
        face_triangles(&model, solid, blend, GATE_CHORD),
        169,
        "blend face triangle count at the gate chord changed"
    );
    assert_eq!(
        face_triangles(&model, solid, blend, DISPLAY_CHORD),
        233,
        "blend face triangle count at the display chord changed"
    );

    // Every face of the shell must contribute triangles at BOTH chords.
    // A face that emits ZERO is the failure mode behind the display
    // chord's old 14 boundary edges -- the trimmed planar neighbours of
    // a varying fillet vanished from the mesh entirely.
    let shell = model
        .shells
        .get(model.solids.get(solid).expect("solid").outer_shell)
        .expect("outer shell");
    for chord in [GATE_CHORD, DISPLAY_CHORD] {
        for &fid in &shell.faces {
            assert!(
                face_triangles(&model, solid, fid, chord) > 0,
                "face {fid} emitted no triangles at chord {chord} -- a hole the size of a \
                 whole face, which reads as boundary edges and a wrong volume"
            );
        }
    }
}

/// The per-edge spelling of the same schedule. It reaches the same
/// surgery through a different dispatch arm, so it needs its own pin --
/// a fix that only reached the top-level arm would leave this red.
#[test]
fn per_edge_profile_variable_fillet_on_a_cube_edge_closes() {
    let mut probe = BRepModel::new();
    let _ = make_box(&mut probe);
    let edge = first_open_edge(&probe);

    let mut map = HashMap::new();
    map.insert(
        edge,
        EdgeFilletProfile::Radius(BlendRadius::Variable(vec![(0.0, 1.0), (1.0, 2.0)])),
    );
    assert_closed_sound_and_correct(
        FilletType::PerEdgeProfile(map),
        1.0,
        2.0,
        "PerEdgeProfile(Variable([(0,1),(1,2)]))",
    );
}

/// `Variable(r0, r1)` -- the two-endpoint spelling -- is the variant
/// `fillet_variable_cap_arc_validation.rs` exercises with
/// `validate_result: false` because "the unequal-end variable band does
/// not yet weld watertight against its neighbours". It does now, and
/// this asserts it on the default path with the post-flight ON.
#[test]
fn two_endpoint_variable_fillet_on_a_cube_edge_closes() {
    assert_closed_sound_and_correct(
        FilletType::Variable(0.4, 0.8),
        0.4,
        0.8,
        "Variable(0.4, 0.8)",
    );
}

/// A schedule whose radius ramps DOWN. The blend's short cap is then at
/// the far end of the edge rather than the near one, which exercises
/// the mirror of the sample-count mismatch that broke the ramp-up case.
#[test]
fn descending_variable_stations_fillet_on_a_cube_edge_closes() {
    assert_closed_sound_and_correct(
        FilletType::VariableStations(vec![(0.0, 2.0), (1.0, 1.0)]),
        2.0,
        1.0,
        "VariableStations([(0,2),(1,1)])",
    );
}

/// The two spellings of one schedule must produce the same solid, not
/// merely two solids that each close.
#[test]
fn the_two_variable_spellings_agree_on_volume() {
    let stations = assert_closed_sound_and_correct(
        FilletType::VariableStations(vec![(0.0, 1.0), (1.0, 2.0)]),
        1.0,
        2.0,
        "VariableStations",
    );
    let endpoints =
        assert_closed_sound_and_correct(FilletType::Variable(1.0, 2.0), 1.0, 2.0, "Variable(1, 2)");
    assert!(
        (stations - endpoints).abs() <= 1.0e-9 * endpoints.abs().max(1.0),
        concat!(
            "VariableStations([(0,1),(1,2)]) and Variable(1, 2) are one schedule written ",
            "two ways; volumes {} vs {}"
        ),
        stations,
        endpoints
    );
}

/// The constant-radius path must be untouched by the varying-radius
/// fix. Its two caps are congruent and its two rails are the same
/// length, so both boundary caches agree in count and the conforming
/// stitch must never fire -- the historical grid still meshes it.
#[test]
fn the_constant_radius_path_is_unchanged() {
    let mut model = BRepModel::new();
    let solid = make_box(&mut model);
    let edge = first_open_edge(&model);
    let faces = fillet_edges(
        &mut model,
        solid,
        vec![edge],
        options(FilletType::Constant(1.0)),
    )
    .expect("a 1 mm constant fillet on a cube edge succeeds");
    assert_eq!(faces.len(), 1);

    let report = manifold_report(&model, solid, GATE_CHORD, WELD_EPS)
        .expect("constant fillet tessellates to a mesh");
    // Pinned counts, measured on the pre-fix tree. The conforming
    // stitch emits a different (also-closed) triangulation, so any
    // number here other than 76 means the mismatch gate started
    // claiming a constant-radius blend.
    assert_eq!(
        report.triangles, 76,
        "the constant-radius blend's mesh changed: {} triangles, expected the historical 76",
        report.triangles
    );
    assert_eq!(report.welded_vertices, 40);
    assert_eq!(report.boundary_edges, 0);
    assert_eq!(report.euler_characteristic, 2);

    let volume = model
        .calculate_solid_volume(solid)
        .expect("constant fillet volume");
    let expected = analytic_volume(1.0, 1.0);
    assert!(
        (volume - expected).abs() / expected <= 1.0e-3,
        "constant 1 mm fillet volume {volume:.5} vs closed form {expected:.5}"
    );
}
