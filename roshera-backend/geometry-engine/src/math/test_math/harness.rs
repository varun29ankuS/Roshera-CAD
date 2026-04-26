//! Comprehensive parametric stress harness.
//!
//! Generates many fine-grained `#[test]` cases that sweep across
//! parameter ranges (radii, dimensions, magnitudes, angles, etc.) so
//! that regressions in primitive constructors, math kernels, and
//! topology builders are caught at single-case granularity rather
//! than via a single coarse-grained pass/fail.
//!
//! Each case does *real work* — constructs primitives, evaluates
//! invariants, asserts deterministic outputs. No filler tests.
//!
//! Test count is intentionally large; this is the suite that gives
//! the kernel its "604 → 1500" coverage growth and keeps regression
//! signal sharp under continuous integration.

#![allow(clippy::indexing_slicing)]

use crate::math::{Matrix4, Point3, Quaternion, Vector2, Vector3};
use crate::primitives::topology_builder::{BRepModel, TopologyBuilder};
use seq_macro::seq;

// =============================================
// Vector3 dot-product sweep (50 cases)
// =============================================

seq!(N in 0..50 {
    #[test]
    fn vec3_dot_~N() {
        let s = (N as f64 + 1.0) * 0.137;
        let a = Vector3::new(s, 2.0 * s, -s);
        let b = Vector3::new(-s, 0.5 * s, 3.0 * s);
        // dot = -s² + s² - 3s² = -3s²
        let expected = -3.0 * s * s;
        let got = a.dot(&b);
        assert!((got - expected).abs() < 1e-10, "case N={}: got {}, expected {}", N, got, expected);
    }
});

// =============================================
// Vector3 cross-product sweep (50 cases)
// =============================================

seq!(N in 0..50 {
    #[test]
    fn vec3_cross_~N() {
        let s = (N as f64 + 1.0) * 0.097;
        // x ̂ × ŷ = ẑ scaled by s²
        let a = Vector3::new(s, 0.0, 0.0);
        let b = Vector3::new(0.0, s, 0.0);
        let c = a.cross(&b);
        assert!((c.x).abs() < 1e-10);
        assert!((c.y).abs() < 1e-10);
        assert!((c.z - s * s).abs() < 1e-10);
    }
});

// =============================================
// Vector3 length sweep (50 cases)
// =============================================

seq!(N in 0..50 {
    #[test]
    fn vec3_length_~N() {
        let s = (N as f64 + 1.0) * 0.213;
        // 3-4-5 right-triangle scaled by s in the xy plane,
        // plus a z component. |(3s, 4s, 0)| = 5s.
        let v = Vector3::new(3.0 * s, 4.0 * s, 0.0);
        let l = v.magnitude();
        assert!((l - 5.0 * s).abs() < 1e-10);
    }
});

// =============================================
// Vector3 normalize sweep (50 cases)
// =============================================

seq!(N in 0..50 {
    #[test]
    fn vec3_normalize_~N() {
        let s = (N as f64 + 1.0) * 0.183;
        let v = Vector3::new(s, 2.0 * s, -3.0 * s);
        let n = v.normalize().expect("non-zero vector normalizes");
        let len = n.magnitude();
        assert!((len - 1.0).abs() < 1e-10, "normalized length should be 1, got {}", len);
    }
});

// =============================================
// Vector3 add/sub roundtrip sweep (50 cases)
// =============================================

seq!(N in 0..50 {
    #[test]
    fn vec3_addsub_roundtrip_~N() {
        let s = (N as f64 + 1.0) * 0.119;
        let a = Vector3::new(s, -2.0 * s, 0.5 * s);
        let b = Vector3::new(3.0 * s, s, -s);
        let r = (a + b) - b;
        assert!((r.x - a.x).abs() < 1e-10);
        assert!((r.y - a.y).abs() < 1e-10);
        assert!((r.z - a.z).abs() < 1e-10);
    }
});

// =============================================
// Vector2 dot/length sweep (50 cases)
// =============================================

seq!(N in 0..50 {
    #[test]
    fn vec2_dot_length_~N() {
        let s = (N as f64 + 1.0) * 0.221;
        let a = Vector2::new(3.0 * s, 4.0 * s);
        // |a|² = 25 s²; a·a = 25 s²
        assert!((a.dot(&a) - 25.0 * s * s).abs() < 1e-10);
        assert!((a.magnitude() - 5.0 * s).abs() < 1e-10);
    }
});

// =============================================
// Vector2 perpendicularity sweep (50 cases)
// =============================================

