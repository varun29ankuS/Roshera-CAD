//! Assembly module for multi-part CAD models
//!
//! This module provides comprehensive assembly support including:
//! - Part instances and references
//! - Mate constraints between components
//! - Assembly tree management
//! - Motion simulation
//! - Exploded views
//!
//! # Example
//! ```
//! let mut assembly = Assembly::new("Engine Assembly");
//! let piston = assembly.add_part(piston_model, "Piston");
//! let cylinder = assembly.add_part(cylinder_model, "Cylinder");
//!
//! // Add mate constraint
//! assembly.add_mate(
//!     MateType::Concentric,
//!     piston.get_axis("center_axis"),
//!     cylinder.get_axis("bore_axis"),
//! );
//! ```

use crate::math::{Matrix4, Point3, Quaternion, Vector3};
use crate::primitives::topology_builder::BRepModel;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use uuid::Uuid;

/// Unique identifier for assembly components
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ComponentId(pub Uuid);

impl Default for ComponentId {
    fn default() -> Self {
        Self::new()
    }
}

impl ComponentId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

/// Unique identifier for mate constraints
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MateId(pub Uuid);

impl Default for MateId {
    fn default() -> Self {
        Self::new()
    }
}

impl MateId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

/// Assembly structure managing multiple parts
#[derive(Debug, Clone)]
pub struct Assembly {
    /// Assembly name
    pub name: String,
    /// Unique assembly ID
    pub id: Uuid,
    /// Components in the assembly
    components: Arc<DashMap<ComponentId, Component>>,
    /// Mate constraints between components
    mates: Arc<DashMap<MateId, MateConstraint>>,
    /// Assembly tree structure
    tree: Arc<DashMap<ComponentId, Vec<ComponentId>>>,
    /// Root component (usually the base/fixed part)
    root_component: Option<ComponentId>,
    /// Exploded view configuration
    exploded_config: Option<ExplodedViewConfig>,
    /// Motion limits for moving parts
    motion_limits: Arc<DashMap<ComponentId, MotionLimits>>,
    /// Persistent neutral-pose transforms for kinematic mates that
    /// require a fixed reference frame (Gear). Captured at the moment
    /// the mate is created and never overwritten — distinct from the
    /// solver's `initial_transforms`, which is reseeded from the
    /// current state on every relaxation pass.
    ///
    /// Keyed by `MateId`; values are `(neutral_transform_for_component1,
    /// neutral_transform_for_component2)`.
    gear_neutrals: Arc<DashMap<MateId, (Matrix4, Matrix4)>>,
}

/// Component in an assembly (part instance)
#[derive(Debug, Clone)]
pub struct Component {
    /// Component ID
    pub id: ComponentId,
    /// Component name
    pub name: String,
    /// Reference to the actual part geometry
    pub part: Arc<BRepModel>,
    /// Transform matrix (position and orientation)
    pub transform: Matrix4,
    /// Is this component fixed in space?
    pub is_fixed: bool,
    /// Parent component (for sub-assemblies)
    pub parent: Option<ComponentId>,
    /// Component properties
    pub properties: ComponentProperties,
    /// Reference geometry for mating
    pub mate_references: HashMap<String, MateReference>,
    /// Degrees of freedom (0-6)
    pub degrees_of_freedom: u8,
}

/// Component properties
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentProperties {
    /// Mass in kg
    pub mass: Option<f64>,
    /// Material name
    pub material: Option<String>,
    /// Color for visualization
    pub color: Option<[f32; 4]>,
    /// Visibility
    pub visible: bool,
    /// Suppressed (excluded from assembly)
    pub suppressed: bool,
    /// Custom properties
    pub custom: HashMap<String, String>,
}

impl Default for ComponentProperties {
    fn default() -> Self {
        Self {
            mass: None,
            material: None,
            color: None,
            visible: true,
            suppressed: false,
            custom: HashMap::new(),
        }
    }
}

/// Reference geometry for mate constraints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MateReference {
    /// A face on the part
    Face { face_id: Uuid, normal: Vector3 },
    /// An edge on the part
    Edge { edge_id: Uuid, direction: Vector3 },
    /// A vertex/point
    Point { position: Point3 },
    /// An axis (cylindrical features)
    Axis { origin: Point3, direction: Vector3 },
    /// A plane
    Plane { origin: Point3, normal: Vector3 },
}

/// Types of mate constraints
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum MateType {
    /// Faces are coincident (touching)
    Coincident,
    /// Axes/edges are concentric
    Concentric,
    /// Faces/edges are parallel
    Parallel,
    /// Faces/edges are perpendicular
    Perpendicular,
    /// Fixed distance between references
    Distance(f64),
    /// Fixed angle between references
    Angle(f64),
    /// Tangent constraint
    Tangent,
    /// Symmetric about a plane
    Symmetric,
    /// Gear ratio constraint
    Gear { ratio: f64 },
    /// Cam follower constraint
    Cam,
    /// Path constraint (part follows a path)
    Path,
    /// Lock all degrees of freedom
    Lock,
}

/// Mate constraint between components
#[derive(Debug, Clone)]
pub struct MateConstraint {
    /// Unique mate ID
    pub id: MateId,
    /// Mate name
    pub name: String,
    /// Type of mate
    pub mate_type: MateType,
    /// First component and reference
    pub component1: ComponentId,
    pub reference1: String,
    /// Second component and reference
    pub component2: ComponentId,
    pub reference2: String,
    /// Is this mate suppressed?
    pub suppressed: bool,
    /// Flip alignment
    pub flip: bool,
    /// Solved state
    pub solved: bool,
    /// Error if constraint cannot be satisfied
    pub error: Option<String>,
}

/// Motion limits for components
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MotionLimits {
    /// Linear motion limits (min, max) along each axis
    pub linear: Option<[(f64, f64); 3]>,
    /// Rotational limits (min, max) around each axis in radians
    pub angular: Option<[(f64, f64); 3]>,
    /// Spring constant for elastic connections
    pub spring_constant: Option<f64>,
    /// Damping coefficient
    pub damping: Option<f64>,
}

/// Exploded view configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplodedViewConfig {
    /// Explosion steps
    pub steps: Vec<ExplosionStep>,
    /// Current step index
    pub current_step: usize,
    /// Auto-explode along assembly sequence
    pub auto_explode: bool,
    /// Explosion scale factor
    pub scale: f64,
}

/// Single step in exploded view animation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplosionStep {
    /// Component to move
    pub component: ComponentId,
    /// Translation vector
    pub translation: Vector3,
    /// Rotation (optional)
    pub rotation: Option<Quaternion>,
    /// Duration in seconds
    pub duration: f64,
}

