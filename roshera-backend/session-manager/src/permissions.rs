//! Permission system for session access control
//!
//! This module provides fine-grained permission management for CAD sessions,
//! supporting roles, permissions, and access control policies.

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use shared_types::{ObjectId, SessionError};
use std::collections::HashSet;
use std::sync::Arc;

/// User role in a session
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Role {
    /// Full control over session
    Owner,
    /// Can modify geometry and settings
    Editor,
    /// Can view but not modify
    Viewer,
    /// Can only comment/annotate
    Commenter,
    /// Custom role with specific permissions
    Custom(u32),
}

/// Specific permissions that can be granted
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Permission {
    // Session management
    /// Can delete the session
    DeleteSession,
    /// Can invite new users
    InviteUsers,
    /// Can remove users
    RemoveUsers,
    /// Can change user roles
    ChangeRoles,
    /// Can modify session settings
    ModifySettings,

    // Geometry operations
    /// Can create new geometry
    CreateGeometry,
    /// Can modify existing geometry
    ModifyGeometry,
    /// Can delete geometry
    DeleteGeometry,

    // Object operations (aliases for geometry operations)
    /// Can create objects
    CreateObjects,
    /// Can edit objects
    EditObjects,
    /// Can delete objects
    DeleteObjects,
    /// Can view objects
    ViewObjects,
    /// Can execute boolean operations
    BooleanOperations,
    /// Can use advanced features
    AdvancedFeatures,

    // View operations
    /// Can view geometry
    ViewGeometry,
    /// Can measure geometry
    MeasureGeometry,
    /// Can export geometry
    ExportGeometry,
    /// Can export session
    ExportSession,
    /// Can take screenshots
    TakeScreenshots,

    // Timeline operations
    /// Can undo/redo operations
    UndoRedo,
    /// Can create branches
    CreateBranches,
    /// Can merge branches
    MergeBranches,
    /// Can view history
    ViewHistory,

    // Collaboration
    /// Can add comments
    AddComments,
    /// Can use voice chat
    VoiceChat,
    /// Can share screen
    ScreenShare,
    /// Can record session
    RecordSession,

    // Administrative permissions
    /// Can manage user permissions
    ManagePermissions,
    /// Can view all sessions
    ViewAllSessions,
    /// Can delete all sessions
    DeleteAllSessions,
    /// Can create new sessions
    CreateSession,
    /// Can join any session
    JoinSession,
}

/// User permission profile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPermissions {
    /// User ID
    pub user_id: String,
    /// User's role
    pub role: Role,
    /// Explicit permissions (override role defaults)
    pub explicit_permissions: HashSet<Permission>,
    /// Denied permissions (override role defaults)
    pub denied_permissions: HashSet<Permission>,
    /// When permissions were last updated
    pub updated_at: DateTime<Utc>,
    /// Who granted these permissions
    pub granted_by: String,
}

/// Object-level permissions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectPermissions {
    /// Object ID
    pub object_id: ObjectId,
    /// Owner of the object
    pub owner: String,
    /// Users with explicit access
    pub allowed_users: HashSet<String>,
    /// Users explicitly denied access
    pub denied_users: HashSet<String>,
    /// Is object locked
    pub locked: bool,
    /// Who has lock (if locked)
    pub locked_by: Option<String>,
}

/// Permission manager for sessions
pub struct PermissionManager {
    /// Session permissions by session ID
    session_permissions: Arc<DashMap<String, SessionPermissions>>,
    /// Object permissions by object ID
    object_permissions: Arc<DashMap<ObjectId, ObjectPermissions>>,
    /// Permission policies
    policies: Arc<DashMap<String, PermissionPolicy>>,
}

/// Session-wide permissions
#[derive(Debug, Clone)]
pub struct SessionPermissions {
    /// Session ID
    pub session_id: String,
    /// Session owner
    pub owner: String,
    /// User permissions
    pub users: DashMap<String, UserPermissions>,
    /// Default role for new users
    pub default_role: Role,
    /// Is session public
    pub is_public: bool,
    /// Maximum number of users (0 = unlimited)
    pub max_users: usize,
}

/// Permission policy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionPolicy {
    /// Policy name
    pub name: String,
    /// Policy description
    pub description: String,
    /// Rules in the policy
    pub rules: Vec<PolicyRule>,
    /// Is policy active
    pub active: bool,
}

