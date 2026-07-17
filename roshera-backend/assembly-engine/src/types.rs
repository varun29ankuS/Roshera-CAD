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
    /// A full connector FRAME (kinematic-assembly campaign, Slice 2): the
    /// resolved mate-connector coordinate frame in the instance's LOCAL
    /// space — `z_axis` primary (face normal / bore axis), `x_axis`
    /// secondary, `y = z × x` implied. The frame-pair mate kinds (Fastened,
    /// Revolute, Slider, …) are defined over these.
    Frame {
        origin: [f64; 3],
        z_axis: [f64; 3],
        x_axis: [f64; 3],
    },
}

/// The mate taxonomy (kinematic-assembly campaign, Slice 2; spec §3.2).
///
/// # Frame-pair convention
///
/// The joint kinds (Fastened … PinSlot) and overlays are defined over TWO
/// [`FeatureRef::Frame`] connectors. The mated configuration ALIGNS the
/// frames: `z_b = z_a` and, where the kind locks spin, `x_b = x_a`
/// (Onshape's primary-axis alignment). A "flipped" mate is authored by
/// flipping the connector frame itself — the residual never guesses a
/// side.
///
/// # Joint parameters
///
/// For a satisfied frame-pair mate, θ = the angle from `x_a` to `x_b`
/// about `z_a`, and s = `(o_b − o_a)·z_a` — the rotational/translational
/// joint parameters the coupling kinds (GearRatio / RackPinion / Screw)
/// relate. `at` fields carry the coupled parameters at declaration (the
/// coupling's reference configuration).
///
/// # Limits
///
/// `limits` fields are first-class joint parameters `(min, max)` — part of
/// the wire contract now; ACTIVE enforcement (at-limit facts) is campaign
/// Slice 5.
///
/// # Honest refusal
///
/// `Cam` / `Path` / `Symmetric` are TYPED but not numerically enforced —
/// [`MateKind::is_numerically_enforced`] is `false`, the enforcement
/// report names them, and the certificate refuses to count them as
/// constraints (never a silent zero-DOF lie; the sketch #19 contract).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum MateKind {
    /// Two planar faces flush (face-to-face coincident). Legacy Phase-1
    /// kind over `FeatureRef::Face`; rank 3.
    Coincident,
    /// Two axes collinear (concentric). Legacy Phase-1 kind over
    /// `FeatureRef::Axis`; rank 4.
    Concentric,
    /// Fully rigid — a bolt pattern. Consumes all 6 DOF: the declared face
    /// points coincide, the faces sit flush (antiparallel normals), and the
    /// deterministically derived in-plane tangents align (spin locked). See
    /// `mate_residual::fixed_residual`. Legacy alias of [`Self::Fastened`]
    /// over `FeatureRef::Face`.
    Fixed,
    // ── Frame-pair joint kinds ─────────────────────────────────────────
    /// 0 DOF — the true rigid lock over connector frames. Rank 6.
    Fastened,
    /// 1 rotational DOF about z. Rank 5.
    Revolute {
        limits: Option<(f64, f64)>,
    },
    /// 1 translational DOF along z. Rank 5.
    Slider {
        limits: Option<(f64, f64)>,
    },
    /// 1 rot + 1 trans about/along z. Rank 4.
    Cylindrical {
        rot_limits: Option<(f64, f64)>,
        trans_limits: Option<(f64, f64)>,
    },
    /// 2 trans + 1 rot in the mating plane. Rank 3.
    Planar,
    /// 3 rotational DOF about the coincident origins. Rank 3.
    Ball,
    /// 1 rot (pin spin) + 1 trans (along the slot). `slot_dir_x` picks the
    /// slot direction on frame A: x (true) or y = z × x (false). Rank 4.
    PinSlot {
        slot_dir_x: bool,
        limits: Option<(f64, f64)>,
    },
    // ── Dimensional overlays ───────────────────────────────────────────
    /// Signed offset of `o_b` from frame A's plane along `z_a`. Rank 1.
    Distance {
        value: f64,
    },
    /// Angle between the z axes (radians). Rank 1.
    Angle {
        value: f64,
    },
    /// z axes parallel or antiparallel. Rank 2.
    Parallel,
    /// Frame B's cylindrical/spherical feature (radius from the connector)
    /// tangent to frame A's plane: |distance of `o_b` to the plane| =
    /// radius. Rank 1.
    Tangent {
        radius: f64,
    },
    // ── DOF couplings between EXISTING mates ───────────────────────────
    // `couples` fields are indices into `Assembly::mates` naming the
    // mate(s) whose joint parameters are related; a coupling mate's own
    // `feature_a`/`feature_b` are descriptive (the gear-centre frames) and
    // do not enter its residual.
    /// `ratio·(θ₁ − at[0]) + (θ₂ − at[1]) = 0` over two Revolute /
    /// Cylindrical mates (external gears counter-rotate). Rank 1.
    GearRatio {
        ratio: f64,
        at: [f64; 2],
        couples: [u32; 2],
    },
    /// `pinion_radius·(θ − at[0]) − (s − at[1]) = 0` over a Revolute mate
    /// (θ) and a Slider mate (s). Rank 1.
    RackPinion {
        pinion_radius: f64,
        at: [f64; 2],
        couples: [u32; 2],
    },
    /// `(s − at[1]) − lead·(θ − at[0])/2π = 0` WITHIN one Cylindrical mate
    /// (couples its two DOF into a helix). Rank 1.
    Screw {
        lead: f64,
        at: [f64; 2],
        couples: u32,
    },
    // ── Honest-refuse set: typed, NOT numerically enforced ─────────────
    Cam,
    Path,
    Symmetric,
}

impl MateKind {
    /// Whether the solver numerically enforces this kind. `false` = the
    /// typed refuse set: the mate is carried, reported, and REFUSED in the
    /// enforcement report — it never silently consumes zero DOF while
    /// presenting itself as a constraint.
    pub fn is_numerically_enforced(&self) -> bool {
        !matches!(self, MateKind::Cam | MateKind::Path | MateKind::Symmetric)
    }

    /// Whether this kind is a DOF coupling over OTHER mates (consumes
    /// `Mate::couples`) rather than a geometric relationship between the
    /// mate's own features.
    pub fn is_coupling(&self) -> bool {
        matches!(
            self,
            MateKind::GearRatio { .. } | MateKind::RackPinion { .. } | MateKind::Screw { .. }
        )
    }
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
