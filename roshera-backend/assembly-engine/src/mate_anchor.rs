//! Mate-anchor verification — the dimension that makes a FABRICATED mate a lie
//! the kernel can catch.
//!
//! Grounding (the mate graph) trusts the mates it's handed. That is a hole: a
//! mate declared against an INVENTED coordinate — an axis floating off in space,
//! a face point on nothing — still creates a graph edge, so a part hangs in air
//! while the certificate reports it "grounded". A design tool that can be talked
//! into certifying a part bolted to nothing certifies nothing at all.
//!
//! This closes the hole. Every mate feature must be ANCHORED to its part's real
//! geometry:
//!   * a FACE point must lie on the part's surface;
//!   * an AXIS must either pass through the part (a symmetry/bore axis) OR graze
//!     its surface (a port axis) — a real engine has both kinds, a fabricated
//!     axis has neither.
//! A feature that floats farther than `tol` off its part is reported, and the
//! certificate is not sound. The kernel cannot be told a part connects to a
//! coordinate that isn't there.

use crate::types::{Assembly, FeatureRef, Instance, InstanceId};
use parry3d_f64::na::{Point3, Vector3};
use parry3d_f64::query::PointQuery;
use parry3d_f64::shape::TriMesh;
use serde::{Deserialize, Serialize};

/// A mate feature that does not sit on the part it claims to attach to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnanchoredFeature {
    pub mate_index: usize,
    pub part: InstanceId,
    /// How far the feature floats off the part's geometry (the lie, measured).
    pub offset: f64,
}

/// The anchoring verdict for an assembly's mates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MateAnchorReport {
    pub unanchored: Vec<UnanchoredFeature>,
}

impl MateAnchorReport {
    /// True when every mate feature is anchored to its part — no fabricated mate.
    pub fn all_anchored(&self) -> bool {
        self.unanchored.is_empty()
    }
}

fn trimesh(instance: &Instance) -> Option<TriMesh> {
    if instance.mesh.vertices.len() < 4 || instance.mesh.triangles.is_empty() {
        return None;
    }
    let verts: Vec<Point3<f64>> = instance
        .mesh
        .vertices
        .iter()
        .map(|v| Point3::new(v[0], v[1], v[2]))
        .collect();
    Some(TriMesh::new(verts, instance.mesh.triangles.clone()))
}

fn centroid(instance: &Instance) -> Point3<f64> {
    let n = instance.mesh.vertices.len().max(1) as f64;
    let mut acc = Vector3::zeros();
    for v in &instance.mesh.vertices {
        acc += Vector3::new(v[0], v[1], v[2]);
    }
    Point3::from(acc / n)
}

/// Half the bounding-box diagonal — the scale over which to probe an axis.
fn bbox_reach(instance: &Instance) -> f64 {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for v in &instance.mesh.vertices {
        for k in 0..3 {
            lo[k] = lo[k].min(v[k]);
            hi[k] = hi[k].max(v[k]);
        }
    }
    let (dx, dy, dz) = (hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]);
    0.5 * (dx * dx + dy * dy + dz * dz).sqrt()
}

impl Assembly {
    /// For every mate, how far each feature floats off its part. A feature is
    /// anchored when its offset is `≤ tol`; the report lists the rest.
    pub fn mate_anchor_report(&self, tol: f64) -> MateAnchorReport {
        let mut unanchored = Vec::new();
        for (idx, mate) in self.mates.iter().enumerate() {
            for (part, feature) in [(mate.a, &mate.feature_a), (mate.b, &mate.feature_b)] {
                // The ground is the assembly's base frame — its features are
                // datums (the chamber's own axis), legitimately anywhere in that
                // frame. Every OTHER part's features must sit on real geometry.
                if part == self.ground {
                    continue;
                }
                if let Some(offset) = self.feature_offset(part, feature) {
                    if offset > tol {
                        unanchored.push(UnanchoredFeature {
                            mate_index: idx,
                            part,
                            offset,
                        });
                    }
                }
            }
        }
        MateAnchorReport { unanchored }
    }

