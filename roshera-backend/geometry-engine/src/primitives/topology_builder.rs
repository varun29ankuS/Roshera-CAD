//! Universal Topology Builder for 2D and 3D Primitives
//!
//! This module provides the core infrastructure for building watertight B-Rep
//! topology for all primitive types, both 2D and 3D, with timeline support.
//!
//! Indexed access into vertex/edge/face buffers built during primitive
//! construction is bounds-guaranteed by the known topology of each primitive
//! (box=8v/12e/6f, cylinder=2N+2v, etc). All `arr[i]` sites use indices
//! derived from the construction loop counters.
#![allow(clippy::indexing_slicing)]

use crate::math::{Matrix4, Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::{Circle, CurveStore, Line, ParameterRange},
    edge::{Edge, EdgeId, EdgeOrientation, EdgeStore},
    face::{Face, FaceId, FaceOrientation, FaceStore},
    primitive_traits::PrimitiveError,
    r#loop::{Loop, LoopId, LoopStore, LoopType},
    shell::{Shell, ShellId, ShellStore, ShellType},
    solid::{Solid, SolidId, SolidStore},
    surface::{Cylinder, Plane, Sphere, Surface, SurfaceStore},
    vertex::{VertexId, VertexStore},
};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::LazyLock;

/// Verify a computed mass-properties result against the PHYSICAL contract of a
/// real rigid body, returning `Err(reason)` with a machine-readable cause when
/// any check fails so the caller can REFUSE (rather than emit a lie).
///
/// A genuine solid's mass-properties satisfy:
///  1. volume and surface area are positive and finite;
///  2. the inertia tensor is symmetric (`I[i][j] == I[j][i]`);
///  3. its principal moments (eigenvalues of the symmetric tensor) are all
///     finite and NON-NEGATIVE — the tensor is positive-semidefinite; and
///  4. the rigid-body triangle inequality holds on the principal moments
///     (`Ii + Ij ≥ Ik` for every pairing) — a real mass distribution cannot
///     violate it even when the tensor is symmetric.
///
/// Tolerances are scaled to the tensor magnitude (so a large part is not held to
/// an absolute epsilon) and take their *relative* band from the kernel's default
/// [`Tolerance`] distance term rather than a hardcoded literal at the call site.
/// A symmetric, near-degenerate eigenproblem
/// can leave a tiny negative epsilon on one eigenvalue or a hair of asymmetry;
/// those are tolerated. A gross violation (a large negative moment, an
/// asymmetric tensor, a broken triangle inequality) is the husk signature and is
/// rejected.
fn check_mass_properties_physical(
    volume: f64,
    surface_area: f64,
    inertia_tensor: &[[f64; 3]; 3],
    principal_moments: Vector3,
) -> Result<(), String> {
    // Relative band: derive a dimensionless ratio from the kernel's default
    // distance tolerance rather than hardcoding `1e-6`. NORMAL_TOLERANCE's
    // distance is ~1e-6 model units; we use it directly as the relative band on
    // tensor-scaled quantities (the tensor is already a magnitude, so the band
    // is a fraction of it).
    let rel_band = Tolerance::default().distance().max(f64::EPSILON);

    if !(volume.is_finite() && volume > 0.0) {
        return Err(format!("volume not positive-finite: {volume}"));
    }
    if !(surface_area.is_finite() && surface_area > 0.0) {
        return Err(format!("surface area not positive-finite: {surface_area}"));
    }

    // Tensor magnitude scale (≥ 1 so a near-zero tensor isn't held to an
    // absolute epsilon either).
    let scale = inertia_tensor
        .iter()
        .flat_map(|row| row.iter())
        .fold(0.0f64, |m, &x| m.max(x.abs()))
        .max(1.0);

    // Symmetry: the integration writes the tensor symmetrically, but a NaN/Inf
    // accumulator from a degenerate triangle would surface as asymmetry here.
    for (a, b) in [(0, 1), (0, 2), (1, 2)] {
        let asym = (inertia_tensor[a][b] - inertia_tensor[b][a]).abs();
        if !asym.is_finite() || asym > rel_band * scale {
            return Err(format!(
                "inertia tensor not symmetric: I[{a}][{b}]={} vs I[{b}][{a}]={} (asym {asym:.3e})",
                inertia_tensor[a][b], inertia_tensor[b][a]
            ));
        }
    }

    // Positive-semidefiniteness: every principal moment finite and ≥ 0 (within a
    // scaled epsilon for a near-degenerate Jacobi solve).
    let pm = [
        principal_moments.x,
        principal_moments.y,
        principal_moments.z,
    ];
    let neg_tol = -rel_band * scale;
    for (k, &m) in pm.iter().enumerate() {
        if !m.is_finite() || m < neg_tol {
            return Err(format!(
                "principal moment {k} non-physical (negative/NaN): {m}"
            ));
        }
    }

    // Rigid-body triangle inequality: Ii + Ij ≥ Ik for every pairing.
    let tri_tol = rel_band * scale;
    for (a, b, c) in [(0, 1, 2), (0, 2, 1), (1, 2, 0)] {
        if pm[a] + pm[b] + tri_tol < pm[c] {
            return Err(format!(
                "principal-moment triangle inequality violated: I{a}+I{b}={} < I{c}={}",
                pm[a] + pm[b],
                pm[c]
            ));
        }
    }

    Ok(())
}

/// Flatten a `Matrix4` to a 4×4 row-major nested array for JSON
/// recording. Used by the slice 4a datum mediators so timeline replay
/// and the API layer share the same wire shape.
#[inline]
fn matrix4_to_row_major(m: &Matrix4) -> [[f64; 4]; 4] {
    [
        [m.get(0, 0), m.get(0, 1), m.get(0, 2), m.get(0, 3)],
        [m.get(1, 0), m.get(1, 1), m.get(1, 2), m.get(1, 3)],
        [m.get(2, 0), m.get(2, 1), m.get(2, 2), m.get(2, 3)],
        [m.get(3, 0), m.get(3, 1), m.get(3, 2), m.get(3, 3)],
    ]
}

/// Average position of every unique vertex referenced by `loop_id`'s
/// edges, in the model's vertex store. Used by slice 4b derived datum
/// evaluation as a curve-store-free substitute for `Loop::centroid`
/// (which currently passes an empty `CurveStore` to its underlying
/// `compute_stats` and therefore fails on any non-trivial loop).
///
/// Returns `DatumError::UnknownReference` if the loop or any of its
/// referenced edges / vertices cannot be resolved, and
/// `DatumError::DegenerateSource` if the loop has no edges.
fn loop_vertex_centroid(
    model: &BRepModel,
    loop_id: crate::primitives::r#loop::LoopId,
) -> Result<Point3, crate::primitives::datum::DatumError> {
    use crate::primitives::datum::DatumError;
    let lp = model
        .loops
        .get(loop_id)
        .ok_or(DatumError::UnknownReference {
            kind: "loop",
            id: loop_id as u64,
        })?;
    if lp.edges.is_empty() {
        return Err(DatumError::DegenerateSource("loop has no edges"));
    }
    // Collect each edge's start vertex; for a closed loop the start
    // vertices already form the unique vertex set without
    // double-counting.
    let mut sx = 0.0_f64;
    let mut sy = 0.0_f64;
    let mut sz = 0.0_f64;
    let mut count: usize = 0;
    for &edge_id in &lp.edges {
        let edge = model
            .edges
            .get(edge_id)
            .ok_or(DatumError::UnknownReference {
                kind: "edge",
                id: edge_id as u64,
            })?;
        let v = model
            .vertices
            .get(edge.start_vertex)
            .ok_or(DatumError::UnknownReference {
                kind: "vertex",
                id: edge.start_vertex as u64,
            })?;
        sx += v.position[0];
        sy += v.position[1];
        sz += v.position[2];
        count += 1;
    }
    if count == 0 {
        return Err(DatumError::DegenerateSource(
            "loop has no resolvable vertices",
        ));
    }
    let inv = 1.0 / count as f64;
    Ok(Point3::new(sx * inv, sy * inv, sz * inv))
}

/// Tessellated mesh representation for visualization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TessellatedMesh {
    /// Vertex positions as [x, y, z] triples
    pub vertices: Vec<[f32; 3]>,
    /// Normal vectors as [nx, ny, nz] triples
    pub normals: Vec<[f32; 3]>,
    /// Triangle indices (triplets of vertex indices)
    pub indices: Vec<u32>,
}

/// Global timeline operations cache for high-performance parametric updates
static TIMELINE_CACHE: LazyLock<DashMap<u64, Vec<TimelineOperation>>> = LazyLock::new(DashMap::new);

/// Global geometry parameter cache for fast parameter updates
static GEOMETRY_PARAMETERS: LazyLock<DashMap<GeometryId, DashMap<String, f64>>> =
    LazyLock::new(DashMap::new);

/// Cache performance statistics for monitoring
#[derive(Debug, Clone)]
pub struct CacheStatistics {
    pub timeline_entries: usize,
    pub geometry_parameter_entries: usize,
    pub memory_usage_bytes: usize,
}

/// Result type for builder operations
pub type BuilderResult<T> = Result<T, PrimitiveError>;

/// Alias for builder errors
pub type BuilderError = PrimitiveError;

/// Options for primitive creation
#[derive(Debug, Clone, Default)]
pub struct PrimitiveOptions {
    pub tolerance: Option<Tolerance>,
    pub transform: Option<Matrix4>,
}

/// Estimated model complexity for analytical capacity planning
#[derive(Debug, Clone, Copy)]
pub enum EstimatedComplexity {
    /// Simple models: single primitives, basic sketches
    Simple,
    /// Medium models: assemblies with 10-100 parts
    Medium,
    /// Complex models: assemblies with 100-1000 parts
    Complex,
    /// Highly complex: >1000 parts, aerospace/automotive assemblies
    HighlyComplex,
    /// Custom complexity with specific parameters
    Custom {
        expected_parts: usize,
        expected_features_per_part: usize,
    },
}

impl EstimatedComplexity {
    /// Estimate topology storage requirements based on CAD modeling patterns
    /// Uses Euler's formula and empirical ratios from industrial CAD models
    pub fn estimate_topology_requirements(&self) -> (usize, usize, usize, usize, usize) {
        let (parts, features_per_part) = match self {
            Self::Simple => (1, 5),
            Self::Medium => (50, 20),
            Self::Complex => (500, 40),
            Self::HighlyComplex => (2000, 80),
            Self::Custom {
                expected_parts,
                expected_features_per_part,
            } => (*expected_parts, *expected_features_per_part),
        };

        // Analytical estimation based on topology relationships:
        // - Each part has ~features_per_part features (fillets, holes, etc.)
        // - Each feature creates ~8 faces on average (empirical heuristic for CAD features)
        // - Euler formula: V - E + F = 2(1-g) where g is genus
        // - For manifold solids: E ≈ 1.5F, V ≈ 0.5F (empirical ratios)

        let total_features = parts * features_per_part;
        let faces_per_feature = 8; // Average for CAD features (holes, fillets, etc.)
        let estimated_faces = total_features * faces_per_feature;

        // Topology relationships from Euler formula and manifold properties
        let estimated_vertices = (estimated_faces as f64 * 0.5).ceil() as usize;
        let estimated_edges = (estimated_faces as f64 * 1.5).ceil() as usize;
        let estimated_shells = parts; // One shell per part typically
        let estimated_solids = parts;

        (
            estimated_vertices,
            estimated_edges,
            estimated_faces,
            estimated_shells,
            estimated_solids,
        )
    }
}

/// Sketch plane for 2D operations
#[derive(Debug, Clone)]
pub struct SketchPlane {
    pub id: String,
    pub position: Point3,
    pub normal: Vector3,
    pub u_axis: Vector3,
    pub v_axis: Vector3,
    pub size: f64,
}

impl SketchPlane {
    pub fn new(id: String, position: Point3, normal: Vector3, size: f64) -> Self {
        let u_axis = if normal.dot(&Vector3::new(1.0, 0.0, 0.0)).abs() < 0.9 {
            normal
                .cross(&Vector3::new(1.0, 0.0, 0.0))
                .normalize()
                .unwrap_or(Vector3::new(1.0, 0.0, 0.0))
        } else {
            normal
                .cross(&Vector3::new(0.0, 1.0, 0.0))
                .normalize()
                .unwrap_or(Vector3::new(0.0, 1.0, 0.0))
        };
        let v_axis = normal
            .cross(&u_axis)
            .normalize()
            .unwrap_or(Vector3::new(0.0, 0.0, 1.0));

        Self {
            id,
            position,
            normal,
            u_axis,
            v_axis,
            size,
        }
    }
}

/// B-Rep model container with all topology stores
#[derive(Debug)]
pub struct BRepModel {
    /// Vertex storage
    pub vertices: VertexStore,
    /// Curve storage
    pub curves: CurveStore,
    /// Parameter-space curve storage.
    ///
    /// Parallel to `curves` but stores 2D (u, v) curves anchored to a
    /// specific face. Populated by operations that compute the
    /// parameter-space image natively (imprint, surface–surface
    /// intersection, fillet rail projection); consumers
    /// (tessellation, trim) walk `Edge::pcurves` and look up here.
    /// Empty for the lifetime of a model whose operations have not
    /// yet been wired to F7-ε — that is a supported state and code
    /// paths fall back to 3D inverse projection.
    pub pcurves: crate::primitives::p_curve::PCurveStore,
    /// Edge storage
    pub edges: EdgeStore,
    /// Loop storage
    pub loops: LoopStore,
    /// Surface storage
    pub surfaces: SurfaceStore,
    /// Face storage
    pub faces: FaceStore,
    /// Shell storage
    pub shells: ShellStore,
    /// Solid storage
    pub solids: SolidStore,
    /// Sketch plane storage
    pub sketch_planes: DashMap<String, SketchPlane>,
    /// Datum storage (Origin, reference planes, reference axes, plus
    /// future user-authored datums). Seeded with the canonical seven on
    /// model construction; see `crate::primitives::datum`.
    pub datums: crate::primitives::datum::DatumStore,
    /// Slice 5 forward dependency graph: derived-datum sources and
    /// solid anchors. Mutated as a side effect of `create_derived_datum`,
    /// `delete_datum`, and `anchor_solid`; read by
    /// `propagate_datum_change` and the geometry-mutation notifiers.
    pub datum_graph: crate::primitives::datum::DatumGraph,
    /// Slice 5 per-solid `LocationDescriptor` cache. Populated lazily
    /// by `solid_location_descriptor_cached`; invalidated on transform,
    /// anchor reassignment, and datum moves that walk the graph back
    /// to an anchored solid.
    pub location_cache: crate::primitives::datum::LocationDescriptorCache,
    /// Per-face apex hint for spherical-cap tessellation, keyed by `FaceId`.
    /// A cap bounded by a single cut circle has two valid hemispheres;
    /// `(c_center - sphere_centre)·axis` cannot tell them apart at a great circle
    /// (sphere centre on the cut plane). The boolean records the kept cap's apex
    /// (its `SplitFace.interior_point`) here so `tessellate_spherical_cap` picks
    /// the correct side. Populated per boolean result face; absent caps fall back
    /// to the geometric test.
    pub cap_apex_hint: DashMap<FaceId, Point3>,
    /// Optional recorder receiving one event per successful operation.
    /// `None` by default — tests and unattached models incur zero overhead.
    /// Attached via `attach_recorder` by the orchestration layer
    /// (api-server, AI batch driver, …). Not serialized; recorder identity
    /// is an orchestration concern, not a model invariant.
    pub recorder: Option<std::sync::Arc<dyn crate::operations::recorder::OperationRecorder>>,
    /// Working tolerance for this model.
    ///
    /// Used as the default when seeding entity stores, and as the floor
    /// for any operation that doesn't override via `CommonOptions`. Per-
    /// entity tolerances (`Vertex.tolerance`, `Edge.tolerance`) are
    /// always honoured via the `max(stored, caller)` rule — this is the
    /// caller tolerance when no explicit op tolerance is supplied.
    pub tolerance: Tolerance,
    /// Persistent-id sidecar maps (#11, slice 40-A). Transient store ids →
    /// durable [`PersistentId`]s that survive regeneration + parameter edits,
    /// plus their inverses for PID-anchored lookup. Sidecar (not embedded in the
    /// SoA stores) to preserve the columnar cache layout — a PID probe is an O(1)
    /// hashmap lookup, off the math hot path. Empty until operations are wired to
    /// assign PIDs (slices 40-B onward); an empty map is a supported state.
    pub vertex_pids:
        std::collections::HashMap<VertexId, crate::primitives::persistent_id::PersistentId>,
    pub edge_pids:
        std::collections::HashMap<EdgeId, crate::primitives::persistent_id::PersistentId>,
    pub face_pids:
        std::collections::HashMap<FaceId, crate::primitives::persistent_id::PersistentId>,
    pub solid_pids:
        std::collections::HashMap<SolidId, crate::primitives::persistent_id::PersistentId>,
    pub pid_to_vertex:
        std::collections::HashMap<crate::primitives::persistent_id::PersistentId, VertexId>,
    pub pid_to_edge:
        std::collections::HashMap<crate::primitives::persistent_id::PersistentId, EdgeId>,
    pub pid_to_face:
        std::collections::HashMap<crate::primitives::persistent_id::PersistentId, FaceId>,
    pub pid_to_solid:
        std::collections::HashMap<crate::primitives::persistent_id::PersistentId, SolidId>,
    /// Stable key of the operation currently being applied — set by the
    /// orchestration / replay layer before each op so root persistent-ids derive
    /// from the TIMELINE EVENT ID (identical across replays, robust to event
    /// reordering). `None` for standalone/test creation, where root pids fall
    /// back to `root_counter`. Operation context — like `recorder`, NOT snapshotted.
    pub current_event_key: Option<String>,
    /// Monotonic fallback for root-pid seeds when `current_event_key` is unset.
    /// Snapshotted so a rolled-back primitive creation re-mints the same seed.
    pub root_counter: u64,
    /// Ground-truth provenance per solid (PILLAR 1): the kernel's own faithful
    /// record of which operation created each solid, classified primitive vs
    /// designed. Sidecar (like the PID maps) to keep the SoA stores lean. Set by
    /// `assign_primitive_pids` (primitives) and by designed operations on
    /// success; absence is reported honestly as "unrecorded", never assumed.
    pub solid_provenance:
        std::collections::HashMap<SolidId, crate::primitives::provenance::SolidProvenance>,
    /// Per-solid construction geometry (FIX 1 / FIX 2): the source sketch
    /// plane + profile a sketch-derived solid was built from. Sidecar (like
    /// `solid_provenance` and the PID maps) so the SoA stores stay lean.
    /// Absent for bare primitives and any op with no recorded source sketch;
    /// absence is reported honestly as `ConstructionConsistency::NotApplicable`
    /// and does NOT affect soundness. Set by the sketch-extrude path via
    /// `set_solid_construction`; transformed in lock-step with the solid by
    /// `operations::transform`; read by `certify_solid`.
    pub solid_construction:
        std::collections::HashMap<SolidId, crate::primitives::provenance::ConstructionGeometry>,
    /// Document length unit (GD&T Phase 1): the unit lengths/diameters are
    /// *labelled and formatted* in on drawings and agent-facing readouts. The
    /// kernel's native modelling unit is the millimetre (1 kernel unit = 1 mm)
    /// and that does not change with this setting — only the displayed unit
    /// string and number formatting do. Defaults to millimetres. A
    /// document/scene-level setting, not a per-entity property; the clean seam
    /// for true unit conversion lives on [`crate::units::LengthUnit`].
    pub document_unit: crate::units::LengthUnit,
    /// GD&T annotations (PILLAR-aligned): dimensional tolerances and feature
    /// control frames bound to features by [`crate::primitives::persistent_id::PersistentId`].
    /// Sidecar (like the PID maps + provenance) so the SoA stores stay lean and a
    /// tolerance probe is an O(1) lookup off the math hot path. Keyed by
    /// persistent id (which IS snapshotted), so it is carried through
    /// snapshot/restore and emptied by `clear_geometry`. Empty by default — a
    /// supported state for any model with no tolerances authored.
    pub gdt: crate::gdt::sidecar::GdtSidecar,
    /// Human-readable labels (the LABELLER): a NAME pinned to a topological
    /// entity (by [`crate::primitives::persistent_id::PersistentId`]) or a named
    /// cross-section plane, so the agent and the user share one vocabulary for
    /// the geometry. Sidecar (like the PID maps + provenance + GD&T) so the SoA
    /// stores stay lean; keyed durably so a name survives regeneration. Carried
    /// through snapshot/restore and emptied by `clear_geometry`. Empty by
    /// default — a supported state for any unlabelled model.
    pub labels: crate::labels::LabelSidecar,
}

impl BRepModel {
    /// Create new B-Rep model with analytical capacity estimation
    pub fn new() -> Self {
        Self::with_estimated_capacity(EstimatedComplexity::Medium)
    }

    /// Create B-Rep model with capacity estimation based on expected complexity
    pub fn with_estimated_capacity(complexity: EstimatedComplexity) -> Self {
        Self::with_estimated_capacity_and_tolerance(complexity, Tolerance::default())
    }

    /// Create B-Rep model with explicit working tolerance.
    ///
    /// Use this when the application has a non-default tolerance regime
    /// (e.g. boolean ops at 1e-8, manufacturing at 1e-4). The vertex
    /// store is seeded with the supplied tolerance — its grid-hash and
    /// dedup defaults derive from this value.
    pub fn with_estimated_capacity_and_tolerance(
        complexity: EstimatedComplexity,
        tolerance: Tolerance,
    ) -> Self {
        let (vertex_capacity, edge_capacity, face_capacity, shell_capacity, solid_capacity) =
            complexity.estimate_topology_requirements();

        let datums = crate::primitives::datum::DatumStore::new();
        // Every model starts with the canonical seven default datums
        // (Origin + 3 planes + 3 axes). User datums (Slice 3) are added
        // on top of these.
        datums.seed_defaults();

        Self {
            vertices: VertexStore::with_capacity_and_tolerance(
                vertex_capacity,
                tolerance.distance(),
            ),
            curves: CurveStore::new(),
            // Same edge capacity is a workable upper-bound heuristic
            // for pcurve count: in the fully-populated manifold case
            // each edge has at most two pcurves, but most edges in a
            // typical part have zero. Sizing at one-times-edges keeps
            // the initial allocation cheap while still avoiding an
            // immediate reallocation when an imprint-heavy operation
            // begins emitting pcurves.
            pcurves: crate::primitives::p_curve::PCurveStore::with_capacity(edge_capacity),
            edges: EdgeStore::with_capacity(edge_capacity),
            loops: LoopStore::with_capacity(face_capacity), // Loops ≈ faces for typical models
            surfaces: SurfaceStore::new(),
            faces: FaceStore::with_capacity(face_capacity),
            shells: ShellStore::with_capacity(shell_capacity),
            solids: SolidStore::with_capacity(solid_capacity),
            sketch_planes: DashMap::new(),
            datums,
            datum_graph: crate::primitives::datum::DatumGraph::new(),
            location_cache: crate::primitives::datum::LocationDescriptorCache::new(),
            cap_apex_hint: DashMap::new(),
            recorder: None,
            tolerance,
            vertex_pids: std::collections::HashMap::new(),
            edge_pids: std::collections::HashMap::new(),
            face_pids: std::collections::HashMap::new(),
            solid_pids: std::collections::HashMap::new(),
            pid_to_vertex: std::collections::HashMap::new(),
            pid_to_edge: std::collections::HashMap::new(),
            pid_to_face: std::collections::HashMap::new(),
            pid_to_solid: std::collections::HashMap::new(),
            current_event_key: None,
            root_counter: 0,
            solid_provenance: std::collections::HashMap::new(),
            solid_construction: std::collections::HashMap::new(),
            document_unit: crate::units::LengthUnit::default(),
            gdt: crate::gdt::sidecar::GdtSidecar::new(),
            labels: crate::labels::LabelSidecar::new(),
        }
    }

    /// Current working tolerance.
    #[inline]
    pub fn tolerance(&self) -> Tolerance {
        self.tolerance
    }

    /// Override the model's working tolerance.
    ///
    /// This does *not* rebuild the per-entity stores, so the grid-hash
    /// and dedup-enabled flags on the existing `VertexStore` stay at
    /// whatever they were seeded with. Callers that need a different
    /// store configuration should create the model with
    /// `with_estimated_capacity_and_tolerance`.
    #[inline]
    pub fn set_tolerance(&mut self, tolerance: Tolerance) {
        self.tolerance = tolerance;
    }

    /// The document length unit lengths/diameters are labelled in on drawings
    /// and agent readouts (GD&T Phase 1). Defaults to millimetres.
    #[inline]
    pub fn document_unit(&self) -> crate::units::LengthUnit {
        self.document_unit
    }

    /// Set the document length unit. This changes only display/labelling — the
    /// kernel geometry stays in its native millimetre modelling unit.
    #[inline]
    pub fn set_document_unit(&mut self, unit: crate::units::LengthUnit) {
        self.document_unit = unit;
    }

    /// Reset every GEOMETRY store to empty, preserving the seeded datums
    /// (canonical seven), the attached recorder, and the tolerance.
    ///
    /// This is the kernel half of a TRUE "clear the scene". Deleting solids
    /// one-by-one leaves ORPHANED vertices/edges/curves/surfaces behind when an
    /// upstream op added entities that were never folded into a solid — the
    /// classic case is a sketch materialised into edges/curves followed by a
    /// kernel op that fails: the kernel op's own `with_rollback` undoes its
    /// additions, but the pre-materialised sketch entities (added *before* the
    /// rolled-back op) linger. Those orphans then surface as phantom
    /// connectivity errors in later validation. `clear_geometry` sweeps them so
    /// "clear" leaves a genuinely empty model, not invisible residue.
    pub fn clear_geometry(&mut self) {
        let tol = self.tolerance;
        self.vertices = VertexStore::with_capacity_and_tolerance(64, tol.distance());
        self.curves = CurveStore::new();
        self.pcurves = crate::primitives::p_curve::PCurveStore::default();
        self.edges = EdgeStore::default();
        self.loops = LoopStore::default();
        self.surfaces = SurfaceStore::default();
        self.faces = FaceStore::default();
        self.shells = ShellStore::default();
        self.solids = SolidStore::default();
        self.sketch_planes.clear();
        self.cap_apex_hint.clear();
        self.location_cache = crate::primitives::datum::LocationDescriptorCache::new();
        // Persistent-id sidecars are part of the geometry — clear with it.
        self.vertex_pids.clear();
        self.edge_pids.clear();
        self.face_pids.clear();
        self.solid_pids.clear();
        self.pid_to_vertex.clear();
        self.pid_to_edge.clear();
        self.pid_to_face.clear();
        self.pid_to_solid.clear();
        self.root_counter = 0;
        // GD&T annotations are bound to the topology being discarded.
        self.gdt.clear();
        // Construction geometry is bound to the solids being discarded —
        // clear it so a recycled solid id can't inherit a stale orphan
        // sketch and certify falsely Inconsistent.
        self.solid_construction.clear();
        // Labels are bound to the topology (entity labels) / the part (section
        // labels) being discarded.
        self.labels.clear();
        // Preserved: datums (+ seeded defaults), datum_graph, recorder, tolerance,
        // current_event_key (caller op-context, set per-op).
    }

    // --- Persistent-id accessors (#11, slice 40-A) ---

    /// Assign a [`PersistentId`] to a face, maintaining the inverse map.
    pub fn set_face_pid(
        &mut self,
        face: FaceId,
        pid: crate::primitives::persistent_id::PersistentId,
    ) {
        self.face_pids.insert(face, pid);
        self.pid_to_face.insert(pid, face);
    }

    /// Assign a [`PersistentId`] to an edge, maintaining the inverse map.
    pub fn set_edge_pid(
        &mut self,
        edge: EdgeId,
        pid: crate::primitives::persistent_id::PersistentId,
    ) {
        self.edge_pids.insert(edge, pid);
        self.pid_to_edge.insert(pid, edge);
    }

    /// Assign a [`PersistentId`] to a vertex, maintaining the inverse map.
    pub fn set_vertex_pid(
        &mut self,
        vertex: VertexId,
        pid: crate::primitives::persistent_id::PersistentId,
    ) {
        self.vertex_pids.insert(vertex, pid);
        self.pid_to_vertex.insert(pid, vertex);
    }

    /// Assign a [`PersistentId`] to a solid, maintaining the inverse map.
    pub fn set_solid_pid(
        &mut self,
        solid: SolidId,
        pid: crate::primitives::persistent_id::PersistentId,
    ) {
        self.solid_pids.insert(solid, pid);
        self.pid_to_solid.insert(pid, solid);
    }

    /// The persistent id of a face, if assigned.
    pub fn face_pid(&self, face: FaceId) -> Option<crate::primitives::persistent_id::PersistentId> {
        self.face_pids.get(&face).copied()
    }

    /// The live face id for a persistent id, if it still resolves.
    pub fn face_by_pid(
        &self,
        pid: crate::primitives::persistent_id::PersistentId,
    ) -> Option<FaceId> {
        self.pid_to_face.get(&pid).copied()
    }

    /// The persistent id of an edge, if assigned.
    pub fn edge_pid(&self, edge: EdgeId) -> Option<crate::primitives::persistent_id::PersistentId> {
        self.edge_pids.get(&edge).copied()
    }

    /// The live edge id for a persistent id, if it still resolves.
    pub fn edge_by_pid(
        &self,
        pid: crate::primitives::persistent_id::PersistentId,
    ) -> Option<EdgeId> {
        self.pid_to_edge.get(&pid).copied()
    }

    /// The persistent id of a solid, if assigned.
    pub fn solid_pid(
        &self,
        solid: SolidId,
    ) -> Option<crate::primitives::persistent_id::PersistentId> {
        self.solid_pids.get(&solid).copied()
    }

    /// The live solid id for a persistent id, if it still resolves.
    pub fn solid_by_pid(
        &self,
        pid: crate::primitives::persistent_id::PersistentId,
    ) -> Option<SolidId> {
        self.pid_to_solid.get(&pid).copied()
    }

    // --- GD&T annotation + verification (kernel-verified conformance) ---

    /// Resolve a face's durable annotation key, assigning a [`PersistentId`] if
    /// the face does not yet have one. GD&T annotations key off persistent ids
    /// (which survive regeneration); a face created by an op not yet wired for
    /// PIDs has none, so we mint a deterministic one derived from the face's
    /// transient id + the current op-context seed and register it. Idempotent for
    /// an already-PID'd face.
    pub fn ensure_face_annotation_key(
        &mut self,
        face: FaceId,
    ) -> crate::primitives::persistent_id::PersistentId {
        if let Some(p) = self.face_pid(face) {
            return p;
        }
        let seed = self.next_root_seed(&format!("gdt_face:{face}"));
        let pid = crate::primitives::persistent_id::PersistentId::root(&seed);
        self.set_face_pid(face, pid);
        pid
    }

    /// Resolve an edge's durable annotation key, assigning a [`PersistentId`] if
    /// absent (see [`Self::ensure_face_annotation_key`]).
    pub fn ensure_edge_annotation_key(
        &mut self,
        edge: EdgeId,
    ) -> crate::primitives::persistent_id::PersistentId {
        if let Some(p) = self.edge_pid(edge) {
            return p;
        }
        let seed = self.next_root_seed(&format!("gdt_edge:{edge}"));
        let pid = crate::primitives::persistent_id::PersistentId::root(&seed);
        self.set_edge_pid(edge, pid);
        pid
    }

