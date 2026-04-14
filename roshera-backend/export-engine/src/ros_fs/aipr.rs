// src/aipr.rs

//! AI Provenance: Track, Audit & Serialize AI Commands (.ros v3 "AIPR" chunk)
//!
//! Provides comprehensive tracking of AI-driven design operations with:
//! - Command history with full context
//! - Privacy-aware data collection
//! - Forensic-level detail options
//! - Compliance and audit support

use crate::ros_fs::util::{current_time_ms, sha256};
use crate::ros_fs::{ProvenanceError, Result, RosFileError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// AI Provenance Tracking Level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackingLevel {
    /// Minimal tracking - command types and timestamps only
    Basic = 0,
    /// Standard tracking - includes prompts and results
    Detailed = 1,
    /// Full tracking - includes all context and parameters
    Forensic = 2,
}

impl TrackingLevel {
    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            0 => Ok(TrackingLevel::Basic),
            1 => Ok(TrackingLevel::Detailed),
            2 => Ok(TrackingLevel::Forensic),
            _ => Err(ProvenanceError::TrackingLevelMismatch {
                expected: "0-2".to_string(),
                actual: value.to_string(),
            }
            .into()),
        }
    }

    pub fn should_track_prompts(&self) -> bool {
        matches!(self, TrackingLevel::Detailed | TrackingLevel::Forensic)
    }

    pub fn should_track_responses(&self) -> bool {
        matches!(self, TrackingLevel::Forensic)
    }

    pub fn should_track_parameters(&self) -> bool {
        matches!(self, TrackingLevel::Forensic)
    }
}

/// Privacy settings for tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivacySettings {
    pub anonymize_prompts: bool,
    pub hash_only_mode: bool,
    pub exclude_responses: bool,
    pub local_only: bool,
    pub retention_days: u32,
    pub allowed_models: Option<Vec<String>>,
    pub pii_detection: bool,
}

impl Default for PrivacySettings {
    fn default() -> Self {
        Self {
            anonymize_prompts: false,
            hash_only_mode: false,
            exclude_responses: false,
            local_only: false,
            retention_days: 0,    // 0 = no automatic deletion
            allowed_models: None, // None = all models allowed
            pii_detection: true,
        }
    }
}

impl PrivacySettings {
    /// Create privacy settings for maximum privacy
    pub fn maximum_privacy() -> Self {
        Self {
            anonymize_prompts: true,
            hash_only_mode: true,
            exclude_responses: true,
            local_only: true,
            retention_days: 30,
            allowed_models: Some(vec!["approved_model_v1".to_string()]),
            pii_detection: true,
        }
    }

    /// Create privacy settings for compliance mode
    pub fn compliance_mode() -> Self {
        Self {
            anonymize_prompts: true,
            hash_only_mode: false,
            exclude_responses: false,
            local_only: false,
            retention_days: 365, // 1 year retention
            allowed_models: None,
            pii_detection: true,
        }
    }
}

/// .ros v3 AI Provenance Header
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AIProvenanceHeader {
    pub version: u32,
    pub command_count: u32,
    pub first_timestamp: u64,
    pub last_timestamp: u64,
    pub tracking_level: TrackingLevel,
    pub privacy_settings: PrivacySettings,
    pub session_count: u32,
    pub total_compute_time_ms: u64,
}

impl AIProvenanceHeader {
    pub fn new(tracking_level: TrackingLevel, privacy_settings: PrivacySettings) -> Self {
        let now = current_time_ms();
        AIProvenanceHeader {
            version: 1,
            command_count: 0,
            first_timestamp: now,
            last_timestamp: now,
            tracking_level,
            privacy_settings,
            session_count: 1,
            total_compute_time_ms: 0,
        }
    }
}

/// AI Command Types (extensible)
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandType {
    Create = 0x01,
    Modify = 0x02,
    Delete = 0x03,
    Analyze = 0x04,
    Optimize = 0x05,
    Simulate = 0x06,
    Render = 0x07,
    Generate = 0x08,
    Transform = 0x09,
    Validate = 0x0A,
    Export = 0x0B,
    Import = 0x0C,
    Custom(u8),
}

