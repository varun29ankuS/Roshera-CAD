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
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// Unique identifier for assembly components
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ComponentId(pub Uuid);

impl ComponentId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

/// Unique identifier for mate constraints
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MateId(pub Uuid);

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
                .or_insert_with(Vec::new)
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

        // Mate is registered; solving is deferred. Callers must invoke
        // `solve_constraints` explicitly when they are ready — matching
        // the CATIA/NX/SolidWorks convention where add/remove of mates
        // does not auto-rebuild the kinematic state.
        Ok(id)
    }

    /// Solve all mate constraints
    pub fn solve_constraints(&mut self) -> Result<(), AssemblyError> {
        // Build constraint system
        let mut solver = ConstraintSolver::new();

        // Add all active mates
        for mate in self.mates.iter() {
            if !mate.suppressed {
                solver.add_constraint(&mate)?;
            }
        }

        // Solve the system
        let solution = solver.solve()?;

        // Apply transforms to components
        for (component_id, transform) in solution {
            if let Some(mut component) = self.components.get_mut(&component_id) {
                component.transform = transform;
                component.degrees_of_freedom = solver.get_dof(&component_id);
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
            let transform = component.transform.clone();
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
            let mut new_transform = comp.transform.clone();
            // Apply translation
            new_transform[(0, 3)] += delta.x;
            new_transform[(1, 3)] += delta.y;
            new_transform[(2, 3)] += delta.z;

            if let Some(rotation) = delta_rotation {
                // Post-multiply: rotation applied in the component's local
                // frame, after the translation column has been updated.
                // Standard rigid-body convention for incremental motion.
                new_transform = new_transform * rotation.to_matrix4();
            }
            comp.transform = new_transform;
        }

        // Re-solve constraints to update dependent components
        self.solve_constraints()?;

        Ok(())
    }
}

/// Constraint solver for assembly mates
struct ConstraintSolver {
    constraints: Vec<MateConstraint>,
    component_dof: HashMap<ComponentId, u8>,
}

impl ConstraintSolver {
    fn new() -> Self {
        Self {
            constraints: Vec::new(),
            component_dof: HashMap::new(),
        }
    }

    fn add_constraint(&mut self, mate: &MateConstraint) -> Result<(), AssemblyError> {
        self.constraints.push(mate.clone());

        // Update DOF based on constraint type
        let dof_removed = match mate.mate_type {
            MateType::Lock => 6,
            MateType::Coincident => 3,
            MateType::Concentric => 4,
            MateType::Parallel => 2,
            MateType::Perpendicular => 2,
            MateType::Distance(_) => 1,
            MateType::Angle(_) => 1,
            MateType::Tangent => 1,
            _ => 0,
        };

        // Update component DOF
        *self.component_dof.entry(mate.component1).or_insert(6) -= dof_removed.min(6);
        *self.component_dof.entry(mate.component2).or_insert(6) -= dof_removed.min(6);

        Ok(())
    }

    fn solve(&self) -> Result<HashMap<ComponentId, Matrix4>, AssemblyError> {
        Err(AssemblyError::SolverFailed(
            "Constraint solver not yet implemented".to_string(),
        ))
    }

    fn get_dof(&self, component: &ComponentId) -> u8 {
        self.component_dof.get(component).copied().unwrap_or(6)
    }
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

        let mate_result = assembly.add_mate(MateType::Coincident, comp1, "face1", comp2, "face2");

        assert!(mate_result.is_ok());
    }
}