    /// Attach a GD&T annotation to a FACE by its (resolved) persistent id.
    /// Returns the persistent key so the caller can re-query the feature later.
    pub fn attach_face_annotation(
        &mut self,
        face: FaceId,
        annotation: crate::gdt::model::Annotation,
    ) -> crate::primitives::persistent_id::PersistentId {
        let key = self.ensure_face_annotation_key(face);
        self.gdt.attach(key, annotation);
        key
    }

    /// Attach a GD&T annotation to an EDGE by its (resolved) persistent id.
    pub fn attach_edge_annotation(
        &mut self,
        edge: EdgeId,
        annotation: crate::gdt::model::Annotation,
    ) -> crate::primitives::persistent_id::PersistentId {
        let key = self.ensure_edge_annotation_key(edge);
        self.gdt.attach(key, annotation);
        key
    }

    /// Verify EVERY annotation attached to a face, measuring the actual geometry.
    /// Returns one [`crate::gdt::verify::ConformanceResult`] per annotation in
    /// attachment order. Empty when the face has no annotations or no key.
    pub fn verify_face_conformance(
        &self,
        face: FaceId,
    ) -> Vec<crate::gdt::verify::ConformanceResult> {
        let Some(key) = self.face_pid(face) else {
            return Vec::new();
        };
        let tol = self.tolerance;
        self.gdt
            .annotations(key)
            .iter()
            .map(|ann| crate::gdt::verify::verify_face_annotation(self, face, ann, tol))
            .collect()
    }

    /// Verify EVERY annotation attached to an edge (circularity / straightness).
    /// `samples` controls the edge sampling density.
    pub fn verify_edge_conformance(
        &self,
        edge: EdgeId,
        samples: usize,
    ) -> Vec<crate::gdt::verify::ConformanceResult> {
        let Some(key) = self.edge_pid(edge) else {
            return Vec::new();
        };
        let tol = self.tolerance;
        self.gdt
            .annotations(key)
            .iter()
            .filter_map(|ann| match ann {
                crate::gdt::model::Annotation::Geometric(fcf) => Some(
                    crate::gdt::verify::verify_form_on_edge(self, edge, fcf, tol, samples),
                ),
                // A dimensional annotation on an edge is not an edge-form check;
                // skip it here (it belongs on a face).
                crate::gdt::model::Annotation::Dimensional(_) => None,
            })
            .collect()
    }

    // --- Labels (the LABELLER): names pinned to entities / sections ----------

    /// Ensure a VERTEX has a durable persistent id, minting one if absent —
    /// mirror of [`Self::ensure_face_annotation_key`] for the vertex case, used
    /// when a label is pinned to a vertex created by an op not yet PID-wired.
    pub fn ensure_vertex_label_key(
        &mut self,
        vertex: VertexId,
    ) -> crate::primitives::persistent_id::PersistentId {
        if let Some(p) = self.vertex_pids.get(&vertex).copied() {
            return p;
        }
        let seed = self.next_root_seed(&format!("label_vertex:{vertex}"));
        let pid = crate::primitives::persistent_id::PersistentId::root(&seed);
        self.set_vertex_pid(vertex, pid);
        pid
    }

    /// Pin a NAME to a FACE (by its resolved persistent id). The name is the
    /// shared vocabulary; the durable PID is what survives regeneration. Returns
    /// whether the name was created or replaced, or refuses an empty name.
    pub fn label_face(
        &mut self,
        face: FaceId,
        name: &str,
        description: Option<String>,
    ) -> Result<crate::labels::AttachOutcome, crate::labels::LabelError> {
        let pid = self.ensure_face_annotation_key(face);
        self.labels.attach(
            name,
            crate::labels::Label {
                target: crate::labels::LabelTarget::Entity {
                    kind: crate::labels::LabelKind::Face,
                    pid,
                },
                description,
            },
        )
    }

    /// Pin a NAME to an EDGE (by its resolved persistent id).
    pub fn label_edge(
        &mut self,
        edge: EdgeId,
        name: &str,
        description: Option<String>,
    ) -> Result<crate::labels::AttachOutcome, crate::labels::LabelError> {
        let pid = self.ensure_edge_annotation_key(edge);
        self.labels.attach(
            name,
            crate::labels::Label {
                target: crate::labels::LabelTarget::Entity {
                    kind: crate::labels::LabelKind::Edge,
                    pid,
                },
                description,
            },
        )
    }

    /// Pin a NAME to a VERTEX (by its resolved persistent id).
    pub fn label_vertex(
        &mut self,
        vertex: VertexId,
        name: &str,
        description: Option<String>,
    ) -> Result<crate::labels::AttachOutcome, crate::labels::LabelError> {
        let pid = self.ensure_vertex_label_key(vertex);
        self.labels.attach(
            name,
            crate::labels::Label {
                target: crate::labels::LabelTarget::Entity {
                    kind: crate::labels::LabelKind::Vertex,
                    pid,
                },
                description,
            },
        )
    }

    /// Pin a NAME to a cross-section PLANE (`origin` + `normal`). A named section
    /// is not a topological entity — the plane itself is the target.
    pub fn label_section(
        &mut self,
        name: &str,
        origin: Point3,
        normal: Vector3,
        description: Option<String>,
    ) -> Result<crate::labels::AttachOutcome, crate::labels::LabelError> {
        self.labels.attach(
            name,
            crate::labels::Label {
                target: crate::labels::LabelTarget::Section(crate::labels::SectionPlane {
                    origin,
                    normal,
                }),
                description,
            },
        )
    }

    /// The label with this name, if present (the stored target, not yet resolved
    /// to a live id).
    pub fn label(&self, name: &str) -> Option<&crate::labels::Label> {
        self.labels.get(name)
    }

    /// Resolve a label NAME to a live `FaceId`, or refuse. `NotFound` if the name
    /// is unknown or names a non-face target; `Dangling` if the face it named has
    /// been deleted. The kernel never guesses.
    pub fn resolve_label_face(&self, name: &str) -> Result<FaceId, crate::labels::LabelError> {
        use crate::labels::{LabelError, LabelKind, LabelTarget};
        match self.labels.get(name) {
            None => Err(LabelError::NotFound),
            Some(l) => match &l.target {
                LabelTarget::Entity {
                    kind: LabelKind::Face,
                    pid,
                } => self.face_by_pid(*pid).ok_or(LabelError::Dangling),
                _ => Err(LabelError::NotFound),
            },
        }
    }

    /// Resolve a label NAME to a live `EdgeId`, or refuse (see
    /// [`Self::resolve_label_face`]).
    pub fn resolve_label_edge(&self, name: &str) -> Result<EdgeId, crate::labels::LabelError> {
        use crate::labels::{LabelError, LabelKind, LabelTarget};
        match self.labels.get(name) {
            None => Err(LabelError::NotFound),
            Some(l) => match &l.target {
                LabelTarget::Entity {
                    kind: LabelKind::Edge,
                    pid,
                } => self.edge_by_pid(*pid).ok_or(LabelError::Dangling),
                _ => Err(LabelError::NotFound),
            },
        }
    }

    /// Resolve a label NAME to a live `VertexId`, or refuse.
    pub fn resolve_label_vertex(&self, name: &str) -> Result<VertexId, crate::labels::LabelError> {
        use crate::labels::{LabelError, LabelKind, LabelTarget};
        match self.labels.get(name) {
            None => Err(LabelError::NotFound),
            Some(l) => match &l.target {
                LabelTarget::Entity {
                    kind: LabelKind::Vertex,
                    pid,
                } => self
                    .pid_to_vertex
                    .get(pid)
                    .copied()
                    .ok_or(LabelError::Dangling),
                _ => Err(LabelError::NotFound),
            },
        }
    }

    /// Resolve a label NAME to a section PLANE, or refuse.
    pub fn resolve_label_section(
        &self,
        name: &str,
    ) -> Result<crate::labels::SectionPlane, crate::labels::LabelError> {
        use crate::labels::{LabelError, LabelTarget};
        match self.labels.get(name) {
            None => Err(LabelError::NotFound),
            Some(l) => match &l.target {
                LabelTarget::Section(p) => Ok(*p),
                _ => Err(LabelError::NotFound),
            },
        }
    }

    /// The WORLD anchor point for a label's eye-overlay callout: a face's
    /// centroid, an edge's midpoint, a vertex's position, or a section plane's
    /// origin. `None` when the named entity no longer resolves (dangling) or the
    /// stats cannot be computed. `&mut` because face-centroid computation warms a
    /// per-entity cache (same contract as the readable/select query path).
    pub fn label_anchor(&mut self, name: &str) -> Option<Point3> {
        use crate::labels::{LabelKind, LabelTarget};
        let target = self.labels.get(name)?.target.clone();
        match target {
            LabelTarget::Section(p) => Some(p.origin),
            LabelTarget::Entity {
                kind: LabelKind::Vertex,
                pid,
            } => {
                let vid = self.pid_to_vertex.get(&pid).copied()?;
                self.vertices.get(vid).map(|v| v.point())
            }
            LabelTarget::Entity {
                kind: LabelKind::Edge,
                pid,
            } => {
                let eid = self.edge_by_pid(pid)?;
                let edge = self.edges.get(eid)?;
                let a = self.vertices.get(edge.start_vertex)?.point();
                let b = self.vertices.get(edge.end_vertex)?.point();
                Some(Point3::new(
                    0.5 * (a.x + b.x),
                    0.5 * (a.y + b.y),
                    0.5 * (a.z + b.z),
                ))
            }
            LabelTarget::Entity {
                kind: LabelKind::Face,
                pid,
            } => {
                let fid = self.face_by_pid(pid)?;
                // Prefer the exact analytic centroid; fall back to the mean of
                // the face's boundary vertices when the centroid integral
                // declines (e.g. an annular cap with inner + outer loops) — the
                // callout still lands ON the face, which is all the overlay needs.
                {
                    let Self {
                        faces,
                        loops,
                        vertices,
                        edges,
                        curves,
                        surfaces,
                        ..
                    } = self;
                    if let Some(face) = faces.get_mut(fid) {
                        if let Ok(stats) =
                            face.compute_stats(loops, vertices, edges, curves, surfaces)
                        {
                            return Some(stats.centroid);
                        }
                    }
                }
                self.face_boundary_mean(fid)
            }
        }
    }

    /// Mean of a face's boundary vertices (outer + inner loops) — a robust
    /// representative point on the face when the analytic centroid is
    /// unavailable. `None` if the face has no resolvable boundary vertices.
    fn face_boundary_mean(&self, fid: FaceId) -> Option<Point3> {
        let face = self.faces.get(fid)?;
        let mut lids = vec![face.outer_loop];
        lids.extend_from_slice(&face.inner_loops);
        let (mut sx, mut sy, mut sz, mut n) = (0.0_f64, 0.0_f64, 0.0_f64, 0_u64);
        for lid in lids {
            if let Some(lp) = self.loops.get(lid) {
                for &eid in &lp.edges {
                    if let Some(edge) = self.edges.get(eid) {
                        for vid in [edge.start_vertex, edge.end_vertex] {
                            if let Some(v) = self.vertices.get(vid) {
                                let p = v.point();
                                sx += p.x;
                                sy += p.y;
                                sz += p.z;
                                n += 1;
                            }
                        }
                    }
                }
            }
        }
        if n == 0 {
            None
        } else {
            let inv = 1.0 / n as f64;
            Some(Point3::new(sx * inv, sy * inv, sz * inv))
        }
    }

    /// All labels as `(name, target-kind-tag, optional-world-anchor,
    /// description)`, in name order — the agent-facing listing. The anchor is the
    /// callout point (centroid/midpoint/position/section-origin); `None` for a
    /// dangling entity label. `&mut` for the centroid cache, as in
    /// [`Self::label_anchor`].
    pub fn list_labels(&mut self) -> Vec<(String, &'static str, Option<Point3>, Option<String>)> {
        let entries: Vec<(String, &'static str, Option<String>)> = self
            .labels
            .iter()
            .map(|(name, label)| {
                (
                    name.to_string(),
                    label.target.kind_tag(),
                    label.description.clone(),
                )
            })
            .collect();
        entries
            .into_iter()
            .map(|(name, kind, desc)| {
                let anchor = self.label_anchor(&name);
                (name, kind, anchor, desc)
            })
            .collect()
    }

    /// Set the stable key of the operation currently being applied (the timeline
    /// event id), so root persistent-ids derive deterministically across replays.
    /// The orchestration / replay layer sets this before each op and clears it
    /// (`None`) after. No-op for standalone/test creation.
    pub fn set_event_key(&mut self, key: Option<String>) {
        self.current_event_key = key;
    }

    /// Seed bytes for a root persistent-id of a primitive described by
    /// `kind_params`. Uses `current_event_key` when set (timeline-stable across
    /// replays), else a monotonic fallback (distinct within this model). `&mut`
    /// because the fallback path advances `root_counter`.
    pub fn next_root_seed(&mut self, kind_params: &str) -> Vec<u8> {
        if let Some(k) = self.current_event_key.clone() {
            format!("{k}|{kind_params}").into_bytes()
        } else {
            let n = self.root_counter;
            self.root_counter += 1;
            format!("__local:{n}|{kind_params}").into_bytes()
        }
    }

    /// Attach a recorder that will receive one event per successful
    /// operation on this model. Returns the previous recorder, if any.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use std::sync::Arc;
    /// let model = BRepModel::new();
    /// let rec: Arc<dyn OperationRecorder> = Arc::new(my_recorder);
    /// model.attach_recorder(Some(rec));
    /// ```
    pub fn attach_recorder(
        &mut self,
        recorder: Option<std::sync::Arc<dyn crate::operations::recorder::OperationRecorder>>,
    ) -> Option<std::sync::Arc<dyn crate::operations::recorder::OperationRecorder>> {
        std::mem::replace(&mut self.recorder, recorder)
    }

    /// Emit a record of a just-completed operation. Silently no-ops when no
    /// recorder is attached; logs a warning via `tracing` when the recorder
    /// returns an error (the operation has already mutated the model —
    /// recorder failures never become geometry failures).
    pub fn record_operation(&self, operation: crate::operations::recorder::RecordedOperation) {
        if let Some(rec) = self.recorder.as_ref() {
            if let Err(e) = rec.record(operation) {
                tracing::warn!("operation recorder returned error: {}", e);
            }
        }
    }

    // ───────────────────── PILLAR 1: ground truth ─────────────────────

    /// Record how a solid was created (primitive vs designed + input solids).
    /// Called by `assign_primitive_pids` for primitives and by designed
    /// operations on success.
    pub fn set_solid_provenance(
        &mut self,
        solid_id: SolidId,
        created_by: crate::primitives::provenance::OperationKind,
        inputs: Vec<SolidId>,
    ) {
        self.solid_provenance.insert(
            solid_id,
            crate::primitives::provenance::SolidProvenance::new(created_by, inputs),
        );
    }

    /// The kernel's recorded provenance for a solid, if any.
    pub fn solid_provenance(
        &self,
        solid_id: SolidId,
    ) -> Option<&crate::primitives::provenance::SolidProvenance> {
        self.solid_provenance.get(&solid_id)
    }

    /// Record the construction geometry a solid was built FROM — the source
    /// sketch's plane origin + lifted profile points (FIX 1 / FIX 2). Called
    /// by the sketch-extrude orchestration after a sketch-derived solid is
    /// built so the kernel owns the solid↔sketch link and can carry it
    /// through transforms + certify co-location. Overwrites any prior entry.
    pub fn set_solid_construction(
        &mut self,
        solid_id: SolidId,
        construction: crate::primitives::provenance::ConstructionGeometry,
    ) {
        self.solid_construction.insert(solid_id, construction);
    }

    /// The construction geometry linked to a solid, if any.
    pub fn solid_construction(
        &self,
        solid_id: SolidId,
    ) -> Option<&crate::primitives::provenance::ConstructionGeometry> {
        self.solid_construction.get(&solid_id)
    }

    /// Compute the cross-entity consistency verdict for a solid (FIX 2):
    /// is its linked construction geometry co-located with the solid?
    ///
    /// Tri-state and honest:
    /// * `NotApplicable` — no construction geometry linked (bare primitive,
    ///   revolve / loft / NURBS with no recorded profile). Does NOT block
    ///   soundness.
    /// * `Consistent` — every construction point lies within the solid's
    ///   world bbox expanded by `tolerance_band`. The band absorbs the fact
    ///   that a sketch profile sits ON a face of the solid (so a point can
    ///   be flush with a bbox boundary) and accommodates the extrude
    ///   direction adding depth the sketch plane does not span.
    /// * `Inconsistent` — at least one construction point has drifted
    ///   outside the band (an orphaned sketch left behind by a transform).
    ///
    /// `tolerance_band` is a fraction of the solid's bbox diagonal, floored
    /// at the model tolerance, so the check scales with part size rather
    /// than using a hardcoded absolute distance.
    pub fn construction_consistency(
        &self,
        solid_id: SolidId,
    ) -> crate::primitives::provenance::ConstructionConsistency {
        use crate::primitives::provenance::ConstructionConsistency;
        let Some(construction) = self.solid_construction.get(&solid_id) else {
            return ConstructionConsistency::NotApplicable;
        };
        // A linked-but-empty construction record carries no spatial claim to
        // verify; treat it as nothing to check rather than a false failure.
        let Some(construction_bbox) = construction.world_bbox() else {
            return ConstructionConsistency::NotApplicable;
        };
        let Some(solid_bbox) = self.solid_world_bbox(solid_id) else {
            // The solid has no reachable geometry but a sketch IS linked —
            // they cannot be co-located. Honest failure, not a silent pass.
            return ConstructionConsistency::Inconsistent;
        };

        // Tolerance band: 1% of the solid diagonal, floored at the model
        // tolerance. Large enough to absorb on-face flushness + numeric
        // noise, far smaller than the ~17u orphan the gate must catch.
        let band = (solid_bbox.diagonal_length() * 0.01).max(self.tolerance.distance());
        let expanded = solid_bbox.expand(band);

        let all_inside = construction
            .profile_points
            .iter()
            .chain(std::iter::once(&construction.plane_origin))
            .all(|p| expanded.contains_point(p));

        if all_inside {
            ConstructionConsistency::Consistent
        } else {
            ConstructionConsistency::Inconsistent
        }
    }

    /// COMPUTE (never accept) a solid's validity certificate: B-Rep validity
    /// (`validate_solid_scoped`) + watertight/manifold/Euler from the tessellated
    /// mesh (`manifold_report`). This is the kernel's own verdict — the agent
    /// cannot fake it.
    pub fn certify_solid(
        &self,
        solid_id: SolidId,
    ) -> crate::primitives::provenance::ValidityCertificate {
        use crate::primitives::provenance::ValidityCertificate;
        use crate::primitives::validation::{validate_solid_scoped, ValidationLevel};
        let v = validate_solid_scoped(self, solid_id, self.tolerance, ValidationLevel::Standard);
        let (watertight, manifold, euler, be, nm) =
            match crate::harness::watertight::manifold_report(self, solid_id, 0.1, 1e-6) {
                Some(r) => (
                    r.boundary_edges == 0,
                    r.nonmanifold_edges == 0,
                    r.euler_characteristic,
                    r.boundary_edges,
                    r.nonmanifold_edges,
                ),
                None => (false, false, 0, 0, 0),
            };
        // Self-intersection at a COARSE chord (gross self-overlap is visible at
        // low density; keeps the O(n²) pair scan cheap for this on-demand check).
        let self_intersection_free =
            !crate::harness::self_intersection::mesh_self_intersects(self, solid_id, 0.5);
        // FIX 2 — cross-entity consistency: is the solid's linked construction
        // geometry (source sketch) still co-located with the solid? Tri-state,
        // and NotApplicable when no sketch is linked (does not block soundness).
        let construction_consistent = self.construction_consistency(solid_id);
        ValidityCertificate {
            brep_valid: v.is_valid,
            watertight,
            manifold,
            euler_characteristic: euler,
            boundary_edges: be,
            nonmanifold_edges: nm,
            self_intersection_free,
            construction_consistent,
            errors: v.errors.iter().map(|e| format!("{e:?}")).collect(),
        }
    }

    /// The kernel's ground-truth answer for a solid: provenance + computed
    /// certificate. `None` if the solid does not exist. PILLAR 1.
    pub fn ground_truth(
        &self,
        solid_id: SolidId,
    ) -> Option<crate::primitives::provenance::GroundTruth> {
        self.solids.get(solid_id)?;
        Some(crate::primitives::provenance::GroundTruth {
            solid_id,
            provenance: self.solid_provenance.get(&solid_id).cloned(),
            certificate: self.certify_solid(solid_id),
        })
    }

    /// Open a transactional recording scope on the attached recorder, if
    /// any. Used by `operations::lifecycle::with_rollback` to defer event
    /// commitment until the wrapped operation succeeds. No-op when no
    /// recorder is attached.
    pub fn begin_pending_record(&self) {
        if let Some(rec) = self.recorder.as_ref() {
            rec.begin_pending();
        }
    }

    /// Commit every event staged since the matching `begin_pending_record`.
    /// No-op when no recorder is attached.
    pub fn commit_pending_record(&self) {
        if let Some(rec) = self.recorder.as_ref() {
            rec.commit_pending();
        }
    }

    /// Discard every event staged since the matching `begin_pending_record`.
    /// Called by `with_rollback` on the failure path so the timeline never
    /// holds events for an operation whose model mutations were rolled
    /// back. No-op when no recorder is attached.
    pub fn abort_pending_record(&self) {
        if let Some(rec) = self.recorder.as_ref() {
            rec.abort_pending();
        }
    }

    /// Invalidate the cached `SolidMassProperties` of every solid
    /// whose outer shell currently contains `face_id`.
    ///
    /// Topology-mutating ops that swap a face for sub-faces (imprint,
    /// F7 fillet trim/sew) must call this BEFORE the swap to catch
    /// the solid by its current face IDs — after the swap the old
    /// face_id is gone and the linkage is harder to find. Pattern
    /// matches the end-of-op invalidation already in
    /// `fillet_edges` / `chamfer_edges` (Task #45 fix); this method
    /// extends the same guarantee to face-level mutations.
    ///
    /// Linear scan over solids. Cheap in practice: per-model solid
    /// counts are O(1) for typical CAD scenes; we only iterate
    /// `solids.iter()` (already a filtered live-id iterator), and
    /// each solid hits exactly one shell-id lookup + one face-list
    /// `contains` call.
    pub fn invalidate_mass_props_for_face(&mut self, face_id: crate::primitives::face::FaceId) {
        // Two-pass to avoid an &/&mut borrow conflict on `self`.
        let mut owners: Vec<crate::primitives::solid::SolidId> = Vec::new();
        for (solid_id, solid) in self.solids.iter() {
            if let Some(shell) = self.shells.get(solid.outer_shell) {
                if shell.find_face(face_id).is_some() {
                    owners.push(solid_id);
                }
            }
        }
        for solid_id in owners {
            if let Some(solid) = self.solids.get_mut(solid_id) {
                solid.invalidate_mass_props_cache();
            }
        }
    }

    // ───────────────────────────── Datum mediators ────────────────────────────
    //
    // Datum mutations route through `BRepModel` rather than directly into
    // `DatumStore` so each user-driven change emits a `RecordedOperation`
    // for replay / branch / audit. The seven default datums seeded at
    // `BRepModel::new()` are an invariant of the baseline model and are
    // therefore *not* recorded — replay starts from "model with seven
    // defaults already present". Only user-driven mutations (visibility
    // toggle today, create/rename/delete in slice 4) emit events.

    /// Toggle a datum's visibility, recording the change.
    ///
    /// Returns `Some(previous_visible)` on success, or `None` when the
    /// datum id is unknown — matches `DatumStore::set_visible`. The event
    /// records `kind = "datum_set_visibility"` with the datum id in
    /// `inputs` and `{datum_id, visible, previous_visible}` in
    /// `parameters`.
    pub fn set_datum_visibility(
        &self,
        id: crate::primitives::datum::DatumId,
        visible: bool,
    ) -> Option<bool> {
        let previous = self.datums.set_visible(id, visible)?;
        self.record_operation(
            crate::operations::recorder::RecordedOperation::new("datum_set_visibility")
                .with_parameters(serde_json::json!({
                    "datum_id": id,
                    "visible": visible,
                    "previous_visible": previous,
                }))
                .with_input_datums([id as u64]),
        );
        Some(previous)
    }

    // Slice 4a: user-authored datum mediators. Each wraps the
    // corresponding `DatumStore` method with a `RecordedOperation` so
    // the timeline can replay the exact authoring history. The kernel
    // takes the operation kind names from the slice plan in
    // `memory/datum-system.md` (`datum_create`, `datum_rename`,
    // `datum_set_transform`, `datum_delete`).

    /// Create a user-authored plane datum and record `datum_create`.
    pub fn create_datum_plane(
        &self,
        name: String,
        transform: Matrix4,
    ) -> Result<crate::primitives::datum::DatumId, crate::primitives::datum::DatumError> {
        let id = self.datums.create_plane(name.clone(), transform)?;
        let mat = matrix4_to_row_major(&transform);
        self.record_operation(
            crate::operations::recorder::RecordedOperation::new("datum_create")
                .with_parameters(serde_json::json!({
                    "datum_id": id,
                    "kind": "plane",
                    "name": name,
                    "transform": mat,
                }))
                .with_output_datums([id as u64]),
        );
        Ok(id)
    }

    /// Create a user-authored axis datum and record `datum_create`.
    pub fn create_datum_axis(
        &self,
        name: String,
        origin: Point3,
        direction: crate::primitives::datum::AxisDirection,
    ) -> Result<crate::primitives::datum::DatumId, crate::primitives::datum::DatumError> {
        let id = self.datums.create_axis(name.clone(), origin, direction)?;
        let dir_label = match direction {
            crate::primitives::datum::AxisDirection::X => "x",
            crate::primitives::datum::AxisDirection::Y => "y",
            crate::primitives::datum::AxisDirection::Z => "z",
            crate::primitives::datum::AxisDirection::Custom => "custom",
        };
        self.record_operation(
            crate::operations::recorder::RecordedOperation::new("datum_create")
                .with_parameters(serde_json::json!({
                    "datum_id": id,
                    "kind": "axis",
                    "name": name,
                    "origin": [origin.x, origin.y, origin.z],
                    "direction": dir_label,
                }))
                .with_output_datums([id as u64]),
        );
        Ok(id)
    }

    /// Create a user-authored point datum and record `datum_create`.
    pub fn create_datum_point(
        &self,
        name: String,
        position: Point3,
    ) -> Result<crate::primitives::datum::DatumId, crate::primitives::datum::DatumError> {
        let id = self.datums.create_point(name.clone(), position)?;
        self.record_operation(
            crate::operations::recorder::RecordedOperation::new("datum_create")
                .with_parameters(serde_json::json!({
                    "datum_id": id,
                    "kind": "point",
                    "name": name,
                    "position": [position.x, position.y, position.z],
                }))
                .with_output_datums([id as u64]),
        );
        Ok(id)
    }

    /// Rename a user-authored datum and record `datum_rename`.
    pub fn rename_datum(
        &self,
        id: crate::primitives::datum::DatumId,
        name: String,
    ) -> Result<String, crate::primitives::datum::DatumError> {
        let previous = self.datums.rename(id, name.clone())?;
        self.record_operation(
            crate::operations::recorder::RecordedOperation::new("datum_rename")
                .with_parameters(serde_json::json!({
                    "datum_id": id,
                    "name": name,
                    "previous_name": previous,
                }))
                .with_input_datums([id as u64]),
        );
        Ok(previous)
    }

    /// Replace a user-authored datum's transform and record
    /// `datum_set_transform`.
    ///
    /// Slice 5: propagates the change to every derived datum whose
    /// source recipe references this datum (transitively, with cycle
    /// detection) by re-evaluating each dependent's source against
    /// the current model and writing the fresh transform back via
    /// [`crate::primitives::datum::DatumStore::refresh_derived_transform`].
    /// `LocationDescriptor` cache entries for any solid anchored to
    /// the changed datum (or to a refreshed dependent) are also
    /// invalidated.
    ///
    /// Auto-transforming the geometry of solids anchored to the
    /// changed datum is *not* part of this slice — that requires a
    /// `&mut self` lock to call `operations::transform::transform_solid`
    /// and is queued for a follow-up that takes the api-server lock
    /// upgrade. Today the cache is invalidated and the next
    /// descriptor read returns up-to-date `anchor_datum_name` /
    /// frame-relative center fields, but raw vertex positions stay
    /// where they were when the solid was first anchored.
    pub fn set_datum_transform(
        &self,
        id: crate::primitives::datum::DatumId,
        transform: Matrix4,
    ) -> Result<Matrix4, crate::primitives::datum::DatumError> {
        let previous = self.datums.set_transform(id, transform)?;
        // `DatumStore::set_transform` overrides the source to
        // `Manual`, severing any derived links this datum may have
        // had. Drop the stale forward-edges so future moves of the
        // old parents do not trigger no-op re-evaluations on this
        // (now-Manual) datum.
        self.datum_graph.unregister_dependent(id);
        self.propagate_datum_change(id);
        self.record_operation(
            crate::operations::recorder::RecordedOperation::new("datum_set_transform")
                .with_parameters(serde_json::json!({
                    "datum_id": id,
                    "transform": matrix4_to_row_major(&transform),
                    "previous_transform": matrix4_to_row_major(&previous),
                }))
                .with_input_datums([id as u64]),
        );
        Ok(previous)
    }

    // ───────────────────────────── Slice 5 propagation ────────────────────────
    //
    // `propagate_datum_change` is the single re-evaluation entry
    // point. It walks forward edges from the changed datum, refreshes
    // each derived dependent's transform, and invalidates the
    // location cache for any solid whose anchor datum sits anywhere
    // on the propagated subtree.
    //
    // The `*_changed` notifiers below are the geometry-side
    // counterparts: a topology mutator that moves a vertex / splits
    // a face / re-routes an edge calls them so derived datums whose
    // source references that geometry get refreshed too. They are
    // intentionally explicit (not auto-wired into every kernel
    // mutator) — slice 5's scope is the propagation infrastructure;
    // wiring every mutator to call them is the slice 6 readable-
    // surface task once we know which mutators land in the agent
    // surface.

    /// Re-evaluate every derived datum that transitively depends on
    /// `changed` and invalidate the location cache for affected
    /// solids. Cycle-safe via a visited set; no-ops on unknown id.
    pub fn propagate_datum_change(&self, changed: crate::primitives::datum::DatumId) {
        use std::collections::{HashSet, VecDeque};
        let mut visited: HashSet<crate::primitives::datum::DatumId> = HashSet::new();
        let mut queue: VecDeque<crate::primitives::datum::DatumId> = VecDeque::new();
        queue.push_back(changed);
        visited.insert(changed);

        // Invalidate cache for solids anchored to the changed datum
        // up-front — even if no derived dependents exist, the
        // anchor's `transform` (and therefore the descriptor's
        // `anchor_datum_name` / `center_in_anchor_frame`) is stale.
        for solid_id in self.datum_graph.solids_dependent_on_datum(changed) {
            self.location_cache.invalidate(solid_id);
        }
        // Also invalidate any solid whose anchor.datum_id is
        // currently `changed` — covers solids anchored before the
        // graph was populated (legacy creation paths) and solids
        // that wrote their anchor through `Solid::new` directly.
        for (sid, solid) in self.solids.iter() {
            if solid.anchor.datum_id == changed {
                self.location_cache.invalidate(sid);
            }
        }

        while let Some(parent) = queue.pop_front() {
            for dep in self.datum_graph.datums_dependent_on_datum(parent) {
                if !visited.insert(dep) {
                    continue;
                }
                self.refresh_dependent_datum(dep);
                // Also invalidate cache for solids anchored to this
                // refreshed datum.
                for solid_id in self.datum_graph.solids_dependent_on_datum(dep) {
                    self.location_cache.invalidate(solid_id);
                }
                queue.push_back(dep);
            }
        }
    }

    /// Notify the propagation graph that vertex `vid`'s position
    /// changed. Re-evaluates derived datums whose source references
    /// this vertex and invalidates affected location cache entries.
    pub fn vertex_changed(&self, vid: crate::primitives::vertex::VertexId) {
        for dep in self.datum_graph.datums_dependent_on_vertex(vid) {
            self.refresh_dependent_datum(dep);
            self.propagate_datum_change(dep);
        }
    }

    /// Notify the propagation graph that edge `eid` was modified
    /// (curve replaced, vertices reassigned). Re-evaluates derived
    /// datums whose source references this edge.
    pub fn edge_changed(&self, eid: crate::primitives::edge::EdgeId) {
        for dep in self.datum_graph.datums_dependent_on_edge(eid) {
            self.refresh_dependent_datum(dep);
            self.propagate_datum_change(dep);
        }
    }

    /// Notify the propagation graph that face `fid` was modified
    /// (surface replaced, loop edges changed). Re-evaluates derived
    /// datums whose source references this face.
    pub fn face_changed(&self, fid: crate::primitives::face::FaceId) {
        for dep in self.datum_graph.datums_dependent_on_face(fid) {
            self.refresh_dependent_datum(dep);
            self.propagate_datum_change(dep);
        }
    }

    /// Re-evaluate one derived datum's source against the current
    /// model and write the fresh transform back via
    /// `refresh_derived_transform`. On evaluation error logs at
    /// `tracing::warn` and leaves the stale transform in place — a
    /// missing reference (parent datum deleted, vertex consumed by
    /// a Boolean) should not abort the whole propagation walk.
    fn refresh_dependent_datum(&self, id: crate::primitives::datum::DatumId) {
        let Some(d) = self.datums.get(id) else {
            return;
        };
        match self.evaluate_datum_source(&d.source) {
            Ok(fresh) => {
                if let Err(e) = self.datums.refresh_derived_transform(id, fresh) {
                    tracing::warn!("datum {} refresh_derived_transform rejected: {}", id, e);
                }
            }
            Err(e) => {
                tracing::warn!(
                    "datum {} re-evaluation failed during propagation: {}",
                    id,
                    e
                );
            }
        }
    }

    /// Cached `LocationDescriptor` for the solid. On miss falls
    /// through to `solid_location_descriptor`, populates the cache
    /// with the result, and returns it.
    ///
    /// Cache-coherence is the caller's responsibility for direct
    /// geometry edits; `set_datum_transform`, `delete_datum`, and
    /// `transform_solid` already call `location_cache.invalidate`
    /// (or `invalidate_all` for broad-impact ops).
    pub fn solid_location_descriptor_cached(
        &self,
        id: SolidId,
    ) -> Option<crate::primitives::datum::LocationDescriptor> {
        if let Some(hit) = self.location_cache.get(id) {
            return Some(hit);
        }
        let fresh = self.solid_location_descriptor(id)?;
        self.location_cache.insert(fresh.clone());
        Some(fresh)
    }

    /// Delete a user-authored datum and record `datum_delete`.
    ///
    /// Slice 4a deletion is shallow — anchored solids that referenced
    /// this datum keep their `anchor.datum_id` pointing at a now-stale
    /// id. The api-server validates dependents at the request layer
    /// (409 unless `?cascade=detach`).
    pub fn delete_datum(
        &self,
        id: crate::primitives::datum::DatumId,
    ) -> Result<crate::primitives::datum::Datum, crate::primitives::datum::DatumError> {
        let removed = self.datums.delete(id)?;
        // Slice 5: tear down both directions of edges in the
        // dependency graph. The (parent → this datum) edges are gone
        // because the dependent no longer exists; the (this datum →
        // dependent) edges are gone because the upstream is gone and
        // any remaining listings would point at a stale id. The
        // api-server cascade-detach path is responsible for
        // re-binding orphaned dependents to the world Origin.
        self.datum_graph.unregister_dependent(id);
        self.datum_graph.unregister_upstream_datum(id);
        // Drop any cached descriptors for solids that were anchored
        // to this datum — their `anchor_datum_name` is now stale.
        for solid_id in self
            .datum_graph
            .solids_dependent_on_datum(id)
            .into_iter()
            .chain(
                self.solids
                    .iter()
                    .filter(|(_, s)| s.anchor.datum_id == id)
                    .map(|(sid, _)| sid),
            )
        {
            self.location_cache.invalidate(solid_id);
        }
        self.record_operation(
            crate::operations::recorder::RecordedOperation::new("datum_delete")
                .with_parameters(serde_json::json!({
                    "datum_id": id,
                    "name": removed.name.clone(),
                }))
                .with_input_datums([id as u64]),
        );
        Ok(removed)
    }

    /// Re-anchor a solid to a different datum, optionally with a new
    /// local-frame offset. Records `solid_reanchor`.
    ///
    /// Slice 6 mediator backing the agent-facing `anchor_part` tool.
    /// When `local_transform` is `None`, the solid's existing local
    /// transform is preserved (use case: re-parenting under a new
    /// datum without disturbing placement). When `Some(matrix)`, the
    /// supplied matrix replaces the local transform.
    ///
    /// Internally constructs a [`TopologyBuilder`] and delegates to
    /// `anchor_solid` so the slice-5 datum graph + cache invariants
    /// are preserved by a single canonical code path. The recorded
    /// event captures both the previous and new datum ids so timeline
    /// replay and AI tooling can introspect the change.
    pub fn reanchor_solid(
        &mut self,
        solid_id: SolidId,
        new_datum_id: crate::primitives::datum::DatumId,
        new_local_transform: Option<Matrix4>,
    ) -> Result<(), PrimitiveError> {
        // Validate solid id and capture previous anchor before
        // mutating, so the recorded event carries the before-state.
        let (prev_datum_id, preserved_local) = {
            let solid =
                self.solids
                    .get(solid_id)
                    .ok_or_else(|| PrimitiveError::InvalidParameters {
                        parameter: "solid_id".to_string(),
                        value: solid_id.to_string(),
                        constraint: "must reference an existing solid".to_string(),
                    })?;
            (solid.anchor.datum_id, solid.anchor.local_transform)
        };

        // Validate datum id eagerly so the error surface matches
        // `anchor_solid`'s without relying on its internal lookup.
        if self.datums.get(new_datum_id).is_none() {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "datum_id".to_string(),
                value: new_datum_id.to_string(),
                constraint: "must reference an existing datum".to_string(),
            });
        }

