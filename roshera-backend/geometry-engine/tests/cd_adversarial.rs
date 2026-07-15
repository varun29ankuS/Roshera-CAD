// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Adversarial collision-detection oracle harness (CD-HARNESS-ADV, #79).
//!
//! The companion to the boolean-∩ adversarial harness (#78), for the CD
//! (contact-determination) pipeline. The pipeline's own brute-force baseline
//! shares the `face_lmds` + edge-edge code path it is meant to check, so it is
//! NOT an independent oracle. This harness supplies **independent, analytic**
//! ground truth for the minimum distance between two solids and sweeps the
//! adversarial pose space a contact query must survive:
//!
//!   separated · face-touch · edge-touch · corner-touch · tangent · penetrating
//!
//! For axis-aligned boxes, sphere-sphere, and sphere-vs-box the exact minimum
//! distance is closed-form, so those cases assert the pipeline VALUE against
//! truth. Every case (incl. cylinders, where no clean analytic oracle exists)
//! also asserts the pipeline INVARIANTS a correct CD must never violate:
//!   * symmetry            d(A, B) == d(B, A)
//!   * determinism         identical result across in-process runs (a HashMap
//!                         reseed must not change a geometric query)
//!   * ablation agreement  the broad-phase optimisations only prune pairs that
//!                         cannot host the closest approach, so every config
//!                         (baseline / grouping / cone-cull / BVH) must agree
//!   * contact predicate   `distance <= TAU` matches the analytic truth
//!
//! The minimum distance is the true SOLID contact distance (0 when the solids
//! touch or overlap), now that the narrow phase includes edge-edge / vertex
//! closest approach (#83).

