//! 2D Pattern and Array Operations
//!
//! This module provides pattern and array operations for duplicating sketch entities
//! in organized arrangements. Supports linear, circular, and rectangular patterns.
//!
//! # Pattern Types
//! - **Linear Pattern**: Entities arranged along a straight line with spacing
//! - **Circular Pattern**: Entities arranged around a center point
//! - **Rectangular Pattern**: Entities arranged in a grid pattern
//! - **Mirror Pattern**: Entities mirrored across a line

use super::constraints::EntityRef;
use super::{Line2d, Matrix3, Point2d, Sketch2dError, Sketch2dResult, Tolerance2d, Vector2d};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for a pattern
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PatternId(pub Uuid);

impl PatternId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

/// Types of patterns available
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PatternType {
    /// Linear pattern along a vector
    Linear {
        /// Direction vector for the pattern
        direction: Vector2d,
        /// Spacing between instances
        spacing: f64,
        /// Number of instances (including original)
        count: usize,
    },

    /// Circular pattern around a center point
    Circular {
        /// Center point of rotation
        center: Point2d,
        /// Angle between instances (in radians)
        angle_step: f64,
        /// Number of instances (including original)
        count: usize,
        /// Whether to rotate entities themselves
        rotate_entities: bool,
    },

    /// Rectangular grid pattern
    Rectangular {
        /// Direction of first axis
        direction_u: Vector2d,
        /// Direction of second axis
        direction_v: Vector2d,
        /// Spacing along first axis
        spacing_u: f64,
        /// Spacing along second axis
        spacing_v: f64,
        /// Number of instances along first axis
        count_u: usize,
        /// Number of instances along second axis
        count_v: usize,
    },

    /// Mirror pattern across a line
    Mirror {
        /// Line to mirror across
        mirror_line: Line2d,
        /// Whether to keep original entities
        keep_original: bool,
    },
}

/// A pattern operation result
#[derive(Debug, Clone)]
pub struct PatternResult {
    /// Pattern ID
    pub pattern_id: PatternId,
    /// Original entity references
    pub source_entities: Vec<EntityRef>,
    /// Generated entity references (organized by instance)
    pub pattern_entities: Vec<Vec<EntityRef>>,
    /// Total number of entities created
    pub total_created: usize,
}

/// Pattern creation parameters
#[derive(Debug, Clone)]
pub struct PatternParams {
    /// Type of pattern to create
    pub pattern_type: PatternType,
    /// Entities to pattern
    pub source_entities: Vec<EntityRef>,
    /// Tolerance for geometric operations
    pub tolerance: Tolerance2d,
    /// Whether to maintain constraints on patterned entities
    pub maintain_constraints: bool,
}

/// Pattern operations manager
pub struct PatternOperations {
    /// Tolerance for geometric operations
    pub tolerance: Tolerance2d,
}

impl PatternOperations {
    /// Create new pattern operations manager
    pub fn new(tolerance: Tolerance2d) -> Self {
        Self { tolerance }
    }

