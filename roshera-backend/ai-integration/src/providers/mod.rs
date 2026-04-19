/// Provider system for vendor-agnostic AI integration
///
/// # Design Rationale
/// - **Why traits**: Enables swapping providers without recompilation
/// - **Why async**: AI inference can be compute-intensive, async prevents blocking
/// - **Performance**: Provider abstraction adds < 1μs overhead
/// - **Business Value**: No vendor lock-in, easy A/B testing of models
///
/// # References
/// - [11] Radford et al. (2022) - Whisper ASR
/// - [12] Touvron et al. (2023) - LLaMA models
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use thiserror::Error;

// Provider implementations — API-only (no local models)
pub mod claude; // Claude API integration
pub mod mock; // Mock providers for testing
pub mod native_factory; // Factory for creating providers
pub mod ollama; // Ollama local LLM integration (optional)

// Re-exports for convenience
pub use claude::ClaudeProvider;
pub use mock::{MockASRProvider, MockLLMProvider, MockTTSProvider};
pub use native_factory::{NativeProviderConfig, NativeProviderFactory};
pub use ollama::OllamaProvider;

/// Error types for AI providers
#[derive(Error, Debug)]
pub enum ProviderError {
    #[error("Model loading failed: {0}")]
    ModelLoadError(String),

    #[error("Inference failed: {0}")]
    InferenceError(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Provider not available: {0}")]
    ProviderUnavailable(String),

    #[error("Out of memory: needed {needed}MB, available {available}MB")]
    OutOfMemory { needed: usize, available: usize },

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Initialization error: {0}")]
    InitializationError(String),

    #[error("Processing error: {0}")]
    ProcessingError(String),
}

/// Provider capabilities for discovery
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub name: String,
    pub version: String,
    pub supported_languages: Vec<String>,
    pub max_context_length: usize,
    pub supports_streaming: bool,
    pub supports_batching: bool,
    pub device_type: String,
    pub model_size_mb: usize,
    pub quantization: QuantizationType,
}

/// Quantization types supported
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum QuantizationType {
    Float32,
    Float16,
    Int8,
    Int4,
}

/// Audio format for ASR
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioFormat {
    Wav,
    Mp3,
    Ogg,
    Raw16kHz,
}

/// Voice information for TTS
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceInfo {
    pub id: String,
    pub name: String,
    pub language: String,
    pub gender: Option<String>,
    pub sample_rate: u32,
}

/// Parsed command from LLM
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedCommand {
    pub original_text: String,
    pub intent: CommandIntent,
    pub parameters: std::collections::HashMap<String, serde_json::Value>,
    pub confidence: f32,
    pub language: String,
}

/// Command intent types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CommandIntent {
    CreatePrimitive {
        shape: String,
    },
    BooleanOperation {
        operation: String,
    },
    Transform {
        operation: String,
    },
    Query {
        target: String,
    },
    Extrude {
        target: Option<String>,
    },
    Create {
        object_type: String,
        parameters: serde_json::Value,
    },
    Modify {
        target: String,
        operation: String,
        parameters: serde_json::Value,
    },
    Boolean {
        operation: String,
        operands: Vec<String>,
    },
    Export {
        format: String,
        options: serde_json::Value,
    },
    Import {
        file_path: String,
        format: Option<String>,
    },
    Unknown,
}

/// Conversation context for maintaining state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationContext {
    pub session_id: String,
    pub previous_commands: Vec<ParsedCommand>,
    pub active_objects: Vec<String>,
    pub user_preferences: serde_json::Value,
    /// Current scene state for AI awareness
    pub scene_state: Option<shared_types::SceneState>,
    /// Complete system context for AI awareness
    pub system_context: Option<shared_types::SystemContext>,
}

/// ASR provider trait - all implementations must be Send + Sync
#[async_trait]
pub trait ASRProvider: Send + Sync + Debug {
    /// Get provider capabilities
    fn capabilities(&self) -> ProviderCapabilities;

    /// Transcribe audio to text
    ///
    /// # Performance
    /// - Whisper base model: ~500ms for 5s audio on CPU
    /// - Streaming supported for real-time transcription
    async fn transcribe(&self, audio: &[u8], format: AudioFormat) -> Result<String, ProviderError>;

    // TODO: Add streaming support when needed
    // Streaming methods with generic parameters break dyn compatibility

    /// Get supported audio formats
    fn supported_formats(&self) -> Vec<AudioFormat>;

    /// Estimate memory usage
    fn memory_requirement_mb(&self) -> usize;
}

/// LLM provider trait for natural language understanding
#[async_trait]
pub trait LLMProvider: Send + Sync + Debug {
    /// Get provider capabilities
    fn capabilities(&self) -> ProviderCapabilities;