impl Assembly {
    /// Create a new assembly
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            id: Uuid::new_v4(),
            components: Arc::new(DashMap::new()),
            mates: Arc::new(DashMap::new()),
            tree: Arc::new(DashMap::new()),
            root_component: None,
            exploded_config: None,
            motion_limits: Arc::new(DashMap::new()),
            gear_neutrals: Arc::new(DashMap::new()),
        }
    }

    /// Add a part to the assembly
    pub fn add_part(&mut self, part: Arc<BRepModel>, name: impl Into<String>) -> ComponentId {
        let id = ComponentId::new();
        let component = Component {
            id,
            name: name.into(),
            part,
            transform: Matrix4::IDENTITY,
            is_fixed: self.root_component.is_none(), // First part is fixed by default
            parent: None,
            properties: ComponentProperties::default(),
            mate_references: HashMap::new(),
            degrees_of_freedom: 6, // Start with all DOF
        };

        if self.root_component.is_none() {
            self.root_component = Some(id);
        }

        self.components.insert(id, component);
        self.tree.insert(id, Vec::new());

        id
    }

    /// Add a sub-assembly
    pub fn add_subassembly(
        &mut self,
        subassembly: Assembly,
        name: impl Into<String>,
        parent: Option<ComponentId>,
    ) -> ComponentId {
        // Create a parent component for the sub-assembly
        let parent_id = ComponentId::new();
        let parent_component = Component {
            id: parent_id,
            name: name.into(),
            part: Arc::new(BRepModel::new()), // Empty container for sub-assembly
            transform: Matrix4::IDENTITY,
            is_fixed: false,
            parent,
            properties: ComponentProperties::default(),
            mate_references: HashMap::new(),
            degrees_of_freedom: 6,
        };

        self.components.insert(parent_id, parent_component);

        // Add all components from the sub-assembly
        for component in subassembly.components.iter() {
            let mut sub_component = component.clone();
            sub_component.parent = Some(parent_id);
            let sub_id = ComponentId::new();
            sub_component.id = sub_id;
            self.components.insert(sub_id, sub_component);

            // Update tree structure
            self.tree
                .entry(parent_id)
                .or_default()
                .push(sub_id);
        }

        // Add all mates from the sub-assembly
        for mate in subassembly.mates.iter() {
            self.mates.insert(MateId::new(), mate.clone());
        }

        parent_id
    }

    /// Add a mate constraint
    pub fn add_mate(
        &mut self,
        mate_type: MateType,
        component1: ComponentId,
        reference1: impl Into<String>,
        component2: ComponentId,
        reference2: impl Into<String>,
    ) -> Result<MateId, AssemblyError> {
        // Validate components exist
        if !self.components.contains_key(&component1) {
            return Err(AssemblyError::ComponentNotFound(component1));
        }
        if !self.components.contains_key(&component2) {
            return Err(AssemblyError::ComponentNotFound(component2));
        }

        let id = MateId::new();
        let mate = MateConstraint {
            id,
            name: format!("{:?} Mate", mate_type),
            mate_type,
            component1,
            reference1: reference1.into(),
            component2,
            reference2: reference2.into(),
            suppressed: false,
            flip: false,
            solved: false,
            error: None,
        };

        self.mates.insert(id, mate);

        // For Gear mates, capture the components' current transforms as
        // the persistent neutral pose. This must be frozen at mate
        // creation; the solver's `initial_transforms` is reseeded each
        // solve pass and would otherwise erase the gear reference.
        if matches!(mate_type, MateType::Gear { .. }) {
            let t1 = self
                .components
                .get(&component1)
                .map(|c| c.transform)
                .unwrap_or(Matrix4::IDENTITY);
            let t2 = self
                .components
                .get(&component2)
                .map(|c| c.transform)
                .unwrap_or(Matrix4::IDENTITY);
            self.gear_neutrals.insert(id, (t1, t2));
        }

        // Solve the constraint system immediately so the assembly state
        // reflects the new mate. The solver tolerates mates whose named
        // references are not yet registered on their components (common
        // during incremental assembly construction) — such mates record
        // a descriptive error on the `MateConstraint::error` field and
        // leave the corresponding component transform unchanged.
        self.solve_constraints()?;

        Ok(id)
    }

    /// Solve all mate constraints.
    ///
    /// Walks the active (non-suppressed) mates and runs a Gauss-Seidel
    /// rigid-body relaxation starting from the components' current
    /// transforms. Components marked `is_fixed` are anchors; free
    /// components are moved to satisfy each constraint in world space.
    ///
    /// Mates whose named references are not registered on their components,
    /// or whose reference combinations do not carry sufficient geometric
    /// data for the constraint type (e.g. Coincident between two Edges,
    /// which lack origins), are recorded on `MateConstraint::error` and
    /// skipped — they never cause the solve to fail overall.
    pub fn solve_constraints(&mut self) -> Result<(), AssemblyError> {
        let mut solver = ConstraintSolver::new();

        // Seed initial transforms and the fixed-anchor set from the
        // current component state.
        for component in self.components.iter() {
            solver
                .initial_transforms
                .insert(component.id, component.transform);
            if component.is_fixed {
                solver.fixed_components.insert(component.id);
            }
        }

        // Copy persistent gear-neutral transforms (frozen at mate
        // creation, never overwritten by solve passes).
        for entry in self.gear_neutrals.iter() {
            solver
                .gear_neutrals
                .insert(*entry.key(), *entry.value());
        }

        // Register active mates with pre-resolved local-frame references.
        for mate in self.mates.iter() {
            if mate.suppressed {
                continue;
            }
            let comp1 = self
                .components
                .get(&mate.component1)
                .ok_or(AssemblyError::ComponentNotFound(mate.component1))?;
            let comp2 = self
                .components
                .get(&mate.component2)
                .ok_or(AssemblyError::ComponentNotFound(mate.component2))?;
            solver.add_constraint(&mate, &comp1, &comp2)?;
        }

        // Run the relaxation solver.
        let solve_report = solver.solve()?;

        // Apply computed transforms and DOF back to the components.
        for (component_id, transform) in solve_report.transforms {
            if let Some(mut component) = self.components.get_mut(&component_id) {
                component.transform = transform;
                component.degrees_of_freedom = solver.get_dof(&component_id);
            }
        }

        // Propagate per-mate solve status (solved flag + optional error
        // message) back onto the stored constraints.
        for (mate_id, status) in solve_report.mate_status {
            if let Some(mut mate) = self.mates.get_mut(&mate_id) {
                mate.solved = status.solved;
                mate.error = status.error;
            }
        }

        Ok(())
    }

    /// Get component by ID
    pub fn get_component(&self, id: ComponentId) -> Option<Component> {
        self.components.get(&id).map(|c| c.clone())
    }

    /// Set component transform
    pub fn set_component_transform(
        &mut self,
        id: ComponentId,
        transform: Matrix4,
    ) -> Result<(), AssemblyError> {
        // Update the component transform
        self.components
            .get_mut(&id)
            .ok_or(AssemblyError::ComponentNotFound(id))?
            .transform = transform;

        // Solve constraints after updating
        self.solve_constraints()?;
        Ok(())
    }

    /// Create exploded view
    pub fn create_exploded_view(&mut self, auto: bool) -> ExplodedViewConfig {
        let mut steps = Vec::new();

        if auto {
            // Auto-generate explosion based on assembly sequence
            for component in self.components.iter() {
                if component.id != self.root_component.unwrap_or(ComponentId::new()) {
                    // Calculate explosion direction based on component position
                    let center = self.get_assembly_center();
                    let comp_pos = Point3::from(component.transform.translation_vector());
                    let diff = Point3::new(
                        comp_pos.x - center.x,
                        comp_pos.y - center.y,
                        comp_pos.z - center.z,
                    );
                    let length = (diff.x * diff.x + diff.y * diff.y + diff.z * diff.z).sqrt();
                    let direction = if length > 0.0 {
                        Vector3::new(diff.x / length, diff.y / length, diff.z / length)
                    } else {
                        Vector3::new(1.0, 0.0, 0.0)
                    };

                    steps.push(ExplosionStep {
                        component: component.id,
                        translation: direction * 100.0, // 100mm explosion
                        rotation: None,
                        duration: 1.0,
                    });
                }
            }
        }

        let config = ExplodedViewConfig {
            steps,
            current_step: 0,
            auto_explode: auto,
            scale: 1.0,
        };

        self.exploded_config = Some(config.clone());
        config
    }

    /// Get assembly bounding box
    pub fn get_bounding_box(&self) -> Option<([f64; 3], [f64; 3])> {
        let min = [f64::MAX; 3];
        let max = [f64::MIN; 3];
        let mut has_geometry = false;

        for component in self.components.iter() {
            if !component.properties.suppressed {
                // Get component bounds and transform them
                // This would use the actual BRep bounds
                has_geometry = true;
                // Update min/max based on transformed bounds
            }
        }

        if has_geometry {
            Some((min, max))
        } else {
            None
        }
    }

    /// Get assembly center of mass
    fn get_assembly_center(&self) -> Point3 {
        let mut weighted_sum_x = 0.0;
        let mut weighted_sum_y = 0.0;
        let mut weighted_sum_z = 0.0;
        let mut total_mass = 0.0;

        for component in self.components.iter() {
            let mass = component.properties.mass.unwrap_or(1.0);
            let transform = component.transform;
            let position = Point3::new(transform[(0, 3)], transform[(1, 3)], transform[(2, 3)]);
            weighted_sum_x += position.x * mass;
            weighted_sum_y += position.y * mass;
            weighted_sum_z += position.z * mass;
            total_mass += mass;
        }

        if total_mass > 0.0 {
            Point3::new(
                weighted_sum_x / total_mass,
                weighted_sum_y / total_mass,
                weighted_sum_z / total_mass,
            )
        } else {
            Point3::new(0.0, 0.0, 0.0)
        }
    }

    /// Check for interferences between components
    pub fn check_interferences(&self) -> Vec<(ComponentId, ComponentId)> {
        let mut interferences = Vec::new();
        let components: Vec<_> = self.components.iter().map(|c| c.clone()).collect();

        for i in 0..components.len() {
            for j in (i + 1)..components.len() {
                if self.components_interfere(&components[i], &components[j]) {
                    interferences.push((components[i].id, components[j].id));
                }
            }
        }

        interferences
    }

    /// Check if two components interfere
    fn components_interfere(&self, _comp1: &Component, _comp2: &Component) -> bool {
        // Not yet implemented — requires bounding box computation + boolean intersection
        false
    }

    /// Get iterator over components
    pub fn components(
        &self,
    ) -> impl Iterator<Item = dashmap::mapref::multiple::RefMulti<'_, ComponentId, Component>> + '_
    {
        self.components.iter()
    }

    /// Get iterator over mates
    pub fn mates(
        &self,
    ) -> impl Iterator<Item = dashmap::mapref::multiple::RefMulti<'_, MateId, MateConstraint>> + '_
    {
        self.mates.iter()
    }

    /// Simulate motion based on constraints
    pub fn simulate_motion(
        &mut self,
        component: ComponentId,
        delta: Vector3,
        delta_rotation: Option<Quaternion>,
    ) -> Result<(), AssemblyError> {
        // Apply motion while respecting constraints
        if let Some(mut comp) = self.components.get_mut(&component) {
            // Check motion limits
            if let Some(_limits) = self.motion_limits.get(&component) {
                // Validate motion against limits
            }

            // Apply transform
            let mut new_transform = comp.transform;
            // Apply translation
            new_transform[(0, 3)] += delta.x;
            new_transform[(1, 3)] += delta.y;
            new_transform[(2, 3)] += delta.z;

            if let Some(rotation) = delta_rotation {
                // Post-multiply: rotation applied in the component's local
                // frame, after the translation column has been updated.
                // Standard rigid-body convention for incremental motion.
                new_transform *= rotation.to_matrix4();
            }
            comp.transform = new_transform;
        }

        // Re-solve constraints to update dependent components
        self.solve_constraints()?;

        Ok(())
    }
}

