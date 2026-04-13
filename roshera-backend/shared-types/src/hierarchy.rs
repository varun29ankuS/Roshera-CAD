use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectHierarchy {
    pub root_assembly: Assembly,
    pub part_library: HashMap<String, PartDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Assembly {
    pub id: String,
    pub name: String,
    pub children: Vec<HierarchyNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HierarchyNode {
    PartInstance(PartInstance),
    SubAssembly(Assembly),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PartInstance {
    pub instance_id: String,
    pub definition_id: String,
    pub instance_number: u32,
    pub transform: Transform,
    pub is_unique: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PartDefinition {
    pub id: String,
    pub name: String,
    pub geometry_id: String,
    pub features: Vec<Feature>,
    pub version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Transform {
    pub position: [f64; 3],
    pub rotation: [f64; 4], // Quaternion
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Feature {
    pub id: String,
    pub feature_type: FeatureType,
    pub parameters: HashMap<String, f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FeatureType {
    Sketch,
    Extrude,
    Revolve,
    Fillet,
    Chamfer,
    Pattern,
    Hole,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EditContext {
    Assembly(String),
    PartDefinition(String),
    PartInstance(String, String), // (assembly_id, instance_id)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowState {
    pub current_stage: WorkflowStage,
    pub current_context: EditContext,
    pub available_tools: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum WorkflowStage {
    Create,
    Define,
    Refine,
    Validate,
    Output,
}

// Commands for hierarchy management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HierarchyCommand {
    CreatePartDefinition {
        name: String,
    },
    CreatePartInstance {
        definition_id: String,
        assembly_id: String,
    },
    EditPartDefinition {
        definition_id: String,
    },
    EditPartInstance {
        assembly_id: String,
        instance_id: String,
    },
    MakeInstanceUnique {
        assembly_id: String,
        instance_id: String,
    },
    UpdateTransform {
        assembly_id: String,
        instance_id: String,
        transform: Transform,
    },
    CreateSubAssembly {
        parent_id: String,
        name: String,
    },
    SetWorkflowStage {
        stage: WorkflowStage,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HierarchyUpdate {
    pub hierarchy: ProjectHierarchy,
    pub workflow_state: WorkflowState,
    pub affected_instances: Vec<(String, String)>, // (assembly_id, instance_id)
}
