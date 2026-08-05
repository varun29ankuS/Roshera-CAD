// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Boolean SDF-classification oracle — an INDEPENDENT falsifier for boolean
//! correctness, built on the analytic signed-distance field.
//!
//! Boolean correctness today is checked by ONE method family: topology-based
//! certificates (soundness, watertight, manifold, Euler characteristic) and
//! closed-form volume oracles, all built on top of the same
//! `reconstruct_topology` face-selection pipeline. This harness adds a
//! genuinely SEPARATE second method: `classify_point`
//! (`src/queries/point.rs`), built on `nearest_on_solid` (closest-point-on-
//! surface) plus exact ray-parity (`src/queries/raycast.rs`) — a completely
//! different code path from boolean topology construction.
//!
//! ## The invariant: set membership, never min/max distance
//!
//! `min(sdf_A, sdf_B)` is the textbook implicit-union distance field, but it
//! is exact ONLY outside the union: once A and B merge, part of A's boundary
//! becomes interior material, so the true distance to the union's boundary
//! can exceed the distance to A's lone boundary. An oracle asserting that
//! identity everywhere would false-positive exactly at the seam — where
//! booleans are hardest — and would rightly get ignored. What IS exactly
//! true, everywhere, is set membership:
//!
//!   * `p ∈ (A ∪ B)`  ⟺  `p ∈ A  OR  p ∈ B`
//!   * `p ∈ (A ∩ B)`  ⟺  `p ∈ A  AND p ∈ B`
//!   * `p ∈ (A \ B)`  ⟺  `p ∈ A  AND NOT p ∈ B`
//!
//! This harness asserts exactly that: classify every sample point against
//! BOTH operands BEFORE the boolean runs, then classify the result AFTER, and
//! compare via the set-logic table above. Operands must be classified first
//! because `boolean_operation` RETIRES them (`model.solids.remove(solid_a)` /
//! `remove(solid_b)`, `src/operations/boolean.rs` ~line 701) — there is no
//! solid left to query afterward.
//!
//! We do NOT additionally assert the outside-only distance identity
//! (`sdf(result) == min(sdf_A, sdf_B)` for union, gated to points outside the
//! result). Set-membership already gives an exact, unconditional check at
//! every non-excluded sample; the distance identity would only add a
//! magnitude comparison restricted to the easier (outside) half of the
//! domain, which is where boolean bugs are least likely to hide. Left out to
//! keep the oracle's claim simple and exact rather than mixing an
//! unconditional check with a conditional one.
//!
//! ## Honesty
//!
//! This is a FALSIFIER, not a prover: "no disagreement found at these
//! points" is the only claim it is entitled to make — nothing here is named
//! or asserted as "correct". Points within `eps` of ANY relevant boundary
//! (either operand's, or the result's) have genuinely ambiguous
//! classification — a real property of the geometry, not a defect — and are
//! excluded from the equality check. Every case reports exactly how many
//! points were excluded, so the exclusion can never quietly swallow the hard
//! points while claiming coverage.
//!
//! One more limit, found while building this harness rather than assumed
//! up front: the expected membership for each sample comes from
//! `classify_point` applied to the OPERANDS, so this oracle is independent
//! of `boolean_operation`/`reconstruct_topology` but is NOT independent of
//! `classify_point`/`raycast_all` itself — a defect in that query path
//! poisons the oracle's own ground truth rather than the boolean under
//! test (see `cylinder_box_straddle_difference_matches_set_membership`
//! below, pinned `#[ignore]` for exactly this reason).
//!
//! ## Sampling
//!
//! Deterministic only: a fixed axis-aligned grid over the combined bounding
//! box of the two operands, plus points placed deliberately in the
//! interaction region (where boolean bugs live — a uniform grid mostly
//! samples empty space and proves little). No RNG, no hash-order dependence.

use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::{
    boolean_operation, transform_solid, BooleanOp, BooleanOptions, TransformOptions,
};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::queries::{classify_point, PointClass};

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

