//! Geometry types and structures
//!
//! Core geometric representations including meshes, transforms, and CAD objects.

use crate::{Color, ObjectId, Position3D, Timestamp, MAX_TRIANGLES, MAX_VERTICES};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Unique identifier for geometry objects
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GeometryId(pub uuid::Uuid);

impl GeometryId {
    /// Create a new geometry ID
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    /// Create from a UUID
    pub fn from_uuid(id: uuid::Uuid) -> Self {
        Self(id)
    }

    /// Get the inner UUID
    pub fn uuid(&self) -> uuid::Uuid {
        self.0
    }
}

impl Default for GeometryId {
    /// Returns the nil (all-zero) geometry id, representing "no geometry".
    ///
    /// Distinct from [`GeometryId::new`], which allocates a fresh random id.
    fn default() -> Self {
        Self(uuid::Uuid::nil())
    }
}

impl From<uuid::Uuid> for GeometryId {
    fn from(id: uuid::Uuid) -> Self {
        Self(id)
    }
}

impl From<GeometryId> for uuid::Uuid {
    fn from(id: GeometryId) -> Self {
        id.0
    }
}

impl std::fmt::Display for GeometryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Triangle mesh representation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Mesh {
    /// Vertex positions as flat array [x0, y0, z0, x1, y1, z1, ...]
    pub vertices: Vec<f32>,
    /// Triangle indices (3 per triangle)
    pub indices: Vec<u32>,
    /// Vertex normals (same layout as vertices)
    pub normals: Vec<f32>,
    /// UV coordinates [u0, v0, u1, v1, ...] (optional)
    pub uvs: Option<Vec<f32>>,
    /// Vertex colors [r0, g0, b0, a0, ...] (optional)
    pub colors: Option<Vec<f32>>,
    /// Maps each triangle to its source B-Rep face ID.
    /// `face_map[triangle_index] = face_id`. Used for face picking in the viewport.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub face_map: Option<Vec<u32>>,
}

/// Apply quaternion rotation to a 3D point
fn apply_quaternion_rotation(
    x: f32,
    y: f32,
    z: f32,
    qx: f32,
    qy: f32,
    qz: f32,
    qw: f32,
) -> (f32, f32, f32) {
    // Quaternion rotation formula: v' = q * v * q^-1
    let ix = qw * x + qy * z - qz * y;
    let iy = qw * y + qz * x - qx * z;
    let iz = qw * z + qx * y - qy * x;
    let iw = -qx * x - qy * y - qz * z;

    let rx = ix * qw + iw * -qx + iy * -qz - iz * -qy;
    let ry = iy * qw + iw * -qy + iz * -qx - ix * -qz;
    let rz = iz * qw + iw * -qz + ix * -qy - iy * -qx;

    (rx, ry, rz)
}

/// 3D transformation representation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Transform3D {
    /// Translation vector [x, y, z]
    pub translation: Position3D,
    /// Rotation quaternion [x, y, z, w]
    pub rotation: [f32; 4],
    /// Scale factors [x, y, z]
    pub scale: Position3D,
}

/// Display quality for tessellation
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum DisplayQuality {
    /// Low quality for fast preview
    Low,
    /// Medium quality for normal use
    #[default]
    Medium,
    /// High quality for production
    High,
    /// Custom quality parameters
    Custom {
        /// Maximum edge length allowed in the tessellation.
        max_edge_length: f64,
        /// Maximum deviation between adjacent face normals (radians).
        max_angle_deviation: f64,
        /// Chordal tolerance between tessellated mesh and exact surface.
        chord_tolerance: f64,
    },
}

/// Analytical geometry representation (exact mathematical form)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyticalGeometry {
    /// Reference to solid in geometry engine
    pub solid_id: u32,
    /// Primitive type for easy identification
    pub primitive_type: String,
    /// Creation parameters for parametric editing
    pub parameters: HashMap<String, f64>,
    /// Analytical properties (volume, surface area, etc.)
    pub properties: AnalyticalProperties,
}

/// Analytical properties computed from exact geometry
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AnalyticalProperties {
    /// Exact volume
    pub volume: f64,
    /// Exact surface area
    pub surface_area: f64,
    /// Exact bounding box
    pub bounding_box: BoundingBox,
    /// Center of mass
    pub center_of_mass: [f64; 3],
    /// Mass properties (if density is known)
    pub mass_properties: Option<MassProperties>,
}

