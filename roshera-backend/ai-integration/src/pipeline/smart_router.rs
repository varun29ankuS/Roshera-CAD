/// Smart Router for Vision Pipeline
///
/// This module implements the intelligent routing system that controls how
/// vision and reasoning are processed. It supports two modes:
/// 1. Unified - Single multimodal model handles both vision and reasoning
/// 2. Separated - Dedicated vision model + dedicated reasoning model
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, error, info, warn};

use crate::providers::{
    LLMProvider, ParsedCommand, ProviderError,
    UniversalEndpoint, UniversalEndpointConfig,
};
use shared_types::vision::{ProcessingMode, ViewportCapture, VisionConfig, VisionProviderType};

/// Smart router errors
#[derive(Error, Debug)]
pub enum SmartRouterError {
    #[error("Provider error: {0}")]
    ProviderError(#[from] ProviderError),
    
    #[error("Configuration error: {0}")]
    ConfigError(String),
    
    #[error("Vision processing failed: {0}")]
    VisionError(String),
    
    #[error("Reasoning failed: {0}")]
    ReasoningError(String),
    
    #[error("Invalid processing mode")]
    InvalidMode,
    
    #[error("No viewport data provided for vision command")]
    MissingViewport,
}

/// Configuration for the smart router
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartRouterConfig {
    /// Processing mode (Unified or Separated)
    pub mode: ProcessingMode,
    
    /// Vision model configuration
    pub vision_config: VisionConfig,
    
    /// Reasoning model configuration (None = use same as vision)
    pub reasoning_config: Option<VisionConfig>,
    
    /// Enable caching of vision analysis
    pub enable_cache: bool,
    
    /// Cache TTL in seconds
    pub cache_ttl_secs: u64,
    
    /// Maximum retries on failure
    pub max_retries: u32,
    
    /// Timeout for vision processing (seconds)
    pub vision_timeout_secs: u64,
    
    /// Timeout for reasoning (seconds)
    pub reasoning_timeout_secs: u64,
}

impl Default for SmartRouterConfig {
    fn default() -> Self {
        Self {
            mode: ProcessingMode::Unified,
            vision_config: VisionConfig {
                provider: VisionProviderType::Anthropic,
                url: "https://api.anthropic.com/v1/messages".to_string(),
                api_key: None,
                model_name: "claude-3-5-sonnet-20241022".to_string(),
            },
            reasoning_config: None,
            enable_cache: true,
            cache_ttl_secs: 300, // 5 minutes
            max_retries: 3,
            vision_timeout_secs: 30,
            reasoning_timeout_secs: 20,
        }
    }
}

/// Smart router that orchestrates vision and reasoning
pub struct SmartRouter {
    config: SmartRouterConfig,
    vision_endpoint: Arc<UniversalEndpoint>,
    reasoning_endpoint: Option<Arc<UniversalEndpoint>>,
    cache: Option<VisionCache>,
}

impl std::fmt::Debug for SmartRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmartRouter")
            .field("config", &self.config)
            .field("vision_endpoint", &"UniversalEndpoint")
            .field("reasoning_endpoint", &self.reasoning_endpoint.is_some())
            .field("cache_enabled", &self.cache.is_some())
            .finish()
    }
}

impl SmartRouter {
    /// Create new smart router
    pub fn new(config: SmartRouterConfig) -> Result<Self, SmartRouterError> {
        info!("Initializing SmartRouter with mode: {:?}", config.mode);
        
        // Create vision endpoint
        let vision_endpoint = Arc::new(UniversalEndpoint::new(
            Self::vision_to_endpoint_config(&config.vision_config, config.vision_timeout_secs)
        ));
        
        // Create reasoning endpoint if in separated mode
        let reasoning_endpoint = match config.mode {
            ProcessingMode::Unified => None,
            ProcessingMode::Separated => {
                let reasoning_config = config.reasoning_config.as_ref()
                    .ok_or_else(|| SmartRouterError::ConfigError(
                        "Separated mode requires reasoning_config".to_string()
                    ))?;
                
                Some(Arc::new(UniversalEndpoint::new(
                    Self::vision_to_endpoint_config(reasoning_config, config.reasoning_timeout_secs)
                )))
            }
        };
        
        // Create cache if enabled
        let cache = if config.enable_cache {
            Some(VisionCache::new(config.cache_ttl_secs))
        } else {
            None
        };
        
        Ok(Self {
            config,
            vision_endpoint,
            reasoning_endpoint,
            cache,
        })
    }
    