/// Deterministic axis-aligned grid over `[min, max]`, `n` samples per axis
/// (`n >= 2`, endpoints included). Same inputs always produce the same
/// points in the same order — no RNG, no hash-map iteration order.
fn grid_points(min: Point3, max: Point3, n: usize) -> Vec<Point3> {
    assert!(n >= 2, "grid needs at least 2 samples per axis");
    let mut pts = Vec::with_capacity(n * n * n);
    for i in 0..n {
        let x = min.x + (max.x - min.x) * (i as f64) / ((n - 1) as f64);
        for j in 0..n {
            let y = min.y + (max.y - min.y) * (j as f64) / ((n - 1) as f64);
            for k in 0..n {
                let z = min.z + (max.z - min.z) * (k as f64) / ((n - 1) as f64);
                pts.push(Point3::new(x, y, z));
            }
        }
    }
    pts
}

/// The exact set-membership identity this harness asserts (never min/max):
/// `A op B` membership derives purely from the operands' OWN membership.
fn expected_membership(op: BooleanOp, in_a: bool, in_b: bool) -> bool {
    match op {
        BooleanOp::Union => in_a || in_b,
        BooleanOp::Intersection => in_a && in_b,
        BooleanOp::Difference => in_a && !in_b,
    }
}

/// Outcome of one oracle run. Reported honestly: `compared` points found
/// "no disagreement" — that is the only claim this struct is entitled to
/// support. It is never evidence the boolean is "correct".
struct OracleStats {
    total: usize,
    excluded: usize,
    compared: usize,
    mismatches: Vec<String>,
}

/// Sample → classify (pre-boolean, both operands) → operate → classify
/// (post-boolean, result) → compare via set logic. `build` constructs two
/// fresh operands in a fresh model (fresh per call: the boolean retires its
/// inputs, so a model can never be reused across cases). `bbox_min/max`
/// bound the coarse grid; `near` supplies deliberate interaction-region
/// points, where boolean bugs live.
fn run_oracle(
    build: impl Fn(&mut BRepModel) -> (SolidId, SolidId),
    op: BooleanOp,
    bbox_min: Point3,
    bbox_max: Point3,
    grid_n: usize,
    near: &[Point3],
) -> OracleStats {
    let mut model = BRepModel::new();
    let (a, b) = build(&mut model);

    // eps: an explicit margin derived from the model's own working tolerance
    // (not the bare 1e-6 weld tolerance) — a SAMPLED point is not surface-
    // fitted the way a solver-produced point is, so near-boundary ambiguity
    // needs headroom over exact-surface tolerance. 100x the model's distance
    // tolerance is still tiny next to the 10-20 unit part scale used below,
    // while reliably excluding genuinely ambiguous points.
    let eps = model.tolerance().distance() * 100.0;

    let mut points = grid_points(bbox_min, bbox_max, grid_n);
    points.extend_from_slice(near);
    let total = points.len();

    // Classify against BOTH operands BEFORE the boolean retires them.
    let pre: Vec<(Point3, PointClass, PointClass)> = points
        .iter()
        .map(|&p| {
            let ca = classify_point(&model, a, p, eps);
            let cb = classify_point(&model, b, p, eps);
            (p, ca, cb)
        })
        .collect();

    let result = boolean_operation(&mut model, a, b, op, BooleanOptions::default())
        .unwrap_or_else(|e| panic!("boolean {op:?} failed: {e:?}"));

    let mut excluded = 0usize;
    let mut compared = 0usize;
    let mut mismatches = Vec::new();

    for (p, ca, cb) in pre {
        // Ambiguous input classification (near either operand's boundary):
        // the expected membership itself is undetermined here, not a defect.
        if ca == PointClass::On || cb == PointClass::On {
            excluded += 1;
            continue;
        }
        let cr = classify_point(&model, result, p, eps);
        // Ambiguous result classification (near the result's boundary,
        // which is always a subset of the operands' own boundaries): also
        // excluded, and still counted, never silently dropped.
        if cr == PointClass::On {
            excluded += 1;
            continue;
        }
        compared += 1;
        let in_a = ca == PointClass::Inside;
        let in_b = cb == PointClass::Inside;
        let expect_inside = expected_membership(op, in_a, in_b);
        let got_inside = cr == PointClass::Inside;
        if expect_inside != got_inside {
            mismatches.push(format!(
                "{op:?} at {p:?}: pre a={ca:?} b={cb:?} -> expected_inside={expect_inside}, \
                 result classified {cr:?}"
            ));
        }
    }

    OracleStats {
        total,
        excluded,
        compared,
        mismatches,
    }
}

