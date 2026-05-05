//! 2D Polyline primitive for sketching
//!
//! This module implements parametric 2D polylines for sketching.
//! A polyline is a series of connected line segments defined by vertices.
//!
//! # Degrees of Freedom
//!
//! A polyline with n vertices has 2n degrees of freedom:
//! - 2 for each vertex position (X, Y)
//!
//! When closed, one constraint is implicitly added (first == last).
//!
//! Indexed access into the polyline vertex array is the canonical idiom —
//! all `vertices[i]` sites use indices bounded by `vertices.len()` (verified
//! at construction). Matches the numerical-kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::{
    LineSegment2d, Matrix3, Point2d, Sketch2dError, Sketch2dResult, SketchEntity2d, Tolerance2d,
    Vector2d,
};
use crate::math::tolerance::STRICT_TOLERANCE;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use uuid::Uuid;

/// Unique identifier for a 2D polyline
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct Polyline2dId(pub Uuid);

impl Polyline2dId {
    /// Create a new unique polyline ID
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for Polyline2dId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Polyline2d_{}", &self.0.to_string()[..8])
    }
}

/// A 2D polyline consisting of connected line segments
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Polyline2d {
    /// Vertices of the polyline
    pub vertices: Vec<Point2d>,
    /// Whether the polyline is closed
    pub is_closed: bool,
}

