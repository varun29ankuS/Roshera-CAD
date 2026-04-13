// src/access.rs

//! Access Control for .ros v3: ACL and ABAC
//!
//! Provides multi-level permissions with role and attribute-based policies

use crate::ros_fs::util::current_time_ms;
use crate::ros_fs::{AccessError, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Chunk-level access levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum AccessLevel {
    Public = 0,  // Thumbnails only
    View = 1,    // Read geometry
    Measure = 2, // Full read access
    Modify = 3,  // Edit capabilities
    Admin = 4,   // Manage AI/IP data
    Owner = 5,   // Full control
}

impl AccessLevel {
    pub fn from_u32(value: u32) -> Result<Self> {
        match value {
            0 => Ok(AccessLevel::Public),
            1 => Ok(AccessLevel::View),
            2 => Ok(AccessLevel::Measure),
            3 => Ok(AccessLevel::Modify),
            4 => Ok(AccessLevel::Admin),
            5 => Ok(AccessLevel::Owner),
            _ => Err(AccessError::ConstraintViolation {
                constraint_type: "access_level".to_string(),
                details: format!("Invalid level: {}", value),
            }
            .into()),
        }
    }

    pub fn can_perform(&self, required: AccessLevel) -> bool {
        *self >= required
    }
}

/// Access control entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessControlEntry {
    pub principal: Principal,
    pub permissions: HashSet<AccessLevel>,
    pub constraints: Vec<Constraint>,
    pub granted_by: Option<String>,
    pub granted_at: u64,
    pub expires_at: Option<u64>,
}

impl AccessControlEntry {
    pub fn new(principal: Principal, permissions: HashSet<AccessLevel>) -> Self {
        AccessControlEntry {
            principal,
            permissions,
            constraints: Vec::new(),
            granted_by: None,
            granted_at: current_time_ms(),
            expires_at: None,
        }
    }

    pub fn with_constraint(mut self, constraint: Constraint) -> Self {
        self.constraints.push(constraint);
        self
    }

    pub fn with_expiration(mut self, expires_at: u64) -> Self {
        self.expires_at = Some(expires_at);
        self
    }

    pub fn is_expired(&self, now: u64) -> bool {
        self.expires_at.map_or(false, |exp| now > exp)
    }
}

/// Principal types
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Principal {
    User(String),
    Role(String),
    Group(String),
    Service(String),
    Anyone,
}

impl Principal {
    pub fn matches(&self, user: &UserContext) -> bool {
        match self {
            Principal::User(id) => &user.user_id == id,
            Principal::Role(role) => user.roles.contains(role),
            Principal::Group(group) => user.groups.contains(group),
            Principal::Service(service) => user.service_account.as_ref() == Some(service),
            Principal::Anyone => true,
        }
    }
}

/// Access constraints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Constraint {
    TimeWindow { start: u64, end: u64 },
    IpRange { ranges: Vec<String> },
    MfaRequired,
    Location { allowed_regions: Vec<String> },
    DeviceType { allowed_types: Vec<String> },
}

impl Constraint {
    pub fn evaluate(&self, ctx: &AccessContext) -> bool {
        match self {
            Constraint::TimeWindow { start, end } => {
                ctx.timestamp >= *start && ctx.timestamp <= *end
            }
            Constraint::IpRange { ranges } => ranges
                .iter()
                .any(|range| self.ip_in_range(&ctx.ip_address, range)),
            Constraint::MfaRequired => ctx.user.mfa_verified,
            Constraint::Location { allowed_regions } => ctx
                .location
                .as_ref()
                .map_or(false, |loc| allowed_regions.contains(loc)),
            Constraint::DeviceType { allowed_types } => ctx
                .device_type
                .as_ref()
                .map_or(false, |dev| allowed_types.contains(dev)),
        }
    }

    fn ip_in_range(&self, ip: &str, range: &str) -> bool {
        // Simple implementation - in production use proper CIDR matching
        ip.starts_with(range.trim_end_matches("*"))
    }
}

/// User context for access checks
#[derive(Debug, Clone)]
pub struct UserContext {
    pub user_id: String,
    pub roles: HashSet<String>,
    pub groups: HashSet<String>,
    pub attributes: HashMap<String, String>,
    pub mfa_verified: bool,
    pub service_account: Option<String>,
}

impl UserContext {
    pub fn new(user_id: String) -> Self {
        UserContext {
            user_id,
            roles: HashSet::new(),
            groups: HashSet::new(),
            attributes: HashMap::new(),
            mfa_verified: false,
            service_account: None,
        }
    }

    pub fn with_role(mut self, role: String) -> Self {
        self.roles.insert(role);
        self
    }

    pub fn with_mfa(mut self) -> Self {
        self.mfa_verified = true;
        self
    }
}

/// Access evaluation context
#[derive(Debug, Clone)]
pub struct AccessContext {
    pub user: UserContext,
    pub timestamp: u64,
    pub ip_address: String,
    pub location: Option<String>,
    pub device_type: Option<String>,
    pub request_id: String,
}

impl AccessContext {
    pub fn new(user: UserContext, ip_address: String) -> Self {
        AccessContext {
            user,
            timestamp: current_time_ms(),
            ip_address,
            location: None,
            device_type: None,
            request_id: crate::ros_fs::util::format_uuid(&crate::ros_fs::util::random_16()),
        }
    }
}