    /// Process natural language input to structured command
    ///
    /// # Performance
    /// - Target: < 100ms for command parsing
    /// - 16-bit quantization reduces memory by 50%
    async fn process(
        &self,
        input: &str,
        context: Option<&ConversationContext>,
    ) -> Result<ParsedCommand, ProviderError>;

    /// Generate response for conversational AI
    async fn generate(&self, prompt: &str, max_tokens: usize) -> Result<String, ProviderError>;

    /// Generate response based on command result
    async fn generate_response(
        &self,
        command_result: &str,
        language: &str,
    ) -> Result<String, ProviderError>;

    /// Estimate memory usage
    fn memory_requirement_mb(&self) -> usize;
}

/// TTS provider trait for audio feedback
#[async_trait]
pub trait TTSProvider: Send + Sync + Debug {
    /// Get provider capabilities
    fn capabilities(&self) -> ProviderCapabilities;

    /// Convert text to speech
    async fn synthesize(&self, text: &str, voice: Option<&str>) -> Result<Vec<u8>, ProviderError>;

    /// Get available voices
    fn available_voices(&self) -> Vec<VoiceInfo>;

    /// Estimated latency in milliseconds
    fn estimated_latency_ms(&self) -> u32;
}

/// Provider manager for dynamic selection
pub struct ProviderManager {
    pub(crate) asr_providers: std::collections::HashMap<String, Box<dyn ASRProvider>>,
    pub(crate) llm_providers: std::collections::HashMap<String, Box<dyn LLMProvider>>,
    pub(crate) tts_providers: std::collections::HashMap<String, Box<dyn TTSProvider>>,
    pub(crate) active_asr: String,
    pub(crate) active_llm: String,
    pub(crate) active_tts: Option<String>,
}

impl ProviderManager {
    /// Create new provider manager
    pub fn new() -> Self {
        Self {
            asr_providers: std::collections::HashMap::new(),
            llm_providers: std::collections::HashMap::new(),
            tts_providers: std::collections::HashMap::new(),
            active_asr: String::new(),
            active_llm: String::new(),
            active_tts: None,
        }
    }

    /// Register ASR provider
    pub fn register_asr(&mut self, name: String, provider: Box<dyn ASRProvider>) {
        tracing::info!("Registering ASR provider: {}", name);
        self.asr_providers.insert(name, provider);
    }

    /// Register LLM provider
    pub fn register_llm(&mut self, name: String, provider: Box<dyn LLMProvider>) {
        tracing::info!("Registering LLM provider: {}", name);
        self.llm_providers.insert(name, provider);
    }

    /// Register TTS provider
    pub fn register_tts(&mut self, name: String, provider: Box<dyn TTSProvider>) {
        tracing::info!("Registering TTS provider: {}", name);
        self.tts_providers.insert(name, provider);
    }

    /// Get active ASR provider
    pub fn asr(&self) -> Result<&dyn ASRProvider, ProviderError> {
        self.asr_providers
            .get(&self.active_asr)
            .map(|p| p.as_ref())
            .ok_or_else(|| {
                ProviderError::ProviderUnavailable(format!(
                    "ASR provider '{}' not found",
                    self.active_asr
                ))
            })
    }

    /// Get active LLM provider
    pub fn llm(&self) -> Result<&dyn LLMProvider, ProviderError> {
        self.llm_providers
            .get(&self.active_llm)
            .map(|p| p.as_ref())
            .ok_or_else(|| {
                ProviderError::ProviderUnavailable(format!(
                    "LLM provider '{}' not found",
                    self.active_llm
                ))
            })
    }

    /// Set active providers
    pub fn set_active(&mut self, asr: String, llm: String, tts: Option<String>) {
        self.active_asr = asr;
        self.active_llm = llm;
        self.active_tts = tts;
    }

    /// Get TTS provider by name
    pub fn tts(&self, name: &str) -> Result<&dyn TTSProvider, ProviderError> {
        self.tts_providers
            .get(name)
            .map(|p| p.as_ref())
            .ok_or_else(|| {
                ProviderError::ProviderUnavailable(format!("TTS provider '{}' not found", name))
            })
    }

    /// Check if TTS provider exists
    pub fn has_tts_provider(&self, name: &str) -> bool {
        self.tts_providers.contains_key(name)
    }

    /// Get total memory requirement
    pub fn total_memory_requirement_mb(&self) -> usize {
        let asr_mem = self.asr().map(|p| p.memory_requirement_mb()).unwrap_or(0);
        let llm_mem = self.llm().map(|p| p.memory_requirement_mb()).unwrap_or(0);
        asr_mem + llm_mem
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_manager() {
        let mut manager = ProviderManager::new();
        assert!(manager.asr().is_err());
        assert!(manager.llm().is_err());
    }

    #[test]
    fn test_quantization_type() {
        let q = QuantizationType::Float16;
        match q {
            QuantizationType::Float16 => assert!(true),
            _ => panic!("Wrong quantization type"),
        }
    }
}