/// Policy rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRule {
    /// Rule condition
    pub condition: RuleCondition,
    /// Action to take
    pub action: RuleAction,
    /// Priority (higher = evaluated first)
    pub priority: i32,
}

/// Rule conditions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RuleCondition {
    /// User has role
    UserHasRole(Role),
    /// User is in group
    UserInGroup(String),
    /// Time-based condition
    TimeBased {
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    },
    /// Resource limit
    ResourceLimit { resource: String, limit: u64 },
    /// Custom condition
    Custom(String),
}

/// Rule actions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RuleAction {
    /// Grant permission
    Grant(Permission),
    /// Deny permission
    Deny(Permission),
    /// Set role
    SetRole(Role),
    /// Execute custom action
    Custom(String),
}

impl PermissionManager {
    /// Create new permission manager
    pub fn new() -> Self {
        let mut manager = Self {
            session_permissions: Arc::new(DashMap::new()),
            object_permissions: Arc::new(DashMap::new()),
            policies: Arc::new(DashMap::new()),
        };

        // Register default policies
        manager.register_default_policies();
        manager
    }

    /// Register default permission policies
    fn register_default_policies(&mut self) {
        // Owner policy
        let owner_policy = PermissionPolicy {
            name: "owner_policy".to_string(),
            description: "Default permissions for session owners".to_string(),
            rules: vec![
                PolicyRule {
                    condition: RuleCondition::UserHasRole(Role::Owner),
                    action: RuleAction::Grant(Permission::DeleteSession),
                    priority: 100,
                },
                PolicyRule {
                    condition: RuleCondition::UserHasRole(Role::Owner),
                    action: RuleAction::Grant(Permission::InviteUsers),
                    priority: 100,
                },
                PolicyRule {
                    condition: RuleCondition::UserHasRole(Role::Owner),
                    action: RuleAction::Grant(Permission::ChangeRoles),
                    priority: 100,
                },
            ],
            active: true,
        };
        self.policies
            .insert("owner_policy".to_string(), owner_policy);

        // Editor policy
        let editor_policy = PermissionPolicy {
            name: "editor_policy".to_string(),
            description: "Default permissions for editors".to_string(),
            rules: vec![
                PolicyRule {
                    condition: RuleCondition::UserHasRole(Role::Editor),
                    action: RuleAction::Grant(Permission::CreateGeometry),
                    priority: 90,
                },
                PolicyRule {
                    condition: RuleCondition::UserHasRole(Role::Editor),
                    action: RuleAction::Grant(Permission::ModifyGeometry),
                    priority: 90,
                },
                PolicyRule {
                    condition: RuleCondition::UserHasRole(Role::Editor),
                    action: RuleAction::Grant(Permission::DeleteGeometry),
                    priority: 90,
                },
            ],
            active: true,
        };
        self.policies
            .insert("editor_policy".to_string(), editor_policy);
    }

    /// Create session permissions
    pub fn create_session_permissions(
        &self,
        session_id: String,
        owner: String,
    ) -> SessionPermissions {
        let owner_permissions = UserPermissions {
            user_id: owner.clone(),
            role: Role::Owner,
            explicit_permissions: HashSet::new(),
            denied_permissions: HashSet::new(),
            updated_at: Utc::now(),
            granted_by: "system".to_string(),
        };

        let users = DashMap::new();
        users.insert(owner.clone(), owner_permissions);

        let permissions = SessionPermissions {
            session_id: session_id.clone(),
            owner,
            users,
            default_role: Role::Viewer,
            is_public: false,
            max_users: 0, // Unlimited
        };

        self.session_permissions
            .insert(session_id, permissions.clone());
        permissions
    }

    /// Check if user has permission
    pub fn check_permission(
        &self,
        session_id: &str,
        user_id: &str,
        permission: Permission,
    ) -> Result<bool, SessionError> {
        let session_perms =
            self.session_permissions
                .get(session_id)
                .ok_or_else(|| SessionError::NotFound {
                    id: session_id.to_string(),
                })?;

        // Get user permissions
        let user_perms = session_perms
            .users
            .get(user_id)
            .ok_or_else(|| SessionError::AccessDenied)?;

        // Check denied permissions first
        if user_perms.denied_permissions.contains(&permission) {
            return Ok(false);
        }

        // Check explicit permissions
        if user_perms.explicit_permissions.contains(&permission) {
            return Ok(true);
        }

        // Check role-based permissions
        Ok(self.role_has_permission(user_perms.role, permission))
    }

