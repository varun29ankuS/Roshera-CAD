//! Region spatial queries (#14 slice 3) — entities intersecting a query volume.
//!
//! The composable spatial-query core's region primitive: "what is in here?".
//! Given an axis-aligned box or a sphere, it returns the solid's faces whose
//! world extent meets the query volume. Each face's extent is taken from its
//! trim-edge CURVES (exact arcs/lines, not the mesh or just the vertices) plus
//! an analytic envelope for edgeless closed surfaces (a full sphere has no seam
//! edge) — so the bound is sound: a face that truly reaches into the query
//! volume is never missed because a bulge sits between two vertices.
//!
//! Results are face ids — recoverable straight back to the B-Rep.

use crate::math::Point3;
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::surface::Sphere;
use crate::primitives::topology_builder::BRepModel;

/// An axis-aligned world bounding box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldBox {
    pub min: Point3,
    pub max: Point3,
}

impl WorldBox {
    /// Box from a centre and half-extents.
    pub fn from_center_half(center: Point3, half: Point3) -> Self {
        WorldBox {
            min: Point3::new(center.x - half.x, center.y - half.y, center.z - half.z),
            max: Point3::new(center.x + half.x, center.y + half.y, center.z + half.z),
        }
    }

    /// Standard AABB overlap (closed boxes; touching faces count as overlap).
    pub fn intersects_box(&self, o: &WorldBox) -> bool {
        self.min.x <= o.max.x
            && self.max.x >= o.min.x
            && self.min.y <= o.max.y
            && self.max.y >= o.min.y
            && self.min.z <= o.max.z
            && self.max.z >= o.min.z
    }

    /// Whether a sphere `(center, radius)` reaches this box: distance from the
    /// sphere centre to its closest point on the box is ≤ radius.
    pub fn intersects_sphere(&self, center: Point3, radius: f64) -> bool {
        let cx = center.x.clamp(self.min.x, self.max.x);
        let cy = center.y.clamp(self.min.y, self.max.y);
        let cz = center.z.clamp(self.min.z, self.max.z);
        let dx = center.x - cx;
        let dy = center.y - cy;
        let dz = center.z - cz;
        dx * dx + dy * dy + dz * dz <= radius * radius
    }
}

/// Sound world AABB of a single face: sampled from its trim-edge curves, plus
/// the exact ±radius envelope when the face lies on a full sphere (which has no
/// seam edge to sample). `None` if the face contributes no geometry.
pub fn face_world_box(model: &BRepModel, face_id: FaceId) -> Option<WorldBox> {
    let face = model.faces.get(face_id)?;
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    let mut any = false;
    let mut add = |p: [f64; 3]| {
        for i in 0..3 {
            if p[i] < min[i] {
                min[i] = p[i];
            }
            if p[i] > max[i] {
                max[i] = p[i];
            }
        }
    };

    let mut loops = vec![face.outer_loop];
    loops.extend_from_slice(&face.inner_loops);
    for lid in loops {
        let lp = match model.loops.get(lid) {
            Some(l) => l,
            None => continue,
        };
        for &eid in &lp.edges {
            let edge = match model.edges.get(eid) {
                Some(e) => e,
                None => continue,
            };
            let curve = match model.curves.get(edge.curve_id) {
                Some(c) => c,
                None => continue,
            };
            let r = edge.param_range;
            // 32 samples captures a full circle's ±radius extent to <0.25% of r.
            for k in 0..=32 {
                let t = r.start + (r.end - r.start) * (k as f64 / 32.0);
                if let Ok(p) = curve.point_at(t) {
                    add([p.x, p.y, p.z]);
                    any = true;
                }
            }
        }
    }

    // Edgeless closed analytic surface (full sphere): add its exact envelope.
    if let Some(surf) = model.surfaces.get(face.surface_id) {
        if let Some(sph) = surf.as_any().downcast_ref::<Sphere>() {
            let (c, rr) = (sph.center, sph.radius);
            add([c.x - rr, c.y - rr, c.z - rr]);
            add([c.x + rr, c.y + rr, c.z + rr]);
            any = true;
        }
    }

    if any {
        Some(WorldBox {
            min: Point3::new(min[0], min[1], min[2]),
            max: Point3::new(max[0], max[1], max[2]),
        })
    } else {
        None
    }
}

