/// Ganga LLM provider for Hindi + English understanding
///
/// # Design Rationale
/// - **Why Ganga**: Indian language model, understands Hindi context better
/// - **Why alongside LLaMA**: Ganga for Hindi, LLaMA for technical English
/// - **Performance**: Similar to LLaMA 3B on CPU
/// - **Business Value**: Better Hindi understanding for Indian users
///
/// References:
/// - [1] AI4Bharat Ganga models: https://ai4bharat.org/ganga
use super::{
    CommandIntent, ConversationContext, LLMProvider, ParsedCommand, ProviderCapabilities,
    ProviderError, QuantizationType,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Configuration for Ganga model
#[derive(Debug, Clone)]
pub struct GangaConfig {
    /// Path to model file
    pub model_path: PathBuf,
    /// Model variant (1b, 3b, etc)
    pub model_size: String,
    /// Device to run on
    pub device: String,
    /// Number of threads
    pub num_threads: usize,
    /// Maximum context length
    pub max_context_length: usize,
}

/// Ganga LLM provider
#[derive(Debug)]
pub struct GangaLLMProvider {
    config: GangaConfig,
    // Model would be loaded here
}

impl GangaLLMProvider {
    /// Create new Ganga provider
    pub fn new(config: GangaConfig) -> Result<Self, ProviderError> {
        // TODO: Load actual model when available
        Ok(Self { config })
    }

    /// Parse Hindi/English mixed commands
    fn parse_command(&self, text: &str) -> ParsedCommand {
        let lower = text.to_lowercase();

        // Hindi command patterns
        let _hindi_patterns = [
            ("बनाओ", "create"),
            ("गोला", "sphere"),
            ("वृत्त", "circle"),
            ("चौकोर", "box"),
            ("सिलेंडर", "cylinder"),
            ("हटाओ", "delete"),
            ("घुमाओ", "rotate"),
            ("बढ़ाओ", "scale"),
            ("छोटा", "smaller"),
            ("बड़ा", "larger"),
        ];

        // Detect intent
        let mut intent = CommandIntent::Unknown;
        let mut params = HashMap::new();

        // Check for create commands
        if lower.contains("create") || lower.contains("बनाओ") || lower.contains("बना")
        {
            if lower.contains("sphere") || lower.contains("गोला") {
                intent = CommandIntent::CreatePrimitive {
                    shape: "sphere".to_string(),
                };

                // Extract radius if mentioned
                if let Some(radius) = self.extract_number(&lower, &["radius", "त्रिज्या"])
                {
                    params.insert("radius".to_string(), serde_json::Value::from(radius));
                }
            } else if lower.contains("box") || lower.contains("चौकोर") || lower.contains("डिब्बा")
            {
                intent = CommandIntent::CreatePrimitive {
                    shape: "box".to_string(),
                };
            } else if lower.contains("cylinder") || lower.contains("सिलेंडर") {
                intent = CommandIntent::CreatePrimitive {
                    shape: "cylinder".to_string(),
                };
            }
        }
        // Check for transform commands
        else if lower.contains("rotate") || lower.contains("घुमाओ") {
            intent = CommandIntent::Transform {
                operation: "rotate".to_string(),
            };
        }

        ParsedCommand {
            original_text: text.to_string(),
            intent,
            parameters: params,
            confidence: 0.8,
            language: self.detect_primary_language(text),
        }
    }

    /// Extract numbers from text (handles Hindi numerals too)
    fn extract_number(&self, text: &str, keywords: &[&str]) -> Option<f64> {
        // Hindi numerals
        let hindi_numbers = [
            ("एक", 1.0),
            ("दो", 2.0),
            ("तीन", 3.0),
            ("चार", 4.0),
            ("पांच", 5.0),
            ("छह", 6.0),
            ("सात", 7.0),
            ("आठ", 8.0),
            ("नौ", 9.0),
            ("दस", 10.0),
        ];

        // Look for number after keyword
        for keyword in keywords {
            if let Some(pos) = text.find(keyword) {
                let after_keyword = &text[pos + keyword.len()..];

                // Try parsing regular number
                if let Some(num) = after_keyword
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse::<f64>().ok())
                {
                    return Some(num);
                }

                // Try Hindi numbers
                for (hindi, val) in &hindi_numbers {
                    if after_keyword.contains(hindi) {
                        return Some(*val);
                    }
                }
            }
        }

        None
    }

    /// Detect primary language
    fn detect_primary_language(&self, text: &str) -> String {
        let devanagari_count = text
            .chars()
            .filter(|c| matches!(*c as u32, 0x0900..=0x097F))
            .count();

        let total_alpha = text.chars().filter(|c| c.is_alphabetic()).count();

        if total_alpha > 0 && devanagari_count as f32 / total_alpha as f32 > 0.3 {
            "hi".to_string()
        } else {
            "en".to_string()
        }
    }
}

