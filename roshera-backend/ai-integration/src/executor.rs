use dashmap::DashMap;
use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::offset::{
    offset_solid, IntersectionHandling, OffsetOptions, OffsetType,
};
use geometry_engine::operations::transform::{mirror, transform_solid, TransformOptions};
use geometry_engine::primitives::face::FaceId;
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
    /// The B-Rep model containing all geometry.
    ///
    /// `tokio::sync::RwLock` rather than `std::sync::RwLock` so this lock
    /// can be shared with the rest of the server (notably `AppState.model`)
    /// without lock-flavor mismatches. Inside `spawn_blocking` we use
    /// `blocking_write()` — explicitly designed for that pattern — to keep
    /// CPU-heavy geometry work off the async runtime threads.
    model: Arc<tokio::sync::RwLock<BRepModel>>,
    /// Map from our GeometryId to engine's SolidId
    id_map: Arc<DashMap<GeometryId, SolidId>>,
    /// Reverse map for queries
    solid_to_geometry: Arc<DashMap<SolidId, GeometryId>>,
}

impl CommandExecutor {
    /// Create a new command executor with an isolated, freshly-initialised
    /// B-Rep model.
    ///
    /// Useful for tests and standalone benchmarks that don't need to share
    /// state with the rest of the server. Production code should use
    /// [`CommandExecutor::with_model`] to bind to the server-wide model so
    /// AI-issued commands operate on the same kernel that REST/WS handlers
    /// observe.
    pub fn new() -> Self {
        Self::with_model(Arc::new(tokio::sync::RwLock::new(BRepModel::new())))
    }