    /// Process command with vision context
    pub async fn process_with_vision(
        &self,
        text: &str,
        viewport: &ViewportCapture,
    ) -> Result<ParsedCommand, SmartRouterError> {
        info!("Processing command with vision: '{}'", text);
        
        // Check cache if enabled
        if let Some(cache) = &self.cache {
            if let Some(cached) = cache.get(viewport, text) {
                info!("Using cached vision analysis");
                return Ok(cached);
            }
        }
        
        // Process based on mode
        let result = match self.config.mode {
            ProcessingMode::Unified => {
                self.process_unified(text, viewport).await?
            }
            ProcessingMode::Separated => {
                self.process_separated(text, viewport).await?
            }
        };
        
        // Cache result if enabled
        if let Some(cache) = &self.cache {
            cache.put(viewport, text, result.clone());
        }
        
        Ok(result)
    }
    
    /// Process in unified mode (single model)
    async fn process_unified(
        &self,
        text: &str,
        viewport: &ViewportCapture,
    ) -> Result<ParsedCommand, SmartRouterError> {
        debug!("Processing in unified mode with single model");
        
        // Single call to multimodal model
        self.vision_endpoint
            .process_with_vision(text, Some(viewport))
            .await
            .map_err(|e| SmartRouterError::VisionError(e.to_string()))
    }
    
    /// Process in separated mode (vision + reasoning)
    async fn process_separated(
        &self,
        text: &str,
        viewport: &ViewportCapture,
    ) -> Result<ParsedCommand, SmartRouterError> {
        debug!("Processing in separated mode with two models");
        
        let reasoning_endpoint = self.reasoning_endpoint.as_ref()
            .ok_or_else(|| SmartRouterError::ConfigError(
                "Reasoning endpoint not configured".to_string()
            ))?;
        
        // Step 1: Vision analysis
        info!("Step 1: Analyzing viewport with vision model");
        let vision_prompt = format!(
            "Describe what you see in this 3D CAD viewport. \
             Focus on: objects, their positions, what's selected, what the cursor points at."
        );
        
        let vision_analysis = self.vision_endpoint
            .process_with_vision(&vision_prompt, Some(viewport))
            .await
            .map_err(|e| SmartRouterError::VisionError(e.to_string()))?;
        
        // Step 2: Reasoning with vision context
        info!("Step 2: Reasoning with command and vision context");
        let reasoning_prompt = format!(
            "Vision analysis of the 3D viewport:\n{}\n\n\
             User command: {}\n\n\
             Parse this into a CAD operation command.",
            vision_analysis.original_text,
            text
        );
        
        reasoning_endpoint
            .process_with_vision(&reasoning_prompt, None)
            .await
            .map_err(|e| SmartRouterError::ReasoningError(e.to_string()))
    }
    
    /// Process text-only command (no vision)
    pub async fn process_text_only(&self, text: &str) -> Result<ParsedCommand, SmartRouterError> {
        info!("Processing text-only command: '{}'", text);
        
        // Use reasoning endpoint if available, otherwise vision endpoint
        let endpoint = self.reasoning_endpoint.as_ref()
            .unwrap_or(&self.vision_endpoint);
        
        endpoint
            .process_with_vision(text, None)
            .await
            .map_err(|e| SmartRouterError::ProviderError(
                ProviderError::ProcessingError(e.to_string())
            ))
    }
    
