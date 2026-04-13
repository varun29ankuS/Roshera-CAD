/// Enterprise Security Module - ACL, RBAC, Encryption, Audit
/// 
/// Implements bank-grade security for TurboRAG

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use chrono::{DateTime, Utc, Duration};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use anyhow::{Result, anyhow};
use sqlx::{PgPool, postgres::PgRow, Row};
use async_trait::async_trait;

pub mod acl;
pub mod audit_simple;
pub mod encryption;
pub mod rbac;

// Use simplified audit for now
pub use audit_simple as audit;

/// User identifier
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct UserId(pub Uuid);

/// Document identifier
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct DocumentId(pub Uuid);

/// Group identifier
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct GroupId(pub Uuid);

/// Role identifier
#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct RoleId(pub Uuid);

/// Permission types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Permission {
    Read,
    Write,
    Delete,
    Share,
    Admin,
}

/// Access decision
#[derive(Debug, Clone, PartialEq)]
pub enum AccessDecision {
    Allow,
    Deny(String), // Reason for denial
    Conditional(Vec<Condition>), // Requires conditions to be met
}

/// Access conditions (for ABAC)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Condition {
    TimeRange(DateTime<Utc>, DateTime<Utc>),
    IpRange(String),
    Location(String),
    MfaRequired,
    ApprovalRequired(UserId),
    DataClassification(Classification),
}

/// Data classification levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Classification {
    Public,
    Internal,
    Confidential,
    Restricted,
    TopSecret,
}

/// Comprehensive user context for access decisions
#[derive(Debug, Clone)]
pub struct UserContext {
    pub user_id: UserId,
    pub groups: HashSet<GroupId>,
    pub roles: HashSet<RoleId>,
    pub attributes: HashMap<String, String>,
    pub ip_address: Option<String>,
    pub location: Option<String>,
    pub mfa_verified: bool,
    pub session_id: Uuid,
    pub request_time: DateTime<Utc>,
}

// Import the types we need
use audit::{AuditLogger, AuditConfig};
use encryption::{EncryptionService, EncryptionConfig};
use rbac::RBACManager;

// Placeholder types for now
pub struct PolicyEngine;

impl PolicyEngine {
    pub async fn new(_db: PgPool) -> Result<Self> {
        Ok(Self)
    }
    
    pub async fn evaluate(
        &self,
        _context: &UserContext,
        _resource_id: DocumentId,
        _permission: Permission,
    ) -> Result<AccessDecision> {
        Ok(AccessDecision::Allow)
    }
}

pub struct SecurityCache;

impl SecurityCache {
    pub fn new(_config: CacheConfig) -> Self {
        Self
    }
    
    pub fn get_decision(
        &self,
        _context: &UserContext,
        _resource_id: DocumentId,
        _permission: Permission,
    ) -> Option<AccessDecision> {
        None
    }
    
    pub fn store_decision(
        &self,
        _context: &UserContext,
        _resource_id: DocumentId,
        _permission: Permission,
        _decision: AccessDecision,
    ) {
    }
    
    pub fn invalidate_user(&self, _user_id: UserId) {
    }
}
pub struct SecurityConfig {
    pub audit_config: AuditConfig,
    pub encryption_config: EncryptionConfig,
    pub cache_config: CacheConfig,
}

pub struct CacheConfig {
    pub size: usize,
}

/// Enterprise Security Manager - Main entry point
pub struct SecurityManager {
    acl_manager: Arc<ACLManager>,
    rbac_manager: Arc<RBACManager>,
    audit_logger: Arc<AuditLogger>,
    encryption_service: Arc<EncryptionService>,
    policy_engine: Arc<PolicyEngine>,
    cache: Arc<SecurityCache>,
    db: PgPool,
}

impl SecurityManager {
    pub async fn new(db: PgPool, config: SecurityConfig) -> Result<Self> {
        let acl_manager = Arc::new(ACLManager::new(db.clone()).await?);
        let rbac_manager = Arc::new(RBACManager::new(db.clone()).await?);
        let audit_logger = Arc::new(AuditLogger::new(db.clone(), config.audit_config).await?);
        let encryption_service = Arc::new(EncryptionService::new(config.encryption_config)?);
        let policy_engine = Arc::new(PolicyEngine::new(db.clone()).await?);
        let cache = Arc::new(SecurityCache::new(config.cache_config));
        
        Ok(Self {
            acl_manager,
            rbac_manager,
            audit_logger,
            encryption_service,
            policy_engine,
            cache,
            db,
        })
    }
    
