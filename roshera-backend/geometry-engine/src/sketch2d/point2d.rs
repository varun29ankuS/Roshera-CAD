//! 2D Point primitive for sketching
//!
//! This module implements parametric 2D points with full constraint support.
//! Points are the foundation of all 2D sketch geometry.
//!
//! # Degrees of Freedom
//!
//! A 2D point has 2 degrees of freedom (X and Y coordinates).
//! Common constraints:
//! - Fixed: Removes both DOF
//! - Horizontal/Vertical distance: Removes 1 DOF
//! - Coincident with another point: Removes 2 DOF
//! - On curve: Removes 1 DOF

use super::{Matrix3, Sketch2dError, Sketch2dResult, SketchEntity2d, Tolerance2d};
use crate::math::tolerance::STRICT_TOLERANCE;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use uuid::Uuid;

/// Unique identifier for a 2D point
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct Point2dId(pub Uuid);

impl Point2dId {
    /// Create a new unique point ID
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for Point2dId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Point2d_{}", &self.0.to_string()[..8])
    }
}

/// A 2D point in sketch space
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point2d {
    /// X coordinate
    pub x: f64,
    /// Y coordinate  
    pub y: f64,
}

impl Point2d {
    /// Create a new 2D point
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Origin point (0, 0)
    pub const ORIGIN: Self = Self { x: 0.0, y: 0.0 };

    /// Distance to another point
    pub fn distance_to(&self, other: &Point2d) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        (dx * dx + dy * dy).sqrt()
    }

    /// Distance squared to another point (avoids sqrt)
    pub fn distance_squared_to(&self, other: &Point2d) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        dx * dx + dy * dy
    }

    /// Check if points are coincident within tolerance
    pub fn coincident_with(&self, other: &Point2d, tolerance: &Tolerance2d) -> bool {
        self.distance_squared_to(other) < tolerance.distance * tolerance.distance
    }

    /// Midpoint between two points
    pub fn midpoint(&self, other: &Point2d) -> Point2d {
        Point2d::new((self.x + other.x) * 0.5, (self.y + other.y) * 0.5)
    }

    /// Linear interpolation between points
    /// t=0 returns self, t=1 returns other
    pub fn lerp(&self, other: &Point2d, t: f64) -> Point2d {
        Point2d::new(
            self.x + (other.x - self.x) * t,
            self.y + (other.y - self.y) * t,
        )
    }

    /// Angle from this point to another (in radians)
    /// Returns angle in range [-π, π]
    pub fn angle_to(&self, other: &Point2d) -> f64 {
        let dx = other.x - self.x;
        let dy = other.y - self.y;
        dy.atan2(dx)
    }

    /// Rotate point around origin by angle (in radians)
    pub fn rotate(&self, angle: f64) -> Point2d {
        let cos_a = angle.cos();
        let sin_a = angle.sin();
        Point2d::new(
            self.x * cos_a - self.y * sin_a,
            self.x * sin_a + self.y * cos_a,
        )
    }

    /// Rotate point around a center by angle (in radians)
    pub fn rotate_around(&self, center: &Point2d, angle: f64) -> Point2d {
        // Translate to origin
        let translated = Point2d::new(self.x - center.x, self.y - center.y);
        // Rotate
        let rotated = translated.rotate(angle);
        // Translate back
        Point2d::new(rotated.x + center.x, rotated.y + center.y)
    }

    /// Vector from origin to this point
    pub fn as_vector(&self) -> Vector2d {
        Vector2d::new(self.x, self.y)
    }

    /// Add a vector to this point
    pub fn add_vector(&self, v: &Vector2d) -> Point2d {
        Point2d::new(self.x + v.x, self.y + v.y)
    }

    /// Subtract a vector from this point
    pub fn sub_vector(&self, v: &Vector2d) -> Point2d {
        Point2d::new(self.x - v.x, self.y - v.y)
    }
}

/// A 2D vector for calculations
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vector2d {
    pub x: f64,
    pub y: f64,
}

