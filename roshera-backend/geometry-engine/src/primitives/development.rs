//! Isometric DEVELOPMENT of developable surfaces.
//!
//! A cylinder or cone has zero Gaussian curvature: it unrolls onto the plane
//! with lengths and angles preserved exactly. The native `(u, v)` chart does
//! not — one unit of `u` (an angle) is `r` long in 3D, and on a cone `r` varies
//! across the face — so anything that measures shape or distance in `(u, v)`
//! is distorted by it:
//!
//! * tessellation: a Delaunay triangulation that is well-shaped in `(u, v)`
//!   can be a 0.5° sliver in 3D whose facet normal is tens of degrees off the
//!   surface (measured on cone minus a square prism, 2026-11-05);
//! * surface–surface intersection: a marching step or a Newton correction
//!   sized in `(u, v)` is a different 3D length at every point of the face.
//!
//! In the development the 2D geometry IS the 3D geometry (up to the bending
//! the isometry removes), so both work in true lengths and angles. This module
//! is only the map: native ↔ development with its Jacobian, no consumer logic.
//!
//! Conventions (the kernel's own parametrisations):
//! * [`Cylinder`]: `u` = angle, `v` = height. `x = r·u`, `y = v`.
//! * [`Cone`]: `u` = angle, `v` = axial distance from the apex. Slant
//!   `ρ = v / cos α`, development angle `φ = u·sin α`;
//!   `x = ρ·cos φ`, `y = −ρ·sin φ` (the sign keeps the handedness of `(u, v)`:
//!   `det ∂(x, y)/∂(u, v) > 0`).
//!
//! A cone's development is single-valued only on a branch of `φ`: a chart is
//! built for a REGION of the surface (an unwrapped `u` range) whose `φ` span is
//! below 2π, and inverts on the branch around that region's centre. The apex
//! (`v = 0`) maps to the origin, where the inverse is singular, so a cone
//! region must stay on one nappe, clear of the apex.

use crate::math::{MathError, MathResult};
use crate::primitives::surface::{Cone, Cylinder, Surface};

/// The isometric development of one developable surface over one region.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Development {
    kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Cylinder {
        radius: f64,
    },
    Cone {
        sin_a: f64,
        cos_a: f64,
        phi_ref: f64,
    },
}