        let local = new_local_transform.unwrap_or(preserved_local);
        let mut builder = TopologyBuilder::new(self);
        builder.anchor_solid(solid_id, new_datum_id, local)?;

        self.record_operation(
            crate::operations::recorder::RecordedOperation::new("solid_reanchor")
                .with_parameters(serde_json::json!({
                    "solid_id": solid_id,
                    "previous_datum_id": prev_datum_id,
                    "new_datum_id": new_datum_id,
                    "local_transform_supplied": new_local_transform.is_some(),
                }))
                .with_input_solids([solid_id as u64])
                .with_input_datums([new_datum_id as u64])
                .with_output_solids([solid_id as u64]),
        );

        Ok(())
    }

    // ──────────────────── 4b: derived datum evaluation + authoring ───────────
    //
    // `evaluate_datum_source` is the single entry point that turns a
    // `DatumSource` recipe into a concrete `Matrix4` by reading vertex
    // positions, edge tangents, and face normals out of the kernel
    // stores. `create_derived_datum` is the recorder mediator: it
    // evaluates the source, inserts the datum via
    // `DatumStore::create_derived`, and emits a `datum_create_derived`
    // event so timeline replay reconstructs the same recipe — not the
    // baked transform, which is the slice 5 propagation invariant.
    //
    // This block is intentionally placed before the slice 3b queries so
    // the read APIs remain at the bottom of the impl, and so the
    // derived datum mediators sit next to the slice 4a authoring
    // mediators they extend.
    pub fn evaluate_datum_source(
        &self,
        source: &crate::primitives::datum::DatumSource,
    ) -> Result<Matrix4, crate::primitives::datum::DatumError> {
        use crate::primitives::datum::{
            frame_from_origin_and_z, frame_z_axis, DatumError, DatumSource,
        };
        match *source {
            DatumSource::Manual { transform } => Ok(DatumSource::unpack_matrix(transform)),
            DatumSource::OffsetPlane { base, distance } => {
                let base_d = self.datums.get(base).ok_or(DatumError::UnknownReference {
                    kind: "datum",
                    id: base as u64,
                })?;
                let normal = frame_z_axis(&base_d.transform);
                // Translate base origin along its +Z by `distance`.
                let new_origin = Point3::new(
                    base_d.origin.x + normal.x * distance,
                    base_d.origin.y + normal.y * distance,
                    base_d.origin.z + normal.z * distance,
                );
                frame_from_origin_and_z(new_origin, normal)
            }
            DatumSource::AnglePlane { base, axis, angle } => {
                let base_d = self.datums.get(base).ok_or(DatumError::UnknownReference {
                    kind: "datum",
                    id: base as u64,
                })?;
                let axis_d = self.datums.get(axis).ok_or(DatumError::UnknownReference {
                    kind: "datum",
                    id: axis as u64,
                })?;
                let axis_dir = frame_z_axis(&axis_d.transform);
                let base_normal = frame_z_axis(&base_d.transform);
                let rotation = Matrix4::from_axis_angle(&axis_dir, angle).map_err(|e| {
                    DatumError::EvaluationFailed(format!("angle-plane axis-angle: {e}"))
                })?;
                let rotated_normal = rotation
                    .transform_vector(&base_normal)
                    .normalize()
                    .map_err(|e| {
                        DatumError::EvaluationFailed(format!("angle-plane normal: {e}"))
                    })?;
                frame_from_origin_and_z(base_d.origin, rotated_normal)
            }
            DatumSource::ThreePoints { p0, p1, p2 } => {
                let v0 = self.vertices.get(p0).ok_or(DatumError::UnknownReference {
                    kind: "vertex",
                    id: p0 as u64,
                })?;
                let v1 = self.vertices.get(p1).ok_or(DatumError::UnknownReference {
                    kind: "vertex",
                    id: p1 as u64,
                })?;
                let v2 = self.vertices.get(p2).ok_or(DatumError::UnknownReference {
                    kind: "vertex",
                    id: p2 as u64,
                })?;
                let p0p = Point3::new(v0.position[0], v0.position[1], v0.position[2]);
                let p1p = Point3::new(v1.position[0], v1.position[1], v1.position[2]);
                let p2p = Point3::new(v2.position[0], v2.position[1], v2.position[2]);
                let e1 = Vector3::new(p1p.x - p0p.x, p1p.y - p0p.y, p1p.z - p0p.z);
                let e2 = Vector3::new(p2p.x - p0p.x, p2p.y - p0p.y, p2p.z - p0p.z);
                let normal = e1.cross(&e2);
                if normal.magnitude_squared() < f64::EPSILON * f64::EPSILON {
                    return Err(DatumError::DegenerateSource(
                        "three-points plane: collinear vertices",
                    ));
                }
                frame_from_origin_and_z(p0p, normal)
            }
            DatumSource::PlaneFromFace { face } => {
                let face_data = self.faces.get(face).ok_or(DatumError::UnknownReference {
                    kind: "face",
                    id: face as u64,
                })?;
                let centroid = loop_vertex_centroid(self, face_data.outer_loop)?;
                let u_mid = 0.5 * (face_data.uv_bounds[0] + face_data.uv_bounds[1]);
                let v_mid = 0.5 * (face_data.uv_bounds[2] + face_data.uv_bounds[3]);
                let normal = face_data
                    .normal_at(u_mid, v_mid, &self.surfaces)
                    .map_err(|e| DatumError::EvaluationFailed(format!("face normal: {e}")))?;
                frame_from_origin_and_z(centroid, normal)
            }
            DatumSource::MidPlane { a, b } => {
                let a_d = self.datums.get(a).ok_or(DatumError::UnknownReference {
                    kind: "datum",
                    id: a as u64,
                })?;
                let b_d = self.datums.get(b).ok_or(DatumError::UnknownReference {
                    kind: "datum",
                    id: b as u64,
                })?;
                let na = frame_z_axis(&a_d.transform);
                let nb = frame_z_axis(&b_d.transform);
                let avg = Vector3::new(na.x + nb.x, na.y + nb.y, na.z + nb.z);
                if avg.magnitude_squared() < f64::EPSILON * f64::EPSILON {
                    return Err(DatumError::DegenerateSource(
                        "mid-plane: parent normals are antiparallel",
                    ));
                }
                let mid = Point3::new(
                    0.5 * (a_d.origin.x + b_d.origin.x),
                    0.5 * (a_d.origin.y + b_d.origin.y),
                    0.5 * (a_d.origin.z + b_d.origin.z),
                );
                frame_from_origin_and_z(mid, avg)
            }
            DatumSource::EdgeAxis { edge } => {
                let edge_data = self.edges.get(edge).ok_or(DatumError::UnknownReference {
                    kind: "edge",
                    id: edge as u64,
                })?;
                let mid = edge_data
                    .evaluate(0.5, &self.curves)
                    .map_err(|e| DatumError::EvaluationFailed(format!("edge midpoint: {e}")))?;
                let tangent = edge_data
                    .tangent_at(0.5, &self.curves)
                    .map_err(|e| DatumError::EvaluationFailed(format!("edge tangent: {e}")))?;
                frame_from_origin_and_z(mid, tangent)
            }
            DatumSource::TwoPointsAxis { p0, p1 } => {
                let v0 = self.vertices.get(p0).ok_or(DatumError::UnknownReference {
                    kind: "vertex",
                    id: p0 as u64,
                })?;
                let v1 = self.vertices.get(p1).ok_or(DatumError::UnknownReference {
                    kind: "vertex",
                    id: p1 as u64,
                })?;
                let p0p = Point3::new(v0.position[0], v0.position[1], v0.position[2]);
                let p1p = Point3::new(v1.position[0], v1.position[1], v1.position[2]);
                let dir = Vector3::new(p1p.x - p0p.x, p1p.y - p0p.y, p1p.z - p0p.z);
                if dir.magnitude_squared() < f64::EPSILON * f64::EPSILON {
                    return Err(DatumError::DegenerateSource(
                        "two-points axis: coincident vertices",
                    ));
                }
                let mid = Point3::new(
                    0.5 * (p0p.x + p1p.x),
                    0.5 * (p0p.y + p1p.y),
                    0.5 * (p0p.z + p1p.z),
                );
                frame_from_origin_and_z(mid, dir)
            }
            DatumSource::NormalAxis { plane, point } => {
                let plane_d = self.datums.get(plane).ok_or(DatumError::UnknownReference {
                    kind: "datum",
                    id: plane as u64,
                })?;
                let v = self
                    .vertices
                    .get(point)
                    .ok_or(DatumError::UnknownReference {
                        kind: "vertex",
                        id: point as u64,
                    })?;
                let pos = Point3::new(v.position[0], v.position[1], v.position[2]);
                let dir = frame_z_axis(&plane_d.transform);
                frame_from_origin_and_z(pos, dir)
            }
            DatumSource::VertexPoint { vertex } => {
                let v = self
                    .vertices
                    .get(vertex)
                    .ok_or(DatumError::UnknownReference {
                        kind: "vertex",
                        id: vertex as u64,
                    })?;
                let pos = Point3::new(v.position[0], v.position[1], v.position[2]);
                Ok(Matrix4::from_translation(&Vector3::new(
                    pos.x, pos.y, pos.z,
                )))
            }
            DatumSource::CurveMidpoint { edge } => {
                let edge_data = self.edges.get(edge).ok_or(DatumError::UnknownReference {
                    kind: "edge",
                    id: edge as u64,
                })?;
                let mid = edge_data
                    .evaluate(0.5, &self.curves)
                    .map_err(|e| DatumError::EvaluationFailed(format!("edge midpoint: {e}")))?;
                Ok(Matrix4::from_translation(&Vector3::new(
                    mid.x, mid.y, mid.z,
                )))
            }
            DatumSource::FaceCentroid { face } => {
                let face_data = self.faces.get(face).ok_or(DatumError::UnknownReference {
                    kind: "face",
                    id: face as u64,
                })?;
                let centroid = loop_vertex_centroid(self, face_data.outer_loop)?;
                Ok(Matrix4::from_translation(&Vector3::new(
                    centroid.x, centroid.y, centroid.z,
                )))
            }
        }
    }

    /// Create a derived datum from a `DatumSource` recipe and record
    /// `datum_create_derived`. Evaluates the source against the current
    /// model, inserts the datum (with `kind` from
    /// `DatumSource::default_kind`), and emits a timeline event whose
    /// `parameters` carry the source recipe — replay re-evaluates the
    /// recipe rather than re-applying a baked transform, which keeps
    /// derived datums sticking to their referenced geometry across
    /// branches.
    pub fn create_derived_datum(
        &self,
        name: String,
        source: crate::primitives::datum::DatumSource,
    ) -> Result<crate::primitives::datum::DatumId, crate::primitives::datum::DatumError> {
        let transform = self.evaluate_datum_source(&source)?;
        let kind = source.default_kind();
        let id = self
            .datums
            .create_derived(name.clone(), kind, transform, source)?;
        // Slice 5: register forward edges from each parent referenced
        // by the source onto this newly-derived datum so subsequent
        // moves of the parents propagate.
        self.datum_graph.register_source(id, &source);
        self.record_operation(
            crate::operations::recorder::RecordedOperation::new("datum_create_derived")
                .with_parameters(serde_json::json!({
                    "datum_id": id,
                    "name": name,
                    "source": source,
                    "transform": matrix4_to_row_major(&transform),
                }))
                .with_output_datums([id as u64]),
        );
        Ok(id)
    }

    // ─────────────────────────── Datum-relative queries ──────────────────────
    //
    // Slice 3b: agent-facing read API. Every solid has a mandatory anchor
    // (slice 3a-i) and every datum carries an explicit `transform`
    // (slice 3d), so we can answer "where is part X relative to datum Y?"
    // without ad-hoc heuristics. These queries are O(faces × edges) over
    // a solid's outer shell and allocate one short-lived `HashSet` per
    // call; slice 5 will memoize per-solid descriptors in a `DashMap` and
    // invalidate them on transform / anchor / topology changes.

    /// World-space axis-aligned bounding box of the solid's outer-shell
    /// vertices. Returns `None` if the solid id is unknown, or if the
    /// shell has no reachable vertices (degenerate model).
    ///
    /// Anchored solids store geometry in world space (slice 2 invariant
    /// — moving a datum does not currently move dependents; that is
    /// slice 5's propagation graph), so this is exactly the world bbox.
    pub fn solid_world_bbox(&self, id: SolidId) -> Option<crate::math::BBox> {
        use std::collections::HashSet;
        let solid = self.solids.get(id)?;

        // Bound the TESSELLATED surface, not just the B-Rep vertices. An
        // analytic curved solid (cylinder / sphere / cone) carries only a
        // couple of seam vertices, so a vertex-only AABB collapses to the
        // seam line — e.g. a cylinder r10 h40 at the origin would report a
        // zero-width box at the seam (r,0) instead of x[-10,10]/y[-10,10].
        // That wrong bbox then poisons the OBB centre, camera auto-framing,
        // mass-property centroid and distance queries (KNOWN_BUGS #42). The
        // tessellation samples the full curved extent of every face.
        let mesh = crate::tessellation::tessellate_solid(
            solid,
            self,
            &crate::tessellation::TessellationParams::default(),
        );
        if !mesh.vertices.is_empty() {
            let pts: Vec<Point3> = mesh.vertices.iter().map(|v| v.position).collect();
            if let Some(bb) = crate::math::BBox::from_points(&pts) {
                return Some(bb);
            }
        }

        // Fallback (degenerate / empty tessellation): the B-Rep vertex hull.
        // Correct for polyhedral solids, where the vertices already bound
        // every planar face.
        let shell = self.shells.get(solid.outer_shell)?;
        let mut seen: HashSet<VertexId> = HashSet::new();
        let mut points: Vec<Point3> = Vec::with_capacity(shell.faces.len() * 4);
        for &face_id in &shell.faces {
            let Some(face) = self.faces.get(face_id) else {
                continue;
            };
            let Some(outer) = self.loops.get(face.outer_loop) else {
                continue;
            };
            for &edge_id in &outer.edges {
                let Some(edge) = self.edges.get(edge_id) else {
                    continue;
                };
                for vid in [edge.start_vertex, edge.end_vertex] {
                    if seen.insert(vid) {
                        if let Some(v) = self.vertices.get(vid) {
                            points.push(Point3::new(v.position[0], v.position[1], v.position[2]));
                        }
                    }
                }
            }
        }
        crate::math::BBox::from_points(&points)
    }

    /// Bounding box of the solid expressed in the local frame of the
    /// given datum. Computed by transforming the eight world-bbox
    /// corners through `datum.transform.inverse()` and rebuilding the
    /// AABB. The result is the *axis-aligned hull* of the rotated box,
    /// so it is conservative — for non-axis-aligned datums the local
    /// bbox is larger than a true OBB. Suitable for agent queries
    /// ("how big is X measured along the FrontPlane axes?") and
    /// containment tests.
    ///
    /// Returns `None` if the solid id is unknown, the datum id is
    /// unknown, or the datum's transform is non-invertible (would
    /// indicate a corrupt frame — never happens for the seven
    /// defaults).
    pub fn solid_bbox_in_frame(
        &self,
        id: SolidId,
        datum_id: crate::primitives::datum::DatumId,
    ) -> Option<crate::math::BBox> {
        let world = self.solid_world_bbox(id)?;
        let datum_frame = self.datums.frame(datum_id)?;
        let inv = datum_frame.inverse().ok()?;
        let local_corners: Vec<Point3> = world
            .corners()
            .iter()
            .map(|p| inv.transform_point(p))
            .collect();
        crate::math::BBox::from_points(&local_corners)
    }

    /// Center-to-center Euclidean distance between two solids' world
    /// bboxes. Returns `None` if either solid is unknown or has no
    /// reachable vertices.
    ///
    /// Slice 3b ships the bbox-center approximation; slice 5 will add
    /// surface-to-surface (`face_face_distance`) once the closest-point
    /// machinery for non-planar surfaces stabilizes. Agents that need a
    /// stricter measure should compose this with `solid_world_bbox` and
    /// inspect the bbox extents themselves.
    pub fn solid_distance(&self, a: SolidId, b: SolidId) -> Option<f64> {
        let bb_a = self.solid_world_bbox(a)?;
        let bb_b = self.solid_world_bbox(b)?;
        Some((bb_a.center() - bb_b.center()).magnitude())
    }

    /// Compose a `LocationDescriptor` for a solid: the agent-facing
    /// blob that summarizes where it lives, in what frame, and how big
    /// it is.
    ///
    /// `signed_distance_{front,top,right}` are measured against the
    /// canonical world planes (FrontPlane = XY / TopPlane = XZ /
    /// RightPlane = YZ at the world origin), independent of whether
    /// the matching default datums have been hidden, renamed, or
    /// otherwise mutated by the user. This guarantees agents always
    /// have a stable reference frame to reason in even when the user
    /// has reorganized their datum tree.
    ///
    /// Returns `None` if the solid id is unknown, the solid's anchor
    /// datum has been deleted (cannot happen today — defaults are
    /// undeletable, slice 4a's `delete` will refuse `is_default`), or
    /// the solid has no reachable vertices.
    pub fn solid_location_descriptor(
        &self,
        id: SolidId,
    ) -> Option<crate::primitives::datum::LocationDescriptor> {
        let solid = self.solids.get(id)?;
        let anchor_datum_id = solid.anchor.datum_id;
        let anchor_datum = self.datums.get(anchor_datum_id)?;
        let world_bbox = self.solid_world_bbox(id)?;
        let world_center = world_bbox.center();
        let world_size = world_bbox.size();

        let local_center = anchor_datum
            .transform
            .inverse()
            .ok()?
            .transform_point(&world_center);

        Some(crate::primitives::datum::LocationDescriptor {
            solid_id: id,
            anchor_datum_id,
            anchor_datum_name: anchor_datum.name.clone(),
            center_world: [world_center.x, world_center.y, world_center.z],
            center_in_anchor_frame: [local_center.x, local_center.y, local_center.z],
            dimensions_world: [world_size.x, world_size.y, world_size.z],
            // FrontPlane = XY plane, normal +Z → signed distance is the z-coord.
            signed_distance_front: world_center.z,
            // TopPlane   = XZ plane, normal +Y → signed distance is the y-coord.
            signed_distance_top: world_center.y,
            // RightPlane = YZ plane, normal +X → signed distance is the x-coord.
            signed_distance_right: world_center.x,
        })
    }

    /// Compute bounding box of all geometry in the model
    pub fn compute_bounding_box(&self) -> Option<crate::math::BBox> {
        use crate::math::BBox;

        let mut bbox: Option<BBox> = None;

        // Include all vertices in bounding box
        for (_, vertex) in self.vertices.iter() {
            // Use the vertex.point() method for consistent type-safe access
            let point = vertex.point();
            if let Some(ref mut bb) = bbox {
                bb.add_point_mut(&point);
            } else {
                bbox = Some(BBox::from_point(point));
            }
        }

        bbox
    }

    /// Get a solid by ID
    pub fn get_solid(&self, id: u32) -> Option<&crate::primitives::solid::Solid> {
        self.solids.get(id)
    }

    /// Calculate exact volume of a solid via the unified mass-properties
    /// pipeline.
    ///
    /// Delegates to [`Self::compute_solid_mass_properties`] so volume,
    /// surface area, centre of mass and inertia all come from the same
    /// integration pass (analytical face traversal on planar-faced
    /// solids, mesh-based Tonon (2004) integration when the analytical
    /// path aborts on degenerate seam loops). Callers (LLM summary
    /// reports, agent-facing part queries) never see numbers that drift
    /// relative to each other.
    ///
    /// Takes `&mut self` because the unified entry point populates
    /// per-entity caches on the solid, shell, face and loop stores on
    /// first call. Subsequent calls hit the cache and are free.
    pub fn calculate_solid_volume(&mut self, solid_id: u32) -> Option<f64> {
        self.compute_solid_mass_properties(solid_id)
            .map(|p| p.volume)
    }

    /// Calculate exact surface area of a solid.
    ///
    /// Delegates to [`Self::compute_solid_mass_properties`] for the
    /// same consistency-across-callers reason as [`Self::calculate_solid_volume`].
    pub fn calculate_solid_surface_area(&mut self, solid_id: u32) -> Option<f64> {
        self.compute_solid_mass_properties(solid_id)
            .map(|p| p.surface_area)
    }

    /// Unified mass-properties entry point. Returns volume, surface
    /// area, centre of mass, inertia tensor, principal moments,
    /// principal axes and radius of gyration in a single
    /// [`crate::primitives::solid::SolidMassProperties`] report.
    ///
    /// **Always routes through [`Self::mesh_based_mass_properties`].**
    /// The analytical face-by-face pipeline on `Solid::compute_mass_properties`
    /// is the only place the kernel can compute volume / surface area /
    /// COM in closed form, but its **inertia tensor** is the shell-level
    /// box-approximation (`Shell::compute_mass_properties`, shell.rs:516)
    /// fed through a parallel-axis shift that mixes density-1 second
    /// moments with full-mass shift terms — wrong by `density` for
    /// every solid and wrong by O(1) for non-box geometry. The mesh
    /// path produces an exact (to tessellation tolerance) inertia
    /// tensor for arbitrary geometry via Tonon (2004) per-tetrahedron
    /// formulas, so we use it uniformly. The wire-visible
    /// [`crate::primitives::solid::MassPropertiesMethod::Analytical`]
    /// variant is reserved for when the shell-level inertia is fixed
    /// (Future Work in `linear-inventing-oasis.md`).
    ///
    /// Result is installed into
    /// [`crate::primitives::solid::Solid::install_mass_props_cache`]
    /// so subsequent calls hit the cache without re-tessellating.
    pub(crate) fn compute_solid_mass_properties(
        &mut self,
        solid_id: u32,
    ) -> Option<crate::primitives::solid::SolidMassProperties> {
        // Hit the cache first — `install_mass_props_cache` populates
        // it after the first successful mesh integration, so repeated
        // calls (mass + obb + part-report on the same solid) avoid
        // re-tessellating.
        if let Some(solid) = self.solids.get(solid_id) {
            if let Some(cached) = solid.cached_mass_props_ref() {
                return Some(cached.clone());
            }
        }
        let props = self.mesh_based_mass_properties(solid_id)?;
        if let Some(solid) = self.solids.get_mut(solid_id) {
            solid.install_mass_props_cache(props.clone());
        }
        Some(props)
    }

    /// Mesh-based mass-properties pipeline. Tessellates the solid at
    /// [`crate::tessellation::TessellationParams::fine`] resolution
    /// and integrates volume, first moment (for COM), second-moment
    /// tensor (for inertia) and surface area in a single pass over
    /// the triangles.
    ///
    /// Each outward-oriented triangle `(v0, v1, v2)` forms a tetrahedron
    /// with the origin whose signed volume is
    /// `V_t = v0 · (v1 × v2) / 6`. Tonon (2004) closed-form formulas
    /// (Eq. 9 specialised to `v_1 = O = 0`) give the second-moment
    /// integrals over that tetrahedron:
    ///
    /// ```text
    ///   ∫x² dV_t = (V_t / 10) · (x0² + x1² + x2² + x0·x1 + x0·x2 + x1·x2)
    ///   ∫xy dV_t = (V_t / 20) · [2·(x0·y0 + x1·y1 + x2·y2)
    ///                            + x0·y1 + x1·y0 + x0·y2 + x2·y0
    ///                            + x1·y2 + x2·y1]
    /// ```
    ///
    /// Summing over every triangle gives the second-moment tensor
    /// about the origin; the parallel-axis shift to COM and the
    /// Jacobi eigendecomposition are factored out via
    /// [`crate::primitives::solid::principal_axes_from_origin_moments`]
    /// so the analytical and mesh paths share the exact same shape of
    /// output.
    ///
    /// Reference: Tonon, F. (2004). "Explicit Exact Formulas for the
    /// 3-D Tetrahedron Inertia Tensor in Terms of its Vertex
    /// Coordinates." *Journal of Mathematics and Statistics*, 1(1),
    /// 8-11.
    fn mesh_based_mass_properties(
        &self,
        solid_id: u32,
    ) -> Option<crate::primitives::solid::SolidMassProperties> {
        let solid = self.solids.get(solid_id)?;
        let density = solid.attributes.material.density;
        let mesh = crate::tessellation::tessellate_solid(
            solid,
            self,
            &crate::tessellation::TessellationParams::fine(),
        );
        if mesh.triangles.is_empty() {
            return None;
        }

        // Single pass: accumulate signed volume, first moments,
        // second moments (upper triangle only — tensor is symmetric)
        // and absolute surface area.
        let mut six_volume = 0.0_f64;
        let mut first_moment_sum = Vector3::ZERO; // Σ V_t · (v0 + v1 + v2)
                                                  // Second moments accumulator: integrals ∫x², ∫y², ∫z², ∫xy, ∫xz, ∫yz over the solid.
        let mut m_xx = 0.0_f64;
        let mut m_yy = 0.0_f64;
        let mut m_zz = 0.0_f64;
        let mut m_xy = 0.0_f64;
        let mut m_xz = 0.0_f64;
        let mut m_yz = 0.0_f64;
        let mut surface_area = 0.0_f64;

        for tri in &mesh.triangles {
            let v0 = mesh.vertices[tri[0] as usize].position.to_vec();
            let v1 = mesh.vertices[tri[1] as usize].position.to_vec();
            let v2 = mesh.vertices[tri[2] as usize].position.to_vec();

            // 6 V_t = v0 · (v1 × v2)
            let six_vt = v0.dot(&v1.cross(&v2));
            six_volume += six_vt;

            // First moment over tet (O, v0, v1, v2): V_t · (v0 + v1 + v2) / 4.
            // We accumulate V_t · (v0 + v1 + v2) here and divide by the
            // final total volume below.
            first_moment_sum += (v0 + v1 + v2) * (six_vt / 6.0);

            // Tonon (2004) Eq. 9 with v_1 = O = 0. For each diagonal
            // integral the coefficient is V_t / 10 on the same-index
            // squares (x_i²) and V_t / 30 on cross-products (x_i·x_j,
            // i ≠ j) — i.e. (V_t/10)·(x0² + x1² + x2² + x0·x1 + x0·x2
            // + x1·x2). We multiply by `six_vt / 6` (= V_t) implicitly
            // by absorbing `six_vt` and dividing by 60 / 180 respectively.
            let xs = [v0.x, v1.x, v2.x];
            let ys = [v0.y, v1.y, v2.y];
            let zs = [v0.z, v1.z, v2.z];
            let mut sx2 = 0.0;
            let mut sy2 = 0.0;
            let mut sz2 = 0.0;
            for i in 0..3 {
                sx2 += xs[i] * xs[i];
                sy2 += ys[i] * ys[i];
                sz2 += zs[i] * zs[i];
                for j in (i + 1)..3 {
                    sx2 += xs[i] * xs[j];
                    sy2 += ys[i] * ys[j];
                    sz2 += zs[i] * zs[j];
                }
            }
            // diag: ∫x² dV_t = (V_t / 10) · sx2 = (six_vt / 60) · sx2
            m_xx += six_vt * sx2 / 60.0;
            m_yy += six_vt * sy2 / 60.0;
            m_zz += six_vt * sz2 / 60.0;

            // Off-diagonals (Tonon Eq. 9 cross term):
            //   ∫xy dV_t = (V_t / 20) · [2·Σ x_i y_i + Σ_{i≠j} x_i y_j]
            //           = (six_vt / 120) · [2·(x0 y0 + x1 y1 + x2 y2)
            //                              + x0 y1 + x1 y0 + x0 y2
            //                              + x2 y0 + x1 y2 + x2 y1]
            let xy_diag = xs[0] * ys[0] + xs[1] * ys[1] + xs[2] * ys[2];
            let xz_diag = xs[0] * zs[0] + xs[1] * zs[1] + xs[2] * zs[2];
            let yz_diag = ys[0] * zs[0] + ys[1] * zs[1] + ys[2] * zs[2];
            let xy_cross = xs[0] * ys[1]
                + xs[1] * ys[0]
                + xs[0] * ys[2]
                + xs[2] * ys[0]
                + xs[1] * ys[2]
                + xs[2] * ys[1];
            let xz_cross = xs[0] * zs[1]
                + xs[1] * zs[0]
                + xs[0] * zs[2]
                + xs[2] * zs[0]
                + xs[1] * zs[2]
                + xs[2] * zs[1];
            let yz_cross = ys[0] * zs[1]
                + ys[1] * zs[0]
                + ys[0] * zs[2]
                + ys[2] * zs[0]
                + ys[1] * zs[2]
                + ys[2] * zs[1];
            m_xy += six_vt * (2.0 * xy_diag + xy_cross) / 120.0;
            m_xz += six_vt * (2.0 * xz_diag + xz_cross) / 120.0;
            m_yz += six_vt * (2.0 * yz_diag + yz_cross) / 120.0;

            // Surface area: ½ ‖(v1 − v0) × (v2 − v0)‖. Symmetric in
            // vertex order so triangle orientation does not matter.
            surface_area += 0.5 * (v1 - v0).cross(&(v2 - v0)).magnitude();
        }

        let signed_volume = six_volume / 6.0;
        let volume = signed_volume.abs();
        if volume < 1e-12 {
            return None;
        }

        // Sign-correct the moments so they refer to the outward-oriented
        // tessellation regardless of whether the kernel happened to emit
        // CW or CCW triangle winding. Tonon's formulas are linear in the
        // signed volume, so flipping the sign of every accumulator when
        // `signed_volume < 0` re-aligns them with the unsigned volume.
        //
        // First moment ∫r dV for tetrahedron (O, v0, v1, v2) is
        // V_t · (v0 + v1 + v2) / 4. Accumulator above sums V_t · (v0 +
        // v1 + v2), so we divide by 4 here when normalising to COM.
        let orient = signed_volume.signum();
        let center_of_mass = Point3::new(
            orient * first_moment_sum.x / (4.0 * volume),
            orient * first_moment_sum.y / (4.0 * volume),
            orient * first_moment_sum.z / (4.0 * volume),
        );

        // Origin-frame inertia: mass-weighted second moments
        //   I_xx = ∫ρ(y² + z²) dV, I_xy = -∫ρ·xy dV, etc.
        // Tonon accumulators above are pure volume integrals (∫r² dV),
        // so we multiply by `density` here to align with the
        // parallel-axis shift in `principal_axes_from_origin_moments`,
        // which uses physical mass.
        let i_origin = [
            [
                orient * density * (m_yy + m_zz),
                -orient * density * m_xy,
                -orient * density * m_xz,
            ],
            [
                -orient * density * m_xy,
                orient * density * (m_xx + m_zz),
                -orient * density * m_yz,
            ],
            [
                -orient * density * m_xz,
                -orient * density * m_yz,
                orient * density * (m_xx + m_yy),
            ],
        ];
        let mass = volume * density;
        let (inertia_tensor, principal_moments, principal_axes) =
            crate::primitives::solid::principal_axes_from_origin_moments(
                i_origin,
                mass,
                &center_of_mass,
            );
        let radius_of_gyration = Vector3::new(
            (principal_moments.x / mass).sqrt(),
            (principal_moments.y / mass).sqrt(),
            (principal_moments.z / mass).sqrt(),
        );

        // PHYSICAL-VALIDITY GATE — the kernel must not LIE with mass-properties.
        //
        // The integration above is the divergence theorem summed per tetrahedron
        // (Tonon 2004). It is only correct over a CLOSED, consistently-wound
        // surface. A malformed / non-watertight solid (e.g. a curved-boolean
        // husk with leaked or oppositely-wound triangles) makes the per-triangle
        // tetrahedra cancel inconsistently and yields a tensor that is symmetric
        // but INDEFINITE — a NEGATIVE principal moment, which a real rigid body
        // cannot have (the inertia tensor is positive-semidefinite). Returning
        // such numbers is a form of lying.
        //
        // So we VERIFY the result against the physical contract and REFUSE
        // (return `None`, logging the reason) if it is non-physical, rather than
        // hand a caller bogus numbers. We do NOT clamp a negative moment to zero
        // — that would be a different lie; we refuse. The public surface
        // (`compute_solid_mass_properties` → `mass_properties_for`) already
        // returns `Option`, so a refusal is a `None` that callers already handle
        // (the agent-facing report surfaces it as "indeterminate", not a fake
        // tensor). Clean solids are unaffected — they pass every check.
        if let Err(reason) =
            check_mass_properties_physical(volume, surface_area, &inertia_tensor, principal_moments)
        {
            tracing::warn!(
                "mass-properties REFUSED for solid {solid_id}: non-physical result \
                 ({reason}) — the source solid is malformed (non-watertight / \
                 inconsistently-wound mesh); returning None rather than emitting \
                 a physically impossible tensor"
            );
            return None;
        }

        Some(crate::primitives::solid::SolidMassProperties {
            volume,
            surface_area,
            mass,
            center_of_mass,
            inertia_tensor,
            principal_moments,
            principal_axes,
            radius_of_gyration,
            method: crate::primitives::solid::MassPropertiesMethod::Tessellated {
                // Empirical bound at `TessellationParams::fine()`:
                // matches analytical formulas to ~5e-3 relative on
                // curved primitives (sphere/cylinder/cone) per the
                // kernel_workflow_regression suite.
                rel_tolerance: 5e-3,
            },
        })
    }

    /// Tessellate a solid into a watertight triangle mesh for visualization
    pub fn tessellate_solid(&self, solid_id: u32, _tolerance: f64) -> Option<TessellatedMesh> {
        let solid = self.solids.get(solid_id)?;
        let shell = self.shells.get(solid.outer_shell)?;

        let mut vertices = Vec::new();
        let mut normals = Vec::new();
        let mut indices = Vec::new();

        // Vertex deduplication map: maps vertex ID to index in vertices array
        // This ensures watertight mesh by sharing vertices between faces
        let mut vertex_index_map: HashMap<VertexId, u32> = HashMap::new();

        // First pass: collect all unique vertices from the solid
        for &face_id in &shell.faces {
            let face = self.faces.get(face_id)?;
            let outer_loop = self.loops.get(face.outer_loop)?;

            for &edge_id in &outer_loop.edges {
                let edge = self.edges.get(edge_id)?;

                // Process start vertex
                if let std::collections::hash_map::Entry::Vacant(e) =
                    vertex_index_map.entry(edge.start_vertex)
                {
                    if let Some(vertex) = self.vertices.get(edge.start_vertex) {
                        let point = vertex.point();
                        let idx = vertices.len() as u32;
                        vertices.push([point.x as f32, point.y as f32, point.z as f32]);
                        // Initialize with zero normal, will accumulate later
                        normals.push([0.0, 0.0, 0.0]);
                        e.insert(idx);
                    }
                }

                // Process end vertex
                if let std::collections::hash_map::Entry::Vacant(e) =
                    vertex_index_map.entry(edge.end_vertex)
                {
                    if let Some(vertex) = self.vertices.get(edge.end_vertex) {
                        let point = vertex.point();
                        let idx = vertices.len() as u32;
                        vertices.push([point.x as f32, point.y as f32, point.z as f32]);
                        normals.push([0.0, 0.0, 0.0]);
                        e.insert(idx);
                    }
                }
            }
        }

        // Second pass: create triangles and accumulate normals
        for &face_id in &shell.faces {
            let face = self.faces.get(face_id)?;
            let outer_loop = self.loops.get(face.outer_loop)?;

            // Collect vertex indices for this face
            let mut face_vertex_ids = Vec::new();
            let mut face_vertex_indices = Vec::new();

            for &edge_id in &outer_loop.edges {
                let edge = self.edges.get(edge_id)?;
                if let Some(&idx) = vertex_index_map.get(&edge.start_vertex) {
                    face_vertex_ids.push(edge.start_vertex);
                    face_vertex_indices.push(idx);
                }
            }

            if face_vertex_indices.len() < 3 {
                continue;
            }

            // Calculate face normal using first three vertices
            let v0 = Point3::new(
                vertices[face_vertex_indices[0] as usize][0] as f64,
                vertices[face_vertex_indices[0] as usize][1] as f64,
                vertices[face_vertex_indices[0] as usize][2] as f64,
            );
            let v1 = Point3::new(
                vertices[face_vertex_indices[1] as usize][0] as f64,
                vertices[face_vertex_indices[1] as usize][1] as f64,
                vertices[face_vertex_indices[1] as usize][2] as f64,
            );
            let v2 = Point3::new(
                vertices[face_vertex_indices[2] as usize][0] as f64,
                vertices[face_vertex_indices[2] as usize][1] as f64,
                vertices[face_vertex_indices[2] as usize][2] as f64,
            );

            let edge1 = v1 - v0;
            let edge2 = v2 - v0;
            let face_normal = edge1.cross(&edge2).normalize().unwrap_or(Vector3::Z);

            // Apply face orientation
            let oriented_normal =
                if face.orientation == crate::primitives::face::FaceOrientation::Forward {
                    face_normal
                } else {
                    -face_normal
                };

            // Add face normal contribution to all vertices of this face
            // This creates smooth normals at shared vertices
            for &idx in &face_vertex_indices {
                normals[idx as usize][0] += oriented_normal.x as f32;
                normals[idx as usize][1] += oriented_normal.y as f32;
                normals[idx as usize][2] += oriented_normal.z as f32;
            }

            // Fan triangulation about vertex[0]. This is valid for the
            // convex faces produced by all primitive constructors used here
            // (boxes, cylinders, spheres, cones, tori). Concave or holed
            // faces require ear-clipping or constrained Delaunay; those go
            // through the dedicated tessellation pipeline instead of this
            // fast-path display mesh builder.
            let base_idx = face_vertex_indices[0];
            for i in 2..face_vertex_indices.len() {
                // Ensure consistent winding order for watertight mesh
                if face.orientation == crate::primitives::face::FaceOrientation::Forward {
                    indices.push(base_idx);
                    indices.push(face_vertex_indices[i - 1]);
                    indices.push(face_vertex_indices[i]);
                } else {
                    indices.push(base_idx);
                    indices.push(face_vertex_indices[i]);
                    indices.push(face_vertex_indices[i - 1]);
                }
            }
        }

        // Process inner shells with the same vertex sharing approach
        for &inner_shell_id in &solid.inner_shells {
            let inner_shell = self.shells.get(inner_shell_id)?;

            for &face_id in &inner_shell.faces {
                let face = self.faces.get(face_id)?;
                let outer_loop = self.loops.get(face.outer_loop)?;

                // Collect vertex indices for this face
                let mut face_vertex_indices = Vec::new();

                for &edge_id in &outer_loop.edges {
                    let edge = self.edges.get(edge_id)?;
                    if let Some(&idx) = vertex_index_map.get(&edge.start_vertex) {
                        face_vertex_indices.push(idx);
                    }
                }

                if face_vertex_indices.len() < 3 {
                    continue;
                }

                // Calculate face normal
                let v0 = Point3::new(
                    vertices[face_vertex_indices[0] as usize][0] as f64,
                    vertices[face_vertex_indices[0] as usize][1] as f64,
                    vertices[face_vertex_indices[0] as usize][2] as f64,
                );
                let v1 = Point3::new(
                    vertices[face_vertex_indices[1] as usize][0] as f64,
                    vertices[face_vertex_indices[1] as usize][1] as f64,
                    vertices[face_vertex_indices[1] as usize][2] as f64,
                );
                let v2 = Point3::new(
                    vertices[face_vertex_indices[2] as usize][0] as f64,
                    vertices[face_vertex_indices[2] as usize][1] as f64,
                    vertices[face_vertex_indices[2] as usize][2] as f64,
                );

                let edge1 = v1 - v0;
                let edge2 = v2 - v0;
                let face_normal = edge1.cross(&edge2).normalize().unwrap_or(Vector3::Z);

                // Inner shells have inverted normals for voids
                let oriented_normal =
                    if face.orientation == crate::primitives::face::FaceOrientation::Forward {
                        -face_normal
                    } else {
                        face_normal
                    };

                // Add normal contribution
                for &idx in &face_vertex_indices {
                    normals[idx as usize][0] += oriented_normal.x as f32;
                    normals[idx as usize][1] += oriented_normal.y as f32;
                    normals[idx as usize][2] += oriented_normal.z as f32;
                }

                // Triangulate with reversed winding for inner shells
                let base_idx = face_vertex_indices[0];
                for i in 2..face_vertex_indices.len() {
                    if face.orientation == crate::primitives::face::FaceOrientation::Forward {
                        // Reversed winding for inner shells
                        indices.push(base_idx);
                        indices.push(face_vertex_indices[i]);
                        indices.push(face_vertex_indices[i - 1]);
                    } else {
                        indices.push(base_idx);
                        indices.push(face_vertex_indices[i - 1]);
                        indices.push(face_vertex_indices[i]);
                    }
                }
            }
        }

        // Normalize all accumulated normals
        for normal in &mut normals {
            let nx = normal[0];
            let ny = normal[1];
            let nz = normal[2];
            let length = (nx * nx + ny * ny + nz * nz).sqrt();
            if length > 1e-6 {
                normal[0] /= length;
                normal[1] /= length;
                normal[2] /= length;
            } else {
                // Default to up vector if degenerate
                normal[0] = 0.0;
                normal[1] = 0.0;
                normal[2] = 1.0;
            }
        }

        Some(TessellatedMesh {
            vertices,
            normals,
            indices,
        })
    }

    /// Cascading delete of a vertex.
    ///
    /// Removes every edge that uses the vertex, then every loop that uses one
    /// of those edges, then every face whose outer or inner loop is removed,
    /// and finally drops the face from each shell that referenced it. The
    /// vertex itself is removed last.
    ///
    /// Linear scans are used to find dependents because the per-store
    /// reverse-index (`vertex_to_edges`, `edge_to_loops`, `loop_to_faces`,
    /// `face_to_shells`) is only maintained on the slow `add_with_indexing`
    /// path; the fast `add` path skips it, so the cached lookup is unreliable
    /// in the general case. Cascade delete is not on the hot creation path —
    /// linear is correct and predictable.
    ///
    /// On success the operation is recorded via [`record_operation`] with the
    /// full set of removed entity ids in the parameters. A vertex that is
    /// already absent yields an empty [`CascadeReport`] and no record.
    pub fn delete_vertex_cascade(&mut self, vertex_id: VertexId) -> CascadeReport {
        let mut report = CascadeReport::default();

        let dependent_edges: Vec<EdgeId> = self
            .edges
            .iter()
            .filter_map(|(eid, e)| {
                (e.start_vertex == vertex_id || e.end_vertex == vertex_id).then_some(eid)
            })
            .collect();
        for eid in dependent_edges {
            self.cascade_delete_edge(eid, &mut report);
        }

        if self.vertices.remove(vertex_id) {
            report.removed_vertices.push(vertex_id);
            self.record_cascade(
                "delete_vertex_cascade",
                crate::operations::recorder::ENTITY_VERTEX,
                vertex_id as u64,
                &report,
            );
        }
        report
    }

    /// Cascading delete of an edge — removes dependent loops, faces, and
    /// shell face-references before dropping the edge.
    pub fn delete_edge_cascade(&mut self, edge_id: EdgeId) -> CascadeReport {
        let mut report = CascadeReport::default();
        let removed = self.cascade_delete_edge(edge_id, &mut report);
        if removed {
            self.record_cascade(
                "delete_edge_cascade",
                crate::operations::recorder::ENTITY_EDGE,
                edge_id as u64,
                &report,
            );
        }
        report
    }

    /// Cascading delete of a face — removes the face from every referencing
    /// shell, then drops the face. Loops are not deleted: they may be shared
    /// with other faces. Use [`delete_loop_cascade`] explicitly if you also
    /// want the bounding loop torn down.
    pub fn delete_face_cascade(&mut self, face_id: FaceId) -> CascadeReport {
        let mut report = CascadeReport::default();
        let removed = self.cascade_delete_face(face_id, &mut report);
        if removed {
            self.record_cascade(
                "delete_face_cascade",
                crate::operations::recorder::ENTITY_FACE,
                face_id as u64,
                &report,
            );
        }
        report
    }

    /// Cascading delete of a loop — removes faces that bound on the loop
    /// (and their shell references), then drops the loop. Edges are not
    /// deleted: they may be shared with other loops.
    pub fn delete_loop_cascade(&mut self, loop_id: LoopId) -> CascadeReport {
        let mut report = CascadeReport::default();
        let removed = self.cascade_delete_loop(loop_id, &mut report);
        if removed {
            self.record_cascade(
                "delete_loop_cascade",
                crate::operations::recorder::ENTITY_LOOP,
                loop_id as u64,
                &report,
            );
        }
        report
    }

    fn cascade_delete_edge(&mut self, edge_id: EdgeId, report: &mut CascadeReport) -> bool {
        if report.removed_edges.contains(&edge_id) {
            return false;
        }

        let dependent_loops: Vec<LoopId> = self
            .loops
            .iter()
            .filter_map(|(lid, l)| l.edges.contains(&edge_id).then_some(lid))
            .collect();
        for lid in dependent_loops {
            self.cascade_delete_loop(lid, report);
        }

        if self.edges.remove(edge_id).is_some() {
            report.removed_edges.push(edge_id);
            true
        } else {
            false
        }
    }

    fn cascade_delete_loop(&mut self, loop_id: LoopId, report: &mut CascadeReport) -> bool {
        if report.removed_loops.contains(&loop_id) {
            return false;
        }

        let dependent_faces: Vec<FaceId> = self
            .faces
            .iter()
            .filter_map(|(fid, f)| {
                (f.outer_loop == loop_id || f.inner_loops.contains(&loop_id)).then_some(fid)
            })
            .collect();
        for fid in dependent_faces {
            self.cascade_delete_face(fid, report);
        }

        if self.loops.remove(loop_id).is_some() {
            report.removed_loops.push(loop_id);
            true
        } else {
            false
        }
    }

    fn cascade_delete_face(&mut self, face_id: FaceId, report: &mut CascadeReport) -> bool {
        if report.removed_faces.contains(&face_id) {
            return false;
        }

        let referencing_shells: Vec<ShellId> = self
            .shells
            .iter()
            .filter_map(|(sid, s)| s.find_face(face_id).map(|_| sid))
            .collect();
        for sid in referencing_shells {
            if let Some(shell) = self.shells.get_mut(sid) {
                shell.remove_face(face_id);
            }
            if !report.affected_shells.contains(&sid) {
                report.affected_shells.push(sid);
            }
        }

        if self.faces.remove(face_id).is_some() {
            report.removed_faces.push(face_id);
            true
        } else {
            false
        }
    }

    fn record_cascade(
        &self,
        kind: &str,
        root_entity_kind: &str,
        root_id: u64,
        report: &CascadeReport,
    ) {
        use crate::operations::recorder::{
            entity_ref, RecordedOperation, ENTITY_EDGE, ENTITY_FACE, ENTITY_LOOP, ENTITY_VERTEX,
        };
        let outputs: Vec<String> = report
            .removed_vertices
            .iter()
            .map(|id| entity_ref(ENTITY_VERTEX, *id as u64))
            .chain(
                report
                    .removed_edges
                    .iter()
                    .map(|id| entity_ref(ENTITY_EDGE, *id as u64)),
            )
            .chain(
                report
                    .removed_loops
                    .iter()
                    .map(|id| entity_ref(ENTITY_LOOP, *id as u64)),
            )
            .chain(
                report
                    .removed_faces
                    .iter()
                    .map(|id| entity_ref(ENTITY_FACE, *id as u64)),
            )
            .collect();
        self.record_operation(
            RecordedOperation::new(kind)
                .with_input_refs([entity_ref(root_entity_kind, root_id)])
                .with_output_refs(outputs)
                .with_parameters(serde_json::json!({
                    "removed_vertices": report.removed_vertices,
                    "removed_edges": report.removed_edges,
                    "removed_loops": report.removed_loops,
                    "removed_faces": report.removed_faces,
                    "affected_shells": report.affected_shells,
                })),
        );
    }
}

