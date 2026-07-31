//! Persistence and resolution for the AI provider connection dialog.
//!
//! Backs `GET/PUT/DELETE /api/ai/provider` and `POST
//! /api/ai/provider/test` (`handlers/ai_provider.rs`). This module owns:
//!
//! - **Persistence**: `state/ai-provider.json` (gitignored — see
//!   `.gitignore:103`). Plaintext by necessity: an API key must be
//!   recoverable in full to attach to outbound requests, so there is no
//!   one-way hash to store instead. OAuth/subscription modes never store
//!   a token here — only the profile/CLI identity NAME. The file gets a
//!   best-effort Windows ACL restriction (see [`restrict_acl_windows`]);
//!   read the doc comment there for exactly what that does and does not
//!   guarantee.
//! - **Resolution with shadow reporting**: `runtime` (this file) →
//!   `ANTHROPIC_API_KEY` → `ANTHROPIC_AUTH_TOKEN` → `%APPDATA%\Anthropic`
//!   (see [`build_chain`]). When a higher-priority source is active, a
//!   present-but-unused lower source is reported with a `note` explaining
//!   why it isn't taking effect — the concrete failure mode this exists
//!   to prevent is a stale `ANTHROPIC_API_KEY` in someone's shell profile
//!   silently overriding a freshly-saved key with no visible reason.
//! - **CLI detection**: locates the `claude`/`codex` npm shims and reads
//!   (never spawns) their own sign-in marker files.
//! - **The env-scrub fix**: goose's `claude_code.rs::build_stream_json_command`
//!   only strips `CLAUDECODE` before spawning the CLI — it does not scrub
//!   `ANTHROPIC_API_KEY`/`ANTHROPIC_AUTH_TOKEN`, so a spawned CLI silently
//!   inherits and bills against those instead of the user's Max/Pro
//!   subscription login. See [`scrub_anthropic_env_for_subscription_mode`].

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ── Env snapshot (captured once, at boot, before any scrub) ───────────

/// Presence (never the value) of the two Anthropic credential env vars,
/// captured once at process boot — before `PUT /api/ai/provider` can
/// ever scrub them for subscription-CLI mode. Shadow reporting needs
/// this even after a scrub: without it, a user who activates subscription
/// mode would see `ANTHROPIC_API_KEY` simply vanish from the resolution
/// report with no explanation of where it went.
///
/// Presence-only by design: this struct never holds the secret bytes, so
/// there is nothing here to redact or leak — the live value (when still
/// present) is read fresh from the environment wherever it's needed.
#[derive(Debug, Clone, Copy)]
pub struct EnvSnapshot {
    pub anthropic_api_key_was_set: bool,
    pub anthropic_auth_token_was_set: bool,
}

impl EnvSnapshot {
    /// Capture current presence of both vars. Call this exactly once, as
    /// early as possible during boot — see the module doc.
    pub fn capture() -> Self {
        Self::from_presence(
            env_var_present("ANTHROPIC_API_KEY"),
            env_var_present("ANTHROPIC_AUTH_TOKEN"),
        )
    }

    fn from_presence(anthropic_api_key_was_set: bool, anthropic_auth_token_was_set: bool) -> Self {
        Self {
            anthropic_api_key_was_set,
            anthropic_auth_token_was_set,
        }
    }
}

fn env_var_present(key: &str) -> bool {
    std::env::var(key).map(|v| !v.is_empty()).unwrap_or(false)
}

/// Scrub `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` from THIS process's
/// environment when Claude subscription-CLI mode is activated.
///
/// goose's `claude_code.rs::build_stream_json_command` only performs
/// `env_remove("CLAUDECODE")` before spawning the `claude` CLI — it does
/// **not** scrub Anthropic credential env vars, so the spawned CLI
/// inherits whatever is in this process's environment. If an API key is
/// present there, the CLI silently authenticates with it instead of the
/// user's Max/Pro subscription login: same conversation, wrong invoice,
/// no visible difference to the user. Removing the vars here — in the
/// process that spawns the CLI — is the only fix available without
/// touching goose's vendored code.
///
/// Safe to call after boot: every `ClaudeProvider` already constructed
/// (from the initial `ANTHROPIC_API_KEY` read, or from a persisted
/// runtime config) captured its credential *by value* at construction
/// time, so scrubbing the process env here does not disturb any
/// already-registered REST-serving provider. `edition = "2021"`
/// (`api-server/Cargo.toml:4`), so `remove_var` is safe to call directly
/// — no `unsafe` block required (that requirement is edition 2024+).
pub fn scrub_anthropic_env_for_subscription_mode() {
    std::env::remove_var("ANTHROPIC_API_KEY");
    std::env::remove_var("ANTHROPIC_AUTH_TOKEN");
    tracing::info!(
        target: "api_server.ai_provider",
        "scrubbed ANTHROPIC_API_KEY / ANTHROPIC_AUTH_TOKEN from the process \
         environment: Claude subscription-CLI mode was activated, and goose's \
         claude-code spawn path does not scrub them on its own"
    );
}

