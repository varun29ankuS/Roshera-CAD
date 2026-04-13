//! Create primitive operation implementation

use super::common::brep_to_entity_state;
use crate::{
    execution::{ExecutionContext, OperationImpl, ResourceEstimate},
    CreatedEntity, EntityId, EntityType, Operation, OperationInputs, OperationOutputs,
    PrimitiveType, TimelineError, TimelineResult,
};
use async_trait::async_trait;
use geometry_engine::{
    math::{Matrix4, Point3, Vector3},
    primitives::{
        box_primitive::BoxPrimitive, cone_primitive::ConePrimitive,
        cylinder_primitive::CylinderPrimitive, sphere_primitive::SpherePrimitive,
        topology_builder::BRepModel, torus_primitive::TorusPrimitive,
    },
};

/// Implementation of create primitive operation
pub struct CreatePrimitiveOp;

#[async_trait]
impl OperationImpl for CreatePrimitiveOp {
    fn operation_type(&self) -> &'static str {
        "create_primitive"
    }

    async fn validate(
        &self,
        operation: &Operation,
        _context: &ExecutionContext,
    ) -> TimelineResult<()> {
        if let Operation::CreatePrimitive {
            primitive_type,
            parameters,
        } = operation
        {
            // Validate parameters based on primitive type
            match primitive_type {
                PrimitiveType::Box => validate_box_params(parameters),
                PrimitiveType::Sphere => validate_sphere_params(parameters),
                PrimitiveType::Cylinder => validate_cylinder_params(parameters),
                PrimitiveType::Cone => validate_cone_params(parameters),
                PrimitiveType::Torus => validate_torus_params(parameters),
            }
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected CreatePrimitive operation".to_string(),
            ))
        }
    }

    async fn execute(
        &self,
        operation: &Operation,
        context: &mut ExecutionContext,
    ) -> TimelineResult<OperationOutputs> {
        if let Operation::CreatePrimitive {
            primitive_type,
            parameters,
        } = operation
        {
            // Create the primitive based on type
            let (brep, name) = match primitive_type {
                PrimitiveType::Box => create_box(parameters)?,
                PrimitiveType::Sphere => create_sphere(parameters)?,
                PrimitiveType::Cylinder => create_cylinder(parameters)?,
                PrimitiveType::Cone => create_cone(parameters)?,
                PrimitiveType::Torus => create_torus(parameters)?,
            };

            // Create entity
            let entity_id = EntityId::new();
            let entity_state =
                brep_to_entity_state(&brep, entity_id, EntityType::Solid, Some(name.clone()))?;

            // Add primitive-specific properties
            let mut final_entity = entity_state;
            if let Some(obj) = final_entity.properties.as_object_mut() {
                obj.insert(
                    "primitive_type".to_string(),
                    serde_json::json!(primitive_type),
                );
                obj.insert("parameters".to_string(), parameters.clone());
            }

            // Add to context
            context.add_temp_entity(final_entity)?;
            context.increment_geometry_ops();

            // Create output
            let outputs = OperationOutputs {
                created: vec![CreatedEntity {
                    id: entity_id,
                    entity_type: EntityType::Solid,
                    name: Some(name),
                }],
                modified: vec![],
                deleted: vec![],
                side_effects: vec![],
            };

            Ok(outputs)
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected CreatePrimitive operation".to_string(),
            ))
        }
    }

    fn estimate_resources(&self, operation: &Operation) -> ResourceEstimate {
        if let Operation::CreatePrimitive { primitive_type, .. } = operation {
            let (vertices, faces) = match primitive_type {
                PrimitiveType::Box => (8, 6),
                PrimitiveType::Sphere => (50, 48), // Approximation
                PrimitiveType::Cylinder => (50, 50), // Approximation
                PrimitiveType::Cone => (30, 30),   // Approximation
                PrimitiveType::Torus => (100, 100), // Approximation
            };

            ResourceEstimate {
                memory_bytes: (vertices * 64 + faces * 256) as u64,
                time_ms: match primitive_type {
                    PrimitiveType::Box => 10,
                    PrimitiveType::Sphere => 50,
                    PrimitiveType::Cylinder => 30,
                    PrimitiveType::Cone => 30,
                    PrimitiveType::Torus => 100,
                },
                entities_created: 1,
                entities_modified: 0,
            }
        } else {
            ResourceEstimate::default()
        }
    }
}

