#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Horn and spindle tori (minor radius >= major radius) are valid surfaces.
//!
//! STEP allows them and Open CASCADE builds them (its torus rejects only
//! negative radii): a fillet that rounds a cylinder's end right down to the
//! axis is a horn torus. Generated CAD is full of them -- 13 of the 81
//! CADGenBench parts built no solid because the kernel refused the torus.
//! The surface is the same parametric torus; it pinches to a point on the
//! axis where R + r cos v = 0, so the normal there comes from the tube, not
//! from du x dv (du vanishes).

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::surface::{Surface, Torus};

#[test]
fn horn_and_spindle_tori_can_be_built() {
    assert!(
        Torus::new(Point3::ZERO, Vector3::Z, 5.0, 5.0).is_ok(),
        "horn torus"
    );
    assert!(
        Torus::new(Point3::ZERO, Vector3::Z, 2.0, 5.0).is_ok(),
        "spindle torus"
    );
    assert!(
        Torus::new(Point3::ZERO, Vector3::Z, 5.0, 0.0).is_err(),
        "zero tube"
    );
    assert!(
        Torus::new(Point3::ZERO, Vector3::Z, -1.0, 2.0).is_err(),
        "negative ring"
    );
}

#[test]
fn a_horn_torus_has_a_normal_at_its_pinch() {
    let t = Torus::new(Point3::ZERO, Vector3::Z, 5.0, 5.0).unwrap();
    for k in 0..8 {
        let u = k as f64 * std::f64::consts::TAU / 8.0;
        let p = t
            .evaluate_full(u, std::f64::consts::PI)
            .expect("the pinch evaluates");
        assert!(
            p.position.distance(&Point3::ZERO) < 1e-9,
            "the pinch is on the axis"
        );
        let n = p.normal;
        assert!(
            (n.magnitude() - 1.0).abs() < 1e-9,
            "unit normal at the pinch"
        );
        // The tube's normal there points back toward the axis-side of the
        // tube centre: -radial.
        let (x, y) = (t.ref_dir, t.axis.cross(&t.ref_dir));
        let radial = x * u.cos() + y * u.sin();
        assert!((n + radial).magnitude() < 1e-9, "normal {n:?} at u={u}");
    }
}

/// Away from the pinch the normal is du x dv, as for any parametric
/// surface, on ring, horn and spindle tori alike (including the spindle's
/// inner lobe, where R + r cos v < 0 flips the parametric orientation).
#[test]
fn the_normal_is_du_cross_dv_wherever_that_is_defined() {
    for (big, small) in [(10.0, 2.0), (5.0, 5.0), (2.0, 5.0)] {
        let t = Torus::new(
            Point3::new(1.0, -2.0, 3.0),
            Vector3::new(0.0, 1.0, 1.0),
            big,
            small,
        )
        .unwrap();
        for i in 0..24 {
            for j in 0..24 {
                let (u, v) = (i as f64 * 0.27, j as f64 * 0.263);
                let p = t.evaluate_full(u, v).expect("evaluates");
                let c = p.du.cross(&p.dv);
                if c.magnitude() < 1e-6 {
                    continue;
                }
                let want = c.normalize().unwrap();
                assert!(
                    (p.normal - want).magnitude() < 1e-9,
                    "R={big} r={small} at ({u},{v}): normal {:?} vs du x dv {want:?}",
                    p.normal
                );
                assert!(p.k1.is_finite() && p.k2.is_finite());
            }
        }
    }
}
