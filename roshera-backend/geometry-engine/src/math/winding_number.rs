//! Generalized winding number (GWN) for robust 3D point-in-solid
//! classification.
//!
//! # Why
//!
//! Boolean classification asks "is this point inside the other solid?".
//! The classic approach casts a ray and counts boundary crossings, or
//! tests a single interior point against a local face — both are fragile
//! at coincident/coplanar configurations and on meshes with small gaps
//! (the exact failure family behind the chained-union bug #27, where an
//! interior point landed inside a hole and misclassified).
//!
//! The **generalized winding number** (Jacobson, Kavan & Sorkine-Hornung,
//! *Robust Inside-Outside Segmentation using Generalized Winding Numbers*,
//! SIGGRAPH 2013) replaces the local test with a GLOBAL one: sum the
//! signed solid angle each oriented boundary triangle subtends at the
//! query point. For a point `p` and a closed, consistently-oriented
//! surface `S`,
//!
//! ```text
//!   w(p) = (1 / 4π) · Σ_{T ∈ S} Ω_T(p)
//! ```
//!
//! where `Ω_T(p)` is the signed solid angle of triangle `T` seen from `p`.
//! `w(p)` is exactly the integer topological winding number when `S` is
//! watertight (≈ ±1 inside, 0 outside), and degrades *gracefully* — not
//! catastrophically — when `S` has small holes or self-intersections,
//! because each triangle contributes independently and continuously.
//! This is what makes it robust where ray-casting and local interior
//! points are not.
//!
//! # Sign convention
//!
//! Triangles are taken with outward-facing orientation (vertices CCW
//! seen from outside the solid). With the solid-angle sign used here
//! (`A · (B × C)` in the numerator), a point strictly inside such a
//! surface evaluates to `w(p) ≈ +1`; a point outside to `w(p) ≈ 0`.
//! [`point_is_inside`] thresholds at `0.5`, the midpoint, which is the
//! standard robust cutoff.

use crate::math::Point3;

/// Signed solid angle (steradians) subtended by the oriented triangle
/// `(a, b, c)` as seen from the point `p`.
///
/// Uses the Van Oosterom–Strackee formula
/// (*The Solid Angle of a Plane Triangle*, IEEE Trans. Biomed. Eng.
/// BME-30(2):125–126, 1983):
///
/// ```text
///   tan(Ω / 2) = (A · (B × C))
///                / (|A||B||C| + (A·B)|C| + (B·C)|A| + (C·A)|B|)
/// ```
///
/// with `A = a − p`, `B = b − p`, `C = c − p`, evaluated via `atan2` so
/// the full signed range `(−2π, 2π]` is covered without quadrant loss.
/// Returns `0.0` when the point lies on a triangle vertex (degenerate
/// zero-length leg), which contributes nothing to the sum.
#[inline]
pub fn signed_solid_angle(p: &Point3, a: &Point3, b: &Point3, c: &Point3) -> f64 {
    let ax = a.x - p.x;
    let ay = a.y - p.y;
    let az = a.z - p.z;
    let bx = b.x - p.x;
    let by = b.y - p.y;
    let bz = b.z - p.z;
    let cx = c.x - p.x;
    let cy = c.y - p.y;
    let cz = c.z - p.z;

    let la = (ax * ax + ay * ay + az * az).sqrt();
    let lb = (bx * bx + by * by + bz * bz).sqrt();
    let lc = (cx * cx + cy * cy + cz * cz).sqrt();

    // Degenerate: query point coincides with a triangle vertex.
    if la == 0.0 || lb == 0.0 || lc == 0.0 {
        return 0.0;
    }

    // B × C
    let cross_x = by * cz - bz * cy;
    let cross_y = bz * cx - bx * cz;
    let cross_z = bx * cy - by * cx;

    // A · (B × C) — the scalar triple product (signed parallelepiped volume).
    let numerator = ax * cross_x + ay * cross_y + az * cross_z;

    let ab = ax * bx + ay * by + az * bz;
    let bc = bx * cx + by * cy + bz * cz;
    let ca = cx * ax + cy * ay + cz * az;

    let denominator = la * lb * lc + ab * lc + bc * la + ca * lb;

    // atan2 handles denominator == 0 (Ω = ±π) and the sign of Ω.
    2.0 * numerator.atan2(denominator)
}

/// Generalized winding number of `p` with respect to the oriented
/// triangle soup `triangles` (each `[a, b, c]` outward-facing).
///
/// Returns the sum of signed solid angles divided by `4π`. For a
/// watertight, consistently-outward surface this is ≈ +1 for interior
/// points and ≈ 0 for exterior points; intermediate values indicate the
/// point is near the surface or the surface has gaps.
pub fn generalized_winding_number(p: &Point3, triangles: &[[Point3; 3]]) -> f64 {
    let mut acc = 0.0;
    for [a, b, c] in triangles {
        acc += signed_solid_angle(p, a, b, c);
    }
    acc / (4.0 * std::f64::consts::PI)
}

