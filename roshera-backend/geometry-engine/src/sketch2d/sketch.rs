//! 2D Sketch container and management
//!
//! This module implements the main sketch container that holds all 2D entities
//! and manages their relationships, constraints, and operations.
//!
//! A sketch is a collection of 2D geometric entities (points, lines, arcs, etc.)
//! with constraints that define their relationships. Sketches are typically
//! created on a plane and used as profiles for 3D operations.

use super::arc2d::ParametricArc2d;
use super::circle2d::ParametricCircle2d;
use super::constraints::{ConstraintStore, EntityRef};
use super::ellipse2d::ParametricEllipse2d;
use super::line2d::ParametricLine2d;
use super::point2d::ParametricPoint2d;
use super::polyline2d::ParametricPolyline2d;
use super::rectangle2d::ParametricRectangle2d;
use super::spline2d::{BSpline2d, ParametricSpline2d};
use super::{
    Arc2d, Arc2dId, Circle2d, Circle2dId, Constraint, ConstraintId, ConstraintSolver, Ellipse2d,
    Ellipse2dId, Line2d, Line2dId, LineSegment2d, Point2d, Point2dId, Polyline2d, Polyline2dId,
    Rectangle2d, Rectangle2dId, Sketch2dError, Sketch2dResult, SketchPlane, SolverResult,
    SolverStatus, Spline2d, Spline2dId, Tolerance2d, Vector2d,
};
use crate::sketch2d::SketchEntity2d;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use uuid::Uuid;

/// Unique identifier for a sketch
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SketchId(pub Uuid);

impl SketchId {
    /// Create a new unique sketch ID
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for SketchId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sketch_{}", &self.0.to_string()[..8])
    }
}

/// Statistics about a sketch
#[derive(Debug, Clone, Default)]
pub struct SketchStatistics {
    /// Number of points
    pub point_count: usize,
    /// Number of lines
    pub line_count: usize,
    /// Number of arcs
    pub arc_count: usize,
    /// Number of circles
    pub circle_count: usize,
    /// Number of rectangles
    pub rectangle_count: usize,
    /// Number of ellipses
    pub ellipse_count: usize,
    /// Number of splines
    pub spline_count: usize,
    /// Number of polylines
    pub polyline_count: usize,
    /// Total number of constraints
    pub constraint_count: usize,
    /// Number of fully constrained entities
    pub fully_constrained_count: usize,
    /// Number of under-constrained entities
    pub under_constrained_count: usize,
    /// Number of over-constrained entities
    pub over_constrained_count: usize,
}

/// A 2D sketch containing entities and constraints
pub struct Sketch {
    /// Unique identifier
    pub id: SketchId,
    /// Name of the sketch
    pub name: String,
    /// Sketch plane
    pub plane: SketchPlane,
    /// Tolerance for geometric operations
    pub tolerance: Tolerance2d,

    // Entity storage using DashMap for concurrent access
    /// Points in the sketch
    points: Arc<DashMap<Point2dId, ParametricPoint2d>>,
    /// Lines in the sketch
    lines: Arc<DashMap<Line2dId, ParametricLine2d>>,
    /// Arcs in the sketch
    arcs: Arc<DashMap<Arc2dId, ParametricArc2d>>,
    /// Circles in the sketch
    circles: Arc<DashMap<Circle2dId, ParametricCircle2d>>,
    /// Rectangles in the sketch
    rectangles: Arc<DashMap<Rectangle2dId, ParametricRectangle2d>>,
    /// Ellipses in the sketch
    ellipses: Arc<DashMap<Ellipse2dId, ParametricEllipse2d>>,
    /// Splines in the sketch
    splines: Arc<DashMap<Spline2dId, ParametricSpline2d>>,
    /// Polylines in the sketch
    polylines: Arc<DashMap<Polyline2dId, ParametricPolyline2d>>,

    /// Constraint store
    constraints: ConstraintStore,

    /// Spatial index for efficient queries (grid-based)
    spatial_index: Arc<DashMap<(i32, i32), Vec<EntityRef>>>,
    /// Grid size for spatial indexing
    grid_size: f64,
}

impl Sketch {
    /// Create a new sketch on the given plane
    pub fn new(name: String, plane: SketchPlane) -> Self {
        Self {
            id: SketchId::new(),
            name,
            plane,
            tolerance: Tolerance2d::default(),
            points: Arc::new(DashMap::new()),
            lines: Arc::new(DashMap::new()),
            arcs: Arc::new(DashMap::new()),
            circles: Arc::new(DashMap::new()),
            rectangles: Arc::new(DashMap::new()),
            ellipses: Arc::new(DashMap::new()),
            splines: Arc::new(DashMap::new()),
            polylines: Arc::new(DashMap::new()),
            constraints: ConstraintStore::new(),
            spatial_index: Arc::new(DashMap::new()),
            grid_size: 10.0, // Default 10mm grid
        }
    }

    /// Create a sketch on the XY plane
    pub fn on_xy_plane(name: String) -> Self {
        Self::new(name, SketchPlane::xy())
    }

