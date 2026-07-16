// Reason: integration-test crate -- panicking (unwrap/expect/assert/index) is
// the test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::indexing_slicing)]

//! SKETCH-DCM #45 — Wave B follow-ups: the Slice 5/7 analytic-profile
//! residual burndown.
//!
//! Item 2 — the closed-ruled zero-triangle tessellation trap is FIXED
//! at the topology root: a closed single-edge NURBS profile is
//! seam-split into two open halves (each an exactly-swept open ruled
//! wall), so a smooth closed blob extrudes to a ground-truth-SOUND
//! solid and the Slice-5/7 typed refusal is retired.
//! Item 3 — coaxial equal-radius arc-railed extrude walls collapse to
//! TRUE trimmed `Cylinder` faces (seam-aligned ref_dir, angle_limits =
//! the arc's own span) so STEP maps them as `CYLINDRICAL_SURFACE` and
//! every cylinder-hardened downstream path engages.
//! Item 1 — ellipse profiles lift to EXACT rational-quadratic NURBS
//! (the affine image of the unit circle, Piegl & Tiller §7.5) instead
//! of 64-gon chord sampling.
//! Item 4 — a circular profile under an OBLIQUE extrude direction is
//! seam-split into two half-circle arcs whose walls are exactly-swept
//! ruled surfaces (rails = true circles displaced by the oblique
//! direction — together they ARE the elliptic-cylinder lateral), so
//! the kernel refusal is retired.

use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::extrude::{extrude_profile_regions, ProfileLoop, ProfileRegion};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::sketch2d::sketch_topology::{
    AnalyticLoop, ProfileEdge, ProfileExtractor, SketchTopology,
};
use geometry_engine::sketch2d::{Point2d, Sketch, SketchAnchor, Tolerance2d};

fn fresh(name: &str) -> Sketch {
    Sketch::new(name.to_string(), SketchAnchor::xy())
}

/// The Slice-7 closed-blob fixture: a smooth closed clamped cubic
/// (last CP == first CP). Same geometry as the retired refusal test —
/// the pin's fixture survives, its verdict flips from refusal to
/// soundness (Slice-6/7 test-flip precedent).
fn closed_blob_sketch() -> Sketch {
    let s = fresh("followups_b_closed_blob");
    let p0 = Point2d::new(10.0, 0.0);
    s.add_bspline(
        3,
        vec![
            p0,
            Point2d::new(14.0, 9.0),
            Point2d::new(-2.0, 12.0),
            Point2d::new(-8.0, 2.0),
            Point2d::new(2.0, -7.0),
            p0,
        ],
        vec![0.0, 0.0, 0.0, 0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0, 1.0, 1.0, 1.0],
    )
    .expect("closed spline");
    s
}

/// Extract the single analytic outer loop of a one-region sketch.
fn analytic_outer(s: &Sketch) -> Vec<ProfileEdge> {
    let topo = SketchTopology::analyze(s, &Tolerance2d::default()).expect("topology");
    let profiles = ProfileExtractor::extract_for_extrusion(&topo).expect("profiles");
    assert_eq!(profiles.len(), 1, "one closed region expected");
    match ProfileExtractor::analytic_loop_edges(s, &topo, &profiles[0].outer_boundary)
        .expect("extraction")
    {
        AnalyticLoop::Edges(edges) => edges,
        other => panic!("loop must lift analytically, got {other:?}"),
    }
}

/// Green's-theorem area of a closed 2D boundary sampled densely from
/// an evaluator — independent of the kernel's tessellation.
fn boundary_area(samples: &[Point2d]) -> f64 {
    let mut acc = 0.0;
    for i in 0..samples.len() {
        let p = &samples[i];
        let q = &samples[(i + 1) % samples.len()];
        acc += p.x * q.y - q.x * p.y;
    }
    (acc / 2.0).abs()
}

// ── Item 2: closed single-edge NURBS profiles extrude SOUND ─────────

const BLOB_H: f64 = 5.0;