impl CommandType {
    pub fn as_str(&self) -> &'static str {
        match self {
            CommandType::Create => "CREATE",
            CommandType::Modify => "MODIFY",
            CommandType::Delete => "DELETE",
            CommandType::Analyze => "ANALYZE",
            CommandType::Optimize => "OPTIMIZE",
            CommandType::Simulate => "SIMULATE",
            CommandType::Render => "RENDER",
            CommandType::Generate => "GENERATE",
            CommandType::Transform => "TRANSFORM",
            CommandType::Validate => "VALIDATE",
            CommandType::Export => "EXPORT",
            CommandType::Import => "IMPORT",
            CommandType::Custom(_) => "CUSTOM",
        }
    }

    pub fn is_destructive(&self) -> bool {
        matches!(
            self,
            CommandType::Delete | CommandType::Modify | CommandType::Transform
        )
    }
}

/// Single AI Command Provenance Record
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AICommand {
    // Core fields (always present)
    pub id: [u8; 16],      // Unique command ID
    pub timestamp: u64,    // Unix ms
    pub session_id: u64,   // Session identifier
    pub sequence_num: u32, // Sequence within session
    pub command_type: CommandType,

    // Model information
    pub model_id: [u8; 32], // Model hash/ID
    pub model_version: u32,
    pub model_name: Option<String>, // Human-readable model name

    // Performance metrics
    pub confidence: f32, // 0.0 - 1.0
    pub compute_time_ms: u32,
    pub token_count: Option<u32>, // For LLM operations

    // Content hashes
    pub prompt_hash: [u8; 32],
    pub response_hash: [u8; 32],
    pub result_hash: [u8; 32],

    // Object tracking
    pub affected_objects: Vec<String>,
    pub parent_command_id: Option<[u8; 16]>, // For command chains

    // Optional detailed content (based on privacy settings)
    pub prompt: Option<String>,
    pub response: Option<String>,
    pub parameters: Option<HashMap<String, serde_json::Value>>,
    pub error: Option<String>, // If command failed

    // Compliance fields
    pub user_id: Option<String>,     // Who initiated the command
    pub approval_id: Option<String>, // For commands requiring approval
    pub tags: Vec<String>,           // Custom tags for filtering
}

impl AICommand {
    /// Create a new AI command record
    pub fn new(
        command_type: CommandType,
        model_id: [u8; 32],
        model_version: u32,
        session_id: u64,
        sequence_num: u32,
    ) -> Self {
        AICommand {
            id: crate::ros_fs::util::random_16(),
            timestamp: current_time_ms(),
            session_id,
            sequence_num,
            command_type,
            model_id,
            model_version,
            model_name: None,
            confidence: 0.0,
            compute_time_ms: 0,
            token_count: None,
            prompt_hash: [0; 32],
            response_hash: [0; 32],
            result_hash: [0; 32],
            affected_objects: Vec::new(),
            parent_command_id: None,
            prompt: None,
            response: None,
            parameters: None,
            error: None,
            user_id: None,
            approval_id: None,
            tags: Vec::new(),
        }
    }

    /// Calculate hashes for content
    pub fn calculate_hashes(&mut self, prompt: &str, response: &str, result: &[u8]) {
        self.prompt_hash = sha256(prompt.as_bytes());
        self.response_hash = sha256(response.as_bytes());
        self.result_hash = sha256(result);
    }

    /// Check if this command matches a filter
    pub fn matches_filter(&self, filter: &CommandFilter) -> bool {
        // Check command type
        if let Some(ref types) = filter.command_types {
            if !types.contains(&self.command_type) {
                return false;
            }
        }

        // Check time range
        if let Some(start) = filter.start_time {
            if self.timestamp < start {
                return false;
            }
        }

        if let Some(end) = filter.end_time {
            if self.timestamp > end {
                return false;
            }
        }

        // Check session
        if let Some(session) = filter.session_id {
            if self.session_id != session {
                return false;
            }
        }

        // Check tags
        if let Some(ref tags) = filter.tags {
            if !tags.iter().any(|tag| self.tags.contains(tag)) {
                return false;
            }
        }

        // Check confidence threshold
        if let Some(min_confidence) = filter.min_confidence {
            if self.confidence < min_confidence {
                return false;
            }
        }

        true
    }
}

