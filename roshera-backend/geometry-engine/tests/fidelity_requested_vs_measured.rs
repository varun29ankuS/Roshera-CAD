// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! FIDELITY — the kernel's answer to "is the geometry you asked for the
//! geometry you got?", measured against the ops the API exposes.
//!
//! Soundness measures TOPOLOGY. The loft octagon shipped CERTIFIED SOUND at a
//! 9.97% volume shortfall (`capability_probe_loft_sweep_pattern.rs`) because
//! nothing in the kernel compared the RESULT to the REQUEST. These cases pin
//! that comparison:
//!
//!   1. CALIBRATION — an analytic cylinder is exactly what was asked for, so
//!      the statistic must read ~0. A non-trivial number here would mean the
//!      statistic is wrong, not the kernel.
//!   2. GREEN — a well-sampled circular loft ring stays inside the band.
//!   3. RED-CLASS — a deliberately-degraded (coarse) ring is skinned into a
//!      cross-section that is NOT the ring that was asked for, and the
//!      certificate still says SOUND. `fidelity_ok` is `Some(false)` and carries
//!      the number; `is_sound` stays `true`, because the topology genuinely is.
//!   4. NO VERDICT WITHOUT A MEASUREMENT — `fidelity_ok` is `None`, never a
//!      green `true`, when nothing could be measured.
//!
//! The comparison is SIGNED. Negative means the kernel built less than was
//! asked for (the octagon class); positive means it built more (a smooth
//! interpolation running outside a coarse request). The cases assert the sign,
//! not just the magnitude, so the two cannot silently swap.
//!
//! Every measured number is printed via `eprintln!` so a re-run documents the
//! then-current value without editing a comment.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::nurbs_loft::{nurbs_loft, NurbsLoftOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::queries::fidelity::{
    cylinder_fidelity, loft_fidelity, planar_faces_in_plane, ring_plane, DEFAULT_FIDELITY_TOLERANCE,
};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams, TriangleMesh};

fn mesh_of(model: &BRepModel, solid_id: SolidId) -> TriangleMesh {
    let solid = model.solids.get(solid_id).expect("solid exists");
    tessellate_solid(solid, model, &TessellationParams::default())
}

/// A ring of `n` points sampled on a circle of `radius` at height `z`.
fn circle_ring(n: usize, radius: f64, z: f64) -> Vec<Point3> {
    (0..n)
        .map(|i| {
            let t = 2.0 * std::f64::consts::PI * (i as f64) / (n as f64);
            Point3::new(radius * t.cos(), radius * t.sin(), z)
        })
        .collect()
}

fn lofted(sections: Vec<Vec<Point3>>) -> (BRepModel, SolidId) {
    let mut model = BRepModel::new();
    let sid = nurbs_loft(&mut model, sections, NurbsLoftOptions::default())
        .expect("nurbs_loft of equal-length planar-capped rings must succeed");
    (model, sid)
}

// ───────────────────────────────────────────────────────────────────────────
// 1. CALIBRATION — the analytic primitive must measure exact.
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn cylinder_fidelity_is_exact_the_calibration_case() {
    let mut model = BRepModel::new();
    let (radius, height) = (7.0_f64, 26.0_f64);
    let axis = Vector3::new(0.0, 0.0, 1.0);
    let base = Point3::new(0.0, 0.0, 0.0);
    let sid = match TopologyBuilder::new(&mut model).create_cylinder_3d(base, axis, radius, height)
    {
        Ok(GeometryId::Solid(id)) => id,
        other => panic!("expected a solid; got {other:?}"),
    };

    let mesh = mesh_of(&model, sid);
    let report = cylinder_fidelity(&mesh, base, axis, radius, height);

    for q in &report.quantities {
        eprintln!(
            "[fidelity cylinder] {} requested={:.9} measured={:.9} deviation={:.3e}",
            q.name, q.requested, q.measured, q.relative_deviation
        );
    }

    assert_eq!(
        report.quantities.len(),
        2,
        "radius and height are measurable"
    );
    assert!(
        report.gaps.is_empty(),
        "nothing should be unmeasurable here"
    );
    assert_eq!(
        report.fidelity_ok(),
        Some(true),
        "an analytic cylinder IS what was asked for"
    );

    // The calibration bite: not merely "inside the 2% band" but EXACT. A mean
    // statistic would land ~1e-3 here; an extremal one on tessellation vertices
    // that lie on the surface lands at float noise.
    let worst = report.worst().expect("a worst quantity exists");
    assert!(
        worst.relative_deviation < 1e-9,
        "the calibration case must be exact, not merely inside the band: \
         worst = {} at {:.3e}",
        worst.name,
        worst.relative_deviation
    );
}