// ── Persisted config ───────────────────────────────────────────────────

/// The persisted shape of `state/ai-provider.json`.
///
/// `api_key` is `Some` only for `mode == "api_key"`. `profile_name` is
/// `Some` only for `mode == "oauth_profile"` / `"subscription_cli"` — it
/// is a human-readable identity label, never a token; the token itself
/// stays in the vendor CLI's own credential store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredProviderConfig {
    pub provider: String,
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    pub saved_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderConfigError {
    #[error("failed to read/write provider config at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to (de)serialize provider config: {0}")]
    Serde(#[from] serde_json::Error),
}

/// Best-effort outcome of restricting `state/ai-provider.json`'s Windows
/// ACL to the current user. Reported to the caller (surfaced in the GET
/// response) rather than assumed — see [`restrict_acl_windows`] for the
/// exact guarantee.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AclOutcome {
    /// `icacls` ran and reported success: inheritance was broken and the
    /// ACL now grants full control to the current user only.
    Restricted,
    /// `icacls` was not run, or ran and failed — the file inherits
    /// whatever permissions its parent directory (and its own creation
    /// defaults) already grant. Logged, never fatal: the write itself
    /// still succeeded.
    DefaultInherited,
}

fn default_state_path() -> PathBuf {
    match std::env::var_os("ROSHERA_AI_PROVIDER_STATE") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from("state").join("ai-provider.json"),
    }
}

/// Restrict a just-written file's Windows ACL to the current user via
/// `icacls <path> /inheritance:r /grant:r "<user>:F"`.
///
/// # What this actually guarantees
///
/// On success, inherited permissions from the parent directory are
/// broken and the ACL is replaced with full control for the
/// current Windows account only. It does **not**:
/// - encrypt the file (it is still plaintext on disk);
/// - stop another process running as the *same* user account (or an
///   Administrator, who can always take ownership) from reading it;
/// - survive a copy — a copied file inherits the destination's ACL, not
///   this one.
///
/// This is a defense against *other user accounts* on a shared machine,
/// not a substitute for OS-level disk encryption or a secret manager.
/// Best-effort: `icacls` absence or failure is logged and reported via
/// [`AclOutcome::DefaultInherited`], never treated as a save failure —
/// the credential is still usable, just less protected than intended.
fn restrict_acl_windows(path: &Path) -> AclOutcome {
    let username = std::env::var("USERNAME").unwrap_or_default();
    if username.is_empty() {
        tracing::warn!(
            target: "api_server.ai_provider",
            "USERNAME env var unset; cannot target an ACL grant, leaving \
             state/ai-provider.json with inherited permissions"
        );
        return AclOutcome::DefaultInherited;
    }
    let grant = format!("{username}:F");
    match std::process::Command::new("icacls")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(&grant)
        .output()
    {
        Ok(output) if output.status.success() => AclOutcome::Restricted,
        Ok(output) => {
            tracing::warn!(
                target: "api_server.ai_provider",
                exit_code = ?output.status.code(),
                stderr = %String::from_utf8_lossy(&output.stderr),
                "icacls failed to restrict state/ai-provider.json's ACL; \
                 file remains with default inherited permissions"
            );
            AclOutcome::DefaultInherited
        }
        Err(source) => {
            tracing::warn!(
                target: "api_server.ai_provider",
                error = %source,
                "icacls could not be run; state/ai-provider.json remains \
                 with default inherited permissions"
            );
            AclOutcome::DefaultInherited
        }
    }
}

fn write_state_file(
    path: &Path,
    cfg: &StoredProviderConfig,
) -> Result<AclOutcome, ProviderConfigError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ProviderConfigError::Io {
            path: parent.display().to_string(),
            source,
        })?;
    }
    let json = serde_json::to_vec_pretty(cfg)?;
    std::fs::write(path, json).map_err(|source| ProviderConfigError::Io {
        path: path.display().to_string(),
        source,
    })?;
    Ok(restrict_acl_windows(path))
}

