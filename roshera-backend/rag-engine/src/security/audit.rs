/// Enterprise Audit Logging and Compliance System
/// 
/// SOC2, GDPR, HIPAA compliant audit trail
/// Immutable, cryptographically signed audit logs

use std::sync::Arc;
use anyhow::{Result, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use sqlx::{PgPool, Row};
use sqlx::postgres::PgRow;
use sha2::{Sha256, Digest};
use ed25519_dalek::{Keypair, PublicKey, SecretKey, Signature, Signer, Verifier};
use dashmap::DashMap;
use std::collections::VecDeque;
use tokio::sync::RwLock;
use std::time::Duration;

use super::{UserId, DocumentId, Permission, UserContext, Classification};

/// Audit configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    pub retention_days: u32,
    pub batch_size: usize,
    pub flush_interval_secs: u64,
    pub sign_logs: bool,
    pub encrypt_sensitive: bool,
    pub compliance_mode: ComplianceMode,
}

/// Compliance standards
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ComplianceMode {
    None,
    SOC2,
    GDPR,
    HIPAA,
    PCI_DSS,
    ISO27001,
    All,
}

/// Audit event types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditEventType {
    // Access events
    AccessGranted,
    AccessDenied,
    AccessRevoked,
    EmergencyAccess,
    
    // Document events
    DocumentCreated,
    DocumentRead,
    DocumentModified,
    DocumentDeleted,
    DocumentShared,
    DocumentExported,
    
    // Search events
    SearchPerformed,
    SearchResultsViewed,
    SearchResultsExported,
    
    // User events
    UserLogin,
    UserLogout,
    UserCreated,
    UserModified,
    UserDeleted,
    PasswordChanged,
    MfaEnabled,
    MfaDisabled,
    
    // Admin events
    PermissionGranted,
    PermissionRevoked,
    RoleAssigned,
    RoleRemoved,
    PolicyCreated,
    PolicyModified,
    PolicyDeleted,
    
    // Security events
    SuspiciousActivity,
    BruteForceAttempt,
    DataExfiltrationAttempt,
    UnauthorizedAccess,
    
    // Compliance events
    DataRetentionApplied,
    DataPurged,
    ConsentGranted,
    ConsentRevoked,
    DataExportRequested,
    DataDeletionRequested,
    
    // System events
    SystemStartup,
    SystemShutdown,
    ConfigurationChanged,
    BackupCreated,
    BackupRestored,
}

/// Audit log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub event_type: AuditEventType,
    pub user_id: Option<UserId>,
    pub session_id: Option<Uuid>,
    pub resource_id: Option<String>,
    pub resource_type: Option<String>,
    pub action: String,
    pub result: AuditResult,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub location: Option<String>,
    pub details: serde_json::Value,
    pub risk_score: f32,
    pub hash: String,
    pub previous_hash: String,
    pub signature: Option<Vec<u8>>,
}

/// Audit result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditResult {
    Success,
    Failure(String),
    Partial,
}

/// GDPR-specific audit data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GDPRAuditData {
    pub lawful_basis: String,
    pub data_categories: Vec<String>,
    pub purposes: Vec<String>,
    pub retention_period: String,
    pub third_parties: Vec<String>,
    pub cross_border_transfer: bool,
    pub automated_decision: bool,
}

/// HIPAA-specific audit data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HIPAAAuditData {
    pub phi_accessed: bool,
    pub patient_id: Option<String>,
    pub covered_entity: String,
    pub business_associate: Option<String>,
    pub disclosure_type: Option<String>,
    pub authorization_id: Option<String>,
}

/// Main audit logger
pub struct AuditLogger {
    db: PgPool,
    config: AuditConfig,
    signing_key: Option<Keypair>,
    buffer: Arc<RwLock<VecDeque<AuditEntry>>>,
    last_hash: Arc<RwLock<String>>,
    anomaly_detector: Arc<AnomalyDetector>,
}

