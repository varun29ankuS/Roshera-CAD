//! 2D Constraint system for sketching
//!
//! This module implements geometric and dimensional constraints for 2D sketches.
//! Constraints define relationships between sketch entities that must be maintained.
//!
//! # Constraint Types
//!
//! ## Geometric Constraints
//! - Coincident: Two points occupy the same location
//! - Parallel: Two lines have the same direction
//! - Perpendicular: Two lines are at 90 degrees
//! - Tangent: A line is tangent to a curve
//! - Concentric: Two circles/arcs share the same center
//! - Equal: Two entities have the same dimension
//! - Horizontal/Vertical: A line is aligned with an axis
//! - Symmetric: Entities are symmetric about a line
//!
//! ## Dimensional Constraints
//! - Distance: Fixed distance between points or parallel lines
//! - Angle: Fixed angle between lines
//! - Radius: Fixed radius for circles/arcs
//! - Length: Fixed length for line segments

use super::{
    Arc2dId, Circle2dId, Ellipse2dId, Line2dId, Point2dId, Polyline2dId, Rectangle2dId, Spline2dId,
};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use uuid::Uuid;

/// Unique identifier for a constraint
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConstraintId(pub Uuid);

impl ConstraintId {
    /// Create a new unique constraint ID
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for ConstraintId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Constraint_{}", &self.0.to_string()[..8])
    }
}

/// Entity reference for constraints
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub enum EntityRef {
    Point(Point2dId),
    Line(Line2dId),
    Arc(Arc2dId),
    Circle(Circle2dId),
    Rectangle(Rectangle2dId),
    Ellipse(Ellipse2dId),
    Spline(Spline2dId),
    Polyline(Polyline2dId),
}

impl fmt::Display for EntityRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EntityRef::Point(id) => write!(f, "{}", id),
            EntityRef::Line(id) => write!(f, "{}", id),
            EntityRef::Arc(id) => write!(f, "{}", id),
            EntityRef::Circle(id) => write!(f, "{}", id),
            EntityRef::Rectangle(id) => write!(f, "{}", id),
            EntityRef::Ellipse(id) => write!(f, "{}", id),
            EntityRef::Spline(id) => write!(f, "{}", id),
            EntityRef::Polyline(id) => write!(f, "{}", id),
        }
    }
}

/// Geometric constraint types
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum GeometricConstraint {
    /// Two points are coincident
    Coincident,
    /// Two lines are parallel
    Parallel,
    /// Two lines are perpendicular
    Perpendicular,
    /// A line is tangent to a curve
    Tangent,
    /// Two circles/arcs are concentric
    Concentric,
    /// Two entities have equal dimension
    Equal,
    /// A line is horizontal
    Horizontal,
    /// A line is vertical
    Vertical,
    /// Entities are symmetric about a line
    Symmetric,
    /// A point lies on a curve
    PointOnCurve,
    /// A point is at the midpoint of a line
    Midpoint,
    /// Lines or curves are collinear
    Collinear,

    // Advanced constraint types
    /// Smooth tangent continuity between curves (G1 continuity)
    SmoothTangent,
    /// Curvature continuity between curves (G2 continuity)
    CurvatureContinuity,
    /// Two entities are offset by a fixed distance
    Offset,
    /// A curve is tangent to multiple entities (multi-tangent)
    MultiTangent,
    /// Entities maintain a fixed area relationship
    EqualArea,
    /// Entities maintain a fixed perimeter relationship
    EqualPerimeter,
    /// Point lies at the center of mass of a closed curve
    Centroid,
    /// Curve has minimum or maximum curvature at a point
    CurvatureExtremum,
    /// Two curves intersect at a specific angle
    IntersectionAngle(f64),
    /// Entity maintains contact with a boundary
    ContactConstraint,
}