/// Mate constraint with references pre-resolved from the owning
/// components' `mate_references` maps. References are stored in each
/// component's **local** frame; the solver transforms them into world
/// space with the current component transforms during relaxation.
#[derive(Debug, Clone)]
struct ResolvedConstraint {
    mate_id: MateId,
    mate_type: MateType,
    flip: bool,
    component1: ComponentId,
    component2: ComponentId,
    /// Local-frame reference on component1, or `None` if the named
    /// reference was not registered on the component.
    ref1: Option<MateReference>,
    /// Local-frame reference on component2, or `None` if the named
    /// reference was not registered on the component.
    ref2: Option<MateReference>,
    /// Original reference names, preserved for diagnostic messages.
    name1: String,
    name2: String,
}

/// Per-mate outcome produced by `ConstraintSolver::solve`.
#[derive(Debug, Clone)]
struct MateStatus {
    solved: bool,
    error: Option<String>,
}

/// Aggregate report returned by the solver: final transforms for every
/// component it was seeded with, plus per-mate solve status.
#[derive(Debug, Clone)]
struct SolveReport {
    transforms: HashMap<ComponentId, Matrix4>,
    mate_status: HashMap<MateId, MateStatus>,
}

/// Rigid-body constraint solver for assembly mates.
///
/// The solver uses Gauss-Seidel relaxation: on each iteration it visits
/// every (non-suppressed) constraint, computes the world-space correction
/// that satisfies that constraint given the current transforms, and
/// applies it to the non-fixed component. Iteration halts when the
/// largest per-component frame change falls below `TRANSLATION_TOLERANCE`
/// and `ROTATION_TOLERANCE`, or when `MAX_ITERATIONS` is reached.
///
/// Design notes:
/// - Fixed components are never moved; if both endpoints of a mate are
///   fixed the mate becomes a consistency assertion and is flagged but
///   not an error (consistent with CATIA behavior).
/// - References that lack the geometric data a given MateType requires
///   (e.g. Coincident between two Edges, which carry no origin) are
///   recorded as unsolved with a descriptive message and skipped.
/// - Higher-order kinematic mates (Gear, Cam, Path, Symmetric, Tangent)
///   are outside the scope of this rigid-body relaxation solver and
///   report that explicitly; they are registered (their DOF bookkeeping
///   still runs) but the solver does not attempt to satisfy them.
struct ConstraintSolver {
    constraints: Vec<ResolvedConstraint>,
    component_dof: HashMap<ComponentId, u8>,
    initial_transforms: HashMap<ComponentId, Matrix4>,
    fixed_components: HashSet<ComponentId>,
    /// Per-mate persistent neutral transforms (Gear). See
    /// `Assembly::gear_neutrals`.
    gear_neutrals: HashMap<MateId, (Matrix4, Matrix4)>,
}

/// Maximum Gauss-Seidel iterations before the solver gives up.
const MAX_ITERATIONS: usize = 64;
/// Translation convergence tolerance (world units, typically mm).
const TRANSLATION_TOLERANCE: f64 = 1e-9;
/// Rotation convergence tolerance (radians).
const ROTATION_TOLERANCE: f64 = 1e-10;

impl ConstraintSolver {
    fn new() -> Self {
        Self {
            constraints: Vec::new(),
            component_dof: HashMap::new(),
            initial_transforms: HashMap::new(),
            fixed_components: HashSet::new(),
            gear_neutrals: HashMap::new(),
        }
    }

    /// Register a mate. Resolves the named references against the two
    /// owning components and caches the local-frame geometry.
    fn add_constraint(
        &mut self,
        mate: &MateConstraint,
        comp1: &Component,
        comp2: &Component,
    ) -> Result<(), AssemblyError> {
        let ref1 = comp1.mate_references.get(&mate.reference1).cloned();
        let ref2 = comp2.mate_references.get(&mate.reference2).cloned();

        self.constraints.push(ResolvedConstraint {
            mate_id: mate.id,
            mate_type: mate.mate_type,
            flip: mate.flip,
            component1: mate.component1,
            component2: mate.component2,
            ref1,
            ref2,
            name1: mate.reference1.clone(),
            name2: mate.reference2.clone(),
        });

        // Track DOF consumed by this constraint type. Over-constrained
        // assemblies saturate at 0 rather than underflowing.
        let dof_removed: u8 = match mate.mate_type {
            MateType::Lock => 6,
            MateType::Coincident => 3,
            MateType::Concentric => 4,
            MateType::Parallel => 2,
            MateType::Perpendicular => 2,
            MateType::Tangent => 1,
            MateType::Distance(_) => 1,
            MateType::Angle(_) => 1,
            MateType::Symmetric => 3,
            MateType::Gear { .. } => 1,
            MateType::Cam => 1,
            MateType::Path => 2,
        };

        for comp in [mate.component1, mate.component2] {
            let entry = self.component_dof.entry(comp).or_insert(6);
            *entry = entry.saturating_sub(dof_removed);
        }

        Ok(())
    }

    /// Run the relaxation and return final transforms plus per-mate
    /// status.
    fn solve(&self) -> Result<SolveReport, AssemblyError> {
        let mut transforms = self.initial_transforms.clone();
        let mut mate_status: HashMap<MateId, MateStatus> = HashMap::new();

        // Sticky status across iterations — a mate that is impossible
        // to satisfy (e.g. unresolved references) stays that way.
        for c in &self.constraints {
            mate_status.insert(
                c.mate_id,
                MateStatus {
                    solved: false,
                    error: None,
                },
            );
        }

        for _iteration in 0..MAX_ITERATIONS {
            let mut max_translation_delta: f64 = 0.0;
            let mut max_rotation_delta: f64 = 0.0;

            for constraint in &self.constraints {
                let outcome = self.apply_constraint(constraint, &mut transforms);
                match outcome {
                    ConstraintOutcome::Satisfied {
                        translation_delta,
                        rotation_delta,
                    } => {
                        if translation_delta > max_translation_delta {
                            max_translation_delta = translation_delta;
                        }
                        if rotation_delta > max_rotation_delta {
                            max_rotation_delta = rotation_delta;
                        }
                        if let Some(status) = mate_status.get_mut(&constraint.mate_id) {
                            status.solved = true;
                            status.error = None;
                        }
                    }
                    ConstraintOutcome::Unresolvable(msg) => {
                        if let Some(status) = mate_status.get_mut(&constraint.mate_id) {
                            status.solved = false;
                            status.error = Some(msg);
                        }
                    }
                }
            }

            if max_translation_delta < TRANSLATION_TOLERANCE
                && max_rotation_delta < ROTATION_TOLERANCE
            {
                break;
            }
        }

        Ok(SolveReport {
            transforms,
            mate_status,
        })
    }

    /// Apply a single constraint's correction to the movable component's
    /// transform. Returns the translation/rotation magnitude of the
    /// correction (used for convergence detection), or an unresolvable
    /// reason.
    fn apply_constraint(
        &self,
        c: &ResolvedConstraint,
        transforms: &mut HashMap<ComponentId, Matrix4>,
    ) -> ConstraintOutcome {
        // Gate on missing references first — this is the common case
        // during incremental assembly construction.
        let (ref1, ref2) = match (&c.ref1, &c.ref2) {
            (Some(r1), Some(r2)) => (r1, r2),
            (None, _) => {
                return ConstraintOutcome::Unresolvable(format!(
                    "mate reference '{}' not registered on component {:?}",
                    c.name1, c.component1
                ));
            }
            (_, None) => {
                return ConstraintOutcome::Unresolvable(format!(
                    "mate reference '{}' not registered on component {:?}",
                    c.name2, c.component2
                ));
            }
        };

        // Identify the movable component. If both are fixed, the mate
        // is a pure consistency assertion and we leave transforms alone.
        let c1_fixed = self.fixed_components.contains(&c.component1);
        let c2_fixed = self.fixed_components.contains(&c.component2);
        let movable = match (c1_fixed, c2_fixed) {
            (true, true) => {
                return ConstraintOutcome::Unresolvable(
                    "both components fixed — mate cannot drive any transform".to_string(),
                );
            }
            (true, false) => c.component2,
            (false, true) => c.component1,
            // Neither fixed: move component2 (downstream convention),
            // keeping component1 as the local anchor for this iteration.
            (false, false) => c.component2,
        };

        let t1 = match transforms.get(&c.component1) {
            Some(t) => *t,
            None => {
                return ConstraintOutcome::Unresolvable(format!(
                    "component {:?} transform missing from solve state",
                    c.component1
                ));
            }
        };
        let t2 = match transforms.get(&c.component2) {
            Some(t) => *t,
            None => {
                return ConstraintOutcome::Unresolvable(format!(
                    "component {:?} transform missing from solve state",
                    c.component2
                ));
            }
        };

        // Compute the correction transform (in world space) that, when
        // pre-multiplied onto the movable component's current transform,
        // brings it into compliance with the constraint.
        let correction = match compute_correction(
            c,
            &t1,
            &t2,
            ref1,
            ref2,
            movable,
            &self.gear_neutrals,
        ) {
            Ok(Some(delta)) => delta,
            Ok(None) => {
                return ConstraintOutcome::Satisfied {
                    translation_delta: 0.0,
                    rotation_delta: 0.0,
                };
            }
            Err(msg) => return ConstraintOutcome::Unresolvable(msg),
        };

        // Measure the correction magnitude for convergence.
        let translation_delta = correction.translation.magnitude();
        let rotation_delta = correction.rotation_angle.abs();

        // Apply: new = T_correction * T_old (world-frame pre-multiply).
        let correction_matrix = correction.to_matrix();
        let old = transforms
            .get(&movable)
            .cloned()
            .unwrap_or(Matrix4::IDENTITY);
        let new_transform = correction_matrix * old;
        transforms.insert(movable, new_transform);

        ConstraintOutcome::Satisfied {
            translation_delta,
            rotation_delta,
        }
    }