impl AuditLogger {
    pub async fn new(db: PgPool, config: AuditConfig) -> Result<Self> {
        // Generate or load signing keypair
        let signing_key = if config.sign_logs {
            Some(Self::load_or_generate_keypair().await?)
        } else {
            None
        };
        
        // Get last hash from database for chain integrity
        let last_hash = Self::get_last_hash(&db).await?;
        
        let logger = Self {
            db,
            config,
            signing_key,
            buffer: Arc::new(RwLock::new(VecDeque::new())),
            last_hash: Arc::new(RwLock::new(last_hash)),
            anomaly_detector: Arc::new(AnomalyDetector::new()),
        };
        
        // Start background flush task
        let logger_clone = logger.clone_for_background();
        tokio::spawn(async move {
            logger_clone.run_flush_loop().await;
        });
        
        Ok(logger)
    }
    
    /// Log an audit event
    pub async fn log(&self, event: AuditEvent) -> Result<Uuid> {
        let entry = self.create_entry(event).await?;
        let entry_id = entry.id;
        
        // Check for anomalies
        if self.anomaly_detector.is_anomalous(&entry).await {
            self.handle_anomaly(&entry).await?;
        }
        
        // Add to buffer
        let mut buffer = self.buffer.write().await;
        buffer.push_back(entry);
        
        // Flush if buffer is full
        if buffer.len() >= self.config.batch_size {
            drop(buffer);
            self.flush().await?;
        }
        
        Ok(entry_id)
    }
    
    /// Log access check
    pub async fn start_access_check(
        &self,
        context: &UserContext,
        resource_id: DocumentId,
        permission: Permission,
    ) -> Result<Uuid> {
        let event = AuditEvent {
            event_type: AuditEventType::AccessGranted,
            user_id: Some(context.user_id),
            session_id: Some(context.session_id),
            resource_id: Some(resource_id.0.to_string()),
            resource_type: Some("document".to_string()),
            action: format!("check_access:{:?}", permission),
            ip_address: context.ip_address.clone(),
            details: serde_json::json!({
                "permission": format!("{:?}", permission),
                "mfa_verified": context.mfa_verified,
            }),
        };
        
        self.log(event).await
    }
    
    /// Log access denial
    pub async fn log_denial(&self, audit_id: Uuid, reason: &str) -> Result<()> {
        let event = AuditEvent {
            event_type: AuditEventType::AccessDenied,
            action: "access_denied".to_string(),
            details: serde_json::json!({
                "audit_id": audit_id,
                "reason": reason,
            }),
            ..Default::default()
        };
        
        self.log(event).await?;
        Ok(())
    }
    
    /// Log successful access
    pub async fn log_success(&self, audit_id: Uuid) -> Result<()> {
        let event = AuditEvent {
            event_type: AuditEventType::AccessGranted,
            action: "access_granted".to_string(),
            details: serde_json::json!({
                "audit_id": audit_id,
            }),
            ..Default::default()
        };
        
        self.log(event).await?;
        Ok(())
    }
    
    /// Log permission grant
    pub async fn log_permission_grant(
        &self,
        granter: &UserContext,
        user_id: UserId,
        resource_id: DocumentId,
        permission: Permission,
    ) -> Result<()> {
        let event = AuditEvent {
            event_type: AuditEventType::PermissionGranted,
            user_id: Some(granter.user_id),
            session_id: Some(granter.session_id),
            resource_id: Some(resource_id.0.to_string()),
            action: "grant_permission".to_string(),
            details: serde_json::json!({
                "target_user": user_id.0,
                "permission": format!("{:?}", permission),
                "granter": granter.user_id.0,
            }),
            ..Default::default()
        };
        
        self.log(event).await?;
        Ok(())
    }
    
