// src/audit.rs

//! Security Audit Logging for Roshera FS
//!
//! Provides tamper-evident audit trails for security and compliance

use crate::util::{current_time_ms, sha256, to_hex};
use crate::{AuditError, Result};
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
            id: crate::util::random_16(),
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

    /// Compute this entry's content hash for the audit chain.
    ///
    /// Serialization failure (e.g. a non-finite float in an
    /// [`AuditEvent`] payload — JSON cannot represent NaN/Infinity) is
    /// propagated rather than degraded to a constant hash: hashing an
    /// empty buffer on failure would make every unserializable entry
    /// hash equal, letting a broken chain verify vacuously.
    pub fn hash(&self) -> Result<[u8; 32]> {
        let data = serde_json::to_vec(self).map_err(|e| AuditError::HashComputationFailed {
            reason: e.to_string(),
        })?;
        Ok(sha256(&data))
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

/// A witness naming exactly where an audit chain broke — the entry index
/// at which the discontinuity was found, the hash that was expected there
/// (the real predecessor's hash, or empty if the chain start was
/// expected), and the hash actually stored on the entry. Kept as hex
/// strings so it serializes identically whether the break involved a
/// `None` (chain start) or a genuine `[u8; 32]` mismatch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainBreakWitness {
    pub break_index: usize,
    pub expected_hash: String,
    pub found_hash: String,
}

/// The result of [`SecurityAuditLog::verify_chain`]: either the chain is
/// intact, or it is broken at a specific, named location. Distinct from a
/// bare `bool` so a compliance reader can learn WHERE a broken chain
/// broke, not just that it did — the project's conflict-witness norm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChainVerdict {
    Intact,
    Broken(ChainBreakWitness),
}

impl ChainVerdict {
    /// `true` iff the chain verified as intact.
    pub fn is_intact(&self) -> bool {
        matches!(self, ChainVerdict::Intact)
    }
}

/// Security audit log
pub struct SecurityAuditLog {
    entries: VecDeque<AuditEntry>,
    max_entries: usize,
    failed_attempts: std::collections::HashMap<String, Vec<u64>>, // user -> timestamps
}

impl SecurityAuditLog {
    pub fn new(max_entries: usize) -> Self {
        SecurityAuditLog {
            entries: VecDeque::with_capacity(max_entries),
            max_entries,
            failed_attempts: std::collections::HashMap::new(),
        }
    }

