// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! **A cross-bore through a hollow skirt meshes with facets folded 90 degrees
//! off the surface.**
//!
//! Surfaced 2026-08-22 from the live piston Varun kept calling "the bad
//! piston". The kernel did NOT lie about it — `verify_part` returned
//! `sound: false` and named the defect. What it named is not the topology
//! failure already pinned in `cross_bore_manifold.rs`:
//!
//! ```text
//!   piston solid_11   manifold=TRUE  boundary_edges=0  nonmanifold_edges=0
//!                     worst_aspect_ratio=456.2  min_angle=0.09deg
//!                     max_normal_deviation=90.1deg  boundary_crossing=6
//! ```
//!
//! Topology is clean and the boundary closes; the failure is entirely in the
//! mesh. On screen it is the long slivers radiating out of the pin bore across
//! the skirt.
//!
//! ## What the reduction showed, against what was predicted
//!
//! The prediction was the periodic-seam wrap that `boundary_crossing_facets`
//! gates — a facet bridging the bore the long way around the lateral. The
//! first measurement appeared to refute that: the gate reported ZERO bridging
//! facets. It was the GATE that was wrong, not the prediction.
//!
//! `is_bridging_facet` used to test `coverage > p * 0.5`, and a bore drilled
//! on the axis breaks out at two exactly antipodal `u`, so its bridging facet
//! covers precisely `p/2` — which a strict `>` cannot catch. Once the gate was
//! corrected (see `tests/bridging_facet_gate.rs`) the same unchanged mesh
//! reported four of them. The lesson is recorded here rather than quietly
//! edited away: **a zero from an instrument is not a fact about the world**,
//! and this file asserted one as a premise until the instrument was fixed.
//!
//! ```text
//!   AXIAL bore r12 (control)  aspect= 30.3  min_angle=1.890deg  dev= 1.0deg  wings=0  clean=TRUE
//!   CROSS bore r12 (red)      aspect=456.2  min_angle=0.068deg  dev=90.0deg  wings=4  clean=false
//! ```
//!
//! The defect is a facet standing perpendicular to the surface it approximates
//! — one straight 86mm chord from `y=+43` to `y=-43` straight through the
//! solid's interior, on a surface of radius 43, with a 456:1 aspect ratio and
//! a 0.068 degree minimum angle. The reduced aspect ratio reproduces the live
//! piston's 456.2485 to four figures, which is what ties this file to the part
//! on screen.
//!
//! That the reduced skirt is manifold while the solid-blank cross bore of
//! `cross_bore_manifold.rs` is not says these are two different defects that
//! share a geometry family. This file is about the mesh one, and it
//! deliberately reproduces the piston's SHAPE — a hollow skirt, so the bore
//! crosses two thin walls — rather than the solid blank.
//!
//! [`axial_bore_through_a_hollow_skirt_meshes_cleanly`] is the control: the
//! same helper, the same skirt, the same tool radius, only the bore axis
//! changes. It must stay green, or the RED below stops being a statement about
//! the cross bore.

use geometry_engine::harness::watertight::{manifold_report, mesh_quality};
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// The live piston's own numbers: OD 86, 6mm wall, 70 tall, pin bore d24.
const SKIRT_OUTER_R: f64 = 43.0;
const SKIRT_INNER_R: f64 = 37.0;
const SKIRT_H: f64 = 70.0;
const PIN_BORE_R: f64 = 12.0;
const PIN_BORE_Z: f64 = 30.0;

const CHORD_TOL: f64 = 0.02;
const WELD_TOL: f64 = 1e-6;

fn cylinder(model: &mut BRepModel, base: Point3, axis: Vector3, r: f64, h: f64) -> SolidId {
    match TopologyBuilder::new(model).create_cylinder_3d(base, axis, r, h) {
        Ok(GeometryId::Solid(id)) => id,
        other => panic!("expected a solid; got {other:?}"),
    }
}

/// What a bored skirt's mesh looks like. Kept as one struct so both arms
/// report the identical set of numbers.
struct WingReport {
    manifold_edges: usize,
    boundary_edges: usize,
    worst_aspect_ratio: f64,
    min_angle_deg: f64,
    max_normal_deviation_deg: f64,
    boundary_crossing_facets: usize,
    clean: bool,
}

