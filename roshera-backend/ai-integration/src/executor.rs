use dashmap::DashMap;
use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::transform::{transform_solid, TransformOptions};
/// Command executor that bridges AI commands to geometry engine
///
/// # Design Rationale
/// - **Why separate executor**: Decouples AI parsing from geometry operations
/// - **Why async**: Geometry operations may be compute-intensive
/// - **Performance**: < 10ms for primitive creation
/// - **Business Value**: Clean separation allows geometry engine evolution
use geometry_engine::primitives::{
    box_primitive::{BoxParameters, BoxPrimitive},
    cone_primitive::{ConeParameters, ConePrimitive},
    cylinder_primitive::{CylinderParameters, CylinderPrimitive},
    primitive_traits::Primitive,
    solid::SolidId,
    sphere_primitive::{SphereParameters, SpherePrimitive},
    topology_builder::BRepModel,
};
use shared_types::geometry::GeometryId;
use shared_types::geometry_commands::Command;
use std::sync::Arc;
use uuid::Uuid;

/// Executes geometry commands from AI system
pub struct CommandExecutor {
    /// The B-Rep model containing all geometry
    model: Arc<std::sync::RwLock<BRepModel>>, // Use sync RwLock
    /// Map from our GeometryId to engine's SolidId
    id_map: Arc<DashMap<GeometryId, SolidId>>,
    /// Reverse map for queries
    solid_to_geometry: Arc<DashMap<SolidId, GeometryId>>,
}

impl CommandExecutor {
    /// Create new command executor
    pub fn new() -> Self {
        Self {
            model: Arc::new(std::sync::RwLock::new(BRepModel::new())),
            id_map: Arc::new(DashMap::new()),
            solid_to_geometry: Arc::new(DashMap::new()),
        }
    }

    /// Execute a geometry command
    ///
    /// # Performance
    /// - Primitive creation: < 10ms
    /// - Boolean operations: < 150ms (target)
    pub async fn execute(&mut self, command: Command) -> Result<GeometryId, ExecutorError> {
        match command {
            Command::CreateBox {
                width,
                height,
                depth,
            } => self.create_box(width, height, depth).await,
            Command::CreateSphere { radius } => self.create_sphere(radius).await,
            Command::CreateCylinder { radius, height } => {
                self.create_cylinder(radius, height).await
            }
            Command::CreateCone { radius, height } => self.create_cone(radius, height).await,
            Command::BooleanUnion { object_a, object_b } => {
                self.boolean_union(object_a, object_b).await
            }
            Command::BooleanIntersection { object_a, object_b } => {
                self.boolean_intersection(object_a, object_b).await
            }
            Command::BooleanDifference { object_a, object_b } => {
                self.boolean_difference(object_a, object_b).await
            }
            Command::Transform { object, transform } => self.transform(object, transform).await,
            _ => Err(ExecutorError::NotImplemented(format!("{:?}", command))),
        }
    }

    /// Create a box primitive
    async fn create_box(
        &mut self,
        width: f64,
        height: f64,
        depth: f64,
    ) -> Result<GeometryId, ExecutorError> {
        // Validate inputs
        if width <= 0.0 || height <= 0.0 || depth <= 0.0 {
            return Err(ExecutorError::InvalidParameters(
                "Box dimensions must be positive".to_string(),
            ));
        }

        // Move CPU-intensive geometry work to background thread.
        // The lock is acquired *inside* spawn_blocking so the async runtime
        // thread is never blocked on it; sync RwLock is intentional here.
        let model_clone = Arc::clone(&self.model);
        let solid_id = tokio::task::spawn_blocking(move || {
            let params = BoxParameters {
                width,
                height,
                depth,
                corner_radius: None,
                transform: None,
                tolerance: None,
            };

            // Build the box into the shared B-Rep model so subsequent
            // commands (boolean, transform) can resolve its SolidId. The
            // earlier code constructed a throwaway BRepModel here, which
            // meant boxes never entered executor state — every follow-up
            // command on a box id failed with InvalidParameters.
            let mut model = model_clone
                .write()
                .expect("BRep model RwLock poisoned; prior holder panicked");
            let solid_id = BoxPrimitive::create(params, &mut model)
                .map_err(|e| ExecutorError::GeometryError(format!("{:?}", e)))?;
            Ok::<SolidId, ExecutorError>(solid_id)
        })
        .await
        .map_err(|e| ExecutorError::GeometryError(format!("Task join error: {}", e)))??;

        // Generate our ID and map it
        let geometry_id = self.generate_geometry_id();
        self.id_map.insert(geometry_id.clone(), solid_id);
        self.solid_to_geometry.insert(solid_id, geometry_id.clone());

        tracing::info!("Created box: {:?} -> {:?}", geometry_id, solid_id);

        Ok(geometry_id)
    }