#[async_trait]
impl LLMProvider for GangaLLMProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            name: format!("Ganga {}", self.config.model_size),
            version: "1.0.0".to_string(),
            supported_languages: vec!["hi".to_string(), "en".to_string()],
            max_context_length: self.config.max_context_length,
            supports_streaming: true,
            supports_batching: false,
            device_type: self.config.device.clone(),
            model_size_mb: self.memory_requirement_mb(),
            quantization: QuantizationType::Float16, // Ganga uses FP16
        }
    }

    async fn process(
        &self,
        input: &str,
        context: Option<&ConversationContext>,
    ) -> Result<ParsedCommand, ProviderError> {
        // Log context for debugging
        if let Some(ctx) = context {
            tracing::debug!(
                "Processing with {} previous commands",
                ctx.previous_commands.len()
            );
        }

        // For now, use rule-based parsing
        // TODO: Use actual Ganga model when available
        Ok(self.parse_command(input))
    }

    async fn generate(&self, _prompt: &str, _max_tokens: usize) -> Result<String, ProviderError> {
        // TODO: Implement actual generation when model is available
        Ok("Ganga generation stub".to_string())
    }

    async fn generate_response(
        &self,
        command_result: &str,
        language: &str,
    ) -> Result<String, ProviderError> {
        // Generate response in requested language
        let response = match language {
            "hi" => match command_result {
                r if r.contains("Created") => "आकार सफलतापूर्वक बनाया गया",
                r if r.contains("Error") => "त्रुटि हुई है",
                _ => "कार्य पूर्ण हुआ",
            },
            _ => command_result,
        };

        Ok(response.to_string())
    }

    fn memory_requirement_mb(&self) -> usize {
        match self.config.model_size.as_str() {
            "1b" => 2500, // ~2.5GB for 1B model
            "3b" => 7500, // ~7.5GB for 3B model
            _ => 5000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_hindi_parsing() {
        let config = GangaConfig {
            model_path: PathBuf::from("models/ganga/1b.bin"),
            model_size: "1b".to_string(),
            device: "cpu".to_string(),
            num_threads: 4,
            max_context_length: 2048,
        };

        let provider = GangaLLMProvider::new(config).unwrap();

        // Test Hindi commands
        let tests = vec![
            ("एक गोला बनाओ", "sphere"),
            ("5 त्रिज्या का गोला बनाओ", "sphere"),
            ("चौकोर बनाओ", "box"),
            ("सिलेंडर बनाओ", "cylinder"),
        ];

        for (input, expected_shape) in tests {
            let result = provider.process(input, None).await.unwrap();

            match result.intent {
                CommandIntent::CreatePrimitive { shape } => {
                    assert_eq!(shape, expected_shape, "Failed for: {}", input);
                }
                _ => panic!("Wrong intent for: {}", input),
            }
        }
    }

    #[test]
    fn test_number_extraction() {
        let config = GangaConfig {
            model_path: PathBuf::from("models/ganga/1b.bin"),
            model_size: "1b".to_string(),
            device: "cpu".to_string(),
            num_threads: 4,
            max_context_length: 2048,
        };

        let provider = GangaLLMProvider::new(config).unwrap();

        // Test number extraction
        assert_eq!(provider.extract_number("radius 5", &["radius"]), Some(5.0));

        assert_eq!(
            provider.extract_number("त्रिज्या पांच", &["त्रिज्या"]),
            Some(5.0)
        );
    }
}