    /// Log an audit event
    pub fn log(&mut self, mut entry: AuditEntry) -> Result<()> {
        // Set previous hash for chain
        if let Some(last) = self.entries.back() {
            entry.prev_hash = Some(last.hash()?);
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
                    let mut suspicious = AuditEntry::new(AuditEvent::SuspiciousActivity {
                        details: format!("Multiple failed auth attempts for user: {}", user_id),
                    });
                    // `suspicious` is inserted into the deque ahead of
                    // `entry` (below), so IT inherits `entry`'s original
                    // prev_hash, and `entry` is re-linked to point at
                    // `suspicious` instead — otherwise the injected entry
                    // breaks the hash chain (verify_chain() would report
                    // this log as tampered even though nothing was).
                    suspicious.prev_hash = entry.prev_hash;
                    entry.prev_hash = Some(suspicious.hash()?);
                    self.entries.push_back(suspicious);
                }
            }
        }

        // Add entry and maintain size limit
        self.entries.push_back(entry);
        while self.entries.len() > self.max_entries {
            self.entries.pop_front();
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

    /// Verify audit chain integrity.
    ///
    /// Returns `Ok(ChainVerdict::Broken(witness))` — a computed, failed
    /// verdict carrying a witness naming the exact entry index and the
    /// expected/found hashes — when an entry's `prev_hash` does not
    /// match the hash of its predecessor (tampering, a spliced entry, or
    /// a ring-buffer eviction that left the surviving head pointing at a
    /// now-missing entry). Returns `Err` only when verification itself
    /// could not be carried out (an entry could not be canonically
    /// serialized to hash) — that case must never be conflated with a
    /// hard "chain broken" verdict, nor with the `Broken` "we checked
    /// and it's broken, and here's where" verdict.
    pub fn verify_chain(&self) -> Result<ChainVerdict> {
        if self.entries.is_empty() {
            return Ok(ChainVerdict::Intact);
        }

        let mut prev_hash: Option<[u8; 32]> = None;

        for (i, entry) in self.entries.iter().enumerate() {
            if entry.prev_hash != prev_hash {
                return Ok(ChainVerdict::Broken(ChainBreakWitness {
                    break_index: i,
                    expected_hash: prev_hash.map(|h| to_hex(&h)).unwrap_or_default(),
                    found_hash: entry.prev_hash.map(|h| to_hex(&h)).unwrap_or_default(),
                }));
            }
            prev_hash = Some(entry.hash()?);
        }

        Ok(ChainVerdict::Intact)
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

    /// Export audit log for compliance.
    ///
    /// The `chain_verified` verdict is computed here by actually
    /// running [`Self::verify_chain`] — it is never read from a cached
    /// flag, so a tampered or corrupted chain cannot silently export
    /// as verified ("the kernel cannot lie"). When broken, `chain_break`
    /// carries the witness naming exactly where.
    pub fn export(&self, filter: &AuditFilter) -> Result<AuditExport> {
        let entries = self.query(filter);
        let verdict = self.verify_chain()?;
        let (chain_verified, chain_break) = match verdict {
            ChainVerdict::Intact => (true, None),
            ChainVerdict::Broken(witness) => (false, Some(witness)),
        };

        Ok(AuditExport {
            export_time: current_time_ms(),
            entry_count: entries.len(),
            entries: entries.into_iter().cloned().collect(),
            chain_verified,
            chain_break,
            export_filter: filter.clone(),
        })
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
    /// Witness naming exactly where the chain broke, when `chain_verified`
    /// is `false`. Additive field — `#[serde(default)]` so a reader built
    /// against the older wire shape (before this field existed) still
    /// deserializes an export that doesn't carry it.
    #[serde(default)]
    pub chain_break: Option<ChainBreakWitness>,
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
        assert!(log.verify_chain().unwrap().is_intact());
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

    // ── H6: audit chain lies ────────────────────────────────────────
    //
    // `export()` must report a chain-verification verdict it actually
    // computed, not the constructor's `chain_verified: true` (flipped
    // false only on ring-buffer eviction). These tests tamper an
    // entry's `prev_hash` directly — a private-field access legal here
    // because `mod tests` is a child of `audit`'s own module — since
    // `AuditEntry` has no other mutation surface once logged.

    #[test]
    fn export_reports_computed_chain_verification_not_stored_flag() {
        let mut log = SecurityAuditLog::new(100);
        log.log(AuditEntry::new(AuditEvent::AccessGranted {
            resource: "GEOM".to_string(),
            level: 1,
        }))
        .unwrap();
        log.log(AuditEntry::new(AuditEvent::AccessGranted {
            resource: "GEOM2".to_string(),
            level: 1,
        }))
        .unwrap();

        let correct_prev_hash = log.entries[0].hash().unwrap();

        // Tamper the second entry's prev_hash. The log never overflowed
        // (eviction is the ONLY place today's code flips chain_verified),
        // so nothing but a real verification pass can catch this.
        if let Some(entry) = log.entries.get_mut(1) {
            entry.prev_hash = Some([0xAAu8; 32]);
        }

        let export = log.export(&AuditFilter::default()).unwrap();
        assert!(
            !export.chain_verified,
            "export() reported chain_verified=true for a log with a tampered prev_hash"
        );

        // The verdict must carry a witness naming WHERE the chain broke —
        // a bare bool regresses the project's conflict-witness norm.
        let witness = export
            .chain_break
            .as_ref()
            .expect("a broken chain must carry a witness naming where it broke");
        assert_eq!(
            witness.break_index, 1,
            "the tampered entry is at index 1 in the deque"
        );
        assert_eq!(
            witness.expected_hash,
            crate::util::to_hex(&correct_prev_hash),
            "expected_hash must be the real predecessor's hash"
        );
        assert_eq!(
            witness.found_hash,
            crate::util::to_hex(&[0xAAu8; 32]),
            "found_hash must be the tampered value actually stored on the entry"
        );
    }

    #[test]
    fn export_reports_true_for_a_genuinely_untampered_chain() {
        let mut log = SecurityAuditLog::new(100);
        log.log(AuditEntry::new(AuditEvent::AccessGranted {
            resource: "GEOM".to_string(),
            level: 1,
        }))
        .unwrap();
        log.log(AuditEntry::new(AuditEvent::AccessGranted {
            resource: "GEOM2".to_string(),
            level: 1,
        }))
        .unwrap();

        let export = log.export(&AuditFilter::default()).unwrap();
        assert!(
            export.chain_verified,
            "a genuinely untampered chain must export as verified"
        );
    }

    #[test]
    fn suspicious_activity_entry_does_not_break_the_chain() {
        let mut log = SecurityAuditLog::new(100);
        let ctx = AuditContext {
            user_id: "attacker".to_string(),
            session_id: None,
            ip_address: Some("192.168.1.100".to_string()),
            user_agent: None,
        };

        for _ in 0..6 {
            let event = AuditEvent::AuthenticationFailed {
                method: "password".to_string(),
                attempts: 1,
            };
            let entry = AuditEntry::new(event).with_context(ctx.clone());
            log.log(entry).unwrap();
        }

        assert!(
            log.verify_chain().unwrap().is_intact(),
            "a log that legitimately triggered suspicious-activity detection must still \
             verify as an intact chain — the injected SuspiciousActivity entry must be \
             linked into it, not appended with prev_hash left at None"
        );
    }

    #[test]
    fn eviction_still_reports_chain_not_verified() {
        let mut log = SecurityAuditLog::new(2);
        for i in 0..5 {
            log.log(AuditEntry::new(AuditEvent::AccessGranted {
                resource: format!("R{}", i),
                level: 1,
            }))
            .unwrap();
        }
        let export = log.export(&AuditFilter::default()).unwrap();
        assert!(
            !export.chain_verified,
            "a ring-buffer log that evicted its earliest entries must not report an \
             intact chain: the surviving head's prev_hash points at a now-missing entry"
        );

        let witness = export
            .chain_break
            .as_ref()
            .expect("eviction break must also carry a witness");
        assert_eq!(
            witness.break_index, 0,
            "the surviving head is the break point — it expected a chain start (None) \
             but carries a real predecessor hash from the evicted entry"
        );
        assert_eq!(
            witness.expected_hash, "",
            "index 0 should have no predecessor (chain start)"
        );
        assert!(
            !witness.found_hash.is_empty(),
            "found_hash should be the real (non-empty) hash of the now-evicted predecessor"
        );
    }

    #[test]
    fn chain_break_deserializes_as_none_from_an_older_wire_shape_without_it() {
        // `chain_break` is additive: a compliance reader built against the
        // wire shape from before this field existed must still deserialize
        // an export that never mentions it. This only holds for a
        // named/map encoding (e.g. JSON, or `rmp_serde::to_vec_named`) —
        // `#[serde(default)]` cannot rescue a compact positional encoding
        // (e.g. plain `rmp_serde::to_vec`) missing a trailing field. There
        // is currently no production serializer for `AuditExport` in this
        // crate (verified: `AuditExport` has no callers outside this
        // module), so this guarantee is exercised here but not yet by any
        // real caller.
        let old_shape_json = serde_json::json!({
            "export_time": 1234u64,
            "entry_count": 0,
            "entries": [],
            "chain_verified": true,
            "export_filter": {
                "start_time": null,
                "end_time": null,
                "user_id": null,
                "severity_min": null,
                "event_types": null,
                "limit": null
            }
        });

        let export: AuditExport = serde_json::from_value(old_shape_json)
            .expect("an export shaped without chain_break must still deserialize");
        assert_eq!(
            export.chain_break, None,
            "a missing chain_break field must default to None, not fail to parse"
        );
    }

    #[test]
    fn hash_returns_a_result_and_a_nan_confidence_does_not_silently_collide() {
        // NOTE ON WHAT THIS TEST CAN AND CANNOT PROVE: `serde_json`
        // degrades a non-finite f32/f64 to JSON `null` rather than
        // returning an `Err` (verified empirically here — this was
        // expected to be the reachable failure case for `hash()`, per
        // the original defect report, and it is NOT: `to_vec` returns
        // `Ok` for a NaN confidence). So today's `AuditEvent` variants
        // give `serde_json::to_vec(self)` no reachable failure path —
        // `hash()`'s `Err` arm (`AuditError::HashComputationFailed`) is
        // currently unreachable from the public API, and that is
        // reported honestly rather than papered over with a contrived
        // failing input.
        //
        // What this test DOES prove: `hash()` now returns a `Result`
        // (propagating any future serialization failure instead of
        // silently defaulting to a constant hash — the actual defect:
        // `unwrap_or_default()` on `Err` hashed an empty buffer, making
        // any two differently-corrupt entries collide), and a NaN
        // payload — the input the original defect report named — hashes
        // successfully and distinctly rather than colliding with other
        // entries.
        let with_nan = AuditEntry::new(AuditEvent::AICommandExecuted {
            command_type: "generate_solid".to_string(),
            model_id: "claude".to_string(),
            confidence: f32::NAN,
        });
        let other = AuditEntry::new(AuditEvent::AICommandExecuted {
            command_type: "generate_solid".to_string(),
            model_id: "claude".to_string(),
            confidence: 0.9,
        });

        let hash_nan = with_nan
            .hash()
            .expect("hash() returns a Result, not a bare array");
        let hash_other = other
            .hash()
            .expect("hash() returns a Result, not a bare array");

        assert_ne!(
            hash_nan, hash_other,
            "distinct entries must not collide, even ones carrying a non-finite float"
        );
    }
}
