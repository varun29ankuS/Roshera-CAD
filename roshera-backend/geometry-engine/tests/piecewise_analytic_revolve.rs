// Reason: integration-test crate -- panicking (unwrap/expect/assert/index) is
// the test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::indexing_slicing)]

//! PIECEWISE-ANALYTIC REVOLVE (spec 2026-07-19) — Slice A gates.
//!
//! The analytic-band revolve already emits Cylinder / Cone / annular-Plane
//! bands per profile edge, but a circular-arc profile edge collapses to a
//! generic `SurfaceOfRevolution` — the exact quadric class (Torus off-axis,
//! Sphere on-axis) was never tagged. These gates pin the fundamental fix:
//!
//! 1. Face-type multiset: a mixed nozzle-style profile (vertical line,
//!    off-axis arc, sloped line, NURBS, caps) revolves 360° to EXACTLY one
//!    Cylinder + one Torus + one Cone + one SurfaceOfRevolution + Plane
//!    caps — one face per profile segment, never ×`segments` patches.
//! 2. Torus exactness: the emitted torus band satisfies the implicit torus
//!    equation of (projected arc center, ring distance R, arc radius ρ)
//!    computed INDEPENDENTLY from the profile arc — pins the parameters,
//!    not just the tag (swapping major/minor radii goes RED).
//! 3. Sphere bands: an on-axis arc → Sphere face (full ball for a
//!    pole-to-pole half-disc, spherical cap for a dome profile), with the
//!    analytic sphere volume.
//! 5. Full-circle lift: an off-axis full `Circle` profile edge revolves to
//!    a sound one-face ring torus; on-axis / spindle circles keep the
//!    honest typed refusal (no exact torus representation exists).

use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::extrude::{ProfileLoop, ProfileRegion};
use geometry_engine::operations::revolve::revolve_profile_regions;
use geometry_engine::primitives::surface::{SurfaceType, Torus};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::sketch2d::sketch_topology::ProfileEdge;
use std::f64::consts::{PI, TAU};

/// Face kinds of the (single-shell) solid.
fn face_kinds(m: &BRepModel, sid: u32) -> Vec<SurfaceType> {
    let solid = m.solids.get(sid).expect("solid");
    let shell = m.shells.get(solid.outer_shell).expect("shell");
    let mut out = Vec::new();
    for &fid in &shell.faces {
        let f = m.faces.get(fid).expect("face");
        let s = m.surfaces.get(f.surface_id).expect("surface");
        out.push(s.surface_type());
    }
    out
}

fn kind_count(k: &[SurfaceType], want: SurfaceType) -> usize {
    k.iter().filter(|&&x| x == want).count()
}

/// Revolve a single typed outer loop 360° about the v-axis (u = 0) of the
/// world XY plane — the same frame the sketch-DCM follow-ups gates use:
/// `u` is the radial coordinate, `v` the axial one, axis = +Y at u = 0.
fn revolve_edges(model: &mut BRepModel, edges: Vec<ProfileEdge>) -> Result<u32, String> {
    revolve_profile_regions(
        model,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::X,
        Vector3::Y,
        &[ProfileRegion {
            outer: ProfileLoop::Edges(edges),
            holes: Vec::new(),
        }],
        [0.0, 0.0],
        [0.0, 1.0],
        TAU,
        48,
        Tolerance::default(),
    )
    .map_err(|e| format!("{e:?}"))
}

