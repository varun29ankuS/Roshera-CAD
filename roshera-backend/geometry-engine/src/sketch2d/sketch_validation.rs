//! Sketch validation and error checking
//!
//! This module provides comprehensive validation for 2D sketches,
//! ensuring geometric integrity, constraint consistency, and
//! readiness for 3D operations.
//!
//! # Validation Levels
//!
//! - **Basic**: Entity validity, bounds checking
//! - **Intermediate**: Constraint satisfaction, no self-intersections
//! - **Advanced**: Topology validity, ready for extrusion/revolution
//! - **Strict**: Production-ready with all checks passed

use super::constraints::{ConstraintId, ConstraintStatus, EntityRef};
use super::line2d::LineGeometry;
use super::sketch_topology::{ProfileType, SketchLoop, SketchTopology, TopologyIssue};
use super::{Point2d, Sketch, SketchEntity2d, Tolerance2d, Vector2d};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Validation severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ValidationSeverity {
    /// Informational - does not prevent operations
    Info,
    /// Warning - may cause issues but not critical
    Warning,
    /// Error - prevents some operations
    Error,
    /// Critical - sketch is invalid
    Critical,
}

/// Validation issue types
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ValidationIssue {
    /// Entity-specific issues
    InvalidEntity {
        entity: EntityRef,
        reason: String,
        severity: ValidationSeverity,
    },

    /// Zero-length line segment
    ZeroLengthLine {
        entity: EntityRef,
        severity: ValidationSeverity,
    },

    /// Degenerate arc (zero radius or angle)
    DegenerateArc {
        entity: EntityRef,
        reason: String,
        severity: ValidationSeverity,
    },

    /// Self-intersecting entity
    SelfIntersection {
        entity: EntityRef,
        points: Vec<Point2d>,
        severity: ValidationSeverity,
    },

    /// Overlapping entities
    OverlappingEntities {
        entity1: EntityRef,
        entity2: EntityRef,
        severity: ValidationSeverity,
    },

    /// Constraint issues
    UnsatisfiedConstraint {
        constraint_id: super::ConstraintId,
        error: f64,
        severity: ValidationSeverity,
    },

    /// Over-constrained system
    OverConstrained {
        entities: Vec<EntityRef>,
        conflicting_constraints: Vec<super::ConstraintId>,
        severity: ValidationSeverity,
    },

    /// Under-constrained system
    UnderConstrained {
        entities: Vec<EntityRef>,
        degrees_of_freedom: usize,
        severity: ValidationSeverity,
    },

    /// Open profile when closed is required
    OpenProfile {
        endpoints: Vec<Point2d>,
        severity: ValidationSeverity,
    },

    /// Invalid nesting of loops
    InvalidNesting {
        outer_loop: usize,
        inner_loop: usize,
        reason: String,
        severity: ValidationSeverity,
    },

    /// T-junction detected
    TJunction {
        point: Point2d,
        entities: Vec<EntityRef>,
        severity: ValidationSeverity,
    },

    /// Entity outside sketch bounds
    OutOfBounds {
        entity: EntityRef,
        bounds: (Point2d, Point2d),
        severity: ValidationSeverity,
    },

    /// Numerical precision issue
    NumericalPrecision {
        entity: EntityRef,
        value: f64,
        expected_range: (f64, f64),
        severity: ValidationSeverity,
    },
}

/// Validation result
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// Whether validation passed
    pub is_valid: bool,
    /// Issues found during validation
    pub issues: Vec<ValidationIssue>,
    /// Statistics about the validation
    pub stats: ValidationStats,
    /// Suggestions for fixing issues
    pub suggestions: Vec<String>,
}

/// Validation statistics
#[derive(Debug, Clone, Default)]
pub struct ValidationStats {
    /// Total entities checked
    pub entities_checked: usize,
    /// Total constraints checked
    pub constraints_checked: usize,
    /// Number of info-level issues
    pub info_count: usize,
    /// Number of warnings
    pub warning_count: usize,
    /// Number of errors
    pub error_count: usize,
    /// Number of critical issues
    pub critical_count: usize,
    /// Validation time in milliseconds
    pub validation_time_ms: f64,
}

/// Validation configuration
#[derive(Debug, Clone)]
pub struct ValidationConfig {
    /// Tolerance for geometric checks
    pub tolerance: Tolerance2d,
    /// Check for self-intersections
    pub check_self_intersections: bool,
    /// Check for overlapping entities
    pub check_overlaps: bool,
    /// Check constraint satisfaction
    pub check_constraints: bool,
    /// Check topology validity
    pub check_topology: bool,
    /// Maximum allowed numerical error
    pub max_numerical_error: f64,
    /// Minimum allowed entity size
    pub min_entity_size: f64,
    /// Maximum sketch bounds
    pub max_bounds: Option<(Point2d, Point2d)>,
}