    /// Create a command executor that operates on an externally-owned model.
    ///
    /// This is the production constructor — `AppState` shares its
    /// `Arc<RwLock<BRepModel>>` so AI, REST, and WebSocket entry points all
    /// mutate the same kernel state and agents see a coherent world.
    pub fn with_model(model: Arc<tokio::sync::RwLock<BRepModel>>) -> Self {
        Self {
            model,
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
            Command::Shell {
                object,
                faces_to_remove,
                thickness,
            } => self.shell(object, faces_to_remove, thickness).await,
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
            let mut model = model_clone.blocking_write();
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
            let mut model = model_clone.blocking_write();
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
            let mut model = model_clone.blocking_write();
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
            let mut model = model_clone.blocking_write();
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
            let mut model = model_clone.blocking_write();
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

        // A mirror goes through the kernel `mirror` — one recorded "mirror"
        // event and the per-face orientation restore — never through a bare
        // reflection matrix. A scale whose factors reverse orientation
        // (negative determinant) IS a reflection; it is refused here with the
        // remedy named rather than executed as an unrecorded mirror.
        if let shared_types::geometry_commands::Transform::Scale { factors } = &xform {
            let determinant = factors[0] as f64 * factors[1] as f64 * factors[2] as f64;
            if determinant < 0.0 {
                return Err(ExecutorError::InvalidParameters(format!(
                    concat!(
                        "scale factors {:?} reverse orientation (their product is ",
                        "negative): that is a reflection; issue a mirror transform ",
                        "about the intended plane instead"
                    ),
                    factors
                )));
            }
        }

        let model_clone = Arc::clone(&self.model);
        tokio::task::spawn_blocking(move || {
            let matrix = match xform {
                shared_types::geometry_commands::Transform::Translate { offset } => {
                    Matrix4::translation(offset[0] as f64, offset[1] as f64, offset[2] as f64)
                }
                shared_types::geometry_commands::Transform::Rotate {
                    axis,
                    angle_radians,
                } => {
                    let axis_vec = Vector3::new(axis[0] as f64, axis[1] as f64, axis[2] as f64);
                    Matrix4::from_axis_angle(&axis_vec, angle_radians)
                        .map_err(|e| ExecutorError::GeometryError(format!("{:?}", e)))?
                }
                shared_types::geometry_commands::Transform::Scale { factors } => {
                    Matrix4::from_scale(&Vector3::new(
                        factors[0] as f64,
                        factors[1] as f64,
                        factors[2] as f64,
                    ))
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
                    let mut model = model_clone.blocking_write();
                    mirror(
                        &mut model,
                        vec![solid_id],
                        point,
                        normal,
                        TransformOptions::default(),
                    )
                    .map_err(|e| ExecutorError::GeometryError(format!("{:?}", e)))?;
                    return Ok::<(), ExecutorError>(());
                }
            };

            let mut model = model_clone.blocking_write();
            transform_solid(&mut model, solid_id, matrix, TransformOptions::default())
                .map_err(|e| ExecutorError::GeometryError(format!("{:?}", e)))?;

            Ok::<(), ExecutorError>(())
        })
        .await
        .map_err(|e| ExecutorError::GeometryError(format!("Task join error: {}", e)))??;

        tracing::info!("Transformed solid: {:?}", object);

        Ok(object)
    }

    /// Hollow a solid into a shell of constant wall thickness.
    ///
    /// `faces_to_remove` are the kernel `FaceId`s of the faces that should be
    /// opened up to expose the interior cavity (e.g., the top face of a box
    /// to make an open-top container). `thickness` is the wall thickness in
    /// model units; pass a positive value — the kernel offsets inward.
    ///
    /// # Design Rationale
    /// - **Why spawn_blocking**: The shell op runs face-by-face surface
    ///   offsets, builds wall faces at each removed-face boundary, and
    ///   re-validates the resulting solid; CPU-heavy enough to keep off
    ///   the async runtime.
    /// - **Why a fresh GeometryId**: `offset_solid` adds a new hollow solid
    ///   to the model; the original solid is left in place but is no
    ///   longer referenced by the executor's id_map (mirroring the
    ///   boolean op's "result is a new entity" convention).
    /// - **Performance**: O(faces) for surface offset + O(rim edges) for
    ///   wall construction; sub-100 ms for typical primitives.
    async fn shell(
        &mut self,
        object: GeometryId,
        faces_to_remove: Vec<u32>,
        thickness: f64,
    ) -> Result<GeometryId, ExecutorError> {
        if !thickness.is_finite() || thickness.abs() < 1e-9 {
            return Err(ExecutorError::InvalidParameters(format!(
                "Shell thickness must be a non-zero finite number, got {thickness}"
            )));
        }

        let solid_id = self.get_solid_id(&object)?;
        let face_ids: Vec<FaceId> = faces_to_remove;

        let model_clone = Arc::clone(&self.model);
        let thickness_for_options = thickness.abs();
        let result_solid_id = tokio::task::spawn_blocking(move || {
            let options = OffsetOptions {
                offset_type: OffsetType::Distance(thickness_for_options),
                intersection_handling: IntersectionHandling::Trim,
                ..OffsetOptions::default()
            };
            let mut model = model_clone.blocking_write();
            offset_solid(
                &mut model,
                solid_id,
                thickness_for_options,
                face_ids,
                options,
            )
            .map_err(|e| ExecutorError::GeometryError(format!("{:?}", e)))
        })
        .await
        .map_err(|e| ExecutorError::GeometryError(format!("Task join error: {}", e)))??;

        let geometry_id = self.generate_geometry_id();
        self.id_map.insert(geometry_id.clone(), result_solid_id);
        self.solid_to_geometry
            .insert(result_solid_id, geometry_id.clone());

        tracing::info!(
            "Shell {:?} (thickness={}) -> {:?}",
            object,
            thickness,
            geometry_id
        );

        Ok(geometry_id)
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
        let mut model = self.model.write().await;
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
    async fn test_shell_rejects_zero_thickness() {
        let mut executor = CommandExecutor::new();
        let id = executor
            .execute(Command::CreateBox {
                width: 10.0,
                height: 10.0,
                depth: 10.0,
            })
            .await
            .expect("box creation should succeed");

        // Zero / NaN thickness must be rejected at the executor boundary
        // before reaching the kernel — surfacing a clean InvalidParameters
        // error to the agent rather than a kernel-side InvalidGeometry.
        let zero = executor
            .execute(Command::Shell {
                object: id.clone(),
                faces_to_remove: vec![],
                thickness: 0.0,
            })
            .await;
        assert!(matches!(zero, Err(ExecutorError::InvalidParameters(_))));

        let nan = executor
            .execute(Command::Shell {
                object: id,
                faces_to_remove: vec![],
                thickness: f64::NAN,
            })
            .await;
        assert!(matches!(nan, Err(ExecutorError::InvalidParameters(_))));
    }

    #[tokio::test]
    async fn test_shell_unknown_object_errors() {
        let mut executor = CommandExecutor::new();
        let bogus = GeometryId(Uuid::new_v4());
        let result = executor
            .execute(Command::Shell {
                object: bogus,
                faces_to_remove: vec![],
                thickness: 1.0,
            })
            .await;
        assert!(matches!(result, Err(ExecutorError::ObjectNotFound(_))));
    }

    /// Signed volume enclosed by the solid's tessellation — positive only when
    /// every face points out of the material.
    async fn signed_volume(executor: &CommandExecutor, id: &GeometryId) -> f64 {
        let solid_id = executor.get_solid_id(id).expect("known object");
        let model = executor.model.read().await;
        let solid = model.solids.get(solid_id).expect("solid exists");
        let mesh = geometry_engine::tessellation::tessellate_solid(
            &solid,
            &model,
            &geometry_engine::tessellation::TessellationParams::default(),
        );
        let mut six = 0.0;
        for tri in &mesh.triangles {
            let p0 = mesh.vertices[tri[0] as usize].position.to_vec();
            let p1 = mesh.vertices[tri[1] as usize].position.to_vec();
            let p2 = mesh.vertices[tri[2] as usize].position.to_vec();
            six += p0.dot(&p1.cross(&p2));
        }
        six / 6.0
    }

    async fn box_10x6x4(executor: &mut CommandExecutor) -> GeometryId {
        executor
            .execute(Command::CreateBox {
                width: 10.0,
                height: 6.0,
                depth: 4.0,
            })
            .await
            .expect("box creation should succeed")
    }

    /// Captures the kind of every operation the kernel records.
    #[derive(Debug, Default)]
    struct KindRecorder {
        kinds: std::sync::Mutex<Vec<String>>,
    }

    impl geometry_engine::operations::recorder::OperationRecorder for KindRecorder {
        fn record(
            &self,
            op: geometry_engine::operations::recorder::RecordedOperation,
        ) -> Result<(), geometry_engine::operations::recorder::RecorderError> {
            self.kinds.lock().expect("recorder mutex").push(op.kind);
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_mirror_runs_the_kernel_mirror_and_stays_outward() {
        let mut executor = CommandExecutor::new();
        let id = box_10x6x4(&mut executor).await;
        let before = signed_volume(&executor, &id).await;
        assert!(
            before > 0.0,
            "fixture: a fresh box is outward, V = {before}"
        );
        let recorder = Arc::new(KindRecorder::default());
        let _ = executor
            .model
            .write()
            .await
            .attach_recorder(Some(recorder.clone()
                as Arc<dyn geometry_engine::operations::recorder::OperationRecorder>));

        executor
            .execute(Command::Transform {
                object: id.clone(),
                transform: shared_types::geometry_commands::Transform::Mirror {
                    plane_normal: [1.0, 0.0, 0.0],
                    plane_point: [7.0, 0.0, 0.0],
                },
            })
            .await
            .expect("mirroring a box must succeed");

        assert_eq!(
            *recorder.kinds.lock().expect("recorder mutex"),
            vec!["mirror".to_string()],
            "an executor mirror must be the kernel mirror: one recorded \"mirror\" event"
        );
        let after = signed_volume(&executor, &id).await;
        assert!(
            (after - before).abs() < 1e-6 * before,
            "a mirrored box must stay outward-oriented with the same volume: V {before} -> {after}"
        );
    }

    #[tokio::test]
    async fn test_orientation_reversing_scale_is_refused() {
        let mut executor = CommandExecutor::new();
        let id = box_10x6x4(&mut executor).await;
        let before = signed_volume(&executor, &id).await;

        let refused = executor
            .execute(Command::Transform {
                object: id.clone(),
                transform: shared_types::geometry_commands::Transform::Scale {
                    factors: [-1.0, 1.0, 1.0],
                },
            })
            .await;
        match refused {
            Err(ExecutorError::InvalidParameters(msg)) => {
                assert!(
                    msg.contains("mirror"),
                    "the refusal names the remedy: {msg}"
                );
                assert!(!msg.contains("  "), "no whitespace runs: {msg:?}");
            }
            other => panic!("a determinant-negative scale must be refused, got {other:?}"),
        }
        let untouched = signed_volume(&executor, &id).await;
        assert!(
            (untouched - before).abs() < 1e-9 * before.abs(),
            "a refused scale must not touch the solid: V {before} -> {untouched}"
        );

        // Two negative factors are a 180° rotation (determinant +1): allowed.
        executor
            .execute(Command::Transform {
                object: id.clone(),
                transform: shared_types::geometry_commands::Transform::Scale {
                    factors: [-1.0, -1.0, 1.0],
                },
            })
            .await
            .expect("a determinant-positive scale must run");
        let rotated = signed_volume(&executor, &id).await;
        assert!(
            (rotated - before).abs() < 1e-6 * before,
            "a 180-degree turn keeps the box outward: V {before} -> {rotated}"
        );
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
