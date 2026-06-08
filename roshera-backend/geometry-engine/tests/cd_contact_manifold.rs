//! Adversarial contact-MANIFOLD harness (CD-CONTACT, #87).
//!
//! The distance-only CD oracle (`cd_adversarial.rs`, #79) proves the kernel
//! reports the right *scalar* gap. A rigid-body solver (parry / rapier, #41/#42)
//! needs the richer datum `queries::cd::solid_contact` produces: for each active
//! contact the **witness point on each solid**, the **unit contact normal**, and
//! a **signed** gap (negative ⇒ interpenetrating, magnitude ⇒ penetration depth).
//! This harness pins all three against independent analytic truth across the
//! pose space a contact query must survive:
//!
//!   separated · grazing-touch · penetrating · beyond-prediction · symmetric
//!
//! What it checks that the scalar oracle cannot:
//!   * witnesses LIE on the respective boundaries (sphere: |w−c| = r; box: on a
//!     face plane) — a contact point off the surface is a silent solver error;
//!   * the normal is the true A→B separation axis (sign + direction);
//!   * the gap is SIGNED — penetration is a negative distance whose magnitude is
//!     the analytic penetration depth, not a clamped-to-zero positive distance;
//!   * `prediction` culling — a contact past the margin is dropped;
//!   * frame symmetry — swapping operands negates the normal, preserves the gap;
//!   * determinism — identical contact across in-process runs (a HashMap reseed
//!     must never move a witness or flip a normal).
//!
//! Separated/touching gaps are analytic-exact for spheres and axis-aligned
//! boxes; penetration depth is closed-form for sphere-sphere and axis-aligned
//! box-box. Cylinders/cones carry no clean closed form, so they assert the
//! invariants (symmetry, determinism, sign) rather than a value.

