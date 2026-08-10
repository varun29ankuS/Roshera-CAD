//! Kernel-layer reproduction attempt for TASK #5 — the live 08-09 report was
//! `signed_distance` / `"inside"` returning a CONSTANT `-10.0` for EVERY
//! probe point on a flange document, including points inside bolt holes
//! (should read OUTSIDE material) and points far outside the part.
//!
//! ## Verdict: the literal symptom (a constant answer for every probe) does
//! ## NOT reproduce in `queries::`. A DIFFERENT, real, partial defect does.
//!
//! `flange_on_x_axis_signed_distance_matches_closed_form` (below) probes
//! four points on a boolean-cut flange revolved about a non-Z axis:
//! material, bolt-hole interior, central bore, far outside. Three of the
//! four come back CORRECT and vary properly with the probe position
//! (`-5.0`/Inside, `+10.0`/Outside, `+150.0`/Outside — see
//! `diag_x_axis_flange_all_four_probes` in the investigation history). Only
//! the bolt-hole-interior probe is wrong (`-5.0`/Inside instead of
//! `+5.0`/Outside). A kernel path that answers three different probes
//! correctly with three different values cannot be the source of a
//! constant response to every probe — that rules `queries::` OUT as the
//! origin of the literal reported symptom, while still surfacing a genuine,
//! closed-form-provable sign defect worth pinning (see the RED tests below).
//!
//! Two live candidates for the CONSTANT-response symptom, neither
//! verifiable without the original live request/response pair (out of
//! reach at the kernel layer, and api-server is out of lane for this pass):
//!   * id-space confusion — `api-server/src/handlers/agent.rs::point_query`
//!     casts its `Path<u32>` straight to a kernel `SolidId`
//!     (`let sid = id as SolidId;`), and per project memory only a BOOLEAN
//!     mints a fresh solid id; if the live probe loop reused an id captured
//!     before the bolt-hole booleans, every probe would be evaluated
//!     against the WRONG (earlier) solid, and every probe on a small,
//!     already-stale solid could plausibly print the same distance;
//!   * a dropped/defaulted `point` field between whatever MCP tool issued
//!     the live probes and `PointQueryRequest { point: [f64; 3] }` — a
//!     silently-defaulted point would make every "different" probe actually
//!     query the SAME point, and `-10.0` would just be that one point's
//!     true answer. Not checked here — would need `roshera-mcp`'s
//!     point-query tool definition and the live request payload.
//!
//! ## What reproduces: a real, closed-form-provable sign defect
//!
//! Follows the closed-form-assertion pattern established in
//! `tests/sphere_cap_signed_distance.rs`: every expectation here is a
//! hand-computed closed-form distance, not a recorded/ratified kernel
//! output, so this cannot pass by agreeing with whatever the kernel already
//! prints.
//!
//! Shape: a washer built EXACTLY the way the REST path builds a flange —
//! `revolve_meridian` for the disc + central bore, then two off-axis
//! `BooleanOp::Difference` cylinders for bolt holes. Meridian profile (r, z):
//! `(10,0) -> (50,0) -> (50,10) -> (10,10)` (closed last->first), so the
//! solid is an annulus: outer radius 50, bore radius 10, thickness z in
//! [0, 10]. Two Ø10 (radius 5) through bolt holes at (±30, 0), z through
//! [-5, 15].
//!
//! DISCRIMINATOR (`flange_on_z_axis_with_offset_origin_matches_closed_form`):
//! the non-Z tests below change BOTH the revolve axis direction AND
//! `axis_origin` at once relative to the canonical tests above them.
//! Re-running the SAME non-zero `axis_origin` on the canonical Z axis
//! PASSES, so the origin offset is ruled out as the trigger.
//!
//! ## RESOLVED (TASK #5). Two defects, both periodic-unwrap, both fixed.
//!
//! The axis direction turned out NOT to be the cause either — it only
//! decided whether each latent defect got asked its question. Neither fix
//! mentions an axis:
//!
//!   1. `tessellation/surface.rs::winding_membership_periodic` — trim
//!      membership compared a canonical query `(u, v)` against a trim
//!      polygon living on a different branch of the surface's universal
//!      cover, rejecting points genuinely on the face. Fixes X and Y.
//!   2. `operations/face_arrangement.rs::nearest_periodic` — the cycle
//!      unwrap could only reach `k ∈ {-1, 0, 1}`, so a cut cycle that had
//!      already wrapped once could not close; the boolean silently dropped
//!      the whole hole-wall face. Fixes the oblique pose.
//!
//! Each test's own doc comment carries its measured numbers.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::revolve::{axis_frame, revolve_meridian, RevolveOptions};
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::queries::{classify_point, nearest_on_solid, signed_distance, PointClass};

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