impl Vector2d {
    /// Create a new 2D vector
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Zero vector
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    /// Unit X vector
    pub const UNIT_X: Self = Self { x: 1.0, y: 0.0 };

    /// Unit Y vector
    pub const UNIT_Y: Self = Self { x: 0.0, y: 1.0 };

    /// Vector from point A to point B
    pub fn from_points(from: &Point2d, to: &Point2d) -> Self {
        Self::new(to.x - from.x, to.y - from.y)
    }

    /// Magnitude (length) of the vector
    pub fn magnitude(&self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    /// Magnitude squared (avoids sqrt)
    pub fn magnitude_squared(&self) -> f64 {
        self.x * self.x + self.y * self.y
    }

    /// Normalize the vector (make unit length)
    pub fn normalize(&self) -> Sketch2dResult<Vector2d> {
        let mag = self.magnitude();
        if mag < STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::DegenerateGeometry {
                entity: "Vector2d".to_string(),
                reason: "Cannot normalize zero-length vector".to_string(),
            });
        }
        Ok(Vector2d::new(self.x / mag, self.y / mag))
    }

    /// Dot product with another vector
    pub fn dot(&self, other: &Vector2d) -> f64 {
        self.x * other.x + self.y * other.y
    }

    /// 2D cross product (returns scalar)
    /// Positive if other is counter-clockwise from self
    pub fn cross(&self, other: &Vector2d) -> f64 {
        self.x * other.y - self.y * other.x
    }

    /// Perpendicular vector (rotated 90° counter-clockwise)
    pub fn perpendicular(&self) -> Vector2d {
        Vector2d::new(-self.y, self.x)
    }

    /// Angle between vectors (in radians)
    pub fn angle_to(&self, other: &Vector2d) -> Sketch2dResult<f64> {
        let mag_product = self.magnitude() * other.magnitude();
        if mag_product < STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::DegenerateGeometry {
                entity: "Vector2d".to_string(),
                reason: "Cannot compute angle with zero-length vector".to_string(),
            });
        }

        let cos_angle = (self.dot(other) / mag_product).clamp(-1.0, 1.0);
        Ok(cos_angle.acos())
    }

    /// Signed angle to another vector (counter-clockwise positive)
    pub fn signed_angle_to(&self, other: &Vector2d) -> Sketch2dResult<f64> {
        let angle = self.angle_to(other)?;
        let cross = self.cross(other);
        if cross < 0.0 {
            Ok(-angle)
        } else {
            Ok(angle)
        }
    }

    /// Scale the vector
    pub fn scale(&self, factor: f64) -> Vector2d {
        Vector2d::new(self.x * factor, self.y * factor)
    }

    /// Negate the vector
    pub fn negate(&self) -> Vector2d {
        Vector2d::new(-self.x, -self.y)
    }

    /// Add another vector
    pub fn add(&self, other: &Vector2d) -> Vector2d {
        Vector2d::new(self.x + other.x, self.y + other.y)
    }

    /// Subtract another vector
    pub fn sub(&self, other: &Vector2d) -> Vector2d {
        Vector2d::new(self.x - other.x, self.y - other.y)
    }
}

/// A parametric 2D point entity with constraint tracking
#[derive(Clone)]
pub struct ParametricPoint2d {
    /// Unique identifier
    pub id: Point2dId,
    /// Current position
    pub position: Point2d,
    /// Number of constraints applied
    constraint_count: usize,
    /// Fixed constraint (removes all DOF)
    pub is_fixed: bool,
    /// Construction geometry flag
    pub is_construction: bool,
}

impl ParametricPoint2d {
    /// Create a new parametric point
    pub fn new(x: f64, y: f64) -> Self {
        Self {
            id: Point2dId::new(),
            position: Point2d::new(x, y),
            constraint_count: 0,
            is_fixed: false,
            is_construction: false,
        }
    }

