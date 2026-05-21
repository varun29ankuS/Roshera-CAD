//! Solid representation for B-Rep topology.
//!
//! Features:
//! - Boolean operations (union, intersection, difference)
//! - Feature recognition and suppression
//! - Parametric history tracking via timeline events
//! - Multi-resolution representations
//! - Solid healing and repair
//! - Mass properties with material support
//! - Collision-detection acceleration structures
//! - Feature-based modeling operations
//!
//! Indexed access into shell/face enumeration arrays is the canonical idiom
//! — bounded by topology length. Matches the pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use crate::math::{consts, MathResult, Matrix4, Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::CurveStore,
    edge::{EdgeId, EdgeStore},
    face::{FaceId, FaceStore},
    r#loop::LoopStore,
    shell::{MassProperties, ShellId, ShellStore},
    surface::SurfaceStore,
    vertex::{VertexId, VertexStore},
};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Solid ID type
pub type SolidId = u32;

/// Invalid solid ID constant
pub const INVALID_SOLID_ID: SolidId = u32::MAX;

/// Material properties
#[derive(Debug, Clone)]
pub struct Material {
    /// Material name
    pub name: String,
    /// Density (kg/m³)
    pub density: f64,
    /// Young's modulus (Pa)
    pub youngs_modulus: f64,
    /// Poisson's ratio
    pub poissons_ratio: f64,
    /// Thermal expansion coefficient (1/K)
    pub thermal_expansion: f64,
    /// Custom properties
    pub properties: HashMap<String, f64>,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            name: "Steel".to_string(),
            density: 7850.0,       // kg/m³
            youngs_modulus: 200e9, // Pa
            poissons_ratio: 0.3,
            thermal_expansion: 12e-6, // 1/K
            properties: HashMap::new(),
        }
    }
}

/// Feature types for feature-based modeling
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FeatureType {
    Hole,
    Boss,
    Pocket,
    Rib,
    Slot,
    Chamfer,
    Fillet,
    Thread,
    Pattern,
    Shell,
    Draft,
    Custom,
}

/// Feature in solid
#[derive(Debug, Clone)]
pub struct Feature {
    /// Feature ID
    pub id: u32,
    /// Feature type
    pub feature_type: FeatureType,
    /// Faces belonging to this feature
    pub faces: Vec<FaceId>,
    /// Parent feature (if dependent)
    pub parent: Option<u32>,
    /// Feature parameters
    pub parameters: HashMap<String, f64>,
    /// Is feature suppressed
    pub suppressed: bool,
}

/// Solid attributes
#[derive(Debug, Clone)]
pub struct SolidAttributes {
    /// Display color (RGBA)
    pub color: [f32; 4],
    /// Material
    pub material: Material,
    /// Layer ID
    pub layer: Option<u32>,
    /// Visibility
    pub visible: bool,
    /// Selection state (currently selected by user)
    pub selected: bool,
    /// Selectable (whether user input may select this solid; locked solids
    /// stay visible but cannot be picked)
    pub selectable: bool,
    /// User-defined attributes
    pub user_data: HashMap<String, String>,
}

impl Default for SolidAttributes {
    fn default() -> Self {
        Self {
            color: [0.7, 0.7, 0.7, 1.0],
            material: Material::default(),
            layer: None,
            visible: true,
            selected: false,
            selectable: true,
            user_data: HashMap::new(),
        }
    }
}

/// Provenance of a [`SolidMassProperties`] report.
///
/// Mirrors Parasolid's `PK_TOPOL_eval_mass_props` "method" indicator so
/// agents and downstream tooling can tell whether the numbers came
/// from a closed-form analytical integral over the B-Rep faces (exact
/// to floating-point noise for planar-faced solids) or from numerical
/// integration over the tessellated surface (mesh-density-bounded
/// tolerance, used as a fallback when analytical loop traversal
/// aborts on degenerate seam loops — the situation produced by every
/// curved primitive: sphere, cylinder, cone, torus).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MassPropertiesMethod {
    /// Computed from analytical face-by-face divergence-theorem
    /// traversal of the B-Rep. Exact to floating-point noise for
    /// planar-faced solids (box, prism, polyhedron).
    Analytical,
    /// Computed from numerical integration over the tessellated
    /// solid using Tonon (2004) per-tetrahedron formulas. Used when
    /// analytical traversal would fail on degenerate seam loops
    /// (curved primitives). `rel_tolerance` is the empirical bound
    /// vs analytical formulas at the tessellation resolution that
    /// was used.
    Tessellated {
        /// Empirical relative-error bound at the tessellation
        /// resolution used to integrate (≈ 5e-3 at `TessellationParams::fine()`
        /// per the calibration in `tests/kernel_workflow_regression.rs`).
        rel_tolerance: f64,
    },
}

/// Mass properties for solid
#[derive(Debug, Clone)]
pub struct SolidMassProperties {
    /// Volume
    pub volume: f64,
    /// Total surface area (outer shell + inner shells; inner-shell faces
    /// contribute their full area because they bound the solid's interior
    /// voids).
    pub surface_area: f64,
    /// Mass (using material density)
    pub mass: f64,
    /// Center of mass
    pub center_of_mass: Point3,
    /// Moments of inertia about center of mass
    pub inertia_tensor: [[f64; 3]; 3],
    /// Principal moments
    pub principal_moments: Vector3,
    /// Principal axes (column vectors)
    pub principal_axes: [Vector3; 3],
    /// Radius of gyration
    pub radius_of_gyration: Vector3,
    /// How this report was computed — analytical face traversal or
    /// numerical mesh integration. Surfaced on the wire so agents
    /// can decide whether to trust the tail digits.
    pub method: MassPropertiesMethod,
}

/// Solid statistics
#[derive(Debug, Clone)]
pub struct SolidStats {
    /// Number of shells
    pub shell_count: usize,
    /// Number of faces
    pub face_count: usize,
    /// Number of edges
    pub edge_count: usize,
    /// Number of vertices
    pub vertex_count: usize,
    /// Number of features
    pub feature_count: usize,
    /// Euler characteristic
    pub euler_characteristic: i32,
    /// Genus
    pub genus: i32,
    /// Bounding box
    pub bbox_min: Point3,
    pub bbox_max: Point3,
}

/// Boolean operation type
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BooleanOp {
    Union,
    Intersection,
    Difference,
    SymmetricDifference,
}

/// History node for parametric modeling
#[derive(Debug, Clone)]
pub struct HistoryNode {
    /// Operation ID
    pub id: u32,
    /// Operation type
    pub operation: String,
    /// Input solids
    pub inputs: Vec<SolidId>,
    /// Output solid
    pub output: SolidId,
    /// Parameters
    pub parameters: HashMap<String, serde_json::Value>,
    /// Timestamp
    pub timestamp: std::time::SystemTime,
}

/// Anchoring metadata that records *which* datum a primitive was placed
/// against and the local-frame transform applied on top of that datum's
/// frame.
///
/// Geometry is still stored in world coordinates — the anchor does not
/// change vertex positions. It is bookkeeping that lets the kernel,
/// the API, and downstream LLM-readable surfaces answer the question
/// "what was this solid placed against?" without re-deriving it from
/// raw vertex coordinates.
///
/// The composed world transform applied to the primitive's vertices at
/// creation time is `datum.frame() * local_transform`.
#[derive(Debug, Clone, PartialEq)]
pub struct SolidAnchor {
    /// Id of the datum this solid is anchored to. Default-seeded model
    /// guarantees `0` is the world Origin.
    pub datum_id: u32,
    /// Local-frame placement on top of the datum's frame. Identity
    /// means "placed at the datum's origin, axes aligned with the
    /// datum's axes".
    pub local_transform: Matrix4,
}

impl SolidAnchor {
    /// Anchor at the world Origin (datum id 0) with no local offset.
    /// Used as the canonical default on every newly-constructed `Solid`
    /// — anchoring is mandatory (slice 3a), so primitives that are not
    /// explicitly placed against a datum land here.
    pub fn world_origin() -> Self {
        Self {
            datum_id: 0,
            local_transform: Matrix4::identity(),
        }
    }
}

/// CF-α — which blend operation kind a [`Solid`] has already accepted
/// at a given edge or vertex. Stored on [`Solid::blended_edges`] and
/// [`Solid::blended_vertices`], consumed by the lifecycle pre-flight
/// gate `operations::lifecycle::validate_blend_conflict` to surface a
/// typed [`BlendFailure::ConflictingBlendKind`] when a caller asks for
/// a fillet on an edge that already carried a chamfer (or vice versa).
///
/// The enum lives here — at the storage layer where the registry
/// physically resides — rather than in `operations::diagnostics`, to
/// avoid a `primitives → operations → primitives` cycle. The
/// diagnostics module re-exports it so call-site imports stay
/// single-source: `use crate::operations::diagnostics::BlendKind;`.
///
/// [`BlendFailure::ConflictingBlendKind`]: crate::operations::diagnostics::BlendFailure::ConflictingBlendKind
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum BlendKind {
    /// Smooth (G1) rolling-ball / spine-marched blend.
    Fillet,
    /// Flat n-gon cap (planar for N=2/3, miter-pre-pass + planar
    /// cap for N≥4 convex corners — Chamfer-β).
    Chamfer,
}