/// Build the washer-with-two-bolt-holes flange described above.
fn build_flange() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let meridian = [(10.0, 0.0), (50.0, 0.0), (50.0, 10.0), (10.0, 10.0)];
    let disc = revolve_meridian(&mut m, &meridian, RevolveOptions::default())
        .expect("revolve flange disc (outer r=50, bore r=10, thickness 10)");

    let hole_a = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(30.0, 0.0, -5.0), Vector3::Z, 5.0, 20.0)
        .expect("bolt hole A"));
    let flange = boolean_operation(
        &mut m,
        disc,
        hole_a,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("bore bolt hole A");

    let hole_b = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(-30.0, 0.0, -5.0), Vector3::Z, 5.0, 20.0)
        .expect("bolt hole B"));
    let flange = boolean_operation(
        &mut m,
        flange,
        hole_b,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("bore bolt hole B");

    (m, flange)
}

/// A point in solid material, away from every hole: (20, 20, 5). Radial
/// distance from the revolve axis is `sqrt(20^2+20^2) = sqrt(800) ~=
/// 28.284`, inside the annulus `[10, 50]`; z=5 is mid-thickness `[0, 10]`.
/// Nearest boundary candidates (closed form):
///   * top/bottom cap (z=0 or z=10): 5
///   * bore wall (r=10): 28.284 - 10 = 18.284
///   * outer wall (r=50): 50 - 28.284 = 21.716
///   * bolt hole A centre (30,0): dist to (20,20) = sqrt(100+400) = 22.36,
///     minus hole radius 5 = 17.36
///   * bolt hole B centre (-30,0): dist to (20,20) = sqrt(2500+400) = 53.85,
///     minus hole radius 5 = 48.85
/// Nearest is the cap at exactly 5.0 -> signed distance must be -5.0.
#[test]
fn flange_point_in_material_is_negative_cap_distance() {
    let (m, flange) = build_flange();
    let p = Point3::new(20.0, 20.0, 5.0);
    let (sd, _face) = signed_distance(&m, flange, p).expect("signed_distance on material point");
    assert!(
        (sd + 5.0).abs() < 1e-6,
        "point in material away from every hole must read -5.0 (nearest cap), got {sd}"
    );
    assert_eq!(
        classify_point(&m, flange, p, 1e-6),
        PointClass::Inside,
        "point in material must classify Inside"
    );
}

/// A point at the CENTRE of bolt hole A, mid-thickness: (30, 0, 5). This is
/// squarely inside a hole that was cut OUT of the material, so it must read
/// OUTSIDE with signed distance +5.0 (closed-form: distance to the Ø10
/// hole's cylindrical wall from its own axis).
#[test]
fn flange_point_in_bolt_hole_is_positive_outside() {
    let (m, flange) = build_flange();
    let p = Point3::new(30.0, 0.0, 5.0);
    let (sd, _face) = signed_distance(&m, flange, p).expect("signed_distance in bolt hole");
    assert!(
        sd > 0.0,
        "point inside a bolt hole must be OUTSIDE material (positive sd), got {sd}"
    );
    assert!(
        (sd - 5.0).abs() < 1e-6,
        "point on the bolt-hole axis must read +5.0 (closed-form radius), got {sd}"
    );
    assert_eq!(
        classify_point(&m, flange, p, 1e-6),
        PointClass::Outside,
        "point inside a bolt hole must classify Outside, not Inside"
    );
}

/// A point on the central-bore axis, mid-thickness: (0, 0, 5). Deep "inside"
/// the part's bounding box but genuinely inside the through-bore, i.e.
/// outside material. Closed form: distance to the bore wall (r=10) is 10.0.
#[test]
fn flange_point_in_central_bore_is_positive_outside() {
    let (m, flange) = build_flange();
    let p = Point3::new(0.0, 0.0, 5.0);
    let (sd, _face) = signed_distance(&m, flange, p).expect("signed_distance in central bore");
    assert!(
        sd > 0.0,
        "point in the central bore must be OUTSIDE material (positive sd), got {sd}"
    );
    assert!(
        (sd - 10.0).abs() < 1e-6,
        "point on the bore axis must read +10.0 (closed-form bore radius), got {sd}"
    );
    assert_eq!(
        classify_point(&m, flange, p, 1e-6),
        PointClass::Outside,
        "point in the central bore must classify Outside"
    );
}