/// Report returned by the cascading-delete entry points on [`BRepModel`].
///
/// Each `removed_*` list contains the entity ids that were marked deleted
/// (in topological discovery order). `affected_shells` lists the shells that
/// had at least one face reference removed but whose own ids remain valid.
#[derive(Debug, Clone, Default)]
pub struct CascadeReport {
    pub removed_vertices: Vec<VertexId>,
    pub removed_edges: Vec<EdgeId>,
    pub removed_loops: Vec<LoopId>,
    pub removed_faces: Vec<FaceId>,
    pub affected_shells: Vec<ShellId>,
}

impl Default for BRepModel {
    fn default() -> Self {
        Self::new()
    }
}

/// Timeline operation types for parametric modeling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TimelineOperation {
    /// 2D primitive creation
    Create2D {
        primitive_type: String,
        parameters: HashMap<String, f64>,
        timestamp: u64,
    },
    /// 3D primitive creation
    Create3D {
        primitive_type: String,
        parameters: HashMap<String, f64>,
        timestamp: u64,
    },
    /// Extrude 2D to 3D
    Extrude {
        profile_id: GeometryId,
        direction: Vector3,
        distance: f64,
        timestamp: u64,
    },
    /// Revolve 2D around axis
    Revolve {
        profile_id: GeometryId,
        axis_origin: Point3,
        axis_direction: Vector3,
        angle: f64,
        timestamp: u64,
    },
    /// Boolean operation
    Boolean {
        operation: BooleanOp,
        operand_ids: Vec<GeometryId>,
        timestamp: u64,
    },
    /// Parameter update
    UpdateParameters {
        geometry_id: GeometryId,
        new_parameters: HashMap<String, f64>,
        timestamp: u64,
    },
}

/// Universal geometry ID that works for 2D and 3D
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GeometryId {
    /// 2D geometry (stored as face)
    Face(FaceId),
    /// 3D geometry (stored as solid)
    Solid(SolidId),
    /// Curve geometry (1D)
    Edge(EdgeId),
    /// Point geometry (0D)
    Vertex(VertexId),
}

impl std::fmt::Display for GeometryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GeometryId::Face(id) => write!(f, "face_{}", id),
            GeometryId::Solid(id) => write!(f, "solid_{}", id),
            GeometryId::Edge(id) => write!(f, "edge_{}", id),
            GeometryId::Vertex(id) => write!(f, "vertex_{}", id),
        }
    }
}

/// Flatten a typed `GeometryId` to the plain `u64` entity handle exposed
/// to external recorders.
///
/// `FaceId`, `SolidId`, `EdgeId`, and `VertexId` are all `u32` aliases, so
/// this is a widening cast with no data loss. The entity *kind* is **not**
/// preserved in the returned u64 — callers relying on round-trip identity
/// must consult the accompanying `RecordedOperation::parameters` payload,
/// which serializes the original `TimelineOperation` in full.
/// Whether `m` is within `eps` of the 4×4 identity, element-wise.
///
/// Used by datum-anchored primitive creators to skip the inner
/// `transform_solid` call (and its timeline event) when the composed
/// world transform is a no-op — the most common case being anchoring
/// to the world Origin with an identity local transform.
fn is_approx_identity(m: &Matrix4, eps: f64) -> bool {
    for r in 0..4 {
        for c in 0..4 {
            let expected = if r == c { 1.0 } else { 0.0 };
            if (m.get(r, c) - expected).abs() > eps {
                return false;
            }
        }
    }
    true
}

/// Convert a typed `GeometryId` to the canonical namespaced wire form
/// (`"<kind>:<id>"`) consumed by `RecordedOperation::inputs` / `outputs`.
///
/// Solids, faces, edges, and vertices each occupy independent `u32`
/// counter namespaces in the kernel, so the bare integer alone cannot
/// disambiguate them downstream (Feature Tree lineage walker, persisted
/// timeline). The namespace prefix is the single source of identity.
fn geometry_id_to_ref(id: GeometryId) -> String {
    use crate::operations::recorder::{
        entity_ref, ENTITY_EDGE, ENTITY_FACE, ENTITY_SOLID, ENTITY_VERTEX,
    };
    match id {
        GeometryId::Face(i) => entity_ref(ENTITY_FACE, i as u64),
        GeometryId::Solid(i) => entity_ref(ENTITY_SOLID, i as u64),
        GeometryId::Edge(i) => entity_ref(ENTITY_EDGE, i as u64),
        GeometryId::Vertex(i) => entity_ref(ENTITY_VERTEX, i as u64),
    }
}

/// Boolean operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BooleanOp {
    Union,
    Intersection,
    Difference,
    SymmetricDifference,
}

/// Universal topology builder that handles all primitive types
pub struct TopologyBuilder<'a> {
    pub model: &'a mut BRepModel,
    timeline: Vec<TimelineOperation>,
    next_timestamp: u64,
    tolerance: Tolerance,
}

/// Builder type alias for backward compatibility
pub type Builder<'a> = TopologyBuilder<'a>;

impl<'a> TopologyBuilder<'a> {
    /// Create new topology builder
    pub fn new(model: &'a mut BRepModel) -> Self {
        Self {
            model,
            timeline: Vec::new(),
            next_timestamp: 0,
            tolerance: Tolerance::default(),
        }
    }

    /// Set construction tolerance
    pub fn with_tolerance(mut self, tolerance: Tolerance) -> Self {
        self.tolerance = tolerance;
        self
    }

    /// Get next timestamp for timeline
    fn next_timestamp(&mut self) -> u64 {
        let ts = self.next_timestamp;
        self.next_timestamp += 1;
        ts
    }

    /// Assign root persistent-ids to a freshly-built primitive solid and its
    /// faces (#11, slice 40-B). The solid gets a root PID seeded from the current
    /// event key (timeline-stable) or the monotonic fallback; each face gets a
    /// PID derived from the solid PID + its creation-order index — a stable
    /// per-face role, since each constructor builds faces in a fixed order. Edge
    /// / vertex PIDs are wired in later op slices.
    fn assign_primitive_pids(
        &mut self,
        solid_id: SolidId,
        kind: crate::primitives::persistent_id::PrimitiveKind,
    ) {
        use crate::primitives::persistent_id::{PersistentId, Role};
        // Seed from (event, kind) ONLY — never the dimensions. The event key (or
        // the counter fallback) already disambiguates distinct creations, and
        // excluding the parameters is what makes the root PID survive a MOULD
        // (edit a dimension → same identity, new geometry). #11/#16.
        let seed = self.model.next_root_seed(&format!("{kind:?}"));
        let solid_pid = PersistentId::root(&seed);
        self.model.set_solid_pid(solid_id, solid_pid);

        // PILLAR 1: a primitive is honestly tagged as a bare stand-in, never a
        // designed surface. Single chokepoint for box/sphere/cylinder/cone/torus.
        self.model.set_solid_provenance(
            solid_id,
            crate::primitives::provenance::OperationKind::Primitive(kind),
            Vec::new(),
        );

        let face_ids: Vec<FaceId> = {
            let mut fs = Vec::new();
            if let Some(solid) = self.model.solids.get(solid_id) {
                let mut shells = vec![solid.outer_shell];
                shells.extend_from_slice(&solid.inner_shells);
                for sh in shells {
                    if let Some(shell) = self.model.shells.get(sh) {
                        fs.extend_from_slice(&shell.faces);
                    }
                }
            }
            fs
        };
        for (i, fid) in face_ids.iter().enumerate() {
            let role = Role::Generic {
                source_pid: solid_pid,
                label: format!("face{i}"),
            };
            let fpid = PersistentId::derive(&[solid_pid], "primitive_face", &role);
            self.model.set_face_pid(*fid, fpid);
        }
    }