    /// Create a sphere primitive
    async fn create_sphere(&mut self, radius: f64) -> Result<GeometryId, ExecutorError> {
        if radius <= 0.0 {
            return Err(ExecutorError::InvalidParameters(
                "Sphere radius must be positive".to_string(),
            ));
        }

        // Move CPU-intensive geometry work to background thread
        let model_clone = Arc::clone(&self.model);
        let solid_id = tokio::task::spawn_blocking(move || {
            let params = SphereParameters {
                radius,
                center: Point3::new(0.0, 0.0, 0.0),
                u_segments: 16,
                v_segments: 8,
                transform: None,
                tolerance: None,
            };

            // Create sphere using the primitive system in the shared model
            let mut model = model_clone
                .write()
                .expect("BRep model RwLock poisoned; prior holder panicked");
            let solid_id = SpherePrimitive::create(params, &mut model)
                .map_err(|e| ExecutorError::GeometryError(format!("{:?}", e)))?;
            Ok::<SolidId, ExecutorError>(solid_id)
        })
        .await
        .map_err(|e| ExecutorError::GeometryError(format!("Task join error: {}", e)))??;

        let geometry_id = self.generate_geometry_id();
        self.id_map.insert(geometry_id.clone(), solid_id);
        self.solid_to_geometry.insert(solid_id, geometry_id.clone());

        Ok(geometry_id)
    }

    /// Create a cylinder primitive
    async fn create_cylinder(
        &mut self,
        radius: f64,
        height: f64,
    ) -> Result<GeometryId, ExecutorError> {
        if radius <= 0.0 || height <= 0.0 {
            return Err(ExecutorError::InvalidParameters(
                "Cylinder dimensions must be positive".to_string(),
            ));
        }

        // Move CPU-intensive geometry work to background thread
        let model_clone = Arc::clone(&self.model);
        let solid_id = tokio::task::spawn_blocking(move || {
            let params = CylinderParameters {
                radius,
                height,
                base_center: Point3::new(0.0, 0.0, 0.0),
                axis: Vector3::new(0.0, 0.0, 1.0),
                segments: 16,
                transform: None,
                tolerance: None,
            };

            // Create cylinder using the primitive system in the shared model
            let mut model = model_clone
                .write()
                .expect("BRep model RwLock poisoned; prior holder panicked");
            let solid_id = CylinderPrimitive::create(params, &mut model)
                .map_err(|e| ExecutorError::GeometryError(format!("{:?}", e)))?;
            Ok::<SolidId, ExecutorError>(solid_id)
        })
        .await
        .map_err(|e| ExecutorError::GeometryError(format!("Task join error: {}", e)))??;

        let geometry_id = self.generate_geometry_id();
        self.id_map.insert(geometry_id.clone(), solid_id);
        self.solid_to_geometry.insert(solid_id, geometry_id.clone());

        Ok(geometry_id)
    }

    /// Create a cone primitive
    async fn create_cone(&mut self, radius: f64, height: f64) -> Result<GeometryId, ExecutorError> {
        if radius <= 0.0 || height <= 0.0 {
            return Err(ExecutorError::InvalidParameters(
                "Cone dimensions must be positive".to_string(),
            ));
        }

        // Move CPU-intensive geometry work to background thread
        let model_clone = Arc::clone(&self.model);
        let solid_id = tokio::task::spawn_blocking(move || {
            let params = ConeParameters {
                apex: Point3::new(0.0, 0.0, height),  // Apex at top
                axis: Vector3::new(0.0, 0.0, -1.0),   // Pointing down
                half_angle: (radius / height).atan(), // Calculate from radius and height
                height,
                bottom_radius: Some(radius),
                angle_range: None, // Full cone
            };

            // Create cone using the primitive system in the shared model
            let mut model = model_clone
                .write()
                .expect("BRep model RwLock poisoned; prior holder panicked");
            let solid_id = ConePrimitive::create(&params, &mut model)
                .map_err(|e| ExecutorError::GeometryError(format!("{:?}", e)))?;
            Ok::<SolidId, ExecutorError>(solid_id)
        })
        .await
        .map_err(|e| ExecutorError::GeometryError(format!("Task join error: {}", e)))??;

        let geometry_id = self.generate_geometry_id();
        self.id_map.insert(geometry_id.clone(), solid_id);
        self.solid_to_geometry.insert(solid_id, geometry_id.clone());

        Ok(geometry_id)
    }