    /// Create a sketch on the XZ plane
    pub fn on_xz_plane(name: String) -> Self {
        Self::new(name, SketchPlane::xz())
    }

    /// Create a sketch on the YZ plane
    pub fn on_yz_plane(name: String) -> Self {
        Self::new(name, SketchPlane::yz())
    }

    // Point operations

    /// Add a point to the sketch
    pub fn add_point(&self, point: Point2d) -> Point2dId {
        let param_point = ParametricPoint2d::new(point.x, point.y);
        let id = param_point.id;

        self.update_spatial_index_point(id, &point);
        self.points.insert(id, param_point);

        id
    }

    /// Get a point by ID
    pub fn get_point(&self, id: &Point2dId) -> Option<Point2d> {
        self.points.get(id).map(|entry| entry.position)
    }

    /// Update a point position
    pub fn update_point(&self, id: &Point2dId, new_position: Point2d) -> Sketch2dResult<()> {
        if let Some(mut entry) = self.points.get_mut(id) {
            // Clear old spatial index
            self.clear_spatial_index_for_entity(&EntityRef::Point(*id));

            // Update position
            entry.position = new_position;

            // Update spatial index
            self.update_spatial_index_point(*id, &new_position);

            Ok(())
        } else {
            Err(Sketch2dError::EntityNotFound {
                entity_type: "Point".to_string(),
                id: id.to_string(),
            })
        }
    }

    // Line operations

    /// Add a line to the sketch
    pub fn add_line(&self, start: Point2dId, end: Point2dId) -> Sketch2dResult<Line2dId> {
        let start_point = self
            .get_point(&start)
            .ok_or_else(|| Sketch2dError::EntityNotFound {
                entity_type: "Point".to_string(),
                id: start.to_string(),
            })?;

        let end_point = self
            .get_point(&end)
            .ok_or_else(|| Sketch2dError::EntityNotFound {
                entity_type: "Point".to_string(),
                id: end.to_string(),
            })?;

        let line = LineSegment2d::new(start_point, end_point)?;
        let param_line = ParametricLine2d::new_segment(line);
        let id = param_line.id;

        let (min, max) = param_line.bounding_box();
        self.update_spatial_index(EntityRef::Line(id), min, max);

        self.lines.insert(id, param_line);

        Ok(id)
    }

    /// Add an infinite line to the sketch
    pub fn add_infinite_line(
        &self,
        point: Point2d,
        direction: Vector2d,
    ) -> Sketch2dResult<Line2dId> {
        let line = Line2d::new(point, direction)?;
        let param_line = ParametricLine2d::new_infinite(line);
        let id = param_line.id;

        // Infinite lines don't have bounds, so we use a large bounding box
        let large_value = 1e6;
        let min = Point2d::new(-large_value, -large_value);
        let max = Point2d::new(large_value, large_value);
        self.update_spatial_index(EntityRef::Line(id), min, max);

        self.lines.insert(id, param_line);

        Ok(id)
    }

    // Arc operations

    /// Add an arc to the sketch
    pub fn add_arc_three_points(
        &self,
        start: Point2d,
        mid: Point2d,
        end: Point2d,
    ) -> Sketch2dResult<Arc2dId> {
        let arc = Arc2d::from_three_points(&start, &mid, &end)?;
        let param_arc = ParametricArc2d::new(arc);
        let id = param_arc.id;

        let (min, max) = param_arc.bounding_box();
        self.update_spatial_index(EntityRef::Arc(id), min, max);

        self.arcs.insert(id, param_arc);

        Ok(id)
    }

    /// Add an arc by center, radius, and angles
    pub fn add_arc_center_angles(
        &self,
        center: Point2d,
        radius: f64,
        start_angle: f64,
        end_angle: f64,
    ) -> Sketch2dResult<Arc2dId> {
        let arc = Arc2d::new(center, radius, start_angle, end_angle, true)?;
        let param_arc = ParametricArc2d::new(arc);
        let id = param_arc.id;

        let (min, max) = param_arc.bounding_box();
        self.update_spatial_index(EntityRef::Arc(id), min, max);

        self.arcs.insert(id, param_arc);

        Ok(id)
    }

    // Circle operations

    /// Add a circle to the sketch
    pub fn add_circle(&self, center: Point2d, radius: f64) -> Sketch2dResult<Circle2dId> {
        let circle = Circle2d::new(center, radius)?;
        let param_circle = ParametricCircle2d::new(circle);
        let id = param_circle.id;

        let (min, max) = param_circle.bounding_box();
        self.update_spatial_index(EntityRef::Circle(id), min, max);

        self.circles.insert(id, param_circle);

        Ok(id)
    }

    /// Add a circle from three points
    pub fn add_circle_three_points(
        &self,
        p1: Point2d,
        p2: Point2d,
        p3: Point2d,
    ) -> Sketch2dResult<Circle2dId> {
        let circle = Circle2d::from_three_points(&p1, &p2, &p3)?;
        let param_circle = ParametricCircle2d::new(circle);
        let id = param_circle.id;

        let (min, max) = param_circle.bounding_box();
        self.update_spatial_index(EntityRef::Circle(id), min, max);

        self.circles.insert(id, param_circle);

        Ok(id)
    }

