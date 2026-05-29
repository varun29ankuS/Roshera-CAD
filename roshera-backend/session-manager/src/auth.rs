//! Authentication and security features for session management
//!
//! This module provides JWT-based authentication, API key management,
//! and security features for the CAD system.

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use chrono::{DateTime, Duration, Utc};
use dashmap::DashMap;
use hmac::{Hmac, Mac};
use jwt::{SignWithKey, VerifyWithKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use shared_types::SessionError;
use std::sync::Arc;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

/// Authentication token claims
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenClaims {
    /// User ID
    pub sub: String,
    /// Token ID
    pub jti: String,
    /// Issued at
    pub iat: i64,
    /// Expiration
    pub exp: i64,
    /// Not before
    pub nbf: i64,
    /// Issuer
    pub iss: String,
    /// Audience
    pub aud: Vec<String>,
    /// User email
    pub email: Option<String>,
    /// User roles
    pub roles: Vec<String>,
    /// Custom claims
    pub custom: serde_json::Value,
}

/// API key information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    /// Key ID
    pub id: String,
    /// Key name
    pub name: String,
    /// Hashed key value
    pub key_hash: String,
    /// Key prefix (for display)
    pub prefix: String,
    /// Owner user ID
    pub user_id: String,
    /// Permissions
    pub permissions: Vec<String>,
    /// Rate limit
    pub rate_limit: Option<RateLimit>,
    /// Created at
    pub created_at: DateTime<Utc>,
    /// Last used
    pub last_used: Option<DateTime<Utc>>,
    /// Expires at
    pub expires_at: Option<DateTime<Utc>>,
    /// Is active
    pub active: bool,
}

/// Rate limiting configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimit {
    /// Requests per window
    pub requests: u32,
    /// Window duration in seconds
    pub window_seconds: u64,
}

/// Session token
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionToken {
    /// Token ID
    pub id: String,
    /// User ID
    pub user_id: String,
    /// Token value
    pub token: String,
    /// Refresh token
    pub refresh_token: Option<String>,
    /// Created at
    pub created_at: DateTime<Utc>,
    /// Expires at
    pub expires_at: DateTime<Utc>,
    /// Last activity
    pub last_activity: DateTime<Utc>,
    /// IP address
    pub ip_address: Option<String>,
    /// User agent
    pub user_agent: Option<String>,
}

/// Two-factor authentication info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwoFactorAuth {
    /// User ID
    pub user_id: String,
    /// Secret key (encrypted)
    pub secret: String,
    /// Backup codes (hashed)
    pub backup_codes: Vec<String>,
    /// Is enabled
    pub enabled: bool,
    /// Last used
    pub last_used: Option<DateTime<Utc>>,
}

/// Security event types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SecurityEvent {
    /// Login attempt
    LoginAttempt {
        user_id: String,
        success: bool,
        ip: Option<String>,
        user_agent: Option<String>,
    },
    /// Token created
    TokenCreated {
        user_id: String,
        token_id: String,
        token_type: String,
    },
    /// Token revoked
    TokenRevoked {
        user_id: String,
        token_id: String,
        reason: String,
    },
    /// Permission changed
    PermissionChanged {
        user_id: String,
        changed_by: String,
        changes: serde_json::Value,
    },
    /// Suspicious activity
    SuspiciousActivity {
        user_id: Option<String>,
        activity_type: String,
        details: String,
    },
    /// API key created
    ApiKeyCreated { user_id: String, key_id: String },
    /// API key revoked
    ApiKeyRevoked {
        user_id: String,
        key_id: String,
        reason: String,
    },
}

/// Authentication manager
pub struct AuthManager {
    /// JWT signing key
    jwt_secret: Arc<HmacSha256>,
    /// Active tokens
    tokens: Arc<DashMap<String, SessionToken>>,
    /// API keys
    api_keys: Arc<DashMap<String, ApiKey>>,
    /// Two-factor auth data
    two_factor: Arc<DashMap<String, TwoFactorAuth>>,
    /// Security events
    security_events: Arc<DashMap<String, Vec<SecurityEvent>>>,
    /// Revoked tokens
    revoked_tokens: Arc<DashMap<String, DateTime<Utc>>>,
    /// Failed login attempts
    failed_attempts: Arc<DashMap<String, Vec<DateTime<Utc>>>>,
    /// Rate limiting tracking
    rate_limits: Arc<DashMap<String, Vec<DateTime<Utc>>>>,
    /// Configuration
    config: AuthConfig,
}

/// Authentication configuration
#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// JWT issuer
    pub issuer: String,
    /// JWT audience
    pub audience: Vec<String>,
    /// Token expiration in seconds (absolute lifetime from issuance).
    pub token_expiry_seconds: i64,
    /// Refresh token expiration in seconds.
    pub refresh_expiry_seconds: i64,
    /// Idle timeout in seconds (AUDIT-H8). A token whose `last_activity`
    /// is older than this is rejected by `verify_token` even when the
    /// JWT `exp` claim has not yet elapsed, and the cached session-
    /// token entry is dropped so a subsequent activity probe cannot
    /// silently revive it. The default of 30 minutes mirrors the
    /// industry-standard idle-session policy for web sessions; tune
    /// via `ROSHERA_IDLE_TIMEOUT_SECONDS` at the api-server boundary
    /// when an operator needs a different policy. Set to `0` to
    /// disable idle enforcement entirely (the absolute JWT `exp`
    /// gate still applies).
    pub idle_timeout_seconds: i64,
    /// Maximum failed login attempts
    pub max_failed_attempts: u32,
    /// Lockout duration in seconds
    pub lockout_duration_seconds: i64,
    /// Require 2FA for sensitive operations
    pub require_2fa_for_sensitive: bool,
    /// API key prefix
    pub api_key_prefix: String,
    /// Password requirements
    pub password_requirements: PasswordRequirements,
}

