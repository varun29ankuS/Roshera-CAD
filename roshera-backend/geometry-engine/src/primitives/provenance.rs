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
    /// B-Rep validation errors (stringified), empty when `brep_valid`.
    pub errors: Vec<String>,
}

impl ValidityCertificate {
    pub fn is_sound(&self) -> bool {
        self.brep_valid && self.watertight && self.manifold && self.self_intersection_free
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
            "solid {} — origin={} designed={} sound={} (brep_valid={} watertight={} manifold={} euler={})",
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
        )
    }
}