/// GATE (item 2): the closed single-spline blob extrudes to a
/// ground-truth-SOUND solid with mesh-oracle volume agreement.
///
/// Pre-fix signature (RED, run on 309d504): `extrude_profile_regions`
/// refused with "closed NURBS profile edge — the extruded wall would
/// be a CLOSED generic ruled surface, the documented zero-triangle
/// tessellation trap". The fix seam-splits the closed edge into two
/// open NURBS halves at the kernel boundary, so the lateral is two
/// OPEN exactly-swept ruled walls — the trap's precondition (a closed
/// boundary loop on a `RuledSurface`, which advertises
/// `is_closed_u == false` and defeats the seam unwrap) never forms.
#[test]
fn gate_closed_single_spline_profile_extrudes_sound() {
    let s = closed_blob_sketch();
    let outer = analytic_outer(&s);
    assert_eq!(outer.len(), 1, "single closed NURBS edge lifts typed");
    assert!(matches!(outer[0], ProfileEdge::Nurbs { .. }));

    let mut model = BRepModel::new();
    let solid = extrude_profile_regions(
        &mut model,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::X,
        Vector3::Y,
        &[ProfileRegion {
            outer: ProfileLoop::Edges(outer),
            holes: Vec::new(),
        }],
        BLOB_H,
        None,
        Tolerance::default(),
    )
    .expect("a smooth closed blob must extrude — the closed-ruled trap is fixed at the root");

    let gt = model.ground_truth(solid).expect("ground truth");
    assert!(
        gt.certificate.is_sound(),
        "the closed-blob solid must be SOUND: {:?}",
        gt.certificate
    );

    // Seam-split census: 2 caps + 2 open NURBS ruled walls.
    let face_count = model
        .solid_outer_face_count(solid)
        .expect("outer face count");
    assert_eq!(face_count, 4, "2 caps + 2 seam-split NURBS walls");

    // Volume oracle: Green's-theorem area of the SKETCH spline
    // boundary (dense, kernel-independent) × height.
    let spline_geo = s
        .splines()
        .iter()
        .next()
        .expect("spline present")
        .value()
        .spline
        .clone();
    let mut boundary: Vec<Point2d> = Vec::with_capacity(4000);
    for i in 0..4000 {
        let u = i as f64 / 4000.0;
        boundary.push(spline_geo.evaluate(u).expect("eval"));
    }
    let expected_volume = boundary_area(&boundary) * BLOB_H;
    let measured = model.calculate_solid_volume(solid).expect("solid volume");
    let rel = (measured - expected_volume).abs() / expected_volume;
    assert!(
        rel < 2e-3,
        "extruded volume must match the boundary oracle: measured {measured}, \
         expected {expected_volume}, rel {rel}"
    );
}

/// The split halves carry the EXACT curve: every sampled rail point of
/// both NURBS walls lies on the sketch spline (no chord fit, no
/// resampling). Kills a mutation that swaps the split for sampling.
#[test]
fn closed_spline_split_walls_carry_exact_nurbs_rails() {
    use geometry_engine::primitives::surface::RuledSurface;

    let s = closed_blob_sketch();
    let outer = analytic_outer(&s);
    let mut model = BRepModel::new();
    let solid = extrude_profile_regions(
        &mut model,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::X,
        Vector3::Y,
        &[ProfileRegion {
            outer: ProfileLoop::Edges(outer),
            holes: Vec::new(),
        }],
        BLOB_H,
        None,
        Tolerance::default(),
    )
    .expect("closed blob extrudes");

    let spline_geo = s
        .splines()
        .iter()
        .next()
        .expect("spline present")
        .value()
        .spline
        .clone();
    // Dense reference sampling of the sketch curve.
    let reference: Vec<Point2d> = (0..=8192)
        .map(|i| spline_geo.evaluate(i as f64 / 8192.0).expect("eval"))
        .collect();
    let closest_ref = |p: Point3| -> f64 {
        reference
            .iter()
            .map(|q| ((q.x - p.x).powi(2) + (q.y - p.y).powi(2) + p.z * p.z).sqrt())
            .fold(f64::INFINITY, f64::min)
    };

    let solid_ref = model.solids.get(solid).expect("solid");
    let shell = model.shells.get(solid_ref.outer_shell).expect("shell");
    let mut ruled_walls = 0usize;
    for &face_id in &shell.faces {
        let face = model.faces.get(face_id).expect("face");
        let surface = model.surfaces.get(face.surface_id).expect("surface");
        if let Some(ruled) = surface.as_any().downcast_ref::<RuledSurface>() {
            ruled_walls += 1;
            // The split halves must be RE-NORMALISED to a [0, 1] knot
            // domain: `RuledSurface` feeds u ∈ [0, 1] RAW to its
            // rails, so a sub-domain rail (e.g. [0.5, 1]) clamps half
            // the parameter square into a degenerate plateau — the
            // documented polyline-subcurve hazard that broke
            // `is_planar` / SSI downstream. Pin the domain AND the
            // absence of a plateau structurally.
            let rail_range = ruled.curve1.parameter_range();
            assert!(
                rail_range.start.abs() < 1e-12 && (rail_range.end - 1.0).abs() < 1e-12,
                "split rail must be re-normalised to [0, 1], got [{}, {}]",
                rail_range.start,
                rail_range.end
            );
            let p_lo = ruled.curve1.point_at(0.0).expect("rail start");
            let p_q = ruled.curve1.point_at(0.25).expect("rail quarter");
            assert!(
                (p_q - p_lo).magnitude() > 1e-3,
                "rail must move over [0, 0.25] — a clamped plateau means the \
                 half kept its sub-domain parameterisation"
            );
            for k in 0..=32 {
                let u = k as f64 / 32.0;
                let p = ruled.curve1.point_at(u).expect("rail point");
                // Bottom rail lies in the sketch plane (z = 0) ON the
                // spline. 8192 reference samples bound the chord gap
                // well under 1e-4; an exact rail sits inside it.
                let d = closest_ref(p);
                assert!(
                    d < 1e-4,
                    "wall rail point {p:?} must lie on the sketch spline (dist {d})"
                );
            }
        }
    }
    assert_eq!(ruled_walls, 2, "two seam-split NURBS ruled walls");
}