/// The mixed nozzle-style profile (closed, CCW, axis at u = 0):
///
/// ```text
/// (0,0) ─line→ (5,0)            bottom cap      → Plane (disc)
/// (5,0) ─line→ (5,3)            chamber wall    → Cylinder
/// (5,3) ─arc→  (6,4)            throat blend    → Torus  [center (6,3), ρ=1, R=6]
/// (6,4) ─line→ (4,6)            converging cone → Cone
/// (4,6) ─nurbs→(2,7)            bell            → SurfaceOfRevolution
/// (2,7) ─line→ (0,7)            top cap         → Plane (disc)
/// (0,7) ─line→ (0,0)            axis segment    → no face
/// ```
fn nozzle_profile() -> Vec<ProfileEdge> {
    vec![
        ProfileEdge::Line {
            start: [0.0, 0.0],
            end: [5.0, 0.0],
        },
        ProfileEdge::Line {
            start: [5.0, 0.0],
            end: [5.0, 3.0],
        },
        // Quarter arc from (5,3) [angle π about (6,3)] to (6,4) [angle π/2],
        // traversed clockwise (π → π/2) so it bulges up-left (concave wall).
        ProfileEdge::Arc {
            center: [6.0, 3.0],
            radius: 1.0,
            start_angle: PI,
            end_angle: PI / 2.0,
            ccw: false,
        },
        ProfileEdge::Line {
            start: [6.0, 4.0],
            end: [4.0, 6.0],
        },
        // Genuinely curved cubic (non-circular, non-linear) — must STAY a
        // SurfaceOfRevolution band (upgrading its tag would be a lie).
        ProfileEdge::Nurbs {
            degree: 3,
            control_points: vec![[4.0, 6.0], [3.5, 6.8], [2.6, 6.2], [2.0, 7.0]],
            weights: None,
            knots: vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        },
        ProfileEdge::Line {
            start: [2.0, 7.0],
            end: [0.0, 7.0],
        },
        ProfileEdge::Line {
            start: [0.0, 7.0],
            end: [0.0, 0.0],
        },
    ]
}

/// GATE 1 — face-type multiset: each typed profile segment revolves to its
/// EXACT surface class, one face per segment (+ nothing for the axis
/// segment), and the solid is watertight. Fails pre-fix: the arc band is
/// tagged `SurfaceOfRevolution`, not `Torus`.
#[test]
fn nozzle_profile_revolves_to_exact_face_type_multiset() {
    let mut model = BRepModel::new();
    let solid = revolve_edges(&mut model, nozzle_profile()).expect("nozzle revolve");

    let k = face_kinds(&model, solid);
    assert_eq!(
        kind_count(&k, SurfaceType::Torus),
        1,
        "off-axis arc segment must revolve to exactly ONE exact Torus band, got {k:?}"
    );
    assert_eq!(
        kind_count(&k, SurfaceType::Cylinder),
        1,
        "vertical line → one Cylinder band, got {k:?}"
    );
    assert_eq!(
        kind_count(&k, SurfaceType::Cone),
        1,
        "sloped line → one Cone band, got {k:?}"
    );
    assert_eq!(
        kind_count(&k, SurfaceType::SurfaceOfRevolution),
        1,
        "NURBS segment stays exactly ONE SurfaceOfRevolution band \
         (a non-circular curve must never be tagged quadric), got {k:?}"
    );
    assert_eq!(
        kind_count(&k, SurfaceType::Plane),
        2,
        "two horizontal cap discs, got {k:?}"
    );
    // One face per non-axis profile segment — NOT multiplied by the 48
    // angular segments the grid path would emit.
    assert_eq!(
        k.len(),
        6,
        "6 profile segments + axis closure = 6 faces total, got {} ({k:?})",
        k.len()
    );

    let rep = manifold_report(&model, solid, 0.1, 1e-6).expect("tessellates");
    assert!(
        rep.boundary_edges == 0 && rep.closed && rep.manifold,
        "analytic multi-band revolve must be watertight: boundary={} closed={} manifold={}",
        rep.boundary_edges,
        rep.closed,
        rep.manifold
    );
}

