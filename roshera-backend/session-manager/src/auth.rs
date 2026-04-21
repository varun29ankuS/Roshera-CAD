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
    /// Token expiration in seconds
    pub token_expiry_seconds: i64,
    /// Refresh token expiration in seconds
    pub refresh_expiry_seconds: i64,
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

impl AuthManager {
    /// Create new auth manager
    pub fn new(config: AuthConfig, jwt_secret: &str) -> Self {
        let jwt_secret =
            HmacSha256::new_from_slice(jwt_secret.as_bytes()).expect("Invalid JWT secret");

        Self {
            jwt_secret: Arc::new(jwt_secret),
            tokens: Arc::new(DashMap::new()),
            api_keys: Arc::new(DashMap::new()),
            two_factor: Arc::new(DashMap::new()),
            security_events: Arc::new(DashMap::new()),
            revoked_tokens: Arc::new(DashMap::new()),
            failed_attempts: Arc::new(DashMap::new()),
            rate_limits: Arc::new(DashMap::new()),
            config,
        }
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

    /// Verify JWT token
    pub fn verify_token(&self, token: &str) -> Result<TokenClaims, SessionError> {
        // Check if token is revoked
        let claims: TokenClaims = token
            .verify_with_key(&*self.jwt_secret)
            .map_err(|_e| SessionError::AccessDenied)?;

        if self.revoked_tokens.contains_key(&claims.jti) {
            return Err(SessionError::AccessDenied);
        }

        // Verify expiration
        let now = Utc::now().timestamp();
        if claims.exp < now {
            return Err(SessionError::Expired { id: claims.jti });
        }

        // Update last activity
        if let Some(mut session_token) = self.tokens.get_mut(&claims.jti) {
            session_token.last_activity = Utc::now();
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
        let auth = AuthManager::new(AuthConfig::default(), "test-secret");

        let password = "StrongP@ssw0rd!";
        let hash = auth.hash_password(password).unwrap();

        assert!(auth.verify_password(password, &hash).unwrap());
        assert!(!auth.verify_password("wrong-password", &hash).unwrap());
    }

    #[test]
    fn test_jwt_tokens() {
        let auth = AuthManager::new(AuthConfig::default(), "test-secret");

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
        let auth = AuthManager::new(AuthConfig::default(), "test-secret");

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

    #[test]
    fn test_lockout() {
        let mut config = AuthConfig::default();
        config.max_failed_attempts = 3;
        config.lockout_duration_seconds = 60;

        let auth = AuthManager::new(config, "test-secret");

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
}