fn load_stored(path: &Path) -> Option<StoredProviderConfig> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice(&bytes) {
        Ok(cfg) => Some(cfg),
        Err(e) => {
            tracing::warn!(
                target: "api_server.ai_provider",
                error = %e,
                path = %path.display(),
                "failed to parse persisted state/ai-provider.json; treating \
                 as unconfigured rather than guessing at a partial value"
            );
            None
        }
    }
}

// ── CLI detection (read-only; never spawns an interactive login) ──────

/// What we can tell about a vendor CLI without running it: whether the
/// npm shim exists on disk, and whether its own sign-in marker file is
/// present. Neither check spawns a process.
#[derive(Debug, Clone, Serialize)]
pub struct CliStatus {
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub signed_in: bool,
}

fn detect_cli_at(
    appdata: Option<&str>,
    userprofile: Option<&str>,
    shim_name: &str,
    credentials_rel_path: &[&str],
) -> CliStatus {
    let path = appdata.map(|a| Path::new(a).join("npm").join(shim_name));
    let installed = path.as_deref().map(Path::exists).unwrap_or(false);

    let signed_in = userprofile
        .map(|home| {
            let mut p = PathBuf::from(home);
            for segment in credentials_rel_path {
                p.push(segment);
            }
            p.exists()
        })
        .unwrap_or(false);

    CliStatus {
        installed,
        path: if installed {
            path.map(|p| p.display().to_string())
        } else {
            None
        },
        signed_in,
    }
}

/// Detect the Claude Code CLI: shim at `%APPDATA%\npm\claude.cmd`,
/// signed-in marker at `~/.claude/.credentials.json`.
pub fn detect_claude_cli() -> CliStatus {
    detect_cli_at(
        std::env::var("APPDATA").ok().as_deref(),
        std::env::var("USERPROFILE").ok().as_deref(),
        "claude.cmd",
        &[".claude", ".credentials.json"],
    )
}

/// Detect the Codex CLI: shim at `%APPDATA%\npm\codex.cmd`, signed-in
/// marker at `~/.codex/auth.json`.
pub fn detect_codex_cli() -> CliStatus {
    detect_cli_at(
        std::env::var("APPDATA").ok().as_deref(),
        std::env::var("USERPROFILE").ok().as_deref(),
        "codex.cmd",
        &[".codex", "auth.json"],
    )
}

/// Report on the `%APPDATA%\Anthropic` credential store link in the
/// resolution chain. This machine does not have that directory —
/// verified, not assumed — so this detects and reports absence rather
/// than pretending a store exists. Read-only: `Path::exists` never
/// creates the directory.
#[derive(Debug, Clone, Serialize)]
pub struct AppdataAnthropicReport {
    pub checked_path: String,
    pub present: bool,
}

fn detect_appdata_anthropic_at(appdata: Option<&str>) -> AppdataAnthropicReport {
    let base = appdata.unwrap_or_default();
    let path = Path::new(base).join("Anthropic");
    AppdataAnthropicReport {
        checked_path: path.display().to_string(),
        present: path.exists(),
    }
}

pub fn detect_appdata_anthropic() -> AppdataAnthropicReport {
    detect_appdata_anthropic_at(std::env::var("APPDATA").ok().as_deref())
}

// ── Resolution chain with shadow reporting ─────────────────────────────