impl Polyline2d {
    /// Create a new polyline from vertices
    #[allow(clippy::expect_used)] // vertices.len() >= 2: verified by early return at fn entry
    pub fn new(vertices: Vec<Point2d>, is_closed: bool) -> Sketch2dResult<Self> {
        if vertices.len() < 2 {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "vertices".to_string(),
                value: format!("{} vertices", vertices.len()),
                constraint: "at least 2 vertices required".to_string(),
            });
        }

        // Check for duplicate consecutive vertices
        let tolerance = Tolerance2d::default();
        for i in 1..vertices.len() {
            if vertices[i - 1].coincident_with(&vertices[i], &tolerance) {
                return Err(Sketch2dError::DegenerateGeometry {
                    entity: "Polyline2d".to_string(),
                    reason: format!("Vertices {} and {} are coincident", i - 1, i),
                });
            }
        }

        // For closed polylines, check first and last
        if is_closed
            && vertices
                .first()
                .expect("vertices.len() >= 2: verified by early return above")
                .coincident_with(
                    vertices
                        .last()
                        .expect("vertices.len() >= 2: verified by early return above"),
                    &tolerance,
                )
        {
            return Err(Sketch2dError::DegenerateGeometry {
                entity: "Polyline2d".to_string(),
                reason: "First and last vertices are coincident in closed polyline".to_string(),
            });
        }

        Ok(Self {
            vertices,
            is_closed,
        })
    }

    /// Create a regular polygon
    pub fn regular_polygon(center: Point2d, radius: f64, num_sides: usize) -> Sketch2dResult<Self> {
        if num_sides < 3 {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "num_sides".to_string(),
                value: num_sides.to_string(),
                constraint: "at least 3 sides required".to_string(),
            });
        }

        if radius <= STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "radius".to_string(),
                value: radius.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        let angle_step = 2.0 * std::f64::consts::PI / num_sides as f64;
        let vertices: Vec<Point2d> = (0..num_sides)
            .map(|i| {
                let angle = i as f64 * angle_step;
                Point2d::new(
                    center.x + radius * angle.cos(),
                    center.y + radius * angle.sin(),
                )
            })
            .collect();

        Ok(Self {
            vertices,
            is_closed: true,
        })
    }

    /// Create a star polygon
    pub fn star_polygon(
        center: Point2d,
        outer_radius: f64,
        inner_radius: f64,
        num_points: usize,
    ) -> Sketch2dResult<Self> {
        if num_points < 3 {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "num_points".to_string(),
                value: num_points.to_string(),
                constraint: "at least 3 points required".to_string(),
            });
        }

        if outer_radius <= STRICT_TOLERANCE.distance()
            || inner_radius <= STRICT_TOLERANCE.distance()
        {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "radius".to_string(),
                value: format!("outer: {}, inner: {}", outer_radius, inner_radius),
                constraint: "both radii must be positive".to_string(),
            });
        }

        if inner_radius >= outer_radius {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "inner_radius".to_string(),
                value: inner_radius.to_string(),
                constraint: "must be less than outer radius".to_string(),
            });
        }

        let angle_step = std::f64::consts::PI / num_points as f64;
        let mut vertices = Vec::with_capacity(2 * num_points);

        for i in 0..(2 * num_points) {
            let angle = i as f64 * angle_step;
            let radius = if i % 2 == 0 {
                outer_radius
            } else {
                inner_radius
            };
            vertices.push(Point2d::new(
                center.x + radius * angle.cos(),
                center.y + radius * angle.sin(),
            ));
        }

        Ok(Self {
            vertices,
            is_closed: true,
        })
    }

    /// Get the number of vertices
    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    /// Get the number of segments
    pub fn segment_count(&self) -> usize {
        if self.is_closed {
            self.vertices.len()
        } else {
            self.vertices.len().saturating_sub(1)
        }
    }

    /// Get all segments as LineSegment2d
    #[allow(clippy::expect_used)] // is_closed branch guarded by vertices.len() >= 2
    pub fn segments(&self) -> Vec<LineSegment2d> {
        let mut segments = Vec::with_capacity(self.segment_count());

        for i in 0..self.vertices.len() - 1 {
            if let Ok(segment) = LineSegment2d::new(self.vertices[i], self.vertices[i + 1]) {
                segments.push(segment);
            }
        }

        // Add closing segment for closed polylines
        if self.is_closed && self.vertices.len() >= 2 {
            if let Ok(segment) = LineSegment2d::new(
                *self
                    .vertices
                    .last()
                    .expect("vertices.len() >= 2: verified by enclosing if-guard"),
                self.vertices[0],
            ) {
                segments.push(segment);
            }
        }

        segments
    }

    /// Get a specific segment by index
    #[allow(clippy::expect_used)] // closed branch guarded by index < segment_count + is_closed
    pub fn segment(&self, index: usize) -> Option<LineSegment2d> {
        if index >= self.segment_count() {
            return None;
        }

        if index < self.vertices.len() - 1 {
            LineSegment2d::new(self.vertices[index], self.vertices[index + 1]).ok()
        } else if self.is_closed && index == self.vertices.len() - 1 {
            // index < segment_count (verified at entry) and is_closed => len >= 1
            LineSegment2d::new(
                *self
                    .vertices
                    .last()
                    .expect("closed polyline with segment_count > 0: vertices are non-empty"),
                self.vertices[0],
            )
            .ok()
        } else {
            None
        }
    }

    /// Compute the total length
    pub fn length(&self) -> f64 {
        self.segments().iter().map(|seg| seg.length()).sum()
    }

    /// Compute the area (only for closed polylines)
    pub fn area(&self) -> Sketch2dResult<f64> {
        if !self.is_closed {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "is_closed".to_string(),
                value: "false".to_string(),
                constraint: "polyline must be closed to compute area".to_string(),
            });
        }

        // Use shoelace formula
        let mut area = 0.0;
        let n = self.vertices.len();

        for i in 0..n {
            let j = (i + 1) % n;
            area += self.vertices[i].x * self.vertices[j].y;
            area -= self.vertices[j].x * self.vertices[i].y;
        }

        Ok(area.abs() / 2.0)
    }

    /// Compute the centroid (only for closed polylines)
    pub fn centroid(&self) -> Sketch2dResult<Point2d> {
        if !self.is_closed {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "is_closed".to_string(),
                value: "false".to_string(),
                constraint: "polyline must be closed to compute centroid".to_string(),
            });
        }

        let area = self.area()?;
        if area < STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::DegenerateGeometry {
                entity: "Polyline2d".to_string(),
                reason: "Zero area".to_string(),
            });
        }

        let mut cx = 0.0;
        let mut cy = 0.0;
        let n = self.vertices.len();

        for i in 0..n {
            let j = (i + 1) % n;
            let factor =
                self.vertices[i].x * self.vertices[j].y - self.vertices[j].x * self.vertices[i].y;
            cx += (self.vertices[i].x + self.vertices[j].x) * factor;
            cy += (self.vertices[i].y + self.vertices[j].y) * factor;
        }

        Ok(Point2d::new(cx / (6.0 * area), cy / (6.0 * area)))
    }

    /// Check if a point is inside (only for closed polylines)
    pub fn contains_point(&self, point: &Point2d) -> Sketch2dResult<bool> {
        if !self.is_closed {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "is_closed".to_string(),
                value: "false".to_string(),
                constraint: "polyline must be closed to test containment".to_string(),
            });
        }

        // Ray casting algorithm
        let mut inside = false;
        let n = self.vertices.len();

        for i in 0..n {
            let j = (i + 1) % n;
            let vi = &self.vertices[i];
            let vj = &self.vertices[j];

            if ((vi.y > point.y) != (vj.y > point.y))
                && (point.x < (vj.x - vi.x) * (point.y - vi.y) / (vj.y - vi.y) + vi.x)
            {
                inside = !inside;
            }
        }

        Ok(inside)
    }

    /// Find the closest point on the polyline to a given point
    pub fn closest_point(&self, point: &Point2d) -> (Point2d, usize, f64) {
        let mut min_dist = f64::INFINITY;
        let mut closest_point = Point2d::ORIGIN;
        let mut closest_segment = 0;
        let mut closest_param = 0.0;

        for (i, segment) in self.segments().iter().enumerate() {
            let pt = segment.closest_point(point);
            let dist = point.distance_squared_to(&pt);

            if dist < min_dist {
                min_dist = dist;
                closest_point = pt;
                closest_segment = i;

                // Calculate parameter t
                let seg_vec = Vector2d::from_points(&segment.start, &segment.end);
                let pt_vec = Vector2d::from_points(&segment.start, &pt);
                let seg_length_sq = seg_vec.magnitude_squared();
                if seg_length_sq > STRICT_TOLERANCE.distance() * STRICT_TOLERANCE.distance() {
                    closest_param = seg_vec.dot(&pt_vec) / seg_length_sq;
                } else {
                    closest_param = 0.0;
                }
            }
        }

        (closest_point, closest_segment, closest_param)
    }

    /// Insert a vertex at the specified index
    pub fn insert_vertex(&mut self, index: usize, vertex: Point2d) -> Sketch2dResult<()> {
        if index > self.vertices.len() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "index".to_string(),
                value: index.to_string(),
                constraint: format!("must be <= {}", self.vertices.len()),
            });
        }

        // Check for duplicate vertices
        let tolerance = Tolerance2d::default();

        if index > 0 && self.vertices[index - 1].coincident_with(&vertex, &tolerance) {
            return Err(Sketch2dError::DegenerateGeometry {
                entity: "Polyline2d".to_string(),
                reason: "New vertex coincident with previous vertex".to_string(),
            });
        }

        if index < self.vertices.len() && self.vertices[index].coincident_with(&vertex, &tolerance)
        {
            return Err(Sketch2dError::DegenerateGeometry {
                entity: "Polyline2d".to_string(),
                reason: "New vertex coincident with next vertex".to_string(),
            });
        }

        self.vertices.insert(index, vertex);
        Ok(())
    }

    /// Remove a vertex at the specified index
    pub fn remove_vertex(&mut self, index: usize) -> Sketch2dResult<Point2d> {
        if index >= self.vertices.len() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "index".to_string(),
                value: index.to_string(),
                constraint: format!("must be < {}", self.vertices.len()),
            });
        }

        if self.vertices.len() <= 2 {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "vertices".to_string(),
                value: format!("{} vertices", self.vertices.len()),
                constraint: "cannot remove vertex, minimum 2 required".to_string(),
            });
        }

        Ok(self.vertices.remove(index))
    }

    /// Reverse the order of vertices
    pub fn reverse(&mut self) {
        self.vertices.reverse();
    }

    /// Simplify the polyline using Douglas-Peucker algorithm
    pub fn simplify(&self, tolerance: f64) -> Sketch2dResult<Self> {
        if tolerance <= 0.0 {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "tolerance".to_string(),
                value: tolerance.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        let simplified = self.douglas_peucker(&self.vertices, tolerance);

        Self::new(simplified, self.is_closed)
    }

    /// Douglas-Peucker algorithm implementation
    #[allow(clippy::expect_used)] // points.len() > 2: verified by early return above
    fn douglas_peucker(&self, points: &[Point2d], tolerance: f64) -> Vec<Point2d> {
        if points.len() <= 2 {
            return points.to_vec();
        }

        // `points.len() > 2` is verified above, so `last()` always returns Some.
        const LEN_INVARIANT: &str =
            "points.len() > 2: verified by early return above (len <= 2 case)";

        // Find the point with maximum distance
        let mut max_dist = 0.0;
        let mut max_index = 0;

        let last_point = *points.last().expect(LEN_INVARIANT);
        let line = match LineSegment2d::new(points[0], last_point) {
            Ok(l) => l,
            Err(_) => return vec![points[0], last_point],
        };

        for i in 1..points.len() - 1 {
            let dist = line.distance_to_point(&points[i]);
            if dist > max_dist {
                max_dist = dist;
                max_index = i;
            }
        }

        // If max distance is greater than tolerance, recursively simplify
        if max_dist > tolerance {
            let mut result = self.douglas_peucker(&points[..=max_index], tolerance);
            result.pop(); // Remove duplicate point
            result.extend(self.douglas_peucker(&points[max_index..], tolerance));
            result
        } else {
            vec![points[0], last_point]
        }
    }
}