impl Default for ValidationConfig {
    fn default() -> Self {
        Self {
            tolerance: Tolerance2d::default(),
            check_self_intersections: true,
            check_overlaps: true,
            check_constraints: true,
            check_topology: true,
            max_numerical_error: 1e-10,
            min_entity_size: 1e-6,
            max_bounds: Some((
                Point2d::new(-10000.0, -10000.0),
                Point2d::new(10000.0, 10000.0),
            )),
        }
    }
}

/// Sketch validator
pub struct SketchValidator {
    config: ValidationConfig,
}

impl SketchValidator {
    /// Create a new validator with default config
    pub fn new() -> Self {
        Self {
            config: ValidationConfig::default(),
        }
    }

    /// Create a validator with custom config
    pub fn with_config(config: ValidationConfig) -> Self {
        Self { config }
    }

    /// Validate a sketch
    pub fn validate(&self, sketch: &Sketch) -> ValidationResult {
        let start_time = std::time::Instant::now();
        let mut issues = Vec::new();
        let mut stats = ValidationStats::default();

        // Validate individual entities
        self.validate_entities(sketch, &mut issues, &mut stats);

        // Check for self-intersections
        if self.config.check_self_intersections {
            self.check_self_intersections(sketch, &mut issues, &mut stats);
        }

        // Check for overlaps
        if self.config.check_overlaps {
            self.check_overlapping_entities(sketch, &mut issues, &mut stats);
        }

        // Validate constraints
        if self.config.check_constraints {
            self.validate_constraints(sketch, &mut issues, &mut stats);
        }

        // Validate topology
        if self.config.check_topology {
            self.validate_topology(sketch, &mut issues, &mut stats);
        }

        // Count issues by severity
        for issue in &issues {
            match issue.severity() {
                ValidationSeverity::Info => stats.info_count += 1,
                ValidationSeverity::Warning => stats.warning_count += 1,
                ValidationSeverity::Error => stats.error_count += 1,
                ValidationSeverity::Critical => stats.critical_count += 1,
            }
        }

        // Generate suggestions
        let suggestions = self.generate_suggestions(&issues);

        // Determine if valid
        let is_valid = stats.error_count == 0 && stats.critical_count == 0;

        stats.validation_time_ms = start_time.elapsed().as_secs_f64() * 1000.0;

        ValidationResult {
            is_valid,
            issues,
            stats,
            suggestions,
        }
    }

    /// Validate individual entities
    fn validate_entities(
        &self,
        sketch: &Sketch,
        issues: &mut Vec<ValidationIssue>,
        stats: &mut ValidationStats,
    ) {
        // Validate points
        for entry in sketch.points().iter() {
            let point = entry.value();
            stats.entities_checked += 1;

            // Check bounds
            if let Some((min, max)) = self.config.max_bounds {
                if !self.point_in_bounds(&point.position, &min, &max) {
                    issues.push(ValidationIssue::OutOfBounds {
                        entity: EntityRef::Point(point.id),
                        bounds: (min, max),
                        severity: ValidationSeverity::Warning,
                    });
                }
            }

            // Check numerical validity
            if !point.position.x.is_finite() || !point.position.y.is_finite() {
                issues.push(ValidationIssue::NumericalPrecision {
                    entity: EntityRef::Point(point.id),
                    value: f64::NAN,
                    expected_range: (f64::NEG_INFINITY, f64::INFINITY),
                    severity: ValidationSeverity::Critical,
                });
            }
        }

        // Validate lines
        for entry in sketch.lines().iter() {
            let line = entry.value();
            stats.entities_checked += 1;

            match &line.geometry {
                LineGeometry::Segment(seg) => {
                    // Check for zero-length segments
                    let length = seg.length();
                    if length < self.config.min_entity_size {
                        issues.push(ValidationIssue::ZeroLengthLine {
                            entity: EntityRef::Line(line.id),
                            severity: ValidationSeverity::Error,
                        });
                    }

                    // Check endpoints
                    if let Some((min, max)) = self.config.max_bounds {
                        if !self.point_in_bounds(&seg.start, &min, &max)
                            || !self.point_in_bounds(&seg.end, &min, &max)
                        {
                            issues.push(ValidationIssue::OutOfBounds {
                                entity: EntityRef::Line(line.id),
                                bounds: (min, max),
                                severity: ValidationSeverity::Warning,
                            });
                        }
                    }
                }
                LineGeometry::Infinite(_) => {
                    // Infinite lines are always valid geometrically
                }
                LineGeometry::Ray(_) => {
                    // Rays are always valid geometrically
                }
            }
        }

        // Validate arcs
        for entry in sketch.arcs().iter() {
            let arc = entry.value();
            stats.entities_checked += 1;

            // Check radius
            if arc.arc.radius < self.config.min_entity_size {
                issues.push(ValidationIssue::DegenerateArc {
                    entity: EntityRef::Arc(arc.id),
                    reason: "Zero or negative radius".to_string(),
                    severity: ValidationSeverity::Error,
                });
            }

            // Check angle span
            let angle_span = if arc.arc.ccw {
                if arc.arc.end_angle >= arc.arc.start_angle {
                    arc.arc.end_angle - arc.arc.start_angle
                } else {
                    2.0 * std::f64::consts::PI - (arc.arc.start_angle - arc.arc.end_angle)
                }
            } else {
                if arc.arc.start_angle >= arc.arc.end_angle {
                    arc.arc.start_angle - arc.arc.end_angle
                } else {
                    2.0 * std::f64::consts::PI - (arc.arc.end_angle - arc.arc.start_angle)
                }
            };
            if angle_span.abs() < self.config.tolerance.angle {
                issues.push(ValidationIssue::DegenerateArc {
                    entity: EntityRef::Arc(arc.id),
                    reason: "Zero angle span".to_string(),
                    severity: ValidationSeverity::Error,
                });
            }

            // Check center bounds
            if let Some((min, max)) = self.config.max_bounds {
                let (arc_min, arc_max) = arc.bounding_box();
                if !self.box_in_bounds(&arc_min, &arc_max, &min, &max) {
                    issues.push(ValidationIssue::OutOfBounds {
                        entity: EntityRef::Arc(arc.id),
                        bounds: (min, max),
                        severity: ValidationSeverity::Warning,
                    });
                }
            }
        }

        // Validate circles
        for entry in sketch.circles().iter() {
            let circle = entry.value();
            stats.entities_checked += 1;

            // Check radius
            if circle.circle.radius < self.config.min_entity_size {
                issues.push(ValidationIssue::InvalidEntity {
                    entity: EntityRef::Circle(circle.id),
                    reason: "Zero or negative radius".to_string(),
                    severity: ValidationSeverity::Error,
                });
            }

            // Check bounds
            if let Some((min, max)) = self.config.max_bounds {
                let (circle_min, circle_max) = circle.bounding_box();
                if !self.box_in_bounds(&circle_min, &circle_max, &min, &max) {
                    issues.push(ValidationIssue::OutOfBounds {
                        entity: EntityRef::Circle(circle.id),
                        bounds: (min, max),
                        severity: ValidationSeverity::Warning,
                    });
                }
            }
        }

        // Similar validation for rectangles, ellipses, splines, polylines...
    }

