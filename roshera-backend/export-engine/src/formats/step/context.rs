//! Mutable state shared across handler invocations during a STEP import.
//!
//! Each dispatcher pass walks the [`super::registry::EntityRegistry`]
//! and, for each entity, hands a handler the [`ImportContext`]:
//!
//! - the destination [`BRepModel`] under construction,
//! - the unit scale factor (1.0 for mm, 25.4 for inch — applied to
//!   every length-typed parameter as it is read),
//! - the per-import modelling tolerance (set by
//!   `UNCERTAINTY_MEASURE_WITH_UNIT`, defaulted to 1e-6 mm in absence),
//! - resolution caches that map source `#N` to the BRep ids minted
//!   by previous handler calls (so a `FACE_OUTER_BOUND` handler can
//!   look up the `EDGE_LOOP` already materialised by the loop
//!   handler),
//! - a resolution stack used for cycle detection by the lazy
//!   cross-phase resolver (handlers can recursively resolve their
//!   dependencies; the stack ensures pathological self-referential
//!   inputs are rejected instead of recursing to a stack overflow),
//! - the active [`ImportReport`] for accumulating warnings.
//!
//! `ImportContext` is single-threaded and owned by the importer
//! coroutine; handlers receive it as `&mut`.

use geometry_engine::primitives::{
    curve::CurveId, edge::EdgeId, face::FaceId, r#loop::LoopId, shell::ShellId, solid::SolidId,
    surface::SurfaceId, topology_builder::BRepModel, vertex::VertexId,
};
use std::collections::HashMap;

use crate::formats::step::diagnostics::ImportReport;

/// Scaling applied to length-typed parameters as they're read from the
/// source. mm files use `scale = 1.0`; inch files use `25.4`.
#[derive(Debug, Clone, Copy)]
pub struct UnitScale {
    /// Multiplier applied to every length-typed value read from the
    /// source. The kernel's canonical length unit is millimetre, so
    /// a STEP file declaring INCH yields `length = 25.4`.
    pub length: f64,
    /// Multiplier applied to every plane-angle value read from the
    /// source. The kernel's canonical angle unit is radian; STEP
    /// schemas overwhelmingly use radians too, so the default is 1.0.
    /// Files that declare degrees yield `π/180`.
    pub angle_radians_per_source: f64,
}

impl Default for UnitScale {
    fn default() -> Self {
        Self {
            length: 1.0,
            angle_radians_per_source: 1.0,
        }
    }
}

/// Parameterless line geometry: origin point + unit direction. Materialised
/// by the `LINE` handler in IMP2.3 and consumed by the `EDGE_CURVE`
/// handler in IMP2.4, which allocates a kernel curve sized to the
/// edge's vertex endpoints (a single STEP `LINE` can be shared by
/// multiple edges, each with different endpoints).
#[derive(Debug, Clone, Copy)]
pub struct StepLineGeom {
    /// `pnt` in `LINE(pnt, vec)` — millimetres after unit scaling.
    pub origin: [f64; 3],
    /// Unit direction extracted from `vec`. Magnitude is discarded
    /// because the STEP line is unbounded; the edge handler resizes.
    pub direction: [f64; 3],
}

/// Parameterless circle geometry: oriented placement + radius. Same
/// rationale as [`StepLineGeom`]: the edge handler chooses a
/// parameter range and constructs the kernel curve.
#[derive(Debug, Clone, Copy)]
pub struct StepCircleGeom {
    /// Right-handed frame whose `origin` is the circle centre, whose
    /// `z` is the circle plane normal, and whose `x` is the angular
    /// reference (parameter `t = 0` lands on `origin + radius * x`).
    pub placement: Axis2Placement,
    /// Radius in millimetres after unit scaling.
    pub radius: f64,
}

/// Orthonormal frame derived from a STEP `AXIS2_PLACEMENT_3D`.
///
/// `AXIS2_PLACEMENT_3D(location, axis, ref_direction)`:
///   - `location`     → `origin`
///   - `axis`         → `z` (placement normal, after normalization)
///   - `ref_direction → x` projected onto the plane normal to `z`,
///                      then normalised — this matches the spec's
///                      "ref_direction is projected into the plane
///                      perpendicular to axis".
///   - `y = z × x`    (right-handed)
///
/// The frame is materialised once at handler time and reused by every
/// downstream entity (`LINE`, `CIRCLE`, `PLANE`, `CYLINDRICAL_SURFACE`,
/// …) that references the same placement instance.
#[derive(Debug, Clone, Copy)]
pub struct Axis2Placement {
    /// Frame origin in model units.
    pub origin: [f64; 3],
    /// Z axis (placement normal). Unit length.
    pub z: [f64; 3],
    /// X axis (reference direction projected into z-perpendicular
    /// plane). Unit length.
    pub x: [f64; 3],
    /// Y axis = z × x. Unit length.
    pub y: [f64; 3],
}

