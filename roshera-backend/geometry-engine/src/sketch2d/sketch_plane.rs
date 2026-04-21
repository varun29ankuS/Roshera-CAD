//! Sketch plane management for 2D sketching
//!
//! This module implements sketch planes that define the 2D coordinate system
//! for sketches in 3D space. A sketch plane is defined by an origin point
//! and two orthogonal axes (X and Y), with the Z axis computed as their cross product.

use super::{Point2d, Sketch2dError, Sketch2dResult, Vector2d};
use crate::math::{Matrix4, Point3, Vector3};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Standard plane orientations
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PlaneOrientation {
    /// XY plane (Z = 0)
    XY,
    /// XZ plane (Y = 0)
    XZ,
    /// YZ plane (X = 0)
    YZ,
    /// Custom orientation
    Custom,
}

/// A sketch plane in 3D space
///
/// Defines a 2D coordinate system embedded in 3D space.
/// The plane is defined by:
/// - Origin: A point in 3D space
/// - X-axis: The local X direction
/// - Y-axis: The local Y direction (orthogonal to X)
/// - Z-axis: The normal to the plane (X × Y)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SketchPlane {
    /// Origin of the sketch plane in 3D space
    pub origin: Point3,
    /// X-axis direction (normalized)
    pub x_axis: Vector3,
    /// Y-axis direction (normalized)
    pub y_axis: Vector3,
    /// Normal to the plane (z_axis = x_axis × y_axis)
    pub normal: Vector3,
    /// Plane orientation type
    pub orientation: PlaneOrientation,
    /// Transformation matrix from 2D sketch to 3D world
    to_world: Matrix4,
    /// Transformation matrix from 3D world to 2D sketch
    to_sketch: Matrix4,
}

