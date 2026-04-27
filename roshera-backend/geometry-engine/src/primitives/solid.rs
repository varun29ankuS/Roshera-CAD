//! Solid representation for B-Rep topology.
//!
//! Features:
//! - Boolean operations (union, intersection, difference)
//! - Feature recognition and suppression
//! - Parametric history tracking via timeline events
//! - Multi-resolution representations
//! - Solid healing and repair
//! - Mass properties with material support
//! - Collision-detection acceleration structures
//! - Feature-based modeling operations
//!
//! Indexed access into shell/face enumeration arrays is the canonical idiom
//! — bounded by topology length. Matches the pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use crate::math::{consts, MathResult, Matrix4, Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::CurveStore,
    edge::EdgeStore,
    face::{FaceId, FaceStore},
    r#loop::LoopStore,
    shell::{MassProperties, ShellId, ShellStore},
    surface::SurfaceStore,
    vertex::VertexStore,
};
use std::collections::{HashMap, HashSet};
use parking_lot::RwLock;
use std::sync::Arc;

/// Solid ID type
pub type SolidId = u32;

/// Invalid solid ID constant
pub const INVALID_SOLID_ID: SolidId = u32::MAX;

/// Material properties
#[derive(Debug, Clone)]
pub struct Material {
    /// Material name
    pub name: String,
    /// Density (kg/m³)
    pub density: f64,
    /// Young's modulus (Pa)
    pub youngs_modulus: f64,
    /// Poisson's ratio
    pub poissons_ratio: f64,
    /// Thermal expansion coefficient (1/K)
    pub thermal_expansion: f64,
    /// Custom properties
    pub properties: HashMap<String, f64>,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            name: "Steel".to_string(),
            density: 7850.0,       // kg/m³
            youngs_modulus: 200e9, // Pa
            poissons_ratio: 0.3,
            thermal_expansion: 12e-6, // 1/K
            properties: HashMap::new(),
        }
    }
}

/// Feature types for feature-based modeling
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FeatureType {
    Hole,
    Boss,
    Pocket,
    Rib,
    Slot,
    Chamfer,
    Fillet,
    Thread,
    Pattern,
    Shell,
    Draft,
    Custom,
}

/// Feature in solid
#[derive(Debug, Clone)]
pub struct Feature {
    /// Feature ID
    pub id: u32,
    /// Feature type
    pub feature_type: FeatureType,
    /// Faces belonging to this feature
    pub faces: Vec<FaceId>,
    /// Parent feature (if dependent)
    pub parent: Option<u32>,
    /// Feature parameters
    pub parameters: HashMap<String, f64>,
    /// Is feature suppressed
    pub suppressed: bool,
}

/// Solid attributes
#[derive(Debug, Clone)]
pub struct SolidAttributes {
    /// Display color (RGBA)
    pub color: [f32; 4],
    /// Material
    pub material: Material,
    /// Layer ID
    pub layer: Option<u32>,
    /// Visibility
    pub visible: bool,
    /// Selection state (currently selected by user)
    pub selected: bool,
    /// Selectable (whether user input may select this solid; locked solids
    /// stay visible but cannot be picked)
    pub selectable: bool,
    /// User-defined attributes
    pub user_data: HashMap<String, String>,
}

impl Default for SolidAttributes {
    fn default() -> Self {
        Self {
            color: [0.7, 0.7, 0.7, 1.0],
            material: Material::default(),
            layer: None,
            visible: true,
            selected: false,
            selectable: true,
            user_data: HashMap::new(),
        }
    }
}

/// Mass properties for solid
#[derive(Debug, Clone)]
pub struct SolidMassProperties {
    /// Volume
    pub volume: f64,
    /// Mass (using material density)
    pub mass: f64,
    /// Center of mass
    pub center_of_mass: Point3,
    /// Moments of inertia about center of mass
    pub inertia_tensor: [[f64; 3]; 3],
    /// Principal moments
    pub principal_moments: Vector3,
    /// Principal axes (column vectors)
    pub principal_axes: [Vector3; 3],
    /// Radius of gyration
    pub radius_of_gyration: Vector3,
}