    fn get_dof(&self, component: &ComponentId) -> u8 {
        self.component_dof.get(component).copied().unwrap_or(6)
    }
}

enum ConstraintOutcome {
    Satisfied {
        translation_delta: f64,
        rotation_delta: f64,
    },
    Unresolvable(String),
}

/// A rigid-body correction in world space: first rotate about `rotation_axis`
/// through `rotation_angle` at `rotation_pivot`, then translate by `translation`.
#[derive(Debug, Clone)]
struct RigidCorrection {
    rotation_axis: Vector3,
    rotation_angle: f64,
    rotation_pivot: Point3,
    translation: Vector3,
}

impl RigidCorrection {
    fn pure_translation(t: Vector3) -> Self {
        Self {
            rotation_axis: Vector3::new(0.0, 0.0, 1.0),
            rotation_angle: 0.0,
            rotation_pivot: Point3::new(0.0, 0.0, 0.0),
            translation: t,
        }
    }

    fn pure_rotation(axis: Vector3, angle: f64, pivot: Point3) -> Self {
        Self {
            rotation_axis: axis,
            rotation_angle: angle,
            rotation_pivot: pivot,
            translation: Vector3::new(0.0, 0.0, 0.0),
        }
    }

    /// Expand to a 4×4 world-frame transform:
    ///   M = T(translation) · T(pivot) · R(axis, angle) · T(-pivot)
    fn to_matrix(&self) -> Matrix4 {
        let rot = if self.rotation_angle.abs() < 1e-14
            || self.rotation_axis.magnitude() < 1e-14
        {
            Matrix4::IDENTITY
        } else {
            let q = match Quaternion::from_axis_angle(&self.rotation_axis, self.rotation_angle) {
                Ok(q) => q,
                Err(_) => return Matrix4::from_translation(&self.translation),
            };
            q.to_matrix4()
        };
        let to_origin = Matrix4::from_translation(&Vector3::new(
            -self.rotation_pivot.x,
            -self.rotation_pivot.y,
            -self.rotation_pivot.z,
        ));
        let from_origin = Matrix4::from_translation(&Vector3::new(
            self.rotation_pivot.x,
            self.rotation_pivot.y,
            self.rotation_pivot.z,
        ));
        let translate = Matrix4::from_translation(&self.translation);
        translate * from_origin * rot * to_origin
    }
}

/// Extract the world-space (origin, direction) representation of a mate
/// reference given the owning component's current world transform.
/// Returns `(Some(origin), Some(direction))` when both are available;
/// Face/Edge carry only a direction, Point carries only an origin.
fn world_origin_direction(
    reference: &MateReference,
    transform: &Matrix4,
) -> (Option<Point3>, Option<Vector3>) {
    match reference {
        MateReference::Face { normal, .. } => (None, Some(transform.transform_vector(normal))),
        MateReference::Edge { direction, .. } => {
            (None, Some(transform.transform_vector(direction)))
        }
        MateReference::Point { position } => (Some(transform.transform_point(position)), None),
        MateReference::Axis { origin, direction } => (
            Some(transform.transform_point(origin)),
            Some(transform.transform_vector(direction)),
        ),
        MateReference::Plane { origin, normal } => (
            Some(transform.transform_point(origin)),
            Some(transform.transform_vector(normal)),
        ),
    }
}

/// Compute the world-space correction required on the `movable` component
/// so that this constraint is satisfied, given the other component's
/// current transform as the anchor.
///
/// Returns `Ok(None)` if the constraint is already satisfied (no move),
/// `Ok(Some(correction))` with the world-frame rigid motion to apply to
/// the movable component, or `Err(message)` if the reference combination
/// is not geometrically viable for this constraint type.
fn compute_correction(
    c: &ResolvedConstraint,
    t1: &Matrix4,
    t2: &Matrix4,
    ref1: &MateReference,
    ref2: &MateReference,
    movable: ComponentId,
    gear_neutrals: &HashMap<MateId, (Matrix4, Matrix4)>,
) -> Result<Option<RigidCorrection>, String> {
    // `a` = the side we keep fixed for this correction (the anchor);
    // `b` = the movable side. We always move the movable component's
    // reference to align with the anchor's reference.
    let (anchor_ref, anchor_t, movable_ref, movable_t) = if movable == c.component2 {
        (ref1, t1, ref2, t2)
    } else {
        (ref2, t2, ref1, t1)
    };

    let (anchor_origin, anchor_dir) = world_origin_direction(anchor_ref, anchor_t);
    let (movable_origin, movable_dir) = world_origin_direction(movable_ref, movable_t);

    // Flip inverts the sign convention for direction-based constraints.
    let sign = if c.flip { -1.0 } else { 1.0 };

    match c.mate_type {
        MateType::Lock => {
            // Lock: copy the anchor component's full world transform onto
            // the movable component. Correction = T_anchor · T_movable^-1.
            let anchor_t_abs = if movable == c.component2 { t1 } else { t2 };
            let movable_t_abs = if movable == c.component2 { t2 } else { t1 };
            let inv = movable_t_abs.inverse().map_err(|_| {
                "Lock mate: movable component transform is non-invertible".to_string()
            })?;
            let delta_matrix = *anchor_t_abs * inv;
            Ok(Some(matrix_to_correction(&delta_matrix)?))
        }

        MateType::Coincident => {
            // Coincident: both reference origins lie on both reference
            // planes. Requires at least one origin and (for planar
            // coincidence) aligned antiparallel normals.
            align_plane_like(
                anchor_origin,
                anchor_dir,
                movable_origin,
                movable_dir,
                sign,
                /* antiparallel = */ true,
            )
        }

        MateType::Concentric => {
            // Concentric: axes colinear. Needs origin+direction on both.
            let ao = anchor_origin.ok_or_else(|| {
                "Concentric mate requires an origin on the anchor reference".to_string()
            })?;
            let ad = anchor_dir.ok_or_else(|| {
                "Concentric mate requires a direction on the anchor reference".to_string()
            })?;
            let mo = movable_origin.ok_or_else(|| {
                "Concentric mate requires an origin on the movable reference".to_string()
            })?;
            let md = movable_dir.ok_or_else(|| {
                "Concentric mate requires a direction on the movable reference".to_string()
            })?;
            concentric_correction(ao, ad, mo, md, sign)
        }

        MateType::Parallel => {
            // Parallel: align directions. Pure rotation.
            let ad = anchor_dir.ok_or_else(|| {
                "Parallel mate requires a direction on the anchor reference".to_string()
            })?;
            let md = movable_dir.ok_or_else(|| {
                "Parallel mate requires a direction on the movable reference".to_string()
            })?;
            let pivot = movable_origin.unwrap_or_else(|| Point3::new(0.0, 0.0, 0.0));
            align_directions(ad, md, sign, pivot, /* antiparallel = */ false)
        }

        MateType::Perpendicular => {
            // Perpendicular: rotate so directions have zero dot product.
            let ad = anchor_dir.ok_or_else(|| {
                "Perpendicular mate requires a direction on the anchor reference".to_string()
            })?;
            let md = movable_dir.ok_or_else(|| {
                "Perpendicular mate requires a direction on the movable reference".to_string()
            })?;
            let pivot = movable_origin.unwrap_or_else(|| Point3::new(0.0, 0.0, 0.0));
            perpendicular_correction(ad, md, pivot)
        }

        MateType::Distance(d) => {
            let ao = anchor_origin.ok_or_else(|| {
                "Distance mate requires an origin on the anchor reference".to_string()
            })?;
            let mo = movable_origin.ok_or_else(|| {
                "Distance mate requires an origin on the movable reference".to_string()
            })?;
            let current = Vector3::new(mo.x - ao.x, mo.y - ao.y, mo.z - ao.z);
            let current_len = current.magnitude();
            if current_len < 1e-14 {
                // Degenerate — pick the anchor's direction if available,
                // else world +X.
                let dir = anchor_dir
                    .or(movable_dir)
                    .unwrap_or(Vector3::new(1.0, 0.0, 0.0));
                let dir = dir.normalize().map_err(|e| e.to_string())?;
                return Ok(Some(RigidCorrection::pure_translation(Vector3::new(
                    dir.x * d,
                    dir.y * d,
                    dir.z * d,
                ))));
            }
            let scale = (d - current_len) / current_len;
            Ok(Some(RigidCorrection::pure_translation(Vector3::new(
                current.x * scale,
                current.y * scale,
                current.z * scale,
            ))))
        }

        MateType::Angle(target) => {
            let ad = anchor_dir.ok_or_else(|| {
                "Angle mate requires a direction on the anchor reference".to_string()
            })?;
            let md = movable_dir.ok_or_else(|| {
                "Angle mate requires a direction on the movable reference".to_string()
            })?;
            let pivot = movable_origin.unwrap_or_else(|| Point3::new(0.0, 0.0, 0.0));
            angle_correction(ad, md, target, pivot)
        }

        MateType::Symmetric => {
            // Mirror across world XY plane: enforces movable.origin =
            // (anchor.x, anchor.y, -anchor.z). Pure translational
            // constraint that locks all three coordinates.
            symmetric_correction(anchor_origin, movable_origin)
        }

        MateType::Tangent => {
            // Movable origin lies on the anchor's plane (planar tangency).
            // For non-planar tangency (cylinder-plane, sphere-plane), the
            // anchor's reference is treated as the contacting plane via
            // its normal/direction. The follower contact-point's
            // perpendicular distance to that plane is driven to zero.
            tangent_correction(anchor_origin, anchor_dir, movable_origin)
        }

        MateType::Gear { ratio } => {
            // Couples the rotational positions of the two components
            // about their respective reference axes:
            //     theta_movable = -ratio * theta_anchor
            // measured from the gear's persistent neutral pose
            // (captured at mate-creation time and never overwritten).
            gear_correction(c, t1, t2, ref1, ref2, movable, gear_neutrals, ratio)
        }

        MateType::Cam => {
            // Cam-follower: the follower contact-point (movable origin)
            // remains in contact with the cam surface. With the current
            // 2-reference API and no explicit cam-profile parameterization,
            // this reduces to the planar-tangent case using anchor's
            // direction as the local cam-surface normal.
            cam_correction(anchor_origin, anchor_dir, movable_origin)
        }

        MateType::Path => {
            // Path mate: movable origin is constrained to lie on the line
            // (origin, direction) carried by the anchor reference. Two
            // perpendicular-distance components are driven to zero,
            // leaving one free DOF along the path direction.
            path_correction(anchor_origin, anchor_dir, movable_origin)
        }
    }
}