/// Result of geometry operations containing both analytical and tessellated data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeometryResult {
    /// Tessellated mesh for visualization
    pub mesh: Mesh,
    /// Analytical properties computed from exact B-Rep
    pub properties: AnalyticalProperties,
}

/// Mass properties for engineering analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MassProperties {
    /// Mass (volume * density)
    pub mass: f64,
    /// Moments of inertia tensor
    pub moments_of_inertia: [[f64; 3]; 3],
    /// Principal axes
    pub principal_axes: [[f64; 3]; 3],
}

/// Cached mesh data for visualization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedMesh {
    /// The tessellated mesh
    pub mesh: Mesh,
    /// Quality parameters used for this tessellation
    pub quality: DisplayQuality,
    /// Cache timestamp for invalidation
    pub cached_at: Timestamp,
    /// Whether this cache is still valid
    pub is_valid: bool,
}

/// Geometry representation that can be either analytical or mesh-based
///
/// The Analytical variant is the common case for kernel-produced objects and
/// is intentionally kept inline (not boxed) so that creation and matching
/// avoid an extra heap allocation on every operation. The size disparity
/// with the Mesh variant is accepted.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GeometryRepresentation {
    /// Analytical B-Rep solid (exact mathematical representation)
    Analytical {
        /// Exact geometry data
        geometry: AnalyticalGeometry,
        /// Cached mesh for visualization (optional)
        cached_mesh: Option<CachedMesh>,
    },
    /// Direct mesh representation (for imported or non-parametric objects)
    Mesh {
        /// The mesh data
        mesh: Mesh,
    },
}

impl GeometryRepresentation {
    /// Get mesh for display, tessellating if necessary
    pub fn get_mesh_for_display(&mut self, quality: DisplayQuality) -> Option<&Mesh> {
        match self {
            GeometryRepresentation::Analytical { cached_mesh, .. } => {
                // Check if we have a valid cached mesh with correct quality
                if let Some(cached) = cached_mesh {
                    if cached.is_valid && cached.quality == quality {
                        return Some(&cached.mesh);
                    }
                }
                // Cache is invalid or wrong quality - caller should tessellate
                None
            }
            GeometryRepresentation::Mesh { mesh } => {
                // Direct mesh, always available
                Some(mesh)
            }
        }
    }

    /// Check if this geometry has valid cached mesh
    pub fn has_valid_mesh_cache(&self, quality: DisplayQuality) -> bool {
        match self {
            GeometryRepresentation::Analytical { cached_mesh, .. } => cached_mesh
                .as_ref()
                .map(|cached| cached.is_valid && cached.quality == quality)
                .unwrap_or(false),
            GeometryRepresentation::Mesh { .. } => true,
        }
    }

    /// Invalidate cached mesh (when analytical geometry changes)
    pub fn invalidate_cache(&mut self) {
        if let GeometryRepresentation::Analytical {
            cached_mesh: Some(cached),
            ..
        } = self
        {
            cached.is_valid = false;
        }
    }

    /// Update cached mesh
    pub fn update_cached_mesh(&mut self, mesh: Mesh, quality: DisplayQuality) {
        if let GeometryRepresentation::Analytical { cached_mesh, .. } = self {
            *cached_mesh = Some(CachedMesh {
                mesh,
                quality,
                cached_at: chrono::Utc::now().timestamp_millis() as u64,
                is_valid: true,
            });
        }
    }

    /// Get analytical properties if available
    pub fn get_analytical_properties(&self) -> Option<&AnalyticalProperties> {
        match self {
            GeometryRepresentation::Analytical { geometry, .. } => Some(&geometry.properties),
            GeometryRepresentation::Mesh { .. } => None,
        }
    }
}