impl SketchPlane {
    /// Create a new sketch plane
    pub fn new(origin: Point3, x_axis: Vector3, y_axis: Vector3) -> Sketch2dResult<Self> {
        // Normalize axes
        let x = x_axis
            .normalize()
            .map_err(|_| Sketch2dError::InvalidParameter {
                parameter: "x_axis".to_string(),
                value: "zero vector".to_string(),
                constraint: "X axis cannot be a zero vector".to_string(),
            })?;
        let y = y_axis
            .normalize()
            .map_err(|_| Sketch2dError::InvalidParameter {
                parameter: "y_axis".to_string(),
                value: "zero vector".to_string(),
                constraint: "Y axis cannot be a zero vector".to_string(),
            })?;

        // Check if axes are orthogonal
        if x.dot(&y).abs() > 1e-6 {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "axes".to_string(),
                value: "non-orthogonal".to_string(),
                constraint: "X and Y axes must be orthogonal".to_string(),
            });
        }

        // Compute normal
        let normal = x
            .cross(&y)
            .normalize()
            .map_err(|_| Sketch2dError::InvalidParameter {
                parameter: "axes".to_string(),
                value: "parallel axes".to_string(),
                constraint: "X and Y axes cannot be parallel".to_string(),
            })?;

        // Build transformation matrices
        let to_world = Self::build_to_world_matrix(&origin, &x, &y, &normal);
        let to_sketch = Self::build_to_sketch_matrix(&origin, &x, &y, &normal);

        Ok(Self {
            origin,
            x_axis: x,
            y_axis: y,
            normal,
            orientation: PlaneOrientation::Custom,
            to_world,
            to_sketch,
        })
    }

    /// Create a sketch plane from origin and normal
    pub fn from_normal(origin: Point3, normal: Vector3) -> Sketch2dResult<Self> {
        let n = normal
            .normalize()
            .map_err(|_| Sketch2dError::InvalidParameter {
                parameter: "normal".to_string(),
                value: "zero vector".to_string(),
                constraint: "Normal vector cannot be zero".to_string(),
            })?;

        // Generate orthogonal axes
        let (x_axis, y_axis) = Self::generate_axes_from_normal(&n);

        Self::new(origin, x_axis, y_axis)
    }

    /// Create a sketch plane from three points
    pub fn from_three_points(p1: Point3, p2: Point3, p3: Point3) -> Sketch2dResult<Self> {
        // Use p1 as origin
        let origin = p1;

        // X-axis from p1 to p2
        let x_axis = Vector3::new(p2.x - p1.x, p2.y - p1.y, p2.z - p1.z);
        if x_axis.magnitude() < 1e-6 {
            return Err(Sketch2dError::DegenerateGeometry {
                entity: "SketchPlane".to_string(),
                reason: "Points p1 and p2 are coincident".to_string(),
            });
        }

        // Temporary vector from p1 to p3
        let v13 = Vector3::new(p3.x - p1.x, p3.y - p1.y, p3.z - p1.z);
        if v13.magnitude() < 1e-6 {
            return Err(Sketch2dError::DegenerateGeometry {
                entity: "SketchPlane".to_string(),
                reason: "Points p1 and p3 are coincident".to_string(),
            });
        }

        // Normal is perpendicular to both vectors
        let normal = x_axis.cross(&v13);
        if normal.magnitude() < 1e-6 {
            return Err(Sketch2dError::DegenerateGeometry {
                entity: "SketchPlane".to_string(),
                reason: "Three points are collinear".to_string(),
            });
        }

        // Y-axis is perpendicular to both X and normal
        let y_axis = normal.cross(&x_axis);

        Self::new(origin, x_axis, y_axis)
    }

    /// Create the XY plane (Z = 0)
    pub fn xy() -> Self {
        Self::xy_at(0.0)
    }

    /// Create the XY plane at a given Z coordinate
    pub fn xy_at(z: f64) -> Self {
        let mut plane = Self {
            origin: Point3::new(0.0, 0.0, z),
            x_axis: Vector3::X,
            y_axis: Vector3::Y,
            normal: Vector3::Z,
            orientation: PlaneOrientation::XY,
            to_world: Matrix4::IDENTITY,
            to_sketch: Matrix4::IDENTITY,
        };

        plane.to_world =
            Self::build_to_world_matrix(&plane.origin, &plane.x_axis, &plane.y_axis, &plane.normal);
        plane.to_sketch = Self::build_to_sketch_matrix(
            &plane.origin,
            &plane.x_axis,
            &plane.y_axis,
            &plane.normal,
        );

        plane
    }

    /// Create the XZ plane (Y = 0)
    pub fn xz() -> Self {
        Self::xz_at(0.0)
    }

    /// Create the XZ plane at a given Y coordinate
    pub fn xz_at(y: f64) -> Self {
        let mut plane = Self {
            origin: Point3::new(0.0, y, 0.0),
            x_axis: Vector3::X,
            y_axis: Vector3::Z,
            normal: Vector3::new(0.0, -1.0, 0.0), // -Y direction
            orientation: PlaneOrientation::XZ,
            to_world: Matrix4::IDENTITY,
            to_sketch: Matrix4::IDENTITY,
        };

        plane.to_world =
            Self::build_to_world_matrix(&plane.origin, &plane.x_axis, &plane.y_axis, &plane.normal);
        plane.to_sketch = Self::build_to_sketch_matrix(
            &plane.origin,
            &plane.x_axis,
            &plane.y_axis,
            &plane.normal,
        );

        plane
    }

    /// Create the YZ plane (X = 0)
    pub fn yz() -> Self {
        Self::yz_at(0.0)
    }

    /// Create the YZ plane at a given X coordinate
    pub fn yz_at(x: f64) -> Self {
        let mut plane = Self {
            origin: Point3::new(x, 0.0, 0.0),
            x_axis: Vector3::Y,
            y_axis: Vector3::Z,
            normal: Vector3::X,
            orientation: PlaneOrientation::YZ,
            to_world: Matrix4::IDENTITY,
            to_sketch: Matrix4::IDENTITY,
        };

        plane.to_world =
            Self::build_to_world_matrix(&plane.origin, &plane.x_axis, &plane.y_axis, &plane.normal);
        plane.to_sketch = Self::build_to_sketch_matrix(
            &plane.origin,
            &plane.x_axis,
            &plane.y_axis,
            &plane.normal,
        );

        plane
    }

    /// Transform a 2D point to 3D world coordinates
    pub fn to_world_point(&self, point: &Point2d) -> Point3 {
        let p = Point3::new(point.x, point.y, 0.0);
        self.to_world.transform_point(&p)
    }

    /// Transform a 3D point to 2D sketch coordinates
    pub fn to_sketch_point(&self, point: &Point3) -> Point2d {
        let p = self.to_sketch.transform_point(point);
        Point2d::new(p.x, p.y)
    }

    /// Transform a 2D vector to 3D world coordinates
    pub fn to_world_vector(&self, vector: &Vector2d) -> Vector3 {
        let v = Vector3::new(vector.x, vector.y, 0.0);
        self.to_world.transform_vector(&v)
    }

    /// Transform a 3D vector to 2D sketch coordinates
    pub fn to_sketch_vector(&self, vector: &Vector3) -> Vector2d {
        let v = self.to_sketch.transform_vector(vector);
        Vector2d::new(v.x, v.y)
    }

    /// Project a 3D point onto the plane
    pub fn project_point(&self, point: &Point3) -> Point3 {
        // Vector from origin to point
        let v = Vector3::new(
            point.x - self.origin.x,
            point.y - self.origin.y,
            point.z - self.origin.z,
        );

        // Remove component normal to plane
        let distance = v.dot(&self.normal);
        Point3::new(
            point.x - distance * self.normal.x,
            point.y - distance * self.normal.y,
            point.z - distance * self.normal.z,
        )
    }

    /// Get the signed distance from a point to the plane
    pub fn distance_to_point(&self, point: &Point3) -> f64 {
        let v = Vector3::new(
            point.x - self.origin.x,
            point.y - self.origin.y,
            point.z - self.origin.z,
        );
        v.dot(&self.normal)
    }

    /// Check if a point lies on the plane (within tolerance)
    pub fn contains_point(&self, point: &Point3, tolerance: f64) -> bool {
        self.distance_to_point(point).abs() < tolerance
    }

    /// Get the intersection of a line with the plane
    pub fn intersect_line(&self, line_origin: &Point3, line_direction: &Vector3) -> Option<Point3> {
        let denominator = line_direction.dot(&self.normal);

        // Line is parallel to plane
        if denominator.abs() < 1e-10 {
            return None;
        }

        let v = Vector3::new(
            self.origin.x - line_origin.x,
            self.origin.y - line_origin.y,
            self.origin.z - line_origin.z,
        );
        let t = v.dot(&self.normal) / denominator;

        Some(Point3::new(
            line_origin.x + t * line_direction.x,
            line_origin.y + t * line_direction.y,
            line_origin.z + t * line_direction.z,
        ))
    }

    /// Offset the plane by a distance along its normal
    pub fn offset(&self, distance: f64) -> Self {
        let new_origin = Point3::new(
            self.origin.x + distance * self.normal.x,
            self.origin.y + distance * self.normal.y,
            self.origin.z + distance * self.normal.z,
        );

        let mut plane = self.clone();
        plane.origin = new_origin;
        plane.to_world =
            Self::build_to_world_matrix(&new_origin, &self.x_axis, &self.y_axis, &self.normal);
        plane.to_sketch =
            Self::build_to_sketch_matrix(&new_origin, &self.x_axis, &self.y_axis, &self.normal);

        plane
    }

    /// Rotate the plane around an axis
    pub fn rotate(&self, axis: &Vector3, angle: f64) -> Sketch2dResult<Self> {
        let rotation =
            Matrix4::from_axis_angle(axis, angle).map_err(|_| Sketch2dError::InvalidParameter {
                parameter: "axis".to_string(),
                value: "invalid axis or angle".to_string(),
                constraint: "Axis must be non-zero vector".to_string(),
            })?;

        let new_x = rotation.transform_vector(&self.x_axis);
        let new_y = rotation.transform_vector(&self.y_axis);
        let new_origin = rotation.transform_point(&self.origin);

        Self::new(new_origin, new_x, new_y)
    }

    /// Build transformation matrix from sketch to world
    fn build_to_world_matrix(origin: &Point3, x: &Vector3, y: &Vector3, z: &Vector3) -> Matrix4 {
        Matrix4::from_cols(
            Vector3::new(x.x, y.x, z.x),
            Vector3::new(x.y, y.y, z.y),
            Vector3::new(x.z, y.z, z.z),
            Vector3::new(origin.x, origin.y, origin.z),
        )
    }

    /// Build transformation matrix from world to sketch
    fn build_to_sketch_matrix(origin: &Point3, x: &Vector3, y: &Vector3, z: &Vector3) -> Matrix4 {
        // Inverse of to_world matrix
        // For orthonormal basis, inverse is transpose of rotation part
        let translation = Vector3::new(
            -origin.x * x.x - origin.y * x.y - origin.z * x.z,
            -origin.x * y.x - origin.y * y.y - origin.z * y.z,
            -origin.x * z.x - origin.y * z.y - origin.z * z.z,
        );

        Matrix4::from_cols(
            Vector3::new(x.x, x.y, x.z),
            Vector3::new(y.x, y.y, y.z),
            Vector3::new(z.x, z.y, z.z),
            translation,
        )
    }

    /// Generate orthogonal axes from a normal vector
    fn generate_axes_from_normal(normal: &Vector3) -> (Vector3, Vector3) {
        // Choose a vector not parallel to normal
        let up = if normal.z.abs() < 0.9 {
            Vector3::Z
        } else {
            Vector3::X
        };

        // X-axis is perpendicular to normal and up
        let x_axis = up.cross(normal).normalize().unwrap_or(Vector3::X);

        // Y-axis completes the orthonormal basis
        let y_axis = normal.cross(&x_axis).normalize().unwrap_or(Vector3::Y);

        (x_axis, y_axis)
    }
}

