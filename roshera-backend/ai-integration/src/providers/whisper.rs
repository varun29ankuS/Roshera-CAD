/// Whisper ASR provider implementation
///
/// # Design Rationale
/// - **Why Whisper**: State-of-the-art accuracy, multilingual support
/// - **Why Base model**: Balance of accuracy (WER ~5%) and speed (~140MB)
/// - **Performance**: ~500ms for 5s audio on i7-1355U
/// - **Business Value**: Hands-free CAD commands, accessibility
///
/// # References
/// - [11] Radford et al. (2022) "Robust Speech Recognition via Large-Scale Weak Supervision"
use super::*;
use hound;
use std::sync::Arc;
use tokio::sync::Mutex;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Whisper ASR provider
#[derive(Debug)]
pub struct WhisperProvider {
    context: Arc<Mutex<Option<WhisperContext>>>,
    model_path: String,
    model_size: ModelSize,
    capabilities: ProviderCapabilities,
}

/// Whisper model sizes
#[derive(Debug, Clone, Copy)]
pub enum ModelSize {
    Tiny,   // 39MB
    Base,   // 142MB
    Small,  // 466MB
    Medium, // 1.5GB
}

impl ModelSize {
    fn memory_mb(&self) -> usize {
        match self {
            ModelSize::Tiny => 39,
            ModelSize::Base => 142,
            ModelSize::Small => 466,
            ModelSize::Medium => 1500,
        }
    }

    fn model_name(&self) -> &'static str {
        match self {
            ModelSize::Tiny => "tiny",
            ModelSize::Base => "base",
            ModelSize::Small => "small",
            ModelSize::Medium => "medium",
        }
    }
}

impl WhisperProvider {
    /// Create new Whisper provider
    ///
    /// # Example
    /// ```
    /// let provider = WhisperProvider::new("models/whisper-base.bin", ModelSize::Base)?;
    /// ```
    pub async fn new(model_path: String, model_size: ModelSize) -> Result<Self, ProviderError> {
        let capabilities = ProviderCapabilities {
            name: "Whisper".to_string(),
            version: format!("{}.en", model_size.model_name()),
            supported_languages: vec![
                "en".to_string(),
                "hi".to_string(),
                "es".to_string(),
                "fr".to_string(),
                "de".to_string(),
                "zh".to_string(),
            ],
            max_context_length: 30_000, // ~30s of audio
            supports_streaming: false,  // Whisper doesn't support streaming natively
            supports_batching: false,
            device_type: "cpu".to_string(),
            model_size_mb: model_size.memory_mb(),
            quantization: QuantizationType::Float16,
        };

        Ok(Self {
            context: Arc::new(Mutex::new(None)),
            model_path,
            model_size,
            capabilities,
        })
    }

    /// Load model if not already loaded
    async fn ensure_loaded(&self) -> Result<(), ProviderError> {
        let mut ctx = self.context.lock().await;
        if ctx.is_none() {
            tracing::info!("Loading Whisper model from {}", self.model_path);

            // Create context parameters
            let ctx_params = WhisperContextParameters::default();

            // Load the model
            let whisper_ctx = WhisperContext::new_with_params(&self.model_path, ctx_params)
                .map_err(|e| {
                    ProviderError::ModelLoadError(format!("Failed to load Whisper model: {}", e))
                })?;

            *ctx = Some(whisper_ctx);
            tracing::info!("Whisper model loaded successfully");
        }
        Ok(())
    }

