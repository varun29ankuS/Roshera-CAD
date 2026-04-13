use crate::providers::{
    CommandIntent, ConversationContext, LLMProvider, ParsedCommand, ProviderCapabilities,
    ProviderError,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Ollama provider for local LLM inference (TinyLLaMA, Llama, etc.)
#[derive(Debug)]
pub struct OllamaProvider {
    base_url: String,
    model: String,
}

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    system: String,
    temperature: f32,
    format: Option<String>,
    stream: bool,
}

#[derive(Deserialize)]
struct OllamaStreamResponse {
    model: String,
    created_at: String,
    response: String,
    done: bool,
}

#[derive(Deserialize)]
struct OllamaResponse {
    response: String,
}

impl OllamaProvider {
    pub fn new(model: String) -> Self {
        Self {
            base_url: "http://localhost:11434".to_string(),
            model: model, // Use the provided model (e.g., mistral:7b-instruct)
        }
    }

    /// Fast local pattern matching for common commands to avoid LLM overhead
    fn try_fast_parse(&self, input: &str) -> Option<ParsedCommand> {
        use regex::Regex;
        use std::collections::HashMap;

        let input_lower = input.to_lowercase();
        let mut parameters = HashMap::new();

        // Box creation with dimensions (5,10,20 or 5x10x20 or "5 10 20")
        if let Some(caps) = Regex::new(r"(?:make|create|add)?\s*(?:a\s+)?box.*?(\d+(?:\.\d+)?)[,x\s]+(\d+(?:\.\d+)?)[,x\s]+(\d+(?:\.\d+)?)").unwrap().captures(&input_lower) {
            let width = caps.get(1).unwrap().as_str().parse::<f64>().unwrap();
            let height = caps.get(2).unwrap().as_str().parse::<f64>().unwrap();
            let depth = caps.get(3).unwrap().as_str().parse::<f64>().unwrap();
            parameters.insert("shape".to_string(), serde_json::Value::String("box".to_string()));
            parameters.insert("width".to_string(), serde_json::Value::Number(serde_json::Number::from_f64(width).unwrap()));
            parameters.insert("height".to_string(), serde_json::Value::Number(serde_json::Number::from_f64(height).unwrap()));
            parameters.insert("depth".to_string(), serde_json::Value::Number(serde_json::Number::from_f64(depth).unwrap()));
            parameters.insert("response".to_string(), serde_json::Value::String(
                format!("Creating a box with dimensions {}x{}x{}", width, height, depth)
            ));

            return Some(ParsedCommand {
                original_text: input.to_string(),
                intent: CommandIntent::CreatePrimitive { shape: "box".to_string() },
                parameters,
                confidence: 0.95,
                language: "en".to_string(),
            });
        }

        // Simple box
        if input_lower.contains("box")
            && (input_lower.contains("make")
                || input_lower.contains("create")
                || input_lower.contains("add"))
        {
            parameters.insert(
                "shape".to_string(),
                serde_json::Value::String("box".to_string()),
            );
            parameters.insert(
                "width".to_string(),
                serde_json::Value::Number(serde_json::Number::from_f64(10.0).unwrap()),
            );
            parameters.insert(
                "height".to_string(),
                serde_json::Value::Number(serde_json::Number::from_f64(10.0).unwrap()),
            );
            parameters.insert(
                "depth".to_string(),
                serde_json::Value::Number(serde_json::Number::from_f64(10.0).unwrap()),
            );
            parameters.insert(
                "response".to_string(),
                serde_json::Value::String("Creating a 10x10x10 box".to_string()),
            );

            return Some(ParsedCommand {
                original_text: input.to_string(),
                intent: CommandIntent::CreatePrimitive {
                    shape: "box".to_string(),
                },
                parameters,
                confidence: 0.95,
                language: "en".to_string(),
            });
        }

        // Sphere creation with radius (e.g., "create sphere radius 8" or "sphere with radius 3.5")
        if let Some(caps) =
            Regex::new(r"(?:make|create|add)?\s*(?:a\s+)?sphere.*?(?:radius|r)\s*(\d+(?:\.\d+)?)")
                .unwrap()
                .captures(&input_lower)
        {
            let radius = caps.get(1).unwrap().as_str().parse::<f64>().unwrap();

            parameters.insert(
                "shape".to_string(),
                serde_json::Value::String("sphere".to_string()),
            );
            parameters.insert(
                "radius".to_string(),
                serde_json::Value::Number(serde_json::Number::from_f64(radius).unwrap()),
            );
            parameters.insert(
                "response".to_string(),
                serde_json::Value::String(format!("Creating a sphere with radius {}", radius)),
            );

            return Some(ParsedCommand {
                original_text: input.to_string(),
                intent: CommandIntent::CreatePrimitive {
                    shape: "sphere".to_string(),
                },
                parameters,
                confidence: 0.95,
                language: "en".to_string(),
            });
        }

        // Simple sphere without radius
        if input_lower.contains("sphere")
            && (input_lower.contains("make")
                || input_lower.contains("create")
                || input_lower.contains("add"))
        {
            parameters.insert(
                "shape".to_string(),
                serde_json::Value::String("sphere".to_string()),
            );
            parameters.insert(
                "radius".to_string(),
                serde_json::Value::Number(serde_json::Number::from_f64(5.0).unwrap()),
            );
            parameters.insert(
                "response".to_string(),
                serde_json::Value::String("Creating a sphere with radius 5".to_string()),
            );

            return Some(ParsedCommand {
                original_text: input.to_string(),
                intent: CommandIntent::CreatePrimitive {
                    shape: "sphere".to_string(),
                },
                parameters,
                confidence: 0.95,
                language: "en".to_string(),
            });
        }

        // Cylinder creation with dimensions
        if let Some(caps) = Regex::new(r"(?:make|create|add)?\s*(?:a\s+)?cylinder.*?(?:radius|r)\s*(\d+(?:\.\d+)?).*?(?:height|h)\s*(\d+(?:\.\d+)?)").unwrap().captures(&input_lower) {
            let radius = caps.get(1).unwrap().as_str().parse::<f64>().unwrap();
            let height = caps.get(2).unwrap().as_str().parse::<f64>().unwrap();

            parameters.insert("shape".to_string(), serde_json::Value::String("cylinder".to_string()));
            parameters.insert("radius".to_string(), serde_json::Value::Number(serde_json::Number::from_f64(radius).unwrap()));
            parameters.insert("height".to_string(), serde_json::Value::Number(serde_json::Number::from_f64(height).unwrap()));
            parameters.insert("response".to_string(), serde_json::Value::String(
                format!("Creating a cylinder with radius {} and height {}", radius, height)
            ));

            return Some(ParsedCommand {
                original_text: input.to_string(),
                intent: CommandIntent::CreatePrimitive { shape: "cylinder".to_string() },
                parameters,
                confidence: 0.95,
                language: "en".to_string(),
            });
        }

        // Simple cylinder
        if input_lower.contains("cylinder")
            && (input_lower.contains("make")
                || input_lower.contains("create")
                || input_lower.contains("add"))
        {
            parameters.insert(
                "shape".to_string(),
                serde_json::Value::String("cylinder".to_string()),
            );
            parameters.insert(
                "radius".to_string(),
                serde_json::Value::Number(serde_json::Number::from_f64(5.0).unwrap()),
            );
            parameters.insert(
                "height".to_string(),
                serde_json::Value::Number(serde_json::Number::from_f64(10.0).unwrap()),
            );
            parameters.insert(
                "response".to_string(),
                serde_json::Value::String(
                    "Creating a cylinder with radius 5 and height 10".to_string(),
                ),
            );

            return Some(ParsedCommand {
                original_text: input.to_string(),
                intent: CommandIntent::CreatePrimitive {
                    shape: "cylinder".to_string(),
                },
                parameters,
                confidence: 0.95,
                language: "en".to_string(),
            });
        }

        // Quick responses for common queries
        if input_lower.contains("connected") || input_lower.contains("connection") {
            parameters.insert(
                "target".to_string(),
                serde_json::Value::String("connection_status".to_string()),
            );
            parameters.insert(
                "response".to_string(),
                serde_json::Value::String(
                    "Yes, I'm connected and ready to help you with 3D modeling!".to_string(),
                ),
            );

            return Some(ParsedCommand {
                original_text: input.to_string(),
                intent: CommandIntent::Query {
                    target: "connection_status".to_string(),
                },
                parameters,
                confidence: 1.0,
                language: "en".to_string(),
            });
        }

        // Only keep fast pattern matching for basic connection check
        // Let TinyLLaMA handle all other queries for natural responses

        None
    }

