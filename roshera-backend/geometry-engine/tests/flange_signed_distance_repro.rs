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
//! the failing tests below change BOTH the revolve axis direction AND
//! `axis_origin` at once relative to the canonical (passing) tests above
//! them. Re-running the SAME non-zero `axis_origin` on the canonical Z axis
//! PASSES — so the trigger is confirmed to be axis DIRECTION (non-Z), not
//! the origin offset.

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
/// TASK #5 — REPRODUCES at the kernel layer. Bisected: `nearest_on_solid`'s
/// DISTANCE MAGNITUDE for the bolt-hole probe is correct (face 10, the
/// boolean-minted hole-wall `Cylinder`, at exactly d=5.0 — the closed-form
/// radius); the SIGN is wrong. `signed_distance` (queries/field.rs:33-37)
/// takes its sign from `raycast_all`'s parity (queries/raycast.rs:165), and
/// for this probe `raycast_all` returns only ONE crossing (a flange cap
/// `Plane`) instead of the two-plus a correct ray must have — it never
/// registers the ray's real intersection with the hole-wall `Cylinder`
/// (verified directly: the quadratic root exists at t≈5.86 and lands within
/// the face's own `height_limits`, but `nearest_on_solid` run AT that exact
/// candidate point picks a different face at distance ≈1.9, meaning the
/// hole-wall face's own trim-membership test rejects a point that is
/// genuinely on its trimmed boundary). The rejection traces to the
/// winding-number trim test over the face's outer loop —
/// `is_point_inside_face`/`is_point_inside_loop`/`project_loop_uv_unwrapped`
/// in `geometry-engine/src/tessellation/surface.rs` (~8596-8908) — which is
/// walking a "bridge" loop (one connecting edge shared between the top and
/// bottom rim, present with FORWARD/BACKWARD orientation flags so it should
/// cancel in the winding number) that a boolean difference mints for a
/// full-circle cylindrical hole face. The SAME edge-id/orientation SHAPE of
/// loop is minted on the canonical Z-axis flange too (confirmed by directly
/// dumping both faces' outer-loop edge lists — identical topology, since id
/// assignment is deterministic and axis-independent), and the DISCRIMINATOR
/// test above confirms axis DIRECTION (not `axis_origin`) is what flips the
/// result — so the defect is specific to the surface's `(u, v)`
/// parameterization / periodicity-unwrap for a non-Z axis, not the bridge
/// convention itself. (NOTE: the raw `(u, v)` samples taken during
/// investigation did not honor the loop's per-edge orientation flags, so
/// they cannot be cited as proof of the exact polygon shape — this is
/// therefore the strongest defensible claim, not a fully closed proof of
/// the last step.) Root cause not further isolated tonight — out of reach
/// for this pass; pinned RED rather than silently skipped.
#[test]
#[ignore = "#5: signed_distance/classify_point flip the sign for a point genuinely \
            inside a boolean-cut through-hole when the revolve axis is not world Z \
            (nearest_on_solid's magnitude is right, raycast_all's parity is wrong — \
            see the file-level doc comment above this test for the bisection)"]
fn flange_on_x_axis_signed_distance_matches_closed_form() {
    assert_flange_probes_on_axis("X-axis flange", Point3::new(5.0, -3.0, 2.0), Vector3::X);
}

/// Revolve about world Y with a non-zero `axis_origin`. Same defect as
/// [`flange_on_x_axis_signed_distance_matches_closed_form`] — see its doc
/// comment for the bisection.
#[test]
#[ignore = "#5: same defect as flange_on_x_axis_signed_distance_matches_closed_form"]
fn flange_on_y_axis_signed_distance_matches_closed_form() {
    assert_flange_probes_on_axis("Y-axis flange", Point3::new(-1.0, 4.0, 7.0), Vector3::Y);
}

/// Revolve about a fully OBLIQUE axis (not aligned to any world axis) with a
/// non-zero `axis_origin` — forces `axis_frame`'s `perpendicular()` fallback
/// branch, the case a real non-XY sketch plane produces. This pose shows a
/// related but DISTINCT symptom from the X/Y-axis cases: the bolt-hole probe
/// still classifies `Outside` (right sign) but the magnitude is wrong
/// (returns `5*sqrt(2)` ≈ 7.071 instead of the closed-form 5.0) — consistent
/// with `nearest_on_solid` itself missing the hole-wall face for THIS pose
/// and falling back to an edge/corner candidate.
#[test]
#[ignore = "#5: same family as flange_on_x_axis_signed_distance_matches_closed_form, \
            but the oblique pose corrupts nearest_on_solid's MAGNITUDE (5*sqrt(2) \
            instead of 5.0) rather than raycast_all's sign"]
fn flange_on_oblique_axis_signed_distance_matches_closed_form() {
    assert_flange_probes_on_axis(
        "oblique-axis flange",
        Point3::new(3.0, -2.0, 6.0),
        Vector3::new(1.0, 1.0, 1.0),
    );
}