    /// Check for self-intersecting entities
    fn check_self_intersections(
        &self,
        sketch: &Sketch,
        issues: &mut Vec<ValidationIssue>,
        _stats: &mut ValidationStats,
    ) {
        // Check polylines for self-intersection
        for entry in sketch.polylines().iter() {
            let polyline = entry.value();
            let vertices = &polyline.polyline.vertices;

            // Check each segment against all non-adjacent segments
            for i in 0..vertices.len() - 1 {
                for j in i + 2..vertices.len() - 1 {
                    if i == 0 && j == vertices.len() - 2 && polyline.polyline.is_closed {
                        continue; // Skip adjacent segments in closed polyline
                    }

                    if let Some(intersection) = self.segment_intersection(
                        &vertices[i],
                        &vertices[i + 1],
                        &vertices[j],
                        &vertices[j + 1],
                    ) {
                        issues.push(ValidationIssue::SelfIntersection {
                            entity: EntityRef::Polyline(polyline.id),
                            points: vec![intersection],
                            severity: ValidationSeverity::Error,
                        });
                    }
                }
            }
        }

        // Check splines for self-intersection (would need curve-curve intersection)
        // This is complex and would require numerical methods
    }

    /// Check for overlapping entities
    fn check_overlapping_entities(
        &self,
        sketch: &Sketch,
        issues: &mut Vec<ValidationIssue>,
        _stats: &mut ValidationStats,
    ) {
        // Build a list of all entities with their bounding boxes
        let mut entities: Vec<(EntityRef, Point2d, Point2d)> = Vec::new();

        // Add all entity types
        for entry in sketch.lines().iter() {
            let (min, max) = entry.value().bounding_box();
            entities.push((EntityRef::Line(*entry.key()), min, max));
        }

        for entry in sketch.arcs().iter() {
            let (min, max) = entry.value().bounding_box();
            entities.push((EntityRef::Arc(*entry.key()), min, max));
        }

        // Add other entity types...

        // Check for overlaps using spatial partitioning
        for i in 0..entities.len() {
            for j in i + 1..entities.len() {
                let (entity1, min1, max1) = &entities[i];
                let (entity2, min2, max2) = &entities[j];

                // Quick bounding box check
                if self.boxes_overlap(min1, max1, min2, max2) {
                    // Detailed overlap check would go here
                    // For now, just flag potential overlaps
                    if self.entities_might_overlap(sketch, entity1, entity2) {
                        issues.push(ValidationIssue::OverlappingEntities {
                            entity1: *entity1,
                            entity2: *entity2,
                            severity: ValidationSeverity::Warning,
                        });
                    }
                }
            }
        }
    }