    // Rectangle operations

    /// Add an axis-aligned rectangle to the sketch
    pub fn add_rectangle(
        &self,
        corner1: Point2d,
        corner2: Point2d,
    ) -> Sketch2dResult<Rectangle2dId> {
        let rect = Rectangle2d::from_corners(&corner1, &corner2)?;
        let param_rect = ParametricRectangle2d::new(rect);
        let id = param_rect.id;

        let (min, max) = param_rect.bounding_box();
        self.update_spatial_index(EntityRef::Rectangle(id), min, max);

        self.rectangles.insert(id, param_rect);

        Ok(id)
    }

    /// Add a rotated rectangle to the sketch
    pub fn add_rectangle_rotated(
        &self,
        center: Point2d,
        width: f64,
        height: f64,
        rotation: f64,
    ) -> Sketch2dResult<Rectangle2dId> {
        let rect = Rectangle2d::new_rotated(center, width, height, rotation)?;
        let param_rect = ParametricRectangle2d::new(rect);
        let id = param_rect.id;

        let (min, max) = param_rect.bounding_box();
        self.update_spatial_index(EntityRef::Rectangle(id), min, max);

        self.rectangles.insert(id, param_rect);

        Ok(id)
    }

    // Ellipse operations

    /// Add an ellipse to the sketch
    pub fn add_ellipse(
        &self,
        center: Point2d,
        semi_major: f64,
        semi_minor: f64,
        rotation: f64,
    ) -> Sketch2dResult<Ellipse2dId> {
        let ellipse = Ellipse2d::new(center, semi_major, semi_minor, rotation)?;
        let param_ellipse = ParametricEllipse2d::new(ellipse);
        let id = param_ellipse.id;

        let (min, max) = param_ellipse.bounding_box();
        self.update_spatial_index(EntityRef::Ellipse(id), min, max);

        self.ellipses.insert(id, param_ellipse);

        Ok(id)
    }

    // Spline operations

    /// Add a B-spline to the sketch
    pub fn add_bspline(
        &self,
        degree: usize,
        control_points: Vec<Point2d>,
        knots: Vec<f64>,
    ) -> Sketch2dResult<Spline2dId> {
        let bspline = BSpline2d::new(degree, control_points, knots)?;
        let spline = Spline2d::BSpline(bspline);
        let param_spline = ParametricSpline2d::new(spline);
        let id = param_spline.id;

        let (min, max) = param_spline.bounding_box();
        self.update_spatial_index(EntityRef::Spline(id), min, max);

        self.splines.insert(id, param_spline);

        Ok(id)
    }

    // Polyline operations

    /// Add a polyline to the sketch
    pub fn add_polyline(
        &self,
        vertices: Vec<Point2d>,
        is_closed: bool,
    ) -> Sketch2dResult<Polyline2dId> {
        let polyline = Polyline2d::new(vertices, is_closed)?;
        let param_polyline = ParametricPolyline2d::new(polyline);
        let id = param_polyline.id;

        let (min, max) = param_polyline.bounding_box();
        self.update_spatial_index(EntityRef::Polyline(id), min, max);

        self.polylines.insert(id, param_polyline);

        Ok(id)
    }

    // Constraint operations

    /// Add a constraint to the sketch
    pub fn add_constraint(&self, constraint: Constraint) -> ConstraintId {
        // Update constraint counts for affected entities
        for entity in &constraint.entities {
            match entity {
                EntityRef::Point(id) => {
                    if let Some(mut point) = self.points.get_mut(id) {
                        if let Err(e) = point.value_mut().add_constraint() {
                            tracing::warn!(point_id = ?id, error = %e, "point add_constraint failed (over-constrained); outer constraint still recorded");
                        }
                    }
                }
                EntityRef::Line(id) => {
                    if let Some(mut line) = self.lines.get_mut(id) {
                        line.value_mut().add_constraint();
                    }
                }
                EntityRef::Arc(id) => {
                    if let Some(mut arc) = self.arcs.get_mut(id) {
                        arc.value_mut().add_constraint();
                    }
                }
                EntityRef::Circle(id) => {
                    if let Some(mut circle) = self.circles.get_mut(id) {
                        circle.value_mut().add_constraint();
                    }
                }
                EntityRef::Rectangle(id) => {
                    if let Some(mut rect) = self.rectangles.get_mut(id) {
                        rect.value_mut().add_constraint();
                    }
                }
                EntityRef::Ellipse(id) => {
                    if let Some(mut ellipse) = self.ellipses.get_mut(id) {
                        ellipse.value_mut().add_constraint();
                    }
                }
                EntityRef::Spline(id) => {
                    if let Some(mut spline) = self.splines.get_mut(id) {
                        spline.value_mut().add_constraint();
                    }
                }
                EntityRef::Polyline(id) => {
                    if let Some(mut polyline) = self.polylines.get_mut(id) {
                        polyline.value_mut().add_constraint();
                    }
                }
            }
        }

        self.constraints.add_constraint(constraint)
    }