/// Build the hollow skirt, cut one through-bore along `axis`, and measure the
/// mesh. Both arms go through this, so neither can pass or fail for a reason
/// the other does not share.
fn bore_skirt_and_measure(axis: Vector3, tool_base: Point3, label: &str) -> WingReport {
    let mut model = BRepModel::new();

    let outer = cylinder(
        &mut model,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
        SKIRT_OUTER_R,
        SKIRT_H,
    );
    // Overlong bore so the inner cut is a clean through-cut, not a blind pocket.
    let core = cylinder(
        &mut model,
        Point3::new(0.0, 0.0, -10.0),
        Vector3::new(0.0, 0.0, 1.0),
        SKIRT_INNER_R,
        SKIRT_H + 20.0,
    );
    let skirt = boolean_operation(
        &mut model,
        outer,
        core,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("hollowing the skirt must not error");

    // The skirt itself must be clean, or nothing below is about the pin bore.
    let before =
        manifold_report(&model, skirt, CHORD_TOL, WELD_TOL).expect("the hollow skirt must mesh");
    assert_eq!(
        before.nonmanifold_edges, 0,
        "{label}: the SKIRT must be manifold before the pin bore"
    );
    let before_q = mesh_quality(&model, skirt).expect("the hollow skirt must measure");
    assert_eq!(
        before_q.boundary_crossing_facets, 0,
        "{label}: the SKIRT must have no wings before the pin bore, else this \
         test proves nothing about the bore"
    );

    let tool = cylinder(
        &mut model,
        tool_base,
        axis,
        PIN_BORE_R,
        SKIRT_OUTER_R * 2.0 + 20.0,
    );
    let bored = boolean_operation(
        &mut model,
        skirt,
        tool,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("the pin bore difference must not error");

    let after =
        manifold_report(&model, bored, CHORD_TOL, WELD_TOL).expect("the bored skirt must mesh");
    let q = mesh_quality(&model, bored).expect("the bored skirt must measure");

    eprintln!(
        "{label}: tris={} nonmanifold={} boundary={} aspect={:.1} min_angle={:.3}deg \
         normal_dev={:.1}deg wings={} clean={}",
        q.triangles,
        after.nonmanifold_edges,
        after.boundary_edges,
        q.worst_aspect_ratio,
        q.min_angle_deg,
        q.max_normal_deviation_deg,
        q.boundary_crossing_facets,
        q.clean,
    );
    if let Some(w) = &q.worst_face {
        eprintln!(
            "{label}: worst face {} -> aspect={:.1} min_angle={:.3}deg dev={:.1}deg wings={}",
            w.face_id,
            w.worst_aspect_ratio,
            w.min_angle_deg,
            w.max_normal_deviation_deg,
            w.boundary_crossing_facets,
        );
    }

    WingReport {
        manifold_edges: after.nonmanifold_edges,
        boundary_edges: after.boundary_edges,
        worst_aspect_ratio: q.worst_aspect_ratio,
        min_angle_deg: q.min_angle_deg,
        max_normal_deviation_deg: q.max_normal_deviation_deg,
        boundary_crossing_facets: q.boundary_crossing_facets,
        clean: q.clean,
    }
}

/// CONTROL, and it must pass: a second bore along the skirt's OWN axis meshes
/// cleanly. Same helper, same skirt, same tool radius — only the axis differs.
#[test]
fn axial_bore_through_a_hollow_skirt_meshes_cleanly() {
    let r = bore_skirt_and_measure(
        Vector3::new(0.0, 0.0, 1.0),
        Point3::new(0.0, 0.0, -10.0),
        "axial-r12",
    );
    assert_eq!(
        r.boundary_edges, 0,
        "an axial bore must leave a closed solid"
    );
    assert_eq!(
        r.boundary_crossing_facets, 0,
        "an axial bore must not produce facets bridging the surface"
    );
    assert!(
        r.max_normal_deviation_deg <= 30.0,
        "an axial bore must not fold facets off the surface; got {:.1}deg",
        r.max_normal_deviation_deg
    );
    assert!(
        r.clean,
        "an axial bore must mesh clean (aspect {:.1}, min angle {:.3}deg, dev {:.1}deg)",
        r.worst_aspect_ratio, r.min_angle_deg, r.max_normal_deviation_deg
    );
}

/// RED, pinned. The pin bore across the skirt — the live piston's own geometry.
///
/// `#[ignore]` because the fix is a tessellation change that has not been
/// attempted, not because the assertion is soft. Run it with
/// `cargo test -p geometry-engine --test cross_bore_mesh_wings -- --ignored`.
///
/// The assertions are ordered by what actually discriminates. `max_normal_
/// deviation_deg` is the headline: it is 1.0 degrees on the control and 90.0
/// here, and `MeshQuality` gates it precisely because a facet tens of degrees
/// off-surface is a bridging facet however the rest of the mesh scores.
#[test]
#[ignore = "pinned RED: cross-bore breakout meshes with 90-degree off-surface slivers"]
fn cross_bore_through_a_hollow_skirt_meshes_cleanly() {
    let r = bore_skirt_and_measure(
        Vector3::new(0.0, 1.0, 0.0),
        Point3::new(0.0, -(SKIRT_OUTER_R + 10.0), PIN_BORE_Z),
        "cross-r12",
    );
    // Guard: if this moves, the defect under test changed and every number in
    // this file's header is describing something else. Topology stays clean —
    // that is what separates this from the RED in cross_bore_manifold.rs.
    assert_eq!(
        r.manifold_edges, 0,
        "the reduced cross bore is manifold; a non-zero count here means this \
         test has drifted onto the topology defect pinned in cross_bore_manifold.rs"
    );
    assert_eq!(
        r.boundary_crossing_facets, 0,
        "facets bridge straight across the bore — measured 4, each an 86mm chord \
         through the interior of a part whose radius is 43mm"
    );
    assert!(
        r.max_normal_deviation_deg <= 30.0,
        "facets at the bore breakout stand off the surface they approximate; \
         got {:.1}deg against 1.0deg on the axial control",
        r.max_normal_deviation_deg,
    );
    assert!(
        r.clean,
        "mesh_quality must be clean (aspect {:.1}, min angle {:.3}deg)",
        r.worst_aspect_ratio, r.min_angle_deg
    );
}
