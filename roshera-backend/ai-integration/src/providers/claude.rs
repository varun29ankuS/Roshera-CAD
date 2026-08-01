//! Claude provider using Anthropic's tool_use API protocol.
//!
//! This provider:
//! 1. Sends geometry tool schemas alongside the user prompt to Claude
//! 2. Claude returns structured `tool_use` content blocks
//! 3. The tool dispatch layer converts those into `ParsedCommand`
//!
//! When no API key is configured every entry point returns
//! `ProviderError::ProviderUnavailable` — there is no local-keyword
//! fallback in production builds. Mock traffic must come from
//! `MockLLMProvider`, gated behind the `mock-providers` feature.

use super::{
    CommandIntent, LLMProvider, LLMTokenStream, ParsedCommand, ProviderCapabilities, ProviderError,
};
use crate::tool_dispatch::{self, DispatchResult, ToolUseBlock};
use async_trait::async_trait;
use futures::stream::StreamExt;
use geometry_engine::primitives::tool_schema_generator::ToolTier;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

/// A credential the Claude provider authenticates with.
///
/// Two transport-level auth schemes exist on the Anthropic API and they
/// are NOT interchangeable headers:
///
/// - **API key** → `x-api-key: <key>`
/// - **OAuth access token** (from an `ant auth login` profile or
///   `ANTHROPIC_AUTH_TOKEN`) → `Authorization: Bearer <token>` **plus**
///   `anthropic-beta: oauth-2025-04-20`. The beta header requirement is
///   endpoint-dependent, so it is always sent — a request that happens
///   to work without it on one endpoint breaks on another.
///
/// `Debug` is implemented manually so secret bytes never reach log
/// streams; only the credential *kind* is rendered.
#[derive(Clone, PartialEq, Eq)]
pub enum ClaudeCredential {
    /// Static Anthropic API key (`sk-ant-…`).
    ApiKey(String),
    /// Short-lived OAuth access token (Claude account sign-in).
    OauthAccessToken(String),
}

impl ClaudeCredential {
    /// True when the underlying secret is the empty string — treated
    /// everywhere as "not configured", never sent over the wire.
    pub fn is_empty(&self) -> bool {
        match self {
            ClaudeCredential::ApiKey(s) | ClaudeCredential::OauthAccessToken(s) => s.is_empty(),
        }
    }

    /// Stable name of the credential kind, for status reporting.
    pub fn kind(&self) -> &'static str {
        match self {
            ClaudeCredential::ApiKey(_) => "api_key",
            ClaudeCredential::OauthAccessToken(_) => "oauth_access_token",
        }
    }

    /// Attach this credential's auth headers to a request.
    pub fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self {
            ClaudeCredential::ApiKey(key) => req.header("x-api-key", key),
            ClaudeCredential::OauthAccessToken(token) => req
                .header("authorization", format!("Bearer {token}"))
                .header("anthropic-beta", "oauth-2025-04-20"),
        }
    }
}

impl std::fmt::Debug for ClaudeCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Secret bytes are withheld; the kind is triage-relevant.
        write!(
            f,
            "{}(<redacted>)",
            match self {
                ClaudeCredential::ApiKey(_) => "ApiKey",
                ClaudeCredential::OauthAccessToken(_) => "OauthAccessToken",
            }
        )
    }
}

/// Configuration for the Claude provider.
///
/// `Debug` is implemented manually so that the credential is never
/// leaked through log streams, error reports, or the
/// `{:?}` formatter used by debug-assertion failures. We render the
/// `credential` field as `Some(<kind>(<redacted>))` or `None` —
/// preserving presence information (often needed when triaging "is the
/// provider configured at all?") while withholding the secret material.
#[derive(Clone)]
pub struct ClaudeConfig {
    /// Credential (API key or OAuth token). When `None` (or empty)
    /// every method returns `ProviderError::ProviderUnavailable` —
    /// there is no offline fallback.
    pub credential: Option<ClaudeCredential>,
    /// Model ID (defaults to [`shared_types::DEFAULT_CLAUDE_MODEL`])
    pub model: String,
    /// Maximum tokens for the response
    pub max_tokens: usize,
    /// Tool tier to expose to the LLM
    pub tool_tier: ToolTier,
    /// API base URL (for proxies or self-hosted)
    pub api_base: String,
    /// Request timeout, in seconds, applied to every HTTP call this
    /// provider makes (P5: an unbounded client can hang a request
    /// forever on a stalled connection).
    pub request_timeout_secs: u64,
}