    /// Remove a constraint from the sketch
    pub fn remove_constraint(&self, id: &ConstraintId) -> Option<Constraint> {
        if let Some(constraint) = self.constraints.remove_constraint(id) {
            // Update constraint counts for affected entities
            for entity in &constraint.entities {
                match entity {
                    EntityRef::Point(id) => {
                        if let Some(mut point) = self.points.get_mut(id) {
                            if let Err(e) = point.value_mut().remove_constraint() {
                                tracing::warn!(point_id = ?id, error = %e, "point remove_constraint failed (counter already zero); outer constraint still removed");
                            }
                        }
                    }
                    EntityRef::Line(id) => {
                        if let Some(mut line) = self.lines.get_mut(id) {
                            line.value_mut().remove_constraint();
                        }
                    }
                    EntityRef::Arc(id) => {
                        if let Some(mut arc) = self.arcs.get_mut(id) {
                            arc.value_mut().remove_constraint();
                        }
                    }
                    EntityRef::Circle(id) => {
                        if let Some(mut circle) = self.circles.get_mut(id) {
                            circle.value_mut().remove_constraint();
                        }
                    }
                    EntityRef::Rectangle(id) => {
                        if let Some(mut rect) = self.rectangles.get_mut(id) {
                            rect.value_mut().remove_constraint();
                        }
                    }
                    EntityRef::Ellipse(id) => {
                        if let Some(mut ellipse) = self.ellipses.get_mut(id) {
                            ellipse.value_mut().remove_constraint();
                        }
                    }
                    EntityRef::Spline(id) => {
                        if let Some(mut spline) = self.splines.get_mut(id) {
                            spline.value_mut().remove_constraint();
                        }
                    }
                    EntityRef::Polyline(id) => {
                        if let Some(mut polyline) = self.polylines.get_mut(id) {
                            polyline.value_mut().remove_constraint();
                        }
                    }
                }
            }

            Some(constraint)
        } else {
            None
        }
    }

    /// Get all constraints in the sketch
    pub fn all_constraints(&self) -> Vec<Constraint> {
        self.constraints.all_constraints()
    }

    /// Find conflicting constraints in the sketch
    pub fn find_constraint_conflicts(&self) -> Vec<(ConstraintId, ConstraintId)> {
        self.constraints.find_conflicts()
    }

    /// Get constraints by entity
    pub fn get_constraints_by_entity(&self, entity: &EntityRef) -> Vec<Constraint> {
        self.constraints.get_entity_constraints(entity)
    }

    /// Solve all constraints in the sketch
    pub fn solve_constraints(&self) -> SolverResult {
        let mut solver = ConstraintSolver::new();

        // Add all entities to the solver
        // This would involve converting sketch entities to solver format

        // Set constraints
        solver.set_constraints(self.all_constraints());

        // Solve
        let result = solver.solve();

        // Apply results back to sketch entities
        if matches!(result.status, SolverStatus::Converged { .. }) {
            // Update entity positions based on solver results
            // This would involve applying entity_updates from result
        }

        result
    }

    // Delete operations

    /// Delete a point from the sketch
    /// Removes all constraints that reference this point
    pub fn delete_point(&self, id: &Point2dId) -> Sketch2dResult<()> {
        // Check if point exists
        if !self.points.contains_key(id) {
            return Err(Sketch2dError::EntityNotFound {
                entity_type: "Point".to_string(),
                id: id.to_string(),
            });
        }

        // Remove all constraints that reference this point
        let entity_ref = EntityRef::Point(*id);
        self.remove_constraints_for_entity(&entity_ref);

        // Remove from spatial index
        self.remove_from_spatial_index(&entity_ref);

        // Remove the point
        self.points.remove(id);

        Ok(())
    }

    /// Delete a line from the sketch
    /// Removes all constraints that reference this line
    pub fn delete_line(&self, id: &Line2dId) -> Sketch2dResult<()> {
        // Check if line exists
        if !self.lines.contains_key(id) {
            return Err(Sketch2dError::EntityNotFound {
                entity_type: "Line".to_string(),
                id: id.to_string(),
            });
        }

        // Remove all constraints that reference this line
        let entity_ref = EntityRef::Line(*id);
        self.remove_constraints_for_entity(&entity_ref);

        // Remove from spatial index
        self.remove_from_spatial_index(&entity_ref);

        // Remove the line
        self.lines.remove(id);

        Ok(())
    }

    /// Delete an arc from the sketch
    /// Removes all constraints that reference this arc
    pub fn delete_arc(&self, id: &Arc2dId) -> Sketch2dResult<()> {
        // Check if arc exists
        if !self.arcs.contains_key(id) {
            return Err(Sketch2dError::EntityNotFound {
                entity_type: "Arc".to_string(),
                id: id.to_string(),
            });
        }

        // Remove all constraints that reference this arc
        let entity_ref = EntityRef::Arc(*id);
        self.remove_constraints_for_entity(&entity_ref);

        // Remove from spatial index
        self.remove_from_spatial_index(&entity_ref);

        // Remove the arc
        self.arcs.remove(id);

        Ok(())
    }

