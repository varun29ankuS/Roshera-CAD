//! Property tests for analytic-surface curvature (CD-φ.1.1 fundamental
//! forms), checked against closed-form oracles that hold at every interior
//! parameter point.
//!
//! Pins `Surface::fundamental_forms_at` + the `FundamentalForms` curvature
//! accessors against textbook values:
//!   - sphere radius r:   K = 1/r², umbilic |k1| = |k2| = 1/r, |H| = 1/r
//!   - cylinder radius r: K = 0 (developable), principal {±1/r, 0},
//!     |H| = 1/(2r)
//!
//! Oracles are chosen INDEPENDENT of the surface-normal sign convention:
//! Gaussian curvature is invariant under a normal flip (both principal
//! curvatures flip together), and principal/mean curvature is asserted via
//! absolute value. A future change to which way a surface's normal points
//! therefore cannot spuriously redden these.

#![allow(clippy::unwrap_used)]
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::surface::{Cylinder, Sphere, Surface};

const TOL: f64 = 1e-6;

/// Assert `a` and `b` agree within `TOL` (absolute).
fn close(a: f64, b: f64, what: &str) {
    assert!(
        (a - b).abs() < TOL,
        "{what}: expected {b}, got {a} (|Δ| = {})",
        (a - b).abs()
    );
}

/// A spread of interior parametric fractions, mapped into a surface's
/// `parameter_bounds` so the sample never lands on a domain edge / seam /
/// pole where the first fundamental form degenerates.
fn interior_samples(surface: &dyn Surface) -> Vec<(f64, f64)> {
    let ((u0, u1), (v0, v1)) = surface.parameter_bounds();
    let frac = [0.25_f64, 0.5, 0.75];
    let mut out = Vec::new();
    for &fu in &frac {
        for &fv in &frac {
            out.push((u0 + (u1 - u0) * fu, v0 + (v1 - v0) * fv));
        }
    }
    out
}

// ---- Sphere: K = 1/r², umbilic |k1| = |k2| = 1/r, |H| = 1/r ----------

fn sphere_curvature_matches_closed_form(radius: f64) {
    let sphere = Sphere::new(Point3::ORIGIN, radius).expect("sphere builds");
    let k_expected = 1.0 / (radius * radius);
    let kappa = 1.0 / radius;
    for (u, v) in interior_samples(&sphere) {
        let ff = sphere
            .fundamental_forms_at(u, v)
            .unwrap_or_else(|e| panic!("forms at ({u},{v}) on r={radius}: {e:?}"));
        close(
            ff.gaussian_curvature().unwrap(),
            k_expected,
            "sphere Gaussian curvature",
        );
        let (k1, k2) = ff.principal_curvatures().unwrap();
        close(k1.abs(), kappa, "sphere |k1|");
        close(k2.abs(), kappa, "sphere |k2|");
        // Umbilic: the two principal curvatures are equal.
        close(k1, k2, "sphere principal curvatures equal (umbilic)");
        close(ff.mean_curvature().unwrap().abs(), kappa, "sphere |H|");
    }
}

#[test]
fn sphere_unit_radius_curvature() {
    sphere_curvature_matches_closed_form(1.0);
}

#[test]
fn sphere_small_radius_curvature() {
    sphere_curvature_matches_closed_form(0.5);
}

#[test]
fn sphere_large_radius_curvature() {
    sphere_curvature_matches_closed_form(5.0);
}

#[test]
fn sphere_curvature_scales_inversely_with_radius() {
    // K = 1/r² is monotone-decreasing in r: a bigger sphere is flatter.
    let small = Sphere::new(Point3::ORIGIN, 1.0).unwrap();
    let big = Sphere::new(Point3::ORIGIN, 4.0).unwrap();
    let (us, vs) = {
        let b = small.parameter_bounds();
        ((b.0 .0 + b.0 .1) * 0.5, (b.1 .0 + b.1 .1) * 0.5)
    };
    let ks = small
        .fundamental_forms_at(us, vs)
        .unwrap()
        .gaussian_curvature()
        .unwrap();
    let kb = big
        .fundamental_forms_at(us, vs)
        .unwrap()
        .gaussian_curvature()
        .unwrap();
    assert!(ks > kb, "smaller sphere must be more curved: {ks} !> {kb}");
    close(ks / kb, 16.0, "K ratio = (4/1)²");
}

// ---- Cylinder: K = 0, principal {±1/r, 0}, |H| = 1/(2r) --------------

fn cylinder_curvature_matches_closed_form(radius: f64) {
    let cyl = Cylinder::new(Point3::ORIGIN, Vector3::Z, radius).expect("cylinder builds");
    let kappa = 1.0 / radius;
    for (u, v) in interior_samples(&cyl) {
        let ff = cyl
            .fundamental_forms_at(u, v)
            .unwrap_or_else(|e| panic!("forms at ({u},{v}) on r={radius}: {e:?}"));
        // A cylinder is developable: Gaussian curvature is exactly zero.
        close(
            ff.gaussian_curvature().unwrap(),
            0.0,
            "cylinder Gaussian curvature",
        );
        let (k1, k2) = ff.principal_curvatures().unwrap();
        // One principal direction is flat (0), the other is the hoop 1/r.
        let max_abs = k1.abs().max(k2.abs());
        let min_abs = k1.abs().min(k2.abs());
        close(max_abs, kappa, "cylinder hoop curvature |1/r|");
        close(min_abs, 0.0, "cylinder axial curvature 0");
        close(
            ff.mean_curvature().unwrap().abs(),
            kappa * 0.5,
            "cylinder |H| = 1/(2r)",
        );
    }
}

#[test]
fn cylinder_unit_radius_curvature() {
    cylinder_curvature_matches_closed_form(1.0);
}

#[test]
fn cylinder_small_radius_curvature() {
    cylinder_curvature_matches_closed_form(0.5);
}

#[test]
fn cylinder_large_radius_curvature() {
    cylinder_curvature_matches_closed_form(3.0);
}
