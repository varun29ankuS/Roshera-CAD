//! Authentication handlers for login, registration, and token management

use crate::AppState;
use axum::{
    extract::State,
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::{Json, Result},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use session_manager::{AuthManager, Permission};
use tracing::{error, info, warn};

/// Response payload for the logout endpoint.
#[derive(Debug, Serialize)]
pub struct LogoutResponse {
    /// Indicates whether the token was successfully revoked.
    pub success: bool,
    /// Human-readable status string.
    pub message: String,
    /// Populated only on the unhappy path; mirrors the codes used by
    /// the other auth handlers in this module.
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    pub remember_me: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub success: bool,
    pub token: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    pub user_id: Option<String>,
    pub permissions: Option<Vec<String>>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub email: String,
    pub password: String,
    pub full_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct RegisterResponse {
    pub success: bool,
    pub user_id: Option<String>,
    pub message: String,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Serialize)]
pub struct RefreshResponse {
    pub success: bool,
    pub token: Option<String>,
    pub expires_in: Option<u64>,
    pub error: Option<String>,
}

/// Handle user login
///
/// Validates credentials against the user store held by the database persistence layer.
/// The supplied password is verified via Argon2 against the hash stored in `UserData`.
/// A JWT access token and a JWT refresh token are returned on success.
pub async fn login(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<LoginResponse>> {
    info!("Login attempt for user: {}", payload.username);

    let auth_manager = &state.auth_manager;

    // Derive the canonical user_id from the username (consistent with registration).
    let user_id = format!("user_{}", payload.username);

    // Load user data from the database — this is the single source of truth.
    let user_data = match state.database.load_user(&user_id).await {
        Ok(u) => u,
        Err(_) => {
            // Use a constant-time-equivalent response to prevent username enumeration.
            warn!("Login failed — user not found: {}", payload.username);
            return Ok(Json(LoginResponse {
                success: false,
                token: None,
                refresh_token: None,
                expires_in: None,
                user_id: None,
                permissions: None,
                error: Some("Invalid credentials".to_string()),
            }));
        }
    };

    if !user_data.is_active {
        warn!("Login rejected — account inactive for user: {}", user_id);
        return Ok(Json(LoginResponse {
            success: false,
            token: None,
            refresh_token: None,
            expires_in: None,
            user_id: None,
            permissions: None,
            error: Some("Account is inactive".to_string()),
        }));
    }

    // Verify the supplied password against the stored Argon2 hash.
    let password_valid = auth_manager
        .verify_password(&payload.password, &user_data.password_hash)
        .unwrap_or(false);

    if !password_valid {
        warn!("Login failed — wrong password for user: {}", user_id);
        return Ok(Json(LoginResponse {
            success: false,
            token: None,
            refresh_token: None,
            expires_in: None,
            user_id: None,
            permissions: None,
            error: Some("Invalid credentials".to_string()),
        }));
    }

    info!("Login successful for user: {}", user_id);

    let token = auth_manager
        .create_token(
            &user_id,
            Some(user_data.email.clone()),
            vec!["user".to_string()],
        )
        .map_err(|e| {
            error!("Failed to create token: {:?}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Token creation failed")
        })?;

    // The refresh token is embedded inside the SessionToken returned by create_token.
    let refresh_token = token.refresh_token.clone().unwrap_or_default();
    let permissions = get_user_permission_strings();

    Ok(Json(LoginResponse {
        success: true,
        token: Some(token.token),
        refresh_token: Some(refresh_token),
        expires_in: Some(token.expires_at.timestamp() as u64),
        user_id: Some(user_id),
        permissions: Some(permissions),
        error: None,
    }))
}

/// Handle user registration
///
/// Validates the supplied username, email, and password, hashes the password with
/// Argon2, and persists a new `UserData` record through the database persistence layer.
/// The canonical `user_id` is derived as `"user_{username}"` for consistent lookups
/// in the login handler.
pub async fn register(
    State(state): State<AppState>,
    Json(payload): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>> {
    info!("Registration attempt for user: {}", payload.username);

    let auth_manager = &state.auth_manager;

    if payload.username.len() < 3 {
        return Ok(Json(RegisterResponse {
            success: false,
            user_id: None,
            message: "Username must be at least 3 characters".to_string(),
            error: Some("INVALID_USERNAME".to_string()),
        }));
    }

    if !payload
        .username
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
    {
        return Ok(Json(RegisterResponse {
            success: false,
            user_id: None,
            message: "Username may only contain letters, digits, hyphens, and underscores"
                .to_string(),
            error: Some("INVALID_USERNAME_CHARS".to_string()),
        }));
    }

    // Derive the canonical user_id to ensure consistent lookup in login.
    let user_id = format!("user_{}", payload.username);

    // Reject if this username is already registered.
    if state.database.load_user(&user_id).await.is_ok() {
        warn!(
            "Registration rejected — username already taken: {}",
            payload.username
        );
        return Ok(Json(RegisterResponse {
            success: false,
            user_id: None,
            message: "Username is already taken".to_string(),
            error: Some("USERNAME_TAKEN".to_string()),
        }));
    }

    // Validate and hash the password using Argon2 via AuthManager.
    // hash_password also enforces the configured password complexity requirements.
    let password_hash = match auth_manager.hash_password(&payload.password) {
        Ok(hash) => hash,
        Err(e) => {
            warn!(
                "Password validation failed for user {}: {:?}",
                payload.username, e
            );
            return Ok(Json(RegisterResponse {
                success: false,
                user_id: None,
                message: "Password does not meet security requirements".to_string(),
                error: Some(format!("{:?}", e)),
            }));
        }
    };

    let now = Utc::now();
    let user_data = session_manager::UserData {
        id: user_id.clone(),
        email: payload.email.clone(),
        name: payload
            .full_name
            .clone()
            .unwrap_or_else(|| payload.username.clone()),
        password_hash,
        created_at: now,
        last_login: None,
        is_active: true,
        is_verified: false, // Email verification is a separate flow.
        profile: serde_json::Value::Object(serde_json::Map::new()),
    };

    if let Err(e) = state.database.save_user(&user_data).await {
        error!(
            "Failed to persist user {} during registration: {:?}",
            user_id, e
        );
        return Ok(Json(RegisterResponse {
            success: false,
            user_id: None,
            message: "Registration failed due to a server error".to_string(),
            error: Some("STORAGE_ERROR".to_string()),
        }));
    }

    info!(
        "Registration successful: user '{}' (id: {})",
        payload.username, user_id
    );

    Ok(Json(RegisterResponse {
        success: true,
        user_id: Some(user_id),
        message: format!(
            "User '{}' registered successfully. Please login with your credentials.",
            payload.username
        ),
        error: None,
    }))
}

/// Handle token refresh
///
/// The supplied refresh token must be a valid, unexpired JWT signed by this server.
/// The `sub` claim in the refresh token is used to identify the user for whom
/// a new access token is issued. The refresh token itself is validated via
/// `AuthManager::verify_token` so revoked or expired refresh tokens are rejected.
pub async fn refresh_token(
    State(state): State<AppState>,
    Json(payload): Json<RefreshRequest>,
) -> Result<Json<RefreshResponse>> {
    info!("Token refresh attempt");

    let auth_manager = &state.auth_manager;

    // Validate the refresh token as a JWT. verify_token checks signature, expiry,
    // and revocation list, so a bare UUID or tampered token is rejected here.
    let claims = match auth_manager.verify_token(&payload.refresh_token) {
        Ok(c) => c,
        Err(e) => {
            warn!("Token refresh rejected — invalid token: {:?}", e);
            return Ok(Json(RefreshResponse {
                success: false,
                token: None,
                expires_in: None,
                error: Some("Invalid or expired refresh token".to_string()),
            }));
        }
    };

    let user_id = claims.sub;

    let new_token = auth_manager
        .create_token(&user_id, claims.email, claims.roles)
        .map_err(|e| {
            error!("Failed to create new token during refresh: {:?}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Token creation failed")
        })?;

    info!("Token refresh successful for user: {}", user_id);

    Ok(Json(RefreshResponse {
        success: true,
        token: Some(new_token.token),
        expires_in: Some(new_token.expires_at.timestamp() as u64),
        error: None,
    }))
}

/// Handle user logout
///
/// Extracts the bearer token from the `Authorization` header, verifies it via
/// `AuthManager::verify_token` (which rejects tampered, expired, or already
/// revoked tokens), then revokes its `jti` so subsequent requests using the
/// same token fail authentication.
///
/// Idempotent: revoking an already-revoked token surfaces as a verification
/// failure and returns success: false with an explanatory error code, but
/// otherwise has no side effects.
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<LogoutResponse>> {
    let auth_manager = &state.auth_manager;

    // Extract the Bearer token from the Authorization header.
    let raw_token = match headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.trim().to_string())
    {
        Some(t) if !t.is_empty() => t,
        _ => {
            warn!("Logout rejected — missing or malformed Authorization header");
            return Ok(Json(LogoutResponse {
                success: false,
                message: "Missing bearer token".to_string(),
                error: Some("MISSING_TOKEN".to_string()),
            }));
        }
    };

    // Verify the token to surface its jti. A revoked or expired token will
    // fail verification — we treat that as a no-op success from the caller's
    // perspective (logout is idempotent) but log the rejection for audit.
    let claims = match auth_manager.verify_token(&raw_token) {
        Ok(c) => c,
        Err(e) => {
            warn!("Logout: token already invalid or revoked: {:?}", e);
            return Ok(Json(LogoutResponse {
                success: false,
                message: "Token already invalid".to_string(),
                error: Some("INVALID_TOKEN".to_string()),
            }));
        }
    };

    auth_manager.revoke_token(&claims.jti, "user_logout", &claims.sub);
    info!(
        "Logout successful — token {} revoked for user {}",
        claims.jti, claims.sub
    );

    Ok(Json(LogoutResponse {
        success: true,
        message: "Logged out successfully".to_string(),
        error: None,
    }))
}

/// Default permission strings returned on successful login.
///
/// These reflect the baseline "user" role. Fine-grained per-session permissions
/// are enforced by the `PermissionManager` when the user joins a session and are
/// stored in `UserPermissions` records, not in the JWT itself.
fn get_user_permission_strings() -> Vec<String> {
    vec![
        "ViewGeometry".to_string(),
        "CreateGeometry".to_string(),
        "ModifyGeometry".to_string(),
        "ExportGeometry".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_login() {
        // Test implementation
    }

    #[tokio::test]
    async fn test_register() {
        // Test implementation
    }

    #[tokio::test]
    async fn test_refresh_token() {
        // Test implementation
    }
}