/// Password requirements
#[derive(Debug, Clone)]
pub struct PasswordRequirements {
    /// Minimum length
    pub min_length: usize,
    /// Require uppercase
    pub require_uppercase: bool,
    /// Require lowercase
    pub require_lowercase: bool,
    /// Require numbers
    pub require_numbers: bool,
    /// Require special characters
    pub require_special: bool,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            issuer: "roshera-cad".to_string(),
            audience: vec!["roshera-api".to_string()],
            token_expiry_seconds: 3600,     // 1 hour
            refresh_expiry_seconds: 604800, // 7 days
            idle_timeout_seconds: 1800,     // 30 minutes (AUDIT-H8)
            max_failed_attempts: 5,
            lockout_duration_seconds: 900, // 15 minutes
            require_2fa_for_sensitive: true,
            api_key_prefix: "rosh_".to_string(),
            password_requirements: PasswordRequirements {
                min_length: 8,
                require_uppercase: true,
                require_lowercase: true,
                require_numbers: true,
                require_special: true,
            },
        }
    }
}

impl AuthConfig {
    /// Build [`AuthConfig`] by reading the documented `ROSHERA_*`
    /// environment variables, falling back to [`AuthConfig::default`]
    /// for any unset or unparseable value (AUDIT-M5).
    ///
    /// # Environment variables
    ///
    /// | Variable                              | Field                                       | Default          |
    /// |---------------------------------------|---------------------------------------------|------------------|
    /// | `ROSHERA_AUTH_ISSUER`                 | `issuer`                                    | `"roshera-cad"`  |
    /// | `ROSHERA_AUTH_AUDIENCE` (comma-list)  | `audience`                                  | `["roshera-api"]`|
    /// | `ROSHERA_TOKEN_EXPIRY_SECONDS`        | `token_expiry_seconds`                      | `3600`           |
    /// | `ROSHERA_REFRESH_EXPIRY_SECONDS`      | `refresh_expiry_seconds`                    | `604800`         |
    /// | `ROSHERA_IDLE_TIMEOUT_SECONDS`        | `idle_timeout_seconds`                      | `1800`           |
    /// | `ROSHERA_MAX_FAILED_ATTEMPTS`         | `max_failed_attempts`                       | `5`              |
    /// | `ROSHERA_LOCKOUT_DURATION_SECONDS`    | `lockout_duration_seconds`                  | `900`            |
    /// | `ROSHERA_REQUIRE_2FA_SENSITIVE`       | `require_2fa_for_sensitive`                 | `true`           |
    /// | `ROSHERA_API_KEY_PREFIX`              | `api_key_prefix`                            | `"rosh_"`        |
    /// | `ROSHERA_PASSWORD_MIN_LENGTH`         | `password_requirements.min_length`          | `8`              |
    /// | `ROSHERA_PASSWORD_REQUIRE_UPPERCASE`  | `password_requirements.require_uppercase`   | `true`           |
    /// | `ROSHERA_PASSWORD_REQUIRE_LOWERCASE`  | `password_requirements.require_lowercase`   | `true`           |
    /// | `ROSHERA_PASSWORD_REQUIRE_NUMBERS`    | `password_requirements.require_numbers`     | `true`           |
    /// | `ROSHERA_PASSWORD_REQUIRE_SPECIAL`    | `password_requirements.require_special`     | `true`           |
    ///
    /// Booleans parse `"true"`/`"false"`/`"1"`/`"0"`/`"yes"`/`"no"`
    /// case-insensitively. Anything else falls back to the default.
    /// Integer fields fall back to the default on parse failure rather
    /// than panicking — startup must succeed even with a typoed env.
    pub fn from_env() -> Self {
        Self::from_env_with(|k| std::env::var(k).ok())
    }

    /// Testing seam for [`AuthConfig::from_env`]. Caller supplies the
    /// environment-getter closure so tests can drive deterministic
    /// values without racing on real-process env mutation.
    pub(crate) fn from_env_with<F>(get: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let default = Self::default();
        Self {
            issuer: env_string(&get, "ROSHERA_AUTH_ISSUER", default.issuer),
            audience: env_csv(&get, "ROSHERA_AUTH_AUDIENCE", default.audience),
            token_expiry_seconds: env_i64(
                &get,
                "ROSHERA_TOKEN_EXPIRY_SECONDS",
                default.token_expiry_seconds,
            ),
            refresh_expiry_seconds: env_i64(
                &get,
                "ROSHERA_REFRESH_EXPIRY_SECONDS",
                default.refresh_expiry_seconds,
            ),
            idle_timeout_seconds: env_i64(
                &get,
                "ROSHERA_IDLE_TIMEOUT_SECONDS",
                default.idle_timeout_seconds,
            ),
            max_failed_attempts: env_u32(
                &get,
                "ROSHERA_MAX_FAILED_ATTEMPTS",
                default.max_failed_attempts,
            ),
            lockout_duration_seconds: env_i64(
                &get,
                "ROSHERA_LOCKOUT_DURATION_SECONDS",
                default.lockout_duration_seconds,
            ),
            require_2fa_for_sensitive: env_bool(
                &get,
                "ROSHERA_REQUIRE_2FA_SENSITIVE",
                default.require_2fa_for_sensitive,
            ),
            api_key_prefix: env_string(&get, "ROSHERA_API_KEY_PREFIX", default.api_key_prefix),
            password_requirements: PasswordRequirements {
                min_length: env_usize(
                    &get,
                    "ROSHERA_PASSWORD_MIN_LENGTH",
                    default.password_requirements.min_length,
                ),
                require_uppercase: env_bool(
                    &get,
                    "ROSHERA_PASSWORD_REQUIRE_UPPERCASE",
                    default.password_requirements.require_uppercase,
                ),
                require_lowercase: env_bool(
                    &get,
                    "ROSHERA_PASSWORD_REQUIRE_LOWERCASE",
                    default.password_requirements.require_lowercase,
                ),
                require_numbers: env_bool(
                    &get,
                    "ROSHERA_PASSWORD_REQUIRE_NUMBERS",
                    default.password_requirements.require_numbers,
                ),
                require_special: env_bool(
                    &get,
                    "ROSHERA_PASSWORD_REQUIRE_SPECIAL",
                    default.password_requirements.require_special,
                ),
            },
        }
    }
}

