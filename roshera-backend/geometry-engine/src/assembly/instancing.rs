//! Positioned-instance assemblies (#19, Phase 1).
//!
//! A TRUE assembly is NOT "boolean everything into one solid". It is a set
//! of PART INSTANCES positioned in space, each one a *reference* to a part
//! plus a world transform — geometry is reused, never copied. This is the
//! scaling primitive for the 100-part north star: the assembly stores N
//! instances referencing M ≤ N distinct parts, and the cost of an extra
//! instance is one transform, not one geometric copy.
//!
//! # Relationship to [`crate::assembly::Assembly`]
//!
//! The older [`Assembly`](crate::assembly::Assembly) type is mate-centric:
//! every `Component` owns its own `Arc<BRepModel>` (a geometric copy) and
//! the assembly carries a Gauss-Seidel constraint solver, gear neutrals,
//! exploded views. That is the right home for Phase-2 MATES, but it copies
//! geometry per component, which is exactly what an instanced assembly must
//! avoid.
//!
//! [`InstancedAssembly`] is the lighter, reference-only model. An
//! [`Instance`] holds only:
//!   * `part_id` — the api-server's part document UUID it instantiates,
//!   * `transform` — the world pose,
//!   * presentation (name, optional colour).
//!
//! No `BRepModel`, no solid, no copy. The geometry lives once in the active
//! model; resolving `part_id → solid` and compositing at the transform is
//! the renderer's job (see `render::render_instances_dir`). The Phase-2
//! seam: mates would be added here as a `Vec<InstanceMate>` driving instance
//! transforms, OR the two models would be unified once the document store
//! makes `Component` reference-only too. Either way nothing in this module
//! presumes a copy, so the seam stays clean.

use crate::math::{BBox, Matrix4};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

/// Unique identifier for a part instance inside an [`InstancedAssembly`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct InstanceId(pub Uuid);

impl Default for InstanceId {
    fn default() -> Self {
        Self::new()
    }
}

impl InstanceId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

/// A single positioned reference to a part.
///
/// `part_id` is the source part's document UUID — the SAME `part_id` may
/// appear in many instances of the same assembly (that IS the instancing:
/// one part, many placements). The geometry is never stored here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    /// Stable instance id (assembly-local handle for transform/remove).
    pub id: InstanceId,
    /// The part document this instance references. Reused across instances.
    pub part_id: Uuid,
    /// World transform (4×4) placing the referenced part. Identity = the
    /// part sits exactly where its own model defines it.
    pub transform: Matrix4,
    /// Optional display name for this placement (e.g. "front-left wheel").
    pub name: Option<String>,
    /// Optional per-instance display colour (RGB). When `None` the renderer
    /// falls back to the part's registered colour or neutral grey.
    pub color: Option<[u8; 3]>,
}

impl Instance {
    /// Construct an instance referencing `part_id` at `transform`.
    pub fn new(part_id: Uuid, transform: Matrix4, name: Option<String>) -> Self {
        Self {
            id: InstanceId::new(),
            part_id,
            transform,
            name,
            color: None,
        }
    }
}

/// A positioned-instance assembly: a named set of part instances.
///
/// Unlike [`Assembly`](crate::assembly::Assembly) this carries no geometry
/// and no solver — it is a pure scene description. Storage is a `Vec` (an
/// assembly's instance list is small and order-stable; iteration is the
/// only hot path and a `Vec` keeps render order deterministic).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstancedAssembly {
    pub id: Uuid,
    pub name: String,
    instances: Vec<Instance>,
}