impl std::fmt::Display for BlendKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlendKind::Fillet => write!(f, "fillet"),
            BlendKind::Chamfer => write!(f, "chamfer"),
        }
    }
}

/// CF-β — the set of blend kinds applied at a single vertex. The
/// CF-α registry stored exactly one [`BlendKind`] per vertex, which
/// modelled "this corner has been blended" but could not represent
/// the mixed case "this corner has been chamfered on edge A and
/// filleted on edge B". CF-β unlocks degree-3 convex equal-displacement
/// mixed-kind corners (chamfer on one edge, fillet on the other two,
/// stitched by a planar hexagonal cap); the registry shape lifts to
/// this newtype so the lifecycle gate and the corner-cap dispatch can
/// distinguish "single-kind so far" from "already mixed".
///
/// Stored as `(bool, bool)` rather than `EnumSet<BlendKind>` to avoid
/// pulling a new dependency for a two-element domain. `Copy` so the
/// existing `Solid::deep_copy` clones it for free; serde-derived so
/// the field round-trips through any serialiser that touches `Solid`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize,
)]
pub struct VertexBlendKindSet {
    /// Whether a fillet has been applied at this vertex.
    pub has_fillet: bool,
    /// Whether a chamfer has been applied at this vertex.
    pub has_chamfer: bool,
}

impl VertexBlendKindSet {
    /// A set carrying exactly `kind`.
    pub fn single(kind: BlendKind) -> Self {
        let mut s = Self::default();
        s.insert(kind);
        s
    }

    /// Insert `kind`; idempotent.
    pub fn insert(&mut self, kind: BlendKind) {
        match kind {
            BlendKind::Fillet => self.has_fillet = true,
            BlendKind::Chamfer => self.has_chamfer = true,
        }
    }

    /// Whether `kind` is in the set.
    pub fn contains(&self, kind: BlendKind) -> bool {
        match kind {
            BlendKind::Fillet => self.has_fillet,
            BlendKind::Chamfer => self.has_chamfer,
        }
    }

    /// Both kinds present.
    pub fn is_mixed(&self) -> bool {
        self.has_fillet && self.has_chamfer
    }

    /// Number of distinct kinds in the set (0, 1, or 2).
    pub fn len(&self) -> usize {
        usize::from(self.has_fillet) + usize::from(self.has_chamfer)
    }

    /// True iff no kind has been recorded.
    pub fn is_empty(&self) -> bool {
        !self.has_fillet && !self.has_chamfer
    }

    /// If exactly one kind is present, return it; otherwise `None`.
    /// Used by callers (e.g. CF-α legacy paths) that only support a
    /// single-kind vertex and want to reject the mixed case upstream.
    pub fn as_single(&self) -> Option<BlendKind> {
        match (self.has_fillet, self.has_chamfer) {
            (true, false) => Some(BlendKind::Fillet),
            (false, true) => Some(BlendKind::Chamfer),
            _ => None,
        }
    }
}

impl std::fmt::Display for VertexBlendKindSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.has_fillet, self.has_chamfer) {
            (false, false) => write!(f, "∅"),
            (true, false) => write!(f, "{{fillet}}"),
            (false, true) => write!(f, "{{chamfer}}"),
            (true, true) => write!(f, "{{chamfer, fillet}}"),
        }
    }
}

/// Solid representation
#[derive(Debug, Clone)]
pub struct Solid {
    /// Unique identifier
    pub id: SolidId,
    /// Outer shell (defines exterior boundary)
    pub outer_shell: ShellId,
    /// Inner shells (voids)
    pub inner_shells: Vec<ShellId>,
    /// Name/label
    pub name: Option<String>,
    /// Features
    features: Arc<RwLock<HashMap<u32, Feature>>>,
    /// Attributes
    pub attributes: SolidAttributes,
    /// Datum anchoring metadata. Every solid carries an anchor — solids
    /// created without an explicit datum (legacy creators, derived
    /// solids from booleans / extrudes) default to `SolidAnchor::world_origin()`
    /// (datum id 0 + identity local transform). This guarantees agents
    /// and downstream queries always have a reference frame to reason
    /// against, never raw world coordinates.
    pub anchor: SolidAnchor,
    /// Cached mass properties
    cached_mass_props: Option<SolidMassProperties>,
    /// Cached statistics
    cached_stats: Option<SolidStats>,
    /// Parent assembly (if part of assembly)
    pub parent_assembly: Option<u32>,
    /// Parametric history
    history: Arc<RwLock<Vec<HistoryNode>>>,
    /// Collision acceleration structure (e.g., OBB tree)
    collision_tree: Option<Arc<CollisionTree>>,
    /// CF-α — per-edge blend registry. Each entry records the
    /// [`BlendKind`] applied to the edge ID *as it was at the moment
    /// of the blend call*, so the `validate_blend_conflict` pre-flight
    /// gate in `operations::lifecycle` can reject a subsequent blend
    /// request on the same (now-destroyed) edge with a typed
    /// [`BlendFailure::ConflictingBlendKind`] instead of the legacy
    /// `edge not found in model` string surface.
    ///
    /// Populated by `fillet::fillet_edges` and
    /// `chamfer::chamfer_edges` on successful completion, before
    /// `record_operation`. Cleared / re-populated never — entries
    /// accumulate across the solid's life; the registry survives
    /// `deep_copy` for snapshot fidelity.
    ///
    /// [`BlendFailure::ConflictingBlendKind`]: crate::operations::diagnostics::BlendFailure::ConflictingBlendKind
    pub(crate) blended_edges: HashMap<EdgeId, BlendKind>,
    /// CF-α — per-vertex blend registry. Mirrors `blended_edges` for
    /// the corners that participate in a vertex blend (F5-α three-edge
    /// convex apex sphere, Chamfer-α planar cap, Chamfer-β n-gon cap)
    /// and survive into the post-surgery topology (only when the
    /// `original_v*_corner_shared` flag fires — otherwise the vertex
    /// is removed by `splice_blend_edge` and never lands here).
    ///
    /// Used to detect cross-kind conflict at a corner shared between
    /// a previously-blended edge and the edge currently being requested.
    ///
    /// **CF-β** lifts the value type from `BlendKind` to
    /// [`VertexBlendKindSet`]: a single corner can carry both a fillet
    /// (on one edge) and a chamfer (on another) simultaneously. The
    /// lifecycle gate consults [`VertexBlendKindSet::contains`] to
    /// decide between "same-kind reuse (allowed)", "cross-kind
    /// feasibility pre-flight (CF-β)", and "blanket reject (CF-α
    /// fallback)".
    pub(crate) blended_vertices: HashMap<VertexId, VertexBlendKindSet>,
    /// CF-β.3 — per-face blend registry. Records the [`BlendKind`]
    /// every blend face was emitted by, so the mixed-kind corner cap
    /// synthesizer can locate the surviving chamfer face(s) and
    /// fillet face(s) incident to a shared corner without re-deriving
    /// the classification from surface geometry.
    ///
    /// Populated by `chamfer::chamfer_edges` (entries tagged
    /// [`BlendKind::Chamfer`]) and `fillet::fillet_edges` (entries
    /// tagged [`BlendKind::Fillet`]) once the surgery has emitted the
    /// blend face IDs and before `record_operation`. Look-ups go
    /// through [`Self::blend_kind_at_face`] and the per-vertex
    /// helpers [`Self::chamfer_faces_at_vertex`] /
    /// [`Self::fillet_faces_at_vertex`].
    pub(crate) blend_faces_by_kind: HashMap<FaceId, BlendKind>,
    /// CF-β.4 — vertices left with a deliberate, partially-blended open
    /// boundary by the *first* of two kind-mismatched calls at the same
    /// corner. The second call's dispatch hook
    /// (`mixed_kind_corner_cap::synthesize_mixed_kind_corner_cap`) closes
    /// the boundary and removes the vertex from this map; the
    /// post-operation `validate_result` gate consults the keys to
    /// short-circuit non-manifold-edge and Euler-characteristic checks
    /// at edges incident to a pending corner, so the intermediate state
    /// passes validation without weakening the gate for everything else.
    ///
    /// CF-β.5.2-B: the *value* is the corner vertex's incident-edge
    /// degree captured at opt-in time, **before** the first call's
    /// surgery destroyed any of those edges. The feasibility pre-flight
    /// (`lifecycle::validate_mixed_kind_corner_feasibility`) consults
    /// this stored degree on the *second* call so it can reach the
    /// degree-3 carve-out even after the first call's splice reduced
    /// the per-edge incident count.
    ///
    /// Mutated by [`Self::mark_pending_mixed_kind_corner`] (first call,
    /// after surgery) and [`Self::clear_pending_mixed_kind_corner`]
    /// (second call, after cap synthesis). Idempotent on both sides.
    /// Survives [`Self::deep_copy`] so timeline replay preserves the
    /// intermediate-state expectation across snapshots.
    pub(crate) pending_mixed_kind_corners: HashMap<VertexId, usize>,
}