/// Filter for querying AI commands
#[derive(Debug, Clone, Default)]
pub struct CommandFilter {
    pub command_types: Option<Vec<CommandType>>,
    pub start_time: Option<u64>,
    pub end_time: Option<u64>,
    pub session_id: Option<u64>,
    pub tags: Option<Vec<String>>,
    pub min_confidence: Option<f32>,
    pub affected_objects: Option<Vec<String>>,
}

/// Main AI Command Tracker
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AICommandTracker {
    pub header: AIProvenanceHeader,
    pub commands: Vec<AICommand>,
    pub current_session: u64,
    pub sessions: HashMap<u64, SessionInfo>,
}

/// Session information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: u64,
    pub start_time: u64,
    pub end_time: Option<u64>,
    pub command_count: u32,
    pub user_id: Option<String>,
    pub context: HashMap<String, String>,
}

impl AICommandTracker {
    /// Create a new tracker
    pub fn new(
        tracking_level: TrackingLevel,
        privacy_settings: PrivacySettings,
        session_id: Option<u64>,
    ) -> Self {
        let session_id = session_id.unwrap_or_else(rand::random);
        let mut sessions = HashMap::new();

        sessions.insert(
            session_id,
            SessionInfo {
                id: session_id,
                start_time: current_time_ms(),
                end_time: None,
                command_count: 0,
                user_id: None,
                context: HashMap::new(),
            },
        );

        Self {
            header: AIProvenanceHeader::new(tracking_level, privacy_settings),
            commands: Vec::new(),
            current_session: session_id,
            sessions,
        }
    }

    /// Start a new session
    pub fn start_session(&mut self, user_id: Option<String>) -> u64 {
        let session_id = rand::random();

        self.sessions.insert(
            session_id,
            SessionInfo {
                id: session_id,
                start_time: current_time_ms(),
                end_time: None,
                command_count: 0,
                user_id,
                context: HashMap::new(),
            },
        );

        self.current_session = session_id;
        self.header.session_count += 1;
        session_id
    }

    /// End current session
    pub fn end_session(&mut self) {
        if let Some(session) = self.sessions.get_mut(&self.current_session) {
            session.end_time = Some(current_time_ms());
        }
    }

    /// Track a new AI command
    pub fn track_command(
        &mut self,
        command_type: CommandType,
        model_id: [u8; 32],
        model_version: u32,
        prompt: &str,
        response: &str,
        affected_objects: &[String],
        confidence: f32,
        compute_time_ms: u32,
        parameters: Option<HashMap<String, serde_json::Value>>,
    ) -> Result<[u8; 16]> {
        // Check if model is allowed
        if let Some(ref allowed) = self.header.privacy_settings.allowed_models {
            let model_name = format!("{:?}", model_id); // In production, use proper model name lookup
            if !allowed.iter().any(|m| m == &model_name) {
                return Err(RosFileError::Other {
                    message: "Model not in allowed list".to_string(),
                    source: None,
                });
            }
        }

        // Get sequence number for this session
        let sequence_num = self
            .sessions
            .get(&self.current_session)
            .map(|s| s.command_count)
            .unwrap_or(0);

        // Create command record
        let mut cmd = AICommand::new(
            command_type,
            model_id,
            model_version,
            self.current_session,
            sequence_num,
        );

        // Set basic fields
        cmd.confidence = confidence;
        cmd.compute_time_ms = compute_time_ms;
        cmd.affected_objects = affected_objects.to_vec();

        // Calculate hashes
        cmd.calculate_hashes(prompt, response, &[]);

        // Apply privacy settings
        let privacy = &self.header.privacy_settings;

        // Store prompt based on settings
        if !privacy.hash_only_mode && self.header.tracking_level.should_track_prompts() {
            let prompt_to_store = if privacy.anonymize_prompts {
                Self::anonymize_text(prompt)
            } else {
                prompt.to_string()
            };
            cmd.prompt = Some(prompt_to_store);
        }

        // Store response based on settings
        if !privacy.exclude_responses
            && !privacy.hash_only_mode
            && self.header.tracking_level.should_track_responses()
        {
            cmd.response = Some(response.to_string());
        }

        // Store parameters if forensic level
        if self.header.tracking_level.should_track_parameters() {
            cmd.parameters = parameters;
        }

        // Update header stats
        self.header.command_count += 1;
        self.header.last_timestamp = cmd.timestamp;
        self.header.total_compute_time_ms += compute_time_ms as u64;

        // Update session stats
        if let Some(session) = self.sessions.get_mut(&self.current_session) {
            session.command_count += 1;
        }

        let command_id = cmd.id;
        self.commands.push(cmd);

        Ok(command_id)
    }

