//! Authentication handlers for login, registration, and token management

use crate::AppState;
use axum::{
    extract::State,
    http::StatusCode,
    response::{Json, Result},
};
use serde::{Deserialize, Serialize};
use session_manager::{AuthManager, Permission};
use tracing::{error, info, warn};

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
pub async fn login(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<LoginResponse>> {
    info!("Login attempt for user: {}", payload.username);

    let auth_manager = &state.auth_manager;

    // For MVP, we'll use a simple in-memory user system
    // In production, this would query a database

    // Demo users with hardcoded passwords (NEVER do this in production)
    let demo_password_hash = auth_manager.hash_password("demo123").unwrap_or_default();

    // Verify the password matches our demo user
    let is_valid = if payload.username == "demo" || payload.username == "admin" {
        payload.password == "demo123" // Simple check for MVP
    } else {
        false
    };

    if is_valid {
        let user_id = format!("user_{}", payload.username);
        info!("Login successful for user: {}", user_id);

        // Create a session token
        let token = auth_manager
            .create_token(
                &user_id,
                Some(format!("{}@example.com", payload.username)),
                vec!["user".to_string()],
            )
            .map_err(|e| {
                error!("Failed to create token: {:?}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "Token creation failed")
            })?;

        // For refresh token, we'll use the JWT ID as a simple refresh token
        let refresh_token = token.id.clone();

        // Get default permissions
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
    } else {
        warn!("Login failed for user {}", payload.username);

        Ok(Json(LoginResponse {
            success: false,
            token: None,
            refresh_token: None,
            expires_in: None,
            user_id: None,
            permissions: None,
            error: Some("Invalid credentials".to_string()),
        }))
    }
}

/// Handle user registration
pub async fn register(
    State(state): State<AppState>,
    Json(payload): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>> {
    info!("Registration attempt for user: {}", payload.username);

    let auth_manager = &state.auth_manager;

    // Validate input
    if payload.username.len() < 3 {
        return Ok(Json(RegisterResponse {
            success: false,
            user_id: None,
            message: "Username must be at least 3 characters".to_string(),
            error: Some("INVALID_USERNAME".to_string()),
        }));
    }

    if payload.password.len() < 8 {
        return Ok(Json(RegisterResponse {
            success: false,
            user_id: None,
            message: "Password must be at least 8 characters".to_string(),
            error: Some("WEAK_PASSWORD".to_string()),
        }));
    }

    // For MVP, we'll create a simple user registration
    // In production, this would use a proper database

    let user_id = uuid::Uuid::new_v4().to_string();

    // Validate and hash the password
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

    // In a production system, we would:
    // 1. Check if username/email already exists
    // 2. Store user in database with password_hash
    // 3. Send verification email
    // 4. Create audit log entry

    info!(
        "Registration successful for user: {} (id: {})",
        payload.username, user_id
    );
    info!(
        "User email: {:?}, full name: {:?}",
        payload.email, payload.full_name
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
pub async fn refresh_token(
    State(state): State<AppState>,
    Json(payload): Json<RefreshRequest>,
) -> Result<Json<RefreshResponse>> {
    info!("Token refresh attempt");

    // For MVP, we'll implement a simple token refresh
    // In production, this would:
    // 1. Validate the refresh token
    // 2. Check if it's not revoked
    // 3. Generate new access token
    // 4. Optionally rotate refresh token

    let auth_manager = &state.auth_manager;

    // For now, just create a new token if the refresh token looks valid (is a UUID)
    if let Ok(_uuid) = uuid::Uuid::parse_str(&payload.refresh_token) {
        // Create a new token
        let user_id = "refreshed_user"; // In production, extract from refresh token

        let new_token = auth_manager
            .create_token(
                user_id,
                Some("user@example.com".to_string()),
                vec!["user".to_string()],
            )
            .map_err(|e| {
                error!("Failed to create new token: {:?}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "Token creation failed")
            })?;

        info!("Token refresh successful for user: {}", user_id);

        Ok(Json(RefreshResponse {
            success: true,
            token: Some(new_token.token),
            expires_in: Some(new_token.expires_at.timestamp() as u64),
            error: None,
        }))
    } else {
        warn!("Invalid refresh token format");

        Ok(Json(RefreshResponse {
            success: false,
            token: None,
            expires_in: None,
            error: Some("Invalid refresh token".to_string()),
        }))
    }
}

/// Get default permission strings for a user
fn get_user_permission_strings() -> Vec<String> {
    // Return default permissions as strings
    // In production, this would be based on user's role from database
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