/// A point far outside the part entirely: (200, 0, 5). Closed form: nearest
/// boundary is the outer rim at r=50, so distance is 200-50 = 150.0.
#[test]
fn flange_point_far_outside_is_large_positive() {
    let (m, flange) = build_flange();
    let p = Point3::new(200.0, 0.0, 5.0);
    let (sd, _face) = signed_distance(&m, flange, p).expect("signed_distance far outside");
    assert!(
        sd > 0.0,
        "point far outside the part must be positive, got {sd}"
    );
    assert!(
        (sd - 150.0).abs() < 1e-6,
        "point far outside on the equatorial plane must read +150.0 (closed-form: 200 - outer radius 50), got {sd}"
    );
    assert_eq!(
        classify_point(&m, flange, p, 1e-6),
        PointClass::Outside,
        "point far outside must classify Outside"
    );
}

/// Cross-check `nearest_on_solid` directly (not just through `signed_distance`)
/// for the material point, since `signed_distance` is built on it — if the
/// magnitude is wrong, it must already be wrong here.
#[test]
fn flange_nearest_on_solid_matches_closed_form_for_material_point() {
    let (m, flange) = build_flange();
    let p = Point3::new(20.0, 20.0, 5.0);
    let (_face, _pt, d) = nearest_on_solid(&m, flange, p).expect("nearest_on_solid");
    assert!(
        (d - 5.0).abs() < 1e-6,
        "nearest_on_solid distance must be 5.0 (closed-form cap distance), got {d}"
    );
}

// ===========================================================================
// Same washer-with-two-bolt-holes flange, but revolved about a NON-canonical
// axis (not world Z) — the shape a real `sketch_revolve` produces once the
// sketch plane is not XY (SKETCH-DCM #45, "the sketch leaves the XY plane",
// landed on THIS branch). `revolve_meridian`'s own `axis_frame` picks e1/e2
// for the given axis direction; this builds the identical washer through
// that frame instead of assuming world (X, Y, Z), so any bug specific to an
// oblique axis frame (rather than the r/z math itself) shows up here even
// though the canonical-Z tests above are clean.
// ===========================================================================

/// Build the washer flange revolved about an arbitrary `axis_direction` /
/// `axis_origin`, using the SAME `axis_frame` the production revolve path
/// uses to place the meridian and the bolt holes — so this is a faithful
/// stand-in for a sketch-plane-driven revolve, not a special canonical case.
fn build_flange_on_axis(axis_origin: Point3, axis_direction: Vector3) -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let meridian = [(10.0, 0.0), (50.0, 0.0), (50.0, 10.0), (10.0, 10.0)];
    let opts = RevolveOptions {
        axis_origin,
        axis_direction,
        ..RevolveOptions::default()
    };
    let disc =
        revolve_meridian(&mut m, &meridian, opts).expect("revolve flange disc on oblique axis");

    let (axis, e1, _e2) = axis_frame(axis_direction).expect("axis frame");
    let lift = |r: f64, z: f64| -> Point3 { axis_origin + axis * z + e1 * r };

    // Bolt hole A: centred at meridian-frame (r=30, z=-5), axis along the
    // revolve axis, radius 5, through the full thickness (z: -5..15).
    let hole_a_base = lift(30.0, -5.0);
    let hole_a = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(hole_a_base, axis, 5.0, 20.0)
        .expect("bolt hole A (oblique axis)"));
    let flange = boolean_operation(
        &mut m,
        disc,
        hole_a,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("bore bolt hole A (oblique axis)");

    // Bolt hole B: centred at meridian-frame (r=-30, z=-5) — i.e. diametrically
    // opposite hole A, same axis.
    let hole_b_base = lift(-30.0, -5.0);
    let hole_b = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(hole_b_base, axis, 5.0, 20.0)
        .expect("bolt hole B (oblique axis)"));
    let flange = boolean_operation(
        &mut m,
        flange,
        hole_b,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("bore bolt hole B (oblique axis)");

    (m, flange)
}

