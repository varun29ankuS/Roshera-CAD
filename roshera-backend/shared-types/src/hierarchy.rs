//! Assembly and part hierarchy types for organizing CAD project structure.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Root project hierarchy containing the top-level assembly and a shared part library.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectHierarchy {
    /// Top-level assembly that anchors the hierarchy tree.
    pub root_assembly: Assembly,
    /// Library of reusable part definitions keyed by definition ID.
    pub part_library: HashMap<String, PartDefinition>,
}

/// An assembly node that can contain part instances and nested sub-assemblies.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Assembly {
    /// Unique assembly identifier.
    pub id: String,
    /// Display name of the assembly.
    pub name: String,
    /// Ordered list of child nodes (instances or sub-assemblies).
    pub children: Vec<HierarchyNode>,
}

/// A single node in the hierarchy — either a part instance or a nested assembly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HierarchyNode {
    /// Instance of a part defined in the part library.
    PartInstance(PartInstance),
    /// Nested sub-assembly.
    SubAssembly(Assembly),
}

/// Placement of a part within an assembly, referencing a shared definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PartInstance {
    /// Unique identifier of this instance within its assembly.
    pub instance_id: String,
    /// Identifier of the part definition this instance references.
    pub definition_id: String,
    /// Sequential instance number (e.g. Bolt #3).
    pub instance_number: u32,
    /// Local transform applied on top of the definition's geometry.
    pub transform: Transform,
    /// Whether this instance has been promoted to a unique copy (no longer sharing definition).
    pub is_unique: bool,
}

/// Definition of a reusable part, shared across all its instances.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PartDefinition {
    /// Unique part definition identifier.
    pub id: String,
    /// Display name of the part.
    pub name: String,
    /// Identifier of the part's underlying geometry body.
    pub geometry_id: String,
    /// Ordered list of features that make up the part.
    pub features: Vec<Feature>,
    /// Monotonically incremented version number for this definition.
    pub version: u32,
}

/// Rigid-body transform (position + quaternion rotation + per-axis scale).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Transform {
    /// Translation vector [x, y, z].
    pub position: [f64; 3],
    /// Rotation quaternion [x, y, z, w].
    pub rotation: [f64; 4],
    /// Per-axis scale factors [sx, sy, sz].
    pub scale: [f64; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        }
    }
}

/// Individual feature in a part (sketch, extrude, fillet, etc.).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Feature {
    /// Unique feature identifier.
    pub id: String,
    /// Category of feature.
    pub feature_type: FeatureType,
    /// Named numeric parameters for the feature.
    pub parameters: HashMap<String, f64>,
}

/// Supported feature categories in a part definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FeatureType {
    /// 2D sketch feature.
    Sketch,
    /// Extrude a sketch along a direction.
    Extrude,
    /// Revolve a sketch around an axis.
    Revolve,
    /// Round an edge with a constant-radius fillet.
    Fillet,
    /// Bevel an edge with a chamfer.
    Chamfer,
    /// Pattern a feature linearly or circularly.
    Pattern,
    /// Cylindrical hole feature.
    Hole,
}

/// Active edit scope within the hierarchy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EditContext {
    /// Editing the assembly identified by the given ID.
    Assembly(String),
    /// Editing the part definition identified by the given ID.
    PartDefinition(String),
    /// Editing a specific part instance identified by `(assembly_id, instance_id)`.
    PartInstance(String, String),
}

/// Current workflow state (stage + active edit context + available tools).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowState {
    /// Current high-level workflow stage.
    pub current_stage: WorkflowStage,
    /// Current edit context (what the user is focused on).
    pub current_context: EditContext,
    /// Tool names currently available in this state.
    pub available_tools: Vec<String>,
}

/// High-level stages of the CAD workflow.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum WorkflowStage {
    /// Initial creation stage (new project or new part).
    Create,
    /// Defining geometry and features.
    Define,
    /// Refining and iterating on existing geometry.
    Refine,
    /// Validating the design (checks, simulation).
    Validate,
    /// Generating outputs (drawings, exports, manufacturing data).
    Output,
}

/// Commands for mutating the hierarchy and workflow.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HierarchyCommand {
    /// Create a new part definition in the part library.
    CreatePartDefinition {
        /// Display name for the new part.
        name: String,
    },
    /// Instantiate an existing part definition in an assembly.
    CreatePartInstance {
        /// Part definition to instantiate.
        definition_id: String,
        /// Target assembly that will host the new instance.
        assembly_id: String,
    },
    /// Enter edit mode on a part definition.
    EditPartDefinition {
        /// Part definition to edit.
        definition_id: String,
    },
    /// Enter edit mode on a specific part instance.
    EditPartInstance {
        /// Assembly containing the instance.
        assembly_id: String,
        /// Instance being edited.
        instance_id: String,
    },
    /// Promote an instance to a unique definition so edits don't propagate.
    MakeInstanceUnique {
        /// Assembly containing the instance.
        assembly_id: String,
        /// Instance to make unique.
        instance_id: String,
    },
    /// Update the transform of a part instance.
    UpdateTransform {
        /// Assembly containing the instance.
        assembly_id: String,
        /// Instance to transform.
        instance_id: String,
        /// New transform for the instance.
        transform: Transform,
    },
    /// Create a new sub-assembly under an existing parent assembly.
    CreateSubAssembly {
        /// Parent assembly that will own the new sub-assembly.
        parent_id: String,
        /// Display name for the sub-assembly.
        name: String,
    },
    /// Transition the workflow to a new stage.
    SetWorkflowStage {
        /// Target workflow stage.
        stage: WorkflowStage,
    },
}

/// Snapshot of the hierarchy and workflow after a mutation, with a list of affected instances.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HierarchyUpdate {
    /// Updated full project hierarchy.
    pub hierarchy: ProjectHierarchy,
    /// Updated workflow state.
    pub workflow_state: WorkflowState,
    /// Instances affected by the update as `(assembly_id, instance_id)` pairs.
    pub affected_instances: Vec<(String, String)>,
}
