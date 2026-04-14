use shared_types::hierarchy::{
    Assembly, EditContext, HierarchyCommand, HierarchyNode, HierarchyUpdate,
    PartDefinition, PartInstance, ProjectHierarchy, Transform, WorkflowStage, WorkflowState,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Clone)]
pub struct HierarchyManager {
    sessions: Arc<RwLock<HashMap<String, Arc<RwLock<ProjectHierarchy>>>>>,
    workflow_states: Arc<RwLock<HashMap<String, WorkflowState>>>,
}

impl HierarchyManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            workflow_states: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn create_session(&self, session_id: String) -> ProjectHierarchy {
        let root_assembly = Assembly {
            id: Uuid::new_v4().to_string(),
            name: "Untitled_Assembly".to_string(),
            children: Vec::new(),
        };

        let hierarchy = ProjectHierarchy {
            root_assembly,
            part_library: std::collections::HashMap::new(),
        };

        let workflow_state = WorkflowState {
            current_stage: WorkflowStage::Create,
            current_context: EditContext::Assembly(hierarchy.root_assembly.id.clone()),
            available_tools: self.get_tools_for_stage(WorkflowStage::Create),
        };

        self.sessions
            .write()
            .await
            .insert(session_id.clone(), Arc::new(RwLock::new(hierarchy.clone())));
        self.workflow_states
            .write()
            .await
            .insert(session_id, workflow_state);

