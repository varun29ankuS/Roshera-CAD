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

/// Per-face display-tessellation defect — lets an agent point at exactly which
/// face renders wrong, without rendering a pixel. Returned as the single worst
/// face inside [`TessellationQuality`].
#[derive(Debug, Clone, PartialEq)]
pub struct TessFaceDefect {
    /// The offending face id (matches the kernel `FaceId` / the per-triangle
    /// `face_map` the frontend uses for picking).
    pub face_id: u64,
    /// Triangles tessellated for this face.
    pub triangles: usize,
    /// Zero-area facets on this face.
    pub degenerate_triangles: usize,
    /// Fraction of this face's facets whose winding normal agrees with the
    /// stored (analytic-intent) vertex normals. `1.0` = clean; an inner-bore
    /// scribble drops well below.
    pub normal_agreement: f64,
    /// Fraction of this face's facets whose winding normal sits in the same
    /// hemisphere as the TRUE surface normal at the facet centroid. This is the
    /// ground-truth check: a scribble whose stored normals are wrong-but-self-
    /// consistent scores `1.0` on `normal_agreement` yet drops here.
    pub analytic_normal_agreement: f64,
}

/// Display-tessellation quality — the render-mesh analogue of B-Rep soundness.
///
/// A solid can be a valid, watertight, manifold B-Rep yet tessellate to a
/// degenerate / inverted-normal mesh (the inner-bore "scribble" class): the mesh
/// still closes and every edge borders two faces, so the *topological* checks
/// ([`ValidityCertificate::watertight`] / [`manifold`](ValidityCertificate::manifold))
/// all pass — but the facets are zero-area slivers or face the wrong way and the
/// part renders as garbage. This dimension is the missing invariant: the kernel
/// must not certify a solid "sound" while its display mesh is broken.
///
/// The verdict compares each facet's winding normal to its stored vertex normals
/// (the analytic normal the tessellator *intended*), so a mis-oriented face is
/// caught against ground truth, not merely against its neighbours.
#[derive(Debug, Clone, PartialEq)]
pub struct TessellationQuality {
    /// Total triangles in the certification mesh.
    pub triangles: usize,
    /// Zero-area facets (coincident or collinear vertices).
    pub degenerate_triangles: usize,
    /// Fraction of non-degenerate facets whose winding normal agrees with their
    /// stored vertex normals. `1.0` = every facet shaded the way its geometry
    /// faces.
    pub normal_agreement: f64,
    /// Fraction of non-degenerate facets whose winding normal sits in the same
    /// hemisphere as the TRUE surface normal at the facet centroid. THIS is the
    /// ground-truth check: a scribble whose stored normals are wrong-but-self-
    /// consistent scores `1.0` on `normal_agreement` yet drops here, because the
    /// off-radial facets disagree with the actual analytic surface normal.
    pub analytic_normal_agreement: f64,
    /// Facets whose winding disagrees with their stored normals (inverted/skewed).
    pub inconsistent_facets: usize,
    /// Facets pointing into the wrong hemisphere of the TRUE surface normal — the
    /// off-radial scribble count. The decisive signal for an inner-bore defect.
    pub off_surface_facets: usize,
    /// The single worst face by normal disagreement — the agent's pointer to the
    /// defect. `None` when the mesh is clean.
    pub worst_face: Option<TessFaceDefect>,
    /// `true` when there are no degenerate facets, normal agreement clears
    /// [`Self::MIN_NORMAL_AGREEMENT`], and there are zero off-surface facets.
    /// Conservative (gross-defect only) so a clean part at a coarse chord never
    /// false-positives.
    pub clean: bool,
}

impl TessellationQuality {
    /// Soundness threshold for stored-normal agreement. Set well below `1.0` so
    /// coarse-chord numerical noise never trips it, yet far above the ~0.5–0.7 a
    /// scribbled bore scores.
    pub const MIN_NORMAL_AGREEMENT: f64 = 0.98;

    /// Soundness threshold for analytic-normal agreement. A correct (even coarse)
    /// facet always lies in the surface-normal hemisphere, so any shortfall is a
    /// genuinely off-surface (scribbled / inverted) facet.
    pub const MIN_ANALYTIC_AGREEMENT: f64 = 0.999;