    /// Query commands with a filter
    pub fn query_commands(&self, filter: &CommandFilter) -> Vec<&AICommand> {
        self.commands
            .iter()
            .filter(|cmd| cmd.matches_filter(filter))
            .collect()
    }

    /// Get command by ID
    pub fn get_command(&self, id: &[u8; 16]) -> Option<&AICommand> {
        self.commands.iter().find(|cmd| &cmd.id == id)
    }

    /// Validate command chain integrity
    pub fn validate_chain(&self) -> Result<()> {
        // Check sequence numbers
        let mut sessions_sequences: HashMap<u64, Vec<u32>> = HashMap::new();

        for cmd in &self.commands {
            sessions_sequences
                .entry(cmd.session_id)
                .or_default()
                .push(cmd.sequence_num);
        }

        // Verify sequences are contiguous
        for (session_id, mut sequences) in sessions_sequences {
            sequences.sort();
            for (i, seq) in sequences.iter().enumerate() {
                if *seq != i as u32 {
                    return Err(ProvenanceError::CommandSequenceError {
                        command_id: format!("session:{}", session_id),
                        details: format!("Expected sequence {}, got {}", i, seq),
                    }
                    .into());
                }
            }
        }

        Ok(())
    }

    /// Serialize to bytes for storage
    pub fn serialize(&self) -> Vec<u8> {
        rmp_serde::to_vec_named(self).unwrap_or_default()
    }

    /// Deserialize from bytes
    pub fn deserialize(data: &[u8]) -> Result<Self> {
        rmp_serde::from_slice(data).map_err(|e| RosFileError::Other {
            message: format!("Failed to deserialize AI provenance: {}", e),
            source: None,
        })
    }

    /// Anonymize text by removing potential PII
    fn anonymize_text(text: &str) -> String {
        use regex::Regex;

        // Email pattern
        let email_re = Regex::new(r"[\w.-]+@[\w.-]+\.\w+").unwrap();
        let mut result = email_re.replace_all(text, "[EMAIL]").to_string();

        // Phone pattern
        let phone_re =
            Regex::new(r"\+?\d{1,3}[-.\s]?\(?\d{1,4}\)?[-.\s]?\d{1,4}[-.\s]?\d{1,9}").unwrap();
        result = phone_re.replace_all(&result, "[PHONE]").to_string();

        // Credit card pattern
        let cc_re = Regex::new(r"\b\d{4}[\s-]?\d{4}[\s-]?\d{4}[\s-]?\d{4}\b").unwrap();
        result = cc_re.replace_all(&result, "[CREDIT_CARD]").to_string();

        // SSN pattern
        let ssn_re = Regex::new(r"\b\d{3}-\d{2}-\d{4}\b").unwrap();
        result = ssn_re.replace_all(&result, "[SSN]").to_string();

        // IP address pattern
        let ip_re = Regex::new(r"\b(?:\d{1,3}\.){3}\d{1,3}\b").unwrap();
        result = ip_re.replace_all(&result, "[IP_ADDRESS]").to_string();

        result
    }

    /// Export commands in a privacy-compliant way
    pub fn export_for_compliance(&self) -> ComplianceExport {
        ComplianceExport {
            header: self.header.clone(),
            command_summary: self
                .commands
                .iter()
                .map(|cmd| CommandSummary {
                    id: cmd.id,
                    timestamp: cmd.timestamp,
                    command_type: cmd.command_type,
                    model_id: cmd.model_id,
                    confidence: cmd.confidence,
                    affected_object_count: cmd.affected_objects.len(),
                    has_error: cmd.error.is_some(),
                    tags: cmd.tags.clone(),
                })
                .collect(),
            total_sessions: self.header.session_count,
            total_compute_time_ms: self.header.total_compute_time_ms,
        }
    }
}

/// Summary for compliance exports
#[derive(Debug, Serialize, Deserialize)]
pub struct CommandSummary {
    pub id: [u8; 16],
    pub timestamp: u64,
    pub command_type: CommandType,
    pub model_id: [u8; 32],
    pub confidence: f32,
    pub affected_object_count: usize,
    pub has_error: bool,
    pub tags: Vec<String>,
}

