//! Characterization + hardening fixtures for the general marching SSI
//! (`math::surface_intersection`). These stress the marcher on freeform-style
//! (purely numerical, no analytic QSIC path) configurations and assert
//! quantitative curve quality: extent, sample count, on-both-surfaces
//! residual, and loop topology.
//!
//! Roadmap item #4 (marching SSI hardening). Lineage: Barnhill-Kersey /
//! Patrikalakis-Maekawa ch.5 / Grandine-Klein.

use geometry_engine::math::surface_intersection::{intersect_surfaces, IntersectionCurve};
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::primitives::surface::{Cylinder, Sphere};

fn tol() -> Tolerance {
    Tolerance::from_distance(1e-6)
}

/// Diagnostics for a traced polyline.
struct CurveStats {
    n_points: usize,
    n_distinct: usize,
    bbox_diag: f64,
    arc_len: f64,
}

fn stats(c: &IntersectionCurve) -> CurveStats {
    let n_points = c.points.len();
    // distinct within 1e-4
    let mut distinct: Vec<Point3> = Vec::new();
    for p in &c.points {
        if !distinct.iter().any(|q| (*q - *p).magnitude() < 1e-4) {
            distinct.push(*p);
        }
    }
    let (mut lo, mut hi) = (
        Point3::new(f64::MAX, f64::MAX, f64::MAX),
        Point3::new(f64::MIN, f64::MIN, f64::MIN),
    );
    for p in &c.points {
        lo = Point3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
        hi = Point3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
    }
    let bbox_diag = if n_points == 0 {
        0.0
    } else {
        (hi - lo).magnitude()
    };
    let mut arc_len = 0.0;
    for i in 1..c.points.len() {
        arc_len += (c.points[i] - c.points[i - 1]).magnitude();
    }
    CurveStats {
        n_points,
        n_distinct: distinct.len(),
        bbox_diag,
        arc_len,
    }
}

/// Two unit spheres offset by 1 along X → intersection is a circle in the
/// plane x=0.5 of radius sqrt(3)/2 ≈ 0.866. Circumference ≈ 5.44, bbox
/// diagonal of the circle ≈ 2*r*sqrt(2) ≈ 2.449 (spans y,z).
///
/// A correct marcher traces the whole circle: arc length near 5.44, bbox
/// diagonal near 2.449, dozens of distinct points, and closes.
#[test]
fn sphere_sphere_overlap_traces_full_circle() {
    let s1 = Sphere::new(Point3::ORIGIN, 1.0).expect("s1");
    let s2 = Sphere::new(Point3::new(1.0, 0.0, 0.0), 1.0).expect("s2");
    let t = tol();
    let curves = intersect_surfaces(&s1, &s2, &t).expect("ssi");
    assert!(!curves.is_empty(), "overlapping spheres must intersect");

    // Take the richest curve (most distinct points).
    let best = curves
        .iter()
        .max_by_key(|c| stats(c).n_distinct)
        .expect("at least one curve");
    let st = stats(best);
    let r = (3.0_f64).sqrt() / 2.0;
    let expected_circumference = 2.0 * std::f64::consts::PI * r;
    let expected_diag = 2.0 * r * (2.0_f64).sqrt();

    eprintln!(
        "sphere_sphere: curves={} best n_points={} n_distinct={} bbox_diag={:.4} (exp {:.4}) arc_len={:.4} (exp {:.4}) closed={}",
        curves.len(),
        st.n_points,
        st.n_distinct,
        st.bbox_diag,
        expected_diag,
        st.arc_len,
        expected_circumference,
        best.is_closed
    );

    assert!(
        st.bbox_diag > 0.9 * expected_diag,
        "traced curve bbox diagonal {:.4} collapsed vs expected circle diagonal {:.4} — marcher is not advancing",
        st.bbox_diag,
        expected_diag
    );
    assert!(
        st.arc_len > 0.8 * expected_circumference,
        "traced arc length {:.4} far short of circle circumference {:.4}",
        st.arc_len,
        expected_circumference
    );
    assert!(
        st.n_distinct >= 20,
        "only {} distinct points on a full circle — marcher stalled",
        st.n_distinct
    );
    assert!(best.is_closed, "sphere-sphere circle must close");
    // One connected component — the single circle — not a pile of re-traces.
    assert_eq!(
        curves.len(),
        1,
        "sphere-sphere intersection is one circle; got {} curves",
        curves.len()
    );
}

/// Small cylinders (radius 0.2) crossing orthogonally. High curvature of the
/// intersection relative to a fixed world-space step stresses adaptive step
/// control. Every sample must lie on both cylinders and the polyline must
/// span a real extent (not collapse to the seed cloud).
#[test]
fn small_orthogonal_cylinders_trace_and_stay_on_surface() {
    let r = 0.2;
    let cyl_z = Cylinder::new(Point3::new(0.0, 0.0, -1.0), Vector3::Z, r).expect("cyl_z");
    let cyl_x = Cylinder::new(Point3::new(-1.0, 0.0, 0.0), Vector3::X, r).expect("cyl_x");
    let t = tol();
    let curves = intersect_surfaces(&cyl_z, &cyl_x, &t).expect("ssi");
    assert!(
        !curves.is_empty(),
        "small orthogonal cylinders must intersect"
    );

    let best = curves
        .iter()
        .max_by_key(|c| stats(c).n_distinct)
        .expect("curve");
    let st = stats(best);
    eprintln!(
        "small_cyl: curves={} best n_distinct={} bbox_diag={:.4} arc_len={:.4} closed={}",
        curves.len(),
        st.n_distinct,
        st.bbox_diag,
        st.arc_len,
        best.is_closed
    );

    // On-both-surfaces residual: distance from Z-axis and X-axis both ≈ r.
    for c in &curves {
        for p in &c.points {
            let r_z = (p.x * p.x + p.y * p.y).sqrt();
            let r_x = (p.y * p.y + p.z * p.z).sqrt();
            assert!(
                (r_z - r).abs() < 1e-3,
                "off Z-cylinder: r_z={} vs {}",
                r_z,
                r
            );
            assert!(
                (r_x - r).abs() < 1e-3,
                "off X-cylinder: r_x={} vs {}",
                r_x,
                r
            );
        }
    }
    // The Steinmetz seam for equal radii spans roughly 2r in each of x and z.
    assert!(
        st.bbox_diag > 0.5 * r,
        "small-cylinder seam collapsed: bbox_diag={:.4}",
        st.bbox_diag
    );
    assert!(
        st.n_distinct >= 12,
        "small-cylinder seam only {} distinct points — marcher stalled at high curvature",
        st.n_distinct
    );
}
