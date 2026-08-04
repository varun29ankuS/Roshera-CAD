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

use crate::ai_provider_config::{self, ModelVerificationState, StoredProviderConfig};
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
    /// User-selected model, or absent/`"default"` for "the provider's own
    /// choice". Never a client-pushed runtime override — this is the
    /// authenticated server-side config path (`PUT /api/ai/provider`);
    /// the `/acp` surface's own `_meta.provider` hole stays closed
    /// (`acp_gate.rs`). See `resolve_requested_model` for how this is
    /// normalized and validated before anything is persisted.
    #[serde(default)]
    pub model: Option<String>,
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
    ApiKey {
        /// The allowlisted provider this key was validated for
        /// ("anthropic", "xai", "mistral", "glm", "kimi", ...) — needed
        /// because, unlike every other `Validated` variant, `api_key`
        /// mode is now wired for more than one provider and `put_provider`
        /// must persist and repin the one actually requested, never a
        /// hardcoded "anthropic".
        provider: String,
        key: String,
        model: Option<String>,
        model_verified: Option<bool>,
    },
    OauthToken {
        token: String,
        profile_name: String,
        model: Option<String>,
        model_verified: Option<bool>,
    },
    SubscriptionCli {
        cli_path: String,
        profile_name: String,
        model: Option<String>,
        model_verified: Option<bool>,
    },
}

/// `None` / `"default"` (case-sensitive — the literal sentinel
/// `claude-code`'s own `CLAUDE_CODE_DEFAULT_MODEL` uses) / whitespace-only
/// all normalize to `None`, meaning "the provider's own choice" — shared
/// by every mode's model handling so none of them can drift on what
/// counts as "no override requested".
fn normalize_model_request(requested: Option<&str>) -> Option<String> {
    requested
        .map(str::trim)
        .filter(|m| !m.is_empty() && *m != "default")
        .map(str::to_string)
}