    /// Build a verdict from the measured counts, deriving `clean`.
    pub fn evaluate(
        triangles: usize,
        degenerate_triangles: usize,
        normal_agreement: f64,
        analytic_normal_agreement: f64,
        inconsistent_facets: usize,
        off_surface_facets: usize,
        worst_face: Option<TessFaceDefect>,
    ) -> Self {
        let clean = degenerate_triangles == 0
            && normal_agreement >= Self::MIN_NORMAL_AGREEMENT
            && off_surface_facets == 0
            && analytic_normal_agreement >= Self::MIN_ANALYTIC_AGREEMENT;
        Self {
            triangles,
            degenerate_triangles,
            normal_agreement,
            analytic_normal_agreement,
            inconsistent_facets,
            off_surface_facets,
            worst_face,
            clean,
        }
    }

    /// The verdict for a solid that tessellates to nothing. Such a solid is
    /// already flagged unsound by `brep_valid`/`watertight`, so an empty mesh is
    /// reported `clean` (quality is not-applicable) rather than double-penalised.
    pub fn empty() -> Self {
        Self {
            triangles: 0,
            degenerate_triangles: 0,
            normal_agreement: 1.0,
            analytic_normal_agreement: 1.0,
            inconsistent_facets: 0,
            off_surface_facets: 0,
            worst_face: None,
            clean: true,
        }
    }
}

/// Per-face mesh-shape defect — the agent's pointer to which face violates a
/// CAD mesh-quality rule and on which metric.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshFaceQualityDefect {
    pub face_id: u64,
    /// Largest facet aspect ratio on this face (longest edge / shortest edge).
    pub worst_aspect_ratio: f64,
    /// Smallest interior facet angle on this face, degrees.
    pub min_angle_deg: f64,
    /// Largest angle between a facet's winding normal and the true surface normal
    /// at its centroid, degrees.
    pub max_normal_deviation_deg: f64,
    /// Facets on this face that cross an analytical boundary (a periodic lateral
    /// facet spanning > half a period bridges the interior — the bore "wing").
    pub boundary_crossing_facets: usize,
}

/// **Mesh-quality** verdict — the render mesh measured against the established
/// CAD/FEA tessellation rules, not just "is it closed / correctly-oriented"
/// ([`TessellationQuality`]). A facet can be watertight, non-degenerate, and
/// correctly oriented yet still violate a *shape* rule — a sliver-angle triangle,
/// a triangle that bridges across a bore (the "wing"), a facet whose normal
/// strays far from the surface, or a triangle that crosses a face boundary. This
/// dimension encodes those rules so the kernel can REFUSE a badly-shaped mesh the
/// way it refuses a non-manifold one — and name the offending face + metric.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshQuality {
    pub triangles: usize,
    /// Worst (largest) facet aspect ratio over the whole mesh. REPORTED, not
    /// gated: a faithful developable lateral is legitimately *tall* (~20), so a
    /// high aspect alone is not a defect — it's a readout for the agent.
    pub worst_aspect_ratio: f64,
    /// Smallest interior facet angle over the whole mesh, degrees. REPORTED.
    pub min_angle_deg: f64,
    /// Largest angle between a facet's winding normal and the TRUE surface normal
    /// at its centroid, degrees. **GATED** — a bridging wing points tens of
    /// degrees off-surface while a faithful (even tall) facet stays within a few.
    pub max_normal_deviation_deg: f64,
    /// Facets crossing an analytical boundary — on a periodic/closed lateral, a
    /// facet whose UV spans more than half the period bridges across the interior
    /// (the bore "wing"). **GATED** — boundary conformance is a hard rule.
    pub boundary_crossing_facets: usize,
    /// The single worst face, the agent's pointer to the defect. `None` if clean.
    pub worst_face: Option<MeshFaceQualityDefect>,
    /// `true` when no facet crosses a boundary and the worst normal deviation is
    /// within [`Self::MAX_NORMAL_DEVIATION_DEG`].
    pub clean: bool,
}

impl MeshQuality {
    /// A facet whose winding normal strays beyond this from the true surface
    /// normal is off-surface (a bridge / fold). Well above a faithful developable
    /// facet (a few degrees) and below a folded one (tens of degrees).
    pub const MAX_NORMAL_DEVIATION_DEG: f64 = 40.0;

