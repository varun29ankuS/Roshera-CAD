//! Analytic ray-cast (#12 slice 1 / #14 ray primitive) — the sound visual
//! channel's foundation.
//!
//! A ray is intersected with each face's ANALYTIC surface (exact quadratics for
//! plane/cylinder/sphere/cone), and every candidate hit is clipped to the
//! face's real trim loops (`point_inside_face_uv`). The nearest surviving hit is
//! returned with the exact world point, the oriented surface normal, the
//! distance, and the FACE ID — so a pixel/probe resolves to `(entity,
//! world-xyz, normal)`, never an approximation off a mesh.
//!
//! Defect-revealing by construction: if a face is missing, no surface is there
//! to hit, so the ray passes through to whatever is behind (or nothing). A hole
//! in the B-Rep renders as see-through — the eye cannot report a surface that
//! is not in the model.

use crate::math::{Point3, Vector3};
use crate::primitives::face::{Face, FaceId, FaceOrientation};
use crate::primitives::solid::SolidId;
use crate::primitives::surface::{Cone, Cylinder, Plane, Sphere, Surface};
use crate::primitives::topology_builder::BRepModel;

/// One ray–solid intersection, fully recoverable to the B-Rep.
#[derive(Debug, Clone)]
pub struct RayHit {
    pub face_id: FaceId,
    /// Exact world-space hit point (on the analytic surface).
    pub point: Point3,
    /// Outward-oriented surface normal at the hit (face orientation applied).
    pub normal: Vector3,
    /// Ray parameter `t` (distance along a unit `direction`).
    pub distance: f64,
}

const EPS: f64 = 1e-7;

/// Cast a ray from `origin` along `direction` (need not be unit; distances are
/// in `direction` units) and return the NEAREST hit on `solid_id`, or `None`.
pub fn raycast_solid(
    model: &BRepModel,
    solid_id: SolidId,
    origin: Point3,
    direction: Vector3,
) -> Option<RayHit> {
    let dir = direction.normalize().ok()?;
    let solid = model.solids.get(solid_id)?;
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);

    let mut best: Option<RayHit> = None;
    for shell_id in shells {
        let shell = match model.shells.get(shell_id) {
            Some(s) => s,
            None => continue,
        };
        for &face_id in &shell.faces {
            let face = match model.faces.get(face_id) {
                Some(f) => f,
                None => continue,
            };
            let surface = match model.surfaces.get(face.surface_id) {
                Some(s) => s,
                None => continue,
            };
            for t in surface_ray_ts(surface, origin, dir) {
                if t <= EPS {
                    continue;
                }
                let p = Point3::new(
                    origin.x + dir.x * t,
                    origin.y + dir.y * t,
                    origin.z + dir.z * t,
                );
                // Clip to the face's real trim loops (handles caps / height
                // limits / holes), not just the infinite surface.
                let (u, v) = match surface.closest_point(&p, model.tolerance()) {
                    Ok(uv) => uv,
                    Err(_) => continue,
                };
                if !crate::tessellation::surface::point_inside_face_uv(u, v, face, model) {
                    continue;
                }
                if best.as_ref().map(|b| t < b.distance).unwrap_or(true) {
                    let n = oriented_normal(surface, face, u, v);
                    best = Some(RayHit {
                        face_id,
                        point: p,
                        normal: n,
                        distance: t,
                    });
                }
            }
        }
    }
    best
}

