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

// AUDIT-M4: the policy-machinery types (`PermissionPolicy`,
// `PolicyRule`, `RuleCondition`, `RuleAction`) and the
// `PermissionManager::policies` / `register_default_policies` /
// `apply_policy` / `evaluate_condition` / `apply_action` surfaces
// were removed in this slice. Two reasons:
//   1. The two seeded "default policies" (owner_policy, editor_policy)
//      were strictly redundant with the role-grant table inside
//      `role_has_permission` — `Role::Owner` already returns `true`
//      for every permission, so the policy that "grants
//      DeleteSession to Owners" added zero behaviour.
//   2. `apply_policy` had no callers in the workspace. The
//      conditions `UserInGroup`, `ResourceLimit`, and `Custom` had
//      no backing schema (groups, counters, custom-predicate
//      registry) and could only fail-closed.
// A future policy layer should land alongside the schema it depends
// on, not as a free-standing surface that cannot evaluate its own
// conditions.

impl PermissionManager {
    /// Create new permission manager
    pub fn new() -> Self {
        Self {
            session_permissions: Arc::new(DashMap::new()),
            object_permissions: Arc::new(DashMap::new()),
        }
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
            let holder = obj_perms.locked_by.as_deref().unwrap_or("<unknown>");
            return Err(SessionError::ConflictError {
                details: format!("Object locked by {}", holder),
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
        let object_id = uuid::Uuid::new_v4();
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
