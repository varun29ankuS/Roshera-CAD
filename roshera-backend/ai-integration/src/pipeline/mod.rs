/// Vision Pipeline Module
///
/// This module contains the vision pipeline components for processing
/// viewport captures and routing them through appropriate AI models.

pub mod smart_router;

pub use smart_router::{SmartRouter, SmartRouterConfig, SmartRouterError};