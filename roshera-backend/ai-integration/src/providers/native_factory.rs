/// Native provider factory for pure Rust AI integration
///
/// # Design Rationale
/// - **Why factory pattern**: Clean initialization of complex providers
/// - **Why native-first**: No Python/system dependencies by default
/// - **Performance**: Faster startup, no subprocess overhead
/// - **Business Value**: Simpler deployment, better reliability
use super::*;
use std::path::Path;

/// Configuration for native AI providers
#[derive(Debug, Clone)]
pub struct NativeProviderConfig {
    /// Model directory containing all AI models
    pub model_dir: std::path::PathBuf,
    /// Enable CUDA if available
    pub use_cuda: bool,
    /// TTS configuration
    pub tts_config: NativeTTSConfig,
}

impl Default for NativeProviderConfig {
    fn default() -> Self {
        Self {
            model_dir: std::path::PathBuf::from("models"),
            use_cuda: true,
            tts_config: NativeTTSConfig::default(),
        }
    }
}

/// Factory for creating native AI providers
pub struct NativeProviderFactory;

impl NativeProviderFactory {
    /// Create a native Whisper ASR provider
    pub async fn create_whisper_provider(
        config: &NativeProviderConfig,
    ) -> Result<WhisperProvider, ProviderError> {
        tracing::info!("Creating Whisper Base provider");
        let model_path = config.model_dir.join("whisper/base.bin");
        WhisperProvider::new(
            model_path.to_string_lossy().to_string(),
            super::whisper::ModelSize::Base,
        )
        .await
    }

    /// Create a LLaMA 8-bit quantized provider
    pub async fn create_llama_provider(
        config: &NativeProviderConfig,
    ) -> Result<LlamaCandleProvider, ProviderError> {
        tracing::info!("Creating LLaMA 8-bit quantized provider");
        let model_path = config.model_dir.join("llama/3b-q8.gguf");
        let tokenizer_path = config.model_dir.join("llama/tokenizer.json");
        LlamaCandleProvider::new(
            model_path.to_string_lossy().to_string(),
            tokenizer_path.to_string_lossy().to_string(),
        )
        .await
    }

    /// Create a native TTS provider
    pub async fn create_tts_provider(
        config: &NativeProviderConfig,
    ) -> Result<NativeTTSProvider, ProviderError> {
        tracing::info!("Creating native TTS provider");
        NativeTTSProvider::new(config.tts_config.clone()).await
    }

    /// Create a complete native provider manager
    pub async fn create_provider_manager(
        config: &NativeProviderConfig,
    ) -> Result<ProviderManager, ProviderError> {
        let mut manager = ProviderManager::new();

        // Try to create native providers, fall back to mocks if models are missing
        tracing::info!("Initializing native AI provider manager...");

        // ASR Provider
        match Self::create_whisper_provider(config).await {
            Ok(provider) => {
                manager.register_asr("whisper-native".to_string(), Box::new(provider));
                tracing::info!("✓ Native Whisper ASR provider registered");
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to create native Whisper provider: {}. Using mock.",
                    e
                );
                manager.register_asr("mock".to_string(), Box::new(MockASRProvider::new()));
            }
        }

        // LLM Provider
        match Self::create_llama_provider(config).await {
            Ok(provider) => {
                manager.register_llm("llama-native".to_string(), Box::new(provider));
                tracing::info!("✓ Native LLaMA LLM provider registered");
            }
            Err(e) => {
                tracing::warn!("Failed to create native LLaMA provider: {}. Using mock.", e);
                manager.register_llm("mock".to_string(), Box::new(MockLLMProvider::new()));
            }
        }

        // TTS Provider
        match Self::create_tts_provider(config).await {
            Ok(provider) => {
                manager.register_tts("native".to_string(), Box::new(provider));
                tracing::info!("✓ Native TTS provider registered");
            }
            Err(e) => {
                tracing::warn!("Failed to create native TTS provider: {}. Using mock.", e);
                manager.register_tts("mock".to_string(), Box::new(MockTTSProvider::new()));
            }
        }

        // Set active providers
        let asr_provider = if manager.asr_providers.contains_key("whisper-native") {
            "whisper-native"
        } else {
            "mock"
        };

        let llm_provider = if manager.llm_providers.contains_key("llama-native") {
            "llama-native"
        } else {
            "mock"
        };

        let tts_provider = if manager.tts_providers.contains_key("native") {
            Some("native".to_string())
        } else {
            Some("mock".to_string())
        };

        manager.set_active(
            asr_provider.to_string(),
            llm_provider.to_string(),
            tts_provider,
        );

        let memory_mb = manager.total_memory_requirement_mb();
        tracing::info!(
            "Provider manager initialized. Total memory requirement: {}MB",
            memory_mb
        );

        Ok(manager)
    }

    /// Check if all required model files are present
    pub fn check_model_availability(config: &NativeProviderConfig) -> ModelAvailability {
        // For stub implementations, models are not required
        // This will be updated when real model loading is implemented
        ModelAvailability {
            whisper_model: true, // Stub doesn't need real models
            whisper_tokenizer: true,
            llama_model: true,
            model_dir_exists: config.model_dir.exists(),
        }
    }

    /// Download missing models (placeholder for future implementation)
    pub async fn download_missing_models(
        config: &NativeProviderConfig,
        availability: &ModelAvailability,
    ) -> Result<(), ProviderError> {
        if !availability.model_dir_exists {
            std::fs::create_dir_all(&config.model_dir).map_err(|e| {
                ProviderError::ConfigError(format!("Failed to create model directory: {}", e))
            })?;
        }

        tracing::info!("Model download not implemented. Please run the download scripts manually:");

        if !availability.whisper_model || !availability.whisper_tokenizer {
            tracing::info!("  - For Whisper: python download_whisper_models.py");
        }

        if !availability.llama_model {
            tracing::info!("  - For LLaMA: python download_llama_models.py");
        }

        Ok(())
    }
}

/// Model availability status
#[derive(Debug, Clone)]
pub struct ModelAvailability {
    pub whisper_model: bool,
    pub whisper_tokenizer: bool,
    pub llama_model: bool,
    pub model_dir_exists: bool,
}

impl ModelAvailability {
    pub fn all_available(&self) -> bool {
        self.whisper_model && self.whisper_tokenizer && self.llama_model
    }

    pub fn any_available(&self) -> bool {
        self.whisper_model || self.llama_model
    }

    pub fn missing_models(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();

        if !self.whisper_model {
            missing.push("whisper-model");
        }
        if !self.whisper_tokenizer {
            missing.push("whisper-tokenizer");
        }
        if !self.llama_model {
            missing.push("llama-model");
        }

        missing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_availability_check() {
        let config = NativeProviderConfig::default();
        let availability = NativeProviderFactory::check_model_availability(&config);

        // Should not panic
        let _all = availability.all_available();
        let _any = availability.any_available();
        let _missing = availability.missing_models();
    }

    #[tokio::test]
    async fn test_provider_manager_creation() {
        let config = NativeProviderConfig::default();

        // Should succeed even without models (will use mocks)
        let manager = NativeProviderFactory::create_provider_manager(&config).await;
        assert!(manager.is_ok());
    }
}