/// Bundles every cache the dispatcher maintains, keyed by source `#N`.
///
/// Each map is sparse: only entities the dispatcher has actually
/// resolved appear. A handler that follows a reference into a yet-
/// unresolved entity must trigger a recursive resolve via
/// `super::handlers::tier1::resolver` (which mutates these caches
/// as it walks).
#[derive(Debug, Default)]
pub struct ResolutionCaches {
    /// Source `#N` → 3-D point (model units, after `unit.length` scaling).
    pub points: HashMap<u64, [f64; 3]>,
    /// Source `#N` → 3-D direction (unit length, dimensionless).
    pub directions: HashMap<u64, [f64; 3]>,
    /// Source `#N` → 3-D vector (direction × magnitude, model units).
    pub vectors: HashMap<u64, [f64; 3]>,
    /// Source `#N` → orthonormal frame (origin + axes).
    pub placements: HashMap<u64, Axis2Placement>,
    /// Source `#N` → kernel vertex id.
    pub vertices: HashMap<u64, VertexId>,
    /// Source `#N` → kernel curve id (analytic or NURBS).
    pub curves: HashMap<u64, CurveId>,
    /// Source `#N` → kernel surface id.
    pub surfaces: HashMap<u64, SurfaceId>,
    /// Source `#N` → kernel edge id.
    pub edges: HashMap<u64, EdgeId>,
    /// Source `#N` → `(edge_id, forward)` pair for STEP `ORIENTED_EDGE`.
    /// Oriented edges don't allocate a new `Edge` in the kernel —
    /// they record an orientation flag against an existing edge.
    pub oriented_edges: HashMap<u64, (EdgeId, bool)>,
    /// Source `#N` → kernel loop id.
    pub loops: HashMap<u64, LoopId>,
    /// Source `#N` → `(loop_id, is_outer, forward)` triple for
    /// STEP `FACE_BOUND` / `FACE_OUTER_BOUND`.
    pub face_bounds: HashMap<u64, (LoopId, bool, bool)>,
    /// Source `#N` → kernel face id.
    pub faces: HashMap<u64, FaceId>,
    /// Source `#N` → kernel shell id.
    pub shells: HashMap<u64, ShellId>,
    /// Source `#N` → kernel solid id.
    pub solids: HashMap<u64, SolidId>,
    /// Source `#N` of a root representation (`SHAPE_REPRESENTATION` /
    /// `ADVANCED_BREP_SHAPE_REPRESENTATION`) → the kernel solid ids
    /// produced by its items list. Populated by the root handlers in
    /// [`super::handlers::tier1::root`]. A non-empty entry per root
    /// is what drives `ImportReport::ok = true` after dispatch.
    pub roots: HashMap<u64, Vec<SolidId>>,
    /// Source `#N` → millimetres-per-source-unit for a length-typed unit
    /// entity (`SI_UNIT(.MILLI.,.METRE.)`, `CONVERSION_BASED_UNIT('INCH',…)`
    /// or any complex carrying `LENGTH_UNIT`). Resolved during the
    /// `Unit` phase by [`super::handlers::tier1::units`].
    pub length_units: HashMap<u64, f64>,
    /// Source `#N` → radians-per-source-unit for a plane-angle-typed
    /// unit (`SI_UNIT($,.RADIAN.)`, `CONVERSION_BASED_UNIT('DEGREE',…)`).
    pub angle_units: HashMap<u64, f64>,
    /// Source `#N` → steradians-per-source-unit for a solid-angle-typed
    /// unit. Almost always `STERADIAN` (= 1.0); recorded for
    /// completeness so downstream measure entities can resolve.
    pub solid_angle_units: HashMap<u64, f64>,
    /// Source `#N` → `LINE(pnt, vec)` geometry (origin + unit
    /// direction). Consumed by the `EDGE_CURVE` handler in IMP2.4 to
    /// allocate a kernel `Line` sized to each edge's vertex
    /// endpoints. Separate from [`Self::curves`] because the same
    /// STEP `LINE` instance can be shared by multiple edges with
    /// different domains.
    pub step_lines: HashMap<u64, StepLineGeom>,
    /// Source `#N` → `CIRCLE` geometry (placement + radius). Same
    /// rationale as [`Self::step_lines`].
    pub step_circles: HashMap<u64, StepCircleGeom>,
}

/// Sentinel inserted into [`ResolutionCaches`] keys by the lazy
/// resolver while a handler is mid-resolve, used purely for the
/// cycle-detection stack below. (Not stored — we use
/// [`ImportContext::resolution_stack`] for that.)
///
/// Per-import mutable state.
pub struct ImportContext<'a> {
    /// Destination BRep — handlers append to its stores as they
    /// resolve entities.
    pub model: &'a mut BRepModel,
    /// Unit scaling derived from `GLOBAL_UNIT_ASSIGNED_CONTEXT`.
    pub unit: UnitScale,
    /// Per-import modelling tolerance. Set by
    /// `UNCERTAINTY_MEASURE_WITH_UNIT` (after unit scaling) or
    /// defaulted to 1e-6 mm when the file omits it.
    pub default_tolerance: f64,
    /// Lookup maps from source `#N` to BRep ids.
    pub caches: ResolutionCaches,
    /// Cycle-detection stack for the lazy cross-phase resolver. A
    /// handler that recursively resolves a dependency pushes the
    /// dependency's instance number on entry and pops on exit; if
    /// an instance is already on the stack the cycle is reported
    /// as a structured warning instead of recursing to overflow.
    pub resolution_stack: Vec<u64>,
    /// Diagnostics accumulator.
    pub report: &'a mut ImportReport,
}

impl<'a> ImportContext<'a> {
    /// Construct a new context bound to `model` and `report`. Caches
    /// start empty; unit defaults to mm; tolerance defaults to 1e-6.
    pub fn new(model: &'a mut BRepModel, report: &'a mut ImportReport) -> Self {
        Self {
            model,
            unit: UnitScale::default(),
            default_tolerance: 1e-6,
            caches: ResolutionCaches::default(),
            resolution_stack: Vec::with_capacity(16),
            report,
        }
    }

    /// `true` when `instance` is currently being resolved further up
    /// the call stack — used by the lazy resolver to break cycles.
    pub fn is_resolving(&self, instance: u64) -> bool {
        self.resolution_stack.iter().any(|&i| i == instance)
    }
}