/// Access control manager
pub struct AccessControlManager {
    acl: HashMap<String, Vec<AccessControlEntry>>,
    default_level: AccessLevel,
}

impl AccessControlManager {
    pub fn new() -> Self {
        AccessControlManager {
            acl: HashMap::new(),
            default_level: AccessLevel::Public,
        }
    }

    pub fn with_default_level(mut self, level: AccessLevel) -> Self {
        self.default_level = level;
        self
    }

    /// Add ACL entry for a resource
    pub fn grant_access(&mut self, resource: &str, entry: AccessControlEntry) -> Result<()> {
        // Validate entry
        if entry.is_expired(current_time_ms()) {
            return Err(AccessError::ConstraintViolation {
                constraint_type: "expiration".to_string(),
                details: "Cannot grant already expired access".to_string(),
            }
            .into());
        }

        self.acl
            .entry(resource.to_string())
            .or_default()
            .push(entry);
        Ok(())
    }

    /// Revoke access for a principal
    pub fn revoke_access(&mut self, resource: &str, principal: &Principal) -> Result<bool> {
        if let Some(entries) = self.acl.get_mut(resource) {
            let before_len = entries.len();
            entries.retain(|e| &e.principal != principal);
            Ok(entries.len() < before_len)
        } else {
            Ok(false)
        }
    }

    /// Check if access is allowed
    pub fn check_access(
        &self,
        resource: &str,
        required_level: AccessLevel,
        context: &AccessContext,
    ) -> Result<bool> {
        let entries = match self.acl.get(resource) {
            Some(entries) => entries,
            None => return Ok(self.default_level.can_perform(required_level)),
        };

        let now = context.timestamp;

        for entry in entries {
            // Skip expired entries
            if entry.is_expired(now) {
                continue;
            }

            // Check principal match
            if !entry.principal.matches(&context.user) {
                continue;
            }

            // Check permission level
            if !entry
                .permissions
                .iter()
                .any(|&p| p.can_perform(required_level))
            {
                continue;
            }

            // Check all constraints
            if entry.constraints.iter().all(|c| c.evaluate(context)) {
                return Ok(true);
            }
        }

        // Check default level if no specific grant
        Ok(self.default_level.can_perform(required_level))
    }

    /// Get effective permissions for a user
    pub fn get_effective_permissions(
        &self,
        resource: &str,
        context: &AccessContext,
    ) -> HashSet<AccessLevel> {
        let mut permissions = HashSet::new();

        if let Some(entries) = self.acl.get(resource) {
            for entry in entries {
                if !entry.is_expired(context.timestamp)
                    && entry.principal.matches(&context.user)
                    && entry.constraints.iter().all(|c| c.evaluate(context))
                {
                    permissions.extend(&entry.permissions);
                }
            }
        }

        if permissions.is_empty() {
            permissions.insert(self.default_level);
        }

        permissions
    }

    /// Export ACL for a resource
    pub fn export_acl(&self, resource: &str) -> Vec<AccessControlEntry> {
        self.acl.get(resource).cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_access_levels() {
        assert!(AccessLevel::Admin.can_perform(AccessLevel::Modify));
        assert!(!AccessLevel::View.can_perform(AccessLevel::Modify));
        assert_eq!(AccessLevel::from_u32(3).unwrap(), AccessLevel::Modify);
    }

    #[test]
    fn test_principal_matching() {
        let mut user = UserContext::new("alice".to_string());
        user.roles.insert("engineer".to_string());

        assert!(Principal::User("alice".to_string()).matches(&user));
        assert!(Principal::Role("engineer".to_string()).matches(&user));
        assert!(!Principal::User("bob".to_string()).matches(&user));
    }

    #[test]
    fn test_access_control() {
        let mut acm = AccessControlManager::new();

        // Grant access
        let entry = AccessControlEntry::new(
            Principal::User("alice".to_string()),
            [AccessLevel::View, AccessLevel::Measure]
                .into_iter()
                .collect(),
        );
        acm.grant_access("GEOM", entry).unwrap();

        // Check access
        let user = UserContext::new("alice".to_string());
        let ctx = AccessContext::new(user, "192.168.1.1".to_string());

        assert!(acm.check_access("GEOM", AccessLevel::View, &ctx).unwrap());
        assert!(!acm.check_access("GEOM", AccessLevel::Modify, &ctx).unwrap());
    }

    #[test]
    fn test_constraints() {
        let mut acm = AccessControlManager::new();

        let entry =
            AccessControlEntry::new(Principal::Anyone, [AccessLevel::View].into_iter().collect())
                .with_constraint(Constraint::MfaRequired);

        acm.grant_access("SECURE", entry).unwrap();

        // Without MFA
        let user = UserContext::new("bob".to_string());
        let ctx = AccessContext::new(user, "10.0.0.1".to_string());
        assert!(!acm.check_access("SECURE", AccessLevel::View, &ctx).unwrap());

        // With MFA
        let user = UserContext::new("bob".to_string()).with_mfa();
        let ctx = AccessContext::new(user, "10.0.0.1".to_string());
        assert!(acm.check_access("SECURE", AccessLevel::View, &ctx).unwrap());
    }
}