/// Top-level AABB used as a conservative collision proxy. The detailed
/// hierarchical descent (per-face BVH / OBB nodes) lives in
/// `topology::accel_tree`; this struct caches only the root box so the
/// `Solid::collides_with` fast path can reject pairs whose world-space
/// extents do not overlap without ever touching the topology stores.
#[derive(Debug)]
pub struct CollisionTree {
    /// Root axis-aligned bounding box (min, max) in world coordinates.
    pub root_bbox: (Point3, Point3),
}

impl Solid {
    /// Create new solid
    pub fn new(id: SolidId, outer_shell: ShellId) -> Self {
        Self {
            id,
            outer_shell,
            inner_shells: Vec::new(),
            name: None,
            features: Arc::new(RwLock::new(HashMap::new())),
            attributes: SolidAttributes::default(),
            anchor: SolidAnchor::world_origin(),
            cached_mass_props: None,
            cached_stats: None,
            parent_assembly: None,
            history: Arc::new(RwLock::new(Vec::new())),
            collision_tree: None,
            blended_edges: HashMap::new(),
            blended_vertices: HashMap::new(),
            blend_faces_by_kind: HashMap::new(),
            pending_mixed_kind_corners: HashMap::new(),
        }
    }

    /// Create named solid with material
    pub fn new_with_material(
        id: SolidId,
        outer_shell: ShellId,
        name: String,
        material: Material,
    ) -> Self {
        let mut solid = Self::new(id, outer_shell);
        solid.name = Some(name);
        solid.attributes.material = material;
        solid
    }

    /// Deep copy of this solid for the F2-δ ModelSnapshot primitive.
    ///
    /// The derived `Clone` impl is unsafe for snapshotting: `features`
    /// and `history` are `Arc<RwLock<…>>` and a derived clone hands
    /// back the same underlying allocation, so a later mutation
    /// through `add_feature` / history push would leak into the
    /// snapshot. This method snapshots the inner contents under a
    /// read guard and rewraps each in a fresh `Arc<RwLock<…>>`.
    /// `collision_tree` is logically read-only after construction
    /// (`Solid::compute_collision_tree` always replaces, never mutates
    /// the inner), so `Arc::clone` is safe; we keep that as a
    /// reference-bump.
    pub(crate) fn deep_copy(&self) -> Self {
        let features_snapshot = self.features.read().clone();
        let history_snapshot = self.history.read().clone();
        Self {
            id: self.id,
            outer_shell: self.outer_shell,
            inner_shells: self.inner_shells.clone(),
            name: self.name.clone(),
            features: Arc::new(RwLock::new(features_snapshot)),
            attributes: self.attributes.clone(),
            anchor: self.anchor.clone(),
            cached_mass_props: self.cached_mass_props.clone(),
            cached_stats: self.cached_stats.clone(),
            parent_assembly: self.parent_assembly,
            history: Arc::new(RwLock::new(history_snapshot)),
            collision_tree: self.collision_tree.clone(),
            blended_edges: self.blended_edges.clone(),
            blended_vertices: self.blended_vertices.clone(),
            blend_faces_by_kind: self.blend_faces_by_kind.clone(),
            pending_mixed_kind_corners: self.pending_mixed_kind_corners.clone(),
        }
    }

    /// CF-α — record that `edge` carried a successful blend of `kind`
    /// on this solid. Called by `operations::fillet::fillet_edges` and
    /// `operations::chamfer::chamfer_edges` after surgery succeeds,
    /// before `record_operation`. Idempotent on `(edge, kind)`; a
    /// later call with the same kind overwrites silently. A later
    /// call with a different kind also overwrites — the conflict gate
    /// runs *pre-flight* in `lifecycle::validate_blend_conflict`, so
    /// by the time we reach this writer the kernel has already
    /// validated the absence of a cross-kind clash.
    pub(crate) fn record_blended_edge(&mut self, edge: EdgeId, kind: BlendKind) {
        self.blended_edges.insert(edge, kind);
    }

    /// CF-α / CF-β — record that `vertex` participates in a successful
    /// blend of `kind` on this solid. Only called for vertices that
    /// survive `splice_blend_edge` (the `original_v*_corner_shared`
    /// flag is set), since others are destroyed and would never be
    /// looked up.
    ///
    /// Idempotent and order-independent: under CF-β the same vertex
    /// can be visited by a fillet call and a chamfer call (in either
    /// order); each call inserts its kind into the per-vertex
    /// [`VertexBlendKindSet`], leaving the other slot untouched.
    pub(crate) fn record_blended_vertex(&mut self, vertex: VertexId, kind: BlendKind) {
        self.blended_vertices.entry(vertex).or_default().insert(kind);
    }

    /// CF-α — look up the previously-applied blend kind at `edge`, if
    /// any. Used by `lifecycle::validate_blend_conflict` to test
    /// whether the requested kind clashes with what the registry
    /// already holds.
    pub fn blend_kind_at_edge(&self, edge: EdgeId) -> Option<BlendKind> {
        self.blended_edges.get(&edge).copied()
    }

    /// CF-α — look up the previously-applied blend kind at `vertex`
    /// when *exactly one* kind has been recorded there. Returns
    /// `None` when the vertex has never participated in a blend on
    /// this solid **or** when both fillet and chamfer have been
    /// recorded (the CF-β mixed case).
    ///
    /// Callers that need to handle the mixed case must use
    /// [`Self::vertex_blend_set`] instead.
    pub fn blend_kind_at_vertex_single(&self, vertex: VertexId) -> Option<BlendKind> {
        self.blended_vertices
            .get(&vertex)
            .and_then(VertexBlendKindSet::as_single)
    }

    /// CF-β — look up the full set of blend kinds recorded at `vertex`.
    /// Returns `None` when the vertex has never participated in a
    /// blend, `Some(set)` otherwise. The returned set may carry one
    /// kind (single-kind CF-α corner) or both (CF-β mixed corner).
    pub fn vertex_blend_set(&self, vertex: VertexId) -> Option<VertexBlendKindSet> {
        self.blended_vertices.get(&vertex).copied()
    }

    /// CF-β.3 — record that the newly-emitted blend face `face` is of
    /// kind `kind`. Called by `chamfer::chamfer_edges` once per
    /// chamfer trim face (and once per planar cap face) and by
    /// `fillet::fillet_edges` once per fillet transition face on
    /// successful completion, before `record_operation`. Idempotent;
    /// a re-emission with the same kind overwrites silently. A re-
    /// emission with a different kind would be a kernel defect (a
    /// face cannot belong to two distinct blends) and overwrites the
    /// last writer; the gate against that is upstream in the
    /// per-edge / per-vertex registries.
    pub(crate) fn record_blend_face(&mut self, face: FaceId, kind: BlendKind) {
        self.blend_faces_by_kind.insert(face, kind);
    }

    /// CF-β.3 — look up the [`BlendKind`] that produced `face`, if
    /// any. Used by the mixed-kind corner cap synthesizer to filter
    /// the faces incident to a shared corner into chamfer-rim and
    /// fillet-rim subsets without re-deriving the classification
    /// from surface geometry.
    pub fn blend_kind_at_face(&self, face: FaceId) -> Option<BlendKind> {
        self.blend_faces_by_kind.get(&face).copied()
    }

    /// CF-β.4 — mark `vertex` as carrying a deliberate, partially-
    /// blended open boundary contributed by the first of two kind-
    /// mismatched blend calls. Called by `chamfer::chamfer_edges` /
    /// `fillet::fillet_edges` after surgery, before `record_operation`,
    /// for every corner vertex that was preserved by the
    /// `original_v*_corner_shared` flag *and* whose
    /// [`VertexBlendKindSet`] is single-kind after the call (i.e. the
    /// matching opposite-kind call has not yet fired).
    ///
    /// CF-β.5.2-B: `original_degree` is the corner's incident-edge
    /// count captured *before* surgery destroyed any of those edges.
    /// The feasibility pre-flight uses it to reach the degree-3 carve-
    /// out on the second call. Idempotent: a second call with the same
    /// `(vertex, degree)` overwrites silently. A call with a different
    /// `degree` for the same vertex also overwrites — kernel internals
    /// invariably re-capture from the same pre-surgery model.
    pub(crate) fn mark_pending_mixed_kind_corner(
        &mut self,
        vertex: VertexId,
        original_degree: usize,
    ) {
        self.pending_mixed_kind_corners.insert(vertex, original_degree);
    }

    /// CF-β.4 — clear `vertex` from the pending map once the matching
    /// opposite-kind call has fired
    /// [`super::super::operations::mixed_kind_corner_cap::synthesize_mixed_kind_corner_cap`]
    /// and the open boundary is closed by a stitched cap face.
    /// Idempotent — clearing a vertex that was never pending is a
    /// no-op (returns `false`).
    pub(crate) fn clear_pending_mixed_kind_corner(&mut self, vertex: VertexId) -> bool {
        self.pending_mixed_kind_corners.remove(&vertex).is_some()
    }