    /// Validate constraints
    fn validate_constraints(
        &self,
        sketch: &Sketch,
        issues: &mut Vec<ValidationIssue>,
        stats: &mut ValidationStats,
    ) {
        let constraints = sketch.all_constraints();
        stats.constraints_checked = constraints.len();

        // Check each constraint
        for constraint in &constraints {
            // Check if constraint is satisfied
            if let super::ConstraintStatus::Violated { error, .. } = constraint.status {
                if error > self.config.tolerance.distance {
                    issues.push(ValidationIssue::UnsatisfiedConstraint {
                        constraint_id: constraint.id,
                        error,
                        severity: ValidationSeverity::Error,
                    });
                }
            }
        }

        // Check for over/under-constrained entities
        let mut entity_constraints: HashMap<EntityRef, Vec<super::ConstraintId>> = HashMap::new();
        let mut entity_dof: HashMap<EntityRef, usize> = HashMap::new();

        // Count constraints per entity
        for constraint in &constraints {
            for entity in &constraint.entities {
                entity_constraints
                    .entry(*entity)
                    .or_default()
                    .push(constraint.id);
            }
        }

        // Check degrees of freedom
        for (entity, constraint_ids) in &entity_constraints {
            let dof = self.get_entity_dof(sketch, entity);
            let constraints_count = constraint_ids.len();

            entity_dof.insert(*entity, dof);

            if constraints_count > dof {
                // Over-constrained
                issues.push(ValidationIssue::OverConstrained {
                    entities: vec![*entity],
                    conflicting_constraints: constraint_ids.clone(),
                    severity: ValidationSeverity::Error,
                });
            }
        }
    }

    /// Validate topology for 3D operations
    fn validate_topology(
        &self,
        sketch: &Sketch,
        issues: &mut Vec<ValidationIssue>,
        _stats: &mut ValidationStats,
    ) {
        // Analyze topology
        match SketchTopology::analyze(sketch, &self.config.tolerance) {
            Ok(topology) => {
                // Check profile type
                match topology.profile_type() {
                    ProfileType::Open => {
                        // Find open endpoints
                        let endpoints = Vec::new();
                        // Would need to extract endpoints from topology

                        issues.push(ValidationIssue::OpenProfile {
                            endpoints,
                            severity: ValidationSeverity::Warning,
                        });
                    }
                    ProfileType::Mixed => {
                        issues.push(ValidationIssue::InvalidEntity {
                            entity: EntityRef::Point(super::Point2dId::new()), // Placeholder
                            reason: "Sketch contains both open and closed profiles".to_string(),
                            severity: ValidationSeverity::Warning,
                        });
                    }
                    _ => {} // Simple, Disjoint, Nested are valid
                }

                // Check for topology issues
                for issue in topology.issues() {
                    match issue {
                        TopologyIssue::Gap {
                            entity1,
                            entity2: _,
                            distance,
                        } => {
                            if *distance > self.config.tolerance.distance {
                                issues.push(ValidationIssue::InvalidEntity {
                                    entity: *entity1,
                                    reason: format!("Gap of {} to another entity", distance),
                                    severity: ValidationSeverity::Error,
                                });
                            }
                        }
                        TopologyIssue::TJunction {
                            edge1,
                            edge2,
                            point,
                        } => {
                            issues.push(ValidationIssue::TJunction {
                                point: *point,
                                entities: vec![*edge1, *edge2],
                                severity: ValidationSeverity::Warning,
                            });
                        }
                        _ => {} // Handle other topology issues
                    }
                }

                // Check loop nesting
                for region in topology.regions().iter() {
                    for &inner_loop in &region.inner_loops {
                        // Verify proper nesting
                        if !self.is_loop_inside_loop(
                            &topology.loops()[region.outer_loop],
                            &topology.loops()[inner_loop],
                        ) {
                            issues.push(ValidationIssue::InvalidNesting {
                                outer_loop: region.outer_loop,
                                inner_loop,
                                reason: "Inner loop not fully contained in outer loop".to_string(),
                                severity: ValidationSeverity::Error,
                            });
                        }
                    }
                }
            }
            Err(e) => {
                issues.push(ValidationIssue::InvalidEntity {
                    entity: EntityRef::Point(super::Point2dId::new()), // Placeholder
                    reason: format!("Topology analysis failed: {}", e),
                    severity: ValidationSeverity::Critical,
                });
            }
        }
    }

