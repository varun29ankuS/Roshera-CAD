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

/// The api-server's authentication posture, resolved once at startup
/// from the environment and threaded through [`AppState`] so it is a
/// property of the running process rather than a per-request
/// environment read.
///
/// # Secure by default
///
/// The default — an empty environment — is [`AuthPosture::Required`].
/// This is the launch-gate invariant: a server that nobody configured
/// enforces authentication. The previous design defaulted to a soft
/// pass-through mode (`ROSHERA_REQUIRE_AUTH` unset ⇒ anonymous requests
/// accepted), which meant the default and only-reachable configuration
/// was open. That is inverted here.
///
/// # The insecure opt-out is explicit and loud
///
/// [`AuthPosture::InsecureDevBypass`] is selected *only* by
/// `ROSHERA_DEV_INSECURE` set to `1`/`true`. It exists so Varun's local
/// dev loop — a frontend that does not yet emit `Authorization` headers
/// — keeps working without a credential. It is impossible to enable by
/// accident: no other variable selects it, a merely-present variable
/// (`ROSHERA_DEV_INSECURE=0`) does not, and every resolution that lands
/// on it logs an unmissable warning (see [`AuthPosture::from_env`]).
///
/// # Decoupled from `ROSHERA_DEV_BRIDGE`
///
/// `ROSHERA_DEV_BRIDGE` mounts the viewport debug bridge. Previously it
/// *also* silently granted full permissions without a credential check,
/// so enabling a debug viewport disabled authentication as a side
/// effect. Those two concerns are now separate: `ROSHERA_DEV_BRIDGE`
/// mounts routes and nothing more; only `ROSHERA_DEV_INSECURE` moves the
/// auth posture. Overloading a convenience flag with an auth bypass is
/// exactly how a bypass gets shipped by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthPosture {
    /// Enforce a valid credential on every non-exempt request. Missing
    /// or invalid credentials are rejected with 401 at the middleware
    /// boundary before any handler runs. The default.
    Required,
    /// Local-development bypass: inject a permissive `AuthInfo` (every
    /// geometry permission) without checking any credential. Selected
    /// only by `ROSHERA_DEV_INSECURE=1`. Never a default.
    InsecureDevBypass,
}

impl AuthPosture {
    /// Resolve the posture from the process environment. Logs loudly
    /// when it lands on [`AuthPosture::InsecureDevBypass`] so an
    /// operator who set the flag — or inherited it from a shell profile
    /// — sees it on every boot.
    pub fn from_env() -> Self {
        let posture = Self::from_env_with(|k| std::env::var(k).ok());
        if posture == AuthPosture::InsecureDevBypass {
            tracing::warn!(
                target: "api_server.auth.insecure",
                "ROSHERA_DEV_INSECURE is set — authentication is DISABLED. Every \
                 request is granted full permissions without a credential. This \
                 is a local-development convenience and MUST NOT be set in any \
                 deployment that fronts real users or is reachable from a network."
            );
        }
        posture
    }

    /// Environment-getter seam for [`AuthPosture::from_env`]. Callers
    /// (and tests) supply the lookup closure so resolution is
    /// deterministic and does not race the process-global environment.
    pub fn from_env_with<F>(get: F) -> Self
    where
        F: Fn(&str) -> Option<String>,
    {
        let truthy = get("ROSHERA_DEV_INSECURE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        if truthy {
            AuthPosture::InsecureDevBypass
        } else {
            AuthPosture::Required
        }
    }
}

/// State handed to the global [`auth_middleware`] layer: the process's
/// single `AuthManager` plus the resolved [`AuthPosture`]. Bundled so
/// the posture is baked into the router at build time rather than read
/// from the environment on every request — which makes the enforcement
/// path deterministic under test and immune to mid-flight env mutation.
#[derive(Clone)]
pub struct AuthLayerState {
    pub auth_manager: Arc<AuthManager>,
    pub posture: AuthPosture,
}

