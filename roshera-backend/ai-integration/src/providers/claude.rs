use super::{CommandIntent, LLMProvider, ParsedCommand, ProviderCapabilities, ProviderError};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ClaudeProvider {
    name: String,
}

impl ClaudeProvider {
    pub fn new() -> Self {
        Self {
            name: "Claude-Direct".to_string(),
        }
    }
}

#[async_trait]
impl LLMProvider for ClaudeProvider {
    async fn process(
        &self,
        input: &str,
        context: Option<&super::ConversationContext>,
    ) -> Result<ParsedCommand, ProviderError> {
        // Parse natural language to geometry commands
        let input_lower = input.to_lowercase();

        // Log scene context if available
        if let Some(ctx) = context {
            if let Some(ref scene) = ctx.scene_state {
                tracing::info!(
                    "Processing command with scene context: {} objects",
                    scene.objects.len()
                );
                for obj in &scene.objects {
                    tracing::debug!("  - {}: {:?}", obj.name, obj.object_type);
                }
            }
        }

        let (intent, parameters) = if input_lower.contains("select all") {
            // Scene-aware command: select all objects
            if let Some(ctx) = context {
                if let Some(ref scene) = ctx.scene_state {
                    let object_ids: Vec<String> =
                        scene.objects.iter().map(|obj| obj.id.to_string()).collect();
                    let mut params = HashMap::new();
                    params.insert("objects".to_string(), serde_json::json!(object_ids));
                    (
                        CommandIntent::Query {
                            target: "select_all".to_string(),
                        },
                        params,
                    )
                } else {
                    return Err(ProviderError::InvalidInput(
                        "No objects in scene to select".to_string(),
                    ));
                }
            } else {
                return Err(ProviderError::InvalidInput(
                    "No scene context available".to_string(),
                ));
            }
        } else if input_lower.contains("how many") || input_lower.contains("count") {
            // Scene-aware query: count objects
            if let Some(ctx) = context {
                if let Some(ref scene) = ctx.scene_state {
                    let count = scene.objects.len();
                    let mut params = HashMap::new();
                    params.insert("count".to_string(), serde_json::json!(count));
                    params.insert(
                        "response".to_string(),
                        serde_json::json!(format!("There are {} objects in the scene", count)),
                    );
                    (
                        CommandIntent::Query {
                            target: "count_objects".to_string(),
                        },
                        params,
                    )
                } else {
                    let mut params = HashMap::new();
                    params.insert("count".to_string(), serde_json::json!(0));
                    params.insert(
                        "response".to_string(),
                        serde_json::json!("There are no objects in the scene"),
                    );
                    (
                        CommandIntent::Query {
                            target: "count_objects".to_string(),
                        },
                        params,
                    )
                }
            } else {
                return Err(ProviderError::InvalidInput(
                    "No scene context available".to_string(),
                ));
            }
        } else if input_lower.contains("box") {
            let width = extract_number(&input_lower).unwrap_or(2.0);
            let mut params = HashMap::new();
            params.insert("width".to_string(), serde_json::json!(width));
            params.insert("height".to_string(), serde_json::json!(width));
            params.insert("depth".to_string(), serde_json::json!(width));
            (
                CommandIntent::CreatePrimitive {
                    shape: "box".to_string(),
                },
                params,
            )
        } else if input_lower.contains("sphere") {
            let radius = extract_number(&input_lower).unwrap_or(1.0);
            let mut params = HashMap::new();
            params.insert("radius".to_string(), serde_json::json!(radius));
            (
                CommandIntent::CreatePrimitive {
                    shape: "sphere".to_string(),
                },
                params,
            )
        } else if input_lower.contains("cylinder") {
            let height = extract_number(&input_lower).unwrap_or(2.0);
            let mut params = HashMap::new();
            params.insert("radius".to_string(), serde_json::json!(1.0));
            params.insert("height".to_string(), serde_json::json!(height));
            (
                CommandIntent::CreatePrimitive {
                    shape: "cylinder".to_string(),
                },
                params,
            )
        } else if input_lower.contains("cone") {
            let mut params = HashMap::new();
            params.insert("bottom_radius".to_string(), serde_json::json!(1.0));
            params.insert("top_radius".to_string(), serde_json::json!(0.5));
            params.insert("height".to_string(), serde_json::json!(2.0));
            (
                CommandIntent::CreatePrimitive {
                    shape: "cone".to_string(),
                },
                params,
            )
        } else if input_lower.contains("torus") {
            let mut params = HashMap::new();
            params.insert("major_radius".to_string(), serde_json::json!(1.0));
            params.insert("minor_radius".to_string(), serde_json::json!(0.3));
            (
                CommandIntent::CreatePrimitive {
                    shape: "torus".to_string(),
                },
                params,
            )
        } else if input_lower.contains("list")
            || input_lower.contains("show") && input_lower.contains("objects")
        {
            // Scene-aware command: list all objects
            if let Some(ctx) = context {
                if let Some(ref scene) = ctx.scene_state {
                    let object_list: Vec<String> = scene
                        .objects
                        .iter()
                        .map(|obj| format!("{}: {:?}", obj.name, obj.object_type))
                        .collect();
                    let mut params = HashMap::new();
                    params.insert("objects".to_string(), serde_json::json!(object_list));
                    params.insert(
                        "response".to_string(),
                        serde_json::json!(if object_list.is_empty() {
                            "No objects in the scene".to_string()
                        } else {
                            format!("Objects in scene:\n{}", object_list.join("\n"))
                        }),
                    );
                    (
                        CommandIntent::Query {
                            target: "list_objects".to_string(),
                        },
                        params,
                    )
                } else {
                    let mut params = HashMap::new();
                    params.insert(
                        "objects".to_string(),
                        serde_json::json!(Vec::<String>::new()),
                    );
                    params.insert(
                        "response".to_string(),
                        serde_json::json!("No objects in the scene"),
                    );
                    (
                        CommandIntent::Query {
                            target: "list_objects".to_string(),
                        },
                        params,
                    )
                }
            } else {
                return Err(ProviderError::InvalidInput(
                    "No scene context available".to_string(),
                ));
            }
        } else if input_lower.contains("biggest") || input_lower.contains("largest") {
            // Scene-aware query: find largest object
            if let Some(ctx) = context {
                if let Some(ref scene) = ctx.scene_state {
                    let largest = scene.objects.iter().max_by_key(|obj| {
                        let bbox = &obj.bounding_box;
                        let dims = [
                            bbox.max[0] - bbox.min[0],
                            bbox.max[1] - bbox.min[1],
                            bbox.max[2] - bbox.min[2],
                        ];
                        (dims[0] * dims[1] * dims[2] * 1000.0) as i32 // Volume approximation
                    });

                    if let Some(obj) = largest {
                        let mut params = HashMap::new();
                        params.insert(
                            "object_id".to_string(),
                            serde_json::json!(obj.id.to_string()),
                        );
                        params.insert("object_name".to_string(), serde_json::json!(obj.name));
                        params.insert(
                            "response".to_string(),
                            serde_json::json!(format!(
                                "The largest object is {} ({:?})",
                                obj.name, obj.object_type
                            )),
                        );
                        (
                            CommandIntent::Query {
                                target: "find_largest".to_string(),
                            },
                            params,
                        )
                    } else {
                        return Err(ProviderError::InvalidInput(
                            "No objects in scene".to_string(),
                        ));
                    }
                } else {
                    return Err(ProviderError::InvalidInput(
                        "No objects in scene".to_string(),
                    ));
                }
            } else {
                return Err(ProviderError::InvalidInput(
                    "No scene context available".to_string(),
                ));
            }
        } else if input_lower.contains("delete")
            && (input_lower.contains("all") || input_lower.contains("everything"))
        {
            // Scene-aware command: delete all objects
            if let Some(ctx) = context {
                if let Some(ref scene) = ctx.scene_state {
                    if scene.objects.is_empty() {
                        return Err(ProviderError::InvalidInput(
                            "No objects to delete".to_string(),
                        ));
                    }
                    let object_ids: Vec<String> =
                        scene.objects.iter().map(|obj| obj.id.to_string()).collect();
                    let mut params = HashMap::new();
                    params.insert("objects".to_string(), serde_json::json!(object_ids));
                    params.insert("action".to_string(), serde_json::json!("delete_all"));
                    (
                        CommandIntent::Transform {
                            operation: "delete_all".to_string(),
                        },
                        params,
                    )
                } else {
                    return Err(ProviderError::InvalidInput(
                        "No objects to delete".to_string(),
                    ));
                }
            } else {
                return Err(ProviderError::InvalidInput(
                    "No scene context available".to_string(),
                ));
            }
        } else {
            // Provide context-aware error message
            if let Some(ctx) = context {
                if let Some(ref scene) = ctx.scene_state {
                    if scene.objects.is_empty() {
                        return Err(ProviderError::InvalidInput(
                            format!("I don't understand '{}'. The scene is empty. Try: 'create a box', 'make a sphere', or 'create a cylinder'", input)
                        ));
                    } else {
                        return Err(ProviderError::InvalidInput(
                            format!("I don't understand '{}'. You have {} objects in the scene. Try: 'list objects', 'select all', 'count objects', or create new geometry", input, scene.objects.len())
                        ));
                    }
                } else {
                    return Err(ProviderError::InvalidInput(
                        format!("I don't understand '{}'. Try: 'create a box', 'make a sphere', 'create a cylinder'", input)
                    ));
                }
            } else {
                return Err(ProviderError::InvalidInput(
                    format!("I don't understand '{}'. Try: 'create a box', 'make a sphere', 'create a cylinder'", input)
                ));
            }
        };

        Ok(ParsedCommand {
            original_text: input.to_string(),
            intent,
            parameters,
            confidence: 0.95,
            language: "en".to_string(),
        })
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            name: "Claude Direct".to_string(),
            version: "1.0".to_string(),
            supported_languages: vec!["en".to_string()],
            max_context_length: 4096,
            supports_streaming: false,
            supports_batching: false,
            device_type: "cpu".to_string(),
            model_size_mb: 0,
            quantization: super::QuantizationType::Float32,
        }
    }

    async fn generate(&self, prompt: &str, _max_tokens: usize) -> Result<String, ProviderError> {
        // Generate helpful responses
        Ok(format!("I can help you create 3D objects. You said: '{}'. Try commands like 'create a box' or 'make a sphere'.", prompt))
    }

    async fn generate_response(
        &self,
        command_result: &str,
        _language: &str,
    ) -> Result<String, ProviderError> {
        // Generate response based on command result
        Ok(format!("Command executed successfully: {}", command_result))
    }

    fn memory_requirement_mb(&self) -> usize {
        // Claude integration requires minimal memory
        1
    }
}

fn extract_number(text: &str) -> Option<f64> {
    // Simple number extraction from text
    let words: Vec<&str> = text.split_whitespace().collect();
    for word in words {
        if let Ok(num) = word.parse::<f64>() {
            return Some(num);
        }
    }
    // Look for written numbers
    if text.contains("one") {
        return Some(1.0);
    }
    if text.contains("two") {
        return Some(2.0);
    }
    if text.contains("three") {
        return Some(3.0);
    }
    if text.contains("four") {
        return Some(4.0);
    }
    if text.contains("five") {
        return Some(5.0);
    }
    None
}
