//! Advanced rotation/transform invariants: Quaternion interpolation, Euler
//! round-trips, rotation-between, powers; Matrix4 mirror, scale-about-point,
//! rotation-about-axis. Pure arithmetic, fast.

use geometry_engine::math::{Matrix4, Point3, Quaternion, Vector3};

const FTOL: f64 = 1e-6;

fn v(x: f64, y: f64, z: f64) -> Vector3 {
    Vector3::new(x, y, z)
}
fn pt(x: f64, y: f64, z: f64) -> Point3 {
    Point3::new(x, y, z)
}
fn vclose(a: Vector3, b: Vector3, t: f64) -> bool {
    (a.x - b.x).abs() <= t && (a.y - b.y).abs() <= t && (a.z - b.z).abs() <= t
}
fn pclose(a: Point3, b: Point3, t: f64) -> bool {
    (a.x - b.x).abs() <= t && (a.y - b.y).abs() <= t && (a.z - b.z).abs() <= t
}
fn quat(axis: Vector3, angle: f64) -> Quaternion {
    Quaternion::from_axis_angle(&axis.normalize_or_zero(), angle).expect("quat")
}

// =====================================================================
// Quaternion interpolation.
// =====================================================================

macro_rules! slerp_idempotent_test {
    ($name:ident, $ax:expr, $ay:expr, $az:expr, $ang:expr, $t:expr) => {
        #[test]
        fn $name() {
            let q = quat(v($ax, $ay, $az), $ang);
            let probe = v(1.0, 2.0, 3.0);
            // slerp/nlerp between a quaternion and itself is that quaternion.
            let s = q.slerp(&q, $t);
            let n = q.nlerp(&q, $t);
            assert!(
                vclose(s.rotate_vector(&probe), q.rotate_vector(&probe), FTOL),
                "slerp(q,q,t) != q"
            );
            assert!(
                vclose(n.rotate_vector(&probe), q.rotate_vector(&probe), FTOL),
                "nlerp(q,q,t) != q"
            );
            // Both interpolants are unit quaternions.
            assert!((s.magnitude() - 1.0).abs() <= FTOL, "slerp result not unit");
            assert!((n.magnitude() - 1.0).abs() <= FTOL, "nlerp result not unit");
        }
    };
}

slerp_idempotent_test!(slerp_z_quarter, 0.0, 0.0, 1.0, 1.0, 0.25);
slerp_idempotent_test!(slerp_x_half, 1.0, 0.0, 0.0, 2.0, 0.5);
slerp_idempotent_test!(slerp_diag, 1.0, 1.0, 1.0, 0.7, 0.75);
slerp_idempotent_test!(slerp_arb, 2.0, -1.0, 3.0, 1.4, 0.3);

#[test]
fn slerp_and_nlerp_stay_unit_across_t() {
    let q0 = quat(v(0.0, 0.0, 1.0), 0.4);
    let q1 = quat(v(1.0, 1.0, 0.0), 1.8);
    for i in 0..=10 {
        let t = i as f64 / 10.0;
        assert!(
            (q0.slerp(&q1, t).magnitude() - 1.0).abs() <= FTOL,
            "slerp not unit at t={t}"
        );
        assert!(
            (q0.nlerp(&q1, t).magnitude() - 1.0).abs() <= FTOL,
            "nlerp not unit at t={t}"
        );
    }
}

// =====================================================================
// Euler round-trip (avoiding gimbal lock at pitch = ±π/2).
// =====================================================================

macro_rules! euler_roundtrip_test {
    ($name:ident, $x:expr, $y:expr, $z:expr) => {
        #[test]
        fn $name() {
            let q = Quaternion::from_euler_xyz($x, $y, $z);
            let (rx, ry, rz) = q.to_euler_xyz();
            let q2 = Quaternion::from_euler_xyz(rx, ry, rz);
            // The recovered Euler angles must reproduce the same rotation.
            let probe = v(1.0, -2.0, 0.5);
            assert!(
                vclose(q.rotate_vector(&probe), q2.rotate_vector(&probe), FTOL),
                "euler round-trip diverged"
            );
        }
    };
}