// ───────────────────────────────────────────────────────────────────────────
// 2. GREEN — a well-sampled loft ring is honoured.
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn well_sampled_loft_rings_stay_inside_the_fidelity_band() {
    let sections = vec![circle_ring(64, 7.0, 0.0), circle_ring(64, 4.5, 26.0)];
    let requested = sections.clone();
    let (model, sid) = lofted(sections);
    let mesh = mesh_of(&model, sid);

    let report = loft_fidelity(&model, &mesh, sid, &requested, 1e-6);
    for q in &report.quantities {
        eprintln!(
            "[fidelity loft 64-pt] {} requested={:.6} measured={:.6} deviation={:.4}%",
            q.name,
            q.requested,
            q.measured,
            q.relative_deviation * 100.0
        );
    }

    assert_eq!(report.quantities.len(), 2, "both end caps are measurable");
    assert_eq!(
        report.fidelity_ok(),
        Some(true),
        "a 64-point ring is skinned into essentially the ring that was asked for; \
         worst = {:?}",
        report.worst()
    );
}

// ───────────────────────────────────────────────────────────────────────────
// 3. THE RED CLASS — sound, and NOT what was asked for.
// ───────────────────────────────────────────────────────────────────────────

/// A deliberately-degraded ring density: eight points, the exact density the
/// historical loft defect collapsed every circular profile to.
///
/// The caller asks for an OCTAGONAL section. `nurbs_loft` closes each ring with
/// a PERIODIC CUBIC in U, which interpolates the eight points and rounds
/// everything between them — so the cross-section actually built is a circle-
/// like curve enclosing materially more area than the octagon that was
/// requested. The solid is genuinely closed, genuinely manifold, and certifies
/// SOUND. Only the fidelity comparison can see it.
///
/// READ THE SIGN, AND DO NOT OVERCLAIM. This is NOT the octagon defect: it is
/// the same MECHANISM (requested cross-section vs built cross-section, compared
/// by area) firing in the opposite direction. The octagon was a kernel defect —
/// a circle was requested and a polygon built, measured BELOW requested. Here
/// the kernel does exactly what `nurbs_loft` documents and the REQUEST is the
/// coarse side, so the deviation is measured ABOVE requested. Both are real
/// fidelity facts, both are invisible to soundness, and
/// `signed_relative_deviation` is what tells them apart — this case asserts the
/// sign so the distinction cannot quietly rot.
///
/// Inverse tripwire: if `nurbs_loft` ever STOPPED interpolating and built
/// literal 8-gons, requested would equal measured and this case would go green.
/// That would be a genuine behaviour change worth the red it causes here.
#[test]
fn coarse_loft_rings_are_sound_but_not_the_geometry_that_was_asked_for() {
    let sections = vec![circle_ring(8, 7.0, 0.0), circle_ring(8, 4.5, 26.0)];
    let requested = sections.clone();
    let (mut model, sid) = lofted(sections);

    let cert = model.certify_solid(sid);
    let sound = cert.is_sound();
    let mesh = mesh_of(&model, sid);

    let report = loft_fidelity(&model, &mesh, sid, &requested, 1e-6);
    for q in &report.quantities {
        eprintln!(
            "[fidelity loft 8-pt] {} requested={:.6} measured={:.6} deviation={:.4}% \
             cert_sound={sound}",
            q.name,
            q.requested,
            q.measured,
            q.relative_deviation * 100.0
        );
    }

    assert!(
        sound,
        "the point of this case is that the SOUNDNESS certificate is clean: {cert:?}"
    );
    assert_eq!(report.quantities.len(), 2, "both end caps are measurable");
    assert_eq!(
        report.fidelity_ok(),
        Some(false),
        "an 8-point ring skinned by a periodic cubic is NOT the section that was \
         requested; worst = {:?}",
        report.worst()
    );

    let worst = report.worst().expect("a worst quantity exists");
    assert!(
        worst.relative_deviation > DEFAULT_FIDELITY_TOLERANCE,
        "the deviation must actually exceed the band, not merely be reported: {:.4}%",
        worst.relative_deviation * 100.0
    );
    assert!(
        worst.signed_relative_deviation > 0.0,
        "and the SIGN must say which direction: this is interpolation running \
         OUTSIDE a coarse request (positive), not the octagon defect's \
         built-smaller-than-asked (negative). Measured {:+.4}%",
        worst.signed_relative_deviation * 100.0
    );
}

