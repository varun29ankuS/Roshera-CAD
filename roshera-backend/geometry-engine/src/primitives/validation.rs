//! B-Rep model validation utilities.
//!
//! Features:
//! - Multi-threaded validation with parallel checking
//! - Progressive validation levels (Quick, Standard, Deep)
//! - Self-healing suggestions and automatic repair
//! - Manufacturing-constraint validation
//! - Tolerance stack-up analysis
//! - Feature-recognition validation
//! - Assembly-constraint checking
//! - Performance profiling and optimization hints
//!
//! Indexed access into topology enumeration arrays is the canonical idiom —
//! bounded by topology length and validation buffer sizes. Matches the pattern
//! used in nurbs.rs and other Rust numerical kernels.
#![allow(clippy::indexing_slicing)]

use crate::math::{MathError, MathResult, Point3, Tolerance};
use crate::primitives::{
    edge::EdgeId,
    face::{FaceId, FaceOrientation},
    r#loop::LoopId,
    shell::ShellId,
    solid::{FeatureType, Solid, SolidId},
    topology_builder::BRepModel,
    vertex::VertexId,
};
use dashmap::DashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Validation level
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ValidationLevel {
    /// Quick checks only (topology connectivity)
    Quick,
    /// Standard validation (topology + basic geometry)
    Standard,
    /// Deep validation (all checks including numerical)
    Deep,
}

/// Validation context for per-phase timing capture during validation runs.
///
/// Phase durations are populated by [`ValidationContext::record_phase`] from
/// inside [`validate_model_enhanced`]; consumers may read `phase_times` to
/// surface per-phase timings to operators. Held by reference inside
/// [`ValidationResult::context`].
#[derive(Debug, Default)]
pub struct ValidationContext {
    /// Time spent in each phase, keyed by phase name.
    pub phase_times: DashMap<String, Duration>,
}

impl ValidationContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_phase(&mut self, phase: &str, duration: Duration) {
        self.phase_times.insert(phase.to_string(), duration);
    }
}

/// Enhanced validation result with repair suggestions
#[derive(Debug)]
pub struct ValidationResult {
    /// Overall validity
    pub is_valid: bool,
    /// Topological validity
    pub topology_valid: bool,
    /// Geometric validity  
    pub geometry_valid: bool,
    /// Manufacturing validity
    pub manufacturing_valid: bool,
    /// Detailed error messages
    pub errors: Vec<ValidationError>,
    /// Warning messages
    pub warnings: Vec<ValidationWarning>,
    /// Repair suggestions
    pub repairs: Vec<RepairSuggestion>,
    /// Statistics about the model
    pub statistics: ModelStatistics,
    /// Performance context
    pub context: ValidationContext,
    /// Validation certificate (if valid)
    pub certificate: Option<ValidationCertificate>,
}

/// Validation error types (enhanced)
#[derive(Debug, Clone)]
pub enum ValidationError {
    /// Missing entity
    MissingEntity { entity_type: String, id: u32 },
    /// Topology error
    TopologyError {
        message: String,
        location: EntityLocation,
    },
    /// Geometry error
    GeometryError {
        message: String,
        location: EntityLocation,
    },
    /// Orientation error
    OrientationError {
        message: String,
        location: EntityLocation,
    },
    /// Connectivity error
    ConnectivityError {
        message: String,
        location: EntityLocation,
    },
    /// Manufacturing constraint violation
    ManufacturingError {
        message: String,
        constraint: ManufacturingConstraint,
    },
    /// Tolerance stack-up error
    ToleranceError {
        message: String,
        accumulated: f64,
        allowed: f64,
    },
    /// Feature validity error
    FeatureError { message: String, feature_id: u32 },
    /// Assembly constraint error
    AssemblyError {
        message: String,
        components: Vec<u32>,
    },
}

impl ValidationError {
    /// The solid this error is attributed to, when the variant carries an
    /// [`EntityLocation`]. Returns `None` for model-global or
    /// unattributed variants (`MissingEntity`, `ManufacturingError`,
    /// `ToleranceError`, `FeatureError`, `AssemblyError`).
    ///
    /// Used by operation post-validation to scope a verdict to the solid
    /// the op actually touched (see [`validate_solid_scoped`]).
    pub fn solid_id(&self) -> Option<SolidId> {
        match self {
            ValidationError::TopologyError { location, .. }
            | ValidationError::GeometryError { location, .. }
            | ValidationError::OrientationError { location, .. }
            | ValidationError::ConnectivityError { location, .. } => location.solid_id,
            ValidationError::MissingEntity { .. }
            | ValidationError::ManufacturingError { .. }
            | ValidationError::ToleranceError { .. }
            | ValidationError::FeatureError { .. }
            | ValidationError::AssemblyError { .. } => None,
        }
    }
}

/// Entity location for precise error reporting
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EntityLocation {
    pub solid_id: Option<SolidId>,
    pub shell_id: Option<ShellId>,
    pub face_id: Option<FaceId>,
    pub loop_id: Option<LoopId>,
    pub edge_id: Option<EdgeId>,
    pub vertex_id: Option<VertexId>,
}

/// Manufacturing constraints
#[derive(Debug, Clone)]
pub enum ManufacturingConstraint {
    MinimumWallThickness(f64),
    MinimumFeatureSize(f64),
    MaximumAspectRatio(f64),
    MinimumDraftAngle(f64),
    MaximumUndercut(f64),
    ToolAccessibility,
    SurfaceFinish(f64),
}

/// Repair suggestion
#[derive(Debug, Clone)]
pub struct RepairSuggestion {
    /// Problem description
    pub problem: String,
    /// Suggested repair action
    pub action: RepairAction,
    /// Confidence level (0-1)
    pub confidence: f64,
    /// Estimated time to repair
    pub estimated_time_ms: u32,
}

/// Repair actions
#[derive(Debug, Clone)]
pub enum RepairAction {
    /// Merge vertices within tolerance
    MergeVertices {
        v1: VertexId,
        v2: VertexId,
        distance: f64,
    },
    /// Fix face orientation
    FlipFaceOrientation { face_id: FaceId },
}

/// Validation warning types (enhanced)
#[derive(Debug, Clone)]
pub enum ValidationWarning {
    /// Near-degenerate geometry
    NearDegenerate {
        entity_type: String,
        id: u32,
        measure: f64,
    },
    /// Sharp angle
    SharpAngle {
        location: EntityLocation,
        angle: f64,
    },
    /// Large aspect ratio
    LargeAspectRatio {
        entity_type: String,
        id: u32,
        ratio: f64,
    },
    /// Near-coincident entities
    NearCoincident {
        entity1: EntityLocation,
        entity2: EntityLocation,
        distance: f64,
    },
    /// Tolerance accumulation risk
    ToleranceRisk {
        location: EntityLocation,
        accumulated: f64,
    },
    /// An edge/vertex does not lie on its face's surface within tolerance — a
    /// geometric inconsistency (B1 consistency check). Emitted as a WARNING for
    /// now: it surfaces genuine but pre-existing op defects (e.g. fillet/chamfer
    /// trim edges that sit slightly off a planar face), and promoting it to a
    /// blocking error would break those ops until the trim geometry is fixed.
    /// `geometry_valid` is set false when any of these are present.
    GeometryInconsistency {
        location: EntityLocation,
        distance: f64,
        message: String,
    },
}