/// Validate box parameters
fn validate_box_params(params: &serde_json::Value) -> TimelineResult<()> {
    let width = params
        .get("width")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| {
            TimelineError::ValidationError("Box requires 'width' parameter".to_string())
        })?;

    let height = params
        .get("height")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| {
            TimelineError::ValidationError("Box requires 'height' parameter".to_string())
        })?;

    let depth = params
        .get("depth")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| {
            TimelineError::ValidationError("Box requires 'depth' parameter".to_string())
        })?;

    if width <= 0.0 {
        return Err(TimelineError::ValidationError(
            "Box width must be positive".to_string(),
        ));
    }
    if height <= 0.0 {
        return Err(TimelineError::ValidationError(
            "Box height must be positive".to_string(),
        ));
    }
    if depth <= 0.0 {
        return Err(TimelineError::ValidationError(
            "Box depth must be positive".to_string(),
        ));
    }

    Ok(())
}

/// Validate sphere parameters
fn validate_sphere_params(params: &serde_json::Value) -> TimelineResult<()> {
    let radius = params
        .get("radius")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| {
            TimelineError::ValidationError("Sphere requires 'radius' parameter".to_string())
        })?;

    if radius <= 0.0 {
        return Err(TimelineError::ValidationError(
            "Sphere radius must be positive".to_string(),
        ));
    }

    Ok(())
}

/// Validate cylinder parameters
fn validate_cylinder_params(params: &serde_json::Value) -> TimelineResult<()> {
    let radius = params
        .get("radius")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| {
            TimelineError::ValidationError("Cylinder requires 'radius' parameter".to_string())
        })?;

    let height = params
        .get("height")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| {
            TimelineError::ValidationError("Cylinder requires 'height' parameter".to_string())
        })?;

    if radius <= 0.0 {
        return Err(TimelineError::ValidationError(
            "Cylinder radius must be positive".to_string(),
        ));
    }
    if height <= 0.0 {
        return Err(TimelineError::ValidationError(
            "Cylinder height must be positive".to_string(),
        ));
    }

    Ok(())
}

/// Validate cone parameters
fn validate_cone_params(params: &serde_json::Value) -> TimelineResult<()> {
    let radius1 = params
        .get("radius1")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| {
            TimelineError::ValidationError("Cone requires 'radius1' parameter".to_string())
        })?;

    let radius2 = params
        .get("radius2")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0); // Default to 0 for a proper cone

    let height = params
        .get("height")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| {
            TimelineError::ValidationError("Cone requires 'height' parameter".to_string())
        })?;

    if radius1 < 0.0 {
        return Err(TimelineError::ValidationError(
            "Cone radius1 must be non-negative".to_string(),
        ));
    }
    if radius2 < 0.0 {
        return Err(TimelineError::ValidationError(
            "Cone radius2 must be non-negative".to_string(),
        ));
    }
    if radius1 == 0.0 && radius2 == 0.0 {
        return Err(TimelineError::ValidationError(
            "At least one cone radius must be positive".to_string(),
        ));
    }
    if height <= 0.0 {
        return Err(TimelineError::ValidationError(
            "Cone height must be positive".to_string(),
        ));
    }

    Ok(())
}

/// Validate torus parameters
fn validate_torus_params(params: &serde_json::Value) -> TimelineResult<()> {
    let major_radius = params
        .get("major_radius")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| {
            TimelineError::ValidationError("Torus requires 'major_radius' parameter".to_string())
        })?;

    let minor_radius = params
        .get("minor_radius")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| {
            TimelineError::ValidationError("Torus requires 'minor_radius' parameter".to_string())
        })?;

    if major_radius <= 0.0 {
        return Err(TimelineError::ValidationError(
            "Torus major radius must be positive".to_string(),
        ));
    }
    if minor_radius <= 0.0 {
        return Err(TimelineError::ValidationError(
            "Torus minor radius must be positive".to_string(),
        ));
    }
    if minor_radius >= major_radius {
        return Err(TimelineError::ValidationError(
            "Torus minor radius must be less than major radius".to_string(),
        ));
    }

    Ok(())
}