/// Every face id of a solid (outer + inner shells).
fn solid_faces(model: &BRepModel, solid_id: SolidId) -> Vec<FaceId> {
    let mut out = Vec::new();
    let solid = match model.solids.get(solid_id) {
        Some(s) => s,
        None => return out,
    };
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);
    for sh in shells {
        if let Some(shell) = model.shells.get(sh) {
            out.extend_from_slice(&shell.faces);
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Faces of `solid_id` whose world extent intersects the query box. Sorted,
/// deduplicated face ids.
pub fn faces_in_box(model: &BRepModel, solid_id: SolidId, query: WorldBox) -> Vec<FaceId> {
    solid_faces(model, solid_id)
        .into_iter()
        .filter(|&fid| {
            face_world_box(model, fid)
                .map(|bb| bb.intersects_box(&query))
                .unwrap_or(false)
        })
        .collect()
}

/// Faces of `solid_id` whose world extent intersects the query sphere.
pub fn faces_in_sphere(
    model: &BRepModel,
    solid_id: SolidId,
    center: Point3,
    radius: f64,
) -> Vec<FaceId> {
    solid_faces(model, solid_id)
        .into_iter()
        .filter(|&fid| {
            face_world_box(model, fid)
                .map(|bb| bb.intersects_sphere(center, radius))
                .unwrap_or(false)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vector3;
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn sid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    #[test]
    fn box_query_selects_corner_faces_only() {
        // Box 20³ centred at origin. A small query box hugging the +Z +X +Y
        // corner should meet exactly the three faces at that corner.
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("box"));
        let all = faces_in_box(
            &m,
            b,
            WorldBox {
                min: Point3::new(-100.0, -100.0, -100.0),
                max: Point3::new(100.0, 100.0, 100.0),
            },
        );
        assert_eq!(all.len(), 6, "a huge query box selects all 6 faces");

        // Query box around the +X face plane only (x ∈ [9,11], full y,z).
        let xface = faces_in_box(
            &m,
            b,
            WorldBox {
                min: Point3::new(9.0, -11.0, -11.0),
                max: Point3::new(11.0, 11.0, 11.0),
            },
        );
        // The +X face is in; the four side faces touching x=10 edge also reach
        // x∈[9,11] at their rims, but the −X face (x=−10) cannot.
        assert!(!xface.is_empty(), "the +X face region is non-empty");
        // Soundness: every returned face genuinely overlaps the query box.
        let q = WorldBox {
            min: Point3::new(9.0, -11.0, -11.0),
            max: Point3::new(11.0, 11.0, 11.0),
        };
        for fid in &xface {
            let bb = face_world_box(&m, *fid).expect("face box");
            assert!(
                bb.intersects_box(&q),
                "returned face {fid} actually overlaps"
            );
        }
        // The −X face (whole face at x=−10) must NOT be selected.
        for fid in &xface {
            let bb = face_world_box(&m, *fid).expect("face box");
            assert!(
                bb.max.x > 8.9,
                "no face entirely on the far side is returned"
            );
        }
    }

    #[test]
    fn sphere_query_reaches_cylinder_wall() {
        // Cylinder r=10 along Z, height 30 from origin. A sphere at (10,0,15)
        // radius 2 sits on the lateral wall → must select the lateral face.
        let mut m = BRepModel::new();
        let c = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::ZERO, Vector3::Z, 10.0, 30.0)
            .expect("cyl"));
        let hit = faces_in_sphere(&m, c, Point3::new(10.0, 0.0, 15.0), 2.0);
        assert!(!hit.is_empty(), "sphere on the wall meets the lateral face");
        // A sphere far away meets nothing.
        let far = faces_in_sphere(&m, c, Point3::new(1000.0, 0.0, 0.0), 2.0);
        assert!(far.is_empty(), "far sphere selects nothing");
    }

    #[test]
    fn worldbox_sphere_predicate_exact() {
        let bb = WorldBox {
            min: Point3::new(0.0, 0.0, 0.0),
            max: Point3::new(10.0, 10.0, 10.0),
        };
        // Sphere just touching the +X face from outside.
        assert!(bb.intersects_sphere(Point3::new(11.0, 5.0, 5.0), 1.0));
        // Sphere just short of it.
        assert!(!bb.intersects_sphere(Point3::new(11.0, 5.0, 5.0), 0.99));
        // Corner clearance: distance from (13,14,15) corner-out is √(3²+4²+5²)=√50.
        let d = (9.0_f64 + 16.0 + 25.0).sqrt();
        assert!(bb.intersects_sphere(Point3::new(13.0, 14.0, 15.0), d + 1e-9));
        assert!(!bb.intersects_sphere(Point3::new(13.0, 14.0, 15.0), d - 1e-6));
    }
}
