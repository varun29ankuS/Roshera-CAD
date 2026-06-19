//! Ground-truth provenance + validity for solids — the kernel's own account of
//! WHAT it made and WHETHER it is real, so an agent cannot misrepresent a
//! placeholder primitive as a designed surface (or a broken solid as finished).
//!
//! The root defect this closes: to the kernel, a hand-designed lofted surface
//! and a `Box` dropped in as a stand-in are indistinguishable objects. Here the
//! kernel records, as first-class data, the operation that created each solid
//! (classified primitive-vs-designed) and computes — never accepts from the
//! caller — a validity certificate. The agent's honesty becomes structural: the
//! kernel can answer "what did you actually make, and which parts are real?"
//! without consulting the LLM.

use crate::math::{Matrix4, Point3};
use crate::primitives::persistent_id::PrimitiveKind;
use crate::primitives::solid::SolidId;

/// How a solid came to exist — the kernel's faithful classification. A bare
/// `Primitive` is honestly flagged as a stand-in, not a designed feature; the
/// designed variants record a real modelling operation.
#[derive(Debug, Clone, PartialEq)]
pub enum OperationKind {
    /// A bare primitive (box / cylinder / sphere / cone / torus). The honest
    /// "this is a stand-in, not a designed surface" signal.
    Primitive(PrimitiveKind),
    Extrude,
    Revolve,
    Loft,
    /// A freeform NURBS-skinned loft (`operations::nurbs_loft`).
    NurbsLoft,
    Sweep,
    Boolean,
    Fillet,
    Chamfer,
    Shell,
    Transform,
    /// Created by an operation not yet wired to record provenance — honestly
    /// "unknown", never silently assumed designed.
    Other(String),
}

impl OperationKind {
    /// True for solids that are a genuine designed feature (not a bare primitive
    /// stand-in and not an unrecorded op).
    pub fn is_designed(&self) -> bool {
        matches!(
            self,
            OperationKind::Extrude
                | OperationKind::Revolve
                | OperationKind::Loft
                | OperationKind::NurbsLoft
                | OperationKind::Sweep
                | OperationKind::Boolean
                | OperationKind::Fillet
                | OperationKind::Chamfer
                | OperationKind::Shell
        )
    }

    pub fn is_primitive(&self) -> bool {
        matches!(self, OperationKind::Primitive(_))
    }

    /// Short human/agent-facing label.
    pub fn label(&self) -> String {
        match self {
            OperationKind::Primitive(k) => format!("primitive:{k:?}"),
            OperationKind::Extrude => "extrude".into(),
            OperationKind::Revolve => "revolve".into(),
            OperationKind::Loft => "loft".into(),
            OperationKind::NurbsLoft => "nurbs_loft".into(),
            OperationKind::Sweep => "sweep".into(),
            OperationKind::Boolean => "boolean".into(),
            OperationKind::Fillet => "fillet".into(),
            OperationKind::Chamfer => "chamfer".into(),
            OperationKind::Shell => "shell".into(),
            OperationKind::Transform => "transform".into(),
            OperationKind::Other(s) => format!("other:{s}"),
        }
    }
}

/// Provenance attached to a solid: the operation that created it and the input
/// solids it consumed (empty for primitives / fresh creations).
#[derive(Debug, Clone, PartialEq)]
pub struct SolidProvenance {
    pub created_by: OperationKind,
    pub inputs: Vec<SolidId>,
}

impl SolidProvenance {
    pub fn new(created_by: OperationKind, inputs: Vec<SolidId>) -> Self {
        Self { created_by, inputs }
    }
}

/// The construction geometry a solid was built FROM — the source sketch's
/// plane frame plus the world-space points that bound the drawn profile.
///
/// This is the kernel-side anchor of the solid↔sketch link. A
/// sketch-derived solid (extrude / revolve-from-sketch) records the plane
/// origin and the lifted profile points here so the kernel can (a) carry
/// the construction geometry through a [`crate::operations::transform`]
/// in lock-step with the solid (FIX 1 — sketch and solid never diverge),
/// and (b) certify that the two are still co-located (FIX 2 — a stale /
/// orphaned sketch is flagged honestly).
///
/// Solids with no source sketch (bare primitives, revolve / loft with no
/// recorded profile, NURBS skins) simply have no entry, and the
/// consistency check reports [`ConstructionConsistency::NotApplicable`].
#[derive(Debug, Clone, PartialEq)]
pub struct ConstructionGeometry {
    /// World-space origin of the source sketch plane (the lift of plane
    /// (0, 0)).
    pub plane_origin: Point3,
    /// World-space points of the drawn profile (the lifted sketch loop
    /// vertices). Used to derive the construction bbox for the
    /// co-location check; never empty for a recorded sketch.
    pub profile_points: Vec<Point3>,
}

impl ConstructionGeometry {
    pub fn new(plane_origin: Point3, profile_points: Vec<Point3>) -> Self {
        Self {
            plane_origin,
            profile_points,
        }
    }

    /// Apply an affine transform to every stored point (FIX 1). Keeps the
    /// construction geometry rigidly attached to the solid it built so a
    /// `transform_solid` can never leave the sketch behind.
    pub fn transformed(&self, m: &Matrix4) -> Self {
        Self {
            plane_origin: m.transform_point(&self.plane_origin),
            profile_points: self
                .profile_points
                .iter()
                .map(|p| m.transform_point(p))
                .collect(),
        }
    }

    /// Axis-aligned world bbox of the construction geometry (plane origin
    /// + every profile point). `None` only if there are no points at all.
    pub fn world_bbox(&self) -> Option<crate::math::BBox> {
        let mut pts = Vec::with_capacity(self.profile_points.len() + 1);
        pts.push(self.plane_origin);
        pts.extend_from_slice(&self.profile_points);
        crate::math::BBox::from_points(&pts)
    }
}