/// Decompose a rigid transform matrix into a RigidCorrection (axis-angle
/// rotation about the world origin plus translation). Used for Lock mates.
fn matrix_to_correction(m: &Matrix4) -> Result<RigidCorrection, String> {
    let translation = m.translation_vector();
    let q = Quaternion::from_matrix4(m);
    let normalized = q.normalize().map_err(|e| e.to_string())?;
    // Extract axis-angle from unit quaternion: angle = 2·acos(w).
    let w = normalized.w.clamp(-1.0, 1.0);
    let angle = 2.0 * w.acos();
    let s = (1.0 - w * w).sqrt();
    let axis = if s < 1e-9 {
        Vector3::new(1.0, 0.0, 0.0)
    } else {
        Vector3::new(normalized.x / s, normalized.y / s, normalized.z / s)
    };
    Ok(RigidCorrection {
        rotation_axis: axis,
        rotation_angle: angle,
        rotation_pivot: Point3::new(0.0, 0.0, 0.0),
        translation,
    })
}

/// Rotate `movable_dir` onto `anchor_dir` (or its negation if
/// `antiparallel`), rotating about `pivot`. `sign` allows caller flipping.
fn align_directions(
    anchor_dir: Vector3,
    movable_dir: Vector3,
    sign: f64,
    pivot: Point3,
    antiparallel: bool,
) -> Result<Option<RigidCorrection>, String> {
    let anchor_n = anchor_dir.normalize().map_err(|e| e.to_string())?;
    let movable_n = movable_dir.normalize().map_err(|e| e.to_string())?;

    let target = if antiparallel {
        Vector3::new(-anchor_n.x * sign, -anchor_n.y * sign, -anchor_n.z * sign)
    } else {
        Vector3::new(anchor_n.x * sign, anchor_n.y * sign, anchor_n.z * sign)
    };

    let q = Quaternion::from_rotation_between(&movable_n, &target)
        .map_err(|e| e.to_string())?;

    // Convert quaternion to axis-angle.
    let w = q.w.clamp(-1.0, 1.0);
    let angle = 2.0 * w.acos();
    if angle.abs() < 1e-12 {
        return Ok(None);
    }
    let s = (1.0 - w * w).sqrt();
    let axis = if s < 1e-9 {
        movable_n.perpendicular()
    } else {
        Vector3::new(q.x / s, q.y / s, q.z / s)
    };

    Ok(Some(RigidCorrection::pure_rotation(axis, angle, pivot)))
}

/// Make two directions perpendicular by rotating the movable direction
/// toward the component of itself that is orthogonal to the anchor.
fn perpendicular_correction(
    anchor_dir: Vector3,
    movable_dir: Vector3,
    pivot: Point3,
) -> Result<Option<RigidCorrection>, String> {
    let an = anchor_dir.normalize().map_err(|e| e.to_string())?;
    let mn = movable_dir.normalize().map_err(|e| e.to_string())?;
    let d = mn.dot(&an);
    if d.abs() < 1e-12 {
        return Ok(None);
    }
    // Project out the parallel component:
    // target = normalize(mn - d·an). This is the closest unit vector to
    // mn that is perpendicular to an.
    let target = Vector3::new(
        mn.x - d * an.x,
        mn.y - d * an.y,
        mn.z - d * an.z,
    );
    if target.magnitude() < 1e-12 {
        // mn is parallel to an — rotate by pi/2 about any perpendicular.
        let axis = an.perpendicular();
        return Ok(Some(RigidCorrection::pure_rotation(
            axis,
            std::f64::consts::FRAC_PI_2,
            pivot,
        )));
    }
    let target = target.normalize().map_err(|e| e.to_string())?;
    let q = Quaternion::from_rotation_between(&mn, &target).map_err(|e| e.to_string())?;
    let w = q.w.clamp(-1.0, 1.0);
    let angle = 2.0 * w.acos();
    if angle.abs() < 1e-12 {
        return Ok(None);
    }
    let s = (1.0 - w * w).sqrt();
    let axis = if s < 1e-9 {
        mn.perpendicular()
    } else {
        Vector3::new(q.x / s, q.y / s, q.z / s)
    };
    Ok(Some(RigidCorrection::pure_rotation(axis, angle, pivot)))
}

/// Rotate the movable direction so the angle between anchor and movable
/// equals `target_angle` (radians). The rotation is in the plane spanned
/// by the two directions; if they're parallel the rotation plane is
/// chosen via `anchor_dir.perpendicular()`.
fn angle_correction(
    anchor_dir: Vector3,
    movable_dir: Vector3,
    target_angle: f64,
    pivot: Point3,
) -> Result<Option<RigidCorrection>, String> {
    let an = anchor_dir.normalize().map_err(|e| e.to_string())?;
    let mn = movable_dir.normalize().map_err(|e| e.to_string())?;
    let current_dot = mn.dot(&an).clamp(-1.0, 1.0);
    let current_angle = current_dot.acos();
    let delta = target_angle - current_angle;
    if delta.abs() < 1e-12 {
        return Ok(None);
    }
    // Rotation axis = an × mn (plane normal). If degenerate, pick any
    // perpendicular to an.
    let axis_raw = an.cross(&mn);
    let axis = if axis_raw.magnitude() < 1e-12 {
        an.perpendicular()
    } else {
        axis_raw.normalize().map_err(|e| e.to_string())?
    };
    Ok(Some(RigidCorrection::pure_rotation(axis, delta, pivot)))
}

/// Coincident / planar alignment:
/// 1. Align the movable direction antiparallel (for mating planes) to
///    the anchor direction.
/// 2. Translate so the movable origin lies on the anchor plane.
fn align_plane_like(
    anchor_origin: Option<Point3>,
    anchor_dir: Option<Vector3>,
    movable_origin: Option<Point3>,
    movable_dir: Option<Vector3>,
    sign: f64,
    antiparallel: bool,
) -> Result<Option<RigidCorrection>, String> {
    match (anchor_origin, anchor_dir, movable_origin, movable_dir) {
        // Both sides have plane data: full planar coincidence.
        (Some(ao), Some(ad), Some(mo), Some(md)) => {
            // Step 1: rotate to align normals.
            let rot = align_directions(ad, md, sign, mo, antiparallel)?;
            // Step 2: translate the (possibly rotated) movable origin
            // onto the anchor plane. If rot is Some, apply it to mo first.
            let mo_rotated = match &rot {
                Some(r) => {
                    let m = r.to_matrix();
                    m.transform_point(&mo)
                }
                None => mo,
            };
            let an = ad.normalize().map_err(|e| e.to_string())?;
            let diff = Vector3::new(
                mo_rotated.x - ao.x,
                mo_rotated.y - ao.y,
                mo_rotated.z - ao.z,
            );
            let signed_dist = diff.dot(&an);
            let translation = Vector3::new(
                -an.x * signed_dist,
                -an.y * signed_dist,
                -an.z * signed_dist,
            );
            Ok(Some(compose(rot, translation)))
        }
        // Point–point coincidence: translate only.
        (Some(ao), None, Some(mo), None) => {
            let t = Vector3::new(ao.x - mo.x, ao.y - mo.y, ao.z - mo.z);
            if t.magnitude() < 1e-14 {
                Ok(None)
            } else {
                Ok(Some(RigidCorrection::pure_translation(t)))
            }
        }
        // Plane–point: translate the point onto the plane.
        (Some(ao), Some(ad), Some(mo), None) => {
            let an = ad.normalize().map_err(|e| e.to_string())?;
            let diff = Vector3::new(mo.x - ao.x, mo.y - ao.y, mo.z - ao.z);
            let signed_dist = diff.dot(&an);
            if signed_dist.abs() < 1e-14 {
                Ok(None)
            } else {
                Ok(Some(RigidCorrection::pure_translation(Vector3::new(
                    -an.x * signed_dist,
                    -an.y * signed_dist,
                    -an.z * signed_dist,
                ))))
            }
        }
        (Some(ao), None, Some(mo), Some(_)) => {
            // Point–plane symmetric: translate the plane-origin onto the point.
            let t = Vector3::new(ao.x - mo.x, ao.y - mo.y, ao.z - mo.z);
            if t.magnitude() < 1e-14 {
                Ok(None)
            } else {
                Ok(Some(RigidCorrection::pure_translation(t)))
            }
        }
        _ => Err(
            "Coincident mate: reference pair does not provide sufficient \
             geometric data (need at least one origin on each side)"
                .to_string(),
        ),
    }
}