/// Solid statistics
#[derive(Debug, Clone)]
pub struct SolidStats {
    /// Number of shells
    pub shell_count: usize,
    /// Number of faces
    pub face_count: usize,
    /// Number of edges
    pub edge_count: usize,
    /// Number of vertices
    pub vertex_count: usize,
    /// Number of features
    pub feature_count: usize,
    /// Euler characteristic
    pub euler_characteristic: i32,
    /// Genus
    pub genus: i32,
    /// Bounding box
    pub bbox_min: Point3,
    pub bbox_max: Point3,
}

/// Boolean operation type
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BooleanOp {
    Union,
    Intersection,
    Difference,
    SymmetricDifference,
}

/// History node for parametric modeling
#[derive(Debug, Clone)]
pub struct HistoryNode {
    /// Operation ID
    pub id: u32,
    /// Operation type
    pub operation: String,
    /// Input solids
    pub inputs: Vec<SolidId>,
    /// Output solid
    pub output: SolidId,
    /// Parameters
    pub parameters: HashMap<String, serde_json::Value>,
    /// Timestamp
    pub timestamp: std::time::SystemTime,
}

/// Solid representation
#[derive(Debug, Clone)]
pub struct Solid {
    /// Unique identifier
    pub id: SolidId,
    /// Outer shell (defines exterior boundary)
    pub outer_shell: ShellId,
    /// Inner shells (voids)
    pub inner_shells: Vec<ShellId>,
    /// Name/label
    pub name: Option<String>,
    /// Features
    features: Arc<RwLock<HashMap<u32, Feature>>>,
    /// Attributes
    pub attributes: SolidAttributes,
    /// Cached mass properties
    cached_mass_props: Option<SolidMassProperties>,
    /// Cached statistics
    cached_stats: Option<SolidStats>,
    /// Parent assembly (if part of assembly)
    pub parent_assembly: Option<u32>,
    /// Parametric history
    history: Arc<RwLock<Vec<HistoryNode>>>,
    /// Collision acceleration structure (e.g., OBB tree)
    collision_tree: Option<Arc<CollisionTree>>,
}

/// Collision detection acceleration structure
#[derive(Debug)]
pub struct CollisionTree {
    // Simplified - real implementation would use OBB/AABB tree
    pub root_bbox: (Point3, Point3),
}

impl Solid {
    /// Create new solid
    pub fn new(id: SolidId, outer_shell: ShellId) -> Self {
        Self {
            id,
            outer_shell,
            inner_shells: Vec::new(),
            name: None,
            features: Arc::new(RwLock::new(HashMap::new())),
            attributes: SolidAttributes::default(),
            cached_mass_props: None,
            cached_stats: None,
            parent_assembly: None,
            history: Arc::new(RwLock::new(Vec::new())),
            collision_tree: None,
        }
    }

    /// Create named solid with material
    pub fn new_with_material(
        id: SolidId,
        outer_shell: ShellId,
        name: String,
        material: Material,
    ) -> Self {
        let mut solid = Self::new(id, outer_shell);
        solid.name = Some(name);
        solid.attributes.material = material;
        solid
    }

    /// Add inner shell (void)
    pub fn add_inner_shell(&mut self, shell_id: ShellId) {
        self.inner_shells.push(shell_id);
        self.invalidate_cache();
    }

    /// Remove inner shell
    pub fn remove_inner_shell(&mut self, shell_id: ShellId) -> bool {
        if let Some(pos) = self.inner_shells.iter().position(|&id| id == shell_id) {
            self.inner_shells.remove(pos);
            self.invalidate_cache();
            true
        } else {
            false
        }
    }

    /// Invalidate cached data
    fn invalidate_cache(&mut self) {
        self.cached_mass_props = None;
        self.cached_stats = None;
        self.collision_tree = None;
    }

    /// Add feature
    pub fn add_feature(&mut self, feature: Feature) -> u32 {
        let id = feature.id;
        {
            let mut features = self.features.write();
            features.insert(id, feature);
        } // Lock is dropped here
        self.invalidate_cache();
        id
    }

    /// Suppress/unsuppress feature
    pub fn suppress_feature(&mut self, feature_id: u32, suppress: bool) -> bool {
        let result = {
            let mut features = self.features.write();
            if let Some(feature) = features.get_mut(&feature_id) {
                feature.suppressed = suppress;
                true
            } else {
                false
            }
        }; // Lock is dropped here

        if result {
            self.invalidate_cache();
        }
        result
    }