        hierarchy
    }

    pub async fn execute_command(
        &self,
        session_id: &str,
        command: HierarchyCommand,
    ) -> Result<HierarchyUpdate, String> {
        let sessions = self.sessions.read().await;
        let session = sessions.get(session_id).ok_or("Session not found")?.clone();
        drop(sessions);
        let mut hierarchy = session.write().await;

        let mut affected_instances = Vec::new();

        match command {
            HierarchyCommand::CreatePartDefinition { name } => {
                let part_id = Uuid::new_v4().to_string();
                let part_def = PartDefinition {
                    id: part_id.clone(),
                    name,
                    geometry_id: Uuid::new_v4().to_string(),
                    features: Vec::new(),
                    version: 1,
                };
                hierarchy.part_library.insert(part_id.clone(), part_def);

                // Update workflow state
                let mut workflow_states = self.workflow_states.write().await;
                if let Some(state) = workflow_states.get_mut(session_id) {
                    state.current_context = EditContext::PartDefinition(part_id);
                }
            }

            HierarchyCommand::CreatePartInstance {
                definition_id,
                assembly_id,
            } => {
                if !hierarchy.part_library.contains_key(&definition_id) {
                    return Err("Part definition not found".to_string());
                }

                let instance_count = self.count_instances(&hierarchy, &definition_id);
                let instance = PartInstance {
                    instance_id: Uuid::new_v4().to_string(),
                    definition_id: definition_id.clone(),
                    instance_number: instance_count + 1,
                    transform: Transform::default(),
                    is_unique: false,
                };

                affected_instances.push((assembly_id.clone(), instance.instance_id.clone()));

                // Add to assembly
                self.add_to_assembly(
                    &mut hierarchy,
                    &assembly_id,
                    HierarchyNode::PartInstance(instance),
                )?;
            }

            HierarchyCommand::MakeInstanceUnique {
                assembly_id,
                instance_id,
            } => {
                // First get the definition ID and check if unique
                let (should_make_unique, definition_id) = {
                    if let Some(instance) =
                        self.find_instance(&hierarchy, &assembly_id, &instance_id)
                    {
                        (!instance.is_unique, instance.definition_id.clone())
                    } else {
                        return Err("Instance not found".to_string());
                    }
                };

                if should_make_unique {
                    // Create a copy of the part definition
                    let original_def = hierarchy
                        .part_library
                        .get(&definition_id)
                        .ok_or("Part definition not found")?
                        .clone();

                    let new_def_id = Uuid::new_v4().to_string();
                    let mut new_def = original_def;
                    new_def.id = new_def_id.clone();
                    new_def.name = format!("{}_Modified", new_def.name);
                    new_def.version = 1;

                    hierarchy.part_library.insert(new_def_id.clone(), new_def);

                    // Now update the instance
                    if let Some(instance) =
                        self.find_instance_mut(&mut hierarchy, &assembly_id, &instance_id)
                    {
                        instance.definition_id = new_def_id;
                        instance.is_unique = true;
                    }

                    affected_instances.push((assembly_id, instance_id));
                }
            }

            HierarchyCommand::UpdateTransform {
                assembly_id,
                instance_id,
                transform,
            } => {
                if let Some(instance) =
                    self.find_instance_mut(&mut hierarchy, &assembly_id, &instance_id)
                {
                    instance.transform = transform;
                    affected_instances.push((assembly_id, instance_id));
                }
            }

            HierarchyCommand::EditPartDefinition { definition_id } => {
                if !hierarchy.part_library.contains_key(&definition_id) {
                    return Err("Part definition not found".to_string());
                }

                // Update workflow state
                let mut workflow_states = self.workflow_states.write().await;
                if let Some(state) = workflow_states.get_mut(session_id) {
                    state.current_context = EditContext::PartDefinition(definition_id.clone());
                }

                // Find all instances of this definition
                affected_instances = self.find_all_instances(&hierarchy, &definition_id);
            }

            HierarchyCommand::EditPartInstance {
                assembly_id,
                instance_id,
            } => {
                // Verify instance exists
                if self
                    .find_instance(&hierarchy, &assembly_id, &instance_id)
                    .is_none()
                {
                    return Err("Instance not found".to_string());
                }

                // Update workflow state
                let mut workflow_states = self.workflow_states.write().await;
                if let Some(state) = workflow_states.get_mut(session_id) {
                    state.current_context =
                        EditContext::PartInstance(assembly_id.clone(), instance_id.clone());
                }

                affected_instances.push((assembly_id, instance_id));
            }

            HierarchyCommand::CreateSubAssembly { parent_id, name } => {
                let sub_assembly = Assembly {
                    id: Uuid::new_v4().to_string(),
                    name,
                    children: Vec::new(),
                };

                self.add_to_assembly(
                    &mut hierarchy,
                    &parent_id,
                    HierarchyNode::SubAssembly(sub_assembly),
                )?;
            }

            HierarchyCommand::SetWorkflowStage { stage } => {
                let mut workflow_states = self.workflow_states.write().await;
                if let Some(state) = workflow_states.get_mut(session_id) {
                    state.current_stage = stage;
                    state.available_tools = self.get_tools_for_stage(stage);
                }
            }
        }

        let workflow_states = self.workflow_states.read().await;
        let workflow_state =
            workflow_states
                .get(session_id)
                .cloned()
                .unwrap_or_else(|| WorkflowState {
                    current_stage: WorkflowStage::Create,
                    current_context: EditContext::Assembly(hierarchy.root_assembly.id.clone()),
                    available_tools: self.get_tools_for_stage(WorkflowStage::Create),
                });

        Ok(HierarchyUpdate {
            hierarchy: hierarchy.clone(),
            workflow_state,
            affected_instances,
        })
    }

    fn add_to_assembly(
        &self,
        hierarchy: &mut ProjectHierarchy,
        assembly_id: &str,
        node: HierarchyNode,
    ) -> Result<(), String> {
        if hierarchy.root_assembly.id == assembly_id {
            hierarchy.root_assembly.children.push(node);
            return Ok(());
        }

        // Recursively search for the assembly
        self.add_to_assembly_recursive(&mut hierarchy.root_assembly, assembly_id, node)
    }

    fn add_to_assembly_recursive(
        &self,
        assembly: &mut Assembly,
        target_id: &str,
        node: HierarchyNode,
    ) -> Result<(), String> {
        for child in &mut assembly.children {
            if let HierarchyNode::SubAssembly(sub_assembly) = child {
                if sub_assembly.id == target_id {
                    sub_assembly.children.push(node);
                    return Ok(());
                }
                if self
                    .add_to_assembly_recursive(sub_assembly, target_id, node.clone())
                    .is_ok()
                {
                    return Ok(());
                }
            }
        }
        Err("Assembly not found".to_string())
    }

    fn find_instance<'a>(
        &self,
        hierarchy: &'a ProjectHierarchy,
        assembly_id: &str,
        instance_id: &str,
    ) -> Option<&'a PartInstance> {
        self.find_instance_in_assembly(&hierarchy.root_assembly, assembly_id, instance_id)
    }

    fn find_instance_mut<'a>(
        &self,
        hierarchy: &'a mut ProjectHierarchy,
        assembly_id: &str,
        instance_id: &str,
    ) -> Option<&'a mut PartInstance> {
        self.find_instance_in_assembly_mut(&mut hierarchy.root_assembly, assembly_id, instance_id)
    }

    fn find_instance_in_assembly<'a>(
        &self,
        assembly: &'a Assembly,
        target_assembly_id: &str,
        instance_id: &str,
    ) -> Option<&'a PartInstance> {
        if assembly.id == target_assembly_id {
            for child in &assembly.children {
                if let HierarchyNode::PartInstance(instance) = child {
                    if instance.instance_id == instance_id {
                        return Some(instance);
                    }
                }
            }
        }

        for child in &assembly.children {
            if let HierarchyNode::SubAssembly(sub_assembly) = child {
                if let Some(instance) =
                    self.find_instance_in_assembly(sub_assembly, target_assembly_id, instance_id)
                {
                    return Some(instance);
                }
            }
        }

        None
    }

    fn find_instance_in_assembly_mut<'a>(
        &self,
        assembly: &'a mut Assembly,
        target_assembly_id: &str,
        instance_id: &str,
    ) -> Option<&'a mut PartInstance> {
        if assembly.id == target_assembly_id {
            for child in &mut assembly.children {
                if let HierarchyNode::PartInstance(instance) = child {
                    if instance.instance_id == instance_id {
                        return Some(instance);
                    }
                }
            }
            return None;
        }

        // Search recursively in subassemblies
        for child in &mut assembly.children {
            if let HierarchyNode::SubAssembly(sub_assembly) = child {
                if let Some(instance) = self.find_instance_in_assembly_mut(
                    sub_assembly,
                    target_assembly_id,
                    instance_id,
                ) {
                    return Some(instance);
                }
            }
        }

        None
    }

    fn count_instances(&self, hierarchy: &ProjectHierarchy, definition_id: &str) -> u32 {
        self.count_instances_in_assembly(&hierarchy.root_assembly, definition_id)
    }

    fn count_instances_in_assembly(&self, assembly: &Assembly, definition_id: &str) -> u32 {
        let mut count = 0;

        for child in &assembly.children {
            match child {
                HierarchyNode::PartInstance(instance) => {
                    if instance.definition_id == definition_id {
                        count += 1;
                    }
                }
                HierarchyNode::SubAssembly(sub_assembly) => {
                    count += self.count_instances_in_assembly(sub_assembly, definition_id);
                }
            }
        }

        count
    }

    fn find_all_instances(
        &self,
        hierarchy: &ProjectHierarchy,
        definition_id: &str,
    ) -> Vec<(String, String)> {
        let mut instances = Vec::new();
        self.find_instances_in_assembly(&hierarchy.root_assembly, definition_id, &mut instances);
        instances
    }

    fn find_instances_in_assembly(
        &self,
        assembly: &Assembly,
        definition_id: &str,
        instances: &mut Vec<(String, String)>,
    ) {
        for child in &assembly.children {
            match child {
                HierarchyNode::PartInstance(instance) => {
                    if instance.definition_id == definition_id {
                        instances.push((assembly.id.clone(), instance.instance_id.clone()));
                    }
                }
                HierarchyNode::SubAssembly(sub_assembly) => {
                    self.find_instances_in_assembly(sub_assembly, definition_id, instances);
                }
            }
        }
    }

    fn get_tools_for_stage(&self, stage: WorkflowStage) -> Vec<String> {
        match stage {
            WorkflowStage::Create => vec![
                "new_part".to_string(),
                "new_assembly".to_string(),
                "import_part".to_string(),
                "primitive".to_string(),
            ],
            WorkflowStage::Define => vec![
                "sketch".to_string(),
                "extrude".to_string(),
                "revolve".to_string(),
                "constrain".to_string(),
            ],
            WorkflowStage::Refine => vec![
                "fillet".to_string(),
                "chamfer".to_string(),
                "pattern".to_string(),
                "transform".to_string(),
            ],
            WorkflowStage::Validate => vec![
                "measure".to_string(),
                "analyze".to_string(),
                "check_interference".to_string(),
            ],
            WorkflowStage::Output => vec![
                "export_stl".to_string(),
                "export_step".to_string(),
                "create_drawing".to_string(),
            ],
        }
    }

    pub async fn get_hierarchy(&self, session_id: &str) -> Option<ProjectHierarchy> {
        let sessions = self.sessions.read().await;
        if let Some(session) = sessions.get(session_id) {
            let hierarchy = session.read().await;
            Some(hierarchy.clone())
        } else {
            None
        }
    }

    pub async fn get_workflow_state(&self, session_id: &str) -> Option<WorkflowState> {
        let workflow_states = self.workflow_states.read().await;
        workflow_states.get(session_id).cloned()
    }
}

impl Default for HierarchyManager {
    fn default() -> Self {
        Self::new()
    }
}