/// Concentric axis alignment: align directions, then translate so
/// movable origin lies on the anchor axis line.
fn concentric_correction(
    anchor_origin: Point3,
    anchor_dir: Vector3,
    movable_origin: Point3,
    movable_dir: Vector3,
    sign: f64,
) -> Result<Option<RigidCorrection>, String> {
    // Step 1: align directions parallel (or antiparallel with flip).
    let rot = align_directions(
        anchor_dir,
        movable_dir,
        sign,
        movable_origin,
        /* antiparallel = */ false,
    )?;
    // Step 2: apply rotation (if any) to movable_origin, then compute
    // offset from anchor axis.
    let mo_after_rot = match &rot {
        Some(r) => r.to_matrix().transform_point(&movable_origin),
        None => movable_origin,
    };
    let an = anchor_dir.normalize().map_err(|e| e.to_string())?;
    let diff = Vector3::new(
        mo_after_rot.x - anchor_origin.x,
        mo_after_rot.y - anchor_origin.y,
        mo_after_rot.z - anchor_origin.z,
    );
    // Project diff onto axis; the component perpendicular to the axis
    // is the translation we need to cancel.
    let parallel = diff.dot(&an);
    let perp = Vector3::new(
        diff.x - an.x * parallel,
        diff.y - an.y * parallel,
        diff.z - an.z * parallel,
    );
    let translation = Vector3::new(-perp.x, -perp.y, -perp.z);
    Ok(Some(compose(rot, translation)))
}

/// Sequentially compose: first apply the optional rotation, then the
/// translation, into a single RigidCorrection.
fn compose(rotation: Option<RigidCorrection>, translation: Vector3) -> RigidCorrection {
    match rotation {
        Some(r) => RigidCorrection {
            rotation_axis: r.rotation_axis,
            rotation_angle: r.rotation_angle,
            rotation_pivot: r.rotation_pivot,
            translation: Vector3::new(
                r.translation.x + translation.x,
                r.translation.y + translation.y,
                r.translation.z + translation.z,
            ),
        },
        None => RigidCorrection::pure_translation(translation),
    }
}

/// Symmetric mate (2-reference variant): the movable origin is the
/// reflection of the anchor origin across the world XY plane (z = 0).
///
/// In standard CAD UIs a Symmetric mate takes three references — two
/// entities and a symmetry plane. With only two references in this
/// solver's API, the world XY plane serves as the implicit symmetry
/// plane. Only origin positions are mirrored; orientations are not
/// (a reflection is not a rigid motion).
///
/// Locks all three translational DOFs of the movable component.
fn symmetric_correction(
    anchor_origin: Option<Point3>,
    movable_origin: Option<Point3>,
) -> Result<Option<RigidCorrection>, String> {
    let ao = anchor_origin.ok_or_else(|| {
        "Symmetric mate requires an origin on the anchor reference".to_string()
    })?;
    let mo = movable_origin.ok_or_else(|| {
        "Symmetric mate requires an origin on the movable reference".to_string()
    })?;
    let target = Point3::new(ao.x, ao.y, -ao.z);
    let t = Vector3::new(target.x - mo.x, target.y - mo.y, target.z - mo.z);
    if t.magnitude() < 1e-14 {
        Ok(None)
    } else {
        Ok(Some(RigidCorrection::pure_translation(t)))
    }
}

/// Tangent mate (planar contact): the movable origin lies on the plane
/// defined by `(anchor_origin, anchor_dir)`. Signed perpendicular
/// distance is driven to zero by translation along the anchor normal.
///
/// Locks one translational DOF (along the anchor normal); leaves the
/// other five DOFs free.
fn tangent_correction(
    anchor_origin: Option<Point3>,
    anchor_dir: Option<Vector3>,
    movable_origin: Option<Point3>,
) -> Result<Option<RigidCorrection>, String> {
    let ao = anchor_origin.ok_or_else(|| {
        "Tangent mate requires an origin on the anchor reference".to_string()
    })?;
    let ad = anchor_dir.ok_or_else(|| {
        "Tangent mate requires a direction on the anchor reference".to_string()
    })?;
    let mo = movable_origin.ok_or_else(|| {
        "Tangent mate requires an origin on the movable reference".to_string()
    })?;
    let an = ad.normalize().map_err(|e| e.to_string())?;
    let diff = Vector3::new(mo.x - ao.x, mo.y - ao.y, mo.z - ao.z);
    let signed_dist = diff.dot(&an);
    if signed_dist.abs() < 1e-14 {
        Ok(None)
    } else {
        Ok(Some(RigidCorrection::pure_translation(Vector3::new(
            -an.x * signed_dist,
            -an.y * signed_dist,
            -an.z * signed_dist,
        ))))
    }
}

/// Cam-follower mate: with the current 2-reference API and no explicit
/// cam-profile parameterization, this reduces to the planar-tangent
/// case using the anchor's direction as the local cam-surface normal.
///
/// Locks one translational DOF.
fn cam_correction(
    anchor_origin: Option<Point3>,
    anchor_dir: Option<Vector3>,
    movable_origin: Option<Point3>,
) -> Result<Option<RigidCorrection>, String> {
    tangent_correction(anchor_origin, anchor_dir, movable_origin)
}

/// Path mate: the movable origin must lie on the line through
/// `anchor_origin` with direction `anchor_dir`. Two perpendicular-
/// distance components are driven to zero by translation; the parallel
/// component is left free.
///
/// Locks two translational DOFs; one translational DOF along the path
/// remains free.
fn path_correction(
    anchor_origin: Option<Point3>,
    anchor_dir: Option<Vector3>,
    movable_origin: Option<Point3>,
) -> Result<Option<RigidCorrection>, String> {
    let ao = anchor_origin.ok_or_else(|| {
        "Path mate requires an origin on the anchor reference".to_string()
    })?;
    let ad = anchor_dir.ok_or_else(|| {
        "Path mate requires a direction on the anchor reference".to_string()
    })?;
    let mo = movable_origin.ok_or_else(|| {
        "Path mate requires an origin on the movable reference".to_string()
    })?;
    let an = ad.normalize().map_err(|e| e.to_string())?;
    let diff = Vector3::new(mo.x - ao.x, mo.y - ao.y, mo.z - ao.z);
    let parallel = diff.dot(&an);
    // Perpendicular component (the offset from the line we must cancel).
    let perp = Vector3::new(
        diff.x - an.x * parallel,
        diff.y - an.y * parallel,
        diff.z - an.z * parallel,
    );
    if perp.magnitude() < 1e-14 {
        Ok(None)
    } else {
        Ok(Some(RigidCorrection::pure_translation(Vector3::new(
            -perp.x, -perp.y, -perp.z,
        ))))
    }
}

