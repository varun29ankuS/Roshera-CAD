//! Regression suite for the `RstarIndex`-backed broad-phase face-pair
//! pruning wired into [`operations::boolean::compute_face_intersections`].
//!
//! These tests pin three properties that a faulty pruning pass would
//! quietly break:
//!
//! 1. **Correctness under broad-phase**: a boolean op that triggers
//!    broad-phase pruning (total pair count > `BROAD_PHASE_PAIR_THRESHOLD`
//!    = 64) produces topologically valid solids with volumes that
//!    match the analytical expectation. A bbox false-negative would
//!    drop a real intersection, producing a corrupted shell.
//! 2. **Disjoint inputs**: two disjoint solids unioned produce a solid
//!    whose volume equals the sum of inputs — pruning should reject
//!    every cross-solid face pair and the narrow phase should never
//!    be reached.
//! 3. **Overlapping inputs**: pruning must NOT reject genuinely
//!    intersecting face pairs.
//!
//! See `geometry-engine/src/spatial/mod.rs` for the trait surface and
//! `compute_face_intersections` in `operations/boolean.rs` for the
//! wire-in.
//!
//! # Companion fix
//!
//! Writing this suite surfaced an unrelated kernel bug: primitive
//! constructors deduplicate coincident vertex positions via
//! `VertexStore::add_or_find`, which silently shares corner vertices
//! across primitives built at the same coordinates (e.g. two
//! `create_box_3d` calls before one is translated away). A subsequent
//! in-place `transform_solid` would then mutate the foreign solid's
//! geometry. The fix lives in
//! [`operations::transform::isolate_shared_topology`] (clones each
//! shared vertex and rewrites this solid's edge endpoints to point at
//! the clones); only `disjoint_unit_boxes_brute_force_path` exercises
//! that code path here, but it protects every transform-then-boolean
//! workflow in the kernel.
//!
//! # Companion fixes (bugs #50 and #51)
//!
//! Two further unrelated kernel bugs were surfaced and fixed while
//! writing this suite:
//!
//! - **#50** (overlapping unit boxes, brute-force path) — the T-
//!   junction pre-split in `presplit_boundary_t_junctions`
//!   (boolean.rs) now splits boundary edges whose interior is hit by
//!   a cut endpoint, so Greiner-Hormann imprint cuts that land on a
//!   boundary-edge interior are detected and merged correctly.
//! - **#51** (disjoint cylinders, brute-force path) — the brute-force
//!   `compute_face_intersections` loop now does bbox pre-pruning that
//!   matches the broad-phase path's semantics; `is_point_in_face`
//!   densely samples curves when the boundary has fewer than 3 corner
//!   vertices (closed-seam circular caps); and the shell-builder no
//!   longer rejects valid analytical-cap components (cone=2, cylinder
//!   =3 faces) under the spurious ≥4 polyhedral-shell heuristic.

// AUDIT-H13: Reason for `#![allow(clippy::expect_used)]` — test-only file.
// `expect(...)` on fixture/scaffolding code surfaces invariant violations
// with a clear message at the failure site, which is the desired failure
// mode in tests. The workspace `expect_used = "deny"` lint targets
// production panic-freedom; test scaffolding is exempt by design.
#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]

use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::{
    boolean_operation, transform_solid, BooleanOp, BooleanOptions, TransformOptions,
};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    let geom = TopologyBuilder::new(model)
        .create_box_3d(w, h, d)
        .expect("create_box_3d must succeed for positive dims");
    match geom {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid; got {other:?}"),
    }
}

fn make_cylinder(model: &mut BRepModel, radius: f64, height: f64) -> SolidId {
    let geom = TopologyBuilder::new(model)
        .create_cylinder_3d(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            radius,
            height,
        )
        .expect("create_cylinder_3d must succeed for positive dims");
    match geom {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid; got {other:?}"),
    }
}

fn translate(model: &mut BRepModel, solid: SolidId, delta: Vector3) {
    let mat = Matrix4::from_translation(&delta);
    transform_solid(model, solid, mat, TransformOptions::default())
        .expect("translation of a valid solid must succeed");
}

fn volume(model: &mut BRepModel, solid: SolidId) -> f64 {
    model
        .calculate_solid_volume(solid)
        .expect("volume must compute")
}

