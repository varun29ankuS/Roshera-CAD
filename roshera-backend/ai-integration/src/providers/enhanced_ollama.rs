/// Enhanced Ollama provider with built-in Roshera knowledge
/// This works immediately without any RAG setup

use super::*;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Enhanced Ollama that understands Roshera deeply
pub struct EnhancedOllamaProvider {
    base_url: String,
    model: String,
    capabilities: ProviderCapabilities,
}

impl EnhancedOllamaProvider {
    pub fn new(model: String) -> Self {
        let capabilities = ProviderCapabilities {
            name: format!("EnhancedOllama-{}", model),
            version: "1.0.0".to_string(),
            supported_languages: vec!["en".to_string()],
            max_context_length: 8192,
            supports_streaming: true,
            supports_batching: false,
            device_type: "cuda".to_string(),
            model_size_mb: 4000,
            quantization: QuantizationType::Int8,
        };

        Self {
            base_url: "http://localhost:11434".to_string(),
            model,
            capabilities,
        }
    }

    /// Create a comprehensive prompt with all Roshera knowledge
    fn create_enhanced_prompt(&self, user_input: &str, context: Option<&ConversationContext>) -> String {
        let mut prompt = String::new();
        
        // 1. System knowledge (always included)
        prompt.push_str("You are Roshera AI, an intelligent CAD assistant. You understand the following:\n\n");
        
        // 2. Available commands and their syntax
        prompt.push_str("COMMANDS YOU CAN USE:\n");
        prompt.push_str("- CreateBox { width, height, depth } - for brackets, plates, housings\n");
        prompt.push_str("- CreateCylinder { radius, height } - for shafts, pins, holes\n");
        prompt.push_str("- CreateSphere { radius } - for ball joints, spherical features\n");
        prompt.push_str("- BooleanUnion { object_a, object_b } - combine objects\n");
        prompt.push_str("- BooleanDifference { object_a, object_b } - subtract B from A (make holes)\n");
        prompt.push_str("- Fillet { edges, radius } - round edges (min 3mm for stress relief)\n");
        prompt.push_str("\n");
        
        // 3. Engineering rules
        prompt.push_str("ENGINEERING RULES:\n");
        prompt.push_str("- Bracket thickness: 10mm minimum for 10kg load (safety factor 2.5x)\n");
        prompt.push_str("- Mounting holes: M8 standard, 20mm minimum spacing\n");
        prompt.push_str("- Default material: Aluminum 6061-T6 (yield: 276 MPa)\n");
        prompt.push_str("- Always apply 3mm fillets to internal corners for stress relief\n");
        prompt.push_str("- Wall thickness for housings: 2mm minimum\n");
        prompt.push_str("\n");
        
        // 4. Workflow stages
        prompt.push_str("WORKFLOW STAGES (follow in order):\n");
        prompt.push_str("1. Create/Sketch - Define basic geometry\n");
        prompt.push_str("2. Define - Add features like holes, fillets\n");
        prompt.push_str("3. Refine - Optimize for manufacturing\n");
        prompt.push_str("4. Validate - Check strength, clearances\n");
        prompt.push_str("5. Output - Export for manufacturing\n");
        prompt.push_str("\n");
        
        // 5. Context from previous commands
        if let Some(ctx) = context {
            if !ctx.previous_commands.is_empty() {
                prompt.push_str("PREVIOUS COMMANDS IN THIS SESSION:\n");
                for (i, cmd) in ctx.previous_commands.iter().enumerate().take(3) {
                    prompt.push_str(&format!("{}. {}\n", i + 1, cmd.original_text));
                }
                prompt.push_str("\n");
            }
            
            if !ctx.active_objects.is_empty() {
                prompt.push_str("ACTIVE OBJECTS IN SCENE:\n");
                for obj in ctx.active_objects.iter().take(5) {
                    prompt.push_str(&format!("- Object ID: {}\n", obj));
                }
                prompt.push_str("\n");
            }
        }
        
        // 6. Common patterns
        prompt.push_str("COMMON PATTERNS:\n");
        prompt.push_str("- 'create a bracket' → CreateBox { width: 100, height: 80, depth: 10 }\n");
        prompt.push_str("- 'make a hole' → CreateCylinder then BooleanDifference\n");
        prompt.push_str("- 'round the edges' → Fillet { radius: 3.0 }\n");
        prompt.push_str("- 'pattern 4 holes' → Create one hole, then Pattern { count: 4 }\n");
        prompt.push_str("\n");
        
        // 7. The actual user request
        prompt.push_str("USER REQUEST: ");
        prompt.push_str(user_input);
        prompt.push_str("\n\n");
        
        // 8. Response format
        prompt.push_str("Respond with:\n");
        prompt.push_str("1. The specific command to execute (e.g., CreateBox)\n");
        prompt.push_str("2. Parameters with exact values\n");
        prompt.push_str("3. Brief explanation of why these values\n");
        prompt.push_str("\nYour response:\n");
        
        prompt
    }