    /// Check if role has permission by default
    fn role_has_permission(&self, role: Role, permission: Permission) -> bool {
        match role {
            Role::Owner => true, // Owners have all permissions
            Role::Editor => matches!(
                permission,
                Permission::ViewGeometry
                    | Permission::CreateGeometry
                    | Permission::ModifyGeometry
                    | Permission::DeleteGeometry
                    | Permission::BooleanOperations
                    | Permission::MeasureGeometry
                    | Permission::UndoRedo
                    | Permission::CreateBranches
                    | Permission::ViewHistory
                    | Permission::AddComments
            ),
            Role::Viewer => matches!(
                permission,
                Permission::ViewGeometry
                    | Permission::MeasureGeometry
                    | Permission::ViewHistory
                    | Permission::AddComments
            ),
            Role::Commenter => matches!(
                permission,
                Permission::ViewGeometry | Permission::AddComments
            ),
            Role::Custom(_) => false, // Custom roles only have explicit permissions
        }
    }

    /// Grant permission to user
    pub fn grant_permission(
        &self,
        session_id: &str,
        user_id: &str,
        permission: Permission,
        granted_by: &str,
    ) -> Result<(), SessionError> {
        // Check if granter has permission to change roles
        self.check_permission(session_id, granted_by, Permission::ChangeRoles)?;

        // Get session permissions and update user in same scope
        let session_perms =
            self.session_permissions
                .get(session_id)
                .ok_or_else(|| SessionError::NotFound {
                    id: session_id.to_string(),
                })?;

        // Update user permissions
        let result = match session_perms.users.get_mut(user_id) {
            Some(mut user_perms) => {
                user_perms.explicit_permissions.insert(permission);
                user_perms.denied_permissions.remove(&permission);
                user_perms.updated_at = Utc::now();
                user_perms.granted_by = granted_by.to_string();
                Ok(())
            }
            None => Err(SessionError::NotFound {
                id: user_id.to_string(),
            }),
        };
        result
    }

    /// Deny permission to user
    pub fn deny_permission(
        &self,
        session_id: &str,
        user_id: &str,
        permission: Permission,
        denied_by: &str,
    ) -> Result<(), SessionError> {
        // Check if denier has permission to change roles
        self.check_permission(session_id, denied_by, Permission::ChangeRoles)?;

        // Get session permissions and update user in same scope
        let session_perms =
            self.session_permissions
                .get(session_id)
                .ok_or_else(|| SessionError::NotFound {
                    id: session_id.to_string(),
                })?;

        // Update user permissions
        let result = match session_perms.users.get_mut(user_id) {
            Some(mut user_perms) => {
                user_perms.denied_permissions.insert(permission);
                user_perms.explicit_permissions.remove(&permission);
                user_perms.updated_at = Utc::now();
                user_perms.granted_by = denied_by.to_string();
                Ok(())
            }
            None => Err(SessionError::NotFound {
                id: user_id.to_string(),
            }),
        };
        result
    }

    /// Add user to session
    pub fn add_user(
        &self,
        session_id: &str,
        user_id: String,
        role: Role,
        added_by: &str,
    ) -> Result<(), SessionError> {
        let session_perms =
            self.session_permissions
                .get(session_id)
                .ok_or_else(|| SessionError::NotFound {
                    id: session_id.to_string(),
                })?;

        // Check if adder has permission
        self.check_permission(session_id, added_by, Permission::InviteUsers)?;

        // Check max users limit
        if session_perms.max_users > 0 && session_perms.users.len() >= session_perms.max_users {
            return Err(SessionError::ConflictError {
                details: "Maximum user limit reached".to_string(),
            });
        }

        // Add user
        let user_perms = UserPermissions {
            user_id: user_id.clone(),
            role,
            explicit_permissions: HashSet::new(),
            denied_permissions: HashSet::new(),
            updated_at: Utc::now(),
            granted_by: added_by.to_string(),
        };

        session_perms.users.insert(user_id, user_perms);
        Ok(())
    }