/// Complete CAD object representation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CADObject {
    /// Unique identifier
    pub id: ObjectId,
    /// Human-readable name
    pub name: String,
    /// Legacy mesh field (for backward compatibility)
    pub mesh: Mesh,
    /// Optional analytical geometry (for parametric objects)
    pub analytical_geometry: Option<AnalyticalGeometry>,
    /// Cached mesh data for different qualities
    pub cached_meshes: HashMap<String, CachedMesh>,
    /// 3D transformation
    pub transform: Transform3D,
    /// Material properties
    pub material: MaterialProperties,
    /// Visibility flag
    pub visible: bool,
    /// Lock state (prevents modification)
    pub locked: bool,
    /// Parent object ID (for hierarchies)
    pub parent: Option<ObjectId>,
    /// Child object IDs
    pub children: Vec<ObjectId>,
    /// Custom metadata
    pub metadata: HashMap<String, serde_json::Value>,
    /// Creation timestamp
    pub created_at: Timestamp,
    /// Last modification timestamp
    pub modified_at: Timestamp,
}

impl CADObject {
    /// Create a new CAD object with mesh-only representation
    pub fn new_mesh_object(id: ObjectId, name: String, mesh: Mesh) -> Self {
        Self {
            id,
            name,
            mesh,
            analytical_geometry: None,
            cached_meshes: HashMap::new(),
            transform: Transform3D::identity(),
            material: MaterialProperties::default(),
            visible: true,
            locked: false,
            parent: None,
            children: Vec::new(),
            metadata: HashMap::new(),
            created_at: chrono::Utc::now().timestamp_millis() as u64,
            modified_at: chrono::Utc::now().timestamp_millis() as u64,
        }
    }

    /// Create a new CAD object with analytical geometry
    pub fn new_analytical_object(
        id: ObjectId,
        name: String,
        analytical_geometry: AnalyticalGeometry,
        display_mesh: Mesh,
    ) -> Self {
        Self {
            id,
            name,
            mesh: display_mesh, // Legacy field gets the display mesh
            analytical_geometry: Some(analytical_geometry),
            cached_meshes: HashMap::new(),
            transform: Transform3D::identity(),
            material: MaterialProperties::default(),
            visible: true,
            locked: false,
            parent: None,
            children: Vec::new(),
            metadata: HashMap::new(),
            created_at: chrono::Utc::now().timestamp_millis() as u64,
            modified_at: chrono::Utc::now().timestamp_millis() as u64,
        }
    }

    /// Check if this is an analytical (parametric) object
    pub fn is_analytical(&self) -> bool {
        self.analytical_geometry.is_some()
    }

    /// Get the solid ID if this is an analytical object
    pub fn solid_id(&self) -> Option<u32> {
        self.analytical_geometry.as_ref().map(|g| g.solid_id)
    }

    /// Get analytical properties if available
    pub fn analytical_properties(&self) -> Option<&AnalyticalProperties> {
        self.analytical_geometry.as_ref().map(|g| &g.properties)
    }

    /// Get cached mesh for specific quality, if available
    pub fn get_cached_mesh(&self, quality: &str) -> Option<&Mesh> {
        self.cached_meshes
            .get(quality)
            .filter(|cached| cached.is_valid)
            .map(|cached| &cached.mesh)
    }

    /// Update cached mesh for specific quality
    pub fn update_cached_mesh(&mut self, quality: String, mesh: Mesh) {
        let cached_mesh = CachedMesh {
            mesh,
            quality: match quality.as_str() {
                "low" => DisplayQuality::Low,
                "high" => DisplayQuality::High,
                _ => DisplayQuality::Medium,
            },
            cached_at: chrono::Utc::now().timestamp_millis() as u64,
            is_valid: true,
        };
        self.cached_meshes.insert(quality, cached_mesh);
        self.modified_at = chrono::Utc::now().timestamp_millis() as u64;
    }

    /// Invalidate all cached meshes (call when analytical geometry changes)
    pub fn invalidate_mesh_cache(&mut self) {
        for cached in self.cached_meshes.values_mut() {
            cached.is_valid = false;
        }
        self.modified_at = chrono::Utc::now().timestamp_millis() as u64;
    }

    /// Get display mesh (either cached or legacy mesh field)
    pub fn get_display_mesh(&self, quality: &str) -> &Mesh {
        self.get_cached_mesh(quality).unwrap_or(&self.mesh)
    }
}

/// Material properties for rendering
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MaterialProperties {
    /// Base color [r, g, b, a]
    pub diffuse_color: Color,
    /// Metallic factor (0.0 = dielectric, 1.0 = metal)
    pub metallic: f32,
    /// Roughness factor (0.0 = smooth, 1.0 = rough)
    pub roughness: f32,
    /// Emission color [r, g, b]
    pub emission: [f32; 3],
    /// Material name
    pub name: String,
}

