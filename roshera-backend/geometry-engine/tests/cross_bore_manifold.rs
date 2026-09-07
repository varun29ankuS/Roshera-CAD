// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! **A cross-drilled hole through a cylinder is non-manifold.**
//!
//! Surfaced 2026-08-21 by an agent driving the live MCP surface: it built a
//! piston, certified the revolved body SOUND, then cut a pin bore across it
//! and the result came back `manifold: false`. The agent's own reading was
//! that its circlip groove had made a near-tangent lens (ceiling #86). That
//! was wrong, and reducing it says so: the plain bore alone already fails,
//! with no groove and nothing tangent anywhere near it.
//!
//! What actually varies is the ANGLE of the bore relative to the blank's own
//! axis. Measured against the live kernel, same blank and same tool, only the
//! axis changed:
//!
//! ```text
//!   AXIAL  bore r12 along +Z   sound=true   manifold=true
//!   CROSS  bore r5  along +Y   sound=false  manifold=FALSE
//!   CROSS  bore r12 along +Y   sound=false  manifold=FALSE
//!   CROSS  bore r25 along +Y   sound=false  manifold=FALSE
//! ```
//!
//! Every cross case is `brep_valid` and `watertight` and NOT manifold, so the
//! boundary closes but the topology at the saddle — where the bore breaks out
//! through the curved lateral face — is wrong. An axial bore meets the blank
//! on planar caps and a coaxial wall and is clean; the cross bore is a curved
//! cylinder-cylinder intersection, the same family as the `#[ignore]`d
//! PIPE-TEE case in `agent_build_eval.rs` (which panics rather than returning
//! bad topology) and the quadric-SSI rung of the OCCT-parity ladder.
//!
//! ## The mesh measure, and the lead it carries
//!
//! ```text
//!   cross r12   tris=5840  boundary_edges=0  nonmanifold_edges=110  directed_dup=220
//!   cross r5    tris=5788  boundary_edges=0  nonmanifold_edges= 66  directed_dup=132
//!   axial r12                     (control)  nonmanifold_edges=  0  directed_dup=  0
//! ```
//!
//! `boundary_edges` is ZERO — nothing leaks, which is why the certificate can
//! still call it watertight. And `inconsistent_directed_edges` is EXACTLY
//! twice `nonmanifold_edges` in both cases. A directed edge traversed twice in
//! the same direction is a duplicated facet, so the breakout curve where the
//! bore meets the lateral face appears to be emitted TWICE and stitched into
//! both. That ratio holding across two radii is the most specific lead here,
//! and it points at the intersection-curve emission rather than at meshing.
//!
//! **THE KERNEL DID NOT LIE.** It reported `manifold: false`, withheld
//! soundness, and refused the next boolean with `unsound_base` rather than
//! building on a broken body. This is a capability ceiling, not a
//! certification failure — which is why the RED below is held with its real
//! assertion rather than softened into something that passes.
//!
//! [`axial_bore_through_a_cylinder_is_manifold`] is the control. It asserts
//! the SAME contract against the SAME helper and passes, which is what makes
//! the cross-bore failure a statement about the intersection rather than about
//! the assertion.

use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// Blank radius and height, shared by both arms so the only difference between
/// them is the bore axis.
const BLANK_R: f64 = 43.0;
const BLANK_H: f64 = 70.0;
/// Mesh tolerances, matching `boolean_multibody.rs` so the two files' mesh
/// measures are comparable.
const CHORD_TOL: f64 = 0.02;
const WELD_TOL: f64 = 1e-6;

fn cylinder(model: &mut BRepModel, base: Point3, axis: Vector3, r: f64, h: f64) -> SolidId {
    match TopologyBuilder::new(model).create_cylinder_3d(base, axis, r, h) {
        Ok(GeometryId::Solid(id)) => id,
        other => panic!("expected a solid; got {other:?}"),
    }
}

