// src/audit.rs

//! Security Audit Logging for Roshera FS
//!
//! Provides tamper-evident audit trails for security and compliance

use crate::ros_fs::util::{current_time_ms, sha256, to_hex};
use crate::ros_fs::{AuditError, Result};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Audit event types
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AuditEvent {
    // Access events
    AccessGranted {
        resource: String,
        level: u32,
    },
    AccessDenied {
        resource: String,
        level: u32,
        reason: String,
    },
    AccessRevoked {
        resource: String,
        principal: String,
    },

    // Encryption events
    ChunkEncrypted {
        chunk_type: String,
        size: u64,
    },
    ChunkDecrypted {
        chunk_type: String,
        success: bool,
    },
    KeyRotation {
        key_count: u32,
    },

    // File operations
    FileCreated {
        file_id: String,
    },
    FileOpened {
        file_id: String,
        version: String,
    },
    FileSigned {
        signer_id: String,
        algorithm: String,
    },
    FileExported {
        format: String,
        chunks: Vec<String>,
    },

    // AI operations
    AICommandExecuted {
        command_type: String,
        model_id: String,
        confidence: f32,
    },
    AITrackingEnabled {
        level: String,
    },

    // Security events
    AuthenticationFailed {
        method: String,
        attempts: u32,
    },
    SuspiciousActivity {
        details: String,
    },
    ConfigurationChanged {
        setting: String,
        old_value: String,
        new_value: String,
    },
}

impl AuditEvent {
    pub fn severity(&self) -> AuditSeverity {
        use AuditEvent::*;
        match self {
            AccessDenied { .. } | AuthenticationFailed { .. } => AuditSeverity::Warning,
            SuspiciousActivity { .. } => AuditSeverity::Critical,
            ConfigurationChanged { .. } | KeyRotation { .. } => AuditSeverity::High,
            _ => AuditSeverity::Info,
        }
    }
}

/// Audit severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AuditSeverity {
    Info = 0,
    Warning = 1,
    High = 2,
    Critical = 3,
}

/// Audit log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: [u8; 16],
    pub timestamp: u64,
    pub event: AuditEvent,
    pub severity: AuditSeverity,
    pub user_id: Option<String>,
    pub session_id: Option<String>,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub prev_hash: Option<[u8; 32]>,
}

impl AuditEntry {
    pub fn new(event: AuditEvent) -> Self {
        let severity = event.severity();
        AuditEntry {
            id: crate::ros_fs::util::random_16(),
            timestamp: current_time_ms(),
            event,
            severity,
            user_id: None,
            session_id: None,
            ip_address: None,
            user_agent: None,
            prev_hash: None,
        }
    }

    pub fn with_context(mut self, ctx: AuditContext) -> Self {
        self.user_id = Some(ctx.user_id);
        self.session_id = ctx.session_id;
        self.ip_address = ctx.ip_address;
        self.user_agent = ctx.user_agent;
        self
    }

    pub fn hash(&self) -> [u8; 32] {
        let data = serde_json::to_vec(self).unwrap_or_default();
        sha256(&data)
    }
}

/// Context for audit entries
#[derive(Debug, Clone)]
pub struct AuditContext {
    pub user_id: String,
    pub session_id: Option<String>,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
}

/// Query filter for audit logs
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuditFilter {
    pub start_time: Option<u64>,
    pub end_time: Option<u64>,
    pub user_id: Option<String>,
    pub severity_min: Option<AuditSeverity>,
    pub event_types: Option<Vec<String>>,
    pub limit: Option<usize>,
}

/// Security audit log
pub struct SecurityAuditLog {
    entries: VecDeque<AuditEntry>,
    max_entries: usize,
    chain_verified: bool,
    failed_attempts: std::collections::HashMap<String, Vec<u64>>, // user -> timestamps
}

impl SecurityAuditLog {
    pub fn new(max_entries: usize) -> Self {
        SecurityAuditLog {
            entries: VecDeque::with_capacity(max_entries),
            max_entries,
            chain_verified: true,
            failed_attempts: std::collections::HashMap::new(),
        }
    }

    /// Log an audit event
    pub fn log(&mut self, mut entry: AuditEntry) -> Result<()> {
        // Set previous hash for chain
        if let Some(last) = self.entries.back() {
            entry.prev_hash = Some(last.hash());
        }

        // Track failed attempts for suspicious activity detection
        if let AuditEvent::AuthenticationFailed { .. } = &entry.event {
            if let Some(user_id) = &entry.user_id {
                self.failed_attempts
                    .entry(user_id.clone())
                    .or_default()
                    .push(entry.timestamp);

                // Check for suspicious pattern
                if self.is_suspicious_pattern(user_id) {
                    let suspicious = AuditEntry::new(AuditEvent::SuspiciousActivity {
                        details: format!("Multiple failed auth attempts for user: {}", user_id),
                    });
                    self.entries.push_back(suspicious);
                }
            }
        }

        // Add entry and maintain size limit
        self.entries.push_back(entry);
        while self.entries.len() > self.max_entries {
            self.entries.pop_front();
            self.chain_verified = false; // Chain broken when entries removed
        }

        Ok(())
    }

