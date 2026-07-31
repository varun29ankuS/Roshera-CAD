//! Handlers for the AI provider connection dialog:
//! `GET/PUT/DELETE /api/ai/provider`, `POST /api/ai/provider/test`.
//!
//! All four routes are gated on `Permission::ModifySettings`
//! (`auth_middleware::require_modify_settings`) at the router-definition
//! site — the dialog exposes CLI sign-in state and the credential
//! resolution/shadow report, configuration surface a Viewer-role caller
//! has no business probing even though none of it is a raw secret.
//!
//! Only `PUT` mutates process state (persists, registers the live
//! provider, and — subscription-CLI mode only — repins goose and scrubs
//! the process env). `POST /test` runs the identical validation path and
//! stops before any of that: "test this before saving" without side
//! effects, sharing one code path with `PUT` so the two can never
//! diverge on what counts as a valid credential.

use crate::ai_provider_config::{self, StoredProviderConfig};
use crate::error_catalog::{ApiError, ErrorCode};
use crate::AppState;
use ai_integration::providers::allowlist::{
    self, CredentialMode, WiringStatus, PROVIDER_ALLOWLIST,
};
use ai_integration::providers::claude::{ClaudeConfig, ClaudeCredential};
use ai_integration::providers::ClaudeProvider;
use axum::extract::State;
use axum::response::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;

/// Request body for `PUT /api/ai/provider` and `POST /api/ai/provider/test`.
#[derive(Debug, Deserialize)]
pub struct ProviderConfigRequest {
    pub provider: String,
    pub mode: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub profile_name: Option<String>,
    /// Must be `true` for any mode the allowlist marks
    /// `spawns_local_process` (currently `subscription_cli`). A
    /// spawning mode requested without this is refused by name, never
    /// silently accepted.
    #[serde(default)]
    pub consent_spawn_local_process: bool,
}

/// What credential-mode validation produced, before the `PUT` handler
/// decides what to persist / register / scrub. Shared between `PUT`
/// (which acts on this) and `POST /test` (which reports it and stops).
/// `Debug` is needed only for `Result::unwrap_err()` in this module's
/// own tests — never logged or serialized (it can carry a live token).
#[derive(Debug)]
enum Validated {
    ApiKey(String),
    OauthToken {
        token: String,
        profile_name: String,
    },
    SubscriptionCli {
        cli_path: String,
        profile_name: String,
    },
}

/// `GET /api/ai/provider` — active config summary (never the secret),
/// the server-owned allowlist, the resolution/shadow chain, and CLI
/// sign-in detection.
pub async fn get_provider(State(state): State<AppState>) -> Json<Value> {
    let mgr = &state.ai_provider_manager;
    let stored = mgr.stored().await;
    let chain = mgr.resolution_report().await;
    let active_source = chain.iter().find(|e| e.active).map(|e| e.source);

    let active = stored.as_ref().map(|s| {
        json!({
            "provider": s.provider,
            "mode": s.mode,
            "profile_name": s.profile_name,
            "saved_at": s.saved_at,
            "has_api_key": s.api_key.is_some(),
        })
    });

    let env_snapshot = mgr.env_snapshot();

    Json(json!({
        "active": active,
        "ai_configured": state.ai_configured.load(Ordering::SeqCst),
        "resolution": {
            "chain": chain,
            "active_source": active_source,
            // Presence at server boot — before anything could have
            // scrubbed the credential env vars for subscription-CLI
            // mode. Explains the "note" fields in `chain` above without
            // making the caller cross-reference process history.
            "env_snapshot_at_boot": {
                "anthropic_api_key_was_set": env_snapshot.anthropic_api_key_was_set,
                "anthropic_auth_token_was_set": env_snapshot.anthropic_auth_token_was_set,
            },
        },
        "allowlist": PROVIDER_ALLOWLIST,
        "cli": {
            "claude": ai_provider_config::detect_claude_cli(),
            "codex": ai_provider_config::detect_codex_cli(),
        },
        "appdata_anthropic": ai_provider_config::detect_appdata_anthropic(),
    }))
}

