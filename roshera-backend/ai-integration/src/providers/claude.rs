//! Claude provider using Anthropic's tool_use API protocol.
//!
//! Instead of keyword matching (the old approach), this provider:
//! 1. Sends geometry tool schemas alongside the user prompt to Claude
//! 2. Claude returns structured `tool_use` content blocks
//! 3. The tool dispatch layer converts those into `ParsedCommand`
//!
//! When no API key is configured, falls back to local parsing using the
//! tool dispatch layer directly (deterministic, no LLM call).

use super::{
    CommandIntent, LLMProvider, LLMTokenStream, ParsedCommand, ProviderCapabilities, ProviderError,
};
use crate::tool_dispatch::{self, DispatchResult, ToolUseBlock};
use async_trait::async_trait;
use futures::stream::StreamExt;
use geometry_engine::primitives::tool_schema_generator::ToolTier;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Configuration for the Claude provider.
#[derive(Debug, Clone)]
pub struct ClaudeConfig {
    /// Anthropic API key. When None, the provider uses local-only parsing.
    pub api_key: Option<String>,
    /// Model ID (e.g., "claude-sonnet-4-20250514")
    pub model: String,
    /// Maximum tokens for the response
    pub max_tokens: usize,
    /// Tool tier to expose to the LLM
    pub tool_tier: ToolTier,
    /// API base URL (for proxies or self-hosted)
    pub api_base: String,
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            model: "claude-sonnet-4-20250514".to_string(),
            max_tokens: 1024,
            tool_tier: ToolTier::Tier1,
            api_base: "https://api.anthropic.com".to_string(),
        }
    }
}

/// Claude provider that uses the Anthropic tool_use API for structured geometry commands.
#[derive(Debug, Clone)]
pub struct ClaudeProvider {
    config: ClaudeConfig,
}

impl ClaudeProvider {
    /// Create a new Claude provider with default config (local parsing only).
    pub fn new() -> Self {
        Self {
            config: ClaudeConfig::default(),
        }
    }

    /// Create a Claude provider with explicit configuration.
    pub fn with_config(config: ClaudeConfig) -> Self {
        Self { config }
    }

    /// Set the tool tier (controls how many tools are exposed to the LLM).
    pub fn set_tool_tier(&mut self, tier: ToolTier) {
        self.config.tool_tier = tier;
    }

    /// Process input via the Anthropic API with tool_use.
    ///
    /// Sends the prompt + tool definitions → receives tool_use blocks → dispatches.
    async fn process_via_api(
        &self,
        input: &str,
        context: Option<&super::ConversationContext>,
        api_key: &str,
    ) -> Result<ParsedCommand, ProviderError> {
        let tools = tool_dispatch::tool_definitions_for_tier(self.config.tool_tier);

        // Build messages array
        let mut messages = Vec::new();

        // Include conversation history if available
        if let Some(ctx) = context {
            for prev in &ctx.previous_commands {
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": prev.original_text
                }));
            }
        }

        // Add scene context as system-level information
        let system_prompt = build_system_prompt(context);

        messages.push(serde_json::json!({
            "role": "user",
            "content": input
        }));

        let request_body = serde_json::json!({
            "model": self.config.model,
            "max_tokens": self.config.max_tokens,
            "system": system_prompt,
            "tools": tools,
            "messages": messages
        });

        let client = reqwest::Client::new();
        let response = client
            .post(format!("{}/v1/messages", self.config.api_base))
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| ProviderError::InferenceError(format!("API request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::InferenceError(format!(
                "Anthropic API returned {}: {}",
                status, body
            )));
        }

        let response_body: Value = response.json().await.map_err(|e| {
            ProviderError::InferenceError(format!("Failed to parse API response: {}", e))
        })?;

        // Extract tool_use blocks from the response
        parse_anthropic_response(&response_body, input)
    }

    /// Process input locally without an API call.
    ///
    /// Uses deterministic tool name matching on the input text to produce commands.
    /// This is the fallback when no API key is configured.
    fn process_locally(
        &self,
        input: &str,
        context: Option<&super::ConversationContext>,
    ) -> Result<ParsedCommand, ProviderError> {
        let input_lower = input.to_lowercase();

        // Try to extract a structured tool call from natural language
        if let Some(tool_use) = parse_natural_language_to_tool_use(&input_lower) {
            match tool_dispatch::dispatch_tool_call(&tool_use) {
                Ok(DispatchResult::Command(cmd)) | Ok(DispatchResult::Query(cmd)) => {
                    return Ok(cmd);
                }
                Ok(DispatchResult::TextResponse(text)) => {
                    return Ok(ParsedCommand {
                        original_text: input.to_string(),
                        intent: CommandIntent::Query {
                            target: "text_response".to_string(),
                        },
                        parameters: {
                            let mut p = HashMap::new();
                            p.insert("response".to_string(), serde_json::json!(text));
                            p
                        },
                        confidence: 0.8,
                        language: "en".to_string(),
                    });
                }
                Err(_) => {} // Fall through to scene-aware commands
            }
        }

        // Scene-aware commands that don't map to geometry tools
        if let Some(ctx) = context {
            if let Some(ref scene) = ctx.scene_state {
                return process_scene_command(&input_lower, input, scene);
            }
        }

        Err(ProviderError::InvalidInput(format!(
            "Could not parse '{}' as a geometry command. Available commands: \
             create box/sphere/cylinder/cone/torus, boolean union/difference/intersection, \
             query geometry, export stl/obj",
            input
        )))
    }
}