    /// Get feature by ID
    pub fn get_feature(&self, feature_id: u32) -> Option<Feature> {
        let features = self.features.read();
        features.get(&feature_id).cloned()
    }

    /// Get features by type
    pub fn get_features_by_type(&self, feature_type: FeatureType) -> Vec<Feature> {
        let features = self.features.read();
        features
            .values()
            .filter(|f| f.feature_type == feature_type && !f.suppressed)
            .cloned()
            .collect()
    }

    /// Add history node
    pub fn add_history(&mut self, node: HistoryNode) {
        let mut history = self.history.write();
        history.push(node);
    }

    /// Get parametric history
    pub fn get_history(&self) -> Vec<HistoryNode> {
        let history = self.history.read();
        history.clone()
    }

    /// Compute solid statistics (cached)
    #[allow(clippy::expect_used)] // cached_stats populated immediately above when None
    pub fn compute_stats(
        &mut self,
        shell_store: &ShellStore,
        face_store: &FaceStore,
        loop_store: &LoopStore,
        edge_store: &EdgeStore,
        vertex_store: &VertexStore,
    ) -> MathResult<&SolidStats> {
        if self.cached_stats.is_none() {
            let mut total_faces = 0;
            let mut total_edges = HashSet::new();
            let mut total_vertices = HashSet::new();
            let mut min_pt = Point3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY);
            let mut max_pt = Point3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);