    /// Delete a circle from the sketch
    /// Removes all constraints that reference this circle
    pub fn delete_circle(&self, id: &Circle2dId) -> Sketch2dResult<()> {
        // Check if circle exists
        if !self.circles.contains_key(id) {
            return Err(Sketch2dError::EntityNotFound {
                entity_type: "Circle".to_string(),
                id: id.to_string(),
            });
        }

        // Remove all constraints that reference this circle
        let entity_ref = EntityRef::Circle(*id);
        self.remove_constraints_for_entity(&entity_ref);

        // Remove from spatial index
        self.remove_from_spatial_index(&entity_ref);

        // Remove the circle
        self.circles.remove(id);

        Ok(())
    }

    /// Delete a rectangle from the sketch
    /// Removes all constraints that reference this rectangle
    pub fn delete_rectangle(&self, id: &Rectangle2dId) -> Sketch2dResult<()> {
        // Check if rectangle exists
        if !self.rectangles.contains_key(id) {
            return Err(Sketch2dError::EntityNotFound {
                entity_type: "Rectangle".to_string(),
                id: id.to_string(),
            });
        }

        // Remove all constraints that reference this rectangle
        let entity_ref = EntityRef::Rectangle(*id);
        self.remove_constraints_for_entity(&entity_ref);

        // Remove from spatial index
        self.remove_from_spatial_index(&entity_ref);

        // Remove the rectangle
        self.rectangles.remove(id);

        Ok(())
    }

    /// Delete an ellipse from the sketch
    /// Removes all constraints that reference this ellipse
    pub fn delete_ellipse(&self, id: &Ellipse2dId) -> Sketch2dResult<()> {
        // Check if ellipse exists
        if !self.ellipses.contains_key(id) {
            return Err(Sketch2dError::EntityNotFound {
                entity_type: "Ellipse".to_string(),
                id: id.to_string(),
            });
        }

        // Remove all constraints that reference this ellipse
        let entity_ref = EntityRef::Ellipse(*id);
        self.remove_constraints_for_entity(&entity_ref);

        // Remove from spatial index
        self.remove_from_spatial_index(&entity_ref);

        // Remove the ellipse
        self.ellipses.remove(id);

        Ok(())
    }

    /// Delete a spline from the sketch
    /// Removes all constraints that reference this spline
    pub fn delete_spline(&self, id: &Spline2dId) -> Sketch2dResult<()> {
        // Check if spline exists
        if !self.splines.contains_key(id) {
            return Err(Sketch2dError::EntityNotFound {
                entity_type: "Spline".to_string(),
                id: id.to_string(),
            });
        }

        // Remove all constraints that reference this spline
        let entity_ref = EntityRef::Spline(*id);
        self.remove_constraints_for_entity(&entity_ref);

        // Remove from spatial index
        self.remove_from_spatial_index(&entity_ref);

        // Remove the spline
        self.splines.remove(id);

        Ok(())
    }

    /// Delete a polyline from the sketch
    /// Removes all constraints that reference this polyline
    pub fn delete_polyline(&self, id: &Polyline2dId) -> Sketch2dResult<()> {
        // Check if polyline exists
        if !self.polylines.contains_key(id) {
            return Err(Sketch2dError::EntityNotFound {
                entity_type: "Polyline".to_string(),
                id: id.to_string(),
            });
        }

        // Remove all constraints that reference this polyline
        let entity_ref = EntityRef::Polyline(*id);
        self.remove_constraints_for_entity(&entity_ref);

        // Remove from spatial index
        self.remove_from_spatial_index(&entity_ref);

        // Remove the polyline
        self.polylines.remove(id);

        Ok(())
    }

    /// Delete any entity by EntityRef
    /// This is the generic delete method that delegates to specific delete methods
    pub fn delete_entity(&self, entity: &EntityRef) -> Sketch2dResult<()> {
        match entity {
            EntityRef::Point(id) => self.delete_point(id),
            EntityRef::Line(id) => self.delete_line(id),
            EntityRef::Arc(id) => self.delete_arc(id),
            EntityRef::Circle(id) => self.delete_circle(id),
            EntityRef::Rectangle(id) => self.delete_rectangle(id),
            EntityRef::Ellipse(id) => self.delete_ellipse(id),
            EntityRef::Spline(id) => self.delete_spline(id),
            EntityRef::Polyline(id) => self.delete_polyline(id),
        }
    }