/// One link in the resolution chain reported by `GET /api/ai/provider`.
#[derive(Debug, Clone, Serialize)]
pub struct ResolutionEntry {
    pub source: &'static str,
    pub present: bool,
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Pure resolution logic — no env/filesystem access — so the priority
/// and shadowing rules are unit-testable without mutating global process
/// state. `runtime → ANTHROPIC_API_KEY → ANTHROPIC_AUTH_TOKEN →
/// %APPDATA%\Anthropic`, first-present-wins; anything present below the
/// active source gets an explanatory `note`.
fn build_chain(
    stored: Option<&StoredProviderConfig>,
    env_snapshot: &EnvSnapshot,
    live_api_key: bool,
    live_auth_token: bool,
    appdata: &AppdataAnthropicReport,
) -> Vec<ResolutionEntry> {
    let runtime_present = matches!(
        stored,
        Some(s) if s.mode == "api_key"
            && s.api_key.as_deref().map(|k| !k.is_empty()).unwrap_or(false)
    );

    let mut chain = Vec::with_capacity(4);
    let mut resolved = runtime_present;

    chain.push(ResolutionEntry {
        source: "runtime",
        present: runtime_present,
        active: runtime_present,
        note: if runtime_present {
            None
        } else {
            Some("no API key saved via PUT /api/ai/provider".to_string())
        },
    });

    let api_key_note = if resolved && live_api_key {
        Some(
            "shadowed by the saved runtime provider config — DELETE \
             /api/ai/provider to fall back to this env var"
                .to_string(),
        )
    } else if !live_api_key && env_snapshot.anthropic_api_key_was_set {
        Some(
            "was set at server boot; removed from this process's \
             environment when Claude Max/Pro subscription-CLI mode was \
             activated, to stop the CLI from silently billing this API \
             key instead of the subscription"
                .to_string(),
        )
    } else {
        None
    };
    chain.push(ResolutionEntry {
        source: "env:ANTHROPIC_API_KEY",
        present: live_api_key,
        active: !resolved && live_api_key,
        note: api_key_note,
    });
    resolved = resolved || live_api_key;

    let auth_token_note = if resolved && live_auth_token {
        Some("shadowed by a higher-priority source in the resolution chain".to_string())
    } else if !live_auth_token && env_snapshot.anthropic_auth_token_was_set {
        Some(
            "was set at server boot; removed from this process's \
             environment when Claude Max/Pro subscription-CLI mode was \
             activated, to stop the CLI from silently billing this token \
             instead of the subscription"
                .to_string(),
        )
    } else {
        None
    };
    chain.push(ResolutionEntry {
        source: "env:ANTHROPIC_AUTH_TOKEN",
        present: live_auth_token,
        active: !resolved && live_auth_token,
        note: auth_token_note,
    });
    resolved = resolved || live_auth_token;

    chain.push(ResolutionEntry {
        source: "appdata_anthropic",
        present: appdata.present,
        active: !resolved && appdata.present,
        note: Some(format!(
            "checked {} (detection only — never creates the directory)",
            appdata.checked_path
        )),
    });

    chain
}

// ── goose repin (subscription-CLI activation) ──────────────────────────

/// Pin goose's active provider to `claude-code` (NOT `claude-acp`, which
/// needs an npm adapter this deployment does not install) and point it
/// at the resolved `claude.cmd` shim. Config reads are live in goose —
/// no restart needed.
///
/// Called from the PUT handler only after: (1) the allowlist confirmed
/// `subscription_cli` is `Wired` for this provider, (2) explicit consent
/// was supplied in the request body, and (3) CLI detection found the
/// shim installed and signed in. This function itself performs no
/// consent or detection checks — it is the mechanical config write.
pub fn repin_goose_to_claude_code(claude_cli_path: &Path) -> Result<(), String> {
    let config = goose::config::Config::global();
    goose::config::set_active_provider(config, "claude-code", "default")
        .map_err(|e| format!("failed to pin goose's active_provider to claude-code: {e}"))?;
    config
        .set_param("CLAUDE_CODE_COMMAND", claude_cli_path.display().to_string())
        .map_err(|e| format!("failed to set CLAUDE_CODE_COMMAND: {e}"))?;
    Ok(())
}

// ── The manager ─────────────────────────────────────────────────────────

struct Inner {
    stored: Option<StoredProviderConfig>,
}

/// Owns the persisted provider config, the boot-time env snapshot, and
/// the resolution-chain computation. One instance lives in `AppState`
/// for the process lifetime.
pub struct AiProviderManager {
    state_path: PathBuf,
    env_snapshot: EnvSnapshot,
    inner: tokio::sync::RwLock<Inner>,
}

impl AiProviderManager {
    /// Boot-time construction. Captures the env snapshot FIRST — before
    /// anything in this process can have scrubbed the credential env
    /// vars — then loads whatever was persisted from a previous run at
    /// `state/ai-provider.json` (override via `ROSHERA_AI_PROVIDER_STATE`).
    /// Synchronous: both steps are one-shot blocking calls appropriate
    /// for process startup, so callers don't need an executor yet.
    pub fn boot() -> Self {
        Self::boot_at(default_state_path())
    }

    /// Same as [`Self::boot`] but with an explicit state-file path,
    /// bypassing the `ROSHERA_AI_PROVIDER_STATE` env lookup. Lets tests
    /// isolate their state file to a private temp path without mutating
    /// process-global env (which would race against other tests in the
    /// same binary running in parallel threads).
    pub fn boot_at(state_path: PathBuf) -> Self {
        let env_snapshot = EnvSnapshot::capture();
        let stored = load_stored(&state_path);
        Self {
            state_path,
            env_snapshot,
            inner: tokio::sync::RwLock::new(Inner { stored }),
        }
    }