/// Axis-aligned bounding box
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub struct BoundingBox {
    /// Minimum corner [x, y, z]
    pub min: [f32; 3],
    /// Maximum corner [x, y, z]
    pub max: [f32; 3],
}

impl BoundingBox {
    /// Get the center point of the bounding box
    pub fn center(&self) -> [f32; 3] {
        [
            (self.min[0] + self.max[0]) * 0.5,
            (self.min[1] + self.max[1]) * 0.5,
            (self.min[2] + self.max[2]) * 0.5,
        ]
    }

    /// Get the size of the bounding box
    pub fn size(&self) -> [f32; 3] {
        [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ]
    }

    /// Get the volume
    pub fn volume(&self) -> f32 {
        let size = self.size();
        size[0] * size[1] * size[2]
    }

    /// Check if a point is inside the box
    pub fn contains_point(&self, point: &[f32; 3]) -> bool {
        point[0] >= self.min[0]
            && point[0] <= self.max[0]
            && point[1] >= self.min[1]
            && point[1] <= self.max[1]
            && point[2] >= self.min[2]
            && point[2] <= self.max[2]
    }

    /// Expand the box to include a point
    pub fn expand_to_include(&mut self, point: &[f32; 3]) {
        for ((min_c, max_c), &p) in self
            .min
            .iter_mut()
            .zip(self.max.iter_mut())
            .zip(point.iter())
        {
            *min_c = min_c.min(p);
            *max_c = max_c.max(p);
        }
    }

    /// Check if this box intersects with another
    pub fn intersects(&self, other: &BoundingBox) -> bool {
        self.max
            .iter()
            .zip(other.min.iter())
            .zip(self.min.iter())
            .zip(other.max.iter())
            .all(|(((&s_max, &o_min), &s_min), &o_max)| s_max >= o_min && s_min <= o_max)
    }

    /// Compute the intersection of two bounding boxes
    pub fn intersection(&self, other: &BoundingBox) -> Option<BoundingBox> {
        if !self.intersects(other) {
            return None;
        }

        Some(BoundingBox {
            min: [
                self.min[0].max(other.min[0]),
                self.min[1].max(other.min[1]),
                self.min[2].max(other.min[2]),
            ],
            max: [
                self.max[0].min(other.max[0]),
                self.max[1].min(other.max[1]),
                self.max[2].min(other.max[2]),
            ],
        })
    }

    /// Compute the union of two bounding boxes
    pub fn union(&self, other: &BoundingBox) -> BoundingBox {
        BoundingBox {
            min: [
                self.min[0].min(other.min[0]),
                self.min[1].min(other.min[1]),
                self.min[2].min(other.min[2]),
            ],
            max: [
                self.max[0].max(other.max[0]),
                self.max[1].max(other.max[1]),
                self.max[2].max(other.max[2]),
            ],
        }
    }
}

/// Boolean operation types
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum BooleanOp {
    /// Union (A + B)
    Union,
    /// Intersection (A ∩ B)
    Intersection,
    /// Difference (A - B)
    Difference,
}

/// Supported primitive shape types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum PrimitiveType {
    /// Rectangular box
    Box,
    /// Sphere
    Sphere,
    /// Cylinder
    Cylinder,
    /// Cone
    Cone,
    /// Torus
    Torus,
    /// Gear wheel
    Gear,
    /// Mounting bracket
    Bracket,
    /// Parametric surface
    Parametric,
    /// B-Spline curve (converted to tube mesh)
    BSplineCurve,
    /// NURBS curve (converted to tube mesh)
    NURBSCurve,
    /// B-Spline surface
    BSplineSurface,
}

/// Parameters for shape generation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShapeParameters {
    /// Named parameters with values
    pub params: HashMap<String, f64>,
}

/// Parameters for B-Spline/NURBS curves
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurveParameters {
    /// Control points (flattened: [x0,y0,z0, x1,y1,z1, ...])
    pub control_points: Vec<f64>,
    /// Weights for NURBS (same length as control points/3)
    pub weights: Option<Vec<f64>>,
    /// Polynomial degree (3-5)
    pub degree: usize,
    /// Knot vector (must be len = control_points.len()/3 + degree + 1)
    pub knots: Vec<f64>,
}

