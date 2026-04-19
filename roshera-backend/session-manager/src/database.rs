//! Database persistence layer for session management
//!
//! Provides PostgreSQL and SQLite support for persistent storage of sessions,
//! users, permissions, and timeline data.

use crate::auth::{ApiKey, SessionToken};
use crate::permissions::{Permission, Role, UserPermissions};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json;
use sha2::Digest;
use shared_types::{CADObject, GeometryId, SessionError, SessionState};
use sqlx::{
    migrate::MigrateDatabase,
    postgres::{PgPool, PgPoolOptions},
    sqlite::{SqlitePool, SqlitePoolOptions},
    Row,
};
use std::sync::Arc;
use tracing::{error, info, warn};
use uuid::Uuid;

/// Converts a Unix timestamp (seconds since epoch) to a `DateTime<Utc>`.
///
/// Returns the UNIX epoch if `seconds` falls outside the representable range
/// (approximately year -262,144 to +262,143). This keeps database binding
/// sites infallible for audit-only fields without hiding configuration bugs,
/// which would still be visible as a 1970-01-01 row rather than a panic.
fn timestamp_to_datetime(seconds: i64) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(seconds, 0).unwrap_or_else(|| {
        DateTime::<Utc>::from_timestamp(0, 0)
            .expect("UNIX epoch (0s, 0ns) is always a valid timestamp")
    })
}

/// Database backend type
#[derive(Debug, Clone, Copy)]
pub enum DatabaseType {
    /// PostgreSQL database
    PostgreSQL,
    /// SQLite database
    SQLite,
}

/// Database configuration
#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    /// Database type
    pub db_type: DatabaseType,
    /// Connection URL
    pub url: String,
    /// Maximum connections
    pub max_connections: u32,
    /// Connection timeout in seconds
    pub connect_timeout: u64,
    /// Enable migrations
    pub run_migrations: bool,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            db_type: DatabaseType::SQLite,
            url: "sqlite://roshera.db".to_string(),
            max_connections: 10,
            connect_timeout: 30,
            run_migrations: true,
        }
    }
}

/// Database persistence trait
#[async_trait]
pub trait DatabasePersistence: Send + Sync {
    // Session operations
    async fn save_session(&self, session: &SessionState) -> Result<(), SessionError>;
    async fn load_session(&self, session_id: &str) -> Result<SessionState, SessionError>;
    async fn delete_session(&self, session_id: &str) -> Result<(), SessionError>;
    async fn list_sessions(
        &self,
        user_id: Option<&str>,
    ) -> Result<Vec<SessionMetadata>, SessionError>;

    // Object operations
    async fn save_object(&self, session_id: &str, object: &CADObject) -> Result<(), SessionError>;
    async fn load_object(
        &self,
        session_id: &str,
        object_id: &GeometryId,
    ) -> Result<CADObject, SessionError>;
    async fn delete_object(
        &self,
        session_id: &str,
        object_id: &GeometryId,
    ) -> Result<(), SessionError>;
    async fn list_objects(&self, session_id: &str) -> Result<Vec<ObjectMetadata>, SessionError>;

    // User operations
    async fn save_user(&self, user: &UserData) -> Result<(), SessionError>;
    async fn load_user(&self, user_id: &str) -> Result<UserData, SessionError>;
    async fn load_user_by_email(&self, email: &str) -> Result<UserData, SessionError>;
    async fn update_user(&self, user: &UserData) -> Result<(), SessionError>;

    // Permission operations
    async fn save_permissions(
        &self,
        session_id: &str,
        permissions: &UserPermissions,
    ) -> Result<(), SessionError>;
    async fn load_permissions(
        &self,
        session_id: &str,
        user_id: &str,
    ) -> Result<UserPermissions, SessionError>;
    async fn list_permissions(
        &self,
        session_id: &str,
    ) -> Result<Vec<UserPermissions>, SessionError>;

    // Auth operations
    async fn save_token(&self, token: &SessionToken) -> Result<(), SessionError>;
    async fn load_token(&self, token_id: &str) -> Result<SessionToken, SessionError>;
    async fn delete_token(&self, token_id: &str) -> Result<(), SessionError>;
    async fn save_api_key(&self, api_key: &ApiKey) -> Result<(), SessionError>;
    async fn load_api_key(&self, key_id: &str) -> Result<ApiKey, SessionError>;
    async fn delete_api_key(&self, key_id: &str) -> Result<(), SessionError>;

    // Timeline operations
    async fn save_timeline_event(
        &self,
        session_id: &str,
        event: &TimelineEventData,
    ) -> Result<(), SessionError>;
    async fn load_timeline_events(
        &self,
        session_id: &str,
        start: i64,
        end: i64,
    ) -> Result<Vec<TimelineEventData>, SessionError>;
    async fn get_event_count(&self, session_id: &str) -> Result<i64, SessionError>;
}

/// Session metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub id: String,
    pub name: String,
    pub owner: String,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    pub object_count: i32,
    pub user_count: i32,
    pub is_public: bool,
}

/// Object metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectMetadata {
    pub id: GeometryId,
    pub name: Option<String>,
    pub object_type: String,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    pub created_by: String,
    pub locked_by: Option<String>,
}

/// User data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserData {
    pub id: String,
    pub email: String,
    pub name: String,
    pub password_hash: String,
    pub created_at: DateTime<Utc>,
    pub last_login: Option<DateTime<Utc>>,
    pub is_active: bool,
    pub is_verified: bool,
    pub profile: serde_json::Value,
}

/// Timeline event data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEventData {
    pub id: String,
    pub session_id: String,
    pub event_type: String,
    pub user_id: String,
    pub timestamp: DateTime<Utc>,
    pub data: serde_json::Value,
    pub branch_id: Option<String>,
}

/// PostgreSQL implementation
pub struct PostgresDatabase {
    pool: PgPool,
}