impl std::fmt::Debug for ClaudeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Redact the credential. Presence (and kind) is preserved so
        // operators can still tell whether the provider is configured;
        // the secret itself never reaches the formatter output
        // (ClaudeCredential's own Debug impl redacts).
        f.debug_struct("ClaudeConfig")
            .field("credential", &self.credential)
            .field("model", &self.model)
            .field("max_tokens", &self.max_tokens)
            .field("tool_tier", &self.tool_tier)
            .field("api_base", &self.api_base)
            .field("request_timeout_secs", &self.request_timeout_secs)
            .finish()
    }
}

impl Default for ClaudeConfig {
    fn default() -> Self {
        Self {
            credential: None,
            model: shared_types::DEFAULT_CLAUDE_MODEL.to_string(),
            max_tokens: 1024,
            tool_tier: ToolTier::Tier1,
            api_base: "https://api.anthropic.com".to_string(),
            request_timeout_secs: 120,
        }
    }
}

/// Claude provider that uses the Anthropic tool_use API for structured geometry commands.
#[derive(Debug, Clone)]
pub struct ClaudeProvider {
    config: ClaudeConfig,
    /// Built once (per `ClaudeProvider` construction) with
    /// `config.request_timeout_secs` applied, so every HTTP call this
    /// provider makes shares one bounded-timeout client rather than
    /// constructing a fresh unbounded one per call.
    client: reqwest::Client,
}

impl ClaudeProvider {
    /// Create a new Claude provider with default config.
    ///
    /// The default config has no credential set; every method will
    /// return `ProviderError::ProviderUnavailable` until a credential
    /// is configured via `with_config(...)` or the `ANTHROPIC_API_KEY`
    /// env var is honored by a wrapper that builds the config.
    pub fn new() -> Self {
        Self::with_config(ClaudeConfig::default())
    }

    /// Create a Claude provider with explicit configuration.
    pub fn with_config(config: ClaudeConfig) -> Self {
        let client = build_http_client(&config);
        Self { config, client }
    }

    /// Set the tool tier (controls how many tools are exposed to the LLM).
    pub fn set_tool_tier(&mut self, tier: ToolTier) {
        self.config.tool_tier = tier;
    }

    /// Read-only access to the provider's configuration — lets callers
    /// (and tests) confirm which model/tool-tier/timeout a constructed
    /// provider actually carries, e.g. after `NativeProviderFactory::
    /// create_claude_provider` builds one from a `NativeProviderConfig`.
    pub fn config(&self) -> &ClaudeConfig {
        &self.config
    }

    /// The configured credential, or a typed refusal when absent/empty.
    /// Every network entry point funnels through this — there is no
    /// keyword-parser or mock fallback in production.
    fn require_credential(&self) -> Result<&ClaudeCredential, ProviderError> {
        match &self.config.credential {
            Some(cred) if !cred.is_empty() => Ok(cred),
            _ => Err(ProviderError::ProviderUnavailable(
                "Claude provider has no credential configured (Anthropic API \
                 key or OAuth access token); refusing to fabricate a response"
                    .to_string(),
            )),
        }
    }