fn assert_no_disagreement(stats: OracleStats, label: &str) {
    println!(
        "{label}: total={} excluded={} compared={}",
        stats.total, stats.excluded, stats.compared
    );
    assert!(
        stats.compared > 0,
        "{label}: every sample point was excluded as near-boundary ({} of {}); \
         the oracle asserted nothing",
        stats.excluded,
        stats.total
    );
    assert!(
        stats.mismatches.is_empty(),
        "{label}: SDF classification disagrees with boolean set-membership at {} of {} \
         compared points ({} excluded as near-boundary):\n{}",
        stats.mismatches.len(),
        stats.compared,
        stats.excluded,
        stats.mismatches.join("\n")
    );
}

/// Two 20^3 boxes, B shifted by (10,10,10) so they overlap in a corner
/// octant — the shared region is neither box's full body.
fn overlapping_boxes(model: &mut BRepModel) -> (SolidId, SolidId) {
    let a = sid(TopologyBuilder::new(model)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("box a"));
    let b = sid(TopologyBuilder::new(model)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("box b"));
    transform_solid(
        model,
        b,
        Matrix4::translation(10.0, 10.0, 10.0),
        TransformOptions::default(),
    )
    .expect("translate box b");
    (a, b)
}

/// Interaction-region points for the overlapping-box pair: inside the shared
/// corner octant, and just off each side of it along the diagonal, where the
/// new boolean seam faces are built.
fn overlapping_box_near_points() -> Vec<Point3> {
    vec![
        Point3::new(5.0, 5.0, 5.0),    // deep in the shared octant
        Point3::new(2.0, 2.0, 2.0),    // near A's far corner, inside overlap
        Point3::new(8.0, 8.0, 8.0),    // near B's near corner, inside overlap
        Point3::new(-3.0, 5.0, 5.0),   // in A only, outside overlap
        Point3::new(15.0, 15.0, 15.0), // in B only, outside overlap
        Point3::new(1.0, 9.0, 9.0),
        Point3::new(9.0, 1.0, 1.0),
        Point3::new(-8.0, -8.0, -8.0), // in neither
    ]
}

#[test]
fn box_union_overlapping_matches_set_membership() {
    let stats = run_oracle(
        overlapping_boxes,
        BooleanOp::Union,
        Point3::new(-10.0, -10.0, -10.0),
        Point3::new(20.0, 20.0, 20.0),
        6,
        &overlapping_box_near_points(),
    );
    assert_no_disagreement(stats, "box_union_overlapping");
}

#[test]
fn box_intersection_overlapping_matches_set_membership() {
    let stats = run_oracle(
        overlapping_boxes,
        BooleanOp::Intersection,
        Point3::new(-10.0, -10.0, -10.0),
        Point3::new(20.0, 20.0, 20.0),
        6,
        &overlapping_box_near_points(),
    );
    assert_no_disagreement(stats, "box_intersection_overlapping");
}

#[test]
fn box_difference_overlapping_matches_set_membership() {
    let stats = run_oracle(
        overlapping_boxes,
        BooleanOp::Difference,
        Point3::new(-10.0, -10.0, -10.0),
        Point3::new(20.0, 20.0, 20.0),
        6,
        &overlapping_box_near_points(),
    );
    assert_no_disagreement(stats, "box_difference_overlapping");
}