    /// Generate suggestions for fixing issues
    fn generate_suggestions(&self, issues: &[ValidationIssue]) -> Vec<String> {
        let mut suggestions = Vec::new();
        let mut has_zero_length = false;
        let mut has_gaps = false;
        let mut has_over_constrained = false;
        let mut has_under_constrained = false;

        for issue in issues {
            match issue {
                ValidationIssue::ZeroLengthLine { .. } => has_zero_length = true,
                ValidationIssue::InvalidEntity { reason, .. } if reason.contains("Gap") => {
                    has_gaps = true;
                }
                ValidationIssue::OverConstrained { .. } => has_over_constrained = true,
                ValidationIssue::UnderConstrained { .. } => has_under_constrained = true,
                _ => {}
            }
        }

        if has_zero_length {
            suggestions
                .push("Remove zero-length line segments or merge coincident points".to_string());
        }

        if has_gaps {
            suggestions
                .push("Use coincident constraints to close gaps between entities".to_string());
        }

        if has_over_constrained {
            suggestions.push(
                "Remove redundant constraints to resolve over-constrained entities".to_string(),
            );
        }

        if has_under_constrained {
            suggestions.push(
                "Add dimensional or geometric constraints to fully define the sketch".to_string(),
            );
        }

        suggestions
    }

    // Helper methods

    fn point_in_bounds(&self, point: &Point2d, min: &Point2d, max: &Point2d) -> bool {
        point.x >= min.x && point.x <= max.x && point.y >= min.y && point.y <= max.y
    }

    fn box_in_bounds(
        &self,
        box_min: &Point2d,
        box_max: &Point2d,
        bounds_min: &Point2d,
        bounds_max: &Point2d,
    ) -> bool {
        box_min.x >= bounds_min.x
            && box_max.x <= bounds_max.x
            && box_min.y >= bounds_min.y
            && box_max.y <= bounds_max.y
    }

    fn boxes_overlap(
        &self,
        min1: &Point2d,
        max1: &Point2d,
        min2: &Point2d,
        max2: &Point2d,
    ) -> bool {
        !(max1.x < min2.x || min1.x > max2.x || max1.y < min2.y || min1.y > max2.y)
    }

    fn segment_intersection(
        &self,
        p1: &Point2d,
        p2: &Point2d,
        p3: &Point2d,
        p4: &Point2d,
    ) -> Option<Point2d> {
        let d1 = Vector2d::new(p2.x - p1.x, p2.y - p1.y);
        let d2 = Vector2d::new(p4.x - p3.x, p4.y - p3.y);
        let d3 = Vector2d::new(p3.x - p1.x, p3.y - p1.y);

        let cross = d1.cross(&d2);

        if cross.abs() < 1e-10 {
            return None; // Parallel
        }

        let t1 = d3.cross(&d2) / cross;
        let t2 = d3.cross(&d1) / cross;

        if (0.0..=1.0).contains(&t1) && (0.0..=1.0).contains(&t2) {
            Some(Point2d::new(p1.x + t1 * d1.x, p1.y + t1 * d1.y))
        } else {
            None
        }
    }

    fn get_entity_dof(&self, _sketch: &Sketch, entity: &EntityRef) -> usize {
        match entity {
            EntityRef::Point(_) => 2,     // x, y
            EntityRef::Line(_) => 4,      // 2 points or point + direction
            EntityRef::Arc(_) => 5,       // center + radius + 2 angles
            EntityRef::Circle(_) => 3,    // center + radius
            EntityRef::Rectangle(_) => 5, // center + width + height + rotation
            EntityRef::Ellipse(_) => 5,   // center + 2 radii + rotation
            EntityRef::Spline(_) => 0,    // Complex - depends on control points
            EntityRef::Polyline(_) => 0,  // Complex - depends on vertices
        }
    }

    fn entities_might_overlap(
        &self,
        _sketch: &Sketch,
        _entity1: &EntityRef,
        _entity2: &EntityRef,
    ) -> bool {
        // This would need detailed geometric intersection tests
        // For now, return false to avoid false positives
        false
    }

    fn is_loop_inside_loop(&self, _outer: &SketchLoop, _inner: &SketchLoop) -> bool {
        // Check if inner loop is fully contained in outer loop
        // This would use point-in-polygon tests
        true // Placeholder
    }
}

impl ValidationIssue {
    /// Get the severity of this issue
    pub fn severity(&self) -> ValidationSeverity {
        match self {
            ValidationIssue::InvalidEntity { severity, .. } => *severity,
            ValidationIssue::ZeroLengthLine { severity, .. } => *severity,
            ValidationIssue::DegenerateArc { severity, .. } => *severity,
            ValidationIssue::SelfIntersection { severity, .. } => *severity,
            ValidationIssue::OverlappingEntities { severity, .. } => *severity,
            ValidationIssue::UnsatisfiedConstraint { severity, .. } => *severity,
            ValidationIssue::OverConstrained { severity, .. } => *severity,
            ValidationIssue::UnderConstrained { severity, .. } => *severity,
            ValidationIssue::OpenProfile { severity, .. } => *severity,
            ValidationIssue::InvalidNesting { severity, .. } => *severity,
            ValidationIssue::TJunction { severity, .. } => *severity,
            ValidationIssue::OutOfBounds { severity, .. } => *severity,
            ValidationIssue::NumericalPrecision { severity, .. } => *severity,
        }
    }
}