    /// Distance a mate feature floats off its part's geometry (0 = on the part).
    /// Features and mesh are both in the part's LOCAL frame, so no pose is
    /// applied — the question is purely "does this feature sit on this part".
    fn feature_offset(&self, part: InstanceId, feature: &FeatureRef) -> Option<f64> {
        let instance = self.instance(part)?;
        let mesh = trimesh(instance)?;
        match feature {
            FeatureRef::Face { point, .. } => {
                // A face point must lie on the part's surface.
                let p = Point3::new(point[0], point[1], point[2]);
                Some(mesh.distance_to_local_point(&p, false))
            }
            FeatureRef::Axis { origin, direction } => {
                let o = Point3::new(origin[0], origin[1], origin[2]);
                let d =
                    Vector3::new(direction[0], direction[1], direction[2]).try_normalize(1e-12)?;
                let c = centroid(instance);
                // (a) a SYMMETRY/bore axis passes through the centroid.
                let foot = o + d * (c - o).dot(&d);
                let through = (c - foot).norm();
                // (b) a PORT axis grazes the surface — probe along the axis.
                let reach = bbox_reach(instance);
                let samples = 64;
                let mut graze = f64::INFINITY;
                for i in 0..=samples {
                    let t = -reach + 2.0 * reach * (i as f64) / (samples as f64);
                    let p = foot + d * t;
                    graze = graze.min(mesh.distance_to_local_point(&p, false));
                }
                // Anchored if EITHER holds; a fabricated axis satisfies neither.
                Some(through.min(graze))
            }
            // A connector FRAME is anchored if either its origin sits on the
            // part (a planar-face frame — origin on the face) OR its z line
            // behaves like an anchored axis (a bore frame — origin ON the
            // axis, off the surface). Reuse both probes and take the best;
            // a fabricated frame satisfies neither.
            FeatureRef::Frame { origin, z_axis, .. } => {
                let on_surface = {
                    let p = Point3::new(origin[0], origin[1], origin[2]);
                    mesh.distance_to_local_point(&p, false)
                };
                let as_axis = self.feature_offset(
                    part,
                    &FeatureRef::Axis {
                        origin: *origin,
                        direction: *z_axis,
                    },
                )?;
                Some(on_surface.min(as_axis))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Mate, MateKind, Mesh};

    fn cube(h: f64) -> Mesh {
        Mesh {
            vertices: vec![
                [-h, -h, -h],
                [h, -h, -h],
                [h, h, -h],
                [-h, h, -h],
                [-h, -h, h],
                [h, -h, h],
                [h, h, h],
                [-h, h, h],
            ],
            triangles: vec![
                [0, 2, 1],
                [0, 3, 2],
                [4, 5, 6],
                [4, 6, 7],
                [0, 1, 5],
                [0, 5, 4],
                [2, 3, 7],
                [2, 7, 6],
                [1, 2, 6],
                [1, 6, 5],
                [3, 0, 4],
                [3, 4, 7],
            ],
        }
    }

    fn block(id: u32) -> Instance {
        Instance::new(InstanceId(id), format!("block_{id}"), cube(1.0))
    }

    fn offset(part: u32, feature: FeatureRef) -> f64 {
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(block(part));
        assembly
            .feature_offset(InstanceId(part), &feature)
            .unwrap_or(-1.0)
    }

    #[test]
    fn symmetry_axis_through_the_part_is_anchored() {
        let o = offset(
            0,
            FeatureRef::Axis {
                origin: [0.0, 0.0, 0.0],
                direction: [0.0, 0.0, 1.0],
            },
        );
        assert!(o < 1e-6, "central axis is anchored, got {o}");
    }

    #[test]
    fn port_axis_grazing_a_face_is_anchored() {
        // Off-centre, but it runs along the +y face — a real port axis.
        let o = offset(
            0,
            FeatureRef::Axis {
                origin: [0.0, 1.0, 0.0],
                direction: [1.0, 0.0, 0.0],
            },
        );
        assert!(o < 1e-6, "port axis grazes the surface, got {o}");
    }

    #[test]
    fn axis_floating_off_the_part_is_unanchored() {
        // A vertical axis 5 units off to the side — anchored to nothing.
        let o = offset(
            0,
            FeatureRef::Axis {
                origin: [5.0, 0.0, 0.0],
                direction: [0.0, 0.0, 1.0],
            },
        );
        assert!(
            o > 3.5,
            "a floating axis must read far off the part, got {o}"
        );
    }

    #[test]
    fn face_point_on_the_surface_is_anchored() {
        let o = offset(
            0,
            FeatureRef::Face {
                point: [1.0, 0.0, 0.0],
                normal: [1.0, 0.0, 0.0],
            },
        );
        assert!(o < 1e-6, "a point on the +x face is anchored, got {o}");
    }

    #[test]
    fn face_point_off_the_part_is_unanchored() {
        let o = offset(
            0,
            FeatureRef::Face {
                point: [5.0, 0.0, 0.0],
                normal: [1.0, 0.0, 0.0],
            },
        );
        assert!(o > 3.5, "a point floating in space is unanchored, got {o}");
    }

    #[test]
    fn a_fabricated_mate_fails_the_anchor_report() {
        // HARNESS: two blocks, one honest concentric mate (shared central axis)
        // and one fabricated mate (an axis bolted to empty space). The report
        // must name exactly the fabricated one.
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(block(0));
        assembly.add_instance(block(1));
        let honest = Mate {
            kind: MateKind::Concentric,
            a: InstanceId(0),
            feature_a: FeatureRef::Axis {
                origin: [0.0, 0.0, 0.0],
                direction: [0.0, 0.0, 1.0],
            },
            b: InstanceId(1),
            feature_b: FeatureRef::Axis {
                origin: [0.0, 0.0, 0.0],
                direction: [0.0, 0.0, 1.0],
            },
        };
        let fabricated = Mate {
            kind: MateKind::Concentric,
            a: InstanceId(0),
            feature_a: FeatureRef::Axis {
                origin: [0.0, 0.0, 0.0],
                direction: [0.0, 0.0, 1.0],
            },
            b: InstanceId(1),
            feature_b: FeatureRef::Axis {
                origin: [9.0, 0.0, 0.0], // floating 9 units off part 1
                direction: [0.0, 0.0, 1.0],
            },
        };
        assembly.add_mate(honest);
        assembly.add_mate(fabricated);

        let report = assembly.mate_anchor_report(0.5);
        assert!(!report.all_anchored(), "the fabricated mate must be caught");
        assert_eq!(report.unanchored.len(), 1, "only the fabricated feature");
        assert_eq!(report.unanchored[0].mate_index, 1);
        assert_eq!(report.unanchored[0].part, InstanceId(1));
    }

    #[test]
    fn a_fully_honest_assembly_is_all_anchored() {
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(block(0));
        assembly.add_instance(block(1));
        assembly.add_mate(Mate {
            kind: MateKind::Concentric,
            a: InstanceId(0),
            feature_a: FeatureRef::Axis {
                origin: [0.0, 0.0, 0.0],
                direction: [0.0, 0.0, 1.0],
            },
            b: InstanceId(1),
            feature_b: FeatureRef::Axis {
                origin: [0.0, 0.0, 0.0],
                direction: [0.0, 0.0, 1.0],
            },
        });
        assert!(assembly.mate_anchor_report(0.5).all_anchored());
    }
}