    /// Remove user from session
    pub fn remove_user(
        &self,
        session_id: &str,
        user_id: &str,
        removed_by: &str,
    ) -> Result<(), SessionError> {
        let session_perms =
            self.session_permissions
                .get(session_id)
                .ok_or_else(|| SessionError::NotFound {
                    id: session_id.to_string(),
                })?;

        // Check if remover has permission
        self.check_permission(session_id, removed_by, Permission::RemoveUsers)?;

        // Can't remove owner
        if session_perms.owner == user_id {
            return Err(SessionError::ConflictError {
                details: "Cannot remove session owner".to_string(),
            });
        }

        // Remove user
        session_perms.users.remove(user_id);
        Ok(())
    }

    /// Lock object for exclusive access
    pub fn lock_object(&self, object_id: ObjectId, user_id: &str) -> Result<(), SessionError> {
        let object_id_copy = object_id; // Copy for use in closure
        let mut obj_perms =
            self.object_permissions
                .entry(object_id)
                .or_insert_with(|| ObjectPermissions {
                    object_id: object_id_copy,
                    owner: user_id.to_string(),
                    allowed_users: HashSet::new(),
                    denied_users: HashSet::new(),
                    locked: false,
                    locked_by: None,
                });

        if obj_perms.locked && obj_perms.locked_by.as_deref() != Some(user_id) {
            return Err(SessionError::ConflictError {
                details: format!("Object locked by {}", obj_perms.locked_by.as_ref().unwrap()),
            });
        }

        obj_perms.locked = true;
        obj_perms.locked_by = Some(user_id.to_string());
        Ok(())
    }

    /// Unlock object
    pub fn unlock_object(&self, object_id: ObjectId, user_id: &str) -> Result<(), SessionError> {
        if let Some(mut obj_perms) = self.object_permissions.get_mut(&object_id) {
            if obj_perms.locked_by.as_deref() != Some(user_id) {
                return Err(SessionError::AccessDenied);
            }

            obj_perms.locked = false;
            obj_perms.locked_by = None;
        }
        Ok(())
    }

    /// Check if user can access object
    pub fn can_access_object(&self, object_id: &ObjectId, user_id: &str) -> bool {
        if let Some(obj_perms) = self.object_permissions.get(object_id) {
            // Check denied list first
            if obj_perms.denied_users.contains(user_id) {
                return false;
            }

            // Owner always has access
            if obj_perms.owner == user_id {
                return true;
            }

            // Check allowed list
            obj_perms.allowed_users.contains(user_id)
        } else {
            // No restrictions = everyone can access
            true
        }
    }

    /// Get all users in session
    pub fn get_session_users(
        &self,
        session_id: &str,
    ) -> Result<Vec<UserPermissions>, SessionError> {
        let session_perms =
            self.session_permissions
                .get(session_id)
                .ok_or_else(|| SessionError::NotFound {
                    id: session_id.to_string(),
                })?;

        Ok(session_perms
            .users
            .iter()
            .map(|entry| entry.value().clone())
            .collect())
    }

    /// Apply policy to session
    pub fn apply_policy(&self, session_id: &str, policy_name: &str) -> Result<(), SessionError> {
        let policy = self
            .policies
            .get(policy_name)
            .ok_or_else(|| SessionError::NotFound {
                id: policy_name.to_string(),
            })?;

        if !policy.active {
            return Ok(());
        }

        let session_perms =
            self.session_permissions
                .get(session_id)
                .ok_or_else(|| SessionError::NotFound {
                    id: session_id.to_string(),
                })?;

        // Apply rules to all users
        for user_entry in session_perms.users.iter() {
            let user_perms = user_entry.value();

            // Sort rules by priority
            let mut rules = policy.rules.clone();
            rules.sort_by_key(|r| -r.priority);

            // Apply rules
            for rule in rules {
                if self.evaluate_condition(&rule.condition, &user_perms) {
                    self.apply_action(session_id, &user_perms.user_id, &rule.action, "policy")?;
                }
            }
        }

        Ok(())
    }

    /// Evaluate rule condition
    fn evaluate_condition(&self, condition: &RuleCondition, user: &UserPermissions) -> bool {
        match condition {
            RuleCondition::UserHasRole(role) => user.role == *role,
            RuleCondition::UserInGroup(_group) => {
                // TODO: Implement group membership check
                false
            }
            RuleCondition::TimeBased { start, end } => {
                let now = Utc::now();
                now >= *start && now <= *end
            }
            RuleCondition::ResourceLimit { .. } => {
                // TODO: Implement resource limit check
                true
            }
            RuleCondition::Custom(_) => {
                // TODO: Implement custom conditions
                false
            }
        }
    }