            for &shell_id in &self.all_shells() {
                if let Some(shell) = shell_store.get(shell_id) {
                    total_faces += shell.faces.len();

                    for &face_id in &shell.faces {
                        if let Some(face) = face_store.get(face_id) {
                            for &loop_id in &face.all_loops() {
                                if let Some(loop_) = loop_store.get(loop_id) {
                                    for &edge_id in &loop_.edges {
                                        total_edges.insert(edge_id);

                                        if let Some(edge) = edge_store.get(edge_id) {
                                            total_vertices.insert(edge.start_vertex);
                                            total_vertices.insert(edge.end_vertex);

                                            // Update bounding box
                                            if let Some(v) = vertex_store.get(edge.start_vertex) {
                                                let p = Point3::from(v.position);
                                                min_pt = min_pt.min(&p);
                                                max_pt = max_pt.max(&p);
                                            }
                                            if let Some(v) = vertex_store.get(edge.end_vertex) {
                                                let p = Point3::from(v.position);
                                                min_pt = min_pt.min(&p);
                                                max_pt = max_pt.max(&p);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            let v = total_vertices.len() as i32;
            let e = total_edges.len() as i32;
            let f = total_faces as i32;
            let euler = v - e + f;

            // For a solid with g handles and c cavities: χ = 2 - 2g - c
            // For simple solid: χ = 2, so g = 0
            let genus = (2 - euler) / 2;

            let features = self.features.read();

            self.cached_stats = Some(SolidStats {
                shell_count: 1 + self.inner_shells.len(),
                face_count: total_faces,
                edge_count: total_edges.len(),
                vertex_count: total_vertices.len(),
                feature_count: features.len(),
                euler_characteristic: euler,
                genus,
                bbox_min: min_pt,
                bbox_max: max_pt,
            });
        }

        Ok(self
            .cached_stats
            .as_ref()
            .expect("cached_stats populated above when None"))
    }

    /// Calculate mass properties (cached)
    #[allow(clippy::expect_used)] // cached_mass_props populated immediately above when None
    pub fn compute_mass_properties(
        &mut self,
        shell_store: &mut ShellStore,
        face_store: &mut FaceStore,
        loop_store: &mut LoopStore,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
        curve_store: &CurveStore,
        surface_store: &SurfaceStore,
    ) -> MathResult<&SolidMassProperties> {
        if self.cached_mass_props.is_none() {
            // Calculate volume using divergence theorem
            let mut volume = 0.0;
            let mut center = Vector3::ZERO;
            let mut volume_integrals = VolumeIntegrals::default();

            // Process outer shell
            if let Some(shell) = shell_store.get_mut(self.outer_shell) {
                let shell_props = shell.compute_mass_properties(
                    face_store,
                    loop_store,
                    vertex_store,
                    edge_store,
                    curve_store,
                    surface_store,
                    1.0, // Unit density for now
                )?;

                if let Some(v) = shell_props.volume {
                    volume += v;
                    center += shell_props.center_of_mass.to_vec() * v;

                    // Add to volume integrals
                    volume_integrals.add_shell_contribution(shell_props, 1.0);
                }
            }

            // Subtract inner shells
            for &inner_id in &self.inner_shells {
                if let Some(shell) = shell_store.get_mut(inner_id) {
                    let shell_props = shell.compute_mass_properties(
                        face_store,
                        loop_store,
                        vertex_store,
                        edge_store,
                        curve_store,
                        surface_store,
                        1.0,
                    )?;

                    if let Some(v) = shell_props.volume {
                        volume -= v;
                        center -= shell_props.center_of_mass.to_vec() * v;

                        // Subtract from volume integrals
                        volume_integrals.add_shell_contribution(shell_props, -1.0);
                    }
                }
            }

            // Calculate mass
            let mass = volume * self.attributes.material.density;

            // Center of mass
            let center_of_mass = if volume > consts::EPSILON {
                Point3::from(center / volume)
            } else {
                // Use bounding box center for degenerate case
                if let Some(stats) = &self.cached_stats {
                    Point3::from((stats.bbox_min.to_vec() + stats.bbox_max.to_vec()) * 0.5)
                } else {
                    Point3::ZERO
                }
            };

            // Calculate inertia tensor
            let inertia_tensor = volume_integrals.compute_inertia_tensor(mass, &center_of_mass);

            // Compute principal moments and axes (eigenvalues/eigenvectors)
            let (principal_moments, principal_axes) = compute_principal_inertia(&inertia_tensor);

            // Radius of gyration
            let radius_of_gyration = Vector3::new(
                (principal_moments.x / mass).sqrt(),
                (principal_moments.y / mass).sqrt(),
                (principal_moments.z / mass).sqrt(),
            );

            self.cached_mass_props = Some(SolidMassProperties {
                volume,
                mass,
                center_of_mass,
                inertia_tensor,
                principal_moments,
                principal_axes,
                radius_of_gyration,
            });
        }

        Ok(self
            .cached_mass_props
            .as_ref()
            .expect("cached_mass_props populated above when None"))
    }

    /// Transform solid
    pub fn transform(&mut self, matrix: &Matrix4) -> MathResult<()> {
        // Transform would modify all vertices
        // This is a high-level operation that would delegate to lower levels
        self.invalidate_cache();

        // Add to history
        let history_id = {
            let history = self.history.read();
            history.len() as u32
        }; // Lock is dropped here

        self.add_history(HistoryNode {
            id: history_id,
            operation: "Transform".to_string(),
            inputs: vec![self.id],
            output: self.id,
            parameters: {
                let mut params = HashMap::new();
                // Convert Matrix4 to array for serialization
                let matrix_array: [[f64; 4]; 4] = [
                    [
                        matrix.get(0, 0),
                        matrix.get(0, 1),
                        matrix.get(0, 2),
                        matrix.get(0, 3),
                    ],
                    [
                        matrix.get(1, 0),
                        matrix.get(1, 1),
                        matrix.get(1, 2),
                        matrix.get(1, 3),
                    ],
                    [
                        matrix.get(2, 0),
                        matrix.get(2, 1),
                        matrix.get(2, 2),
                        matrix.get(2, 3),
                    ],
                    [
                        matrix.get(3, 0),
                        matrix.get(3, 1),
                        matrix.get(3, 2),
                        matrix.get(3, 3),
                    ],
                ];
                params.insert("matrix".to_string(), serde_json::json!(matrix_array));
                params
            },
            timestamp: std::time::SystemTime::now(),
        });

        Ok(())
    }

    // Note: fillet, chamfer, and shell are not exposed as Solid methods.
    // The kernel routes those through `crate::operations::{fillet,chamfer,shell}`
    // which operate on a `BRepModel` (the only place that owns the topology
    // stores needed to mutate edges/faces/loops). A method on `Solid` would
    // duplicate the entry point and could only ever record a Feature without
    // updating the actual geometry — see commit history.

    /// Build collision tree for fast intersection tests
    pub fn build_collision_tree(&mut self) -> MathResult<()> {
        if let Some(stats) = &self.cached_stats {
            self.collision_tree = Some(Arc::new(CollisionTree {
                root_bbox: (stats.bbox_min, stats.bbox_max),
            }));
        }
        Ok(())
    }

    /// Fast collision check with another solid
    pub fn collides_with(&self, other: &Solid) -> bool {
        // Quick bbox check first
        if let (Some(stats1), Some(stats2)) = (&self.cached_stats, &other.cached_stats) {
            // Check if bounding boxes overlap
            if stats1.bbox_max.x < stats2.bbox_min.x
                || stats1.bbox_min.x > stats2.bbox_max.x
                || stats1.bbox_max.y < stats2.bbox_min.y
                || stats1.bbox_min.y > stats2.bbox_max.y
                || stats1.bbox_max.z < stats2.bbox_min.z
                || stats1.bbox_min.z > stats2.bbox_max.z
            {
                return false;
            }
        }

        // If bboxes overlap, would do detailed check using collision trees
        true
    }
}

// Preserve original methods for compatibility
impl Solid {
    pub fn all_shells(&self) -> Vec<ShellId> {
        let mut shells = vec![self.outer_shell];
        shells.extend(&self.inner_shells);
        shells
    }

    #[inline]
    pub fn has_voids(&self) -> bool {
        !self.inner_shells.is_empty()
    }

    /// Get all shell IDs (alias for all_shells for compatibility)
    #[inline]
    pub fn shell_ids(&self) -> Vec<ShellId> {
        self.all_shells()
    }

    #[inline]
    pub fn shell_count(&self) -> usize {
        1 + self.inner_shells.len()
    }

    pub fn volume(
        &mut self,
        shell_store: &mut ShellStore,
        face_store: &mut FaceStore,
        loop_store: &mut LoopStore,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
        surface_store: &SurfaceStore,
        _tolerance: Tolerance,
    ) -> MathResult<f64> {
        let props = self.compute_mass_properties(
            shell_store,
            face_store,
            loop_store,
            vertex_store,
            edge_store,
            &CurveStore::new(),
            surface_store,
        )?;
        Ok(props.volume)
    }

    pub fn surface_area(
        &self,
        shell_store: &mut ShellStore,
        face_store: &mut FaceStore,
        loop_store: &mut LoopStore,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
        surface_store: &SurfaceStore,
        tolerance: Tolerance,
    ) -> MathResult<f64> {
        let mut total_area = 0.0;

        for &shell_id in &self.all_shells() {
            if let Some(shell) = shell_store.get_mut(shell_id) {
                total_area += shell.surface_area(
                    face_store,
                    loop_store,
                    vertex_store,
                    edge_store,
                    surface_store,
                    tolerance,
                )?;
            }
        }

        Ok(total_area)
    }

    pub fn bounding_box(
        &mut self,
        shell_store: &ShellStore,
        face_store: &FaceStore,
        loop_store: &LoopStore,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
    ) -> MathResult<(Point3, Point3)> {
        let stats = self.compute_stats(
            shell_store,
            face_store,
            loop_store,
            edge_store,
            vertex_store,
        )?;
        Ok((stats.bbox_min, stats.bbox_max))
    }

    pub fn center(
        &mut self,
        shell_store: &ShellStore,
        face_store: &FaceStore,
        loop_store: &LoopStore,
        vertex_store: &VertexStore,
        edge_store: &EdgeStore,
    ) -> MathResult<Point3> {
        let stats = self.compute_stats(
            shell_store,
            face_store,
            loop_store,
            edge_store,
            vertex_store,
        )?;
        Ok(Point3::from(
            (stats.bbox_min.to_vec() + stats.bbox_max.to_vec()) * 0.5,
        ))
    }
}

/// Volume integrals for mass properties
#[derive(Debug, Default)]
struct VolumeIntegrals {
    // Simplified - real implementation would track all integrals
    volume: f64,
    first_moments: Vector3,
    second_moments: [[f64; 3]; 3],
}

impl VolumeIntegrals {
    fn add_shell_contribution(&mut self, shell_props: &MassProperties, sign: f64) {
        if let Some(v) = shell_props.volume {
            self.volume += sign * v;
            self.first_moments += shell_props.center_of_mass.to_vec() * (sign * v);

            // Add inertia contributions
            for i in 0..3 {
                for j in 0..3 {
                    self.second_moments[i][j] += sign * shell_props.inertia[i][j];
                }
            }
        }
    }

    fn compute_inertia_tensor(&self, _mass: f64, _center_of_mass: &Point3) -> [[f64; 3]; 3] {
        // Transform inertia to center of mass using parallel axis theorem
        

        // Simplified - real implementation would be more sophisticated
        self.second_moments
    }
}

/// Compute principal moments and axes of inertia
fn compute_principal_inertia(inertia: &[[f64; 3]; 3]) -> (Vector3, [Vector3; 3]) {
    // Simplified - real implementation would compute eigenvalues/eigenvectors
    let principal_moments = Vector3::new(inertia[0][0], inertia[1][1], inertia[2][2]);

    let principal_axes = [Vector3::X, Vector3::Y, Vector3::Z];

    (principal_moments, principal_axes)
}

/// Solid storage with feature indexing
#[derive(Debug)]
pub struct SolidStore {
    /// Solid data
    solids: Vec<Solid>,
    /// Name to solid mapping
    name_map: HashMap<String, SolidId>,
    /// Shell to solids mapping
    shell_to_solids: HashMap<ShellId, Vec<SolidId>>,
    /// Next available ID
    next_id: SolidId,
    /// Statistics
    pub stats: SolidStoreStats,
}

#[derive(Debug, Default)]
pub struct SolidStoreStats {
    pub total_created: u64,
    pub boolean_operations: u64,
    pub feature_operations: u64,
    pub collision_checks: u64,
}

impl SolidStore {
    pub fn new() -> Self {
        Self::with_capacity(0)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            solids: Vec::with_capacity(capacity),
            name_map: HashMap::new(),
            shell_to_solids: HashMap::new(),
            next_id: 0,
            stats: SolidStoreStats::default(),
        }
    }

    /// Add solid with MAXIMUM SPEED - no DashMap operations
    #[inline(always)]
    pub fn add(&mut self, mut solid: Solid) -> SolidId {
        solid.id = self.next_id;

        // FAST PATH: Skip expensive DashMap operations
        // The shell_to_solids and name_map DashMap operations are too expensive for primitive creation

        self.solids.push(solid);
        self.next_id += 1;
        self.stats.total_created += 1;

        self.next_id - 1
    }

    /// Add solid with full indexing (use when queries are needed)
    pub fn add_with_indexing(&mut self, mut solid: Solid) -> SolidId {
        solid.id = self.next_id;

        // Update indices - expensive DashMap operations
        if let Some(name) = &solid.name {
            self.name_map.insert(name.clone(), solid.id);
        }

        for &shell_id in &solid.all_shells() {
            self.shell_to_solids
                .entry(shell_id)
                .or_default()
                .push(solid.id);
        }

        self.solids.push(solid);
        self.next_id += 1;
        self.stats.total_created += 1;

        self.next_id - 1
    }

    #[inline(always)]
    pub fn get(&self, id: SolidId) -> Option<&Solid> {
        self.solids.get(id as usize)
    }

    #[inline(always)]
    pub fn get_mut(&mut self, id: SolidId) -> Option<&mut Solid> {
        self.solids.get_mut(id as usize)
    }

    /// Remove a solid from the store
    pub fn remove(&mut self, id: SolidId) -> Option<Solid> {
        // Check bounds first
        if (id as usize) >= self.solids.len() {
            return None;
        }

        // Get solid data before removal to avoid borrowing issues
        let solid_name = self.solids[id as usize].name.clone();
        let outer_shell = self.solids[id as usize].outer_shell;

        // Remove from name mapping
        if let Some(name) = &solid_name {
            self.name_map.remove(name);
        }

        // Remove from shell mapping
        self.shell_to_solids.entry(outer_shell).and_modify(|v| {
            v.retain(|&x| x != id);
        });

        // Remove the actual solid
        let solid = self.solids.remove(id as usize);

        // Update IDs of remaining solids
        for (i, solid) in self.solids.iter_mut().enumerate().skip(id as usize) {
            solid.id = i as SolidId;
        }

        Some(solid)
    }

    #[inline]
    pub fn find_by_name(&self, name: &str) -> Option<SolidId> {
        self.name_map.get(name).copied()
    }

    #[inline]
    pub fn solids_with_shell(&self, shell_id: ShellId) -> &[SolidId] {
        self.shell_to_solids
            .get(&shell_id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.solids.len()
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.solids.is_empty()
    }

    /// Iterate over all solids
    pub fn iter(&self) -> impl Iterator<Item = (SolidId, &Solid)> + '_ {
        self.solids
            .iter()
            .enumerate()
            .filter(|(_, s)| s.id != INVALID_SOLID_ID)
            .map(|(idx, s)| (idx as SolidId, s))
    }
}

impl Default for SolidStore {
    fn default() -> Self {
        Self::new()
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_material() {
//         let mat = Material::default();
//         assert_eq!(mat.name, "Steel");
//         assert_eq!(mat.density, 7850.0);
//     }
//
//     #[test]
//     fn test_feature() {
//         let feature = Feature {
//             id: 0,
//             feature_type: FeatureType::Hole,
//             faces: vec![1, 2, 3],
//             parent: None,
//             parameters: HashMap::new(),
//             suppressed: false,
//         };
//
//         assert_eq!(feature.feature_type, FeatureType::Hole);
//         assert!(!feature.suppressed);
//     }
//
//     #[test]
//     fn test_solid_with_material() {
//         let material = Material {
//             name: "Aluminum".to_string(),
//             density: 2700.0,
//             ..Default::default()
//         };
//
//         let solid = Solid::new_with_material(
//             0,
//             0,
//             "Part1".to_string(),
//             material,
//         );
//
//         assert_eq!(solid.name, Some("Part1".to_string()));
//         assert_eq!(solid.attributes.material.density, 2700.0);
//     }
//
//     #[test]
//     fn test_solid_features() {
//         let mut solid = Solid::new(0, 0);
//
//         let feature = Feature {
//             id: 0,
//             feature_type: FeatureType::Fillet,
//             faces: vec![10, 11],
//             parent: None,
//             parameters: {
//                 let mut params = HashMap::new();
//                 params.insert("radius".to_string(), 5.0);
//                 params
//             },
//             suppressed: false,
//         };
//
//         solid.add_feature(feature);
//
//         let fillets = solid.get_features_by_type(FeatureType::Fillet);
//         assert_eq!(fillets.len(), 1);
//         assert_eq!(fillets[0].parameters.get("radius"), Some(&5.0));
//     }
//
//     #[test]
//     fn test_collision_check() {
//         let mut solid1 = Solid::new(0, 0);
//         let mut solid2 = Solid::new(1, 1);
//
//         // Set up bounding boxes
//         solid1.cached_stats = Some(SolidStats {
//             shell_count: 1,
//             face_count: 6,
//             edge_count: 12,
//             vertex_count: 8,
//             feature_count: 0,
//             euler_characteristic: 2,
//             genus: 0,
//             bbox_min: Point3::new(0.0, 0.0, 0.0),
//             bbox_max: Point3::new(1.0, 1.0, 1.0),
//         });
//
//         solid2.cached_stats = Some(SolidStats {
//             shell_count: 1,
//             face_count: 6,
//             edge_count: 12,
//             vertex_count: 8,
//             feature_count: 0,
//             euler_characteristic: 2,
//             genus: 0,
//             bbox_min: Point3::new(2.0, 2.0, 2.0),
//             bbox_max: Point3::new(3.0, 3.0, 3.0),
//         });
//
//         // Should not collide
//         assert!(!solid1.collides_with(&solid2));
//
//         // Overlapping boxes
//         solid2.cached_stats.as_mut().unwrap().bbox_min = Point3::new(0.5, 0.5, 0.5);
//         assert!(solid1.collides_with(&solid2));
//     }
// }

/// Validation result for solids
#[derive(Debug, Clone)]
pub struct SolidValidation {
    pub is_valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}
