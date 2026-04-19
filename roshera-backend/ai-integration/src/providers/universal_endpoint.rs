/// Universal Endpoint for all LLM providers.
///
/// This module provides a single, unified interface for interacting with
/// multiple hosted LLM providers (OpenAI, Anthropic, Google, HuggingFace,
/// and a generic CustomAPI) without code duplication. The endpoint
/// automatically formats requests and parses responses according to each
/// provider's API specification.
///
/// Policy: API-only. Local-model runtimes are not supported.
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, error, info, warn};

use crate::providers::{LLMProvider, ParsedCommand, ProviderCapabilities, ProviderError};
use shared_types::vision::{ViewportCapture, VisionProviderType};

/// Universal endpoint errors
#[derive(Error, Debug)]
pub enum UniversalEndpointError {
    #[error("HTTP request failed: {0}")]
    HttpError(#[from] reqwest::Error),
    
    #[error("Invalid response format from provider: {0}")]
    InvalidResponse(String),
    
    #[error("Provider not supported: {0:?}")]
    UnsupportedProvider(VisionProviderType),
    
    #[error("Authentication failed: {0}")]
    AuthenticationError(String),
    
    #[error("Rate limit exceeded")]
    RateLimitExceeded,
    
    #[error("Timeout after {0} seconds")]
    Timeout(u64),
    
    #[error("JSON parsing error: {0}")]
    JsonError(#[from] serde_json::Error),
}

/// Configuration for the universal endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UniversalEndpointConfig {
    /// Provider type
    pub provider: VisionProviderType,
    
    /// API endpoint URL
    pub url: String,
    
    /// API key (if required)
    pub api_key: Option<String>,
    
    /// Model name
    pub model_name: String,
    
    /// Request timeout in seconds
    pub timeout_secs: u64,
    
    /// Maximum tokens for response
    pub max_tokens: usize,
    
    /// Temperature for generation
    pub temperature: f32,
    
    /// System prompt for context
    pub system_prompt: Option<String>,
}

impl Default for UniversalEndpointConfig {
    fn default() -> Self {
        Self {
            provider: VisionProviderType::Anthropic,
            url: "https://api.anthropic.com/v1/messages".to_string(),
            api_key: None,
            model_name: "claude-3-5-sonnet-20241022".to_string(),
            timeout_secs: 30,
            max_tokens: 1000,
            temperature: 0.7,
            system_prompt: Some(
                "You are a CAD assistant that helps users create and modify 3D geometry. \
                 Parse user commands and respond with structured CAD operations."
                    .to_string(),
            ),
        }
    }
}

/// Universal endpoint that handles all providers
pub struct UniversalEndpoint {
    config: UniversalEndpointConfig,
    client: Client,
}

impl std::fmt::Debug for UniversalEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UniversalEndpoint")
            .field("config", &self.config)
            .field("client", &"<reqwest::Client>")
            .finish()
    }
}

impl UniversalEndpoint {
    /// Create new universal endpoint
    pub fn new(config: UniversalEndpointConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .unwrap_or_default();
        
        Self { config, client }
    }
    
    /// Process text and optional viewport capture
    pub async fn process_with_vision(
        &self,
        text: &str,
        viewport: Option<&ViewportCapture>,
    ) -> Result<ParsedCommand, UniversalEndpointError> {
        info!("Processing command with vision using {:?} provider", self.config.provider);
        
        // Build request based on provider
        let request_body = self.build_request(text, viewport)?;
        
        // Send request
        let response = self.send_request(request_body).await?;
        
        // Parse response based on provider
        let parsed = self.parse_response(response)?;
        
        Ok(parsed)
    }
    
    /// Build request body based on provider type
    fn build_request(
        &self,
        text: &str,
        viewport: Option<&ViewportCapture>,
    ) -> Result<Value, UniversalEndpointError> {
        let request = match self.config.provider {
            VisionProviderType::OpenAI => self.build_openai_request(text, viewport),
            VisionProviderType::Anthropic => self.build_anthropic_request(text, viewport),
            VisionProviderType::Google => self.build_google_request(text, viewport),
            VisionProviderType::HuggingFace => self.build_huggingface_request(text, viewport),
            VisionProviderType::CustomAPI => self.build_custom_request(text, viewport),
        };
        
        debug!("Built request for {:?}: {:?}", self.config.provider, request);
        Ok(request)
    }
    