    /// Conservative aspect-ratio ceiling — a faithful developable lateral is
    /// legitimately tall (~20) and a planar fan can reach ~30, so this is set
    /// well above both to catch only a GROSS sliver "wing" (measured ~290 on the
    /// imported bore), not the merely-tall.
    pub const MAX_ASPECT_RATIO: f64 = 100.0;

    /// Conservative minimum-angle floor, degrees — a faithful developable facet's
    /// apex angle is ~2.8° and a fan's is ~1-2°, while a sliver "wing" collapses
    /// toward 0°. Set below the faithful floor so only a degenerate-shape facet
    /// trips it.
    pub const MIN_ANGLE_DEG: f64 = 0.5;

    pub fn evaluate(
        triangles: usize,
        worst_aspect_ratio: f64,
        min_angle_deg: f64,
        max_normal_deviation_deg: f64,
        boundary_crossing_facets: usize,
        worst_face: Option<MeshFaceQualityDefect>,
    ) -> Self {
        // `clean` (which gates `is_sound`) keys ONLY on the true mesh-TOPOLOGY
        // defects — a facet that bridges across a periodic lateral
        // (`boundary_crossing`) or folds far off the surface (`normal_deviation`).
        // Both are 0 on a clean part. Aspect ratio and min-angle are SHAPE-quality
        // readouts (reported via `worst_aspect_ratio`/`min_angle_deg` + the worst
        // face), NOT gates: a faithful tessellation routinely carries aspect ~150-
        // 290 slivers (planar fans near curved boundaries, the developable
        // collapse), so gating on them would flag every real part. They are the
        // OPTIMISATION ORACLE — the number to drive down — not a soundness bar.
        let clean = boundary_crossing_facets == 0
            && max_normal_deviation_deg <= Self::MAX_NORMAL_DEVIATION_DEG;
        Self {
            triangles,
            worst_aspect_ratio,
            min_angle_deg,
            max_normal_deviation_deg,
            boundary_crossing_facets,
            worst_face,
            clean,
        }
    }

    /// Verdict for a solid that tessellates to nothing — already flagged unsound
    /// by `watertight`, so reported clean (quality not-applicable).
    pub fn empty() -> Self {
        Self {
            triangles: 0,
            worst_aspect_ratio: 1.0,
            min_angle_deg: 60.0,
            max_normal_deviation_deg: 0.0,
            boundary_crossing_facets: 0,
            worst_face: None,
            clean: true,
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
    /// Display-tessellation quality — the render-mesh analogue of the topological
    /// checks above. A degenerate / inverted-normal mesh (the inner-bore scribble)
    /// is a real defect the closure/manifold checks cannot see; `clean == false`
    /// blocks `is_sound()` so the kernel cannot certify a solid that renders wrong.
    pub tessellation: TessellationQuality,
    /// Mesh-quality verdict — the render mesh against the CAD tessellation rules
    /// (boundary conformance, normal deviation, aspect/min-angle). A facet that
    /// bridges across a bore (the "wing") or crosses a face boundary is caught
    /// here even though it is watertight + correctly oriented; `clean == false`
    /// blocks `is_sound()`.
    pub mesh_quality: MeshQuality,
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
            && self.tessellation.clean
            && self.mesh_quality.clean
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
            "solid {} — origin={} designed={} sound={} (brep_valid={} watertight={} manifold={} euler={} construction={} labels={} tess_clean={} normal_agreement={:.3} degenerate={})",
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
            self.certificate.tessellation.clean,
            self.certificate.tessellation.normal_agreement,
            self.certificate.tessellation.degenerate_triangles,
        ) + &format!(
            " analytic_agreement={:.3} off_surface={} | mesh_clean={} worst_aspect={:.1} min_angle={:.1} max_normal_dev={:.1} boundary_crossing={}",
            self.certificate.tessellation.analytic_normal_agreement,
            self.certificate.tessellation.off_surface_facets,
            self.certificate.mesh_quality.clean,
            self.certificate.mesh_quality.worst_aspect_ratio,
            self.certificate.mesh_quality.min_angle_deg,
            self.certificate.mesh_quality.max_normal_deviation_deg,
            self.certificate.mesh_quality.boundary_crossing_facets,
        )
    }
}