    /// Prove the configured credential is accepted by the live Anthropic
    /// API — a real round-trip, zero tokens spent.
    ///
    /// `GET /v1/models?limit=1` requires valid authentication but bills
    /// nothing, which makes it the honest "test this key before saving
    /// it" probe: a bad key fails HERE, at configuration time, instead
    /// of mid-conversation.
    ///
    /// # Errors
    /// - `ProviderUnavailable` — no credential configured, or the API
    ///   rejected it (401/403). The message carries the upstream status.
    /// - `InferenceError` — transport failure or unexpected status.
    pub async fn validate_credential(&self) -> Result<(), ProviderError> {
        let cred = self.require_credential()?;
        let response = cred
            .apply(
                self.client
                    .get(format!("{}/v1/models?limit=1", self.config.api_base)),
            )
            .header("anthropic-version", "2023-06-01")
            .send()
            .await
            .map_err(|e| {
                ProviderError::InferenceError(format!("credential validation request failed: {e}"))
            })?;

        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = response.text().await.unwrap_or_default();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            Err(ProviderError::ProviderUnavailable(format!(
                "Anthropic rejected the {} ({}): {}",
                cred.kind(),
                status,
                body
            )))
        } else {
            Err(ProviderError::InferenceError(format!(
                "credential validation returned unexpected status {}: {}",
                status, body
            )))
        }
    }

    /// Prove a requested model ID is one the configured credential can
    /// actually serve, via Anthropic's Models API (`GET /v1/models/{id}`)
    /// — the authoritative source: an API key and a Max/Pro OAuth token do
    /// not necessarily serve the same catalog, so this asks the live
    /// provider rather than checking a hardcoded list. Zero tokens spent.
    ///
    /// `"default"` is never passed here — callers treat it as "the
    /// provider's own choice" and skip this round-trip entirely (see
    /// `handlers/ai_provider.rs::resolve_requested_model`).
    ///
    /// # Errors
    /// - `ProviderUnavailable` — no credential configured, or the model ID
    ///   was rejected (404, or 401/403 on the credential itself). The
    ///   message names the model and the status so the refusal is
    ///   specific, not a generic failure.
    /// - `InferenceError` — transport failure or an unexpected status this
    ///   provider does not have a specific interpretation for.
    pub async fn validate_model(&self, model: &str) -> Result<(), ProviderError> {
        let cred = self.require_credential()?;
        let response = cred
            .apply(
                self.client
                    .get(format!("{}/v1/models/{}", self.config.api_base, model)),
            )
            .header("anthropic-version", "2023-06-01")
            .send()
            .await
            .map_err(|e| {
                ProviderError::InferenceError(format!("model validation request failed: {e}"))
            })?;

        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = response.text().await.unwrap_or_default();
        if status.as_u16() == 404 {
            Err(ProviderError::ProviderUnavailable(format!(
                "Anthropic does not recognize model '{model}' ({status}) for this \
                 credential: {body}"
            )))
        } else if status.as_u16() == 401 || status.as_u16() == 403 {
            Err(ProviderError::ProviderUnavailable(format!(
                "Anthropic rejected the {} while validating model '{}' ({}): {}",
                cred.kind(),
                model,
                status,
                body
            )))
        } else {
            Err(ProviderError::InferenceError(format!(
                "model validation for '{}' returned unexpected status {}: {}",
                model, status, body
            )))
        }
    }

    /// Send a text prompt alongside a PNG image to Claude and return the plain-text reply.
    ///
    /// Uses the Anthropic multimodal messages API: the PNG is base64-encoded and
    /// placed as an `image` content block ahead of the `text` block, matching the
    /// layout Claude's vision tier expects.  All transport details (endpoint,
    /// model, `max_tokens`, auth header) come from the provider's existing
    /// `ClaudeConfig` — no extra configuration is needed.
    ///
    /// # Errors
    ///
    /// Returns `ProviderError::ProviderUnavailable` when no `ANTHROPIC_API_KEY`
    /// is configured — identical to the policy enforced by every other entry
    /// point on this provider.  All network / decode failures surface as
    /// `ProviderError::InferenceError`.
    pub async fn generate_with_image(
        &self,
        prompt: &str,
        png_bytes: &[u8],
    ) -> Result<String, ProviderError> {
        let cred = self.require_credential()?;

        let request_body = build_image_message_body(
            &self.config.model,
            self.config.max_tokens,
            prompt,
            png_bytes,
        );

        let response = cred
            .apply(
                self.client
                    .post(format!("{}/v1/messages", self.config.api_base)),
            )
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

        let body: Value = response.json().await.map_err(|e| {
            ProviderError::InferenceError(format!("Failed to parse response: {}", e))
        })?;

        extract_text_from_response(&body)
    }

    /// Process input via the Anthropic API with tool_use.
    ///
    /// Sends the prompt + tool definitions → receives tool_use blocks → dispatches.
    async fn process_via_api(
        &self,
        input: &str,
        context: Option<&super::ConversationContext>,
        credential: &ClaudeCredential,
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

        let response = credential
            .apply(
                self.client
                    .post(format!("{}/v1/messages", self.config.api_base)),
            )
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
}