    /// Build OpenAI request
    fn build_openai_request(&self, text: &str, viewport: Option<&ViewportCapture>) -> Value {
        let mut messages = vec![];
        
        // System message
        if let Some(system) = &self.config.system_prompt {
            messages.push(json!({
                "role": "system",
                "content": system
            }));
        }
        
        // Build user message with optional vision
        let mut user_content = vec![];
        
        // Add text
        user_content.push(json!({
            "type": "text",
            "text": if let Some(vp) = viewport {
                format!("{}\n\nUser command: {}", self.format_viewport_context(vp), text)
            } else {
                text.to_string()
            }
        }));
        
        // Add image if available and using vision model
        if let Some(vp) = viewport {
            if self.config.model_name.contains("vision") || self.config.model_name == "gpt-4o" {
                user_content.push(json!({
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:image/png;base64,{}", vp.image)
                    }
                }));
            }
        }
        
        messages.push(json!({
            "role": "user",
            "content": user_content
        }));
        
        json!({
            "model": self.config.model_name,
            "messages": messages,
            "max_tokens": self.config.max_tokens,
            "temperature": self.config.temperature
        })
    }
    
    /// Build Anthropic request
    fn build_anthropic_request(&self, text: &str, viewport: Option<&ViewportCapture>) -> Value {
        let mut messages = vec![];
        
        // Build user message
        let mut content = vec![];
        
        // Add viewport context and text
        if let Some(vp) = viewport {
            content.push(json!({
                "type": "text",
                "text": self.format_viewport_context(vp)
            }));
            
            // Add image for vision models
            if self.config.model_name.contains("claude-3") {
                content.push(json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": vp.image
                    }
                }));
            }
        }
        
        content.push(json!({
            "type": "text",
            "text": format!("User command: {}", text)
        }));
        
        messages.push(json!({
            "role": "user",
            "content": content
        }));
        
        json!({
            "model": self.config.model_name,
            "messages": messages,
            "max_tokens": self.config.max_tokens,
            "temperature": self.config.temperature,
            "system": self.config.system_prompt
        })
    }
    
    /// Build Google request
    fn build_google_request(&self, text: &str, viewport: Option<&ViewportCapture>) -> Value {
        let mut parts = vec![];
        
        // Add text part
        let text_content = if let Some(vp) = viewport {
            format!("{}\n\nUser command: {}", self.format_viewport_context(vp), text)
        } else {
            text.to_string()
        };
        
        parts.push(json!({
            "text": text_content
        }));
        
        // Add image if available
        if let Some(vp) = viewport {
            parts.push(json!({
                "inline_data": {
                    "mime_type": "image/png",
                    "data": vp.image
                }
            }));
        }
        
        json!({
            "contents": [{
                "parts": parts
            }],
            "generationConfig": {
                "temperature": self.config.temperature,
                "maxOutputTokens": self.config.max_tokens
            }
        })
    }
    
    /// Build HuggingFace request
    fn build_huggingface_request(&self, text: &str, viewport: Option<&ViewportCapture>) -> Value {
        let inputs = if let Some(vp) = viewport {
            format!("{}\n\nUser command: {}", self.format_viewport_context(vp), text)
        } else {
            text.to_string()
        };
        
        json!({
            "inputs": inputs,
            "parameters": {
                "max_new_tokens": self.config.max_tokens,
                "temperature": self.config.temperature,
                "return_full_text": false
            }
        })
    }
    
    /// Build custom API request (generic format)
    fn build_custom_request(&self, text: &str, viewport: Option<&ViewportCapture>) -> Value {
        json!({
            "prompt": text,
            "viewport": viewport,
            "model": self.config.model_name,
            "max_tokens": self.config.max_tokens,
            "temperature": self.config.temperature
        })
    }
    
    /// Format viewport context as text
    fn format_viewport_context(&self, viewport: &ViewportCapture) -> String {
        let mut context = String::from("Current 3D viewport context:\n");
        
        // Camera position
        context.push_str(&format!(
            "- Camera at ({:.2}, {:.2}, {:.2})\n",
            viewport.camera.position[0],
            viewport.camera.position[1],
            viewport.camera.position[2]
        ));
        
        // Cursor target
        if let Some(target) = &viewport.cursor_target {
            context.push_str(&format!(
                "- Cursor pointing at {} (ID: {:?})\n",
                target.object_type.as_deref().unwrap_or("object"),
                target.object_id
            ));
        }
        
        // Selected objects
        if !viewport.selection.object_ids.is_empty() {
            context.push_str(&format!(
                "- {} objects selected\n",
                viewport.selection.object_ids.len()
            ));
        }
        
        // Scene objects summary
        context.push_str(&format!(
            "- {} objects in scene\n",
            viewport.scene_objects.len()
        ));
        
        // List visible objects
        for obj in &viewport.scene_objects {
            if obj.visible {
                context.push_str(&format!(
                    "  - {} '{}' at ({:.2}, {:.2}, {:.2})",
                    obj.object_type, obj.name,
                    obj.position[0], obj.position[1], obj.position[2]
                ));
                if obj.selected {
                    context.push_str(" [SELECTED]");
                }
                context.push('\n');
            }
        }
        
        context
    }
    
    /// Send HTTP request to provider
    async fn send_request(&self, body: Value) -> Result<Value, UniversalEndpointError> {
        let mut request = self.client
            .post(&self.config.url)
            .json(&body);
        
        // Add authentication if needed
        match self.config.provider {
            VisionProviderType::OpenAI => {
                if let Some(key) = &self.config.api_key {
                    request = request.header("Authorization", format!("Bearer {}", key));
                }
            }
            VisionProviderType::Anthropic => {
                if let Some(key) = &self.config.api_key {
                    request = request.header("x-api-key", key);
                    request = request.header("anthropic-version", "2023-06-01");
                }
            }
            VisionProviderType::Google => {
                if let Some(key) = &self.config.api_key {
                    request = request.header("x-goog-api-key", key);
                }
            }
            VisionProviderType::HuggingFace => {
                if let Some(key) = &self.config.api_key {
                    request = request.header("Authorization", format!("Bearer {}", key));
                }
            }
            _ => {}
        }
        
        let response = request.send().await?;
        
        // Check status
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            
            if status.as_u16() == 429 {
                return Err(UniversalEndpointError::RateLimitExceeded);
            } else if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(UniversalEndpointError::AuthenticationError(text));
            } else {
                return Err(UniversalEndpointError::InvalidResponse(
                    format!("HTTP {}: {}", status, text)
                ));
            }
        }
        
        response.json().await.map_err(Into::into)
    }
    
    /// Parse response based on provider type
    fn parse_response(&self, response: Value) -> Result<ParsedCommand, UniversalEndpointError> {
        let text = match self.config.provider {
            VisionProviderType::OpenAI => {
                response["choices"][0]["message"]["content"].as_str()
                    .ok_or_else(|| UniversalEndpointError::InvalidResponse(
                        "Missing choices[0].message.content".to_string()
                    ))?
                    .to_string()
            }
            VisionProviderType::Anthropic => {
                response["content"][0]["text"].as_str()
                    .ok_or_else(|| UniversalEndpointError::InvalidResponse(
                        "Missing content[0].text".to_string()
                    ))?
                    .to_string()
            }
            VisionProviderType::Google => {
                response["candidates"][0]["content"]["parts"][0]["text"].as_str()
                    .ok_or_else(|| UniversalEndpointError::InvalidResponse(
                        "Missing candidates[0].content.parts[0].text".to_string()
                    ))?
                    .to_string()
            }
            VisionProviderType::HuggingFace => {
                response[0]["generated_text"].as_str()
                    .ok_or_else(|| UniversalEndpointError::InvalidResponse(
                        "Missing [0].generated_text".to_string()
                    ))?
                    .to_string()
            }
            VisionProviderType::CustomAPI => {
                response["text"].as_str()
                    .or_else(|| response["response"].as_str())
                    .or_else(|| response["output"].as_str())
                    .ok_or_else(|| UniversalEndpointError::InvalidResponse(
                        "No recognized text field in response".to_string()
                    ))?
                    .to_string()
            }
        };
        
        // Parse the text response into a command
        self.parse_command_from_text(&text)
    }
    
    /// Parse command from LLM text response
    fn parse_command_from_text(&self, text: &str) -> Result<ParsedCommand, UniversalEndpointError> {
        // Try to parse as JSON first
        if let Ok(json) = serde_json::from_str::<Value>(text) {
            if let Ok(cmd) = serde_json::from_value::<ParsedCommand>(json) {
                return Ok(cmd);
            }
        }
        
        // Otherwise, create a basic parsed command
        // In production, this would use more sophisticated parsing
        Ok(ParsedCommand {
            original_text: text.to_string(),
            intent: crate::providers::CommandIntent::Unknown,
            parameters: std::collections::HashMap::new(),
            confidence: 0.5,
            language: "en".to_string(),
        })
    }
}