/// A closed-spline HOLE inside a rectangle: the split applies to inner
/// loops identically — region area = rect − blob, solid SOUND.
#[test]
fn closed_spline_hole_extrudes_sound() {
    let s = closed_blob_sketch();
    s.add_polyline(
        vec![
            Point2d::new(-20.0, -15.0),
            Point2d::new(25.0, -15.0),
            Point2d::new(25.0, 20.0),
            Point2d::new(-20.0, 20.0),
        ],
        true,
    )
    .expect("outer rectangle");

    let topo = SketchTopology::analyze(&s, &Tolerance2d::default()).expect("topology");
    let profiles = ProfileExtractor::extract_for_extrusion(&topo).expect("profiles");
    assert_eq!(profiles.len(), 1, "blob nests as a hole");
    assert_eq!(profiles[0].holes.len(), 1);

    let outer = match ProfileExtractor::analytic_loop_edges(&s, &topo, &profiles[0].outer_boundary)
        .expect("extraction")
    {
        AnalyticLoop::Edges(edges) => edges,
        other => panic!("rectangle loop lifts analytically: {other:?}"),
    };
    let hole = match ProfileExtractor::analytic_loop_edges(&s, &topo, &profiles[0].holes[0])
        .expect("extraction")
    {
        AnalyticLoop::Edges(edges) => edges,
        other => panic!("blob hole lifts analytically: {other:?}"),
    };

    let mut model = BRepModel::new();
    let solid = extrude_profile_regions(
        &mut model,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::X,
        Vector3::Y,
        &[ProfileRegion {
            outer: ProfileLoop::Edges(outer),
            holes: vec![ProfileLoop::Edges(hole)],
        }],
        BLOB_H,
        None,
        Tolerance::default(),
    )
    .expect("rect-with-blob-hole must extrude");

    let gt = model.ground_truth(solid).expect("ground truth");
    assert!(
        gt.certificate.is_sound(),
        "rect-with-blob-hole must be SOUND: {:?}",
        gt.certificate
    );

    let spline_geo = s
        .splines()
        .iter()
        .next()
        .expect("spline present")
        .value()
        .spline
        .clone();
    let blob: Vec<Point2d> = (0..4000)
        .map(|i| spline_geo.evaluate(i as f64 / 4000.0).expect("eval"))
        .collect();
    let expected_volume = (45.0 * 35.0 - boundary_area(&blob)) * BLOB_H;
    let measured = model.calculate_solid_volume(solid).expect("solid volume");
    let rel = (measured - expected_volume).abs() / expected_volume;
    assert!(
        rel < 2e-3,
        "hole volume subtracts: measured {measured}, expected {expected_volume}, rel {rel}"
    );
}