impl PostgresDatabase {
    pub async fn new(config: &DatabaseConfig) -> Result<Self, SessionError> {
        // Create database if it doesn't exist
        if !sqlx::Postgres::database_exists(&config.url)
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to check database existence: {}", e),
            })?
        {
            sqlx::Postgres::create_database(&config.url)
                .await
                .map_err(|e| SessionError::PersistenceError {
                    reason: format!("Failed to create database: {}", e),
                })?;
        }

        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .acquire_timeout(std::time::Duration::from_secs(config.connect_timeout))
            .connect(&config.url)
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to connect to PostgreSQL: {}", e),
            })?;

        let db = Self { pool };

        if config.run_migrations {
            db.run_migrations().await?;
        }

        Ok(db)
    }

    async fn run_migrations(&self) -> Result<(), SessionError> {
        // Create users table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS users (
                id VARCHAR(255) PRIMARY KEY,
                email VARCHAR(255) UNIQUE NOT NULL,
                name VARCHAR(255) NOT NULL,
                password_hash TEXT NOT NULL,
                created_at TIMESTAMPTZ NOT NULL,
                last_login TIMESTAMPTZ,
                is_active BOOLEAN NOT NULL DEFAULT true,
                is_verified BOOLEAN NOT NULL DEFAULT false,
                profile JSONB NOT NULL DEFAULT '{}'::jsonb
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to create users table: {}", e),
        })?;

        // Create sessions table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id VARCHAR(255) PRIMARY KEY,
                name VARCHAR(255) NOT NULL,
                owner VARCHAR(255) NOT NULL REFERENCES users(id),
                created_at TIMESTAMPTZ NOT NULL,
                modified_at TIMESTAMPTZ NOT NULL,
                is_public BOOLEAN NOT NULL DEFAULT false,
                settings JSONB NOT NULL DEFAULT '{}'::jsonb,
                metadata JSONB NOT NULL DEFAULT '{}'::jsonb
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to create sessions table: {}", e),
        })?;

        // Create objects table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS objects (
                id VARCHAR(255) NOT NULL,
                session_id VARCHAR(255) NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                name VARCHAR(255),
                object_type VARCHAR(50) NOT NULL,
                data JSONB NOT NULL,
                created_at TIMESTAMPTZ NOT NULL,
                modified_at TIMESTAMPTZ NOT NULL,
                created_by VARCHAR(255) NOT NULL REFERENCES users(id),
                locked_by VARCHAR(255) REFERENCES users(id),
                PRIMARY KEY (id, session_id)
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to create objects table: {}", e),
        })?;

        // Create permissions table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS permissions (
                session_id VARCHAR(255) NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                user_id VARCHAR(255) NOT NULL REFERENCES users(id),
                role VARCHAR(50) NOT NULL,
                explicit_permissions TEXT[],
                denied_permissions TEXT[],
                updated_at TIMESTAMPTZ NOT NULL,
                granted_by VARCHAR(255) NOT NULL REFERENCES users(id),
                PRIMARY KEY (session_id, user_id)
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to create permissions table: {}", e),
        })?;

        // Create tokens table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS tokens (
                id VARCHAR(255) PRIMARY KEY,
                user_id VARCHAR(255) NOT NULL REFERENCES users(id),
                token_hash VARCHAR(255) NOT NULL,
                refresh_token_hash VARCHAR(255),
                created_at TIMESTAMPTZ NOT NULL,
                expires_at TIMESTAMPTZ NOT NULL,
                last_activity TIMESTAMPTZ NOT NULL,
                ip_address VARCHAR(45),
                user_agent TEXT
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to create tokens table: {}", e),
        })?;

        // Create api_keys table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS api_keys (
                id VARCHAR(255) PRIMARY KEY,
                name VARCHAR(255) NOT NULL,
                key_hash VARCHAR(255) NOT NULL,
                prefix VARCHAR(20) NOT NULL,
                user_id VARCHAR(255) NOT NULL REFERENCES users(id),
                permissions TEXT[],
                rate_limit JSONB,
                created_at TIMESTAMPTZ NOT NULL,
                last_used TIMESTAMPTZ,
                expires_at TIMESTAMPTZ,
                active BOOLEAN NOT NULL DEFAULT true
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to create api_keys table: {}", e),
        })?;

        // Create timeline_events table (without INDEX inside CREATE TABLE)
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS timeline_events (
                id VARCHAR(255) PRIMARY KEY,
                session_id VARCHAR(255) NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                event_type VARCHAR(50) NOT NULL,
                user_id VARCHAR(255) NOT NULL REFERENCES users(id),
                timestamp TIMESTAMPTZ NOT NULL,
                data JSONB NOT NULL,
                branch_id VARCHAR(255)
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to create timeline_events table: {}", e),
        })?;

        // Create indexes
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_timeline_session_time ON timeline_events(session_id, timestamp)")
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to create timeline index: {}", e),
            })?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_objects_session ON objects(session_id)")
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to create objects session index: {}", e),
            })?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_objects_created_by ON objects(created_by)")
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to create objects created_by index: {}", e),
            })?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_permissions_user ON permissions(user_id)")
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to create permissions user index: {}", e),
            })?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_tokens_user ON tokens(user_id)")
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to create tokens user index: {}", e),
            })?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_tokens_expires ON tokens(expires_at)")
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to create tokens expires index: {}", e),
            })?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_api_keys_user ON api_keys(user_id)")
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to create api_keys user index: {}", e),
            })?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_api_keys_hash ON api_keys(key_hash)")
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to create api_keys hash index: {}", e),
            })?;

        info!("PostgreSQL migrations completed");
        Ok(())
    }
}

