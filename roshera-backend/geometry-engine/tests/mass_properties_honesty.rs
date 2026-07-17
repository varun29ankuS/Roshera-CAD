// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Mass-properties HONESTY gate.
//!
//! The "kernel cannot lie" thesis requires that every mass-property value the
//! kernel serves carries its exactness provenance: a caller must be able to
//! learn, per quantity (volume, centroid, inertia), whether the number is
//! `Exact` (closed-form / algebraically exact), `Approximate { method, bound }`
//! (a numerical estimate with a stated relative-error bound), or `Unavailable`.
//!
//! Two non-negotiable contracts pinned here:
//!  1. NOTHING labelled `Exact` may diverge from closed form beyond floating
//!     noise. (A bbox / mesh number stamped "exact" is the launch-blocker lie.)
//!  2. An `Approximate` label MUST carry a bound that actually contains the
//!     observed error (an honest self-certified accuracy, not a fiction).

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::mass_properties::integrate_solid;
use geometry_engine::primitives::solid::{Exactness, SolidId};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

#[allow(clippy::expect_used, clippy::panic)]
fn expect_solid(geom: GeometryId) -> SolidId {
    match geom {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid geometry, got {other:?}"),
    }
}

fn make_box(m: &mut BRepModel, w: f64, d: f64, h: f64) -> SolidId {
    expect_solid(TopologyBuilder::new(m).create_box_3d(w, d, h).expect("box"))
}

fn make_cyl(m: &mut BRepModel, base: Point3, axis: Vector3, r: f64, h: f64) -> SolidId {
    expect_solid(
        TopologyBuilder::new(m)
            .create_cylinder_3d(base, axis, r, h)
            .expect("cyl"),
    )
}

/// RED / characterisation: record the numeric divergence of the production
/// (mesh Tonon) inertia vs the exact analytic integrator for a box, so the
/// per-quantity gap is on the record, not asserted-away.
#[test]
fn characterize_box_mesh_vs_analytic_inertia() {
    let mut m = BRepModel::new();
    let id = make_box(&mut m, 3.0, 4.0, 5.0);

    // Exact analytic integrator (density 1) — closed form m=60:
    // Ixx=205, Iyy=170, Izz=125.
    let analytic = integrate_solid(id, &m, 1.0, 1e-9).expect("analytic mp");
    let mesh = m.mass_properties_for(id).expect("mesh report");

    // Analytic must be exact to floating noise.
    let approx = |a: f64, b: f64| (a - b).abs() / b;
    eprintln!(
        "BOX analytic I = [{:.6}, {:.6}, {:.6}] (closed form 205/170/125)",
        analytic.inertia_tensor[0][0], analytic.inertia_tensor[1][1], analytic.inertia_tensor[2][2]
    );
    // Mesh ratios I/m vs closed form (density-independent).
    let mi = |k: usize| mesh.inertia_tensor[k][k] / mesh.mass;
    eprintln!(
        "BOX mesh   I/m = [{:.6}, {:.6}, {:.6}] closed form [{:.6}, {:.6}, {:.6}]",
        mi(0),
        mi(1),
        mi(2),
        205.0 / 60.0,
        170.0 / 60.0,
        125.0 / 60.0
    );
    eprintln!(
        "BOX mesh inertia rel-divergence from closed form: Ixx {:.2e} Iyy {:.2e} Izz {:.2e}",
        approx(mi(0), 205.0 / 60.0),
        approx(mi(1), 170.0 / 60.0),
        approx(mi(2), 125.0 / 60.0)
    );
    assert!(approx(analytic.inertia_tensor[0][0], 205.0) < 1e-9);
    assert!(approx(analytic.inertia_tensor[2][2], 125.0) < 1e-9);
}

/// A BOX is an untrimmed polyhedron — the analytic integrator is algebraically
/// exact, so the served report MUST advertise `Exactness::Exact` for every
/// quantity AND match closed form to floating noise. Anything labelled `Exact`
/// that misses closed form is the thesis violation.
#[test]
fn box_report_is_labelled_exact_and_is_exact() {
    let mut m = BRepModel::new();
    let id = make_box(&mut m, 2.0, 2.0, 2.0);
    let r = m.mass_properties_for(id).expect("report");

    assert_eq!(
        r.provenance.inertia,
        Exactness::Exact,
        "an untrimmed polyhedron's inertia must be served Exact, got {:?}",
        r.provenance.inertia
    );
    assert_eq!(r.provenance.volume, Exactness::Exact);
    assert_eq!(r.provenance.center_of_mass, Exactness::Exact);

    // If we claim Exact, we must BE exact: I/m = (4+4)/12 = 2/3 on every axis.
    for k in 0..3 {
        let got = r.inertia_tensor[k][k] / r.mass;
        assert!(
            (got - 2.0 / 3.0).abs() < 1e-9,
            "Exact-labelled box I[{k}][{k}]/m = {got} != 2/3",
        );
        for j in 0..3 {
            if j != k {
                assert!(r.inertia_tensor[k][j].abs() / r.mass < 1e-9);
            }
        }
    }
}

/// A CYLINDER has trimmed disk caps — the analytic integrator is NOT reliable
/// there, so the kernel must NOT claim `Exact`. It serves the mesh estimate,
/// honestly labelled `Approximate`, and the stated bound MUST contain the
/// observed error (self-certified accuracy that does not lie).
#[test]
fn cylinder_inertia_is_honest_approximate_with_containing_bound() {
    let mut m = BRepModel::new();
    // r=0.5, h=2, centered on Z (base z=-1): Ixx/m=Iyy/m=(3r²+h²)/12=4.75/12,
    // Izz/m=r²/2=0.125.
    let id = make_cyl(&mut m, Point3::new(0.0, 0.0, -1.0), Vector3::Z, 0.5, 2.0);
    let r = m.mass_properties_for(id).expect("report");

    let bound = match r.provenance.inertia {
        Exactness::Approximate {
            rel_error_bound, ..
        } => rel_error_bound,
        other => panic!("cylinder inertia must be Approximate, got {other:?}"),
    };
    assert!(
        bound > 0.0 && bound.is_finite(),
        "bound must be a real number"
    );

    let closed = [4.75 / 12.0, 4.75 / 12.0, 0.125];
    for k in 0..3 {
        let got = r.inertia_tensor[k][k] / r.mass;
        let observed = (got - closed[k]).abs() / closed[k];
        eprintln!(
            "CYL I[{k}][{k}]/m observed rel-error {observed:.2e} vs stated bound {bound:.2e}"
        );
        assert!(
            observed <= bound,
            "stated Approximate bound {bound:.2e} must contain observed error {observed:.2e} on axis {k}",
        );
    }
}