/// `PUT /api/ai/provider` — validate against the live vendor / local CLI
/// state, then persist + register + (subscription-CLI only) repin goose
/// and scrub the process env. Nothing is saved unless validation
/// succeeds first.
pub async fn put_provider(
    State(state): State<AppState>,
    Json(payload): Json<ProviderConfigRequest>,
) -> Result<Json<Value>, ApiError> {
    let validated = validate_request(&payload).await?;

    match validated {
        Validated::ApiKey(key) => {
            let stored = StoredProviderConfig {
                provider: "anthropic".to_string(),
                mode: CredentialMode::ApiKey.as_str().to_string(),
                api_key: Some(key.clone()),
                profile_name: None,
                saved_at: chrono::Utc::now(),
            };
            let acl = state
                .ai_provider_manager
                .save(stored)
                .await
                .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;

            register_claude_credential(&state, ClaudeCredential::ApiKey(key)).await;
            state.ai_configured.store(true, Ordering::SeqCst);

            Ok(Json(json!({
                "success": true,
                "provider": "anthropic",
                "mode": "api_key",
                "acl": acl,
            })))
        }
        Validated::OauthToken {
            token,
            profile_name,
        } => {
            let stored = StoredProviderConfig {
                provider: "anthropic".to_string(),
                mode: CredentialMode::OauthProfile.as_str().to_string(),
                api_key: None,
                profile_name: Some(profile_name.clone()),
                saved_at: chrono::Utc::now(),
            };
            let acl = state
                .ai_provider_manager
                .save(stored)
                .await
                .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;

            register_claude_credential(&state, ClaudeCredential::OauthAccessToken(token)).await;
            state.ai_configured.store(true, Ordering::SeqCst);

            Ok(Json(json!({
                "success": true,
                "provider": "anthropic",
                "mode": "oauth_profile",
                "profile_name": profile_name,
                "acl": acl,
            })))
        }
        Validated::SubscriptionCli {
            cli_path,
            profile_name,
        } => {
            let stored = StoredProviderConfig {
                provider: "anthropic".to_string(),
                mode: CredentialMode::SubscriptionCli.as_str().to_string(),
                api_key: None,
                profile_name: Some(profile_name.clone()),
                saved_at: chrono::Utc::now(),
            };
            let acl = state
                .ai_provider_manager
                .save(stored)
                .await
                .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;

            ai_provider_config::repin_goose_to_claude_code(std::path::Path::new(&cli_path))
                .map_err(|e| ApiError::new(ErrorCode::Internal, e))?;

            // Must run only after the repin above succeeded, and relies
            // on the boot-time `EnvSnapshot` already having been
            // captured (it was — at process start, in
            // `AiProviderManager::boot`, before any request could reach
            // this handler). See
            // `ai_provider_config::scrub_anthropic_env_for_subscription_mode`
            // for why this exists: goose's claude-code spawn path
            // inherits ANTHROPIC_API_KEY/ANTHROPIC_AUTH_TOKEN otherwise.
            ai_provider_config::scrub_anthropic_env_for_subscription_mode();

            Ok(Json(json!({
                "success": true,
                "provider": "anthropic",
                "mode": "subscription_cli",
                "profile_name": profile_name,
                "acl": acl,
                "note": "this wires the agent (/acp) surface via the Claude \
                         Code CLI. REST routes (/api/ai/command) still need \
                         an api_key or oauth_profile credential — their \
                         tool_use protocol isn't carried over the CLI \
                         transport.",
            })))
        }
    }
}