    /// Delete multiple entities at once
    /// More efficient than calling delete_entity multiple times
    pub fn delete_entities(&self, entities: &[EntityRef]) -> Sketch2dResult<Vec<EntityRef>> {
        let mut deleted = Vec::new();
        let mut errors = Vec::new();

        for entity in entities {
            match self.delete_entity(entity) {
                Ok(()) => deleted.push(*entity),
                Err(e) => errors.push((entity.clone(), e)),
            }
        }

        if !errors.is_empty() {
            // Return error with details about which entities failed
            let error_messages: Vec<String> = errors
                .iter()
                .map(|(entity, error)| format!("{:?}: {}", entity, error))
                .collect();

            return Err(Sketch2dError::InvalidOperation {
                operation: "delete_entities".to_string(),
                reason: format!(
                    "Failed to delete some entities: {}",
                    error_messages.join(", ")
                ),
            });
        }

        Ok(deleted)
    }

    /// Clear all entities from the sketch
    /// This removes all points, lines, arcs, circles, etc. and all constraints
    pub fn clear(&self) -> Sketch2dResult<usize> {
        let mut total_removed = 0;

        // Clear all constraints first
        let constraint_count = self.constraints.constraint_count();
        self.constraints.clear();
        total_removed += constraint_count;

        // Clear all entities
        total_removed += self.points.len();
        total_removed += self.lines.len();
        total_removed += self.arcs.len();
        total_removed += self.circles.len();
        total_removed += self.rectangles.len();
        total_removed += self.ellipses.len();
        total_removed += self.splines.len();
        total_removed += self.polylines.len();

        self.points.clear();
        self.lines.clear();
        self.arcs.clear();
        self.circles.clear();
        self.rectangles.clear();
        self.ellipses.clear();
        self.splines.clear();
        self.polylines.clear();

        // Clear spatial index
        self.spatial_index.clear();

        Ok(total_removed)
    }

    /// Delete all entities within a bounding box
    /// Useful for selective clearing of regions
    pub fn delete_in_box(&self, min: Point2d, max: Point2d) -> Sketch2dResult<Vec<EntityRef>> {
        let entities_in_box = self.query_box(min, max);
        self.delete_entities(&entities_in_box)
    }

    /// Delete all entities of a specific type
    pub fn delete_all_of_type(&self, entity_type: &str) -> Sketch2dResult<usize> {
        let mut count = 0;

        match entity_type.to_lowercase().as_str() {
            "point" | "points" => {
                let ids: Vec<Point2dId> = self.points.iter().map(|entry| *entry.key()).collect();
                for id in ids {
                    if self.delete_point(&id).is_ok() {
                        count += 1;
                    }
                }
            }
            "line" | "lines" => {
                let ids: Vec<Line2dId> = self.lines.iter().map(|entry| *entry.key()).collect();
                for id in ids {
                    if self.delete_line(&id).is_ok() {
                        count += 1;
                    }
                }
            }
            "arc" | "arcs" => {
                let ids: Vec<Arc2dId> = self.arcs.iter().map(|entry| *entry.key()).collect();
                for id in ids {
                    if self.delete_arc(&id).is_ok() {
                        count += 1;
                    }
                }
            }
            "circle" | "circles" => {
                let ids: Vec<Circle2dId> = self.circles.iter().map(|entry| *entry.key()).collect();
                for id in ids {
                    if self.delete_circle(&id).is_ok() {
                        count += 1;
                    }
                }
            }
            "rectangle" | "rectangles" => {
                let ids: Vec<Rectangle2dId> =
                    self.rectangles.iter().map(|entry| *entry.key()).collect();
                for id in ids {
                    if self.delete_rectangle(&id).is_ok() {
                        count += 1;
                    }
                }
            }
            "ellipse" | "ellipses" => {
                let ids: Vec<Ellipse2dId> =
                    self.ellipses.iter().map(|entry| *entry.key()).collect();
                for id in ids {
                    if self.delete_ellipse(&id).is_ok() {
                        count += 1;
                    }
                }
            }
            "spline" | "splines" => {
                let ids: Vec<Spline2dId> = self.splines.iter().map(|entry| *entry.key()).collect();
                for id in ids {
                    if self.delete_spline(&id).is_ok() {
                        count += 1;
                    }
                }
            }
            "polyline" | "polylines" => {
                let ids: Vec<Polyline2dId> =
                    self.polylines.iter().map(|entry| *entry.key()).collect();
                for id in ids {
                    if self.delete_polyline(&id).is_ok() {
                        count += 1;
                    }
                }
            }
            _ => {
                return Err(Sketch2dError::InvalidParameter {
                    parameter: "entity_type".to_string(),
                    value: entity_type.to_string(),
                    constraint: "Must be one of: point, line, arc, circle, rectangle, ellipse, spline, polyline".to_string(),
                });
            }
        }

        Ok(count)
    }

    // Helper methods for delete operations

    /// Remove all constraints that reference a specific entity
    fn remove_constraints_for_entity(&self, entity: &EntityRef) {
        let constraints_to_remove: Vec<ConstraintId> = self
            .constraints
            .all_constraints()
            .iter()
            .filter(|constraint| constraint.entities.contains(entity))
            .map(|constraint| constraint.id)
            .collect();

        for constraint_id in constraints_to_remove {
            self.remove_constraint(&constraint_id);
        }
    }