    /// Push a `TimelineOperation` to the builder's internal timeline **and**
    /// forward a canonical `RecordedOperation` to the model's attached
    /// recorder (if any).
    ///
    /// This is the single emission point that keeps the two history paths
    /// in sync:
    ///
    /// 1. `self.timeline` — the kernel-internal accumulator (kept for any
    ///    existing consumer of `get_timeline`).
    /// 2. `self.model.record_operation` — the dependency-inverted trait
    ///    handoff to `timeline-engine` (or any other recorder) living
    ///    outside the kernel.
    ///
    /// `outputs` should list the typed `GeometryId`s produced by the
    /// operation (e.g. the newly created solid/face/edge). Pass an empty
    /// `Vec` when the call is purely destructive or modifies existing
    /// entities in place. Each `GeometryId` carries its entity kind, which
    /// is preserved through the recorder as a namespaced `"<kind>:<id>"`
    /// string — solid/face/edge/vertex counters overlap in integer space,
    /// so bare integers cannot disambiguate them downstream.
    fn record_and_push(&mut self, operation: TimelineOperation, outputs: Vec<GeometryId>) {
        // Preserve existing in-builder timeline semantics verbatim.
        self.timeline.push(operation.clone());

        // Build the canonical outward record.
        let kind = match &operation {
            TimelineOperation::Create2D { primitive_type, .. } => {
                format!("create_{}_2d", primitive_type)
            }
            TimelineOperation::Create3D { primitive_type, .. } => {
                format!("create_{}_3d", primitive_type)
            }
            TimelineOperation::Extrude { .. } => "extrude".to_string(),
            TimelineOperation::Revolve { .. } => "revolve".to_string(),
            TimelineOperation::Boolean {
                operation: op_kind, ..
            } => {
                let suffix = match op_kind {
                    BooleanOp::Union => "union",
                    BooleanOp::Intersection => "intersection",
                    BooleanOp::Difference => "difference",
                    BooleanOp::SymmetricDifference => "symmetric_difference",
                };
                format!("boolean_{}", suffix)
            }
            TimelineOperation::UpdateParameters { .. } => "update_parameters".to_string(),
        };

        // Derive inputs structurally from variants that reference existing
        // entities. Each input carries its `GeometryId` kind, so we route
        // through the namespacing helper rather than dropping to bare u64.
        let inputs: Vec<String> = match &operation {
            TimelineOperation::Extrude { profile_id, .. } => {
                vec![geometry_id_to_ref(*profile_id)]
            }
            TimelineOperation::Boolean { operand_ids, .. } => operand_ids
                .iter()
                .copied()
                .map(geometry_id_to_ref)
                .collect(),
            TimelineOperation::UpdateParameters { geometry_id, .. } => {
                vec![geometry_id_to_ref(*geometry_id)]
            }
            _ => Vec::new(),
        };

        let output_refs: Vec<String> = outputs.into_iter().map(geometry_id_to_ref).collect();

        // Serialize the full TimelineOperation as the parameters payload
        // so a recorder can replay without lossy encoding.
        let parameters = match serde_json::to_value(&operation) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("failed to serialize TimelineOperation for recorder: {}", e);
                serde_json::Value::Null
            }
        };

        let record = crate::operations::recorder::RecordedOperation::new(kind)
            .with_parameters(parameters)
            .with_input_refs(inputs)
            .with_output_refs(output_refs);

        self.model.record_operation(record);
    }

    // =====================================
    // 2D PRIMITIVE CREATION METHODS
    // =====================================

    /// Create 2D point
    pub fn create_point_2d(&mut self, x: f64, y: f64) -> Result<GeometryId, PrimitiveError> {
        let vertex_id = self
            .model
            .vertices
            .add_or_find(x, y, 0.0, self.tolerance.distance());

        // Record in timeline + forward to attached recorder.
        let operation = TimelineOperation::Create2D {
            primitive_type: "point".to_string(),
            parameters: [("x".to_string(), x), ("y".to_string(), y)].into(),
            timestamp: self.next_timestamp(),
        };
        self.record_and_push(operation, vec![GeometryId::Vertex(vertex_id)]);

        Ok(GeometryId::Vertex(vertex_id))
    }

    /// Create 2D line segment
    pub fn create_line_2d(
        &mut self,
        start: Point3,
        end: Point3,
    ) -> Result<GeometryId, PrimitiveError> {
        // Create vertices
        let start_vertex =
            self.model
                .vertices
                .add_or_find(start.x, start.y, 0.0, self.tolerance.distance());
        let end_vertex =
            self.model
                .vertices
                .add_or_find(end.x, end.y, 0.0, self.tolerance.distance());

        // Create line curve
        let line = Line::new(start, end);
        let curve_id = self.model.curves.add(Box::new(line));

        // Create edge
        let edge = Edge::new(
            0, // temporary ID
            start_vertex,
            end_vertex,
            curve_id,
            EdgeOrientation::Forward,
            crate::primitives::curve::ParameterRange::new(0.0, 1.0),
        );
        let edge_id = self.model.edges.add(edge);

        // Record in timeline + forward to attached recorder.
        let operation = TimelineOperation::Create2D {
            primitive_type: "line".to_string(),
            parameters: [
                ("start_x".to_string(), start.x),
                ("start_y".to_string(), start.y),
                ("end_x".to_string(), end.x),
                ("end_y".to_string(), end.y),
            ]
            .into(),
            timestamp: self.next_timestamp(),
        };
        self.record_and_push(operation, vec![GeometryId::Edge(edge_id)]);

        Ok(GeometryId::Edge(edge_id))
    }

    /// Create 2D circle
    pub fn create_circle_2d(
        &mut self,
        center: Point3,
        radius: f64,
    ) -> Result<GeometryId, PrimitiveError> {
        if radius <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "radius".to_string(),
                value: radius.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        // Create circle curve
        let circle = Circle::new(center, Vector3::Z, radius)?;
        let curve_id = self.model.curves.add(Box::new(circle));

        // Create single vertex at arbitrary point on circle
        let point_on_circle = Point3::new(center.x + radius, center.y, center.z);
        let vertex_id = self.model.vertices.add_or_find(
            point_on_circle.x,
            point_on_circle.y,
            point_on_circle.z,
            self.tolerance.distance(),
        );

        // Create circular edge (self-closing)
        let edge = Edge::new(
            0, // temporary ID
            vertex_id,
            vertex_id, // same vertex for closed curve
            curve_id,
            EdgeOrientation::Forward,
            crate::primitives::curve::ParameterRange::new(0.0, 1.0),
        );
        let edge_id = self.model.edges.add(edge);

        // Record in timeline + forward to attached recorder.
        let operation = TimelineOperation::Create2D {
            primitive_type: "circle".to_string(),
            parameters: [
                ("center_x".to_string(), center.x),
                ("center_y".to_string(), center.y),
                ("radius".to_string(), radius),
            ]
            .into(),
            timestamp: self.next_timestamp(),
        };
        self.record_and_push(operation, vec![GeometryId::Edge(edge_id)]);

        Ok(GeometryId::Edge(edge_id))
    }

    /// Create 2D rectangle as closed face
    pub fn create_rectangle_2d(
        &mut self,
        corner: Point3,
        width: f64,
        height: f64,
    ) -> Result<GeometryId, PrimitiveError> {
        if width <= 0.0 || height <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "dimensions".to_string(),
                value: format!("{}x{}", width, height),
                constraint: "width and height must be positive".to_string(),
            });
        }

        // Create four corner vertices
        let v0 = self.model.vertices.add_or_find(
            corner.x,
            corner.y,
            corner.z,
            self.tolerance.distance(),
        );
        let v1 = self.model.vertices.add_or_find(
            corner.x + width,
            corner.y,
            corner.z,
            self.tolerance.distance(),
        );
        let v2 = self.model.vertices.add_or_find(
            corner.x + width,
            corner.y + height,
            corner.z,
            self.tolerance.distance(),
        );
        let v3 = self.model.vertices.add_or_find(
            corner.x,
            corner.y + height,
            corner.z,
            self.tolerance.distance(),
        );

        // Create four edges
        let edges = self.create_rectangle_edges(
            v0,
            v1,
            v2,
            v3,
            corner,
            Point3::new(corner.x + width, corner.y, corner.z),
            Point3::new(corner.x + width, corner.y + height, corner.z),
            Point3::new(corner.x, corner.y + height, corner.z),
        )?;

        // Create loop
        let mut loop_obj = Loop::new(0, LoopType::Outer);
        for edge_id in &edges {
            loop_obj.add_edge(*edge_id, true);
        }
        let loop_id = self.model.loops.add(loop_obj);

        // Create plane surface
        let normal = Vector3::Z; // 2D rectangle in XY plane
        let plane = Plane::from_point_normal(corner, normal).map_err(|_| {
            PrimitiveError::TopologyError {
                message: "Failed to create plane surface for rectangle".to_string(),
                euler_characteristic: None,
            }
        })?;
        let surface_id = self.model.surfaces.add(Box::new(plane));

        // Create face
        let mut face = Face::new(0, surface_id, loop_id, FaceOrientation::Forward);
        face.outer_loop = loop_id;
        let face_id = self.model.faces.add(face);

        // Record in timeline + forward to attached recorder.
        let operation = TimelineOperation::Create2D {
            primitive_type: "rectangle".to_string(),
            parameters: [
                ("corner_x".to_string(), corner.x),
                ("corner_y".to_string(), corner.y),
                ("width".to_string(), width),
                ("height".to_string(), height),
            ]
            .into(),
            timestamp: self.next_timestamp(),
        };
        self.record_and_push(operation, vec![GeometryId::Face(face_id)]);

        Ok(GeometryId::Face(face_id))
    }

    // =====================================
    // 3D PRIMITIVE CREATION METHODS
    // =====================================

    /// Create 3D box using watertight topology construction
    pub fn create_box_3d(
        &mut self,
        width: f64,
        height: f64,
        depth: f64,
    ) -> Result<GeometryId, PrimitiveError> {
        if width <= 0.0 || height <= 0.0 || depth <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "dimensions".to_string(),
                value: format!("{}x{}x{}", width, height, depth),
                constraint: "all dimensions must be positive".to_string(),
            });
        }

        let hw = width / 2.0;
        let hh = height / 2.0;
        let hd = depth / 2.0;

        // Create 8 vertices
        let vertices = self.create_box_vertices(hw, hh, hd)?;

        // Create 12 edges
        let edges = self.create_box_edges(&vertices)?;

        // Create 6 faces
        let faces = self.create_box_faces(&edges, hw, hh, hd)?;

        // Create shell
        let shell = self.create_box_shell(&faces)?;

        // Create solid
        let solid_id = self.create_box_solid(shell)?;

        // Persistent-id roots (#11).
        self.assign_primitive_pids(
            solid_id,
            crate::primitives::persistent_id::PrimitiveKind::Box,
        );

        // Record in timeline + forward to attached recorder.
        let operation = TimelineOperation::Create3D {
            primitive_type: "box".to_string(),
            parameters: [
                ("width".to_string(), width),
                ("height".to_string(), height),
                ("depth".to_string(), depth),
            ]
            .into(),
            timestamp: self.next_timestamp(),
        };
        self.record_and_push(operation, vec![GeometryId::Solid(solid_id)]);

        Ok(GeometryId::Solid(solid_id))
    }

    /// Create 3D sphere
    pub fn create_sphere_3d(
        &mut self,
        center: Point3,
        radius: f64,
    ) -> Result<GeometryId, PrimitiveError> {
        if radius <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "radius".to_string(),
                value: radius.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        // Create sphere surface
        let sphere = Sphere::new(center, radius)?;
        // Capture the surface's true parameter domain before boxing it; the
        // face must carry these bounds (azimuth ∈ [0, 2π], polar ∈ [0, π]),
        // not the `Face::new` default of [0,1]². The sphere face has a
        // degenerate (empty) boundary loop, so every UV-domain consumer —
        // `compute_bbox_and_centroid`, the broad-phase Boolean culler,
        // `contains_uv_point` — relies on `uv_bounds` being the real domain.
        // With the [0,1]² default the bbox collapses to a single octant
        // patch, and the Boolean broad phase prunes 4 of the 6 plane-sphere
        // intersections on a poke-through, silently dropping faces.
        let ((u_min, u_max), (v_min, v_max)) = sphere.parameter_bounds();
        let surface_id = self.model.surfaces.add(Box::new(sphere));

        // Sphere is a special case - single face, no edges, no vertices
        // Create degenerate loop (empty edge list for closed surface)
        let loop_obj = Loop::new(0, LoopType::Outer);
        let loop_id = self.model.loops.add(loop_obj);

        // Create face
        let mut face = Face::new(0, surface_id, loop_id, FaceOrientation::Forward);
        face.outer_loop = loop_id;
        face.set_uv_bounds(u_min, u_max, v_min, v_max);
        let face_id = self.model.faces.add(face);

        // Create shell
        let mut shell = Shell::new(0, ShellType::Closed);
        shell.add_face(face_id);
        let shell_id = self.model.shells.add(shell);

        // Create solid
        let solid = Solid::new(0, shell_id);
        let solid_id = self.model.solids.add(solid);

        // Persistent-id roots (#11).
        self.assign_primitive_pids(
            solid_id,
            crate::primitives::persistent_id::PrimitiveKind::Sphere,
        );

        // Record in timeline + forward to attached recorder.
        let operation = TimelineOperation::Create3D {
            primitive_type: "sphere".to_string(),
            parameters: [
                ("center_x".to_string(), center.x),
                ("center_y".to_string(), center.y),
                ("center_z".to_string(), center.z),
                ("radius".to_string(), radius),
            ]
            .into(),
            timestamp: self.next_timestamp(),
        };
        self.record_and_push(operation, vec![GeometryId::Solid(solid_id)]);

        Ok(GeometryId::Solid(solid_id))
    }

    /// Create 3D cylinder
    pub fn create_cylinder_3d(
        &mut self,
        base_center: Point3,
        axis: Vector3,
        radius: f64,
        height: f64,
    ) -> Result<GeometryId, PrimitiveError> {
        if radius <= 0.0 || height <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "dimensions".to_string(),
                value: format!("r={}, h={}", radius, height),
                constraint: "radius and height must be positive".to_string(),
            });
        }

        // Normalize axis
        let axis = axis
            .normalize()
            .map_err(|_| PrimitiveError::InvalidParameters {
                parameter: "axis".to_string(),
                value: format!("{:?}", axis),
                constraint: "axis must be non-zero".to_string(),
            })?;

        // Create cylinder topology
        let solid_id = self.create_cylinder_topology(base_center, axis, radius, height)?;

        // Persistent-id roots (#11).
        self.assign_primitive_pids(
            solid_id,
            crate::primitives::persistent_id::PrimitiveKind::Cylinder,
        );

        // Record in timeline + forward to attached recorder.
        let operation = TimelineOperation::Create3D {
            primitive_type: "cylinder".to_string(),
            parameters: [
                ("base_x".to_string(), base_center.x),
                ("base_y".to_string(), base_center.y),
                ("base_z".to_string(), base_center.z),
                ("axis_x".to_string(), axis.x),
                ("axis_y".to_string(), axis.y),
                ("axis_z".to_string(), axis.z),
                ("radius".to_string(), radius),
                ("height".to_string(), height),
            ]
            .into(),
            timestamp: self.next_timestamp(),
        };
        self.record_and_push(operation, vec![GeometryId::Solid(solid_id)]);

        Ok(GeometryId::Solid(solid_id))
    }

    /// Create a 3D cone primitive
    pub fn create_cone_3d(
        &mut self,
        base_center: Point3,
        axis: Vector3,
        base_radius: f64,
        top_radius: f64,
        height: f64,
    ) -> Result<GeometryId, PrimitiveError> {
        if base_radius < 0.0 || top_radius < 0.0 || height <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "dimensions".to_string(),
                value: format!("base_r={}, top_r={}, h={}", base_radius, top_radius, height),
                constraint: "radii must be non-negative and height must be positive".to_string(),
            });
        }

        if base_radius == 0.0 && top_radius == 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "radii".to_string(),
                value: "both radii are zero".to_string(),
                constraint: "at least one radius must be positive".to_string(),
            });
        }

        // Normalize axis
        let axis = axis
            .normalize()
            .map_err(|_| PrimitiveError::InvalidParameters {
                parameter: "axis".to_string(),
                value: format!("{:?}", axis),
                constraint: "axis must be non-zero".to_string(),
            })?;

        // Create cone topology using existing cone primitive
        let solid_id =
            self.create_cone_topology(base_center, axis, base_radius, top_radius, height)?;

        // Persistent-id roots (#11).
        self.assign_primitive_pids(
            solid_id,
            crate::primitives::persistent_id::PrimitiveKind::Cone,
        );

        // Record in timeline + forward to attached recorder.
        let operation = TimelineOperation::Create3D {
            primitive_type: "cone".to_string(),
            parameters: [
                ("base_x".to_string(), base_center.x),
                ("base_y".to_string(), base_center.y),
                ("base_z".to_string(), base_center.z),
                ("axis_x".to_string(), axis.x),
                ("axis_y".to_string(), axis.y),
                ("axis_z".to_string(), axis.z),
                ("base_radius".to_string(), base_radius),
                ("top_radius".to_string(), top_radius),
                ("height".to_string(), height),
            ]
            .into(),
            timestamp: self.next_timestamp(),
        };
        self.record_and_push(operation, vec![GeometryId::Solid(solid_id)]);

        Ok(GeometryId::Solid(solid_id))
    }

    /// Create 3D torus
    ///
    /// Delegates topology construction to
    /// [`crate::primitives::torus_primitive::TorusPrimitive::create`] and
    /// records the operation on the timeline. The axis is normalised by
    /// `TorusParameters::new`; pass any non-zero direction.
    pub fn create_torus_3d(
        &mut self,
        center: Point3,
        axis: Vector3,
        major_radius: f64,
        minor_radius: f64,
    ) -> Result<GeometryId, PrimitiveError> {
        // Build & validate parameters (also normalises the axis and
        // rejects degenerate radii / self-intersecting tori).
        let params = crate::primitives::torus_primitive::TorusParameters::new(
            center,
            axis,
            major_radius,
            minor_radius,
        )?;

        let solid_id =
            crate::primitives::torus_primitive::TorusPrimitive::create(&params, self.model)?;

        // Persistent-id roots (#11).
        self.assign_primitive_pids(
            solid_id,
            crate::primitives::persistent_id::PrimitiveKind::Torus,
        );

        // Record in timeline + forward to attached recorder.
        let operation = TimelineOperation::Create3D {
            primitive_type: "torus".to_string(),
            parameters: [
                ("center_x".to_string(), center.x),
                ("center_y".to_string(), center.y),
                ("center_z".to_string(), center.z),
                ("axis_x".to_string(), params.axis.x),
                ("axis_y".to_string(), params.axis.y),
                ("axis_z".to_string(), params.axis.z),
                ("major_radius".to_string(), major_radius),
                ("minor_radius".to_string(), minor_radius),
            ]
            .into(),
            timestamp: self.next_timestamp(),
        };
        self.record_and_push(operation, vec![GeometryId::Solid(solid_id)]);

        Ok(GeometryId::Solid(solid_id))
    }

    // ─────────────────────────────────────────────────────────────────
    // Datum-anchored primitive creation (Slice 2)
    //
    // Each `create_*_anchored` runs the existing world-origin creator
    // and then composes the datum's reference frame with the caller's
    // local transform to position the freshly-created solid. The anchor
    // metadata is stamped on the solid so downstream consumers (REST,
    // model tree, LLM-readable surfaces) can answer "what was this
    // primitive placed against?" without re-deriving it from vertex
    // coordinates.
    // ─────────────────────────────────────────────────────────────────

    /// Apply datum-anchoring to an already-created solid: composes the
    /// datum's local-to-world frame with the caller's local transform,
    /// transforms the solid's geometry into world space, and stamps
    /// `SolidAnchor` metadata so the placement can be queried later.
    ///
    /// Identity world transforms are skipped — anchoring to the world
    /// Origin with identity local transform is a no-op on geometry but
    /// still records the anchor metadata.
    pub fn anchor_solid(
        &mut self,
        solid_id: SolidId,
        datum_id: u32,
        local_transform: Matrix4,
    ) -> Result<(), PrimitiveError> {
        let frame =
            self.model
                .datums
                .frame(datum_id)
                .ok_or_else(|| PrimitiveError::InvalidParameters {
                    parameter: "datum_id".to_string(),
                    value: datum_id.to_string(),
                    constraint: "must reference an existing datum".to_string(),
                })?;
        let world_transform = frame * local_transform;

        if !is_approx_identity(&world_transform, 1e-12) {
            crate::operations::transform::transform_solid(
                self.model,
                solid_id,
                world_transform,
                crate::operations::transform::TransformOptions::default(),
            )
            .map_err(|e| PrimitiveError::GeometryError {
                operation: "anchor_solid".to_string(),
                details: format!("{:?}", e),
            })?;
        }

        let solid = self.model.solids.get_mut(solid_id).ok_or_else(|| {
            PrimitiveError::InvalidParameters {
                parameter: "solid_id".to_string(),
                value: solid_id.to_string(),
                constraint: "must reference an existing solid".to_string(),
            }
        })?;
        let prev_datum = solid.anchor.datum_id;
        solid.anchor = crate::primitives::solid::SolidAnchor {
            datum_id,
            local_transform,
        };
        // Slice 5: keep the propagation graph in sync. Drop the old
        // anchor edge (if any) and register the new one so future
        // datum moves invalidate this solid's cached descriptor.
        if prev_datum != datum_id {
            self.model
                .datum_graph
                .unregister_solid_anchor(solid_id, prev_datum);
        }
        self.model
            .datum_graph
            .register_solid_anchor(solid_id, datum_id);
        // Anchor reassignment alters the descriptor's
        // `anchor_datum_name` and `center_in_anchor_frame`, plus the
        // geometry was just transformed — flush the cache.
        self.model.location_cache.invalidate(solid_id);

        Ok(())
    }

    /// Create a 3D box anchored to a datum.
    ///
    /// `local_transform` is applied on top of the datum's frame.
    /// `Matrix4::identity()` places the box centred on the datum's
    /// origin with axes aligned to the datum's frame.
    pub fn create_box_3d_anchored(
        &mut self,
        width: f64,
        height: f64,
        depth: f64,
        datum_id: u32,
        local_transform: Matrix4,
    ) -> Result<GeometryId, PrimitiveError> {
        let geo = self.create_box_3d(width, height, depth)?;
        if let GeometryId::Solid(sid) = geo {
            self.anchor_solid(sid, datum_id, local_transform)?;
        }
        Ok(geo)
    }

    /// Create a 3D sphere anchored to a datum.
    pub fn create_sphere_3d_anchored(
        &mut self,
        radius: f64,
        datum_id: u32,
        local_transform: Matrix4,
    ) -> Result<GeometryId, PrimitiveError> {
        let geo = self.create_sphere_3d(Point3::ORIGIN, radius)?;
        if let GeometryId::Solid(sid) = geo {
            self.anchor_solid(sid, datum_id, local_transform)?;
        }
        Ok(geo)
    }

    /// Create a 3D cylinder anchored to a datum. The cylinder axis is
    /// the datum frame's local +Z; the caller's `local_transform` can
    /// re-orient or offset it further.
    pub fn create_cylinder_3d_anchored(
        &mut self,
        radius: f64,
        height: f64,
        datum_id: u32,
        local_transform: Matrix4,
    ) -> Result<GeometryId, PrimitiveError> {
        let geo = self.create_cylinder_3d(Point3::ORIGIN, Vector3::Z, radius, height)?;
        if let GeometryId::Solid(sid) = geo {
            self.anchor_solid(sid, datum_id, local_transform)?;
        }
        Ok(geo)
    }

    /// Create a 3D cone (frustum) anchored to a datum. Axis is the
    /// datum frame's local +Z.
    pub fn create_cone_3d_anchored(
        &mut self,
        base_radius: f64,
        top_radius: f64,
        height: f64,
        datum_id: u32,
        local_transform: Matrix4,
    ) -> Result<GeometryId, PrimitiveError> {
        let geo =
            self.create_cone_3d(Point3::ORIGIN, Vector3::Z, base_radius, top_radius, height)?;
        if let GeometryId::Solid(sid) = geo {
            self.anchor_solid(sid, datum_id, local_transform)?;
        }
        Ok(geo)
    }

    /// Create a 3D torus anchored to a datum. Axis is the datum
    /// frame's local +Z.
    pub fn create_torus_3d_anchored(
        &mut self,
        major_radius: f64,
        minor_radius: f64,
        datum_id: u32,
        local_transform: Matrix4,
    ) -> Result<GeometryId, PrimitiveError> {
        let geo = self.create_torus_3d(Point3::ORIGIN, Vector3::Z, major_radius, minor_radius)?;
        if let GeometryId::Solid(sid) = geo {
            self.anchor_solid(sid, datum_id, local_transform)?;
        }
        Ok(geo)
    }

    /// Create a plane primitive as a thin box
    pub fn plane_primitive(
        &mut self,
        origin: Point3,
        normal: Vector3,
        u_dir: Vector3,
        width: f64,
        height: f64,
        thickness: f64,
    ) -> BuilderResult<SolidId> {
        if width <= 0.0 || height <= 0.0 || thickness <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "dimensions".to_string(),
                value: format!("{}x{}x{}", width, height, thickness),
                constraint: "all dimensions must be positive".to_string(),
            });
        }

        // Normalize vectors
        let normal = normal
            .normalize()
            .map_err(|_| PrimitiveError::InvalidParameters {
                parameter: "normal".to_string(),
                value: format!("{:?}", normal),
                constraint: "must be non-zero".to_string(),
            })?;
        let u_dir = u_dir
            .normalize()
            .map_err(|_| PrimitiveError::InvalidParameters {
                parameter: "u_dir".to_string(),
                value: format!("{:?}", u_dir),
                constraint: "must be non-zero".to_string(),
            })?;

        // Ensure u_dir is perpendicular to normal
        let u_perp = u_dir - normal * u_dir.dot(&normal);
        let u_dir = u_perp
            .normalize()
            .map_err(|_| PrimitiveError::InvalidParameters {
                parameter: "u_dir".to_string(),
                value: format!("{:?}", u_dir),
                constraint: "must not be parallel to normal".to_string(),
            })?;

        // Calculate v direction
        let v_dir = normal.cross(&u_dir);

        // Create a thin box aligned with the plane
        let hw = width / 2.0;
        let hh = height / 2.0;
        let ht = thickness / 2.0;

        // Use existing box creation but with custom orientation
        // This will create box vertices in world coordinates directly
        let center = origin;

        // Calculate the 8 vertices of the oriented box
        let mut vertices = [0u32; 8];
        for i in 0..8 {
            let local_x = if i & 1 == 0 { -hw } else { hw };
            let local_y = if i & 2 == 0 { -hh } else { hh };
            let local_z = if i & 4 == 0 { -ht } else { ht };

            let world_pt = center + u_dir * local_x + v_dir * local_y + normal * local_z;
            vertices[i] = self.model.vertices.add_or_find(
                world_pt.x,
                world_pt.y,
                world_pt.z,
                self.tolerance.distance(),
            );
        }

        // Create edges
        let edges = self.create_box_edges(&vertices)?;

        // Create faces
        let faces = self.create_box_faces(&edges, hw, hh, ht)?;

        // Create shell
        let shell = self.create_box_shell(&faces)?;

        // Create solid
        let solid_id = self.create_box_solid(shell)?;

        // Record in timeline + forward to attached recorder.
        let operation = TimelineOperation::Create3D {
            primitive_type: "plane".to_string(),
            parameters: [
                ("origin_x".to_string(), origin.x),
                ("origin_y".to_string(), origin.y),
                ("origin_z".to_string(), origin.z),
                ("normal_x".to_string(), normal.x),
                ("normal_y".to_string(), normal.y),
                ("normal_z".to_string(), normal.z),
                ("width".to_string(), width),
                ("height".to_string(), height),
                ("thickness".to_string(), thickness),
            ]
            .into(),
            timestamp: self.next_timestamp(),
        };
        self.record_and_push(operation, vec![GeometryId::Solid(solid_id)]);

        Ok(solid_id)
    }

    // =====================================
    // TOPOLOGY CONSTRUCTION HELPERS
    // =====================================

    /// Create vertices for box
    fn create_box_vertices(
        &mut self,
        hw: f64,
        hh: f64,
        hd: f64,
    ) -> Result<[VertexId; 8], PrimitiveError> {
        let vertex_positions = [
            (-hw, -hh, -hd), // v0: bottom-front-left
            (hw, -hh, -hd),  // v1: bottom-front-right
            (hw, hh, -hd),   // v2: bottom-back-right
            (-hw, hh, -hd),  // v3: bottom-back-left
            (-hw, -hh, hd),  // v4: top-front-left
            (hw, -hh, hd),   // v5: top-front-right
            (hw, hh, hd),    // v6: top-back-right
            (-hw, hh, hd),   // v7: top-back-left
        ];

        let mut vertices = [0u32; 8];
        for (i, &(x, y, z)) in vertex_positions.iter().enumerate() {
            vertices[i] = self
                .model
                .vertices
                .add_or_find(x, y, z, self.tolerance.distance());
        }

        Ok(vertices)
    }

    /// Create edges for box
    fn create_box_edges(
        &mut self,
        vertices: &[VertexId; 8],
    ) -> Result<[EdgeId; 12], PrimitiveError> {
        let edge_vertex_pairs = [
            // Bottom face edges (0-3)
            (0, 1),
            (1, 2),
            (2, 3),
            (3, 0),
            // Top face edges (4-7)
            (4, 5),
            (5, 6),
            (6, 7),
            (7, 4),
            // Vertical edges (8-11)
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
        ];

        let mut edges = [0u32; 12];
        for (i, &(start_idx, end_idx)) in edge_vertex_pairs.iter().enumerate() {
            let start_vertex = vertices[start_idx];
            let end_vertex = vertices[end_idx];

            // Get vertex positions
            let start_pos = self
                .model
                .vertices
                .get_position(start_vertex)
                .ok_or_else(|| PrimitiveError::TopologyError {
                    message: format!("Start vertex {:?} not found", start_vertex),
                    euler_characteristic: None,
                })?;
            let end_pos = self
                .model
                .vertices
                .get_position(end_vertex)
                .ok_or_else(|| PrimitiveError::TopologyError {
                    message: format!("End vertex {:?} not found", end_vertex),
                    euler_characteristic: None,
                })?;

            // Create line curve
            let line = Line::new(
                Point3::new(start_pos[0], start_pos[1], start_pos[2]),
                Point3::new(end_pos[0], end_pos[1], end_pos[2]),
            );
            let curve_id = self.model.curves.add(Box::new(line));

            // Create edge
            let edge = Edge::new(
                0, // temporary ID
                start_vertex,
                end_vertex,
                curve_id,
                EdgeOrientation::Forward,
                crate::primitives::curve::ParameterRange::new(0.0, 1.0),
            );
            edges[i] = self.model.edges.add(edge);
        }

        Ok(edges)
    }

    /// Create rectangle edges helper
    fn create_rectangle_edges(
        &mut self,
        v0: VertexId,
        v1: VertexId,
        v2: VertexId,
        v3: VertexId,
        p0: Point3,
        p1: Point3,
        p2: Point3,
        p3: Point3,
    ) -> Result<[EdgeId; 4], PrimitiveError> {
        let edge_data = [
            (v0, v1, p0, p1), // bottom
            (v1, v2, p1, p2), // right
            (v2, v3, p2, p3), // top
            (v3, v0, p3, p0), // left
        ];

        let mut edges = [0u32; 4];
        for (i, &(start_v, end_v, start_p, end_p)) in edge_data.iter().enumerate() {
            let line = Line::new(start_p, end_p);
            let curve_id = self.model.curves.add(Box::new(line));

            let edge = Edge::new(
                0,
                start_v,
                end_v,
                curve_id,
                EdgeOrientation::Forward,
                crate::primitives::curve::ParameterRange::new(0.0, 1.0),
            );
            edges[i] = self.model.edges.add(edge);
        }

        Ok(edges)
    }

    /// Create faces for box
    fn create_box_faces(
        &mut self,
        edges: &[EdgeId; 12],
        hw: f64,
        hh: f64,
        hd: f64,
    ) -> Result<[FaceId; 6], PrimitiveError> {
        // Face topology: edges and per-edge orientations chosen so that the
        // outer-loop vertex traversal is CCW when viewed from outside the
        // solid, i.e. the right-hand-rule normal of the loop matches the
        // outward face normal stored on the surface.
        //
        // Vertex layout (set in `create_box_vertices`):
        //   v0=(-,-,-) v1=(+,-,-) v2=(+,+,-) v3=(-,+,-)   bottom (z=-hd)
        //   v4=(-,-,+) v5=(+,-,+) v6=(+,+,+) v7=(-,+,+)   top    (z=+hd)
        //
        // Edge layout (set in `create_box_edges`, all stored start→end):
        //   e0:v0→v1  e1:v1→v2  e2:v2→v3  e3:v3→v0  (bottom)
        //   e4:v4→v5  e5:v5→v6  e6:v6→v7  e7:v7→v4  (top)
        //   e8:v0→v4  e9:v1→v5  e10:v2→v6 e11:v3→v7 (vertical)
        //
        // `Loop::vertices_cached` derives vertex i as edge.start if
        // orientations[i] is true, else edge.end. The arrays below were
        // chosen so that the resulting vertex chain is a continuous,
        // non-degenerate quad whose right-hand normal matches the face
        // surface normal.
        let face_edge_data = [
            // Bottom (z=-hd, outward -Z): traversal v0→v3→v2→v1→v0
            //   v0→v3 = e3 reversed (e3:v3→v0)
            //   v3→v2 = e2 reversed (e2:v2→v3)
            //   v2→v1 = e1 reversed (e1:v1→v2)
            //   v1→v0 = e0 reversed (e0:v0→v1)
            (
                [3, 2, 1, 0],
                [false, false, false, false],
                Point3::new(0.0, 0.0, -hd),
                Vector3::new(0.0, 0.0, -1.0),
            ),
            // Top (z=+hd, outward +Z): traversal v4→v5→v6→v7→v4
            (
                [4, 5, 6, 7],
                [true, true, true, true],
                Point3::new(0.0, 0.0, hd),
                Vector3::new(0.0, 0.0, 1.0),
            ),
            // Front (y=-hh, outward -Y): traversal v0→v1→v5→v4→v0
            //   vertices come out as [e0.start, e9.start, e4.end, e8.end]
            //   = [v0, v1, v5, v4] — Newell normal in (x,z) = -Y. ✓
            (
                [0, 9, 4, 8],
                [true, true, false, false],
                Point3::new(0.0, -hh, 0.0),
                Vector3::new(0.0, -1.0, 0.0),
            ),
            // Back (y=+hh, outward +Y): traversal v2→v3→v7→v6→v2
            //   v2→v3 = e2 forward, v3→v7 = e11 forward,
            //   v7→v6 = e6 reversed, v6→v2 = e10 reversed
            (
                [2, 11, 6, 10],
                [true, true, false, false],
                Point3::new(0.0, hh, 0.0),
                Vector3::new(0.0, 1.0, 0.0),
            ),
            // Left (x=-hw, outward -X): traversal v0→v4→v7→v3→v0
            //   v0→v4 = e8 forward, v4→v7 = e7 reversed (e7:v7→v4),
            //   v7→v3 = e11 reversed (e11:v3→v7), v3→v0 = e3 forward
            (
                [8, 7, 11, 3],
                [true, false, false, true],
                Point3::new(-hw, 0.0, 0.0),
                Vector3::new(-1.0, 0.0, 0.0),
            ),
            // Right (x=+hw, outward +X): traversal v1→v2→v6→v5→v1
            //   v1→v2 = e1 forward, v2→v6 = e10 forward,
            //   v6→v5 = e5 reversed, v5→v1 = e9 reversed
            (
                [1, 10, 5, 9],
                [true, true, false, false],
                Point3::new(hw, 0.0, 0.0),
                Vector3::new(1.0, 0.0, 0.0),
            ),
        ];

        let mut faces = [0u32; 6];
        for (face_idx, &(edge_indices, orientations, point, normal)) in
            face_edge_data.iter().enumerate()
        {
            // Create plane surface
            let plane = Plane::from_point_normal(point, normal).map_err(|_| {
                PrimitiveError::TopologyError {
                    message: format!("Failed to create plane surface for face {}", face_idx),
                    euler_characteristic: None,
                }
            })?;
            let surface_id = self.model.surfaces.add(Box::new(plane));

            // Create loop
            let mut loop_obj = Loop::new(0, LoopType::Outer);
            for (i, &edge_idx) in edge_indices.iter().enumerate() {
                loop_obj.add_edge(edges[edge_idx], orientations[i]);
            }
            let loop_id = self.model.loops.add(loop_obj);

            // Create face
            let mut face = Face::new(0, surface_id, loop_id, FaceOrientation::Forward);
            face.outer_loop = loop_id;
            faces[face_idx] = self.model.faces.add(face);
        }

        Ok(faces)
    }

    /// Create shell for box
    fn create_box_shell(&mut self, faces: &[FaceId; 6]) -> Result<ShellId, PrimitiveError> {
        let mut shell = Shell::new(0, ShellType::Closed);
        for &face_id in faces {
            shell.add_face(face_id);
        }
        Ok(self.model.shells.add(shell))
    }

    /// Create solid for box
    fn create_box_solid(&mut self, shell_id: ShellId) -> Result<SolidId, PrimitiveError> {
        let solid = Solid::new(0, shell_id);
        Ok(self.model.solids.add(solid))
    }

    /// Create a watertight B-Rep cylinder solid.
    ///
    /// Topology produced:
    /// - 2 vertices on the seam (one on each circular cap, at the
    ///   `ref_dir = axis.perpendicular()` reference direction).
    /// - 3 edges: a closed circle on the bottom cap, a closed circle on
    ///   the top cap, and a linear seam connecting the two seam vertices.
    /// - 3 faces:
    ///   - Bottom cap: planar surface with normal `-axis`. Outer loop
    ///     traverses the bottom circle in the orientation that yields
    ///     a CCW boundary when viewed from outside (along `-axis`),
    ///     i.e. `Backward` relative to the underlying parametric circle.
    ///   - Top cap: planar surface with normal `+axis`. Outer loop
    ///     traverses the top circle `Forward`.
    ///   - Lateral cylindrical face: outer loop is the canonical
    ///     seamed rectangle in (u, v) parameter space — bottom-circle
    ///     forward, seam forward, top-circle backward, seam backward.
    /// - 1 closed shell containing all three faces.
    ///
    /// References: Mäntylä §4 (B-Rep solid modelling), Stroud §3
    /// (seamed surfaces), Hoffmann §5 (analytical primitives).
    fn create_cylinder_topology(
        &mut self,
        base_center: Point3,
        axis: Vector3,
        radius: f64,
        height: f64,
    ) -> Result<SolidId, PrimitiveError> {
        let topology_err = |msg: String| PrimitiveError::TopologyError {
            message: msg,
            euler_characteristic: None,
        };

        let top_center = base_center + axis * height;

        // ---- curves: two circles + one line. ----
        let bottom_circle = Circle::new(base_center, axis, radius)
            .map_err(|e| topology_err(format!("bottom circle: {e}")))?;
        let top_circle = Circle::new(top_center, axis, radius)
            .map_err(|e| topology_err(format!("top circle: {e}")))?;

        // The seam MUST coincide with the circles' parametric origin
        // (their `t = 0`), or the lateral face's u-parameter sweeps a full
        // 2π that *straddles* the seam — and the curved-CDT path then
        // projects a non-simple polygon and fails with
        // `CdtFailed(PointOnFixedEdge)` (CDT-γ.3). `Circle`/`Arc` choose a
        // canonical `x_axis` (e.g. `+X` for a `+Z` normal) that is NOT
        // `axis.perpendicular()` in general, so anchor the seam to the
        // circle's actual `x_axis` rather than to `axis.perpendicular()`.
        let ref_dir = bottom_circle.x_axis();

        // ---- vertices: one seam vertex per cap, on the circles' t=0. ----
        let v_bottom = self.model.vertices.add_or_find(
            base_center.x + ref_dir.x * radius,
            base_center.y + ref_dir.y * radius,
            base_center.z + ref_dir.z * radius,
            self.tolerance.distance(),
        );
        let v_top = self.model.vertices.add_or_find(
            top_center.x + ref_dir.x * radius,
            top_center.y + ref_dir.y * radius,
            top_center.z + ref_dir.z * radius,
            self.tolerance.distance(),
        );

        let seam_line = Line::new(
            base_center + ref_dir * radius,
            top_center + ref_dir * radius,
        );
        let bottom_circle_id = self.model.curves.add(Box::new(bottom_circle));
        let top_circle_id = self.model.curves.add(Box::new(top_circle));
        let seam_line_id = self.model.curves.add(Box::new(seam_line));

        // ---- edges: closed circles + linear seam. ----
        // Closed circle edges: start_vertex == end_vertex (the seam vertex).
        // The underlying `Circle` curve uses the `Arc` parameterization
        // with `range = ParameterRange::unit()` (i.e. t ∈ [0, 1]) and
        // `angle_at(t) = sweep_angle · t`, where `Arc::evaluate(t)`
        // *clamps* `t` to `[0, 1]`. So the edge's parameter sub-range
        // must match: full circle ⇒ `[0, 1]`, NOT `[0, 2π]`. Using
        // `[0, 2π]` here causes any tessellator that samples at
        // `t = j · 2π / N` to clamp every `t > 1` (i.e. every `j ≥ ⌈N/(2π)⌉`)
        // to `t = 1` → angle 2π → all collapsed to the seam vertex.
        // For r=5, default params (N=100), only the first ~16 samples
        // are unique; the remaining 84 pile up at +ref_dir·radius,
        // producing a 16-gon-shaped cap and visible cracks where the
        // cap boundary fails to meet the lateral cylinder boundary.
        let bottom_edge = self.model.edges.add(Edge::new(
            0,
            v_bottom,
            v_bottom,
            bottom_circle_id,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        ));
        let top_edge = self.model.edges.add(Edge::new(
            0,
            v_top,
            v_top,
            top_circle_id,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        ));
        let seam_edge = self.model.edges.add(Edge::new(
            0,
            v_bottom,
            v_top,
            seam_line_id,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        ));

        // ---- surfaces: 2 planes + 1 finite cylinder. ----
        let bottom_plane = Plane::from_point_normal(base_center, -axis)
            .map_err(|e| topology_err(format!("bottom plane: {e}")))?;
        let top_plane = Plane::from_point_normal(top_center, axis)
            .map_err(|e| topology_err(format!("top plane: {e}")))?;
        let mut lateral_cyl = Cylinder::new_finite(base_center, axis, radius, height)
            .map_err(|e| topology_err(format!("lateral cylinder: {e}")))?;
        // INVARIANT: the cylinder surface's parametric origin (`ref_dir`,
        // i.e. u=0) must coincide with the topological seam vertex. The seam
        // sits at `bottom_circle.x_axis()` (see `ref_dir` above), but
        // `Cylinder::new_finite` defaults `ref_dir` to `axis.perpendicular()`,
        // which is π/2 away for a canonical-axis circle. Consumers that read
        // `cylinder.ref_dir` to locate the seam — notably
        // `operations::fillet::cylinder_rim_fillet`, which places the blend's
        // new seam vertices off this direction — otherwise compute positions
        // π/2 out of phase with the real seam vertex and tear the shell.
        // Realign the surface frame to the seam.
        lateral_cyl.ref_dir = ref_dir;
        let bottom_surface_id = self.model.surfaces.add(Box::new(bottom_plane));
        let top_surface_id = self.model.surfaces.add(Box::new(top_plane));
        let lateral_surface_id = self.model.surfaces.add(Box::new(lateral_cyl));

        // ---- loops. ----
        // Bottom cap: outward normal is `-axis`. The Circle is
        // parameterized CCW when viewed from `+axis`. Looking from
        // `-axis` (outside the bottom cap), that traversal appears CW,
        // so we walk the edge `Backward` to get an outward-CCW loop.
        let mut bottom_loop = Loop::new(0, LoopType::Outer);
        bottom_loop.add_edge(bottom_edge, false);
        let bottom_loop_id = self.model.loops.add(bottom_loop);

        // Top cap: outward normal is `+axis`, same orientation as the
        // Circle's parametric CCW direction → walk `Forward`.
        let mut top_loop = Loop::new(0, LoopType::Outer);
        top_loop.add_edge(top_edge, true);
        let top_loop_id = self.model.loops.add(top_loop);

        // Lateral seamed face: in (u, v) parameter space the outer loop
        // is a CCW rectangle with corners at (0, 0), (2π, 0), (2π, h),
        // (0, h). The seam is the degenerate segment u=0 ≡ u=2π
        // traversed twice (once forward, once backward) to close the
        // rectangle. Edge sequence:
        //   (0,0)→(2π,0): bottom_circle forward
        //   (2π,0)→(2π,h): seam forward
        //   (2π,h)→(0,h): top_circle backward
        //   (0,h)→(0,0): seam backward
        let mut lateral_loop = Loop::new(0, LoopType::Outer);
        lateral_loop.add_edge(bottom_edge, true);
        lateral_loop.add_edge(seam_edge, true);
        lateral_loop.add_edge(top_edge, false);
        lateral_loop.add_edge(seam_edge, false);
        let lateral_loop_id = self.model.loops.add(lateral_loop);

        // ---- faces. ----
        let mut bottom_face = Face::new(
            0,
            bottom_surface_id,
            bottom_loop_id,
            FaceOrientation::Forward,
        );
        bottom_face.outer_loop = bottom_loop_id;
        let bottom_face_id = self.model.faces.add(bottom_face);

        let mut top_face = Face::new(0, top_surface_id, top_loop_id, FaceOrientation::Forward);
        top_face.outer_loop = top_loop_id;
        let top_face_id = self.model.faces.add(top_face);

        let mut lateral_face = Face::new(
            0,
            lateral_surface_id,
            lateral_loop_id,
            FaceOrientation::Forward,
        );
        lateral_face.outer_loop = lateral_loop_id;
        let lateral_face_id = self.model.faces.add(lateral_face);

        // ---- shell + solid. ----
        let mut shell = Shell::new(0, ShellType::Closed);
        shell.add_face(bottom_face_id);
        shell.add_face(top_face_id);
        shell.add_face(lateral_face_id);
        let shell_id = self.model.shells.add(shell);

        let solid = Solid::new(0, shell_id);
        Ok(self.model.solids.add(solid))
    }

    /// Create a true (seamed) frustum B-Rep: a truncated cone with two
    /// distinct circular ends. Mirrors [`Self::create_cylinder_topology`]
    /// exactly — shared circle edges + a seam line → a single rectangular
    /// lateral loop, the proven watertight structure — but with two radii
    /// and a `Cone` lateral surface, so the lateral tessellates through the
    /// same curved-CDT path as the cylinder. Replaces the prior lossy
    /// "approximate the frustum as an apex cone" path, which left the
    /// truncation cap disconnected from the lateral (non-watertight).
    ///
    /// Precondition (enforced by the caller): `base_radius` and `top_radius`
    /// are both positive and meaningfully different (a near-equal pair is
    /// routed to `create_cylinder_topology`).
    fn create_frustum_topology(
        &mut self,
        base_center: Point3,
        axis: Vector3,
        base_radius: f64,
        top_radius: f64,
        height: f64,
    ) -> Result<SolidId, PrimitiveError> {
        let topology_err = |msg: String| PrimitiveError::TopologyError {
            message: msg,
            euler_characteristic: None,
        };

        let top_center = base_center + axis * height;

        // Lateral cone geometry: apex + half-angle from the two radii. The
        // cone axis points apex → base (toward increasing radius): a cone
        // narrowing along +axis has its apex beyond the top (cone axis =
        // -axis); a widening one has its apex below the base (cone axis =
        // +axis).
        let tan_half = (base_radius - top_radius).abs() / height;
        let half_angle = tan_half.atan();
        let (apex, cone_axis) = if base_radius > top_radius {
            (base_center + axis * (base_radius / tan_half), -axis)
        } else {
            (base_center - axis * (base_radius / tan_half), axis)
        };

        // ---- curves: two circles + one seam line. ----
        let bottom_circle = Circle::new(base_center, axis, base_radius)
            .map_err(|e| topology_err(format!("bottom circle: {e}")))?;
        let top_circle = Circle::new(top_center, axis, top_radius)
            .map_err(|e| topology_err(format!("top circle: {e}")))?;

        // Seam at the circles' parametric origin (t = 0), same as the
        // cylinder. Both circles share `axis`, hence the same canonical
        // `x_axis`, so a single `ref_dir` anchors both seam vertices.
        let ref_dir = bottom_circle.x_axis();

        let v_bottom = self.model.vertices.add_or_find(
            base_center.x + ref_dir.x * base_radius,
            base_center.y + ref_dir.y * base_radius,
            base_center.z + ref_dir.z * base_radius,
            self.tolerance.distance(),
        );
        let v_top = self.model.vertices.add_or_find(
            top_center.x + ref_dir.x * top_radius,
            top_center.y + ref_dir.y * top_radius,
            top_center.z + ref_dir.z * top_radius,
            self.tolerance.distance(),
        );

        let seam_line = Line::new(
            base_center + ref_dir * base_radius,
            top_center + ref_dir * top_radius,
        );
        let bottom_circle_id = self.model.curves.add(Box::new(bottom_circle));
        let top_circle_id = self.model.curves.add(Box::new(top_circle));
        let seam_line_id = self.model.curves.add(Box::new(seam_line));

        // ---- edges: closed circles (shared with caps) + linear seam. ----
        let bottom_edge = self.model.edges.add(Edge::new(
            0,
            v_bottom,
            v_bottom,
            bottom_circle_id,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        ));
        let top_edge = self.model.edges.add(Edge::new(
            0,
            v_top,
            v_top,
            top_circle_id,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        ));
        let seam_edge = self.model.edges.add(Edge::new(
            0,
            v_bottom,
            v_top,
            seam_line_id,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        ));

        // ---- surfaces: 2 planes + 1 cone. ----
        let bottom_plane = Plane::from_point_normal(base_center, -axis)
            .map_err(|e| topology_err(format!("bottom plane: {e}")))?;
        let top_plane = Plane::from_point_normal(top_center, axis)
            .map_err(|e| topology_err(format!("top plane: {e}")))?;
        let v_base_d = (base_center - apex).dot(&cone_axis);
        let v_top_d = (top_center - apex).dot(&cone_axis);
        let lateral_cone = crate::primitives::surface::Cone {
            apex,
            axis: cone_axis,
            half_angle,
            ref_dir: cone_axis.perpendicular(),
            height_limits: Some([v_base_d.min(v_top_d), v_base_d.max(v_top_d)]),
            angle_limits: None,
        };
        let bottom_surface_id = self.model.surfaces.add(Box::new(bottom_plane));
        let top_surface_id = self.model.surfaces.add(Box::new(top_plane));
        let lateral_surface_id = self.model.surfaces.add(Box::new(lateral_cone));

        // Orient the lateral so its outward normal points radially away
        // from the axis (replicates `orient_cone_lateral_outward`): compare
        // the surface normal at the parametric midpoint against the radial
        // direction there; `Face::normal_at` applies `orientation.sign()`.
        let lateral_orientation = {
            let surface = self
                .model
                .surfaces
                .get(lateral_surface_id)
                .ok_or_else(|| topology_err("lateral cone surface missing".into()))?;
            let ((u_min, u_max), (v_min, v_max)) = surface.parameter_bounds();
            let (u_mid, v_mid) = (0.5 * (u_min + u_max), 0.5 * (v_min + v_max));
            match (
                surface.point_at(u_mid, v_mid),
                surface.normal_at(u_mid, v_mid),
            ) {
                (Ok(mid), Ok(n)) => {
                    let from_apex = mid - apex;
                    let radial = from_apex - cone_axis * from_apex.dot(&cone_axis);
                    if n.dot(&radial) >= 0.0 {
                        FaceOrientation::Forward
                    } else {
                        FaceOrientation::Backward
                    }
                }
                _ => FaceOrientation::Forward,
            }
        };

        // ---- loops (identical structure to the cylinder). ----
        let mut bottom_loop = Loop::new(0, LoopType::Outer);
        bottom_loop.add_edge(bottom_edge, false);
        let bottom_loop_id = self.model.loops.add(bottom_loop);

        let mut top_loop = Loop::new(0, LoopType::Outer);
        top_loop.add_edge(top_edge, true);
        let top_loop_id = self.model.loops.add(top_loop);

        // Lateral seamed face: bottom_circle(fwd) → seam(fwd) →
        // top_circle(back) → seam(back), a CCW rectangle in (u, v).
        let mut lateral_loop = Loop::new(0, LoopType::Outer);
        lateral_loop.add_edge(bottom_edge, true);
        lateral_loop.add_edge(seam_edge, true);
        lateral_loop.add_edge(top_edge, false);
        lateral_loop.add_edge(seam_edge, false);
        let lateral_loop_id = self.model.loops.add(lateral_loop);

        // ---- faces. ----
        let mut bottom_face = Face::new(
            0,
            bottom_surface_id,
            bottom_loop_id,
            FaceOrientation::Forward,
        );
        bottom_face.outer_loop = bottom_loop_id;
        let bottom_face_id = self.model.faces.add(bottom_face);

        let mut top_face = Face::new(0, top_surface_id, top_loop_id, FaceOrientation::Forward);
        top_face.outer_loop = top_loop_id;
        let top_face_id = self.model.faces.add(top_face);

        let mut lateral_face =
            Face::new(0, lateral_surface_id, lateral_loop_id, lateral_orientation);
        lateral_face.outer_loop = lateral_loop_id;
        let lateral_face_id = self.model.faces.add(lateral_face);

        // ---- shell + solid. ----
        let mut shell = Shell::new(0, ShellType::Closed);
        shell.add_face(bottom_face_id);
        shell.add_face(top_face_id);
        shell.add_face(lateral_face_id);
        let shell_id = self.model.shells.add(shell);

        let solid = Solid::new(0, shell_id);
        Ok(self.model.solids.add(solid))
    }

    /// Create cone topology using the full cone primitive implementation
    fn create_cone_topology(
        &mut self,
        base_center: Point3,
        axis: Vector3,
        base_radius: f64,
        top_radius: f64,
        height: f64,
    ) -> Result<SolidId, PrimitiveError> {
        use crate::primitives::cone_primitive::ConeParameters;

        // Convert from base/top radius representation to apex/half-angle representation
        if base_radius == 0.0 && top_radius == 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "radii".to_string(),
                value: "both zero".to_string(),
                constraint: "at least one radius must be positive".to_string(),
            });
        }

        // Calculate apex, cone axis (apex→wide-end), and half angle from the
        // base/top radii. The cone surface evaluates as `apex + cone_axis·v`,
        // so `cone_axis` must point from the apex toward the WIDE end for the
        // body to land in the right place.
        let (apex, cone_axis, half_angle, actual_height) = if base_radius == 0.0 {
            // Apex at the base; the body opens along +axis toward the wide top.
            let half_angle = (top_radius / height).atan();
            (base_center, axis, half_angle, height)
        } else if top_radius == 0.0 {
            // Apex at the TOP (`base_center + axis·height`); the body opens
            // DOWNWARD, back toward `base_center`, so the cone's apex→base axis
            // is `-axis`. Without the flip the wide base circle is placed at
            // `apex + axis·height` — mirrored ABOVE the apex — so the whole
            // cone sits in the wrong location. That is invisible to standalone
            // volume (position-independent) and manifold (topology-only) checks
            // but fatal for Booleans, which then classify the cone's faces
            // against the wrong region (interior point landed at z=4.5 for a
            // cone whose base should be at z=-1.5).
            let half_angle = (base_radius / height).atan();
            let apex = base_center + axis * height;
            (apex, -axis, half_angle, height)
        } else {
            // True frustum (both radii positive). A near-equal pair is a
            // cylinder; otherwise build a real two-end frustum B-Rep (shared
            // circle edges + seam, like the cylinder) rather than the prior
            // lossy apex-cone approximation that left the truncation cap
            // disconnected from the lateral.
            let slope = (top_radius - base_radius) / height;
            if slope.abs() < 1e-10 {
                return self.create_cylinder_topology(base_center, axis, base_radius, height);
            }
            return self.create_frustum_topology(
                base_center,
                axis,
                base_radius,
                top_radius,
                height,
            );
        };

        // Create cone parameters
        let params = ConeParameters::new(apex, cone_axis, half_angle, actual_height)?;

        // Use the full cone implementation
        use crate::primitives::cone_primitive::ConePrimitive;
        ConePrimitive::create(&params, self.model)
    }

    // =====================================
    // TIMELINE AND PARAMETRIC OPERATIONS
    // =====================================

    /// Get timeline of operations
    pub fn get_timeline(&self) -> &[TimelineOperation] {
        &self.timeline
    }

    /// Update parameters of existing geometry with thread-safe caching
    pub fn update_parameters(
        &mut self,
        geometry_id: GeometryId,
        new_parameters: HashMap<String, f64>,
    ) -> Result<(), PrimitiveError> {
        let operation = TimelineOperation::UpdateParameters {
            geometry_id,
            new_parameters: new_parameters.clone(),
            timestamp: self.next_timestamp(),
        };
        // Purely mutating — no new outputs produced. Inputs are derived
        // inside `record_and_push` from the variant itself.
        self.record_and_push(operation.clone(), Vec::new());

        // Update global parameter cache for fast access
        let param_map = DashMap::new();
        for (key, value) in new_parameters {
            param_map.insert(key, value);
        }
        GEOMETRY_PARAMETERS.insert(geometry_id, param_map);

        // Cache timeline for this geometry's session
        let session_id = self.compute_session_id(geometry_id);
        TIMELINE_CACHE
            .entry(session_id)
            .or_default()
            .push(operation);

        // Implement actual parameter update logic with dependency tracking
        self.rebuild_geometry_with_parameters(geometry_id)?;

        Ok(())
    }

    /// Get cached parameters for geometry (production implementation)
    pub fn get_cached_parameters(&self, geometry_id: GeometryId) -> Option<DashMap<String, f64>> {
        GEOMETRY_PARAMETERS
            .get(&geometry_id)
            .map(|entry| entry.clone())
    }

    /// Rebuild geometry with new parameters (production implementation)
    fn rebuild_geometry_with_parameters(
        &mut self,
        geometry_id: GeometryId,
    ) -> Result<(), PrimitiveError> {
        // Get cached parameters
        let params = match GEOMETRY_PARAMETERS.get(&geometry_id) {
            Some(params) => params,
            None => return Ok(()), // No parameters to update
        };

        // Find original creation operation in timeline
        let session_id = self.compute_session_id(geometry_id);
        if let Some(timeline) = TIMELINE_CACHE.get(&session_id) {
            for operation in timeline.iter() {
                match operation {
                    TimelineOperation::Create3D { primitive_type, .. } => {
                        // Rebuild based on primitive type
                        match primitive_type.as_str() {
                            "box" => self.rebuild_box(geometry_id, &params)?,
                            "sphere" => self.rebuild_sphere(geometry_id, &params)?,
                            "cylinder" => self.rebuild_cylinder(geometry_id, &params)?,
                            _ => {} // Other types not implemented yet
                        }
                        break;
                    }
                    TimelineOperation::Create2D { primitive_type, .. } => {
                        // Rebuild 2D geometry
                        match primitive_type.as_str() {
                            "rectangle" => self.rebuild_rectangle(geometry_id, &params)?,
                            "circle" => self.rebuild_circle_2d(geometry_id, &params)?,
                            _ => {}
                        }
                        break;
                    }
                    _ => continue,
                }
            }
        }

        Ok(())
    }

    /// Compute session ID for geometry (production implementation)
    fn compute_session_id(&self, geometry_id: GeometryId) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        geometry_id.hash(&mut hasher);
        self.next_timestamp.hash(&mut hasher); // Include timestamp for uniqueness
        hasher.finish()
    }

    /// Validate updated box parameters.
    ///
    /// The actual topology rewrite is performed by
    /// `BoxPrimitive::update_parameters` (delete + recreate path); this
    /// function exists only to surface invalid cached parameters early so
    /// the timeline doesn't accept obviously bad updates.
    fn rebuild_box(
        &mut self,
        _geometry_id: GeometryId,
        params: &DashMap<String, f64>,
    ) -> Result<(), PrimitiveError> {
        let width = params.get("width").map(|v| *v).unwrap_or(1.0);
        let height = params.get("height").map(|v| *v).unwrap_or(1.0);
        let depth = params.get("depth").map(|v| *v).unwrap_or(1.0);

        if width <= 0.0 || height <= 0.0 || depth <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "dimensions".to_string(),
                value: format!("{}x{}x{}", width, height, depth),
                constraint: "all dimensions must be positive".to_string(),
            });
        }
        Ok(())
    }

    /// Validate updated sphere parameters.
    ///
    /// Topology rewrite happens in `SpherePrimitive::update_parameters`.
    fn rebuild_sphere(
        &mut self,
        _geometry_id: GeometryId,
        params: &DashMap<String, f64>,
    ) -> Result<(), PrimitiveError> {
        let radius = params.get("radius").map(|v| *v).unwrap_or(1.0);

        if radius <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "radius".to_string(),
                value: radius.to_string(),
                constraint: "must be positive".to_string(),
            });
        }
        Ok(())
    }

    /// Validate updated cylinder parameters.
    ///
    /// Topology rewrite happens in `CylinderPrimitive::update_parameters`.
    fn rebuild_cylinder(
        &mut self,
        _geometry_id: GeometryId,
        params: &DashMap<String, f64>,
    ) -> Result<(), PrimitiveError> {
        let radius = params.get("radius").map(|v| *v).unwrap_or(1.0);
        let height = params.get("height").map(|v| *v).unwrap_or(1.0);

        if radius <= 0.0 || height <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "dimensions".to_string(),
                value: format!("r={}, h={}", radius, height),
                constraint: "radius and height must be positive".to_string(),
            });
        }
        Ok(())
    }

    /// Validate updated 2D rectangle parameters.
    ///
    /// Topology rewrite happens through the 2D primitive update path.
    fn rebuild_rectangle(
        &mut self,
        _geometry_id: GeometryId,
        params: &DashMap<String, f64>,
    ) -> Result<(), PrimitiveError> {
        let width = params.get("width").map(|v| *v).unwrap_or(1.0);
        let height = params.get("height").map(|v| *v).unwrap_or(1.0);

        if width <= 0.0 || height <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "dimensions".to_string(),
                value: format!("{}x{}", width, height),
                constraint: "width and height must be positive".to_string(),
            });
        }
        Ok(())
    }

    /// Validate updated 2D circle parameters.
    ///
    /// Topology rewrite happens through the 2D primitive update path.
    fn rebuild_circle_2d(
        &mut self,
        _geometry_id: GeometryId,
        params: &DashMap<String, f64>,
    ) -> Result<(), PrimitiveError> {
        let radius = params.get("radius").map(|v| *v).unwrap_or(1.0);

        if radius <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "radius".to_string(),
                value: radius.to_string(),
                constraint: "must be positive".to_string(),
            });
        }
        Ok(())
    }

    /// Clear all cached data for a session (production memory management)
    pub fn clear_session_cache(&self, session_id: u64) {
        TIMELINE_CACHE.remove(&session_id);

        // Clean up geometry parameters that belong to this session
        // (In production, we'd have better session tracking)
        let mut to_remove = vec![];
        for entry in GEOMETRY_PARAMETERS.iter() {
            let computed_session = self.compute_session_id(*entry.key());
            if computed_session == session_id {
                to_remove.push(*entry.key());
            }
        }

        for geometry_id in to_remove {
            GEOMETRY_PARAMETERS.remove(&geometry_id);
        }
    }

    /// Get performance statistics for cached operations (production monitoring)
    pub fn get_cache_statistics() -> CacheStatistics {
        CacheStatistics {
            timeline_entries: TIMELINE_CACHE.len(),
            geometry_parameter_entries: GEOMETRY_PARAMETERS.len(),
            memory_usage_bytes: (TIMELINE_CACHE.len() * std::mem::size_of::<TimelineOperation>())
                + (GEOMETRY_PARAMETERS.len() * std::mem::size_of::<DashMap<String, f64>>()),
        }
    }
}