    /// Execute a boolean operation between two solids
    ///
    /// # Design Rationale
    /// - **Why spawn_blocking**: Boolean operations are CPU-intensive (face-face
    ///   intersection, face classification, topology reconstruction)
    /// - **Why Arc clones**: The model lock must be held inside the blocking task
    ///   to satisfy Send bounds on the spawned future
    /// - **Performance**: Target < 150ms for 1k-face solids
    async fn execute_boolean(
        &mut self,
        object_a: GeometryId,
        object_b: GeometryId,
        op: BooleanOp,
    ) -> Result<GeometryId, ExecutorError> {
        let solid_a = self.get_solid_id(&object_a)?;
        let solid_b = self.get_solid_id(&object_b)?;

        let op_name = format!("{:?}", op);
        let model_clone = Arc::clone(&self.model);
        let result_solid_id = tokio::task::spawn_blocking(move || {
            let mut model = model_clone
                .write()
                .expect("BRep model RwLock poisoned; prior holder panicked");
            boolean_operation(&mut model, solid_a, solid_b, op, BooleanOptions::default())
                .map_err(|e| ExecutorError::GeometryError(format!("{:?}", e)))
        })
        .await
        .map_err(|e| ExecutorError::GeometryError(format!("Task join error: {}", e)))??;

        let geometry_id = self.generate_geometry_id();
        self.id_map.insert(geometry_id.clone(), result_solid_id);
        self.solid_to_geometry
            .insert(result_solid_id, geometry_id.clone());

        tracing::info!(
            "Boolean {:?}: {:?} x {:?} -> {:?}",
            op_name,
            object_a,
            object_b,
            geometry_id
        );

        Ok(geometry_id)
    }

    /// Boolean union operation (A ∪ B)
    async fn boolean_union(
        &mut self,
        object_a: GeometryId,
        object_b: GeometryId,
    ) -> Result<GeometryId, ExecutorError> {
        self.execute_boolean(object_a, object_b, BooleanOp::Union)
            .await
    }

    /// Boolean intersection operation (A ∩ B)
    async fn boolean_intersection(
        &mut self,
        object_a: GeometryId,
        object_b: GeometryId,
    ) -> Result<GeometryId, ExecutorError> {
        self.execute_boolean(object_a, object_b, BooleanOp::Intersection)
            .await
    }

    /// Boolean difference operation (A - B)
    async fn boolean_difference(
        &mut self,
        object_a: GeometryId,
        object_b: GeometryId,
    ) -> Result<GeometryId, ExecutorError> {
        self.execute_boolean(object_a, object_b, BooleanOp::Difference)
            .await
    }