/// ALL ray–solid hits along the ray (every face crossing), sorted near→far.
/// Used for point-in-solid parity and multi-hit field queries.
pub fn raycast_all(
    model: &BRepModel,
    solid_id: SolidId,
    origin: Point3,
    direction: Vector3,
) -> Vec<RayHit> {
    let dir = match direction.normalize() {
        Ok(d) => d,
        Err(_) => return vec![],
    };
    let solid = match model.solids.get(solid_id) {
        Some(s) => s,
        None => return vec![],
    };
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);

    let mut hits = Vec::new();
    for shell_id in shells {
        let shell = match model.shells.get(shell_id) {
            Some(s) => s,
            None => continue,
        };
        for &face_id in &shell.faces {
            let face = match model.faces.get(face_id) {
                Some(f) => f,
                None => continue,
            };
            let surface = match model.surfaces.get(face.surface_id) {
                Some(s) => s,
                None => continue,
            };
            for t in surface_ray_ts(surface, origin, dir) {
                if t <= EPS {
                    continue;
                }
                let p = Point3::new(
                    origin.x + dir.x * t,
                    origin.y + dir.y * t,
                    origin.z + dir.z * t,
                );
                let (u, v) = match surface.closest_point(&p, model.tolerance()) {
                    Ok(uv) => uv,
                    Err(_) => continue,
                };
                if !crate::tessellation::surface::point_inside_face_uv(u, v, face, model) {
                    continue;
                }
                let n = oriented_normal(surface, face, u, v);
                hits.push(RayHit {
                    face_id,
                    point: p,
                    normal: n,
                    distance: t,
                });
            }
        }
    }
    hits.sort_by(|a, b| {
        a.distance
            .partial_cmp(&b.distance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits
}

fn oriented_normal(surface: &dyn Surface, face: &Face, u: f64, v: f64) -> Vector3 {
    let n = surface.normal_at(u, v).unwrap_or(Vector3::Z);
    let s = match face.orientation {
        FaceOrientation::Forward => 1.0,
        FaceOrientation::Backward => -1.0,
    };
    (n * s).normalize().unwrap_or(n)
}

/// Ray-parameter candidates for the ray ∩ a face's analytic surface. Returns
/// the (possibly two) `t` values on the INFINITE surface; the caller clips to
/// the face trim. Non-analytic surfaces return none (handled by the mesh path).
fn surface_ray_ts(surface: &dyn Surface, o: Point3, d: Vector3) -> Vec<f64> {
    let any = surface.as_any();
    if let Some(pl) = any.downcast_ref::<Plane>() {
        let denom = d.dot(&pl.normal);
        if denom.abs() < EPS {
            return vec![];
        }
        let t = (pl.origin - o).dot(&pl.normal) / denom;
        return vec![t];
    }
    if let Some(sp) = any.downcast_ref::<Sphere>() {
        let oc = o - sp.center;
        return solve_quadratic(
            d.dot(&d),
            2.0 * oc.dot(&d),
            oc.dot(&oc) - sp.radius * sp.radius,
        );
    }
    if let Some(cy) = any.downcast_ref::<Cylinder>() {
        let a = cy.axis.normalize().unwrap_or(cy.axis);
        let w = o - cy.origin;
        let dp = d - a * d.dot(&a);
        let wp = w - a * w.dot(&a);
        return solve_quadratic(
            dp.dot(&dp),
            2.0 * dp.dot(&wp),
            wp.dot(&wp) - cy.radius * cy.radius,
        );
    }
    if let Some(co) = any.downcast_ref::<Cone>() {
        let a = co.axis.normalize().unwrap_or(co.axis);
        let cos2 = co.half_angle.cos() * co.half_angle.cos();
        let co_v = o - co.apex;
        let da = d.dot(&a);
        let ca = co_v.dot(&a);
        let qa = da * da - cos2 * d.dot(&d);
        let qb = 2.0 * (da * ca - cos2 * d.dot(&co_v));
        let qc = ca * ca - cos2 * co_v.dot(&co_v);
        // Keep only the nappe opening along +axis (radius increases with `a`).
        return solve_quadratic(qa, qb, qc)
            .into_iter()
            .filter(|&t| {
                let p = o + d * t;
                (p - co.apex).dot(&a) >= 0.0
            })
            .collect();
    }
    vec![]
}

/// Real roots of `a t² + b t + c = 0`. Handles the linear (a≈0) case.
fn solve_quadratic(a: f64, b: f64, c: f64) -> Vec<f64> {
    if a.abs() < 1e-12 {
        if b.abs() < 1e-12 {
            return vec![];
        }
        return vec![-c / b];
    }
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return vec![];
    }
    let sq = disc.sqrt();
    vec![(-b - sq) / (2.0 * a), (-b + sq) / (2.0 * a)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn sid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    #[test]
    fn ray_hits_box_top_face_exactly() {
        // Box 20×20×20 centred at origin (z in [-10, 10]). A ray straight down
        // from above must hit the +Z top face at z=10, normal +Z, t=10.
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("box"));
        let hit = raycast_solid(
            &m,
            b,
            Point3::new(0.0, 0.0, 20.0),
            Vector3::new(0.0, 0.0, -1.0),
        )
        .expect("ray must hit the box");
        assert!((hit.point.z - 10.0).abs() < 1e-6, "hit z = {}", hit.point.z);
        assert!((hit.distance - 10.0).abs() < 1e-6, "t = {}", hit.distance);
        assert!(
            hit.normal.z > 0.999,
            "top face normal points +Z: {:?}",
            hit.normal
        );
    }

    #[test]
    fn ray_hits_sphere_at_exact_radius_with_radial_normal() {
        let mut m = BRepModel::new();
        let s = sid(TopologyBuilder::new(&mut m)
            .create_sphere_3d(Point3::ZERO, 15.0)
            .expect("sphere"));
        let hit = raycast_solid(
            &m,
            s,
            Point3::new(40.0, 0.0, 0.0),
            Vector3::new(-1.0, 0.0, 0.0),
        )
        .expect("ray must hit sphere");
        assert!(
            (hit.point.x - 15.0).abs() < 1e-6,
            "hit at +X radius: {}",
            hit.point.x
        );
        assert!(
            hit.normal.x > 0.999,
            "sphere normal radial-out (+X): {:?}",
            hit.normal
        );
        assert!((hit.distance - 25.0).abs() < 1e-6, "t = 40-15 = 25");
    }

    #[test]
    fn ray_hits_cylinder_wall_exactly() {
        let mut m = BRepModel::new();
        let c = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::ZERO, Vector3::Z, 10.0, 40.0)
            .expect("cyl"));
        // Inbound along -Y at mid-height → near wall at y=10 (angle 90°, away
        // from the +X seam). NOTE: a ray that hits exactly at the seam (u=0)
        // is currently rejected by the winding trim test and reports the far
        // wall — a seam-grazing caveat pinned for the #13 soundness harness.
        let hit = raycast_solid(
            &m,
            c,
            Point3::new(0.0, 30.0, 20.0),
            Vector3::new(0.0, -1.0, 0.0),
        )
        .expect("ray hits cylinder wall");
        assert!(
            (hit.point.y - 10.0).abs() < 1e-6,
            "wall at y=10: {}",
            hit.point.y
        );
        assert!(
            hit.normal.y > 0.999,
            "wall normal radial-out (+Y): {:?}",
            hit.normal
        );
        assert!((hit.distance - 20.0).abs() < 1e-6, "t = 30-10 = 20");
    }

    #[test]
    fn missing_face_renders_see_through() {
        // THE soundness property: drop the top face → a downward ray no longer
        // hits at z=10. It either passes through to the bottom cap (z=-10) or
        // misses — but it must NEVER report a surface that isn't in the model.
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("box"));
        // remove the face the downward ray would hit first (the +Z top).
        let top = raycast_solid(
            &m,
            b,
            Point3::new(0.0, 0.0, 20.0),
            Vector3::new(0.0, 0.0, -1.0),
        )
        .expect("hit before removal")
        .face_id;
        let shell_id = m.solids.get(b).expect("solid").outer_shell;
        if let Some(shell) = m.shells.get_mut(shell_id) {
            shell.faces.retain(|&f| f != top);
        }
        let hit = raycast_solid(
            &m,
            b,
            Point3::new(0.0, 0.0, 20.0),
            Vector3::new(0.0, 0.0, -1.0),
        );
        match hit {
            None => {}
            Some(h) => {
                assert_ne!(h.face_id, top, "must not hit the removed face");
                assert!(
                    h.point.z < 0.0,
                    "see-through: next hit is the bottom cap (z≈-10), got z={}",
                    h.point.z
                );
            }
        }
    }
}