/// Create a box primitive
fn create_box(params: &serde_json::Value) -> TimelineResult<(BRepModel, String)> {
    let width = params.get("width").and_then(|v| v.as_f64()).unwrap();
    let height = params.get("height").and_then(|v| v.as_f64()).unwrap();
    let depth = params.get("depth").and_then(|v| v.as_f64()).unwrap();

    // Get optional center position
    let center = if let Some(center_val) = params.get("center") {
        if let Some(arr) = center_val.as_array() {
            if arr.len() == 3 {
                Point3::new(
                    arr[0].as_f64().unwrap_or(0.0),
                    arr[1].as_f64().unwrap_or(0.0),
                    arr[2].as_f64().unwrap_or(0.0),
                )
            } else {
                Point3::new(0.0, 0.0, 0.0)
            }
        } else {
            Point3::new(0.0, 0.0, 0.0)
        }
    } else {
        Point3::new(0.0, 0.0, 0.0)
    };

    // Create a new BRep model
    let mut brep = BRepModel::new();

    // Create box parameters
    let params = geometry_engine::primitives::box_primitive::BoxParameters {
        width,
        height,
        depth,
        corner_radius: None,
        transform: Some(Matrix4::from_translation(&Vector3::new(
            center.x, center.y, center.z,
        ))),
        tolerance: None,
    };

    // Use BoxPrimitive to create the box
    use geometry_engine::primitives::{box_primitive::BoxPrimitive, primitive_traits::Primitive};
    let _solid_id = BoxPrimitive::create(params, &mut brep)
        .map_err(|e| TimelineError::ExecutionError(format!("Failed to create box: {:?}", e)))?;

    Ok((brep, format!("Box {}x{}x{}", width, height, depth)))
}

/// Create a sphere primitive
fn create_sphere(params: &serde_json::Value) -> TimelineResult<(BRepModel, String)> {
    let radius = params.get("radius").and_then(|v| v.as_f64()).unwrap();

    // Get optional center position
    let center = if let Some(center_val) = params.get("center") {
        if let Some(arr) = center_val.as_array() {
            if arr.len() == 3 {
                Point3::new(
                    arr[0].as_f64().unwrap_or(0.0),
                    arr[1].as_f64().unwrap_or(0.0),
                    arr[2].as_f64().unwrap_or(0.0),
                )
            } else {
                Point3::new(0.0, 0.0, 0.0)
            }
        } else {
            Point3::new(0.0, 0.0, 0.0)
        }
    } else {
        Point3::new(0.0, 0.0, 0.0)
    };

    // Create a new BRep model
    let mut brep = BRepModel::new();

    // Create sphere parameters
    let params = geometry_engine::primitives::sphere_primitive::SphereParameters {
        radius,
        center,
        u_segments: 16,
        v_segments: 16,
        transform: None,
        tolerance: None,
    };

    // Use SpherePrimitive to create the sphere
    use geometry_engine::primitives::{
        primitive_traits::Primitive, sphere_primitive::SpherePrimitive,
    };
    let _solid_id = SpherePrimitive::create(params, &mut brep)
        .map_err(|e| TimelineError::ExecutionError(format!("Failed to create sphere: {:?}", e)))?;

    Ok((brep, format!("Sphere R{}", radius)))
}