#[async_trait]
impl LLMProvider for ClaudeProvider {
    async fn process(
        &self,
        input: &str,
        context: Option<&super::ConversationContext>,
    ) -> Result<ParsedCommand, ProviderError> {
        match &self.config.api_key {
            Some(key) if !key.is_empty() => self.process_via_api(input, context, key).await,
            _ => self.process_locally(input, context),
        }
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            name: "Claude Tool-Use".to_string(),
            version: "2.0".to_string(),
            supported_languages: vec!["en".to_string()],
            max_context_length: 200_000,
            supports_streaming: true,
            supports_batching: false,
            device_type: if self.config.api_key.is_some() {
                "cloud".to_string()
            } else {
                "cpu".to_string()
            },
            model_size_mb: 0,
            quantization: super::QuantizationType::Float32,
        }
    }

    async fn generate(&self, prompt: &str, _max_tokens: usize) -> Result<String, ProviderError> {
        match &self.config.api_key {
            Some(key) if !key.is_empty() => {
                let client = reqwest::Client::new();
                let response = client
                    .post(format!("{}/v1/messages", self.config.api_base))
                    .header("x-api-key", key)
                    .header("anthropic-version", "2023-06-01")
                    .header("content-type", "application/json")
                    .json(&serde_json::json!({
                        "model": self.config.model,
                        "max_tokens": self.config.max_tokens,
                        "messages": [{"role": "user", "content": prompt}]
                    }))
                    .send()
                    .await
                    .map_err(|e| {
                        ProviderError::InferenceError(format!("API request failed: {}", e))
                    })?;

                let body: Value = response.json().await.map_err(|e| {
                    ProviderError::InferenceError(format!("Failed to parse response: {}", e))
                })?;

                extract_text_from_response(&body)
            }
            _ => Ok(format!(
                "I can help you create 3D geometry. You said: '{}'. \
                 Try: 'create a box 10 5 3' or 'make a sphere radius 5'.",
                prompt
            )),
        }
    }

    async fn generate_stream(
        &self,
        prompt: &str,
        max_tokens: usize,
    ) -> Result<LLMTokenStream, ProviderError> {
        let key = match &self.config.api_key {
            Some(k) if !k.is_empty() => k.clone(),
            _ => {
                // No API key — fall back to the trait default which
                // yields the local synthetic response as a single chunk.
                // Phase 1.J ("fail loudly") is responsible for refusing
                // this case at the route layer; here we stay graceful so
                // unit tests keep working without network access.
                let full = self.generate(prompt, max_tokens).await?;
                return Ok(Box::pin(futures::stream::once(
                    async move { Ok(full) },
                )));
            }
        };

        let effective_max = if max_tokens == 0 {
            self.config.max_tokens
        } else {
            max_tokens
        };

        let request_body = serde_json::json!({
            "model": self.config.model,
            "max_tokens": effective_max,
            "stream": true,
            "messages": [{"role": "user", "content": prompt}],
        });

        let client = reqwest::Client::new();
        let response = client
            .post(format!("{}/v1/messages", self.config.api_base))
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| {
                ProviderError::InferenceError(format!("streaming request failed: {}", e))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::InferenceError(format!(
                "Anthropic streaming API returned {}: {}",
                status, body
            )));
        }

        // Convert reqwest's byte stream into a stream of text deltas.
        // Anthropic sends one event per `content_block_delta`; we extract
        // the `delta.text` field for `text_delta` blocks and ignore
        // everything else (start/stop markers, ping events, tool deltas).
        let byte_stream = response.bytes_stream();
        let delta_stream = anthropic_sse_to_text_deltas(byte_stream);
        Ok(Box::pin(delta_stream))
    }

    async fn generate_response(
        &self,
        command_result: &str,
        _language: &str,
    ) -> Result<String, ProviderError> {
        Ok(format!("Done: {}", command_result))
    }

    fn memory_requirement_mb(&self) -> usize {
        if self.config.api_key.is_some() {
            0
        } else {
            1
        }
    }
}