    /// Create a linear pattern
    pub fn create_linear_pattern(
        &self,
        entities: Vec<EntityRef>,
        direction: Vector2d,
        spacing: f64,
        count: usize,
    ) -> Sketch2dResult<PatternResult> {
        if count == 0 {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "count".to_string(),
                value: count.to_string(),
                constraint: "must be greater than 0".to_string(),
            });
        }

        if spacing <= 0.0 {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "spacing".to_string(),
                value: spacing.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        let normalized_direction =
            direction
                .normalize()
                .map_err(|_| Sketch2dError::InvalidParameter {
                    parameter: "direction".to_string(),
                    value: "zero vector".to_string(),
                    constraint: "must be non-zero".to_string(),
                })?;
        let mut pattern_entities = Vec::new();

        // Create pattern instances
        for i in 1..count {
            // Skip instance 0 (original)
            let offset = normalized_direction.scale(spacing * i as f64);
            let transform = Matrix3::translation(offset.x, offset.y);

            // Transform entities for this instance
            let instance_entities = self.transform_entities(&entities, &transform)?;
            pattern_entities.push(instance_entities);
        }

        let total_created = pattern_entities.iter().map(|v| v.len()).sum();

        Ok(PatternResult {
            pattern_id: PatternId::new(),
            source_entities: entities,
            pattern_entities,
            total_created,
        })
    }

    /// Create a circular pattern
    pub fn create_circular_pattern(
        &self,
        entities: Vec<EntityRef>,
        center: Point2d,
        angle_step: f64,
        count: usize,
        rotate_entities: bool,
    ) -> Sketch2dResult<PatternResult> {
        if count == 0 {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "count".to_string(),
                value: count.to_string(),
                constraint: "must be greater than 0".to_string(),
            });
        }

        let mut pattern_entities = Vec::new();

        // Create pattern instances
        for i in 1..count {
            // Skip instance 0 (original)
            let angle = angle_step * i as f64;

            // Create transformation matrix
            let transform = if rotate_entities {
                // Rotate around center with entity rotation
                let translate_to_origin = Matrix3::translation(-center.x, -center.y);
                let rotate = Matrix3::rotation(angle);
                let translate_back = Matrix3::translation(center.x, center.y);
                translate_back
                    .multiply(&rotate)
                    .multiply(&translate_to_origin)
            } else {
                // Only translate entities around circle, don't rotate them
                let offset_x = center.x
                    + angle.cos() * self.get_entity_distance_from_center(&entities, &center)?;
                let offset_y = center.y
                    + angle.sin() * self.get_entity_distance_from_center(&entities, &center)?;
                Matrix3::translation(offset_x - center.x, offset_y - center.y)
            };

            // Transform entities for this instance
            let instance_entities = self.transform_entities(&entities, &transform)?;
            pattern_entities.push(instance_entities);
        }

        let total_created = pattern_entities.iter().map(|v| v.len()).sum();

        Ok(PatternResult {
            pattern_id: PatternId::new(),
            source_entities: entities,
            pattern_entities,
            total_created,
        })
    }

    /// Create a rectangular pattern
    pub fn create_rectangular_pattern(
        &self,
        entities: Vec<EntityRef>,
        direction_u: Vector2d,
        direction_v: Vector2d,
        spacing_u: f64,
        spacing_v: f64,
        count_u: usize,
        count_v: usize,
    ) -> Sketch2dResult<PatternResult> {
        if count_u == 0 || count_v == 0 {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "count".to_string(),
                value: format!("{}, {}", count_u, count_v),
                constraint: "both counts must be greater than 0".to_string(),
            });
        }

        if spacing_u <= 0.0 || spacing_v <= 0.0 {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "spacing".to_string(),
                value: format!("{}, {}", spacing_u, spacing_v),
                constraint: "both spacings must be positive".to_string(),
            });
        }

        let normalized_u =
            direction_u
                .normalize()
                .map_err(|_| Sketch2dError::InvalidParameter {
                    parameter: "direction_u".to_string(),
                    value: "zero vector".to_string(),
                    constraint: "must be non-zero".to_string(),
                })?;
        let normalized_v =
            direction_v
                .normalize()
                .map_err(|_| Sketch2dError::InvalidParameter {
                    parameter: "direction_v".to_string(),
                    value: "zero vector".to_string(),
                    constraint: "must be non-zero".to_string(),
                })?;
        let mut pattern_entities = Vec::new();

        // Create pattern instances
        for i in 0..count_u {
            for j in 0..count_v {
                // Skip the original (0, 0)
                if i == 0 && j == 0 {
                    continue;
                }

                let offset_u = normalized_u.scale(spacing_u * i as f64);
                let offset_v = normalized_v.scale(spacing_v * j as f64);
                let total_offset = offset_u.add(&offset_v);
                let transform = Matrix3::translation(total_offset.x, total_offset.y);

                // Transform entities for this instance
                let instance_entities = self.transform_entities(&entities, &transform)?;
                pattern_entities.push(instance_entities);
            }
        }

        let total_created = pattern_entities.iter().map(|v| v.len()).sum();

        Ok(PatternResult {
            pattern_id: PatternId::new(),
            source_entities: entities,
            pattern_entities,
            total_created,
        })
    }

    /// Create a mirror pattern
    pub fn create_mirror_pattern(
        &self,
        entities: Vec<EntityRef>,
        mirror_line: Line2d,
        keep_original: bool,
    ) -> Sketch2dResult<PatternResult> {
        // Create reflection transformation across the mirror line
        let line_point = mirror_line.point_at(0.0);
        let line_direction = mirror_line.direction;
        let line_normal = line_direction.perpendicular();

        // Reflection matrix across a line through origin with normal n:
        // R = I - 2*n*n^T
        // But we need to translate to line first
        let translate_to_origin = Matrix3::translation(-line_point.x, -line_point.y);
        let translate_back = Matrix3::translation(line_point.x, line_point.y);

        // Create reflection matrix
        let nx = line_normal.x;
        let ny = line_normal.y;
        let reflection_data = [
            [1.0 - 2.0 * nx * nx, -2.0 * nx * ny, 0.0],
            [-2.0 * nx * ny, 1.0 - 2.0 * ny * ny, 0.0],
            [0.0, 0.0, 1.0],
        ];
        let reflection = Matrix3 {
            data: reflection_data,
        };

        let transform = translate_back
            .multiply(&reflection)
            .multiply(&translate_to_origin);

        // Transform entities
        let mirrored_entities = self.transform_entities(&entities, &transform)?;

        let pattern_entities = if keep_original {
            vec![mirrored_entities]
        } else {
            vec![mirrored_entities] // Original will be replaced
        };

        let total_created = pattern_entities.iter().map(|v| v.len()).sum();

        Ok(PatternResult {
            pattern_id: PatternId::new(),
            source_entities: entities,
            pattern_entities,
            total_created,
        })
    }

    /// Helper function to transform entities (placeholder)
    fn transform_entities(
        &self,
        entities: &[EntityRef],
        _transform: &Matrix3,
    ) -> Sketch2dResult<Vec<EntityRef>> {
        // This would need access to the actual sketch to transform entities
        // For now, return placeholder entity refs
        Ok(entities
            .iter()
            .map(|entity| {
                // Create new entity IDs - in real implementation would transform geometry
                match entity {
                    EntityRef::Point(_) => EntityRef::Point(crate::sketch2d::Point2dId::new()),
                    EntityRef::Line(_) => EntityRef::Line(crate::sketch2d::Line2dId::new()),
                    EntityRef::Arc(_) => EntityRef::Arc(crate::sketch2d::Arc2dId::new()),
                    EntityRef::Circle(_) => EntityRef::Circle(crate::sketch2d::Circle2dId::new()),
                    EntityRef::Rectangle(_) => {
                        EntityRef::Rectangle(crate::sketch2d::Rectangle2dId::new())
                    }
                    EntityRef::Ellipse(_) => {
                        EntityRef::Ellipse(crate::sketch2d::Ellipse2dId::new())
                    }
                    EntityRef::Spline(_) => EntityRef::Spline(crate::sketch2d::Spline2dId::new()),
                    EntityRef::Polyline(_) => {
                        EntityRef::Polyline(crate::sketch2d::Polyline2dId::new())
                    }
                }
            })
            .collect())
    }

    /// Helper function to estimate entity distance from center
    fn get_entity_distance_from_center(
        &self,
        _entities: &[EntityRef],
        _center: &Point2d,
    ) -> Sketch2dResult<f64> {
        // This would calculate the representative distance of entities from center
        // For now, return a default distance
        Ok(10.0)
    }
}