    /// Log permission revocation
    pub async fn log_permission_revoke(
        &self,
        revoker: &UserContext,
        user_id: UserId,
        resource_id: DocumentId,
        permission: Permission,
    ) -> Result<()> {
        let event = AuditEvent {
            event_type: AuditEventType::PermissionRevoked,
            user_id: Some(revoker.user_id),
            session_id: Some(revoker.session_id),
            resource_id: Some(resource_id.0.to_string()),
            action: "revoke_permission".to_string(),
            details: serde_json::json!({
                "target_user": user_id.0,
                "permission": format!("{:?}", permission),
                "revoker": revoker.user_id.0,
            }),
            ..Default::default()
        };
        
        self.log(event).await?;
        Ok(())
    }
    
    /// Log decryption event
    pub async fn log_decryption(
        &self,
        context: &UserContext,
        classification: Classification,
    ) -> Result<()> {
        let event = AuditEvent {
            event_type: AuditEventType::DocumentRead,
            user_id: Some(context.user_id),
            session_id: Some(context.session_id),
            action: "decrypt_field".to_string(),
            details: serde_json::json!({
                "classification": format!("{:?}", classification),
            }),
            ..Default::default()
        };
        
        self.log(event).await?;
        Ok(())
    }
    
    /// Log emergency access
    pub async fn log_emergency_access(
        &self,
        context: &UserContext,
        resource_id: DocumentId,
        justification: &str,
    ) -> Result<()> {
        let event = AuditEvent {
            event_type: AuditEventType::EmergencyAccess,
            user_id: Some(context.user_id),
            session_id: Some(context.session_id),
            resource_id: Some(resource_id.0.to_string()),
            action: "emergency_access".to_string(),
            details: serde_json::json!({
                "justification": justification,
                "timestamp": Utc::now(),
            }),
            ..Default::default()
        };
        
        self.log(event).await?;
        
        // Send immediate alert
        self.send_alert(&format!(
            "EMERGENCY ACCESS: User {} accessed document {} with justification: {}",
            context.user_id.0, resource_id.0, justification
        )).await?;
        
        Ok(())
    }
    
    /// Query audit logs
    pub async fn query(
        &self,
        filter: AuditFilter,
        limit: usize,
    ) -> Result<Vec<AuditEntry>> {
        let mut query = sqlx::QueryBuilder::new(
            "SELECT * FROM audit_logs WHERE 1=1"
        );
        
        if let Some(user_id) = filter.user_id {
            query.push(" AND user_id = ");
            query.push_bind(user_id.0);
        }
        
        if let Some(start) = filter.start_time {
            query.push(" AND timestamp >= ");
            query.push_bind(start);
        }
        
        if let Some(end) = filter.end_time {
            query.push(" AND timestamp <= ");
            query.push_bind(end);
        }
        
        if let Some(event_type) = filter.event_type {
            query.push(" AND event_type = ");
            query.push_bind(serde_json::to_string(&event_type)?);
        }
        
        query.push(" ORDER BY timestamp DESC LIMIT ");
        query.push_bind(limit as i64);
        
        let entries = query
            .build()
            .fetch_all(&self.db)
            .await?
            .into_iter()
            .map(|row| self.row_to_entry(row))
            .collect::<Result<Vec<_>>>()?;
        
        // Verify signatures if enabled
        if self.config.sign_logs {
            for entry in &entries {
                self.verify_signature(entry)?;
            }
        }
        
        Ok(entries)
    }
    