    /// Transform a solid (translate, rotate, scale, or mirror)
    ///
    /// # Design Rationale
    /// - **Why in-place**: Transforms modify vertex positions directly; no new
    ///   topology is created, so we return the same GeometryId
    /// - **Why spawn_blocking**: Matrix construction may fail (e.g., zero-length
    ///   axis for rotation) and vertex iteration is O(n)
    /// - **Performance**: O(V) where V is vertex count; < 5ms for typical solids
    async fn transform(
        &mut self,
        object: GeometryId,
        xform: shared_types::geometry_commands::Transform,
    ) -> Result<GeometryId, ExecutorError> {
        let solid_id = self.get_solid_id(&object)?;

        let model_clone = Arc::clone(&self.model);
        tokio::task::spawn_blocking(move || {
            let matrix = match xform {
                shared_types::geometry_commands::Transform::Translate { offset } => Ok(
                    Matrix4::translation(offset[0] as f64, offset[1] as f64, offset[2] as f64),
                ),
                shared_types::geometry_commands::Transform::Rotate {
                    axis,
                    angle_radians,
                } => {
                    let axis_vec = Vector3::new(axis[0] as f64, axis[1] as f64, axis[2] as f64);
                    Matrix4::from_axis_angle(&axis_vec, angle_radians)
                        .map_err(|e| ExecutorError::GeometryError(format!("{:?}", e)))
                }
                shared_types::geometry_commands::Transform::Scale { factors } => {
                    Ok(Matrix4::from_scale(&Vector3::new(
                        factors[0] as f64,
                        factors[1] as f64,
                        factors[2] as f64,
                    )))
                }
                shared_types::geometry_commands::Transform::Mirror {
                    plane_normal,
                    plane_point,
                } => {
                    let normal = Vector3::new(
                        plane_normal[0] as f64,
                        plane_normal[1] as f64,
                        plane_normal[2] as f64,
                    );
                    let point = Point3::new(
                        plane_point[0] as f64,
                        plane_point[1] as f64,
                        plane_point[2] as f64,
                    );
                    Matrix4::mirror(point, normal)
                        .map_err(|e| ExecutorError::GeometryError(format!("{:?}", e)))
                }
            }?;

            let mut model = model_clone
                .write()
                .expect("BRep model RwLock poisoned; prior holder panicked");
            transform_solid(&mut model, solid_id, matrix, TransformOptions::default())
                .map_err(|e| ExecutorError::GeometryError(format!("{:?}", e)))?;

            Ok::<(), ExecutorError>(())
        })
        .await
        .map_err(|e| ExecutorError::GeometryError(format!("Task join error: {}", e)))??;

        tracing::info!("Transformed solid: {:?}", object);

        Ok(object)
    }

    /// Get solid ID from geometry ID
    fn get_solid_id(&self, geometry_id: &GeometryId) -> Result<SolidId, ExecutorError> {
        self.id_map
            .get(geometry_id)
            .map(|entry| *entry)
            .ok_or_else(|| ExecutorError::ObjectNotFound(geometry_id.clone()))
    }

    /// Generate unique geometry ID
    fn generate_geometry_id(&self) -> GeometryId {
        GeometryId(Uuid::new_v4())
    }

    /// Get all created objects
    pub fn get_all_objects(&self) -> Vec<GeometryId> {
        self.id_map
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Clear all objects
    pub async fn clear(&mut self) {
        self.id_map.clear();
        self.solid_to_geometry.clear();
        let mut model = self
            .model
            .write()
            .expect("BRep model RwLock poisoned; prior holder panicked");
        *model = BRepModel::new();
    }
}

/// Executor error types
#[derive(Debug, thiserror::Error)]
pub enum ExecutorError {
    #[error("Invalid parameters: {0}")]
    InvalidParameters(String),

    #[error("Geometry error: {0}")]
    GeometryError(String),

    #[error("Object not found: {0:?}")]
    ObjectNotFound(GeometryId),

    #[error("Operation not implemented: {0}")]
    NotImplemented(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_box() {
        let mut executor = CommandExecutor::new();
        let result = executor
            .execute(Command::CreateBox {
                width: 1.0,
                height: 2.0,
                depth: 3.0,
            })
            .await;

        assert!(result.is_ok());
        let id = result.unwrap();
        assert!(executor.get_all_objects().contains(&id));
    }

    #[tokio::test]
    async fn test_invalid_dimensions() {
        let mut executor = CommandExecutor::new();
        let result = executor
            .execute(Command::CreateSphere { radius: -1.0 })
            .await;

        assert!(matches!(result, Err(ExecutorError::InvalidParameters(_))));
    }

    #[tokio::test]
    async fn test_clear() {
        let mut executor = CommandExecutor::new();
        let _ = executor
            .execute(Command::CreateBox {
                width: 1.0,
                height: 1.0,
                depth: 1.0,
            })
            .await
            .unwrap();

        assert_eq!(executor.get_all_objects().len(), 1);
        executor.clear().await;
        assert_eq!(executor.get_all_objects().len(), 0);
    }
}