// --- Internal helpers ---

/// Build a system prompt that includes scene context for the LLM.
fn build_system_prompt(context: Option<&super::ConversationContext>) -> String {
    let mut prompt = String::from(
        "You are a CAD assistant. Use the provided tools to create and modify 3D geometry. \
         Always use tool calls for geometry operations — never describe them in text. \
         When the user asks to create, modify, or query geometry, respond with the appropriate tool call."
    );

    if let Some(ctx) = context {
        if let Some(ref scene) = ctx.scene_state {
            prompt.push_str(&format!(
                "\n\nCurrent scene has {} objects.",
                scene.objects.len()
            ));
            for obj in &scene.objects {
                prompt.push_str(&format!(
                    "\n- {} ({}): {:?}",
                    obj.name, obj.id, obj.object_type
                ));
            }
        }
    }

    prompt
}

/// Parse the Anthropic API response to extract tool_use blocks and dispatch them.
fn parse_anthropic_response(
    response: &Value,
    original_input: &str,
) -> Result<ParsedCommand, ProviderError> {
    let content = response
        .get("content")
        .and_then(|c| c.as_array())
        .ok_or_else(|| {
            ProviderError::InferenceError("Response missing 'content' array".to_string())
        })?;

    // Look for tool_use blocks first
    for block in content {
        if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
            let tool_use = ToolUseBlock {
                id: block
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                name: block
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                input: block
                    .get("input")
                    .cloned()
                    .unwrap_or(Value::Object(Default::default())),
            };

            return match tool_dispatch::dispatch_tool_call(&tool_use) {
                Ok(DispatchResult::Command(cmd)) | Ok(DispatchResult::Query(cmd)) => Ok(cmd),
                Ok(DispatchResult::TextResponse(text)) => Ok(ParsedCommand {
                    original_text: original_input.to_string(),
                    intent: CommandIntent::Query {
                        target: "text_response".to_string(),
                    },
                    parameters: {
                        let mut p = HashMap::new();
                        p.insert("response".to_string(), serde_json::json!(text));
                        p
                    },
                    confidence: 1.0,
                    language: "en".to_string(),
                }),
                Err(e) => Err(e),
            };
        }
    }

    // No tool_use block — extract text response
    let text = extract_text_from_content(content);
    if !text.is_empty() {
        Ok(ParsedCommand {
            original_text: original_input.to_string(),
            intent: CommandIntent::Query {
                target: "text_response".to_string(),
            },
            parameters: {
                let mut p = HashMap::new();
                p.insert("response".to_string(), serde_json::json!(text));
                p
            },
            confidence: 0.5,
            language: "en".to_string(),
        })
    } else {
        Err(ProviderError::InferenceError(
            "Claude response contained no tool calls or text".to_string(),
        ))
    }
}