/// Compliance export format
#[derive(Debug, Serialize, Deserialize)]
pub struct ComplianceExport {
    pub header: AIProvenanceHeader,
    pub command_summary: Vec<CommandSummary>,
    pub total_sessions: u32,
    pub total_compute_time_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tracking_levels() {
        assert!(TrackingLevel::from_u8(0).is_ok());
        assert!(TrackingLevel::from_u8(5).is_err());

        assert!(!TrackingLevel::Basic.should_track_prompts());
        assert!(TrackingLevel::Detailed.should_track_prompts());
        assert!(TrackingLevel::Forensic.should_track_responses());
    }

    #[test]
    fn test_command_tracking() {
        let mut tracker =
            AICommandTracker::new(TrackingLevel::Detailed, PrivacySettings::default(), None);

        let cmd_id = tracker
            .track_command(
                CommandType::Create,
                [1u8; 32],
                1,
                "Create a box with dimensions 10x10x10",
                "Box created successfully",
                &["box_001".to_string()],
                0.95,
                150,
                None,
            )
            .unwrap();

        assert_eq!(tracker.commands.len(), 1);
        assert_eq!(tracker.header.command_count, 1);

        let cmd = tracker.get_command(&cmd_id).unwrap();
        assert_eq!(cmd.command_type, CommandType::Create);
        assert_eq!(cmd.confidence, 0.95);
        assert!(cmd.prompt.is_some()); // Detailed level tracks prompts
    }

    #[test]
    fn test_privacy_anonymization() {
        let text = "Contact john@example.com or call +1-555-123-4567";
        let anonymized = AICommandTracker::anonymize_text(text);

        assert!(anonymized.contains("[EMAIL]"));
        assert!(anonymized.contains("[PHONE]"));
        assert!(!anonymized.contains("john@example.com"));
        assert!(!anonymized.contains("555-123-4567"));
    }

    #[test]
    fn test_command_filtering() {
        let mut tracker =
            AICommandTracker::new(TrackingLevel::Basic, PrivacySettings::default(), None);

        // Add various commands
        for i in 0..5 {
            tracker
                .track_command(
                    if i % 2 == 0 {
                        CommandType::Create
                    } else {
                        CommandType::Modify
                    },
                    [i as u8; 32],
                    1,
                    &format!("Command {}", i),
                    "Response",
                    &[],
                    0.8 + (i as f32 * 0.05),
                    100,
                    None,
                )
                .unwrap();
        }

        // Filter by command type
        let filter = CommandFilter {
            command_types: Some(vec![CommandType::Create]),
            ..Default::default()
        };

        let results = tracker.query_commands(&filter);
        assert_eq!(results.len(), 3); // Commands 0, 2, 4

        // Filter by confidence
        let filter = CommandFilter {
            min_confidence: Some(0.9),
            ..Default::default()
        };

        let results = tracker.query_commands(&filter);
        assert_eq!(results.len(), 3); // Commands 2, 3, 4 (not 2)
    }

    #[test]
    fn test_session_management() {
        let mut tracker =
            AICommandTracker::new(TrackingLevel::Basic, PrivacySettings::default(), None);

        let initial_session = tracker.current_session;

        // Start new session
        let new_session = tracker.start_session(Some("user123".to_string()));
        assert_ne!(initial_session, new_session);
        assert_eq!(tracker.current_session, new_session);
        assert_eq!(tracker.header.session_count, 2);

        // End session
        tracker.end_session();
        let session = &tracker.sessions[&new_session];
        assert!(session.end_time.is_some());
    }

    #[test]
    fn test_serialization_roundtrip() {
        let mut tracker = AICommandTracker::new(
            TrackingLevel::Forensic,
            PrivacySettings::compliance_mode(),
            None,
        );

        tracker
            .track_command(
                CommandType::Generate,
                [42u8; 32],
                2,
                "Generate optimized design",
                "Design generated",
                &["design_v2".to_string()],
                0.99,
                500,
                None,
            )
            .unwrap();

        let serialized = tracker.serialize();
        let deserialized = AICommandTracker::deserialize(&serialized).unwrap();

        assert_eq!(deserialized.commands.len(), 1);
        assert_eq!(deserialized.header.command_count, 1);
        assert_eq!(deserialized.header.tracking_level, TrackingLevel::Forensic);
    }
}