/// A parametric polyline entity with constraint tracking
pub struct ParametricPolyline2d {
    /// Unique identifier
    pub id: Polyline2dId,
    /// Polyline geometry
    pub polyline: Polyline2d,
    /// Number of constraints applied
    constraint_count: usize,
    /// Construction geometry flag
    pub is_construction: bool,
}

impl ParametricPolyline2d {
    /// Create a new parametric polyline
    pub fn new(polyline: Polyline2d) -> Self {
        Self {
            id: Polyline2dId::new(),
            polyline,
            constraint_count: 0,
            is_construction: false,
        }
    }

    /// Add a constraint
    pub fn add_constraint(&mut self) {
        self.constraint_count += 1;
    }

    /// Remove a constraint
    pub fn remove_constraint(&mut self) {
        if self.constraint_count > 0 {
            self.constraint_count -= 1;
        }
    }
}

impl SketchEntity2d for ParametricPolyline2d {
    fn degrees_of_freedom(&self) -> usize {
        self.polyline.vertices.len() * 2
    }

    fn constraint_count(&self) -> usize {
        self.constraint_count
    }

    fn bounding_box(&self) -> (Point2d, Point2d) {
        if self.polyline.vertices.is_empty() {
            return (Point2d::ORIGIN, Point2d::ORIGIN);
        }

        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;

        for vertex in &self.polyline.vertices {
            min_x = min_x.min(vertex.x);
            min_y = min_y.min(vertex.y);
            max_x = max_x.max(vertex.x);
            max_y = max_y.max(vertex.y);
        }

        (Point2d::new(min_x, min_y), Point2d::new(max_x, max_y))
    }