/// Surface parameters for B-Spline/NURBS surfaces
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurfaceParameters {
    /// Control points grid (row-major, flattened)
    pub control_points: Vec<f64>,
    /// Grid dimensions (rows, cols)
    pub dimensions: (usize, usize),
    /// Weights for NURBS (same layout as control points)
    pub weights: Option<Vec<f64>>,
    /// Polynomial degrees (u, v)
    pub degrees: (usize, usize),
    /// Knot vectors (u, v)
    pub knots: (Vec<f64>, Vec<f64>),
}

// Implementation blocks
impl Transform3D {
    /// Create identity transform
    pub fn identity() -> Self {
        Self {
            translation: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        }
    }

    /// Create transform from position only
    pub fn from_position(position: Position3D) -> Self {
        Self {
            translation: position,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        }
    }

    /// Transform a point by this transform
    ///
    /// Applies scale, rotation, then translation (standard TRS order)
    ///
    /// # Performance
    /// O(1) - Direct computation with quaternion rotation
    pub fn transform_point(&self, point: &[f32; 3]) -> [f32; 3] {
        // Apply scale
        let x = point[0] * self.scale[0];
        let y = point[1] * self.scale[1];
        let z = point[2] * self.scale[2];

        // Apply rotation (quaternion)
        let (rx, ry, rz) = apply_quaternion_rotation(
            x,
            y,
            z,
            self.rotation[0],
            self.rotation[1],
            self.rotation[2],
            self.rotation[3],
        );

        // Apply translation
        [
            rx + self.translation[0],
            ry + self.translation[1],
            rz + self.translation[2],
        ]
    }

    /// Transform a vector (direction) by this transform
    ///
    /// Only applies rotation and scale, not translation
    ///
    /// # Performance
    /// O(1) - Direct computation with quaternion rotation
    pub fn transform_vector(&self, vector: &[f32; 3]) -> [f32; 3] {
        // Apply scale
        let x = vector[0] * self.scale[0];
        let y = vector[1] * self.scale[1];
        let z = vector[2] * self.scale[2];

        // Apply rotation (quaternion)
        let (rx, ry, rz) = apply_quaternion_rotation(
            x,
            y,
            z,
            self.rotation[0],
            self.rotation[1],
            self.rotation[2],
            self.rotation[3],
        );

        [rx, ry, rz]
    }

    /// Compose two transforms (multiply)
    ///
    /// Returns a new transform that represents applying `self` then `other`
    pub fn compose(&self, other: &Transform3D) -> Transform3D {
        // Compose scales (multiply)
        let scale = [
            self.scale[0] * other.scale[0],
            self.scale[1] * other.scale[1],
            self.scale[2] * other.scale[2],
        ];

        // Compose rotations (quaternion multiplication)
        let rotation = Self::multiply_quaternions(&self.rotation, &other.rotation);

        // Transform other's translation by self, then add
        let translated = self.transform_point(&other.translation);

        Transform3D {
            translation: translated,
            rotation,
            scale,
        }
    }

    /// Multiply two quaternions
    fn multiply_quaternions(q1: &[f32; 4], q2: &[f32; 4]) -> [f32; 4] {
        [
            q1[3] * q2[0] + q1[0] * q2[3] + q1[1] * q2[2] - q1[2] * q2[1],
            q1[3] * q2[1] - q1[0] * q2[2] + q1[1] * q2[3] + q1[2] * q2[0],
            q1[3] * q2[2] + q1[0] * q2[1] - q1[1] * q2[0] + q1[2] * q2[3],
            q1[3] * q2[3] - q1[0] * q2[0] - q1[1] * q2[1] - q1[2] * q2[2],
        ]
    }

    /// Get the inverse transform
    ///
    /// Returns a transform that undoes this transform
    pub fn inverse(&self) -> Transform3D {
        // Inverse scale
        let inv_scale = [
            1.0 / self.scale[0],
            1.0 / self.scale[1],
            1.0 / self.scale[2],
        ];

        // Inverse rotation (conjugate for unit quaternion)
        let inv_rotation = [
            -self.rotation[0],
            -self.rotation[1],
            -self.rotation[2],
            self.rotation[3],
        ];

        // Inverse translation (transform by inverse rotation and scale)
        let neg_trans = [
            -self.translation[0],
            -self.translation[1],
            -self.translation[2],
        ];

        let inv_transform = Transform3D {
            translation: [0.0, 0.0, 0.0],
            rotation: inv_rotation,
            scale: inv_scale,
        };

        let inv_translation = inv_transform.transform_point(&neg_trans);

        Transform3D {
            translation: inv_translation,
            rotation: inv_rotation,
            scale: inv_scale,
        }
    }
}