#[async_trait]
impl DatabasePersistence for PostgresDatabase {
    async fn save_session(&self, session: &SessionState) -> Result<(), SessionError> {
        let settings_json = serde_json::to_value(&session.settings).map_err(|e| {
            SessionError::PersistenceError {
                reason: format!("Failed to serialize settings: {}", e),
            }
        })?;

        let metadata_json = serde_json::to_value(&session.metadata).map_err(|e| {
            SessionError::PersistenceError {
                reason: format!("Failed to serialize metadata: {}", e),
            }
        })?;

        sqlx::query(
            r#"
            INSERT INTO sessions (id, name, owner, created_at, modified_at, is_public, settings, metadata)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (id) DO UPDATE SET
                name = EXCLUDED.name,
                modified_at = EXCLUDED.modified_at,
                is_public = EXCLUDED.is_public,
                settings = EXCLUDED.settings,
                metadata = EXCLUDED.metadata
            "#
        )
        .bind(session.id.to_string())
        .bind(&session.name)
        .bind(&session.owner_id)
        .bind(timestamp_to_datetime(session.created_at as i64))
        .bind(timestamp_to_datetime(session.modified_at as i64))
        .bind(false) // is_public
        .bind(settings_json)
        .bind(metadata_json)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to save session: {}", e),
        })?;

        Ok(())
    }

    async fn load_session(&self, session_id: &str) -> Result<SessionState, SessionError> {
        let row = sqlx::query("SELECT * FROM sessions WHERE id = $1")
            .bind(session_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| SessionError::NotFound {
                id: session_id.to_string(),
            })?;

        let id: String = row.get("id");
        let name: String = row.get("name");
        let owner: String = row.get("owner");
        let created_at: DateTime<Utc> = row.get("created_at");
        let modified_at: DateTime<Utc> = row.get("modified_at");
        let settings_json: serde_json::Value = row.get("settings");
        let metadata_json: serde_json::Value = row.get("metadata");

        let settings =
            serde_json::from_value(settings_json).map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to deserialize settings: {}", e),
            })?;

        let metadata =
            serde_json::from_value(metadata_json).map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to deserialize metadata: {}", e),
            })?;

        let parsed_id = Uuid::parse_str(&id).map_err(|e| SessionError::PersistenceError {
            reason: format!("Invalid UUID stored for session {}: {}", id, e),
        })?;
        Ok(SessionState {
            id: parsed_id,
            name,
            owner_id: owner,
            objects: std::collections::HashMap::new(), // Load separately
            history: std::collections::VecDeque::new(), // Load from timeline
            history_index: 0,
            created_at: created_at.timestamp() as u64,
            modified_at: modified_at.timestamp() as u64,
            active_users: Vec::new(), // Load from permissions
            settings,
            metadata,
            sketch_planes: std::collections::HashMap::new(), // Load separately if needed
            active_sketch_plane: None,
            orientation_cube: shared_types::session::OrientationCubeState::default(),
            sketch_state: shared_types::session::SketchState::default(),
        })
    }

    async fn delete_session(&self, session_id: &str) -> Result<(), SessionError> {
        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to delete session: {}", e),
            })?;

        Ok(())
    }

    async fn list_sessions(
        &self,
        user_id: Option<&str>,
    ) -> Result<Vec<SessionMetadata>, SessionError> {
        let query = if let Some(user_id) = user_id {
            sqlx::query(
                r#"
                SELECT s.*, 
                    COUNT(DISTINCT o.id) as object_count,
                    COUNT(DISTINCT p.user_id) as user_count
                FROM sessions s
                LEFT JOIN objects o ON o.session_id = s.id
                LEFT JOIN permissions p ON p.session_id = s.id
                WHERE s.owner = $1 OR p.user_id = $1
                GROUP BY s.id
                "#,
            )
            .bind(user_id)
        } else {
            sqlx::query(
                r#"
                SELECT s.*, 
                    COUNT(DISTINCT o.id) as object_count,
                    COUNT(DISTINCT p.user_id) as user_count
                FROM sessions s
                LEFT JOIN objects o ON o.session_id = s.id
                LEFT JOIN permissions p ON p.session_id = s.id
                GROUP BY s.id
                "#,
            )
        };

        let rows =
            query
                .fetch_all(&self.pool)
                .await
                .map_err(|e| SessionError::PersistenceError {
                    reason: format!("Failed to list sessions: {}", e),
                })?;

        let sessions = rows
            .into_iter()
            .map(|row| SessionMetadata {
                id: row.get("id"),
                name: row.get("name"),
                owner: row.get("owner"),
                created_at: row.get("created_at"),
                modified_at: row.get("modified_at"),
                object_count: row.get("object_count"),
                user_count: row.get("user_count"),
                is_public: row.get("is_public"),
            })
            .collect();

        Ok(sessions)
    }

    async fn save_object(&self, session_id: &str, object: &CADObject) -> Result<(), SessionError> {
        let data = serde_json::to_value(object).map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to serialize object: {}", e),
        })?;

        sqlx::query(
            r#"
            INSERT INTO objects (id, session_id, name, object_type, data, created_at, modified_at, created_by)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (id, session_id) DO UPDATE SET
                name = EXCLUDED.name,
                data = EXCLUDED.data,
                modified_at = EXCLUDED.modified_at
            "#
        )
        .bind(object.id.to_string())
        .bind(session_id)
        .bind(&object.name)
        .bind("unknown") // Object type would be determined from mesh or other properties
        .bind(data)
        .bind(timestamp_to_datetime(object.created_at as i64))
        .bind(timestamp_to_datetime(object.modified_at as i64))
        .bind("system") // TODO: Track actual creator
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to save object: {}", e),
        })?;

        Ok(())
    }

    async fn load_object(
        &self,
        session_id: &str,
        object_id: &GeometryId,
    ) -> Result<CADObject, SessionError> {
        let row = sqlx::query("SELECT data FROM objects WHERE id = $1 AND session_id = $2")
            .bind(&object_id.0)
            .bind(session_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| SessionError::NotFound {
                id: object_id.0.to_string(),
            })?;

        let data: serde_json::Value = row.get("data");
        let object = serde_json::from_value(data).map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to deserialize object: {}", e),
        })?;

        Ok(object)
    }

    async fn delete_object(
        &self,
        session_id: &str,
        object_id: &GeometryId,
    ) -> Result<(), SessionError> {
        sqlx::query("DELETE FROM objects WHERE id = $1 AND session_id = $2")
            .bind(&object_id.0)
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to delete object: {}", e),
            })?;

        Ok(())
    }

    async fn list_objects(&self, session_id: &str) -> Result<Vec<ObjectMetadata>, SessionError> {
        let rows =
            sqlx::query("SELECT * FROM objects WHERE session_id = $1 ORDER BY created_at DESC")
                .bind(session_id)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| SessionError::PersistenceError {
                    reason: format!("Failed to list objects: {}", e),
                })?;

        let objects = rows
            .into_iter()
            .map(|row| ObjectMetadata {
                id: GeometryId(
                    uuid::Uuid::parse_str(&row.get::<String, _>("id"))
                        .unwrap_or_else(|_| uuid::Uuid::new_v4()),
                ),
                name: row.get("name"),
                object_type: row.get("object_type"),
                created_at: row.get("created_at"),
                modified_at: row.get("modified_at"),
                created_by: row.get("created_by"),
                locked_by: row.get("locked_by"),
            })
            .collect();

        Ok(objects)
    }

    async fn save_user(&self, user: &UserData) -> Result<(), SessionError> {
        sqlx::query(
            r#"
            INSERT INTO users (id, email, name, password_hash, created_at, last_login, is_active, is_verified, profile)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#
        )
        .bind(&user.id)
        .bind(&user.email)
        .bind(&user.name)
        .bind(&user.password_hash)
        .bind(user.created_at)
        .bind(user.last_login)
        .bind(user.is_active)
        .bind(user.is_verified)
        .bind(&user.profile)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to save user: {}", e),
        })?;

        Ok(())
    }

    async fn load_user(&self, user_id: &str) -> Result<UserData, SessionError> {
        let row = sqlx::query("SELECT * FROM users WHERE id = $1")
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| SessionError::NotFound {
                id: user_id.to_string(),
            })?;

        Ok(UserData {
            id: row.get("id"),
            email: row.get("email"),
            name: row.get("name"),
            password_hash: row.get("password_hash"),
            created_at: row.get("created_at"),
            last_login: row.get("last_login"),
            is_active: row.get("is_active"),
            is_verified: row.get("is_verified"),
            profile: row.get("profile"),
        })
    }

    async fn load_user_by_email(&self, email: &str) -> Result<UserData, SessionError> {
        let row = sqlx::query("SELECT * FROM users WHERE email = $1")
            .bind(email)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| SessionError::NotFound {
                id: email.to_string(),
            })?;

        Ok(UserData {
            id: row.get("id"),
            email: row.get("email"),
            name: row.get("name"),
            password_hash: row.get("password_hash"),
            created_at: row.get("created_at"),
            last_login: row.get("last_login"),
            is_active: row.get("is_active"),
            is_verified: row.get("is_verified"),
            profile: row.get("profile"),
        })
    }

    async fn update_user(&self, user: &UserData) -> Result<(), SessionError> {
        sqlx::query(
            r#"
            UPDATE users SET
                email = $2,
                name = $3,
                password_hash = $4,
                last_login = $5,
                is_active = $6,
                is_verified = $7,
                profile = $8
            WHERE id = $1
            "#,
        )
        .bind(&user.id)
        .bind(&user.email)
        .bind(&user.name)
        .bind(&user.password_hash)
        .bind(user.last_login)
        .bind(user.is_active)
        .bind(user.is_verified)
        .bind(&user.profile)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to update user: {}", e),
        })?;

        Ok(())
    }

    async fn save_permissions(
        &self,
        session_id: &str,
        permissions: &UserPermissions,
    ) -> Result<(), SessionError> {
        let explicit_perms: Vec<String> = permissions
            .explicit_permissions
            .iter()
            .map(|p| format!("{:?}", p))
            .collect();

        let denied_perms: Vec<String> = permissions
            .denied_permissions
            .iter()
            .map(|p| format!("{:?}", p))
            .collect();

        sqlx::query(
            r#"
            INSERT INTO permissions (session_id, user_id, role, explicit_permissions, denied_permissions, updated_at, granted_by)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (session_id, user_id) DO UPDATE SET
                role = EXCLUDED.role,
                explicit_permissions = EXCLUDED.explicit_permissions,
                denied_permissions = EXCLUDED.denied_permissions,
                updated_at = EXCLUDED.updated_at,
                granted_by = EXCLUDED.granted_by
            "#
        )
        .bind(session_id)
        .bind(&permissions.user_id)
        .bind(format!("{:?}", permissions.role))
        .bind(explicit_perms.as_slice())
        .bind(denied_perms.as_slice())
        .bind(permissions.updated_at)
        .bind(&permissions.granted_by)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to save permissions: {}", e),
        })?;

        Ok(())
    }

    async fn load_permissions(
        &self,
        session_id: &str,
        user_id: &str,
    ) -> Result<UserPermissions, SessionError> {
        let row = sqlx::query("SELECT * FROM permissions WHERE session_id = $1 AND user_id = $2")
            .bind(session_id)
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| SessionError::NotFound {
                id: format!("{}/{}", session_id, user_id),
            })?;

        // Parse role and permissions from strings
        // This is simplified - in production you'd have proper enum serialization
        Ok(UserPermissions {
            user_id: row.get("user_id"),
            role: Role::Viewer, // TODO: Parse from string
            explicit_permissions: std::collections::HashSet::new(), // TODO: Parse from array
            denied_permissions: std::collections::HashSet::new(), // TODO: Parse from array
            updated_at: row.get("updated_at"),
            granted_by: row.get("granted_by"),
        })
    }

    async fn list_permissions(
        &self,
        session_id: &str,
    ) -> Result<Vec<UserPermissions>, SessionError> {
        let rows = sqlx::query("SELECT * FROM permissions WHERE session_id = $1")
            .bind(session_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to list permissions: {}", e),
            })?;

        let permissions = rows
            .into_iter()
            .map(|row| {
                let role_str: String = row.get("role");
                let role = match role_str.as_str() {
                    "Owner" => Role::Owner,
                    "Editor" => Role::Editor,
                    "Viewer" => Role::Viewer,
                    _ => Role::Viewer,
                };

                let explicit_perms: Vec<String> = row.get("explicit_permissions");
                let denied_perms: Vec<String> = row.get("denied_permissions");

                let explicit_permissions = explicit_perms
                    .into_iter()
                    .filter_map(|p| match p.as_str() {
                        "DeleteSession" => Some(Permission::DeleteSession),
                        "InviteUsers" => Some(Permission::InviteUsers),
                        "RemoveUsers" => Some(Permission::RemoveUsers),
                        "ChangeRoles" => Some(Permission::ChangeRoles),
                        "ModifySettings" => Some(Permission::ModifySettings),
                        "CreateGeometry" => Some(Permission::CreateGeometry),
                        "ModifyGeometry" => Some(Permission::ModifyGeometry),
                        "DeleteGeometry" => Some(Permission::DeleteGeometry),
                        "BooleanOperations" => Some(Permission::BooleanOperations),
                        "AdvancedFeatures" => Some(Permission::AdvancedFeatures),
                        "ViewGeometry" => Some(Permission::ViewGeometry),
                        "MeasureGeometry" => Some(Permission::MeasureGeometry),
                        "ExportGeometry" => Some(Permission::ExportGeometry),
                        "TakeScreenshots" => Some(Permission::TakeScreenshots),
                        "UndoRedo" => Some(Permission::UndoRedo),
                        "CreateBranches" => Some(Permission::CreateBranches),
                        "MergeBranches" => Some(Permission::MergeBranches),
                        "ViewHistory" => Some(Permission::ViewHistory),
                        "AddComments" => Some(Permission::AddComments),
                        "VoiceChat" => Some(Permission::VoiceChat),
                        "ScreenShare" => Some(Permission::ScreenShare),
                        "RecordSession" => Some(Permission::RecordSession),
                        _ => None,
                    })
                    .collect();

                let denied_permissions = denied_perms
                    .into_iter()
                    .filter_map(|p| match p.as_str() {
                        "DeleteSession" => Some(Permission::DeleteSession),
                        "InviteUsers" => Some(Permission::InviteUsers),
                        "RemoveUsers" => Some(Permission::RemoveUsers),
                        "ChangeRoles" => Some(Permission::ChangeRoles),
                        "ModifySettings" => Some(Permission::ModifySettings),
                        "CreateGeometry" => Some(Permission::CreateGeometry),
                        "ModifyGeometry" => Some(Permission::ModifyGeometry),
                        "DeleteGeometry" => Some(Permission::DeleteGeometry),
                        "BooleanOperations" => Some(Permission::BooleanOperations),
                        "AdvancedFeatures" => Some(Permission::AdvancedFeatures),
                        "ViewGeometry" => Some(Permission::ViewGeometry),
                        "MeasureGeometry" => Some(Permission::MeasureGeometry),
                        "ExportGeometry" => Some(Permission::ExportGeometry),
                        "TakeScreenshots" => Some(Permission::TakeScreenshots),
                        "UndoRedo" => Some(Permission::UndoRedo),
                        "CreateBranches" => Some(Permission::CreateBranches),
                        "MergeBranches" => Some(Permission::MergeBranches),
                        "ViewHistory" => Some(Permission::ViewHistory),
                        "AddComments" => Some(Permission::AddComments),
                        "VoiceChat" => Some(Permission::VoiceChat),
                        "ScreenShare" => Some(Permission::ScreenShare),
                        "RecordSession" => Some(Permission::RecordSession),
                        _ => None,
                    })
                    .collect();

                UserPermissions {
                    user_id: row.get("user_id"),
                    role,
                    explicit_permissions,
                    denied_permissions,
                    updated_at: row.get("updated_at"),
                    granted_by: row.get("granted_by"),
                }
            })
            .collect();

        Ok(permissions)
    }

    async fn save_token(&self, token: &SessionToken) -> Result<(), SessionError> {
        // Hash tokens before storing
        let mut hasher = sha2::Sha256::new();
        hasher.update(token.token.as_bytes());
        let token_hash = format!("{:x}", hasher.finalize());

        let refresh_hash = token.refresh_token.as_ref().map(|rt| {
            let mut hasher = sha2::Sha256::new();
            hasher.update(rt.as_bytes());
            format!("{:x}", hasher.finalize())
        });

        sqlx::query(
            r#"
            INSERT INTO tokens (id, user_id, token_hash, refresh_token_hash, created_at, expires_at, last_activity, ip_address, user_agent)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#
        )
        .bind(&token.id)
        .bind(&token.user_id)
        .bind(token_hash)
        .bind(refresh_hash)
        .bind(token.created_at)
        .bind(token.expires_at)
        .bind(token.last_activity)
        .bind(&token.ip_address)
        .bind(&token.user_agent)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to save token: {}", e),
        })?;

        Ok(())
    }

    async fn load_token(&self, token_id: &str) -> Result<SessionToken, SessionError> {
        let row = sqlx::query("SELECT * FROM tokens WHERE id = $1")
            .bind(token_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|_e| SessionError::NotFound {
                id: token_id.to_string(),
            })?;

        Ok(SessionToken {
            id: row.get("id"),
            user_id: row.get("user_id"),
            token: String::new(), // We don't store the actual token
            refresh_token: None,  // We don't store the actual refresh token
            created_at: row.get("created_at"),
            expires_at: row.get("expires_at"),
            last_activity: row.get("last_activity"),
            ip_address: row.get("ip_address"),
            user_agent: row.get("user_agent"),
        })
    }

    async fn delete_token(&self, token_id: &str) -> Result<(), SessionError> {
        sqlx::query("DELETE FROM tokens WHERE id = $1")
            .bind(token_id)
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to delete token: {}", e),
            })?;

        Ok(())
    }

    async fn save_api_key(&self, api_key: &ApiKey) -> Result<(), SessionError> {
        let permissions: Vec<String> = api_key.permissions.clone();
        let rate_limit = serde_json::to_value(&api_key.rate_limit).map_err(|e| {
            SessionError::PersistenceError {
                reason: format!("Failed to serialize rate limit: {}", e),
            }
        })?;

        sqlx::query(
            r#"
            INSERT INTO api_keys (id, name, key_hash, prefix, user_id, permissions, rate_limit, created_at, last_used, expires_at, active)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            "#
        )
        .bind(&api_key.id)
        .bind(&api_key.name)
        .bind(&api_key.key_hash)
        .bind(&api_key.prefix)
        .bind(&api_key.user_id)
        .bind(permissions.as_slice())
        .bind(rate_limit)
        .bind(api_key.created_at)
        .bind(api_key.last_used)
        .bind(api_key.expires_at)
        .bind(api_key.active)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to save API key: {}", e),
        })?;

        Ok(())
    }

    async fn load_api_key(&self, key_id: &str) -> Result<ApiKey, SessionError> {
        let row = sqlx::query("SELECT * FROM api_keys WHERE id = $1")
            .bind(key_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| SessionError::NotFound {
                id: key_id.to_string(),
            })?;

        let permissions: Vec<String> = row.get("permissions");
        let rate_limit: serde_json::Value = row.get("rate_limit");
        let rate_limit = serde_json::from_value(rate_limit).ok();

        Ok(ApiKey {
            id: row.get("id"),
            name: row.get("name"),
            key_hash: row.get("key_hash"),
            prefix: row.get("prefix"),
            user_id: row.get("user_id"),
            permissions,
            rate_limit,
            created_at: row.get("created_at"),
            last_used: row.get("last_used"),
            expires_at: row.get("expires_at"),
            active: row.get("active"),
        })
    }

    async fn delete_api_key(&self, key_id: &str) -> Result<(), SessionError> {
        sqlx::query("DELETE FROM api_keys WHERE id = $1")
            .bind(key_id)
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to delete API key: {}", e),
            })?;

        Ok(())
    }

    async fn save_timeline_event(
        &self,
        session_id: &str,
        event: &TimelineEventData,
    ) -> Result<(), SessionError> {
        sqlx::query(
            r#"
            INSERT INTO timeline_events (id, session_id, event_type, user_id, timestamp, data, branch_id)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            "#
        )
        .bind(&event.id)
        .bind(session_id)
        .bind(&event.event_type)
        .bind(&event.user_id)
        .bind(event.timestamp)
        .bind(&event.data)
        .bind(&event.branch_id)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to save timeline event: {}", e),
        })?;

        Ok(())
    }

    async fn load_timeline_events(
        &self,
        session_id: &str,
        start: i64,
        end: i64,
    ) -> Result<Vec<TimelineEventData>, SessionError> {
        let rows = sqlx::query(
            r#"
            SELECT * FROM timeline_events 
            WHERE session_id = $1 
                AND timestamp >= $2 
                AND timestamp <= $3
            ORDER BY timestamp ASC
            "#,
        )
        .bind(session_id)
        .bind(timestamp_to_datetime(start))
        .bind(timestamp_to_datetime(end))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to load timeline events: {}", e),
        })?;

        let events = rows
            .into_iter()
            .map(|row| TimelineEventData {
                id: row.get("id"),
                session_id: row.get("session_id"),
                event_type: row.get("event_type"),
                user_id: row.get("user_id"),
                timestamp: row.get("timestamp"),
                data: row.get("data"),
                branch_id: row.get("branch_id"),
            })
            .collect();

        Ok(events)
    }

    async fn get_event_count(&self, session_id: &str) -> Result<i64, SessionError> {
        let row =
            sqlx::query("SELECT COUNT(*) as count FROM timeline_events WHERE session_id = $1")
                .bind(session_id)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| SessionError::PersistenceError {
                    reason: format!("Failed to count events: {}", e),
                })?;

        Ok(row.get("count"))
    }
}