/// Create a cylinder primitive
fn create_cylinder(params: &serde_json::Value) -> TimelineResult<(BRepModel, String)> {
    let radius = params.get("radius").and_then(|v| v.as_f64()).unwrap();
    let height = params.get("height").and_then(|v| v.as_f64()).unwrap();

    // Get optional base position
    let base = if let Some(base_val) = params.get("base") {
        if let Some(arr) = base_val.as_array() {
            if arr.len() == 3 {
                Point3::new(
                    arr[0].as_f64().unwrap_or(0.0),
                    arr[1].as_f64().unwrap_or(0.0),
                    arr[2].as_f64().unwrap_or(0.0),
                )
            } else {
                Point3::new(0.0, 0.0, 0.0)
            }
        } else {
            Point3::new(0.0, 0.0, 0.0)
        }
    } else {
        Point3::new(0.0, 0.0, 0.0)
    };

    // Get optional axis direction
    let axis = if let Some(axis_val) = params.get("axis") {
        if let Some(arr) = axis_val.as_array() {
            if arr.len() == 3 {
                let v = Vector3::new(
                    arr[0].as_f64().unwrap_or(0.0),
                    arr[1].as_f64().unwrap_or(0.0),
                    arr[2].as_f64().unwrap_or(1.0),
                );
                v.normalize().unwrap_or(Vector3::Z)
            } else {
                Vector3::Z
            }
        } else {
            Vector3::Z
        }
    } else {
        Vector3::Z
    };

    // Create a new BRep model
    let mut brep = BRepModel::new();

    // Create cylinder parameters
    let params = geometry_engine::primitives::cylinder_primitive::CylinderParameters {
        radius,
        height,
        base_center: base,
        axis,
        segments: 16,
        transform: None,
        tolerance: None,
    };

    // Use CylinderPrimitive to create the cylinder
    use geometry_engine::primitives::{
        cylinder_primitive::CylinderPrimitive, primitive_traits::Primitive,
    };
    let _solid_id = CylinderPrimitive::create(params, &mut brep).map_err(|e| {
        TimelineError::ExecutionError(format!("Failed to create cylinder: {:?}", e))
    })?;

    Ok((brep, format!("Cylinder R{} H{}", radius, height)))
}

/// Create a cone primitive
fn create_cone(params: &serde_json::Value) -> TimelineResult<(BRepModel, String)> {
    let radius1 = params.get("radius1").and_then(|v| v.as_f64()).unwrap();
    let radius2 = params
        .get("radius2")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let height = params.get("height").and_then(|v| v.as_f64()).unwrap();

    // Get optional base position
    let base = if let Some(base_val) = params.get("base") {
        if let Some(arr) = base_val.as_array() {
            if arr.len() == 3 {
                Point3::new(
                    arr[0].as_f64().unwrap_or(0.0),
                    arr[1].as_f64().unwrap_or(0.0),
                    arr[2].as_f64().unwrap_or(0.0),
                )
            } else {
                Point3::new(0.0, 0.0, 0.0)
            }
        } else {
            Point3::new(0.0, 0.0, 0.0)
        }
    } else {
        Point3::new(0.0, 0.0, 0.0)
    };

    // Get optional axis direction
    let axis = if let Some(axis_val) = params.get("axis") {
        if let Some(arr) = axis_val.as_array() {
            if arr.len() == 3 {
                let v = Vector3::new(
                    arr[0].as_f64().unwrap_or(0.0),
                    arr[1].as_f64().unwrap_or(0.0),
                    arr[2].as_f64().unwrap_or(1.0),
                );
                v.normalize().unwrap_or(Vector3::Z)
            } else {
                Vector3::Z
            }
        } else {
            Vector3::Z
        }
    } else {
        Vector3::Z
    };

    // Create a new BRep model
    let mut brep = BRepModel::new();

    // Calculate half angle from radii
    let half_angle = if radius2 > 0.0 {
        ((radius1 - radius2) / height).atan()
    } else {
        (radius1 / height).atan()
    };

    // Create cone parameters
    let params = geometry_engine::primitives::cone_primitive::ConeParameters {
        apex: base,
        axis,
        half_angle,
        height,
        bottom_radius: if radius2 > 0.0 { Some(radius2) } else { None },
        angle_range: None,
    };

    // Use ConePrimitive to create the cone
    use geometry_engine::primitives::cone_primitive::ConePrimitive;
    let _solid_id = ConePrimitive::create(&params, &mut brep)
        .map_err(|e| TimelineError::ExecutionError(format!("Failed to create cone: {:?}", e)))?;

    let name = if radius2 == 0.0 {
        format!("Cone R{} H{}", radius1, height)
    } else {
        format!("Frustum R{}-R{} H{}", radius1, radius2, height)
    };

    Ok((brep, name))
}