/// Sphere r=10 centred exactly ON the box's +X face plane (box 20^3 centred
/// at the origin has its +X face at x=10) — the sphere's own centre sits on
/// the box boundary, so the sphere straddles the face: roughly half of it
/// buried in the box, half proud of it. The degenerate case
/// `boolean_intersection_adversarial.rs` motivates (a boundary-coincident
/// pose), rebuilt here since that file's fixtures are private to its own
/// test binary.
fn sphere_box_face_straddle(model: &mut BRepModel) -> (SolidId, SolidId) {
    let box_id = sid(TopologyBuilder::new(model)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("box"));
    let sphere_id = sid(TopologyBuilder::new(model)
        .create_sphere_3d(Point3::ZERO, 10.0)
        .expect("sphere"));
    transform_solid(
        model,
        sphere_id,
        Matrix4::translation(10.0, 0.0, 0.0),
        TransformOptions::default(),
    )
    .expect("translate sphere onto the +X face");
    (box_id, sphere_id)
}

fn sphere_box_near_points() -> Vec<Point3> {
    vec![
        Point3::new(10.0, 0.0, 0.0), // sphere centre, exactly on the box face
        Point3::new(8.0, 0.0, 0.0),  // inside box, inside sphere
        Point3::new(15.0, 0.0, 0.0), // outside box, inside sphere
        Point3::new(5.0, 0.0, 0.0),  // inside box, outside sphere
        Point3::new(10.0, 3.0, 3.0), // near the straddle seam, off-axis
        Point3::new(18.0, 0.0, 0.0), // outside both
        Point3::new(10.0, 6.0, 0.0),
        Point3::new(10.0, 0.0, 6.0),
    ]
}

