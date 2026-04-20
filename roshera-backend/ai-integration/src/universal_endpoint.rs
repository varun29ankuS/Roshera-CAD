//! Universal endpoint for multi-provider vision API access
//!
//! Provides a unified interface for sending vision requests to different
//! AI providers (Ollama, OpenAI, Anthropic, Google, HuggingFace, custom).

use shared_types::vision::VisionProviderType;

/// Configuration for a universal endpoint
#[derive(Debug, Clone)]
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
    /// Maximum tokens in response
    pub max_tokens: u32,
    /// Temperature for generation
    pub temperature: f64,
    /// Optional system prompt
    pub system_prompt: Option<String>,
}

impl Default for UniversalEndpointConfig {
    fn default() -> Self {
        Self {
            provider: VisionProviderType::Ollama,
            url: "http://localhost:11434/api/generate".to_string(),
            api_key: None,
            model_name: "llava:latest".to_string(),
            timeout_secs: 30,
            max_tokens: 1000,
            temperature: 0.7,
            system_prompt: None,
        }
    }
}

/// Capabilities reported by an endpoint
#[derive(Debug, Clone)]
pub struct EndpointCapabilities {
    /// Human-readable name combining "Universal-{provider}"
    pub name: String,
    /// Whether this endpoint supports image input
    pub supports_vision: bool,
    /// Whether this endpoint supports streaming
    pub supports_streaming: bool,
    /// Maximum context length
    pub max_context: usize,
}

/// Universal endpoint that adapts requests to different provider APIs
pub struct UniversalEndpoint {
    config: UniversalEndpointConfig,
}

impl UniversalEndpoint {
    /// Create a new universal endpoint
    pub fn new(config: UniversalEndpointConfig) -> Self {
        Self { config }
    }

    /// Get the capabilities of this endpoint
    pub fn capabilities(&self) -> EndpointCapabilities {
        let name = format!("Universal-{:?}", self.config.provider);
        let (supports_vision, supports_streaming, max_context) = match self.config.provider {
            VisionProviderType::Ollama => (true, true, 4096),
            VisionProviderType::OpenAI => (true, true, 128_000),
            VisionProviderType::Anthropic => (true, true, 200_000),
            VisionProviderType::Google => (true, true, 32_000),
            VisionProviderType::HuggingFace => (true, false, 4096),
            VisionProviderType::CustomAPI => (true, false, 4096),
        };
        EndpointCapabilities {
            name,
            supports_vision,
            supports_streaming,
            max_context,
        }
    }

    /// Get the provider type
    pub fn provider(&self) -> &VisionProviderType {
        &self.config.provider
    }
}