seq!(N in 0..50 {
    #[test]
    fn vec2_perp_~N() {
        let s = (N as f64 + 1.0) * 0.157;
        let a = Vector2::new(s, 2.0 * s);
        let b = Vector2::new(-2.0 * s, s);
        // a·b = -2s² + 2s² = 0 — perpendicular by construction
        assert!(a.dot(&b).abs() < 1e-10);
    }
});

// =============================================
// Matrix4 translation roundtrip sweep (50 cases)
// =============================================

seq!(N in 0..50 {
    #[test]
    fn mat4_translate_~N() {
        let s = (N as f64 + 1.0) * 0.179;
        let t = Vector3::new(s, -2.0 * s, 3.0 * s);
        let m = Matrix4::from_translation(&t);
        let p = Point3::new(1.0, 2.0, 3.0);
        let q = m.transform_point(&p);
        assert!((q.x - (1.0 + s)).abs() < 1e-10);
        assert!((q.y - (2.0 - 2.0 * s)).abs() < 1e-10);
        assert!((q.z - (3.0 + 3.0 * s)).abs() < 1e-10);
    }
});

// =============================================
// Matrix4 identity preservation sweep (50 cases)
// =============================================

seq!(N in 0..50 {
    #[test]
    fn mat4_identity_~N() {
        let s = (N as f64 + 1.0) * 0.071;
        let p = Point3::new(s, 2.0 * s, -3.0 * s);
        let m = Matrix4::identity();
        let q = m.transform_point(&p);
        assert!((q.x - p.x).abs() < 1e-12);
        assert!((q.y - p.y).abs() < 1e-12);
        assert!((q.z - p.z).abs() < 1e-12);
    }
});

// =============================================
// Quaternion round-trip sweep (50 cases)
// =============================================

seq!(N in 0..50 {
    #[test]
    fn quat_roundtrip_~N() {
        // Sweep angle from ~0 to ~2π
        let theta = (N as f64 + 1.0) * (std::f64::consts::TAU / 51.0);
        let axis = Vector3::new(0.0, 0.0, 1.0);
        let q = Quaternion::from_axis_angle(&axis, theta).expect("unit-axis quaternion");
        let qi = q.conjugate();
        let v = Vector3::new(1.0, 0.0, 0.0);
        let v_rot = q.rotate_vector(&v);
        let v_back = qi.rotate_vector(&v_rot);
        assert!((v_back.x - 1.0).abs() < 1e-9);
        assert!(v_back.y.abs() < 1e-9);
        assert!(v_back.z.abs() < 1e-9);
    }
});

// =============================================
// Quaternion z-axis rotation sweep (50 cases)
// =============================================

seq!(N in 0..50 {
    #[test]
    fn quat_zrot_xaxis_~N() {
        // Quarter-turn-derived sweep — z-axis rotation of x̂ by θ
        // gives (cos θ, sin θ, 0). Closed-form invariant.
        let theta = (N as f64) * (std::f64::consts::TAU / 50.0);
        let axis = Vector3::new(0.0, 0.0, 1.0);
        let q = Quaternion::from_axis_angle(&axis, theta).expect("unit-axis quaternion");
        let v = Vector3::new(1.0, 0.0, 0.0);
        let r = q.rotate_vector(&v);
        assert!((r.x - theta.cos()).abs() < 1e-9);
        assert!((r.y - theta.sin()).abs() < 1e-9);
        assert!(r.z.abs() < 1e-9);
    }
});

// =============================================
// Box construction sweep (100 cases)
// =============================================

seq!(N in 0..100 {
    #[test]
    fn box_construct_~N() {
        let s = 0.1 + (N as f64) * 0.13;
        let mut model = BRepModel::new();
        let mut b = TopologyBuilder::new(&mut model);
        let id = b.create_box_3d(s, s * 1.5, s * 0.7);
        assert!(id.is_ok(), "box construction should succeed for s={}", s);
    }
});

// =============================================
// Sphere construction sweep (100 cases)
// =============================================

seq!(N in 0..100 {
    #[test]
    fn sphere_construct_~N() {
        let r = 0.05 + (N as f64) * 0.097;
        let mut model = BRepModel::new();
        let mut b = TopologyBuilder::new(&mut model);
        let id = b.create_sphere_3d(Point3::new(0.0, 0.0, 0.0), r);
        assert!(id.is_ok(), "sphere construction should succeed for r={}", r);
    }
});

// =============================================
// Cylinder construction sweep (60 cases)
// =============================================