impl Development {
    /// The development of `surface` over the region its native `(u, v)`
    /// samples `region` span (`u` already periodicity-unwrapped, so the
    /// samples trace the region continuously). `None` for a surface that is
    /// not an analytic cylinder or cone, or a cone region that reaches the
    /// apex / the far nappe or wraps more than one development turn.
    pub fn for_region(surface: &dyn Surface, region: &[(f64, f64)]) -> Option<Self> {
        if region.is_empty() {
            return None;
        }
        if let Some(cyl) = surface.as_any().downcast_ref::<Cylinder>() {
            return (cyl.radius > 0.0).then_some(Self {
                kind: Kind::Cylinder { radius: cyl.radius },
            });
        }
        let cone = surface.as_any().downcast_ref::<Cone>()?;
        let (sin_a, cos_a) = cone.half_angle.sin_cos();
        if !(sin_a > 0.0 && cos_a > 0.0) || region.iter().any(|&(_, v)| !(v > 0.0)) {
            return None;
        }
        let (lo, hi) = region
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &(u, _)| {
                (lo.min(u), hi.max(u))
            });
        if !((hi - lo) * sin_a < std::f64::consts::TAU) {
            return None;
        }
        Some(Self {
            kind: Kind::Cone {
                sin_a,
                cos_a,
                phi_ref: 0.5 * (lo + hi) * sin_a,
            },
        })
    }

    /// Native `(u, v)` → development `(x, y)`.
    pub fn to_development(&self, u: f64, v: f64) -> (f64, f64) {
        match self.kind {
            Kind::Cylinder { radius } => (radius * u, v),
            Kind::Cone { sin_a, cos_a, .. } => {
                let rho = v / cos_a;
                let phi = u * sin_a;
                (rho * phi.cos(), -rho * phi.sin())
            }
        }
    }

    /// Development `(x, y)` → native `(u, v)` on this region's branch, with
    /// the Jacobian `∂(u, v)/∂(x, y)` as `[[u_x, u_y], [v_x, v_y]]`. Errors at
    /// a cone's apex, which has no unique parameter.
    pub fn to_native(&self, x: f64, y: f64) -> MathResult<((f64, f64), [[f64; 2]; 2])> {
        match self.kind {
            Kind::Cylinder { radius } => Ok(((x / radius, y), [[1.0 / radius, 0.0], [0.0, 1.0]])),
            Kind::Cone {
                sin_a,
                cos_a,
                phi_ref,
            } => {
                let rho2 = x * x + y * y;
                if !(rho2 > 0.0) {
                    return Err(MathError::InvalidParameter(
                        "cone development: the apex has no unique parameter".into(),
                    ));
                }
                let rho = rho2.sqrt();
                // φ = atan2(−y, x), on the branch around phi_ref.
                let pi = std::f64::consts::PI;
                let raw = (-y).atan2(x);
                let phi = phi_ref + (raw - phi_ref + pi).rem_euclid(std::f64::consts::TAU) - pi;
                // ρ_x = x/ρ, ρ_y = y/ρ, φ_x = y/ρ², φ_y = −x/ρ²;
                // u = φ / sin α, v = ρ·cos α.
                let jac = [
                    [(y / rho2) / sin_a, (-x / rho2) / sin_a],
                    [(x / rho) * cos_a, (y / rho) * cos_a],
                ];
                Ok(((phi / sin_a, rho * cos_a), jac))
            }
        }
    }

    /// Native `(u, v)` → CONFORMAL chart `(a, b)`: angles exact, the surface's
    /// straight RULINGS on the `b` axis (iso-`a` lines).
    ///
    /// * Cylinder: the development itself, `(r·u, v)` (isometric ⇒ conformal).
    /// * Cone: log-polar of the development, `a = φ = u·sin α`,
    ///   `b = ln ρ = ln(v / cos α)` — since `ds² = dρ² + ρ²dφ² =
    ///   ρ²(d(ln ρ)² + dφ²)`, the chart is conformal with local scale `ρ`, and
    ///   a ruling (fixed `u`) is a vertical line.
    ///
    /// Use this where SHAPE matters (meshing: Delaunay maximises true angles,
    /// and the ruling direction stays a chart axis, so straight-direction
    /// economies still apply); use [`Self::to_development`] where LENGTH
    /// matters.
    pub fn to_conformal(&self, u: f64, v: f64) -> (f64, f64) {
        match self.kind {
            Kind::Cylinder { radius } => (radius * u, v),
            Kind::Cone { sin_a, cos_a, .. } => (u * sin_a, (v / cos_a).ln()),
        }
    }

    /// Conformal `(a, b)` → native `(u, v)`, with `∂(u, v)/∂(a, b)` as
    /// `[[u_a, u_b], [v_a, v_b]]`. Single-valued (no branch: `a` is linear in
    /// the unwrapped `u`).
    pub fn from_conformal(&self, a: f64, b: f64) -> ((f64, f64), [[f64; 2]; 2]) {
        match self.kind {
            Kind::Cylinder { radius } => ((a / radius, b), [[1.0 / radius, 0.0], [0.0, 1.0]]),
            Kind::Cone { sin_a, cos_a, .. } => {
                let v = b.exp() * cos_a;
                ((a / sin_a, v), [[1.0 / sin_a, 0.0], [0.0, v]])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::math::{Point3, Vector3};

    /// The map must be an ISOMETRY: chart distances equal 3D distances for
    /// nearby points, in every direction — that is the whole point of it.
    fn assert_isometric(surface: &dyn Surface, dev: &Development, samples: &[(f64, f64)]) {
        for &(u, v) in samples {
            let (x, y) = dev.to_development(u, v);
            for (dx, dy) in [(1e-4, 0.0), (0.0, 1e-4), (7e-5, -7e-5)] {
                let ((u0, v0), _) = dev.to_native(x, y).unwrap();
                let ((u1, v1), _) = dev.to_native(x + dx, y + dy).unwrap();
                let p0 = surface.point_at(u0, v0).unwrap();
                let p1 = surface.point_at(u1, v1).unwrap();
                let d3 = (p1 - p0).magnitude();
                let d2 = (dx * dx + dy * dy as f64).sqrt();
                assert!(
                    (d3 - d2).abs() < 1e-6 * d2.max(1e-12) + 1e-12,
                    "not isometric at (u,v)=({u},{v}) dir=({dx},{dy}): 3D {d3:e} vs 2D {d2:e}"
                );
            }
            // Round trip.
            let ((ur, vr), _) = dev.to_native(x, y).unwrap();
            assert!((ur - u).abs() < 1e-9 && (vr - v).abs() < 1e-9, "round trip");
        }
    }

    #[test]
    fn cylinder_development_is_an_isometry() {
        let cyl = Cylinder::new(Point3::ORIGIN, Vector3::Z, 4.0).expect("cylinder");
        let region = [(0.3, 1.0), (2.0, 3.0), (5.9, -2.0)];
        let dev = Development::for_region(&cyl, &region).expect("cylinder develops");
        assert_isometric(&cyl, &dev, &region);
    }

    #[test]
    fn cone_development_is_an_isometry_including_past_the_seam() {
        let cone = Cone::new(Point3::new(0.0, 0.0, 10.0), -Vector3::Z, 0.6).expect("cone");
        // An unwrapped region straddling u = 2π.
        let region = [(5.5, 2.0), (6.2, 4.0), (7.0, 8.0), (6.5, 6.0)];
        let dev = Development::for_region(&cone, &region).expect("cone develops");
        assert_isometric(&cone, &dev, &region);
    }

    #[test]
    fn cone_development_declines_the_apex_and_a_full_turn_too_many() {
        let cone = Cone::new(Point3::ORIGIN, Vector3::Z, 1.4).expect("wide cone");
        assert!(Development::for_region(&cone, &[(0.0, 0.0), (1.0, 2.0)]).is_none());
        // sin(1.4) ≈ 0.985: a 7-rad span develops past 2π.
        assert!(Development::for_region(&cone, &[(0.0, 1.0), (7.0, 2.0)]).is_none());
    }

    /// The conformal chart must preserve ANGLES (an isotropic local scale) and
    /// put the cone's rulings on the `b` axis.
    #[test]
    fn cone_conformal_chart_preserves_angles_and_aligns_rulings() {
        let cone = Cone::new(Point3::new(0.0, 0.0, 10.0), -Vector3::Z, 0.6).expect("cone");
        let dev = Development::for_region(&cone, &[(5.5, 2.0), (7.0, 8.0)]).expect("dev");
        for &(u, v) in &[(5.6, 2.5), (6.3, 5.0), (6.9, 7.5)] {
            let (a, b) = dev.to_conformal(u, v);
            let ((ur, vr), _) = dev.from_conformal(a, b);
            assert!((ur - u).abs() < 1e-9 && (vr - v).abs() < 1e-9, "round trip");
            // Two orthogonal chart steps of equal length must be orthogonal,
            // equal-length steps in 3D (conformal = isotropic local scale).
            let h = 1e-5;
            let p0 = cone.point_at(u, v).unwrap();
            let step = |da: f64, db: f64| {
                let ((uu, vv), _) = dev.from_conformal(a + da, b + db);
                cone.point_at(uu, vv).unwrap() - p0
            };
            let (ea, eb) = (step(h, 0.0), step(0.0, h));
            let cos = ea.dot(&eb) / (ea.magnitude() * eb.magnitude());
            let ratio = ea.magnitude() / eb.magnitude();
            assert!(
                cos.abs() < 1e-4,
                "chart axes not orthogonal in 3D: cos {cos}"
            );
            assert!((ratio - 1.0).abs() < 1e-4, "scale not isotropic: {ratio}");
            // A b-step is along the ruling: it points away from the apex.
            let ruling = (p0 - cone.apex).normalize().unwrap();
            assert!(
                eb.normalize().unwrap().dot(&ruling) > 1.0 - 1e-6,
                "b axis is the ruling"
            );
        }
    }

    #[test]
    fn the_jacobian_matches_finite_differences() {
        let cone = Cone::new(Point3::ORIGIN, Vector3::Z, 0.5).expect("cone");
        let dev = Development::for_region(&cone, &[(1.0, 3.0), (2.0, 5.0)]).expect("dev");
        let (x, y) = dev.to_development(1.4, 4.0);
        let ((u, v), j) = dev.to_native(x, y).unwrap();
        let h = 1e-6;
        let ((ux, vx), _) = dev.to_native(x + h, y).unwrap();
        let ((uy, vy), _) = dev.to_native(x, y + h).unwrap();
        let fd = [[(ux - u) / h, (uy - u) / h], [(vx - v) / h, (vy - v) / h]];
        for r in 0..2 {
            for c in 0..2 {
                assert!(
                    (j[r][c] - fd[r][c]).abs() < 1e-5,
                    "J[{r}][{c}] {} vs {}",
                    j[r][c],
                    fd[r][c]
                );
            }
        }
    }
}