/// Quick validation for common checks
pub mod quick_checks {
    use super::*;

    /// Check if a sketch is ready for extrusion
    pub fn is_ready_for_extrusion(sketch: &Sketch) -> bool {
        let validator = SketchValidator::new();
        let result = validator.validate(sketch);

        result.is_valid && matches!(result.stats.error_count, 0)
    }

    /// Check if a sketch has any critical issues
    pub fn has_critical_issues(sketch: &Sketch) -> bool {
        let validator = SketchValidator::new();
        let result = validator.validate(sketch);

        result.stats.critical_count > 0
    }

    /// Get a quick summary of sketch health
    pub fn sketch_health(sketch: &Sketch) -> String {
        let validator = SketchValidator::new();
        let result = validator.validate(sketch);

        if result.stats.critical_count > 0 {
            "Critical issues found".to_string()
        } else if result.stats.error_count > 0 {
            "Errors need fixing".to_string()
        } else if result.stats.warning_count > 0 {
            "Has warnings but usable".to_string()
        } else {
            "Healthy".to_string()
        }
    }
}

/// Advanced sketch analysis tools
pub mod analysis_tools {
    use super::*;

    /// Constraint analysis results
    #[derive(Debug, Clone)]
    pub struct ConstraintAnalysis {
        /// Total number of constraints
        pub total_constraints: usize,
        /// Number of satisfied constraints  
        pub satisfied_constraints: usize,
        /// Number of violated constraints
        pub violated_constraints: usize,
        /// Number of conflicting constraints
        pub conflicting_constraints: usize,
        /// Over-constrained entities
        pub over_constrained_entities: Vec<EntityRef>,
        /// Under-constrained entities  
        pub under_constrained_entities: Vec<EntityRef>,
        /// Degrees of freedom remaining
        pub degrees_of_freedom: isize,
        /// Constraint conflicts detected
        pub conflicts: Vec<(ConstraintId, ConstraintId)>,
    }

    /// Entity constraint status
    #[derive(Debug, Clone, PartialEq)]
    pub enum EntityConstraintStatus {
        /// Entity is fully constrained (no remaining DOF)
        FullyConstrained,
        /// Entity has optimal constraint level
        WellConstrained,
        /// Entity needs more constraints
        UnderConstrained { missing_dof: usize },
        /// Entity has too many constraints
        OverConstrained { excess_constraints: usize },
        /// Entity has conflicting constraints
        Conflicted,
    }

    /// Sketch design quality metrics
    #[derive(Debug, Clone)]
    pub struct DesignQuality {
        /// Overall quality score (0.0 to 1.0)
        pub overall_score: f64,
        /// Constraint organization score
        pub constraint_organization: f64,
        /// Geometric stability score
        pub geometric_stability: f64,
        /// Manufacturability score
        pub manufacturability: f64,
        /// Performance recommendations
        pub recommendations: Vec<String>,
    }

    /// Sketch analysis engine
    pub struct SketchAnalyzer;

    impl SketchAnalyzer {
        /// Create new sketch analyzer
        pub fn new() -> Self {
            Self
        }

        /// Analyze constraint status of all entities in sketch
        pub fn analyze_constraints(&self, sketch: &Sketch) -> ConstraintAnalysis {
            let mut analysis = ConstraintAnalysis {
                total_constraints: 0,
                satisfied_constraints: 0,
                violated_constraints: 0,
                conflicting_constraints: 0,
                over_constrained_entities: Vec::new(),
                under_constrained_entities: Vec::new(),
                degrees_of_freedom: 0,
                conflicts: Vec::new(),
            };

            // Analyze each entity type
            self.analyze_points_constraints(sketch, &mut analysis);
            self.analyze_lines_constraints(sketch, &mut analysis);
            self.analyze_arcs_constraints(sketch, &mut analysis);
            self.analyze_circles_constraints(sketch, &mut analysis);

            // Find constraint conflicts
            analysis.conflicts = sketch.find_constraint_conflicts();
            analysis.conflicting_constraints = analysis.conflicts.len();

            analysis
        }

        /// Get constraint status for a specific entity
        pub fn get_entity_constraint_status(
            &self,
            sketch: &Sketch,
            entity: &EntityRef,
        ) -> EntityConstraintStatus {
            // Get constraints for entity via public interface
            let constraints = sketch.get_constraints_by_entity(entity);
            let entity_dof = self.calculate_entity_dof(entity);

            let constraint_count = constraints.len();

            // Check for conflicts first
            for constraint in &constraints {
                if matches!(constraint.status, ConstraintStatus::Conflicting) {
                    return EntityConstraintStatus::Conflicted;
                }
            }

            // Analyze constraint level
            match constraint_count.cmp(&entity_dof) {
                std::cmp::Ordering::Equal => EntityConstraintStatus::FullyConstrained,
                std::cmp::Ordering::Less => {
                    if constraint_count == 0 {
                        EntityConstraintStatus::UnderConstrained {
                            missing_dof: entity_dof,
                        }
                    } else if constraint_count >= entity_dof * 2 / 3 {
                        EntityConstraintStatus::WellConstrained
                    } else {
                        EntityConstraintStatus::UnderConstrained {
                            missing_dof: entity_dof - constraint_count,
                        }
                    }
                }
                std::cmp::Ordering::Greater => EntityConstraintStatus::OverConstrained {
                    excess_constraints: constraint_count - entity_dof,
                },
            }
        }