#[cfg(test)]
mod cascade_tests {
    use super::*;
    use crate::primitives::edge::{Edge, EdgeOrientation};
    use crate::primitives::r#loop::Loop;
    use crate::primitives::shell::Shell;

    /// Build a single-face triangle on z = 0:
    ///     v1 = (0, 0, 0)
    ///     v2 = (1, 0, 0)
    ///     v3 = (0.5, 1, 0)
    /// returns (model, [v1, v2, v3], [e1_v1v2, e2_v2v3, e3_v3v1], loop_id,
    /// face_id, shell_id).
    fn make_triangle() -> (
        BRepModel,
        [VertexId; 3],
        [EdgeId; 3],
        LoopId,
        FaceId,
        ShellId,
    ) {
        let mut model = BRepModel::new();
        let tol = Tolerance::default().distance();

        let v1 = model.vertices.add_or_find(0.0, 0.0, 0.0, tol);
        let v2 = model.vertices.add_or_find(1.0, 0.0, 0.0, tol);
        let v3 = model.vertices.add_or_find(0.5, 1.0, 0.0, tol);

        let c1 = model.curves.add(Box::new(Line::new(
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
        )));
        let c2 = model.curves.add(Box::new(Line::new(
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.5, 1.0, 0.0),
        )));
        let c3 = model.curves.add(Box::new(Line::new(
            Point3::new(0.5, 1.0, 0.0),
            Point3::new(0.0, 0.0, 0.0),
        )));