/// PINS THE MEASUREMENT CHOICE. The cap face IS found by plane coincidence —
/// the loft measurement does not fail for want of a face — but the face's own
/// trimmed area integral (`Face::area`, the path `queries::measure` uses for
/// every ordinary planar face) does not produce a usable area for a cap whose
/// outer loop is ONE closed periodic-NURBS edge. That is why
/// `mesh_cross_section_area` reads the tessellation instead, and this case is
/// the evidence rather than an assertion in a comment. If the integral ever
/// learns this loop shape, this test goes red and the cheaper, exact path
/// becomes available.
#[test]
fn loft_cap_faces_are_found_but_the_trimmed_integral_is_not_the_path() {
    let sections = vec![circle_ring(48, 6.0, 0.0), circle_ring(48, 6.0, 10.0)];
    let requested = sections.clone();
    let (mut model, sid) = lofted(sections);

    let bottom = ring_plane(requested.first().expect("bottom ring")).expect("bottom ring plane");
    let faces = planar_faces_in_plane(&model, sid, bottom.centroid, bottom.normal, 1e-6);
    eprintln!("[fidelity loft caps] plane-coincident faces at the bottom ring: {faces:?}");
    assert_eq!(
        faces.len(),
        1,
        "exactly one planar cap face lies in the bottom section's plane"
    );

    let face_id = *faces.first().expect("one cap face");
    let tol = model
        .faces
        .get(face_id)
        .map(|f| geometry_engine::math::Tolerance::from_distance(f.tolerance))
        .expect("cap face tolerance");
    let mut face_clone = model.faces.get(face_id).expect("cap face").clone();
    let integral = face_clone.area(
        &mut model.loops,
        &model.vertices,
        &model.edges,
        &model.curves,
        &model.surfaces,
        tol,
    );
    let expected = std::f64::consts::PI * 36.0;
    eprintln!(
        "[fidelity loft caps] Face::area on the cap = {integral:?} (a circle of r=6 encloses \
         {expected:.4})"
    );
    let usable = integral
        .as_ref()
        .ok()
        .is_some_and(|a| a.is_finite() && (a - expected).abs() / expected <= 0.02);
    assert!(
        !usable,
        "Face::area now measures a single-closed-NURBS-edge cap loop correctly \
         ({integral:?}) — switch mesh_cross_section_area back to the exact trimmed \
         integral and delete this case"
    );
}

/// A gap is never a zero: a two-point-per-ring request cannot be lofted at all,
/// and a request whose sections are absent reports a stated reason rather than
/// a fabricated `requested: 0 / measured: 0` pair.
#[test]
fn an_unmeasurable_loft_reports_a_gap_never_a_zero() {
    let sections = vec![circle_ring(48, 5.0, 0.0), circle_ring(48, 5.0, 12.0)];
    let (model, sid) = lofted(sections);
    let mesh = mesh_of(&model, sid);

    // One section only — the request no longer describes a loft, so there is
    // nothing to compare.
    let report = loft_fidelity(&model, &mesh, sid, &[circle_ring(48, 5.0, 0.0)], 1e-6);
    assert!(
        report.quantities.is_empty(),
        "a gap must never mint a quantity"
    );
    assert_eq!(report.gaps.len(), 1);
    assert!(
        report.gaps.first().is_some_and(|g| !g.reason.is_empty()),
        "every gap states its reason"
    );
    assert_eq!(
        report.fidelity_ok(),
        None,
        "nothing measured yields NO verdict — a green boolean over an unmeasured          quantity is exactly the silent pass this module exists to remove"
    );
}