        /// Calculate degrees of freedom for an entity
        fn calculate_entity_dof(&self, entity: &EntityRef) -> usize {
            match entity {
                EntityRef::Point(_) => 2,     // x, y
                EntityRef::Line(_) => 4,      // 2 endpoints * 2 coords
                EntityRef::Arc(_) => 5,       // center (2) + radius + start angle + end angle
                EntityRef::Circle(_) => 3,    // center (2) + radius
                EntityRef::Rectangle(_) => 5, // center (2) + width + height + rotation
                EntityRef::Ellipse(_) => 5,   // center (2) + major + minor + rotation
                EntityRef::Spline(_) => 8,    // Estimate: depends on control points
                EntityRef::Polyline(_) => 6,  // Estimate: depends on vertices
            }
        }

        /// Analyze design quality and provide recommendations
        pub fn analyze_design_quality(&self, sketch: &Sketch) -> DesignQuality {
            let constraint_analysis = self.analyze_constraints(sketch);

            // Calculate sub-scores
            let constraint_score = self.calculate_constraint_quality_score(&constraint_analysis);
            let stability_score = self.calculate_stability_score(sketch);
            let manufacturability_score = self.calculate_manufacturability_score(sketch);

            // Overall score (weighted average)
            let overall_score =
                constraint_score * 0.4 + stability_score * 0.4 + manufacturability_score * 0.2;

            // Generate recommendations
            let mut recommendations = Vec::new();

            if constraint_score < 0.7 {
                recommendations
                    .push("Consider adding more constraints to stabilize geometry".to_string());
            }

            if constraint_analysis.conflicting_constraints > 0 {
                recommendations
                    .push("Resolve conflicting constraints for better stability".to_string());
            }

            if !constraint_analysis.under_constrained_entities.is_empty() {
                recommendations.push("Some entities need additional constraints".to_string());
            }

            if manufacturability_score < 0.6 {
                recommendations.push(
                    "Consider manufacturability constraints (minimum radii, etc.)".to_string(),
                );
            }

            DesignQuality {
                overall_score,
                constraint_organization: constraint_score,
                geometric_stability: stability_score,
                manufacturability: manufacturability_score,
                recommendations,
            }
        }

        /// Calculate constraint quality score
        fn calculate_constraint_quality_score(&self, analysis: &ConstraintAnalysis) -> f64 {
            if analysis.total_constraints == 0 {
                return 0.0;
            }

            let satisfaction_ratio =
                analysis.satisfied_constraints as f64 / analysis.total_constraints as f64;
            let conflict_penalty =
                analysis.conflicting_constraints as f64 / analysis.total_constraints as f64;

            (satisfaction_ratio - conflict_penalty * 0.5).max(0.0)
        }

        /// Calculate geometric stability score
        fn calculate_stability_score(&self, _sketch: &Sketch) -> f64 {
            // This would analyze geometric stability
            // For now, return a placeholder score
            0.8
        }

        /// Calculate manufacturability score
        fn calculate_manufacturability_score(&self, _sketch: &Sketch) -> f64 {
            // This would analyze manufacturability constraints:
            // - Minimum feature sizes
            // - Corner radii
            // - Draft angles
            // - Material considerations
            0.7
        }

        /// Suggest fixes for constraint issues
        pub fn suggest_constraint_fixes(&self, sketch: &Sketch) -> Vec<String> {
            let analysis = self.analyze_constraints(sketch);
            let mut suggestions = Vec::new();

            // Suggestions for under-constrained entities
            for entity in &analysis.under_constrained_entities {
                match entity {
                    EntityRef::Point(_) => {
                        suggestions.push(format!(
                            "Fix point {} position with coordinate or distance constraints",
                            entity
                        ));
                    }
                    EntityRef::Line(_) => {
                        suggestions.push(format!(
                            "Add length, angle, or endpoint constraints to line {}",
                            entity
                        ));
                    }
                    EntityRef::Circle(_) => {
                        suggestions.push(format!(
                            "Add radius and center position constraints to circle {}",
                            entity
                        ));
                    }
                    _ => {
                        suggestions.push(format!(
                            "Add positioning and sizing constraints to {}",
                            entity
                        ));
                    }
                }
            }

            // Suggestions for over-constrained entities
            for entity in &analysis.over_constrained_entities {
                suggestions.push(format!(
                    "Remove redundant constraints from {} to avoid conflicts",
                    entity
                ));
            }

            // Suggestions for conflicts
            if analysis.conflicting_constraints > 0 {
                suggestions.push(
                    "Review conflicting constraints and remove or modify incompatible ones"
                        .to_string(),
                );
            }

            suggestions
        }