    /// Remove an entity from the spatial index
    fn remove_from_spatial_index(&self, entity: &EntityRef) {
        // Get the entity's current grid cells
        let grid_cells = self.get_entity_grid_cells(entity);

        // Remove from each grid cell
        for (grid_x, grid_y) in grid_cells {
            if let Some(mut cell) = self.spatial_index.get_mut(&(grid_x, grid_y)) {
                cell.retain(|e| e != entity);

                // Remove empty cells to save memory
                if cell.is_empty() {
                    drop(cell);
                    self.spatial_index.remove(&(grid_x, grid_y));
                }
            }
        }
    }

    /// Get the grid cells that an entity occupies
    fn get_entity_grid_cells(&self, entity: &EntityRef) -> Vec<(i32, i32)> {
        let mut cells = Vec::new();

        if let Some((min, max)) = self.get_entity_bounds(entity) {
            let min_grid_x = (min.x / self.grid_size).floor() as i32;
            let min_grid_y = (min.y / self.grid_size).floor() as i32;
            let max_grid_x = (max.x / self.grid_size).floor() as i32;
            let max_grid_y = (max.y / self.grid_size).floor() as i32;

            for x in min_grid_x..=max_grid_x {
                for y in min_grid_y..=max_grid_y {
                    cells.push((x, y));
                }
            }
        }

        cells
    }

    /// Get the bounding box of an entity
    fn get_entity_bounds(&self, entity: &EntityRef) -> Option<(Point2d, Point2d)> {
        match entity {
            EntityRef::Point(id) => self.points.get(id).map(|point| {
                let pos = point.position;
                (pos, pos)
            }),
            EntityRef::Line(id) => self.lines.get(id).map(|line| line.bounding_box()),
            EntityRef::Arc(id) => self.arcs.get(id).map(|arc| arc.bounding_box()),
            EntityRef::Circle(id) => self.circles.get(id).map(|circle| circle.bounding_box()),
            EntityRef::Rectangle(id) => self.rectangles.get(id).map(|rect| rect.bounding_box()),
            EntityRef::Ellipse(id) => self.ellipses.get(id).map(|ellipse| ellipse.bounding_box()),
            EntityRef::Spline(id) => self.splines.get(id).map(|spline| spline.bounding_box()),
            EntityRef::Polyline(id) => self
                .polylines
                .get(id)
                .map(|polyline| polyline.bounding_box()),
        }
    }

    // Query operations

    /// Get entities within a bounding box
    pub fn query_box(&self, min: Point2d, max: Point2d) -> Vec<EntityRef> {
        let mut results = Vec::new();

        let min_grid_x = (min.x / self.grid_size).floor() as i32;
        let min_grid_y = (min.y / self.grid_size).floor() as i32;
        let max_grid_x = (max.x / self.grid_size).ceil() as i32;
        let max_grid_y = (max.y / self.grid_size).ceil() as i32;

        for x in min_grid_x..=max_grid_x {
            for y in min_grid_y..=max_grid_y {
                if let Some(entities) = self.spatial_index.get(&(x, y)) {
                    for entity in entities.value() {
                        // Check if entity actually intersects the query box
                        if self.entity_intersects_box(entity, &min, &max) {
                            results.push(*entity);
                        }
                    }
                }
            }
        }

        // Remove duplicates
        results.sort();
        results.dedup();

        results
    }

    /// Find entities near a point
    pub fn query_point(&self, point: &Point2d, radius: f64) -> Vec<EntityRef> {
        let min = Point2d::new(point.x - radius, point.y - radius);
        let max = Point2d::new(point.x + radius, point.y + radius);

        self.query_box(min, max)
            .into_iter()
            .filter(|entity| self.entity_distance_to_point(entity, point) <= radius)
            .collect()
    }

    /// Get sketch statistics
    pub fn statistics(&self) -> SketchStatistics {
        let mut stats = SketchStatistics::default();

        // Count entities
        stats.point_count = self.points.len();
        stats.line_count = self.lines.len();
        stats.arc_count = self.arcs.len();
        stats.circle_count = self.circles.len();
        stats.rectangle_count = self.rectangles.len();
        stats.ellipse_count = self.ellipses.len();
        stats.spline_count = self.splines.len();
        stats.polyline_count = self.polylines.len();
        stats.constraint_count = self.constraints.all_constraints().len();

        // Count constraint status
        // This would involve checking each entity's constraint status

        stats
    }

    // Public accessor methods for topology analysis

    /// Get reference to points storage
    pub fn points(&self) -> &Arc<DashMap<Point2dId, ParametricPoint2d>> {
        &self.points
    }

    /// Get reference to lines storage  
    pub fn lines(&self) -> &Arc<DashMap<Line2dId, ParametricLine2d>> {
        &self.lines
    }

    /// Get reference to arcs storage
    pub fn arcs(&self) -> &Arc<DashMap<Arc2dId, ParametricArc2d>> {
        &self.arcs
    }

    /// Get reference to circles storage
    pub fn circles(&self) -> &Arc<DashMap<Circle2dId, ParametricCircle2d>> {
        &self.circles
    }