    /// Create a fixed point
    pub fn new_fixed(x: f64, y: f64) -> Self {
        Self {
            id: Point2dId::new(),
            position: Point2d::new(x, y),
            constraint_count: 2, // Fixed removes both DOF
            is_fixed: true,
            is_construction: false,
        }
    }

    /// Add a constraint
    pub fn add_constraint(&mut self) -> Sketch2dResult<()> {
        if self.is_fixed {
            return Err(Sketch2dError::OverConstrained {
                entity: self.id.to_string(),
                constraints: self.constraint_count + 1,
                max: 2,
            });
        }

        if self.constraint_count >= 2 {
            return Err(Sketch2dError::OverConstrained {
                entity: self.id.to_string(),
                constraints: self.constraint_count + 1,
                max: 2,
            });
        }

        self.constraint_count += 1;
        Ok(())
    }

    /// Remove a constraint
    pub fn remove_constraint(&mut self) -> Sketch2dResult<()> {
        if self.constraint_count == 0 {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "constraint_count".to_string(),
                value: "0".to_string(),
                constraint: "must be greater than 0 to remove".to_string(),
            });
        }

        self.constraint_count -= 1;
        Ok(())
    }

    /// Fix the point (remove all degrees of freedom)
    pub fn fix(&mut self) {
        self.is_fixed = true;
        self.constraint_count = 2;
    }

    /// Unfix the point
    pub fn unfix(&mut self) {
        self.is_fixed = false;
        // Constraint count stays the same as other constraints may still apply
    }

    /// Set as construction geometry
    pub fn set_construction(&mut self, is_construction: bool) {
        self.is_construction = is_construction;
    }
}

impl SketchEntity2d for ParametricPoint2d {
    fn degrees_of_freedom(&self) -> usize {
        2 // X and Y coordinates
    }

    fn constraint_count(&self) -> usize {
        self.constraint_count
    }

    fn bounding_box(&self) -> (Point2d, Point2d) {
        // Point has zero size, so min and max are the same
        (self.position, self.position)
    }

    fn transform(&mut self, matrix: &Matrix3) {
        self.position = matrix.transform_point(&self.position);
    }

    fn clone_entity(&self) -> Box<dyn SketchEntity2d> {
        Box::new(ParametricPoint2d {
            id: Point2dId::new(), // New ID for clone
            position: self.position,
            constraint_count: 0, // Constraints don't copy
            is_fixed: false,
            is_construction: self.is_construction,
        })
    }
}

/// Storage for 2D points with spatial indexing
pub struct Point2dStore {
    /// All points indexed by ID
    points: Arc<DashMap<Point2dId, ParametricPoint2d>>,
    /// Spatial index for fast proximity queries
    spatial_index: Arc<DashMap<(i32, i32), Vec<Point2dId>>>,
    /// Grid size for spatial indexing
    grid_size: f64,
}

impl Point2dStore {
    /// Create a new point store
    pub fn new(grid_size: f64) -> Self {
        Self {
            points: Arc::new(DashMap::new()),
            spatial_index: Arc::new(DashMap::new()),
            grid_size,
        }
    }

    /// Add a point to the store
    pub fn add_point(&self, point: ParametricPoint2d) -> Point2dId {
        let id = point.id;
        let grid_key = self.grid_key(&point.position);

        // Add to spatial index
        self.spatial_index
            .entry(grid_key)
            .or_insert_with(Vec::new)
            .push(id);

        // Add to main storage
        self.points.insert(id, point);

        id
    }

    /// Get a point by ID
    pub fn get(&self, id: &Point2dId) -> Option<ParametricPoint2d> {
        self.points.get(id).map(|entry| entry.value().clone())
    }