    pub async fn stored(&self) -> Option<StoredProviderConfig> {
        self.inner.read().await.stored.clone()
    }

    pub fn env_snapshot(&self) -> EnvSnapshot {
        self.env_snapshot
    }

    /// Persist a new config, replacing whatever was stored before.
    pub async fn save(&self, cfg: StoredProviderConfig) -> Result<AclOutcome, ProviderConfigError> {
        let acl = write_state_file(&self.state_path, &cfg)?;
        self.inner.write().await.stored = Some(cfg);
        Ok(acl)
    }

    /// Remove the persisted config (idempotent — no error if absent).
    pub async fn delete(&self) -> Result<(), ProviderConfigError> {
        if self.state_path.exists() {
            std::fs::remove_file(&self.state_path).map_err(|source| ProviderConfigError::Io {
                path: self.state_path.display().to_string(),
                source,
            })?;
        }
        self.inner.write().await.stored = None;
        Ok(())
    }

    /// The full resolution chain, computed against the live environment
    /// at call time (via the pure [`build_chain`]).
    pub async fn resolution_report(&self) -> Vec<ResolutionEntry> {
        let stored = self.inner.read().await.stored.clone();
        build_chain(
            stored.as_ref(),
            &self.env_snapshot,
            env_var_present("ANTHROPIC_API_KEY"),
            env_var_present("ANTHROPIC_AUTH_TOKEN"),
            &detect_appdata_anthropic(),
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn stored(mode: &str, api_key: Option<&str>) -> StoredProviderConfig {
        StoredProviderConfig {
            provider: "anthropic".to_string(),
            mode: mode.to_string(),
            api_key: api_key.map(str::to_string),
            profile_name: None,
            saved_at: chrono::Utc::now(),
        }
    }

    fn env(api_key_was_set: bool, auth_token_was_set: bool) -> EnvSnapshot {
        EnvSnapshot::from_presence(api_key_was_set, auth_token_was_set)
    }

    fn appdata_absent() -> AppdataAnthropicReport {
        AppdataAnthropicReport {
            checked_path: "C:\\fake\\Anthropic".to_string(),
            present: false,
        }
    }

    // --- build_chain: priority + shadowing ---

    #[test]
    fn runtime_wins_when_present_and_env_key_is_reported_shadowed() {
        let cfg = stored("api_key", Some("sk-ant-real"));
        let chain = build_chain(
            Some(&cfg),
            &env(false, false),
            true,
            false,
            &appdata_absent(),
        );

        assert!(chain[0].active, "runtime must be the active source");
        assert!(chain[0].present);
        assert!(
            !chain[1].active,
            "env:ANTHROPIC_API_KEY must not be active while runtime wins"
        );
        assert!(chain[1].present, "env var is still present");
        assert!(
            chain[1].note.as_deref().unwrap_or("").contains("shadowed"),
            "shadowed source must explain why it isn't active: {:?}",
            chain[1].note
        );
    }

    #[test]
    fn env_api_key_wins_when_no_runtime_config() {
        let chain = build_chain(None, &env(false, false), true, false, &appdata_absent());
        assert!(!chain[0].active); // runtime absent
        assert!(chain[1].active, "env:ANTHROPIC_API_KEY must resolve next");
        assert!(chain[1].note.is_none());
    }

    #[test]
    fn auth_token_only_active_when_nothing_higher_present() {
        let chain = build_chain(None, &env(false, false), false, true, &appdata_absent());
        assert!(!chain[0].active);
        assert!(!chain[1].active);
        assert!(chain[2].active, "env:ANTHROPIC_AUTH_TOKEN must resolve");
    }

    #[test]
    fn appdata_is_last_resort_and_never_active_when_env_present() {
        let present = AppdataAnthropicReport {
            checked_path: "C:\\Users\\x\\AppData\\Roaming\\Anthropic".to_string(),
            present: true,
        };
        let chain = build_chain(None, &env(false, false), true, false, &present);
        assert!(chain[1].active, "env key still wins over appdata");
        assert!(
            !chain[3].active,
            "appdata must not be active while a higher source resolves"
        );
    }

    #[test]
    fn appdata_absent_on_this_machine_is_reported_not_assumed() {
        let chain = build_chain(None, &env(false, false), false, false, &appdata_absent());
        assert!(!chain[3].present);
        assert!(!chain[3].active);
        assert!(chain[3].note.as_deref().unwrap_or("").contains("fake"));
    }

    #[test]
    fn scrubbed_env_key_is_explained_not_silently_dropped() {
        // Snapshot says the var WAS set at boot; live query says it's
        // gone now (subscription-mode scrub). The report must explain
        // the disappearance instead of just showing `present: false`
        // with no context.
        let chain = build_chain(None, &env(true, false), false, false, &appdata_absent());
        assert!(!chain[1].present);
        let note = chain[1].note.as_deref().unwrap_or("");
        assert!(
            note.contains("subscription"),
            "must explain the scrub, got: {note}"
        );
    }

    // --- persistence round trip ---

    #[test]
    fn write_then_load_round_trips_and_never_writes_profile_name_for_api_key_mode() {
        let dir =
            std::env::temp_dir().join(format!("roshera-ai-provider-test-{}", uuid::Uuid::new_v4()));
        let path = dir.join("ai-provider.json");
        let cfg = stored("api_key", Some("sk-ant-roundtrip"));

        write_state_file(&path, &cfg).expect("write must succeed");
        let raw = std::fs::read_to_string(&path).expect("file must exist");
        assert!(
            !raw.contains("profile_name"),
            "api_key-mode config must not serialize an absent profile_name: {raw}"
        );

        let loaded = load_stored(&path).expect("must load what was just written");
        assert_eq!(loaded.api_key.as_deref(), Some("sk-ant-roundtrip"));
        assert_eq!(loaded.mode, "api_key");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_stored_returns_none_for_missing_or_corrupt_file() {
        let dir = std::env::temp_dir().join(format!(
            "roshera-ai-provider-test-missing-{}",
            uuid::Uuid::new_v4()
        ));
        assert!(load_stored(&dir.join("nope.json")).is_none());

        std::fs::create_dir_all(&dir).expect("temp dir must create");
        let corrupt = dir.join("corrupt.json");
        std::fs::write(&corrupt, b"not json").expect("write must succeed");
        assert!(
            load_stored(&corrupt).is_none(),
            "corrupt state must be treated as unconfigured, not panic"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn manager_save_delete_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "roshera-ai-provider-mgr-test-{}",
            uuid::Uuid::new_v4()
        ));
        // `boot_at` takes the path directly — no process-global env
        // mutation, so this test is safe to run in parallel with the
        // rest of the suite.
        let mgr = AiProviderManager::boot_at(dir.join("ai-provider.json"));
        assert!(mgr.stored().await.is_none());

        mgr.save(stored("api_key", Some("sk-ant-mgr")))
            .await
            .expect("save must succeed");
        assert_eq!(
            mgr.stored().await.map(|s| s.api_key).unwrap(),
            Some("sk-ant-mgr".to_string())
        );

        mgr.delete().await.expect("delete must succeed");
        assert!(mgr.stored().await.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- CLI / appdata detection: pure, no global env mutation ---

    #[test]
    fn detect_cli_reports_not_installed_when_shim_absent() {
        let status = detect_cli_at(
            Some("C:\\definitely\\does\\not\\exist"),
            Some("C:\\definitely\\does\\not\\exist"),
            "claude.cmd",
            &[".claude", ".credentials.json"],
        );
        assert!(!status.installed);
        assert!(status.path.is_none());
        assert!(!status.signed_in);
    }

    #[test]
    fn detect_appdata_anthropic_at_reports_presence_accurately() {
        let dir =
            std::env::temp_dir().join(format!("roshera-appdata-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("Anthropic")).expect("temp dir must create");

        let report = detect_appdata_anthropic_at(Some(dir.to_str().expect("utf8 temp path")));
        assert!(report.present);

        let _ = std::fs::remove_dir_all(&dir);

        let report_after_removal =
            detect_appdata_anthropic_at(Some(dir.to_str().expect("utf8 temp path")));
        assert!(!report_after_removal.present);
    }

    // --- ACL: best-effort, must never panic or fail the save ---

    #[test]
    fn restrict_acl_windows_never_panics_and_leaves_file_readable() {
        let dir = std::env::temp_dir().join(format!("roshera-acl-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("temp dir must create");
        let path = dir.join("secret.json");
        std::fs::write(&path, b"{}").expect("write must succeed");

        // Whatever the outcome, the file must remain present and readable —
        // ACL restriction is best-effort and must never destroy the write.
        let _ = restrict_acl_windows(&path);
        assert!(std::fs::read(&path).is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
