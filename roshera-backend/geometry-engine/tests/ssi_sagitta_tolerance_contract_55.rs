// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Task #55 — the canonical SSI marcher must honour the caller's DISTANCE
//! tolerance in the polyline it emits.
//!
//! `trace_direction` documents a two-part step law whose accuracy arm is a
//! sagitta bound: for a chord `c` on a curve of local radius `R` the mid-chord
//! deviation is `s = c²/(8R)`, so `s ≤ tol` requires `c ≤ √(8·R·tol)`. Before
//! this fix the step was bounded ONLY by the turn criterion (~11.5° per
//! sample), which is scale-free and therefore tolerance-blind: it pins the
//! sagitta at `TURN_TARGET²/8 · R ≈ 0.5 %` of the radius whatever tolerance the
//! caller passed. At `R = 6` that is ≈ 0.03 — ~30,000× coarser than a
//! `1e-6` request, and silently so.
//!
//! A plane through a cylinder of radius 6 has an analytic answer — a circle of
//! radius 6 — so the mid-chord deviation of the returned polyline can be
//! measured exactly against the true curve. The vertices lie ON both surfaces
//! (the corrector puts them there); what the sagitta term governs is how far
//! the straight SEGMENTS between them bow inside the true circle.

use geometry_engine::math::surface_intersection::intersect_surfaces;
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::primitives::surface::{Cylinder, Plane};

/// Plane z = 0 (normal +Z) cutting a cylinder of radius 6 about the Z axis:
/// the intersection is the circle x² + y² = 36 in the plane z = 0. The mid-
/// chord sagitta of every emitted segment must respect the distance tolerance.
#[test]
fn plane_cylinder_polyline_honours_distance_tolerance() {
    const RADIUS: f64 = 6.0;
    let tol_dist = 1e-6;
    let tol = Tolerance::from_distance(tol_dist);

    let cyl = Cylinder::new(Point3::ORIGIN, Vector3::Z, RADIUS).expect("cylinder");
    let plane = Plane::new(Point3::ORIGIN, Vector3::Z, Vector3::X).expect("plane");

    let curves = intersect_surfaces(&plane, &cyl, &tol).expect("ssi");
    assert!(
        !curves.is_empty(),
        "plane×cylinder must intersect in a circle"
    );

    // Richest branch (the traced circle).
    let circle = curves
        .iter()
        .max_by_key(|c| c.points.len())
        .expect("at least one curve");
    assert!(
        circle.points.len() >= 3,
        "circle traced with only {} points — no polyline to measure",
        circle.points.len()
    );

    // Every vertex must sit on the true circle (sanity: the corrector converged).
    for p in &circle.points {
        let r = (p.x * p.x + p.y * p.y).sqrt();
        assert!(
            (r - RADIUS).abs() < 1e-4 && p.z.abs() < 1e-4,
            "vertex off the true circle: r={r:.6}, z={:.6}",
            p.z
        );
    }

    // Mid-chord deviation: the midpoint of a chord of a radius-R circle lies at
    // radius R·cos(θ/2) < R, so the segment's peak deviation from the true
    // curve is the sagitta R − dist_from_axis(midpoint). Measure the max over
    // all segments, including the closing segment when the loop is closed.
    let mut max_sagitta = 0.0_f64;
    let n = circle.points.len();
    let seg_count = if circle.is_closed { n } else { n - 1 };
    for i in 0..seg_count {
        let a = circle.points[i];
        let b = circle.points[(i + 1) % n];
        let m = (a + b) * 0.5;
        let r_mid = (m.x * m.x + m.y * m.y).sqrt();
        let sagitta = (RADIUS - r_mid).abs();
        max_sagitta = max_sagitta.max(sagitta);
    }

    eprintln!(
        "plane_cylinder sagitta: points={} closed={} max_sagitta={max_sagitta:.3e} tol={tol_dist:.1e} (ratio {:.2})",
        n,
        circle.is_closed,
        max_sagitta / tol_dist,
    );

    // The contract is `sagitta ≤ tol`; the estimate uses `R = travel/turn` from
    // the previous step, so allow a small multiple for that lag. The turn-only
    // predecessor sat at ≈ 0.03 (5e3× this bound), so the assertion is a sharp
    // separator, not an aspirational one.
    let bound = 20.0 * tol_dist;
    assert!(
        max_sagitta < bound,
        "polyline mid-chord deviation {max_sagitta:.3e} exceeds {bound:.3e} \
         ({:.0}× the distance tolerance) — the emitted curve does not honour \
         the caller's tolerance; step control is turn-only, not sagitta-bounded",
        max_sagitta / tol_dist,
    );
}
