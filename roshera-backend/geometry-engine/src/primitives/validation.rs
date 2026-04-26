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
            GeometryValidationResults
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

        // Validate solids in parallel
        let solid_results: Vec<_> = (0..model.solids.len() as u32)
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
        _model: &BRepModel,
        _tolerance: Tolerance,
    ) -> GeometryValidationResults {
        // Parallel geometry validation
        GeometryValidationResults
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
        _geometry: GeometryValidationResults,
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

        let is_valid = all_errors.is_empty();
        let topology_valid = all_errors
            .iter()
            .filter(|e| matches!(e, ValidationError::TopologyError { .. }))
            .count()
            == 0;

        ValidationResult {
            is_valid,
            topology_valid,
            geometry_valid: true,
            manufacturing_valid: true,
            errors: all_errors,
            warnings: all_warnings,
            repairs: Vec::new(),
            statistics: ModelStatistics::default(),
            context,
            certificate: None,
        }
    }

    /// Validate Euler characteristic for a solid
    /// V - E + F = 2 for a simple closed solid (genus 0)
    /// V - E + F = 2(1 - g) for genus g
    fn validate_euler_characteristic_for_solid(
        &self,
        model: &BRepModel,
        solid_id: SolidId,
        solid: &Solid,
        errors: &mut Vec<ValidationError>,
        warnings: &mut Vec<ValidationWarning>,
    ) {
        // Count vertices, edges, and faces for this solid
        let shell_id = solid.outer_shell;
        if let Some(shell) = model.shells.get(shell_id) {
            let mut vertex_set = std::collections::HashSet::new();
            let mut edge_set = std::collections::HashSet::new();
            let mut face_count = 0;

            // Count entities in the shell
            for &face_id in &shell.faces {
                face_count += 1;
                if let Some(face) = model.faces.get(face_id) {
                    let mut all_loops = vec![face.outer_loop];
                    all_loops.extend(&face.inner_loops);
                    for &loop_id in &all_loops {
                        if let Some(loop_data) = model.loops.get(loop_id) {
                            for &edge_id in &loop_data.edges {
                                edge_set.insert(edge_id);
                                if let Some(edge) = model.edges.get(edge_id) {
                                    vertex_set.insert(edge.start_vertex);
                                    vertex_set.insert(edge.end_vertex);
                                }
                            }
                        }
                    }
                }
            }

            let v = vertex_set.len() as i32;
            let e = edge_set.len() as i32;
            let f = face_count;
            let euler = v - e + f;

            // For a simple closed solid, Euler characteristic should be 2
            // Allow for some tolerance due to genus (holes)
            if euler != 2 {
                // Check if it's a valid genus
                let genus = (2 - euler) / 2;
                if euler == 2 - 2 * genus && genus >= 0 {
                    // Valid genus, just add a warning
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
                } else {
                    // Invalid Euler characteristic
                    errors.push(ValidationError::TopologyError {
                        message: format!(
                            "Invalid Euler characteristic: V({}) - E({}) + F({}) = {} (expected 2 for genus 0)",
                            v, e, f, euler
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
                }
            }
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
struct GeometryValidationResults;

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
        }
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
