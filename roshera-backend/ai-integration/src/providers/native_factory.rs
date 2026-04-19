/// Provider factory for API-based AI integration
///
/// # Design Rationale
/// - **Why factory pattern**: Clean initialization of provider manager
/// - **Why API-only**: No local models — uses Claude API for LLM, mock for ASR/TTS
/// - **Performance**: Fast startup, no model loading overhead
/// - **Business Value**: Simpler deployment, no GPU requirements
use super::*;

/// Configuration for AI providers
#[derive(Debug, Clone)]
pub struct NativeProviderConfig {
    /// Claude API model to use (e.g. "claude-sonnet-4-20250514")
    pub claude_model: String,
    /// Maximum tokens for LLM responses
    pub max_tokens: usize,
}

impl Default for NativeProviderConfig {
    fn default() -> Self {
        Self {
            claude_model: "claude-sonnet-4-20250514".to_string(),
            max_tokens: 4096,
        }
    }
}

/// Factory for creating AI providers
pub struct NativeProviderFactory;

impl NativeProviderFactory {
    /// Create a Claude API LLM provider
    pub async fn create_claude_provider(
        _config: &NativeProviderConfig,
    ) -> Result<ClaudeProvider, ProviderError> {
        tracing::info!("Creating Claude API LLM provider");
        Ok(ClaudeProvider::new())
    }

    /// Create a complete provider manager with API-based providers
    ///
    /// Uses Claude for LLM and mock providers for ASR/TTS.
    /// ASR and TTS will be replaced with API providers (e.g. OpenAI Whisper API)
    /// when those integrations are built.
    pub async fn create_provider_manager(
        config: &NativeProviderConfig,
    ) -> Result<ProviderManager, ProviderError> {
        let mut manager = ProviderManager::new();

        tracing::info!("Initializing AI provider manager (API-only mode)...");

        // ASR Provider — mock until API-based ASR is integrated
        manager.register_asr("mock".to_string(), Box::new(MockASRProvider::new()));
        tracing::info!("Registered mock ASR provider (API ASR not yet integrated)");

        // LLM Provider — Claude API
        match Self::create_claude_provider(config).await {
            Ok(provider) => {
                manager.register_llm("claude".to_string(), Box::new(provider));
                tracing::info!("Registered Claude API LLM provider");
            }
            Err(e) => {
                tracing::warn!("Failed to create Claude provider: {}. Using mock.", e);
                manager.register_llm("mock".to_string(), Box::new(MockLLMProvider::new()));
            }
        }

        // TTS Provider — mock until API-based TTS is integrated
        manager.register_tts("mock".to_string(), Box::new(MockTTSProvider::new()));
        tracing::info!("Registered mock TTS provider (API TTS not yet integrated)");

        // Set active providers
        let llm_provider = if manager.llm_providers.contains_key("claude") {
            "claude"
        } else {
            "mock"
        };

        manager.set_active(
            "mock".to_string(),
            llm_provider.to_string(),
            Some("mock".to_string()),
        );

        let memory_mb = manager.total_memory_requirement_mb();
        tracing::info!(
            "Provider manager initialized. Total memory requirement: {}MB",
            memory_mb
        );

        Ok(manager)
    }

    /// Check API provider availability
    pub fn check_provider_availability() -> ProviderAvailability {
        ProviderAvailability {
            claude_api_key: std::env::var("ANTHROPIC_API_KEY").is_ok(),
        }
    }
}

/// Provider availability status
#[derive(Debug, Clone)]
pub struct ProviderAvailability {
    /// Whether ANTHROPIC_API_KEY environment variable is set
    pub claude_api_key: bool,
}

impl ProviderAvailability {
    /// Check if all required providers are available
    pub fn all_available(&self) -> bool {
        self.claude_api_key
    }

    /// List missing provider configurations
    pub fn missing_providers(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if !self.claude_api_key {
            missing.push("ANTHROPIC_API_KEY");
        }
        missing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_availability_check() {
        let availability = NativeProviderFactory::check_provider_availability();
        let _all = availability.all_available();
        let _missing = availability.missing_providers();
    }

    #[tokio::test]
    async fn test_provider_manager_creation() {
        let config = NativeProviderConfig::default();
        let manager = NativeProviderFactory::create_provider_manager(&config).await;
        assert!(manager.is_ok());
    }
}