    /// Query audit log with filters
    pub fn query(&self, filter: &AuditFilter) -> Vec<&AuditEntry> {
        let mut results: Vec<&AuditEntry> = self
            .entries
            .iter()
            .filter(|e| {
                // Time filter
                if let Some(start) = filter.start_time {
                    if e.timestamp < start {
                        return false;
                    }
                }
                if let Some(end) = filter.end_time {
                    if e.timestamp > end {
                        return false;
                    }
                }

                // User filter
                if let Some(ref user) = filter.user_id {
                    if e.user_id.as_ref() != Some(user) {
                        return false;
                    }
                }

                // Severity filter
                if let Some(min_sev) = filter.severity_min {
                    if e.severity < min_sev {
                        return false;
                    }
                }

                true
            })
            .collect();

        // Apply limit
        if let Some(limit) = filter.limit {
            results.truncate(limit);
        }

        results
    }

    /// Verify audit chain integrity
    pub fn verify_chain(&self) -> Result<bool> {
        if self.entries.is_empty() {
            return Ok(true);
        }

        let mut prev_hash: Option<[u8; 32]> = None;

        for (i, entry) in self.entries.iter().enumerate() {
            if entry.prev_hash != prev_hash {
                return Err(AuditError::ChainBroken {
                    break_index: i,
                    expected_hash: prev_hash.map(|h| to_hex(&h)).unwrap_or_default(),
                }
                .into());
            }
            prev_hash = Some(entry.hash());
        }

        Ok(true)
    }

    /// Check for suspicious patterns
    fn is_suspicious_pattern(&self, user_id: &str) -> bool {
        const WINDOW_MS: u64 = 5 * 60 * 1000; // 5 minutes
        const THRESHOLD: usize = 5;

        if let Some(attempts) = self.failed_attempts.get(user_id) {
            let now = current_time_ms();
            let recent = attempts.iter().filter(|&&ts| now - ts < WINDOW_MS).count();
            recent >= THRESHOLD
        } else {
            false
        }
    }

    /// Export audit log for compliance
    pub fn export(&self, filter: &AuditFilter) -> AuditExport {
        let entries = self.query(filter);

        AuditExport {
            export_time: current_time_ms(),
            entry_count: entries.len(),
            entries: entries.into_iter().cloned().collect(),
            chain_verified: self.chain_verified,
            export_filter: filter.clone(),
        }
    }

    /// Get statistics
    pub fn statistics(&self) -> AuditStatistics {
        let mut stats = AuditStatistics::default();

        for entry in &self.entries {
            stats.total_events += 1;
            match entry.severity {
                AuditSeverity::Info => stats.info_count += 1,
                AuditSeverity::Warning => stats.warning_count += 1,
                AuditSeverity::High => stats.high_count += 1,
                AuditSeverity::Critical => stats.critical_count += 1,
            }
        }

        if let Some(first) = self.entries.front() {
            stats.oldest_entry = Some(first.timestamp);
        }
        if let Some(last) = self.entries.back() {
            stats.newest_entry = Some(last.timestamp);
        }

        stats
    }
}

/// Audit log export format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditExport {
    pub export_time: u64,
    pub entry_count: usize,
    pub entries: Vec<AuditEntry>,
    pub chain_verified: bool,
    pub export_filter: AuditFilter,
}

/// Audit statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuditStatistics {
    pub total_events: usize,
    pub info_count: usize,
    pub warning_count: usize,
    pub high_count: usize,
    pub critical_count: usize,
    pub oldest_entry: Option<u64>,
    pub newest_entry: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_logging() {
        let mut log = SecurityAuditLog::new(100);

        let event = AuditEvent::AccessGranted {
            resource: "GEOM".to_string(),
            level: 3,
        };

        let entry = AuditEntry::new(event);
        log.log(entry).unwrap();

        assert_eq!(log.entries.len(), 1);
        assert!(log.verify_chain().unwrap());
    }

    #[test]
    fn test_suspicious_activity() {
        let mut log = SecurityAuditLog::new(100);
        let ctx = AuditContext {
            user_id: "attacker".to_string(),
            session_id: None,
            ip_address: Some("192.168.1.100".to_string()),
            user_agent: None,
        };

        // Generate multiple failed attempts
        for _ in 0..6 {
            let event = AuditEvent::AuthenticationFailed {
                method: "password".to_string(),
                attempts: 1,
            };
            let entry = AuditEntry::new(event).with_context(ctx.clone());
            log.log(entry).unwrap();
        }

        // Should have triggered suspicious activity
        let suspicious = log
            .entries
            .iter()
            .any(|e| matches!(e.event, AuditEvent::SuspiciousActivity { .. }));
        assert!(suspicious);
    }

    #[test]
    fn test_query_filtering() {
        let mut log = SecurityAuditLog::new(100);

        // Add various events
        for i in 0..10 {
            let event = if i % 2 == 0 {
                AuditEvent::AccessGranted {
                    resource: format!("RES_{}", i),
                    level: 1,
                }
            } else {
                AuditEvent::AccessDenied {
                    resource: format!("RES_{}", i),
                    level: 3,
                    reason: "Insufficient permissions".to_string(),
                }
            };
            log.log(AuditEntry::new(event)).unwrap();
        }

        // Query warnings only
        let filter = AuditFilter {
            severity_min: Some(AuditSeverity::Warning),
            ..Default::default()
        };

        let results = log.query(&filter);
        assert_eq!(results.len(), 5); // Only AccessDenied events
    }
}