    /// Update point position (maintains spatial index)
    pub fn update_position(&self, id: &Point2dId, new_pos: Point2d) -> Sketch2dResult<()> {
        let mut point = self
            .points
            .get_mut(id)
            .ok_or_else(|| Sketch2dError::EntityNotFound {
                entity_type: "Point2d".to_string(),
                id: id.to_string(),
            })?;

        // Remove from old spatial index
        let old_key = self.grid_key(&point.position);
        let new_key = self.grid_key(&new_pos);

        if old_key != new_key {
            if let Some(mut cell) = self.spatial_index.get_mut(&old_key) {
                cell.retain(|&p| p != *id);
            }

            // Add to new spatial index
            self.spatial_index
                .entry(new_key)
                .or_insert_with(Vec::new)
                .push(*id);
        }

        // Update position
        point.position = new_pos;
        Ok(())
    }

    /// Find points within a radius
    pub fn find_within_radius(&self, center: &Point2d, radius: f64) -> Vec<Point2dId> {
        let mut results = Vec::new();
        let radius_squared = radius * radius;

        // Check grid cells that could contain points within radius
        let min_x = ((center.x - radius) / self.grid_size).floor() as i32;
        let max_x = ((center.x + radius) / self.grid_size).ceil() as i32;
        let min_y = ((center.y - radius) / self.grid_size).floor() as i32;
        let max_y = ((center.y + radius) / self.grid_size).ceil() as i32;

        for x in min_x..=max_x {
            for y in min_y..=max_y {
                if let Some(cell) = self.spatial_index.get(&(x, y)) {
                    for &id in cell.iter() {
                        if let Some(point) = self.points.get(&id) {
                            if point.position.distance_squared_to(center) <= radius_squared {
                                results.push(id);
                            }
                        }
                    }
                }
            }
        }

        results
    }

    /// Find the nearest point to a given position
    pub fn find_nearest(&self, pos: &Point2d, max_distance: Option<f64>) -> Option<Point2dId> {
        let search_radius = max_distance.unwrap_or(f64::MAX);
        let candidates = self.find_within_radius(pos, search_radius);

        candidates
            .into_iter()
            .filter_map(|id| {
                self.points
                    .get(&id)
                    .map(|p| (id, p.position.distance_squared_to(pos)))
            })
            .min_by(|(_, d1), (_, d2)| d1.partial_cmp(d2).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(id, _)| id)
    }

    /// Get grid key for spatial indexing
    fn grid_key(&self, pos: &Point2d) -> (i32, i32) {
        (
            (pos.x / self.grid_size).floor() as i32,
            (pos.y / self.grid_size).floor() as i32,
        )
    }

    /// Remove a point
    pub fn remove(&self, id: &Point2dId) -> Sketch2dResult<()> {
        let point = self
            .points
            .remove(id)
            .ok_or_else(|| Sketch2dError::EntityNotFound {
                entity_type: "Point2d".to_string(),
                id: id.to_string(),
            })?;

        // Remove from spatial index
        let grid_key = self.grid_key(&point.1.position);
        if let Some(mut cell) = self.spatial_index.get_mut(&grid_key) {
            cell.retain(|&p| p != *id);
        }

        Ok(())
    }

    /// Clear all points
    pub fn clear(&self) {
        self.points.clear();
        self.spatial_index.clear();
    }

    /// Get total number of points
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// Check if store is empty
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_point_creation() {
        let p = Point2d::new(3.0, 4.0);
        assert_eq!(p.x, 3.0);
        assert_eq!(p.y, 4.0);
    }

    #[test]
    fn test_point_distance() {
        let p1 = Point2d::new(0.0, 0.0);
        let p2 = Point2d::new(3.0, 4.0);
        assert_eq!(p1.distance_to(&p2), 5.0);
        assert_eq!(p1.distance_squared_to(&p2), 25.0);
    }

    #[test]
    fn test_point_midpoint() {
        let p1 = Point2d::new(0.0, 0.0);
        let p2 = Point2d::new(4.0, 6.0);
        let mid = p1.midpoint(&p2);
        assert_eq!(mid.x, 2.0);
        assert_eq!(mid.y, 3.0);
    }

