//! Authentication middleware for API server
//!
//! This module provides JWT-based authentication and API key validation
//! middleware for all API endpoints.

use axum::{
    extract::{Request, State},
    http::{header::AUTHORIZATION, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use session_manager::{AuthManager, Permission};
use std::sync::Arc;
use tracing::{error, info, warn};

/// Authentication error response
#[derive(Debug, Serialize)]
pub struct AuthError {
    pub error: String,
    pub code: String,
    pub status: u16,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        (
            StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(self),
        )
            .into_response()
    }
}

/// Extracted authentication information
#[derive(Debug, Clone)]
pub struct AuthInfo {
    pub user_id: String,
    pub session_id: Option<String>,
    pub permissions: Vec<Permission>,
    pub roles: Vec<String>, // Added roles field for production compatibility
    pub is_api_key: bool,
}

/// Returns true when the api-server is running in local dev mode and
/// should accept unauthenticated requests with full permissions. Gated
/// by `ROSHERA_DEV_BRIDGE=1`, the same flag that exposes the viewport
/// debug bridge — both are dev-only conveniences and the env-gate
/// makes accidental production exposure impossible.
fn dev_mode_enabled() -> bool {
    std::env::var("ROSHERA_DEV_BRIDGE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Build a permissive AuthInfo for dev-mode requests. Every permission
/// is granted; user_id is a stable sentinel so audit logs are still
/// distinguishable from real users.
fn dev_auth_info() -> AuthInfo {
    AuthInfo {
        user_id: "dev-bridge".to_string(),
        session_id: Some("dev-session".to_string()),
        permissions: vec![
            Permission::CreateGeometry,
            Permission::ModifyGeometry,
            Permission::DeleteGeometry,
            Permission::ViewGeometry,
            Permission::ExportGeometry,
            Permission::RecordSession,
        ],
        roles: vec!["dev".to_string()],
        is_api_key: false,
    }
}

/// Tower middleware that injects a permissive `AuthInfo` extension on
/// every request when `ROSHERA_DEV_BRIDGE=1`. This lets handlers using
/// the built-in `Extension<AuthInfo>` extractor succeed in local dev
/// without a real session, mirroring the dev-mode bypass in the
/// canonical `auth_middleware` (which is not currently layered onto
/// the router globally).
pub async fn dev_auth_layer(mut request: Request, next: Next) -> Response {
    if dev_mode_enabled() && request.extensions().get::<AuthInfo>().is_none() {
        request.extensions_mut().insert(dev_auth_info());
    }
    next.run(request).await
}

/// Authentication middleware that validates JWT tokens or API keys.
///
/// In dev mode (`ROSHERA_DEV_BRIDGE=1`) the middleware injects a
/// permissive `AuthInfo` and skips header validation so the toolbar /
/// debug bridge can drive the kernel without a real session.
pub async fn auth_middleware(
    State(auth_manager): State<Arc<AuthManager>>,
    mut request: Request,
    next: Next,
) -> Result<Response, AuthError> {
    if dev_mode_enabled() {
        request.extensions_mut().insert(dev_auth_info());
        return Ok(next.run(request).await);
    }

    // Extract authorization header
    let auth_header = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| AuthError {
            error: "Missing authorization header".to_string(),
            code: "AUTH_MISSING".to_string(),
            status: 401,
        })?;

    // Parse authorization header
    let auth_info = if auth_header.starts_with("Bearer ") {
        // JWT token authentication
        let token = &auth_header[7..];
        validate_jwt(auth_manager.as_ref(), token).await?
    } else if auth_header.starts_with("ApiKey ") {
        // API key authentication
        let api_key = &auth_header[7..];
        validate_api_key(auth_manager.as_ref(), api_key).await?
    } else {
        return Err(AuthError {
            error: "Invalid authorization format".to_string(),
            code: "AUTH_INVALID_FORMAT".to_string(),
            status: 401,
        });
    };

    // Insert auth info into request extensions
    request.extensions_mut().insert(auth_info);

    Ok(next.run(request).await)
}

/// Permission-checking middleware
pub async fn require_permission(
    required_permission: Permission,
) -> impl Fn(
    Request,
    Next,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<Response, AuthError>> + Send>,
> + Clone {
    move |request: Request, next: Next| {
        let required = required_permission.clone();
        Box::pin(async move {
            // Get auth info from request extensions
            let auth_info = request
                .extensions()
                .get::<AuthInfo>()
                .ok_or_else(|| AuthError {
                    error: "Authentication required".to_string(),
                    code: "AUTH_REQUIRED".to_string(),
                    status: 401,
                })?
                .clone();

            // Check if user has required permission
            if !auth_info.permissions.contains(&required) {
                return Err(AuthError {
                    error: format!("Permission denied: {:?} required", required),
                    code: "PERMISSION_DENIED".to_string(),
                    status: 403,
                });
            }

            Ok(next.run(request).await)
        })
    }
}

/// Validate JWT token
async fn validate_jwt(auth_manager: &AuthManager, token: &str) -> Result<AuthInfo, AuthError> {
    match auth_manager.verify_token(token) {
        Ok(claims) => {
            info!("JWT validated for user: {}", claims.sub);

            // Get user permissions (simplified - in real app, query from DB)
            let permissions = get_default_user_permissions();

            Ok(AuthInfo {
                user_id: claims.sub.clone(),
                session_id: Some(claims.jti.clone()), // Use JWT ID as session ID
                permissions,
                roles: vec!["user".to_string()], // Default role
                is_api_key: false,
            })
        }
        Err(e) => {
            warn!("JWT validation failed: {}", e);
            Err(AuthError {
                error: "Invalid or expired token".to_string(),
                code: "TOKEN_INVALID".to_string(),
                status: 401,
            })
        }
    }
}

/// Validate API key
async fn validate_api_key(
    auth_manager: &AuthManager,
    api_key: &str,
) -> Result<AuthInfo, AuthError> {
    match auth_manager.verify_api_key(api_key) {
        Ok(key_info) => {
            info!("API key validated for user: {}", key_info.user_id);

            // Convert string permissions to Permission enum
            let permissions: Vec<Permission> = key_info
                .permissions
                .iter()
                .filter_map(|p| match p.as_str() {
                    "create_geometry" => Some(Permission::CreateGeometry),
                    "modify_geometry" => Some(Permission::ModifyGeometry),
                    "delete_geometry" => Some(Permission::DeleteGeometry),
                    "view_geometry" => Some(Permission::ViewGeometry),
                    "export_geometry" => Some(Permission::ExportGeometry),
                    "record_session" => Some(Permission::RecordSession),
                    _ => None,
                })
                .collect();

            Ok(AuthInfo {
                user_id: key_info.user_id,
                session_id: None,
                permissions,
                roles: vec!["api_user".to_string()], // API key role
                is_api_key: true,
            })
        }
        Err(e) => {
            warn!("API key validation failed: {}", e);
            Err(AuthError {
                error: "Invalid API key".to_string(),
                code: "API_KEY_INVALID".to_string(),
                status: 401,
            })
        }
    }
}

/// Get default permissions for authenticated users
fn get_default_user_permissions() -> Vec<Permission> {
    vec![
        Permission::CreateGeometry,
        Permission::ModifyGeometry,
        Permission::ViewGeometry,
        Permission::ExportGeometry,
        Permission::RecordSession,
    ]
}

/// Rate limiting middleware
pub async fn rate_limit_middleware(
    State(auth_manager): State<Arc<AuthManager>>,
    request: Request,
    next: Next,
) -> Result<Response, AuthError> {
    // Get client identifier (user ID or IP)
    let client_id = if let Some(auth_info) = request.extensions().get::<AuthInfo>() {
        auth_info.user_id.clone()
    } else {
        // Use IP address for unauthenticated requests
        request
            .headers()
            .get("x-forwarded-for")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("unknown")
            .to_string()
    };

    // Check rate limit
    match auth_manager.check_rate_limit(&client_id) {
        Ok(_) => Ok(next.run(request).await),
        Err(_) => Err(AuthError {
            error: "Rate limit exceeded".to_string(),
            code: "RATE_LIMIT_EXCEEDED".to_string(),
            status: 429,
        }),
    }
}

/// Extract auth info from request
pub fn get_auth_info(request: &Request) -> Option<&AuthInfo> {
    request.extensions().get::<AuthInfo>()
}

/// Implement FromRequestParts for AuthInfo to allow it as a handler parameter.
///
/// When `ROSHERA_DEV_BRIDGE=1` and no extension is present, fall back to a
/// permissive dev `AuthInfo`. The auth middleware is the canonical injector
/// when wired as a layer; the dev fallback exists because the router
/// currently doesn't apply the layer globally and we still want every
/// AuthInfo-extracting handler to work in local development.
impl<S> axum::extract::FromRequestParts<S> for AuthInfo
where
    S: Send + Sync,
{
    type Rejection = AuthError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        if let Some(info) = parts.extensions.get::<AuthInfo>().cloned() {
            return Ok(info);
        }
        if dev_mode_enabled() {
            return Ok(dev_auth_info());
        }
        Err(AuthError {
            error: "Authentication required".to_string(),
            code: "AUTH_REQUIRED".to_string(),
            status: 401,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_auth_middleware() {
        // Test implementation
    }
}
