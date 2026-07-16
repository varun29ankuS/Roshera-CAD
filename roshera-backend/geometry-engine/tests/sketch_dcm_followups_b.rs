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
use geometry_engine::primitives::face::FaceOrientation;
use geometry_engine::primitives::surface::Cylinder;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::sketch2d::sketch_topology::{
    AnalyticLoop, ProfileEdge, ProfileExtractor, SketchTopology,
};
use geometry_engine::sketch2d::{Point2d, Sketch, SketchAnchor, Tolerance2d};
use std::f64::consts::PI;

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

// ── Item 3: partial-arc walls → TRUE trimmed Cylinder faces ──────────

const SLOT_L: f64 = 10.0; // arc centers at x = ±SLOT_L
const SLOT_R: f64 = 5.0;
const SLOT_H: f64 = 8.0;

/// Stadium/slot profile (the Slice-5 fixture): two horizontal lines
/// y = ±r for x ∈ [−L, L] plus two semicircular end arcs of radius r
/// centred at (±L, 0).
fn slot_sketch() -> Sketch {
    let sketch = fresh("followups_b_slot");
    let bl = sketch.add_point(Point2d::new(-SLOT_L, -SLOT_R));
    let br = sketch.add_point(Point2d::new(SLOT_L, -SLOT_R));
    let tr = sketch.add_point(Point2d::new(SLOT_L, SLOT_R));
    let tl = sketch.add_point(Point2d::new(-SLOT_L, SLOT_R));
    sketch.add_line(bl, br).expect("bottom line");
    sketch.add_line(tr, tl).expect("top line");
    sketch
        .add_arc_center_angles(Point2d::new(SLOT_L, 0.0), SLOT_R, -PI / 2.0, PI / 2.0)
        .expect("right arc");
    sketch
        .add_arc_center_angles(Point2d::new(-SLOT_L, 0.0), SLOT_R, PI / 2.0, 3.0 * PI / 2.0)
        .expect("left arc");
    sketch
}

fn extruded_slot() -> (BRepModel, u32) {
    let s = slot_sketch();
    let topo = SketchTopology::analyze(&s, &Tolerance2d::default()).expect("topology");
    let profiles = ProfileExtractor::extract_for_extrusion(&topo).expect("profiles");
    assert_eq!(profiles.len(), 1);
    let outer = match ProfileExtractor::analytic_loop_edges(&s, &topo, &profiles[0].outer_boundary)
        .expect("extraction")
    {
        AnalyticLoop::Edges(edges) => edges,
        other => panic!("slot loop lifts analytically: {other:?}"),
    };
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
        SLOT_H,
        None,
        Tolerance::default(),
    )
    .expect("slot extrude");
    (model, solid)
}