impl fmt::Display for SketchPlane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.orientation {
            PlaneOrientation::XY => write!(f, "XY Plane at Z={}", self.origin.z),
            PlaneOrientation::XZ => write!(f, "XZ Plane at Y={}", self.origin.y),
            PlaneOrientation::YZ => write!(f, "YZ Plane at X={}", self.origin.x),
            PlaneOrientation::Custom => write!(
                f,
                "Custom Plane at ({:.2}, {:.2}, {:.2})",
                self.origin.x, self.origin.y, self.origin.z
            ),
        }
    }
}

/// Reference to a sketch plane
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlaneReference {
    /// Reference to a standard plane
    Standard(PlaneOrientation),
    /// Reference to a face on a 3D model
    Face(String), // Face ID
    /// Custom plane definition
    Custom(SketchPlane),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_standard_planes() {
        // XY plane
        let xy = SketchPlane::xy();
        assert_eq!(xy.origin, Point3::new(0.0, 0.0, 0.0));
        assert_eq!(xy.normal, Vector3::Z);

        // XZ plane
        let xz = SketchPlane::xz();
        assert_eq!(xz.origin, Point3::new(0.0, 0.0, 0.0));
        assert_eq!(xz.normal, Vector3::new(0.0, -1.0, 0.0));

        // YZ plane
        let yz = SketchPlane::yz();
        assert_eq!(yz.origin, Point3::new(0.0, 0.0, 0.0));
        assert_eq!(yz.normal, Vector3::X);
    }

    #[test]
    fn test_point_transformation() {
        let plane = SketchPlane::xy_at(5.0);

        // Transform 2D to 3D
        let p2d = Point2d::new(1.0, 2.0);
        let p3d = plane.to_world_point(&p2d);
        assert_eq!(p3d, Point3::new(1.0, 2.0, 5.0));

        // Transform 3D to 2D
        let p3d = Point3::new(3.0, 4.0, 5.0);
        let p2d = plane.to_sketch_point(&p3d);
        assert_eq!(p2d, Point2d::new(3.0, 4.0));
    }

    #[test]
    fn test_plane_from_three_points() {
        let p1 = Point3::new(0.0, 0.0, 0.0);
        let p2 = Point3::new(1.0, 0.0, 0.0);
        let p3 = Point3::new(0.0, 1.0, 0.0);

        let plane = SketchPlane::from_three_points(p1, p2, p3).unwrap();

        assert_eq!(plane.origin, p1);
        assert!((plane.normal - Vector3::Z).magnitude() < 1e-6);
    }

    #[test]
    fn test_point_projection() {
        let plane = SketchPlane::xy();

        let point = Point3::new(1.0, 2.0, 3.0);
        let projected = plane.project_point(&point);

        assert_eq!(projected, Point3::new(1.0, 2.0, 0.0));
    }

    #[test]
    fn test_line_intersection() {
        let plane = SketchPlane::xy();

        // Line pointing down from above
        let line_origin = Point3::new(1.0, 2.0, 5.0);
        let line_direction = Vector3::new(0.0, 0.0, -1.0);

        let intersection = plane.intersect_line(&line_origin, &line_direction).unwrap();
        assert_eq!(intersection, Point3::new(1.0, 2.0, 0.0));

        // Line parallel to plane
        let parallel_dir = Vector3::new(1.0, 0.0, 0.0);
        assert!(plane.intersect_line(&line_origin, &parallel_dir).is_none());
    }
}