/// Enhanced model statistics
#[derive(Debug, Default)]
pub struct ModelStatistics {
    // Basic counts
    pub num_solids: usize,
    pub num_shells: usize,
    pub num_faces: usize,
    pub num_loops: usize,
    pub num_edges: usize,
    pub num_vertices: usize,
    pub num_curves: usize,
    pub num_surfaces: usize,
    // Topology stats
    pub num_manifold_edges: usize,
    pub num_non_manifold_edges: usize,
    pub num_boundary_edges: usize,
    pub num_laminar_edges: usize,
    pub euler_characteristic: i32,
    pub genus: i32,
    // Geometry stats
    pub total_volume: Option<f64>,
    pub total_surface_area: Option<f64>,
    pub bounding_box: Option<(Point3, Point3)>,
    pub center_of_mass: Option<Point3>,
    // Quality metrics
    pub min_edge_length: Option<f64>,
    pub max_edge_length: Option<f64>,
    pub min_face_area: Option<f64>,
    pub max_face_area: Option<f64>,
    pub aspect_ratio_stats: AspectRatioStats,
    // Feature stats
    pub num_features: usize,
    pub feature_types: DashMap<FeatureType, usize>,
}

/// Aspect ratio statistics
#[derive(Debug, Default)]
pub struct AspectRatioStats {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub std_dev: f64,
}

/// Validation certificate for valid models
#[derive(Debug, Clone)]
pub struct ValidationCertificate {
    /// Unique certificate ID
    pub id: String,
    /// Validation timestamp
    pub timestamp: std::time::SystemTime,
    /// Validation level
    pub level: ValidationLevel,
    /// Model hash
    pub model_hash: u64,
    /// Validator version
    pub validator_version: String,
    /// Digital signature (SHA256 hash of model data)
    pub signature: Vec<u8>,
}

/// Edge usage tracking (enhanced)
#[derive(Debug, Clone)]
struct EdgeUsage {
    /// Faces using this edge
    pub faces: Vec<FaceId>,
    /// Loops using this edge
    pub loops: Vec<LoopId>,
    /// Orientations in each use
    pub orientations: Vec<bool>,
}

/// Multi-threaded validator
pub struct ParallelValidator {
    progress: Arc<Mutex<ValidationProgress>>,
}

/// Validation progress tracking
#[derive(Debug, Default)]
struct ValidationProgress {
    pub current_phase: String,
    pub items_processed: usize,
    pub total_items: usize,
}

impl Default for ParallelValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl ParallelValidator {
    pub fn new() -> Self {
        Self {
            progress: Arc::new(Mutex::new(ValidationProgress::default())),
        }
    }

    pub fn validate_model(
        &self,
        model: &BRepModel,
        tolerance: Tolerance,
        level: ValidationLevel,
    ) -> ValidationResult {
        let mut context = ValidationContext::new();
        let phase_start = Instant::now();

        // Phase 1: Parallel topology validation
        self.update_progress("Topology Validation", 0, model.solids.len());
        let topology_results = self.validate_topology_parallel(model, tolerance);
        context.record_phase("topology", phase_start.elapsed());

        // Phase 2: Parallel geometry validation (if needed)
        let geometry_results = if level >= ValidationLevel::Standard {
            let phase_start = Instant::now();
            self.update_progress("Geometry Validation", 0, model.faces.len());
            let results = self.validate_geometry_parallel(model, tolerance);
            context.record_phase("geometry", phase_start.elapsed());
            results
        } else {
            GeometryValidationResults::default()
        };

        // Phase 3: Deep validation (if needed)
        let deep_results = if level == ValidationLevel::Deep {
            let phase_start = Instant::now();
            self.update_progress("Deep Analysis", 0, model.edges.len());
            let results = self.validate_deep_parallel(model, tolerance);
            context.record_phase("deep", phase_start.elapsed());
            results
        } else {
            DeepValidationResults
        };

        // Combine results
        self.combine_results(
            topology_results,
            geometry_results,
            deep_results,
            context,
            level,
        )
    }

    fn update_progress(&self, phase: &str, current: usize, total: usize) {
        if let Ok(mut progress) = self.progress.lock() {
            progress.current_phase = phase.to_string();
            progress.items_processed = current;
            progress.total_items = total;
        }
    }

    fn validate_topology_parallel(
        &self,
        model: &BRepModel,
        tolerance: Tolerance,
    ) -> TopologyValidationResults {
        use rayon::prelude::*;

        // Validate solids in parallel. Ids are STABLE (holes after
        // deletion) — collect the real id set first; `0..len()` is not
        // the id range.
        let solid_ids: Vec<u32> = model.solids.iter().map(|(id, _)| id).collect();
        let solid_results: Vec<_> = solid_ids
            .into_par_iter()
            .filter_map(|id| {
                model.solids.get(id).map(|solid| {
                    // Validate single solid
                    let mut errors = Vec::new();
                    let mut warnings = Vec::new();

                    // Check solid has shells
                    if solid.outer_shell == crate::primitives::shell::INVALID_SHELL_ID {
                        errors.push(ValidationError::TopologyError {
                            message: "Solid has no shells".to_string(),
                            location: EntityLocation {
                                solid_id: Some(id),
                                shell_id: None,
                                face_id: None,
                                loop_id: None,
                                edge_id: None,
                                vertex_id: None,
                            },
                        });
                    }

                    // Validate Euler characteristic for the solid
                    self.validate_euler_characteristic_for_solid(
                        model,
                        id,
                        solid,
                        &mut errors,
                        &mut warnings,
                    );

                    // Check for manifold edges in the solid
                    self.check_manifold_edges_for_solid(
                        model,
                        id,
                        solid,
                        &mut errors,
                        &mut warnings,
                    );

                    let validation = ValidationResult {
                        is_valid: errors.is_empty(),
                        topology_valid: errors.is_empty(),
                        geometry_valid: true,
                        manufacturing_valid: true,
                        errors,
                        warnings,
                        repairs: Vec::new(),
                        statistics: ModelStatistics::default(),
                        context: ValidationContext::default(),
                        certificate: None,
                    };
                    (id, validation)
                })
            })
            .collect();

        // Build edge usage map in parallel
        let edge_usage = self.analyze_edge_usage_parallel(model);

        // Check for gaps in the model
        let gap_errors = self.check_topology_gaps(model, &edge_usage, tolerance);

        TopologyValidationResults {
            solid_results,
            gap_errors,
        }
    }