/// Build the blank, cut one through-bore along `axis`, and return the mesh
/// measure of the result. Both arms go through this, so neither can pass or
/// fail for a reason the other does not share.
fn bore_and_measure(
    axis: Vector3,
    tool_base: Point3,
    tool_r: f64,
    tool_h: f64,
    label: &str,
) -> geometry_engine::harness::watertight::ManifoldReport {
    let mut model = BRepModel::new();
    let blank = cylinder(
        &mut model,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
        BLANK_R,
        BLANK_H,
    );

    let before =
        manifold_report(&model, blank, CHORD_TOL, WELD_TOL).expect("the blank cylinder must mesh");
    assert_eq!(
        before.nonmanifold_edges, 0,
        "{label}: the BLANK must be manifold before the bore, else this test \
         proves nothing about the bore"
    );
    assert_eq!(
        before.boundary_edges, 0,
        "{label}: the BLANK must be closed before the bore"
    );

    let tool = cylinder(&mut model, tool_base, axis, tool_r, tool_h);
    let bored = boolean_operation(
        &mut model,
        blank,
        tool,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("a through-bore difference must not error");

    let after =
        manifold_report(&model, bored, CHORD_TOL, WELD_TOL).expect("the bored solid must mesh");
    eprintln!(
        "{label}: tris={} boundary_edges={} nonmanifold_edges={} directed_dup={} components={}",
        after.triangles,
        after.boundary_edges,
        after.nonmanifold_edges,
        after.inconsistent_directed_edges,
        after.components,
    );
    after
}

/// CONTROL, and it passes: a bore along the blank's OWN axis is clean.
///
/// This exists to keep the RED below honest. It asserts the identical contract
/// through the identical helper, so a future change that made
/// `nonmanifold_edges` trivially zero — or that broke meshing outright — would
/// move this test too, and the pair would stop discriminating silently.
#[test]
fn axial_bore_through_a_cylinder_is_manifold() {
    let r = bore_and_measure(
        Vector3::new(0.0, 0.0, 1.0),
        Point3::new(0.0, 0.0, -10.0),
        12.0,
        BLANK_H + 20.0,
        "axial-r12",
    );
    assert_eq!(
        r.nonmanifold_edges, 0,
        "an axial through-bore must leave a manifold solid"
    );
    assert_eq!(
        r.boundary_edges, 0,
        "an axial through-bore must leave a closed solid"
    );
}

/// WAS RED, held with its real assertion: a cross-drilled bore through a
/// cylinder returned a NON-MANIFOLD solid (watertight and brep_valid, manifold
/// false) at every radius measured — curved cyl-cyl SSI, same family as the
/// PIPE-TEE case in `agent_build_eval.rs`; surfaced 2026-08-21 by an agent build.
///
/// FIXED by `b6604a31` ("kernel: the Steiner collapse was right about walls and
/// wrong about walls with holes"): the CDT no longer joins one bore breakout
/// straight to the antipodal one on a lateral carrying interior loops. Measured
/// in a detached worktree with this exact assertion: RED at `9121f7e7` with
/// `nonmanifold_edges = 110` (tris 5840), GREEN at `b6604a31` with
/// `nonmanifold_edges = 0` (tris 24118).
///
/// `9121f7e7` is NOT `b6604a31`'s parent — `95afeeda` is, with five UI/MCP/
/// timeline commits in between. The RED run stands for the parent anyway because
/// `git diff --stat 9121f7e7 95afeeda -- geometry-engine/` is EMPTY: the crate is
/// byte-identical across those five, so the kernel measured at `9121f7e7` IS the
/// kernel at `95afeeda`. The assertion was never edited after `03f2d678` pinned
/// it, so the pass is a kernel change, not a weakened fixture. `#[ignore]`
/// removed (Task 45).
///
/// The mesh QUALITY red for the same feature family is separate and still
/// parked: see `cross_bore_mesh_wings.rs`.
#[test]
fn cross_bore_through_a_cylinder_is_manifold() {
    let r = bore_and_measure(
        Vector3::new(0.0, 1.0, 0.0),
        Point3::new(0.0, -80.0, 30.0),
        12.0,
        160.0,
        "cross-r12",
    );
    assert_eq!(
        r.nonmanifold_edges, 0,
        "a cross-drilled through-bore must leave a manifold solid"
    );
    assert_eq!(
        r.boundary_edges, 0,
        "a cross-drilled through-bore must leave a closed solid"
    );
}

/// The failure did not depend on the bore being large or small, which is what
/// ruled out a tolerance-sized coincidence and pointed at the intersection
/// itself. Was held RED alongside its sibling for the same reason.
///
/// FIXED by `b6604a31`, with its sibling. Measured in a detached worktree: RED at
/// `9121f7e7` on the very first radius (`nonmanifold_edges = 66` at r5, tris
/// 5788), GREEN at `b6604a31` across r5/r12/r25 (0 nonmanifold edges, tris
/// 24814/24118/20254). See the sibling above for why a RED at `9121f7e7` is a RED
/// at `b6604a31`'s actual parent `95afeeda`. `#[ignore]` removed (Task 45).
///
/// RENAMED (Task 45) from `cross_bore_is_non_manifold_at_every_radius`. That name
/// recorded the defect the fixture was written to pin, but the body asserts — as
/// it always did — the manifoldness the defect denied. Once the ignore came off,
/// a green test named for the defect was a name that lies, so the name now says
/// what the assertion checks. The history is here instead.
#[test]
fn cross_bore_is_manifold_at_every_radius() {
    for radius in [5.0_f64, 12.0, 25.0] {
        let r = bore_and_measure(
            Vector3::new(0.0, 1.0, 0.0),
            Point3::new(0.0, -80.0, 30.0),
            radius,
            160.0,
            &format!("cross-r{radius}"),
        );
        assert_eq!(
            r.nonmanifold_edges, 0,
            "a cross-drilled bore of radius {radius} must leave a manifold solid"
        );
    }
}
