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
    /// Mate connectors — frames bound to PLACES on instances (kinematic-
    /// assembly campaign; see [`super::mates`]). Serde-default so documents
    /// serialized before the mate slice keep deserializing.
    #[serde(default)]
    connectors: Vec<super::mates::MateConnector>,
    /// Mates over connector pairs. Pure description — the assembly-engine
    /// solves them over a borrowed view; this document never carries solver
    /// state (the `instancing.rs` header promise, kept).
    #[serde(default)]
    mates: Vec<super::mates::DocMate>,
}

impl InstancedAssembly {
    /// Create an empty assembly with the given display name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            instances: Vec::new(),
            connectors: Vec::new(),
            mates: Vec::new(),
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

    /// Insert an instance with a CALLER-SUPPLIED id — the timeline-replay
    /// path, which must reconstruct the exact ids the original session
    /// minted so later events (`transform_instance`, `remove_instance`)
    /// resolve against the rebuilt document. Returns `false` (and inserts
    /// nothing) when the id is already present — a duplicate would make the
    /// id ambiguous for every later event.
    pub fn add_instance_with_id(
        &mut self,
        id: InstanceId,
        part_id: Uuid,
        transform: Matrix4,
        name: Option<String>,
    ) -> bool {
        if self.instances.iter().any(|i| i.id == id) {
            return false;
        }
        self.instances.push(Instance {
            id,
            part_id,
            transform,
            name,
            color: None,
        });
        true
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
    ///
    /// CASCADES: connectors anchored on the instance are dropped, and so is
    /// every mate referencing a dropped connector (a mate to a place that no
    /// longer exists is not "stale", it is meaningless). The cascade is part
    /// of the document method so a timeline replay of `remove_instance`
    /// reproduces it deterministically.
    pub fn remove_instance(&mut self, id: InstanceId) -> bool {
        let before = self.instances.len();
        self.instances.retain(|i| i.id != id);
        let removed = self.instances.len() != before;
        if removed {
            let dropped: std::collections::BTreeSet<super::mates::MateConnectorId> = self
                .connectors
                .iter()
                .filter(|c| c.instance == id)
                .map(|c| c.id)
                .collect();
            if !dropped.is_empty() {
                self.connectors.retain(|c| c.instance != id);
                self.mates
                    .retain(|m| !dropped.contains(&m.a) && !dropped.contains(&m.b));
            }
        }
        removed
    }

    // ── Mate connectors + mates (document only; see super::mates) ──────

    /// All connectors in stable insertion order.
    pub fn connectors(&self) -> &[super::mates::MateConnector] {
        &self.connectors
    }

    /// Borrow one connector by id.
    pub fn connector(
        &self,
        id: super::mates::MateConnectorId,
    ) -> Option<&super::mates::MateConnector> {
        self.connectors.iter().find(|c| c.id == id)
    }

    /// Add a connector. Refused (`false`, nothing stored) when its id is
    /// already present or its instance does not exist in this assembly.
    pub fn add_connector(&mut self, connector: super::mates::MateConnector) -> bool {
        if self.connectors.iter().any(|c| c.id == connector.id) {
            return false;
        }
        if !self.instances.iter().any(|i| i.id == connector.instance) {
            return false;
        }
        self.connectors.push(connector);
        true
    }

    /// Replace a connector's stored frame (+ feature radius) — the resolve
    /// path re-deriving from the anchored feature. Returns `false` for an
    /// unknown id.
    pub fn set_connector_frame(
        &mut self,
        id: super::mates::MateConnectorId,
        frame: super::mates::ConnectorFrame,
        radius: Option<f64>,
    ) -> bool {
        match self.connectors.iter_mut().find(|c| c.id == id) {
            Some(c) => {
                c.frame = frame;
                c.radius = radius;
                true
            }
            None => false,
        }
    }

    /// Remove a connector. Refused (`false`) when unknown OR still
    /// referenced by a mate — delete the mate first (no silent cascades on
    /// explicit connector deletion; only instance removal cascades).
    pub fn remove_connector(&mut self, id: super::mates::MateConnectorId) -> bool {
        if !self.connectors.iter().any(|c| c.id == id) {
            return false;
        }
        if self.mates.iter().any(|m| m.a == id || m.b == id) {
            return false;
        }
        self.connectors.retain(|c| c.id != id);
        true
    }

    /// All mates in stable insertion order.
    pub fn mates(&self) -> &[super::mates::DocMate] {
        &self.mates
    }

    /// Borrow one mate by id.
    pub fn mate(&self, id: super::mates::DocMateId) -> Option<&super::mates::DocMate> {
        self.mates.iter().find(|m| m.id == id)
    }

    /// Add a mate. Refused (`false`) when its id already exists, either
    /// connector is unknown, both connectors sit on the SAME instance (a
    /// mate is a relationship between two bodies), or a coupling reference
    /// names an unknown mate.
    pub fn add_mate(&mut self, mate: super::mates::DocMate) -> bool {
        if self.mates.iter().any(|m| m.id == mate.id) {
            return false;
        }
        let (Some(ca), Some(cb)) = (self.connector(mate.a), self.connector(mate.b)) else {
            return false;
        };
        if ca.instance == cb.instance {
            return false;
        }
        if !mate
            .couples
            .iter()
            .all(|cid| self.mates.iter().any(|m| m.id == *cid))
        {
            return false;
        }
        self.mates.push(mate);
        true
    }

    /// Replace a mate's kind/parameters in place (PATCH semantics — value,
    /// limits, driven flags all live on the kind). Returns `false` for an
    /// unknown id.
    pub fn set_mate_kind(
        &mut self,
        id: super::mates::DocMateId,
        kind: super::mates::DocMateKind,
    ) -> bool {
        match self.mates.iter_mut().find(|m| m.id == id) {
            Some(m) => {
                m.kind = kind;
                true
            }
            None => false,
        }
    }

    /// Remove a mate. Refused (`false`) when unknown or when another mate
    /// COUPLES onto it (gear/rack-pinion/screw reference it) — remove the
    /// coupling first.
    pub fn remove_mate(&mut self, id: super::mates::DocMateId) -> bool {
        if !self.mates.iter().any(|m| m.id == id) {
            return false;
        }
        if self.mates.iter().any(|m| m.couples.contains(&id)) {
            return false;
        }
        self.mates.retain(|m| m.id != id);
        true
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

    // ── Connectors + mates (document; kinematic-assembly campaign) ─────

    use crate::assembly::mates::{
        ConnectorAnchor, ConnectorFrame, DocMate, DocMateId, DocMateKind, MateConnector,
        MateConnectorId,
    };

    fn raw_connector(instance: InstanceId) -> MateConnector {
        MateConnector {
            id: MateConnectorId::new(),
            instance,
            anchor: ConnectorAnchor::RawFrame,
            frame: ConnectorFrame {
                origin: [0.0; 3],
                z_axis: [0.0, 0.0, 1.0],
                x_axis: [1.0, 0.0, 0.0],
            },
            radius: None,
        }
    }

    fn mate_between(a: MateConnectorId, b: MateConnectorId) -> DocMate {
        DocMate {
            id: DocMateId::new(),
            kind: DocMateKind::Fastened,
            a,
            b,
            couples: Vec::new(),
            at: Vec::new(),
        }
    }

    #[test]
    fn connector_requires_live_instance() {
        let mut a = InstancedAssembly::new("rig");
        assert!(
            !a.add_connector(raw_connector(InstanceId::new())),
            "a connector on a nonexistent instance must be refused"
        );
        let iid = a.add_instance(Uuid::new_v4(), Matrix4::IDENTITY, None);
        assert!(a.add_connector(raw_connector(iid)));
        assert_eq!(a.connectors().len(), 1);
    }

    #[test]
    fn mate_requires_two_distinct_instances() {
        let mut a = InstancedAssembly::new("rig");
        let part = Uuid::new_v4();
        let i1 = a.add_instance(part, Matrix4::IDENTITY, None);
        let i2 = a.add_instance(part, Matrix4::IDENTITY, None);
        let c1 = raw_connector(i1);
        let c1b = raw_connector(i1);
        let c2 = raw_connector(i2);
        let (id1, id1b, id2) = (c1.id, c1b.id, c2.id);
        assert!(a.add_connector(c1) && a.add_connector(c1b) && a.add_connector(c2));
        assert!(
            !a.add_mate(mate_between(id1, id1b)),
            "both connectors on ONE instance is not a mate"
        );
        assert!(a.add_mate(mate_between(id1, id2)));
        assert_eq!(a.mates().len(), 1);
    }

    #[test]
    fn connector_referenced_by_mate_cannot_be_removed() {
        let mut a = InstancedAssembly::new("rig");
        let part = Uuid::new_v4();
        let i1 = a.add_instance(part, Matrix4::IDENTITY, None);
        let i2 = a.add_instance(part, Matrix4::IDENTITY, None);
        let c1 = raw_connector(i1);
        let c2 = raw_connector(i2);
        let (id1, id2) = (c1.id, c2.id);
        a.add_connector(c1);
        a.add_connector(c2);
        let m = mate_between(id1, id2);
        let mid = m.id;
        assert!(a.add_mate(m));
        assert!(!a.remove_connector(id1), "still referenced by the mate");
        assert!(a.remove_mate(mid));
        assert!(a.remove_connector(id1), "free after the mate is gone");
    }

    #[test]
    fn removing_an_instance_cascades_its_connectors_and_mates() {
        let mut a = InstancedAssembly::new("rig");
        let part = Uuid::new_v4();
        let i1 = a.add_instance(part, Matrix4::IDENTITY, None);
        let i2 = a.add_instance(part, Matrix4::IDENTITY, None);
        let c1 = raw_connector(i1);
        let c2 = raw_connector(i2);
        let (id1, id2) = (c1.id, c2.id);
        a.add_connector(c1);
        a.add_connector(c2);
        assert!(a.add_mate(mate_between(id1, id2)));
        assert!(a.remove_instance(i1));
        assert!(
            a.connector(id1).is_none(),
            "connector on the removed instance dropped"
        );
        assert!(a.connector(id2).is_some(), "the other side survives");
        assert!(
            a.mates().is_empty(),
            "the mate to the vanished place is dropped"
        );
    }

    #[test]
    fn pre_mate_documents_still_deserialize() {
        // Additive-serde contract: a document serialized BEFORE the mate
        // fields existed must keep loading (connectors/mates default empty).
        let raw = serde_json::json!({
            "id": Uuid::nil(),
            "name": "old",
            "instances": [],
        });
        let doc: Result<InstancedAssembly, _> = serde_json::from_value(raw);
        assert!(
            doc.as_ref()
                .is_ok_and(|d| d.connectors().is_empty() && d.mates().is_empty()),
            "old document must parse: {doc:?}"
        );
    }
}