impl InstancedAssembly {
    /// Create an empty assembly with the given display name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            instances: Vec::new(),
        }
    }

    /// Add a positioned instance of `part_id`. The same `part_id` may be
    /// added any number of times — each call yields a distinct
    /// [`InstanceId`] over the SAME referenced geometry. Returns the new id.
    pub fn add_instance(
        &mut self,
        part_id: Uuid,
        transform: Matrix4,
        name: Option<String>,
    ) -> InstanceId {
        let inst = Instance::new(part_id, transform, name);
        let id = inst.id;
        self.instances.push(inst);
        id
    }

    /// All instances in stable insertion order.
    pub fn instances(&self) -> &[Instance] {
        &self.instances
    }

    /// Borrow one instance by id.
    pub fn instance(&self, id: InstanceId) -> Option<&Instance> {
        self.instances.iter().find(|i| i.id == id)
    }

    /// Number of instances (placements).
    pub fn instance_count(&self) -> usize {
        self.instances.len()
    }

    /// Number of DISTINCT parts referenced. `instance_count -
    /// unique_part_count` is the count of placements served by reuse — the
    /// instancing payoff in one number.
    pub fn unique_part_count(&self) -> usize {
        self.instances
            .iter()
            .map(|i| i.part_id)
            .collect::<BTreeSet<_>>()
            .len()
    }

    /// Replace an instance's world transform. Returns `false` if the id is
    /// unknown (caller surfaces a not-found error).
    pub fn transform_instance(&mut self, id: InstanceId, transform: Matrix4) -> bool {
        match self.instances.iter_mut().find(|i| i.id == id) {
            Some(inst) => {
                inst.transform = transform;
                true
            }
            None => false,
        }
    }

    /// Set an instance's display colour. Returns `false` if unknown.
    pub fn set_instance_color(&mut self, id: InstanceId, color: Option<[u8; 3]>) -> bool {
        match self.instances.iter_mut().find(|i| i.id == id) {
            Some(inst) => {
                inst.color = color;
                true
            }
            None => false,
        }
    }

    /// Remove an instance by id. Returns `true` if one was removed.
    pub fn remove_instance(&mut self, id: InstanceId) -> bool {
        let before = self.instances.len();
        self.instances.retain(|i| i.id != id);
        self.instances.len() != before
    }

    /// Combined world bounding box over every instance, given a resolver
    /// that maps a `part_id` to the part's own (untransformed) world bbox.
    ///
    /// Each instance's transform is applied to the part bbox's 8 corners and
    /// the result unioned — so a rotated instance contributes its true
    /// swept extent, not an axis-aligned approximation of the local box.
    /// `None` when no instance resolves to a bbox (empty assembly or every
    /// referenced part missing).
    pub fn combined_bbox<F>(&self, mut part_bbox: F) -> Option<BBox>
    where
        F: FnMut(Uuid) -> Option<BBox>,
    {
        let mut acc: Option<BBox> = None;
        for inst in &self.instances {
            let Some(local) = part_bbox(inst.part_id) else {
                continue;
            };
            let corners: Vec<_> = local
                .corners()
                .iter()
                .map(|p| inst.transform.transform_point(p))
                .collect();
            let Some(world) = BBox::from_points(&corners) else {
                continue;
            };
            acc = Some(match acc {
                Some(a) => a.union(&world),
                None => world,
            });
        }
        acc
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Point3;

    fn translate(x: f64, y: f64, z: f64) -> Matrix4 {
        let mut m = Matrix4::IDENTITY;
        m[(0, 3)] = x;
        m[(1, 3)] = y;
        m[(2, 3)] = z;
        m
    }

    #[test]
    fn create_is_empty_with_name() {
        let a = InstancedAssembly::new("rig");
        assert_eq!(a.name, "rig");
        assert_eq!(a.instance_count(), 0);
        assert_eq!(a.unique_part_count(), 0);
        assert!(a.instances().is_empty());
    }

    #[test]
    fn add_instance_returns_distinct_ids() {
        let mut a = InstancedAssembly::new("rig");
        let part = Uuid::new_v4();
        let i1 = a.add_instance(part, Matrix4::IDENTITY, Some("a".into()));
        let i2 = a.add_instance(part, translate(10.0, 0.0, 0.0), Some("b".into()));
        assert_ne!(i1, i2);
        assert_eq!(a.instance_count(), 2);
    }

    #[test]
    fn two_instances_share_one_part_id() {
        // THE instancing proof: same part_id, two placements, ONE distinct
        // part. instance_count - unique_part_count = 1 reuse.
        let mut a = InstancedAssembly::new("rig");
        let shared = Uuid::new_v4();
        let other = Uuid::new_v4();
        a.add_instance(shared, Matrix4::IDENTITY, None);
        a.add_instance(shared, translate(50.0, 0.0, 0.0), None);
        a.add_instance(other, translate(0.0, 50.0, 0.0), None);
        assert_eq!(a.instance_count(), 3);
        assert_eq!(a.unique_part_count(), 2);
        // The two shared instances reference the identical UUID — no copy.
        let shared_refs: Vec<_> = a
            .instances()
            .iter()
            .filter(|i| i.part_id == shared)
            .collect();
        assert_eq!(shared_refs.len(), 2);
        assert_eq!(shared_refs[0].part_id, shared_refs[1].part_id);
    }

    #[test]
    fn transform_instance_updates_pose() {
        let mut a = InstancedAssembly::new("rig");
        let part = Uuid::new_v4();
        let id = a.add_instance(part, Matrix4::IDENTITY, None);
        assert!(a.transform_instance(id, translate(7.0, 8.0, 9.0)));
        let t = a.instance(id).unwrap().transform;
        assert_eq!(t[(0, 3)], 7.0);
        assert_eq!(t[(1, 3)], 8.0);
        assert_eq!(t[(2, 3)], 9.0);
        // Unknown id is a clean miss.
        assert!(!a.transform_instance(InstanceId::new(), Matrix4::IDENTITY));
    }

    #[test]
    fn remove_instance_drops_only_that_placement() {
        let mut a = InstancedAssembly::new("rig");
        let part = Uuid::new_v4();
        let keep = a.add_instance(part, Matrix4::IDENTITY, None);
        let drop = a.add_instance(part, translate(10.0, 0.0, 0.0), None);
        assert!(a.remove_instance(drop));
        assert!(!a.remove_instance(drop)); // already gone
        assert_eq!(a.instance_count(), 1);
        assert!(a.instance(keep).is_some());
        assert!(a.instance(drop).is_none());
    }

    #[test]
    fn combined_bbox_unions_transformed_corners() {
        let mut a = InstancedAssembly::new("rig");
        let part = Uuid::new_v4();
        // Unit cube at origin for the referenced part.
        let unit = BBox {
            min: Point3::new(0.0, 0.0, 0.0),
            max: Point3::new(1.0, 1.0, 1.0),
        };
        a.add_instance(part, Matrix4::IDENTITY, None);
        a.add_instance(part, translate(10.0, 0.0, 0.0), None);
        let bb = a
            .combined_bbox(|pid| if pid == part { Some(unit) } else { None })
            .expect("non-empty bbox");
        // x spans 0..11 (two unit cubes 10 apart), y/z span 0..1.
        assert!((bb.min.x - 0.0).abs() < 1e-9);
        assert!((bb.max.x - 11.0).abs() < 1e-9);
        assert!((bb.max.y - 1.0).abs() < 1e-9);
        assert!((bb.max.z - 1.0).abs() < 1e-9);
    }

    #[test]
    fn combined_bbox_none_when_unresolvable() {
        let mut a = InstancedAssembly::new("rig");
        a.add_instance(Uuid::new_v4(), Matrix4::IDENTITY, None);
        assert!(a.combined_bbox(|_| None).is_none());
    }

    #[test]
    fn set_instance_color_round_trips() {
        let mut a = InstancedAssembly::new("rig");
        let id = a.add_instance(Uuid::new_v4(), Matrix4::IDENTITY, None);
        assert!(a.set_instance_color(id, Some([10, 20, 30])));
        assert_eq!(a.instance(id).unwrap().color, Some([10, 20, 30]));
    }
}