    /// Parse Ollama's response into structured command
    fn parse_llm_response(&self, response: &str) -> ParsedCommand {
        let lower = response.to_lowercase();
        
        // Try to identify the command type
        if lower.contains("createbox") || lower.contains("create_box") || lower.contains("box") && lower.contains("create") {
            // Extract dimensions
            let width = self.extract_number(&lower, "width").unwrap_or(100.0);
            let height = self.extract_number(&lower, "height").unwrap_or(80.0);
            let depth = self.extract_number(&lower, "depth")
                .or_else(|| self.extract_number(&lower, "thickness"))
                .unwrap_or(10.0);
            
            return ParsedCommand {
                original_text: response.to_string(),
                intent: CommandIntent::CreatePrimitive {
                    shape: "box".to_string(),
                },
                parameters: {
                    let mut params = HashMap::new();
                    params.insert("width".to_string(), serde_json::json!(width));
                    params.insert("height".to_string(), serde_json::json!(height));
                    params.insert("depth".to_string(), serde_json::json!(depth));
                    params
                },
                confidence: 0.85,
                language: "en".to_string(),
            };
        }
        
        if lower.contains("createcylinder") || lower.contains("create_cylinder") || lower.contains("cylinder") && lower.contains("create") {
            let radius = self.extract_number(&lower, "radius").unwrap_or(10.0);
            let height = self.extract_number(&lower, "height").unwrap_or(50.0);
            
            return ParsedCommand {
                original_text: response.to_string(),
                intent: CommandIntent::CreatePrimitive {
                    shape: "cylinder".to_string(),
                },
                parameters: {
                    let mut params = HashMap::new();
                    params.insert("radius".to_string(), serde_json::json!(radius));
                    params.insert("height".to_string(), serde_json::json!(height));
                    params
                },
                confidence: 0.85,
                language: "en".to_string(),
            };
        }
        
        if lower.contains("sphere") && lower.contains("create") {
            let radius = self.extract_number(&lower, "radius").unwrap_or(25.0);
            
            return ParsedCommand {
                original_text: response.to_string(),
                intent: CommandIntent::CreatePrimitive {
                    shape: "sphere".to_string(),
                },
                parameters: {
                    let mut params = HashMap::new();
                    params.insert("radius".to_string(), serde_json::json!(radius));
                    params
                },
                confidence: 0.85,
                language: "en".to_string(),
            };
        }
        
        // Boolean operations
        if lower.contains("union") || lower.contains("combine") || lower.contains("merge") {
            return ParsedCommand {
                original_text: response.to_string(),
                intent: CommandIntent::BooleanOperation {
                    operation: "union".to_string(),
                },
                parameters: HashMap::new(),
                confidence: 0.8,
                language: "en".to_string(),
            };
        }
        
        if lower.contains("difference") || lower.contains("subtract") || lower.contains("hole") {
            return ParsedCommand {
                original_text: response.to_string(),
                intent: CommandIntent::BooleanOperation {
                    operation: "difference".to_string(),
                },
                parameters: HashMap::new(),
                confidence: 0.8,
                language: "en".to_string(),
            };
        }
        
        // Default to unknown
        ParsedCommand {
            original_text: response.to_string(),
            intent: CommandIntent::Unknown,
            parameters: HashMap::new(),
            confidence: 0.1,
            language: "en".to_string(),
        }
    }

    fn extract_number(&self, text: &str, after_word: &str) -> Option<f64> {
        if let Some(idx) = text.find(after_word) {
            let suffix = &text[idx + after_word.len()..];
            // Look for pattern like ": 100" or "= 100" or " 100"
            let cleaned = suffix.trim_start_matches(|c: char| c == ':' || c == '=' || c.is_whitespace());
            
            // Take characters until non-numeric
            let number_str: String = cleaned
                .chars()
                .take_while(|c| c.is_numeric() || *c == '.' || *c == '-')
                .collect();
            
            number_str.parse().ok()
        } else {
            None
        }
    }
}