fn env_string<F: Fn(&str) -> Option<String>>(get: &F, var: &str, default: String) -> String {
    get(var).filter(|s| !s.is_empty()).unwrap_or(default)
}

fn env_csv<F: Fn(&str) -> Option<String>>(get: &F, var: &str, default: Vec<String>) -> Vec<String> {
    match get(var) {
        Some(raw) => {
            let parsed: Vec<String> = raw
                .split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect();
            if parsed.is_empty() {
                default
            } else {
                parsed
            }
        }
        None => default,
    }
}

fn env_i64<F: Fn(&str) -> Option<String>>(get: &F, var: &str, default: i64) -> i64 {
    get(var).and_then(|s| s.parse().ok()).unwrap_or(default)
}

fn env_u32<F: Fn(&str) -> Option<String>>(get: &F, var: &str, default: u32) -> u32 {
    get(var).and_then(|s| s.parse().ok()).unwrap_or(default)
}

fn env_usize<F: Fn(&str) -> Option<String>>(get: &F, var: &str, default: usize) -> usize {
    get(var).and_then(|s| s.parse().ok()).unwrap_or(default)
}

fn env_bool<F: Fn(&str) -> Option<String>>(get: &F, var: &str, default: bool) -> bool {
    get(var)
        .and_then(|s| match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Some(true),
            "false" | "0" | "no" => Some(false),
            _ => None,
        })
        .unwrap_or(default)
}

impl AuthManager {
    /// Create new auth manager.
    ///
    /// Returns `SessionError::InvalidInput` if `jwt_secret` is empty
    /// or otherwise rejected by the HMAC-SHA256 key constructor. Up
    /// until now this case panicked at startup with `expect("Invalid
    /// JWT secret")`; making it a typed error lets the API server
    /// surface a configuration problem to the operator instead of
    /// crashing the process.
    pub fn new(config: AuthConfig, jwt_secret: &str) -> Result<Self, SessionError> {
        if jwt_secret.is_empty() {
            return Err(SessionError::InvalidInput {
                field: "jwt_secret must not be empty".to_string(),
            });
        }
        let jwt_secret = HmacSha256::new_from_slice(jwt_secret.as_bytes()).map_err(|e| {
            SessionError::InvalidInput {
                field: format!("jwt_secret rejected by HMAC-SHA256: {e}"),
            }
        })?;

        Ok(Self {
            jwt_secret: Arc::new(jwt_secret),
            tokens: Arc::new(DashMap::new()),
            api_keys: Arc::new(DashMap::new()),
            two_factor: Arc::new(DashMap::new()),
            security_events: Arc::new(DashMap::new()),
            revoked_tokens: Arc::new(DashMap::new()),
            failed_attempts: Arc::new(DashMap::new()),
            rate_limits: Arc::new(DashMap::new()),
            config,
        })
    }