#[async_trait]
impl LLMProvider for ClaudeProvider {
    async fn process(
        &self,
        input: &str,
        context: Option<&super::ConversationContext>,
    ) -> Result<ParsedCommand, ProviderError> {
        let cred = self.require_credential()?.clone();
        self.process_via_api(input, context, &cred).await
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            name: "Claude Tool-Use".to_string(),
            version: "2.0".to_string(),
            supported_languages: vec!["en".to_string()],
            max_context_length: 200_000,
            supports_streaming: true,
            supports_batching: false,
            device_type: "cloud".to_string(),
            model_size_mb: 0,
            quantization: super::QuantizationType::Float32,
        }
    }

    async fn generate(&self, prompt: &str, _max_tokens: usize) -> Result<String, ProviderError> {
        let cred = self.require_credential()?;

        let response = cred
            .apply(
                self.client
                    .post(format!("{}/v1/messages", self.config.api_base)),
            )
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&serde_json::json!({
                "model": self.config.model,
                "max_tokens": self.config.max_tokens,
                "messages": [{"role": "user", "content": prompt}]
            }))
            .send()
            .await
            .map_err(|e| ProviderError::InferenceError(format!("API request failed: {}", e)))?;

        // P4: mirror the status check every sibling entry point performs
        // (process_via_api, generate_with_image, generate_stream). Without
        // it, a 401/429/529 JSON error envelope parses "successfully" as a
        // `Value` and `extract_text_from_response` surfaces a misleading
        // "no text blocks" error instead of the real HTTP failure.
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::InferenceError(format!(
                "Anthropic API returned {}: {}",
                status, body
            )));
        }

        let body: Value = response.json().await.map_err(|e| {
            ProviderError::InferenceError(format!("Failed to parse response: {}", e))
        })?;

        extract_text_from_response(&body)
    }

    async fn generate_stream(
        &self,
        prompt: &str,
        max_tokens: usize,
    ) -> Result<LLMTokenStream, ProviderError> {
        let cred = self.require_credential()?.clone();

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

        let response = cred
            .apply(
                self.client
                    .post(format!("{}/v1/messages", self.config.api_base)),
            )
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
        // Cloud-only provider; no in-process model.
        0
    }
}

// --- Internal helpers ---

/// Build the shared HTTP client for a `ClaudeProvider`, applying
/// `config.request_timeout_secs` (P5: an unbounded `reqwest::Client` can
/// hang a request against a stalled connection indefinitely). Mirrors the
/// `Client::builder().timeout(...).build()` pattern used by
/// `UniversalEndpoint::new` elsewhere in this crate. Falls back to an
/// unconfigured default client on the (practically unreachable) builder
/// error path, rather than propagating a fallible result out of
/// constructors that are not themselves fallible.
fn build_http_client(config: &ClaudeConfig) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(config.request_timeout_secs))
        .build()
        .unwrap_or_default()
}

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

/// Build the JSON request body for a multimodal (image + text) Anthropic API call.
///
/// Produces the exact shape the Anthropic `/v1/messages` endpoint expects:
///
/// ```json
/// {
///   "model": "…",
///   "max_tokens": N,
///   "messages": [{
///     "role": "user",
///     "content": [
///       { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "…" } },
///       { "type": "text", "text": "…" }
///     ]
///   }]
/// }
/// ```
///
/// Separated from the network path so tests can assert the body shape without
/// a live API key or network connection.
fn build_image_message_body(
    model: &str,
    max_tokens: usize,
    prompt: &str,
    png_bytes: &[u8],
) -> Value {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let b64 = STANDARD.encode(png_bytes);
    serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": [{
            "role": "user",
            "content": [
                {
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": b64
                    }
                },
                {
                    "type": "text",
                    "text": prompt
                }
            ]
        }]
    })
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

#[cfg(test)]
mod tests {
    use super::*;

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

    /// AUDIT-M1 contract: the secret material must not appear in the
    /// `Debug` output of `ClaudeConfig` — for either credential kind.
    /// A regression would expose every API key / OAuth token on the
    /// first `tracing::error!(?config, …)` line the operator types.
    #[test]
    fn debug_redacts_credential_when_present() {
        for cred in [
            ClaudeCredential::ApiKey("sk-ant-real-secret-do-not-leak".to_string()),
            ClaudeCredential::OauthAccessToken("oauth-real-secret-do-not-leak".to_string()),
        ] {
            let cfg = ClaudeConfig {
                credential: Some(cred),
                ..ClaudeConfig::default()
            };
            let rendered = format!("{:?}", cfg);
            assert!(
                !rendered.contains("real-secret-do-not-leak"),
                "Debug output must not contain the raw secret; got: {rendered}"
            );
            assert!(
                rendered.contains("<redacted>"),
                "Debug output must mark the credential as redacted; got: {rendered}"
            );
        }
    }