/// Dimensional constraint types
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum DimensionalConstraint {
    /// Fixed distance between points
    Distance(f64),
    /// Fixed angle between lines (in radians)
    Angle(f64),
    /// Fixed radius for circle/arc
    Radius(f64),
    /// Fixed diameter for circle
    Diameter(f64),
    /// Fixed length for line segment
    Length(f64),
    /// Fixed X coordinate
    XCoordinate(f64),
    /// Fixed Y coordinate
    YCoordinate(f64),

    // Advanced dimensional constraints
    /// Fixed area for closed curves
    Area(f64),
    /// Fixed perimeter for closed curves
    Perimeter(f64),
    /// Fixed arc length for curves
    ArcLength(f64),
    /// Fixed curvature at a point
    Curvature(f64),
    /// Fixed slope (dy/dx) at a point
    Slope(f64),
    /// Fixed offset distance from a curve
    OffsetDistance(f64),
    /// Fixed aspect ratio (width/height)
    AspectRatio(f64),
    /// Fixed minimum distance between entities
    MinDistance(f64),
    /// Fixed maximum distance between entities
    MaxDistance(f64),
    /// Fixed moment of inertia
    MomentOfInertia(f64),
    /// Fixed center of mass position
    CenterOfMass { x: f64, y: f64 },
}

/// Combined constraint type
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ConstraintType {
    Geometric(GeometricConstraint),
    Dimensional(DimensionalConstraint),
}

/// Constraint status
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ConstraintStatus {
    /// Constraint is satisfied
    Satisfied,
    /// Constraint is violated
    Violated {
        /// Current error/deviation
        error: f64,
        /// Suggested correction
        suggestion: Option<f64>,
    },
    /// Constraint is temporarily disabled
    Disabled,
    /// Constraint conflicts with others
    Conflicting,
}

/// Constraint priority for solver
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ConstraintPriority {
    /// Cannot be violated (e.g., user-fixed points)
    Required = 0,
    /// High priority (most constraints)
    High = 1,
    /// Medium priority
    Medium = 2,
    /// Low priority (can be relaxed if needed)
    Low = 3,
}

/// A constraint between sketch entities
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Constraint {
    /// Unique identifier
    pub id: ConstraintId,
    /// Type of constraint
    pub constraint_type: ConstraintType,
    /// Entities involved in the constraint
    pub entities: Vec<EntityRef>,
    /// Priority for solving
    pub priority: ConstraintPriority,
    /// Current status
    pub status: ConstraintStatus,
    /// User-defined name (optional)
    pub name: Option<String>,
}

impl Constraint {
    /// Create a new geometric constraint
    pub fn new_geometric(
        constraint_type: GeometricConstraint,
        entities: Vec<EntityRef>,
        priority: ConstraintPriority,
    ) -> Self {
        Self {
            id: ConstraintId::new(),
            constraint_type: ConstraintType::Geometric(constraint_type),
            entities,
            priority,
            status: ConstraintStatus::Satisfied,
            name: None,
        }
    }

    /// Create a new dimensional constraint
    pub fn new_dimensional(
        constraint_type: DimensionalConstraint,
        entities: Vec<EntityRef>,
        priority: ConstraintPriority,
    ) -> Self {
        Self {
            id: ConstraintId::new(),
            constraint_type: ConstraintType::Dimensional(constraint_type),
            entities,
            priority,
            status: ConstraintStatus::Satisfied,
            name: None,
        }
    }

    /// Check if constraint involves a specific entity
    pub fn involves_entity(&self, entity: &EntityRef) -> bool {
        self.entities.contains(entity)
    }