    /// Apply rule action
    fn apply_action(
        &self,
        session_id: &str,
        user_id: &str,
        action: &RuleAction,
        applied_by: &str,
    ) -> Result<(), SessionError> {
        match action {
            RuleAction::Grant(permission) => {
                self.grant_permission(session_id, user_id, *permission, applied_by)
            }
            RuleAction::Deny(permission) => {
                self.deny_permission(session_id, user_id, *permission, applied_by)
            }
            RuleAction::SetRole(role) => {
                if let Some(session_perms) = self.session_permissions.get(session_id) {
                    if let Some(mut user_perms) = session_perms.users.get_mut(user_id) {
                        user_perms.role = *role;
                        user_perms.updated_at = Utc::now();
                        user_perms.granted_by = applied_by.to_string();
                    }
                }
                Ok(())
            }
            RuleAction::Custom(_) => {
                // TODO: Implement custom actions
                Ok(())
            }
        }
    }
}

impl Default for PermissionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_owner_permissions() {
        let manager = PermissionManager::new();
        let session_id = "test-session";
        let owner = "owner-user";

        manager.create_session_permissions(session_id.to_string(), owner.to_string());

        // Owner should have all permissions
        assert!(manager
            .check_permission(session_id, owner, Permission::DeleteSession)
            .unwrap());
        assert!(manager
            .check_permission(session_id, owner, Permission::CreateGeometry)
            .unwrap());
        assert!(manager
            .check_permission(session_id, owner, Permission::InviteUsers)
            .unwrap());
    }

    #[test]
    fn test_role_permissions() {
        let manager = PermissionManager::new();
        let session_id = "test-session";
        let owner = "owner-user";
        let editor = "editor-user";
        let viewer = "viewer-user";

        manager.create_session_permissions(session_id.to_string(), owner.to_string());

        // Add users with different roles
        manager
            .add_user(session_id, editor.to_string(), Role::Editor, owner)
            .unwrap();
        manager
            .add_user(session_id, viewer.to_string(), Role::Viewer, owner)
            .unwrap();

        // Check editor permissions
        assert!(manager
            .check_permission(session_id, editor, Permission::CreateGeometry)
            .unwrap());
        assert!(manager
            .check_permission(session_id, editor, Permission::ModifyGeometry)
            .unwrap());
        assert!(!manager
            .check_permission(session_id, editor, Permission::DeleteSession)
            .unwrap());

        // Check viewer permissions
        assert!(manager
            .check_permission(session_id, viewer, Permission::ViewGeometry)
            .unwrap());
        assert!(!manager
            .check_permission(session_id, viewer, Permission::CreateGeometry)
            .unwrap());
        assert!(!manager
            .check_permission(session_id, viewer, Permission::ModifyGeometry)
            .unwrap());
    }

    #[test]
    fn test_explicit_permissions() {
        let manager = PermissionManager::new();
        let session_id = "test-session";
        let owner = "owner-user";
        let user = "test-user";

        manager.create_session_permissions(session_id.to_string(), owner.to_string());
        manager
            .add_user(session_id, user.to_string(), Role::Viewer, owner)
            .unwrap();

        // Viewer shouldn't have create permission by default
        assert!(!manager
            .check_permission(session_id, user, Permission::CreateGeometry)
            .unwrap());

        // Grant explicit permission
        manager
            .grant_permission(session_id, user, Permission::CreateGeometry, owner)
            .unwrap();

        // Now they should have it
        assert!(manager
            .check_permission(session_id, user, Permission::CreateGeometry)
            .unwrap());

        // Deny the permission
        manager
            .deny_permission(session_id, user, Permission::CreateGeometry, owner)
            .unwrap();

        // Now they shouldn't have it
        assert!(!manager
            .check_permission(session_id, user, Permission::CreateGeometry)
            .unwrap());
    }

    #[test]
    fn test_object_locking() {
        let manager = PermissionManager::new();
        let object_id = Uuid::new_v4();
        let user1 = "user1";
        let user2 = "user2";

        // User1 locks object
        manager.lock_object(object_id, user1).unwrap();

        // User2 can't lock it
        assert!(manager.lock_object(object_id, user2).is_err());

        // User1 can relock it
        manager.lock_object(object_id, user1).unwrap();

        // User1 unlocks
        manager.unlock_object(object_id, user1).unwrap();

        // Now user2 can lock it
        manager.lock_object(object_id, user2).unwrap();
    }
}