/// GATE 2 — torus exactness: sample the emitted torus band across its
/// parameter domain; every point must satisfy the implicit torus equation
/// of (projected center, R, ρ) computed independently from the profile
/// arc: center (6,3), ρ = 1 ⇒ axis point y = 3, R = 6. Swapping
/// major/minor radii turns this RED (the mutation proof).
#[test]
fn torus_band_matches_analytic_torus_parameters() {
    let mut model = BRepModel::new();
    let solid = revolve_edges(&mut model, nozzle_profile()).expect("nozzle revolve");

    // Locate the torus face.
    let solid_ref = model.solids.get(solid).expect("solid");
    let shell = model.shells.get(solid_ref.outer_shell).expect("shell");
    let mut torus: Option<&Torus> = None;
    for &fid in &shell.faces {
        let f = model.faces.get(fid).expect("face");
        let s = model.surfaces.get(f.surface_id).expect("surface");
        if s.surface_type() == SurfaceType::Torus {
            torus = s.as_any().downcast_ref::<Torus>();
        }
    }
    let torus = torus.expect("nozzle solid must carry exactly one Torus band");

    // Independent analytic parameters from the PROFILE arc (never read back
    // from the surface): arc center [6,3] radius 1 about axis +Y at u=0.
    let big_r = 6.0;
    let rho = 1.0;
    let z0 = 3.0;

    assert!(
        (torus.major_radius - big_r).abs() < 1e-9,
        "major radius must be the arc center's axis distance {big_r}, got {}",
        torus.major_radius
    );
    assert!(
        (torus.minor_radius - rho).abs() < 1e-9,
        "minor radius must be the arc radius {rho}, got {}",
        torus.minor_radius
    );
    assert!(
        torus.axis.cross(&Vector3::Y).magnitude() < 1e-9,
        "torus axis must be the revolve axis (±Y), got {:?}",
        torus.axis
    );
    assert!(
        (torus.center - Point3::new(0.0, z0, 0.0)).magnitude() < 1e-9,
        "torus center must be the arc center projected onto the axis (0,{z0},0), got {:?}",
        torus.center
    );
    let limits = torus
        .param_limits
        .expect("quarter-arc band must carry v-parameter limits");
    assert!(
        ((limits[1] - limits[0]) - TAU).abs() < 1e-9,
        "full revolve ⇒ full u-range, got [{}, {}]",
        limits[0],
        limits[1]
    );
    assert!(
        ((limits[3] - limits[2]).abs() - PI / 2.0).abs() < 1e-9,
        "quarter arc ⇒ π/2 v-extent, got [{}, {}]",
        limits[2],
        limits[3]
    );

    // Grid-sample the band over its own domain; every surface point must lie
    // on the independent implicit torus (√(x²+z²) − R)² + (y − z0)² = ρ².
    use geometry_engine::primitives::surface::Surface;
    let n = 17;
    for i in 0..=n {
        for j in 0..=n {
            let u = limits[0] + (limits[1] - limits[0]) * (i as f64) / (n as f64);
            let v = limits[2] + (limits[3] - limits[2]) * (j as f64) / (n as f64);
            let p = torus
                .evaluate_full(u, v)
                .expect("torus band evaluates on its own domain")
                .position;
            let ring = (p.x * p.x + p.z * p.z).sqrt();
            let resid = ((ring - big_r).powi(2) + (p.y - z0).powi(2)).sqrt() - rho;
            assert!(
                resid.abs() < 1e-9,
                "torus band point (u={u:.4}, v={v:.4}) off the analytic torus by {resid:.3e} \
                 (point {p:?}; expected (√(x²+z²)−{big_r})²+(y−{z0})²={rho}²)"
            );
        }
    }
}

/// GATE 3a — on-axis arc, pole-to-pole (half-disc profile): the revolved
/// solid IS the ball; its lateral must be exactly ONE `Sphere` face with
/// the analytic volume 4πr³/3.
#[test]
fn on_axis_arc_half_disc_revolves_to_full_sphere() {
    let mut model = BRepModel::new();
    // Semicircle from the south pole (0,−2) through (2,0) to the north pole
    // (0,2) about the on-axis center (0,0), closed by the axis segment.
    let solid = revolve_edges(
        &mut model,
        vec![
            ProfileEdge::Arc {
                center: [0.0, 0.0],
                radius: 2.0,
                start_angle: -PI / 2.0,
                end_angle: PI / 2.0,
                ccw: true,
            },
            ProfileEdge::Line {
                start: [0.0, 2.0],
                end: [0.0, -2.0],
            },
        ],
    )
    .expect("half-disc revolve");

    let k = face_kinds(&model, solid);
    assert_eq!(
        k,
        vec![SurfaceType::Sphere],
        "pole-to-pole on-axis arc must revolve to exactly ONE exact Sphere face, got {k:?}"
    );

    let rep = manifold_report(&model, solid, 0.1, 1e-6).expect("tessellates");
    assert!(
        rep.boundary_edges == 0 && rep.closed && rep.manifold,
        "sphere solid must be watertight: boundary={} closed={} manifold={}",
        rep.boundary_edges,
        rep.closed,
        rep.manifold
    );

    let analytic = 4.0 / 3.0 * PI * 8.0;
    let v = model.calculate_solid_volume(solid).expect("volume");
    let rel = (v - analytic).abs() / analytic;
    assert!(
        rel < 1e-3,
        "ball volume: got {v:.9}, analytic {analytic:.9}, rel {rel:.3e}"
    );
}