/// `DELETE /api/ai/provider` — remove the persisted runtime config, then
/// honestly re-resolve: if `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN`
/// is still present in the environment, that becomes the new active
/// source (matching the resolution chain GET reports), rather than
/// hardcoding `ai_configured = false` and lying about it.
pub async fn delete_provider(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    state
        .ai_provider_manager
        .delete()
        .await
        .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;

    if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        if !key.is_empty() {
            register_claude_credential(&state, ClaudeCredential::ApiKey(key)).await;
            state.ai_configured.store(true, Ordering::SeqCst);
            return Ok(Json(json!({
                "success": true,
                "ai_configured": true,
                "fallback_source": "env:ANTHROPIC_API_KEY",
            })));
        }
    }
    if let Ok(token) = std::env::var("ANTHROPIC_AUTH_TOKEN") {
        if !token.is_empty() {
            register_claude_credential(&state, ClaudeCredential::OauthAccessToken(token)).await;
            state.ai_configured.store(true, Ordering::SeqCst);
            return Ok(Json(json!({
                "success": true,
                "ai_configured": true,
                "fallback_source": "env:ANTHROPIC_AUTH_TOKEN",
            })));
        }
    }

    {
        let mut mgr = state.provider_manager.lock().await;
        // Clear the active LLM name so `ProviderManager::llm()` misses
        // the lookup and returns `ProviderUnavailable`, rather than
        // leaving a stale credential registered under "claude" that
        // `ai_configured = false` would then contradict.
        mgr.set_active(String::new(), String::new(), None);
    }
    state.ai_configured.store(false, Ordering::SeqCst);

    Ok(Json(json!({
        "success": true,
        "ai_configured": false,
        "fallback_source": Value::Null,
    })))
}

/// `POST /api/ai/provider/test` — run the exact same validation as
/// `PUT` and report the outcome, without persisting, registering,
/// repinning, or scrubbing anything.
pub async fn test_provider(
    Json(payload): Json<ProviderConfigRequest>,
) -> Result<Json<Value>, ApiError> {
    let validated = validate_request(&payload).await?;
    let (mode, detail) = match validated {
        Validated::ApiKey(_) => ("api_key", json!({})),
        Validated::OauthToken { profile_name, .. } => {
            ("oauth_profile", json!({ "profile_name": profile_name }))
        }
        Validated::SubscriptionCli {
            cli_path,
            profile_name,
        } => (
            "subscription_cli",
            json!({ "cli_path": cli_path, "profile_name": profile_name }),
        ),
    };
    Ok(Json(json!({
        "success": true,
        "provider": "anthropic",
        "mode": mode,
        "detail": detail,
    })))
}

// ── Shared validation ───────────────────────────────────────────────────

/// Resolve the requested (provider, mode) against the server-owned
/// allowlist, refuse `SeamOnly` modes and unconsented spawning modes by
/// name, then run the mode-specific live validation. Shared by `PUT` and
/// `POST /test` so "test before saving" and "save" can never diverge.
async fn validate_request(payload: &ProviderConfigRequest) -> Result<Validated, ApiError> {
    let mode = CredentialMode::parse(&payload.mode).ok_or_else(|| {
        ApiError::ai_provider_refused(
            format!("unknown credential mode '{}'", payload.mode),
            json!({
                "requested_mode": payload.mode,
                "valid_modes": ["api_key", "oauth_profile", "workload_identity", "subscription_cli"],
            }),
        )
    })?;

    let (_provider, mode_entry) =
        allowlist::resolve_mode(&payload.provider, mode).map_err(|refusal| {
            let details = serde_json::to_value(&refusal).unwrap_or(Value::Null);
            ApiError::ai_provider_refused(refusal.to_string(), details)
        })?;

    if let WiringStatus::SeamOnly(reason) = mode_entry.wiring {
        return Err(ApiError::ai_provider_refused(
            format!(
                "{}/{} is seam-only in this build and refused by name — {reason}",
                payload.provider,
                mode.as_str()
            ),
            json!({
                "provider": payload.provider,
                "mode": mode.as_str(),
                "reason": reason,
            }),
        ));
    }

    if mode_entry.spawns_local_process && !payload.consent_spawn_local_process {
        return Err(ApiError::ai_provider_refused(
            format!(
                "{} spawns a local process and requires explicit consent \
                 (consent_spawn_local_process: true)",
                mode.as_str()
            ),
            json!({
                "provider": payload.provider,
                "mode": mode.as_str(),
                "spawns_local_process": true,
            }),
        ));
    }

    match mode {
        CredentialMode::ApiKey => validate_api_key(payload).await,
        CredentialMode::OauthProfile => validate_oauth_profile(payload).await,
        CredentialMode::SubscriptionCli => validate_subscription_cli(payload),
        CredentialMode::WorkloadIdentity => {
            // Unreachable today: every allowlisted WorkloadIdentity entry
            // is `SeamOnly` and was already refused above. A typed
            // refusal (not `unreachable!()`, which the workspace lints
            // deny) so a future `Wired` entry fails loudly here instead
            // of silently falling through with no credential to test.
            Err(ApiError::ai_provider_refused(
                "workload_identity has no PUT-time credential to validate \
                 in this build"
                    .to_string(),
                json!({ "provider": payload.provider, "mode": "workload_identity" }),
            ))
        }
    }
}