    /// CF-β.4 — query whether `vertex` currently carries a partially-
    /// blended open boundary that the `validate_result` gate should
    /// tolerate. The post-operation validator
    /// (`chamfer::validate_chamfered_solid` /
    /// `fillet::validate_filleted_solid`) consults this to short-
    /// circuit non-manifold-edge and Euler-characteristic checks at
    /// edges incident to a pending corner.
    pub fn is_mixed_kind_corner_pending(&self, vertex: VertexId) -> bool {
        self.pending_mixed_kind_corners.contains_key(&vertex)
    }

    /// CF-β.5.2-B — read the original incident-edge degree captured
    /// for `vertex` at the *first* of two kind-mismatched calls (the
    /// one that opted V in via `partial_corner_vertices`). Returns
    /// `None` when `vertex` is not pending. Used by
    /// [`lifecycle::validate_mixed_kind_corner_feasibility`] to bypass
    /// the per-current-edge degree count (which is meaningless on the
    /// second call because surgery has already destroyed half the
    /// corner's edges) and apply the degree-3 carve-out using the
    /// pre-surgery topology.
    pub fn pending_corner_original_degree(&self, vertex: VertexId) -> Option<usize> {
        self.pending_mixed_kind_corners.get(&vertex).copied()
    }

    /// CF-β.4 — read-only borrow of the full pending-corners map.
    /// Used by `record_operation` to serialise the intermediate-state
    /// expectation into the timeline event payload, so a replay can
    /// reproduce the same partially-blended boundary. Callers that
    /// only need the vertex keys (without the captured degrees)
    /// iterate `.keys()`.
    pub fn pending_mixed_kind_corners(&self) -> &HashMap<VertexId, usize> {
        &self.pending_mixed_kind_corners
    }

    /// Add inner shell (void)
    pub fn add_inner_shell(&mut self, shell_id: ShellId) {
        self.inner_shells.push(shell_id);
        self.invalidate_cache();
    }

    /// Remove inner shell
    pub fn remove_inner_shell(&mut self, shell_id: ShellId) -> bool {
        if let Some(pos) = self.inner_shells.iter().position(|&id| id == shell_id) {
            self.inner_shells.remove(pos);
            self.invalidate_cache();
            true
        } else {
            false
        }
    }

    /// Invalidate cached data
    fn invalidate_cache(&mut self) {
        self.cached_mass_props = None;
        self.cached_stats = None;
        self.collision_tree = None;
    }

    /// Add feature
    pub fn add_feature(&mut self, feature: Feature) -> u32 {
        let id = feature.id;
        {
            let mut features = self.features.write();
            features.insert(id, feature);
        } // Lock is dropped here
        self.invalidate_cache();
        id
    }

    /// Suppress/unsuppress feature
    pub fn suppress_feature(&mut self, feature_id: u32, suppress: bool) -> bool {
        let result = {
            let mut features = self.features.write();
            if let Some(feature) = features.get_mut(&feature_id) {
                feature.suppressed = suppress;
                true
            } else {
                false
            }
        }; // Lock is dropped here

        if result {
            self.invalidate_cache();
        }
        result
    }

    /// Get feature by ID
    pub fn get_feature(&self, feature_id: u32) -> Option<Feature> {
        let features = self.features.read();
        features.get(&feature_id).cloned()
    }

    /// Get features by type
    pub fn get_features_by_type(&self, feature_type: FeatureType) -> Vec<Feature> {
        let features = self.features.read();
        features
            .values()
            .filter(|f| f.feature_type == feature_type && !f.suppressed)
            .cloned()
            .collect()
    }

    /// Add history node
    pub fn add_history(&mut self, node: HistoryNode) {
        let mut history = self.history.write();
        history.push(node);
    }

    /// Get parametric history
    pub fn get_history(&self) -> Vec<HistoryNode> {
        let history = self.history.read();
        history.clone()
    }