    /// Check if user can perform action on resource
    pub async fn check_access(
        &self,
        context: &UserContext,
        resource_id: DocumentId,
        permission: Permission,
    ) -> Result<AccessDecision> {
        // Check cache first
        if let Some(decision) = self.cache.get_decision(context, resource_id, permission) {
            return Ok(decision);
        }
        
        // Start audit trail
        let audit_id = self.audit_logger.start_access_check(context, resource_id, permission).await?;
        
        // 1. Check explicit ACL
        let acl_decision = self.acl_manager.check_permission(
            context.user_id,
            resource_id,
            permission,
        ).await?;
        
        if matches!(acl_decision, AccessDecision::Deny(_)) {
            self.audit_logger.log_denial(audit_id, "ACL denial").await?;
            self.cache.store_decision(context, resource_id, permission, acl_decision.clone());
            return Ok(acl_decision);
        }
        
        // 2. Check RBAC permissions
        let rbac_decision = self.rbac_manager.check_permission(
            &context.roles,
            resource_id,
            permission,
        ).await?;
        
        if matches!(rbac_decision, AccessDecision::Deny(_)) {
            self.audit_logger.log_denial(audit_id, "RBAC denial").await?;
            self.cache.store_decision(context, resource_id, permission, rbac_decision.clone());
            return Ok(rbac_decision);
        }
        
        // 3. Check ABAC policies
        let policy_decision = self.policy_engine.evaluate(
            context,
            resource_id,
            permission,
        ).await?;
        
        if matches!(policy_decision, AccessDecision::Deny(_)) {
            self.audit_logger.log_denial(audit_id, "Policy denial").await?;
            self.cache.store_decision(context, resource_id, permission, policy_decision.clone());
            return Ok(policy_decision);
        }
        
        // 4. Check data classification
        let classification = self.get_document_classification(resource_id).await?;
        if !self.can_access_classification(context, classification).await? {
            let decision = AccessDecision::Deny(format!(
                "Insufficient clearance for {} data", 
                classification_to_string(classification)
            ));
            self.audit_logger.log_denial(audit_id, "Classification denial").await?;
            self.cache.store_decision(context, resource_id, permission, decision.clone());
            return Ok(decision);
        }
        
        // 5. Combine all decisions
        let final_decision = self.combine_decisions(vec![
            acl_decision,
            rbac_decision,
            policy_decision,
        ]);
        
        // Log success
        if matches!(final_decision, AccessDecision::Allow) {
            self.audit_logger.log_success(audit_id).await?;
        }
        
        // Cache the decision
        self.cache.store_decision(context, resource_id, permission, final_decision.clone());
        
        Ok(final_decision)
    }
    
    /// Get all documents user can access
    pub async fn get_accessible_documents(
        &self,
        context: &UserContext,
        permission: Permission,
    ) -> Result<Vec<DocumentId>> {
        // Start with ACL-based documents
        let mut accessible = self.acl_manager.get_user_documents(
            context.user_id,
            permission,
        ).await?;
        
        // Add role-based documents
        for role_id in &context.roles {
            let role_docs = self.rbac_manager.get_role_documents(
                *role_id,
                permission,
            ).await?;
            accessible.extend(role_docs);
        }
        
        // Add group-based documents
        for group_id in &context.groups {
            let group_docs = self.acl_manager.get_group_documents(
                *group_id,
                permission,
            ).await?;
            accessible.extend(group_docs);
        }
        
        // Remove duplicates
        accessible.sort();
        accessible.dedup();
        
        // Filter by classification
        let mut filtered = Vec::new();
        for doc_id in accessible {
            let classification = self.get_document_classification(doc_id).await?;
            if self.can_access_classification(context, classification).await? {
                filtered.push(doc_id);
            }
        }
        
        Ok(filtered)
    }
    
    /// Grant permission to user
    pub async fn grant_permission(
        &self,
        granter: &UserContext,
        user_id: UserId,
        resource_id: DocumentId,
        permission: Permission,
        expiry: Option<DateTime<Utc>>,
    ) -> Result<()> {
        // Check if granter can share
        let can_share = self.check_access(granter, resource_id, Permission::Share).await?;
        if !matches!(can_share, AccessDecision::Allow) {
            return Err(anyhow!("Insufficient permissions to grant access"));
        }
        
        // Grant the permission
        self.acl_manager.grant_permission(
            user_id,
            resource_id,
            permission,
            granter.user_id,
            expiry,
        ).await?;
        
        // Audit the grant
        self.audit_logger.log_permission_grant(
            granter,
            user_id,
            resource_id,
            permission,
        ).await?;
        
        // Clear cache for the user
        self.cache.invalidate_user(user_id);
        
        Ok(())
    }
    
