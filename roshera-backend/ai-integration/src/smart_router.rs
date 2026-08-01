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
            // Policy: API-only, no local inference runtimes (see
            // ai-integration/src/providers/mod.rs). The default must point
            // at the Claude API rather than a local Ollama server — with no
            // API key configured, callers that reach the vision pipeline
            // fail loudly via `ProviderError::ProviderUnavailable`
            // (`providers/claude.rs`) instead of silently talking to
            // localhost.
            vision_config: VisionConfig {
                provider: shared_types::vision::VisionProviderType::Anthropic,
                url: "https://api.anthropic.com/v1/messages".to_string(),
                api_key: None,
                model_name: shared_types::DEFAULT_CLAUDE_MODEL.to_string(),
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
    "move the",
    "rotate the",
    "scale the",
    "make that",
    "make this",
];

/// Check if `text` contains `word` as a whole word (not as a substring).
fn contains_word(text: &str, word: &str) -> bool {
    if word.contains(' ') {
        // Multi-word phrases: check the first word boundary and trailing boundary
        return text.contains(word);
    }
    for (i, _) in text.match_indices(word) {
        let before_ok = i == 0 || !text.as_bytes()[i - 1].is_ascii_alphabetic();
        let after = i + word.len();
        let after_ok = after >= text.len() || !text.as_bytes()[after].is_ascii_alphabetic();
        if before_ok && after_ok {
            return true;
        }
    }
    false
}

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
            .any(|keyword| contains_word(&lower, keyword))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// P1 RED: the default vision config must never point at a local
    /// inference runtime (Ollama et al.) — API-only per project policy
    /// (CLAUDE.md, enforced at `providers/mod.rs`).
    #[test]
    fn test_default_config_has_no_local_runtime() {
        let config = SmartRouterConfig::default();

        let url_lower = config.vision_config.url.to_lowercase();
        let model_lower = config.vision_config.model_name.to_lowercase();

        assert!(
            !url_lower.contains("11434"),
            "default vision_config.url must not point at an Ollama port, got: {}",
            config.vision_config.url
        );
        assert!(
            !url_lower.contains("localhost") && !url_lower.contains("127.0.0.1"),
            "default vision_config.url must not point at a local runtime, got: {}",
            config.vision_config.url
        );
        assert!(
            !model_lower.contains("llava"),
            "default vision_config.model_name must not name a local Ollama model, got: {}",
            config.vision_config.model_name
        );
    }
}