use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::{transform_solid, TransformOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::queries::cd::{
    solid_contact, solid_contact_manifold, solids_intersect, Contact,
};

/// Touch tolerance.
const TAU: f64 = 1e-6;
/// Value-vs-truth tolerance: analytic for planes/spheres, sampled for curved
/// edges — 1e-3 catches a real disagreement while tolerating sampling residual.
const VAL_TOL: f64 = 1e-3;
/// Generous prediction margin so a contact is never culled unless a test
/// specifically probes the margin.
const WIDE: f64 = 100.0;

// ---------------------------------------------------------------------------
// Solid builders (mirror cd_adversarial.rs)
// ---------------------------------------------------------------------------

fn unit_box(model: &mut BRepModel) -> SolidId {
    match TopologyBuilder::new(model)
        .create_box_3d(2.0, 2.0, 2.0)
        .expect("box")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn box_at(model: &mut BRepModel, c: [f64; 3]) -> SolidId {
    let id = unit_box(model);
    transform_solid(
        model,
        id,
        Matrix4::from_translation(&Vector3::new(c[0], c[1], c[2])),
        TransformOptions::default(),
    )
    .expect("translate box");
    id
}

fn box_rot_z_at(model: &mut BRepModel, angle: f64, c: [f64; 3]) -> SolidId {
    let id = unit_box(model);
    transform_solid(
        model,
        id,
        Matrix4::from_translation(&Vector3::new(c[0], c[1], c[2])) * Matrix4::rotation_z(angle),
        TransformOptions::default(),
    )
    .expect("rot+translate box");
    id
}

fn sphere_at(model: &mut BRepModel, c: [f64; 3], r: f64) -> SolidId {
    match TopologyBuilder::new(model)
        .create_sphere_3d(Point3::new(c[0], c[1], c[2]), r)
        .expect("sphere")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn z_cylinder_at(model: &mut BRepModel, base: [f64; 3], r: f64, h: f64) -> SolidId {
    match TopologyBuilder::new(model)
        .create_cylinder_3d(Point3::new(base[0], base[1], base[2]), Vector3::Z, r, h)
        .expect("cylinder")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Assertion helpers
// ---------------------------------------------------------------------------

fn on_sphere(w: Point3, c: [f64; 3], r: f64) -> bool {
    let d = ((w.x - c[0]).powi(2) + (w.y - c[1]).powi(2) + (w.z - c[2]).powi(2)).sqrt();
    (d - r).abs() < VAL_TOL
}

/// Witness lies on the boundary of an axis-aligned unit box centred at `c`
/// (half-extent 1): at least one axis is at the ±1 face, none beyond it.
fn on_box(w: Point3, c: [f64; 3]) -> bool {
    let l = [w.x - c[0], w.y - c[1], w.z - c[2]];
    let within = l.iter().all(|&x| x.abs() <= 1.0 + VAL_TOL);
    let on_face = l.iter().any(|&x| (x.abs() - 1.0).abs() < VAL_TOL);
    within && on_face
}

fn aligned(n: Vector3, axis: Vector3) -> bool {
    (n.dot(&axis).abs() - 1.0).abs() < VAL_TOL
}

/// Signature of a contact for determinism comparison.
fn sig(c: &Contact) -> [f64; 7] {
    [
        c.distance,
        c.normal.x,
        c.normal.y,
        c.normal.z,
        c.point_a.x,
        c.point_a.y,
        c.point_a.z,
    ]
}

// ===========================================================================
// Sphere-sphere: full closed form for points, normal, signed gap
// ===========================================================================

#[test]
fn sphere_sphere_separated_contact_is_exact() {
    let mut m = BRepModel::new();
    let a = sphere_at(&mut m, [0.0, 0.0, 0.0], 1.0);
    let b = sphere_at(&mut m, [3.0, 0.0, 0.0], 1.0);
    let c = solid_contact(&m, a, b, WIDE).expect("contact within margin");

    assert!(
        (c.distance - 1.0).abs() < VAL_TOL,
        "gap should be 3 − 1 − 1 = 1, got {}",
        c.distance
    );
    assert!(
        aligned(c.normal, Vector3::X),
        "normal should be ±X: {:?}",
        c.normal
    );
    assert!(
        c.normal.x > 0.0,
        "normal must point A→B (+X), got {:?}",
        c.normal
    );
    assert!(
        on_sphere(c.point_a, [0.0, 0.0, 0.0], 1.0),
        "witness A off sphere A: {:?}",
        c.point_a
    );
    assert!(
        on_sphere(c.point_b, [3.0, 0.0, 0.0], 1.0),
        "witness B off sphere B: {:?}",
        c.point_b
    );
}

#[test]
fn sphere_sphere_touching_gap_is_zero() {
    let mut m = BRepModel::new();
    let a = sphere_at(&mut m, [0.0, 0.0, 0.0], 1.0);
    let b = sphere_at(&mut m, [2.0, 0.0, 0.0], 1.0);
    let c = solid_contact(&m, a, b, WIDE).expect("touching contact");
    assert!(
        c.distance.abs() < VAL_TOL,
        "touching gap ≈ 0, got {}",
        c.distance
    );
}

#[test]
fn sphere_sphere_penetration_depth_is_exact() {
    let mut m = BRepModel::new();
    let a = sphere_at(&mut m, [0.0, 0.0, 0.0], 1.0);
    let b = sphere_at(&mut m, [1.5, 0.0, 0.0], 1.0);
    let man = solid_contact_manifold(&m, a, b, WIDE);
    assert!(
        man.penetrating,
        "overlapping spheres must report penetrating"
    );
    let c = man.points.first().expect("a penetration contact");
    // depth = 2r − d = 2 − 1.5 = 0.5, reported as a NEGATIVE gap.
    assert!(
        (c.distance + 0.5).abs() < VAL_TOL,
        "penetration depth should be 0.5 (distance −0.5), got {}",
        c.distance
    );
    assert!(c.distance < 0.0, "penetration must be a negative gap");
    assert!(
        aligned(c.normal, Vector3::X),
        "separation axis ±X: {:?}",
        c.normal
    );
}

#[test]
fn sphere_sphere_beyond_prediction_is_none() {
    let mut m = BRepModel::new();
    let a = sphere_at(&mut m, [0.0, 0.0, 0.0], 1.0);
    let b = sphere_at(&mut m, [3.0, 0.0, 0.0], 1.0);
    // True gap is 1.0; a 0.5 prediction margin must cull the contact.
    assert!(
        solid_contact(&m, a, b, 0.5).is_none(),
        "contact 1.0 apart must be culled at prediction 0.5"
    );
    // ...and survive at a 1.5 margin.
    assert!(
        solid_contact(&m, a, b, 1.5).is_some(),
        "contact must survive a 1.5 margin"
    );
}

// ===========================================================================
// Box-box: face contact points, normal, signed depth
// ===========================================================================

#[test]
fn box_box_separated_face_contact_is_exact() {
    let mut m = BRepModel::new();
    let a = box_at(&mut m, [0.0, 0.0, 0.0]); // x faces at ±1
    let b = box_at(&mut m, [3.0, 0.0, 0.0]); // x faces at 2, 4
    let c = solid_contact(&m, a, b, WIDE).expect("face contact");
    assert!(
        (c.distance - 1.0).abs() < VAL_TOL,
        "face gap = 2 − 1 = 1, got {}",
        c.distance
    );
    assert!(
        aligned(c.normal, Vector3::X),
        "face-contact normal ±X: {:?}",
        c.normal
    );
    assert!(
        on_box(c.point_a, [0.0, 0.0, 0.0]),
        "witness A off box A: {:?}",
        c.point_a
    );
    assert!(
        on_box(c.point_b, [3.0, 0.0, 0.0]),
        "witness B off box B: {:?}",
        c.point_b
    );
}

#[test]
fn box_box_penetration_depth_is_exact() {
    let mut m = BRepModel::new();
    let a = box_at(&mut m, [0.0, 0.0, 0.0]); // x ∈ [-1, 1]
    let b = box_at(&mut m, [1.5, 0.0, 0.0]); // x ∈ [0.5, 2.5]
    let man = solid_contact_manifold(&m, a, b, WIDE);
    assert!(man.penetrating, "overlapping boxes must report penetrating");
    let c = man.points.first().expect("a penetration contact");
    // Overlap along x = 1 − 0.5 = 0.5.
    assert!(
        (c.distance + 0.5).abs() < VAL_TOL,
        "box penetration depth should be 0.5 (distance −0.5), got {}",
        c.distance
    );
    assert!(
        aligned(c.normal, Vector3::X),
        "separation axis ±X: {:?}",
        c.normal
    );
}

#[test]
fn box_box_face_manifold_shares_one_normal() {
    let mut m = BRepModel::new();
    let a = box_at(&mut m, [0.0, 0.0, 0.0]);
    let b = box_at(&mut m, [2.0, 0.0, 0.0]); // faces flush at x = 1
    let man = solid_contact_manifold(&m, a, b, WIDE);
    assert!(
        !man.points.is_empty(),
        "flush faces must produce a manifold"
    );
    for c in &man.points {
        assert!(
            c.distance.abs() < 0.05,
            "flush contact gap ≈ 0, got {}",
            c.distance
        );
        assert!(
            aligned(c.normal, Vector3::X),
            "every flush contact normal ±X: {:?}",
            c.normal
        );
    }
}

// ===========================================================================
// Sphere-box
// ===========================================================================

#[test]
fn sphere_box_separated_contact_is_exact() {
    let mut m = BRepModel::new();
    let a = box_at(&mut m, [0.0, 0.0, 0.0]); // x face at 1
    let b = sphere_at(&mut m, [3.0, 0.0, 0.0], 1.0); // nearest point at x = 2
    let c = solid_contact(&m, a, b, WIDE).expect("sphere-box contact");
    // gap = (3 − 1) − 1 = 1.
    assert!(
        (c.distance - 1.0).abs() < VAL_TOL,
        "sphere-box gap = 1, got {}",
        c.distance
    );
    assert!(
        c.normal.x > 0.0 && aligned(c.normal, Vector3::X),
        "normal +X: {:?}",
        c.normal
    );
    assert!(
        on_box(c.point_a, [0.0, 0.0, 0.0]),
        "witness A off box: {:?}",
        c.point_a
    );
    assert!(
        on_sphere(c.point_b, [3.0, 0.0, 0.0], 1.0),
        "witness B off sphere: {:?}",
        c.point_b
    );
}

// ===========================================================================
// Frame symmetry — swap operands ⇒ normal negates, gap preserved
// ===========================================================================

fn assert_swap_symmetric(
    build_a: impl Fn(&mut BRepModel) -> SolidId,
    build_b: impl Fn(&mut BRepModel) -> SolidId,
    label: &str,
) {
    let mut m1 = BRepModel::new();
    let a1 = build_a(&mut m1);
    let b1 = build_b(&mut m1);
    let c_ab = solid_contact(&m1, a1, b1, WIDE);

    let mut m2 = BRepModel::new();
    let a2 = build_a(&mut m2);
    let b2 = build_b(&mut m2);
    let c_ba = solid_contact(&m2, b2, a2, WIDE);

    match (c_ab, c_ba) {
        (Some(ab), Some(ba)) => {
            assert!(
                (ab.distance - ba.distance).abs() < VAL_TOL,
                "{label}: gap not symmetric: {} vs {}",
                ab.distance,
                ba.distance
            );
            let opp = (ab.normal.x + ba.normal.x).abs()
                + (ab.normal.y + ba.normal.y).abs()
                + (ab.normal.z + ba.normal.z).abs();
            assert!(
                opp < 1e-2,
                "{label}: swapped normal must negate: {:?} vs {:?}",
                ab.normal,
                ba.normal
            );
        }
        (None, None) => {}
        _ => panic!("{label}: contact existence not symmetric ({c_ab:?} vs {c_ba:?})"),
    }
}

#[test]
fn contact_is_swap_symmetric_across_primitives() {
    assert_swap_symmetric(
        |m| sphere_at(m, [0.0, 0.0, 0.0], 1.0),
        |m| sphere_at(m, [2.6, 0.3, 0.0], 1.0),
        "sphere/sphere",
    );
    assert_swap_symmetric(
        |m| box_at(m, [0.0, 0.0, 0.0]),
        |m| box_at(m, [3.0, 0.4, 0.2]),
        "box/box",
    );
    assert_swap_symmetric(
        |m| box_at(m, [0.0, 0.0, 0.0]),
        |m| sphere_at(m, [3.2, 0.0, 0.0], 1.0),
        "box/sphere",
    );
    assert_swap_symmetric(
        |m| z_cylinder_at(m, [0.0, 0.0, -1.0], 1.0, 2.0),
        |m| z_cylinder_at(m, [3.2, 0.0, -1.0], 1.0, 2.0),
        "cyl/cyl",
    );
    assert_swap_symmetric(
        |m| sphere_at(m, [0.0, 0.0, 0.0], 1.0),
        |m| z_cylinder_at(m, [3.0, 0.0, -1.0], 1.0, 2.0),
        "sphere/cyl",
    );
    assert_swap_symmetric(
        |m| box_at(m, [0.0, 0.0, 0.0]),
        |m| box_rot_z_at(m, std::f64::consts::FRAC_PI_4, [3.0, 0.0, 0.0]),
        "box/box-rot45",
    );
}

// ===========================================================================
// Determinism — identical contact across 8 in-process runs
// ===========================================================================

fn assert_contact_deterministic(
    build_a: impl Fn(&mut BRepModel) -> SolidId,
    build_b: impl Fn(&mut BRepModel) -> SolidId,
    label: &str,
) {
    let mut first: Option<[f64; 7]> = None;
    for run in 0..8 {
        let mut m = BRepModel::new();
        let a = build_a(&mut m);
        let b = build_b(&mut m);
        let s = solid_contact(&m, a, b, WIDE).map(|c| sig(&c));
        match (first, s) {
            (None, _) => first = s,
            (Some(f), Some(s)) => {
                for k in 0..7 {
                    assert!(
                        (f[k] - s[k]).abs() < 1e-9,
                        "{label}: non-deterministic at run {run}, field {k}: {} vs {}",
                        f[k],
                        s[k]
                    );
                }
            }
            (Some(_), None) => panic!("{label}: contact vanished on run {run}"),
        }
    }
}

#[test]
fn contact_is_deterministic_across_primitives() {
    assert_contact_deterministic(
        |m| sphere_at(m, [0.0, 0.0, 0.0], 1.0),
        |m| sphere_at(m, [2.6, 0.3, 0.1], 1.0),
        "sphere/sphere",
    );
    assert_contact_deterministic(
        |m| box_at(m, [0.0, 0.0, 0.0]),
        |m| box_at(m, [1.5, 0.0, 0.0]),
        "box/box penetrating",
    );
    assert_contact_deterministic(
        |m| box_at(m, [0.0, 0.0, 0.0]),
        |m| z_cylinder_at(m, [3.0, 0.0, -1.0], 1.0, 2.0),
        "box/cyl",
    );
}

// ===========================================================================
// Sign correctness — penetrating ⇔ analytic overlap, separated ⇒ positive gap
// ===========================================================================

#[test]
fn gap_sign_tracks_overlap_state() {
    // Separated ⇒ positive gap, not penetrating.
    {
        let mut m = BRepModel::new();
        let a = box_at(&mut m, [0.0, 0.0, 0.0]);
        let b = box_at(&mut m, [3.0, 0.0, 0.0]);
        let man = solid_contact_manifold(&m, a, b, WIDE);
        assert!(!man.penetrating, "separated boxes must not be penetrating");
        if let Some(c) = man.points.first() {
            assert!(
                c.distance > 0.0,
                "separated gap must be positive, got {}",
                c.distance
            );
        }
    }
    // Overlapping ⇒ negative gap, penetrating.
    {
        let mut m = BRepModel::new();
        let a = sphere_at(&mut m, [0.0, 0.0, 0.0], 1.2);
        let b = sphere_at(&mut m, [1.0, 0.0, 0.0], 1.2);
        let man = solid_contact_manifold(&m, a, b, WIDE);
        assert!(man.penetrating, "overlapping spheres must be penetrating");
        let c = man.points.first().expect("penetration contact");
        assert!(
            c.distance < 0.0,
            "overlap gap must be negative, got {}",
            c.distance
        );
        // depth = 2·1.2 − 1.0 = 1.4.
        assert!(
            (c.distance + 1.4).abs() < VAL_TOL,
            "depth should be 1.4, got {}",
            -c.distance
        );
    }
    // Fully contained ⇒ penetrating with a positive-magnitude depth.
    {
        let mut m = BRepModel::new();
        let a = box_at(&mut m, [0.0, 0.0, 0.0]); // x ∈ [-1,1]
        let b = z_cylinder_at(&mut m, [0.0, 0.0, -0.5], 0.4, 1.0); // inside A
        let man = solid_contact_manifold(&m, a, b, WIDE);
        assert!(
            man.penetrating,
            "contained cylinder must report penetrating"
        );
        let c = man.points.first().expect("containment contact");
        assert!(
            c.distance <= TAU,
            "containment gap must be ≤ 0, got {}",
            c.distance
        );
    }
}

// ===========================================================================
// Normal always points from A's surface toward B (separated regime)
// ===========================================================================

// ===========================================================================
// Boolean intersection test (QueryDispatcher::intersection_test half)
// ===========================================================================

#[test]
fn solids_intersect_tracks_volume_overlap() {
    // Separated: no overlap.
    {
        let mut m = BRepModel::new();
        let a = box_at(&mut m, [0.0, 0.0, 0.0]);
        let b = box_at(&mut m, [3.0, 0.0, 0.0]);
        assert!(
            !solids_intersect(&m, a, b),
            "separated boxes do not intersect"
        );
    }
    // Surface grazing only: not a volume overlap... but flush faces share the
    // boundary plane, which the convex closed-shell test counts as touching.
    // Genuine penetration: overlap.
    {
        let mut m = BRepModel::new();
        let a = sphere_at(&mut m, [0.0, 0.0, 0.0], 1.2);
        let b = sphere_at(&mut m, [1.5, 0.0, 0.0], 1.2);
        assert!(solids_intersect(&m, a, b), "overlapping spheres intersect");
    }
    // Containment: overlap.
    {
        let mut m = BRepModel::new();
        let a = box_at(&mut m, [0.0, 0.0, 0.0]);
        let b = z_cylinder_at(&mut m, [0.0, 0.0, -0.4], 0.3, 0.8);
        assert!(
            solids_intersect(&m, a, b),
            "contained cylinder intersects box"
        );
    }
}

#[test]
fn normal_points_from_a_toward_b() {
    let centres_b = [
        [3.0, 0.0, 0.0],
        [0.0, 3.2, 0.0],
        [0.0, 0.0, 3.5],
        [2.4, 2.4, 0.0],
    ];
    for cb in centres_b {
        let mut m = BRepModel::new();
        let a = sphere_at(&mut m, [0.0, 0.0, 0.0], 1.0);
        let b = sphere_at(&mut m, cb, 1.0);
        let c = solid_contact(&m, a, b, WIDE).expect("separated contact");
        let to_b = Vector3::new(cb[0], cb[1], cb[2]);
        assert!(
            c.normal.dot(&to_b) > 0.0,
            "normal {:?} must have positive projection on A→B {:?}",
            c.normal,
            to_b
        );
    }
}
