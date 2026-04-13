/// Mock providers for testing without loading actual models
///
/// # Design Rationale
/// - **Why mock providers**: Enable fast unit tests without 8GB models
/// - **Why deterministic**: Reproducible tests across environments
/// - **Performance**: < 1ms response time for CI/CD pipelines
/// - **Business Value**: Faster development cycles, reliable tests
use super::*;
use std::collections::HashMap;

/// Mock ASR provider for testing
#[derive(Debug)]
pub struct MockASRProvider {
    responses: HashMap<String, String>,
    capabilities: ProviderCapabilities,
}

impl MockASRProvider {
    pub fn new() -> Self {
        let mut responses = HashMap::new();
        responses.insert(
            "create_sphere".to_string(),
            "create a sphere with radius 5".to_string(),
        );
        responses.insert(
            "boolean_union".to_string(),
            "unite the two objects".to_string(),
        );

        let capabilities = ProviderCapabilities {
            name: "MockASR".to_string(),
            version: "1.0.0".to_string(),
            supported_languages: vec!["en".to_string()],
            max_context_length: 1000,
            supports_streaming: false,
            supports_batching: false,
            device_type: "cpu".to_string(),
            model_size_mb: 0,
            quantization: QuantizationType::Float32,
        };

        Self {
            responses,
            capabilities,
        }
    }
}

#[async_trait]
impl ASRProvider for MockASRProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities.clone()
    }

    async fn transcribe(
        &self,
        audio: &[u8],
        _format: AudioFormat,
    ) -> Result<String, ProviderError> {
        // Simulate processing delay
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Return predetermined response based on audio length
        let response = if audio.len() < 1000 {
            "create a sphere with radius 5"
        } else {
            "unite the two objects"
        };

        Ok(response.to_string())
    }

    fn supported_formats(&self) -> Vec<AudioFormat> {
        vec![AudioFormat::Wav, AudioFormat::Raw16kHz]
    }

    fn memory_requirement_mb(&self) -> usize {
        1
    }
}

/// Mock LLM provider for testing
#[derive(Debug)]
pub struct MockLLMProvider {
    responses: HashMap<String, ParsedCommand>,
    capabilities: ProviderCapabilities,
}

impl MockLLMProvider {
    pub fn new() -> Self {
        let mut responses = HashMap::new();

        responses.insert(
            "create a sphere with radius 5".to_string(),
            ParsedCommand {
                original_text: "create a sphere with radius 5".to_string(),
                intent: CommandIntent::CreatePrimitive {
                    shape: "sphere".to_string(),
                },
                parameters: {
                    let mut params = std::collections::HashMap::new();
                    params.insert("radius".to_string(), serde_json::json!(5.0));
                    params
                },
                confidence: 0.95,
                language: "en".to_string(),
            },
        );

        responses.insert(
            "unite the two objects".to_string(),
            ParsedCommand {
                original_text: "unite the two objects".to_string(),
                intent: CommandIntent::BooleanOperation {
                    operation: "union".to_string(),
                },
                parameters: {
                    let mut params = std::collections::HashMap::new();
                    params.insert("object_a".to_string(), serde_json::json!("obj_1"));
                    params.insert("object_b".to_string(), serde_json::json!("obj_2"));
                    params
                },
                confidence: 0.92,
                language: "en".to_string(),
            },
        );

        let capabilities = ProviderCapabilities {
            name: "MockLLM".to_string(),
            version: "1.0.0".to_string(),
            supported_languages: vec!["en".to_string()],
            max_context_length: 1000,
            supports_streaming: false,
            supports_batching: false,
            device_type: "cpu".to_string(),
            model_size_mb: 0,
            quantization: QuantizationType::Float32,
        };

        Self {
            responses,
            capabilities,
        }
    }
}

#[async_trait]
impl LLMProvider for MockLLMProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities.clone()
    }

    async fn process(
        &self,
        input: &str,
        _context: Option<&ConversationContext>,
    ) -> Result<ParsedCommand, ProviderError> {
        // Simulate processing delay
        tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;

        // Return predetermined response or generic unknown
        Ok(self.responses.get(input).cloned().unwrap_or(ParsedCommand {
            original_text: input.to_string(),
            intent: CommandIntent::Unknown,
            parameters: std::collections::HashMap::new(),
            confidence: 0.1,
            language: "en".to_string(),
        }))
    }

    async fn generate(&self, _prompt: &str, _max_tokens: usize) -> Result<String, ProviderError> {
        Ok("Mock generated response".to_string())
    }

    async fn generate_response(
        &self,
        _command_result: &str,
        _language: &str,
    ) -> Result<String, ProviderError> {
        Ok("Command executed successfully".to_string())
    }

    fn memory_requirement_mb(&self) -> usize {
        1
    }
}

/// Mock TTS provider for testing
#[derive(Debug)]
pub struct MockTTSProvider {
    capabilities: ProviderCapabilities,
}

impl MockTTSProvider {
    pub fn new() -> Self {
        let capabilities = ProviderCapabilities {
            name: "MockTTS".to_string(),
            version: "1.0.0".to_string(),
            supported_languages: vec!["en".to_string()],
            max_context_length: 1000,
            supports_streaming: false,
            supports_batching: false,
            device_type: "cpu".to_string(),
            model_size_mb: 0,
            quantization: QuantizationType::Float32,
        };

        Self { capabilities }
    }
}

#[async_trait]
impl TTSProvider for MockTTSProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities.clone()
    }

    async fn synthesize(&self, text: &str, _voice: Option<&str>) -> Result<Vec<u8>, ProviderError> {
        // Return dummy audio data
        Ok(vec![0u8; text.len() * 100])
    }

    fn available_voices(&self) -> Vec<VoiceInfo> {
        vec![VoiceInfo {
            id: "voice1".to_string(),
            name: "Mock Voice 1".to_string(),
            language: "en".to_string(),
            gender: Some("neutral".to_string()),
            sample_rate: 22050,
        }]
    }

    fn estimated_latency_ms(&self) -> u32 {
        5 // Mock is fast
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_asr() {
        let provider = MockASRProvider::new();
        let result = provider
            .transcribe(&[0u8; 100], AudioFormat::Wav)
            .await
            .unwrap();
        assert_eq!(result, "create a sphere with radius 5");
    }

    #[tokio::test]
    async fn test_mock_llm() {
        let provider = MockLLMProvider::new();
        let result = provider
            .process("create a sphere with radius 5", None)
            .await
            .unwrap();
        assert!(matches!(
            result.intent,
            CommandIntent::CreatePrimitive { .. }
        ));
        assert_eq!(result.confidence, 0.95);
    }

    #[tokio::test]
    async fn test_mock_tts() {
        let provider = MockTTSProvider::new();
        let result = provider.synthesize("Hello world", None).await.unwrap();
        assert_eq!(result.len(), 1100); // "Hello world" = 11 chars * 100
    }
}