        let e1 = model.edges.add_or_find(Edge::new(
            0,
            v1,
            v2,
            c1,
            EdgeOrientation::Forward,
            ParameterRange::unit(),
        ));
        let e2 = model.edges.add_or_find(Edge::new(
            0,
            v2,
            v3,
            c2,
            EdgeOrientation::Forward,
            ParameterRange::unit(),
        ));
        let e3 = model.edges.add_or_find(Edge::new(
            0,
            v3,
            v1,
            c3,
            EdgeOrientation::Forward,
            ParameterRange::unit(),
        ));

        let mut face_loop = Loop::new(0, LoopType::Outer);
        face_loop.add_edge(e1, true);
        face_loop.add_edge(e2, true);
        face_loop.add_edge(e3, true);
        let loop_id = model.loops.add(face_loop);

        let plane = Plane::new(Point3::ORIGIN, Vector3::Z, Vector3::X)
            .expect("plane construction must succeed for axis-aligned XY plane");
        let surface_id = model.surfaces.add(Box::new(plane));
        let face = Face::new(0, surface_id, loop_id, FaceOrientation::Forward);
        let face_id = model.faces.add(face);

        let mut shell = Shell::new(0, ShellType::Open);
        shell.add_face(face_id);
        let shell_id = model.shells.add(shell);

        (
            model,
            [v1, v2, v3],
            [e1, e2, e3],
            loop_id,
            face_id,
            shell_id,
        )
    }

    #[test]
    fn delete_face_cascade_drops_face_and_shell_reference() {
        let (mut model, _v, _e, _loop_id, face_id, shell_id) = make_triangle();

        let report = model.delete_face_cascade(face_id);

        assert_eq!(report.removed_faces, vec![face_id]);
        assert!(report.removed_loops.is_empty());
        assert!(report.removed_edges.is_empty());
        assert!(report.removed_vertices.is_empty());
        assert_eq!(report.affected_shells, vec![shell_id]);

        assert_eq!(model.faces.iter().count(), 0);
        assert_eq!(model.loops.iter().count(), 1);
        assert_eq!(model.edges.iter().count(), 3);
        let live_shell = model
            .shells
            .get(shell_id)
            .expect("shell still exists after face cascade");
        assert!(live_shell.find_face(face_id).is_none());
    }

    #[test]
    fn delete_edge_cascade_propagates_through_loop_and_face() {
        let (mut model, _v, e, loop_id, face_id, shell_id) = make_triangle();

        let report = model.delete_edge_cascade(e[1]);

        assert!(report.removed_edges.contains(&e[1]));
        assert_eq!(report.removed_loops, vec![loop_id]);
        assert_eq!(report.removed_faces, vec![face_id]);
        assert_eq!(report.affected_shells, vec![shell_id]);

        let live_edges: Vec<_> = model.edges.iter().map(|(eid, _)| eid).collect();
        assert!(!live_edges.contains(&e[1]));
        assert_eq!(model.loops.iter().count(), 0);
        assert_eq!(model.faces.iter().count(), 0);
    }

    #[test]
    fn delete_loop_cascade_drops_face_but_preserves_edges_and_vertices() {
        let (mut model, _v, _e, loop_id, face_id, _shell_id) = make_triangle();

        let report = model.delete_loop_cascade(loop_id);

        assert_eq!(report.removed_loops, vec![loop_id]);
        assert_eq!(report.removed_faces, vec![face_id]);
        assert!(report.removed_edges.is_empty());
        assert!(report.removed_vertices.is_empty());

        assert_eq!(model.loops.iter().count(), 0);
        assert_eq!(model.faces.iter().count(), 0);
        // Edges and vertices belong to no other face, but cascading does not
        // chase ownership downward — they stay live.
        assert_eq!(model.edges.iter().count(), 3);
        assert_eq!(model.vertices.iter().count(), 3);
    }

    #[test]
    fn delete_vertex_cascade_on_missing_id_is_a_noop() {
        let mut model = BRepModel::new();
        let report = model.delete_vertex_cascade(99);
        assert!(report.removed_vertices.is_empty());
        assert!(report.removed_edges.is_empty());
        assert!(report.removed_loops.is_empty());
        assert!(report.removed_faces.is_empty());
        assert!(report.affected_shells.is_empty());
    }

    #[test]
    fn delete_vertex_cascade_on_isolated_vertex_does_not_touch_topology() {
        let (mut model, v, _e, loop_id, face_id, _shell_id) = make_triangle();
        let tol = Tolerance::default().distance();
        let isolated = model.vertices.add_or_find(5.0, 5.0, 5.0, tol);

        let report = model.delete_vertex_cascade(isolated);

        assert_eq!(report.removed_vertices, vec![isolated]);
        assert!(report.removed_edges.is_empty());
        assert!(report.removed_loops.is_empty());
        assert!(report.removed_faces.is_empty());

        // Original triangle survives intact.
        assert!(model.loops.get(loop_id).is_some());
        assert!(model.faces.get(face_id).is_some());
        for vid in v {
            assert!(model.vertices.get(vid).is_some());
        }
    }

    // ---- F1-β: model working tolerance ------------------------------------

    #[test]
    fn brep_model_default_tolerance_is_normal() {
        let model = BRepModel::new();
        assert!((model.tolerance().distance() - 1e-6).abs() < 1e-18);
    }

    #[test]
    fn brep_model_with_explicit_tolerance_seeds_vertex_store() {
        let model = BRepModel::with_estimated_capacity_and_tolerance(
            EstimatedComplexity::Medium,
            crate::math::tolerance::BOOLEAN_TOLERANCE,
        );
        assert!((model.tolerance().distance() - 1e-8).abs() < 1e-20);
    }

    #[test]
    fn brep_model_set_tolerance_overrides_field() {
        let mut model = BRepModel::new();
        model.set_tolerance(crate::math::tolerance::MANUFACTURING_TOLERANCE);
        assert!((model.tolerance().distance() - 1e-4).abs() < 1e-18);
    }
}

#[cfg(test)]
mod anchor_tests {
    //! Integration tests for Slice 2 datum anchoring.
    //!
    //! Each test creates a `BRepModel` (which seeds the canonical seven
    //! default datums), then exercises the `create_*_anchored` builders
    //! and verifies both:
    //!
    //! 1. `solid.anchor` metadata is stamped correctly (datum id +
    //!    local transform), so downstream consumers can answer "what
    //!    was this primitive placed against?".
    //! 2. The geometry is actually transformed into world space —
    //!    anchoring to a translated frame must shift vertex positions.
    //!
    //! Tests deliberately avoid asserting on rotation-specific vertex
    //! positions to stay robust against `Matrix4` convention changes.
    //! The bounding-box deltas verify the integration end-to-end.
    use super::*;
    use crate::math::Matrix4;
    use crate::primitives::datum::DatumKind;
    use crate::sketch2d::sketch_plane::PlaneOrientation;
    use std::collections::HashSet;

    /// Walk the solid's topology and return every unique vertex position.
    fn collect_vertex_positions(model: &BRepModel, solid_id: SolidId) -> Vec<Point3> {
        let solid = model
            .solids
            .get(solid_id)
            .expect("solid exists for collect_vertex_positions");
        let shell = model
            .shells
            .get(solid.outer_shell)
            .expect("outer shell exists");

        let mut seen: HashSet<VertexId> = HashSet::new();
        let mut positions = Vec::new();
        for &face_id in &shell.faces {
            let face = model.faces.get(face_id).expect("face exists");
            let outer = model.loops.get(face.outer_loop).expect("loop exists");
            for &edge_id in &outer.edges {
                let edge = model.edges.get(edge_id).expect("edge exists");
                for vid in [edge.start_vertex, edge.end_vertex] {
                    if seen.insert(vid) {
                        let v = model
                            .vertices
                            .get(vid)
                            .expect("vertex exists for collected edge");
                        positions.push(Point3::new(v.position[0], v.position[1], v.position[2]));
                    }
                }
            }
        }
        positions
    }

    /// Tight bounding box `(min, max)` from a list of points.
    fn bbox_of(points: &[Point3]) -> (Point3, Point3) {
        let mut min = Point3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
        let mut max = Point3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        for p in points {
            min.x = min.x.min(p.x);
            min.y = min.y.min(p.y);
            min.z = min.z.min(p.z);
            max.x = max.x.max(p.x);
            max.y = max.y.max(p.y);
            max.z = max.z.max(p.z);
        }
        (min, max)
    }

    /// Find the seeded `TopPlane` (XZ orientation) datum id. The seed
    /// order is fixed by `DatumStore::seed_defaults`, but tests look up
    /// by kind to stay decoupled from the seed's allocation order.
    fn top_plane_id(model: &BRepModel) -> u32 {
        model
            .datums
            .snapshot()
            .into_iter()
            .find(|d| matches!(d.kind, DatumKind::Plane(PlaneOrientation::XZ)))
            .expect("default TopPlane is seeded by BRepModel::new")
            .id
    }

    /// Find the seeded `Origin` datum id. Always 0 in current
    /// `seed_defaults`, but resolved by kind for the same reason.
    fn origin_id(model: &BRepModel) -> u32 {
        model
            .datums
            .snapshot()
            .into_iter()
            .find(|d| matches!(d.kind, DatumKind::Origin))
            .expect("default Origin is seeded by BRepModel::new")
            .id
    }

    #[test]
    fn anchor_metadata_recorded_for_top_plane() {
        let mut model = BRepModel::new();
        let datum_id = top_plane_id(&model);

        let geo = {
            let mut builder = TopologyBuilder::new(&mut model);
            builder
                .create_box_3d_anchored(10.0, 10.0, 10.0, datum_id, Matrix4::identity())
                .expect("anchored box creation succeeds")
        };

        let solid_id = match geo {
            GeometryId::Solid(sid) => sid,
            other => panic!("expected GeometryId::Solid, got {:?}", other),
        };

        let solid = model.solids.get(solid_id).expect("solid in store");
        let anchor = &solid.anchor;
        assert_eq!(anchor.datum_id, datum_id);
        // Identity round-trip: every diagonal element 1, off-diagonals 0.
        for r in 0..4 {
            for c in 0..4 {
                let expected = if r == c { 1.0 } else { 0.0 };
                assert!(
                    (anchor.local_transform.get(r, c) - expected).abs() < 1e-12,
                    "local_transform[{},{}] expected {}, got {}",
                    r,
                    c,
                    expected,
                    anchor.local_transform.get(r, c)
                );
            }
        }
    }

    #[test]
    fn anchored_with_identity_to_origin_matches_unanchored_geometry() {
        // Anchoring to Origin with identity local transform is a no-op
        // on geometry — the world transform composes to identity, the
        // `transform_solid` call is skipped, and vertex positions
        // should match the unanchored creator exactly.
        let mut a = BRepModel::new();
        let unanchored = {
            let mut builder = TopologyBuilder::new(&mut a);
            builder
                .create_box_3d(10.0, 10.0, 10.0)
                .expect("unanchored box creation succeeds")
        };
        let unanchored_sid = match unanchored {
            GeometryId::Solid(sid) => sid,
            other => panic!("expected Solid, got {:?}", other),
        };
        let unanchored_positions = collect_vertex_positions(&a, unanchored_sid);
        let (a_min, a_max) = bbox_of(&unanchored_positions);

        let mut b = BRepModel::new();
        let datum_id = origin_id(&b);
        let anchored = {
            let mut builder = TopologyBuilder::new(&mut b);
            builder
                .create_box_3d_anchored(10.0, 10.0, 10.0, datum_id, Matrix4::identity())
                .expect("anchored box creation succeeds")
        };
        let anchored_sid = match anchored {
            GeometryId::Solid(sid) => sid,
            other => panic!("expected Solid, got {:?}", other),
        };
        let anchored_positions = collect_vertex_positions(&b, anchored_sid);
        let (b_min, b_max) = bbox_of(&anchored_positions);

        assert!(
            (a_min.x - b_min.x).abs() < 1e-9
                && (a_min.y - b_min.y).abs() < 1e-9
                && (a_min.z - b_min.z).abs() < 1e-9,
            "min mismatch: unanchored={:?} anchored={:?}",
            a_min,
            b_min
        );
        assert!(
            (a_max.x - b_max.x).abs() < 1e-9
                && (a_max.y - b_max.y).abs() < 1e-9
                && (a_max.z - b_max.z).abs() < 1e-9,
            "max mismatch: unanchored={:?} anchored={:?}",
            a_max,
            b_max
        );

        let solid = b.solids.get(anchored_sid).expect("anchored solid in store");
        let anchor = &solid.anchor;
        assert_eq!(anchor.datum_id, datum_id);
    }

    #[test]
    fn anchored_translation_shifts_bbox_by_local_translation() {
        // Compose a translation-only local transform on the Origin
        // datum. Origin's frame is identity, so world_transform =
        // local_transform = translation(10, 0, 0). The box's
        // bounding-box min/max should be exactly 10mm shifted along +X
        // relative to an unanchored box of the same dimensions.
        let mut base = BRepModel::new();
        let base_geo = {
            let mut builder = TopologyBuilder::new(&mut base);
            builder
                .create_box_3d(10.0, 10.0, 10.0)
                .expect("unanchored box creation succeeds")
        };
        let base_sid = match base_geo {
            GeometryId::Solid(sid) => sid,
            other => panic!("expected Solid, got {:?}", other),
        };
        let (base_min, base_max) = bbox_of(&collect_vertex_positions(&base, base_sid));

        let mut shifted = BRepModel::new();
        let datum_id = origin_id(&shifted);
        let local = Matrix4::translation(10.0, 0.0, 0.0);
        let shifted_geo = {
            let mut builder = TopologyBuilder::new(&mut shifted);
            builder
                .create_box_3d_anchored(10.0, 10.0, 10.0, datum_id, local)
                .expect("anchored box with translation succeeds")
        };
        let shifted_sid = match shifted_geo {
            GeometryId::Solid(sid) => sid,
            other => panic!("expected Solid, got {:?}", other),
        };
        let (s_min, s_max) = bbox_of(&collect_vertex_positions(&shifted, shifted_sid));

        // +10 on X, identical on Y and Z.
        assert!(
            (s_min.x - (base_min.x + 10.0)).abs() < 1e-9,
            "min.x not shifted: base={} shifted={}",
            base_min.x,
            s_min.x
        );
        assert!(
            (s_max.x - (base_max.x + 10.0)).abs() < 1e-9,
            "max.x not shifted: base={} shifted={}",
            base_max.x,
            s_max.x
        );
        assert!((s_min.y - base_min.y).abs() < 1e-9, "min.y changed");
        assert!((s_max.y - base_max.y).abs() < 1e-9, "max.y changed");
        assert!((s_min.z - base_min.z).abs() < 1e-9, "min.z changed");
        assert!((s_max.z - base_max.z).abs() < 1e-9, "max.z changed");

        let solid = shifted
            .solids
            .get(shifted_sid)
            .expect("shifted solid in store");
        let anchor = &solid.anchor;
        assert_eq!(anchor.datum_id, datum_id);
        // Local transform round-trip: the (0,3) entry should be 10.0.
        assert!(
            (anchor.local_transform.get(0, 3) - 10.0).abs() < 1e-12,
            "translation in local_transform should round-trip"
        );
    }

    #[test]
    fn unknown_datum_id_returns_invalid_parameters_error() {
        let mut model = BRepModel::new();
        let bogus_id = u32::MAX - 1;
        let mut builder = TopologyBuilder::new(&mut model);
        let result =
            builder.create_box_3d_anchored(10.0, 10.0, 10.0, bogus_id, Matrix4::identity());
        match result {
            Err(PrimitiveError::InvalidParameters { parameter, .. }) => {
                assert_eq!(parameter, "datum_id");
            }
            other => panic!(
                "expected InvalidParameters {{parameter: \"datum_id\"}}, got {:?}",
                other
            ),
        }
    }

    /// Slice 3a invariant: every solid carries an anchor — no `Option` —
    /// and unanchored creators default to `SolidAnchor::world_origin()`.
    /// Agents and downstream queries can therefore always read
    /// `solid.anchor.datum_id` without first proving the anchor exists.
    #[test]
    fn default_creator_solid_carries_world_origin_anchor() {
        let mut model = BRepModel::new();
        let geo = {
            let mut builder = TopologyBuilder::new(&mut model);
            builder
                .create_box_3d(10.0, 10.0, 10.0)
                .expect("box creation succeeds")
        };
        let sid = match geo {
            GeometryId::Solid(s) => s,
            other => panic!("expected Solid, got {:?}", other),
        };
        let solid = model.solids.get(sid).expect("solid in store");
        assert_eq!(
            solid.anchor.datum_id, 0,
            "unanchored creator must default to Origin (datum id 0)"
        );
        for r in 0..4 {
            for c in 0..4 {
                let expected = if r == c { 1.0 } else { 0.0 };
                assert!(
                    (solid.anchor.local_transform.get(r, c) - expected).abs() < 1e-12,
                    "default anchor's local_transform must be identity at [{},{}]",
                    r,
                    c
                );
            }
        }
    }

    // ────────────────────────── 3c: datum recording ───────────────────────────

    use crate::operations::recorder::{OperationRecorder, RecordedOperation, RecorderError};
    use std::sync::{Arc, Mutex};

    /// Test recorder that captures every event for inspection. Mirrors the
    /// one in `operations/recorder.rs` tests but is reachable from this
    /// module's `#[cfg(test)]` scope.
    #[derive(Debug, Default)]
    struct CaptureRecorder {
        events: Mutex<Vec<RecordedOperation>>,
    }

    impl OperationRecorder for CaptureRecorder {
        fn record(&self, operation: RecordedOperation) -> Result<(), RecorderError> {
            self.events
                .lock()
                .expect("CaptureRecorder mutex poisoned")
                .push(operation);
            Ok(())
        }
    }

    #[test]
    fn set_datum_visibility_records_event_when_recorder_attached() {
        let mut model = BRepModel::new();
        let recorder = Arc::new(CaptureRecorder::default());
        model.attach_recorder(Some(recorder.clone()));

        // Origin (id 0) starts visible. Hide it.
        let prev = model
            .set_datum_visibility(0, false)
            .expect("origin id 0 exists");
        assert!(prev, "origin starts visible");

        let events = recorder
            .events
            .lock()
            .expect("CaptureRecorder mutex poisoned")
            .clone();
        assert_eq!(events.len(), 1, "exactly one event recorded");
        let ev = &events[0];
        assert_eq!(ev.kind, "datum_set_visibility");
        assert_eq!(ev.inputs, vec!["datum:0".to_string()]);
        assert_eq!(ev.parameters["datum_id"], 0);
        assert_eq!(ev.parameters["visible"], false);
        assert_eq!(ev.parameters["previous_visible"], true);
    }

    #[test]
    fn set_datum_visibility_returns_none_for_unknown_id_and_skips_record() {
        let mut model = BRepModel::new();
        let recorder = Arc::new(CaptureRecorder::default());
        model.attach_recorder(Some(recorder.clone()));

        let result = model.set_datum_visibility(9999, false);
        assert!(result.is_none(), "unknown datum id returns None");

        let events = recorder
            .events
            .lock()
            .expect("CaptureRecorder mutex poisoned")
            .clone();
        assert!(events.is_empty(), "no event recorded on lookup miss");
    }

    /// Default-seeded datums are an invariant of `BRepModel::new()` — they
    /// are *not* recorded as `datum_create` events. Replay starts from
    /// "model with seven defaults already present", so the recorder
    /// should be empty immediately after construction even when attached.
    #[test]
    fn default_seed_does_not_emit_recorded_events() {
        let mut model = BRepModel::new();
        let recorder = Arc::new(CaptureRecorder::default());
        model.attach_recorder(Some(recorder.clone()));

        // Construction is already complete; attaching the recorder
        // afterwards must not back-fill a seed event.
        let events = recorder
            .events
            .lock()
            .expect("CaptureRecorder mutex poisoned")
            .clone();
        assert!(
            events.is_empty(),
            "default seeding is an invariant — must not be recorded"
        );
        assert_eq!(model.datums.len(), 7, "seven defaults are present");
    }

    // ─────────────────────────── 3b: query API ────────────────────────────────

    /// Helper: build a 10×10×10 mm box at the world origin and return its id.
    fn build_unit_box(model: &mut BRepModel) -> SolidId {
        let geo = {
            let mut builder = TopologyBuilder::new(model);
            builder
                .create_box_3d(10.0, 10.0, 10.0)
                .expect("box creation succeeds")
        };
        match geo {
            GeometryId::Solid(s) => s,
            other => panic!("expected Solid, got {:?}", other),
        }
    }

    #[test]
    fn solid_world_bbox_matches_input_dimensions() {
        let mut model = BRepModel::new();
        let sid = build_unit_box(&mut model);
        let bb = model.solid_world_bbox(sid).expect("box has world bbox");
        let size = bb.size();
        // create_box_3d centers the box at the origin, so a 10×10×10 box
        // spans (-5,-5,-5) → (+5,+5,+5).
        assert!((size.x - 10.0).abs() < 1e-9, "x extent");
        assert!((size.y - 10.0).abs() < 1e-9, "y extent");
        assert!((size.z - 10.0).abs() < 1e-9, "z extent");
        let center = bb.center();
        assert!(center.x.abs() < 1e-9, "center x ≈ 0");
        assert!(center.y.abs() < 1e-9, "center y ≈ 0");
        assert!(center.z.abs() < 1e-9, "center z ≈ 0");
    }

    #[test]
    fn solid_world_bbox_returns_none_for_unknown_id() {
        let model = BRepModel::new();
        assert!(model.solid_world_bbox(9999).is_none());
    }

