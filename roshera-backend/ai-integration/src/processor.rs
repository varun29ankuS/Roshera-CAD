use crate::executor::CommandExecutor;
/// AI command processor using the vendor-agnostic provider system
///
/// # Design Rationale
/// - **Why provider-based**: Allows runtime switching between AI models
/// - **Why async**: AI inference is I/O bound, prevents blocking
/// - **Performance**: End-to-end < 600ms (ASR: 500ms + LLM: 100ms)
/// - **Business Value**: Flexible AI integration without vendor lock-in
use crate::providers::{
    AudioFormat, CommandIntent, ConversationContext, ParsedCommand, ProviderManager,
};
use crate::{Operation, VoiceCommand};
use shared_types::{Command, CommandResult, GeometryId, PrimitiveType, ShapeParameters};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

/// Result of command processing
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProcessedCommand {
    /// Original input text
    pub original_text: String,
    /// Parsed command
    pub command: ParsedCommand,
    /// Execution result
    pub result: CommandResult,
    /// Execution time in milliseconds
    pub execution_time_ms: u64,
}

/// Main AI processor coordinating ASR, LLM, and command execution
pub struct AIProcessor {
    provider_manager: Arc<Mutex<ProviderManager>>,
    executor: Arc<Mutex<CommandExecutor>>,
    context: Arc<Mutex<ConversationContext>>,
}

// Helper functions for creating CommandResult instances
// Architecture: Accept impl Into<String> for flexibility with &str and String types
fn create_success_result(
    message: impl Into<String>,
    object_id: Option<GeometryId>,
) -> CommandResult {
    let mut result = CommandResult::success(message.into());
    result.object_id = object_id;
    result
}

fn create_error_result(message: impl Into<String>) -> CommandResult {
    let mut result = CommandResult::success("Command failed");
    result.success = false;
    result.error = Some(message.into());
    result
}

fn create_query_result(message: impl Into<String>, data: serde_json::Value) -> CommandResult {
    let mut result = CommandResult::success(message.into());
    result.data = Some(data);
    result
}

impl AIProcessor {
    /// Create new AI processor
    ///
    /// # Example
    /// ```
    /// let processor = AIProcessor::new(provider_manager, executor);
    /// ```
    pub fn new(
        provider_manager: Arc<Mutex<ProviderManager>>,
        executor: Arc<Mutex<CommandExecutor>>,
    ) -> Self {
        // Generate session ID as UUID for type safety, convert to string for API/serialization
        // Architecture: UUID internally for performance, String for external interfaces
        let session_uuid = Uuid::new_v4();
        let session_id_for_api = session_uuid.to_string();

        let context = Arc::new(Mutex::new(ConversationContext {
            session_id: session_id_for_api, // String for JSON serialization and API consistency
            previous_commands: Vec::new(),
            active_objects: Vec::new(),
            user_preferences: serde_json::json!({}),
            scene_state: None,
            system_context: Some(shared_types::SystemContext::default()),
        }));

        Self {
            provider_manager,
            executor,
            context,
        }
    }

    /// Process voice input end-to-end
    ///
    /// # Performance
    /// - Target: < 600ms total
    /// - ASR: ~500ms
    /// - LLM: ~100ms
    /// - Execution: < 10ms
    pub async fn process_voice(
        &self,
        audio: &[u8],
        format: AudioFormat,
    ) -> Result<ProcessedCommand, Box<dyn std::error::Error + Send + Sync + 'static>> {
        let start = std::time::Instant::now();

        // Step 1: Speech to text
        let text = {
            let manager = self.provider_manager.lock().await;
            let asr = manager
                .asr()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync + 'static>)?;
            asr.transcribe(audio, format)
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync + 'static>)?
        };

        tracing::info!("Transcribed: {}", text);

        // Step 2: Process text
        let result = self.process_text(&text).await?;