/// Normalize + validate `payload.model` against the credential that just
/// proved live. `None` (see [`normalize_model_request`]) means "the
/// provider's own choice" and is always accepted without a network
/// round-trip — this function must never invent a concrete model name
/// for that case, and must never claim it validated something it did not
/// check.
///
/// For `anthropic`'s `api_key` / `oauth_profile`, an explicit model is
/// checked against Anthropic's own model-listing endpoint
/// (`ClaudeProvider::validate_model`) — the authoritative source, not a
/// hardcoded menu — and the save is refused by name (typed
/// `AiModelRejected`) if the provider does not recognize it.
///
/// For `subscription_cli`, no side-effect-free synchronous enumeration
/// exists on this server (the Claude Code CLI's own model listing
/// requires spawning a subprocess per check — `fetch_supported_models`
/// in goose's `claude_code.rs`, not something this handler calls per
/// save). The string is accepted, but persisted with `model_verified:
/// false` — surfaced honestly in both the `PUT` and `GET` responses —
/// rather than presented as a confirmed model; `PUT`'s `subscription_cli`
/// branch kicks off a bounded background check that can later flip this
/// (see `ai_provider_config::verify_subscription_cli_model`). An invalid
/// model also surfaces at first use regardless: `goose_acp::apply_configured_model`
/// applies it at `session/new`, and the Claude Code CLI itself rejects
/// an unrecognized model on that session's first turn.
async fn resolve_requested_model(
    requested: Option<&str>,
    credential: Option<&ClaudeCredential>,
    is_subscription_cli: bool,
) -> Result<(Option<String>, Option<bool>), ApiError> {
    let Some(model) = normalize_model_request(requested) else {
        return Ok((None, None));
    };

    if is_subscription_cli {
        return Ok((Some(model), Some(false)));
    }
    let model = model.as_str();

    let Some(credential) = credential else {
        return Err(ApiError::ai_model_rejected(
            model,
            "no credential available to verify this model against",
        ));
    };
    let probe = ClaudeProvider::with_config(ClaudeConfig {
        credential: Some(credential.clone()),
        ..Default::default()
    });
    probe
        .validate_model(model)
        .await
        .map_err(|e| ApiError::ai_model_rejected(model, e.to_string()))?;
    Ok((Some(model.to_string()), Some(true)))
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
            // `None` means "the provider's own default choice", never a
            // fabricated model name. `model_verified: false` (only
            // reachable for subscription_cli) must render as an honest
            // caveat, not as a confirmed-active model.
            "model": s.model,
            "model_verified": s.model_verified,
            // The full three(-plus-pending)-state detail behind
            // `model_verified` for subscription_cli: `pending` while the
            // bounded background check is still running, `rejected`
            // naming the model and the CLI's own reason, `unknown` when
            // the check itself couldn't run to a conclusion (never a
            // stand-in for verified or rejected). `None` for api_key/
            // oauth_profile or when no model override was requested.
            "model_verification": s.model_verification,
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

/// End every live ACP connection minted under the previous provider, so
/// the next prompt necessarily starts a fresh session on the new one.
/// Called by every successful `PUT` branch — subscription CLI,
/// declarative vendor, and the anthropic default — AFTER its persist +
/// repin succeeded, never on a refused or failed save.
///
/// This is the fundamental fix for the stale-session defect: goose stores
/// the provider ON THE SESSION and restores it (`Restoring evicted
/// session … (provider: Some("sarvam"))`, observed live), so repinning
/// alone changed only future sessions while the open browser tab kept
/// prompting the old provider indefinitely. The bump makes that state
/// unreachable — the tab's next request gets the bare-404 reestablish
/// signature `acp-client.ts` already recovers from (see
/// `acp_provider_epoch.rs`, including the deliberate choice to let an
/// in-flight turn finish on the provider that started it).
fn invalidate_agent_sessions(state: &AppState, provider: &str, mode: &str) {
    let epoch = state.acp_provider_epoch.invalidate_connections();
    tracing::info!(
        target: "goose_acp",
        provider,
        mode,
        epoch,
        "provider repinned — ACP connections from the previous provider \
         are ended; the next prompt starts a fresh session on the new one"
    );
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
        Validated::ApiKey {
            provider,
            key,
            model,
            model_verified,
        } => {
            let stored = StoredProviderConfig {
                provider: provider.clone(),
                mode: CredentialMode::ApiKey.as_str().to_string(),
                api_key: Some(key.clone()),
                profile_name: None,
                model: model.clone(),
                model_verified,
                model_verification: None,
                saved_at: chrono::Utc::now(),
            };
            let acl = state
                .ai_provider_manager
                .save(stored)
                .await
                .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;

            let repin_note = if provider == "anthropic" {
                // Roshera's own native ClaudeProvider serves BOTH the
                // REST `/api/ai/command` surface and (via the same
                // ANTHROPIC_API_KEY goose's hardcoded default already
                // reads) the /acp agent surface — no explicit goose
                // repin needed, exactly as before this change.
                register_claude_credential(&state, ClaudeCredential::ApiKey(key)).await;
                state.ai_configured.store(true, Ordering::SeqCst);
                None
            } else {
                // No Roshera-native LLMProvider exists for these vendors
                // — /api/ai/command stays unavailable through this key.
                // Only the /acp agent surface is wired, via goose's own
                // provider for this vendor.
                ai_provider_config::repin_goose_to_declarative_provider(
                    &provider,
                    &key,
                    model.as_deref(),
                )
                .map_err(|e| ApiError::new(ErrorCode::Internal, e))?;
                Some(format!(
                    "this wires the agent (/acp) surface only, via goose's own \
                     provider for '{provider}'. REST routes (/api/ai/command) \
                     have no native Roshera provider for this vendor and remain \
                     unavailable through this key."
                ))
            };

            invalidate_agent_sessions(&state, &provider, "api_key");

            Ok(Json(json!({
                "success": true,
                "provider": provider,
                "mode": "api_key",
                "model": model,
                "model_verified": model_verified,
                "agent_sessions_invalidated": true,
                "acl": acl,
                "note": repin_note,
            })))
        }
        Validated::OauthToken {
            token,
            profile_name,
            model,
            model_verified,
        } => {
            let stored = StoredProviderConfig {
                provider: "anthropic".to_string(),
                mode: CredentialMode::OauthProfile.as_str().to_string(),
                api_key: None,
                profile_name: Some(profile_name.clone()),
                model: model.clone(),
                model_verified,
                model_verification: None,
                saved_at: chrono::Utc::now(),
            };
            let acl = state
                .ai_provider_manager
                .save(stored)
                .await
                .map_err(|e| ApiError::new(ErrorCode::Internal, e.to_string()))?;

            register_claude_credential(&state, ClaudeCredential::OauthAccessToken(token)).await;
            state.ai_configured.store(true, Ordering::SeqCst);

            invalidate_agent_sessions(&state, "anthropic", "oauth_profile");

            Ok(Json(json!({
                "success": true,
                "provider": "anthropic",
                "mode": "oauth_profile",
                "profile_name": profile_name,
                "model": model,
                "model_verified": model_verified,
                "agent_sessions_invalidated": true,
                "acl": acl,
            })))
        }
        Validated::SubscriptionCli {
            cli_path,
            profile_name,
            model,
            model_verified: _,
        } => {
            // Decide whether this model was already proven (or
            // disproven) by a previous save's background check, or needs
            // a fresh one kicked off after this save returns. Re-verify
            // only when the model actually changed (or nothing
            // conclusive was ever recorded) — never on every PUT of the
            // same model, and never on a GET (GET only ever reads
            // whatever is already stored).
            let previous = state.ai_provider_manager.stored().await;
            let model_verification: Option<ModelVerificationState> = model.as_deref().map(|m| {
                let cached = previous
                    .as_ref()
                    .filter(|p| p.model.as_deref() == Some(m))
                    .and_then(|p| p.model_verification.clone());
                match cached {
                    Some(state @ ModelVerificationState::Verified) => state,
                    Some(state @ ModelVerificationState::Rejected { .. }) => state,
                    // No cache, the model changed, or the last attempt
                    // was itself inconclusive (`Unknown`/`Pending`) —
                    // none of those are a terminal answer worth reusing.
                    _ => ModelVerificationState::Pending,
                }
            });
            let should_spawn_check =
                matches!(model_verification, Some(ModelVerificationState::Pending));
            // Derived from the (possibly reused) verification state, not
            // from `resolve_requested_model`'s always-`Some(false)`
            // subscription_cli default — a reused `Verified` outcome
            // must be reported as verified immediately, not lag a GET
            // behind what's already known.
            let model_verified = match &model_verification {
                Some(ModelVerificationState::Verified) => Some(true),
                Some(_) => Some(false),
                None => None,
            };

            let stored = StoredProviderConfig {
                provider: "anthropic".to_string(),
                mode: CredentialMode::SubscriptionCli.as_str().to_string(),
                api_key: None,
                profile_name: Some(profile_name.clone()),
                model: model.clone(),
                model_verified,
                model_verification: model_verification.clone(),
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

            invalidate_agent_sessions(&state, "anthropic", "subscription_cli");

            // Off the request path: the PUT above already returned a
            // fast, saved response. This spawns a bounded (~45s),
            // killed-on-timeout probe of the live CLI and persists
            // whatever it finds via `AiProviderManager::update_model_verification`
            // — never awaited here, never blocking this handler.
            if should_spawn_check {
                if let Some(m) = model.clone() {
                    let manager = state.ai_provider_manager.clone();
                    let cli_path_buf = std::path::PathBuf::from(&cli_path);
                    let probe = ai_provider_config::default_model_probe();
                    tokio::spawn(ai_provider_config::verify_subscription_cli_model(
                        probe,
                        cli_path_buf,
                        m,
                        manager,
                    ));
                }
            }

            Ok(Json(json!({
                "success": true,
                "provider": "anthropic",
                "mode": "subscription_cli",
                "profile_name": profile_name,
                "model": model,
                "model_verified": model_verified,
                "agent_sessions_invalidated": true,
                "model_verification": model_verification,
                "model_verification_note": if should_spawn_check {
                    Some("a bounded (~45s) background check against the live \
                          CLI was just kicked off — GET /api/ai/provider \
                          reports the outcome once it lands (verified / \
                          rejected, naming the model / unknown, if the check \
                          itself couldn't run to a conclusion). The model is \
                          also applied when an agent session starts; an \
                          unrecognized model surfaces there too, independent \
                          of this check.")
                } else {
                    None
                },
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
        Validated::ApiKey {
            provider,
            model,
            model_verified,
            ..
        } => (
            "api_key",
            json!({ "provider": provider, "model": model, "model_verified": model_verified }),
        ),
        Validated::OauthToken {
            profile_name,
            model,
            model_verified,
            ..
        } => (
            "oauth_profile",
            json!({
                "profile_name": profile_name,
                "model": model,
                "model_verified": model_verified,
            }),
        ),
        Validated::SubscriptionCli {
            cli_path,
            profile_name,
            model,
            model_verified,
        } => (
            "subscription_cli",
            json!({
                "cli_path": cli_path,
                "profile_name": profile_name,
                "model": model,
                "model_verified": model_verified,
            }),
        ),
    };
    Ok(Json(json!({
        "success": true,
        "provider": "anthropic",
        "mode": mode,
        "detail": detail,
    })))
}

/// Request body for `POST /api/ai/provider/models`.
#[derive(Debug, Deserialize)]
pub struct ModelDiscoveryRequest {
    pub provider: String,
    pub api_key: String,
}

/// `POST /api/ai/provider/models` — ask the vendor what it actually
/// serves (`GET {base_url}/models`) instead of trusting a hardcoded
/// default. Generalizes the same live-validation precedent
/// `ClaudeProvider::validate_credential` already set for Anthropic
/// (`GET /v1/models`) to any OpenAI-compatible vendor this build knows a
/// base URL for.
///
/// Follows `POST /api/ai/provider/test`'s convention: failure is
/// signalled via the HTTP status `ApiError`'s `ErrorCode` maps to (400
/// here — checked against `test_provider`/`validate_request` above,
/// which do the same for every AI-surface refusal), never a `200` with
/// `success: false` in the body. Never persists anything — this is a
/// pure lookup, run before the user commits to saving a key.
///
/// Ordered so the caller learns the cheapest, most specific failure
/// first: (1) key-shape rejection — no network call at all; (2) base-URL
/// resolution — no network call either, a typed refusal naming the
/// provider when neither the custom-provider JSON tier nor goose's
/// bundled declarative tier has one (e.g. `xai`, a hand-written native
/// goose provider with no JSON definition); (3) the live vendor round
/// trip, whose failure modes (401/404/timeout/other) stay distinguishable
/// all the way to the response body.
pub async fn discover_provider_models(
    Json(payload): Json<ModelDiscoveryRequest>,
) -> Result<Json<Value>, ApiError> {
    ai_provider_config::reject_implausible_key_shape(&payload.api_key)
        .map_err(|reason| ApiError::ai_api_key_implausible(reason))?;

    let resolved =
        ai_provider_config::resolve_provider_base_url(&payload.provider).map_err(|unresolved| {
            ApiError::ai_model_discovery_failed(
                &payload.provider,
                "no_base_url",
                unresolved.to_string(),
                None,
                None,
            )
        })?;

    let models_url = ai_provider_config::models_url(&resolved.base_url);

    let models = ai_provider_config::fetch_vendor_models(&models_url, &payload.api_key)
        .await
        .map_err(|e| {
            let (outcome, vendor_status, vendor_message) = match &e {
                ai_provider_config::VendorModelsError::Unauthorized { status, message } => {
                    ("unauthorized", Some(*status), Some(message.clone()))
                }
                ai_provider_config::VendorModelsError::NotFound { status, message } => {
                    ("not_found", Some(*status), Some(message.clone()))
                }
                ai_provider_config::VendorModelsError::UnexpectedStatus { status, message } => {
                    ("unexpected_status", Some(*status), Some(message.clone()))
                }
                ai_provider_config::VendorModelsError::Timeout => ("timeout", None, None),
                ai_provider_config::VendorModelsError::Transport(_) => {
                    ("transport_error", None, None)
                }
            };
            ApiError::ai_model_discovery_failed(
                &payload.provider,
                outcome,
                e.to_string(),
                vendor_status,
                vendor_message,
            )
        })?;

    Ok(Json(json!({
        "success": true,
        "provider": payload.provider,
        "base_url": resolved.base_url,
        "base_url_source": resolved.source,
        "models_url": models_url,
        "models": models,
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
        CredentialMode::SubscriptionCli => validate_subscription_cli(payload).await,
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

/// `anthropic`'s `api_key` mode is checked live against Anthropic's own
/// `/v1/models` before it is ever persisted (`ClaudeProvider::validate_credential`).
/// The other allowlisted `api_key` vendors (`xai`, `mistral`, `glm`,
/// `kimi`) have no vetted synchronous credential-check client in this
/// build — `ai-integration` has no native HTTP client for any of them,
/// and building an unverified one here (this server has no real key or
/// network access to prove it against) would risk exactly the "silent
/// wrong answer" this codebase's honesty rules forbid. So the same
/// precedent already established for `subscription_cli`'s model check
/// applies: the key is accepted and stored, flagged unverified
/// (`model_verified: Some(false)` when a model was requested, `None`
/// otherwise — there is no equivalent "credential_verified" flag because
/// unlike the model check there is no later background check that could
/// ever prove it either), never silently presented as confirmed. An
/// invalid key surfaces at first use instead, when goose's own
/// declarative provider actually calls the vendor's API.
async fn validate_api_key(payload: &ProviderConfigRequest) -> Result<Validated, ApiError> {
    let key = payload
        .api_key
        .clone()
        .filter(|k| !k.is_empty())
        .ok_or_else(|| ApiError::missing_parameter("api_key"))?;

    if payload.provider != "anthropic" {
        let model = normalize_model_request(payload.model.as_deref());
        let model_verified = model.as_ref().map(|_| false);
        return Ok(Validated::ApiKey {
            provider: payload.provider.clone(),
            key,
            model,
            model_verified,
        });
    }

    let credential = ClaudeCredential::ApiKey(key.clone());
    let probe = ClaudeProvider::with_config(ClaudeConfig {
        credential: Some(credential.clone()),
        ..Default::default()
    });
    probe
        .validate_credential()
        .await
        .map_err(|e| ApiError::ai_credential_invalid(e.to_string()))?;

    let (model, model_verified) =
        resolve_requested_model(payload.model.as_deref(), Some(&credential), false).await?;

    Ok(Validated::ApiKey {
        provider: payload.provider.clone(),
        key,
        model,
        model_verified,
    })
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

    let credential = ClaudeCredential::OauthAccessToken(token.clone());
    let probe = ClaudeProvider::with_config(ClaudeConfig {
        credential: Some(credential.clone()),
        ..Default::default()
    });
    probe
        .validate_credential()
        .await
        .map_err(|e| ApiError::ai_credential_invalid(e.to_string()))?;

    let (model, model_verified) =
        resolve_requested_model(payload.model.as_deref(), Some(&credential), false).await?;

    let profile_name = payload
        .profile_name
        .clone()
        .unwrap_or_else(|| "default".to_string());
    Ok(Validated::OauthToken {
        token,
        profile_name,
        model,
        model_verified,
    })
}

async fn validate_subscription_cli(payload: &ProviderConfigRequest) -> Result<Validated, ApiError> {
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
    // The npm shim (`claude.cmd`) is installed, but `detect_claude_cli`
    // could not resolve a real, directly-spawnable executable from it —
    // neither the known Claude Code binary layout nor a parse of the shim
    // itself found one. This is a legitimate, nameable refusal (not an
    // internal bug): a `.cmd`/`.ps1` path must never reach
    // `CLAUDE_CODE_COMMAND` (Windows' `CreateProcess` cannot spawn it —
    // see `ai_provider_config`'s CLI-detection doc), so this refuses by
    // name, citing `resolution_note`, rather than silently falling back to
    // the unspawnable shim path.
    let cli_path = cli.path.ok_or_else(|| {
        ApiError::ai_credential_invalid(format!(
            "Claude Code CLI is installed and signed in, but no directly \
             spawnable executable could be resolved from its npm shim{}",
            cli.resolution_note
                .as_deref()
                .map(|note| format!(" — {note}"))
                .unwrap_or_default()
        ))
    })?;

    // No credential to probe a model against here — `is_subscription_cli:
    // true` short-circuits `resolve_requested_model` before it would ever
    // need one, so `None` is safe.
    let (model, model_verified) =
        resolve_requested_model(payload.model.as_deref(), None, true).await?;

    let profile_name = payload
        .profile_name
        .clone()
        .unwrap_or_else(|| "default".to_string());
    Ok(Validated::SubscriptionCli {
        cli_path,
        profile_name,
        model,
        model_verified,
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
            model: None,
            consent_spawn_local_process: false,
        }
    }

    // --- validate_api_key: the generalized-provider path — xai/mistral/
    //     glm/kimi accept an api_key without a network probe (no vetted
    //     synchronous credential-check client exists for them), anthropic
    //     is unaffected. Network-free: these never reach ClaudeProvider. ---

    #[tokio::test]
    async fn validate_request_accepts_xai_api_key_without_a_network_probe() {
        let mut payload = request("xai", "api_key");
        payload.api_key = Some("fake-xai-key".to_string());
        let validated = validate_request(&payload)
            .await
            .expect("xai/api_key must validate without hitting Anthropic's API");
        match validated {
            Validated::ApiKey {
                provider,
                key,
                model,
                model_verified,
            } => {
                assert_eq!(provider, "xai");
                assert_eq!(key, "fake-xai-key");
                assert_eq!(model, None, "no model requested must stay None");
                assert_eq!(
                    model_verified, None,
                    "no credential-check client exists, so there is nothing to \
                     report a verification state for when no model was requested"
                );
            }
            other => panic!("expected Validated::ApiKey, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validate_request_accepts_mistral_glm_kimi_api_key_flagging_an_explicit_model_unverified(
    ) {
        for (id, model_name) in [
            ("mistral", "mistral-medium-latest"),
            ("glm", "glm-4.6"),
            ("kimi", "kimi-k2-turbo-preview"),
        ] {
            let mut payload = request(id, "api_key");
            payload.api_key = Some(format!("fake-{id}-key"));
            payload.model = Some(model_name.to_string());
            let validated = validate_request(&payload)
                .await
                .unwrap_or_else(|e| panic!("{id}/api_key must validate: {e:?}"));
            match validated {
                Validated::ApiKey {
                    provider,
                    model,
                    model_verified,
                    ..
                } => {
                    assert_eq!(provider, id);
                    assert_eq!(model.as_deref(), Some(model_name));
                    assert_eq!(
                        model_verified,
                        Some(false),
                        "{id}: an explicit model must be flagged unverified, \
                         never silently confirmed with no check having run"
                    );
                }
                other => panic!("{id}: expected Validated::ApiKey, got {other:?}"),
            }
        }
    }

    // --- resolve_requested_model: the model-honesty gate, network-free ---
    // (no test here hits a live network — `credential: None` proves the
    // `None`/`"default"` branches short-circuit before any probe would be
    // attempted; the mode-specific network probe itself is covered live
    // in `ai_integration::providers::claude`'s `validate_model` tests).

    #[tokio::test]
    async fn resolve_requested_model_none_is_the_providers_own_choice() {
        let (model, verified) = resolve_requested_model(None, None, false)
            .await
            .expect("None must never be treated as a rejection");
        assert_eq!(model, None);
        assert_eq!(verified, None);
    }

    #[tokio::test]
    async fn resolve_requested_model_default_sentinel_normalizes_to_none() {
        // "default" must never be persisted as if it were a real model
        // name — it is claude-code's own CLAUDE_CODE_DEFAULT_MODEL
        // sentinel and the dialog's own default.
        let (model, verified) = resolve_requested_model(Some("default"), None, false)
            .await
            .expect("the default sentinel must never be treated as a rejection");
        assert_eq!(model, None);
        assert_eq!(verified, None);
    }

    #[tokio::test]
    async fn resolve_requested_model_blank_string_normalizes_to_none() {
        let (model, verified) = resolve_requested_model(Some("   "), None, false)
            .await
            .expect("whitespace-only must not be treated as a real model request");
        assert_eq!(model, None);
        assert_eq!(verified, None);
    }

    #[tokio::test]
    async fn resolve_requested_model_subscription_cli_accepts_unverified_never_silently() {
        // subscription_cli has no side-effect-free synchronous enumeration
        // available — the model is accepted but must come back flagged
        // `Some(false)`, never `Some(true)` (that would claim a check that
        // never happened) and never silently dropped to `None`.
        let (model, verified) = resolve_requested_model(Some("opus"), None, true)
            .await
            .expect("subscription_cli must accept an explicit model string");
        assert_eq!(model.as_deref(), Some("opus"));
        assert_eq!(verified, Some(false));
    }

    #[tokio::test]
    async fn resolve_requested_model_refuses_by_name_without_a_credential_to_check_against() {
        let err = resolve_requested_model(Some("claude-opus-4"), None, false)
            .await
            .expect_err("api_key/oauth_profile modes must refuse an unverifiable explicit model");
        assert_eq!(err.code, ErrorCode::AiModelRejected);
        let details = err.details.expect("refusal must carry details");
        assert_eq!(details["rejected_model"], "claude-opus-4");
    }

    /// The PUT→invalidation seam: every successful repin branch calls
    /// `invalidate_agent_sessions`, and that call must end (stale-ify)
    /// every ACP connection minted before it while leaving later ones
    /// serviceable. The full PUT branches themselves cannot run here —
    /// they write goose's process-global config, which exactly one test
    /// in this binary is allowed to touch
    /// (`goose_acp::tests::goose_lockdown_leaves_exactly_roshera_reachable`)
    /// — so the branch wiring is proven live (repin, then a turn on the
    /// new provider) and this pins the helper's contract against the
    /// SHARED `AppState` instance the `/acp` middleware reads.
    #[tokio::test]
    async fn invalidate_agent_sessions_ends_connections_minted_before_the_repin() {
        let state = make_test_state().await;
        state.acp_provider_epoch.register("conn-before-repin");
        assert!(
            !state.acp_provider_epoch.is_stale("conn-before-repin"),
            "a connection must serve until a repin actually happens"
        );

        invalidate_agent_sessions(&state, "anthropic", "subscription_cli");

        assert!(
            state.acp_provider_epoch.is_stale("conn-before-repin"),
            "a successful repin must end every ACP connection minted \
             before it — otherwise the open tab keeps prompting the old \
             provider (goose restores the provider stored on the session)"
        );
        state.acp_provider_epoch.register("conn-after-repin");
        assert!(
            !state.acp_provider_epoch.is_stale("conn-after-repin"),
            "a connection minted after the repin must serve"
        );
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

    /// `openai`/`api_key` was seam-only when this suite was written; it is
    /// `WiringStatus::Wired` today, through goose's native OpenAI provider
    /// (see that entry in `allowlist.rs` and its line in
    /// `KNOWN_WIRED_PATHS`). So the request no longer stops at a by-name
    /// refusal — it reaches `validate_api_key`, which makes an absent key a
    /// missing parameter and a supplied key an unverified acceptance on the
    /// same precedent as `xai` above.
    #[tokio::test]
    async fn put_provider_openai_api_key_is_wired_not_refused_by_name() {
        let missing = request("openai", "api_key");
        let err = validate_request(&missing).await.unwrap_err();
        assert_eq!(
            err.code,
            ErrorCode::MissingParameter,
            "openai/api_key is wired, so an absent key must fail as a missing \
             parameter — a by-name refusal here would mean the allowlist had \
             silently regressed to seam-only"
        );

        let mut supplied = request("openai", "api_key");
        supplied.api_key = Some("fake-openai-key".to_string());
        match validate_request(&supplied)
            .await
            .expect("openai/api_key must validate without a network probe")
        {
            Validated::ApiKey {
                provider,
                key,
                model,
                model_verified,
            } => {
                assert_eq!(provider, "openai");
                assert_eq!(key, "fake-openai-key");
                assert_eq!(model, None, "no model requested must stay None");
                assert_eq!(
                    model_verified, None,
                    "no credential-check client exists for openai, and no model \
                     was requested, so there is nothing to report verified for"
                );
            }
            other => panic!("expected Validated::ApiKey, got {other:?}"),
        }
    }

    /// The openai seam that does remain: `subscription_cli` (Codex) is
    /// `SeamOnly` in this build and must still be refused by name. Consent
    /// is granted here so the refusal proves the seam rather than the
    /// consent gate — `validate_request` checks seam-only first, and this
    /// test would still pass if that order ever inverted.
    #[tokio::test]
    async fn put_provider_refuses_openai_subscription_cli_seam_only() {
        let mut payload = request("openai", "subscription_cli");
        payload.consent_spawn_local_process = true;
        let err = validate_request(&payload).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::AiProviderRefused);
        assert!(
            err.error.contains("seam-only"),
            "refusal must name the seam, got: {}",
            err.error
        );
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

    // --- subscription_cli model verification surfaced through GET,
    //     end to end via the real orchestration function
    //     (`verify_subscription_cli_model`) with an injected fake probe —
    //     never spawns the real ~250 MB claude.exe. ---

    fn subscription_cli_config_with_model(
        model: &str,
        model_verification: Option<ModelVerificationState>,
    ) -> ai_provider_config::StoredProviderConfig {
        ai_provider_config::StoredProviderConfig {
            provider: "anthropic".to_string(),
            mode: "subscription_cli".to_string(),
            api_key: None,
            profile_name: Some("default".to_string()),
            model: Some(model.to_string()),
            model_verified: Some(false),
            model_verification,
            saved_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn get_provider_reports_rejected_model_naming_it() {
        let state = make_test_state().await;
        state
            .ai_provider_manager
            .save(subscription_cli_config_with_model("bogus-model", None))
            .await
            .expect("save must succeed");

        let probe: ai_provider_config::ModelProbe = std::sync::Arc::new(|_cli_path, _model| {
            Box::pin(async {
                ai_provider_config::ProbeOutcome::Rejected(
                    "There's an issue with the selected model (bogus-model).".to_string(),
                )
            })
        });
        ai_provider_config::verify_subscription_cli_model(
            probe,
            std::path::PathBuf::from("C:\\fake\\claude.exe"),
            "bogus-model".to_string(),
            state.ai_provider_manager.clone(),
        )
        .await;

        let Json(body) = get_provider(State(state)).await;
        assert_eq!(body["active"]["model_verification"]["state"], "rejected");
        assert_eq!(body["active"]["model_verification"]["model"], "bogus-model");
        assert!(body["active"]["model_verification"]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("issue with the selected model"));
        assert_eq!(body["active"]["model_verified"], json!(false));
    }

    #[tokio::test]
    async fn get_provider_reports_unknown_on_timeout_never_verified_or_rejected() {
        let state = make_test_state().await;
        state
            .ai_provider_manager
            .save(subscription_cli_config_with_model(
                "opus",
                Some(ModelVerificationState::Pending),
            ))
            .await
            .expect("save must succeed");

        // A fake probe standing in for a hung/timed-out CLI round trip —
        // the timeout-wrapper mechanics themselves (kill-on-drop,
        // `Unknown` on elapsed) are unit-tested directly in
        // `ai_provider_config`; this proves the observable GET-facing
        // outcome of that path is `unknown`, never a guessed pass or fail.
        let probe: ai_provider_config::ModelProbe = std::sync::Arc::new(|_cli_path, _model| {
            Box::pin(async {
                ai_provider_config::ProbeOutcome::Unknown(
                    "model verification timed out after 45s".to_string(),
                )
            })
        });
        ai_provider_config::verify_subscription_cli_model(
            probe,
            std::path::PathBuf::from("C:\\fake\\claude.exe"),
            "opus".to_string(),
            state.ai_provider_manager.clone(),
        )
        .await;

        let Json(body) = get_provider(State(state)).await;
        assert_eq!(body["active"]["model_verification"]["state"], "unknown");
        assert_ne!(body["active"]["model_verification"]["state"], "verified");
        assert_ne!(body["active"]["model_verification"]["state"], "rejected");
        assert_eq!(body["active"]["model_verified"], json!(false));
    }

    #[tokio::test]
    async fn get_provider_reports_pending_before_the_background_check_lands() {
        let state = make_test_state().await;
        state
            .ai_provider_manager
            .save(subscription_cli_config_with_model(
                "opus",
                Some(ModelVerificationState::Pending),
            ))
            .await
            .expect("save must succeed");

        let Json(body) = get_provider(State(state)).await;
        assert_eq!(body["active"]["model_verification"]["state"], "pending");
        assert_eq!(
            body["active"]["model_verified"],
            json!(false),
            "pending must never read as verified"
        );
    }
}