/// FINDING (this oracle, first run): fails with 9/124 compared points
/// disagreeing. NOT the boolean:
///   * The result's volume is 2094.3245 vs the analytic hemisphere
///     `(2/3)*pi*r^3` = 2094.3951 (0.003% error) — `reconstruct_topology`
///     built the right shape.
///   * `validate_solid_scoped` on the result reports zero errors — the
///     topology certificate sees nothing wrong either.
///   * The disagreeing points' PRE-boolean operand classifications were
///     independently checked against the closed-form sphere/box distance
///     and are correct (e.g. `(2,-2,-2)`: distance to sphere centre
///     `(10,0,0)` is `sqrt(72)=8.485 < 10` → genuinely inside the sphere).
///
/// ## Root cause — measured, and NOT where this comment used to say
///
/// This comment previously recorded the diagnosis "a curved trimmed face is
/// never even tried as a nearest candidate" in `nearest_on_solid`. That is
/// DISPROVED. Instrumenting the intersection result directly (probe point
/// `(2,-2,-2)`, the two-face shell `[7, 8]`):
///
///   * face 8 IS the spherical cap, and `nearest_on_solid`'s face pass DOES
///     generate its candidate correctly. `Sphere::closest_point` returns
///     `(u,v) = (3.3865713167166573, 1.808737451625105)`, `point_at` gives
///     `(0.571909584179366, -2.3570226039551576, -2.3570226039551585)`, and
///     the distance is `1.5147186257614298` — exactly the analytic
///     `|p - centre| - r = 10 - sqrt(72)`. The candidate is right.
///   * It is then THROWN AWAY by the trim test at `queries/point.rs:79-81`.
///     `point_inside_face_uv` → `tessellation::surface::is_point_inside_face`
///     short-circuits (that fn's first block) through
///     `operations::boolean::spherical_circular_membership`, which returns
///     `Some(false)` for this point.
///   * WHY it returns `Some(false)`: the `on_kept_side` closure
///     (`operations/boolean.rs`, in `spherical_circular_membership`) decides
///     which cap an outer circular trim loop keeps by comparing the query
///     point's side of the trim plane against THE SPHERE CENTRE's side. This
///     fixture translates the sphere so its centre lands exactly ON the box's
///     +X face plane (`x = 10`) — the trim is a GREAT circle. So
///     `point_plane_sidedness(n, c, centre)` is `Ordering::Equal`, `on_far` is
///     `false` for EVERY query point, and an outer loop (`keep_far = true`)
///     rejects the ENTIRE face. That closure's own comment already states the
///     behaviour ("an exactly-on-plane centre (a true great circle) has NO far
///     cap and reports `on_far = false` for every point") — it is documented,
///     but it is wrong for a face that genuinely IS a hemisphere.
///
/// ONE predicate, TWO symptoms — which is why a `nearest_on_solid`-only fix
/// could not turn this test green:
///   * `nearest_on_solid` skips the cap and returns face 7 (the flat
///     box-derived disk) at distance `8.0` instead of face 8 at `1.5147`;
///   * `raycast_all` gates every hit through the SAME predicate
///     (`queries/raycast.rs:213`), so every crossing of the cap is dropped,
///     the parity count loses crossings, and Inside/Outside flips. The parity
///     flip is what this test's mismatches actually measure — at `(2,-2,-2)`
///     `raycast_all` returns ZERO hits (even ⇒ `Outside`) where the truth is
///     `Inside`.
///
/// ## Why the obvious minimal fix is NOT taken
///
/// "Decline (`None`) when the trim plane contains the sphere centre, and let
/// the legacy winding test decide" works HERE and only here. Measured, same
/// instrumentation:
///   * this pose (cut plane `x=10`, sphere north_dir `+Z`): the cut is a
///     MERIDIAN great circle, so its UV footprint is a genuine rectangle
///     `u in [pi/2, 3pi/2] x v in [0, pi]`, signed area `-9.540618`, winding
///     `-1.0` at the query `(u,v)` → the winding test would answer CORRECTLY.
///   * the symmetric pose (sphere translated `(0,0,10)` instead, cut plane
///     `z=10` PERPENDICULAR to north_dir): the cut is an EQUATORIAL great
///     circle, iso-`v`, UV signed area `1.776357e-15`. That is below
///     `is_point_inside_loop`'s `DEGENERATE_AREA_TOL`, so it returns
///     `is_outer` = `true` and accepts the WHOLE sphere.
/// So the minimal fix swaps "rejects everything" for "accepts everything" on a
/// pose a user reaches simply by seating the sphere on the box's +Z face
/// instead of its +X face. That is a special case that passes this fixture and
/// breaks its mirror image, so it is deliberately not shipped.
///
/// ## The fix this needs (not in the query layer)
///
/// Replace the sphere-CENTRE reference in `on_kept_side` with a reference
/// derived from the trim curve's own traversal: at a sample point on the loop
/// take the co-edge tangent `T` (loop orientation flag and `face.orientation`
/// applied) and the face's outward normal `N`; the in-face direction is
/// `D = N x T`, and the kept side is `sign((p - c).n) == sign(D.n)`. It never
/// mentions the surface type, so it generalises to `conical_band_membership`
/// and to any planar-trimmed surface, and it is well-conditioned exactly where
/// the centre reference dies: measured `D.n = -1.0` — the correct kept side —
/// in BOTH great-circle poses, including the equatorial one where the winding
/// fallback degenerates.
///
/// BLOCKER, and why this is filed rather than fixed: that construction needs
/// co-edge orientation to be trustworthy in `reconstruct_topology` output, and
/// it is not. In the OFF-CENTRE pose (sphere translated `(14,0,0)`, an
/// ordinary small cap, which the current centre heuristic happens to get
/// right) the two faces of the two-face shell traverse the shared rim
/// IDENTICALLY — face 7 walks edges `[13,14,15]` as `v 8->9, 9->10, 10->8`
/// with flags `[true, true, true]`, and face 8 walks the SAME edges with the
/// SAME vertices and the SAME flags — which a closed 2-manifold forbids (the
/// two faces adjacent to an edge must traverse it oppositely). The two
/// great-circle poses DO get this right (face 7 `[15,16,17]` forward, face 8
/// `[17,16,15]` with `[false,false,false]`). So co-edge integrity in boolean
/// output must be established first, with its own red test, before a
/// co-edge-derived membership predicate can be trusted.
///
/// `spatial_query_core.rs`'s own cylinder/sphere probes do not cover this:
/// they deliberately avoid axis/seam-adjacent points (see that file's
/// comments), so this is a real gap this independent oracle closes.
///
/// CONFIRMED A SEPARATE DEFECT from the `cylinder_box_straddle` fix below
/// (`Surface::exact_uv`, the `Cylinder`/`Cone` height-clamp bug in raycast
/// trim-checking): re-run unmodified, this case still fails at exactly 9/124.
/// That fix does not touch `Sphere` (its `closest_point` never clamps `u`/`v`
/// to `param_limits` in the first place) and, per the measurements above, this
/// case is not a boundary mis-clamp at all.
///
/// ⚠ SERVED LIVE: `signed_distance` (`queries/field.rs`) returns
/// `nearest_on_solid`'s magnitude and face id unchanged, so the agent-facing
/// `signed_distance` endpoint reports `8.0` on face 7 where the truth is
/// `1.5147` on face 8. That is wrong in the equatorial pose too, where
/// `classify_point` happens to come out right by luck — so the live harm is
/// broader than the Inside/Outside flip this test pins.
#[test]
#[ignore = "spherical_circular_membership's sphere-centre reference degenerates \
            on a GREAT-circle trim and rejects the whole cap face, poisoning \
            both nearest_on_solid and raycast_all parity; fix belongs in \
            operations/boolean.rs and is blocked on co-edge integrity — see \
            doc comment"]