        let execution_time_ms = start.elapsed().as_millis() as u64;
        Ok(ProcessedCommand {
            execution_time_ms,
            ..result
        })
    }

    /// Process text input
    pub async fn process_text(
        &self,
        text: &str,
    ) -> Result<ProcessedCommand, Box<dyn std::error::Error + Send + Sync + 'static>> {
        let start = std::time::Instant::now();

        // Step 1: Parse with LLM
        let parsed_command = {
            let manager = self.provider_manager.lock().await;
            let llm = manager
                .llm()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync + 'static>)?;
            let context = self.context.lock().await;
            llm.process(text, Some(&context))
                .await
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync + 'static>)?
        };

        tracing::info!("Parsed command: {:?}", parsed_command);

        // Step 2: Execute command
        let result = self.execute_command(&parsed_command).await?;

        // Step 3: Update context
        {
            let mut context = self.context.lock().await;
            context.previous_commands.push(parsed_command.clone());
            if context.previous_commands.len() > 10 {
                context.previous_commands.remove(0);
            }
        }

        let execution_time_ms = start.elapsed().as_millis() as u64;

        Ok(ProcessedCommand {
            original_text: text.to_string(),
            command: parsed_command,
            result,
            execution_time_ms,
        })
    }

    /// Execute parsed command
    async fn execute_command(
        &self,
        command: &ParsedCommand,
    ) -> Result<CommandResult, Box<dyn std::error::Error + Send + Sync + 'static>> {
        let mut executor = self.executor.lock().await;

        match &command.intent {
            CommandIntent::CreatePrimitive { shape } => {
                let params = &command.parameters;
                let geometry_command = match shape.as_str() {
                    "sphere" => {
                        let radius = params.get("radius").and_then(|v| v.as_f64()).unwrap_or(1.0);
                        Command::CreateSphere { radius }
                    }
                    "box" => {
                        let width = params.get("width").and_then(|v| v.as_f64()).unwrap_or(1.0);
                        let height = params.get("height").and_then(|v| v.as_f64()).unwrap_or(1.0);
                        let depth = params.get("depth").and_then(|v| v.as_f64()).unwrap_or(1.0);
                        Command::CreateBox {
                            width,
                            height,
                            depth,
                        }
                    }
                    "cylinder" => {
                        let radius = params.get("radius").and_then(|v| v.as_f64()).unwrap_or(1.0);
                        let height = params.get("height").and_then(|v| v.as_f64()).unwrap_or(2.0);
                        Command::CreateCylinder { radius, height }
                    }
                    _ => return Ok(create_error_result(&format!("Unknown shape: {}", shape))),
                };

                match executor.execute(geometry_command).await {
                    Ok(id) => {
                        let mut result =
                            CommandResult::success(format!("Created {} successfully", shape));
                        result.object_id = Some(id);
                        Ok(result)
                    }
                    Err(e) => {
                        let mut result = CommandResult::success("Command failed".to_string());
                        result.success = false;
                        result.error = Some(e.to_string());
                        Ok(result)
                    }
                }
            }
            CommandIntent::BooleanOperation { operation } => {
                // For Boolean operations, we need existing objects from the executor
                // In a real implementation, we would get these from the current scene
                // For now, use the first two objects if available
                let all_objects = executor.get_all_objects();

                if all_objects.len() < 2 {
                    return Ok(create_error_result("Boolean operations require at least 2 existing objects. Please create some objects first."));
                }

                let object_a = all_objects[0].clone();
                let object_b = all_objects[1].clone();

                let geometry_command = match operation.as_str() {
                    "union" | "combine" | "merge" => Command::BooleanUnion { object_a, object_b },
                    "intersection" | "intersect" | "overlap" => {
                        Command::BooleanIntersection { object_a, object_b }
                    }
                    "difference" | "subtract" | "cut" => {
                        Command::BooleanDifference { object_a, object_b }
                    }
                    _ => {
                        return Ok(create_error_result(&format!(
                            "Unknown boolean operation: {}",
                            operation
                        )))
                    }
                };

                match executor.execute(geometry_command).await {
                    Ok(id) => Ok(create_success_result(
                        &format!("Boolean {} operation completed successfully", operation),
                        Some(id),
                    )),
                    Err(e) => Ok(create_error_result(&e.to_string())),
                }
            }
            CommandIntent::Transform { operation } => {
                // For Transform operations, we need an existing object from the executor
                let all_objects = executor.get_all_objects();

                if all_objects.is_empty() {
                    return Ok(create_error_result("Transform operations require at least 1 existing object. Please create an object first."));
                }

                // Use the most recently created object (last in the list)
                let object = all_objects[all_objects.len() - 1].clone();

                // Create transform based on operation
                let transform = match operation.as_str() {
                    "translate" | "move" => {
                        let offset = [
                            command
                                .parameters
                                .get("x")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0) as f32,
                            command
                                .parameters
                                .get("y")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0) as f32,
                            command
                                .parameters
                                .get("z")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0) as f32,
                        ];
                        shared_types::GeometryTransform::Translate { offset }
                    }
                    "rotate" | "spin" => {
                        let axis = [
                            command
                                .parameters
                                .get("axis_x")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0) as f32,
                            command
                                .parameters
                                .get("axis_y")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0) as f32,
                            command
                                .parameters
                                .get("axis_z")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(1.0) as f32, // Default to Z axis
                        ];
                        let angle = command
                            .parameters
                            .get("angle")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(90.0);
                        shared_types::GeometryTransform::Rotate {
                            axis,
                            angle_radians: angle.to_radians(),
                        }
                    }
                    "scale" | "resize" => {
                        let factors = [
                            command
                                .parameters
                                .get("scale_x")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(1.0) as f32,
                            command
                                .parameters
                                .get("scale_y")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(1.0) as f32,
                            command
                                .parameters
                                .get("scale_z")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(1.0) as f32,
                        ];
                        shared_types::GeometryTransform::Scale { factors }
                    }
                    _ => {
                        return Ok(create_error_result(&format!(
                            "Unknown transform operation: {}",
                            operation
                        )))
                    }
                };

                let geometry_command = Command::Transform { object, transform };

                match executor.execute(geometry_command).await {
                    Ok(id) => Ok(create_success_result(
                        &format!("Transform {} operation completed successfully", operation),
                        Some(id),
                    )),
                    Err(e) => Ok(create_error_result(&e.to_string())),
                }
            }
            CommandIntent::Extrude { .. } => {
                // Return a special result that indicates user interaction needed
                Ok(create_error_result("Please select a face to extrude or specify a face index. For example: 'extrude face 2 by 5'"))
            }
            CommandIntent::Query { target } => {
                // Handle scene-aware queries
                match target.as_str() {
                    "count_objects" => {
                        if let Some(response) = command.parameters.get("response") {
                            Ok(create_query_result(
                                "Query result",
                                serde_json::json!({
                                    "query": target,
                                    "result": response,
                                    "count": command.parameters.get("count")
                                }),
                            ))
                        } else {
                            Ok(create_query_result(
                                "Query result",
                                serde_json::json!({
                                    "query": target,
                                    "result": "Count query processed"
                                }),
                            ))
                        }
                    }
                    "list_objects" => {
                        if let Some(response) = command.parameters.get("response") {
                            Ok(create_query_result(
                                "Query result",
                                serde_json::json!({
                                    "query": target,
                                    "result": response,
                                    "objects": command.parameters.get("objects")
                                }),
                            ))
                        } else {
                            Ok(create_query_result(
                                "Query result",
                                serde_json::json!({
                                    "query": target,
                                    "result": "List query processed"
                                }),
                            ))
                        }
                    }
                    "find_largest" => {
                        if let Some(response) = command.parameters.get("response") {
                            Ok(create_query_result(
                                "Query result",
                                serde_json::json!({
                                    "query": target,
                                    "result": response,
                                    "object_id": command.parameters.get("object_id"),
                                    "object_name": command.parameters.get("object_name")
                                }),
                            ))
                        } else {
                            Ok(create_query_result(
                                "Query result",
                                serde_json::json!({
                                    "query": target,
                                    "result": "Largest object query processed"
                                }),
                            ))
                        }
                    }
                    "select_all" => {
                        // This would trigger selection in the frontend
                        Ok(create_query_result(
                            "Query result",
                            serde_json::json!({
                                "query": target,
                                "result": "Selected all objects",
                                "objects": command.parameters.get("objects")
                            }),
                        ))
                    }
                    _ => {
                        // Check if there's a response from the LLM
                        if let Some(response) = command.parameters.get("response") {
                            Ok(create_query_result(
                                "Query result",
                                serde_json::json!({
                                    "query": target,
                                    "result": response
                                }),
                            ))
                        } else {
                            Ok(create_query_result(
                                "Query result",
                                serde_json::json!({
                                    "query": target,
                                    "result": "Query processed"
                                }),
                            ))
                        }
                    }
                }
            }
            CommandIntent::Create {
                object_type,
                parameters,
            } => Ok(create_error_result(format!(
                "Create command for {} not yet implemented",
                object_type
            ))),
            CommandIntent::Modify {
                target,
                operation,
                parameters,
            } => Ok(create_error_result(format!(
                "Modify command {} on {} not yet implemented",
                operation, target
            ))),
            CommandIntent::Boolean {
                operation,
                operands,
            } => Ok(create_error_result(format!(
                "Boolean {} operation not yet implemented",
                operation
            ))),
            CommandIntent::Export { format, options } => Ok(create_error_result(format!(
                "Export to {} not yet implemented",
                format
            ))),
            CommandIntent::Import { file_path, format } => Ok(create_error_result(format!(
                "Import from {} not yet implemented",
                file_path
            ))),
            CommandIntent::Unknown => Ok(create_error_result("Could not understand command")),
        }
    }

    /// Generate audio response using TTS
    pub async fn generate_audio_response(
        &self,
        text: &str,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync + 'static>> {
        let manager = self.provider_manager.lock().await;

        // TTS is optional
        if let Some(tts_name) = &manager.active_tts {
            if let Some(tts) = manager.tts_providers.get(tts_name) {
                return tts.synthesize(text, None).await.map_err(|e| {
                    Box::new(e) as Box<dyn std::error::Error + Send + Sync + 'static>
                });
            }
        }

        // Return empty audio if no TTS
        Ok(vec![])
    }

    /// Alias for generate_audio_response for compatibility
    pub async fn synthesize_response(
        &self,
        text: &str,
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync + 'static>> {
        self.generate_audio_response(text).await
    }

    /// Process text command and return VoiceCommand (simplified interface)
    /// This now follows the proper CAD workflow: workflow activation → plane selection → geometry creation
    pub async fn process_text_command(
        &self,
        text: &str,
    ) -> Result<VoiceCommand, Box<dyn std::error::Error + Send + Sync + 'static>> {
        let processed = self.process_text(text).await?;

        // Convert ParsedCommand to VoiceCommand
        match &processed.command.intent {
            CommandIntent::CreatePrimitive { shape } => {
                // For any geometry creation, we need to follow the workflow:
                // 1. Activate Part workflow (Create → Define)
                // 2. Select or create a sketch plane (default XY)
                // 3. Create the geometry with plane context

                let primitive = match shape.as_str() {
                    "sphere" => PrimitiveType::Sphere,
                    "box" => PrimitiveType::Box,
                    "cylinder" => PrimitiveType::Cylinder,
                    "cone" => PrimitiveType::Cone,
                    _ => PrimitiveType::Sphere, // Default
                };

                // Extract parameters from processed command
                let params = &processed.command.parameters;
                let parameters = match primitive {
                    PrimitiveType::Sphere => {
                        let radius = params.get("radius").and_then(|v| v.as_f64()).unwrap_or(1.0);
                        ShapeParameters::sphere_params(radius)
                    }
                    PrimitiveType::Box => {
                        let width = params.get("width").and_then(|v| v.as_f64()).unwrap_or(1.0);
                        let height = params.get("height").and_then(|v| v.as_f64()).unwrap_or(1.0);
                        let depth = params.get("depth").and_then(|v| v.as_f64()).unwrap_or(1.0);
                        ShapeParameters::box_params(width, height, depth)
                    }
                    PrimitiveType::Cylinder => {
                        let radius = params.get("radius").and_then(|v| v.as_f64()).unwrap_or(1.0);
                        let height = params.get("height").and_then(|v| v.as_f64()).unwrap_or(2.0);
                        ShapeParameters::cylinder_params(radius, height)
                    }
                    PrimitiveType::Cone => {
                        let radius = params.get("radius").and_then(|v| v.as_f64()).unwrap_or(1.0);
                        let height = params.get("height").and_then(|v| v.as_f64()).unwrap_or(2.0);
                        // Use cylinder params for cone (similar structure)
                        ShapeParameters::cylinder_params(radius, height)
                    }
                    _ => {
                        // Default to box params for unsupported types
                        ShapeParameters::box_params(1.0, 1.0, 1.0)
                    }
                };

                // Use Part Maturity workflow for consistency
                // Default to XY plane if not specified
                Ok(VoiceCommand::ActivatePartMaturityWorkflow {
                    primitive,
                    parameters,
                    sketch_plane: "XY".to_string(), // Default plane as String for consistency
                    natural_text: processed.original_text.clone(),
                })
            }
            CommandIntent::BooleanOperation { .. } | CommandIntent::Transform { .. } => {
                // Convert string operation to Operation enum
                let operation = Operation::Move {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                }; // Default for now
                Ok(VoiceCommand::Modify {
                    target: Uuid::new_v4(), // Default target
                    operation,
                    parameters: HashMap::new(),
                })
            }
            CommandIntent::Query { target } => Ok(VoiceCommand::Query {
                question: target.clone(),
                target: None,
            }),
            CommandIntent::Create {
                object_type,
                parameters,
            } => Err(format!("Create {} not supported in voice commands yet", object_type).into()),
            CommandIntent::Modify {
                target,
                operation,
                parameters,
            } => Err(format!(
                "Modify {} operation not supported in voice commands yet",
                operation
            )
            .into()),
            CommandIntent::Boolean {
                operation,
                operands,
            } => Err(format!("Boolean {} not supported in voice commands yet", operation).into()),
            CommandIntent::Export { format, options } => {
                Err(format!("Export to {} not supported in voice commands yet", format).into())
            }
            CommandIntent::Import { file_path, format } => Err(format!(
                "Import from {} not supported in voice commands yet",
                file_path
            )
            .into()),
            CommandIntent::Unknown => Err("Unknown command".into()),
            CommandIntent::Extrude { target } => Ok(VoiceCommand::Extrude {
                target: target.as_ref().and_then(|t| Uuid::parse_str(t).ok()),
                face_index: None,
                direction: None,
                distance: None,
                natural_text: text.to_string(), // Convert to String for API consistency
            }),
        }
    }

    /// Get conversation context
    pub async fn get_context(&self) -> ConversationContext {
        self.context.lock().await.clone()
    }

    /// Clear conversation context
    pub async fn clear_context(&self) {
        let mut context = self.context.lock().await;
        context.previous_commands.clear();
        context.active_objects.clear();
        context.scene_state = None;
        context.system_context = Some(shared_types::SystemContext::default());
    }

    /// Update scene state for AI awareness
    pub async fn update_scene_state(&self, scene_state: shared_types::SceneState) {
        let mut context = self.context.lock().await;

        // Update active objects list from scene
        context.active_objects = scene_state
            .objects
            .iter()
            .map(|obj| obj.id.to_string())
            .collect();

        // Store the full scene state
        context.scene_state = Some(scene_state);

        tracing::info!(
            "Updated AI context with {} objects in scene",
            context.active_objects.len()
        );
    }

    /// Update system context for AI awareness
    pub async fn update_system_context(&self, system_context: shared_types::SystemContext) {
        let mut context = self.context.lock().await;
        context.system_context = Some(system_context);

        tracing::info!(
            "Updated AI system context - users: {}, workflow: {:?}, AI: {}",
            context
                .system_context
                .as_ref()
                .map(|c| c.session.connected_users.len())
                .unwrap_or(0),
            context
                .system_context
                .as_ref()
                .map(|c| &c.workflow.current_stage),
            context
                .system_context
                .as_ref()
                .map(|c| c.status.ai_connected)
                .unwrap_or(false)
        );
    }

    /// Get scene-aware command suggestions
    pub async fn get_suggestions(&self) -> Vec<String> {
        let context = self.context.lock().await;
        // Initialize as Vec<String> for API consistency - hybrid architecture uses owned strings for external interfaces
        let mut suggestions: Vec<String> = vec![
            "Create a box".to_string(),
            "Create a sphere".to_string(),
            "Create a cylinder".to_string(),
        ];

        if let Some(ref scene) = context.scene_state {
            // Add suggestions based on scene content
            if !scene.objects.is_empty() {
                suggestions.push("Select all objects".to_string());
                suggestions.push("Delete selected objects".to_string());

                // Add object-specific suggestions
                for obj in &scene.objects {
                    match &obj.object_type {
                        shared_types::ObjectType::Box { .. } => {
                            suggestions.push(format!("Extrude face of {}", obj.name));
                        }
                        _ => {}
                    }
                }

                if scene.objects.len() >= 2 {
                    suggestions.push("Boolean union selected objects".to_string());
                    suggestions.push("Boolean difference selected objects".to_string());
                }
            }
        }

        suggestions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::mock::{MockASRProvider, MockLLMProvider};

    #[tokio::test]
    async fn test_processor_text() {
        // Create mock providers
        let mut manager = ProviderManager::new();
        manager.register_asr("mock".to_string(), Box::new(MockASRProvider::new()));
        manager.register_llm("mock".to_string(), Box::new(MockLLMProvider::new()));
        manager.set_active("mock".to_string(), "mock".to_string(), None);

        let provider_manager = Arc::new(Mutex::new(manager));
        let executor = Arc::new(Mutex::new(CommandExecutor::new()));

        let processor = AIProcessor::new(provider_manager, executor);

        // Test text processing
        let result = processor
            .process_text("create a sphere with radius 5")
            .await
            .unwrap();
        assert!(matches!(
            result.command.intent,
            CommandIntent::CreatePrimitive { .. }
        ));
        assert!(result.execution_time_ms < 100); // Should be fast with mocks
    }

    #[tokio::test]
    async fn test_context_management() {
        let provider_manager = Arc::new(Mutex::new(ProviderManager::new()));
        let executor = Arc::new(Mutex::new(CommandExecutor::new()));
        let processor = AIProcessor::new(provider_manager, executor);

        // Test context
        let context = processor.get_context().await;
        assert!(context.previous_commands.is_empty());

        // Clear context
        processor.clear_context().await;
        let context = processor.get_context().await;
        assert!(context.previous_commands.is_empty());
    }
}
