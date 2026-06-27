//! Assembly data model: instances, mates, and the named geometric features a
//! mate references.

use serde::{Deserialize, Serialize};

/// Stable identifier of an instance within an assembly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct InstanceId(pub u32);

/// A display-mesh (the kernel tessellation) carried for collision and clearance.
/// Vertices are in the instance's LOCAL frame; the instance pose is applied at
/// query time, so the mesh is shared across reuse of the same part.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Mesh {
    pub vertices: Vec<[f64; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

/// One placed occurrence of a part. A part used N times is N instances (reuse,
/// not copy). The pose is `translation` plus a unit `rotation` quaternion
/// `[x, y, z, w]`, relative to the assembly origin. In Phase 1 the pose is plain
/// data; the Phase-2 mate solver writes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Instance {
    pub id: InstanceId,
    pub name: String,
    pub mesh: Mesh,
    pub translation: [f64; 3],
    pub rotation: [f64; 4],
}

impl Instance {
    /// A part placed at the origin with identity orientation.
    pub fn new(id: InstanceId, name: impl Into<String>, mesh: Mesh) -> Self {
        Self {
            id,
            name: name.into(),
            mesh,
            translation: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
        }
    }
}

/// A named geometric feature a mate references. Resolved upstream from the
/// semantic layer (Pillar-3 `resolve_face`/`resolve_edge`, the labeller): the
/// durable feature NAME lives in the caller; this carries the resolved
/// primitive the solver consumes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FeatureRef {
    /// A planar face: a point on it and its outward unit normal.
    Face { point: [f64; 3], normal: [f64; 3] },
    /// A straight axis (cylinder / cone / revolve axis): origin and unit direction.
    Axis {
        origin: [f64; 3],
        direction: [f64; 3],
    },
}

/// The constraint kinds Phase 1 models. Each removes degrees of freedom; the DOF
/// arithmetic and the geometric solve land in Phase 2 (slices S4–S6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MateKind {
    /// Two planar faces flush (face-to-face coincident).
    Coincident,
    /// Two axes collinear (concentric).
    Concentric,
    /// Fully rigid — a bolt pattern (concentric + coincident + one angular lock).
    Fixed,
}

/// A constraint between named features on two instances.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mate {
    pub kind: MateKind,
    pub a: InstanceId,
    pub feature_a: FeatureRef,
    pub b: InstanceId,
    pub feature_b: FeatureRef,
}

/// An assembly: instances connected by mates, with one grounded instance. Every
/// other instance must reach `ground` through the mate graph, or it is FLOATING.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Assembly {
    pub instances: Vec<Instance>,
    pub mates: Vec<Mate>,
    pub ground: InstanceId,
}

impl Assembly {
    /// An empty assembly grounded on `ground`.
    pub fn new(ground: InstanceId) -> Self {
        Self {
            instances: Vec::new(),
            mates: Vec::new(),
            ground,
        }
    }

    /// Look up an instance by id.
    pub fn instance(&self, id: InstanceId) -> Option<&Instance> {
        self.instances.iter().find(|i| i.id == id)
    }

    /// Add an instance and return its id.
    pub fn add_instance(&mut self, instance: Instance) -> InstanceId {
        let id = instance.id;
        self.instances.push(instance);
        id
    }

    /// Add a mate between two instances.
    pub fn add_mate(&mut self, mate: Mate) {
        self.mates.push(mate);
    }
}