/// Build a permissive AuthInfo for the `ROSHERA_DEV_INSECURE` bypass.
/// Every permission is granted; user_id is a stable sentinel so audit
/// logs are still distinguishable from real users.
///
/// This is reached only when the resolved [`AuthPosture`] is
/// [`AuthPosture::InsecureDevBypass`], which itself logs a loud warning
/// once at startup ([`AuthPosture::from_env`]). Full permissions —
/// including `DeleteGeometry` — so a local dev frontend can exercise the
/// whole surface without a credential.
fn dev_auth_info() -> AuthInfo {
    AuthInfo {
        user_id: "dev-insecure".to_string(),
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

/// Returns true when `auth_middleware` should admit this path without a
/// credential. This is the authentication surface's public allowlist —
/// the exact set of routes reachable before you hold a credential — and
/// it is deliberately tiny and enumerated (no prefix wildcards beyond
/// the two that are load-bearing) so a new route cannot fall into it by
/// accident:
///
/// * `/` and `/health` — liveness/readiness probes. An orchestrator
///   must be able to health-check the container without a credential.
/// * `/ws` — the WebSocket *upgrade*. The socket carries the full
///   geometry command surface and is authenticated in-band, per
///   connection, in `protocol::message_handlers`: a header-based 401
///   here would abort the upgrade before the client can send its
///   `Authenticate` frame. The in-band gate refuses every command frame
///   until authentication succeeds, so exempting the upgrade does not
///   expose the command surface.
/// * `/api/auth/login`, `/api/auth/register`, `/api/auth/refresh` — the
///   credential-issuing routes. Requiring a credential to reach the
///   only routes that mint one would make authentication unreachable.
///   `/api/auth/logout` is deliberately *not* here: revoking a session
///   requires presenting that session's token, which the middleware
///   validates like any other request.
fn path_is_exempt(path: &str) -> bool {
    matches!(
        path,
        "/" | "/health" | "/api/auth/login" | "/api/auth/register" | "/api/auth/refresh"
    ) || path == "/ws"
        || path.starts_with("/ws/")
}

/// Authentication middleware that validates JWT tokens or API keys.
///
/// Layered globally onto the router with an [`AuthLayerState`] carrying
/// the process's single `AuthManager` and the resolved [`AuthPosture`].
///
/// Path-based exemptions (`path_is_exempt`) skip `/` and `/health` so
/// liveness probes need no credential. **`/ws` is no longer exempt at
/// this layer for anything but the upgrade handshake** — the WebSocket
/// carries the full geometry command surface and is gated in-band per
/// connection in `protocol::message_handlers` (a header-based 401 here
/// would break the upgrade before the client can send its
/// `Authenticate` frame; see that module for the in-band gate).
///
/// Posture:
///
/// * [`AuthPosture::Required`] (default) — a missing, malformed, or
///   invalid `Authorization` header returns 401 immediately. There is
///   no anonymous pass-through: the previous soft mode is gone.
/// * [`AuthPosture::InsecureDevBypass`] (`ROSHERA_DEV_INSECURE=1`) —
///   inject a permissive dev `AuthInfo` and skip credential validation.
///   The posture logs a loud warning once at startup.
pub async fn auth_middleware(
    State(layer): State<AuthLayerState>,
    mut request: Request,
    next: Next,
) -> Response {
    match auth_middleware_inner(&layer, &mut request).await {
        Ok(()) => next.run(request).await,
        Err(e) => e.into_response(),
    }
}

/// Internal helper that performs the credential check and mutates the
/// request's extensions, returning `Ok(())` if the request should
/// proceed (with `AuthInfo` injected) or `Err(AuthError)` if it should
/// be rejected at the middleware boundary. Split out so the outer
/// `auth_middleware` returns plain `Response` — axum's
/// `from_fn_with_state` requires the middleware future's output to be
/// `IntoResponse`, and a `Result<…, AuthError>` falls foul of the
/// `FromFn` extractor-tuple resolver in axum 0.8 when mixed with the
/// `State` extractor.
async fn auth_middleware_inner(
    layer: &AuthLayerState,
    request: &mut Request,
) -> Result<(), AuthError> {
    // Exempt the public surface (health checks, WS upgrade, root).
    if path_is_exempt(request.uri().path()) {
        return Ok(());
    }

    if layer.posture == AuthPosture::InsecureDevBypass {
        request.extensions_mut().insert(dev_auth_info());
        return Ok(());
    }

    // AuthPosture::Required — a valid credential is mandatory.
    let auth_header = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .map(|s| s.to_string())
        .ok_or_else(|| AuthError {
            error: "Missing authorization header".to_string(),
            code: "AUTH_MISSING".to_string(),
            status: 401,
        })?;

    let auth_info = if let Some(token) = auth_header.strip_prefix("Bearer ") {
        validate_jwt(layer.auth_manager.as_ref(), token).await?
    } else if let Some(api_key) = auth_header.strip_prefix("ApiKey ") {
        validate_api_key(layer.auth_manager.as_ref(), api_key).await?
    } else {
        return Err(AuthError {
            error: "Invalid authorization format".to_string(),
            code: "AUTH_INVALID_FORMAT".to_string(),
            status: 401,
        });
    };

    // Insert auth info into request extensions
    request.extensions_mut().insert(auth_info);

    Ok(())
}

/// Permission-checking middleware (legacy `AuthError`-shaped variant).
///
/// Retained for callers that want the simple `AuthError` JSON shape.
/// New mutation routes should layer one of the typed
/// `require_*_geometry` middlewares below, which return the catalogued
/// `ApiError::permission_denied(...)` so the failure participates in
/// the stable error catalog the rest of the API exposes.
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

// =====================================================================
// AUDIT-H7 — typed per-route permission gates for kernel mutations.
//
// Every mutation handler exposed by the api-server (fillet, chamfer,
// boolean, extrude, delete-geometry, …) receives `AuthInfo` via the
// `FromRequestParts` impl above but, prior to AUDIT-H7, never inspected
// the `permissions` field. In soft mode (`ROSHERA_REQUIRE_AUTH` unset)
// the anonymous `AuthInfo` carries an empty permission list, so no
// caller is ever rejected; in strict mode the same handlers still
// happily executed against any valid token regardless of the scopes
// it actually carries. The middlewares below close that gap:
//
//   1. They are layered per-route via `.route_layer(...)` in
//      `build_router`, so adding a new mutation route forces the
//      developer to think about which permission gates it.
//   2. They short-circuit with `ApiError::permission_denied(...)`,
//      which serialises to the stable `permission_denied` code in
//      the error catalog (HTTP 403, non-retryable). Agents
//      pattern-matching on `error_code` get the same wire shape they
//      already handle from `/api/permissions/*` and the explicit
//      lock-conflict surface.
//   3. They are *enforced only when `ROSHERA_REQUIRE_AUTH=1`*. In
//      soft mode (development default) they pass through. This
//      matches `auth_middleware`'s own soft/strict mode matrix and
//      lets existing dev frontends that have not yet wired
//      Authorization headers continue working until the operator
//      flips the env var. The strict-mode toggle is the single
//      switch that activates both layers of the protection.
// =====================================================================

/// Inner helper shared by every `require_*_geometry` middleware. In
/// strict mode (`ROSHERA_REQUIRE_AUTH=1`) the request's `AuthInfo`
/// extension must contain `required`; otherwise the layer short-
/// circuits with `ApiError::permission_denied(name)`. In soft mode
/// the layer is a no-op and forwards to `next`.
async fn enforce_permission_layer(
    required: Permission,
    name: &'static str,
    request: Request,
    next: Next,
) -> Response {
    // The global `auth_middleware` runs before this per-route layer and
    // always injects an `AuthInfo`: a validated credential under
    // `AuthPosture::Required`, or a full-permission dev identity under
    // `AuthPosture::InsecureDevBypass`. This layer therefore only has to
    // refine the check to scope. A missing extension is unreachable in
    // normal operation (it would mean the front door was bypassed); we
    // fail closed on it regardless.
    let auth_info = match request.extensions().get::<AuthInfo>().cloned() {
        Some(info) => info,
        None => {
            return crate::error_catalog::ApiError::permission_denied(name).into_response();
        }
    };

    if auth_info.permissions.contains(&required) {
        return next.run(request).await;
    }

    tracing::warn!(
        target: "api_server.auth.permission_denied",
        user_id = %auth_info.user_id,
        required = %name,
        "rejected mutation: caller lacks required permission",
    );
    crate::error_catalog::ApiError::permission_denied(name).into_response()
}

/// Direct in-handler check used by handlers that don't take a
/// `route_layer` (typically because the same Axum path serves both a
/// read and a mutate verb, and only the mutating verb should gate).
/// Returns `Ok(())` to proceed, `Err(ApiError)` to short-circuit with
/// a 403.
///
/// The caller reaches this only after the global `auth_middleware` has
/// admitted the request, so `auth` is either a validated credential
/// (`AuthPosture::Required`) or the full-permission dev identity
/// (`AuthPosture::InsecureDevBypass`). The check is therefore an
/// unconditional scope test. Use the route-layer form when possible —
/// it keeps the policy at the router definition site rather than buried
/// in handler bodies.
pub fn enforce_permission(
    auth: &AuthInfo,
    required: Permission,
    name: &'static str,
) -> Result<(), crate::error_catalog::ApiError> {
    if auth.permissions.contains(&required) {
        return Ok(());
    }
    tracing::warn!(
        target: "api_server.auth.permission_denied",
        user_id = %auth.user_id,
        required = %name,
        "rejected mutation: caller lacks required permission",
    );
    Err(crate::error_catalog::ApiError::permission_denied(name))
}

/// Route layer for endpoints that introduce new geometry into the
/// model: `/api/geometry` (POST), `/api/geometry/extrude` (POST),
/// `/api/sketch/{id}/extrude` (POST), `/api/sketch/{id}/revolve`
/// (POST), …
pub async fn require_create_geometry(request: Request, next: Next) -> Response {
    enforce_permission_layer(Permission::CreateGeometry, "create_geometry", request, next).await
}

/// Route layer for endpoints that mutate an existing solid in place:
/// fillet, chamfer, shell, mirror, pattern, boolean, face-extrude,
/// transform, …
pub async fn require_modify_geometry(request: Request, next: Next) -> Response {
    enforce_permission_layer(Permission::ModifyGeometry, "modify_geometry", request, next).await
}

/// Route layer for endpoints that delete a solid from the model.
pub async fn require_delete_geometry(request: Request, next: Next) -> Response {
    enforce_permission_layer(Permission::DeleteGeometry, "delete_geometry", request, next).await
}

/// Route layer for endpoints that export geometry to STL/OBJ/STEP/…
pub async fn require_export_geometry(request: Request, next: Next) -> Response {
    enforce_permission_layer(Permission::ExportGeometry, "export_geometry", request, next).await
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

/// Get default permissions for authenticated users.
///
/// Includes `DeleteGeometry`: a logged-in human driving the frontend
/// must be able to delete a solid. Omitting it (as an earlier revision
/// did) meant that under `AuthPosture::Required` a valid JWT user was
/// silently 403'd on every delete while creates and modifies succeeded
/// — an inconsistency that only surfaces once auth is actually enforced.
/// Scoped-down credentials (API keys) still carry exactly the
/// permissions minted into them; this default applies to JWT sessions.
fn get_default_user_permissions() -> Vec<Permission> {
    vec![
        Permission::CreateGeometry,
        Permission::ModifyGeometry,
        Permission::DeleteGeometry,
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
/// The global `auth_middleware` is the canonical injector: under
/// `AuthPosture::Required` it has already validated a credential and
/// inserted the `AuthInfo`; under `AuthPosture::InsecureDevBypass` it
/// inserted the permissive dev identity. This extractor therefore reads
/// the extension the middleware placed. If the extension is absent — a
/// handler reached without passing the front door — it fails closed with
/// 401 rather than fabricating an identity, so a routing mistake cannot
/// silently grant access.
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

    /// Permission matrix for `enforce_permission`.
    ///
    /// Since the soft/strict env toggle was removed (authentication is
    /// enforced by default; the only bypass is the whole-process
    /// `AuthPosture::InsecureDevBypass`, which injects a full-permission
    /// identity upstream), `enforce_permission` is an unconditional
    /// scope test. No env mutation, so this is fully deterministic.
    #[test]
    fn enforce_permission_is_an_unconditional_scope_check() {
        let granted = AuthInfo {
            user_id: "alice".into(),
            session_id: None,
            permissions: vec![Permission::ModifyGeometry],
            roles: vec![],
            is_api_key: false,
        };
        let anon = AuthInfo {
            user_id: "anonymous".into(),
            session_id: None,
            permissions: vec![],
            roles: vec![],
            is_api_key: false,
        };

        // Holding the required scope permits.
        assert!(
            enforce_permission(&granted, Permission::ModifyGeometry, "modify_geometry").is_ok()
        );

        // An empty permission list is always rejected — there is no
        // longer a mode in which a scopeless identity passes.
        assert!(
            enforce_permission(&anon, Permission::ModifyGeometry, "modify_geometry").is_err(),
            "an identity holding no permissions must never be admitted to a mutation"
        );

        // Holding one scope does not grant another.
        assert!(
            enforce_permission(&granted, Permission::DeleteGeometry, "delete_geometry").is_err(),
            "holding ModifyGeometry must not grant DeleteGeometry"
        );
    }

    /// The posture default is secure, and the insecure opt-out is
    /// explicit, single-valued, and decoupled from `ROSHERA_DEV_BRIDGE`.
    #[test]
    fn auth_posture_defaults_secure_and_opt_out_is_explicit() {
        // Empty environment → Required.
        assert_eq!(
            AuthPosture::from_env_with(|_| None),
            AuthPosture::Required,
            "a server with no ROSHERA_* variables must enforce authentication"
        );

        // The opt-out fires only on its own variable, set truthy.
        for value in ["1", "true", "TRUE"] {
            assert_eq!(
                AuthPosture::from_env_with(
                    |k| (k == "ROSHERA_DEV_INSECURE").then(|| value.to_string())
                ),
                AuthPosture::InsecureDevBypass,
                "ROSHERA_DEV_INSECURE={value} must select the dev bypass"
            );
        }

        // A merely-present or falsey value is not an opt-out.
        for value in ["0", "false", "", "yes", "no"] {
            assert_eq!(
                AuthPosture::from_env_with(
                    |k| (k == "ROSHERA_DEV_INSECURE").then(|| value.to_string())
                ),
                AuthPosture::Required,
                "ROSHERA_DEV_INSECURE={value:?} must NOT disable authentication"
            );
        }

        // ROSHERA_DEV_BRIDGE mounts a debug viewport; it must not move
        // the auth posture. Overloading it with a bypass is how a bypass
        // ships by accident.
        assert_eq!(
            AuthPosture::from_env_with(|k| (k == "ROSHERA_DEV_BRIDGE").then(|| "1".to_string())),
            AuthPosture::Required,
            "ROSHERA_DEV_BRIDGE must not disable authentication"
        );
    }
}
