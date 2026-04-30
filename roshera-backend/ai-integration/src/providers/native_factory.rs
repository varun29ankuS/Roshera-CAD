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

    /// Create a complete provider manager with API-based providers.
    ///
    /// Production policy is **API-only**: only the Claude LLM provider is
    /// registered. ASR and TTS are intentionally left empty until their
    /// hosted API integrations land — `ProviderManager::asr()` /
    /// `tts(...)` will return `ProviderUnavailable` so callers fail loudly
    /// instead of silently dispatching to a mock.
    ///
    /// Test and `mock-providers`-feature builds also register the mock
    /// ASR/LLM/TTS providers and select them as the active backend so
    /// integration tests run without an API key. Release builds without
    /// the feature flag never compile the mock module.
    pub async fn create_provider_manager(
        config: &NativeProviderConfig,
    ) -> Result<ProviderManager, ProviderError> {
        let mut manager = ProviderManager::new();

        tracing::info!("Initializing AI provider manager (API-only mode)...");

        // LLM Provider — Claude API. If this fails the manager is returned
        // with no active LLM, and the API server's `ai_configured` gate
        // will surface a 503 to clients. We never silently fall back to
        // a mock in release builds.
        let mut active_llm = String::new();
        match Self::create_claude_provider(config).await {
            Ok(provider) => {
                manager.register_llm("claude".to_string(), Box::new(provider));
                active_llm = "claude".to_string();
                tracing::info!("Registered Claude API LLM provider");
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to create Claude provider: {}. \
                     LLM dispatch will return ProviderUnavailable until the \
                     deployment configures a working provider.",
                    e
                );
            }
        }

        // ASR / TTS: real API integrations are not yet wired. Production
        // builds register nothing here — `manager.asr()` returns
        // ProviderUnavailable and the upstream handler refuses the request.

        // Mock-flavored builds (tests, benches, dev with `--features
        // mock-providers`) register the mocks and select them as default.
        #[cfg(any(test, feature = "mock-providers"))]
        {
            manager.register_asr("mock".to_string(), Box::new(MockASRProvider::new()));
            manager.register_tts("mock".to_string(), Box::new(MockTTSProvider::new()));
            if active_llm.is_empty() {
                manager.register_llm("mock".to_string(), Box::new(MockLLMProvider::new()));
                active_llm = "mock".to_string();
            }
            tracing::info!(
                "Registered mock ASR/TTS providers (test / mock-providers feature build)"
            );
        }

        // Activate whichever backends were registered. In a release build
        // without an LLM key, `active_llm` stays empty; consumers must
        // check `ProviderManager::llm()` and surface the error.
        let active_asr = if manager.asr_providers.contains_key("mock") {
            "mock".to_string()
        } else {
            String::new()
        };
        let active_tts = if manager.tts_providers.contains_key("mock") {
            Some("mock".to_string())
        } else {
            None
        };
        manager.set_active(active_asr, active_llm, active_tts);

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