/// Same four closed-form probes as the canonical-Z flange, but expressed in
/// the oblique axis frame and lifted to world space through `axis_frame` —
/// so the CLOSED-FORM expectations are geometry, not a re-statement of
/// whatever the kernel already computes.
fn assert_flange_probes_on_axis(label: &str, axis_origin: Point3, axis_direction: Vector3) {
    let (m, flange) = build_flange_on_axis(axis_origin, axis_direction);
    let (axis, e1, e2) = axis_frame(axis_direction).expect("axis frame");
    let lift =
        |r_e1: f64, r_e2: f64, z: f64| -> Point3 { axis_origin + axis * z + e1 * r_e1 + e2 * r_e2 };

    // Material point, away from both holes: (r=28.284 in a 20/20 e1/e2 split,
    // z=5) — nearest boundary is the z=10 cap at distance 5 (see the
    // canonical-Z derivation above; the geometry is identical, just rotated).
    let p_material = lift(20.0, 20.0, 5.0);
    let (sd, _) = signed_distance(&m, flange, p_material)
        .unwrap_or_else(|| panic!("{label}: signed_distance material point returned None"));
    assert!(
        (sd + 5.0).abs() < 1e-6,
        "{label}: material point must read -5.0 (nearest cap), got {sd}"
    );
    assert_eq!(
        classify_point(&m, flange, p_material, 1e-6),
        PointClass::Inside,
        "{label}: material point must classify Inside"
    );

    // Point at the centre of bolt hole A, mid-thickness: r=30 along e1, z=5.
    let p_hole_a = lift(30.0, 0.0, 5.0);
    let (sd, _) = signed_distance(&m, flange, p_hole_a)
        .unwrap_or_else(|| panic!("{label}: signed_distance bolt-hole point returned None"));
    assert!(
        sd > 0.0,
        "{label}: point inside bolt hole A must be OUTSIDE (positive sd), got {sd}"
    );
    assert!(
        (sd - 5.0).abs() < 1e-6,
        "{label}: point on bolt hole A's axis must read +5.0, got {sd}"
    );
    assert_eq!(
        classify_point(&m, flange, p_hole_a, 1e-6),
        PointClass::Outside,
        "{label}: point inside bolt hole A must classify Outside"
    );

    // Point on the central-bore axis, mid-thickness: r=0, z=5.
    let p_bore = lift(0.0, 0.0, 5.0);
    let (sd, _) = signed_distance(&m, flange, p_bore)
        .unwrap_or_else(|| panic!("{label}: signed_distance central-bore point returned None"));
    assert!(
        sd > 0.0,
        "{label}: point in the central bore must be OUTSIDE (positive sd), got {sd}"
    );
    assert!(
        (sd - 10.0).abs() < 1e-6,
        "{label}: point on the bore axis must read +10.0, got {sd}"
    );
    assert_eq!(
        classify_point(&m, flange, p_bore, 1e-6),
        PointClass::Outside,
        "{label}: point in the central bore must classify Outside"
    );

    // Point far outside the part entirely: r=200 along e1, z=5.
    let p_far = lift(200.0, 0.0, 5.0);
    let (sd, _) = signed_distance(&m, flange, p_far)
        .unwrap_or_else(|| panic!("{label}: signed_distance far-outside point returned None"));
    assert!(
        sd > 0.0,
        "{label}: point far outside must be positive, got {sd}"
    );
    assert!(
        (sd - 150.0).abs() < 1e-6,
        "{label}: point far outside must read +150.0 (200 - outer radius 50), got {sd}"
    );
    assert_eq!(
        classify_point(&m, flange, p_far, 1e-6),
        PointClass::Outside,
        "{label}: point far outside must classify Outside"
    );
}

/// DISCRIMINATOR: world Z (the SAME axis direction as the canonical tests
/// above) but with the SAME non-zero `axis_origin` the X-axis failing test
/// uses. The canonical tests only ever used `axis_origin = ZERO`; the
/// failing oblique-axis tests below change BOTH axis direction and origin at
/// once. This isolates which variable actually triggers the defect.
#[test]
fn flange_on_z_axis_with_offset_origin_matches_closed_form() {
    assert_flange_probes_on_axis(
        "Z-axis flange, offset origin",
        Point3::new(5.0, -3.0, 2.0),
        Vector3::Z,
    );
}