        // Helper methods for constraint analysis
        fn analyze_points_constraints(&self, sketch: &Sketch, analysis: &mut ConstraintAnalysis) {
            for point_entry in sketch.points().iter() {
                let entity_ref = EntityRef::Point(*point_entry.key());
                let status = self.get_entity_constraint_status(sketch, &entity_ref);

                match status {
                    EntityConstraintStatus::UnderConstrained { .. } => {
                        analysis.under_constrained_entities.push(entity_ref);
                    }
                    EntityConstraintStatus::OverConstrained { .. } => {
                        analysis.over_constrained_entities.push(entity_ref);
                    }
                    _ => {}
                }
            }
        }

        fn analyze_lines_constraints(&self, sketch: &Sketch, analysis: &mut ConstraintAnalysis) {
            for line_entry in sketch.lines().iter() {
                let entity_ref = EntityRef::Line(*line_entry.key());
                let status = self.get_entity_constraint_status(sketch, &entity_ref);

                match status {
                    EntityConstraintStatus::UnderConstrained { .. } => {
                        analysis.under_constrained_entities.push(entity_ref);
                    }
                    EntityConstraintStatus::OverConstrained { .. } => {
                        analysis.over_constrained_entities.push(entity_ref);
                    }
                    _ => {}
                }
            }
        }

        fn analyze_arcs_constraints(&self, sketch: &Sketch, analysis: &mut ConstraintAnalysis) {
            for arc_entry in sketch.arcs().iter() {
                let entity_ref = EntityRef::Arc(*arc_entry.key());
                let status = self.get_entity_constraint_status(sketch, &entity_ref);

                match status {
                    EntityConstraintStatus::UnderConstrained { .. } => {
                        analysis.under_constrained_entities.push(entity_ref);
                    }
                    EntityConstraintStatus::OverConstrained { .. } => {
                        analysis.over_constrained_entities.push(entity_ref);
                    }
                    _ => {}
                }
            }
        }

        fn analyze_circles_constraints(&self, sketch: &Sketch, analysis: &mut ConstraintAnalysis) {
            for circle_entry in sketch.circles().iter() {
                let entity_ref = EntityRef::Circle(*circle_entry.key());
                let status = self.get_entity_constraint_status(sketch, &entity_ref);

                match status {
                    EntityConstraintStatus::UnderConstrained { .. } => {
                        analysis.under_constrained_entities.push(entity_ref);
                    }
                    EntityConstraintStatus::OverConstrained { .. } => {
                        analysis.over_constrained_entities.push(entity_ref);
                    }
                    _ => {}
                }
            }
        }
    }

    /// Quick analysis functions
    pub fn is_sketch_fully_constrained(sketch: &Sketch) -> bool {
        let analyzer = SketchAnalyzer::new();
        let analysis = analyzer.analyze_constraints(sketch);

        analysis.under_constrained_entities.is_empty() && analysis.conflicting_constraints == 0
    }

    pub fn get_sketch_dof_count(sketch: &Sketch) -> isize {
        let analyzer = SketchAnalyzer::new();
        let analysis = analyzer.analyze_constraints(sketch);
        analysis.degrees_of_freedom
    }

    pub fn find_problematic_entities(sketch: &Sketch) -> Vec<(EntityRef, EntityConstraintStatus)> {
        let analyzer = SketchAnalyzer::new();
        let mut problematic = Vec::new();

        // Check all entity types
        for point_entry in sketch.points().iter() {
            let entity_ref = EntityRef::Point(*point_entry.key());
            let status = analyzer.get_entity_constraint_status(sketch, &entity_ref);

            match status {
                EntityConstraintStatus::UnderConstrained { .. }
                | EntityConstraintStatus::OverConstrained { .. }
                | EntityConstraintStatus::Conflicted => {
                    problematic.push((entity_ref, status));
                }
                _ => {}
            }
        }

        // Similar for other entity types...

        problematic
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_validation() {
        let sketch = Sketch::on_xy_plane("Test".to_string());
        let validator = SketchValidator::new();

        let result = validator.validate(&sketch);
        assert!(result.is_valid);
        assert_eq!(result.issues.len(), 0);
    }

    #[test]
    fn test_validation_config() {
        let mut config = ValidationConfig::default();
        config.min_entity_size = 1.0;
        config.check_self_intersections = false;

        let validator = SketchValidator::with_config(config);
        let sketch = Sketch::on_xy_plane("Test".to_string());

        let result = validator.validate(&sketch);
        assert!(result.is_valid);
    }
}