    /// Export audit logs for compliance
    pub async fn export_for_compliance(
        &self,
        compliance: ComplianceMode,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<ComplianceExport> {
        let entries = self.query(
            AuditFilter {
                start_time: Some(start),
                end_time: Some(end),
                ..Default::default()
            },
            1000000, // Large limit
        ).await?;
        
        match compliance {
            ComplianceMode::GDPR => self.export_gdpr(entries).await,
            ComplianceMode::HIPAA => self.export_hipaa(entries).await,
            ComplianceMode::SOC2 => self.export_soc2(entries).await,
            ComplianceMode::PCI_DSS => self.export_pci_dss(entries).await,
            ComplianceMode::ISO27001 => self.export_iso27001(entries).await,
            _ => Ok(ComplianceExport::default()),
        }
    }
    
    // Private methods
    
    async fn create_entry(&self, event: AuditEvent) -> Result<AuditEntry> {
        let last_hash = self.last_hash.read().await.clone();
        
        let mut entry = AuditEntry {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            event_type: event.event_type,
            user_id: event.user_id,
            session_id: event.session_id,
            resource_id: event.resource_id,
            resource_type: event.resource_type,
            action: event.action,
            result: AuditResult::Success,
            ip_address: event.ip_address,
            user_agent: event.user_agent,
            location: event.location,
            details: event.details,
            risk_score: self.calculate_risk_score(&event),
            hash: String::new(),
            previous_hash: last_hash.clone(),
            signature: None,
        };
        
        // Calculate hash
        entry.hash = self.calculate_hash(&entry);
        
        // Sign if enabled
        if let Some(ref keypair) = self.signing_key {
            entry.signature = Some(self.sign_entry(&entry, keypair)?);
        }
        
        // Update last hash
        *self.last_hash.write().await = entry.hash.clone();
        
        Ok(entry)
    }
    
    fn calculate_hash(&self, entry: &AuditEntry) -> String {
        let mut hasher = Sha256::new();
        hasher.update(entry.id.as_bytes());
        hasher.update(entry.timestamp.to_rfc3339().as_bytes());
        hasher.update(
            serde_json::to_string(&entry.event_type)
                .expect("AuditEntry::event_type Serialize impl is infallible")
                .as_bytes(),
        );
        hasher.update(entry.action.as_bytes());
        hasher.update(entry.previous_hash.as_bytes());
        hasher.update(
            serde_json::to_string(&entry.details)
                .expect("AuditEntry::details Serialize impl is infallible")
                .as_bytes(),
        );
        
        format!("{:x}", hasher.finalize())
    }
    
    fn sign_entry(&self, entry: &AuditEntry, keypair: &Keypair) -> Result<Vec<u8>> {
        let message = format!("{}{}", entry.hash, entry.previous_hash);
        let signature = keypair.sign(message.as_bytes());
        Ok(signature.to_bytes().to_vec())
    }
    
    fn verify_signature(&self, entry: &AuditEntry) -> Result<()> {
        if let Some(ref keypair) = self.signing_key {
            if let Some(ref sig_bytes) = entry.signature {
                let signature = Signature::from_bytes(sig_bytes)
                    .map_err(|e| anyhow!("Invalid signature: {}", e))?;
                
                let message = format!("{}{}", entry.hash, entry.previous_hash);
                keypair.public.verify(message.as_bytes(), &signature)
                    .map_err(|e| anyhow!("Signature verification failed: {}", e))?;
            }
        }
        Ok(())
    }
    
    fn calculate_risk_score(&self, event: &AuditEvent) -> f32 {
        let mut score = 0.0;
        
        // High-risk events
        match event.event_type {
            AuditEventType::EmergencyAccess => score += 0.8,
            AuditEventType::DataExfiltrationAttempt => score += 1.0,
            AuditEventType::UnauthorizedAccess => score += 0.9,
            AuditEventType::BruteForceAttempt => score += 0.7,
            AuditEventType::DocumentDeleted => score += 0.5,
            AuditEventType::PermissionGranted => score += 0.4,
            _ => score += 0.1,
        }
        
        // Unusual location
        if let Some(ref location) = event.location {
            if self.is_unusual_location(location) {
                score += 0.3;
            }
        }
        
        // Outside business hours
        let hour = Utc::now().hour();
        if hour < 6 || hour > 22 {
            score += 0.2;
        }
        
        score.min(1.0)
    }
    
    fn is_unusual_location(&self, _location: &str) -> bool {
        // Implement geolocation checking
        false
    }
    
    async fn flush(&self) -> Result<()> {
        let mut buffer = self.buffer.write().await;
        if buffer.is_empty() {
            return Ok(());
        }
        
        let entries: Vec<_> = buffer.drain(..).collect();
        drop(buffer);
        
        // Batch insert into database
        let mut tx = self.db.begin().await?;
        
        for entry in entries {
            sqlx::query!(
                r#"
                INSERT INTO audit_logs (
                    id, timestamp, event_type, user_id, session_id,
                    resource_id, resource_type, action, result,
                    ip_address, user_agent, location, details,
                    risk_score, hash, previous_hash, signature
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)
                "#,
                entry.id,
                entry.timestamp,
                serde_json::to_string(&entry.event_type)?,
                entry.user_id.map(|u| u.0),
                entry.session_id,
                entry.resource_id,
                entry.resource_type,
                entry.action,
                serde_json::to_string(&entry.result)?,
                entry.ip_address,
                entry.user_agent,
                entry.location,
                entry.details,
                entry.risk_score,
                entry.hash,
                entry.previous_hash,
                entry.signature
            )
            .execute(&mut tx)
            .await?;
        }
        
        tx.commit().await?;
        Ok(())
    }
    
    async fn run_flush_loop(&self) {
        let flush_interval = Duration::from_secs(self.config.flush_interval_secs);
        let mut interval = tokio::time::interval(flush_interval);
        
        loop {
            interval.tick().await;
            if let Err(e) = self.flush().await {
                eprintln!("Audit flush error: {}", e);
            }
        }
    }
    
    async fn handle_anomaly(&self, entry: &AuditEntry) -> Result<()> {
        // Send alert
        self.send_alert(&format!(
            "ANOMALY DETECTED: Event {} by user {:?} with risk score {}",
            entry.action,
            entry.user_id,
            entry.risk_score
        )).await?;
        
        // Log the anomaly itself
        let anomaly_event = AuditEvent {
            event_type: AuditEventType::SuspiciousActivity,
            action: "anomaly_detected".to_string(),
            details: serde_json::json!({
                "original_event": entry.id,
                "risk_score": entry.risk_score,
            }),
            ..Default::default()
        };
        
        self.log(anomaly_event).await?;
        Ok(())
    }
    
    async fn send_alert(&self, message: &str) -> Result<()> {
        // Integrate with alerting system (PagerDuty, Slack, etc.)
        println!("SECURITY ALERT: {}", message);
        Ok(())
    }
    
    async fn export_gdpr(&self, entries: Vec<AuditEntry>) -> Result<ComplianceExport> {
        // Format for GDPR compliance
        let gdpr_entries: Vec<GDPRAuditEntry> = entries
            .into_iter()
            .filter(|e| self.is_gdpr_relevant(e))
            .map(|e| self.to_gdpr_entry(e))
            .collect();
        
        Ok(ComplianceExport {
            standard: ComplianceMode::GDPR,
            entries: serde_json::to_value(gdpr_entries)?,
            generated_at: Utc::now(),
            hash: String::new(), // Calculate hash
        })
    }
    
    async fn export_hipaa(&self, entries: Vec<AuditEntry>) -> Result<ComplianceExport> {
        // Format for HIPAA compliance
        let hipaa_entries: Vec<HIPAAAuditEntry> = entries
            .into_iter()
            .filter(|e| self.is_hipaa_relevant(e))
            .map(|e| self.to_hipaa_entry(e))
            .collect();
        
        Ok(ComplianceExport {
            standard: ComplianceMode::HIPAA,
            entries: serde_json::to_value(hipaa_entries)?,
            generated_at: Utc::now(),
            hash: String::new(),
        })
    }
    
    async fn export_soc2(&self, entries: Vec<AuditEntry>) -> Result<ComplianceExport> {
        // Format for SOC2 Type II
        Ok(ComplianceExport {
            standard: ComplianceMode::SOC2,
            entries: serde_json::to_value(entries)?,
            generated_at: Utc::now(),
            hash: String::new(),
        })
    }
    
    async fn export_pci_dss(&self, entries: Vec<AuditEntry>) -> Result<ComplianceExport> {
        // Format for PCI-DSS
        Ok(ComplianceExport {
            standard: ComplianceMode::PCI_DSS,
            entries: serde_json::to_value(entries)?,
            generated_at: Utc::now(),
            hash: String::new(),
        })
    }
    
    async fn export_iso27001(&self, entries: Vec<AuditEntry>) -> Result<ComplianceExport> {
        // Format for ISO 27001
        Ok(ComplianceExport {
            standard: ComplianceMode::ISO27001,
            entries: serde_json::to_value(entries)?,
            generated_at: Utc::now(),
            hash: String::new(),
        })
    }
    
    fn is_gdpr_relevant(&self, entry: &AuditEntry) -> bool {
        matches!(
            entry.event_type,
            AuditEventType::ConsentGranted |
            AuditEventType::ConsentRevoked |
            AuditEventType::DataExportRequested |
            AuditEventType::DataDeletionRequested |
            AuditEventType::DocumentRead |
            AuditEventType::DocumentModified
        )
    }
    
    fn is_hipaa_relevant(&self, entry: &AuditEntry) -> bool {
        // Check if PHI was accessed
        entry.details.get("phi_accessed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }
    
    fn to_gdpr_entry(&self, entry: AuditEntry) -> GDPRAuditEntry {
        GDPRAuditEntry {
            id: entry.id,
            timestamp: entry.timestamp,
            data_subject: entry.user_id.map(|u| u.0.to_string()),
            processing_activity: entry.action,
            lawful_basis: "legitimate_interest".to_string(), // Extract from details
            purposes: vec![],
            data_categories: vec![],
            recipients: vec![],
            retention_period: "6_months".to_string(),
            cross_border_transfer: false,
        }
    }
    
    fn to_hipaa_entry(&self, entry: AuditEntry) -> HIPAAAuditEntry {
        HIPAAAuditEntry {
            id: entry.id,
            timestamp: entry.timestamp,
            user: entry.user_id.map(|u| u.0.to_string()),
            patient_id: entry.details.get("patient_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            action: entry.action,
            phi_accessed: true,
            covered_entity: "TurboRAG".to_string(),
            authorization: None,
        }
    }
    
    async fn load_or_generate_keypair() -> Result<Keypair> {
        // In production, load from secure key management system
        let mut csprng = rand::rngs::OsRng {};
        Ok(Keypair::generate(&mut csprng))
    }
    
    async fn get_last_hash(db: &PgPool) -> Result<String> {
        let result = sqlx::query_scalar!(
            "SELECT hash FROM audit_logs ORDER BY timestamp DESC LIMIT 1"
        )
        .fetch_optional(db)
        .await?;
        
        Ok(result.unwrap_or_else(|| "genesis".to_string()))
    }
    
    fn row_to_entry(&self, row: PgRow) -> Result<AuditEntry> {
        let id: Uuid = row.try_get("id")?;
        let timestamp: DateTime<Utc> = row.try_get("timestamp")?;
        let event_type_str: String = row.try_get("event_type")?;
        let event_type: AuditEventType = serde_json::from_str(&event_type_str)
            .map_err(|e| anyhow!("Failed to deserialize event_type: {}", e))?;
        let user_id: Option<Uuid> = row.try_get("user_id")?;
        let session_id: Option<Uuid> = row.try_get("session_id")?;
        let resource_id: Option<String> = row.try_get("resource_id")?;
        let resource_type: Option<String> = row.try_get("resource_type")?;
        let action: String = row.try_get("action")?;
        let result_str: String = row.try_get("result")?;
        let result: AuditResult = serde_json::from_str(&result_str)
            .map_err(|e| anyhow!("Failed to deserialize result: {}", e))?;
        let ip_address: Option<String> = row.try_get("ip_address")?;
        let user_agent: Option<String> = row.try_get("user_agent")?;
        let location: Option<String> = row.try_get("location")?;
        let details: serde_json::Value = row.try_get("details")?;
        let risk_score: f32 = row.try_get("risk_score")?;
        let hash: String = row.try_get("hash")?;
        let previous_hash: String = row.try_get("previous_hash")?;
        let signature: Option<Vec<u8>> = row.try_get("signature")?;

        Ok(AuditEntry {
            id,
            timestamp,
            event_type,
            user_id: user_id.map(UserId),
            session_id,
            resource_id,
            resource_type,
            action,
            result,
            ip_address,
            user_agent,
            location,
            details,
            risk_score,
            hash,
            previous_hash,
            signature,
        })
    }
    
    fn clone_for_background(&self) -> AuditLogger {
        AuditLogger {
            db: self.db.clone(),
            config: self.config.clone(),
            signing_key: None,
            buffer: Arc::clone(&self.buffer),
            last_hash: Arc::clone(&self.last_hash),
            anomaly_detector: Arc::clone(&self.anomaly_detector),
        }
    }
}

/// Anomaly detector for suspicious activity
pub struct AnomalyDetector {
    patterns: DashMap<UserId, UserPattern>,
}

impl AnomalyDetector {
    pub fn new() -> Self {
        Self {
            patterns: DashMap::new(),
        }
    }
    
    pub async fn is_anomalous(&self, entry: &AuditEntry) -> bool {
        if let Some(user_id) = entry.user_id {
            // Check user patterns
            if let Some(pattern) = self.patterns.get(&user_id) {
                return pattern.is_anomalous(entry);
            }
        }
        
        // Check general anomalies
        entry.risk_score > 0.7
    }
}

/// User behavior pattern
#[derive(Debug, Clone)]
struct UserPattern {
    usual_hours: Vec<u32>,
    usual_locations: Vec<String>,
    usual_actions: Vec<String>,
    access_frequency: f32,
}

impl UserPattern {
    fn is_anomalous(&self, _entry: &AuditEntry) -> bool {
        // Implement ML-based anomaly detection
        false
    }
}

/// Audit event input
#[derive(Debug, Default)]
pub struct AuditEvent {
    pub event_type: AuditEventType,
    pub user_id: Option<UserId>,
    pub session_id: Option<Uuid>,
    pub resource_id: Option<String>,
    pub resource_type: Option<String>,
    pub action: String,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub location: Option<String>,
    pub details: serde_json::Value,
}

/// Audit query filter
#[derive(Debug, Default)]
pub struct AuditFilter {
    pub user_id: Option<UserId>,
    pub start_time: Option<DateTime<Utc>>,
    pub end_time: Option<DateTime<Utc>>,
    pub event_type: Option<AuditEventType>,
    pub resource_id: Option<String>,
    pub min_risk_score: Option<f32>,
}

/// Compliance export
#[derive(Debug, Serialize, Deserialize)]
pub struct ComplianceExport {
    pub standard: ComplianceMode,
    pub entries: serde_json::Value,
    pub generated_at: DateTime<Utc>,
    pub hash: String,
}

/// GDPR audit entry
#[derive(Debug, Serialize, Deserialize)]
struct GDPRAuditEntry {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub data_subject: Option<String>,
    pub processing_activity: String,
    pub lawful_basis: String,
    pub purposes: Vec<String>,
    pub data_categories: Vec<String>,
    pub recipients: Vec<String>,
    pub retention_period: String,
    pub cross_border_transfer: bool,
}

/// HIPAA audit entry
#[derive(Debug, Serialize, Deserialize)]
struct HIPAAAuditEntry {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub user: Option<String>,
    pub patient_id: Option<String>,
    pub action: String,
    pub phi_accessed: bool,
    pub covered_entity: String,
    pub authorization: Option<String>,
}

impl Default for AuditEventType {
    fn default() -> Self {
        AuditEventType::AccessGranted
    }
}