impl Default for MaterialProperties {
    /// Default material: neutral gray plastic.
    fn default() -> Self {
        Self {
            diffuse_color: [0.7, 0.7, 0.7, 1.0],
            metallic: 0.0,
            roughness: 0.5,
            emission: [0.0, 0.0, 0.0],
            name: "default".to_string(),
        }
    }
}

impl MaterialProperties {

    /// Create steel material
    pub fn steel() -> Self {
        Self {
            diffuse_color: [0.6, 0.6, 0.65, 1.0],
            metallic: 0.9,
            roughness: 0.3,
            emission: [0.0, 0.0, 0.0],
            name: "steel".to_string(),
        }
    }

    /// Create aluminum material
    pub fn aluminum() -> Self {
        Self {
            diffuse_color: [0.8, 0.8, 0.82, 1.0],
            metallic: 0.8,
            roughness: 0.4,
            emission: [0.0, 0.0, 0.0],
            name: "aluminum".to_string(),
        }
    }

    /// Create plastic material
    pub fn plastic() -> Self {
        Self {
            diffuse_color: [0.85, 0.85, 0.85, 1.0],
            metallic: 0.0,
            roughness: 0.6,
            emission: [0.0, 0.0, 0.0],
            name: "plastic".to_string(),
        }
    }

    /// Create glass material
    pub fn glass() -> Self {
        Self {
            diffuse_color: [0.94, 0.94, 0.94, 0.3],
            metallic: 0.0,
            roughness: 0.1,
            emission: [0.0, 0.0, 0.0],
            name: "glass".to_string(),
        }
    }
}

impl ShapeParameters {
    /// Create box parameters
    pub fn box_params(width: f64, height: f64, depth: f64) -> Self {
        let mut params = HashMap::new();
        params.insert("width".to_string(), width);
        params.insert("height".to_string(), height);
        params.insert("depth".to_string(), depth);
        Self { params }
    }

    /// Create sphere parameters
    pub fn sphere_params(radius: f64) -> Self {
        let mut params = HashMap::new();
        params.insert("radius".to_string(), radius);
        Self { params }
    }

    /// Create cylinder parameters
    pub fn cylinder_params(radius: f64, height: f64) -> Self {
        let mut params = HashMap::new();
        params.insert("radius".to_string(), radius);
        params.insert("height".to_string(), height);
        Self { params }
    }

    /// Create cone parameters
    pub fn cone_params(radius: f64, height: f64) -> Self {
        let mut params = HashMap::new();
        params.insert("radius".to_string(), radius);
        params.insert("height".to_string(), height);
        Self { params }
    }

    /// Create torus parameters
    pub fn torus_params(major_radius: f64, minor_radius: f64) -> Self {
        let mut params = HashMap::new();
        params.insert("major_radius".to_string(), major_radius);
        params.insert("minor_radius".to_string(), minor_radius);
        Self { params }
    }
}

impl Mesh {
    /// Create a new empty mesh
    pub fn new() -> Self {
        Self {
            vertices: Vec::new(),
            indices: Vec::new(),
            normals: Vec::new(),
            uvs: None,
            colors: None,
            face_map: None,
        }
    }

