//! Smart router for vision-aware AI command processing
//!
//! Routes commands through the appropriate AI pipeline based on whether
//! they require visual context (viewport capture) or can be processed
//! as text-only commands.

use crate::providers::ParsedCommand;
use shared_types::vision::{ProcessingMode, ViewportCapture, VisionConfig};
use std::fmt;

/// Error type for smart router operations
#[derive(Debug)]
pub enum SmartRouterError {
    /// Configuration error
    ConfigError(String),
    /// Provider error during processing
    ProviderError(String),
    /// Timeout during vision processing
    Timeout(String),
}

impl fmt::Display for SmartRouterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfigError(msg) => write!(f, "SmartRouter config error: {msg}"),
            Self::ProviderError(msg) => write!(f, "SmartRouter provider error: {msg}"),
            Self::Timeout(msg) => write!(f, "SmartRouter timeout: {msg}"),
        }
    }
}

impl std::error::Error for SmartRouterError {}

/// Configuration for the smart router
#[derive(Debug, Clone)]
pub struct SmartRouterConfig {
    /// Processing mode (unified or separated vision/reasoning)
    pub mode: ProcessingMode,
    /// Vision provider configuration
    pub vision_config: VisionConfig,
    /// Optional separate reasoning provider (required for Separated mode)
    pub reasoning_config: Option<VisionConfig>,
    /// Enable response caching
    pub enable_cache: bool,
    /// Cache TTL in seconds
    pub cache_ttl_secs: u64,
    /// Maximum retry attempts
    pub max_retries: u32,
    /// Vision processing timeout in seconds
    pub vision_timeout_secs: u64,
    /// Reasoning processing timeout in seconds
    pub reasoning_timeout_secs: u64,
}

impl Default for SmartRouterConfig {
    fn default() -> Self {
        Self {
            mode: ProcessingMode::Unified,
            vision_config: VisionConfig {
                provider: shared_types::vision::VisionProviderType::CustomAPI,
                url: "http://localhost:11434/api/generate".to_string(),
                api_key: None,
                model_name: "llava:latest".to_string(),
            },
            reasoning_config: None,
            enable_cache: true,
            cache_ttl_secs: 300,
            max_retries: 3,
            vision_timeout_secs: 30,
            reasoning_timeout_secs: 30,
        }
    }
}

/// Smart router that directs commands through vision or text-only pipelines
pub struct SmartRouter {
    config: SmartRouterConfig,
}

/// Keywords that indicate a command needs visual context
const VISION_KEYWORDS: &[&str] = &[
    "this",
    "that",
    "these",
    "those",
    "here",
    "there",
    "select",
    "selected",
    "pointing",
    "cursor",
    "click",
    "red",
    "blue",
    "green",
    "yellow",
    "white",
    "black",
    "left",
    "right",
    "top",
    "bottom",
    "front",
    "back",
    "the",
    "move the",
    "rotate the",
    "scale the",
    "make that",
    "make this",
];

impl SmartRouter {
    /// Create a new SmartRouter with the given configuration
    pub fn new(config: SmartRouterConfig) -> Result<Self, SmartRouterError> {
        if config.mode == ProcessingMode::Separated && config.reasoning_config.is_none() {
            return Err(SmartRouterError::ConfigError(
                "Separated mode requires a reasoning_config".to_string(),
            ));
        }
        Ok(Self { config })
    }

    /// Determine if a command requires viewport vision context
    pub fn requires_vision(command: &str) -> bool {
        let lower = command.to_lowercase();
        VISION_KEYWORDS
            .iter()
            .any(|keyword| lower.contains(keyword))
    }

    /// Process a command with viewport vision context
    pub async fn process_with_vision(
        &self,
        command: &str,
        _viewport: &ViewportCapture,
    ) -> Result<ParsedCommand, SmartRouterError> {
        // Build the vision-augmented prompt with viewport context
        // In production, this sends the viewport image + scene metadata
        // to the vision model for spatial understanding
        Err(SmartRouterError::ProviderError(format!(
            "No vision provider connected for command: {command}"
        )))
    }

    /// Process a text-only command without viewport context
    pub async fn process_text_only(
        &self,
        command: &str,
    ) -> Result<ParsedCommand, SmartRouterError> {
        // Route through the text-only LLM pipeline
        Err(SmartRouterError::ProviderError(format!(
            "No text provider connected for command: {command}"
        )))
    }

    /// Get the current processing mode
    pub fn mode(&self) -> &ProcessingMode {
        &self.config.mode
    }
}