euler_roundtrip_test!(euler_small, 0.3, 0.2, 0.1);
euler_roundtrip_test!(euler_mid, 0.8, -0.6, 1.2);
euler_roundtrip_test!(euler_neg, -0.5, 0.4, -0.9);
euler_roundtrip_test!(euler_z_only, 0.0, 0.0, 1.5);
euler_roundtrip_test!(euler_x_only, 1.1, 0.0, 0.0);

// =====================================================================
// from_rotation_between rotates one direction onto another.
// =====================================================================

macro_rules! rotation_between_test {
    ($name:ident, $fx:expr, $fy:expr, $fz:expr, $tx:expr, $ty:expr, $tz:expr) => {
        #[test]
        fn $name() {
            let from = v($fx, $fy, $fz);
            let to = v($tx, $ty, $tz);
            let q = Quaternion::from_rotation_between(&from, &to).expect("rotation_between");
            let rotated = q.rotate_vector(&from.normalize_or_zero());
            assert!(
                vclose(rotated, to.normalize_or_zero(), FTOL),
                "rotation_between did not map from→to: {:?} vs {:?}",
                rotated,
                to.normalize_or_zero()
            );
        }
    };
}

rotation_between_test!(rotbtw_x_to_y, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0);
rotation_between_test!(rotbtw_x_to_z, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0);
rotation_between_test!(rotbtw_diag, 1.0, 1.0, 1.0, 1.0, -1.0, 0.0);
rotation_between_test!(rotbtw_arb, 2.0, 3.0, -1.0, -1.0, 4.0, 2.0);
rotation_between_test!(rotbtw_near, 1.0, 0.0, 0.0, 1.0, 0.1, 0.0);

// =====================================================================
// Quaternion powers.
// =====================================================================

macro_rules! quat_pow_test {
    ($name:ident, $ax:expr, $ay:expr, $az:expr, $ang:expr) => {
        #[test]
        fn $name() {
            let q = quat(v($ax, $ay, $az), $ang);
            let probe = v(1.0, 2.0, -1.0);
            // q^1 == q.
            let p1 = q.pow(1.0).expect("pow1");
            assert!(
                vclose(p1.rotate_vector(&probe), q.rotate_vector(&probe), FTOL),
                "q^1 != q"
            );
            // q^2 applied == q applied twice.
            let p2 = q.pow(2.0).expect("pow2");
            let twice = q.rotate_vector(&q.rotate_vector(&probe));
            assert!(vclose(p2.rotate_vector(&probe), twice, FTOL), "q^2 != q∘q");
            // q^0 == identity (no rotation).
            let p0 = q.pow(0.0).expect("pow0");
            assert!(
                vclose(p0.rotate_vector(&probe), probe, FTOL),
                "q^0 != identity"
            );
        }
    };
}

quat_pow_test!(pow_z, 0.0, 0.0, 1.0, 0.8);
quat_pow_test!(pow_x, 1.0, 0.0, 0.0, 1.1);
quat_pow_test!(pow_diag, 1.0, 1.0, 1.0, 0.6);
quat_pow_test!(pow_arb, 2.0, -1.0, 1.0, 1.3);

// =====================================================================
// Matrix4 mirror — an involution that preserves distances.
// =====================================================================

