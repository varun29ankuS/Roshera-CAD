//! AI integration for natural language CAD commands
//!
//! This crate provides natural language processing capabilities
//! for the Roshera CAD system.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod audio_processor;
pub mod audio_processor_advanced;
pub mod audio_processor_rnnoise;
pub mod audio_processor_simple;
pub mod commands;
pub mod context_builder;
pub mod executor;
pub mod full_integration_executor;
pub mod parser;
pub mod processor;
pub mod providers;
pub mod session_aware_processor;
pub mod timeline_aware_executor;
pub mod translator;

pub use commands::{Operation, VoiceCommand};
pub use context_builder::{ContextBuilder, RichAIContext, SceneAnalysis, UserContext};
pub use full_integration_executor::{FullIntegrationConfig, FullIntegrationExecutor};
pub use processor::*;
pub use providers::{ASRProvider, LLMProvider, ProviderManager, TTSProvider};
pub use session_aware_processor::{AIAuthContext, SessionAwareAIProcessor, SessionAwareConfig};
pub use timeline_aware_executor::{AISuggestion, TimelineAwareExecutor, TimelineConfig};

// Re-export commonly used types from shared-types