    /// Validate mesh structure
    pub fn validate(&self) -> Result<(), crate::GeometryError> {
        // Check vertex array
        if self.vertices.len() % 3 != 0 {
            return Err(crate::GeometryError::InvalidMesh {
                reason: "Vertex count not multiple of 3".to_string(),
            });
        }

        // Check index array
        if self.indices.len() % 3 != 0 {
            return Err(crate::GeometryError::InvalidMesh {
                reason: "Index count not multiple of 3".to_string(),
            });
        }

        // Check vertex count limit
        let vertex_count = self.vertices.len() / 3;
        if vertex_count > MAX_VERTICES {
            return Err(crate::GeometryError::InvalidMesh {
                reason: format!(
                    "Vertex count {} exceeds maximum {}",
                    vertex_count, MAX_VERTICES
                ),
            });
        }

        // Check triangle count limit
        let triangle_count = self.indices.len() / 3;
        if triangle_count > MAX_TRIANGLES {
            return Err(crate::GeometryError::InvalidMesh {
                reason: format!(
                    "Triangle count {} exceeds maximum {}",
                    triangle_count, MAX_TRIANGLES
                ),
            });
        }

        // Validate indices
        for &index in &self.indices {
            if index as usize >= vertex_count {
                return Err(crate::GeometryError::InvalidMesh {
                    reason: format!(
                        "Index {} out of bounds (vertex count: {})",
                        index, vertex_count
                    ),
                });
            }
        }

        // Validate normals if present
        if !self.normals.is_empty() && self.normals.len() != self.vertices.len() {
            return Err(crate::GeometryError::InvalidMesh {
                reason: "Normal count doesn't match vertex count".to_string(),
            });
        }

        // Validate UVs if present
        if let Some(ref uvs) = self.uvs {
            if uvs.len() != (vertex_count * 2) {
                return Err(crate::GeometryError::InvalidMesh {
                    reason: "UV count doesn't match vertex count".to_string(),
                });
            }
        }

        Ok(())
    }

    /// Get vertex count
    pub fn vertex_count(&self) -> usize {
        self.vertices.len() / 3
    }

    /// Get triangle count
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Calculate bounding box
    pub fn bounds(&self) -> BoundingBox {
        if self.vertices.is_empty() {
            return BoundingBox {
                min: [0.0, 0.0, 0.0],
                max: [0.0, 0.0, 0.0],
            };
        }

        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];

        for chunk in self.vertices.chunks_exact(3) {
            for ((min_c, max_c), &v) in
                min.iter_mut().zip(max.iter_mut()).zip(chunk.iter())
            {
                *min_c = min_c.min(v);
                *max_c = max_c.max(v);
            }
        }

