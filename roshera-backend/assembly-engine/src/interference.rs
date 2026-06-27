//! Static interference + clearance via Parry.
//!
//! Each instance's kernel tessellation becomes a Parry `TriMesh` in its LOCAL
//! frame; the instance pose is the world isometry. Pairwise, Parry answers
//! whether two parts overlap (a build error — interference) and, where
//! supported, their separation. Broad-phase BVH pruning and swept CCD arrive in
//! later slices; this is the correctness slice.

use crate::types::{Assembly, Instance, InstanceId};
use parry3d_f64::na::{Isometry3, Point3, Quaternion, Translation3, UnitQuaternion};
use parry3d_f64::query;
use parry3d_f64::shape::TriMesh;

/// Two instances found overlapping in world space.
#[derive(Debug, Clone, PartialEq)]
pub struct InterferencePair {
    pub a: InstanceId,
    pub b: InstanceId,
    /// Best-effort separation at detection (≤ 0 ⇒ overlapping). `None` when the
    /// exact distance is unsupported for the shape pair — the overlap is still
    /// reported.
    pub clearance: Option<f64>,
}

/// The static-interference verdict for an assembly.
#[derive(Debug, Clone, PartialEq)]
pub struct InterferenceReport {
    pub interfering: Vec<InterferencePair>,
}

impl InterferenceReport {
    /// True when no two parts overlap.
    pub fn no_static_interference(&self) -> bool {
        self.interfering.is_empty()
    }
}

/// World isometry of an instance (`translation` + unit quaternion `[x, y, z, w]`).
fn instance_isometry(instance: &Instance) -> Isometry3<f64> {
    let translation = Translation3::new(
        instance.translation[0],
        instance.translation[1],
        instance.translation[2],
    );
    // nalgebra's `Quaternion::new` is (w, i, j, k); our storage is [x, y, z, w].
    let quaternion = Quaternion::new(
        instance.rotation[3],
        instance.rotation[0],
        instance.rotation[1],
        instance.rotation[2],
    );
    Isometry3::from_parts(translation, UnitQuaternion::from_quaternion(quaternion))
}

/// Build the instance's mesh as a Parry `TriMesh` (local frame). `None` when the
/// mesh is empty or Parry rejects it as degenerate.
fn instance_trimesh(instance: &Instance) -> Option<TriMesh> {
    if instance.mesh.vertices.is_empty() || instance.mesh.triangles.is_empty() {
        return None;
    }
    let vertices: Vec<Point3<f64>> = instance
        .mesh
        .vertices
        .iter()
        .map(|v| Point3::new(v[0], v[1], v[2]))
        .collect();
    Some(TriMesh::new(vertices, instance.mesh.triangles.clone()))
}

impl Assembly {
    /// Pairwise static interference across the assembly. O(n²) for now — broad-
    /// phase BVH pruning is a later slice.
    pub fn interference_report(&self) -> InterferenceReport {
        let prepared: Vec<(InstanceId, Isometry3<f64>, Option<TriMesh>)> = self
            .instances
            .iter()
            .map(|instance| {
                (
                    instance.id,
                    instance_isometry(instance),
                    instance_trimesh(instance),
                )
            })
            .collect();

        let mut interfering = Vec::new();
        for i in 0..prepared.len() {
            for j in (i + 1)..prepared.len() {
                let (Some((id_a, pos_a, mesh_a)), Some((id_b, pos_b, mesh_b))) =
                    (prepared.get(i), prepared.get(j))
                else {
                    continue;
                };
                let (Some(ma), Some(mb)) = (mesh_a, mesh_b) else {
                    continue; // a mesh-less instance cannot interfere geometrically
                };
                if query::intersection_test(pos_a, ma, pos_b, mb).unwrap_or(false) {
                    interfering.push(InterferencePair {
                        a: *id_a,
                        b: *id_b,
                        clearance: query::distance(pos_a, ma, pos_b, mb).ok(),
                    });
                }
            }
        }
        InterferenceReport { interfering }
    }

    /// Best-effort separation between two instances (positive ⇒ a gap, 0 ⇒
    /// touching/overlapping). `None` when a mesh is missing or the exact
    /// distance is unsupported for the pair.
    pub fn clearance(&self, a: InstanceId, b: InstanceId) -> Option<f64> {
        let instance_a = self.instance(a)?;
        let instance_b = self.instance(b)?;
        let mesh_a = instance_trimesh(instance_a)?;
        let mesh_b = instance_trimesh(instance_b)?;
        query::distance(
            &instance_isometry(instance_a),
            &mesh_a,
            &instance_isometry(instance_b),
            &mesh_b,
        )
        .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Mesh;

    /// An axis-aligned cube of side `2*h` centred at the origin (local frame).
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

    fn cube_at(id: u32, h: f64, x: f64) -> Instance {
        let mut instance = Instance::new(InstanceId(id), format!("cube_{id}"), cube(h));
        instance.translation = [x, 0.0, 0.0];
        instance
    }

    #[test]
    fn overlapping_parts_interfere() {
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(cube_at(0, 1.0, 0.0)); // x in [-1, 1]
        assembly.add_instance(cube_at(1, 1.0, 0.5)); // x in [-0.5, 1.5] — overlaps
        let report = assembly.interference_report();
        assert!(!report.no_static_interference());
        assert_eq!(report.interfering.len(), 1);
    }

    #[test]
    fn separated_parts_are_clear() {
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(cube_at(0, 1.0, 0.0)); // x in [-1, 1]
        assembly.add_instance(cube_at(1, 1.0, 5.0)); // x in [4, 6] — gap of 3
        let report = assembly.interference_report();
        assert!(report.no_static_interference());
        // Clearance is best-effort; when supported it must report the ~3 gap.
        if let Some(gap) = assembly.clearance(InstanceId(0), InstanceId(1)) {
            assert!(gap > 2.5 && gap < 3.5, "expected ~3, got {gap}");
        }
    }

    #[test]
    fn clearance_is_symmetric() {
        // VERIFY/HARNESS invariant: clearance(a, b) == clearance(b, a).
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(cube_at(0, 1.0, 0.0));
        assembly.add_instance(cube_at(1, 1.0, 5.0));
        let ab = assembly.clearance(InstanceId(0), InstanceId(1));
        let ba = assembly.clearance(InstanceId(1), InstanceId(0));
        let symmetric = match (ab, ba) {
            (Some(x), Some(y)) => (x - y).abs() < 1e-9,
            (None, None) => true,
            _ => false,
        };
        assert!(symmetric, "clearance must be symmetric: {ab:?} vs {ba:?}");
    }

    #[test]
    fn scales_to_many_parts_without_false_interference() {
        // BENCHMARK/VERIFY: a row of well-separated parts → zero interference,
        // and the O(n²) sweep completes promptly (perf sanity; BVH broad-phase
        // is a later slice).
        let mut assembly = Assembly::new(InstanceId(0));
        for k in 0..30u32 {
            assembly.add_instance(cube_at(k, 0.4, f64::from(k) * 2.0));
        }
        let report = assembly.interference_report();
        assert!(report.no_static_interference());
    }
}