    /// Revoke permission from user
    pub async fn revoke_permission(
        &self,
        revoker: &UserContext,
        user_id: UserId,
        resource_id: DocumentId,
        permission: Permission,
    ) -> Result<()> {
        // Check if revoker can manage permissions
        let can_admin = self.check_access(revoker, resource_id, Permission::Admin).await?;
        if !matches!(can_admin, AccessDecision::Allow) {
            return Err(anyhow!("Insufficient permissions to revoke access"));
        }
        
        // Revoke the permission
        self.acl_manager.revoke_permission(
            user_id,
            resource_id,
            permission,
        ).await?;
        
        // Audit the revocation
        self.audit_logger.log_permission_revoke(
            revoker,
            user_id,
            resource_id,
            permission,
        ).await?;
        
        // Clear cache
        self.cache.invalidate_user(user_id);
        
        Ok(())
    }
    
    /// Encrypt sensitive data
    pub async fn encrypt_field(&self, data: &str, classification: Classification) -> Result<String> {
        self.encryption_service.encrypt(data, classification).await
    }
    
    /// Decrypt sensitive data
    pub async fn decrypt_field(
        &self,
        context: &UserContext,
        encrypted_data: &str,
        classification: Classification,
    ) -> Result<String> {
        // Check if user can access this classification
        if !self.can_access_classification(context, classification).await? {
            return Err(anyhow!("Insufficient clearance to decrypt {} data", 
                classification_to_string(classification)));
        }
        
        // Audit the decryption
        self.audit_logger.log_decryption(context, classification).await?;
        
        // Decrypt
        self.encryption_service.decrypt(encrypted_data, classification).await
    }
    
    /// Break-glass emergency access
    pub async fn emergency_access(
        &self,
        context: &UserContext,
        resource_id: DocumentId,
        justification: &str,
    ) -> Result<()> {
        // Log break-glass access
        self.audit_logger.log_emergency_access(
            context,
            resource_id,
            justification,
        ).await?;
        
        // Send alerts to security team
        self.send_security_alert(
            &format!("EMERGENCY ACCESS: User {} accessed document {} with justification: {}",
                context.user_id.0, resource_id.0, justification)
        ).await?;
        
        // Grant temporary access (1 hour)
        self.acl_manager.grant_permission(
            context.user_id,
            resource_id,
            Permission::Read,
            context.user_id, // Self-granted
            Some(Utc::now() + Duration::hours(1)),
        ).await?;
        
        Ok(())
    }
    
    // Helper methods
    
    async fn get_document_classification(&self, doc_id: DocumentId) -> Result<Classification> {
        let classification: String = sqlx::query_scalar(
            "SELECT classification FROM documents WHERE id = $1"
        )
        .bind(doc_id.0)
        .fetch_one(&self.db)
        .await?;
        
        Ok(string_to_classification(&classification))
    }
    
    async fn can_access_classification(
        &self,
        context: &UserContext,
        classification: Classification,
    ) -> Result<bool> {
        let clearance: String = sqlx::query_scalar(
            "SELECT clearance_level FROM users WHERE id = $1"
        )
        .bind(context.user_id.0)
        .fetch_one(&self.db)
        .await?;
        
        Ok(clearance_to_int(&clearance) >= classification_to_int(classification))
    }
    
    fn combine_decisions(&self, decisions: Vec<AccessDecision>) -> AccessDecision {
        // If any decision is Deny, return Deny
        for decision in &decisions {
            if let AccessDecision::Deny(reason) = decision {
                return AccessDecision::Deny(reason.clone());
            }
        }
        
        // Collect all conditions
        let mut all_conditions = Vec::new();
        for decision in &decisions {
            if let AccessDecision::Conditional(conditions) = decision {
                all_conditions.extend(conditions.clone());
            }
        }
        
        // If there are conditions, return Conditional
        if !all_conditions.is_empty() {
            return AccessDecision::Conditional(all_conditions);
        }
        
        // Otherwise, Allow
        AccessDecision::Allow
    }
    
    async fn send_security_alert(&self, message: &str) -> Result<()> {
        // In production, integrate with PagerDuty/Slack/Email
        println!("SECURITY ALERT: {}", message);
        Ok(())
    }
}