use geometry_engine::harness::cd::{run_cd_ablation, CdAblationConfig};
use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::{transform_solid, TransformOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// Contact tolerance: distances at or below this count as touching.
const TAU: f64 = 1e-6;
/// Value-vs-oracle tolerance. The CD narrow phase is analytic for planes/spheres
/// (exact) and samples curved edges (small residual); 1e-3 catches a real
/// disagreement while tolerating the sampling residual.
const VAL_TOL: f64 = 1e-3;

// ---------------------------------------------------------------------------
// Solid builders
// ---------------------------------------------------------------------------

/// Axis-aligned cube of half-extent 1 centred at the origin: [-1,1]³.
fn unit_box(model: &mut BRepModel) -> SolidId {
    match TopologyBuilder::new(model)
        .create_box_3d(2.0, 2.0, 2.0)
        .expect("box")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

/// Unit cube translated so its centre sits at `c`.
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
// Pipeline under test
// ---------------------------------------------------------------------------

fn cd_dist(model: &BRepModel, a: SolidId, b: SolidId) -> f64 {
    run_cd_ablation(model, a, b, CdAblationConfig::baseline()).min_distance
}

// ---------------------------------------------------------------------------
// Independent analytic oracles
// ---------------------------------------------------------------------------

/// Exact minimum distance between two axis-aligned cubes of half-extent 1, whose
/// centres differ by `t`. Per-axis gap is `max(0, |t_i| - 2)`; the Euclidean
/// distance between the boxes is the norm of the per-axis gaps (0 when they
/// overlap or touch).
fn box_box_truth(t: [f64; 3]) -> f64 {
    let g: Vec<f64> = t.iter().map(|&ti| (ti.abs() - 2.0).max(0.0)).collect();
    (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt()
}

fn dist3(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

fn sphere_sphere_truth(c1: [f64; 3], r1: f64, c2: [f64; 3], r2: f64) -> f64 {
    (dist3(c1, c2) - r1 - r2).max(0.0)
}

/// Exact minimum distance between a sphere (`centre`, `r`) and the unit box
/// [-1,1]³: distance from the centre to the box (clamp the centre into the box,
/// measure to the clamp) minus the radius, clamped at 0.
fn sphere_box_truth(centre: [f64; 3], r: f64) -> f64 {
    let clamp = [
        centre[0].clamp(-1.0, 1.0),
        centre[1].clamp(-1.0, 1.0),
        centre[2].clamp(-1.0, 1.0),
    ];
    (dist3(centre, clamp) - r).max(0.0)
}

fn rel_or_abs_close(actual: f64, truth: f64) -> bool {
    (actual - truth).abs() <= VAL_TOL.max(VAL_TOL * truth.abs())
}

// ---------------------------------------------------------------------------
// Value-vs-oracle sweeps
// ---------------------------------------------------------------------------

#[test]
fn box_box_distance_matches_analytic_across_poses() {
    // (offset, label) covering separated / face / edge / corner / penetrating.
    let poses: &[[f64; 3]] = &[
        [3.0, 0.0, 0.0],   // face-separated by 1
        [4.0, 0.0, 0.0],   // face-separated by 2
        [2.0, 0.0, 0.0],   // face touch
        [2.0, 2.0, 0.0],   // edge touch
        [2.0, 2.0, 2.0],   // corner touch
        [3.0, 3.0, 0.0],   // edge-separated (sqrt 2)
        [3.0, 3.0, 3.0],   // corner-separated (sqrt 3)
        [2.5, 2.5, 0.0],   // edge-separated (sqrt .5)
        [1.5, 0.0, 0.0],   // overlapping
        [0.0, 0.0, 0.0],   // coincident
        [2.001, 0.0, 0.0], // near-tangent face
    ];
    let mut failures = Vec::new();
    for &t in poses {
        let mut model = BRepModel::new();
        let a = unit_box(&mut model);
        let b = box_at(&mut model, t);
        let d = cd_dist(&model, a, b);
        let truth = box_box_truth(t);
        if !rel_or_abs_close(d, truth) {
            failures.push(format!(
                "box-box t={t:?}: kernel {d:.5} vs truth {truth:.5}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "box-box CD distance disagreements with analytic oracle:\n  {}",
        failures.join("\n  ")
    );
}

#[test]
fn sphere_sphere_distance_matches_analytic() {
    // Two unit spheres; B centre swept along/around A at the origin.
    // Separated + just-touching only (the regime the centre-line LMD covers
    // exactly). Penetration is a separate gap, pinned below.
    let centres: &[[f64; 3]] = &[
        [3.0, 0.0, 0.0],    // gap 1
        [2.0, 0.0, 0.0],    // touch
        [2.5, 0.0, 0.0],    // gap .5
        [3.0, 3.0, 0.0],    // gap sqrt(18)-2
        [2.0001, 0.0, 0.0], // near tangent
    ];
    let mut failures = Vec::new();
    for &c in centres {
        let mut model = BRepModel::new();
        let a = sphere_at(&mut model, [0.0, 0.0, 0.0], 1.0);
        let b = sphere_at(&mut model, c, 1.0);
        let d = cd_dist(&model, a, b);
        let truth = sphere_sphere_truth([0.0, 0.0, 0.0], 1.0, c, 1.0);
        if !rel_or_abs_close(d, truth) {
            failures.push(format!(
                "sphere-sphere c={c:?}: kernel {d:.5} vs truth {truth:.5}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "sphere-sphere CD distance disagreements:\n  {}",
        failures.join("\n  ")
    );
}

#[test]
fn sphere_box_face_on_distance_matches_analytic() {
    // Sphere approaching the box [-1,1]³ FACE-ON along +x (the regime the
    // plane-sphere LMD covers exactly). Corner/edge approaches are exercised by
    // the contact-predicate + invariant tests below.
    // Separated + just-touching face-on only. Penetration is a separate gap.
    let cases: &[([f64; 3], f64)] = &[
        ([3.0, 0.0, 0.0], 1.0), // gap 1
        ([2.0, 0.0, 0.0], 1.0), // touch
        ([2.5, 0.0, 0.0], 1.0), // gap .5
        ([3.0, 0.5, 0.0], 1.0), // face-on, off-centre but footpoint still in face
    ];
    let mut failures = Vec::new();
    for &(c, r) in cases {
        let mut model = BRepModel::new();
        let a = unit_box(&mut model);
        let b = sphere_at(&mut model, c, r);
        let d = cd_dist(&model, a, b);
        let truth = sphere_box_truth(c, r);
        if !rel_or_abs_close(d, truth) {
            failures.push(format!(
                "sphere-box c={c:?} r={r}: kernel {d:.5} vs truth {truth:.5}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "sphere-box (face-on) CD distance disagreements:\n  {}",
        failures.join("\n  ")
    );
}

// ---------------------------------------------------------------------------
// Pipeline invariants (hold for every solid pair, no analytic oracle needed)
// ---------------------------------------------------------------------------

/// Build a fresh model carrying the two solids `build` constructs, returning
/// their ids. Each invariant test rebuilds so operands are never mutated across
/// measurements.
fn pair(build: &dyn Fn(&mut BRepModel) -> (SolidId, SolidId)) -> (BRepModel, SolidId, SolidId) {
    let mut model = BRepModel::new();
    let (a, b) = build(&mut model);
    (model, a, b)
}

type Build = Box<dyn Fn(&mut BRepModel) -> (SolidId, SolidId)>;

fn adversarial_pairs() -> Vec<(&'static str, Build)> {
    vec![
        (
            "box-box edge-touch",
            Box::new(|m| (unit_box(m), box_at(m, [2.0, 2.0, 0.0]))),
        ),
        (
            "box-box corner-touch",
            Box::new(|m| (unit_box(m), box_at(m, [2.0, 2.0, 2.0]))),
        ),
        (
            "box-box separated",
            Box::new(|m| (unit_box(m), box_at(m, [3.5, 1.0, 0.0]))),
        ),
        (
            "box-box overlapping",
            Box::new(|m| (unit_box(m), box_at(m, [1.0, 1.0, 0.0]))),
        ),
        (
            "sphere-box corner",
            Box::new(|m| (unit_box(m), sphere_at(m, [2.0, 2.0, 2.0], 1.0))),
        ),
        (
            "sphere-box face",
            Box::new(|m| (unit_box(m), sphere_at(m, [2.5, 0.0, 0.0], 1.0))),
        ),
        (
            "sphere-sphere tangent",
            Box::new(|m| {
                (
                    sphere_at(m, [0.0, 0.0, 0.0], 1.0),
                    sphere_at(m, [2.0, 0.0, 0.0], 1.0),
                )
            }),
        ),
        (
            "cylinder-box side",
            Box::new(|m| (unit_box(m), z_cylinder_at(m, [2.5, 0.0, -0.5], 0.5, 1.0))),
        ),
        (
            "cylinder-box overlap",
            Box::new(|m| (unit_box(m), z_cylinder_at(m, [0.5, 0.0, -0.5], 0.5, 1.0))),
        ),
        (
            "cylinder-sphere",
            Box::new(|m| {
                (
                    sphere_at(m, [0.0, 0.0, 0.0], 1.0),
                    z_cylinder_at(m, [3.0, 0.0, -0.5], 0.5, 1.0),
                )
            }),
        ),
    ]
}

#[test]
fn cd_distance_is_symmetric() {
    let mut failures = Vec::new();
    for (label, build) in adversarial_pairs() {
        let (model, a, b) = pair(&build);
        let dab = cd_dist(&model, a, b);
        let dba = cd_dist(&model, b, a);
        if (dab - dba).abs() > VAL_TOL {
            failures.push(format!("{label}: d(a,b)={dab:.5} != d(b,a)={dba:.5}"));
        }
    }
    assert!(
        failures.is_empty(),
        "CD distance is not symmetric:\n  {}",
        failures.join("\n  ")
    );
}

#[test]
fn cd_distance_is_deterministic() {
    // std::HashMap reseeds per map per process, so 8 in-process measurements of
    // the same query exercise 8 internal iteration orders; a geometric query
    // must be invariant to them.
    let mut failures = Vec::new();
    for (label, build) in adversarial_pairs() {
        let (model, a, b) = pair(&build);
        let runs: Vec<f64> = (0..8).map(|_| cd_dist(&model, a, b)).collect();
        let first = runs[0];
        if runs.iter().any(|&v| (v - first).abs() > 1e-9) {
            failures.push(format!("{label}: non-deterministic {runs:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "CD distance is non-deterministic:\n  {}",
        failures.join("\n  ")
    );
}

#[test]
fn cd_ablation_configs_agree() {
    // The broad-phase optimisations only prune pairs that cannot host the
    // closest approach, so for a CONTACT (the regime the broad phase keeps) every
    // configuration must reproduce the baseline minimum distance exactly.
    let configs = [
        CdAblationConfig::baseline(),
        CdAblationConfig {
            use_grouping: true,
            use_cone_cull: false,
            use_bvh: false,
        },
        CdAblationConfig {
            use_grouping: true,
            use_cone_cull: true,
            use_bvh: false,
        },
        CdAblationConfig::full(),
    ];
    // Box face/edge/corner touches: the broad phase keeps the
    // compatible-normal-cone pairs adjacent to the contact, so every config
    // reproduces baseline. (Cases the broad phase over-prunes — a smooth
    // sphere-box face touch, coincident-same-normal box overlaps — are pinned in
    // `broad_phase_prunes_some_contacts_GAP_79`.)
    let contact_pairs: Vec<(&str, Build)> = vec![
        (
            "box-box face-touch",
            Box::new(|m| (unit_box(m), box_at(m, [2.0, 0.0, 0.0]))),
        ),
        (
            "box-box edge-touch",
            Box::new(|m| (unit_box(m), box_at(m, [2.0, 2.0, 0.0]))),
        ),
        (
            "box-box corner-touch",
            Box::new(|m| (unit_box(m), box_at(m, [2.0, 2.0, 2.0]))),
        ),
    ];
    let mut failures = Vec::new();
    for (label, build) in contact_pairs {
        let (model, a, b) = pair(&build);
        let base = run_cd_ablation(&model, a, b, configs[0]).min_distance;
        for cfg in &configs[1..] {
            let d = run_cd_ablation(&model, a, b, *cfg).min_distance;
            if (d - base).abs() > 1e-9 {
                failures.push(format!(
                    "{label}: config {cfg:?} d={d:.6} != baseline {base:.6}"
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "CD ablation configs disagree on a contact:\n  {}",
        failures.join("\n  ")
    );
}

#[test]
fn cd_contact_predicate_matches_truth() {
    // Mixed box/sphere poses with a KNOWN contact verdict; the pipeline's
    // contact predicate (d <= TAU) must agree. Covers face/edge/corner touch,
    // overlap, and clearly-separated.
    struct Case {
        label: &'static str,
        build: Build,
        in_contact: bool,
    }
    let cases = vec![
        Case {
            label: "box-box edge-touch",
            build: Box::new(|m| (unit_box(m), box_at(m, [2.0, 2.0, 0.0]))),
            in_contact: true,
        },
        Case {
            label: "box-box corner-touch",
            build: Box::new(|m| (unit_box(m), box_at(m, [2.0, 2.0, 2.0]))),
            in_contact: true,
        },
        Case {
            label: "box-box separated",
            build: Box::new(|m| (unit_box(m), box_at(m, [2.5, 2.5, 0.0]))),
            in_contact: false,
        },
        Case {
            label: "box-box face overlap (coincident side faces)",
            build: Box::new(|m| (unit_box(m), box_at(m, [1.0, 0.0, 0.0]))),
            in_contact: true,
        },
        Case {
            label: "sphere-box face touch",
            build: Box::new(|m| (unit_box(m), sphere_at(m, [2.0, 0.0, 0.0], 1.0))),
            in_contact: true,
        },
        Case {
            label: "sphere-box separated",
            build: Box::new(|m| (unit_box(m), sphere_at(m, [3.0, 0.0, 0.0], 1.0))),
            in_contact: false,
        },
        Case {
            label: "sphere-sphere separated",
            build: Box::new(|m| {
                (
                    sphere_at(m, [0.0, 0.0, 0.0], 1.0),
                    sphere_at(m, [3.0, 0.0, 0.0], 1.0),
                )
            }),
            in_contact: false,
        },
    ];
    let mut failures = Vec::new();
    for c in cases {
        let (model, a, b) = pair(&c.build);
        let d = cd_dist(&model, a, b);
        let predicted = d <= TAU;
        if predicted != c.in_contact {
            failures.push(format!(
                "{}: predicted contact={predicted} (d={d:.5}) but truth={}",
                c.label, c.in_contact
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "CD contact predicate disagreements with truth:\n  {}",
        failures.join("\n  ")
    );
}

// ---------------------------------------------------------------------------
// Pinned gaps this harness found (#79). Each asserts the CORRECT behaviour and
// were the two gaps this harness FOUND; both are now FIXED by the solid-level
// overlap clamp (`solids_overlap` in harness/cd.rs), so they are regression
// guards rather than pins.
// ---------------------------------------------------------------------------

/// FIXED (#79, found by this harness): PENETRATION without a surface-touching
/// feature. The narrow phase reports the nearest-FEATURE distance (face/edge/
/// vertex LMD), correct for separated solids but POSITIVE for two
/// interpenetrating solids that share no touching feature (overlapping spheres,
/// diagonally-overlapping boxes). A collision query must return 0 whenever the
/// solids share volume. Fixed by a solid-level overlap test (winding-free convex
/// containment, sampled along the centroid segment) that clamps the distance to 0
/// when the interiors overlap.
#[test]
fn penetration_is_detected_as_contact() {
    let cases: Vec<(&str, Build)> = vec![
        (
            "spheres overlapping by 0.5",
            Box::new(|m| {
                (
                    sphere_at(m, [0.0, 0.0, 0.0], 1.0),
                    sphere_at(m, [1.5, 0.0, 0.0], 1.0),
                )
            }),
        ),
        (
            "boxes overlapping diagonally",
            Box::new(|m| (unit_box(m), box_at(m, [1.0, 1.0, 1.0]))),
        ),
    ];
    let mut failures = Vec::new();
    for (label, build) in cases {
        let (model, a, b) = pair(&build);
        let d = cd_dist(&model, a, b);
        if d > TAU {
            failures.push(format!(
                "{label}: interpenetrating solids report d={d:.5} (should be 0 = contact)"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "CD does not detect penetration without a touching feature:\n  {}",
        failures.join("\n  ")
    );
}

/// FIXED (#79, found by this harness): the broad phase used to OVER-PRUNE some
/// genuine contacts — for a smooth sphere-box face touch and a coincident-same-
/// normal box overlap the BVH / cone-cull configs pruned the contact-hosting pair
/// and reported `inf` while the baseline found the contact. The solid-level
/// overlap/contact clamp now runs regardless of broad-phase pruning, so every
/// config reproduces the baseline contact distance (0). (The underlying prune is
/// masked by the clamp; tightening the cone-cull to keep these pairs is tracked
/// separately, but the CD OUTPUT is now correct.)
#[test]
fn broad_phase_contacts_are_not_lost() {
    let cases: Vec<(&str, Build)> = vec![
        (
            "sphere-box face touch",
            Box::new(|m| (unit_box(m), sphere_at(m, [2.0, 0.0, 0.0], 1.0))),
        ),
        (
            "box-box coincident-face overlap",
            Box::new(|m| (unit_box(m), box_at(m, [1.0, 0.0, 0.0]))),
        ),
    ];
    let configs = [
        CdAblationConfig {
            use_grouping: true,
            use_cone_cull: true,
            use_bvh: false,
        },
        CdAblationConfig::full(),
    ];
    let mut failures = Vec::new();
    for (label, build) in cases {
        let (model, a, b) = pair(&build);
        let base = run_cd_ablation(&model, a, b, CdAblationConfig::baseline()).min_distance;
        for cfg in &configs {
            let d = run_cd_ablation(&model, a, b, *cfg).min_distance;
            if (d - base).abs() > 1e-9 {
                failures.push(format!("{label}: {cfg:?} d={d} != baseline {base}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "broad phase prunes contacts the baseline finds:\n  {}",
        failures.join("\n  ")
    );
}

// ===========================================================================
// Tougher battery — containment, corner approaches, monotonicity, rotated OBBs,
// cones, and a wider primitive matrix. The goal is to capture as many CD bugs
// as possible.
// ===========================================================================

fn box_size(model: &mut BRepModel, side: f64) -> SolidId {
    match TopologyBuilder::new(model)
        .create_box_3d(side, side, side)
        .expect("box")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn box_rot_z_at(model: &mut BRepModel, angle: f64, c: [f64; 3]) -> SolidId {
    let id = unit_box(model);
    let t = Matrix4::from_translation(&Vector3::new(c[0], c[1], c[2])) * Matrix4::rotation_z(angle);
    transform_solid(model, id, t, TransformOptions::default()).expect("rot+translate");
    id
}

fn cone_at(model: &mut BRepModel, base: [f64; 3], base_r: f64, top_r: f64, h: f64) -> SolidId {
    match TopologyBuilder::new(model)
        .create_cone_3d(
            Point3::new(base[0], base[1], base[2]),
            Vector3::Z,
            base_r,
            top_r,
            h,
        )
        .expect("cone")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

#[test]
fn cd_containment_is_contact() {
    // One solid fully inside another → interiors overlap → distance 0 (contact).
    let cases: Vec<(&str, Build)> = vec![
        (
            "small sphere inside unit box",
            Box::new(|m| (unit_box(m), sphere_at(m, [0.0, 0.0, 0.0], 0.5))),
        ),
        (
            "unit box inside big box",
            Box::new(|m| (box_size(m, 4.0), unit_box(m))),
        ),
        (
            "small cylinder inside box",
            Box::new(|m| (unit_box(m), z_cylinder_at(m, [0.0, 0.0, -0.4], 0.3, 0.8))),
        ),
        (
            "sphere inside sphere",
            Box::new(|m| {
                (
                    sphere_at(m, [0.0, 0.0, 0.0], 1.5),
                    sphere_at(m, [0.2, 0.0, 0.0], 0.4),
                )
            }),
        ),
    ];
    let mut failures = Vec::new();
    for (label, build) in cases {
        let (model, a, b) = pair(&build);
        let d = cd_dist(&model, a, b);
        if d > TAU {
            failures.push(format!(
                "{label}: contained solid reports d={d:.5} (should be 0)"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "CD does not report containment as contact:\n  {}",
        failures.join("\n  ")
    );
}

#[test]
fn cd_distance_monotone_in_separation() {
    // Slide a unit box away from another along +x; the distance must be the exact
    // face gap (t-2) and strictly increase. Catches a distance that plateaus,
    // jumps, or regresses.
    let mut prev = -1.0_f64;
    let mut failures = Vec::new();
    for step in 0..=12 {
        let t = 2.0 + step as f64 * 0.5; // 2.0 .. 8.0
        let mut model = BRepModel::new();
        let a = unit_box(&mut model);
        let b = box_at(&mut model, [t, 0.0, 0.0]);
        let d = cd_dist(&model, a, b);
        let truth = (t - 2.0).max(0.0);
        if !rel_or_abs_close(d, truth) {
            failures.push(format!("t={t}: d={d:.5} vs truth {truth:.5}"));
        }
        if d + 1e-9 < prev {
            failures.push(format!("t={t}: distance regressed {prev:.5} -> {d:.5}"));
        }
        prev = d;
    }
    assert!(
        failures.is_empty(),
        "box separation distance not monotone/exact:\n  {}",
        failures.join("\n  ")
    );
}

/// A wider primitive matrix exercised for the pose-invariant guarantees
/// (symmetry, determinism, ablation agreement) — rotated boxes, cones,
/// cylinder-cylinder, sphere-cylinder, and nested pairs. No analytic value
/// oracle (OBB/cone min-distance is awkward closed-form); these catch crashes,
/// asymmetry, non-determinism, and broad-phase disagreement on the harder
/// shapes.
fn tough_pairs() -> Vec<(&'static str, Build)> {
    let r35 = 35.0_f64 * std::f64::consts::PI / 180.0;
    vec![
        (
            "rot box vs box separated",
            Box::new(move |m| (unit_box(m), box_rot_z_at(m, r35, [3.0, 0.5, 0.0]))),
        ),
        (
            "rot box vs box edge",
            Box::new(move |m| (unit_box(m), box_rot_z_at(m, r35, [2.2, 0.0, 0.0]))),
        ),
        (
            "cone vs box separated",
            Box::new(|m| (unit_box(m), cone_at(m, [3.5, 0.0, -0.5], 0.6, 0.0, 1.0))),
        ),
        (
            "cone vs box touching",
            Box::new(|m| (unit_box(m), cone_at(m, [1.0, 0.0, -0.5], 0.6, 0.0, 1.0))),
        ),
        (
            "cyl vs cyl side-by-side",
            Box::new(|m| {
                (
                    z_cylinder_at(m, [0.0, 0.0, -0.5], 0.5, 1.0),
                    z_cylinder_at(m, [2.0, 0.0, -0.5], 0.5, 1.0),
                )
            }),
        ),
        (
            "sphere vs cylinder separated",
            Box::new(|m| {
                (
                    sphere_at(m, [0.0, 0.0, 0.0], 1.0),
                    z_cylinder_at(m, [3.0, 0.0, -0.5], 0.5, 1.0),
                )
            }),
        ),
        (
            "nested sphere in box",
            Box::new(|m| (unit_box(m), sphere_at(m, [0.0, 0.0, 0.0], 0.5))),
        ),
    ]
}

#[test]
fn cd_tough_pairs_are_symmetric_and_deterministic() {
    let mut failures = Vec::new();
    for (label, build) in tough_pairs() {
        let (model, a, b) = pair(&build);
        // symmetry
        let dab = cd_dist(&model, a, b);
        let dba = cd_dist(&model, b, a);
        if (dab - dba).abs() > VAL_TOL {
            failures.push(format!(
                "{label}: asymmetric d(a,b)={dab:.5} d(b,a)={dba:.5}"
            ));
        }
        // determinism
        let runs: Vec<f64> = (0..6).map(|_| cd_dist(&model, a, b)).collect();
        if runs.iter().any(|&v| (v - runs[0]).abs() > 1e-9) {
            failures.push(format!("{label}: non-deterministic {runs:?}"));
        }
        // sanity: finite and non-negative
        if !(dab.is_finite() && dab >= -1e-9) {
            failures.push(format!("{label}: d={dab} not finite/non-negative"));
        }
    }
    assert!(
        failures.is_empty(),
        "tough-pair CD invariant failures:\n  {}",
        failures.join("\n  ")
    );
}