/// Create a torus primitive
fn create_torus(params: &serde_json::Value) -> TimelineResult<(BRepModel, String)> {
    let major_radius = params.get("major_radius").and_then(|v| v.as_f64()).unwrap();
    let minor_radius = params.get("minor_radius").and_then(|v| v.as_f64()).unwrap();

    // Get optional center position
    let center = if let Some(center_val) = params.get("center") {
        if let Some(arr) = center_val.as_array() {
            if arr.len() == 3 {
                Point3::new(
                    arr[0].as_f64().unwrap_or(0.0),
                    arr[1].as_f64().unwrap_or(0.0),
                    arr[2].as_f64().unwrap_or(0.0),
                )
            } else {
                Point3::new(0.0, 0.0, 0.0)
            }
        } else {
            Point3::new(0.0, 0.0, 0.0)
        }
    } else {
        Point3::new(0.0, 0.0, 0.0)
    };

    // Get optional axis direction
    let axis = if let Some(axis_val) = params.get("axis") {
        if let Some(arr) = axis_val.as_array() {
            if arr.len() == 3 {
                let v = Vector3::new(
                    arr[0].as_f64().unwrap_or(0.0),
                    arr[1].as_f64().unwrap_or(0.0),
                    arr[2].as_f64().unwrap_or(1.0),
                );
                v.normalize().unwrap_or(Vector3::Z)
            } else {
                Vector3::Z
            }
        } else {
            Vector3::Z
        }
    } else {
        Vector3::Z
    };

    // Create a new BRep model
    let mut brep = BRepModel::new();

    // Create torus parameters
    let params = geometry_engine::primitives::torus_primitive::TorusParameters {
        center,
        axis,
        major_radius,
        minor_radius,
        major_angle_range: None,
        minor_angle_range: None,
    };

    // Use TorusPrimitive to create the torus
    use geometry_engine::primitives::torus_primitive::TorusPrimitive;
    let _solid_id = TorusPrimitive::create(&params, &mut brep)
        .map_err(|e| TimelineError::ExecutionError(format!("Failed to create torus: {:?}", e)))?;

    Ok((brep, format!("Torus R{}/r{}", major_radius, minor_radius)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::EntityStateStore;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_box_validation() {
        let op = CreatePrimitiveOp;
        let store = Arc::new(EntityStateStore::new());
        let context = ExecutionContext::new(crate::BranchId::main(), store);

        // Valid box
        let operation = Operation::CreatePrimitive {
            primitive_type: PrimitiveType::Box,
            parameters: serde_json::json!({
                "width": 10.0,
                "height": 20.0,
                "depth": 30.0
            }),
        };
        assert!(op.validate(&operation, &context).await.is_ok());

        // Invalid box - missing parameter
        let operation = Operation::CreatePrimitive {
            primitive_type: PrimitiveType::Box,
            parameters: serde_json::json!({
                "width": 10.0,
                "height": 20.0
            }),
        };
        assert!(op.validate(&operation, &context).await.is_err());

        // Invalid box - negative dimension
        let operation = Operation::CreatePrimitive {
            primitive_type: PrimitiveType::Box,
            parameters: serde_json::json!({
                "width": -10.0,
                "height": 20.0,
                "depth": 30.0
            }),
        };
        assert!(op.validate(&operation, &context).await.is_err());
    }

    #[tokio::test]
    async fn test_create_primitive_execution() {
        let op = CreatePrimitiveOp;
        let store = Arc::new(EntityStateStore::new());
        let mut context = ExecutionContext::new(crate::BranchId::main(), store);

        // Create a sphere
        let operation = Operation::CreatePrimitive {
            primitive_type: PrimitiveType::Sphere,
            parameters: serde_json::json!({
                "radius": 5.0,
                "center": [10.0, 20.0, 30.0]
            }),
        };

        let outputs = op.execute(&operation, &mut context).await.unwrap();
        assert_eq!(outputs.created.len(), 1);
        assert_eq!(outputs.created[0].entity_type, EntityType::Solid);
        assert!(outputs.created[0].name.is_some());

        // Verify entity was added to context
        assert!(context.entity_exists(outputs.created[0].id));
    }
}