/// Revolve about world X (still axis-aligned, but NOT the default Z the
/// canonical tests above exercise) with a non-zero `axis_origin`.
///
/// TASK #5 — WAS RED, now GREEN. Root cause (measured, not inferred):
/// **period-blind trim membership**, in
/// `tessellation/surface.rs::is_point_inside_loop` / `classify_cached`.
///
/// `signed_distance` (queries/field.rs) takes its sign from `raycast_all`'s
/// parity; for this probe the ray genuinely crosses the boolean-minted
/// hole-wall `Cylinder` at `t ≈ 5.859`, `v ≈ 13.05` — squarely inside that
/// face's trimmed band `v ∈ [5, 15]`. The crossing was discarded because the
/// face's trim polygon, built by `project_loop_uv_unwrapped`, lives in the
/// surface's UNIVERSAL COVER at `u ∈ [-2π, 0]` (the boolean's rim circles
/// run against the cylinder's `u` direction), while the query `u ≈ 1.162`
/// arrives canonicalised to `[0, 2π)` by `Surface::exact_uv`. The plain
/// winding-number test compared the two on different branches of the
/// covering map and rejected a point that is the SAME point on the surface.
///
/// The earlier "axis-dependent unwrap math" diagnosis was WRONG, and the
/// instrumentation says so: the Z-axis flange's hole-wall face carries a
/// polygon with the IDENTICAL `u ∈ [-2π, 0]` window and its query `u` is
/// rejected the same way. Z only passes because with a world-Z flange the
/// fixed world-space parity ray leaves the hole above the rim — its wall
/// root lands at `v ≈ 16.30`, genuinely past `height_limits`, so the reject
/// is correct there and the defect never gets asked the question. The bug is
/// universal to periodic surfaces; the axis only decided whether it showed.
///
/// Fix: `winding_membership_periodic` tests every lift `u + k·period` that
/// can reach the polygon's extent (soundness argument on the function).
#[test]
fn flange_on_x_axis_signed_distance_matches_closed_form() {
    assert_flange_probes_on_axis("X-axis flange", Point3::new(5.0, -3.0, 2.0), Vector3::X);
}

/// Revolve about world Y with a non-zero `axis_origin`. Same defect as
/// [`flange_on_x_axis_signed_distance_matches_closed_form`] — see its doc
/// comment for the root cause. Measured wall crossing here: `t ≈ 5.315`,
/// `u ≈ 0.587`, `v ≈ 11.80`.
#[test]
fn flange_on_y_axis_signed_distance_matches_closed_form() {
    assert_flange_probes_on_axis("Y-axis flange", Point3::new(-1.0, 4.0, 7.0), Vector3::Y);
}

/// Revolve about a fully OBLIQUE axis (not aligned to any world axis) with a
/// non-zero `axis_origin` — forces `axis_frame`'s `perpendicular()` fallback
/// branch, the case a real non-XY sketch plane produces.
///
/// This pose was RED for a SECOND, independent defect, in the same
/// periodic-unwrap family but a different file: `BooleanOp::Difference` never
/// minted the hole-wall face at all (the result carried 4 faces where the
/// X/Y/Z poses carry 5), leaving an OPEN shell. `nearest_on_solid` then had
/// no wall to find and fell back to the cap's hole rim at
/// `sqrt(5² + 5²) = 5·sqrt(2) ≈ 7.071` — the wrong magnitude the old
/// `#[ignore]` reason recorded as a distinct symptom. It was the same class
/// of bug, not a distinct one.
///
/// Root cause: `operations/face_arrangement.rs::nearest_periodic` searched
/// only `k ∈ {-1, 0, 1}`. `unwrap_cycle_uv` walks a cycle cumulatively, so
/// after the hole-wall's cut cycle wraps once around the cylinder its anchor
/// sits near `-5.236` while the closing seam vertex's raw `u` is still
/// reported canonically as `2π`; closing it needs `k = -2`. The three-way
/// search returned the nearest of the three instead, the cycle failed to
/// close, its shoelace area came out `0.0` instead of `2π·10 ≈ 62.83`, and
/// `extract_regions` dropped the region as degenerate. The X pose survived
/// only because `atan2` happened to return `+0.0` rather than a tiny
/// negative at that seam vertex (which `closest_point` folds to `2π`), so
/// `k = -1` sufficed. `nearest_periodic` now solves `k` by rounding the
/// quotient, which is correct for any winding count.
#[test]
fn flange_on_oblique_axis_signed_distance_matches_closed_form() {
    assert_flange_probes_on_axis(
        "oblique-axis flange",
        Point3::new(3.0, -2.0, 6.0),
        Vector3::new(1.0, 1.0, 1.0),
    );
}