    /// Create the system prompt that makes the LLM behave as a CAD assistant
    fn create_system_prompt(&self, context: Option<&ConversationContext>) -> String {
        let mut prompt = r#"You are Roshera CAD Assistant, an AI specialized in 3D modeling and CAD operations. Your role is to help users create and manipulate 3D geometry using natural language.

IMPORTANT INSTRUCTIONS:
1. You ONLY respond with valid JSON in the exact format specified below
2. You interpret user commands and convert them to CAD operations
3. You are aware of the current scene state and objects
4. You provide helpful responses while staying focused on CAD tasks

Available operations you can perform:
- Create primitives: box, sphere, cylinder, cone, torus
- Boolean operations: union, intersection, difference
- Transformations: move, rotate, scale
- Queries: count objects, list objects, find largest, select objects

RESPONSE FORMAT (You MUST respond ONLY with this JSON structure):
{
  "intent": "create_primitive|boolean_operation|transform|query|unknown",
  "parameters": {
    // Parameters specific to the operation
  },
  "confidence": 0.0-1.0,
  "response": "Human-readable response to show the user"
}

EXAMPLES:

User: "Create a box with width 5"
{
  "intent": "create_primitive",
  "parameters": {
    "shape": "box",
    "width": 5.0,
    "height": 10.0,
    "depth": 10.0
  },
  "confidence": 0.9,
  "response": "I'll create a box with width 5 units"
}

User: "Create a box with width 2 and height 3"
{
  "intent": "create_primitive",
  "parameters": {
    "shape": "box",
    "width": 2.0,
    "height": 3.0,
    "depth": 10.0
  },
  "confidence": 0.95,
  "response": "I'll create a box with width 2 units and height 3 units"
}

User: "make a box with 5,10,20 dimensions"
{
  "intent": "create_primitive",
  "parameters": {
    "shape": "box",
    "width": 5.0,
    "height": 10.0,
    "depth": 20.0
  },
  "confidence": 0.95,
  "response": "Creating a box with width 5, height 10, and depth 20 units"
}

User: "create a box 2x3x4"
{
  "intent": "create_primitive",
  "parameters": {
    "shape": "box",
    "width": 2.0,
    "height": 3.0,
    "depth": 4.0
  },
  "confidence": 0.9,
  "response": "Creating a 2x3x4 box"
}

User: "How many objects are in the scene?"
{
  "intent": "query",
  "parameters": {
    "target": "count_objects"
  },
  "confidence": 1.0,
  "response": "Let me count the objects in the scene"
}

User: "Make a sphere"
{
  "intent": "create_primitive",
  "parameters": {
    "shape": "sphere",
    "radius": 1.0
  },
  "confidence": 0.95,
  "response": "I'll create a sphere with default radius of 1 unit"
}

User: "lets create a box then"
{
  "intent": "create_primitive",
  "parameters": {
    "shape": "box",
    "width": 10.0,
    "height": 10.0,
    "depth": 10.0
  },
  "confidence": 0.9,
  "response": "I'll create a box with default dimensions of 10x10x10"
}

User: "add a cylinder"
{
  "intent": "create_primitive",
  "parameters": {
    "shape": "cylinder",
    "radius": 5.0,
    "height": 10.0
  },
  "confidence": 0.95,
  "response": "Creating a cylinder with radius 5 and height 10"
}

User: "Are we connected?"
{
  "intent": "query",
  "parameters": {
    "target": "connection_status"
  },
  "confidence": 1.0,
  "response": "Yes, I'm connected and ready to help you with 3D modeling!"
}

User: "Hello"
{
  "intent": "query",
  "parameters": {
    "target": "greeting"
  },
  "confidence": 1.0,
  "response": "Hello! I'm Roshera CAD Assistant. I can help you create 3D models using natural language. Try asking me to create shapes like boxes, spheres, or cylinders!"
}

User: "tell me more about this product"
{
  "intent": "query",
  "parameters": {
    "target": "product_info"
  },
  "confidence": 1.0,
  "response": "Roshera is an AI-native CAD system that combines professional-grade B-Rep geometry modeling with natural language interfaces. Built with Rust for performance, it supports real-time collaboration and runs entirely in your browser via WebAssembly. You can create 3D models by simply describing what you want - like 'create a box' or 'make a sphere with radius 5'."
}

User: "what can you do"
{
  "intent": "query",
  "parameters": {
    "target": "capabilities"
  },
  "confidence": 1.0,
  "response": "I can help you with 3D CAD modeling using natural language! I can create primitive shapes (boxes, spheres, cylinders, cones, torus), perform boolean operations (union, intersection, difference), transform objects, and answer questions about your designs. Just tell me what you want to create and I'll make it happen!"
}

"#.to_string();

        // Add comprehensive context if available
        if let Some(ctx) = context {
            // System context
            if let Some(ref sys) = ctx.system_context {
                prompt.push_str("\nSYSTEM CONTEXT:\n");

                // Users
                prompt.push_str(&format!(
                    "- Current user: {} ({})\n",
                    sys.session.current_user.name, sys.session.current_user.role
                ));
                prompt.push_str(&format!(
                    "- Connected users: {}\n",
                    sys.session
                        .connected_users
                        .iter()
                        .map(|u| format!(
                            "{} ({})",
                            u.name,
                            match u.status {
                                shared_types::UserStatus::Active => "active",
                                shared_types::UserStatus::Idle => "idle",
                                shared_types::UserStatus::Away => "away",
                                shared_types::UserStatus::Disconnected => "disconnected",
                            }
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));

                // Workflow
                prompt.push_str(&format!(
                    "- Workflow stage: {:?}\n",
                    sys.workflow.current_stage
                ));
                if let Some(ref tool) = sys.workflow.active_tool {
                    prompt.push_str(&format!("- Active tool: {}\n", tool));
                }

                // Environment
                prompt.push_str(&format!("- Units: {}\n", sys.environment.unit_system));
                prompt.push_str(&format!("- Theme: {}\n", sys.environment.theme));

                // AI Status
                prompt.push_str(&format!("- AI connected: {}\n", sys.status.ai_connected));
                prompt.push_str(&format!("- Model: {}\n", sys.status.ai_model));

                // Available commands summary
                prompt.push_str("\nAVAILABLE COMMANDS:\n");
                for cmd in &sys.commands.creation_commands {
                    prompt.push_str(&format!(
                        "- {}: {} (examples: {})\n",
                        cmd.name,
                        cmd.description,
                        cmd.examples.join(", ")
                    ));
                }
            }

            // Scene context
            if let Some(ref scene) = ctx.scene_state {
                prompt.push_str("\nCURRENT SCENE STATE:\n");
                prompt.push_str(&format!("- Number of objects: {}\n", scene.objects.len()));
                if !scene.objects.is_empty() {
                    prompt.push_str(&format!(
                        "- Objects: {}\n",
                        scene
                            .objects
                            .iter()
                            .map(|obj| format!(
                                "{} ({})",
                                obj.name,
                                match &obj.object_type {
                                    shared_types::ObjectType::Box { .. } => "box",
                                    shared_types::ObjectType::Sphere { .. } => "sphere",
                                    shared_types::ObjectType::Cylinder { .. } => "cylinder",
                                    shared_types::ObjectType::Cone { .. } => "cone",
                                    shared_types::ObjectType::Torus { .. } => "torus",
                                    _ => "unknown",
                                }
                            ))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                prompt.push_str(&format!(
                    "- Selected objects: {}\n",
                    scene.selection.selected_objects.len()
                ));
            }

            // Add recent command history
            if !ctx.previous_commands.is_empty() {
                prompt.push_str("\nRECENT COMMANDS:\n");
                for cmd in ctx.previous_commands.iter().rev().take(3) {
                    prompt.push_str(&format!("- {}\n", cmd.original_text));
                }
            }
        }

        prompt.push_str("\nRemember: Respond ONLY with valid JSON in the format shown above.");
        prompt
    }
}

#[async_trait]
impl LLMProvider for OllamaProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            name: format!("ollama-{}", self.model),
            version: "1.0.0".to_string(),
            supported_languages: vec!["en".to_string()],
            max_context_length: if self.model.contains("mistral") {
                8192
            } else {
                2048
            },
            supports_streaming: true,
            supports_batching: false,
            device_type: "cpu/gpu".to_string(),
            model_size_mb: if self.model.contains("mistral") {
                7370
            } else {
                637
            }, // Mistral 7B vs TinyLLaMA
            quantization: crate::providers::QuantizationType::Int4,
        }
    }

    async fn process(
        &self,
        input: &str,
        context: Option<&ConversationContext>,
    ) -> Result<ParsedCommand, ProviderError> {
        // Fast local pattern matching for common commands to avoid LLM calls
        if let Some(fast_result) = self.try_fast_parse(input) {
            tracing::info!("Fast-parsed command: {}", input);
            return Ok(fast_result);
        }

        let system_prompt = self.create_system_prompt(context);

        // Create request
        let request = OllamaRequest {
            model: self.model.clone(),
            prompt: input.to_string(),
            system: system_prompt,
            temperature: 0.1,
            format: Some("json".to_string()),
            stream: false,
        };

        // Send request to Ollama
        tracing::info!(
            "Sending request to Ollama: {} with prompt: {}",
            self.model,
            input
        );
        let client = reqwest::Client::new();
        let response = client
            .post(&format!("{}/api/generate", self.base_url))
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                ProviderError::ProcessingError(format!("Failed to connect to Ollama: {}", e))
            })?;

        if !response.status().is_success() {
            return Err(ProviderError::ProcessingError(format!(
                "Ollama returned error: {}",
                response.status()
            )));
        }

        let ollama_response: OllamaResponse = response.json().await.map_err(|e| {
            ProviderError::ProcessingError(format!("Failed to parse Ollama response: {}", e))
        })?;

        tracing::info!("Ollama raw response: {}", ollama_response.response);

        // Parse the JSON response with fallback handling
        let cleaned_response = ollama_response.response.trim();

        // Try to extract JSON if it's wrapped in markdown or other text
        let json_str = if let Some(start) = cleaned_response.find('{') {
            if let Some(end) = cleaned_response.rfind('}') {
                &cleaned_response[start..=end]
            } else {
                cleaned_response
            }
        } else {
            cleaned_response
        };

        let parsed_json: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
            tracing::error!(
                "Failed to parse JSON: {}. Raw response: {}",
                e,
                ollama_response.response
            );
            ProviderError::ProcessingError(format!(
                "Invalid JSON from model: {}. Response: {}",
                e,
                ollama_response
                    .response
                    .chars()
                    .take(200)
                    .collect::<String>()
            ))
        })?;

        // Extract fields from response
        let intent_str = parsed_json["intent"].as_str().unwrap_or("unknown");
        let mut parameters: std::collections::HashMap<String, serde_json::Value> = parsed_json
            ["parameters"]
            .as_object()
            .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        let confidence = parsed_json["confidence"].as_f64().unwrap_or(0.5) as f32;
        let response_text = parsed_json["response"]
            .as_str()
            .unwrap_or("Processing your request");

        // Add the response text to parameters so it can be accessed by the processor
        parameters.insert(
            "response".to_string(),
            serde_json::Value::String(response_text.to_string()),
        );

        // Convert intent string to CommandIntent enum
        let intent = match intent_str {
            "create_primitive" => {
                if let Some(shape) = parameters.get("shape").and_then(|s| s.as_str()) {
                    CommandIntent::CreatePrimitive {
                        shape: shape.to_string(),
                    }
                } else {
                    CommandIntent::Unknown
                }
            }
            "boolean_operation" => {
                if let Some(op) = parameters.get("operation").and_then(|s| s.as_str()) {
                    CommandIntent::BooleanOperation {
                        operation: op.to_string(),
                    }
                } else {
                    CommandIntent::Unknown
                }
            }
            "transform" => {
                if let Some(op) = parameters.get("operation").and_then(|s| s.as_str()) {
                    CommandIntent::Transform {
                        operation: op.to_string(),
                    }
                } else {
                    CommandIntent::Unknown
                }
            }
            "query" => {
                if let Some(target) = parameters.get("target").and_then(|s| s.as_str()) {
                    // Handle scene-aware queries
                    let mut enriched_params = parameters.clone();

                    match target {
                        "count_objects" => {
                            if let Some(ctx) = context {
                                if let Some(ref scene) = ctx.scene_state {
                                    enriched_params
                                        .insert("count".to_string(), json!(scene.objects.len()));
                                    enriched_params.insert(
                                        "response".to_string(),
                                        json!(format!(
                                            "There are {} objects in the scene",
                                            scene.objects.len()
                                        )),
                                    );
                                }
                            }
                        }
                        "list_objects" => {
                            if let Some(ctx) = context {
                                if let Some(ref scene) = ctx.scene_state {
                                    let objects: Vec<String> =
                                        scene.objects.iter().map(|obj| obj.name.clone()).collect();
                                    enriched_params.insert("objects".to_string(), json!(objects));
                                    enriched_params.insert(
                                        "response".to_string(),
                                        json!(format!("Objects in scene: {}", objects.join(", "))),
                                    );
                                }
                            }
                        }
                        "find_largest" => {
                            if let Some(ctx) = context {
                                if let Some(ref scene) = ctx.scene_state {
                                    if let Some(largest) = scene.objects.iter().max_by(|a, b| {
                                        let vol_a = a.bounding_box.volume();
                                        let vol_b = b.bounding_box.volume();
                                        vol_a
                                            .partial_cmp(&vol_b)
                                            .unwrap_or(std::cmp::Ordering::Equal)
                                    }) {
                                        enriched_params
                                            .insert("object_id".to_string(), json!(largest.id));
                                        enriched_params
                                            .insert("object_name".to_string(), json!(largest.name));
                                        enriched_params.insert(
                                            "response".to_string(),
                                            json!(format!(
                                                "The largest object is '{}'",
                                                largest.name
                                            )),
                                        );
                                    }
                                }
                            }
                        }
                        "product_info" => {
                            enriched_params.insert("response".to_string(),
                                json!("Roshera is an AI-native CAD system that combines professional-grade B-Rep geometry modeling with natural language interfaces. Built with Rust for performance, it supports real-time collaboration and runs entirely in your browser via WebAssembly. You can create 3D models by simply describing what you want - like 'create a box' or 'make a sphere with radius 5'."));
                        }
                        "capabilities" => {
                            enriched_params.insert("response".to_string(),
                                json!("I can help you with 3D CAD modeling using natural language! I can create primitive shapes (boxes, spheres, cylinders, cones, torus), perform boolean operations (union, intersection, difference), transform objects, and answer questions about your designs. Just tell me what you want to create and I'll make it happen!"));
                        }
                        "greeting" => {
                            enriched_params.insert("response".to_string(),
                                json!("Hello! I'm Roshera CAD Assistant. I can help you create 3D models using natural language. Try asking me to create shapes like boxes, spheres, or cylinders!"));
                        }
                        "connection_status" => {
                            enriched_params.insert(
                                "response".to_string(),
                                json!("Yes, I'm connected and ready to help you with 3D modeling!"),
                            );
                        }
                        _ => {}
                    }

                    CommandIntent::Query {
                        target: target.to_string(),
                    }
                } else {
                    CommandIntent::Unknown
                }
            }
            _ => CommandIntent::Unknown,
        };

        Ok(ParsedCommand {
            original_text: input.to_string(),
            intent,
            parameters,
            confidence,
            language: "en".to_string(),
        })
    }

    async fn generate(&self, prompt: &str, _max_tokens: usize) -> Result<String, ProviderError> {
        // For general text generation (not command parsing)
        let request = OllamaRequest {
            model: self.model.clone(),
            prompt: prompt.to_string(),
            system: "You are a helpful CAD assistant.".to_string(),
            temperature: 0.7,
            format: None,
            stream: false,
        };

        let client = reqwest::Client::new();
        let response = client
            .post(&format!("{}/api/generate", self.base_url))
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                ProviderError::ProcessingError(format!("Failed to connect to Ollama: {}", e))
            })?;

        let ollama_response: OllamaResponse = response.json().await.map_err(|e| {
            ProviderError::ProcessingError(format!("Failed to parse response: {}", e))
        })?;

        Ok(ollama_response.response)
    }

    async fn generate_response(
        &self,
        command_result: &str,
        _language: &str,
    ) -> Result<String, ProviderError> {
        self.generate(
            &format!("Respond to this CAD operation result: {}", command_result),
            256,
        )
        .await
    }

    fn memory_requirement_mb(&self) -> usize {
        637 // TinyLLaMA 1.1B size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_prompt_generation() {
        let provider = OllamaProvider::new("tinyllama:latest".to_string());
        let prompt = provider.create_system_prompt(None);
        assert!(prompt.contains("Roshera CAD Assistant"));
        assert!(prompt.contains("create_primitive"));
    }
}