/// GATE (item 3): the slot's two semicircular end-cap walls are TRUE
/// trimmed `Cylinder` faces — typed carrier, exact radius, extrusion
/// axis, seam-aligned `ref_dir` with `angle_limits` = the arc's own
/// span (so the face never straddles the carrier's parameterisation
/// seam: the EXTRUDE-CYL-MESH-INVERTED trap class).
///
/// Pre-fix (RED, run on f97120a): the arc walls were exactly-swept
/// generic `RuledSurface`s (Slice-5 residual 2) — 0 Cylinder faces.
#[test]
fn gate_slot_arc_walls_are_trimmed_cylinder_faces() {
    let (mut model, solid) = extruded_slot();

    let solid_ref = model.solids.get(solid).expect("solid").clone();
    let shell = model.shells.get(solid_ref.outer_shell).expect("shell");
    let mut cylinder_walls = 0usize;
    for &fid in &shell.faces {
        let face = model.faces.get(fid).expect("face");
        let surface = model.surfaces.get(face.surface_id).expect("surface");
        if let Some(cyl) = surface.as_any().downcast_ref::<Cylinder>() {
            cylinder_walls += 1;
            assert!(
                (cyl.radius - SLOT_R).abs() < 1e-9,
                "cylinder wall radius must be exact: {}",
                cyl.radius
            );
            assert!(
                cyl.axis.cross(&Vector3::Z).magnitude() < 1e-9,
                "cylinder axis must be the extrusion direction: {:?}",
                cyl.axis
            );
            // Trim = the arc's own span, anchored at u = 0 (seam-
            // aligned ref_dir): the face NEVER straddles the carrier
            // seam and the parametric midpoint lies ON the face.
            let limits = cyl
                .angle_limits
                .expect("partial-arc wall must carry angle_limits");
            assert!(
                limits[0].abs() < 1e-12 && (limits[1] - PI).abs() < 1e-9,
                "angle_limits must be [0, π] (the semicircle span), got {limits:?}"
            );
            let height = cyl
                .height_limits
                .expect("extrude wall must carry height_limits");
            assert!(
                (height[1] - height[0] - SLOT_H).abs() < 1e-9,
                "height span must equal the extrusion distance, got {height:?}"
            );
            // Seam anchor: the u = 0 rim point is the arc's angular-
            // minimum endpoint, which for the slot's end caps sits at
            // (±L, ∓r) / (±L, ±r) — |x| = L, |y| = r exactly (the
            // axis is Z, so x/y are height-independent).
            let seam = cyl.origin + cyl.ref_dir * cyl.radius;
            assert!(
                (seam.x.abs() - SLOT_L).abs() < 1e-9 && (seam.y.abs() - SLOT_R).abs() < 1e-9,
                "seam (u=0) must anchor at the arc's own endpoint, got {seam:?}"
            );
        }
    }
    assert_eq!(cylinder_walls, 2, "both end-cap walls are typed Cylinders");

    let gt = model.ground_truth(solid).expect("ground truth");
    assert!(
        gt.certificate.is_sound(),
        "slot with cylinder walls must be SOUND: {}",
        gt.summary()
    );

    // Volume: the cylinder-hardened tessellation path is denser than
    // the generic ruled path (Slice-5 measured 1.39e-4 there); the
    // analytic value is (2L·2r + πr²)·h.
    let analytic = (2.0 * SLOT_L * 2.0 * SLOT_R + PI * SLOT_R * SLOT_R) * SLOT_H;
    let v = model.calculate_solid_volume(solid).expect("volume");
    let rel = (v - analytic).abs() / analytic;
    assert!(
        rel < 2e-4,
        "slot volume: got {v:.9}, analytic {analytic:.9}, rel {rel:.3e}"
    );
}

/// Normals/orientation gate (item 3): the EXTRUDE-CYL-MESH-INVERTED
/// trap manifests as a wall oriented INTO the material. Assert
/// analytically (oriented outward normal at the surface's parametric
/// midpoint points AWAY from the arc's center axis) AND via the mesh
/// certificate (oriented, zero inconsistent facets, analytic normal
/// agreement).
#[test]
fn slot_cylinder_walls_oriented_outward_no_seam_inversion() {
    let (mut model, solid) = extruded_slot();

    let solid_ref = model.solids.get(solid).expect("solid").clone();
    let shell = model.shells.get(solid_ref.outer_shell).expect("shell");
    let mut checked = 0usize;
    for &fid in &shell.faces {
        let face = model.faces.get(fid).expect("face");
        let surface = model.surfaces.get(face.surface_id).expect("surface");
        let Some(cyl) = surface.as_any().downcast_ref::<Cylinder>() else {
            continue;
        };
        checked += 1;
        let ((u0, u1), (v0, v1)) = surface.parameter_bounds();
        let (u_mid, v_mid) = (0.5 * (u0 + u1), 0.5 * (v0 + v1));
        let sp = surface.point_at(u_mid, v_mid).expect("midpoint");
        let n = surface.normal_at(u_mid, v_mid).expect("midpoint normal");
        let sign = match face.orientation {
            FaceOrientation::Forward => 1.0,
            FaceOrientation::Backward => -1.0,
        };
        let oriented = n * sign;
        // Outward for a convex end cap = radially away from the arc's
        // own axis (the slot material is on the axis side).
        let axis_foot = cyl.origin + cyl.axis * (sp - cyl.origin).dot(&cyl.axis);
        let radial = sp - axis_foot;
        assert!(
            oriented.dot(&radial) > 0.0,
            "cylinder wall {fid} oriented INTO the material (EXTRUDE-CYL-MESH-INVERTED): \
             oriented {oriented:?}, radial {radial:?}"
        );
    }
    assert_eq!(checked, 2, "two cylinder walls checked");

    let gt = model.ground_truth(solid).expect("ground truth");
    assert!(gt.certificate.oriented, "mesh must be orientable");
    assert_eq!(
        gt.certificate.inconsistent_directed_edges, 0,
        "no inversion seams in the mesh"
    );
    assert!(
        gt.certificate.tessellation.analytic_normal_agreement > 0.999,
        "facet normals must agree with the analytic carrier: {}",
        gt.certificate.tessellation.analytic_normal_agreement
    );
}