async fn validate_api_key(payload: &ProviderConfigRequest) -> Result<Validated, ApiError> {
    let key = payload
        .api_key
        .clone()
        .filter(|k| !k.is_empty())
        .ok_or_else(|| ApiError::missing_parameter("api_key"))?;

    let probe = ClaudeProvider::with_config(ClaudeConfig {
        credential: Some(ClaudeCredential::ApiKey(key.clone())),
        ..Default::default()
    });
    probe
        .validate_credential()
        .await
        .map_err(|e| ApiError::ai_credential_invalid(e.to_string()))?;

    Ok(Validated::ApiKey(key))
}

async fn validate_oauth_profile(payload: &ProviderConfigRequest) -> Result<Validated, ApiError> {
    let token = std::env::var("ANTHROPIC_AUTH_TOKEN")
        .ok()
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            ApiError::ai_credential_invalid(
                "no OAuth token available to validate 'oauth_profile' on this \
                 machine — sign in via the vendor's own login tooling first \
                 (this surfaces as ANTHROPIC_AUTH_TOKEN, or eventually the \
                 %APPDATA%\\Anthropic credential store) and retry",
            )
        })?;

    let probe = ClaudeProvider::with_config(ClaudeConfig {
        credential: Some(ClaudeCredential::OauthAccessToken(token.clone())),
        ..Default::default()
    });
    probe
        .validate_credential()
        .await
        .map_err(|e| ApiError::ai_credential_invalid(e.to_string()))?;

    let profile_name = payload
        .profile_name
        .clone()
        .unwrap_or_else(|| "default".to_string());
    Ok(Validated::OauthToken {
        token,
        profile_name,
    })
}

fn validate_subscription_cli(payload: &ProviderConfigRequest) -> Result<Validated, ApiError> {
    let cli = ai_provider_config::detect_claude_cli();
    if !cli.installed {
        return Err(ApiError::ai_credential_invalid(
            "Claude Code CLI not found — expected the npm shim at \
             %APPDATA%\\npm\\claude.cmd; install it first",
        ));
    }
    if !cli.signed_in {
        return Err(ApiError::ai_credential_invalid(
            "Claude Code CLI is installed but not signed in (no \
             ~/.claude/.credentials.json) — run the CLI's own login flow \
             first; Roshera does not spawn an interactive login",
        ));
    }
    // `installed` is only ever true when `detect_claude_cli` also
    // populated `path` — a defensive typed refusal (not
    // expect/unwrap, which the workspace lints deny) rather than an
    // assumption.
    let cli_path = cli.path.ok_or_else(|| {
        ApiError::new(
            ErrorCode::Internal,
            "CLI reported installed with no resolved path".to_string(),
        )
    })?;

    let profile_name = payload
        .profile_name
        .clone()
        .unwrap_or_else(|| "default".to_string());
    Ok(Validated::SubscriptionCli {
        cli_path,
        profile_name,
    })
}