    /// Compute solid statistics (cached)
    #[allow(clippy::expect_used)] // cached_stats populated immediately above when None
    pub fn compute_stats(
        &mut self,
        shell_store: &ShellStore,
        face_store: &FaceStore,
        loop_store: &LoopStore,
        edge_store: &EdgeStore,
        vertex_store: &VertexStore,
    ) -> MathResult<&SolidStats> {
        if self.cached_stats.is_none() {
            let mut total_faces = 0;
            let mut total_edges = HashSet::new();
            let mut total_vertices = HashSet::new();
            let mut min_pt = Point3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
            let mut max_pt = Point3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);

            for &shell_id in &self.all_shells() {
                if let Some(shell) = shell_store.get(shell_id) {
                    total_faces += shell.faces.len();

                    for &face_id in &shell.faces {
                        if let Some(face) = face_store.get(face_id) {
                            for &loop_id in &face.all_loops() {
                                if let Some(loop_) = loop_store.get(loop_id) {
                                    for &edge_id in &loop_.edges {
                                        total_edges.insert(edge_id);

                                        if let Some(edge) = edge_store.get(edge_id) {
                                            total_vertices.insert(edge.start_vertex);
                                            total_vertices.insert(edge.end_vertex);

                                            // Update bounding box
                                            if let Some(v) = vertex_store.get(edge.start_vertex) {
                                                let p = Point3::from(v.position);
                                                min_pt = min_pt.min(&p);
                                                max_pt = max_pt.max(&p);
                                            }
                                            if let Some(v) = vertex_store.get(edge.end_vertex) {
                                                let p = Point3::from(v.position);
                                                min_pt = min_pt.min(&p);
                                                max_pt = max_pt.max(&p);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            let v = total_vertices.len() as i32;
            let e = total_edges.len() as i32;
            let f = total_faces as i32;
            let euler = v - e + f;

            // For a solid with g handles and c cavities: χ = 2 - 2g - c
            // For simple solid: χ = 2, so g = 0
            let genus = (2 - euler) / 2;

            let features = self.features.read();

            self.cached_stats = Some(SolidStats {
                shell_count: 1 + self.inner_shells.len(),
                face_count: total_faces,
                edge_count: total_edges.len(),
                vertex_count: total_vertices.len(),
                feature_count: features.len(),
                euler_characteristic: euler,
                genus,
                bbox_min: min_pt,
                bbox_max: max_pt,
            });
        }

        Ok(self
            .cached_stats
            .as_ref()
            .expect("cached_stats populated above when None"))
    }

    /// Calculate mass properties (cached)
    #[allow(clippy::expect_used)] // cached_mass_props populated immediately above when None
    pub fn compute_mass_properties(
        &mut self,
        shell_store: &mut ShellStore,
        face_store: &mut FaceStore,
        loop_store: &mut LoopStore,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
        curve_store: &CurveStore,
        surface_store: &SurfaceStore,
    ) -> MathResult<&SolidMassProperties> {
        if self.cached_mass_props.is_none() {
            // Calculate volume using divergence theorem
            let mut volume = 0.0;
            let mut surface_area = 0.0;
            let mut center = Vector3::ZERO;
            let mut volume_integrals = VolumeIntegrals::default();

            // Process outer shell
            if let Some(shell) = shell_store.get_mut(self.outer_shell) {
                let shell_props = shell.compute_mass_properties(
                    face_store,
                    loop_store,
                    vertex_store,
                    edge_store,
                    curve_store,
                    surface_store,
                    1.0, // Unit density for now
                )?;

                if let Some(v) = shell_props.volume {
                    volume += v;
                    center += shell_props.center_of_mass.to_vec() * v;

                    // Add to volume integrals
                    volume_integrals.add_shell_contribution(shell_props, 1.0);
                }
                // Surface area always accumulates positively — the outer
                // shell's faces bound the solid from outside.
                surface_area += shell_props.surface_area;
            }

            // Subtract inner shells
            for &inner_id in &self.inner_shells {
                if let Some(shell) = shell_store.get_mut(inner_id) {
                    let shell_props = shell.compute_mass_properties(
                        face_store,
                        loop_store,
                        vertex_store,
                        edge_store,
                        curve_store,
                        surface_store,
                        1.0,
                    )?;

                    if let Some(v) = shell_props.volume {
                        volume -= v;
                        center -= shell_props.center_of_mass.to_vec() * v;

                        // Subtract from volume integrals
                        volume_integrals.add_shell_contribution(shell_props, -1.0);
                    }
                    // Inner-shell faces also bound the solid (from inside,
                    // around voids) — they add to total surface area, not
                    // subtract. A hollow box has more surface area than a
                    // solid box of the same outer dimensions.
                    surface_area += shell_props.surface_area;
                }
            }

            // Calculate mass
            let mass = volume * self.attributes.material.density;

            // Center of mass
            let center_of_mass = if volume > consts::EPSILON {
                Point3::from(center / volume)
            } else {
                // Use bounding box center for degenerate case
                if let Some(stats) = &self.cached_stats {
                    Point3::from((stats.bbox_min.to_vec() + stats.bbox_max.to_vec()) * 0.5)
                } else {
                    Point3::ZERO
                }
            };

            // Calculate inertia tensor
            let inertia_tensor = volume_integrals.compute_inertia_tensor(mass, &center_of_mass);

            // Compute principal moments and axes (eigenvalues/eigenvectors)
            let (principal_moments, principal_axes) = compute_principal_inertia(&inertia_tensor);

            // Radius of gyration
            let radius_of_gyration = Vector3::new(
                (principal_moments.x / mass).sqrt(),
                (principal_moments.y / mass).sqrt(),
                (principal_moments.z / mass).sqrt(),
            );

            self.cached_mass_props = Some(SolidMassProperties {
                volume,
                surface_area,
                mass,
                center_of_mass,
                inertia_tensor,
                principal_moments,
                principal_axes,
                radius_of_gyration,
                method: MassPropertiesMethod::Analytical,
            });
        }

        Ok(self
            .cached_mass_props
            .as_ref()
            .expect("cached_mass_props populated above when None"))
    }

    /// Install a pre-computed [`SolidMassProperties`] into the cache.
    ///
    /// Used exclusively by [`crate::primitives::topology_builder::BRepModel::compute_solid_mass_properties`]
    /// when the analytical path fails and the mesh-based fallback
    /// produces a [`MassPropertiesMethod::Tessellated`] report. Cache
    /// invalidation continues to flow through [`Self::invalidate_cache`]
    /// — no separate path is needed.
    pub(crate) fn install_mass_props_cache(&mut self, props: SolidMassProperties) {
        self.cached_mass_props = Some(props);
    }

    /// Read-only access to the cached mass-properties report, if any.
    ///
    /// Used by [`crate::primitives::topology_builder::BRepModel::compute_solid_mass_properties`]
    /// to short-circuit the mesh fallback when a prior call has already
    /// populated the cache.
    pub(crate) fn cached_mass_props_ref(&self) -> Option<&SolidMassProperties> {
        self.cached_mass_props.as_ref()
    }

    /// Drop the cached mass-properties report.
    ///
    /// Called by topology-mutating operations (fillet, chamfer, boolean,
    /// shell, …) whose shell-level changes invalidate the cached volume,
    /// surface area, COM, and inertia tensor. The next
    /// [`crate::primitives::topology_builder::BRepModel::compute_solid_mass_properties`]
    /// call re-runs the mesh integration from scratch.
    pub(crate) fn invalidate_mass_props_cache(&mut self) {
        self.cached_mass_props = None;
    }

    /// Transform solid
    pub fn transform(&mut self, matrix: &Matrix4) -> MathResult<()> {
        // Transform would modify all vertices
        // This is a high-level operation that would delegate to lower levels
        self.invalidate_cache();

        // Add to history
        let history_id = {
            let history = self.history.read();
            history.len() as u32
        }; // Lock is dropped here

        self.add_history(HistoryNode {
            id: history_id,
            operation: "Transform".to_string(),
            inputs: vec![self.id],
            output: self.id,
            parameters: {
                let mut params = HashMap::new();
                // Convert Matrix4 to array for serialization
                let matrix_array: [[f64; 4]; 4] = [
                    [
                        matrix.get(0, 0),
                        matrix.get(0, 1),
                        matrix.get(0, 2),
                        matrix.get(0, 3),
                    ],
                    [
                        matrix.get(1, 0),
                        matrix.get(1, 1),
                        matrix.get(1, 2),
                        matrix.get(1, 3),
                    ],
                    [
                        matrix.get(2, 0),
                        matrix.get(2, 1),
                        matrix.get(2, 2),
                        matrix.get(2, 3),
                    ],
                    [
                        matrix.get(3, 0),
                        matrix.get(3, 1),
                        matrix.get(3, 2),
                        matrix.get(3, 3),
                    ],
                ];
                params.insert("matrix".to_string(), serde_json::json!(matrix_array));
                params
            },
            timestamp: std::time::SystemTime::now(),
        });

        Ok(())
    }

    // Note: fillet, chamfer, and shell are not exposed as Solid methods.
    // The kernel routes those through `crate::operations::{fillet,chamfer,shell}`
    // which operate on a `BRepModel` (the only place that owns the topology
    // stores needed to mutate edges/faces/loops). A method on `Solid` would
    // duplicate the entry point and could only ever record a Feature without
    // updating the actual geometry — see commit history.

    /// Build collision tree for fast intersection tests
    pub fn build_collision_tree(&mut self) -> MathResult<()> {
        if let Some(stats) = &self.cached_stats {
            self.collision_tree = Some(Arc::new(CollisionTree {
                root_bbox: (stats.bbox_min, stats.bbox_max),
            }));
        }
        Ok(())
    }

    /// Fast collision check with another solid
    pub fn collides_with(&self, other: &Solid) -> bool {
        // Quick bbox check first
        if let (Some(stats1), Some(stats2)) = (&self.cached_stats, &other.cached_stats) {
            // Check if bounding boxes overlap
            if stats1.bbox_max.x < stats2.bbox_min.x
                || stats1.bbox_min.x > stats2.bbox_max.x
                || stats1.bbox_max.y < stats2.bbox_min.y
                || stats1.bbox_min.y > stats2.bbox_max.y
                || stats1.bbox_max.z < stats2.bbox_min.z
                || stats1.bbox_min.z > stats2.bbox_max.z
            {
                return false;
            }
        }

        // If bboxes overlap, would do detailed check using collision trees
        true
    }
}

// Preserve original methods for compatibility
impl Solid {
    pub fn all_shells(&self) -> Vec<ShellId> {
        let mut shells = vec![self.outer_shell];
        shells.extend(&self.inner_shells);
        shells
    }

    #[inline]
    pub fn has_voids(&self) -> bool {
        !self.inner_shells.is_empty()
    }

    /// Get all shell IDs (alias for all_shells for compatibility)
    #[inline]
    pub fn shell_ids(&self) -> Vec<ShellId> {
        self.all_shells()
    }

    #[inline]
    pub fn shell_count(&self) -> usize {
        1 + self.inner_shells.len()
    }

    pub fn volume(
        &mut self,
        shell_store: &mut ShellStore,
        face_store: &mut FaceStore,
        loop_store: &mut LoopStore,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
        surface_store: &SurfaceStore,
        _tolerance: Tolerance,
    ) -> MathResult<f64> {
        let props = self.compute_mass_properties(
            shell_store,
            face_store,
            loop_store,
            vertex_store,
            edge_store,
            &CurveStore::new(),
            surface_store,
        )?;
        Ok(props.volume)
    }

    /// Total surface area (outer shell + inner-shell void boundaries).
    ///
    /// Delegates to [`Self::compute_mass_properties`] so the result is
    /// served from the same cache as `volume` / `mass` / `inertia` and
    /// never diverges from them. The `tolerance` parameter is retained
    /// for API stability but no longer consulted — surface area is
    /// derived from the same divergence-theorem face traversal that
    /// computes the rest of the mass properties.
    pub fn surface_area(
        &mut self,
        shell_store: &mut ShellStore,
        face_store: &mut FaceStore,
        loop_store: &mut LoopStore,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
        curve_store: &CurveStore,
        surface_store: &SurfaceStore,
        _tolerance: Tolerance,
    ) -> MathResult<f64> {
        let props = self.compute_mass_properties(
            shell_store,
            face_store,
            loop_store,
            vertex_store,
            edge_store,
            curve_store,
            surface_store,
        )?;
        Ok(props.surface_area)
    }

    pub fn bounding_box(
        &mut self,
        shell_store: &ShellStore,
        face_store: &FaceStore,
        loop_store: &LoopStore,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
    ) -> MathResult<(Point3, Point3)> {
        let stats = self.compute_stats(
            shell_store,
            face_store,
            loop_store,
            edge_store,
            vertex_store,
        )?;
        Ok((stats.bbox_min, stats.bbox_max))
    }

    pub fn center(
        &mut self,
        shell_store: &ShellStore,
        face_store: &FaceStore,
        loop_store: &LoopStore,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
    ) -> MathResult<Point3> {
        let stats = self.compute_stats(
            shell_store,
            face_store,
            loop_store,
            edge_store,
            vertex_store,
        )?;
        Ok(Point3::from(
            (stats.bbox_min.to_vec() + stats.bbox_max.to_vec()) * 0.5,
        ))
    }
}

/// Volume integrals for rigid-body mass properties: volume V = ∫dV,
/// first moments ∫r dV (which yield centre-of-mass after dividing by V),
/// and second-moment tensor ∫(r ⊗ r) dV (which yields the inertia tensor
/// after the parallel-axis shift to the COM frame). This is the full set
/// of integrals required for the standard rigid-body mass-properties
/// pipeline; anything beyond this (third moments, higher inertia
/// tensors) is not consumed by the kernel or any downstream module.
#[derive(Debug, Default)]
struct VolumeIntegrals {
    volume: f64,
    first_moments: Vector3,
    second_moments: [[f64; 3]; 3],
}

impl VolumeIntegrals {
    fn add_shell_contribution(&mut self, shell_props: &MassProperties, sign: f64) {
        if let Some(v) = shell_props.volume {
            self.volume += sign * v;
            self.first_moments += shell_props.center_of_mass.to_vec() * (sign * v);

            // Add inertia contributions
            for i in 0..3 {
                for j in 0..3 {
                    self.second_moments[i][j] += sign * shell_props.inertia[i][j];
                }
            }
        }
    }

    /// Translate the accumulated origin-frame inertia tensor to the
    /// center-of-mass frame via the parallel-axis theorem (Goldstein,
    /// *Classical Mechanics*, §5.3):
    ///
    /// ```text
    /// I_C = I_O − m · [(c·c) I₃ − c ⊗ c]
    /// ```
    ///
    /// where `I_O` is `self.second_moments` (computed about the origin
    /// during volume-integral accumulation), `c` is the center of mass,
    /// `m` is the total mass, `I₃` is the 3×3 identity, and `⊗` denotes
    /// the outer product. Returns the symmetric tensor about COM.
    fn compute_inertia_tensor(&self, mass: f64, center_of_mass: &Point3) -> [[f64; 3]; 3] {
        let c = [center_of_mass.x, center_of_mass.y, center_of_mass.z];
        let cc = c[0] * c[0] + c[1] * c[1] + c[2] * c[2];

        let mut i_com = self.second_moments;
        for i in 0..3 {
            for j in 0..3 {
                let kron = if i == j { 1.0 } else { 0.0 };
                // Subtract m·[ (c·c) δ_ij − c_i c_j ]
                i_com[i][j] -= mass * (cc * kron - c[i] * c[j]);
            }
        }
        i_com
    }
}

/// Compute principal moments (eigenvalues) and principal axes
/// (eigenvectors) of a 3×3 symmetric inertia tensor by Jacobi rotations
/// (Press et al., *Numerical Recipes*, §11.1).
///
/// Returns `(principal_moments, principal_axes)` where eigenvalues are
/// sorted in descending order and `principal_axes[k]` is the unit
/// eigenvector corresponding to `principal_moments[k]`. The axes form
/// an orthonormal frame.
///
/// Convergence: at most `MAX_SWEEPS` cyclic sweeps; in practice 5–10
/// sweeps suffice for the off-diagonal sum to drop below `EPS`.
fn compute_principal_inertia(inertia: &[[f64; 3]; 3]) -> (Vector3, [Vector3; 3]) {
    const MAX_SWEEPS: usize = 50;
    const EPS: f64 = 1e-14;

    // Working copy of the symmetric matrix; eigenvalues end up on diag.
    let mut a = *inertia;
    // Eigenvector matrix accumulator, starts as identity.
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

    for _ in 0..MAX_SWEEPS {
        // Off-diagonal Frobenius norm squared.
        let off = a[0][1] * a[0][1] + a[0][2] * a[0][2] + a[1][2] * a[1][2];
        if off < EPS {
            break;
        }

        // Rotate every off-diagonal pair (cyclic Jacobi).
        for &(p, q) in &[(0usize, 1usize), (0, 2), (1, 2)] {
            let apq = a[p][q];
            if apq.abs() < EPS {
                continue;
            }
            let app = a[p][p];
            let aqq = a[q][q];

            // Givens rotation angle that zeroes a[p][q]:
            //   tan(2θ) = 2 a_pq / (a_pp − a_qq)
            // Numerically stable form (Press §11.1).
            let theta = (aqq - app) / (2.0 * apq);
            let t = if theta >= 0.0 {
                1.0 / (theta + (1.0 + theta * theta).sqrt())
            } else {
                1.0 / (theta - (1.0 + theta * theta).sqrt())
            };
            let c = 1.0 / (1.0 + t * t).sqrt();
            let s = t * c;

            // Update diagonals; off-diagonal a[p][q] becomes 0.
            a[p][p] = app - t * apq;
            a[q][q] = aqq + t * apq;
            a[p][q] = 0.0;
            a[q][p] = 0.0;

            // Update remaining row/column entries (the third index r).
            for r in 0..3 {
                if r != p && r != q {
                    let arp = a[r][p];
                    let arq = a[r][q];
                    a[r][p] = c * arp - s * arq;
                    a[p][r] = a[r][p];
                    a[r][q] = s * arp + c * arq;
                    a[q][r] = a[r][q];
                }
            }

            // Accumulate rotation into eigenvector matrix.
            for r in 0..3 {
                let vrp = v[r][p];
                let vrq = v[r][q];
                v[r][p] = c * vrp - s * vrq;
                v[r][q] = s * vrp + c * vrq;
            }
        }
    }

    // Sort eigenvalues descending; permute eigenvectors in lockstep.
    let mut idx = [0usize, 1, 2];
    idx.sort_by(|&i, &j| {
        a[j][j]
            .partial_cmp(&a[i][i])
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let principal_moments = Vector3::new(a[idx[0]][idx[0]], a[idx[1]][idx[1]], a[idx[2]][idx[2]]);
    let principal_axes = [
        Vector3::new(v[0][idx[0]], v[1][idx[0]], v[2][idx[0]]),
        Vector3::new(v[0][idx[1]], v[1][idx[1]], v[2][idx[1]]),
        Vector3::new(v[0][idx[2]], v[1][idx[2]], v[2][idx[2]]),
    ];

    (principal_moments, principal_axes)
}

/// Given the **origin-frame** inertia tensor (`I_xx = ∫(y² + z²) dV`,
/// `I_xy = -∫xy dV`, etc.), shift it to the centre-of-mass frame via
/// the parallel-axis theorem and return the eigendecomposition.
///
/// Exposed at module scope so the mesh-based mass-properties pipeline
/// in [`crate::primitives::topology_builder::BRepModel::compute_solid_mass_properties`]
/// can reuse the same parallel-axis-shift + Jacobi solver the
/// analytical path uses, guaranteeing the inertia tensor returned to
/// agents is consistent across the two methods.
///
/// Returns `(inertia_tensor_at_com, principal_moments, principal_axes)`
/// — same triple of shapes the analytical pipeline computes.
pub(crate) fn principal_axes_from_origin_moments(
    i_origin: [[f64; 3]; 3],
    mass: f64,
    com: &Point3,
) -> ([[f64; 3]; 3], Vector3, [Vector3; 3]) {
    // Parallel-axis shift: I_C = I_O − m · [(c·c) I₃ − c ⊗ c]
    // (Goldstein, *Classical Mechanics*, §5.3).
    let c = [com.x, com.y, com.z];
    let cc = c[0] * c[0] + c[1] * c[1] + c[2] * c[2];
    let mut i_com = i_origin;
    for i in 0..3 {
        for j in 0..3 {
            let kron = if i == j { 1.0 } else { 0.0 };
            i_com[i][j] -= mass * (cc * kron - c[i] * c[j]);
        }
    }
    let (principal_moments, principal_axes) = compute_principal_inertia(&i_com);
    (i_com, principal_moments, principal_axes)
}

/// Solid storage with feature indexing
#[derive(Debug)]
pub struct SolidStore {
    /// Solid data
    solids: Vec<Solid>,
    /// Name to solid mapping
    name_map: HashMap<String, SolidId>,
    /// Shell to solids mapping
    shell_to_solids: HashMap<ShellId, Vec<SolidId>>,
    /// Next available ID
    next_id: SolidId,
    /// Statistics
    pub stats: SolidStoreStats,
}

#[derive(Debug, Default, Clone)]
pub struct SolidStoreStats {
    pub total_created: u64,
    pub boolean_operations: u64,
    pub feature_operations: u64,
    pub collision_checks: u64,
}

impl SolidStore {
    pub fn new() -> Self {
        Self::with_capacity(0)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            solids: Vec::with_capacity(capacity),
            name_map: HashMap::new(),
            shell_to_solids: HashMap::new(),
            next_id: 0,
            stats: SolidStoreStats::default(),
        }
    }

    /// Deep copy of this store for the F2-δ ModelSnapshot primitive.
    /// Defers to `Solid::deep_copy` so that each solid's
    /// `Arc<RwLock<…>>` features and history are unshared — see
    /// the note on `Solid::deep_copy`.
    pub(crate) fn deep_copy(&self) -> Self {
        Self {
            solids: self.solids.iter().map(Solid::deep_copy).collect(),
            name_map: self.name_map.clone(),
            shell_to_solids: self.shell_to_solids.clone(),
            next_id: self.next_id,
            stats: self.stats.clone(),
        }
    }

    /// Add solid with MAXIMUM SPEED - no DashMap operations
    #[inline(always)]
    pub fn add(&mut self, mut solid: Solid) -> SolidId {
        solid.id = self.next_id;

        // FAST PATH: Skip expensive DashMap operations
        // The shell_to_solids and name_map DashMap operations are too expensive for primitive creation

        self.solids.push(solid);
        self.next_id += 1;
        self.stats.total_created += 1;

        self.next_id - 1
    }

    /// Add solid with full indexing (use when queries are needed)
    pub fn add_with_indexing(&mut self, mut solid: Solid) -> SolidId {
        solid.id = self.next_id;

        // Update indices - expensive DashMap operations
        if let Some(name) = &solid.name {
            self.name_map.insert(name.clone(), solid.id);
        }

        for &shell_id in &solid.all_shells() {
            self.shell_to_solids
                .entry(shell_id)
                .or_default()
                .push(solid.id);
        }

        self.solids.push(solid);
        self.next_id += 1;
        self.stats.total_created += 1;

        self.next_id - 1
    }

    #[inline(always)]
    pub fn get(&self, id: SolidId) -> Option<&Solid> {
        self.solids.get(id as usize)
    }

    #[inline(always)]
    pub fn get_mut(&mut self, id: SolidId) -> Option<&mut Solid> {
        self.solids.get_mut(id as usize)
    }

    /// Remove a solid from the store
    pub fn remove(&mut self, id: SolidId) -> Option<Solid> {
        // Check bounds first
        if (id as usize) >= self.solids.len() {
            return None;
        }

        // Get solid data before removal to avoid borrowing issues
        let solid_name = self.solids[id as usize].name.clone();
        let outer_shell = self.solids[id as usize].outer_shell;

        // Remove from name mapping
        if let Some(name) = &solid_name {
            self.name_map.remove(name);
        }

        // Remove from shell mapping
        self.shell_to_solids.entry(outer_shell).and_modify(|v| {
            v.retain(|&x| x != id);
        });

        // Remove the actual solid
        let solid = self.solids.remove(id as usize);

        // Update IDs of remaining solids
        for (i, solid) in self.solids.iter_mut().enumerate().skip(id as usize) {
            solid.id = i as SolidId;
        }

        Some(solid)
    }

    #[inline]
    pub fn find_by_name(&self, name: &str) -> Option<SolidId> {
        self.name_map.get(name).copied()
    }

    #[inline]
    pub fn solids_with_shell(&self, shell_id: ShellId) -> &[SolidId] {
        self.shell_to_solids
            .get(&shell_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.solids.len()
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.solids.is_empty()
    }

    /// Iterate over all solids
    pub fn iter(&self) -> impl Iterator<Item = (SolidId, &Solid)> + '_ {
        self.solids
            .iter()
            .enumerate()
            .filter(|(_, s)| s.id != INVALID_SOLID_ID)
            .map(|(idx, s)| (idx as SolidId, s))
    }
}

impl Default for SolidStore {
    fn default() -> Self {
        Self::new()
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_material() {
//         let mat = Material::default();
//         assert_eq!(mat.name, "Steel");
//         assert_eq!(mat.density, 7850.0);
//     }
//
//     #[test]
//     fn test_feature() {
//         let feature = Feature {
//             id: 0,
//             feature_type: FeatureType::Hole,
//             faces: vec![1, 2, 3],
//             parent: None,
//             parameters: HashMap::new(),
//             suppressed: false,
//         };
//
//         assert_eq!(feature.feature_type, FeatureType::Hole);
//         assert!(!feature.suppressed);
//     }
//
//     #[test]
//     fn test_solid_with_material() {
//         let material = Material {
//             name: "Aluminum".to_string(),
//             density: 2700.0,
//             ..Default::default()
//         };
//
//         let solid = Solid::new_with_material(
//             0,
//             0,
//             "Part1".to_string(),
//             material,
//         );
//
//         assert_eq!(solid.name, Some("Part1".to_string()));
//         assert_eq!(solid.attributes.material.density, 2700.0);
//     }
//
//     #[test]
//     fn test_solid_features() {
//         let mut solid = Solid::new(0, 0);
//
//         let feature = Feature {
//             id: 0,
//             feature_type: FeatureType::Fillet,
//             faces: vec![10, 11],
//             parent: None,
//             parameters: {
//                 let mut params = HashMap::new();
//                 params.insert("radius".to_string(), 5.0);
//                 params
//             },
//             suppressed: false,
//         };
//
//         solid.add_feature(feature);
//
//         let fillets = solid.get_features_by_type(FeatureType::Fillet);
//         assert_eq!(fillets.len(), 1);
//         assert_eq!(fillets[0].parameters.get("radius"), Some(&5.0));
//     }
//
//     #[test]
//     fn test_collision_check() {
//         let mut solid1 = Solid::new(0, 0);
//         let mut solid2 = Solid::new(1, 1);
//
//         // Set up bounding boxes
//         solid1.cached_stats = Some(SolidStats {
//             shell_count: 1,
//             face_count: 6,
//             edge_count: 12,
//             vertex_count: 8,
//             feature_count: 0,
//             euler_characteristic: 2,
//             genus: 0,
//             bbox_min: Point3::new(0.0, 0.0, 0.0),
//             bbox_max: Point3::new(1.0, 1.0, 1.0),
//         });
//
//         solid2.cached_stats = Some(SolidStats {
//             shell_count: 1,
//             face_count: 6,
//             edge_count: 12,
//             vertex_count: 8,
//             feature_count: 0,
//             euler_characteristic: 2,
//             genus: 0,
//             bbox_min: Point3::new(2.0, 2.0, 2.0),
//             bbox_max: Point3::new(3.0, 3.0, 3.0),
//         });
//
//         // Should not collide
//         assert!(!solid1.collides_with(&solid2));
//
//         // Overlapping boxes
//         solid2.cached_stats.as_mut().unwrap().bbox_min = Point3::new(0.5, 0.5, 0.5);
//         assert!(solid1.collides_with(&solid2));
//     }
// }

/// Validation result for solids
#[derive(Debug, Clone)]
pub struct SolidValidation {
    pub is_valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

#[cfg(test)]
mod inertia_tests {
    use super::*;

    /// Diagonal inertia tensor must round-trip through Jacobi unchanged
    /// (off-diagonals already zero ⇒ no rotations applied).
    #[test]
    fn diagonal_tensor_returns_diagonal_eigenvalues_descending() {
        let inertia = [[3.0, 0.0, 0.0], [0.0, 7.0, 0.0], [0.0, 0.0, 5.0]];
        let (moments, axes) = compute_principal_inertia(&inertia);

        // Sorted descending
        assert!((moments.x - 7.0).abs() < 1e-10);
        assert!((moments.y - 5.0).abs() < 1e-10);
        assert!((moments.z - 3.0).abs() < 1e-10);

        // Axes must be permuted unit vectors (up to sign).
        for axis in &axes {
            let m = axis.magnitude();
            assert!((m - 1.0).abs() < 1e-10, "axis not unit length: {}", m);
        }
    }

    /// Verify eigendecomposition reconstructs the original tensor:
    /// I ≈ V · diag(λ) · V^T for a non-trivially coupled symmetric matrix.
    #[test]
    fn jacobi_reconstructs_symmetric_matrix() {
        let inertia = [[4.0, 1.0, -2.0], [1.0, 6.0, 0.5], [-2.0, 0.5, 8.0]];
        let (moments, axes) = compute_principal_inertia(&inertia);

        let lambda = [moments.x, moments.y, moments.z];
        // Reconstruct: A_ij = sum_k λ_k v_ki v_kj
        let v: [[f64; 3]; 3] = [
            [axes[0].x, axes[0].y, axes[0].z],
            [axes[1].x, axes[1].y, axes[1].z],
            [axes[2].x, axes[2].y, axes[2].z],
        ];
        for i in 0..3 {
            for j in 0..3 {
                let mut acc = 0.0;
                for k in 0..3 {
                    acc += lambda[k] * v[k][i] * v[k][j];
                }
                assert!(
                    (acc - inertia[i][j]).abs() < 1e-9,
                    "reconstruction failed at ({}, {}): got {}, expected {}",
                    i,
                    j,
                    acc,
                    inertia[i][j]
                );
            }
        }
    }

    /// Eigenvectors must be mutually orthogonal (Jacobi guarantees this
    /// for symmetric input by construction, but the test pins it down).
    #[test]
    fn eigenvectors_form_orthonormal_frame() {
        let inertia = [[10.0, 3.0, 1.0], [3.0, 5.0, -2.0], [1.0, -2.0, 7.0]];
        let (_, axes) = compute_principal_inertia(&inertia);

        for i in 0..3 {
            for j in (i + 1)..3 {
                let dot = axes[i].dot(&axes[j]);
                assert!(
                    dot.abs() < 1e-9,
                    "axes {} and {} not orthogonal: dot = {}",
                    i,
                    j,
                    dot
                );
            }
        }
    }

    /// Parallel-axis transform: a point mass at distance `r` from the
    /// origin contributes `m·r²` to the off-COM moment about each
    /// transverse axis. After translating to COM the inertia drops to
    /// the body's intrinsic inertia (zero for a point mass).
    #[test]
    fn parallel_axis_subtracts_offset_correctly() {
        // Point mass m at (2, 0, 0): I_origin =
        //   [[0, 0, 0], [0, m·4, 0], [0, 0, m·4]]
        let m = 3.0;
        let integrals = VolumeIntegrals {
            volume: 1.0,
            first_moments: Vector3::new(2.0, 0.0, 0.0),
            second_moments: [[0.0, 0.0, 0.0], [0.0, m * 4.0, 0.0], [0.0, 0.0, m * 4.0]],
        };
        let com = Point3::new(2.0, 0.0, 0.0);
        let i_com = integrals.compute_inertia_tensor(m, &com);

        // Should be ≈ zero tensor (point mass has no intrinsic inertia).
        for i in 0..3 {
            for j in 0..3 {
                assert!(
                    i_com[i][j].abs() < 1e-10,
                    "expected zero at ({}, {}), got {}",
                    i,
                    j,
                    i_com[i][j]
                );
            }
        }
    }
}

#[cfg(test)]
mod vertex_blend_kind_set_tests {
    //! CF-β.1 — pin the [`VertexBlendKindSet`] contract that lifts the
    //! CF-α single-kind registry to a two-element set so a single
    //! corner vertex can carry both a fillet (on one edge) and a
    //! chamfer (on another) simultaneously.

    use super::{BlendKind, VertexBlendKindSet};

    #[test]
    fn insert_idempotent_and_order_independent() {
        let mut a = VertexBlendKindSet::default();
        a.insert(BlendKind::Fillet);
        a.insert(BlendKind::Fillet); // idempotent
        assert!(a.contains(BlendKind::Fillet));
        assert!(!a.contains(BlendKind::Chamfer));
        assert_eq!(a.len(), 1);
        assert!(!a.is_mixed());

        let mut b = VertexBlendKindSet::default();
        b.insert(BlendKind::Chamfer);
        b.insert(BlendKind::Fillet);
        let mut c = VertexBlendKindSet::default();
        c.insert(BlendKind::Fillet);
        c.insert(BlendKind::Chamfer);
        assert_eq!(b, c, "set membership must be order-independent");
    }

    #[test]
    fn is_mixed_only_when_both_set() {
        let empty = VertexBlendKindSet::default();
        assert!(!empty.is_mixed());
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);
        assert_eq!(empty.as_single(), None);

        let single_fillet = VertexBlendKindSet::single(BlendKind::Fillet);
        assert!(!single_fillet.is_mixed());
        assert_eq!(single_fillet.len(), 1);
        assert_eq!(single_fillet.as_single(), Some(BlendKind::Fillet));

        let single_chamfer = VertexBlendKindSet::single(BlendKind::Chamfer);
        assert!(!single_chamfer.is_mixed());
        assert_eq!(single_chamfer.as_single(), Some(BlendKind::Chamfer));

        let mut mixed = single_fillet;
        mixed.insert(BlendKind::Chamfer);
        assert!(mixed.is_mixed());
        assert_eq!(mixed.len(), 2);
        assert_eq!(
            mixed.as_single(),
            None,
            "as_single must collapse to None for the CF-β mixed case"
        );
    }

    #[test]
    fn serde_round_trip_preserves_all_four_states() {
        for set in [
            VertexBlendKindSet::default(),
            VertexBlendKindSet::single(BlendKind::Fillet),
            VertexBlendKindSet::single(BlendKind::Chamfer),
            VertexBlendKindSet {
                has_fillet: true,
                has_chamfer: true,
            },
        ] {
            let json = serde_json::to_string(&set)
                .expect("VertexBlendKindSet serialises");
            let back: VertexBlendKindSet = serde_json::from_str(&json)
                .expect("VertexBlendKindSet deserialises");
            assert_eq!(set, back, "round-trip mismatch for {set}");
        }
    }
}

#[cfg(test)]
mod blend_faces_by_kind_tests {
    //! CF-β.3 — pin the per-face blend registry that lets the
    //! mixed-kind corner cap synthesizer filter the faces incident
    //! to a shared corner into chamfer-rim and fillet-rim subsets
    //! without re-deriving the classification from surface geometry.

    use super::{BlendKind, ShellId, Solid, SolidId};

    fn empty_solid() -> Solid {
        Solid::new(0 as SolidId, 0 as ShellId)
    }

    #[test]
    fn record_blend_face_round_trips_through_lookup() {
        let mut s = empty_solid();
        s.record_blend_face(7, BlendKind::Chamfer);
        s.record_blend_face(11, BlendKind::Fillet);
        assert_eq!(s.blend_kind_at_face(7), Some(BlendKind::Chamfer));
        assert_eq!(s.blend_kind_at_face(11), Some(BlendKind::Fillet));
        assert_eq!(s.blend_kind_at_face(99), None);
    }

    #[test]
    fn record_blend_face_idempotent_on_same_kind() {
        let mut s = empty_solid();
        s.record_blend_face(3, BlendKind::Fillet);
        s.record_blend_face(3, BlendKind::Fillet);
        assert_eq!(s.blend_kind_at_face(3), Some(BlendKind::Fillet));
        assert_eq!(s.blend_faces_by_kind.len(), 1);
    }

    #[test]
    fn deep_copy_preserves_blend_faces_by_kind() {
        let mut s = empty_solid();
        s.record_blend_face(5, BlendKind::Chamfer);
        s.record_blend_face(9, BlendKind::Fillet);
        let snap = s.deep_copy();
        assert_eq!(snap.blend_kind_at_face(5), Some(BlendKind::Chamfer));
        assert_eq!(snap.blend_kind_at_face(9), Some(BlendKind::Fillet));
        // Mutating the original after the snapshot must not leak in.
        s.record_blend_face(5, BlendKind::Fillet);
        assert_eq!(
            snap.blend_kind_at_face(5),
            Some(BlendKind::Chamfer),
            "snapshot must own its blend_faces_by_kind clone"
        );
    }
}

#[cfg(test)]
mod pending_mixed_kind_corners_tests {
    //! CF-β.4 — pin the per-solid pending-mixed-kind-corner registry
    //! that lets the post-operation `validate_result` gate tolerate the
    //! deliberate open boundary left by the first of two kind-
    //! mismatched blend calls at the same corner. The second call's
    //! synthesizer (`mixed_kind_corner_cap::synthesize_mixed_kind_corner_cap`)
    //! is responsible for clearing the entry once the cap is stitched.

    use super::{ShellId, Solid, SolidId};

    fn empty_solid() -> Solid {
        Solid::new(0 as SolidId, 0 as ShellId)
    }

    #[test]
    fn mark_then_query_round_trips() {
        let mut s = empty_solid();
        assert!(!s.is_mixed_kind_corner_pending(17));
        s.mark_pending_mixed_kind_corner(17, 3);
        assert!(s.is_mixed_kind_corner_pending(17));
        assert!(!s.is_mixed_kind_corner_pending(18));
        assert_eq!(s.pending_corner_original_degree(17), Some(3));
        assert_eq!(s.pending_corner_original_degree(18), None);
    }

    #[test]
    fn mark_is_idempotent() {
        let mut s = empty_solid();
        s.mark_pending_mixed_kind_corner(3, 3);
        s.mark_pending_mixed_kind_corner(3, 3);
        assert_eq!(s.pending_mixed_kind_corners().len(), 1);
    }

    #[test]
    fn clear_returns_true_only_when_present() {
        let mut s = empty_solid();
        s.mark_pending_mixed_kind_corner(5, 3);
        assert!(s.clear_pending_mixed_kind_corner(5));
        assert!(!s.is_mixed_kind_corner_pending(5));
        // Second clear is a no-op.
        assert!(!s.clear_pending_mixed_kind_corner(5));
        // Clearing a never-marked vertex is also a no-op.
        assert!(!s.clear_pending_mixed_kind_corner(99));
    }

    #[test]
    fn deep_copy_preserves_pending_set() {
        let mut s = empty_solid();
        s.mark_pending_mixed_kind_corner(11, 3);
        s.mark_pending_mixed_kind_corner(13, 4);
        let snap = s.deep_copy();
        assert!(snap.is_mixed_kind_corner_pending(11));
        assert!(snap.is_mixed_kind_corner_pending(13));
        assert_eq!(snap.pending_corner_original_degree(13), Some(4));
        // Mutating the original after the snapshot must not leak in.
        s.clear_pending_mixed_kind_corner(11);
        assert!(
            snap.is_mixed_kind_corner_pending(11),
            "snapshot must own its pending_mixed_kind_corners clone"
        );
    }

    #[test]
    fn pending_corners_getter_reflects_current_state() {
        let mut s = empty_solid();
        assert!(s.pending_mixed_kind_corners().is_empty());
        s.mark_pending_mixed_kind_corner(1, 3);
        s.mark_pending_mixed_kind_corner(2, 3);
        s.mark_pending_mixed_kind_corner(3, 4);
        assert_eq!(s.pending_mixed_kind_corners().len(), 3);
        s.clear_pending_mixed_kind_corner(2);
        assert_eq!(s.pending_mixed_kind_corners().len(), 2);
        assert!(s.pending_mixed_kind_corners().contains_key(&1));
        assert!(s.pending_mixed_kind_corners().contains_key(&3));
        assert!(!s.pending_mixed_kind_corners().contains_key(&2));
        assert_eq!(s.pending_corner_original_degree(3), Some(4));
    }
}