    /// Hash password using Argon2
    pub fn hash_password(&self, password: &str) -> Result<String, SessionError> {
        // Validate password
        self.validate_password(password)?;

        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();

        let password_hash = argon2
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to hash password: {}", e),
            })?
            .to_string();

        Ok(password_hash)
    }

    /// Verify password
    pub fn verify_password(&self, password: &str, hash: &str) -> Result<bool, SessionError> {
        let parsed_hash = PasswordHash::new(hash).map_err(|e| SessionError::PersistenceError {
            reason: format!("Invalid password hash: {}", e),
        })?;

        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .is_ok())
    }

    /// Validate password against requirements
    fn validate_password(&self, password: &str) -> Result<(), SessionError> {
        let req = &self.config.password_requirements;

        if password.len() < req.min_length {
            return Err(SessionError::InvalidInput {
                field: format!("Password must be at least {} characters", req.min_length),
            });
        }

        if req.require_uppercase && !password.chars().any(|c| c.is_uppercase()) {
            return Err(SessionError::InvalidInput {
                field: "Password must contain uppercase letter".to_string(),
            });
        }

        if req.require_lowercase && !password.chars().any(|c| c.is_lowercase()) {
            return Err(SessionError::InvalidInput {
                field: "Password must contain lowercase letter".to_string(),
            });
        }

        if req.require_numbers && !password.chars().any(|c| c.is_numeric()) {
            return Err(SessionError::InvalidInput {
                field: "Password must contain number".to_string(),
            });
        }

        if req.require_special && !password.chars().any(|c| !c.is_alphanumeric()) {
            return Err(SessionError::InvalidInput {
                field: "Password must contain special character".to_string(),
            });
        }

        Ok(())
    }

    /// Create JWT token
    pub fn create_token(
        &self,
        user_id: &str,
        email: Option<String>,
        roles: Vec<String>,
    ) -> Result<SessionToken, SessionError> {
        let now = Utc::now();
        let token_id = Uuid::new_v4().to_string();

        let claims = TokenClaims {
            sub: user_id.to_string(),
            jti: token_id.clone(),
            iat: now.timestamp(),
            exp: (now + Duration::seconds(self.config.token_expiry_seconds)).timestamp(),
            nbf: now.timestamp(),
            iss: self.config.issuer.clone(),
            aud: self.config.audience.clone(),
            email,
            roles,
            custom: serde_json::Value::Object(serde_json::Map::new()),
        };

        let token = claims.sign_with_key(&*self.jwt_secret).map_err(|e| {
            SessionError::PersistenceError {
                reason: format!("Failed to create token: {}", e),
            }
        })?;

        // Create refresh token
        let refresh_claims = TokenClaims {
            sub: user_id.to_string(),
            jti: Uuid::new_v4().to_string(),
            iat: now.timestamp(),
            exp: (now + Duration::seconds(self.config.refresh_expiry_seconds)).timestamp(),
            nbf: now.timestamp(),
            iss: self.config.issuer.clone(),
            aud: vec!["refresh".to_string()],
            email: None,
            roles: vec![],
            custom: serde_json::json!({ "parent_jti": token_id }),
        };

        let refresh_token = refresh_claims
            .sign_with_key(&*self.jwt_secret)
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Failed to create refresh token: {}", e),
            })?;

        let session_token = SessionToken {
            id: token_id.clone(),
            user_id: user_id.to_string(),
            token: token.clone(),
            refresh_token: Some(refresh_token),
            created_at: now,
            expires_at: now + Duration::seconds(self.config.token_expiry_seconds),
            last_activity: now,
            ip_address: None,
            user_agent: None,
        };

        // Store token
        self.tokens.insert(token_id.clone(), session_token.clone());

        // Log security event
        self.log_security_event(SecurityEvent::TokenCreated {
            user_id: user_id.to_string(),
            token_id,
            token_type: "access".to_string(),
        });

        Ok(session_token)
    }

    /// Verify JWT token.
    ///
    /// Checks (in order):
    ///
    /// 1. JWT signature against the server's secret.
    /// 2. `revoked_tokens` (AUDIT-C9).
    /// 3. Absolute expiry from the JWT `exp` claim.
    /// 4. **Idle timeout** (AUDIT-H8). When `idle_timeout_seconds > 0`
    ///    and a cached `SessionToken` exists for this `jti`, reject
    ///    the token if `last_activity` is older than the configured
    ///    idle window and drop the cached entry so it cannot be
    ///    silently re-armed by a later probe. The JWT itself remains
    ///    structurally valid until its `exp` elapses, which is why
    ///    the cached drop is the load-bearing step here — without
    ///    it, an attacker who captured the token could still use it
    ///    after a re-issue if the `tokens` map was repopulated by
    ///    any path that didn't go through `create_token`. No
    ///    `SessionToken` entry (e.g. the manager was restarted but
    ///    the JWT is still within `exp`) falls through to absolute-
    ///    expiry enforcement only.
    pub fn verify_token(&self, token: &str) -> Result<TokenClaims, SessionError> {
        // Check if token is revoked
        let claims: TokenClaims = token
            .verify_with_key(&*self.jwt_secret)
            .map_err(|_e| SessionError::AccessDenied)?;

        if self.revoked_tokens.contains_key(&claims.jti) {
            return Err(SessionError::AccessDenied);
        }

        // Verify expiration (absolute lifetime)
        let now_ts = Utc::now().timestamp();
        if claims.exp < now_ts {
            return Err(SessionError::Expired { id: claims.jti });
        }

        // AUDIT-H8: idle-timeout enforcement. `idle_timeout_seconds == 0`
        // is the documented opt-out for operators that intentionally
        // run with long-lived tokens (e.g. an automation key that
        // makes a single call per day). All other values enforce.
        let idle_budget = self.config.idle_timeout_seconds;
        let now = Utc::now();
        if idle_budget > 0 {
            // Cache the decision so we can drop the entry *after*
            // releasing the read guard. DashMap's `get_mut` would
            // upgrade the lock; we need the simpler split to avoid
            // holding it across the remove.
            let stale = self
                .tokens
                .get(&claims.jti)
                .map(|entry| (now - entry.last_activity).num_seconds() > idle_budget)
                .unwrap_or(false);
            if stale {
                self.tokens.remove(&claims.jti);
                return Err(SessionError::Expired { id: claims.jti });
            }
        }

        // Update last activity
        if let Some(mut session_token) = self.tokens.get_mut(&claims.jti) {
            session_token.last_activity = now;
        }

        Ok(claims)
    }

    /// Revoke token
    pub fn revoke_token(&self, token_id: &str, reason: &str, revoked_by: &str) {
        self.revoked_tokens.insert(token_id.to_string(), Utc::now());
        self.tokens.remove(token_id);

        self.log_security_event(SecurityEvent::TokenRevoked {
            user_id: revoked_by.to_string(),
            token_id: token_id.to_string(),
            reason: reason.to_string(),
        });
    }

    /// Create API key
    pub fn create_api_key(
        &self,
        user_id: &str,
        name: &str,
        permissions: Vec<String>,
        expires_in_days: Option<i64>,
    ) -> Result<(String, ApiKey), SessionError> {
        let key_id = Uuid::new_v4().to_string();
        let raw_key = format!("{}{}", self.config.api_key_prefix, Uuid::new_v4());

        // Hash the key
        let mut hasher = Sha256::new();
        hasher.update(raw_key.as_bytes());
        let key_hash = format!("{:x}", hasher.finalize());

        let now = Utc::now();
        let expires_at = expires_in_days.map(|days| now + Duration::days(days));

        let api_key = ApiKey {
            id: key_id.clone(),
            name: name.to_string(),
            key_hash,
            prefix: raw_key[..8].to_string(),
            user_id: user_id.to_string(),
            permissions,
            rate_limit: Some(RateLimit {
                requests: 1000,
                window_seconds: 3600,
            }),
            created_at: now,
            last_used: None,
            expires_at,
            active: true,
        };

        self.api_keys.insert(key_id.clone(), api_key.clone());

        self.log_security_event(SecurityEvent::ApiKeyCreated {
            user_id: user_id.to_string(),
            key_id,
        });

        Ok((raw_key, api_key))
    }

    /// Verify API key
    pub fn verify_api_key(&self, raw_key: &str) -> Result<ApiKey, SessionError> {
        // Hash the provided key
        let mut hasher = Sha256::new();
        hasher.update(raw_key.as_bytes());
        let key_hash = format!("{:x}", hasher.finalize());

        // Find matching key
        for entry in self.api_keys.iter() {
            let api_key = entry.value();

            if api_key.key_hash == key_hash {
                // Check if active
                if !api_key.active {
                    return Err(SessionError::AccessDenied);
                }

                // Check expiration
                if let Some(expires_at) = api_key.expires_at {
                    if expires_at < Utc::now() {
                        return Err(SessionError::Expired {
                            id: api_key.id.clone(),
                        });
                    }
                }

                // AUDIT-H9: enforce the per-key configured rate limit
                // *before* marking the key as used. If the window is
                // exhausted, reject without recording last_used or
                // bumping the request counter — otherwise a hammering
                // caller could keep extending the window out from
                // under itself. The check is keyed by the API key's
                // UUID, so it scopes per credential, not per client IP.
                self.enforce_api_key_rate_limit(api_key)?;

                // Update last used
                drop(api_key);
                if let Some(mut key) = self.api_keys.get_mut(&entry.key().clone()) {
                    key.last_used = Some(Utc::now());
                }

                return Ok(entry.value().clone());
            }
        }

        Err(SessionError::AccessDenied)
    }

    /// Record login attempt
    pub fn record_login_attempt(
        &self,
        user_id: &str,
        success: bool,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Result<(), SessionError> {
        self.log_security_event(SecurityEvent::LoginAttempt {
            user_id: user_id.to_string(),
            success,
            ip: ip.clone(),
            user_agent,
        });

        if !success {
            // Track failed attempts
            let mut attempts = self
                .failed_attempts
                .entry(user_id.to_string())
                .or_insert_with(Vec::new);

            let now = Utc::now();
            attempts.push(now);

            // Remove old attempts
            let cutoff = now - Duration::seconds(self.config.lockout_duration_seconds);
            attempts.retain(|&t| t > cutoff);

            // Check lockout
            if attempts.len() >= self.config.max_failed_attempts as usize {
                return Err(SessionError::ConflictError {
                    details: format!(
                        "Account locked due to {} failed login attempts",
                        attempts.len()
                    ),
                });
            }
        } else {
            // Clear failed attempts on success
            self.failed_attempts.remove(user_id);
        }

        Ok(())
    }

    /// Check if user is locked out
    pub fn is_locked_out(&self, user_id: &str) -> bool {
        if let Some(attempts) = self.failed_attempts.get(user_id) {
            let now = Utc::now();
            let cutoff = now - Duration::seconds(self.config.lockout_duration_seconds);
            let recent_attempts: Vec<_> = attempts.iter().filter(|&&t| t > cutoff).collect();

            recent_attempts.len() >= self.config.max_failed_attempts as usize
        } else {
            false
        }
    }

    /// Enable 2FA for user
    pub fn enable_2fa(&self, user_id: &str) -> Result<(String, Vec<String>), SessionError> {
        // Generate secret
        let secret = base32::encode(
            base32::Alphabet::RFC4648 { padding: false },
            Uuid::new_v4().as_bytes(),
        );

        // Generate backup codes
        let backup_codes: Vec<String> = (0..8)
            .map(|_| {
                let code = format!("{:08}", rand::random::<u32>() % 100_000_000);
                code
            })
            .collect();

        // Hash backup codes
        let hashed_codes: Vec<String> = backup_codes
            .iter()
            .map(|code| {
                let mut hasher = Sha256::new();
                hasher.update(code.as_bytes());
                format!("{:x}", hasher.finalize())
            })
            .collect();

        let two_factor = TwoFactorAuth {
            user_id: user_id.to_string(),
            secret: secret.clone(),
            backup_codes: hashed_codes,
            enabled: true,
            last_used: None,
        };

        self.two_factor.insert(user_id.to_string(), two_factor);

        Ok((secret, backup_codes))
    }

    /// Verify 2FA code
    pub fn verify_2fa(&self, user_id: &str, code: &str) -> Result<bool, SessionError> {
        use totp_lite::{totp, Sha1};

        let two_factor = self
            .two_factor
            .get(user_id)
            .ok_or_else(|| SessionError::NotFound {
                id: user_id.to_string(),
            })?;

        if !two_factor.enabled {
            return Ok(true); // 2FA not enabled
        }

        // Check if it's a backup code
        let mut hasher = Sha256::new();
        hasher.update(code.as_bytes());
        let code_hash = format!("{:x}", hasher.finalize());

        if two_factor.backup_codes.contains(&code_hash) {
            // Remove used backup code
            drop(two_factor);
            if let Some(mut two_factor) = self.two_factor.get_mut(user_id) {
                two_factor.backup_codes.retain(|h| h != &code_hash);
                two_factor.last_used = Some(Utc::now());
            }
            return Ok(true);
        }

        // Verify TOTP code. Fall back to `0` if the wall clock is pre-UNIX_EPOCH;
        // this merely causes the TOTP window to mismatch rather than panicking.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let expected = totp::<Sha1>(&two_factor.secret.as_bytes(), now / 30);

        if code == expected {
            drop(two_factor);
            if let Some(mut two_factor) = self.two_factor.get_mut(user_id) {
                two_factor.last_used = Some(Utc::now());
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Log security event
    fn log_security_event(&self, event: SecurityEvent) {
        let user_id = match &event {
            SecurityEvent::LoginAttempt { user_id, .. } => user_id.clone(),
            SecurityEvent::TokenCreated { user_id, .. } => user_id.clone(),
            SecurityEvent::TokenRevoked { user_id, .. } => user_id.clone(),
            SecurityEvent::PermissionChanged { user_id, .. } => user_id.clone(),
            SecurityEvent::SuspiciousActivity { user_id, .. } => {
                user_id.clone().unwrap_or_else(|| "unknown".to_string())
            }
            SecurityEvent::ApiKeyCreated { user_id, .. } => user_id.clone(),
            SecurityEvent::ApiKeyRevoked { user_id, .. } => user_id.clone(),
        };

        self.security_events
            .entry(user_id)
            .or_insert_with(Vec::new)
            .push(event);
    }

    /// Get security events for user
    pub fn get_security_events(&self, user_id: &str, limit: Option<usize>) -> Vec<SecurityEvent> {
        self.security_events
            .get(user_id)
            .map(|events| {
                let events = events.value();
                if let Some(limit) = limit {
                    events.iter().rev().take(limit).cloned().collect()
                } else {
                    events.clone()
                }
            })
            .unwrap_or_default()
    }

    /// Clean up expired tokens
    pub async fn cleanup_expired(&self) {
        let now = Utc::now();

        // Remove expired tokens
        self.tokens.retain(|_, token| token.expires_at > now);

        // Remove old revoked tokens (keep for 7 days)
        let cutoff = now - Duration::days(7);
        self.revoked_tokens
            .retain(|_, revoked_at| *revoked_at > cutoff);

        // Remove old failed attempts
        let attempt_cutoff = now - Duration::seconds(self.config.lockout_duration_seconds);
        for mut entry in self.failed_attempts.iter_mut() {
            entry.value_mut().retain(|&t| t > attempt_cutoff);
        }
        self.failed_attempts
            .retain(|_, attempts| !attempts.is_empty());
    }

    /// Enforce the per-API-key rate limit configured on `api_key`.
    ///
    /// Sliding-window counter over `rate_limit.window_seconds`, sharing
    /// the `rate_limits` DashMap with the IP-scoped
    /// [`check_rate_limit`](Self::check_rate_limit). Bucket key is the
    /// API key's UUID, which cannot collide with IP-style client IDs
    /// in practice (UUIDs are not valid `IpAddr` strings).
    ///
    /// The prune + count + decide + record sequence runs inside a
    /// single `DashMap::entry` write guard, eliminating the TOCTOU
    /// window between "check count" and "record request" that the
    /// legacy `check_rate_limit` path carries.
    ///
    /// When `api_key.rate_limit` is `None` (legacy persisted keys
    /// minted before AUDIT-H9), the limit is treated as unbounded
    /// and the method returns `Ok(())` without recording. Keys
    /// produced by [`create_api_key`](Self::create_api_key) always
    /// receive the 1000 req/h default, so this branch is dead in
    /// production after a key rotation.
    fn enforce_api_key_rate_limit(&self, api_key: &ApiKey) -> Result<(), SessionError> {
        let rate_limit = match &api_key.rate_limit {
            Some(rl) => rl,
            None => return Ok(()),
        };
        let max_requests = rate_limit.requests as usize;
        let window = Duration::seconds(rate_limit.window_seconds as i64);
        let now = Utc::now();
        let cutoff = now - window;

        let mut bucket = self
            .rate_limits
            .entry(api_key.id.clone())
            .or_insert_with(Vec::new);
        bucket.retain(|&t| t > cutoff);
        if bucket.len() >= max_requests {
            return Err(SessionError::RateLimitExceeded);
        }
        bucket.push(now);
        Ok(())
    }

    /// Check rate limit for a client
    pub fn check_rate_limit(&self, client_id: &str) -> Result<(), SessionError> {
        let now = Utc::now();
        let window = Duration::minutes(1);
        let max_requests = 100; // 100 requests per minute

        let mut request_count = 0;
        if let Some(requests) = self.rate_limits.get(client_id) {
            let cutoff = now - window;
            request_count = requests
                .iter()
                .filter(|&&req_time| req_time > cutoff)
                .count();
        }

        if request_count >= max_requests {
            return Err(SessionError::RateLimitExceeded);
        }

        // Record this request
        self.rate_limits
            .entry(client_id.to_string())
            .or_insert_with(Vec::new)
            .push(now);

        // Clean up old entries periodically
        if let Some(mut requests) = self.rate_limits.get_mut(client_id) {
            let cutoff = now - window;
            requests.retain(|&req_time| req_time > cutoff);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_password_hashing() {
        let auth = AuthManager::new(AuthConfig::default(), "test-secret").unwrap();

        let password = "StrongP@ssw0rd!";
        let hash = auth.hash_password(password).unwrap();

        assert!(auth.verify_password(password, &hash).unwrap());
        assert!(!auth.verify_password("wrong-password", &hash).unwrap());
    }

    #[test]
    fn test_jwt_tokens() {
        let auth = AuthManager::new(AuthConfig::default(), "test-secret").unwrap();

        let token = auth
            .create_token(
                "user123",
                Some("user@example.com".to_string()),
                vec!["user".to_string()],
            )
            .unwrap();

        let claims = auth.verify_token(&token.token).unwrap();
        assert_eq!(claims.sub, "user123");
        assert_eq!(claims.email, Some("user@example.com".to_string()));

        // Revoke token
        auth.revoke_token(&token.id, "test", "admin");

        // Should fail now
        assert!(auth.verify_token(&token.token).is_err());
    }

    #[test]
    fn test_api_keys() {
        let auth = AuthManager::new(AuthConfig::default(), "test-secret").unwrap();

        let (raw_key, api_key) = auth
            .create_api_key(
                "user123",
                "Test Key",
                vec!["read".to_string(), "write".to_string()],
                Some(30),
            )
            .unwrap();

        assert!(raw_key.starts_with("rosh_"));

        let verified = auth.verify_api_key(&raw_key).unwrap();
        assert_eq!(verified.id, api_key.id);
        assert_eq!(verified.permissions, vec!["read", "write"]);
    }

    /// AUDIT-H8: a token whose `last_activity` is older than the
    /// configured idle window must be rejected with `Expired` and
    /// the cached entry dropped.
    #[test]
    fn verify_token_rejects_idle_session() {
        let mut config = AuthConfig::default();
        config.idle_timeout_seconds = 60; // 1 minute
        let auth = AuthManager::new(config, "test-secret").unwrap();

        let session_token = auth
            .create_token("user-idle", None, vec!["user".to_string()])
            .unwrap();

        // Sanity: a fresh token verifies.
        assert!(auth.verify_token(&session_token.token).is_ok());

        // Rewind the cached `last_activity` past the idle window.
        // Direct field access works because tests live in a child
        // module of the same file.
        {
            let mut entry = auth
                .tokens
                .get_mut(&session_token.id)
                .expect("token must be cached after create_token");
            entry.last_activity = Utc::now() - chrono::Duration::seconds(120);
        }

        // Second verify must reject (Expired) and remove the cached
        // entry so a future probe cannot silently re-arm it.
        match auth.verify_token(&session_token.token) {
            Err(SessionError::Expired { .. }) => {}
            other => panic!("expected SessionError::Expired, got {:?}", other),
        }
        assert!(
            !auth.tokens.contains_key(&session_token.id),
            "idle-expired tokens must be dropped from the cache",
        );
    }

    /// AUDIT-H8: `idle_timeout_seconds == 0` is the documented opt-out;
    /// verify_token must not enforce the idle check when disabled.
    #[test]
    fn verify_token_idle_timeout_disabled_when_zero() {
        let mut config = AuthConfig::default();
        config.idle_timeout_seconds = 0;
        let auth = AuthManager::new(config, "test-secret").unwrap();
        let session_token = auth
            .create_token("user-noidle", None, vec!["user".to_string()])
            .unwrap();

        // Push last_activity far into the past; opt-out must still accept.
        {
            let mut entry = auth
                .tokens
                .get_mut(&session_token.id)
                .expect("token must be cached after create_token");
            entry.last_activity = Utc::now() - chrono::Duration::days(7);
        }

        assert!(
            auth.verify_token(&session_token.token).is_ok(),
            "idle_timeout_seconds == 0 must disable idle enforcement entirely",
        );
    }

    /// AUDIT-H9: verify_api_key must consult the per-key rate limit
    /// and reject once the configured window's request budget is
    /// exhausted. The default limit minted by create_api_key is
    /// 1000 req/h — too high to drive in a unit test — so we
    /// mutate the cached key's rate_limit down to a tiny budget
    /// before hammering verify.
    #[test]
    fn verify_api_key_enforces_rate_limit_when_window_exhausted() {
        let auth = AuthManager::new(AuthConfig::default(), "test-secret").unwrap();
        let (raw_key, api_key) = auth
            .create_api_key("user-rate", "rate-test", vec!["read".to_string()], None)
            .unwrap();

        // Shrink the budget to 3 requests per hour so the test can
        // exhaust it deterministically. Direct DashMap mutation is
        // fine here because tests share the module privacy boundary.
        {
            let mut entry = auth
                .api_keys
                .get_mut(&api_key.id)
                .expect("api key cached after create_api_key");
            entry.rate_limit = Some(RateLimit {
                requests: 3,
                window_seconds: 3600,
            });
        }

        for i in 0..3 {
            auth.verify_api_key(&raw_key)
                .unwrap_or_else(|e| panic!("verify {} should pass within budget: {:?}", i, e));
        }

        match auth.verify_api_key(&raw_key) {
            Err(SessionError::RateLimitExceeded) => {}
            other => panic!(
                "fourth verify must trip the rate limit, got {:?}",
                other.map(|_| "Ok")
            ),
        }
    }

    /// AUDIT-H9: a request that trips the rate limit must not be
    /// recorded in the bucket, otherwise a hammering caller could
    /// keep pushing the window edge forward and never recover.
    #[test]
    fn verify_api_key_rate_limit_rejection_does_not_record_request() {
        let auth = AuthManager::new(AuthConfig::default(), "test-secret").unwrap();
        let (raw_key, api_key) = auth
            .create_api_key("user-rate-2", "rate-test", vec!["read".to_string()], None)
            .unwrap();
        {
            let mut entry = auth.api_keys.get_mut(&api_key.id).unwrap();
            entry.rate_limit = Some(RateLimit {
                requests: 1,
                window_seconds: 3600,
            });
        }

        auth.verify_api_key(&raw_key).expect("first verify passes");

        // Now exhaust the budget. The rejected attempt must not
        // bump the bucket count beyond the configured maximum.
        assert!(matches!(
            auth.verify_api_key(&raw_key),
            Err(SessionError::RateLimitExceeded)
        ));

        let bucket_len = auth
            .rate_limits
            .get(&api_key.id)
            .map(|b| b.len())
            .unwrap_or(0);
        assert_eq!(
            bucket_len, 1,
            "rejected requests must not be recorded; bucket should still hold only the successful call"
        );
    }

    /// AUDIT-H9: when a key has no rate_limit configured (legacy
    /// persisted keys minted before this audit), verify_api_key
    /// must continue to accept requests indefinitely. The default
    /// from create_api_key always populates rate_limit, so this
    /// path exists purely for backward-compatible loading.
    #[test]
    fn verify_api_key_no_limit_when_rate_limit_none() {
        let auth = AuthManager::new(AuthConfig::default(), "test-secret").unwrap();
        let (raw_key, api_key) = auth
            .create_api_key("user-rate-3", "rate-test", vec!["read".to_string()], None)
            .unwrap();
        {
            let mut entry = auth.api_keys.get_mut(&api_key.id).unwrap();
            entry.rate_limit = None;
        }

        // Hammer it — none should fail.
        for i in 0..50 {
            auth.verify_api_key(&raw_key)
                .unwrap_or_else(|e| panic!("verify {} must pass with no rate limit: {:?}", i, e));
        }
    }

    #[test]
    fn test_lockout() {
        let mut config = AuthConfig::default();
        config.max_failed_attempts = 3;
        config.lockout_duration_seconds = 60;

        let auth = AuthManager::new(config, "test-secret").unwrap();

        // Record failed attempts
        for _ in 0..3 {
            auth.record_login_attempt("user123", false, None, None).ok();
        }

        // Should be locked out
        assert!(auth.is_locked_out("user123"));

        // Next attempt should fail
        let result = auth.record_login_attempt("user123", false, None, None);
        assert!(result.is_err());

        // Successful login clears attempts
        auth.record_login_attempt("user123", true, None, None)
            .unwrap();
        assert!(!auth.is_locked_out("user123"));
    }

    /// AUDIT-M5 contract: with an empty environment, every field of
    /// `from_env_with` matches `AuthConfig::default()`. Catches a
    /// regression where a new field is added to `AuthConfig` but the
    /// env mapping forgets to read it (would silently produce a
    /// `Default::default()` value at runtime — fine in this test, but
    /// the test exists to force the author to add an override case
    /// below as well).
    #[test]
    fn from_env_with_empty_environment_matches_default() {
        let cfg = AuthConfig::from_env_with(|_| None);
        let default = AuthConfig::default();
        assert_eq!(cfg.issuer, default.issuer);
        assert_eq!(cfg.audience, default.audience);
        assert_eq!(cfg.token_expiry_seconds, default.token_expiry_seconds);
        assert_eq!(cfg.refresh_expiry_seconds, default.refresh_expiry_seconds);
        assert_eq!(cfg.idle_timeout_seconds, default.idle_timeout_seconds);
        assert_eq!(cfg.max_failed_attempts, default.max_failed_attempts);
        assert_eq!(
            cfg.lockout_duration_seconds,
            default.lockout_duration_seconds
        );
        assert_eq!(
            cfg.require_2fa_for_sensitive,
            default.require_2fa_for_sensitive
        );
        assert_eq!(cfg.api_key_prefix, default.api_key_prefix);
        assert_eq!(
            cfg.password_requirements.min_length,
            default.password_requirements.min_length
        );
        assert_eq!(
            cfg.password_requirements.require_uppercase,
            default.password_requirements.require_uppercase
        );
        assert_eq!(
            cfg.password_requirements.require_lowercase,
            default.password_requirements.require_lowercase
        );
        assert_eq!(
            cfg.password_requirements.require_numbers,
            default.password_requirements.require_numbers
        );
        assert_eq!(
            cfg.password_requirements.require_special,
            default.password_requirements.require_special
        );
    }

    /// AUDIT-M5: every `ROSHERA_*` knob round-trips through
    /// `from_env_with` to the corresponding `AuthConfig` field. The
    /// closure supplies a `HashMap`-backed fake env so we never touch
    /// the real process environment (the canonical Rust env-test race
    /// hazard).
    #[test]
    fn from_env_with_overrides_every_field() {
        use std::collections::HashMap;
        let mut env = HashMap::new();
        env.insert("ROSHERA_AUTH_ISSUER", "test-issuer");
        env.insert("ROSHERA_AUTH_AUDIENCE", "api-a, api-b , api-c");
        env.insert("ROSHERA_TOKEN_EXPIRY_SECONDS", "7200");
        env.insert("ROSHERA_REFRESH_EXPIRY_SECONDS", "1209600");
        env.insert("ROSHERA_IDLE_TIMEOUT_SECONDS", "900");
        env.insert("ROSHERA_MAX_FAILED_ATTEMPTS", "10");
        env.insert("ROSHERA_LOCKOUT_DURATION_SECONDS", "600");
        env.insert("ROSHERA_REQUIRE_2FA_SENSITIVE", "true");
        env.insert("ROSHERA_API_KEY_PREFIX", "tst_");
        env.insert("ROSHERA_PASSWORD_MIN_LENGTH", "12");
        env.insert("ROSHERA_PASSWORD_REQUIRE_UPPERCASE", "false");
        env.insert("ROSHERA_PASSWORD_REQUIRE_LOWERCASE", "1");
        env.insert("ROSHERA_PASSWORD_REQUIRE_NUMBERS", "no");
        env.insert("ROSHERA_PASSWORD_REQUIRE_SPECIAL", "yes");

        let cfg = AuthConfig::from_env_with(|k| env.get(k).map(|s| s.to_string()));

        assert_eq!(cfg.issuer, "test-issuer");
        assert_eq!(cfg.audience, vec!["api-a", "api-b", "api-c"]);
        assert_eq!(cfg.token_expiry_seconds, 7200);
        assert_eq!(cfg.refresh_expiry_seconds, 1_209_600);
        assert_eq!(cfg.idle_timeout_seconds, 900);
        assert_eq!(cfg.max_failed_attempts, 10);
        assert_eq!(cfg.lockout_duration_seconds, 600);
        assert!(cfg.require_2fa_for_sensitive);
        assert_eq!(cfg.api_key_prefix, "tst_");
        assert_eq!(cfg.password_requirements.min_length, 12);
        assert!(!cfg.password_requirements.require_uppercase);
        assert!(cfg.password_requirements.require_lowercase);
        assert!(!cfg.password_requirements.require_numbers);
        assert!(cfg.password_requirements.require_special);
    }

    /// AUDIT-M5: unparseable values fall back to the default field
    /// rather than panicking at startup. A typoed env var must not
    /// take the server down.
    #[test]
    fn from_env_with_unparseable_values_fall_back_to_default() {
        use std::collections::HashMap;
        let mut env = HashMap::new();
        env.insert("ROSHERA_TOKEN_EXPIRY_SECONDS", "not-a-number");
        env.insert("ROSHERA_MAX_FAILED_ATTEMPTS", "-7");
        env.insert("ROSHERA_REQUIRE_2FA_SENSITIVE", "maybe");
        env.insert("ROSHERA_PASSWORD_MIN_LENGTH", "");

        let cfg = AuthConfig::from_env_with(|k| env.get(k).map(|s| s.to_string()));
        let default = AuthConfig::default();
        assert_eq!(cfg.token_expiry_seconds, default.token_expiry_seconds);
        assert_eq!(cfg.max_failed_attempts, default.max_failed_attempts);
        assert_eq!(
            cfg.require_2fa_for_sensitive,
            default.require_2fa_for_sensitive
        );
        assert_eq!(
            cfg.password_requirements.min_length,
            default.password_requirements.min_length
        );
    }

    /// AUDIT-M5: empty CSV / whitespace-only entries are pruned, and
    /// a CSV that yields zero non-empty entries falls back to the
    /// default audience list (rather than silently shipping an empty
    /// audience claim).
    #[test]
    fn from_env_with_audience_csv_handles_edge_cases() {
        use std::collections::HashMap;
        let mut env = HashMap::new();
        env.insert("ROSHERA_AUTH_AUDIENCE", " , , ,,");
        let cfg = AuthConfig::from_env_with(|k| env.get(k).map(|s| s.to_string()));
        // All entries pruned → fallback to default.
        assert_eq!(cfg.audience, AuthConfig::default().audience);

        let mut env = HashMap::new();
        env.insert("ROSHERA_AUTH_AUDIENCE", "  one  ,,  two  ");
        let cfg = AuthConfig::from_env_with(|k| env.get(k).map(|s| s.to_string()));
        assert_eq!(cfg.audience, vec!["one", "two"]);
    }
}