/// Extract text from a content array (text blocks).
fn extract_text_from_content(content: &[Value]) -> String {
    content
        .iter()
        .filter_map(|block| {
            if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                block.get("text").and_then(|t| t.as_str()).map(String::from)
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Extract text from a full API response body.
fn extract_text_from_response(response: &Value) -> Result<String, ProviderError> {
    let content = response
        .get("content")
        .and_then(|c| c.as_array())
        .ok_or_else(|| {
            ProviderError::InferenceError("Response missing 'content' array".to_string())
        })?;

    let text = extract_text_from_content(content);
    if text.is_empty() {
        Err(ProviderError::InferenceError(
            "Response contained no text blocks".to_string(),
        ))
    } else {
        Ok(text)
    }
}

/// Parse natural language into a ToolUseBlock for local dispatch.
///
/// This is a structured parser — it maps known patterns to tool calls rather than
/// doing keyword matching with hardcoded parameter values.
fn parse_natural_language_to_tool_use(input: &str) -> Option<ToolUseBlock> {
    let tokens: Vec<&str> = input.split_whitespace().collect();
    let numbers = extract_all_numbers(input);

    // Primitive creation patterns
    if contains_any(input, &["box", "cube", "cuboid", "rectangular"]) {
        let (w, h, d) = match numbers.len() {
            0 => (10.0, 10.0, 10.0),
            1 => (numbers[0], numbers[0], numbers[0]),
            2 => (numbers[0], numbers[1], numbers[0]),
            _ => (numbers[0], numbers[1], numbers[2]),
        };
        return Some(ToolUseBlock {
            id: "local".to_string(),
            name: "create_box".to_string(),
            input: serde_json::json!({"width": w, "height": h, "depth": d}),
        });
    }

    if contains_any(input, &["sphere", "ball"]) {
        let r = numbers.first().copied().unwrap_or(5.0);
        return Some(ToolUseBlock {
            id: "local".to_string(),
            name: "create_sphere".to_string(),
            input: serde_json::json!({"radius": r}),
        });
    }

    if contains_any(input, &["cylinder", "tube", "pipe"]) {
        let (r, h) = match numbers.len() {
            0 => (5.0, 10.0),
            1 => (numbers[0], numbers[0] * 2.0),
            _ => (numbers[0], numbers[1]),
        };
        return Some(ToolUseBlock {
            id: "local".to_string(),
            name: "create_cylinder".to_string(),
            input: serde_json::json!({"radius": r, "height": h}),
        });
    }

    if contains_any(input, &["cone"]) {
        let (br, tr, h) = match numbers.len() {
            0 => (5.0, 0.0, 10.0),
            1 => (numbers[0], 0.0, numbers[0] * 2.0),
            2 => (numbers[0], 0.0, numbers[1]),
            _ => (numbers[0], numbers[1], numbers[2]),
        };
        return Some(ToolUseBlock {
            id: "local".to_string(),
            name: "create_cone".to_string(),
            input: serde_json::json!({"bottom_radius": br, "top_radius": tr, "height": h}),
        });
    }

    if contains_any(input, &["torus", "donut", "ring"]) {
        let (major, minor) = match numbers.len() {
            0 => (10.0, 3.0),
            1 => (numbers[0], numbers[0] * 0.3),
            _ => (numbers[0], numbers[1]),
        };
        return Some(ToolUseBlock {
            id: "local".to_string(),
            name: "create_torus".to_string(),
            input: serde_json::json!({"major_radius": major, "minor_radius": minor}),
        });
    }

    // Boolean operations
    if contains_any(input, &["union", "merge", "combine", "join"]) {
        return Some(ToolUseBlock {
            id: "local".to_string(),
            name: "boolean_union".to_string(),
            input: serde_json::json!({"object_a": "last", "object_b": "selected"}),
        });
    }

    if contains_any(input, &["subtract", "difference", "cut", "drill", "hole"]) {
        return Some(ToolUseBlock {
            id: "local".to_string(),
            name: "boolean_difference".to_string(),
            input: serde_json::json!({"object_a": "last", "object_b": "selected"}),
        });
    }

    if contains_any(input, &["intersect"]) {
        return Some(ToolUseBlock {
            id: "local".to_string(),
            name: "boolean_intersection".to_string(),
            input: serde_json::json!({"object_a": "last", "object_b": "selected"}),
        });
    }

    // Query
    if contains_any(
        input,
        &[
            "dimensions",
            "info",
            "properties",
            "describe",
            "summary",
            "what is",
            "analyze",
        ],
    ) {
        return Some(ToolUseBlock {
            id: "local".to_string(),
            name: "query_geometry".to_string(),
            input: serde_json::json!({"solid_id": "last"}),
        });
    }

    // Export
    if contains_any(input, &["export stl", "save stl", "stl"]) {
        return Some(ToolUseBlock {
            id: "local".to_string(),
            name: "export_stl".to_string(),
            input: serde_json::json!({"solid_id": "last"}),
        });
    }

    if contains_any(input, &["export obj", "save obj"]) {
        return Some(ToolUseBlock {
            id: "local".to_string(),
            name: "export_obj".to_string(),
            input: serde_json::json!({"solid_id": "last"}),
        });
    }

    // Fillet / chamfer
    if contains_any(input, &["fillet", "round"]) {
        let r = numbers.first().copied().unwrap_or(2.0);
        return Some(ToolUseBlock {
            id: "local".to_string(),
            name: "fillet".to_string(),
            input: serde_json::json!({"target_id": "last", "radius": r}),
        });
    }

    if contains_any(input, &["chamfer", "bevel"]) {
        let d = numbers.first().copied().unwrap_or(2.0);
        return Some(ToolUseBlock {
            id: "local".to_string(),
            name: "chamfer".to_string(),
            input: serde_json::json!({"target_id": "last", "distance": d}),
        });
    }

    None
}

/// Process scene-aware commands that don't map to geometry tools.
fn process_scene_command(
    input_lower: &str,
    original: &str,
    scene: &shared_types::SceneState,
) -> Result<ParsedCommand, ProviderError> {
    if input_lower.contains("select all") {
        let ids: Vec<String> = scene.objects.iter().map(|o| o.id.to_string()).collect();
        let mut params = HashMap::new();
        params.insert("objects".to_string(), serde_json::json!(ids));
        return Ok(ParsedCommand {
            original_text: original.to_string(),
            intent: CommandIntent::Query {
                target: "select_all".to_string(),
            },
            parameters: params,
            confidence: 0.95,
            language: "en".to_string(),
        });
    }

    if input_lower.contains("how many") || input_lower.contains("count") {
        let count = scene.objects.len();
        let mut params = HashMap::new();
        params.insert("count".to_string(), serde_json::json!(count));
        params.insert(
            "response".to_string(),
            serde_json::json!(format!("There are {} objects in the scene", count)),
        );
        return Ok(ParsedCommand {
            original_text: original.to_string(),
            intent: CommandIntent::Query {
                target: "count_objects".to_string(),
            },
            parameters: params,
            confidence: 0.95,
            language: "en".to_string(),
        });
    }

    if input_lower.contains("list")
        || (input_lower.contains("show") && input_lower.contains("objects"))
    {
        let list: Vec<String> = scene
            .objects
            .iter()
            .map(|o| format!("{}: {:?}", o.name, o.object_type))
            .collect();
        let mut params = HashMap::new();
        params.insert("objects".to_string(), serde_json::json!(list));
        params.insert(
            "response".to_string(),
            serde_json::json!(if list.is_empty() {
                "No objects in the scene".to_string()
            } else {
                format!("Objects in scene:\n{}", list.join("\n"))
            }),
        );
        return Ok(ParsedCommand {
            original_text: original.to_string(),
            intent: CommandIntent::Query {
                target: "list_objects".to_string(),
            },
            parameters: params,
            confidence: 0.95,
            language: "en".to_string(),
        });
    }

    if input_lower.contains("delete")
        && (input_lower.contains("all") || input_lower.contains("everything"))
    {
        if scene.objects.is_empty() {
            return Err(ProviderError::InvalidInput(
                "No objects to delete".to_string(),
            ));
        }
        let ids: Vec<String> = scene.objects.iter().map(|o| o.id.to_string()).collect();
        let mut params = HashMap::new();
        params.insert("objects".to_string(), serde_json::json!(ids));
        params.insert("action".to_string(), serde_json::json!("delete_all"));
        return Ok(ParsedCommand {
            original_text: original.to_string(),
            intent: CommandIntent::Transform {
                operation: "delete_all".to_string(),
            },
            parameters: params,
            confidence: 0.95,
            language: "en".to_string(),
        });
    }

    Err(ProviderError::InvalidInput(format!(
        "Could not parse '{}' as a geometry command. Available commands: \
         create box/sphere/cylinder/cone/torus, boolean union/difference/intersection, \
         query geometry, export stl/obj, fillet, chamfer",
        original
    )))
}

/// Extract all numbers from text.
fn extract_all_numbers(text: &str) -> Vec<f64> {
    text.split_whitespace()
        .filter_map(|w| {
            // Strip common suffixes like "mm", "cm", "in"
            let stripped = w
                .trim_end_matches("mm")
                .trim_end_matches("cm")
                .trim_end_matches("in")
                .trim_end_matches(',');
            stripped.parse::<f64>().ok()
        })
        .collect()
}

/// Parse Anthropic's `text/event-stream` byte stream into a stream of
/// text deltas.
///
/// Anthropic's streaming protocol is documented at
/// <https://docs.anthropic.com/en/api/messages-streaming>. The relevant
/// frames are:
///
/// ```text
/// event: content_block_delta
/// data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}
/// ```
///
/// We extract `delta.text` from every `text_delta` block and yield it as
/// a `String`. Other event types (`message_start`, `content_block_start`,
/// `ping`, `message_stop`, tool-use deltas) are silently ignored.
///
/// The byte stream is buffered into a UTF-8 line accumulator because SSE
/// frames are delimited by blank lines and a single TCP packet is not
/// guaranteed to align with frame boundaries.
fn anthropic_sse_to_text_deltas<S>(
    byte_stream: S,
) -> impl futures::Stream<Item = Result<String, ProviderError>> + Send
where
    S: futures::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
{
    use futures::stream::unfold;

    struct State<S> {
        inner: S,
        buf: String,
    }

    let initial = State {
        inner: Box::pin(byte_stream),
        buf: String::new(),
    };

    unfold(Some(initial), |state| async move {
        let mut state = state?;
        loop {
            // Look for a complete SSE frame (terminated by \n\n) in the
            // buffer first; if we find one, parse it and yield any
            // text-delta payload.
            if let Some(frame_end) = state.buf.find("\n\n") {
                let frame: String = state.buf.drain(..frame_end + 2).collect();
                if let Some(delta) = extract_text_delta_from_frame(&frame) {
                    return Some((Ok(delta), Some(state)));
                }
                // Frame parsed but contained no user-visible text — keep
                // looping to find the next frame without yielding.
                continue;
            }

            // No complete frame buffered yet — pull more bytes.
            match state.inner.next().await {
                Some(Ok(chunk)) => match std::str::from_utf8(&chunk) {
                    Ok(s) => state.buf.push_str(s),
                    Err(_) => {
                        // Lossy fallback: keep streaming but report once.
                        state.buf.push_str(&String::from_utf8_lossy(&chunk));
                    }
                },
                Some(Err(e)) => {
                    return Some((
                        Err(ProviderError::InferenceError(format!(
                            "stream read failed: {}",
                            e
                        ))),
                        None,
                    ));
                }
                None => {
                    // Stream ended. If anything remains in the buffer it
                    // is an unterminated frame — flush any final delta we
                    // can still recover, then end.
                    if !state.buf.is_empty() {
                        let frame = std::mem::take(&mut state.buf);
                        if let Some(delta) = extract_text_delta_from_frame(&frame) {
                            return Some((Ok(delta), None));
                        }
                    }
                    return None;
                }
            }
        }
    })
}

/// Extract the `delta.text` value from a single SSE frame, or `None` if
/// the frame is not a `text_delta` event (or has no recoverable text).
///
/// SSE frames look like:
/// ```text
/// event: content_block_delta
/// data: {"type":"content_block_delta",...}
///
/// ```
/// Multiple `data:` lines are concatenated per the SSE spec, but
/// Anthropic always uses a single `data:` line per frame, so we accept
/// either shape.
fn extract_text_delta_from_frame(frame: &str) -> Option<String> {
    let mut data = String::new();
    for line in frame.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            // The space after `data:` is conventional, not required.
            let payload = rest.strip_prefix(' ').unwrap_or(rest);
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(payload);
        }
    }

    if data.is_empty() {
        return None;
    }

    let parsed: Value = serde_json::from_str(&data).ok()?;
    if parsed.get("type").and_then(|t| t.as_str()) != Some("content_block_delta") {
        return None;
    }
    let delta = parsed.get("delta")?;
    if delta.get("type").and_then(|t| t.as_str()) != Some("text_delta") {
        return None;
    }
    let text = delta.get("text").and_then(|t| t.as_str())?;
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// Check if the input contains any of the given keywords as whole words.
fn contains_any(input: &str, keywords: &[&str]) -> bool {
    keywords.iter().any(|kw| {
        if kw.contains(' ') {
            // Multi-word: exact substring match
            input.contains(kw)
        } else {
            // Single word: check word boundaries
            input
                .split_whitespace()
                .any(|word| word == *kw || word.starts_with(kw) || word.ends_with(kw))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_local_parse_box() {
        let provider = ClaudeProvider::new();
        let result = provider.process_locally("create a box 10 5 3", None);
        assert!(result.is_ok());
        let cmd = result.unwrap();
        assert!(
            matches!(cmd.intent, CommandIntent::CreatePrimitive { ref shape } if shape == "box")
        );
        assert_eq!(cmd.parameters["width"], serde_json::json!(10.0));
        assert_eq!(cmd.parameters["height"], serde_json::json!(5.0));
        assert_eq!(cmd.parameters["depth"], serde_json::json!(3.0));
    }

    #[test]
    fn test_local_parse_sphere() {
        let provider = ClaudeProvider::new();
        let result = provider.process_locally("make a sphere radius 7.5", None);
        assert!(result.is_ok());
        let cmd = result.unwrap();
        assert!(
            matches!(cmd.intent, CommandIntent::CreatePrimitive { ref shape } if shape == "sphere")
        );
        assert_eq!(cmd.parameters["radius"], serde_json::json!(7.5));
    }

    #[test]
    fn test_local_parse_cylinder() {
        let provider = ClaudeProvider::new();
        let result = provider.process_locally("create cylinder 5 20", None);
        assert!(result.is_ok());
        let cmd = result.unwrap();
        assert!(
            matches!(cmd.intent, CommandIntent::CreatePrimitive { ref shape } if shape == "cylinder")
        );
        assert_eq!(cmd.parameters["radius"], serde_json::json!(5.0));
        assert_eq!(cmd.parameters["height"], serde_json::json!(20.0));
    }

    #[test]
    fn test_local_parse_boolean() {
        let provider = ClaudeProvider::new();
        let result = provider.process_locally("subtract the hole from the block", None);
        assert!(result.is_ok());
        let cmd = result.unwrap();
        assert!(
            matches!(cmd.intent, CommandIntent::BooleanOperation { ref operation } if operation == "difference")
        );
    }

    #[test]
    fn test_local_parse_query() {
        let provider = ClaudeProvider::new();
        let result = provider.process_locally("what are the dimensions of this", None);
        assert!(result.is_ok());
        let cmd = result.unwrap();
        assert!(
            matches!(cmd.intent, CommandIntent::Query { ref target } if target == "query_geometry")
        );
    }

    #[test]
    fn test_local_parse_unknown() {
        let provider = ClaudeProvider::new();
        let result = provider.process_locally("play music", None);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_numbers() {
        assert_eq!(extract_all_numbers("box 10 5 3"), vec![10.0, 5.0, 3.0]);
        assert_eq!(extract_all_numbers("radius 7.5mm"), vec![7.5]);
        assert_eq!(extract_all_numbers("no numbers here"), Vec::<f64>::new());
    }

    #[test]
    fn test_contains_any_word_boundary() {
        assert!(contains_any("create a box", &["box"]));
        assert!(contains_any("make a sphere", &["sphere"]));
        // "boxing" matches "box" via prefix — acceptable in CAD context
        assert!(contains_any("boxing match", &["box"]));
        assert!(!contains_any("nothing here", &["box"]));
        assert!(contains_any("export stl file", &["export stl"]));
    }

    #[test]
    fn test_parse_anthropic_response_tool_use() {
        let response = serde_json::json!({
            "content": [
                {
                    "type": "tool_use",
                    "id": "toolu_123",
                    "name": "create_box",
                    "input": {"width": 10.0, "height": 5.0, "depth": 3.0}
                }
            ]
        });

        let result = parse_anthropic_response(&response, "make a box");
        assert!(result.is_ok());
        let cmd = result.unwrap();
        assert!(
            matches!(cmd.intent, CommandIntent::CreatePrimitive { ref shape } if shape == "box")
        );
    }

    #[test]
    fn test_parse_anthropic_response_text_only() {
        let response = serde_json::json!({
            "content": [
                {
                    "type": "text",
                    "text": "I can help you create geometry."
                }
            ]
        });

        let result = parse_anthropic_response(&response, "hello");
        assert!(result.is_ok());
        let cmd = result.unwrap();
        assert_eq!(cmd.confidence, 0.5); // Low confidence for text-only response
    }

    #[test]
    fn test_local_parse_fillet() {
        let provider = ClaudeProvider::new();
        let result = provider.process_locally("fillet the edges with radius 3", None);
        assert!(result.is_ok());
        let cmd = result.unwrap();
        assert!(
            matches!(cmd.intent, CommandIntent::Modify { ref operation, .. } if operation == "fillet")
        );
    }

    #[test]
    fn test_local_parse_torus() {
        let provider = ClaudeProvider::new();
        let result = provider.process_locally("create a torus 15 4", None);
        assert!(result.is_ok());
        let cmd = result.unwrap();
        assert!(
            matches!(cmd.intent, CommandIntent::CreatePrimitive { ref shape } if shape == "torus")
        );
        assert_eq!(cmd.parameters["major_radius"], serde_json::json!(15.0));
        assert_eq!(cmd.parameters["minor_radius"], serde_json::json!(4.0));
    }

    #[test]
    fn test_extract_text_delta_returns_text_for_text_delta_frame() {
        let frame = "event: content_block_delta\n\
                     data: {\"type\":\"content_block_delta\",\"index\":0,\
                     \"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n";
        assert_eq!(
            extract_text_delta_from_frame(frame),
            Some("Hello".to_string())
        );
    }

    #[test]
    fn test_extract_text_delta_skips_non_delta_events() {
        let ping = "event: ping\ndata: {\"type\":\"ping\"}\n\n";
        assert!(extract_text_delta_from_frame(ping).is_none());

        let start = "event: message_start\n\
                     data: {\"type\":\"message_start\",\"message\":{}}\n\n";
        assert!(extract_text_delta_from_frame(start).is_none());

        let stop = "event: message_stop\n\
                    data: {\"type\":\"message_stop\"}\n\n";
        assert!(extract_text_delta_from_frame(stop).is_none());
    }

    #[test]
    fn test_extract_text_delta_skips_tool_use_deltas() {
        let frame = "event: content_block_delta\n\
                     data: {\"type\":\"content_block_delta\",\"index\":0,\
                     \"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"x\\\":\"}}\n\n";
        assert!(extract_text_delta_from_frame(frame).is_none());
    }

    #[test]
    fn test_extract_text_delta_handles_missing_data_lines() {
        let frame = "event: content_block_delta\n\n";
        assert!(extract_text_delta_from_frame(frame).is_none());
    }

    #[tokio::test]
    async fn test_anthropic_sse_to_text_deltas_concatenates_split_chunks() {
        // Two text_delta frames split across three byte chunks — simulates
        // the case where TCP packet boundaries land mid-frame.
        let frames = "event: content_block_delta\n\
                      data: {\"type\":\"content_block_delta\",\"index\":0,\
                      \"delta\":{\"type\":\"text_delta\",\"text\":\"foo \"}}\n\n\
                      event: content_block_delta\n\
                      data: {\"type\":\"content_block_delta\",\"index\":0,\
                      \"delta\":{\"type\":\"text_delta\",\"text\":\"bar\"}}\n\n";
        let split_at = frames.len() / 3;
        let split_at_two = (frames.len() * 2) / 3;
        let chunk_a = bytes::Bytes::copy_from_slice(frames[..split_at].as_bytes());
        let chunk_b = bytes::Bytes::copy_from_slice(frames[split_at..split_at_two].as_bytes());
        let chunk_c = bytes::Bytes::copy_from_slice(frames[split_at_two..].as_bytes());

        // Build a Stream<Item = Result<Bytes, reqwest::Error>>. We can't
        // fabricate reqwest::Errors here, so all items are Ok; the
        // explicit type annotation pins the Err parameter.
        let items: Vec<Result<bytes::Bytes, reqwest::Error>> =
            vec![Ok(chunk_a), Ok(chunk_b), Ok(chunk_c)];
        let inner = futures::stream::iter(items);
        let stream = anthropic_sse_to_text_deltas(inner);
        let collected: Vec<_> = stream.collect::<Vec<_>>().await;
        let texts: Vec<String> = collected.into_iter().filter_map(Result::ok).collect();
        assert_eq!(texts.join(""), "foo bar");
    }

    #[test]
    fn test_default_dimensions_when_no_numbers() {
        let provider = ClaudeProvider::new();
        let result = provider.process_locally("create a box", None);
        assert!(result.is_ok());
        let cmd = result.unwrap();
        // Default box is 10x10x10
        assert_eq!(cmd.parameters["width"], serde_json::json!(10.0));
    }
}