/// Tri-state verdict on whether a solid's linked construction geometry is
/// consistent with the solid itself (FIX 2). Tri-state so the certificate
/// is HONEST: it distinguishes "checked and good" from "checked and bad"
/// from "nothing to check".
///
/// * `Consistent` — the construction geometry exists and is co-located
///   with the solid (sketch bbox lies within the solid's bbox, expanded
///   by a tolerance band).
/// * `Inconsistent` — the construction geometry exists but has drifted
///   away from the solid (an orphaned sketch left behind by a transform).
///   Folds into `is_sound() == false`.
/// * `NotApplicable` — no construction geometry is linked (bare
///   primitive, revolve / loft / NURBS with no recorded profile). MUST
///   NOT affect soundness — a primitive solid stays sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstructionConsistency {
    Consistent,
    Inconsistent,
    NotApplicable,
}

impl ConstructionConsistency {
    /// Short agent-facing label.
    pub fn label(&self) -> &'static str {
        match self {
            ConstructionConsistency::Consistent => "consistent",
            ConstructionConsistency::Inconsistent => "inconsistent",
            ConstructionConsistency::NotApplicable => "not_applicable",
        }
    }

    /// True when this verdict does NOT block soundness — i.e. anything but
    /// `Inconsistent`. `NotApplicable` is sound by construction so
    /// sketch-less solids never regress.
    pub fn is_sound(&self) -> bool {
        !matches!(self, ConstructionConsistency::Inconsistent)
    }
}

/// Tri-state verdict on whether a part's LABELS are all still consistent with
/// the geometry (D4). Computed by re-verifying every label's assertion (the
/// selector still resolves to the SAME entity, or the entity still matches the
/// captured fingerprint).
///
/// * `Consistent` — every label's assertion still holds.
/// * `Inconsistent` — at least one label is STALE (its selector now finds
///   nothing / a different entity, or no live entity matches its fingerprint).
///   Per D4 this is an ANNOTATION defect, NOT a geometric one: it does NOT
///   force `is_sound() == false` — it is its own honest flag.
/// * `NotApplicable` — the part has no labels (nothing to check).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelsConsistency {
    Consistent,
    Inconsistent,
    NotApplicable,
}

impl LabelsConsistency {
    /// Short agent-facing label.
    pub fn label(&self) -> &'static str {
        match self {
            LabelsConsistency::Consistent => "consistent",
            LabelsConsistency::Inconsistent => "inconsistent",
            LabelsConsistency::NotApplicable => "not_applicable",
        }
    }
}

/// The kernel's COMPUTED verdict on a solid — never written by the caller.
/// `is_sound()` is the honest "this is a real, closed, manufacturable solid"
/// gate: a valid B-Rep that is watertight and manifold.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidityCertificate {
    /// `validate_solid_scoped` Standard verdict (mesh-independent topology).
    pub brep_valid: bool,
    /// Mesh closes (no boundary edges) at the certification chord.
    pub watertight: bool,
    /// Every edge bordered by exactly two faces (no 3+-fan non-manifold edges).
    pub manifold: bool,
    /// Tessellated-mesh Euler characteristic (V − E + F).
    pub euler_characteristic: i64,
    pub boundary_edges: usize,
    pub nonmanifold_edges: usize,
    /// No two non-adjacent faces cross (geometrically non-self-overlapping). A
    /// solid can be valid + watertight yet self-intersect (#70-class); this is
    /// the only check that catches it.
    pub self_intersection_free: bool,
    /// Cross-entity consistency (FIX 2): is the solid's linked construction
    /// geometry (source sketch plane + profile) co-located with the solid?
    /// Tri-state — `NotApplicable` when no sketch is linked, and it does NOT
    /// affect soundness in that case (a bare primitive stays sound).
    pub construction_consistent: ConstructionConsistency,
    /// D4 — labels consistency: are all the part's labels still backed by a
    /// holding assertion? Tri-state, `NotApplicable` when the part has no
    /// labels. A label is an ANNOTATION, not a geometric feature, so an
    /// `Inconsistent` verdict does NOT affect `is_sound()` — it is its own
    /// honest flag the agent/frontend can surface (stale labels rendered amber).
    pub labels_consistent: LabelsConsistency,
    /// B-Rep validation errors (stringified), empty when `brep_valid`.
    pub errors: Vec<String>,
}

impl ValidityCertificate {
    pub fn is_sound(&self) -> bool {
        self.brep_valid
            && self.watertight
            && self.manifold
            && self.self_intersection_free
            && self.construction_consistent.is_sound()
    }
}

/// What the kernel actually made for a solid, and whether it is real — the
/// answer to a ground-truth query, assembled without consulting the agent.
#[derive(Debug, Clone)]
pub struct GroundTruth {
    pub solid_id: SolidId,
    pub provenance: Option<SolidProvenance>,
    pub certificate: ValidityCertificate,
}

impl GroundTruth {
    /// A one-line honest summary for an agent/log.
    pub fn summary(&self) -> String {
        let origin = self
            .provenance
            .as_ref()
            .map(|p| p.created_by.label())
            .unwrap_or_else(|| "unrecorded".into());
        format!(
            "solid {} — origin={} designed={} sound={} (brep_valid={} watertight={} manifold={} euler={} construction={} labels={})",
            self.solid_id,
            origin,
            self.provenance
                .as_ref()
                .map(|p| p.created_by.is_designed())
                .unwrap_or(false),
            self.certificate.is_sound(),
            self.certificate.brep_valid,
            self.certificate.watertight,
            self.certificate.manifold,
            self.certificate.euler_characteristic,
            self.certificate.construction_consistent.label(),
            self.certificate.labels_consistent.label(),
        )
    }
}