/// Pattern validation utilities
impl PatternOperations {
    /// Validate pattern parameters
    pub fn validate_pattern_params(&self, params: &PatternParams) -> Sketch2dResult<()> {
        if params.source_entities.is_empty() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "source_entities".to_string(),
                value: "empty".to_string(),
                constraint: "must contain at least one entity".to_string(),
            });
        }

        match &params.pattern_type {
            PatternType::Linear {
                direction,
                spacing,
                count,
            } => {
                if direction.magnitude() < self.tolerance.distance {
                    return Err(Sketch2dError::InvalidParameter {
                        parameter: "direction".to_string(),
                        value: direction.magnitude().to_string(),
                        constraint: "must be non-zero vector".to_string(),
                    });
                }
                if *spacing <= 0.0 {
                    return Err(Sketch2dError::InvalidParameter {
                        parameter: "spacing".to_string(),
                        value: spacing.to_string(),
                        constraint: "must be positive".to_string(),
                    });
                }
                if *count == 0 {
                    return Err(Sketch2dError::InvalidParameter {
                        parameter: "count".to_string(),
                        value: count.to_string(),
                        constraint: "must be greater than 0".to_string(),
                    });
                }
            }

            PatternType::Circular {
                angle_step, count, ..
            } => {
                if *count == 0 {
                    return Err(Sketch2dError::InvalidParameter {
                        parameter: "count".to_string(),
                        value: count.to_string(),
                        constraint: "must be greater than 0".to_string(),
                    });
                }
                if angle_step.abs() < self.tolerance.angle {
                    return Err(Sketch2dError::InvalidParameter {
                        parameter: "angle_step".to_string(),
                        value: angle_step.to_string(),
                        constraint: "must be non-zero".to_string(),
                    });
                }
            }

            PatternType::Rectangular {
                spacing_u,
                spacing_v,
                count_u,
                count_v,
                ..
            } => {
                if *spacing_u <= 0.0 || *spacing_v <= 0.0 {
                    return Err(Sketch2dError::InvalidParameter {
                        parameter: "spacing".to_string(),
                        value: format!("{}, {}", spacing_u, spacing_v),
                        constraint: "both spacings must be positive".to_string(),
                    });
                }
                if *count_u == 0 || *count_v == 0 {
                    return Err(Sketch2dError::InvalidParameter {
                        parameter: "count".to_string(),
                        value: format!("{}, {}", count_u, count_v),
                        constraint: "both counts must be greater than 0".to_string(),
                    });
                }
            }

            PatternType::Mirror { .. } => {
                // Mirror patterns are generally valid if entities exist
            }
        }

        Ok(())
    }

    /// Calculate the total number of entities that will be created
    pub fn calculate_total_instances(&self, pattern_type: &PatternType) -> usize {
        match pattern_type {
            PatternType::Linear { count, .. } => *count,
            PatternType::Circular { count, .. } => *count,
            PatternType::Rectangular {
                count_u, count_v, ..
            } => count_u * count_v,
            PatternType::Mirror { keep_original, .. } => {
                if *keep_original {
                    2
                } else {
                    1
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sketch2d::{Line2dId, Point2dId};
    use std::f64::consts::PI;

    #[test]
    fn test_linear_pattern_creation() {
        let ops = PatternOperations::new(Tolerance2d::default());
        let entities = vec![EntityRef::Point(Point2dId::new())];
        let direction = Vector2d::new(1.0, 0.0);

        let result = ops
            .create_linear_pattern(entities, direction, 5.0, 3)
            .unwrap();
        assert_eq!(result.pattern_entities.len(), 2); // 3 total - 1 original
        assert_eq!(result.total_created, 2);
    }

    #[test]
    fn test_circular_pattern_creation() {
        let ops = PatternOperations::new(Tolerance2d::default());
        let entities = vec![EntityRef::Line(Line2dId::new())];
        let center = Point2d::new(0.0, 0.0);

        let result = ops
            .create_circular_pattern(entities, center, PI / 4.0, 8, true)
            .unwrap();
        assert_eq!(result.pattern_entities.len(), 7); // 8 total - 1 original
    }

    #[test]
    fn test_rectangular_pattern_creation() {
        let ops = PatternOperations::new(Tolerance2d::default());
        let entities = vec![EntityRef::Point(Point2dId::new())];
        let dir_u = Vector2d::new(1.0, 0.0);
        let dir_v = Vector2d::new(0.0, 1.0);

        let result = ops
            .create_rectangular_pattern(entities, dir_u, dir_v, 2.0, 3.0, 3, 2)
            .unwrap();
        assert_eq!(result.pattern_entities.len(), 5); // 3*2 = 6 total - 1 original
    }

    #[test]
    fn test_pattern_validation() {
        let ops = PatternOperations::new(Tolerance2d::default());

        // Test invalid parameters
        let invalid_params = PatternParams {
            pattern_type: PatternType::Linear {
                direction: Vector2d::new(1.0, 0.0),
                spacing: -1.0, // Invalid negative spacing
                count: 3,
            },
            source_entities: vec![EntityRef::Point(Point2dId::new())],
            tolerance: Tolerance2d::default(),
            maintain_constraints: true,
        };

        assert!(ops.validate_pattern_params(&invalid_params).is_err());
    }
}