    /// AUDIT-M1: when no credential is configured, `Debug` must
    /// preserve `None` so operators can still see "provider not
    /// configured" in triage. (Presence is not secret; the bytes are.)
    #[test]
    fn debug_preserves_none_when_credential_absent() {
        let cfg = ClaudeConfig::default();
        let rendered = format!("{:?}", cfg);
        assert!(
            rendered.contains("credential: None"),
            "Debug output must surface absence as None; got: {rendered}"
        );
        assert!(
            !rendered.contains("<redacted>"),
            "Absent credential must not be labelled redacted; got: {rendered}"
        );
    }

    /// The two credential kinds must emit their scheme-correct headers:
    /// an OAuth token on `x-api-key` (or a key on `Authorization`)
    /// would 401 at the vendor with a misleading message.
    #[tokio::test]
    async fn credential_apply_sets_scheme_correct_headers() {
        let client = reqwest::Client::new();

        let req = ClaudeCredential::ApiKey("k123".to_string())
            .apply(client.get("http://localhost/never-sent"))
            .build()
            .expect("request builds");
        assert_eq!(
            req.headers().get("x-api-key").and_then(|v| v.to_str().ok()),
            Some("k123")
        );
        assert!(req.headers().get("authorization").is_none());

        let req = ClaudeCredential::OauthAccessToken("t456".to_string())
            .apply(client.get("http://localhost/never-sent"))
            .build()
            .expect("request builds");
        assert_eq!(
            req.headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok()),
            Some("Bearer t456")
        );
        assert_eq!(
            req.headers()
                .get("anthropic-beta")
                .and_then(|v| v.to_str().ok()),
            Some("oauth-2025-04-20"),
            "OAuth over raw HTTP requires the oauth beta header"
        );
        assert!(req.headers().get("x-api-key").is_none());
    }

    // --- generate_with_image TDD ---

    /// RED → GREEN: the JSON body built for a multimodal request must have the
    /// Anthropic-specified shape: a single user message whose `content` array
    /// carries an `image` block (base64/png) followed by a `text` block.
    #[test]
    fn build_image_message_body_has_multimodal_content_blocks() {
        let png_bytes = b"\x89PNG\r\n\x1a\n"; // 8-byte PNG magic header
        let body =
            build_image_message_body("claude-test-model", 512, "describe this part", png_bytes);

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);

        let content = messages[0]["content"].as_array().unwrap();
        assert_eq!(
            content.len(),
            2,
            "expected exactly two content blocks: image then text"
        );

        // First block must be an image block with base64/png source.
        assert_eq!(content[0]["type"].as_str().unwrap(), "image");
        assert_eq!(content[0]["source"]["type"].as_str().unwrap(), "base64");
        assert_eq!(
            content[0]["source"]["media_type"].as_str().unwrap(),
            "image/png"
        );
        let data = content[0]["source"]["data"].as_str().unwrap();
        assert!(
            !data.is_empty(),
            "base64 data must be non-empty for non-empty input"
        );

        // Second block must be the text prompt.
        assert_eq!(content[1]["type"].as_str().unwrap(), "text");
        assert_eq!(content[1]["text"].as_str().unwrap(), "describe this part");
    }

    /// RED → GREEN: `generate_with_image` without an API key must return
    /// `ProviderUnavailable` — matching the policy enforced by `generate`.
    #[tokio::test]
    async fn generate_with_image_returns_unavailable_without_key() {
        let provider = ClaudeProvider::new(); // no API key set
        let result = provider
            .generate_with_image("does this part look sound?", b"\x89PNG")
            .await;
        assert!(
            matches!(result, Err(ProviderError::ProviderUnavailable(_))),
            "expected ProviderUnavailable when no key is configured, got {:?}",
            result
        );
    }

    /// P3a: the default model must be the current, live Anthropic model
    /// ID — never a retired/deprecated one. `claude-sonnet-4-20250514` is
    /// deprecated and `claude-3-5-sonnet-20241022` is retired/404s.
    #[test]
    fn default_model_is_not_a_retired_id() {
        let model = ClaudeConfig::default().model;
        assert_ne!(
            model, "claude-sonnet-4-20250514",
            "default model must not be the deprecated ID"
        );
        assert_ne!(
            model, "claude-3-5-sonnet-20241022",
            "default model must not be the retired ID"
        );
        assert_eq!(model, shared_types::DEFAULT_CLAUDE_MODEL);
    }

    /// P4: `generate()` must surface a non-success HTTP status as a typed
    /// `InferenceError` naming the real status code, exactly like its three
    /// siblings (`process_via_api`, `generate_with_image`,
    /// `generate_stream`). Without the status check, a 401 JSON error
    /// envelope (which has no `content` array) parses "successfully" and
    /// falls through to `extract_text_from_response`, which reports the
    /// unrelated "missing 'content' array" error instead of the real 401.
    /// This spins up a local (non-network, 127.0.0.1-only) mock server —
    /// not a live external API call.
    #[tokio::test]
    async fn generate_surfaces_non_success_status_as_typed_error() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding a local test listener must succeed");
        let addr = listener.local_addr().expect("local listener has an addr");

        let server = tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let body = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
                let response = format!(
                    "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });

        let config = ClaudeConfig {
            credential: Some(ClaudeCredential::ApiKey("test-key".to_string())),
            api_base: format!("http://{}", addr),
            ..ClaudeConfig::default()
        };
        let provider = ClaudeProvider::with_config(config);

        let result = provider.generate("hello", 100).await;
        let _ = server.await;

        match result {
            Err(ProviderError::InferenceError(msg)) => {
                assert!(
                    msg.contains("401"),
                    "expected the status check to surface the real HTTP status (401), got: {msg}"
                );
            }
            other => panic!(
                "expected an InferenceError naming the 401 status, got: {:?}",
                other
            ),
        }
    }

    // --- validate_credential: the PUT /api/ai/provider test-before-save
    // primitive. Never exercised before this suite (audit note: "the
    // latter has NEVER been run"). ---

    /// With no credential configured, `validate_credential` must refuse
    /// before ever touching the network — matching every other entry
    /// point's `require_credential` gate.
    #[tokio::test]
    async fn validate_credential_returns_unavailable_without_key() {
        let provider = ClaudeProvider::new();
        let result = provider.validate_credential().await;
        assert!(
            matches!(result, Err(ProviderError::ProviderUnavailable(_))),
            "expected ProviderUnavailable when no credential is configured, got {:?}",
            result
        );
    }

    /// A 401 from `/v1/models` must surface as `ProviderUnavailable`
    /// naming the status — the exact signal the AI provider connection
    /// dialog's PUT handler branches on to refuse saving a bad key.
    /// Local (127.0.0.1-only) mock server — not a live external call.
    #[tokio::test]
    async fn validate_credential_rejects_401_as_provider_unavailable_naming_status() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding a local test listener must succeed");
        let addr = listener.local_addr().expect("local listener has an addr");

        let server = tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let body = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
                let response = format!(
                    "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });

        let config = ClaudeConfig {
            credential: Some(ClaudeCredential::ApiKey("bad-key".to_string())),
            api_base: format!("http://{}", addr),
            ..ClaudeConfig::default()
        };
        let provider = ClaudeProvider::with_config(config);

        let result = provider.validate_credential().await;
        let _ = server.await;

        match result {
            Err(ProviderError::ProviderUnavailable(msg)) => {
                assert!(
                    msg.contains("401"),
                    "expected the rejection to name the 401 status, got: {msg}"
                );
            }
            other => panic!(
                "expected ProviderUnavailable naming the 401 status, got: {:?}",
                other
            ),
        }
    }

    /// A 200 from `/v1/models` must validate as `Ok(())` — the "credential
    /// accepted" path the PUT handler saves the config on.
    #[tokio::test]
    async fn validate_credential_succeeds_on_200() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding a local test listener must succeed");
        let addr = listener.local_addr().expect("local listener has an addr");

        let server = tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let body = r#"{"data":[]}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });

        let config = ClaudeConfig {
            credential: Some(ClaudeCredential::ApiKey("good-key".to_string())),
            api_base: format!("http://{}", addr),
            ..ClaudeConfig::default()
        };
        let provider = ClaudeProvider::with_config(config);

        let result = provider.validate_credential().await;
        let _ = server.await;

        assert!(
            result.is_ok(),
            "expected Ok(()) on a 200 response, got: {:?}",
            result
        );
    }

    // --- validate_model: the PUT /api/ai/provider model-honesty gate ---

    /// A 200 from `/v1/models/{id}` must validate as `Ok(())` — the model
    /// the caller asked for is one the credential can actually serve.
    #[tokio::test]
    async fn validate_model_succeeds_on_200() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding a local test listener must succeed");
        let addr = listener.local_addr().expect("local listener has an addr");

        let server = tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let body = format!(
                    r#"{{"id":"{}","type":"model"}}"#,
                    shared_types::DEFAULT_CLAUDE_MODEL
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });

        let config = ClaudeConfig {
            credential: Some(ClaudeCredential::ApiKey("good-key".to_string())),
            api_base: format!("http://{}", addr),
            ..ClaudeConfig::default()
        };
        let provider = ClaudeProvider::with_config(config);

        let result = provider
            .validate_model(shared_types::DEFAULT_CLAUDE_MODEL)
            .await;
        let _ = server.await;

        assert!(
            result.is_ok(),
            "expected Ok(()) on a 200 response, got: {:?}",
            result
        );
    }

    /// A 404 from `/v1/models/{id}` must surface as `ProviderUnavailable`
    /// naming the rejected model — the exact signal the PUT handler
    /// refuses a save on, and the exact wording the honesty requirement
    /// (never show a model as active that the provider did not accept)
    /// depends on.
    #[tokio::test]
    async fn validate_model_rejects_404_naming_the_model() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding a local test listener must succeed");
        let addr = listener.local_addr().expect("local listener has an addr");

        let server = tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = socket.read(&mut buf).await;
                let body = r#"{"type":"error","error":{"type":"not_found_error","message":"model not found"}}"#;
                let response = format!(
                    "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });

        let config = ClaudeConfig {
            credential: Some(ClaudeCredential::ApiKey("good-key".to_string())),
            api_base: format!("http://{}", addr),
            ..ClaudeConfig::default()
        };
        let provider = ClaudeProvider::with_config(config);

        let result = provider.validate_model("bogus-model-9000").await;
        let _ = server.await;

        match result {
            Err(ProviderError::ProviderUnavailable(msg)) => {
                assert!(
                    msg.contains("bogus-model-9000"),
                    "refusal must name the rejected model, got: {msg}"
                );
                assert!(
                    msg.contains("404"),
                    "refusal must name the status, got: {msg}"
                );
            }
            other => panic!(
                "expected ProviderUnavailable naming the rejected model, got: {:?}",
                other
            ),
        }
    }

    /// With no credential configured, `validate_model` must refuse before
    /// ever touching the network — matching every other entry point's
    /// `require_credential` gate.
    #[tokio::test]
    async fn validate_model_returns_unavailable_without_key() {
        let provider = ClaudeProvider::new();
        let result = provider
            .validate_model(shared_types::DEFAULT_CLAUDE_MODEL)
            .await;
        assert!(
            matches!(result, Err(ProviderError::ProviderUnavailable(_))),
            "expected ProviderUnavailable when no credential is configured, got {:?}",
            result
        );
    }

    /// P5: `generate()` must not hang forever against a stalled connection —
    /// the shared client must apply `config.request_timeout_secs`. This
    /// binds a local listener that accepts the connection and then never
    /// responds, and asserts `generate()` errors out well inside an outer
    /// 5s bound rather than hanging past it.
    #[tokio::test]
    async fn generate_respects_configured_request_timeout() {
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding a local test listener must succeed");
        let addr = listener.local_addr().expect("local listener has an addr");

        let _server = tokio::spawn(async move {
            if let Ok((socket, _)) = listener.accept().await {
                // Hold the connection open and never write a response,
                // simulating a stalled upstream.
                std::mem::forget(socket);
            }
        });

        let config = ClaudeConfig {
            credential: Some(ClaudeCredential::ApiKey("test-key".to_string())),
            api_base: format!("http://{}", addr),
            request_timeout_secs: 1,
            ..ClaudeConfig::default()
        };
        let provider = ClaudeProvider::with_config(config);

        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            provider.generate("hello", 100),
        )
        .await;

        match outcome {
            Ok(Err(ProviderError::InferenceError(_))) => {} // client-level timeout fired — expected
            Ok(Ok(_)) => panic!("a stalled connection must not succeed"),
            Ok(Err(other)) => panic!(
                "expected an InferenceError from the stalled connection, got: {:?}",
                other
            ),
            Err(_) => panic!(
                "generate() did not return within the outer 5s bound — \
                 request_timeout_secs is not being applied to the client (P5 regression)"
            ),
        }
    }
}