    /// Determine if a command requires vision
    pub fn requires_vision(text: &str) -> bool {
        let vision_keywords = [
            "this", "that", "these", "those",
            "here", "there", "where",
            "selected", "highlighted", "pointed",
            "cursor", "mouse", "pointing",
            "red", "blue", "green", "colored",
            "front", "back", "left", "right",
            "above", "below", "near", "far",
        ];

        let lower = text.to_lowercase();
        vision_keywords.iter().any(|keyword| {
            // Word-boundary match to avoid substring false positives (e.g. "sphere" matching "here")
            for (i, _) in lower.match_indices(keyword) {
                let before_ok = i == 0 || !lower.as_bytes()[i - 1].is_ascii_alphabetic();
                let after = i + keyword.len();
                let after_ok = after >= lower.len() || !lower.as_bytes()[after].is_ascii_alphabetic();
                if before_ok && after_ok {
                    return true;
                }
            }
            false
        })
    }
    
    /// Convert VisionConfig to UniversalEndpointConfig
    fn vision_to_endpoint_config(
        vision: &VisionConfig,
        timeout_secs: u64,
    ) -> UniversalEndpointConfig {
        UniversalEndpointConfig {
            provider: vision.provider.clone(),
            url: vision.url.clone(),
            api_key: vision.api_key.clone(),
            model_name: vision.model_name.clone(),
            timeout_secs,
            max_tokens: 1000,
            temperature: 0.7,
            system_prompt: Some(
                "You are an AI assistant for a CAD system. \
                 You can see the 3D viewport and understand spatial relationships. \
                 Parse user commands into structured CAD operations.".to_string()
            ),
        }
    }
    
    /// Get current configuration
    pub fn config(&self) -> &SmartRouterConfig {
        &self.config
    }
    
    /// Update configuration (creates new endpoints)
    pub fn update_config(&mut self, config: SmartRouterConfig) -> Result<(), SmartRouterError> {
        info!("Updating SmartRouter configuration");
        
        // Create new vision endpoint
        self.vision_endpoint = Arc::new(UniversalEndpoint::new(
            Self::vision_to_endpoint_config(&config.vision_config, config.vision_timeout_secs)
        ));
        
        // Update reasoning endpoint
        self.reasoning_endpoint = match config.mode {
            ProcessingMode::Unified => None,
            ProcessingMode::Separated => {
                let reasoning_config = config.reasoning_config.as_ref()
                    .ok_or_else(|| SmartRouterError::ConfigError(
                        "Separated mode requires reasoning_config".to_string()
                    ))?;
                
                Some(Arc::new(UniversalEndpoint::new(
                    Self::vision_to_endpoint_config(reasoning_config, config.reasoning_timeout_secs)
                )))
            }
        };
        
        // Update cache settings
        if config.enable_cache && self.cache.is_none() {
            self.cache = Some(VisionCache::new(config.cache_ttl_secs));
        } else if !config.enable_cache {
            self.cache = None;
        }
        
        self.config = config;
        Ok(())
    }
}

/// Simple cache for vision analysis results
struct VisionCache {
    cache: Arc<dashmap::DashMap<String, (ParsedCommand, std::time::Instant)>>,
    ttl_secs: u64,
}

impl VisionCache {
    fn new(ttl_secs: u64) -> Self {
        Self {
            cache: Arc::new(dashmap::DashMap::new()),
            ttl_secs,
        }
    }
    
    fn get(&self, viewport: &ViewportCapture, text: &str) -> Option<ParsedCommand> {
        let key = Self::make_key(viewport, text);
        
        self.cache.get(&key).and_then(|entry| {
            let (cmd, timestamp) = entry.value();
            
            // Check if cache entry is still valid
            if timestamp.elapsed().as_secs() < self.ttl_secs {
                Some(cmd.clone())
            } else {
                // Remove expired entry
                drop(entry);
                self.cache.remove(&key);
                None
            }
        })
    }
    
    fn put(&self, viewport: &ViewportCapture, text: &str, command: ParsedCommand) {
        let key = Self::make_key(viewport, text);
        self.cache.insert(key, (command, std::time::Instant::now()));
    }
    