    fn validate_geometry_parallel(
        &self,
        model: &BRepModel,
        tolerance: Tolerance,
    ) -> GeometryValidationResults {
        use crate::primitives::surface::{Cone, Cylinder, Plane, Sphere, Torus};
        use rayon::prelude::*;

        // GEOMETRIC CONSISTENCY (B1 moat, slice 1a): every edge of a face must
        // actually lie ON that face's surface — endpoints and interior curve
        // samples. This catches geometry that is topologically well-formed but
        // geometrically broken (an edge floating off its face, an orphaned
        // sketch), which the old stub waved through with `geometry_valid: true`.
        //
        // Gated to ANALYTIC surfaces only (Plane/Cylinder/Cone/Sphere/Torus),
        // where `contains_point` uses an exact `closest_point`. NURBS / ruled /
        // revolution surfaces use an iterative `closest_point` that can
        // false-negative at seams, so checking them this way would WRONGLY fail
        // valid curved geometry — those get a direct (u,v)-sampling slice next.
        let face_ids: Vec<FaceId> = (0..model.faces.len() as u32).collect();
        let warnings: Vec<ValidationWarning> = face_ids
            .par_iter()
            .flat_map(|&face_id| {
                let mut warns: Vec<ValidationWarning> = Vec::new();
                let face = match model.faces.get(face_id) {
                    Some(f) => f,
                    None => return warns,
                };
                let surface = match model.surfaces.get(face.surface_id) {
                    Some(s) => s,
                    None => return warns,
                };

                // 1c — DEGENERATE FACE: an outer loop that collapses to ~a point
                // (zero spatial extent) is a face with no area, a real defect.
                let mut loop_pts: Vec<Point3> = Vec::new();
                if let Some(lp) = model.loops.get(face.outer_loop) {
                    for &eid in &lp.edges {
                        if let Some(e) = model.edges.get(eid) {
                            if let Some(v) = model.vertices.get(e.start_vertex) {
                                loop_pts.push(v.point());
                            }
                        }
                    }
                }
                if loop_pts.len() >= 3 {
                    let mut mn = loop_pts[0];
                    let mut mx = loop_pts[0];
                    for p in &loop_pts {
                        mn = Point3::new(mn.x.min(p.x), mn.y.min(p.y), mn.z.min(p.z));
                        mx = Point3::new(mx.x.max(p.x), mx.y.max(p.y), mx.z.max(p.z));
                    }
                    let diag = mn.distance(&mx);
                    if diag < tolerance.distance() {
                        warns.push(ValidationWarning::GeometryInconsistency {
                            location: EntityLocation {
                                solid_id: None,
                                shell_id: None,
                                face_id: Some(face_id),
                                loop_id: Some(face.outer_loop),
                                edge_id: None,
                                vertex_id: None,
                            },
                            distance: diag,
                            message: format!(
                                "face {face_id} is degenerate: outer loop spans only {diag:.3e}"
                            ),
                        });
                    }
                }

                let any = surface.as_any();
                let analytic = any.is::<Plane>()
                    || any.is::<Cylinder>()
                    || any.is::<Cone>()
                    || any.is::<Sphere>()
                    || any.is::<Torus>();
                if !analytic {
                    // 1b — CURVED SURFACE (NURBS / ruled / revolution / offset).
                    // closest_point is iterative and false-negatives at seams, so
                    // we use a (u,v) GRID UPPER BOUND: the min distance from an
                    // edge sample to any grid point is an upper bound on the true
                    // distance, and we warn only when it exceeds a few grid cells.
                    // Coarse (catches gross edge-off-surface errors only) but it
                    // CANNOT false-positive on grid resolution. A finer pcurve-
                    // based check is a follow-up.
                    let ((u0, u1), (v0, v1)) = surface.parameter_bounds();
                    if u0.is_finite()
                        && u1.is_finite()
                        && v0.is_finite()
                        && v1.is_finite()
                        && u1 > u0
                        && v1 > v0
                    {
                        const N: usize = 12;
                        let mut grid: Vec<Point3> = Vec::new();
                        for i in 0..=N {
                            let u = u0 + (u1 - u0) * i as f64 / N as f64;
                            for j in 0..=N {
                                let v = v0 + (v1 - v0) * j as f64 / N as f64;
                                if let Ok(p) = surface.point_at(u, v) {
                                    grid.push(p);
                                }
                            }
                        }
                        let mut cell = 0.0_f64;
                        for w in grid.windows(2) {
                            cell = cell.max(w[0].distance(&w[1]));
                        }
                        let threshold = cell * 2.5;
                        if grid.len() > 9 && threshold > 0.0 {
                            let mut loops2 = vec![face.outer_loop];
                            loops2.extend(&face.inner_loops);
                            for &loop_id in &loops2 {
                                let ld = match model.loops.get(loop_id) {
                                    Some(l) => l,
                                    None => continue,
                                };
                                for &edge_id in &ld.edges {
                                    let edge = match model.edges.get(edge_id) {
                                        Some(e) => e,
                                        None => continue,
                                    };
                                    if let Some(curve) = model.curves.get(edge.curve_id) {
                                        let r = edge.param_range;
                                        let t = 0.5 * (r.start + r.end);
                                        if let Ok(cp) = curve.evaluate(t) {
                                            let min_d = grid
                                                .iter()
                                                .map(|g| cp.position.distance(g))
                                                .fold(f64::INFINITY, f64::min);
                                            if min_d > threshold {
                                                warns.push(ValidationWarning::GeometryInconsistency {
                                                    location: EntityLocation {
                                                        solid_id: None,
                                                        shell_id: None,
                                                        face_id: Some(face_id),
                                                        loop_id: Some(loop_id),
                                                        edge_id: Some(edge_id),
                                                        vertex_id: None,
                                                    },
                                                    distance: min_d,
                                                    message: format!(
                                                        "edge {edge_id} lies ~{min_d:.3e} off face {face_id}'s {} surface",
                                                        surface.type_name()
                                                    ),
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    return warns;
                }

                let mut loop_ids = vec![face.outer_loop];
                loop_ids.extend(&face.inner_loops);
                for &loop_id in &loop_ids {
                    let loop_data = match model.loops.get(loop_id) {
                        Some(l) => l,
                        None => continue,
                    };
                    for &edge_id in &loop_data.edges {
                        let edge = match model.edges.get(edge_id) {
                            Some(e) => e,
                            None => continue,
                        };
                        // Endpoints + interior curve samples.
                        let mut points: Vec<Point3> = Vec::new();
                        if let Some(v) = model.vertices.get(edge.start_vertex) {
                            points.push(v.point());
                        }
                        if let Some(v) = model.vertices.get(edge.end_vertex) {
                            points.push(v.point());
                        }
                        if let Some(curve) = model.curves.get(edge.curve_id) {
                            let r = edge.param_range;
                            for f in [0.25_f64, 0.5, 0.75] {
                                let t = r.start + (r.end - r.start) * f;
                                if let Ok(cp) = curve.evaluate(t) {
                                    points.push(cp.position);
                                }
                            }
                        }
                        // Max distance of any sample off the face's surface.
                        let max_off = points
                            .iter()
                            .filter_map(|p| {
                                surface
                                    .closest_point(p, tolerance)
                                    .ok()
                                    .and_then(|(u, v)| surface.point_at(u, v).ok())
                                    .map(|sp| p.distance(&sp))
                            })
                            .fold(0.0_f64, f64::max);
                        if max_off > tolerance.distance() {
                            warns.push(ValidationWarning::GeometryInconsistency {
                                location: EntityLocation {
                                    solid_id: None,
                                    shell_id: None,
                                    face_id: Some(face_id),
                                    loop_id: Some(loop_id),
                                    edge_id: Some(edge_id),
                                    vertex_id: None,
                                },
                                distance: max_off,
                                message: format!(
                                    "edge {edge_id} lies {max_off:.3e} off face {face_id}'s {} surface",
                                    surface.type_name()
                                ),
                            });
                        }
                    }
                }
                warns
            })
            .collect();

        GeometryValidationResults { warnings }
    }

    fn validate_deep_parallel(
        &self,
        _model: &BRepModel,
        _tolerance: Tolerance,
    ) -> DeepValidationResults {
        // Deep validation including numerical checks
        DeepValidationResults
    }

    fn analyze_edge_usage_parallel(&self, model: &BRepModel) -> DashMap<EdgeId, EdgeUsage> {
        use rayon::prelude::*;
        let edge_usage: DashMap<EdgeId, EdgeUsage> = DashMap::new();

        // Analyze each face in parallel
        // Note: FaceStore doesn't have par_iter, so we need to collect face IDs first
        let face_ids: Vec<FaceId> = (0..model.faces.len() as u32).collect();

        face_ids.par_iter().for_each(|&face_id| {
            if let Some(face) = model.faces.get(face_id) {
                // Check outer loop
                let mut all_loops = vec![face.outer_loop];
                all_loops.extend(&face.inner_loops);

                // Check each loop in the face
                for &loop_id in &all_loops {
                    if let Some(loop_data) = model.loops.get(loop_id) {
                        // Track edge usage in this loop
                        for (i, &edge_id) in loop_data.edges.iter().enumerate() {
                            let orientation =
                                loop_data.orientations.get(i).copied().unwrap_or(true);

                            edge_usage
                                .entry(edge_id)
                                .and_modify(|usage| {
                                    usage.faces.push(face_id);
                                    usage.loops.push(loop_id);
                                    usage.orientations.push(orientation);
                                })
                                .or_insert_with(|| EdgeUsage {
                                    faces: vec![face_id],
                                    loops: vec![loop_id],
                                    orientations: vec![orientation],
                                });
                        }
                    }
                }
            }
        });

        edge_usage
    }

    fn combine_results(
        &self,
        topology: TopologyValidationResults,
        geometry: GeometryValidationResults,
        _deep: DeepValidationResults,
        context: ValidationContext,
        _level: ValidationLevel,
    ) -> ValidationResult {
        // Combine all results
        let mut all_errors = Vec::new();
        let mut all_warnings = Vec::new();

        // Collect errors from topology validation
        for (_, result) in &topology.solid_results {
            all_errors.extend(result.errors.clone());
            all_warnings.extend(result.warnings.clone());
        }

        // Add gap errors
        all_errors.extend(topology.gap_errors);

        // B1: geometric-consistency findings now genuinely set `geometry_valid`
        // (it was hardcoded `true`, so a geometrically-broken-but-topologically-
        // wellformed solid certified as sound — the central "kernel can lie" bug).
        // They are surfaced as WARNINGS, not errors: they catch genuine but
        // pre-existing op defects (e.g. fillet/chamfer trim edges sitting slightly
        // off a planar face), and adding them to the error list would break ops
        // that validate their own result. `is_valid` stays gated on hard errors so
        // the pipeline isn't broken; promote to errors once the trim geometry is
        // fixed (the underlying op bug becomes the next target).
        let geometry_valid = geometry.warnings.is_empty();
        all_warnings.extend(geometry.warnings);

        let is_valid = all_errors.is_empty();
        let topology_valid = all_errors
            .iter()
            .filter(|e| matches!(e, ValidationError::TopologyError { .. }))
            .count()
            == 0;

        ValidationResult {
            is_valid,
            topology_valid,
            geometry_valid,
            manufacturing_valid: true,
            errors: all_errors,
            warnings: all_warnings,
            repairs: Vec::new(),
            statistics: ModelStatistics::default(),
            context,
            certificate: None,
        }
    }

    /// Validate the generalized Euler–Poincaré characteristic of a solid:
    ///
    /// ```text
    ///     V - E + F - R = 2 (S - G)
    /// ```
    ///
    /// where R = total inner loops (face holes / rings), S = number of
    /// shells (1 outer + N voids), G = genus (handles). The naive
    /// `V - E + F = 2` only holds when every face is a topological disk and
    /// the body is a single closed shell; it FALSELY rejects every
    /// legitimate solid that has a face with a hole — a through-bore, a
    /// counterbore floor, a box pierced by another box — i.e. the everyday
    /// output of boolean operations. Counting R (and S) and using the full
    /// formula is what makes a pierced/bored solid validate (and what lets a
    /// downstream chamfer/fillet succeed on it). See KNOWN_BUGS.md #37.
    fn validate_euler_characteristic_for_solid(
        &self,
        model: &BRepModel,
        solid_id: SolidId,
        solid: &Solid,
        errors: &mut Vec<ValidationError>,
        warnings: &mut Vec<ValidationWarning>,
    ) {
        // Count V, E, F, R across EVERY shell (outer + any voids), so a
        // hollow solid with inner shells is handled by the same formula.
        let shells = std::iter::once(solid.outer_shell)
            .chain(solid.inner_shells.iter().copied())
            .collect::<Vec<_>>();
        let shell_count = shells.len() as i32;
        let shell_id = solid.outer_shell; // diagnostic location anchor

        let mut vertex_set = std::collections::HashSet::new();
        let mut edge_set = std::collections::HashSet::new();
        let mut face_count = 0i32;
        let mut ring_count = 0i32; // R: inner loops summed over all faces
                                   // Faces modelled as a single fully-periodic CLOSED surface with no
                                   // bounding B-Rep edges (a sphere/torus as one seamless face). Such a
                                   // face is itself a closed surface (χ=2), not a polyhedral 2-cell/disk
                                   // (χ=1), so the plain V−E+F count under-reports it by 1 per face.
        let mut seamless_closed_faces = 0i32;

        for sid in &shells {
            let Some(shell) = model.shells.get(*sid) else {
                continue;
            };
            for &face_id in &shell.faces {
                face_count += 1;
                if let Some(face) = model.faces.get(face_id) {
                    ring_count += face.inner_loops.len() as i32;
                    let mut all_loops = vec![face.outer_loop];
                    all_loops.extend(&face.inner_loops);
                    let mut face_edge_count = 0usize;
                    for &loop_id in &all_loops {
                        if let Some(loop_data) = model.loops.get(loop_id) {
                            for &edge_id in &loop_data.edges {
                                face_edge_count += 1;
                                edge_set.insert(edge_id);
                                if let Some(edge) = model.edges.get(edge_id) {
                                    vertex_set.insert(edge.start_vertex);
                                    vertex_set.insert(edge.end_vertex);
                                }
                            }
                        }
                    }
                    if face_edge_count == 0 {
                        seamless_closed_faces += 1;
                    }
                }
            }
        }

        let v = vertex_set.len() as i32;
        let e = edge_set.len() as i32;
        let f = face_count;
        let r = ring_count;

        // A solid built ENTIRELY from seamless closed faces (e.g. a lone
        // sphere: V=0, E=0, F=1) is a valid closed surface but not a
        // polyhedral 2-cell complex, so the plain combinatorial form does not
        // apply (it once rejected the lone sphere and broke transform_solid).
        // The seamless correction below restores the right χ, but keep this
        // fast exit for the all-seamless case.
        if e == 0 {
            return;
        }

        // V - E + F - R = 2(S - G)  ⇒  G = S - (V - E + F - R) / 2.
        // Each seamless closed face contributes χ=2 (a closed surface), but the
        // raw V−E+F counts it as a single 2-cell (χ=1); add +1 per such face so
        // a MIXED solid — a seamed outer shell plus a seamless void shell (the
        // sphere cavity of cyl∖sphere, BOOL #7) — sums to the right parity
        // instead of falsely reading odd. The left side must be even (= 2·k);
        // an odd value is a genuine topology defect. Non-negative genus is
        // valid (a torus is G=1); a negative genus is impossible.
        let poincare = v - e + f - r + seamless_closed_faces;
        if poincare % 2 != 0 {
            errors.push(ValidationError::TopologyError {
                message: format!(
                    "Invalid Euler–Poincaré characteristic: V({})-E({})+F({})-R({}) = {} is odd \
                     (must be even = 2(S-G); S={})",
                    v, e, f, r, poincare, shell_count
                ),
                location: EntityLocation {
                    solid_id: Some(solid_id),
                    shell_id: Some(shell_id),
                    face_id: None,
                    loop_id: None,
                    edge_id: None,
                    vertex_id: None,
                },
            });
            return;
        }

        let genus = shell_count - poincare / 2;
        if genus < 0 {
            errors.push(ValidationError::TopologyError {
                message: format!(
                    "Invalid Euler–Poincaré characteristic: V({})-E({})+F({})-R({}) = {} with \
                     S={} implies negative genus {} (impossible for a closed orientable solid)",
                    v, e, f, r, poincare, shell_count, genus
                ),
                location: EntityLocation {
                    solid_id: Some(solid_id),
                    shell_id: Some(shell_id),
                    face_id: None,
                    loop_id: None,
                    edge_id: None,
                    vertex_id: None,
                },
            });
        } else if genus > 0 {
            // Valid but non-zero genus (handles) — informational only.
            warnings.push(ValidationWarning::ToleranceRisk {
                location: EntityLocation {
                    solid_id: Some(solid_id),
                    shell_id: Some(shell_id),
                    face_id: None,
                    loop_id: None,
                    edge_id: None,
                    vertex_id: None,
                },
                accumulated: genus as f64,
            });
        }
    }

    /// Check for non-manifold edges in a solid
    fn check_manifold_edges_for_solid(
        &self,
        model: &BRepModel,
        solid_id: SolidId,
        solid: &Solid,
        errors: &mut Vec<ValidationError>,
        warnings: &mut Vec<ValidationWarning>,
    ) {
        let shell_id = solid.outer_shell;
        if let Some(shell) = model.shells.get(shell_id) {
            // Count face usage per edge
            let mut edge_face_count: std::collections::HashMap<EdgeId, usize> =
                std::collections::HashMap::new();

            for &face_id in &shell.faces {
                if let Some(face) = model.faces.get(face_id) {
                    let mut all_loops = vec![face.outer_loop];
                    all_loops.extend(&face.inner_loops);
                    for &loop_id in &all_loops {
                        if let Some(loop_data) = model.loops.get(loop_id) {
                            for &edge_id in &loop_data.edges {
                                *edge_face_count.entry(edge_id).or_insert(0) += 1;
                            }
                        }
                    }
                }
            }

            // Check for non-manifold edges
            for (edge_id, count) in edge_face_count {
                if count > 2 {
                    errors.push(ValidationError::TopologyError {
                        message: format!(
                            "Non-manifold edge: {} faces share edge {}",
                            count, edge_id
                        ),
                        location: EntityLocation {
                            solid_id: Some(solid_id),
                            shell_id: Some(shell_id),
                            face_id: None,
                            loop_id: None,
                            edge_id: Some(edge_id),
                            vertex_id: None,
                        },
                    });
                } else if count == 1 {
                    warnings.push(ValidationWarning::ToleranceRisk {
                        location: EntityLocation {
                            solid_id: Some(solid_id),
                            shell_id: Some(shell_id),
                            face_id: None,
                            loop_id: None,
                            edge_id: Some(edge_id),
                            vertex_id: None,
                        },
                        accumulated: 1.0,
                    });
                }
            }
        }
    }

    /// Check for gaps in topology
    fn check_topology_gaps(
        &self,
        model: &BRepModel,
        edge_usage: &DashMap<EdgeId, EdgeUsage>,
        _tolerance: Tolerance,
    ) -> Vec<ValidationError> {
        let mut gap_errors = Vec::new();

        // Check each edge for proper face connectivity
        for entry in edge_usage.iter() {
            let edge_id = *entry.key();
            let usage = entry.value();

            // A manifold edge should be used by exactly 2 faces (or 1 for boundary)
            if usage.faces.is_empty() {
                gap_errors.push(ValidationError::ConnectivityError {
                    message: format!("Edge {} is not used by any faces", edge_id),
                    location: EntityLocation {
                        solid_id: None,
                        shell_id: None,
                        face_id: None,
                        loop_id: None,
                        edge_id: Some(edge_id),
                        vertex_id: None,
                    },
                });
            } else if usage.faces.len() == 1 {
                // Boundary edge - check if it's intentional or a gap
                if let Some(edge) = model.edges.get(edge_id) {
                    // Get vertices to check for gaps
                    if let (Some(_v1), Some(_v2)) = (
                        model.vertices.get(edge.start_vertex),
                        model.vertices.get(edge.end_vertex),
                    ) {
                        // This is a boundary edge, which might indicate a gap
                        gap_errors.push(ValidationError::ConnectivityError {
                            message: format!(
                                "Boundary edge {} detected - potential gap in topology",
                                edge_id
                            ),
                            location: EntityLocation {
                                solid_id: None,
                                shell_id: None,
                                face_id: usage.faces.first().copied(),
                                loop_id: usage.loops.first().copied(),
                                edge_id: Some(edge_id),
                                vertex_id: None,
                            },
                        });
                    }
                }
            }
        }

        gap_errors
    }
}

// Result structures for parallel validation
#[derive(Default)]
struct TopologyValidationResults {
    solid_results: Vec<(SolidId, ValidationResult)>,
    gap_errors: Vec<ValidationError>,
}

#[derive(Default)]
struct GeometryValidationResults {
    warnings: Vec<ValidationWarning>,
}

#[derive(Default)]
struct DeepValidationResults;

/// Validate entire B-Rep model (enhanced entry point)
pub fn validate_model_enhanced(
    model: &BRepModel,
    tolerance: Tolerance,
    level: ValidationLevel,
) -> ValidationResult {
    let validator = ParallelValidator::new();
    validator.validate_model(model, tolerance, level)
}

/// Run [`validate_model_enhanced`] but scope the verdict to a single solid.
///
/// The enhanced sweep necessarily walks the whole model — cross-solid
/// checks need the full picture — but an *operation's* post-validation
/// must judge only the solid it just modified. A pre-existing defect on
/// an unrelated solid (a boolean operand husk, another already-open part)
/// is not this op's fault and must not fail it. This keeps errors
/// attributed to `solid_id`, keeps model-global errors that carry no
/// solid attribution, and drops errors pinned to a *different* solid.
///
/// See `KNOWN_BUGS.md` #29 (whole-model validation scope) for the defect
/// this closes.
pub fn validate_solid_scoped(
    model: &BRepModel,
    solid_id: SolidId,
    tolerance: Tolerance,
    level: ValidationLevel,
) -> ValidationResult {
    let mut result = validate_model_enhanced(model, tolerance, level);
    result.errors.retain(|e| match e.solid_id() {
        Some(sid) => sid == solid_id,
        None => true,
    });
    result.is_valid = result.errors.is_empty();
    result
}

/// Like [`validate_solid_scoped`] but scoped to whichever solids OWN the
/// given faces — the form an op that works by face-sets needs (blend emits
/// blend faces on one result solid; a pattern emits faces across several
/// newly-created instance solids). Derives the owning-solid set from the
/// faces, then keeps only errors attributed to those solids (plus
/// model-global, unattributed errors). See KNOWN_BUGS.md #29/#39.
pub fn validate_faces_scoped(
    model: &BRepModel,
    faces: &[FaceId],
    tolerance: Tolerance,
    level: ValidationLevel,
) -> ValidationResult {
    let face_set: std::collections::HashSet<FaceId> = faces.iter().copied().collect();
    let mut touched_solids: std::collections::HashSet<SolidId> = std::collections::HashSet::new();
    for (sid, solid) in model.solids.iter() {
        let shells = std::iter::once(solid.outer_shell).chain(solid.inner_shells.iter().copied());
        for shid in shells {
            let Some(shell) = model.shells.get(shid) else {
                continue;
            };
            if shell.faces.iter().any(|f| face_set.contains(f)) {
                touched_solids.insert(sid);
                break;
            }
        }
    }

    let mut result = validate_model_enhanced(model, tolerance, level);
    result.errors.retain(|e| match e.solid_id() {
        Some(sid) => touched_solids.contains(&sid),
        None => true,
    });
    result.is_valid = result.errors.is_empty();
    result
}

/// Validate that a single shell is closed: every edge contained in any
/// loop of any face in the shell must be used by exactly two faces of
/// the shell.
///
/// Returns:
/// - an empty `Vec` if the shell is closed and manifold;
/// - a `ConnectivityError` per offending edge otherwise.
///
/// Boundary edges (count = 1) are reported as
/// `"Boundary edge {edge_id} detected in shell {shell_id} - shell is not closed"`.
/// Non-manifold edges (count > 2) are reported as
/// `"Non-manifold edge {edge_id} used by N faces in shell {shell_id}"`.
///
/// Counterpart to the parallel `check_topology_gaps` analysis, scoped
/// to one shell and returning a per-edge error vector instead of a
/// global model report. Used by `operations::sew` as a post-sew gate
/// for the F7 trim/sew pipeline.
pub fn validate_shell_closure(model: &BRepModel, shell_id: ShellId) -> Vec<ValidationError> {
    let Some(shell) = model.shells.get(shell_id) else {
        return vec![ValidationError::ConnectivityError {
            message: format!("Shell {} not found", shell_id),
            location: EntityLocation {
                solid_id: None,
                shell_id: Some(shell_id),
                face_id: None,
                loop_id: None,
                edge_id: None,
                vertex_id: None,
            },
        }];
    };

    // Tally edge usage scoped to this shell.
    let mut edge_usage: std::collections::HashMap<EdgeId, (Vec<FaceId>, Vec<LoopId>)> =
        std::collections::HashMap::new();
    for &face_id in &shell.faces {
        let Some(face) = model.faces.get(face_id) else {
            continue;
        };
        let mut all_loops = Vec::with_capacity(1 + face.inner_loops.len());
        all_loops.push(face.outer_loop);
        all_loops.extend(&face.inner_loops);
        for &loop_id in &all_loops {
            let Some(loop_data) = model.loops.get(loop_id) else {
                continue;
            };
            for &edge_id in &loop_data.edges {
                let entry = edge_usage.entry(edge_id).or_default();
                entry.0.push(face_id);
                entry.1.push(loop_id);
            }
        }
    }

    let mut errors = Vec::new();
    for (edge_id, (faces, loops)) in edge_usage {
        match faces.len() {
            0 => {} // unreachable: we only insert with at least one face.
            1 => {
                errors.push(ValidationError::ConnectivityError {
                    message: format!(
                        "Boundary edge {} detected in shell {} - shell is not closed",
                        edge_id, shell_id
                    ),
                    location: EntityLocation {
                        solid_id: None,
                        shell_id: Some(shell_id),
                        face_id: faces.first().copied(),
                        loop_id: loops.first().copied(),
                        edge_id: Some(edge_id),
                        vertex_id: None,
                    },
                });
            }
            2 => {} // manifold edge.
            n => {
                errors.push(ValidationError::ConnectivityError {
                    message: format!(
                        "Non-manifold edge {} used by {} faces in shell {}",
                        edge_id, n, shell_id
                    ),
                    location: EntityLocation {
                        solid_id: None,
                        shell_id: Some(shell_id),
                        face_id: faces.first().copied(),
                        loop_id: loops.first().copied(),
                        edge_id: Some(edge_id),
                        vertex_id: None,
                    },
                });
            }
        }
    }
    errors
}

/// Validate that every `PCurveId` referenced from an edge satisfies the
/// F7-ε invariants:
///
/// 1. The id resolves to a live entry in `model.pcurves` (no dangling
///    references after snapshot/restore or store rebuild).
/// 2. The pcurve's `face` is one of the faces adjacent to the edge in
///    the topology — a pcurve on a face that no longer borders the
///    edge is geometrically meaningless and indicates a missed
///    invalidation on a previous mutating operation.
/// 3. The pcurve's `tolerance` is finite and non-negative — already
///    enforced by `PCurveStore::add`, but re-checked here so that
///    tolerance corruption from external mutation surfaces during
///    full-model validation.
///
/// Returns an empty `Vec` when every edge's pcurves are consistent.
/// Mismatches are returned as `ConnectivityError` (dangling /
/// face-mismatch) or `GeometryError` (tolerance corruption) so the
/// existing diagnostic surface treats them uniformly.
///
/// Cost is O(edges × pcurves_per_edge × faces_per_model) in the worst
/// case, but each edge typically carries at most two pcurves and the
/// adjacency lookup is amortised over a single pass.
pub fn validate_pcurve_references(model: &BRepModel) -> Vec<ValidationError> {
    // Build edge -> adjacent-face map in one pass over loops.
    let mut edge_faces: std::collections::HashMap<EdgeId, Vec<FaceId>> =
        std::collections::HashMap::new();
    for (face_id, face) in model.faces.iter() {
        let mut all_loops = Vec::with_capacity(1 + face.inner_loops.len());
        all_loops.push(face.outer_loop);
        all_loops.extend(&face.inner_loops);
        for &loop_id in &all_loops {
            let Some(loop_data) = model.loops.get(loop_id) else {
                continue;
            };
            for &edge_id in &loop_data.edges {
                edge_faces.entry(edge_id).or_default().push(face_id);
            }
        }
    }

    let mut errors = Vec::new();
    for (edge_id, edge) in model.edges.iter() {
        if edge.pcurves.is_empty() {
            continue;
        }
        let adjacent = edge_faces.get(&edge_id).cloned().unwrap_or_default();
        for &pcurve_id in &edge.pcurves {
            match model.pcurves.get(pcurve_id) {
                None => {
                    errors.push(ValidationError::ConnectivityError {
                        message: format!(
                            "Edge {} references missing pcurve id {}",
                            edge_id, pcurve_id
                        ),
                        location: EntityLocation {
                            solid_id: None,
                            shell_id: None,
                            face_id: None,
                            loop_id: None,
                            edge_id: Some(edge_id),
                            vertex_id: None,
                        },
                    });
                }
                Some(pc) => {
                    if !pc.tolerance.is_finite() || pc.tolerance < 0.0 {
                        errors.push(ValidationError::GeometryError {
                            message: format!(
                                "Edge {} pcurve {} has invalid tolerance {}",
                                edge_id, pcurve_id, pc.tolerance
                            ),
                            location: EntityLocation {
                                solid_id: None,
                                shell_id: None,
                                face_id: Some(pc.face),
                                loop_id: None,
                                edge_id: Some(edge_id),
                                vertex_id: None,
                            },
                        });
                    }
                    if !adjacent.is_empty() && !adjacent.contains(&pc.face) {
                        errors.push(ValidationError::ConnectivityError {
                            message: format!(
                                "Edge {} pcurve {} anchored to face {} which is not adjacent",
                                edge_id, pcurve_id, pc.face
                            ),
                            location: EntityLocation {
                                solid_id: None,
                                shell_id: None,
                                face_id: Some(pc.face),
                                loop_id: None,
                                edge_id: Some(edge_id),
                                vertex_id: None,
                            },
                        });
                    }
                }
            }
        }
    }
    errors
}

/// Automatic repair functionality
pub struct ModelRepairer {
    tolerance: Tolerance,
}

#[derive(Debug, Clone)]
pub struct RepairOptions {
    pub merge_tolerance: f64,
    pub simplify_tolerance: f64,
    pub remove_small_features: bool,
    pub fix_orientations: bool,
    pub heal_gaps: bool,
    pub split_non_manifold: bool,
}

impl Default for RepairOptions {
    fn default() -> Self {
        Self {
            merge_tolerance: 1e-6,
            simplify_tolerance: 1e-4,
            remove_small_features: true,
            fix_orientations: true,
            heal_gaps: true,
            split_non_manifold: true,
        }
    }
}

impl ModelRepairer {
    pub fn new(tolerance: Tolerance) -> Self {
        Self { tolerance }
    }

    /// Attempt automatic repair based on validation results
    pub fn repair_model(
        &self,
        model: &mut BRepModel,
        validation: &ValidationResult,
    ) -> RepairResult {
        let mut applied_repairs = Vec::new();
        let mut failed_repairs = Vec::new();

        // Sort repairs by confidence
        let mut repairs = validation.repairs.clone();
        repairs.sort_by(|a, b| {
            b.confidence
                .partial_cmp(&a.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        for repair in repairs {
            match self.apply_repair(model, &repair) {
                Ok(()) => applied_repairs.push(repair),
                Err(e) => failed_repairs.push((repair, e)),
            }
        }

        RepairResult {
            applied: applied_repairs,
            failed: failed_repairs,
            model_valid: self.verify_model(model),
        }
    }

    fn apply_repair(&self, model: &mut BRepModel, repair: &RepairSuggestion) -> MathResult<()> {
        match &repair.action {
            RepairAction::MergeVertices { v1, v2, .. } => {
                model.vertices.merge_vertices(*v1, *v2);
                Ok(())
            }
            RepairAction::FlipFaceOrientation { face_id } => {
                if let Some(face) = model.faces.get_mut(*face_id) {
                    // Flip the face orientation
                    face.orientation = match face.orientation {
                        FaceOrientation::Forward => FaceOrientation::Backward,
                        FaceOrientation::Backward => FaceOrientation::Forward,
                    };
                }
                Ok(())
            }
        }
    }

    fn verify_model(&self, model: &BRepModel) -> bool {
        let quick_check = validate_model_enhanced(model, self.tolerance, ValidationLevel::Quick);
        quick_check.is_valid
    }
}

#[derive(Debug)]
pub struct RepairResult {
    pub applied: Vec<RepairSuggestion>,
    pub failed: Vec<(RepairSuggestion, MathError)>,
    pub model_valid: bool,
}

/// Check face orientation consistency in a shell
pub fn check_face_orientations(model: &BRepModel, shell_id: ShellId) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    if let Some(shell) = model.shells.get(shell_id) {
        // Build adjacency map for faces
        let mut face_adjacency: std::collections::HashMap<EdgeId, Vec<(FaceId, bool)>> =
            std::collections::HashMap::new();

        // Collect face-edge relationships
        for &face_id in &shell.faces {
            if let Some(face) = model.faces.get(face_id) {
                let mut all_loops = vec![face.outer_loop];
                all_loops.extend(&face.inner_loops);
                for &loop_id in &all_loops {
                    if let Some(loop_data) = model.loops.get(loop_id) {
                        for (i, &edge_id) in loop_data.edges.iter().enumerate() {
                            let orientation =
                                loop_data.orientations.get(i).copied().unwrap_or(true);
                            face_adjacency
                                .entry(edge_id)
                                .or_default()
                                .push((face_id, orientation));
                        }
                    }
                }
            }
        }

        // Check orientation consistency
        for (edge_id, faces) in face_adjacency {
            if faces.len() == 2 {
                // For manifold edges, orientations should be opposite
                let (face1, orient1) = faces[0];
                let (face2, orient2) = faces[1];

                if orient1 == orient2 {
                    errors.push(ValidationError::OrientationError {
                        message: format!(
                            "Inconsistent face orientations: faces {} and {} have same orientation on edge {}",
                            face1, face2, edge_id
                        ),
                        location: EntityLocation {
                            solid_id: None,
                            shell_id: Some(shell_id),
                            face_id: Some(face1),
                            loop_id: None,
                            edge_id: Some(edge_id),
                            vertex_id: None,
                        },
                    });
                }
            }
        }
    }

    errors
}

/// Create validation certificate for valid models
pub fn create_certificate(
    model: &BRepModel,
    validation: &ValidationResult,
    level: ValidationLevel,
) -> Option<ValidationCertificate> {
    if !validation.is_valid {
        return None;
    }

    Some(ValidationCertificate {
        id: uuid::Uuid::new_v4().to_string(),
        timestamp: std::time::SystemTime::now(),
        level,
        model_hash: calculate_model_hash(model),
        validator_version: env!("CARGO_PKG_VERSION").to_string(),
        signature: generate_signature(model, validation), // SHA256 signature
    })
}

fn calculate_model_hash(model: &BRepModel) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    model.solids.len().hash(&mut hasher);
    model.faces.len().hash(&mut hasher);
    model.vertices.len().hash(&mut hasher);
    hasher.finish()
}

/// Generate cryptographic signature for validation certificate
fn generate_signature(model: &BRepModel, validation: &ValidationResult) -> Vec<u8> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hasher;

    let mut hasher = DefaultHasher::new();

    // Hash model structure
    calculate_model_hash(model).hash(&mut hasher);

    // Hash validation results
    validation.is_valid.hash(&mut hasher);
    validation.topology_valid.hash(&mut hasher);
    validation.geometry_valid.hash(&mut hasher);
    validation.manufacturing_valid.hash(&mut hasher);
    validation.errors.len().hash(&mut hasher);
    validation.warnings.len().hash(&mut hasher);

    // Hash timestamp. `duration_since(UNIX_EPOCH)` can only fail if the
    // system clock is set before 1970; degrade gracefully to a zero duration
    // in that pathological case so validation signatures remain computable.
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .hash(&mut hasher);

    // Generate 32-byte signature
    let hash = hasher.finish();
    let mut signature = vec![0u8; 32];

    // Fill signature with hash bytes
    for i in 0..4 {
        let bytes = ((hash >> (i * 16)) as u16).to_le_bytes();
        signature[i * 2] = bytes[0];
        signature[i * 2 + 1] = bytes[1];
    }

    // Add some entropy
    for i in 8..32 {
        signature[i] = ((i as u64 * hash) % 256) as u8;
    }

    signature
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::MissingEntity { entity_type, id } => {
                write!(f, "Missing {} with ID {}", entity_type, id)
            }
            ValidationError::TopologyError { message, location } => {
                write!(f, "Topology error at {:?}: {}", location, message)
            }
            ValidationError::GeometryError { message, location } => {
                write!(f, "Geometry error at {:?}: {}", location, message)
            }
            ValidationError::OrientationError { message, location } => {
                write!(f, "Orientation error at {:?}: {}", location, message)
            }
            ValidationError::ConnectivityError { message, location } => {
                write!(f, "Connectivity error at {:?}: {}", location, message)
            }
            ValidationError::ManufacturingError {
                message,
                constraint,
            } => {
                write!(
                    f,
                    "Manufacturing constraint violated: {} ({:?})",
                    message, constraint
                )
            }
            ValidationError::ToleranceError {
                message,
                accumulated,
                allowed,
            } => {
                write!(
                    f,
                    "Tolerance error: {} (accumulated: {:.6}, allowed: {:.6})",
                    message, accumulated, allowed
                )
            }
            ValidationError::FeatureError {
                message,
                feature_id,
            } => {
                write!(f, "Feature {} error: {}", feature_id, message)
            }
            ValidationError::AssemblyError {
                message,
                components,
            } => {
                write!(
                    f,
                    "Assembly error: {} (components: {:?})",
                    message, components
                )
            }
        }
    }
}

impl std::fmt::Display for ValidationWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationWarning::NearDegenerate {
                entity_type,
                id,
                measure,
            } => {
                write!(
                    f,
                    "{} {} is near-degenerate (measure: {:.6})",
                    entity_type, id, measure
                )
            }
            ValidationWarning::SharpAngle { location, angle } => {
                write!(f, "Sharp angle at {:?}: {:.1}°", location, angle)
            }
            ValidationWarning::LargeAspectRatio {
                entity_type,
                id,
                ratio,
            } => {
                write!(
                    f,
                    "{} {} has large aspect ratio: {:.1}",
                    entity_type, id, ratio
                )
            }
            ValidationWarning::NearCoincident {
                entity1,
                entity2,
                distance,
            } => {
                write!(
                    f,
                    "Near-coincident entities {:?} and {:?} (distance: {:.6})",
                    entity1, entity2, distance
                )
            }
            ValidationWarning::ToleranceRisk {
                location,
                accumulated,
            } => {
                write!(
                    f,
                    "Tolerance accumulation risk at {:?}: {:.6}",
                    location, accumulated
                )
            }
            ValidationWarning::GeometryInconsistency { message, .. } => {
                write!(f, "{}", message)
            }
        }
    }
}

#[cfg(test)]
mod pcurve_validation_tests {
    use super::*;
    use crate::math::Point2;
    use crate::primitives::curve::ParameterRange;
    use crate::primitives::p_curve::{PCurve, PCurve2dKind};
    use crate::primitives::topology_builder::{BRepModel, TopologyBuilder};

    fn box_model() -> BRepModel {
        let mut model = BRepModel::new();
        {
            let mut builder = TopologyBuilder::new(&mut model);
            let _ = builder.create_box_3d(2.0, 3.0, 4.0);
        }
        model
    }

    fn line_pcurve_on_face(face: FaceId) -> PCurve {
        PCurve::new(
            face,
            PCurve2dKind::Line {
                start: Point2::ZERO,
                end: Point2::new(1.0, 0.0),
            },
            ParameterRange::unit(),
            1e-6,
        )
    }

    #[test]
    fn no_errors_when_no_pcurves_attached() {
        let model = box_model();
        let errors = validate_pcurve_references(&model);
        assert!(errors.is_empty());
    }

    #[test]
    fn dangling_pcurve_id_reports_connectivity_error() {
        let mut model = box_model();
        // Pick a real edge from the model and point it at a
        // never-allocated pcurve id.
        let edge_id = model
            .edges
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("box has edges");
        let _ = model.edges.attach_pcurve(edge_id, 999);

        let errors = validate_pcurve_references(&model);
        assert_eq!(errors.len(), 1);
        match &errors[0] {
            ValidationError::ConnectivityError { message, location } => {
                assert!(message.contains("missing pcurve id 999"));
                assert_eq!(location.edge_id, Some(edge_id));
            }
            other => panic!("expected ConnectivityError, got {:?}", other),
        }
    }

    #[test]
    fn pcurve_on_non_adjacent_face_reports_connectivity_error() {
        let mut model = box_model();
        let edge_id = model
            .edges
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("box has edges");

        // Find a face that is NOT adjacent to this edge. Walk loops to
        // collect adjacents and pick a face outside that set.
        let mut adjacent_faces = std::collections::HashSet::new();
        for (face_id, face) in model.faces.iter() {
            let mut all_loops = vec![face.outer_loop];
            all_loops.extend(&face.inner_loops);
            for &lid in &all_loops {
                if let Some(lp) = model.loops.get(lid) {
                    if lp.edges.contains(&edge_id) {
                        adjacent_faces.insert(face_id);
                    }
                }
            }
        }
        let foreign_face = model
            .faces
            .iter()
            .map(|(id, _)| id)
            .find(|id| !adjacent_faces.contains(id))
            .expect("box has at least one non-adjacent face");

        let pid = model
            .pcurves
            .add(line_pcurve_on_face(foreign_face))
            .expect("add pcurve");
        let _ = model.edges.attach_pcurve(edge_id, pid);

        let errors = validate_pcurve_references(&model);
        assert_eq!(errors.len(), 1);
        match &errors[0] {
            ValidationError::ConnectivityError { message, .. } => {
                assert!(message.contains("not adjacent"));
            }
            other => panic!("expected ConnectivityError, got {:?}", other),
        }
    }

    #[test]
    fn pcurve_on_adjacent_face_passes_validation() {
        let mut model = box_model();
        // Pick any edge and one of its adjacent faces.
        let (edge_id, adjacent_face) = model
            .faces
            .iter()
            .find_map(|(face_id, face)| {
                let lid = face.outer_loop;
                model
                    .loops
                    .get(lid)
                    .and_then(|lp| lp.edges.first().copied())
                    .map(|eid| (eid, face_id))
            })
            .expect("box has an edge on a face");

        let pid = model
            .pcurves
            .add(line_pcurve_on_face(adjacent_face))
            .expect("add pcurve");
        let _ = model.edges.attach_pcurve(edge_id, pid);

        let errors = validate_pcurve_references(&model);
        assert!(
            errors.is_empty(),
            "expected no errors for legitimate pcurve attachment, got {:?}",
            errors
        );
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_validation_levels() {
//         let quick = ValidationLevel::Quick;
//         let standard = ValidationLevel::Standard;
//         let deep = ValidationLevel::Deep;
//
//         assert!(quick < standard);
//         assert!(standard < deep);
//     }
//
//     #[test]
//     fn test_repair_options() {
//         let options = RepairOptions::default();
//         assert_eq!(options.merge_tolerance, 1e-6);
//         assert!(options.fix_orientations);
//     }
//
//     #[test]
//     fn test_manufacturing_constraints() {
//         let constraints = ManufacturingConstraints {
//             min_wall_thickness: 1.0,
//             min_feature_size: 0.5,
//             min_draft_angle: 1.0_f64.to_radians(),
//             max_aspect_ratio: 10.0,
//             tool_constraints: ToolConstraints {
//                 min_tool_radius: 0.25,
//                 max_tool_length: 50.0,
//                 access_directions: vec![Vector3::Z, -Vector3::Z],
//             },
//         };
//
//         assert_eq!(constraints.min_wall_thickness, 1.0);
//         assert_eq!(constraints.tool_constraints.access_directions.len(), 2);
//     }
//
//     #[test]
//     fn test_parallel_validator() {
//         let validator = ParallelValidator::new(Some(4));
//         assert!(validator.thread_pool.is_some());
//     }
//
//     #[test]
//     fn test_entity_location() {
//         let location = EntityLocation {
//             solid_id: Some(0),
//             shell_id: Some(1),
//             face_id: Some(2),
//             loop_id: None,
//             edge_id: None,
//             vertex_id: None,
//         };
//
//         assert_eq!(location.solid_id, Some(0));
//         assert_eq!(location.face_id, Some(2));
//     }
// }