    /// #42 REGRESSION: the bbox must come from the curved-face extent, not
    /// the two seam vertices. A cylinder r10 h40 at the origin (z 0..40)
    /// must report an AABB ≈ x[-10,10] y[-10,10] z[0,40] centred (0,0,20),
    /// NOT a zero-width box collapsed to the seam at (10,0).
    #[test]
    fn solid_world_bbox_captures_cylinder_radial_extent_42() {
        let mut model = BRepModel::new();
        let gid = TopologyBuilder::new(&mut model)
            .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, 10.0, 40.0)
            .expect("cylinder creation succeeds");
        let sid = match gid {
            GeometryId::Solid(id) => id,
            other => panic!("expected Solid, got {:?}", other),
        };
        let bb = model.solid_world_bbox(sid).expect("cylinder world bbox");
        let size = bb.size();
        let c = bb.center();
        // Radial extent ≈ 2r (samples lie on the circle of radius 10, so the
        // span is ≤ 20 and clearly not the collapsed ~0 of the old bug).
        assert!(
            size.x > 18.0 && size.x < 20.5,
            "x extent ≈ 20, got {}",
            size.x
        );
        assert!(
            size.y > 18.0 && size.y < 20.5,
            "y extent ≈ 20, got {}",
            size.y
        );
        assert!((size.z - 40.0).abs() < 1e-6, "z extent 40, got {}", size.z);
        assert!(
            c.x.abs() < 0.5,
            "center x ≈ 0 (not the seam at r), got {}",
            c.x
        );
        assert!(c.y.abs() < 0.5, "center y ≈ 0, got {}", c.y);
        assert!((c.z - 20.0).abs() < 0.5, "center z ≈ 20, got {}", c.z);
    }

    #[test]
    fn solid_bbox_in_origin_frame_equals_world_bbox() {
        // Origin datum has identity transform, so bbox-in-frame must
        // match world bbox exactly.
        let mut model = BRepModel::new();
        let sid = build_unit_box(&mut model);
        let world = model.solid_world_bbox(sid).expect("world bbox");
        let local = model
            .solid_bbox_in_frame(sid, 0)
            .expect("bbox in origin frame");
        let ws = world.size();
        let ls = local.size();
        assert!((ls.x - ws.x).abs() < 1e-9);
        assert!((ls.y - ws.y).abs() < 1e-9);
        assert!((ls.z - ws.z).abs() < 1e-9);
        let wc = world.center();
        let lc = local.center();
        assert!((lc.x - wc.x).abs() < 1e-9);
        assert!((lc.y - wc.y).abs() < 1e-9);
        assert!((lc.z - wc.z).abs() < 1e-9);
    }

    #[test]
    fn solid_bbox_in_frame_returns_none_for_unknown_datum() {
        let mut model = BRepModel::new();
        let sid = build_unit_box(&mut model);
        assert!(model.solid_bbox_in_frame(sid, 9999).is_none());
    }

    #[test]
    fn solid_distance_zero_for_coincident_solids() {
        let mut model = BRepModel::new();
        let a = build_unit_box(&mut model);
        let b = build_unit_box(&mut model);
        let d = model.solid_distance(a, b).expect("distance defined");
        assert!(
            d.abs() < 1e-9,
            "two boxes built at the world origin have coincident centers, got {}",
            d
        );
    }

    #[test]
    fn solid_distance_returns_none_for_unknown_id() {
        let mut model = BRepModel::new();
        let sid = build_unit_box(&mut model);
        assert!(model.solid_distance(sid, 9999).is_none());
        assert!(model.solid_distance(9999, sid).is_none());
    }

    #[test]
    fn solid_location_descriptor_for_origin_box() {
        let mut model = BRepModel::new();
        let sid = build_unit_box(&mut model);
        let desc = model
            .solid_location_descriptor(sid)
            .expect("location descriptor defined");
        assert_eq!(desc.solid_id, sid);
        // Default-anchored solid points at Origin (id 0).
        assert_eq!(desc.anchor_datum_id, 0);
        assert_eq!(desc.anchor_datum_name, "Origin");
        assert!(desc.center_world.iter().all(|c| c.abs() < 1e-9));
        assert!(desc.center_in_anchor_frame.iter().all(|c| c.abs() < 1e-9));
        assert!((desc.dimensions_world[0] - 10.0).abs() < 1e-9);
        assert!((desc.dimensions_world[1] - 10.0).abs() < 1e-9);
        assert!((desc.dimensions_world[2] - 10.0).abs() < 1e-9);
        // Box centered on origin → all three signed distances are zero.
        assert!(desc.signed_distance_front.abs() < 1e-9);
        assert!(desc.signed_distance_top.abs() < 1e-9);
        assert!(desc.signed_distance_right.abs() < 1e-9);
    }

    #[test]
    fn solid_location_descriptor_returns_none_for_unknown_id() {
        let model = BRepModel::new();
        assert!(model.solid_location_descriptor(9999).is_none());
    }

    // ──────────────────── 4a: user-authored datum mediators ──────────────────

    #[test]
    fn create_datum_plane_records_event() {
        let mut model = BRepModel::new();
        let recorder = Arc::new(CaptureRecorder::default());
        model.attach_recorder(Some(recorder.clone()));

        let translation = Matrix4::from_translation(&Vector3::new(1.0, 2.0, 3.0));
        let id = model
            .create_datum_plane("WorkPlane".to_string(), translation)
            .expect("create succeeds");
        assert_eq!(id, 7, "first user datum after seven defaults");

        let events = recorder
            .events
            .lock()
            .expect("CaptureRecorder mutex poisoned")
            .clone();
        assert_eq!(events.len(), 1);
        let ev = &events[0];
        assert_eq!(ev.kind, "datum_create");
        assert_eq!(ev.outputs, vec![format!("datum:{}", id)]);
        assert_eq!(ev.parameters["kind"], "plane");
        assert_eq!(ev.parameters["name"], "WorkPlane");
        assert_eq!(ev.parameters["datum_id"], id);
    }

    #[test]
    fn create_datum_axis_records_event_with_direction() {
        let mut model = BRepModel::new();
        let recorder = Arc::new(CaptureRecorder::default());
        model.attach_recorder(Some(recorder.clone()));

        let id = model
            .create_datum_axis(
                "RefY".to_string(),
                Point3::new(0.0, 5.0, 0.0),
                crate::primitives::datum::AxisDirection::Y,
            )
            .expect("axis create succeeds");

        let events = recorder
            .events
            .lock()
            .expect("CaptureRecorder mutex poisoned")
            .clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "datum_create");
        assert_eq!(events[0].parameters["kind"], "axis");
        assert_eq!(events[0].parameters["direction"], "y");
        assert_eq!(events[0].outputs, vec![format!("datum:{}", id)]);
    }

    #[test]
    fn create_datum_point_records_event_with_position() {
        let mut model = BRepModel::new();
        let recorder = Arc::new(CaptureRecorder::default());
        model.attach_recorder(Some(recorder.clone()));

        let id = model
            .create_datum_point("Probe".to_string(), Point3::new(7.0, 8.0, 9.0))
            .expect("point create succeeds");

        let events = recorder
            .events
            .lock()
            .expect("CaptureRecorder mutex poisoned")
            .clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "datum_create");
        assert_eq!(events[0].parameters["kind"], "point");
        let pos = events[0].parameters["position"]
            .as_array()
            .expect("position is an array");
        assert_eq!(pos[0].as_f64().unwrap_or_default(), 7.0);
        assert_eq!(pos[1].as_f64().unwrap_or_default(), 8.0);
        assert_eq!(pos[2].as_f64().unwrap_or_default(), 9.0);
        assert_eq!(events[0].outputs, vec![format!("datum:{}", id)]);
    }

    #[test]
    fn create_datum_with_empty_name_returns_error_and_skips_record() {
        let mut model = BRepModel::new();
        let recorder = Arc::new(CaptureRecorder::default());
        model.attach_recorder(Some(recorder.clone()));

        let err = model
            .create_datum_point("".to_string(), Point3::ORIGIN)
            .expect_err("empty name rejected");
        assert!(matches!(
            err,
            crate::primitives::datum::DatumError::EmptyName
        ));

        let events = recorder
            .events
            .lock()
            .expect("CaptureRecorder mutex poisoned")
            .clone();
        assert!(events.is_empty(), "no event recorded on validation failure");
    }

    #[test]
    fn rename_datum_records_event_and_refuses_defaults() {
        let mut model = BRepModel::new();
        let recorder = Arc::new(CaptureRecorder::default());
        model.attach_recorder(Some(recorder.clone()));

        let id = model
            .create_datum_point("Old".to_string(), Point3::ORIGIN)
            .expect("created");
        let prev = model
            .rename_datum(id, "New".to_string())
            .expect("rename succeeds");
        assert_eq!(prev, "Old");

        // Refuse rename of a default — this must not record.
        let err = model
            .rename_datum(0, "NotOrigin".to_string())
            .expect_err("default rename rejected");
        assert!(matches!(
            err,
            crate::primitives::datum::DatumError::DefaultDatumNotMutable(0)
        ));

        let events = recorder
            .events
            .lock()
            .expect("CaptureRecorder mutex poisoned")
            .clone();
        // Two: datum_create + datum_rename. The default-rename failure
        // does NOT contribute an event.
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, "datum_create");
        assert_eq!(events[1].kind, "datum_rename");
        assert_eq!(events[1].parameters["previous_name"], "Old");
        assert_eq!(events[1].parameters["name"], "New");
    }

    #[test]
    fn set_datum_transform_records_event_and_refuses_defaults() {
        let mut model = BRepModel::new();
        let recorder = Arc::new(CaptureRecorder::default());
        model.attach_recorder(Some(recorder.clone()));

        let id = model
            .create_datum_point("P".to_string(), Point3::ORIGIN)
            .expect("created");
        let new_t = Matrix4::from_translation(&Vector3::new(4.0, 5.0, 6.0));
        let _prev = model.set_datum_transform(id, new_t).expect("set succeeds");

        // Default refusal.
        let err = model
            .set_datum_transform(0, new_t)
            .expect_err("default refused");
        assert!(matches!(
            err,
            crate::primitives::datum::DatumError::DefaultDatumNotMutable(0)
        ));

        let events = recorder
            .events
            .lock()
            .expect("CaptureRecorder mutex poisoned")
            .clone();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].kind, "datum_set_transform");
        assert_eq!(events[1].inputs, vec![format!("datum:{}", id)]);
    }

    #[test]
    fn delete_datum_records_event_and_refuses_defaults() {
        let mut model = BRepModel::new();
        let recorder = Arc::new(CaptureRecorder::default());
        model.attach_recorder(Some(recorder.clone()));

        let id = model
            .create_datum_point("Tmp".to_string(), Point3::ORIGIN)
            .expect("created");
        let removed = model.delete_datum(id).expect("delete succeeds");
        assert_eq!(removed.id, id);
        assert!(model.datums.get(id).is_none());

        let err = model.delete_datum(0).expect_err("default refused");
        assert!(matches!(
            err,
            crate::primitives::datum::DatumError::DefaultDatumNotMutable(0)
        ));

        let events = recorder
            .events
            .lock()
            .expect("CaptureRecorder mutex poisoned")
            .clone();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].kind, "datum_delete");
        assert_eq!(events[1].parameters["name"], "Tmp");
    }

    // ───────────────────── 4b: derived datum evaluation ────────────────────────

    use crate::primitives::datum::{AxisDirection, DatumError, DatumSource, INVALID_DATUM_ID};
    use crate::primitives::vertex::VertexId;

    fn box_with_seeded_datums() -> (BRepModel, SolidId) {
        let mut model = BRepModel::new();
        model.datums.seed_defaults();
        let geo = {
            let mut builder = TopologyBuilder::new(&mut model);
            builder
                .create_box_3d(2.0, 2.0, 2.0)
                .expect("box creation succeeds")
        };
        let sid = match geo {
            GeometryId::Solid(s) => s,
            other => panic!("expected Solid, got {:?}", other),
        };
        (model, sid)
    }

    fn vertex_at(model: &BRepModel, target: [f64; 3]) -> VertexId {
        for id in 0..(model.vertices.len() as u32) {
            if let Some(v) = model.vertices.get(id) {
                let dx = v.position[0] - target[0];
                let dy = v.position[1] - target[1];
                let dz = v.position[2] - target[2];
                if dx * dx + dy * dy + dz * dz < 1e-18 {
                    return id;
                }
            }
        }
        panic!("no vertex at {:?}", target);
    }

    #[test]
    fn evaluate_manual_round_trips_transform() {
        let model = BRepModel::new();
        let m = Matrix4::from_translation(&Vector3::new(3.0, 4.0, 5.0));
        let source = DatumSource::manual(m);
        let evaluated = model
            .evaluate_datum_source(&source)
            .expect("manual evaluates");
        for r in 0..4 {
            for c in 0..4 {
                assert!((evaluated.get(r, c) - m.get(r, c)).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn evaluate_offset_plane_translates_along_normal() {
        let model = BRepModel::new();
        model.datums.seed_defaults();
        let source = DatumSource::OffsetPlane {
            base: 1,
            distance: 5.0,
        };
        let m = model
            .evaluate_datum_source(&source)
            .expect("offset evaluates");
        assert!((m.get(0, 3) - 0.0).abs() < 1e-12);
        assert!((m.get(1, 3) - 0.0).abs() < 1e-12);
        assert!((m.get(2, 3) - 5.0).abs() < 1e-12);
        assert!((m.get(2, 2) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn evaluate_offset_plane_unknown_base_errors() {
        let model = BRepModel::new();
        let source = DatumSource::OffsetPlane {
            base: INVALID_DATUM_ID,
            distance: 1.0,
        };
        let err = model
            .evaluate_datum_source(&source)
            .expect_err("unknown base");
        assert!(matches!(
            err,
            DatumError::UnknownReference { kind: "datum", .. }
        ));
    }

    #[test]
    fn evaluate_angle_plane_rotates_normal_around_axis() {
        let model = BRepModel::new();
        model.datums.seed_defaults();
        let source = DatumSource::AnglePlane {
            base: 1,
            axis: 4,
            angle: std::f64::consts::FRAC_PI_2,
        };
        let m = model
            .evaluate_datum_source(&source)
            .expect("angle plane evaluates");
        let nz_x = m.get(0, 2);
        let nz_y = m.get(1, 2);
        let nz_z = m.get(2, 2);
        assert!(nz_x.abs() < 1e-9);
        assert!(nz_y.abs() > 0.9);
        assert!(nz_z.abs() < 1e-9);
    }

    #[test]
    fn evaluate_three_points_uses_first_vertex_as_origin() {
        let (model, _sid) = box_with_seeded_datums();
        let p0 = vertex_at(&model, [-1.0, -1.0, -1.0]);
        let p1 = vertex_at(&model, [1.0, -1.0, -1.0]);
        let p2 = vertex_at(&model, [-1.0, 1.0, -1.0]);
        let source = DatumSource::ThreePoints { p0, p1, p2 };
        let m = model
            .evaluate_datum_source(&source)
            .expect("three points evaluate");
        assert!((m.get(0, 3) - -1.0).abs() < 1e-12);
        assert!((m.get(1, 3) - -1.0).abs() < 1e-12);
        assert!((m.get(2, 3) - -1.0).abs() < 1e-12);
        // (p1-p0) × (p2-p0) = (2,0,0) × (0,2,0) = (0,0,4) → +Z normal.
        assert!((m.get(2, 2) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn evaluate_three_points_collinear_is_degenerate() {
        let (model, _sid) = box_with_seeded_datums();
        let p0 = vertex_at(&model, [-1.0, -1.0, -1.0]);
        let source = DatumSource::ThreePoints { p0, p1: p0, p2: p0 };
        let err = model.evaluate_datum_source(&source).expect_err("collinear");
        assert!(matches!(err, DatumError::DegenerateSource(_)));
    }

    #[test]
    fn evaluate_mid_plane_averages_normals_and_origins() {
        let model = BRepModel::new();
        model.datums.seed_defaults();
        let source = DatumSource::MidPlane { a: 1, b: 2 };
        let m = model
            .evaluate_datum_source(&source)
            .expect("mid plane evaluates");
        for c in 0..3 {
            assert!(m.get(c, 3).abs() < 1e-12);
        }
        assert!((m.get(1, 2).abs() - m.get(2, 2).abs()).abs() < 1e-9);
    }

    #[test]
    fn evaluate_mid_plane_antiparallel_is_degenerate() {
        let model = BRepModel::new();
        model.datums.seed_defaults();
        let flip = Matrix4::rotation_x(std::f64::consts::PI);
        let neg_z_id = model
            .datums
            .create_plane("NegZ".to_string(), flip)
            .expect("flip plane created");
        let source = DatumSource::MidPlane { a: 1, b: neg_z_id };
        let err = model
            .evaluate_datum_source(&source)
            .expect_err("antiparallel");
        assert!(matches!(err, DatumError::DegenerateSource(_)));
    }

    #[test]
    fn evaluate_two_points_axis_uses_midpoint() {
        let (model, _sid) = box_with_seeded_datums();
        let p0 = vertex_at(&model, [-1.0, -1.0, -1.0]);
        let p1 = vertex_at(&model, [1.0, -1.0, -1.0]);
        let source = DatumSource::TwoPointsAxis { p0, p1 };
        let m = model
            .evaluate_datum_source(&source)
            .expect("two points axis evaluates");
        assert!((m.get(0, 3) - 0.0).abs() < 1e-12);
        assert!((m.get(1, 3) - -1.0).abs() < 1e-12);
        assert!((m.get(2, 3) - -1.0).abs() < 1e-12);
        assert!((m.get(0, 2) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn evaluate_two_points_axis_coincident_is_degenerate() {
        let (model, _sid) = box_with_seeded_datums();
        let p0 = vertex_at(&model, [-1.0, -1.0, -1.0]);
        let source = DatumSource::TwoPointsAxis { p0, p1: p0 };
        let err = model
            .evaluate_datum_source(&source)
            .expect_err("coincident");
        assert!(matches!(err, DatumError::DegenerateSource(_)));
    }

    #[test]
    fn evaluate_normal_axis_uses_plane_normal_through_vertex() {
        let (model, _sid) = box_with_seeded_datums();
        let p = vertex_at(&model, [1.0, 1.0, 1.0]);
        let source = DatumSource::NormalAxis { plane: 1, point: p };
        let m = model
            .evaluate_datum_source(&source)
            .expect("normal axis evaluates");
        assert!((m.get(0, 3) - 1.0).abs() < 1e-12);
        assert!((m.get(1, 3) - 1.0).abs() < 1e-12);
        assert!((m.get(2, 3) - 1.0).abs() < 1e-12);
        assert!((m.get(2, 2) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn evaluate_vertex_point_translates_to_position() {
        let (model, _sid) = box_with_seeded_datums();
        let p = vertex_at(&model, [-1.0, 1.0, -1.0]);
        let source = DatumSource::VertexPoint { vertex: p };
        let m = model
            .evaluate_datum_source(&source)
            .expect("vertex point evaluates");
        assert!((m.get(0, 3) - -1.0).abs() < 1e-12);
        assert!((m.get(1, 3) - 1.0).abs() < 1e-12);
        assert!((m.get(2, 3) - -1.0).abs() < 1e-12);
    }

    #[test]
    fn evaluate_curve_midpoint_lies_on_box_edge() {
        let (model, _sid) = box_with_seeded_datums();
        let edge = model.edges.get(0).expect("at least one edge");
        let start = model.vertices.get(edge.start_vertex).expect("start vertex");
        let end = model.vertices.get(edge.end_vertex).expect("end vertex");
        let expected = [
            0.5 * (start.position[0] + end.position[0]),
            0.5 * (start.position[1] + end.position[1]),
            0.5 * (start.position[2] + end.position[2]),
        ];
        let source = DatumSource::CurveMidpoint { edge: 0 };
        let m = model
            .evaluate_datum_source(&source)
            .expect("curve midpoint evaluates");
        assert!((m.get(0, 3) - expected[0]).abs() < 1e-9);
        assert!((m.get(1, 3) - expected[1]).abs() < 1e-9);
        assert!((m.get(2, 3) - expected[2]).abs() < 1e-9);
    }

    #[test]
    fn evaluate_face_centroid_lies_inside_box_bbox() {
        let (model, _sid) = box_with_seeded_datums();
        let source = DatumSource::FaceCentroid { face: 0 };
        let m = model
            .evaluate_datum_source(&source)
            .expect("face centroid evaluates");
        let x = m.get(0, 3);
        let y = m.get(1, 3);
        let z = m.get(2, 3);
        assert!(
            x.abs() <= 1.0 + 1e-9 && y.abs() <= 1.0 + 1e-9 && z.abs() <= 1.0 + 1e-9,
            "centroid {:?} outside box bbox",
            (x, y, z)
        );
    }

    #[test]
    fn evaluate_edge_axis_returns_unit_direction() {
        let (model, _sid) = box_with_seeded_datums();
        let source = DatumSource::EdgeAxis { edge: 0 };
        let m = model
            .evaluate_datum_source(&source)
            .expect("edge axis evaluates");
        let nz = (m.get(0, 2).powi(2) + m.get(1, 2).powi(2) + m.get(2, 2).powi(2)).sqrt();
        assert!((nz - 1.0).abs() < 1e-9);
    }

    #[test]
    fn evaluate_plane_from_face_returns_unit_normal() {
        let (model, _sid) = box_with_seeded_datums();
        let source = DatumSource::PlaneFromFace { face: 0 };
        let m = model
            .evaluate_datum_source(&source)
            .expect("plane from face evaluates");
        let nz = (m.get(0, 2).powi(2) + m.get(1, 2).powi(2) + m.get(2, 2).powi(2)).sqrt();
        assert!((nz - 1.0).abs() < 1e-9);
    }

    #[test]
    fn create_derived_datum_records_event_and_uses_default_kind() {
        let mut model = BRepModel::new();
        model.datums.seed_defaults();
        let recorder = Arc::new(CaptureRecorder::default());
        model.attach_recorder(Some(recorder.clone()));

        let source = DatumSource::OffsetPlane {
            base: 1,
            distance: 7.0,
        };
        let id = model
            .create_derived_datum("Offset7".to_string(), source)
            .expect("create_derived succeeds");
        let datum = model.datums.get(id).expect("datum present");
        assert_eq!(datum.name, "Offset7");
        assert!(!datum.is_default);
        assert!(matches!(
            datum.kind,
            DatumKind::Plane(crate::sketch2d::sketch_plane::PlaneOrientation::Custom)
        ));
        match datum.source {
            DatumSource::OffsetPlane { base, distance } => {
                assert_eq!(base, 1);
                assert!((distance - 7.0).abs() < 1e-12);
            }
            other => panic!("expected OffsetPlane, got {:?}", other),
        }

        let events = recorder
            .events
            .lock()
            .expect("CaptureRecorder mutex poisoned")
            .clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "datum_create_derived");
        assert_eq!(events[0].outputs, vec![format!("datum:{}", id)]);
        assert_eq!(events[0].parameters["name"], "Offset7");
    }

    #[test]
    fn create_derived_datum_axis_uses_axis_custom_kind() {
        let (model, _sid) = box_with_seeded_datums();
        let p0 = vertex_at(&model, [-1.0, -1.0, -1.0]);
        let p1 = vertex_at(&model, [1.0, -1.0, -1.0]);
        let id = model
            .create_derived_datum(
                "EdgeAxis".to_string(),
                DatumSource::TwoPointsAxis { p0, p1 },
            )
            .expect("create_derived succeeds");
        let datum = model.datums.get(id).expect("datum present");
        assert!(matches!(datum.kind, DatumKind::Axis(AxisDirection::Custom)));
    }

    #[test]
    fn create_derived_datum_point_uses_origin_kind() {
        let (model, _sid) = box_with_seeded_datums();
        let p = vertex_at(&model, [1.0, 1.0, 1.0]);
        let id = model
            .create_derived_datum("Corner".to_string(), DatumSource::VertexPoint { vertex: p })
            .expect("create_derived succeeds");
        let datum = model.datums.get(id).expect("datum present");
        assert!(matches!(datum.kind, DatumKind::Origin));
    }

    #[test]
    fn create_derived_datum_propagates_evaluation_error() {
        let model = BRepModel::new();
        model.datums.seed_defaults();
        let source = DatumSource::VertexPoint {
            vertex: u32::MAX - 1,
        };
        let err = model
            .create_derived_datum("Bad".to_string(), source)
            .expect_err("unknown vertex");
        assert!(matches!(
            err,
            DatumError::UnknownReference { kind: "vertex", .. }
        ));
    }

    #[test]
    fn create_derived_datum_rejects_empty_name() {
        let model = BRepModel::new();
        model.datums.seed_defaults();
        let source = DatumSource::OffsetPlane {
            base: 1,
            distance: 1.0,
        };
        let err = model
            .create_derived_datum("".to_string(), source)
            .expect_err("empty name");
        assert!(matches!(err, DatumError::EmptyName));
    }

    // ──────────────────── Slice 5: propagation graph + cache ────────────────

    #[test]
    fn create_derived_datum_registers_parent_datum_edges() {
        let model = BRepModel::new();
        model.datums.seed_defaults();
        // OffsetPlane has one parent: base = 1 (FrontPlane).
        let _id = model
            .create_derived_datum(
                "Off".to_string(),
                DatumSource::OffsetPlane {
                    base: 1,
                    distance: 5.0,
                },
            )
            .expect("offset plane");
        assert_eq!(
            model.datum_graph.datum_edge_count(),
            1,
            "OffsetPlane registers 1 edge from FrontPlane"
        );
        let deps = model.datum_graph.datums_dependent_on_datum(1);
        assert_eq!(deps.len(), 1, "FrontPlane has one dependent");
    }

    #[test]
    fn create_derived_datum_registers_all_parents_for_angle_plane() {
        let model = BRepModel::new();
        model.datums.seed_defaults();
        // AnglePlane has two datum parents: base + axis.
        let _id = model
            .create_derived_datum(
                "Tilted".to_string(),
                DatumSource::AnglePlane {
                    base: 1,
                    axis: 4,
                    angle: 0.5,
                },
            )
            .expect("angle plane");
        assert_eq!(model.datum_graph.datum_edge_count(), 2);
        assert_eq!(model.datum_graph.datums_dependent_on_datum(1).len(), 1);
        assert_eq!(model.datum_graph.datums_dependent_on_datum(4).len(), 1);
    }

    #[test]
    fn create_derived_datum_registers_geometry_references() {
        let (model, _sid) = box_with_seeded_datums();
        // 2×2×2 box is centered at origin, so corners are at ±1.0.
        let v0 = vertex_at(&model, [-1.0, -1.0, -1.0]);
        let v1 = vertex_at(&model, [1.0, -1.0, -1.0]);
        let v2 = vertex_at(&model, [-1.0, 1.0, -1.0]);
        let _id = model
            .create_derived_datum(
                "ThreePts".to_string(),
                DatumSource::ThreePoints {
                    p0: v0,
                    p1: v1,
                    p2: v2,
                },
            )
            .expect("three points plane");
        assert_eq!(
            model.datum_graph.datums_dependent_on_vertex(v0).len(),
            1,
            "vertex v0 has one dependent datum"
        );
        assert_eq!(model.datum_graph.datums_dependent_on_vertex(v1).len(), 1);
        assert_eq!(model.datum_graph.datums_dependent_on_vertex(v2).len(), 1);
    }

    #[test]
    fn delete_datum_removes_dependency_edges() {
        let model = BRepModel::new();
        model.datums.seed_defaults();
        let derived = model
            .create_derived_datum(
                "Off".to_string(),
                DatumSource::OffsetPlane {
                    base: 1,
                    distance: 5.0,
                },
            )
            .expect("offset plane");
        assert_eq!(model.datum_graph.datum_edge_count(), 1);
        model.delete_datum(derived).expect("delete");
        assert_eq!(
            model.datum_graph.datum_edge_count(),
            0,
            "deleting the dependent drops the (parent → dependent) edge"
        );
    }

    #[test]
    fn set_datum_transform_propagates_to_offset_plane_dependent() {
        let model = BRepModel::new();
        model.datums.seed_defaults();
        // Create a custom plane base (id 7) we are allowed to move
        // (defaults refuse `set_transform`).
        let base = model
            .create_datum_plane(
                "MovableBase".to_string(),
                Matrix4::from_translation(&Vector3::new(0.0, 0.0, 0.0)),
            )
            .expect("base plane");
        let derived = model
            .create_derived_datum(
                "Off10".to_string(),
                DatumSource::OffsetPlane {
                    base,
                    distance: 10.0,
                },
            )
            .expect("offset plane");
        // Initial: derived plane sits at z = 10 (base at origin + 10
        // along base +Z).
        let before = model.datums.get(derived).expect("derived").transform;
        assert!((before.get(2, 3) - 10.0).abs() < 1e-9);
        // Move base by +Z = 5. Expected: derived now at z = 15.
        model
            .set_datum_transform(
                base,
                Matrix4::from_translation(&Vector3::new(0.0, 0.0, 5.0)),
            )
            .expect("move base");
        let after = model.datums.get(derived).expect("derived").transform;
        assert!(
            (after.get(2, 3) - 15.0).abs() < 1e-9,
            "derived datum followed parent: expected z=15, got {}",
            after.get(2, 3)
        );
    }

    #[test]
    fn propagate_walks_multi_level_chain() {
        let model = BRepModel::new();
        model.datums.seed_defaults();
        let a = model
            .create_datum_plane("A".to_string(), Matrix4::identity())
            .expect("A");
        let b = model
            .create_derived_datum(
                "B".to_string(),
                DatumSource::OffsetPlane {
                    base: a,
                    distance: 1.0,
                },
            )
            .expect("B");
        let c = model
            .create_derived_datum(
                "C".to_string(),
                DatumSource::OffsetPlane {
                    base: b,
                    distance: 2.0,
                },
            )
            .expect("C");
        // Initial chain offsets: B at z=1, C at z=3.
        assert!((model.datums.get(c).expect("C").transform.get(2, 3) - 3.0).abs() < 1e-9);
        // Move A by +Z=10. Expected: B at z=11, C at z=13.
        model
            .set_datum_transform(a, Matrix4::from_translation(&Vector3::new(0.0, 0.0, 10.0)))
            .expect("move A");
        let b_after = model.datums.get(b).expect("B").transform;
        let c_after = model.datums.get(c).expect("C").transform;
        assert!((b_after.get(2, 3) - 11.0).abs() < 1e-9);
        assert!((c_after.get(2, 3) - 13.0).abs() < 1e-9);
    }

    #[test]
    fn propagate_terminates_on_cycle() {
        // The graph cannot organically contain cycles (a derived
        // source is fixed at creation and can only reference earlier
        // datums by id), but if a future op or a malformed timeline
        // replay produced one, propagate must not hang.
        let model = BRepModel::new();
        model.datums.seed_defaults();
        let a = model
            .create_datum_plane("A".to_string(), Matrix4::identity())
            .expect("A");
        let b = model
            .create_derived_datum(
                "B".to_string(),
                DatumSource::OffsetPlane {
                    base: a,
                    distance: 1.0,
                },
            )
            .expect("B");
        // Manually inject the back-edge B → A so the graph reads
        // "moving A invalidates B; moving B invalidates A". The
        // visited-set guard inside propagate_datum_change keeps the
        // walk finite.
        model.datum_graph.register_solid_anchor(0, b); // unrelated edge to keep graph non-empty
                                                       // Use the public push helper indirectly via a fake source —
                                                       // we just need a (b → a) edge in datum_to_datums:
        let fake_source = DatumSource::OffsetPlane {
            base: b,
            distance: 0.0,
        };
        model.datum_graph.register_source(a, &fake_source);
        // If the cycle wasn't detected, this would loop until stack
        // overflow / hang the test. Bound the test by panicking via
        // a thread join in a real harness; here the test just
        // returning is the success condition (cargo test default
        // timeout is on the order of seconds).
        model
            .set_datum_transform(a, Matrix4::identity())
            .expect("move A");
    }

    #[test]
    fn set_datum_transform_severs_derived_link() {
        let model = BRepModel::new();
        model.datums.seed_defaults();
        let base = model
            .create_datum_plane("Base".to_string(), Matrix4::identity())
            .expect("base");
        let derived = model
            .create_derived_datum(
                "Off".to_string(),
                DatumSource::OffsetPlane {
                    base,
                    distance: 5.0,
                },
            )
            .expect("derived");
        assert_eq!(model.datum_graph.datums_dependent_on_datum(base).len(), 1);
        // User pins the derived datum with an explicit transform —
        // this overrides the source to Manual and severs the link.
        model
            .set_datum_transform(
                derived,
                Matrix4::from_translation(&Vector3::new(99.0, 0.0, 0.0)),
            )
            .expect("pin");
        assert_eq!(
            model.datum_graph.datums_dependent_on_datum(base).len(),
            0,
            "derived datum no longer listed as dependent of base"
        );
        // Moving base now leaves the pinned datum untouched.
        model
            .set_datum_transform(
                base,
                Matrix4::from_translation(&Vector3::new(0.0, 0.0, 50.0)),
            )
            .expect("move base");
        let pinned = model.datums.get(derived).expect("derived").transform;
        assert!((pinned.get(0, 3) - 99.0).abs() < 1e-9);
        assert!(pinned.get(2, 3).abs() < 1e-9);
    }

    // ─────────────────── Slice 5: LocationDescriptor cache ──────────────────

    #[test]
    fn cached_descriptor_matches_uncached_first_read() {
        let (model, sid) = box_with_seeded_datums();
        assert!(model.location_cache.is_empty());
        let cached = model
            .solid_location_descriptor_cached(sid)
            .expect("descriptor");
        assert_eq!(model.location_cache.len(), 1, "cache populated on miss");
        let direct = model.solid_location_descriptor(sid).expect("uncached");
        assert_eq!(cached.dimensions_world, direct.dimensions_world);
        assert_eq!(cached.center_world, direct.center_world);
        // Second read hits the cache and returns identical bytes.
        let again = model.solid_location_descriptor_cached(sid).expect("hit");
        assert_eq!(again.dimensions_world, cached.dimensions_world);
    }

    #[test]
    fn cache_invalidates_on_transform_solid() {
        let (mut model, sid) = box_with_seeded_datums();
        let _ = model
            .solid_location_descriptor_cached(sid)
            .expect("warm cache");
        assert_eq!(model.location_cache.len(), 1);
        let t = Matrix4::from_translation(&Vector3::new(7.0, 0.0, 0.0));
        crate::operations::transform::transform_solid(
            &mut model,
            sid,
            t,
            crate::operations::transform::TransformOptions::default(),
        )
        .expect("transform");
        assert_eq!(
            model.location_cache.len(),
            0,
            "transform_solid invalidates the cached descriptor"
        );
        // Subsequent read recomputes against the moved geometry.
        let after = model
            .solid_location_descriptor_cached(sid)
            .expect("post-transform");
        assert!((after.center_world[0] - 7.0).abs() < 1e-9);
    }

    #[test]
    fn cache_invalidates_on_anchor_reassignment() {
        let (mut model, sid) = box_with_seeded_datums();
        let _ = model.solid_location_descriptor_cached(sid).expect("warm");
        assert_eq!(model.location_cache.len(), 1);
        let custom = model
            .create_datum_plane(
                "Anchor".to_string(),
                Matrix4::from_translation(&Vector3::new(0.0, 0.0, 0.0)),
            )
            .expect("custom");
        {
            let mut builder = TopologyBuilder::new(&mut model);
            builder
                .anchor_solid(sid, custom, Matrix4::identity())
                .expect("anchor");
        }
        assert_eq!(
            model.location_cache.len(),
            0,
            "anchor_solid invalidates the cache"
        );
        let solid = model.solids.get(sid).expect("solid still present");
        assert_eq!(solid.anchor.datum_id, custom);
        // Graph edge is in place.
        assert!(model
            .datum_graph
            .solids_dependent_on_datum(custom)
            .contains(&sid));
    }

    #[test]
    fn set_datum_transform_invalidates_cache_for_anchored_solid() {
        let (mut model, sid) = box_with_seeded_datums();
        let custom = model
            .create_datum_plane("Anchor".to_string(), Matrix4::identity())
            .expect("custom");
        {
            let mut builder = TopologyBuilder::new(&mut model);
            builder
                .anchor_solid(sid, custom, Matrix4::identity())
                .expect("anchor");
        }
        let _ = model.solid_location_descriptor_cached(sid).expect("warm");
        assert_eq!(model.location_cache.len(), 1);
        model
            .set_datum_transform(
                custom,
                Matrix4::from_translation(&Vector3::new(3.0, 0.0, 0.0)),
            )
            .expect("move anchor datum");
        assert_eq!(
            model.location_cache.len(),
            0,
            "anchor datum move invalidates the descriptor cache"
        );
    }

    #[test]
    fn delete_datum_invalidates_cache_for_anchored_solid() {
        let (mut model, sid) = box_with_seeded_datums();
        let custom = model
            .create_datum_plane("Anchor".to_string(), Matrix4::identity())
            .expect("custom");
        {
            let mut builder = TopologyBuilder::new(&mut model);
            builder
                .anchor_solid(sid, custom, Matrix4::identity())
                .expect("anchor");
        }
        let _ = model.solid_location_descriptor_cached(sid).expect("warm");
        assert_eq!(model.location_cache.len(), 1);
        model.delete_datum(custom).expect("delete anchor datum");
        assert_eq!(
            model.location_cache.len(),
            0,
            "deleting the anchor datum invalidates the cache for dependents"
        );
    }

    #[test]
    fn vertex_changed_refreshes_dependent_datum() {
        let mut model = BRepModel::new();
        model.datums.seed_defaults();
        let _ = {
            let mut builder = TopologyBuilder::new(&mut model);
            builder
                .create_box_3d(2.0, 2.0, 2.0)
                .expect("box creation succeeds")
        };
        // 2×2×2 box centered at origin.
        let v = vertex_at(&model, [1.0, -1.0, -1.0]);
        let derived = model
            .create_derived_datum("Vp".to_string(), DatumSource::VertexPoint { vertex: v })
            .expect("vertex point");
        let before = model.datums.get(derived).expect("derived").transform;
        assert!((before.get(0, 3) - 1.0).abs() < 1e-9);
        // Move the vertex via the store, then notify.
        assert!(model.vertices.set_position(v, 9.0, 5.0, 0.0));
        model.vertex_changed(v);
        let after = model.datums.get(derived).expect("derived").transform;
        assert!((after.get(0, 3) - 9.0).abs() < 1e-9);
        assert!((after.get(1, 3) - 5.0).abs() < 1e-9);
    }
}

// Circle and Sphere implementations are in their respective modules