    fn make_key(viewport: &ViewportCapture, text: &str) -> String {
        // Simple key based on camera position, selection, and text
        // In production, might use a hash
        format!(
            "{:.2},{:.2},{:.2}|{}|{}",
            viewport.camera.position[0],
            viewport.camera.position[1],
            viewport.camera.position[2],
            viewport.selection.object_ids.join(","),
            text
        )
    }
}

/// Adapter to use SmartRouter as an LLMProvider
#[async_trait]
impl LLMProvider for SmartRouter {
    fn capabilities(&self) -> crate::providers::ProviderCapabilities {
        self.vision_endpoint.capabilities()
    }
    
    async fn process(
        &self,
        input: &str,
        _context: Option<&crate::providers::ConversationContext>,
    ) -> Result<ParsedCommand, ProviderError> {
        self.process_text_only(input)
            .await
            .map_err(|e| ProviderError::ProcessingError(e.to_string()))
    }
    
    async fn generate(&self, prompt: &str, max_tokens: usize) -> Result<String, ProviderError> {
        self.vision_endpoint.generate(prompt, max_tokens).await
    }
    
    async fn generate_response(
        &self,
        command_result: &str,
        language: &str,
    ) -> Result<String, ProviderError> {
        self.vision_endpoint.generate_response(command_result, language).await
    }
    
    fn memory_requirement_mb(&self) -> usize {
        0 // API-based
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_requires_vision() {
        assert!(SmartRouter::requires_vision("select this object"));
        assert!(SmartRouter::requires_vision("make that edge rounder"));
        assert!(SmartRouter::requires_vision("the red cylinder"));
        assert!(SmartRouter::requires_vision("what the cursor is pointing at"));
        
        assert!(!SmartRouter::requires_vision("create a sphere"));
        assert!(!SmartRouter::requires_vision("export as STL"));
    }
    
    #[test]
    fn test_config_default() {
        let config = SmartRouterConfig::default();
        assert_eq!(config.mode, ProcessingMode::Unified);
        assert_eq!(config.vision_config.provider, VisionProviderType::Anthropic);
    }
    
    #[test]
    fn test_cache_key_generation() {
        let viewport = ViewportCapture {
            camera: shared_types::vision::CameraInfo {
                position: [1.0, 2.0, 3.0],
                rotation: [0.0; 3],
                quaternion: [0.0, 0.0, 0.0, 1.0],
                target: [0.0; 3],
                up: [0.0, 1.0, 0.0],
                fov: 50.0,
                aspect: 1.6,
                near: 0.1,
                far: 1000.0,
                zoom: 1.0,
                matrix_world: [0.0; 16],
                projection_matrix: [0.0; 16],
            },
            selection: shared_types::vision::SelectionInfo {
                object_ids: vec!["obj1".to_string(), "obj2".to_string()],
                bounding_box: None,
                center: None,
            },
            // ... other fields with defaults
            image: String::new(),
            cursor_target: None,
            scene_objects: vec![],
            viewport: shared_types::vision::ViewportInfo {
                width: 1920,
                height: 1080,
                client_width: 1920,
                client_height: 1080,
                pixel_ratio: 1.0,
                mouse_screen: shared_types::vision::MousePosition { x: 0.0, y: 0.0 },
                mouse_pixels: shared_types::vision::PixelPosition { x: 0.0, y: 0.0 },
                mouse_world: None,
            },
            lighting: vec![],
            clipping_planes: vec![],
            render_stats: shared_types::vision::RenderStats {
                triangles: 0,
                points: 0,
                lines: 0,
                frame: 0,
                calls: 0,
                vertices: 0,
                faces: 0,
            },
            measurements: shared_types::vision::Measurements {
                distance_between_selected: None,
                camera_to_selection: None,
            },
            timestamp: 0,
        };
        
        let key = VisionCache::make_key(&viewport, "test command");
        assert_eq!(key, "1.00,2.00,3.00|obj1,obj2|test command");
    }
}