    /// Get reference to rectangles storage
    pub fn rectangles(&self) -> &Arc<DashMap<Rectangle2dId, ParametricRectangle2d>> {
        &self.rectangles
    }

    /// Get reference to ellipses storage
    pub fn ellipses(&self) -> &Arc<DashMap<Ellipse2dId, ParametricEllipse2d>> {
        &self.ellipses
    }

    /// Get reference to splines storage
    pub fn splines(&self) -> &Arc<DashMap<Spline2dId, ParametricSpline2d>> {
        &self.splines
    }

    /// Get reference to polylines storage
    pub fn polylines(&self) -> &Arc<DashMap<Polyline2dId, ParametricPolyline2d>> {
        &self.polylines
    }

    // Private helper methods

    /// Update spatial index for a point
    fn update_spatial_index_point(&self, id: Point2dId, point: &Point2d) {
        let grid_x = (point.x / self.grid_size).floor() as i32;
        let grid_y = (point.y / self.grid_size).floor() as i32;

        self.spatial_index
            .entry((grid_x, grid_y))
            .or_insert_with(Vec::new)
            .push(EntityRef::Point(id));
    }

    /// Update spatial index for an entity with bounds
    fn update_spatial_index(&self, entity: EntityRef, min: Point2d, max: Point2d) {
        let min_grid_x = (min.x / self.grid_size).floor() as i32;
        let min_grid_y = (min.y / self.grid_size).floor() as i32;
        let max_grid_x = (max.x / self.grid_size).ceil() as i32;
        let max_grid_y = (max.y / self.grid_size).ceil() as i32;

        for x in min_grid_x..=max_grid_x {
            for y in min_grid_y..=max_grid_y {
                self.spatial_index
                    .entry((x, y))
                    .or_insert_with(Vec::new)
                    .push(entity);
            }
        }
    }

    /// Clear spatial index entries for an entity
    fn clear_spatial_index_for_entity(&self, entity: &EntityRef) {
        for mut entry in self.spatial_index.iter_mut() {
            entry.value_mut().retain(|e| e != entity);
        }
    }

    /// Check if an entity intersects a bounding box
    fn entity_intersects_box(&self, entity: &EntityRef, min: &Point2d, max: &Point2d) -> bool {
        match entity {
            EntityRef::Point(id) => {
                if let Some(point) = self.points.get(id) {
                    let p = &point.position;
                    p.x >= min.x && p.x <= max.x && p.y >= min.y && p.y <= max.y
                } else {
                    false
                }
            }
            EntityRef::Line(id) => {
                if let Some(line) = self.lines.get(id) {
                    let (entity_min, entity_max) = line.bounding_box();
                    !(entity_max.x < min.x
                        || entity_min.x > max.x
                        || entity_max.y < min.y
                        || entity_min.y > max.y)
                } else {
                    false
                }
            }
            // Similar for other entity types
            _ => true, // Conservative default
        }
    }

    /// Get distance from entity to point
    fn entity_distance_to_point(&self, entity: &EntityRef, point: &Point2d) -> f64 {
        match entity {
            EntityRef::Point(id) => {
                if let Some(p) = self.points.get(id) {
                    p.position.distance_to(point)
                } else {
                    f64::INFINITY
                }
            }
            EntityRef::Circle(id) => {
                if let Some(circle) = self.circles.get(id) {
                    circle.circle.distance_to_point(point).abs()
                } else {
                    f64::INFINITY
                }
            }
            // Similar for other entity types
            _ => 0.0, // Conservative default
        }
    }
}

/// Storage for multiple sketches
pub struct SketchStore {
    /// All sketches indexed by ID
    sketches: Arc<DashMap<SketchId, Arc<Sketch>>>,
    /// Active sketch ID
    active_sketch: Arc<DashMap<String, SketchId>>,
}

impl SketchStore {
    /// Create a new sketch store
    pub fn new() -> Self {
        Self {
            sketches: Arc::new(DashMap::new()),
            active_sketch: Arc::new(DashMap::new()),
        }
    }

    /// Add a sketch to the store
    pub fn add(&self, sketch: Sketch) -> SketchId {
        let id = sketch.id;
        self.sketches.insert(id, Arc::new(sketch));
        id
    }

    /// Get a sketch by ID
    pub fn get(&self, id: &SketchId) -> Option<Arc<Sketch>> {
        self.sketches.get(id).map(|entry| entry.clone())
    }

    /// Remove a sketch from the store
    pub fn remove(&self, id: &SketchId) -> Option<Arc<Sketch>> {
        self.sketches.remove(id).map(|(_, sketch)| sketch)
    }

    /// Set the active sketch
    pub fn set_active(&self, session: String, id: SketchId) {
        self.active_sketch.insert(session, id);
    }

    /// Get the active sketch for a session
    pub fn get_active(&self, session: &str) -> Option<Arc<Sketch>> {
        self.active_sketch.get(session).and_then(|id| self.get(&id))
    }

    /// Get all sketch IDs
    pub fn all_ids(&self) -> Vec<SketchId> {
        self.sketches.iter().map(|entry| *entry.key()).collect()
    }
}