    /// Get the number of degrees of freedom this constraint removes
    pub fn degrees_of_freedom_removed(&self) -> usize {
        match &self.constraint_type {
            ConstraintType::Geometric(g) => match g {
                GeometricConstraint::Coincident => 2,    // Removes X and Y
                GeometricConstraint::Parallel => 1,      // Removes angle
                GeometricConstraint::Perpendicular => 1, // Removes angle
                GeometricConstraint::Tangent => 1,       // Removes one DOF
                GeometricConstraint::Concentric => 2,    // Removes center position
                GeometricConstraint::Equal => 1,         // Removes one dimension
                GeometricConstraint::Horizontal => 1,    // Removes Y variation
                GeometricConstraint::Vertical => 1,      // Removes X variation
                GeometricConstraint::Symmetric => 2,     // Depends on entities
                GeometricConstraint::PointOnCurve => 1,  // One parameter
                GeometricConstraint::Midpoint => 2,      // X and Y
                GeometricConstraint::Collinear => 1,     // One DOF per entity

                // Advanced constraint types
                GeometricConstraint::SmoothTangent => 1,
                GeometricConstraint::CurvatureContinuity => 2,
                GeometricConstraint::Offset => 1,
                GeometricConstraint::MultiTangent => 1,
                GeometricConstraint::EqualArea => 1,
                GeometricConstraint::EqualPerimeter => 1,
                GeometricConstraint::Centroid => 2,
                GeometricConstraint::CurvatureExtremum => 1,
                GeometricConstraint::IntersectionAngle(_) => 1,
                GeometricConstraint::ContactConstraint => 1,
            },
            ConstraintType::Dimensional(d) => match d {
                DimensionalConstraint::Distance(_) => 1,
                DimensionalConstraint::Angle(_) => 1,
                DimensionalConstraint::Radius(_) => 1,
                DimensionalConstraint::Diameter(_) => 1,
                DimensionalConstraint::Length(_) => 1,
                DimensionalConstraint::XCoordinate(_) => 1,
                DimensionalConstraint::YCoordinate(_) => 1,

                // Advanced dimensional constraints
                DimensionalConstraint::Area(_) => 1,
                DimensionalConstraint::Perimeter(_) => 1,
                DimensionalConstraint::ArcLength(_) => 1,
                DimensionalConstraint::Curvature(_) => 1,
                DimensionalConstraint::Slope(_) => 1,
                DimensionalConstraint::OffsetDistance(_) => 1,
                DimensionalConstraint::AspectRatio(_) => 1,
                DimensionalConstraint::MinDistance(_) => 1,
                DimensionalConstraint::MaxDistance(_) => 1,
                DimensionalConstraint::MomentOfInertia(_) => 1,
                DimensionalConstraint::CenterOfMass { .. } => 2,
            },
        }
    }
}

/// Constraint storage using DashMap for concurrent access
pub struct ConstraintStore {
    /// All constraints indexed by ID
    constraints: Arc<DashMap<ConstraintId, Constraint>>,
    /// Constraints indexed by entity
    entity_constraints: Arc<DashMap<EntityRef, Vec<ConstraintId>>>,
    /// Constraint groups for related constraints
    constraint_groups: Arc<DashMap<String, Vec<ConstraintId>>>,
}

impl ConstraintStore {
    /// Create a new constraint store
    pub fn new() -> Self {
        Self {
            constraints: Arc::new(DashMap::new()),
            entity_constraints: Arc::new(DashMap::new()),
            constraint_groups: Arc::new(DashMap::new()),
        }
    }

    /// Add a constraint
    pub fn add_constraint(&self, constraint: Constraint) -> ConstraintId {
        let id = constraint.id;

        // Update entity index
        for entity in &constraint.entities {
            self.entity_constraints
                .entry(*entity)
                .or_insert_with(Vec::new)
                .push(id);
        }

        // Store constraint
        self.constraints.insert(id, constraint);

        id
    }

    /// Remove a constraint
    pub fn remove_constraint(&self, id: &ConstraintId) -> Option<Constraint> {
        if let Some((_, constraint)) = self.constraints.remove(id) {
            // Remove from entity index
            for entity in &constraint.entities {
                if let Some(mut entity_constraints) = self.entity_constraints.get_mut(entity) {
                    entity_constraints.retain(|&c| c != *id);
                }
            }

            Some(constraint)
        } else {
            None
        }
    }

    /// Get a constraint by ID
    pub fn get(&self, id: &ConstraintId) -> Option<Constraint> {
        self.constraints.get(id).map(|entry| entry.clone())
    }

    /// Get all constraints for an entity
    pub fn get_entity_constraints(&self, entity: &EntityRef) -> Vec<Constraint> {
        self.entity_constraints
            .get(entity)
            .map(|ids| ids.iter().filter_map(|id| self.get(id)).collect())
            .unwrap_or_default()
    }

    /// Update constraint status
    pub fn update_status(&self, id: &ConstraintId, status: ConstraintStatus) {
        if let Some(mut constraint) = self.constraints.get_mut(id) {
            constraint.status = status;
        }
    }