/// GATE 3b — on-axis arc, ring-to-pole (dome profile): quarter arc from the
/// equator ring to the north pole → hemisphere = Plane disc + Sphere cap,
/// volume 2πr³/3.
#[test]
fn on_axis_arc_dome_revolves_to_sphere_cap() {
    let mut model = BRepModel::new();
    let solid = revolve_edges(
        &mut model,
        vec![
            ProfileEdge::Line {
                start: [0.0, 0.0],
                end: [3.0, 0.0],
            },
            // Quarter arc (3,0) → (0,3) about the on-axis center (0,0).
            ProfileEdge::Arc {
                center: [0.0, 0.0],
                radius: 3.0,
                start_angle: 0.0,
                end_angle: PI / 2.0,
                ccw: true,
            },
            ProfileEdge::Line {
                start: [0.0, 3.0],
                end: [0.0, 0.0],
            },
        ],
    )
    .expect("dome revolve");

    let k = face_kinds(&model, solid);
    assert_eq!(
        kind_count(&k, SurfaceType::Sphere),
        1,
        "on-axis arc → exactly one Sphere cap face, got {k:?}"
    );
    assert_eq!(
        kind_count(&k, SurfaceType::Plane),
        1,
        "equator disc cap, got {k:?}"
    );
    assert_eq!(k.len(), 2, "hemisphere = disc + spherical cap, got {k:?}");

    let rep = manifold_report(&model, solid, 0.1, 1e-6).expect("tessellates");
    assert!(
        rep.boundary_edges == 0 && rep.closed && rep.manifold,
        "hemisphere must be watertight: boundary={} closed={} manifold={}",
        rep.boundary_edges,
        rep.closed,
        rep.manifold
    );

    let analytic = 2.0 / 3.0 * PI * 27.0;
    let v = model.calculate_solid_volume(solid).expect("volume");
    let rel = (v - analytic).abs() / analytic;
    assert!(
        rel < 1e-3,
        "hemisphere volume: got {v:.9}, analytic {analytic:.9}, rel {rel:.3e}"
    );
}

/// GATE 5a — full-circle lift: an off-axis full `Circle` profile edge (ring
/// torus class, ρ < R) now revolves to a SOUND one-face ring `Torus`
/// lateral with the Pappus volume 2π·R·πρ². Fails pre-fix with the typed
/// full-circle refusal.
#[test]
fn off_axis_full_circle_revolves_to_ring_torus() {
    let mut model = BRepModel::new();
    let solid = revolve_edges(
        &mut model,
        vec![ProfileEdge::Circle {
            center: [6.0, 0.0],
            radius: 1.5,
        }],
    )
    .expect("off-axis circle must revolve to a ring torus");

    let k = face_kinds(&model, solid);
    assert_eq!(
        k,
        vec![SurfaceType::Torus],
        "ring-torus solid is exactly ONE full Torus face, got {k:?}"
    );

    let rep = manifold_report(&model, solid, 0.1, 1e-6).expect("tessellates");
    assert!(
        rep.boundary_edges == 0 && rep.closed && rep.manifold,
        "ring torus must be watertight: boundary={} closed={} manifold={}",
        rep.boundary_edges,
        rep.closed,
        rep.manifold
    );

    // Pappus: V = 2πR · πρ².
    let analytic = 2.0 * PI * 6.0 * PI * 1.5 * 1.5;
    let v = model.calculate_solid_volume(solid).expect("volume");
    let rel = (v - analytic).abs() / analytic;
    assert!(
        rel < 1e-3,
        "torus volume: got {v:.9}, analytic {analytic:.9}, rel {rel:.3e}"
    );
}

/// GATE 5b — the honest refusal STAYS for circles with no exact torus
/// representation: on-axis (spindle) and axis-crossing circles.
#[test]
fn on_axis_and_axis_crossing_circles_still_refused() {
    for center in [[0.0, 0.0], [1.0, 0.0]] {
        let mut model = BRepModel::new();
        let err = revolve_edges(
            &mut model,
            vec![ProfileEdge::Circle {
                center,
                radius: 1.5,
            }],
        )
        .expect_err("on-axis / axis-crossing circle has no exact torus representation");
        let msg = err.to_lowercase();
        assert!(
            msg.contains("circle") && msg.contains("torus"),
            "refusal must name the circle→torus limitation honestly, got: {msg}"
        );
    }
}