/// SQLite implementation (simplified version)
pub struct SqliteDatabase {
    pool: SqlitePool,
}

impl SqliteDatabase {
    pub async fn new(config: &DatabaseConfig) -> Result<Self, SessionError> {
        let pool = SqlitePoolOptions::new()
            .max_connections(config.max_connections)
            .connect(&config.url)
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to connect to SQLite: {}", e),
            })?;

        let db = Self { pool };

        if config.run_migrations {
            db.run_migrations().await?;
        }

        Ok(db)
    }

    async fn run_migrations(&self) -> Result<(), SessionError> {
        // Create sessions table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                name TEXT,
                owner TEXT NOT NULL,
                created_at DATETIME NOT NULL,
                modified_at DATETIME NOT NULL,
                is_public BOOLEAN NOT NULL DEFAULT FALSE,
                data JSON NOT NULL
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to create sessions table: {}", e),
        })?;

        // Create objects table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS objects (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                name TEXT,
                object_type TEXT NOT NULL,
                data JSON NOT NULL,
                created_at DATETIME NOT NULL,
                modified_at DATETIME NOT NULL,
                created_by TEXT NOT NULL,
                locked_by TEXT,
                FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to create objects table: {}", e),
        })?;

        // Create users table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS users (
                id TEXT PRIMARY KEY,
                email TEXT UNIQUE NOT NULL,
                name TEXT NOT NULL,
                password_hash TEXT NOT NULL,
                created_at DATETIME NOT NULL,
                last_login DATETIME,
                is_active BOOLEAN NOT NULL DEFAULT TRUE,
                is_verified BOOLEAN NOT NULL DEFAULT FALSE,
                profile JSON
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to create users table: {}", e),
        })?;

        // Create permissions table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS permissions (
                session_id TEXT NOT NULL,
                user_id TEXT NOT NULL,
                role TEXT NOT NULL,
                explicit_permissions JSON,
                denied_permissions JSON,
                granted_by TEXT NOT NULL,
                updated_at DATETIME NOT NULL,
                PRIMARY KEY (session_id, user_id),
                FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to create permissions table: {}", e),
        })?;

        // Create tokens table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS tokens (
                id TEXT PRIMARY KEY,
                token_hash TEXT UNIQUE NOT NULL,
                user_id TEXT NOT NULL,
                refresh_hash TEXT,
                created_at DATETIME NOT NULL,
                expires_at DATETIME NOT NULL,
                last_activity DATETIME NOT NULL,
                ip_address TEXT,
                user_agent TEXT,
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to create tokens table: {}", e),
        })?;

        // Create api_keys table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS api_keys (
                id TEXT PRIMARY KEY,
                key_hash TEXT UNIQUE NOT NULL,
                prefix TEXT NOT NULL,
                user_id TEXT NOT NULL,
                name TEXT NOT NULL,
                permissions JSON,
                rate_limit JSON,
                created_at DATETIME NOT NULL,
                expires_at DATETIME,
                last_used DATETIME,
                active BOOLEAN NOT NULL DEFAULT 1,
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to create api_keys table: {}", e),
        })?;

        // Create timeline_events table
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS timeline_events (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                event_type TEXT NOT NULL,
                user_id TEXT NOT NULL,
                timestamp DATETIME NOT NULL,
                data JSON NOT NULL,
                branch_id TEXT,
                FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
                FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to create timeline_events table: {}", e),
        })?;

        // Create indices for performance
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_objects_session ON objects(session_id)")
            .execute(&self.pool)
            .await
            .ok();

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_permissions_user ON permissions(user_id)")
            .execute(&self.pool)
            .await
            .ok();

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_tokens_user ON tokens(user_id)")
            .execute(&self.pool)
            .await
            .ok();

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_api_keys_user ON api_keys(user_id)")
            .execute(&self.pool)
            .await
            .ok();

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_timeline_events_session ON timeline_events(session_id, timestamp)")
            .execute(&self.pool)
            .await
            .ok();

        info!("SQLite migrations completed successfully");
        Ok(())
    }
}