/// Gear mate: couples the rotational positions of two components about
/// their respective reference axes:
///
/// ```text
/// theta_movable + ratio * theta_anchor == 0    (modulo 2*pi)
/// ```
///
/// where `theta_X` is the signed rotation of component X about its
/// reference axis, measured from the component's initial transform
/// (the "neutral" gear position).
///
/// Locks one rotational DOF: the movable component's rotation about
/// its reference axis is fully determined by the anchor's rotation.
/// Translation along/perpendicular to the gear axis is unaffected;
/// gear-pair center distance is typically enforced by a separate
/// Distance or Concentric mate on the same component pair.
fn gear_correction(
    c: &ResolvedConstraint,
    t1: &Matrix4,
    t2: &Matrix4,
    ref1: &MateReference,
    ref2: &MateReference,
    movable: ComponentId,
    gear_neutrals: &HashMap<MateId, (Matrix4, Matrix4)>,
    ratio: f64,
) -> Result<Option<RigidCorrection>, String> {
    // Identify which side is the anchor and which is the movable, and
    // pick the corresponding references, current transforms, and
    // matching slot in the persistent neutral pair (which is keyed
    // by the constraint's component1/component2 order, NOT by
    // anchor/movable).
    let (anchor_ref, anchor_t, movable_ref, movable_t, neutral_anchor, neutral_movable) = {
        let (n1, n2) = gear_neutrals.get(&c.mate_id).ok_or_else(|| {
            "Gear mate: persistent neutral transforms missing — was the mate \
             registered through Assembly::add_mate?"
                .to_string()
        })?;
        if movable == c.component2 {
            (ref1, t1, ref2, t2, n1, n2)
        } else {
            (ref2, t2, ref1, t1, n2, n1)
        }
    };

    let (ao_opt, ad_opt) = world_origin_direction(anchor_ref, anchor_t);
    let (mo_opt, md_opt) = world_origin_direction(movable_ref, movable_t);
    let _ao = ao_opt.ok_or_else(|| {
        "Gear mate requires an origin on the anchor reference".to_string()
    })?;
    let ad = ad_opt.ok_or_else(|| {
        "Gear mate requires a direction on the anchor reference".to_string()
    })?;
    let mo = mo_opt.ok_or_else(|| {
        "Gear mate requires an origin on the movable reference".to_string()
    })?;
    let md = md_opt.ok_or_else(|| {
        "Gear mate requires a direction on the movable reference".to_string()
    })?;

    // The "neutral" axis directions live in the neutral world frame.
    // Since each component's axis rotates with the component, we
    // measure each component's rotation relative to its own neutral
    // transform, projected onto its neutral-world axis.
    let anchor_initial = neutral_anchor;
    let movable_initial = neutral_movable;

    let anchor_axis_initial = anchor_initial.transform_vector(match anchor_ref {
        MateReference::Face { normal, .. } => normal,
        MateReference::Edge { direction, .. } => direction,
        MateReference::Axis { direction, .. } => direction,
        MateReference::Plane { normal, .. } => normal,
        MateReference::Point { .. } => {
            return Err(
                "Gear mate: anchor reference is a Point with no axis direction".to_string(),
            );
        }
    });
    let movable_axis_initial = movable_initial.transform_vector(match movable_ref {
        MateReference::Face { normal, .. } => normal,
        MateReference::Edge { direction, .. } => direction,
        MateReference::Axis { direction, .. } => direction,
        MateReference::Plane { normal, .. } => normal,
        MateReference::Point { .. } => {
            return Err(
                "Gear mate: movable reference is a Point with no axis direction".to_string(),
            );
        }
    });

    // Compute delta = t_current * t_initial^-1 — the world-frame rigid
    // motion that takes the component from its neutral position to its
    // current position.
    let anchor_initial_inv = anchor_initial.inverse().map_err(|_| {
        "Gear mate: anchor initial transform is non-invertible".to_string()
    })?;
    let movable_initial_inv = movable_initial.inverse().map_err(|_| {
        "Gear mate: movable initial transform is non-invertible".to_string()
    })?;
    let anchor_delta = *anchor_t * anchor_initial_inv;
    let movable_delta = *movable_t * movable_initial_inv;

    let theta_anchor = signed_rotation_about_axis(&anchor_delta, anchor_axis_initial)?;
    let theta_movable = signed_rotation_about_axis(&movable_delta, movable_axis_initial)?;

    // Constraint: theta_movable + ratio * theta_anchor == 0.
    // Apply correction to the movable side: rotate by
    //     delta_theta = -(theta_movable + ratio * theta_anchor)
    // about the movable axis at the movable origin.
    let delta_theta = -(theta_movable + ratio * theta_anchor);

    // Wrap into (-pi, pi] to take the shortest signed rotation each
    // iteration; the constraint is intrinsically modulo 2*pi anyway.
    let two_pi = 2.0 * std::f64::consts::PI;
    let mut wrapped = delta_theta % two_pi;
    if wrapped > std::f64::consts::PI {
        wrapped -= two_pi;
    } else if wrapped <= -std::f64::consts::PI {
        wrapped += two_pi;
    }
    if wrapped.abs() < 1e-12 {
        return Ok(None);
    }

    // Rotate about the *current* world axis of the movable reference,
    // not the initial axis — the component may have translated.
    let axis_world = md.normalize().map_err(|e| e.to_string())?;
    let _ = ad; // anchor axis only needed to verify presence above
    Ok(Some(RigidCorrection::pure_rotation(axis_world, wrapped, mo)))
}

/// Extract the signed scalar rotation of a rigid motion about a given
/// world-space axis. Computes the angle by transforming a reference
/// vector perpendicular to the axis and measuring the signed angle in
/// the plane normal to the axis.
///
/// Returns 0 for pure translations or when the rotation has no
/// component about the requested axis.
fn signed_rotation_about_axis(
    delta: &Matrix4,
    axis: Vector3,
) -> Result<f64, String> {
    let axis_n = axis.normalize().map_err(|e| e.to_string())?;
    // Pick any unit vector perpendicular to axis.
    let r = axis_n.perpendicular();
    let r_rot = delta.transform_vector(&r);
    // Project r_rot onto the plane normal to axis (defensive — for a
    // pure rotation about axis the projection is exact).
    let r_rot_parallel = r_rot.dot(&axis_n);
    let r_rot_perp = Vector3::new(
        r_rot.x - axis_n.x * r_rot_parallel,
        r_rot.y - axis_n.y * r_rot_parallel,
        r_rot.z - axis_n.z * r_rot_parallel,
    );
    if r_rot_perp.magnitude() < 1e-12 {
        // Degenerate: r_rot is collinear with axis — only happens if the
        // delta has rotated r entirely onto the axis (a 90° rotation
        // about an axis perpendicular to both r and axis_n). For a
        // gear coupling this is non-physical; return 0 conservatively.
        return Ok(0.0);
    }
    // Signed angle in the plane: theta = atan2(axis · (r × r_perp),
    //                                          r · r_perp).
    let cross = r.cross(&r_rot_perp);
    let sin_t = axis_n.dot(&cross);
    let cos_t = r.dot(&r_rot_perp);
    Ok(sin_t.atan2(cos_t))
}

/// Assembly errors
#[derive(Debug, thiserror::Error)]
pub enum AssemblyError {
    #[error("Component not found: {0:?}")]
    ComponentNotFound(ComponentId),

    #[error("Mate reference not found: {0}")]
    ReferenceNotFound(String),

    #[error("Over-constrained assembly")]
    OverConstrained,

    #[error("Conflicting constraints")]
    ConflictingConstraints,

    #[error("Solver failed: {0}")]
    SolverFailed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_assembly() {
        let assembly = Assembly::new("Test Assembly");
        assert_eq!(assembly.name, "Test Assembly");
        assert!(assembly.root_component.is_none());
    }

    #[test]
    fn test_add_parts() {
        let mut assembly = Assembly::new("Test Assembly");

        // Create dummy parts
        let part1 = Arc::new(BRepModel::new());
        let part2 = Arc::new(BRepModel::new());

        let comp1 = assembly.add_part(part1, "Part 1");
        let comp2 = assembly.add_part(part2, "Part 2");

        assert!(assembly.get_component(comp1).is_some());
        assert!(assembly.get_component(comp2).is_some());
    }

    #[test]
    fn test_mate_constraints() {
        let mut assembly = Assembly::new("Test Assembly");

        let part1 = Arc::new(BRepModel::new());
        let part2 = Arc::new(BRepModel::new());

        let comp1 = assembly.add_part(part1, "Part 1");
        let comp2 = assembly.add_part(part2, "Part 2");

        // Mate whose named references have not been registered on either
        // component — the solver must register the mate without failing,
        // recording a descriptive diagnostic on the stored constraint.
        let mate_result = assembly.add_mate(MateType::Coincident, comp1, "face1", comp2, "face2");

        assert!(mate_result.is_ok());
        let mate_id = mate_result.unwrap();
        let mate = assembly
            .mates
            .get(&mate_id)
            .expect("mate registered");
        assert!(!mate.solved);
        assert!(mate.error.as_deref().unwrap_or("").contains("not registered"));
    }

    fn register_plane_reference(
        assembly: &Assembly,
        component: ComponentId,
        name: &str,
        origin: Point3,
        normal: Vector3,
    ) {
        let mut comp = assembly
            .components
            .get_mut(&component)
            .expect("component exists");
        comp.mate_references.insert(
            name.to_string(),
            MateReference::Plane { origin, normal },
        );
    }

    fn register_axis_reference(
        assembly: &Assembly,
        component: ComponentId,
        name: &str,
        origin: Point3,
        direction: Vector3,
    ) {
        let mut comp = assembly
            .components
            .get_mut(&component)
            .expect("component exists");
        comp.mate_references.insert(
            name.to_string(),
            MateReference::Axis { origin, direction },
        );
    }

    #[test]
    fn test_coincident_planes_solver_drives_distance_to_zero() {
        let mut assembly = Assembly::new("coincident");
        let comp1 = assembly.add_part(Arc::new(BRepModel::new()), "Fixed");
        let comp2 = assembly.add_part(Arc::new(BRepModel::new()), "Movable");

        // Place component2 at translation (10, 0, 0) so its plane
        // reference starts 10 units away from component1's reference.
        {
            let mut c2 = assembly.components.get_mut(&comp2).unwrap();
            c2.transform = Matrix4::from_translation(&Vector3::new(10.0, 0.0, 0.0));
        }

        register_plane_reference(
            &assembly,
            comp1,
            "p1",
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
        );
        register_plane_reference(
            &assembly,
            comp2,
            "p2",
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, -1.0),
        );

        assembly
            .add_mate(MateType::Coincident, comp1, "p1", comp2, "p2")
            .expect("mate accepted");