    /// Get all constraints
    pub fn all_constraints(&self) -> Vec<Constraint> {
        self.constraints
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get constraints by type
    pub fn get_by_type(&self, constraint_type: ConstraintType) -> Vec<Constraint> {
        self.constraints
            .iter()
            .filter(|entry| entry.constraint_type == constraint_type)
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Check for conflicts between constraints
    pub fn find_conflicts(&self) -> Vec<(ConstraintId, ConstraintId)> {
        let mut conflicts = Vec::new();
        let all_constraints: Vec<_> = self.constraints.iter().collect();

        // Check for conflicts between pairs of constraints
        for i in 0..all_constraints.len() {
            for j in (i + 1)..all_constraints.len() {
                let constraint1 = all_constraints[i].value();
                let constraint2 = all_constraints[j].value();

                if self.constraints_conflict(constraint1, constraint2) {
                    conflicts.push((constraint1.id, constraint2.id));
                }
            }
        }

        conflicts
    }

    /// Check if two constraints conflict with each other
    fn constraints_conflict(&self, c1: &Constraint, c2: &Constraint) -> bool {
        // If constraints don't share any entities, they can't conflict
        if !self.constraints_share_entities(c1, c2) {
            return false;
        }

        match (&c1.constraint_type, &c2.constraint_type) {
            // Dimensional conflicts: two different fixed values for same property
            (ConstraintType::Dimensional(d1), ConstraintType::Dimensional(d2)) => {
                self.dimensional_constraints_conflict(c1, c2, d1, d2)
            }

            // Geometric conflicts: contradictory geometric relationships
            (ConstraintType::Geometric(g1), ConstraintType::Geometric(g2)) => {
                self.geometric_constraints_conflict(c1, c2, g1, g2)
            }

            // Mixed conflicts: dimensional constraint contradicts geometric
            (ConstraintType::Dimensional(d), ConstraintType::Geometric(g))
            | (ConstraintType::Geometric(g), ConstraintType::Dimensional(d)) => {
                self.mixed_constraints_conflict(c1, c2, d, g)
            }
        }
    }

    /// Check if two constraints share any entities
    fn constraints_share_entities(&self, c1: &Constraint, c2: &Constraint) -> bool {
        for entity1 in &c1.entities {
            for entity2 in &c2.entities {
                if std::mem::discriminant(entity1) == std::mem::discriminant(entity2) {
                    match (entity1, entity2) {
                        (EntityRef::Point(id1), EntityRef::Point(id2)) => {
                            if id1 == id2 {
                                return true;
                            }
                        }
                        (EntityRef::Line(id1), EntityRef::Line(id2)) => {
                            if id1 == id2 {
                                return true;
                            }
                        }
                        (EntityRef::Arc(id1), EntityRef::Arc(id2)) => {
                            if id1 == id2 {
                                return true;
                            }
                        }
                        (EntityRef::Circle(id1), EntityRef::Circle(id2)) => {
                            if id1 == id2 {
                                return true;
                            }
                        }
                        (EntityRef::Rectangle(id1), EntityRef::Rectangle(id2)) => {
                            if id1 == id2 {
                                return true;
                            }
                        }
                        (EntityRef::Ellipse(id1), EntityRef::Ellipse(id2)) => {
                            if id1 == id2 {
                                return true;
                            }
                        }
                        (EntityRef::Spline(id1), EntityRef::Spline(id2)) => {
                            if id1 == id2 {
                                return true;
                            }
                        }
                        (EntityRef::Polyline(id1), EntityRef::Polyline(id2)) => {
                            if id1 == id2 {
                                return true;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        false
    }

    /// Check conflicts between dimensional constraints
    fn dimensional_constraints_conflict(
        &self,
        c1: &Constraint,
        c2: &Constraint,
        d1: &DimensionalConstraint,
        d2: &DimensionalConstraint,
    ) -> bool {
        use DimensionalConstraint::*;

        // Check for conflicting fixed values on same entities
        match (d1, d2) {
            // Two different distances for same point pair
            (Distance(v1), Distance(v2)) => {
                if self.same_entity_pairs(c1, c2) && (v1 - v2).abs() > 1e-10 {
                    return true;
                }
            }

            // Two different lengths for same line
            (Length(v1), Length(v2)) => {
                if self.constraints_share_entities(c1, c2) && (v1 - v2).abs() > 1e-10 {
                    return true;
                }
            }

            // Two different radii for same circle/arc
            (Radius(v1), Radius(v2)) => {
                if self.constraints_share_entities(c1, c2) && (v1 - v2).abs() > 1e-10 {
                    return true;
                }
            }

            // Radius vs diameter conflict
            (Radius(r), Diameter(d)) | (Diameter(d), Radius(r)) => {
                if self.constraints_share_entities(c1, c2) && (2.0 * r - d).abs() > 1e-10 {
                    return true;
                }
            }

            // Different coordinates for same point
            (XCoordinate(v1), XCoordinate(v2)) | (YCoordinate(v1), YCoordinate(v2)) => {
                if self.constraints_share_entities(c1, c2) && (v1 - v2).abs() > 1e-10 {
                    return true;
                }
            }

            _ => {}
        }

        false
    }

    /// Check conflicts between geometric constraints
    fn geometric_constraints_conflict(
        &self,
        c1: &Constraint,
        c2: &Constraint,
        g1: &GeometricConstraint,
        g2: &GeometricConstraint,
    ) -> bool {
        use GeometricConstraint::*;

        if !self.constraints_share_entities(c1, c2) {
            return false;
        }

        match (g1, g2) {
            // Parallel and perpendicular are contradictory
            (Parallel, Perpendicular) | (Perpendicular, Parallel) => true,

            // Horizontal and vertical are contradictory (for same line)
            (Horizontal, Vertical) | (Vertical, Horizontal) => true,

            // A line can't be both horizontal and have an angle constraint
            (Horizontal, _) | (_, Horizontal) | (Vertical, _) | (_, Vertical) => {
                // This would need more context to determine conflict
                false
            }

            _ => false,
        }
    }

    /// Check conflicts between dimensional and geometric constraints
    fn mixed_constraints_conflict(
        &self,
        _c1: &Constraint,
        _c2: &Constraint,
        _d: &DimensionalConstraint,
        _g: &GeometricConstraint,
    ) -> bool {
        // Examples of mixed conflicts:
        // - Distance constraint of 0 with non-coincident geometric constraint
        // - Angle constraint of 0/180 degrees with non-parallel/non-collinear constraint
        // - Angle constraint of 90 degrees with non-perpendicular constraint

        // For now, return false - more sophisticated analysis needed
        false
    }

    /// Check if two constraints apply to the same entity pairs
    fn same_entity_pairs(&self, c1: &Constraint, c2: &Constraint) -> bool {
        if c1.entities.len() != c2.entities.len() {
            return false;
        }

        // For now, simple check - could be more sophisticated
        c1.entities == c2.entities
    }

    /// Clear all constraints
    pub fn clear(&self) {
        self.constraints.clear();
        self.entity_constraints.clear();
        self.constraint_groups.clear();
    }

    /// Get the total number of constraints
    pub fn constraint_count(&self) -> usize {
        self.constraints.len()
    }

    /// Create symmetry constraint between two entities about a line
    pub fn add_symmetry_constraint(
        &self,
        entity1: EntityRef,
        entity2: EntityRef,
        symmetry_line: EntityRef,
        priority: ConstraintPriority,
    ) -> ConstraintId {
        let constraint = Constraint {
            id: ConstraintId::new(),
            constraint_type: ConstraintType::Geometric(GeometricConstraint::Symmetric),
            entities: vec![entity1, entity2, symmetry_line],
            priority,
            status: ConstraintStatus::Satisfied,
            name: Some("Symmetric about line".to_string()),
        };

        let constraint_id = constraint.id;
        self.add_constraint(constraint);
        constraint_id
    }

    /// Create smooth tangent continuity constraint between curves
    pub fn add_smooth_tangent_constraint(
        &self,
        curve1: EntityRef,
        curve2: EntityRef,
        connection_point: EntityRef,
        priority: ConstraintPriority,
    ) -> ConstraintId {
        let constraint = Constraint {
            id: ConstraintId::new(),
            constraint_type: ConstraintType::Geometric(GeometricConstraint::SmoothTangent),
            entities: vec![curve1, curve2, connection_point],
            priority,
            status: ConstraintStatus::Satisfied,
            name: Some("G1 continuity between curves".to_string()),
        };

        let constraint_id = constraint.id;
        self.add_constraint(constraint);
        constraint_id
    }

    /// Create curvature continuity constraint between curves
    pub fn add_curvature_continuity_constraint(
        &self,
        curve1: EntityRef,
        curve2: EntityRef,
        connection_point: EntityRef,
        priority: ConstraintPriority,
    ) -> ConstraintId {
        let constraint = Constraint {
            id: ConstraintId::new(),
            constraint_type: ConstraintType::Geometric(GeometricConstraint::CurvatureContinuity),
            entities: vec![curve1, curve2, connection_point],
            priority,
            status: ConstraintStatus::Satisfied,
            name: Some("G2 continuity between curves".to_string()),
        };

        let constraint_id = constraint.id;
        self.add_constraint(constraint);
        constraint_id
    }

    /// Create multi-tangent constraint (curve tangent to multiple entities)
    pub fn add_multi_tangent_constraint(
        &self,
        curve: EntityRef,
        tangent_entities: Vec<EntityRef>,
        priority: ConstraintPriority,
    ) -> ConstraintId {
        let mut entities = vec![curve];
        entities.extend(tangent_entities);

        let constraint = Constraint {
            id: ConstraintId::new(),
            constraint_type: ConstraintType::Geometric(GeometricConstraint::MultiTangent),
            entities,
            priority,
            status: ConstraintStatus::Satisfied,
            name: Some("Multi-tangent constraint".to_string()),
        };

        let constraint_id = constraint.id;
        self.add_constraint(constraint);
        constraint_id
    }

    /// Create area constraint for closed curves
    pub fn add_area_constraint(
        &self,
        entity: EntityRef,
        target_area: f64,
        priority: ConstraintPriority,
    ) -> ConstraintId {
        let constraint = Constraint {
            id: ConstraintId::new(),
            constraint_type: ConstraintType::Dimensional(DimensionalConstraint::Area(target_area)),
            entities: vec![entity],
            priority,
            status: ConstraintStatus::Satisfied,
            name: Some(format!("Area = {}", target_area)),
        };

        let constraint_id = constraint.id;
        self.add_constraint(constraint);
        constraint_id
    }

    /// Create perimeter constraint for closed curves
    pub fn add_perimeter_constraint(
        &self,
        entity: EntityRef,
        target_perimeter: f64,
        priority: ConstraintPriority,
    ) -> ConstraintId {
        let constraint = Constraint {
            id: ConstraintId::new(),
            constraint_type: ConstraintType::Dimensional(DimensionalConstraint::Perimeter(
                target_perimeter,
            )),
            entities: vec![entity],
            priority,
            status: ConstraintStatus::Satisfied,
            name: Some(format!("Perimeter = {}", target_perimeter)),
        };

        let constraint_id = constraint.id;
        self.add_constraint(constraint);
        constraint_id
    }

    /// Create aspect ratio constraint for rectangular entities
    pub fn add_aspect_ratio_constraint(
        &self,
        entity: EntityRef,
        aspect_ratio: f64,
        priority: ConstraintPriority,
    ) -> ConstraintId {
        let constraint = Constraint {
            id: ConstraintId::new(),
            constraint_type: ConstraintType::Dimensional(DimensionalConstraint::AspectRatio(
                aspect_ratio,
            )),
            entities: vec![entity],
            priority,
            status: ConstraintStatus::Satisfied,
            name: Some(format!("Aspect ratio = {}", aspect_ratio)),
        };

        let constraint_id = constraint.id;
        self.add_constraint(constraint);
        constraint_id
    }

    /// Create offset constraint between two entities
    pub fn add_offset_constraint(
        &self,
        entity1: EntityRef,
        entity2: EntityRef,
        offset_distance: f64,
        priority: ConstraintPriority,
    ) -> ConstraintId {
        let constraint = Constraint {
            id: ConstraintId::new(),
            constraint_type: ConstraintType::Dimensional(DimensionalConstraint::OffsetDistance(
                offset_distance,
            )),
            entities: vec![entity1, entity2],
            priority,
            status: ConstraintStatus::Satisfied,
            name: Some(format!("Offset distance = {}", offset_distance)),
        };

        let constraint_id = constraint.id;
        self.add_constraint(constraint);
        constraint_id
    }

    /// Create intersection angle constraint between two curves
    pub fn add_intersection_angle_constraint(
        &self,
        curve1: EntityRef,
        curve2: EntityRef,
        intersection_point: EntityRef,
        angle: f64,
        priority: ConstraintPriority,
    ) -> ConstraintId {
        let constraint = Constraint {
            id: ConstraintId::new(),
            constraint_type: ConstraintType::Geometric(GeometricConstraint::IntersectionAngle(
                angle,
            )),
            entities: vec![curve1, curve2, intersection_point],
            priority,
            status: ConstraintStatus::Satisfied,
            name: Some(format!("Intersection angle = {} rad", angle)),
        };

        let constraint_id = constraint.id;
        self.add_constraint(constraint);
        constraint_id
    }
}