#[async_trait]
impl LLMProvider for UniversalEndpoint {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            name: format!("Universal-{:?}", self.config.provider),
            version: "1.0.0".to_string(),
            supported_languages: vec!["en".to_string()],
            max_context_length: 4096,
            supports_streaming: false,
            supports_batching: false,
            device_type: "Cloud/Local".to_string(),
            model_size_mb: 0, // Unknown for API-based
            quantization: crate::providers::QuantizationType::Float16,
        }
    }
    
    async fn process(
        &self,
        input: &str,
        _context: Option<&crate::providers::ConversationContext>,
    ) -> Result<ParsedCommand, ProviderError> {
        self.process_with_vision(input, None)
            .await
            .map_err(|e| ProviderError::ProcessingError(e.to_string()))
    }
    
    async fn generate(&self, prompt: &str, _max_tokens: usize) -> Result<String, ProviderError> {
        let cmd = self.process_with_vision(prompt, None)
            .await
            .map_err(|e| ProviderError::ProcessingError(e.to_string()))?;
        
        Ok(cmd.original_text)
    }
    
    async fn generate_response(
        &self,
        command_result: &str,
        _language: &str,
    ) -> Result<String, ProviderError> {
        Ok(command_result.to_string())
    }
    
    fn memory_requirement_mb(&self) -> usize {
        0 // API-based, no local memory
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_config_default() {
        let config = UniversalEndpointConfig::default();
        assert_eq!(config.provider, VisionProviderType::Anthropic);
        assert_eq!(config.timeout_secs, 30);
    }
    
    #[test]
    fn test_viewport_context_formatting() {
        let viewport = ViewportCapture {
            image: "base64data".to_string(),
            camera: shared_types::vision::CameraInfo {
                position: [10.0, 20.0, 30.0],
                rotation: [0.0, 0.0, 0.0],
                quaternion: [0.0, 0.0, 0.0, 1.0],
                target: [0.0, 0.0, 0.0],
                up: [0.0, 1.0, 0.0],
                fov: 50.0,
                aspect: 1.6,
                near: 0.1,
                far: 1000.0,
                zoom: 1.0,
                matrix_world: [0.0; 16],
                projection_matrix: [0.0; 16],
            },
            cursor_target: None,
            scene_objects: vec![],
            selection: shared_types::vision::SelectionInfo {
                object_ids: vec![],
                bounding_box: None,
                center: None,
            },
            viewport: shared_types::vision::ViewportInfo {
                width: 1920,
                height: 1080,
                client_width: 1920,
                client_height: 1080,
                pixel_ratio: 1.0,
                mouse_screen: shared_types::vision::MousePosition { x: 0.0, y: 0.0 },
                mouse_pixels: shared_types::vision::PixelPosition { x: 960.0, y: 540.0 },
                mouse_world: None,
            },
            lighting: vec![],
            clipping_planes: vec![],
            render_stats: shared_types::vision::RenderStats {
                triangles: 0,
                points: 0,
                lines: 0,
                frame: 0,
                calls: 0,
                vertices: 0,
                faces: 0,
            },
            measurements: shared_types::vision::Measurements {
                distance_between_selected: None,
                camera_to_selection: None,
            },
            timestamp: 0,
        };
        
        let endpoint = UniversalEndpoint::new(UniversalEndpointConfig::default());
        let context = endpoint.format_viewport_context(&viewport);
        
        assert!(context.contains("Camera at (10.00, 20.00, 30.00)"));
        assert!(context.contains("0 objects in scene"));
    }
}