        // After solve, component2's plane-origin (in world coords) must
        // lie on component1's plane: plane z == 0.
        let c2_final = assembly.get_component(comp2).unwrap();
        let world_origin = c2_final
            .transform
            .transform_point(&Point3::new(0.0, 0.0, 0.0));
        assert!(
            world_origin.z.abs() < 1e-6,
            "coincident solver left z offset {:.3e}",
            world_origin.z
        );
    }

    #[test]
    fn test_concentric_axes_solver_brings_origin_onto_axis() {
        let mut assembly = Assembly::new("concentric");
        let comp1 = assembly.add_part(Arc::new(BRepModel::new()), "Fixed");
        let comp2 = assembly.add_part(Arc::new(BRepModel::new()), "Movable");

        // Seed comp2 at (0, 5, 0) — 5 units off the X-axis.
        {
            let mut c2 = assembly.components.get_mut(&comp2).unwrap();
            c2.transform = Matrix4::from_translation(&Vector3::new(0.0, 5.0, 0.0));
        }

        register_axis_reference(
            &assembly,
            comp1,
            "a1",
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
        );
        register_axis_reference(
            &assembly,
            comp2,
            "a2",
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
        );

        assembly
            .add_mate(MateType::Concentric, comp1, "a1", comp2, "a2")
            .expect("mate accepted");

        // Component2's axis origin in world coords must project onto the
        // X-axis (y ≈ 0, z ≈ 0).
        let c2_final = assembly.get_component(comp2).unwrap();
        let world_origin = c2_final
            .transform
            .transform_point(&Point3::new(0.0, 0.0, 0.0));
        assert!(world_origin.y.abs() < 1e-6);
        assert!(world_origin.z.abs() < 1e-6);
    }

    #[test]
    fn test_lock_mate_copies_anchor_transform() {
        let mut assembly = Assembly::new("lock");
        let comp1 = assembly.add_part(Arc::new(BRepModel::new()), "Fixed");
        let comp2 = assembly.add_part(Arc::new(BRepModel::new()), "Movable");

        // Pre-position both components differently.
        {
            let mut c1 = assembly.components.get_mut(&comp1).unwrap();
            c1.transform = Matrix4::from_translation(&Vector3::new(3.0, 4.0, 5.0));
        }
        {
            let mut c2 = assembly.components.get_mut(&comp2).unwrap();
            c2.transform = Matrix4::from_translation(&Vector3::new(-1.0, 0.0, 2.0));
        }

        // Lock mates do not read named references — register dummies so
        // the solver sees non-None refs and enters the Lock branch.
        register_plane_reference(
            &assembly,
            comp1,
            "any1",
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
        );
        register_plane_reference(
            &assembly,
            comp2,
            "any2",
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
        );

        assembly
            .add_mate(MateType::Lock, comp1, "any1", comp2, "any2")
            .expect("mate accepted");

        let c2_final = assembly.get_component(comp2).unwrap();
        let t = c2_final.transform.translation_vector();
        assert!((t.x - 3.0).abs() < 1e-6);
        assert!((t.y - 4.0).abs() < 1e-6);
        assert!((t.z - 5.0).abs() < 1e-6);
    }

    #[test]
    fn test_symmetric_mate_mirrors_origin_across_xy_plane() {
        let mut assembly = Assembly::new("symmetric");
        let comp1 = assembly.add_part(Arc::new(BRepModel::new()), "Anchor");
        let comp2 = assembly.add_part(Arc::new(BRepModel::new()), "Movable");

        // Anchor sits at z = +5; movable starts off-mirror at z = +2.
        {
            let mut c1 = assembly.components.get_mut(&comp1).unwrap();
            c1.transform = Matrix4::from_translation(&Vector3::new(2.0, 3.0, 5.0));
        }
        {
            let mut c2 = assembly.components.get_mut(&comp2).unwrap();
            c2.transform = Matrix4::from_translation(&Vector3::new(0.0, 0.0, 2.0));
        }

        register_plane_reference(
            &assembly,
            comp1,
            "p1",
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
        );
        register_plane_reference(
            &assembly,
            comp2,
            "p2",
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
        );

        assembly
            .add_mate(MateType::Symmetric, comp1, "p1", comp2, "p2")
            .expect("symmetric mate accepted");

        // After solve, comp2's plane-origin in world coords must be the
        // reflection of comp1's through z = 0: (2, 3, -5).
        let c2_final = assembly.get_component(comp2).unwrap();
        let world_origin = c2_final
            .transform
            .transform_point(&Point3::new(0.0, 0.0, 0.0));
        assert!((world_origin.x - 2.0).abs() < 1e-6);
        assert!((world_origin.y - 3.0).abs() < 1e-6);
        assert!((world_origin.z + 5.0).abs() < 1e-6);
    }

    #[test]
    fn test_tangent_mate_drives_origin_onto_anchor_plane() {
        let mut assembly = Assembly::new("tangent");
        let comp1 = assembly.add_part(Arc::new(BRepModel::new()), "Anchor");
        let comp2 = assembly.add_part(Arc::new(BRepModel::new()), "Movable");

        // Movable starts 7 units above the anchor's z = 0 plane.
        {
            let mut c2 = assembly.components.get_mut(&comp2).unwrap();
            c2.transform = Matrix4::from_translation(&Vector3::new(4.0, -2.0, 7.0));
        }

        register_plane_reference(
            &assembly,
            comp1,
            "p1",
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
        );
        register_plane_reference(
            &assembly,
            comp2,
            "p2",
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
        );

        assembly
            .add_mate(MateType::Tangent, comp1, "p1", comp2, "p2")
            .expect("tangent mate accepted");

        // Movable origin's z-component is driven to zero (on the anchor
        // plane); x and y are unchanged.
        let c2_final = assembly.get_component(comp2).unwrap();
        let world_origin = c2_final
            .transform
            .transform_point(&Point3::new(0.0, 0.0, 0.0));
        assert!((world_origin.x - 4.0).abs() < 1e-6);
        assert!((world_origin.y + 2.0).abs() < 1e-6);
        assert!(world_origin.z.abs() < 1e-6);
    }

    #[test]
    fn test_path_mate_drives_origin_onto_anchor_axis() {
        let mut assembly = Assembly::new("path");
        let comp1 = assembly.add_part(Arc::new(BRepModel::new()), "Anchor");
        let comp2 = assembly.add_part(Arc::new(BRepModel::new()), "Movable");

        // Movable starts at (3, 4, 5); anchor's path is the X-axis
        // through the world origin. After solve, the y and z
        // coordinates must both go to zero; x is free (ends at 3).
        {
            let mut c2 = assembly.components.get_mut(&comp2).unwrap();
            c2.transform = Matrix4::from_translation(&Vector3::new(3.0, 4.0, 5.0));
        }

        register_axis_reference(
            &assembly,
            comp1,
            "a1",
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
        );
        register_axis_reference(
            &assembly,
            comp2,
            "a2",
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
        );

        assembly
            .add_mate(MateType::Path, comp1, "a1", comp2, "a2")
            .expect("path mate accepted");

        let c2_final = assembly.get_component(comp2).unwrap();
        let world_origin = c2_final
            .transform
            .transform_point(&Point3::new(0.0, 0.0, 0.0));
        assert!((world_origin.x - 3.0).abs() < 1e-6);
        assert!(world_origin.y.abs() < 1e-6);
        assert!(world_origin.z.abs() < 1e-6);
    }

    #[test]
    fn test_gear_mate_couples_rotations_with_ratio() {
        let mut assembly = Assembly::new("gear");
        let comp1 = assembly.add_part(Arc::new(BRepModel::new()), "Anchor");
        let comp2 = assembly.add_part(Arc::new(BRepModel::new()), "Movable");

        // Both gears spin about the world Z-axis at the origin in their
        // neutral pose (no initial rotation, no offset).
        register_axis_reference(
            &assembly,
            comp1,
            "a1",
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
        );
        register_axis_reference(
            &assembly,
            comp2,
            "a2",
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
        );

        // Add the gear mate at the neutral pose: ratio = 2 means
        // theta_movable + 2 * theta_anchor == 0.
        assembly
            .add_mate(MateType::Gear { ratio: 2.0 }, comp1, "a1", comp2, "a2")
            .expect("gear mate accepted");

        // Rotate the anchor by +pi/4 about Z. The solver must rotate the
        // movable by -2*pi/4 = -pi/2 about Z to satisfy the coupling.
        {
            let mut c1 = assembly.components.get_mut(&comp1).unwrap();
            let q = Quaternion::from_axis_angle(
                &Vector3::new(0.0, 0.0, 1.0),
                std::f64::consts::FRAC_PI_4,
            )
            .expect("axis-angle valid");
            c1.transform = q.to_matrix4();
        }
        assembly.solve_constraints().expect("solver ok");

        let c2_final = assembly.get_component(comp2).unwrap();
        // Decompose movable transform: it should be a pure rotation
        // about Z by -pi/2. Probe by rotating a unit X vector and
        // checking the result is approximately (cos(-pi/2), sin(-pi/2), 0) = (0, -1, 0).
        let probe = c2_final
            .transform
            .transform_vector(&Vector3::new(1.0, 0.0, 0.0));
        assert!(probe.x.abs() < 1e-6, "probe.x = {}", probe.x);
        assert!((probe.y + 1.0).abs() < 1e-6, "probe.y = {}", probe.y);
        assert!(probe.z.abs() < 1e-6, "probe.z = {}", probe.z);
    }
}