/// Register a freshly-validated Claude credential as the process's
/// active LLM provider — the same registration `main()` performs at
/// boot for `ANTHROPIC_API_KEY`, now reachable without a restart.
async fn register_claude_credential(state: &AppState, credential: ClaudeCredential) {
    let provider = ClaudeProvider::with_config(ClaudeConfig {
        credential: Some(credential),
        ..Default::default()
    });
    let mut mgr = state.provider_manager.lock().await;
    mgr.register_llm("claude".to_string(), Box::new(provider));
    mgr.set_active(String::new(), "claude".to_string(), None);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::router_integration_tests::make_test_state;

    fn request(provider: &str, mode: &str) -> ProviderConfigRequest {
        ProviderConfigRequest {
            provider: provider.to_string(),
            mode: mode.to_string(),
            api_key: None,
            profile_name: None,
            consent_spawn_local_process: false,
        }
    }

    #[tokio::test]
    async fn get_provider_reports_unconfigured_by_default() {
        let state = make_test_state().await;
        let Json(body) = get_provider(State(state)).await;
        assert_eq!(body["active"], Value::Null);
        assert_eq!(body["ai_configured"], json!(false));
        assert!(body["resolution"]["chain"].is_array());
        assert!(body["allowlist"].is_array());
        assert!(body["cli"]["claude"].is_object());
        assert!(body["appdata_anthropic"]["checked_path"].is_string());
    }

    #[tokio::test]
    async fn put_provider_refuses_unallowlisted_provider_by_name() {
        let payload = request("ollama", "api_key");
        let err = validate_request(&payload).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::AiProviderRefused);
        let details = err.details.expect("refusal must carry details");
        assert_eq!(details["refusal"]["NotAllowlisted"]["requested"], "ollama");
    }

    #[tokio::test]
    async fn put_provider_refuses_unknown_mode_string_by_name() {
        let payload = request("anthropic", "not_a_real_mode");
        let err = validate_request(&payload).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::AiProviderRefused);
    }

    #[tokio::test]
    async fn put_provider_refuses_seam_only_mode_naming_the_reason() {
        // anthropic/workload_identity is SeamOnly in this build —
        // must be refused by name, never silently accepted.
        let payload = request("anthropic", "workload_identity");
        let err = validate_request(&payload).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::AiProviderRefused);
        assert!(
            err.error.contains("seam-only"),
            "refusal must name the seam, got: {}",
            err.error
        );
    }

    #[tokio::test]
    async fn put_provider_refuses_openai_api_key_seam_only() {
        let payload = request("openai", "api_key");
        let err = validate_request(&payload).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::AiProviderRefused);
        assert!(err.error.contains("seam-only"));
    }

    #[tokio::test]
    async fn put_provider_refuses_subscription_cli_without_consent() {
        // Consent gating must fire before any CLI detection — the test
        // must be deterministic regardless of whether Claude Code is
        // actually installed on the machine running this suite.
        let mut payload = request("anthropic", "subscription_cli");
        payload.consent_spawn_local_process = false;
        let err = validate_request(&payload).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::AiProviderRefused);
        assert!(err.error.contains("consent"));
    }

    #[tokio::test]
    async fn put_provider_api_key_mode_without_key_is_missing_parameter() {
        let payload = request("anthropic", "api_key");
        let err = validate_request(&payload).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::MissingParameter);
    }

    #[tokio::test]
    async fn put_provider_oauth_mode_without_env_token_is_credential_invalid() {
        // ANTHROPIC_AUTH_TOKEN is not expected to be set in the test
        // environment; if it somehow is, this test would need the
        // network gate too, so assert only the documented failure mode.
        if std::env::var("ANTHROPIC_AUTH_TOKEN").is_ok() {
            return;
        }
        let payload = request("anthropic", "oauth_profile");
        let err = validate_request(&payload).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::AiCredentialInvalid);
    }

    #[tokio::test]
    async fn delete_provider_is_idempotent_and_reports_no_fallback_when_env_empty() {
        // Only meaningful when the real env vars are absent — mirrors
        // the oauth test's guard so this suite stays deterministic
        // regardless of the machine it runs on.
        if std::env::var("ANTHROPIC_API_KEY").is_ok()
            || std::env::var("ANTHROPIC_AUTH_TOKEN").is_ok()
        {
            return;
        }
        let state = make_test_state().await;
        let Json(first) = delete_provider(State(state.clone()))
            .await
            .expect("delete must succeed");
        assert_eq!(first["ai_configured"], json!(false));
        assert_eq!(first["fallback_source"], Value::Null);

        // Idempotent: deleting again (nothing was ever saved) still
        // succeeds rather than 404ing.
        let Json(second) = delete_provider(State(state))
            .await
            .expect("second delete must succeed");
        assert_eq!(second["ai_configured"], json!(false));
    }
}