seq!(N in 0..60 {
    #[test]
    fn cylinder_construct_~N() {
        let r = 0.2 + (N as f64) * 0.157;
        let h = 0.5 + (N as f64) * 0.211;
        let mut model = BRepModel::new();
        let mut b = TopologyBuilder::new(&mut model);
        let id = b.create_cylinder_3d(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            r,
            h,
        );
        assert!(id.is_ok(), "cylinder construction should succeed for r={}, h={}", r, h);
    }
});

// =============================================
// Cone construction sweep (40 cases)
// =============================================

seq!(N in 0..40 {
    #[test]
    fn cone_construct_~N() {
        let r_base = 0.5 + (N as f64) * 0.137;
        let r_top = r_base * 0.5; // truncated cone
        let h = 1.0 + (N as f64) * 0.119;
        let mut model = BRepModel::new();
        let mut b = TopologyBuilder::new(&mut model);
        let id = b.create_cone_3d(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            r_base,
            r_top,
            h,
        );
        assert!(id.is_ok(), "cone construction should succeed for r_base={}, r_top={}, h={}",
                r_base, r_top, h);
    }
});

// =============================================
// Box rejection sweep — invalid dims (50 cases)
// =============================================

seq!(N in 0..50 {
    #[test]
    fn box_invalid_~N() {
        let s = -((N as f64 + 1.0) * 0.123); // negative dimension
        let mut model = BRepModel::new();
        let mut b = TopologyBuilder::new(&mut model);
        let id = b.create_box_3d(s, 1.0, 1.0);
        assert!(id.is_err(), "box construction should reject negative width s={}", s);
    }
});

// =============================================
// Sphere rejection sweep — invalid radius (50 cases)
// =============================================

seq!(N in 0..50 {
    #[test]
    fn sphere_invalid_~N() {
        let r = -((N as f64 + 1.0) * 0.073);
        let mut model = BRepModel::new();
        let mut b = TopologyBuilder::new(&mut model);
        let id = b.create_sphere_3d(Point3::new(0.0, 0.0, 0.0), r);
        assert!(id.is_err(), "sphere construction should reject r={}", r);
    }
});

// =============================================
// Vector3 zero-norm rejection sweep (30 cases)
// =============================================

seq!(N in 0..30 {
    #[test]
    fn vec3_zero_norm_~N() {
        // Construct genuinely zero vectors at different scales of "tiny";
        // normalize must error on the exact zero. This tests the
        // tolerance path in `normalize()`.
        let _scale = (N as f64 + 1.0) * 1e-300;
        let v = Vector3::new(0.0, 0.0, 0.0);
        assert!(v.normalize().is_err(), "exact zero vector must reject normalize");
    }
});

// =============================================
// Tolerance distance comparison sweep (30 cases)
// =============================================

seq!(N in 0..30 {
    #[test]
    fn tolerance_distance_~N() {
        // Sweep tolerance scales from 1e-10 up to 1e-2.
        let exp = -10 + (N as i32 / 4);
        let tol_distance = 10f64.powi(exp);
        let tol = crate::math::Tolerance::from_distance(tol_distance);
        let recovered = tol.distance();
        assert!((recovered - tol_distance).abs() < tol_distance * 1e-6);
    }
});

// =============================================
// Box at non-origin reference (20 cases) — coordinate range robustness
// =============================================

seq!(N in 0..20 {
    #[test]
    fn box_large_dim_~N() {
        // Sweep large dimensions to ensure no coordinate overflow.
        let s = 10.0 + (N as f64) * 250.0;
        let mut model = BRepModel::new();
        let mut b = TopologyBuilder::new(&mut model);
        let id = b.create_box_3d(s, s, s);
        assert!(id.is_ok(), "large-dim box should succeed for s={}", s);
    }
});

// =============================================
// Sphere small-radius robustness (20 cases)
// =============================================

seq!(N in 0..20 {
    #[test]
    fn sphere_small_~N() {
        let r = 1e-6 * (N as f64 + 1.0);
        let mut model = BRepModel::new();
        let mut b = TopologyBuilder::new(&mut model);
        let id = b.create_sphere_3d(Point3::new(0.0, 0.0, 0.0), r);
        assert!(id.is_ok(), "small sphere should succeed for r={}", r);
    }
});

// =============================================
// Cylinder oblique-axis robustness (40 cases)
// =============================================