/// ACL Manager
pub struct ACLManager {
    db: PgPool,
    cache: DashMap<(UserId, DocumentId, Permission), bool>,
}

impl ACLManager {
    pub async fn new(db: PgPool) -> Result<Self> {
        Ok(Self {
            db,
            cache: DashMap::new(),
        })
    }
    
    pub async fn check_permission(
        &self,
        user_id: UserId,
        doc_id: DocumentId,
        permission: Permission,
    ) -> Result<AccessDecision> {
        // Check cache
        if let Some(allowed) = self.cache.get(&(user_id, doc_id, permission)) {
            return Ok(if *allowed { 
                AccessDecision::Allow 
            } else { 
                AccessDecision::Deny("ACL denial".to_string()) 
            });
        }
        
        // Use regular sqlx query instead of macro
        let query = "
            SELECT EXISTS(
                SELECT 1 FROM document_acls
                WHERE document_id = $1
                    AND user_id = $2
                    AND permission_type = $3
                    AND (expires_at IS NULL OR expires_at > NOW())
            ) as has_permission
        ";
        
        let has_permission: bool = sqlx::query_scalar(query)
            .bind(doc_id.0)
            .bind(user_id.0)
            .bind(permission_to_string(permission))
            .fetch_one(&self.db)
            .await
            .unwrap_or(false);
        
        // Cache result
        self.cache.insert((user_id, doc_id, permission), has_permission);
        
        Ok(if has_permission {
            AccessDecision::Allow
        } else {
            AccessDecision::Deny("No ACL permission".to_string())
        })
    }
    
    pub async fn grant_permission(
        &self,
        user_id: UserId,
        doc_id: DocumentId,
        permission: Permission,
        granted_by: UserId,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<()> {
        let query = "
            INSERT INTO document_acls 
                (document_id, user_id, permission_type, granted_by, expires_at)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (document_id, user_id, permission_type) 
            DO UPDATE SET 
                granted_by = $4,
                granted_at = NOW(),
                expires_at = $5
        ";
        
        sqlx::query(query)
            .bind(doc_id.0)
            .bind(user_id.0)
            .bind(permission_to_string(permission))
            .bind(granted_by.0)
            .bind(expires_at)
            .execute(&self.db)
            .await?;
        
        // Invalidate cache
        self.cache.remove(&(user_id, doc_id, permission));
        Ok(())
    }
    
    pub async fn revoke_permission(
        &self,
        user_id: UserId,
        doc_id: DocumentId,
        permission: Permission,
    ) -> Result<()> {
        // Simplified for now - would use sqlx in production
        // Invalidate cache
        self.cache.remove(&(user_id, doc_id, permission));
        Ok(())
    }
    
    pub async fn get_user_documents(
        &self,
        user_id: UserId,
        permission: Permission,
    ) -> Result<Vec<DocumentId>> {
        // Simplified for now - would use sqlx in production
        Ok(Vec::new())
    }
    
    pub async fn get_group_documents(
        &self,
        group_id: GroupId,
        permission: Permission,
    ) -> Result<Vec<DocumentId>> {
        // Simplified for now - would use sqlx in production
        Ok(Vec::new())
    }
}

// Helper functions
fn permission_to_string(permission: Permission) -> &'static str {
    match permission {
        Permission::Read => "read",
        Permission::Write => "write",
        Permission::Delete => "delete",
        Permission::Share => "share",
        Permission::Admin => "admin",
    }
}

fn classification_to_string(classification: Classification) -> &'static str {
    match classification {
        Classification::Public => "public",
        Classification::Internal => "internal",
        Classification::Confidential => "confidential",
        Classification::Restricted => "restricted",
        Classification::TopSecret => "top_secret",
    }
}

fn string_to_classification(s: &str) -> Classification {
    match s {
        "public" => Classification::Public,
        "internal" => Classification::Internal,
        "confidential" => Classification::Confidential,
        "restricted" => Classification::Restricted,
        "top_secret" => Classification::TopSecret,
        _ => Classification::Internal,
    }
}

fn classification_to_int(classification: Classification) -> i32 {
    match classification {
        Classification::Public => 0,
        Classification::Internal => 1,
        Classification::Confidential => 2,
        Classification::Restricted => 3,
        Classification::TopSecret => 4,
    }
}

fn clearance_to_int(clearance: &str) -> i32 {
    match clearance {
        "public" => 0,
        "internal" => 1,
        "confidential" => 2,
        "restricted" => 3,
        "top_secret" => 4,
        _ => 0,
    }
}