/// Robust inside test: `true` when the generalized winding number rounds
/// to a non-zero integer (|w| > 0.5). This is stable for watertight
/// meshes and degrades gracefully on meshes with small holes — the
/// property ray-casting lacks.
#[inline]
pub fn point_is_inside(p: &Point3, triangles: &[[Point3; 3]]) -> bool {
    generalized_winding_number(p, triangles).abs() > 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 12 outward-facing triangles of the axis-aligned box [lo, hi]³.
    fn unit_box_tris(lo: f64, hi: f64) -> Vec<[Point3; 3]> {
        let v = |x: f64, y: f64, z: f64| Point3::new(x, y, z);
        // 8 corners
        let p000 = v(lo, lo, lo);
        let p100 = v(hi, lo, lo);
        let p110 = v(hi, hi, lo);
        let p010 = v(lo, hi, lo);
        let p001 = v(lo, lo, hi);
        let p101 = v(hi, lo, hi);
        let p111 = v(hi, hi, hi);
        let p011 = v(lo, hi, hi);
        // Each face CCW seen from OUTSIDE (outward normal).
        vec![
            // bottom z=lo (normal -Z): seen from below CCW
            [p000, p110, p100],
            [p000, p010, p110],
            // top z=hi (normal +Z)
            [p001, p101, p111],
            [p001, p111, p011],
            // front y=lo (normal -Y)
            [p000, p100, p101],
            [p000, p101, p001],
            // back y=hi (normal +Y)
            [p010, p111, p110],
            [p010, p011, p111],
            // left x=lo (normal -X)
            [p000, p001, p011],
            [p000, p011, p010],
            // right x=hi (normal +X)
            [p100, p110, p111],
            [p100, p111, p101],
        ]
    }

    #[test]
    fn solid_angle_of_full_sphere_surrogate_is_four_pi_inside() {
        // The total signed solid angle of a closed surface seen from an
        // interior point is ±4π. For an outward box and a centre point it
        // must have magnitude 4π → GWN magnitude 1.
        let tris = unit_box_tris(-1.0, 1.0);
        let w = generalized_winding_number(&Point3::new(0.0, 0.0, 0.0), &tris);
        assert!(
            (w.abs() - 1.0).abs() < 1e-9,
            "interior GWN magnitude ≈ 1, got {w}"
        );
    }

    #[test]
    fn inside_and_outside_classification() {
        let tris = unit_box_tris(-1.0, 1.0);
        // Clearly inside
        assert!(point_is_inside(&Point3::new(0.0, 0.0, 0.0), &tris));
        assert!(point_is_inside(&Point3::new(0.9, -0.9, 0.5), &tris));
        // Clearly outside
        assert!(!point_is_inside(&Point3::new(2.0, 0.0, 0.0), &tris));
        assert!(!point_is_inside(&Point3::new(0.0, 0.0, 5.0), &tris));
        assert!(!point_is_inside(&Point3::new(-3.0, -3.0, -3.0), &tris));
    }

    #[test]
    fn winding_is_near_zero_far_outside() {
        let tris = unit_box_tris(-1.0, 1.0);
        let w = generalized_winding_number(&Point3::new(100.0, 100.0, 100.0), &tris);
        assert!(w.abs() < 1e-3, "far exterior GWN ≈ 0, got {w}");
    }

    #[test]
    fn robust_to_a_small_hole() {
        // THE point of GWN: drop one triangle (a hole in the surface).
        // A ray cast through the hole would miscount, but the GWN of a
        // point far from the hole is still ≈ ±1 inside / 0 outside.
        let mut tris = unit_box_tris(-1.0, 1.0);
        tris.pop(); // remove one of the +X face triangles → open surface
                    // Interior point on the opposite (−X) side, far from the hole.
        let w_in = generalized_winding_number(&Point3::new(-0.8, 0.0, 0.0), &tris);
        assert!(
            w_in.abs() > 0.5,
            "interior still classified inside despite hole, got {w_in}"
        );
        // Exterior point on the −X side stays outside.
        let w_out = generalized_winding_number(&Point3::new(-2.0, 0.0, 0.0), &tris);
        assert!(
            w_out.abs() < 0.5,
            "exterior stays outside despite hole, got {w_out}"
        );
    }

    #[test]
    fn tetrahedron_inside_outside() {
        let a = Point3::new(0.0, 0.0, 0.0);
        let b = Point3::new(1.0, 0.0, 0.0);
        let c = Point3::new(0.0, 1.0, 0.0);
        let d = Point3::new(0.0, 0.0, 1.0);
        // Outward-facing tetra faces.
        let tris = vec![
            [a, c, b], // bottom z=0, normal -Z
            [a, b, d], // front y=0, normal -Y
            [a, d, c], // left x=0, normal -X
            [b, c, d], // slanted face, outward
        ];
        let centroid = Point3::new(0.2, 0.2, 0.2);
        assert!(point_is_inside(&centroid, &tris));
        assert!(!point_is_inside(&Point3::new(1.0, 1.0, 1.0), &tris));
    }

    #[test]
    fn sign_is_consistent_for_all_interior_points() {
        // GWN must have the SAME sign for every interior point (no flips),
        // which is what lets boolean classification threshold reliably.
        let tris = unit_box_tris(0.0, 10.0);
        let mut sign = 0.0;
        for &(x, y, z) in &[
            (1.0, 1.0, 1.0),
            (5.0, 5.0, 5.0),
            (9.0, 1.0, 9.0),
            (2.0, 8.0, 3.0),
        ] {
            let w = generalized_winding_number(&Point3::new(x, y, z), &tris);
            assert!(w.abs() > 0.5, "interior point ({x},{y},{z}) GWN {w}");
            if sign == 0.0 {
                sign = w.signum();
            } else {
                assert_eq!(
                    w.signum(),
                    sign,
                    "interior GWN sign flipped at ({x},{y},{z})"
                );
            }
        }
    }
}