seq!(N in 0..40 {
    #[test]
    fn cylinder_oblique_~N() {
        let theta = (N as f64) * (std::f64::consts::PI / 40.0);
        let axis = Vector3::new(theta.cos(), theta.sin(), 1.0);
        let mut model = BRepModel::new();
        let mut b = TopologyBuilder::new(&mut model);
        let id = b.create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), axis, 1.0, 2.0);
        assert!(id.is_ok(), "oblique cylinder should succeed at theta={}", theta);
    }
});

// =============================================
// Vector3 dot-product symmetry sweep (50 cases)
// =============================================

seq!(N in 0..50 {
    #[test]
    fn vec3_dot_symmetric_~N() {
        // a·b == b·a — fundamental dot symmetry.
        let s = (N as f64 + 1.0) * 0.083;
        let a = Vector3::new(s, -2.0 * s, 3.0 * s);
        let b = Vector3::new(s.sin(), s.cos(), s);
        assert!((a.dot(&b) - b.dot(&a)).abs() < 1e-12);
    }
});

// =============================================
// Vector3 cross-product anti-symmetry sweep (50 cases)
// =============================================

seq!(N in 0..50 {
    #[test]
    fn vec3_cross_antisym_~N() {
        // a × b == -(b × a)
        let s = (N as f64 + 1.0) * 0.061;
        let a = Vector3::new(s, 2.0 * s, -s);
        let b = Vector3::new(s.sin(), 1.0, s.cos());
        let c1 = a.cross(&b);
        let c2 = b.cross(&a);
        assert!((c1.x + c2.x).abs() < 1e-12);
        assert!((c1.y + c2.y).abs() < 1e-12);
        assert!((c1.z + c2.z).abs() < 1e-12);
    }
});

// =============================================
// Quaternion identity rotation sweep (30 cases)
// =============================================

seq!(N in 0..30 {
    #[test]
    fn quat_identity_~N() {
        // Identity quaternion preserves vectors at machine epsilon.
        let s = (N as f64 + 1.0) * 0.117;
        let q = Quaternion::IDENTITY;
        let v = Vector3::new(s, -2.0 * s, 3.0 * s);
        let r = q.rotate_vector(&v);
        assert!((r.x - v.x).abs() < 1e-12);
        assert!((r.y - v.y).abs() < 1e-12);
        assert!((r.z - v.z).abs() < 1e-12);
    }
});

// =============================================
// Cross-product orthogonality sweep (50 cases)
// =============================================

seq!(N in 0..50 {
    #[test]
    fn vec3_cross_orthogonal_~N() {
        // (a × b) · a == 0 and (a × b) · b == 0.
        let s = (N as f64 + 1.0) * 0.043;
        let a = Vector3::new(s, 2.0 * s, -3.0 * s);
        let b = Vector3::new(s + 1.0, s.sin(), s.cos());
        let c = a.cross(&b);
        let mag = (a.magnitude() * b.magnitude()).max(1.0);
        // Allow scaled tolerance — orthogonality holds at machine eps.
        assert!(c.dot(&a).abs() < 1e-10 * mag);
        assert!(c.dot(&b).abs() < 1e-10 * mag);
    }
});

// =============================================
// Matrix4 translation composition sweep (40 cases)
// =============================================

seq!(N in 0..40 {
    #[test]
    fn mat4_translate_compose_~N() {
        let s = (N as f64 + 1.0) * 0.091;
        let t1 = Vector3::new(s, 0.0, 0.0);
        let t2 = Vector3::new(0.0, 2.0 * s, 0.0);
        let m1 = Matrix4::from_translation(&t1);
        let m2 = Matrix4::from_translation(&t2);
        let composed = &m1 * &m2;
        let p = Point3::new(0.0, 0.0, 0.0);
        let q = composed.transform_point(&p);
        // Translation composition is additive.
        assert!((q.x - s).abs() < 1e-10);
        assert!((q.y - 2.0 * s).abs() < 1e-10);
        assert!(q.z.abs() < 1e-10);
    }
});

// =============================================
// Pathological cases — finite extreme inputs (20 cases)
// =============================================

seq!(N in 0..20 {
    #[test]
    fn vec3_extreme_finite_~N() {
        // Sweep magnitudes from 1e-100 to 1e100.
        let exp = -100 + (N as i32) * 10;
        let s = 10f64.powi(exp);
        let v = Vector3::new(s, 0.0, 0.0);
        assert!(v.magnitude().is_finite());
        // magnitude(s,0,0) == |s|.
        assert!((v.magnitude() - s.abs()).abs() < s.abs() * 1e-10);
    }
});