    /// Convert audio to 16kHz mono f32 samples
    fn preprocess_audio(
        &self,
        audio: &[u8],
        format: AudioFormat,
    ) -> Result<Vec<f32>, ProviderError> {
        match format {
            AudioFormat::Wav => {
                let reader = hound::WavReader::new(std::io::Cursor::new(audio))
                    .map_err(|e| ProviderError::InvalidInput(format!("Invalid WAV: {}", e)))?;

                let spec = reader.spec();
                let samples: Vec<f32> = match spec.sample_format {
                    hound::SampleFormat::Float => reader
                        .into_samples::<f32>()
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|e| ProviderError::InvalidInput(e.to_string()))?,
                    hound::SampleFormat::Int => reader
                        .into_samples::<i16>()
                        .map(|s| s.map(|s| s as f32 / i16::MAX as f32))
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|e| ProviderError::InvalidInput(e.to_string()))?,
                };

                // Resample to 16kHz if needed
                if spec.sample_rate != 16000 {
                    // Simple downsampling - for production use a proper resampler
                    let ratio = spec.sample_rate as f32 / 16000.0;
                    let resampled: Vec<f32> = (0..((samples.len() as f32 / ratio) as usize))
                        .map(|i| samples[(i as f32 * ratio) as usize])
                        .collect();
                    Ok(resampled)
                } else {
                    Ok(samples)
                }
            }
            AudioFormat::Raw16kHz => {
                // Assume raw 16-bit PCM at 16kHz
                let samples: Vec<f32> = audio
                    .chunks_exact(2)
                    .map(|chunk| {
                        let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
                        sample as f32 / i16::MAX as f32
                    })
                    .collect();
                Ok(samples)
            }
            _ => Err(ProviderError::InvalidInput(format!(
                "Unsupported audio format: {:?}",
                format
            ))),
        }
    }
}

#[async_trait]
impl ASRProvider for WhisperProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities.clone()
    }

    async fn transcribe(&self, audio: &[u8], format: AudioFormat) -> Result<String, ProviderError> {
        self.ensure_loaded().await?;

        let samples = self.preprocess_audio(audio, format)?;

        let ctx_lock = self.context.lock().await;
        let ctx = ctx_lock.as_ref().unwrap();

        // Configure Whisper parameters
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some("en")); // Default to English, can be "hi" for Hindi
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(false); // Don't suppress blanks - we want all output
        params.set_token_timestamps(false);
        params.set_no_speech_thold(0.5); // Lower threshold for detecting speech
        params.set_max_len(0); // No max length limit

        // Create a state for the context
        let mut state = ctx
            .create_state()
            .map_err(|e| ProviderError::InferenceError(format!("Failed to create state: {}", e)))?;

        // Run inference
        let start = std::time::Instant::now();
        state.full(params, &samples).map_err(|e| {
            ProviderError::InferenceError(format!("Whisper inference failed: {}", e))
        })?;
        let duration = start.elapsed();

        tracing::debug!("Whisper inference took {:?}", duration);

        // Get transcription
        let num_segments = state
            .full_n_segments()
            .map_err(|e| ProviderError::InferenceError(format!("Failed to get segments: {}", e)))?;

        tracing::info!("Whisper found {} segments", num_segments);

        let mut text = String::new();

        for i in 0..num_segments {
            let segment = state.full_get_segment_text(i).map_err(|e| {
                ProviderError::InferenceError(format!("Failed to get segment text: {}", e))
            })?;
            if !segment.trim().is_empty() {
                tracing::info!("Segment {}: '{}'", i, segment);
                text.push_str(&segment);
                text.push(' ');
            }
        }

        let result = text.trim().to_string();
        if result.is_empty() {
            tracing::info!("No speech detected in audio");
        } else {
            tracing::info!("Final transcription: '{}'", result);
        }

        Ok(result)
    }

    fn supported_formats(&self) -> Vec<AudioFormat> {
        vec![AudioFormat::Wav, AudioFormat::Raw16kHz]
    }

    fn memory_requirement_mb(&self) -> usize {
        self.model_size.memory_mb() * 2 // 2x for runtime overhead
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_whisper_provider_creation() {
        let provider = WhisperProvider::new("models/whisper-base.bin".to_string(), ModelSize::Base)
            .await
            .unwrap();

        assert_eq!(provider.capabilities().name, "Whisper");
        assert_eq!(provider.memory_requirement_mb(), 284); // 142 * 2
    }

    #[test]
    fn test_model_size() {
        assert_eq!(ModelSize::Base.memory_mb(), 142);
        assert_eq!(ModelSize::Base.model_name(), "base");
    }
}