        BoundingBox { min, max }
    }

    /// Merge another mesh into this one
    ///
    /// # Arguments
    /// * `other` - The mesh to merge
    /// * `transform` - Optional transformation to apply to the other mesh before merging
    ///
    /// # Performance
    /// O(n) where n is the number of vertices in the other mesh
    pub fn merge(&mut self, other: &Mesh, transform: Option<&Transform3D>) {
        let vertex_offset = (self.vertices.len() / 3) as u32;

        // Apply transformation if provided
        if let Some(t) = transform {
            // Merge transformed vertices
            for chunk in other.vertices.chunks_exact(3) {
                let [vx, vy, vz] = match *chunk {
                    [a, b, c] => [a, b, c],
                    _ => continue,
                };

                // Apply scale
                let x = vx * t.scale[0];
                let y = vy * t.scale[1];
                let z = vz * t.scale[2];

                // Apply rotation (quaternion)
                let (rx, ry, rz) = apply_quaternion_rotation(
                    x,
                    y,
                    z,
                    t.rotation[0],
                    t.rotation[1],
                    t.rotation[2],
                    t.rotation[3],
                );

                // Apply translation
                self.vertices.push(rx + t.translation[0]);
                self.vertices.push(ry + t.translation[1]);
                self.vertices.push(rz + t.translation[2]);
            }

            // Transform normals (rotation only, no translation/scale)
            for chunk in other.normals.chunks_exact(3) {
                let [nx_in, ny_in, nz_in] = match *chunk {
                    [a, b, c] => [a, b, c],
                    _ => continue,
                };
                let (nx, ny, nz) = apply_quaternion_rotation(
                    nx_in,
                    ny_in,
                    nz_in,
                    t.rotation[0],
                    t.rotation[1],
                    t.rotation[2],
                    t.rotation[3],
                );
                self.normals.push(nx);
                self.normals.push(ny);
                self.normals.push(nz);
            }
        } else {
            // Simple append without transformation
            self.vertices.extend_from_slice(&other.vertices);
            self.normals.extend_from_slice(&other.normals);
        }

        // Merge indices with offset
        for &idx in &other.indices {
            self.indices.push(idx + vertex_offset);
        }

        // Merge UVs if both meshes have them
        if let (Some(ref mut self_uvs), Some(ref other_uvs)) = (&mut self.uvs, &other.uvs) {
            self_uvs.extend_from_slice(other_uvs);
        } else if self.uvs.is_some() && other.uvs.is_none() {
            // Fill with default UVs if one mesh has them but the other doesn't
            if let Some(ref mut self_uvs) = &mut self.uvs {
                let num_new_vertices = other.vertices.len() / 3;
                self_uvs.extend(vec![0.0; num_new_vertices * 2]);
            }
        }

        // Merge colors if both meshes have them
        if let (Some(ref mut self_colors), Some(ref other_colors)) =
            (&mut self.colors, &other.colors)
        {
            self_colors.extend_from_slice(other_colors);
        } else if self.colors.is_some() && other.colors.is_none() {
            // Fill with white if one mesh has colors but the other doesn't
            if let Some(ref mut self_colors) = &mut self.colors {
                let num_new_vertices = other.vertices.len() / 3;
                self_colors.extend(vec![1.0; num_new_vertices * 4]);
            }
        }
    }

    /// Merge multiple meshes into a single mesh
    ///
    /// # Arguments
    /// * `meshes` - Vector of meshes to merge
    /// * `transforms` - Optional transformations for each mesh
    ///
    /// # Returns
    /// A new merged mesh containing all input meshes
    pub fn merge_multiple(meshes: Vec<&Mesh>, transforms: Option<Vec<&Transform3D>>) -> Self {
        let mut result = Self::new();

        // Pre-allocate capacity for better performance
        let total_vertices: usize = meshes.iter().map(|m| m.vertices.len()).sum();
        let total_indices: usize = meshes.iter().map(|m| m.indices.len()).sum();
        result.vertices.reserve(total_vertices);
        result.normals.reserve(total_vertices);
        result.indices.reserve(total_indices);

        // Check if any mesh has UVs or colors
        let has_uvs = meshes.iter().any(|m| m.uvs.is_some());
        let has_colors = meshes.iter().any(|m| m.colors.is_some());

        if has_uvs {
            result.uvs = Some(Vec::with_capacity(total_vertices * 2 / 3));
        }
        if has_colors {
            result.colors = Some(Vec::with_capacity(total_vertices * 4 / 3));
        }

        // Merge each mesh
        for (i, mesh) in meshes.iter().enumerate() {
            let transform = transforms.as_ref().and_then(|t| t.get(i).copied());
            result.merge(mesh, transform);
        }

        result
    }

    /// Calculate bounding box for mesh
    pub fn bounding_box(&self) -> ([f32; 3], [f32; 3]) {
        if self.vertices.is_empty() {
            return ([0.0; 3], [0.0; 3]);
        }

        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];

        for chunk in self.vertices.chunks_exact(3) {
            for ((min_c, max_c), &v) in
                min.iter_mut().zip(max.iter_mut()).zip(chunk.iter())
            {
                *min_c = min_c.min(v);
                *max_c = max_c.max(v);
            }
        }

        (min, max)
    }
}

impl Default for Mesh {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mesh_validation() {
        let mut mesh = Mesh::new();

        // Valid empty mesh
        assert!(mesh.validate().is_ok());

        // Add valid triangle
        mesh.vertices = vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        mesh.indices = vec![0, 1, 2];
        assert!(mesh.validate().is_ok());

        // Invalid vertex count
        mesh.vertices.push(1.0);
        assert!(mesh.validate().is_err());
        mesh.vertices.pop();

        // Invalid index
        mesh.indices[0] = 10;
        assert!(mesh.validate().is_err());
    }

    #[test]
    fn test_transform() {
        let transform = Transform3D::from_position([1.0, 2.0, 3.0]);
        let point = [0.0, 0.0, 0.0];
        let transformed = transform.transform_point(&point);
        assert_eq!(transformed, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_bounding_box() {
        let mut bbox = BoundingBox {
            min: [0.0, 0.0, 0.0],
            max: [1.0, 1.0, 1.0],
        };

        assert_eq!(bbox.center(), [0.5, 0.5, 0.5]);
        assert_eq!(bbox.size(), [1.0, 1.0, 1.0]);
        assert_eq!(bbox.volume(), 1.0);

        assert!(bbox.contains_point(&[0.5, 0.5, 0.5]));
        assert!(!bbox.contains_point(&[2.0, 0.5, 0.5]));

        bbox.expand_to_include(&[2.0, 0.5, 0.5]);
        assert_eq!(bbox.max[0], 2.0);
    }
}