// SQLite implementation of DatabasePersistence trait
#[async_trait]
impl DatabasePersistence for SqliteDatabase {
    async fn save_session(&self, session: &SessionState) -> Result<(), SessionError> {
        let data = serde_json::to_value(session).map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to serialize session: {}", e),
        })?;

        sqlx::query(
            r#"
            INSERT INTO sessions (id, name, owner, created_at, modified_at, is_public, data)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                modified_at = excluded.modified_at,
                data = excluded.data
            "#,
        )
        .bind(session.id.to_string())
        .bind(&session.name)
        .bind(
            session
                .active_users
                .first()
                .map(|u| &u.id)
                .unwrap_or(&"system".to_string()),
        )
        .bind(timestamp_to_datetime(session.created_at as i64))
        .bind(timestamp_to_datetime(session.modified_at as i64))
        .bind(false) // Default to private
        .bind(data)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to save session: {}", e),
        })?;

        Ok(())
    }

    async fn load_session(&self, session_id: &str) -> Result<SessionState, SessionError> {
        let row = sqlx::query("SELECT data FROM sessions WHERE id = ?1")
            .bind(session_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| SessionError::NotFound {
                id: session_id.to_string(),
            })?;

        let data: serde_json::Value = row.get("data");
        let session = serde_json::from_value(data).map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to deserialize session: {}", e),
        })?;

        Ok(session)
    }

    async fn delete_session(&self, session_id: &str) -> Result<(), SessionError> {
        sqlx::query("DELETE FROM sessions WHERE id = ?1")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to delete session: {}", e),
            })?;

        Ok(())
    }

    async fn list_sessions(
        &self,
        user_id: Option<&str>,
    ) -> Result<Vec<SessionMetadata>, SessionError> {
        let rows = if let Some(user_id) = user_id {
            // Get sessions where user is owner or has permissions
            sqlx::query(
                r#"
                SELECT DISTINCT s.*, 
                    (SELECT COUNT(*) FROM objects WHERE session_id = s.id) as object_count,
                    (SELECT COUNT(DISTINCT user_id) FROM permissions WHERE session_id = s.id) + 1 as user_count
                FROM sessions s
                LEFT JOIN permissions p ON p.session_id = s.id
                WHERE s.owner = ?1 OR p.user_id = ?1
                ORDER BY s.created_at DESC
                "#
            )
            .bind(user_id)
            .fetch_all(&self.pool)
            .await
        } else {
            // Get all public sessions
            sqlx::query(
                r#"
                SELECT s.*, 
                    (SELECT COUNT(*) FROM objects WHERE session_id = s.id) as object_count,
                    (SELECT COUNT(DISTINCT user_id) FROM permissions WHERE session_id = s.id) + 1 as user_count
                FROM sessions s
                WHERE s.is_public = TRUE
                ORDER BY s.created_at DESC
                "#
            )
            .fetch_all(&self.pool)
            .await
        }
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to list sessions: {}", e),
        })?;

        let sessions = rows
            .into_iter()
            .map(|row| SessionMetadata {
                id: row.get::<String, _>("id"),
                name: row.get("name"),
                owner: row.get("owner"),
                created_at: row.get("created_at"),
                modified_at: row.get("modified_at"),
                object_count: row.get::<i32, _>("object_count"),
                user_count: row.get::<i32, _>("user_count"),
                is_public: row.get("is_public"),
            })
            .collect();

        Ok(sessions)
    }

    async fn save_object(&self, session_id: &str, object: &CADObject) -> Result<(), SessionError> {
        let data = serde_json::to_value(object).map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to serialize object: {}", e),
        })?;

        sqlx::query(
            r#"
            INSERT INTO objects (id, session_id, name, object_type, data, created_at, modified_at, created_by)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                data = excluded.data,
                modified_at = excluded.modified_at
            "#
        )
        .bind(object.id.to_string())
        .bind(session_id)
        .bind(&object.name)
        .bind("unknown") // Object type would be determined from mesh or other properties
        .bind(data)
        .bind(timestamp_to_datetime(object.created_at as i64))
        .bind(timestamp_to_datetime(object.modified_at as i64))
        .bind("system")
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to save object: {}", e),
        })?;

        Ok(())
    }

    async fn load_object(
        &self,
        session_id: &str,
        object_id: &GeometryId,
    ) -> Result<CADObject, SessionError> {
        let row = sqlx::query("SELECT data FROM objects WHERE id = ?1 AND session_id = ?2")
            .bind(&object_id.0)
            .bind(session_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| SessionError::NotFound {
                id: object_id.0.to_string(),
            })?;

        let data: serde_json::Value = row.get("data");
        let object = serde_json::from_value(data).map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to deserialize object: {}", e),
        })?;

        Ok(object)
    }

    async fn delete_object(
        &self,
        session_id: &str,
        object_id: &GeometryId,
    ) -> Result<(), SessionError> {
        sqlx::query("DELETE FROM objects WHERE id = ?1 AND session_id = ?2")
            .bind(&object_id.0)
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to delete object: {}", e),
            })?;

        Ok(())
    }

    async fn list_objects(&self, session_id: &str) -> Result<Vec<ObjectMetadata>, SessionError> {
        let rows =
            sqlx::query("SELECT * FROM objects WHERE session_id = ?1 ORDER BY created_at DESC")
                .bind(session_id)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| SessionError::PersistenceError {
                    reason: format!("Failed to list objects: {}", e),
                })?;

        let objects = rows
            .into_iter()
            .map(|row| ObjectMetadata {
                id: GeometryId(
                    uuid::Uuid::parse_str(&row.get::<String, _>("id"))
                        .unwrap_or_else(|_| uuid::Uuid::new_v4()),
                ),
                name: row.get("name"),
                object_type: row.get("object_type"),
                created_at: row.get("created_at"),
                modified_at: row.get("modified_at"),
                created_by: row.get("created_by"),
                locked_by: row.get("locked_by"),
            })
            .collect();

        Ok(objects)
    }

    async fn save_user(&self, user: &UserData) -> Result<(), SessionError> {
        sqlx::query(
            r#"
            INSERT INTO users (id, email, name, password_hash, created_at, last_login, is_active, is_verified, profile)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            ON CONFLICT(id) DO UPDATE SET
                email = excluded.email,
                name = excluded.name,
                password_hash = excluded.password_hash,
                last_login = excluded.last_login,
                is_active = excluded.is_active,
                is_verified = excluded.is_verified,
                profile = excluded.profile
            "#
        )
        .bind(&user.id)
        .bind(&user.email)
        .bind(&user.name)
        .bind(&user.password_hash)
        .bind(user.created_at)
        .bind(user.last_login)
        .bind(user.is_active)
        .bind(user.is_verified)
        .bind(&user.profile)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to save user: {}", e),
        })?;

        Ok(())
    }

    async fn load_user(&self, user_id: &str) -> Result<UserData, SessionError> {
        let row = sqlx::query("SELECT * FROM users WHERE id = ?1")
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| SessionError::NotFound {
                id: user_id.to_string(),
            })?;

        Ok(UserData {
            id: row.get("id"),
            email: row.get("email"),
            name: row.get("name"),
            password_hash: row.get("password_hash"),
            created_at: row.get("created_at"),
            last_login: row.get("last_login"),
            is_active: row.get("is_active"),
            is_verified: row.get("is_verified"),
            profile: row.get("profile"),
        })
    }

    async fn load_user_by_email(&self, email: &str) -> Result<UserData, SessionError> {
        let row = sqlx::query("SELECT * FROM users WHERE email = ?1")
            .bind(email)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| SessionError::NotFound {
                id: email.to_string(),
            })?;

        Ok(UserData {
            id: row.get("id"),
            email: row.get("email"),
            name: row.get("name"),
            password_hash: row.get("password_hash"),
            created_at: row.get("created_at"),
            last_login: row.get("last_login"),
            is_active: row.get("is_active"),
            is_verified: row.get("is_verified"),
            profile: row.get("profile"),
        })
    }

    async fn update_user(&self, user: &UserData) -> Result<(), SessionError> {
        sqlx::query(
            r#"
            UPDATE users SET
                email = ?2,
                name = ?3,
                password_hash = ?4,
                last_login = ?5,
                is_active = ?6,
                is_verified = ?7,
                profile = ?8
            WHERE id = ?1
            "#,
        )
        .bind(&user.id)
        .bind(&user.email)
        .bind(&user.name)
        .bind(&user.password_hash)
        .bind(user.last_login)
        .bind(user.is_active)
        .bind(user.is_verified)
        .bind(&user.profile)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to update user: {}", e),
        })?;

        Ok(())
    }

    async fn save_permissions(
        &self,
        session_id: &str,
        permissions: &UserPermissions,
    ) -> Result<(), SessionError> {
        let explicit_perms_json =
            serde_json::to_value(&permissions.explicit_permissions).map_err(|e| {
                SessionError::PersistenceError {
                    reason: format!("Failed to serialize explicit permissions: {}", e),
                }
            })?;

        let denied_perms_json =
            serde_json::to_value(&permissions.denied_permissions).map_err(|e| {
                SessionError::PersistenceError {
                    reason: format!("Failed to serialize denied permissions: {}", e),
                }
            })?;

        sqlx::query(
            r#"
            INSERT INTO permissions (session_id, user_id, role, explicit_permissions, denied_permissions, granted_by, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(session_id, user_id) DO UPDATE SET
                role = excluded.role,
                explicit_permissions = excluded.explicit_permissions,
                denied_permissions = excluded.denied_permissions,
                granted_by = excluded.granted_by,
                updated_at = excluded.updated_at
            "#
        )
        .bind(session_id)
        .bind(&permissions.user_id)
        .bind(format!("{:?}", permissions.role))
        .bind(explicit_perms_json)
        .bind(denied_perms_json)
        .bind(&permissions.granted_by)
        .bind(permissions.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to save permissions: {}", e),
        })?;

        Ok(())
    }

    async fn load_permissions(
        &self,
        session_id: &str,
        user_id: &str,
    ) -> Result<UserPermissions, SessionError> {
        let row = sqlx::query("SELECT * FROM permissions WHERE session_id = ?1 AND user_id = ?2")
            .bind(session_id)
            .bind(user_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| SessionError::NotFound {
                id: format!("{}/{}", session_id, user_id),
            })?;

        let role_str: String = row.get("role");
        let role = match role_str.as_str() {
            "Owner" => Role::Owner,
            "Editor" => Role::Editor,
            "Viewer" => Role::Viewer,
            _ => Role::Viewer,
        };

        let explicit_perms_json: serde_json::Value = row.get("explicit_permissions");
        let denied_perms_json: serde_json::Value = row.get("denied_permissions");

        let explicit_permissions = serde_json::from_value(explicit_perms_json)
            .unwrap_or_else(|_| std::collections::HashSet::new());

        let denied_permissions = serde_json::from_value(denied_perms_json)
            .unwrap_or_else(|_| std::collections::HashSet::new());

        Ok(UserPermissions {
            user_id: row.get("user_id"),
            role,
            explicit_permissions,
            denied_permissions,
            updated_at: row.get("updated_at"),
            granted_by: row.get("granted_by"),
        })
    }

    async fn list_permissions(
        &self,
        session_id: &str,
    ) -> Result<Vec<UserPermissions>, SessionError> {
        let rows = sqlx::query("SELECT * FROM permissions WHERE session_id = ?1")
            .bind(session_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to list permissions: {}", e),
            })?;

        let permissions = rows
            .into_iter()
            .map(|row| {
                let role_str: String = row.get("role");
                let role = match role_str.as_str() {
                    "Owner" => Role::Owner,
                    "Editor" => Role::Editor,
                    "Viewer" => Role::Viewer,
                    _ => Role::Viewer,
                };

                let explicit_perms_json: serde_json::Value = row.get("explicit_permissions");
                let denied_perms_json: serde_json::Value = row.get("denied_permissions");

                let explicit_permissions = serde_json::from_value(explicit_perms_json)
                    .unwrap_or_else(|_| std::collections::HashSet::new());

                let denied_permissions = serde_json::from_value(denied_perms_json)
                    .unwrap_or_else(|_| std::collections::HashSet::new());

                UserPermissions {
                    user_id: row.get("user_id"),
                    role,
                    explicit_permissions,
                    denied_permissions,
                    updated_at: row.get("updated_at"),
                    granted_by: row.get("granted_by"),
                }
            })
            .collect();

        Ok(permissions)
    }

    async fn save_token(&self, token: &SessionToken) -> Result<(), SessionError> {
        // Hash tokens before storing
        let mut hasher = sha2::Sha256::new();
        hasher.update(token.token.as_bytes());
        let token_hash = format!("{:x}", hasher.finalize());

        let refresh_hash = token.refresh_token.as_ref().map(|rt| {
            let mut hasher = sha2::Sha256::new();
            hasher.update(rt.as_bytes());
            format!("{:x}", hasher.finalize())
        });

        sqlx::query(
            r#"
            INSERT INTO tokens (id, token_hash, user_id, refresh_hash, created_at, expires_at, last_activity, ip_address, user_agent)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#
        )
        .bind(&token.id)
        .bind(token_hash)
        .bind(&token.user_id)
        .bind(refresh_hash)
        .bind(token.created_at)
        .bind(token.expires_at)
        .bind(token.last_activity)
        .bind(&token.ip_address)
        .bind(&token.user_agent)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to save token: {}", e),
        })?;

        Ok(())
    }

    async fn load_token(&self, token_hash: &str) -> Result<SessionToken, SessionError> {
        let row = sqlx::query("SELECT * FROM tokens WHERE token_hash = ?1")
            .bind(token_hash)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| SessionError::NotFound {
                id: "token".to_string(),
            })?;

        Ok(SessionToken {
            id: row.get("id"),
            user_id: row.get("user_id"),
            token: String::new(), // We don't store the actual token
            refresh_token: None,  // We don't store the actual refresh token
            created_at: row.get("created_at"),
            expires_at: row.get("expires_at"),
            last_activity: row.get("last_activity"),
            ip_address: row.get("ip_address"),
            user_agent: row.get("user_agent"),
        })
    }

    async fn delete_token(&self, token_hash: &str) -> Result<(), SessionError> {
        sqlx::query("DELETE FROM tokens WHERE token_hash = ?1")
            .bind(token_hash)
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to delete token: {}", e),
            })?;

        Ok(())
    }

    async fn save_api_key(&self, key: &ApiKey) -> Result<(), SessionError> {
        let permissions_json =
            serde_json::to_value(&key.permissions).map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to serialize permissions: {}", e),
            })?;

        let rate_limit_json =
            serde_json::to_value(&key.rate_limit).map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to serialize rate limit: {}", e),
            })?;

        sqlx::query(
            r#"
            INSERT INTO api_keys (id, key_hash, prefix, user_id, name, permissions, rate_limit, created_at, expires_at, last_used, active)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            "#
        )
        .bind(&key.id)
        .bind(&key.key_hash)
        .bind(&key.prefix)
        .bind(&key.user_id)
        .bind(&key.name)
        .bind(permissions_json)
        .bind(rate_limit_json)
        .bind(key.created_at)
        .bind(key.expires_at)
        .bind(key.last_used)
        .bind(key.active)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to save API key: {}", e),
        })?;

        Ok(())
    }

    async fn load_api_key(&self, key_hash: &str) -> Result<ApiKey, SessionError> {
        let row = sqlx::query("SELECT * FROM api_keys WHERE key_hash = ?1")
            .bind(key_hash)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| SessionError::NotFound {
                id: "api_key".to_string(),
            })?;

        let permissions_json: serde_json::Value = row.get("permissions");
        let permissions = serde_json::from_value(permissions_json).unwrap_or_else(|_| vec![]);

        Ok(ApiKey {
            id: row.get("id"),
            name: row.get("name"),
            key_hash: row.get("key_hash"),
            prefix: String::new(), // Would need to be stored or regenerated
            user_id: row.get("user_id"),
            permissions,
            rate_limit: None, // Not stored in SQLite version
            created_at: row.get("created_at"),
            last_used: row.get("last_used"),
            expires_at: row.get("expires_at"),
            active: true, // Default to active
        })
    }

    async fn delete_api_key(&self, key_id: &str) -> Result<(), SessionError> {
        sqlx::query("DELETE FROM api_keys WHERE id = ?1")
            .bind(key_id)
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to delete API key: {}", e),
            })?;

        Ok(())
    }

    async fn save_timeline_event(
        &self,
        session_id: &str,
        event: &TimelineEventData,
    ) -> Result<(), SessionError> {
        sqlx::query(
            r#"
            INSERT INTO timeline_events (id, session_id, event_type, user_id, timestamp, data, branch_id)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#
        )
        .bind(&event.id)
        .bind(&event.session_id)
        .bind(&event.event_type)
        .bind(&event.user_id)
        .bind(event.timestamp)
        .bind(&event.data)
        .bind(&event.branch_id)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to save timeline event: {}", e),
        })?;

        Ok(())
    }

    async fn load_timeline_events(
        &self,
        session_id: &str,
        start: i64,
        end: i64,
    ) -> Result<Vec<TimelineEventData>, SessionError> {
        let rows = sqlx::query(
            r#"
            SELECT * FROM timeline_events 
            WHERE session_id = ?1 
            AND timestamp >= ?2 
            AND timestamp <= ?3
            ORDER BY timestamp ASC
            "#,
        )
        .bind(session_id)
        .bind(timestamp_to_datetime(start))
        .bind(timestamp_to_datetime(end))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| SessionError::PersistenceError {
            reason: format!("Failed to load timeline events: {}", e),
        })?;

        let events = rows
            .into_iter()
            .map(|row| TimelineEventData {
                id: row.get("id"),
                session_id: row.get("session_id"),
                event_type: row.get("event_type"),
                user_id: row.get("user_id"),
                timestamp: row.get("timestamp"),
                data: row.get("data"),
                branch_id: row.get("branch_id"),
            })
            .collect();

        Ok(events)
    }

    async fn get_event_count(&self, session_id: &str) -> Result<i64, SessionError> {
        let row =
            sqlx::query("SELECT COUNT(*) as count FROM timeline_events WHERE session_id = ?1")
                .bind(session_id)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| SessionError::PersistenceError {
                    reason: format!("Failed to count timeline events: {}", e),
                })?;

        Ok(row.get("count"))
    }
}

/// Create database instance based on config
pub async fn create_database(
    config: &DatabaseConfig,
) -> Result<Arc<dyn DatabasePersistence>, SessionError> {
    match config.db_type {
        DatabaseType::PostgreSQL => {
            let db = PostgresDatabase::new(config).await?;
            Ok(Arc::new(db))
        }
        DatabaseType::SQLite => {
            let db = SqliteDatabase::new(config).await?;
            Ok(Arc::new(db) as Arc<dyn DatabasePersistence>)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_database_operations() {
        // Tests would go here
    }
}