    fn transform(&mut self, matrix: &Matrix3) {
        for vertex in &mut self.polyline.vertices {
            *vertex = matrix.transform_point(vertex);
        }
    }

    fn clone_entity(&self) -> Box<dyn SketchEntity2d> {
        Box::new(ParametricPolyline2d {
            id: Polyline2dId::new(),
            polyline: self.polyline.clone(),
            constraint_count: 0,
            is_construction: self.is_construction,
        })
    }
}

/// Storage for polylines using DashMap
pub struct Polyline2dStore {
    /// All polylines indexed by ID
    polylines: Arc<DashMap<Polyline2dId, ParametricPolyline2d>>,
    /// Spatial index for efficient queries
    spatial_index: Arc<DashMap<(i32, i32), Vec<Polyline2dId>>>,
    /// Grid size for spatial indexing
    grid_size: f64,
}

impl Polyline2dStore {
    /// Create a new polyline store
    pub fn new(grid_size: f64) -> Self {
        Self {
            polylines: Arc::new(DashMap::new()),
            spatial_index: Arc::new(DashMap::new()),
            grid_size,
        }
    }

    /// Add a polyline to the store
    pub fn add(&self, polyline: ParametricPolyline2d) -> Polyline2dId {
        let id = polyline.id;

        // Update spatial index
        let (min, max) = polyline.bounding_box();
        self.update_spatial_index(id, min, max);

        self.polylines.insert(id, polyline);
        id
    }

    /// Update spatial index for a polyline
    fn update_spatial_index(&self, id: Polyline2dId, min: Point2d, max: Point2d) {
        let min_grid_x = (min.x / self.grid_size).floor() as i32;
        let min_grid_y = (min.y / self.grid_size).floor() as i32;
        let max_grid_x = (max.x / self.grid_size).ceil() as i32;
        let max_grid_y = (max.y / self.grid_size).ceil() as i32;

        for x in min_grid_x..=max_grid_x {
            for y in min_grid_y..=max_grid_y {
                self.spatial_index.entry((x, y)).or_default().push(id);
            }
        }
    }
}