fn sphere_box_face_straddle_intersection_matches_set_membership() {
    let stats = run_oracle(
        sphere_box_face_straddle,
        BooleanOp::Intersection,
        Point3::new(-10.0, -10.0, -10.0),
        Point3::new(20.0, 10.0, 10.0),
        6,
        &sphere_box_near_points(),
    );
    assert_no_disagreement(stats, "sphere_box_face_straddle_intersection");
}

/// A radius-6 cylinder along Z, offset to x=8 within a 20^3 box centred at
/// the origin (box +X face at x=10): the cylinder's own wall (x in
/// [2, 14]) crosses that face, so part of the cylinder pokes out of the
/// box's side rather than staying fully interior or fully exterior — the
/// cylinder∘box degenerate straddle case.
fn cylinder_box_straddle(model: &mut BRepModel) -> (SolidId, SolidId) {
    let box_id = sid(TopologyBuilder::new(model)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("box"));
    let cyl_id = sid(TopologyBuilder::new(model)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -15.0), Vector3::Z, 6.0, 30.0)
        .expect("cylinder"));
    transform_solid(
        model,
        cyl_id,
        Matrix4::translation(8.0, 0.0, 0.0),
        TransformOptions::default(),
    )
    .expect("translate cylinder so its wall straddles the box's +X face");
    (box_id, cyl_id)
}

fn cylinder_box_near_points() -> Vec<Point3> {
    vec![
        Point3::new(8.0, 0.0, 0.0),  // cylinder axis, deep inside box
        Point3::new(2.0, 0.0, 0.0),  // inside both, near cyl's inner wall
        Point3::new(14.0, 0.0, 0.0), // outside box, inside cylinder wall
        Point3::new(9.0, 0.0, 0.0),  // inside box, inside cylinder
        Point3::new(8.0, 0.0, 12.0), // outside box (+Z), inside cylinder radius
        Point3::new(8.0, 5.0, 0.0),
        Point3::new(3.0, 5.0, 0.0),
        Point3::new(13.0, 3.0, 0.0),
    ]
}