/// Two disjoint 1×1×1 boxes far apart. Six faces × six faces = 36
/// total pairs, BELOW the broad-phase threshold of 64 — exercises
/// the brute-force path. Volume must equal V(A) + V(B) = 2.
#[test]
fn disjoint_unit_boxes_brute_force_path() {
    let mut model = BRepModel::new();
    let a = make_box(&mut model, 1.0, 1.0, 1.0);
    let b = make_box(&mut model, 1.0, 1.0, 1.0);
    translate(&mut model, b, Vector3::new(100.0, 0.0, 0.0));

    let result = boolean_operation(
        &mut model,
        a,
        b,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("disjoint union must succeed");

    let v = volume(&mut model, result);
    assert!(
        (v - 2.0).abs() < 0.05,
        "disjoint union volume = {v}, expected ≈ 2.0"
    );
}

/// Two disjoint cylinders far apart. Each cylinder has 3 faces
/// (cap + closed-seam lateral + cap) for a total of 9 pairs, BELOW
/// the broad-phase threshold of 64. Exercises the brute-force path's
/// bbox pre-prune (bug #51): without it, the unbounded
/// `intersect_surface_plane` between A's caps and B's lateral
/// reports phantom imprint circles 50 units away that shred the
/// caps. V(A) + V(B) = 2π.
///
/// Before bug #51 was fixed this case failed in three compounding
/// ways: (a) `compute_face_intersections` below the broad-phase
/// threshold skipped bbox pruning entirely, so disjoint Plane-
/// Cylinder pairs at 50 units distance produced phantom imprint
/// curves; (b) `is_point_in_face` fell through to `Ok(true)` for
/// any face whose boundary loop had fewer than 3 corner vertices,
/// which is every cylinder cap (single closed seam edge), so the
/// coincident-boundary check at the top of
/// `classify_face_relative_to_solid` spuriously fired OnBoundary for
/// pairs of disjoint coplanar caps; (c) `build_shells_from_faces`
/// rejected 3-face components under a ≥4 polyhedral-shell heuristic
/// even though closed-seam cylinders are perfectly valid closed
/// manifolds with that face count.
#[test]
fn disjoint_cylinders_broad_phase_path() {
    let mut model = BRepModel::new();
    let a = make_cylinder(&mut model, 1.0, 1.0);
    let b = make_cylinder(&mut model, 1.0, 1.0);
    translate(&mut model, b, Vector3::new(50.0, 0.0, 0.0));

    let v_a = volume(&mut model, a);
    let v_b = volume(&mut model, b);
    let expected = v_a + v_b;

    let result = boolean_operation(
        &mut model,
        a,
        b,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("disjoint cylinder union must succeed");

    let v = volume(&mut model, result);
    assert!(
        (v - expected).abs() / expected.max(1.0) < 0.05,
        "disjoint cylinder union volume = {v}, expected ≈ {expected}"
    );
}

/// Overlapping 1×1×1 boxes offset by 0.5 along x. Pruning must NOT
/// reject the intersecting face pair (one face of A meets one face
/// of B on a square in the plane x=0.5). V = 2 − 0.5 = 1.5.
///
/// Before bug #50 was fixed this case returned V ≈ 4/3 instead of
/// 3/2: the cap-face coplanar imprint produced cut edges whose
/// endpoints landed on the interior of A's boundary edges, but the
/// shared-vertex skip in `compute_edge_intersections` (boolean.rs)
/// prevented the T-junction from being detected. The fix is the
/// `presplit_boundary_t_junctions` pass in
/// `operations/boolean.rs`, which projects every interior cut
/// endpoint onto the boundary curves and splits the boundary
/// before crossing detection runs.
#[test]
fn overlapping_boxes_union_correct_volume() {
    let mut model = BRepModel::new();
    let a = make_box(&mut model, 1.0, 1.0, 1.0);
    let b = make_box(&mut model, 1.0, 1.0, 1.0);
    translate(&mut model, b, Vector3::new(0.5, 0.0, 0.0));

    let result = boolean_operation(
        &mut model,
        a,
        b,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("overlapping union must succeed");

    let v = volume(&mut model, result);
    assert!(
        (v - 1.5).abs() < 0.05,
        "overlapping union volume = {v}, expected ≈ 1.5"
    );
}

/// Single Difference cut: a 4×4×4 box minus a strictly-contained
/// 2×2×2 hole offset by (0.3, 0.4, 0.5). The inner box lands at
/// corners (-0.7,-0.6,-0.5)→(1.3,1.4,1.5), entirely inside the outer
/// (-2,-2,-2)→(2,2,2) with no coincident face planes. V = 64 − 8 = 56.
/// Below threshold (6×6 = 36 pairs) — pins that the brute-force path
/// produces the same answer as before the broad-phase wire-in.
#[test]
fn single_difference_cut_below_threshold() {
    let mut model = BRepModel::new();
    let a = make_box(&mut model, 4.0, 4.0, 4.0);
    let b = make_box(&mut model, 2.0, 2.0, 2.0);
    translate(&mut model, b, Vector3::new(0.3, 0.4, 0.5));

    let result = boolean_operation(
        &mut model,
        a,
        b,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("box minus inner box must succeed");

    let v = volume(&mut model, result);
    assert!(
        (v - 56.0).abs() / 56.0 < 0.02,
        "box-minus-box volume = {v}, expected ≈ 56.0"
    );
}