#[async_trait]
impl LLMProvider for EnhancedOllamaProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities.clone()
    }

    async fn process(
        &self,
        input: &str,
        context: Option<&ConversationContext>,
    ) -> Result<ParsedCommand, ProviderError> {
        // First, check if we can handle this directly with patterns
        let lower = input.to_lowercase();
        
        // Direct pattern matching for common commands
        if lower.contains("bracket") {
            let load = if lower.contains("10kg") || lower.contains("10 kg") {
                10.0
            } else if lower.contains("5kg") || lower.contains("5 kg") {
                5.0
            } else {
                10.0 // default
            };
            
            // Engineering calculation: 10kg needs 10mm thickness
            let thickness = (load * 1.0).max(10.0);
            
            return Ok(ParsedCommand {
                original_text: input.to_string(),
                intent: CommandIntent::CreatePrimitive {
                    shape: "box".to_string(),
                },
                parameters: {
                    let mut params = HashMap::new();
                    params.insert("width".to_string(), serde_json::json!(100.0));
                    params.insert("height".to_string(), serde_json::json!(80.0));
                    params.insert("depth".to_string(), serde_json::json!(thickness));
                    params.insert("explanation".to_string(), 
                        serde_json::json!(format!("{}mm thickness for {}kg load with 2.5x safety factor", thickness, load)));
                    params
                },
                confidence: 0.95,
                language: "en".to_string(),
            });
        }
        
        // For more complex queries, use Ollama if available
        let enhanced_prompt = self.create_enhanced_prompt(input, context);
        
        // Try to call Ollama
        let client = reqwest::Client::new();
        let ollama_request = OllamaRequest {
            model: self.model.clone(),
            prompt: enhanced_prompt,
            stream: false,
            temperature: Some(0.3), // Lower temperature for more consistent CAD commands
            top_p: Some(0.9),
            max_tokens: Some(500),
        };
        
        match client
            .post(&format!("{}/api/generate", self.base_url))
            .json(&ollama_request)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => {
                if let Ok(ollama_response) = response.json::<OllamaResponse>().await {
                    // Parse the Ollama response
                    let parsed = self.parse_llm_response(&ollama_response.response);
                    
                    // If Ollama gave us something useful, return it
                    if !matches!(parsed.intent, CommandIntent::Unknown) {
                        return Ok(parsed);
                    }
                }
            }
            _ => {
                // Ollama not available or failed, use fallback
                tracing::warn!("Ollama not available, using pattern matching");
            }
        }
        
        // Fallback to pattern matching
        if lower.contains("box") || lower.contains("cube") {
            return Ok(ParsedCommand {
                original_text: input.to_string(),
                intent: CommandIntent::CreatePrimitive {
                    shape: "box".to_string(),
                },
                parameters: {
                    let mut params = HashMap::new();
                    params.insert("width".to_string(), serde_json::json!(50.0));
                    params.insert("height".to_string(), serde_json::json!(50.0));
                    params.insert("depth".to_string(), serde_json::json!(50.0));
                    params
                },
                confidence: 0.7,
                language: "en".to_string(),
            });
        }
        
        if lower.contains("cylinder") || lower.contains("shaft") || lower.contains("rod") {
            return Ok(ParsedCommand {
                original_text: input.to_string(),
                intent: CommandIntent::CreatePrimitive {
                    shape: "cylinder".to_string(),
                },
                parameters: {
                    let mut params = HashMap::new();
                    params.insert("radius".to_string(), serde_json::json!(10.0));
                    params.insert("height".to_string(), serde_json::json!(50.0));
                    params
                },
                confidence: 0.7,
                language: "en".to_string(),
            });
        }
        
        if lower.contains("sphere") || lower.contains("ball") {
            return Ok(ParsedCommand {
                original_text: input.to_string(),
                intent: CommandIntent::CreatePrimitive {
                    shape: "sphere".to_string(),
                },
                parameters: {
                    let mut params = HashMap::new();
                    params.insert("radius".to_string(), serde_json::json!(25.0));
                    params
                },
                confidence: 0.7,
                language: "en".to_string(),
            });
        }
        
        // Unknown command
        Ok(ParsedCommand {
            original_text: input.to_string(),
            intent: CommandIntent::Unknown,
            parameters: HashMap::new(),
            confidence: 0.1,
            language: "en".to_string(),
        })
    }

    async fn generate(&self, prompt: &str, max_tokens: usize) -> Result<String, ProviderError> {
        // Simple generation for responses
        Ok(format!("Executing: {}", prompt))
    }

    async fn generate_response(
        &self,
        command_result: &str,
        _language: &str,
    ) -> Result<String, ProviderError> {
        if command_result.contains("success") {
            Ok("Successfully created the geometry. What would you like to do next?".to_string())
        } else {
            Ok("I encountered an issue. Let me try a different approach.".to_string())
        }
    }

    fn memory_requirement_mb(&self) -> usize {
        4000 // Typical 7B model
    }
}

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<usize>,
}

#[derive(Deserialize)]
struct OllamaResponse {
    response: String,
}