/// RESOLVED. Originally failed 16/158 (this oracle, first run) — but that
/// failure did NOT falsify the boolean; the oracle's own GROUND TRUTH was
/// corrupted. Classifying the same probe points against the bare,
/// translated cylinder alone (no box, no boolean) reproduced most of the
/// wrong answers already, e.g. `(-4,-6,-9)` is `6*sqrt(5) ≈ 13.416` units
/// from the axis `(x=8,y=0)` (radius 6) — genuinely `Outside` — but
/// `classify_point` said `Inside`; `(8,-2,9)` is 2.0 units from the axis —
/// genuinely `Inside` — but `classify_point` said `Outside`.
///
/// ROOT CAUSE (confirmed by direct inspection of `raycast_all`'s hit list):
/// `Cylinder::closest_point` clamps the axial parameter `v` to
/// `height_limits` — correct for its designed purpose, nearest-point
/// PROJECTION of an arbitrary query point onto the finite trimmed face, but
/// wrong when `queries::raycast` reused it to recover the `(u, v)` of a
/// point ALREADY known to lie exactly on the analytic surface (a
/// ray/quadratic root). A lateral-surface root landing just past the real
/// rim (e.g. local `v ≈ 30.11` against a `height_limits` of `[0, 30]`) got
/// silently clamped to exactly `v = 30.0` — precisely on the trimmed face's
/// own UV-rectangle boundary — where the winding-number trim test accepted
/// it due to floating-point noise, fabricating an extra face crossing that
/// flipped the ray-parity even/odd count. Confirmed as the cause, and NOT
/// the `u=0` angular seam `spatial_query_core.rs` already dodges: the two
/// fabricated hits' own angular parameter `u` was computed directly
/// (`atan2` of the hit's radial offset from the axis) rather than assumed —
/// `≈47.6°` for the `(-4,-6,-9)` case and `≈16.8°` for the `(8,-2,9)` case,
/// both far from the `0°`/`360°` seam. The fabricated hit's
/// face/point/distance were printed directly from `raycast_all` and traced
/// to this exact height clamp.
///
/// FIX: `Surface::exact_uv` (new trait method, `primitives/surface.rs`)
/// gives `queries::raycast` the TRUE unclamped `(u, v)` for a point already
/// on the surface — `Cylinder`/`Cone` override it to skip only the
/// height/apex clamp, everything else (including `angle_limits` handling)
/// is unchanged. `closest_point` itself is untouched, so every OTHER
/// caller (nearest-point projection: `nearest_on_solid`, fillet/offset/
/// chamfer projections, etc.) keeps its existing, correct clamped
/// behaviour.
///
/// Once `classify_point` was corrected, this fixture's own ground truth
/// became trustworthy (per the file's `⚠` warning above) — re-run rather
/// than assumed, and it now reports 0 mismatches at 158 compared points
/// (66 excluded as near-boundary). A minimal, closed-form regression
/// (no boolean, no box) is pinned permanently at
/// `queries::point::tests::translated_cylinder_point_beyond_rim_classifies_correctly`.
/// See also `sphere_box_face_straddle_intersection_matches_set_membership`
/// below, still `#[ignore]`d — direct inspection confirms that failure is
/// an UNRELATED defect (curved trimmed-face candidate selection in
/// `nearest_on_solid`, not this clamp) and is untouched by this fix.
#[test]
fn cylinder_box_straddle_difference_matches_set_membership() {
    let stats = run_oracle(
        cylinder_box_straddle,
        BooleanOp::Difference,
        Point3::new(-10.0, -10.0, -15.0),
        Point3::new(20.0, 10.0, 15.0),
        6,
        &cylinder_box_near_points(),
    );
    assert_no_disagreement(stats, "cylinder_box_straddle_difference");
}