    #[test]
    fn test_point_lerp() {
        let p1 = Point2d::new(0.0, 0.0);
        let p2 = Point2d::new(10.0, 20.0);

        let p_0 = p1.lerp(&p2, 0.0);
        assert_eq!(p_0.x, 0.0);
        assert_eq!(p_0.y, 0.0);

        let p_half = p1.lerp(&p2, 0.5);
        assert_eq!(p_half.x, 5.0);
        assert_eq!(p_half.y, 10.0);

        let p_1 = p1.lerp(&p2, 1.0);
        assert_eq!(p_1.x, 10.0);
        assert_eq!(p_1.y, 20.0);
    }

    #[test]
    fn test_point_angle() {
        let p1 = Point2d::new(0.0, 0.0);
        let p2 = Point2d::new(1.0, 0.0);
        let p3 = Point2d::new(0.0, 1.0);
        let p4 = Point2d::new(-1.0, 0.0);

        assert_eq!(p1.angle_to(&p2), 0.0);
        assert_eq!(p1.angle_to(&p3), std::f64::consts::PI / 2.0);
        assert_eq!(p1.angle_to(&p4), std::f64::consts::PI);
    }

    #[test]
    fn test_point_rotation() {
        let p = Point2d::new(1.0, 0.0);
        let rotated = p.rotate(std::f64::consts::PI / 2.0);

        assert!((rotated.x - 0.0).abs() < 1e-10);
        assert!((rotated.y - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_vector_operations() {
        let v1 = Vector2d::new(3.0, 4.0);
        let v2 = Vector2d::new(1.0, 2.0);

        assert_eq!(v1.magnitude(), 5.0);
        assert_eq!(v1.dot(&v2), 11.0);
        assert_eq!(v1.cross(&v2), 2.0);

        let perp = v1.perpendicular();
        assert_eq!(perp.x, -4.0);
        assert_eq!(perp.y, 3.0);
        assert_eq!(v1.dot(&perp), 0.0); // Perpendicular vectors have zero dot product
    }

    #[test]
    fn test_vector_normalization() {
        let v = Vector2d::new(3.0, 4.0);
        let normalized = v.normalize().unwrap();
        assert!((normalized.magnitude() - 1.0).abs() < 1e-10);
        assert!((normalized.x - 0.6).abs() < 1e-10);
        assert!((normalized.y - 0.8).abs() < 1e-10);

        // Test zero vector
        let zero = Vector2d::ZERO;
        assert!(zero.normalize().is_err());
    }

    #[test]
    fn test_parametric_point_constraints() {
        let mut point = ParametricPoint2d::new(1.0, 2.0);

        assert_eq!(point.degrees_of_freedom(), 2);
        assert_eq!(point.constraint_count(), 0);
        assert!(point.is_under_constrained());

        // Add constraints
        point.add_constraint().unwrap();
        assert_eq!(point.constraint_count(), 1);

        point.add_constraint().unwrap();
        assert_eq!(point.constraint_count(), 2);
        assert!(point.is_fully_constrained());

        // Try to over-constrain
        assert!(point.add_constraint().is_err());

        // Test fixed point
        point.fix();
        assert!(point.is_fixed);
        assert!(point.is_fully_constrained());
    }

    #[test]
    fn test_point_store() {
        let store = Point2dStore::new(10.0);

        // Add points
        let p1 = ParametricPoint2d::new(5.0, 5.0);
        let id1 = store.add_point(p1);

        let p2 = ParametricPoint2d::new(15.0, 5.0);
        let id2 = store.add_point(p2);

        assert_eq!(store.len(), 2);

        // Find within radius
        let near = store.find_within_radius(&Point2d::new(5.0, 5.0), 5.0);
        assert_eq!(near.len(), 1);
        assert_eq!(near[0], id1);

        // Find nearest
        let nearest = store.find_nearest(&Point2d::new(14.0, 5.0), None);
        assert_eq!(nearest, Some(id2));

        // Update position
        store
            .update_position(&id1, Point2d::new(12.0, 5.0))
            .unwrap();

        // Remove point
        store.remove(&id1).unwrap();
        assert_eq!(store.len(), 1);
    }
}