macro_rules! mirror_test {
    ($name:ident, $px:expr, $py:expr, $pz:expr, $nx:expr, $ny:expr, $nz:expr) => {
        #[test]
        fn $name() {
            let plane_pt = pt($px, $py, $pz);
            let normal = v($nx, $ny, $nz).normalize_or_zero();
            let m = Matrix4::mirror(plane_pt, normal).expect("mirror");
            let probes = [pt(1.0, 2.0, 3.0), pt(-4.0, 0.0, 5.0), pt(2.0, -3.0, -1.0)];
            // Reflecting twice is the identity.
            for q in probes {
                let twice = m.transform_point(&m.transform_point(&q));
                assert!(pclose(twice, q, FTOL), "mirror not involutive at {q:?}");
            }
            // Reflection is an isometry: distances are preserved.
            let a = m.transform_point(&probes[0]);
            let b = m.transform_point(&probes[1]);
            let orig = (probes[0] - probes[1]).magnitude();
            assert!(
                ((a - b).magnitude() - orig).abs() <= FTOL * (1.0 + orig),
                "mirror changed distance"
            );
        }
    };
}

mirror_test!(mirror_xy, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0);
mirror_test!(mirror_yz, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0);
mirror_test!(mirror_diag, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0);
mirror_test!(mirror_offset, 3.0, -2.0, 1.0, 0.0, 1.0, 0.0);

// =====================================================================
// Matrix4 scale-about-point — fixes the pivot, scales distances.
// =====================================================================

macro_rules! scale_about_test {
    ($name:ident, $px:expr, $py:expr, $pz:expr, $s:expr) => {
        #[test]
        fn $name() {
            let pivot = pt($px, $py, $pz);
            let s = $s as f64;
            let m = Matrix4::scale_about_point(pivot, v(s, s, s));
            // Pivot is fixed.
            assert!(
                pclose(m.transform_point(&pivot), pivot, FTOL),
                "pivot not fixed"
            );
            // A point at distance d from the pivot maps to distance s·d.
            for q in [pt(1.0, 0.0, 0.0), pt(2.0, 3.0, -1.0), pt(-4.0, 1.0, 2.0)] {
                let d0 = (q - pivot).magnitude();
                let mapped = m.transform_point(&q);
                let d1 = (mapped - pivot).magnitude();
                assert!(
                    (d1 - s * d0).abs() <= FTOL * (1.0 + s * d0),
                    "scale-about distance off"
                );
            }
        }
    };
}

scale_about_test!(scale_about_origin_2, 0.0, 0.0, 0.0, 2.0);
scale_about_test!(scale_about_offset_3, 5.0, -2.0, 1.0, 3.0);
scale_about_test!(scale_about_half, 1.0, 1.0, 1.0, 0.5);
scale_about_test!(scale_about_big, 0.0, 0.0, 0.0, 10.0);

// =====================================================================
// Matrix4 rotation-about-axis-through-point — fixes the point, isometric.
// =====================================================================

macro_rules! rotation_axis_test {
    ($name:ident, $px:expr, $py:expr, $pz:expr, $ax:expr, $ay:expr, $az:expr, $ang:expr) => {
        #[test]
        fn $name() {
            let point = pt($px, $py, $pz);
            let axis = v($ax, $ay, $az);
            let m = Matrix4::rotation_axis(point, axis, $ang).expect("rotation_axis");
            // The pivot point on the axis is fixed.
            assert!(
                pclose(m.transform_point(&point), point, FTOL),
                "axis point not fixed"
            );
            // Distances from the pivot are preserved (rigid rotation).
            for q in [pt(1.0, 2.0, 3.0), pt(-2.0, 0.0, 4.0), pt(3.0, -1.0, 0.0)] {
                let d0 = (q - point).magnitude();
                let d1 = (m.transform_point(&q) - point).magnitude();
                assert!(
                    (d1 - d0).abs() <= FTOL * (1.0 + d0),
                    "rotation_axis changed distance"
                );
            }
        }
    };
}

rotation_axis_test!(rotax_z_origin, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0);
rotation_axis_test!(rotax_z_offset, 2.0, 3.0, 0.0, 0.0, 0.0, 1.0, 0.7);
rotation_axis_test!(rotax_diag, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 2.0);
rotation_axis_test!(rotax_x, 0.0, 5.0, 0.0, 1.0, 0.0, 0.0, 1.5);
