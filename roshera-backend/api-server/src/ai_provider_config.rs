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
use serde_json::Value;
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
    /// User-selected model override. `None` means "the provider's own
    /// default choice" — never a fabricated model name; the dialog's
    /// `"default"` sentinel is normalized to `None` before this is built
    /// (`handlers/ai_provider.rs::resolve_requested_model`), so this
    /// field is either absent or a real, provider-checked (or explicitly
    /// unverified — see `model_verified`) model ID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Whether `model` was proven against the live provider. `None` when
    /// `model` is `None` (verification is moot — there is no override to
    /// distrust). For `api_key`/`oauth_profile` this is proven
    /// synchronously before save (`handlers/ai_provider.rs::resolve_requested_model`)
    /// and never changes afterward. For `subscription_cli` the Claude
    /// Code CLI has no side-effect-free synchronous model-listing
    /// endpoint this server can call from the request path, so save
    /// starts as `Some(false)` and a bounded background check
    /// (`verify_subscription_cli_model`) may later flip it to
    /// `Some(true)` — never silently presented as confirmed before that
    /// check actually ran. See `model_verification` for the full
    /// three(-plus-pending)-state detail this boolean can't carry
    /// (a named rejection reason, or why a check came back
    /// inconclusive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_verified: Option<bool>,
    /// The richer three(-plus-pending)-state outcome for `subscription_cli`
    /// model verification (see `ModelVerificationState` below this
    /// struct). `None` when `model` is `None`, or for `api_key`/
    /// `oauth_profile` (those verify synchronously against the live
    /// provider before save, and `model_verified` alone already tells
    /// the whole story for them). This field is what `model_verified`
    /// can't express on its own: `rejected` names the model and the
    /// CLI's own reason, `unknown` names why the check itself failed,
    /// and `pending` says a bounded background check is still running —
    /// never conflated with a pass or a fail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_verification: Option<ModelVerificationState>,
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
/// npm shim exists on disk, whether a real `CreateProcess`-spawnable
/// executable could be resolved from it, and whether its own sign-in
/// marker file is present. Neither check spawns a process.
///
/// `path` is NEVER a `.cmd`/`.ps1` shim — Windows' `CreateProcess` cannot
/// execute a batch/PowerShell wrapper directly (this was the live BLOCKER:
/// goose spawned `CLAUDE_CODE_COMMAND` verbatim via `CreateProcess`, and a
/// `.cmd` path there fails with "Failed to spawn"). `path` is `Some` only
/// when [`resolve_cli_exe`] confirmed a real, present executable; the shim
/// itself is kept separately in `shim_path` for diagnostics only, never as
/// a spawn target.
#[derive(Debug, Clone, Serialize)]
pub struct CliStatus {
    pub installed: bool,
    /// The real, directly-spawnable executable — never the npm shim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The npm shim's own path (`.cmd`), kept for diagnostics/troubleshooting.
    /// Never a valid `CreateProcess` target — never write this into
    /// `CLAUDE_CODE_COMMAND` or any other spawn config.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shim_path: Option<String>,
    /// Set only when `installed` is true and `path` is `None`: names what
    /// was checked and why no directly-spawnable executable could be
    /// resolved, so a refusal built from this can cite specifics instead
    /// of a bare "not found".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution_note: Option<String>,
    pub signed_in: bool,
}

/// Outcome of resolving a shim's real, `CreateProcess`-spawnable target.
struct ExeResolution {
    exe_path: Option<PathBuf>,
    unresolved_reason: Option<String>,
}

/// Resolve the shim's real, directly-spawnable executable.
///
/// Two strategies, tried in order:
///
/// 1. **Known relative layout** (`known_relative_exe`, when the caller has
///    one): join it onto the npm prefix and accept it if the file is
///    actually present. This is the verified Claude Code shape —
///    `claude.cmd` is only a batch wrapper around
///    `node_modules\@anthropic-ai\claude-code\bin\claude.exe`, a real ~250
///    MB native binary (confirmed live 2026-07-31).
/// 2. **Shim parsing** ([`parse_cmd_shim_for_direct_exe`]): read the `.cmd`
///    itself and look for a single, unconditionally-invoked `.exe` target.
///    This is the fallback for a layout that differs from what's hardcoded
///    above (a future Claude Code repackaging, or any other CLI whose shim
///    happens to be a direct-binary wrapper the same way).
///
/// Neither strategy resolving is a legitimate outcome, not a bug: Codex's
/// own shim (`codex.cmd`) dispatches through `node.exe` to a `.js` entry
/// point — there is no single native executable this process can spawn
/// directly, and guessing at a platform-specific binary name (e.g.
/// `codex-x86_64-pc-windows-msvc.exe`) would be exactly the kind of
/// unverified assumption this module's CLI detection exists to avoid. That
/// case reports `unresolved_reason` honestly instead.
fn resolve_cli_exe(
    npm_prefix: &Path,
    shim_path: &Path,
    known_relative_exe: Option<&[&str]>,
) -> ExeResolution {
    if let Some(segments) = known_relative_exe {
        let mut candidate = npm_prefix.to_path_buf();
        for segment in segments {
            candidate.push(segment);
        }
        if candidate.is_file() {
            return ExeResolution {
                exe_path: Some(candidate),
                unresolved_reason: None,
            };
        }
    }

    if let Some(exe) = parse_cmd_shim_for_direct_exe(shim_path) {
        return ExeResolution {
            exe_path: Some(exe),
            unresolved_reason: None,
        };
    }

    ExeResolution {
        exe_path: None,
        unresolved_reason: Some(format!(
            "the npm shim at {} does not directly invoke a native executable \
             this process can spawn (CreateProcess cannot execute a .cmd/.ps1 \
             shim itself, no known real-binary layout matched, and no \
             unconditional .exe target could be parsed out of the shim)",
            shim_path.display()
        )),
    }
}

/// Parse an npm-generated `.cmd` shim for a directly spawnable executable
/// it unconditionally invokes.
///
/// npm's shim generator (`cmd-shim`) emits one of two shapes:
/// - A **direct-binary shim** (verified live for Claude Code): the body
///   invokes a single quoted `"%dp0%\...\<name>.exe"` path unconditionally
///   — this is what this parser resolves.
/// - A **node-dispatch shim** (most JS-only CLIs, including Codex): the
///   body branches on whether a local `node.exe` exists (an `IF EXIST`
///   block) before invoking a `.js` entry point through whichever `node`
///   it found. `CreateProcess` cannot spawn a `.js` file directly, and
///   there are two candidate interpreter paths (local vs. `PATH`-resolved
///   `node`) with no single honest answer — this parser deliberately skips
///   any line inside a conditional rather than guessing which branch a
///   real invocation would take.
///
/// Read-only: never executes the shim. Returns `None` (not a fabricated
/// guess) when no unconditional `.exe` target is found, or the resolved
/// candidate does not actually exist on disk.
fn parse_cmd_shim_for_direct_exe(cmd_path: &Path) -> Option<PathBuf> {
    let contents = std::fs::read_to_string(cmd_path).ok()?;
    let dp0 = cmd_path.parent()?;

    for line in contents.lines() {
        let trimmed = line.trim();
        let upper = trimmed.to_ascii_uppercase();
        // Skip conditional branches entirely — only a line that cannot be
        // part of an IF/ELSE decision counts as "unconditionally invoked".
        if upper.starts_with("IF ") || upper.starts_with("ELSE") || upper.contains("EXIST") {
            continue;
        }
        let Some(start) = trimmed.find("\"%dp0%") else {
            continue;
        };
        let rest = &trimmed[start + 1..];
        let Some(end) = rest.find('"') else {
            continue;
        };
        let quoted = &rest[..end];
        if !quoted.to_ascii_lowercase().ends_with(".exe") {
            continue;
        }
        let relative = quoted
            .trim_start_matches("%dp0%")
            .trim_start_matches(['\\', '/']);
        let candidate = dp0.join(relative);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn detect_cli_at(
    appdata: Option<&str>,
    userprofile: Option<&str>,
    shim_name: &str,
    credentials_rel_path: &[&str],
    known_relative_exe: Option<&[&str]>,
) -> CliStatus {
    let npm_prefix = appdata.map(|a| Path::new(a).join("npm"));
    let shim_path = npm_prefix.as_ref().map(|p| p.join(shim_name));
    let installed = shim_path.as_deref().map(Path::exists).unwrap_or(false);

    let signed_in = userprofile
        .map(|home| {
            let mut p = PathBuf::from(home);
            for segment in credentials_rel_path {
                p.push(segment);
            }
            p.exists()
        })
        .unwrap_or(false);

    if !installed {
        return CliStatus {
            installed: false,
            path: None,
            shim_path: None,
            resolution_note: None,
            signed_in,
        };
    }

    let resolution = match (&npm_prefix, &shim_path) {
        (Some(prefix), Some(shim)) => resolve_cli_exe(prefix, shim, known_relative_exe),
        _ => ExeResolution {
            exe_path: None,
            unresolved_reason: Some(
                "APPDATA was unavailable, so no npm prefix could be resolved".to_string(),
            ),
        },
    };

    CliStatus {
        installed: true,
        path: resolution.exe_path.map(|p| p.display().to_string()),
        shim_path: shim_path.map(|p| p.display().to_string()),
        resolution_note: resolution.unresolved_reason,
        signed_in,
    }
}

/// The relative path, from the npm prefix, to Claude Code's real,
/// directly-spawnable native binary. Verified live 2026-07-31: `claude.cmd`
/// is only a batch wrapper (`"%dp0%\node_modules\@anthropic-ai\claude-code\bin\claude.exe" %*`)
/// around this ~250 MB native executable.
const CLAUDE_CODE_EXE_RELATIVE: &[&str] = &[
    "node_modules",
    "@anthropic-ai",
    "claude-code",
    "bin",
    "claude.exe",
];

/// Detect the Claude Code CLI: shim at `%APPDATA%\npm\claude.cmd`,
/// signed-in marker at `~/.claude/.credentials.json`. `path` (when
/// present) is the real `claude.exe`, never the `.cmd` shim — see
/// [`CliStatus`]'s doc for why that distinction is load-bearing.
pub fn detect_claude_cli() -> CliStatus {
    detect_cli_at(
        std::env::var("APPDATA").ok().as_deref(),
        std::env::var("USERPROFILE").ok().as_deref(),
        "claude.cmd",
        &[".claude", ".credentials.json"],
        Some(CLAUDE_CODE_EXE_RELATIVE),
    )
}

/// Detect the Codex CLI: shim at `%APPDATA%\npm\codex.cmd`, signed-in
/// marker at `~/.codex/auth.json`. No known direct-binary layout is
/// hardcoded for Codex (verified live: its shim dispatches through
/// `node.exe` to a `.js` entry point, not a single native executable) — a
/// present shim whose target `parse_cmd_shim_for_direct_exe` cannot
/// resolve reports `resolution_note` honestly rather than guessing at a
/// platform-specific binary name.
pub fn detect_codex_cli() -> CliStatus {
    detect_cli_at(
        std::env::var("APPDATA").ok().as_deref(),
        std::env::var("USERPROFILE").ok().as_deref(),
        "codex.cmd",
        &[".codex", "auth.json"],
        None,
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
/// at the resolved, directly-spawnable `claude.exe` — NEVER the `.cmd`
/// npm shim. Config reads are live in goose — no restart needed.
///
/// Called from the PUT handler only after: (1) the allowlist confirmed
/// `subscription_cli` is `Wired` for this provider, (2) explicit consent
/// was supplied in the request body, and (3) CLI detection found the
/// shim installed and signed in. This function itself performs no
/// consent or detection checks beyond the one below — it is otherwise the
/// mechanical config write.
///
/// **Structural enforcement of the CLI-detection invariant.** goose spawns
/// `CLAUDE_CODE_COMMAND` via `CreateProcess`, which cannot execute a
/// `.cmd`/`.ps1`/`.bat` wrapper directly — this was the live BLOCKER
/// ("Failed to spawn Claude CLI command '"...\npm...'"). `detect_claude_cli`
/// / [`resolve_boot_provider_pin`] already resolve to the real executable,
/// but this write site is the one place `CLAUDE_CODE_COMMAND` is ever set,
/// so the extension check belongs HERE too: a caller that (today or after
/// a future edit) hands this a shim path must be refused by name rather
/// than silently writing a command Windows cannot spawn.
pub fn repin_goose_to_claude_code(claude_cli_path: &Path) -> Result<(), String> {
    if let Some(ext) = claude_cli_path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
    {
        if matches!(ext.as_str(), "cmd" | "ps1" | "bat") {
            return Err(format!(
                "refusing to pin CLAUDE_CODE_COMMAND to '{}' — it is a .{ext} \
                 shim, which Windows' CreateProcess cannot execute directly \
                 (goose spawns this path as-is); resolve the real executable \
                 first (see detect_claude_cli / resolve_cli_exe)",
                claude_cli_path.display()
            ));
        }
    }
    let config = goose::config::Config::global();
    goose::config::set_active_provider(config, "claude-code", "default")
        .map_err(|e| format!("failed to pin goose's active_provider to claude-code: {e}"))?;
    config
        .set_param("CLAUDE_CODE_COMMAND", claude_cli_path.display().to_string())
        .map_err(|e| format!("failed to set CLAUDE_CODE_COMMAND: {e}"))?;
    Ok(())
}

// ── Subscription-CLI model verification (bounded, off the request path) ──
//
// `subscription_cli` model overrides used to be persisted with
// `model_verified: false` permanently — the API never actually asked the
// CLI whether the model exists. That was a reasonable scoping call (it
// means spawning a subprocess) but not an acceptable permanent answer:
// the CLI itself is the authority on whether a model name is real
// (`claude --model <bad-name> -p ... --output-format json` replies with
// `is_error: true` and a named reason in well under a second — verified
// live on this machine), so the check belongs here, just not on the PUT
// request path. `PUT` kicks off this bounded check in the background
// after the save already returned; `GET` reports whatever the most
// recent outcome is.
//
// No cheaper signal exists: `claude --help` was checked live for a
// models-list/validate subcommand (none — `agents`, `auth`, `auto-mode`,
// `doctor`, `gateway`, `install`, `mcp`, `plugin`, `project`,
// `setup-token`, `ultrareview`, `update` are the only subcommands, none
// of them enumerate or validate a model name without starting a session).
// A live `-p`/`--output-format json` round trip is the cheapest available
// probe, and the rejection path returns fast (the CLI checks the model
// name before spending any inference budget — observed ~1.9s for a bad
// name versus ~19s for an accepted one).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// Hard ceiling on the background verification round trip. On timeout
/// the child is killed (never left orphaned — see [`spawn_and_check`]'s
/// use of `kill_on_drop`) and the outcome is [`ProbeOutcome::Unknown`],
/// never a guessed pass or fail.
const MODEL_VERIFICATION_TIMEOUT: Duration = Duration::from_secs(45);

/// What a single CLI probe produced. Three states, matching the module
/// doc's honesty requirement: `Verified` only when the CLI actually
/// accepted the model, `Rejected` only when the CLI explicitly said so
/// (reason carried verbatim from its own `result` field), `Unknown` for
/// anything that did not run to a conclusion — a timeout, a spawn
/// failure, or output this process could not parse. `Unknown` must never
/// be read as either a pass or a fail by a caller. `pub` (not just
/// crate-visible) so a test in `handlers::ai_provider` can construct a
/// fake [`ModelProbe`] without spawning the real CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    Verified,
    Rejected(String),
    Unknown(String),
}

/// Persisted, reportable form of [`ProbeOutcome`] plus the one state a
/// single probe can never represent on its own: `Pending`, for the
/// window between "the background check was kicked off" and "it
/// finished" — `GET` must be able to say that honestly instead of
/// defaulting to `unknown` (which is reserved for a check that could NOT
/// run to a conclusion, not one still running) or silently omitting the
/// field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ModelVerificationState {
    /// The check was kicked off after save and has not completed yet.
    Pending,
    /// A live `claude --model <model> -p ... --output-format json` call
    /// completed with `is_error: false` — the CLI actually accepted this
    /// model.
    Verified,
    /// The CLI explicitly rejected the model. `model` is named so a
    /// caller never has to cross-reference `active.model` to know what
    /// failed; `reason` is the CLI's own explanation.
    Rejected { model: String, reason: String },
    /// The check could not run to a conclusion — timeout, spawn failure,
    /// or unparsable CLI output. Never a proxy for `verified` or
    /// `rejected`.
    Unknown { reason: String },
}

/// A CLI probe, abstracted so tests can substitute a fake instead of
/// spawning the real ~250 MB `claude.exe` binary. Takes the resolved,
/// directly-spawnable executable path (never a `.cmd`/`.ps1` shim — see
/// [`resolve_cli_exe`]) and the model name; returns the probe outcome.
pub type ModelProbe = Arc<
    dyn Fn(PathBuf, String) -> Pin<Box<dyn Future<Output = ProbeOutcome> + Send>> + Send + Sync,
>;

/// Parse the CLI's `--output-format json` stdout into a [`ProbeOutcome`].
/// `is_error: false` → accepted; `is_error: true` → rejected, reason
/// taken from the CLI's own `result` field (observed live: "There's an
/// issue with the selected model (<name>). It may not exist or you may
/// not have access to it. Run --model to pick a different model.");
/// anything that doesn't parse as JSON, or parses but has no boolean
/// `is_error`, is `Unknown` — an unrecognized shape is not evidence of
/// either outcome.
fn parse_cli_output(stdout: &[u8], stderr: &[u8], exit_code: Option<i32>) -> ProbeOutcome {
    if stdout.is_empty() {
        return ProbeOutcome::Unknown(format!(
            "CLI produced no stdout to verify the model against (exit {:?}): {}",
            exit_code,
            String::from_utf8_lossy(stderr).trim()
        ));
    }
    let parsed: serde_json::Value = match serde_json::from_slice(stdout) {
        Ok(v) => v,
        Err(e) => {
            return ProbeOutcome::Unknown(format!(
                "could not parse the CLI's --output-format json stdout: {e}"
            ))
        }
    };
    match parsed.get("is_error").and_then(Value::as_bool) {
        Some(false) => ProbeOutcome::Verified,
        Some(true) => {
            let reason = parsed
                .get("result")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| {
                    "the CLI reported an error for this model but named no reason".to_string()
                });
            ProbeOutcome::Rejected(reason)
        }
        None => ProbeOutcome::Unknown(
            "CLI output parsed as JSON but had no boolean 'is_error' field".to_string(),
        ),
    }
}

/// Spawn the resolved `claude.exe` (never a `.cmd`/`.ps1` shim — the
/// caller is responsible for that guarantee, exactly as
/// [`repin_goose_to_claude_code`] requires) with the chosen model and a
/// trivial prompt, and parse its `--output-format json` reply.
///
/// `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` are scrubbed from the
/// child's environment — same rule as
/// [`scrub_anthropic_env_for_subscription_mode`]: a spawned CLI must
/// never silently authenticate with a leftover API key instead of the
/// user's subscription login. `kill_on_drop(true)` is the orphan
/// guarantee: if the caller wraps this future in a timeout and the
/// timeout fires, dropping this future drops the still-owned `Child`
/// handle, and tokio kills the OS process on that drop — no separate
/// `.kill()` call is reachable once `wait_with_output` has taken
/// ownership of `child`, so this is the only mechanism available and it
/// is unconditional.
async fn spawn_and_check(cli_path: PathBuf, model: String) -> ProbeOutcome {
    let mut command = tokio::process::Command::new(&cli_path);
    command
        .arg("--model")
        .arg(&model)
        .arg("-p")
        .arg("Reply with exactly: OK")
        .arg("--output-format")
        .arg("json")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);

    let child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            return ProbeOutcome::Unknown(format!(
                "failed to spawn {} to verify the model: {e}",
                cli_path.display()
            ))
        }
    };

    match child.wait_with_output().await {
        Ok(output) => parse_cli_output(&output.stdout, &output.stderr, output.status.code()),
        Err(e) => ProbeOutcome::Unknown(format!("failed while waiting for the CLI: {e}")),
    }
}

/// Apply [`MODEL_VERIFICATION_TIMEOUT`] to a probe future. Split out from
/// [`spawn_and_check`] so the timeout duration is injectable in tests
/// (a real `claude.exe` round trip cannot be made to time out on demand
/// without actually waiting 45s) without needing to fake the process
/// spawn itself.
async fn run_probe_with_timeout(
    probe: impl Future<Output = ProbeOutcome>,
    timeout: Duration,
) -> ProbeOutcome {
    match tokio::time::timeout(timeout, probe).await {
        Ok(outcome) => outcome,
        Err(_) => ProbeOutcome::Unknown(format!(
            "model verification timed out after {}s",
            timeout.as_secs()
        )),
    }
}

/// The production [`ModelProbe`]: a live, timeout-bounded
/// `claude.exe` round trip. The only untested-by-necessity piece of this
/// feature (mirrors [`boot_provider_pin_for`]'s relationship to
/// [`resolve_boot_provider_pin`]) — everything it calls
/// ([`parse_cli_output`], [`run_probe_with_timeout`]) is unit-tested on
/// its own.
pub fn default_model_probe() -> ModelProbe {
    Arc::new(|cli_path, model| {
        Box::pin(run_probe_with_timeout(
            spawn_and_check(cli_path, model),
            MODEL_VERIFICATION_TIMEOUT,
        ))
    })
}

/// Run one bounded verification and persist the outcome — the whole
/// point of doing this off the request path. Called via `tokio::spawn`
/// from `PUT /api/ai/provider`'s `subscription_cli` branch, strictly
/// after the save that already returned to the caller; never awaited by
/// a request handler.
///
/// Stale-result guard lives in [`AiProviderManager::update_model_verification`]:
/// if the model was changed again (a second PUT landed) before this
/// probe finished, the outcome is discarded rather than clobbering a
/// newer save with a result for a model that is no longer active.
pub async fn verify_subscription_cli_model(
    probe: ModelProbe,
    cli_path: PathBuf,
    model: String,
    manager: Arc<AiProviderManager>,
) {
    let outcome: ProbeOutcome = probe(cli_path, model.clone()).await;
    let state = match outcome {
        ProbeOutcome::Verified => ModelVerificationState::Verified,
        ProbeOutcome::Rejected(reason) => ModelVerificationState::Rejected {
            model: model.clone(),
            reason,
        },
        ProbeOutcome::Unknown(reason) => ModelVerificationState::Unknown { reason },
    };
    if let Err(e) = manager.update_model_verification(&model, state).await {
        tracing::warn!(
            target: "api_server.ai_provider",
            error = %e,
            model = %model,
            "failed to persist the background model-verification outcome"
        );
    }
}

// ── goose repin, generalized (declarative-provider API-key vendors) ────
//
// `repin_goose_to_claude_code` above stays special-cased: it resolves a
// real spawnable executable and writes `CLAUDE_CODE_COMMAND`, neither of
// which any other entry needs. The four vendors goose already registers
// through its declarative-provider system (`crates/goose/src/config/
// declarative_providers.rs`, wired at `providers/init.rs:220`, bundled
// JSON at `crates/goose-providers/src/declarative/definitions/
// {mistral,zhipu,moonshot}.json`) — plus `xai`, a hand-written native
// provider registered the same way in `providers/init.rs` — only need
// two things: the API key available where each provider's own
// credential resolution looks for it, and `active_provider` pinned to
// its name. Verified against goose's own source (not guessed): every
// one of these constructs its credential via
// `Config::get_secret(<KEY>)`, and `Config::get_secret` checks
// `env::var(&key.to_uppercase())` BEFORE its keyring
// (`crates/goose/src/config/base.rs::get_secret`) — the exact mechanism
// that already makes a bare `ANTHROPIC_API_KEY` env var work for goose's
// built-in `anthropic` provider with no repin at all. Setting the env
// var here is that same mechanism, generalized — not a new one.

/// Map a Roshera allowlist provider id to the goose provider name it
/// should be repinned to, and the API-key env var goose's own credential
/// resolution reads for it. `None` for anything with no goose repin
/// target: `"anthropic"` needs none (its `api_key`/`oauth_profile` modes
/// already work via goose's hardcoded default, unchanged by this
/// module), and `"baseten"` has no goose provider to repin to at all —
/// see `ai-integration`'s allowlist entry for why.
/// The third element is a DEFAULT MODEL, and it is `None` unless the
/// exact identifier has been verified against that vendor's live API.
///
/// goose resolves `GOOSE_MODEL` from whatever this repin pins, and a
/// provider pinned with no model fails at the first turn with
/// "Configuration value not found: GOOSE_MODEL" — a late, opaque error
/// for a decision made much earlier. Supplying a *guessed* identifier
/// would be worse: `sarvam-30b` was carried in this repo's own config
/// until 2026-08-01, when the live model list showed it does not exist.
/// A wrong default fails at first use while looking configured, which is
/// precisely the shape of failure this module exists to refuse. So an
/// unverified vendor gets `None` and the caller must name the model.
///
/// **This hardcoded default is now the FALLBACK, not the primary
/// source.** Live model discovery (`resolve_provider_base_url` +
/// `fetch_vendor_models`, below) asks the vendor directly and is
/// preferred whenever a caller has a key in hand to query with. This
/// table stays because discovery cannot run at boot from persisted
/// config alone (`PUT`'s stored `api_key` is available then, but nothing
/// calls out over the network at boot) — it is the "connect at boot with
/// no live query" path, kept honest by requiring the identifier to have
/// been verified at least once, same as before.
fn goose_declarative_provider_for(
    roshera_provider_id: &str,
) -> Option<(&'static str, &'static str, Option<&'static str>)> {
    match roshera_provider_id {
        "xai" => Some(("xai", "XAI_API_KEY", None)),
        "mistral" => Some(("mistral", "MISTRAL_API_KEY", None)),
        // Roshera's allowlist id is "glm" (the model family users type);
        // goose's own declarative-provider name for the same vendor is
        // "zhipu" (`zhipu.json`'s `"name"` field) — these are
        // deliberately different strings, not a typo.
        "glm" => Some(("zhipu", "ZHIPU_API_KEY", None)),
        // Roshera's "kimi" is goose's "moonshot"
        // (`moonshot.json`) — distinct from `kimi_code`, an unrelated
        // OAuth-device-flow CLI product goose separately supports that
        // this module does not touch.
        "kimi" => Some(("moonshot", "MOONSHOT_API_KEY", None)),
        // Unlike xai/mistral/glm/kimi, Sarvam has no goose-bundled
        // declarative provider. Instead its definition is a custom-provider
        // JSON at `state/goose-root/config/custom_providers/sarvam.json`,
        // which `declarative_providers.rs::register_declarative_providers`
        // loads and registers into the same `ProviderRegistry` as the
        // bundled vendors, keyed by the same `name` field. Roshera's
        // allowlist id and goose's provider name are therefore identical
        // (`sarvam` -> `sarvam`) — not a rename like glm -> zhipu, just the
        // same string reused because there was never a second name to
        // reconcile.
        // `sarvam-105b` is the ONLY chat model `GET https://api.sarvam.ai/
        // v1/models` returned on 2026-08-01 (checked live, not from the
        // docs — the docs also list `sarvam-30b`, which that endpoint does
        // not serve). It is a default here because it was verified, not
        // because it was plausible.
        "sarvam" => Some(("sarvam", "SARVAM_API_KEY", Some("sarvam-105b"))),
        _ => None,
    }
}

/// Pin goose's `active_provider` to the declarative/native provider
/// backing `roshera_provider_id`, supplying its API key via the exact
/// env var that provider's own credential resolution reads (see the
/// section doc above for why setting the env var — not writing to
/// goose's keyring — is sufficient and correct).
///
/// Called from `PUT /api/ai/provider`'s `api_key`-mode branch, for any
/// allowlisted provider other than `"anthropic"` (which needs no repin —
/// see [`goose_declarative_provider_for`]). Refuses by name, never
/// silently no-ops, when the provider has no known goose repin target —
/// structurally the same guard `repin_goose_to_claude_code` applies for
/// a `.cmd` shim: a caller must not be able to mark a provider `Wired`
/// in the allowlist without this function actually being able to serve
/// it.
pub fn repin_goose_to_declarative_provider(
    roshera_provider_id: &str,
    api_key: &str,
    model: Option<&str>,
) -> Result<(), String> {
    let (goose_provider_name, api_key_env_var, default_model) =
        goose_declarative_provider_for(roshera_provider_id).ok_or_else(|| {
            format!(
                "'{roshera_provider_id}' has no known goose provider to repin to — \
                 refusing rather than silently leaving goose's active_provider \
                 unchanged"
            )
        })?;

    // A provider pinned with no model is not a pinned provider. goose
    // resolves GOOSE_MODEL from this write, so an empty one is accepted
    // here and then fails at the first turn with "Configuration value not
    // found: GOOSE_MODEL" — an error naming a variable the user never set,
    // about a decision made minutes earlier, in a provider the dialog is
    // meanwhile showing as connected. This used to be
    // `model.unwrap_or_default()`, which turns "no model" into "" without
    // saying so. Refuse instead, in the same breath as the unmapped-provider
    // refusal directly above: a caller must not be able to leave goose in a
    // state that cannot serve a turn.
    let resolved_model = model
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .or(default_model)
        .ok_or_else(|| {
            format!(
                "no model given for '{roshera_provider_id}' and no verified default \
                 exists for it — name the model to use. Refusing rather than pinning \
                 a provider with an empty model, which would fail at the first turn \
                 with 'Configuration value not found: GOOSE_MODEL'"
            )
        })?;

    // Same mechanism already documented for ANTHROPIC_API_KEY: goose's
    // own `Config::get_secret` checks this process's environment first.
    // `edition = "2021"` (`api-server/Cargo.toml:4`), so `set_var` needs
    // no `unsafe` block — that requirement is edition 2024+, same note
    // already on `scrub_anthropic_env_for_subscription_mode`.
    std::env::set_var(api_key_env_var, api_key);

    let config = goose::config::Config::global();
    goose::config::set_active_provider(config, goose_provider_name, resolved_model).map_err(
        |e| format!("failed to pin goose's active_provider to {goose_provider_name}: {e}"),
    )?;
    Ok(())
}

// ── Live model discovery (ask the vendor, don't hardcode) ─────────────
//
// `goose_declarative_provider_for` above carries a per-vendor DEFAULT
// model, and the module doc right above it already explains why that
// default must be verified, never guessed: `sarvam-30b` was carried in
// this repo's own config until 2026-08-01, when `GET
// https://api.sarvam.ai/v1/models` showed the live API only ever served
// `sarvam-105b`. That same endpoint — `GET /v1/models`, the exact probe
// `ClaudeProvider::validate_credential` already runs against Anthropic —
// is universal across every OpenAI-compatible vendor this module talks
// to. This section generalizes it: given a Roshera provider id, resolve
// the vendor's real base URL from a source that already exists for a
// different reason (the custom-provider JSON or goose's bundled
// declarative definitions — see `repin_goose_to_declarative_provider`'s
// doc for why those are trustworthy), then ask the vendor what it
// actually serves. `POST /api/ai/provider/models`
// (`handlers/ai_provider.rs::discover_provider_models`) is the one
// caller; this module owns the resolution + HTTP + parsing, exactly the
// existing split between this file and the handler for every other
// entry point here.
//
// NOTE: `state/goose-root/config/custom_providers/sarvam.json` still
// lists `sarvam-30b` in its own `models` array on disk as of this
// writing — the exact drift this endpoint exists to catch. That static
// list is NEVER read by the functions below; only `base_url` is. Live
// discovery supersedes it rather than reconciling it.

/// Which of the two resolution tiers answered. Surfaced to the caller so
/// a discovery response can say where its base URL came from, not just
/// the URL itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BaseUrlSource {
    /// `state/goose-root/config/custom_providers/<name>.json` — checked
    /// FIRST because it is this deployment's own override (Sarvam has no
    /// goose-bundled provider at all, so this is its only source).
    CustomProviderJson,
    /// One of goose's own bundled declarative definitions
    /// (`goose-providers/src/declarative/definitions/*.json`, compiled
    /// into the pinned goose git rev — `api-server/Cargo.toml`).
    BundledDeclarative,
}

/// A resolved (not yet dereferenced) base URL, plus which tier answered.
#[derive(Debug, Clone)]
pub struct ResolvedBaseUrl {
    pub base_url: String,
    pub source: BaseUrlSource,
}

/// Typed refusal naming exactly why no base URL could be resolved for a
/// Roshera provider id — never a guessed fallback. Two independent ways
/// to land here: the id has no known goose provider name at all
/// (`goose_declarative_provider_for` returned `None` — `anthropic`
/// needs no repin, `openai`/`baseten` are seam-only), or it does, but
/// neither tier has a JSON definition for that name. `xai` is the
/// concrete example of the second case: goose registers it as a
/// hand-written native provider whose base URL is a Rust constant
/// (`goose::providers::xai::XAI_API_HOST`), never a JSON file either
/// tier here reads — so it is allowlisted and `Wired` for inference, yet
/// this function still cannot resolve a base URL for it, and says so by
/// name instead of inventing one.
#[derive(Debug, Clone)]
pub struct BaseUrlUnresolved {
    pub roshera_provider_id: String,
    pub reason: String,
}

impl std::fmt::Display for BaseUrlUnresolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "no base URL could be resolved for provider '{}': {}",
            self.roshera_provider_id, self.reason
        )
    }
}

/// Pure resolution over an explicit, already-loaded set of custom +
/// bundled configs — no filesystem or `goose::config::Config::global()`
/// access — so the "custom JSON wins" priority rule is unit-testable
/// with fixture data. Mirrors this module's `build_chain` /
/// `resolve_boot_provider_pin` pattern: decision logic stays pure;
/// [`resolve_provider_base_url`] is the thin live wrapper that supplies
/// the real directory scan and bundled-definition list.
///
/// Only expands `${VAR}` placeholders for the BUNDLED tier (`zhipu`/
/// `moonshot`'s own definitions use them for a region-specific host).
/// `load_custom_providers` never runs goose's lazy env-var expansion —
/// harmless for every custom-provider JSON this repo ships today
/// (Sarvam's `base_url` is a literal string with no `${...}`), but a
/// future custom JSON that used a placeholder would need that expansion
/// added here too.
fn resolve_base_url_from(
    goose_provider_name: &str,
    custom: &[goose::config::declarative_providers::DeclarativeProviderConfig],
    bundled: &[goose::config::declarative_providers::DeclarativeProviderConfig],
) -> Option<(String, BaseUrlSource)> {
    if let Some(cfg) = custom.iter().find(|c| c.name == goose_provider_name) {
        return Some((cfg.base_url.clone(), BaseUrlSource::CustomProviderJson));
    }
    let cfg = bundled.iter().find(|c| c.name == goose_provider_name)?;
    let base_url = match cfg.env_vars.as_deref() {
        Some(vars) if !vars.is_empty() => {
            goose::config::declarative_providers::expand_env_vars(&cfg.base_url, vars).ok()?
        }
        _ => cfg.base_url.clone(),
    };
    Some((base_url, BaseUrlSource::BundledDeclarative))
}

/// Live wrapper around [`resolve_base_url_from`]: loads the real
/// custom-provider directory (`GOOSE_PATH_ROOT`-relative, pinned to
/// `state/goose-root` at boot by `goose_acp::initialize`) and goose's
/// compiled-in bundled definitions.
pub fn resolve_provider_base_url(
    roshera_provider_id: &str,
) -> Result<ResolvedBaseUrl, BaseUrlUnresolved> {
    let goose_provider_name = goose_declarative_provider_for(roshera_provider_id)
        .map(|(name, _, _)| name)
        .ok_or_else(|| BaseUrlUnresolved {
            roshera_provider_id: roshera_provider_id.to_string(),
            reason: "this id has no known goose provider to resolve a base URL for \
                     (the same gap `repin_goose_to_declarative_provider` refuses by \
                     name when pinning the agent surface)"
                .to_string(),
        })?;

    let dir = goose::config::declarative_providers::custom_providers_dir();
    let custom =
        goose::config::declarative_providers::load_custom_providers(&dir).unwrap_or_default();
    let bundled =
        goose::config::declarative_providers::fixed_provider_configs().unwrap_or_default();

    resolve_base_url_from(goose_provider_name, &custom, &bundled)
        .map(|(base_url, source)| ResolvedBaseUrl { base_url, source })
        .ok_or_else(|| BaseUrlUnresolved {
            roshera_provider_id: roshera_provider_id.to_string(),
            reason: format!(
                "no custom-provider JSON at {} and no bundled declarative \
                 definition named '{goose_provider_name}' — e.g. xai is a \
                 hand-written native goose provider (its base URL is a Rust \
                 constant, not a JSON file), invisible to either tier here",
                dir.display()
            ),
        })
}

/// Derive the vendor's `/models` listing URL from its raw, declared base
/// URL. Some declarations are already the API root (Sarvam's
/// custom-provider JSON: `https://api.sarvam.ai/v1`, appended to
/// literally); goose's own bundled declarative definitions instead
/// declare the chat-completions endpoint directly (Mistral:
/// `https://api.mistral.ai/v1/chat/completions`). Appending `/models`
/// onto the latter unmodified would hit
/// `.../v1/chat/completions/models`, a guaranteed 404 — so a trailing
/// completions/responses segment is stripped first. This mirrors
/// verified behaviour already in the dependency being resolved against,
/// not a guessed convention: goose's own `OpenAiProvider` builds its
/// `/models` request the same way (`map_base_path` in
/// `goose-providers/src/openai.rs` replaces a `chat/completions` — or
/// `responses` — tail with `models` before requesting).
pub fn models_url(raw_base_url: &str) -> String {
    let trimmed = raw_base_url.trim_end_matches('/');
    let root = ["/chat/completions", "/responses"]
        .iter()
        .find_map(|suffix| trimmed.strip_suffix(suffix))
        .unwrap_or(trimmed);
    format!("{root}/models")
}

/// Reject input that cannot plausibly be an API key before it is ever
/// sent over the network or held in memory as a credential a moment
/// longer than necessary. Fixes a real incident: a 649-character
/// multi-line Vite error message was pasted into the key field and
/// persisted to `state/ai-provider.json` as a credential. `PUT
/// /api/ai/provider` does not yet apply this check (out of scope for
/// this change — see the doc on this function's caller); wiring it there
/// too is a known residual. Deliberately permissive on the actual
/// character set (vendors vary widely), strict on the SHAPE a real key
/// can never have.
pub fn reject_implausible_key_shape(raw: &str) -> Result<(), String> {
    if raw.trim().is_empty() {
        return Err("is empty".to_string());
    }
    if raw.contains('\n') || raw.contains('\r') {
        return Err(
            "contains a line break — API keys are a single token with no line \
             breaks; this looks like pasted multi-line text (an error message, a \
             log, a JSON blob) instead"
                .to_string(),
        );
    }
    if raw.starts_with(char::is_whitespace) {
        return Err("has leading whitespace — paste just the key itself".to_string());
    }
    if raw.starts_with(['{', '[', '<']) {
        return Err(
            "starts with a bracket character — looks like pasted JSON/HTML/error \
             text, not an API key"
                .to_string(),
        );
    }
    const MAX_PLAUSIBLE_KEY_LEN: usize = 256;
    if raw.len() > MAX_PLAUSIBLE_KEY_LEN {
        return Err(format!(
            "is {} characters long — no known vendor issues an API key that long; \
             rejecting anything over {MAX_PLAUSIBLE_KEY_LEN} characters",
            raw.len()
        ));
    }
    Ok(())
}

/// Hard ceiling on a live model-discovery round trip — the same
/// "distinguishable timeout, never a guessed pass/fail" rule already
/// applied to [`MODEL_VERIFICATION_TIMEOUT`] above, scoped shorter
/// because this is a single unauthenticated-cost `GET`, on the request
/// path (unlike that background probe), not a `-p` inference call.
const MODEL_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);

/// One model as the vendor itself named it — never merged, supplemented,
/// or reordered against any local list.
#[derive(Debug, Clone, Serialize)]
pub struct DiscoveredModel {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_limit: Option<u64>,
}

/// What a live `GET {base}/models` round trip produced, distinguishable
/// enough that a caller can tell "the key is wrong" from "the URL is
/// wrong" from "the vendor didn't answer in time" — never collapsed into
/// one generic failure.
#[derive(Debug)]
pub enum VendorModelsError {
    /// 401/403 — the credential itself was rejected.
    Unauthorized { status: u16, message: String },
    /// 404 — the resolved URL does not exist on the vendor.
    NotFound { status: u16, message: String },
    /// Any other non-2xx status this function has no specific
    /// interpretation for.
    UnexpectedStatus { status: u16, message: String },
    /// The round trip did not complete within [`MODEL_DISCOVERY_TIMEOUT`].
    Timeout,
    /// Connection failure, TLS failure, or a 2xx body that did not parse
    /// as JSON.
    Transport(String),
}

impl std::fmt::Display for VendorModelsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VendorModelsError::Unauthorized { status, message } => {
                write!(f, "vendor rejected the credential ({status}): {message}")
            }
            VendorModelsError::NotFound { status, message } => {
                write!(f, "vendor returned {status} for the models URL: {message}")
            }
            VendorModelsError::UnexpectedStatus { status, message } => {
                write!(f, "vendor returned unexpected status {status}: {message}")
            }
            VendorModelsError::Timeout => write!(
                f,
                "model discovery timed out after {}s",
                MODEL_DISCOVERY_TIMEOUT.as_secs()
            ),
            VendorModelsError::Transport(detail) => write!(f, "{detail}"),
        }
    }
}

/// Live `GET {models_url}` with `Authorization: Bearer <api_key>` —
/// confirmed live against Sarvam (2026-08-01) to accept Bearer alongside
/// its native `api-subscription-key` header, and the same auth style
/// every goose declarative provider here already uses
/// (`AuthMethod::BearerToken` in `goose-providers`). Never falls back to
/// a stored or guessed model list on any failure path — an empty/failed
/// discovery is returned as a typed [`VendorModelsError`], not silently
/// swallowed.
pub async fn fetch_vendor_models(
    models_url: &str,
    api_key: &str,
) -> Result<Vec<DiscoveredModel>, VendorModelsError> {
    let client = reqwest::Client::builder()
        .timeout(MODEL_DISCOVERY_TIMEOUT)
        .build()
        .map_err(|e| VendorModelsError::Transport(format!("failed to build HTTP client: {e}")))?;

    let response = client
        .get(models_url)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                VendorModelsError::Timeout
            } else {
                VendorModelsError::Transport(format!("request to {models_url} failed: {e}"))
            }
        })?;

    let status = response.status();
    if status.is_success() {
        let body: Value = response.json().await.map_err(|e| {
            VendorModelsError::Transport(format!(
                "vendor responded {status} but the body did not parse as JSON: {e}"
            ))
        })?;
        return Ok(parse_discovered_models(&body));
    }

    let message = response.text().await.unwrap_or_default();
    match status.as_u16() {
        401 | 403 => Err(VendorModelsError::Unauthorized {
            status: status.as_u16(),
            message,
        }),
        404 => Err(VendorModelsError::NotFound {
            status: status.as_u16(),
            message,
        }),
        other => Err(VendorModelsError::UnexpectedStatus {
            status: other,
            message,
        }),
    }
}

/// Parse an OpenAI-compatible models-listing body. Accepts `{"data": [...]}`
/// (the standard shape, and what Sarvam returns — verified live
/// 2026-08-01: `{"object":"list","data":[{"id":"sarvam-105b", ...}]}`,
/// with no context-limit field at all) or a bare array (some vendors,
/// mirroring goose's own `parse_model_ids` tolerance for both). Context
/// limit is read from whichever of a few observed vendor key names is
/// present; absent on every vendor probed live so far, so this stays a
/// tolerant best-effort read, never a required field. Order is preserved
/// exactly as the vendor returned it — this must never sort or reorder,
/// unlike goose's own internal `fetch_supported_models` (which sorts for
/// its own UI and is not a contract this function inherits).
fn parse_discovered_models(json: &Value) -> Vec<DiscoveredModel> {
    let Some(arr) = json
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| json.as_array())
    else {
        return Vec::new();
    };

    arr.iter()
        .filter_map(|m| {
            let id = m.get("id").and_then(Value::as_str)?.to_string();
            let context_limit = [
                "context_length",
                "context_window",
                "max_context_length",
                "context_limit",
            ]
            .iter()
            .find_map(|key| m.get(key).and_then(Value::as_u64));
            Some(DiscoveredModel { id, context_limit })
        })
        .collect()
}

// ── Boot-time provider pin (the fix for "boot clobbers the saved
//    provider") ─────────────────────────────────────────────────────────

/// What `goose_acp::initialize()` should pin goose's `active_provider` to
/// at boot, decided BEFORE any goose config code runs (see that
/// function's own ordering doc — the provider pin must be the first
/// config write). Two variants only: either goose's own hardcoded default
/// (`anthropic`, the same credential `api_key`/`oauth_profile` mode
/// already back), or the user's persisted `subscription_cli` choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootProviderPin {
    /// No `subscription_cli` config survived the checks below — pin
    /// goose's hardcoded default (`goose_acp::PINNED_PROVIDER` /
    /// `PINNED_MODEL`). Covers: nothing persisted, `api_key`/
    /// `oauth_profile`/`workload_identity` persisted (all backed by the
    /// same `ANTHROPIC_API_KEY`/`ANTHROPIC_AUTH_TOKEN` goose's `anthropic`
    /// provider already reads), and a persisted `subscription_cli` whose
    /// CLI shim this boot can no longer detect (repinning to a
    /// non-existent command would trade one broken pin for another).
    Default,
    /// A persisted `subscription_cli` config, with the CLI shim detected
    /// present on this boot — pin goose to `claude-code` at `cli_path`,
    /// exactly what `PUT /api/ai/provider` does when the user connects it
    /// live (see `repin_goose_to_claude_code`), so a restart cannot undo
    /// that choice.
    ClaudeCode { cli_path: PathBuf },
    /// A persisted `api_key` config for a vendor goose reaches through a
    /// DECLARATIVE provider (sarvam, xai, mistral, glm, kimi) — repin to
    /// it, exactly as `PUT /api/ai/provider` does live.
    ///
    /// This variant exists because its absence was a silent regression.
    /// `Default` was correct while `api_key` could only mean Anthropic,
    /// whose key goose's built-in provider reads on its own. Once other
    /// vendors gained `api_key` modes, every one of them fell through to
    /// `Default` — so a restart repinned goose to `anthropic`, which has
    /// no credential, and every turn failed with "Provider not set"
    /// (observed 2026-08-01 with `sarvam` persisted). The user's choice
    /// survived in the state file and was ignored on the way back up,
    /// which is the same defect `ClaudeCode` was added to fix, arriving
    /// again through the vendor family added later.
    Declarative {
        /// Roshera's allowlist id, not goose's provider name — the
        /// translation (e.g. `glm` → `zhipu`) belongs to
        /// `goose_declarative_provider_for`, not to callers.
        roshera_provider_id: String,
        api_key: String,
        /// `None` means "no explicit choice was persisted"; the repin then
        /// falls back to that vendor's live-verified default, and refuses
        /// if it has none rather than pinning an empty model.
        model: Option<String>,
    },
}

/// Pure decision logic — no env/filesystem access, `claude_cli` is
/// supplied by the caller — so the "which mode wins" rule is
/// unit-testable without the process-global `goose::config::Config`
/// `OnceCell` this whole module is otherwise careful never to touch from
/// more than one test (see `goose_acp`'s test module doc). Mirrors
/// [`build_chain`]'s pattern: decision logic stays pure and thoroughly
/// tested; the live wrapper ([`boot_provider_pin_for`]) is the thin,
/// untested-by-necessity glue that supplies real detection.
pub fn resolve_boot_provider_pin(
    stored: Option<&StoredProviderConfig>,
    claude_cli: &CliStatus,
) -> BootProviderPin {
    let is_subscription_cli = matches!(stored, Some(s) if s.mode == "subscription_cli");
    if is_subscription_cli {
        if let Some(path) = claude_cli.path.as_deref().filter(|_| claude_cli.installed) {
            return BootProviderPin::ClaudeCode {
                cli_path: PathBuf::from(path),
            };
        }
    }

    // A persisted `api_key` config for a vendor goose reaches through a
    // declarative provider must be repinned, or the choice is lost on every
    // restart. `anthropic` deliberately does NOT match here:
    // `goose_declarative_provider_for` returns `None` for it, because
    // goose's built-in provider already reads `ANTHROPIC_API_KEY` without a
    // repin — so it still resolves to `Default`, which is correct for it and
    // was correct for everything back when it was the only vendor.
    //
    // A stored config naming such a vendor with NO usable key falls through
    // to `Default` as well: repinning to a provider with no credential would
    // trade "Provider not set" for a failure one layer deeper.
    if let Some(s) = stored {
        if s.mode == "api_key" && goose_declarative_provider_for(&s.provider).is_some() {
            if let Some(key) = s
                .api_key
                .as_deref()
                .map(str::trim)
                .filter(|k| !k.is_empty())
            {
                return BootProviderPin::Declarative {
                    roshera_provider_id: s.provider.clone(),
                    api_key: key.to_string(),
                    model: s
                        .model
                        .as_deref()
                        .map(str::trim)
                        .filter(|m| !m.is_empty())
                        .map(str::to_string),
                };
            }
        }
    }

    BootProviderPin::Default
}

/// Live wrapper around [`resolve_boot_provider_pin`]: detects the real
/// `claude` CLI shim on this machine via [`detect_claude_cli`]. Called
/// exactly once, at boot, before `goose_acp::initialize()` touches
/// `goose::config::Config::global()` — see that function's doc for why
/// the ordering matters.
pub fn boot_provider_pin_for(stored: Option<&StoredProviderConfig>) -> BootProviderPin {
    resolve_boot_provider_pin(stored, &detect_claude_cli())
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

    /// Apply a completed background model-verification outcome —
    /// [`verify_subscription_cli_model`]'s only write site. Mirrors
    /// [`Self::save`]'s lock discipline: the blocking file write happens
    /// with no lock held, then a fresh write-lock acquisition installs
    /// the result in memory.
    ///
    /// Stale-result guard: if `stored.model` no longer equals
    /// `model_at_request_time` — a newer PUT changed or cleared the
    /// config while this probe was in flight — the outcome is discarded.
    /// Applying it anyway would let a slow check for an old model
    /// silently overwrite the state of whatever is configured now.
    pub async fn update_model_verification(
        &self,
        model_at_request_time: &str,
        outcome: ModelVerificationState,
    ) -> Result<(), ProviderConfigError> {
        let Some(mut cfg) = self.inner.read().await.stored.clone() else {
            return Ok(());
        };
        if cfg.model.as_deref() != Some(model_at_request_time) {
            return Ok(());
        }
        cfg.model_verification = Some(outcome.clone());
        if outcome == ModelVerificationState::Verified {
            cfg.model_verified = Some(true);
        }
        write_state_file(&self.state_path, &cfg)?;
        self.inner.write().await.stored = Some(cfg);
        Ok(())
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

/// Serializes every test in this binary that mutates the process-global
/// `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` environment variables
/// (this module's child-env scrub proof and `goose_acp`'s lockdown
/// test, which pins the boot-path scrub call site). Without it, one
/// test's sentinel `set_var` can land between another's scrub and its
/// absence assertion under the default parallel runner. Test-only —
/// production code never takes this lock.
#[cfg(test)]
pub(crate) fn anthropic_env_test_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
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
            model: None,
            model_verified: None,
            model_verification: None,
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

    // --- the API-key scrub, proven on a real child's environment ---

    /// THE billing-hazard pin, asserted on the child environment
    /// actually constructed — not on a comment. goose's
    /// `claude_code.rs::build_stream_json_command` (verified at the
    /// pinned rev `022c17c`, lines 332–336) removes ONLY `CLAUDECODE`
    /// before spawning the Claude CLI; the child otherwise inherits
    /// this process's environment verbatim (tokio's `Command` spawns
    /// with inherited env exactly like `std::process::Command` does
    /// here). So the ONE thing standing between a stale
    /// `ANTHROPIC_API_KEY` and a CLI that silently bills the API
    /// instead of the user's Max subscription is
    /// [`scrub_anthropic_env_for_subscription_mode`] having removed
    /// both vars from THIS process before the spawn.
    ///
    /// This test sets sentinel values for both vars, runs the scrub,
    /// then spawns a real child process the same inherit-everything way
    /// goose does and reads the child's own environment dump. If the
    /// scrub is removed or loses a variable, the sentinel reaches the
    /// child and this fails. A control variable proves the dump is
    /// genuinely the child's environment and not an empty read.
    ///
    /// The boot-path CALL SITE of the scrub (`goose_acp::initialize`'s
    /// `ClaudeCode` branch) is pinned separately by
    /// `goose_acp::tests::goose_lockdown_leaves_exactly_roshera_reachable`,
    /// under the same [`anthropic_env_test_lock`].
    #[test]
    fn anthropic_credentials_scrubbed_from_the_actual_child_environment() {
        let _guard = anthropic_env_test_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-SENTINEL-must-not-reach-child");
        std::env::set_var("ANTHROPIC_AUTH_TOKEN", "tok-SENTINEL-must-not-reach-child");
        std::env::set_var("ROSHERA_SCRUB_TEST_CONTROL", "control-marker-present");

        scrub_anthropic_env_for_subscription_mode();

        #[cfg(windows)]
        let output = std::process::Command::new("cmd")
            .args(["/C", "set"])
            .output()
            .expect("spawning `cmd /C set` to dump the child environment must succeed");
        #[cfg(not(windows))]
        let output = std::process::Command::new("sh")
            .args(["-c", "env"])
            .output()
            .expect("spawning `sh -c env` to dump the child environment must succeed");

        std::env::remove_var("ROSHERA_SCRUB_TEST_CONTROL");

        let child_env = String::from_utf8_lossy(&output.stdout);
        assert!(
            child_env.contains("ROSHERA_SCRUB_TEST_CONTROL"),
            "the control variable must appear in the child's environment dump — \
             its absence means the dump did not actually capture the child env, \
             and the two assertions below would be vacuous"
        );
        assert!(
            !child_env.contains("SENTINEL-must-not-reach-child"),
            "a scrubbed Anthropic credential reached a spawned child's \
             environment — goose's claude-code spawn path inherits exactly \
             this environment and would silently bill the API key instead of \
             the user's Max/Pro subscription. Child env dump:\n{child_env}"
        );
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

    /// `model: None` (no user override — "the provider's own choice")
    /// must never serialize a field at all: a stale `null`/absent key on
    /// disk must not be misread as "the user explicitly asked for
    /// nothing", and a real override must round-trip verbatim alongside
    /// its verification flag.
    #[test]
    fn model_override_round_trips_and_is_absent_when_unset() {
        let dir = std::env::temp_dir().join(format!(
            "roshera-ai-provider-model-test-{}",
            uuid::Uuid::new_v4()
        ));
        let path = dir.join("ai-provider.json");

        let mut cfg = stored("api_key", Some("sk-ant-model-test"));
        write_state_file(&path, &cfg).expect("write must succeed");
        let raw = std::fs::read_to_string(&path).expect("file must exist");
        assert!(
            !raw.contains("\"model\""),
            "an unset model override must not serialize a field: {raw}"
        );

        cfg.model = Some("claude-opus-4".to_string());
        cfg.model_verified = Some(true);
        write_state_file(&path, &cfg).expect("write must succeed");
        let loaded = load_stored(&path).expect("must load what was just written");
        assert_eq!(loaded.model.as_deref(), Some("claude-opus-4"));
        assert_eq!(loaded.model_verified, Some(true));

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
            Some(CLAUDE_CODE_EXE_RELATIVE),
        );
        assert!(!status.installed);
        assert!(status.path.is_none());
        assert!(status.shim_path.is_none());
        assert!(status.resolution_note.is_none());
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

    // --- resolve_boot_provider_pin: the boot-clobbers-the-saved-provider fix ---
    //
    // RED before this fix existed: `goose_acp::initialize()` unconditionally
    // pinned `PINNED_PROVIDER`/`PINNED_MODEL` regardless of what was
    // persisted, so a Claude Max subscription connected through the dialog
    // (`subscription_cli`, repinned live via `repin_goose_to_claude_code`)
    // reverted to `anthropic` on the next restart — proven live: a
    // `session/prompt` returned `"Provider not set"` after a boot. These
    // tests pin the decision `initialize()` now acts on.

    fn cli_status(installed: bool, path: Option<&str>) -> CliStatus {
        CliStatus {
            installed,
            path: path.map(str::to_string),
            shim_path: None,
            resolution_note: None,
            signed_in: true,
        }
    }

    #[test]
    fn nothing_persisted_pins_the_hardcoded_default() {
        let cli = cli_status(
            true,
            Some("C:\\Users\\x\\AppData\\Roaming\\npm\\claude.cmd"),
        );
        assert_eq!(
            resolve_boot_provider_pin(None, &cli),
            BootProviderPin::Default
        );
    }

    /// The regression Varun hit: a persisted declarative-vendor choice was
    /// discarded on every restart.
    ///
    /// `sarvam`/`api_key` fell through to `Default`, which repins goose to
    /// `anthropic` — a provider holding no credential — so the first turn
    /// after any restart failed with "Provider not set" while the user's
    /// actual choice sat unread in `state/ai-provider.json`. Without the
    /// `Declarative` arm this asserts `Default` and fails.
    #[test]
    fn a_persisted_declarative_vendor_is_repinned_at_boot_not_dropped_to_anthropic() {
        let cfg = StoredProviderConfig {
            provider: "sarvam".to_string(),
            mode: "api_key".to_string(),
            api_key: Some("sk_live_key".to_string()),
            model: Some("sarvam-105b".to_string()),
            ..stored("api_key", None)
        };
        assert_eq!(
            resolve_boot_provider_pin(Some(&cfg), &cli_status(false, None)),
            BootProviderPin::Declarative {
                roshera_provider_id: "sarvam".to_string(),
                api_key: "sk_live_key".to_string(),
                model: Some("sarvam-105b".to_string()),
            },
            "a persisted declarative vendor must be repinned at boot — dropping to \
             Default silently reverts the user's choice to a provider with no key"
        );
    }

    /// Anthropic must NOT take the new arm. Its `api_key` mode is served by
    /// goose's own built-in provider reading `ANTHROPIC_API_KEY`, needing no
    /// repin — `goose_declarative_provider_for("anthropic")` is `None`, and
    /// that is what keeps this correct rather than an ordering accident.
    #[test]
    fn anthropic_api_key_still_resolves_to_default_not_the_declarative_arm() {
        let cfg = StoredProviderConfig {
            provider: "anthropic".to_string(),
            mode: "api_key".to_string(),
            api_key: Some("sk-ant-real".to_string()),
            ..stored("api_key", None)
        };
        assert_eq!(
            resolve_boot_provider_pin(Some(&cfg), &cli_status(false, None)),
            BootProviderPin::Default
        );
    }

    /// A declarative vendor with no usable key stays on `Default`. Repinning
    /// to a provider that holds no credential would trade "Provider not set"
    /// for the same failure one layer deeper, and lose the honest signal.
    #[test]
    fn a_declarative_vendor_without_a_key_is_not_repinned() {
        for key in [None, Some(String::new()), Some("   ".to_string())] {
            let cfg = StoredProviderConfig {
                provider: "sarvam".to_string(),
                mode: "api_key".to_string(),
                api_key: key.clone(),
                ..stored("api_key", None)
            };
            assert_eq!(
                resolve_boot_provider_pin(Some(&cfg), &cli_status(false, None)),
                BootProviderPin::Default,
                "no usable key ({key:?}) must not produce a repin"
            );
        }
    }

    #[test]
    fn api_key_mode_pins_the_hardcoded_default_not_claude_code() {
        // api_key/oauth_profile/workload_identity are all backed by the
        // same ANTHROPIC_API_KEY/ANTHROPIC_AUTH_TOKEN goose's own
        // `anthropic` provider already reads — only subscription_cli needs
        // the claude-code repin.
        let cfg = stored("api_key", Some("sk-ant-real"));
        let cli = cli_status(true, Some("C:\\claude.cmd"));
        assert_eq!(
            resolve_boot_provider_pin(Some(&cfg), &cli),
            BootProviderPin::Default
        );
    }

    #[test]
    fn oauth_profile_mode_pins_the_hardcoded_default() {
        let cfg = stored("oauth_profile", None);
        let cli = cli_status(true, Some("C:\\claude.cmd"));
        assert_eq!(
            resolve_boot_provider_pin(Some(&cfg), &cli),
            BootProviderPin::Default
        );
    }

    #[test]
    fn persisted_subscription_cli_with_detected_cli_pins_claude_code() {
        // THE proving case: a persisted subscription_cli config must
        // survive a restart — never silently clobbered back to `anthropic`.
        let cfg = stored("subscription_cli", None);
        // `path` is always the resolved real executable, never the `.cmd`
        // shim — see `CliStatus`'s doc and `detect_claude_cli`.
        let cli = cli_status(
            true,
            Some(
                "C:\\Users\\x\\AppData\\Roaming\\npm\\node_modules\\@anthropic-ai\\claude-code\\bin\\claude.exe",
            ),
        );
        assert_eq!(
            resolve_boot_provider_pin(Some(&cfg), &cli),
            BootProviderPin::ClaudeCode {
                cli_path: PathBuf::from(
                    "C:\\Users\\x\\AppData\\Roaming\\npm\\node_modules\\@anthropic-ai\\claude-code\\bin\\claude.exe",
                ),
            }
        );
    }

    #[test]
    fn persisted_subscription_cli_with_cli_no_longer_installed_falls_back_to_default() {
        // The shim this deployment relied on vanished since the config was
        // saved (uninstalled, moved machines, ...) — repinning to a
        // command that doesn't exist would trade one broken pin for
        // another, so this falls back rather than guessing a path.
        let cfg = stored("subscription_cli", None);
        let cli = cli_status(false, None);
        assert_eq!(
            resolve_boot_provider_pin(Some(&cfg), &cli),
            BootProviderPin::Default
        );
    }

    #[test]
    fn persisted_subscription_cli_with_no_detected_path_falls_back_to_default() {
        // Defensive: `installed: true` with `path: None` should not be
        // reachable from `detect_cli_at`, but the decision must not panic
        // or fabricate a path if it ever happens.
        let cfg = stored("subscription_cli", None);
        let cli = cli_status(true, None);
        assert_eq!(
            resolve_boot_provider_pin(Some(&cfg), &cli),
            BootProviderPin::Default
        );
    }

    // --- resolve_cli_exe / parse_cmd_shim_for_direct_exe: the BLOCKER fix ---
    //
    // RED before this fix: `detect_cli_at` reported the `.cmd` shim itself
    // as `path`, and `repin_goose_to_claude_code` wrote that straight into
    // `CLAUDE_CODE_COMMAND` — goose spawns it via `CreateProcess`, which
    // cannot execute a `.cmd`, so every agent turn failed with "Failed to
    // spawn Claude CLI command". These tests pin the resolution logic that
    // replaces it.

    /// A fresh temp dir standing in for an npm prefix (`%APPDATA%\npm`),
    /// auto-removed on drop so parallel test runs never collide.
    struct TempNpmPrefix(PathBuf);

    impl TempNpmPrefix {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "roshera-cli-resolve-{tag}-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&dir).expect("temp npm prefix must create");
            Self(dir)
        }
    }

    impl Drop for TempNpmPrefix {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Write a file (and its parent dirs) under `prefix`, relative segments
    /// joined the same way `resolve_cli_exe` joins `known_relative_exe`.
    fn write_under(prefix: &Path, segments: &[&str], contents: &str) -> PathBuf {
        let mut p = prefix.to_path_buf();
        for s in segments {
            p.push(s);
        }
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("parent dirs must create");
        }
        std::fs::write(&p, contents).expect("file must write");
        p
    }

    /// The exact verified Claude Code shim body: a single unconditional
    /// quoted `.exe` invocation.
    const CLAUDE_CMD_BODY: &str = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\"%dp0%\\node_modules\\@anthropic-ai\\claude-code\\bin\\claude.exe\"   %*\r\n";

    /// The exact verified Codex shim body: an `IF EXIST` node-dispatch,
    /// never a single unconditional `.exe` target.
    const CODEX_CMD_BODY: &str = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n  SET PATHEXT=%PATHEXT:;.JS;=;%\r\n)\r\n\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\@openai\\codex\\bin\\codex.js\" %*\r\n";

    #[test]
    fn resolve_cli_exe_prefers_the_known_relative_layout_when_present() {
        let prefix = TempNpmPrefix::new("known-layout");
        let shim = write_under(&prefix.0, &["claude.cmd"], CLAUDE_CMD_BODY);
        let real_exe = write_under(&prefix.0, CLAUDE_CODE_EXE_RELATIVE, "fake-native-binary");

        let resolution = resolve_cli_exe(&prefix.0, &shim, Some(CLAUDE_CODE_EXE_RELATIVE));
        assert_eq!(resolution.exe_path, Some(real_exe));
        assert!(resolution.unresolved_reason.is_none());
    }

    #[test]
    fn parse_cmd_shim_for_direct_exe_resolves_an_unconditional_target() {
        let prefix = TempNpmPrefix::new("cmd-parse");
        let shim = write_under(&prefix.0, &["claude.cmd"], CLAUDE_CMD_BODY);
        let real_exe = write_under(&prefix.0, CLAUDE_CODE_EXE_RELATIVE, "fake-native-binary");

        // No known-relative hint — must fall back to parsing the shim body
        // itself and still land on the real executable.
        let resolution = resolve_cli_exe(&prefix.0, &shim, None);
        assert_eq!(resolution.exe_path, Some(real_exe));
    }

    #[test]
    fn parse_cmd_shim_for_direct_exe_skips_a_conditional_node_dispatch_shim() {
        let prefix = TempNpmPrefix::new("node-dispatch");
        let shim = write_under(&prefix.0, &["codex.cmd"], CODEX_CMD_BODY);
        // Even with a real node.exe present at the IF-EXIST-checked path,
        // the parser must not treat a line inside a conditional as an
        // unconditional invocation.
        write_under(&prefix.0, &["node.exe"], "fake-node-binary");

        assert_eq!(parse_cmd_shim_for_direct_exe(&shim), None);
    }

    #[test]
    fn resolve_cli_exe_reports_an_honest_reason_when_nothing_resolves() {
        let prefix = TempNpmPrefix::new("unresolved");
        let shim = write_under(&prefix.0, &["codex.cmd"], CODEX_CMD_BODY);

        // No known-relative layout for Codex (see `detect_codex_cli`'s
        // doc) — this must never guess a platform-specific binary name.
        let resolution = resolve_cli_exe(&prefix.0, &shim, None);
        assert!(resolution.exe_path.is_none());
        let reason = resolution
            .unresolved_reason
            .expect("a shim that resolves to nothing must explain why");
        assert!(
            reason.contains("CreateProcess"),
            "the refusal must name the actual mechanism, not just say 'not found': {reason}"
        );
    }

    #[test]
    fn detect_cli_at_reports_unresolved_reason_on_the_public_cli_status_when_shim_unresolvable() {
        // End-to-end through the public `detect_cli_at` entry point (not
        // just the private `resolve_cli_exe` helper): a present shim that
        // resolves to nothing must surface `installed: true`, `path: None`,
        // and a populated `resolution_note` — never a silently-empty status
        // that looks identical to "not installed at all". `detect_cli_at`
        // joins `appdata.join("npm")` itself, so the shim must live under
        // an `npm` subdirectory of whatever "appdata" we hand in.
        let appdata_dir = std::env::temp_dir().join(format!(
            "roshera-cli-resolve-detect-cli-at-appdata-{}",
            uuid::Uuid::new_v4()
        ));
        let npm_dir = appdata_dir.join("npm");
        write_under(&npm_dir, &["codex.cmd"], CODEX_CMD_BODY);

        let status = detect_cli_at(
            appdata_dir.to_str(),
            Some("C:\\definitely\\does\\not\\exist"),
            "codex.cmd",
            &[".codex", "auth.json"],
            None,
        );
        assert!(status.installed);
        assert!(status.path.is_none());
        assert!(status.shim_path.is_some());
        assert!(
            status.resolution_note.is_some(),
            "a present-but-unresolvable shim must explain itself, not just \
             report a missing path with no context"
        );

        let _ = std::fs::remove_dir_all(&appdata_dir);
    }

    // --- repin_goose_to_claude_code: the structural write-site guard ---
    //
    // Both cases return before touching `goose::config::Config::global()`
    // (the extension check is the very first thing the function does), so
    // these are safe to run alongside every other test in this binary —
    // they never become a second owner of that process-global `OnceCell`
    // (see `goose_acp`'s own test-module doc on why that singleton
    // ownership matters).

    #[test]
    fn repin_goose_to_claude_code_refuses_a_cmd_shim_path() {
        let err = repin_goose_to_claude_code(Path::new("C:\\fake\\npm\\claude.cmd"))
            .expect_err("a .cmd path must be refused, never written to CLAUDE_CODE_COMMAND");
        assert!(
            err.contains("CreateProcess"),
            "refusal must name why a .cmd can't be spawned: {err}"
        );
        assert!(
            err.contains("claude.cmd"),
            "refusal must name what it found: {err}"
        );
    }

    #[test]
    fn repin_goose_to_claude_code_refuses_a_ps1_shim_path() {
        let err = repin_goose_to_claude_code(Path::new("C:\\fake\\npm\\claude.ps1"))
            .expect_err("a .ps1 path must be refused the same way as .cmd");
        assert!(err.contains("CreateProcess"));
    }

    // --- live, this-machine proof: the hand-patch becomes redundant ---

    #[test]
    fn detect_claude_cli_on_this_machine_resolves_the_real_exe_never_the_cmd_shim() {
        // Live (not mocked): proves a FRESH `detect_claude_cli()` call on
        // this dev machine produces the real, directly-spawnable
        // `claude.exe` — the exact value Varun's hand-patch of
        // `state/goose-root/config/config.yaml` was standing in for. If
        // this passes, the hand-patch is redundant, not load-bearing: the
        // next boot (or PUT /api/ai/provider) reproduces it on its own.
        // Skips (rather than fails) when the CLI isn't installed on
        // whatever machine runs this suite — CI or a fresh checkout.
        let status = detect_claude_cli();
        if !status.installed {
            return;
        }
        let path = status
            .path
            .as_deref()
            .expect("installed Claude Code CLI must resolve a real executable on this machine");
        assert!(
            path.ends_with("claude-code\\bin\\claude.exe")
                || path.ends_with("claude-code/bin/claude.exe"),
            "resolved path must be the real native binary, got: {path}"
        );
        assert!(
            !path.to_ascii_lowercase().ends_with(".cmd"),
            "must never resolve to the .cmd shim: {path}"
        );
    }

    // --- goose_declarative_provider_for / repin_goose_to_declarative_provider:
    //     the generalized-repin mapping. Only the pure mapping and the
    //     before-any-Config::global()-touch refusal path are tested here —
    //     same discipline as `repin_goose_to_claude_code`'s tests, which
    //     never become a second owner of goose's process-global
    //     `Config` `OnceCell` (see `goose_acp`'s test-module doc).

    #[test]
    fn goose_declarative_provider_for_maps_all_four_new_vendors() {
        assert_eq!(
            goose_declarative_provider_for("xai"),
            Some(("xai", "XAI_API_KEY", None))
        );
        assert_eq!(
            goose_declarative_provider_for("mistral"),
            Some(("mistral", "MISTRAL_API_KEY", None))
        );
        assert_eq!(
            goose_declarative_provider_for("glm"),
            Some(("zhipu", "ZHIPU_API_KEY", None)),
            "Roshera's 'glm' id must map to goose's own 'zhipu' provider name"
        );
        assert_eq!(
            goose_declarative_provider_for("kimi"),
            Some(("moonshot", "MOONSHOT_API_KEY", None)),
            "Roshera's 'kimi' id must map to goose's own 'moonshot' provider name"
        );
    }

    /// A default model may only be present where the identifier was checked
    /// against the vendor's live API. This is the ratchet: adding one from
    /// documentation or memory is how `sarvam-30b` — a model that vendor
    /// does not serve — sat in this repo's config until a live check
    /// removed it.
    #[test]
    fn only_live_verified_vendors_carry_a_default_model() {
        for id in ["xai", "mistral", "glm", "kimi"] {
            let (_, _, default_model) = goose_declarative_provider_for(id)
                .unwrap_or_else(|| panic!("{id} must still map to a goose provider"));
            assert_eq!(
                default_model, None,
                "{id} has no live-verified default model, so it must carry None — a guessed \
                 identifier fails at first use while the dialog shows it connected"
            );
        }

        let (_, _, sarvam_default) =
            goose_declarative_provider_for("sarvam").expect("sarvam must map");
        assert_eq!(
            sarvam_default,
            Some("sarvam-105b"),
            "sarvam-105b is the only chat model GET https://api.sarvam.ai/v1/models served \
             on 2026-08-01 — verified live, which is the bar for being a default here"
        );
    }

    /// The defect Varun hit connecting Sarvam: the repin accepted an empty
    /// model via `unwrap_or_default()`, pinned the provider anyway, and the
    /// turn later died on "Configuration value not found: GOOSE_MODEL".
    /// A provider with no resolvable model must be refused HERE, before any
    /// goose config is written, and the refusal must say what to supply.
    #[test]
    fn repin_refuses_a_provider_with_no_model_and_no_verified_default() {
        for blank in [None, Some(""), Some("   ")] {
            let err = repin_goose_to_declarative_provider("xai", "fake-key", blank).expect_err(
                "a provider with no model and no verified default must be refused, not \
                 pinned with an empty model",
            );
            assert!(
                err.contains("model"),
                "the refusal must name what is missing: {err}"
            );
        }
    }

    #[test]
    fn goose_declarative_provider_for_maps_sarvam_by_identity() {
        assert_eq!(
            goose_declarative_provider_for("sarvam"),
            Some(("sarvam", "SARVAM_API_KEY", Some("sarvam-105b"))),
            "Roshera's 'sarvam' id must map to goose's custom-provider name \
             'sarvam' (identity mapping — there is no goose-bundled provider \
             to rename to, unlike glm -> zhipu)"
        );
    }

    #[test]
    fn goose_declarative_provider_for_has_no_target_for_anthropic_or_baseten() {
        assert_eq!(
            goose_declarative_provider_for("anthropic"),
            None,
            "anthropic needs no repin — goose's hardcoded default already reads \
             ANTHROPIC_API_KEY/ANTHROPIC_AUTH_TOKEN"
        );
        assert_eq!(
            goose_declarative_provider_for("baseten"),
            None,
            "baseten has no goose provider to repin to"
        );
    }

    #[test]
    fn repin_goose_to_declarative_provider_refuses_an_unmapped_provider_by_name() {
        let err = repin_goose_to_declarative_provider("baseten", "fake-key", None)
            .expect_err("a provider with no goose repin target must be refused");
        assert!(
            err.contains("baseten"),
            "refusal must name the provider it can't repin: {err}"
        );
    }

    // --- subscription_cli model verification: parsing, timeout, and the
    //     end-to-end injectable-probe path — none of these spawn the real
    //     ~250 MB claude.exe. ---

    #[test]
    fn parse_cli_output_accepts_is_error_false_as_verified() {
        let stdout = br#"{"is_error":false,"stop_reason":"end_turn"}"#;
        assert_eq!(
            parse_cli_output(stdout, b"", Some(0)),
            ProbeOutcome::Verified
        );
    }

    #[test]
    fn parse_cli_output_names_the_reason_on_is_error_true() {
        // The exact shape observed live for `claude --model
        // this-model-does-not-exist-xyz -p ... --output-format json`.
        let stdout = br#"{"is_error":true,"api_error_status":404,"result":"There's an issue with the selected model (this-model-does-not-exist-xyz). It may not exist or you may not have access to it. Run --model to pick a different model.","type":"result"}"#;
        match parse_cli_output(stdout, b"", Some(1)) {
            ProbeOutcome::Rejected(reason) => {
                assert!(
                    reason.contains("issue with the selected model"),
                    "rejection reason must carry the CLI's own explanation, got: {reason}"
                );
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn parse_cli_output_empty_stdout_is_unknown_not_rejected() {
        match parse_cli_output(b"", b"some crash trace", Some(1)) {
            ProbeOutcome::Unknown(reason) => {
                assert!(reason.contains("no stdout"), "got: {reason}");
            }
            other => panic!("empty stdout must be Unknown, not {other:?}"),
        }
    }

    #[test]
    fn parse_cli_output_unparsable_json_is_unknown() {
        match parse_cli_output(b"not json at all", b"", Some(0)) {
            ProbeOutcome::Unknown(_) => {}
            other => panic!("garbage stdout must be Unknown, not {other:?}"),
        }
    }

    #[test]
    fn parse_cli_output_missing_is_error_field_is_unknown() {
        match parse_cli_output(br#"{"type":"result"}"#, b"", Some(0)) {
            ProbeOutcome::Unknown(_) => {}
            other => panic!("a shape with no is_error field must be Unknown, not {other:?}"),
        }
    }

    /// RED before the timeout wrapper existed: a probe future that never
    /// resolves must produce `Unknown`, never hang the caller forever and
    /// never be silently read as a pass. `std::future::pending()` stands
    /// in for a hung CLI process without needing to actually wait out a
    /// real 45s timeout.
    #[tokio::test]
    async fn run_probe_with_timeout_produces_unknown_never_verified_or_rejected() {
        let outcome = run_probe_with_timeout(
            std::future::pending::<ProbeOutcome>(),
            Duration::from_millis(20),
        )
        .await;
        match outcome {
            ProbeOutcome::Unknown(reason) => {
                assert!(reason.contains("timed out"), "got: {reason}");
            }
            other => panic!("a hung probe must surface as Unknown, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_probe_with_timeout_passes_through_a_probe_that_finishes_in_time() {
        let outcome =
            run_probe_with_timeout(async { ProbeOutcome::Verified }, Duration::from_secs(5)).await;
        assert_eq!(outcome, ProbeOutcome::Verified);
    }

    /// Build a fake [`ModelProbe`] that returns a fixed outcome — the
    /// injection seam that lets the rest of these tests avoid spawning
    /// the real CLI binary entirely.
    fn fake_probe(outcome: ProbeOutcome) -> ModelProbe {
        Arc::new(move |_cli_path, _model| {
            let outcome = outcome.clone();
            Box::pin(async move { outcome })
        })
    }

    #[tokio::test]
    async fn verify_subscription_cli_model_persists_rejected_naming_the_model() {
        let dir = std::env::temp_dir().join(format!(
            "roshera-model-verify-rejected-{}",
            uuid::Uuid::new_v4()
        ));
        let mgr = Arc::new(AiProviderManager::boot_at(dir.join("ai-provider.json")));
        let mut cfg = stored("subscription_cli", None);
        cfg.model = Some("bogus-model-xyz".to_string());
        cfg.model_verified = Some(false);
        cfg.model_verification = Some(ModelVerificationState::Pending);
        mgr.save(cfg).await.expect("save must succeed");

        let probe = fake_probe(ProbeOutcome::Rejected(
            "There's an issue with the selected model (bogus-model-xyz).".to_string(),
        ));
        verify_subscription_cli_model(
            probe,
            PathBuf::from("C:\\fake\\claude.exe"),
            "bogus-model-xyz".to_string(),
            mgr.clone(),
        )
        .await;

        let stored_cfg = mgr.stored().await.expect("config must still be present");
        match stored_cfg.model_verification {
            Some(ModelVerificationState::Rejected { model, reason }) => {
                assert_eq!(model, "bogus-model-xyz");
                assert!(reason.contains("issue with the selected model"));
            }
            other => panic!("expected Rejected naming the model, got {other:?}"),
        }
        assert_eq!(
            stored_cfg.model_verified,
            Some(false),
            "a rejection must never flip model_verified to true"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn verify_subscription_cli_model_persists_unknown_on_timeout_never_verified_or_rejected()
    {
        let dir = std::env::temp_dir().join(format!(
            "roshera-model-verify-unknown-{}",
            uuid::Uuid::new_v4()
        ));
        let mgr = Arc::new(AiProviderManager::boot_at(dir.join("ai-provider.json")));
        let mut cfg = stored("subscription_cli", None);
        cfg.model = Some("opus".to_string());
        cfg.model_verified = Some(false);
        cfg.model_verification = Some(ModelVerificationState::Pending);
        mgr.save(cfg).await.expect("save must succeed");

        // A probe wired straight to the real timeout wrapper with a probe
        // future that never resolves — proves the whole pipeline
        // (probe -> verify_subscription_cli_model -> persisted state)
        // produces Unknown on a timeout, not just the isolated wrapper.
        let probe: ModelProbe = Arc::new(|_cli_path, _model| {
            Box::pin(run_probe_with_timeout(
                std::future::pending::<ProbeOutcome>(),
                Duration::from_millis(20),
            ))
        });
        verify_subscription_cli_model(
            probe,
            PathBuf::from("C:\\fake\\claude.exe"),
            "opus".to_string(),
            mgr.clone(),
        )
        .await;

        let stored_cfg = mgr.stored().await.expect("config must still be present");
        match &stored_cfg.model_verification {
            Some(ModelVerificationState::Unknown { reason }) => {
                assert!(reason.contains("timed out"), "got: {reason}");
            }
            other => panic!("a timed-out probe must persist as Unknown, got {other:?}"),
        }
        assert_ne!(
            stored_cfg.model_verification,
            Some(ModelVerificationState::Verified)
        );
        assert_eq!(
            stored_cfg.model_verified,
            Some(false),
            "unknown must never flip model_verified to true"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn verify_subscription_cli_model_persists_verified_and_flips_model_verified_true() {
        let dir = std::env::temp_dir().join(format!(
            "roshera-model-verify-verified-{}",
            uuid::Uuid::new_v4()
        ));
        let mgr = Arc::new(AiProviderManager::boot_at(dir.join("ai-provider.json")));
        let mut cfg = stored("subscription_cli", None);
        cfg.model = Some("sonnet".to_string());
        cfg.model_verified = Some(false);
        cfg.model_verification = Some(ModelVerificationState::Pending);
        mgr.save(cfg).await.expect("save must succeed");

        verify_subscription_cli_model(
            fake_probe(ProbeOutcome::Verified),
            PathBuf::from("C:\\fake\\claude.exe"),
            "sonnet".to_string(),
            mgr.clone(),
        )
        .await;

        let stored_cfg = mgr.stored().await.expect("config must still be present");
        assert_eq!(
            stored_cfg.model_verification,
            Some(ModelVerificationState::Verified)
        );
        assert_eq!(stored_cfg.model_verified, Some(true));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn update_model_verification_discards_a_stale_result_for_a_superseded_model() {
        // The model changed (a second PUT landed) before the first
        // probe's background check finished — the stale result must be
        // discarded, never overwrite the newer save.
        let dir = std::env::temp_dir().join(format!(
            "roshera-model-verify-stale-{}",
            uuid::Uuid::new_v4()
        ));
        let mgr = AiProviderManager::boot_at(dir.join("ai-provider.json"));
        let mut cfg = stored("subscription_cli", None);
        cfg.model = Some("opus".to_string());
        mgr.save(cfg.clone()).await.expect("save must succeed");

        // Simulate the newer PUT: model changed to "sonnet" before the
        // stale "opus" probe reports back.
        cfg.model = Some("sonnet".to_string());
        cfg.model_verification = Some(ModelVerificationState::Pending);
        mgr.save(cfg).await.expect("second save must succeed");

        mgr.update_model_verification("opus", ModelVerificationState::Verified)
            .await
            .expect("update must not error even when discarded");

        let stored_cfg = mgr.stored().await.expect("config must still be present");
        assert_eq!(stored_cfg.model.as_deref(), Some("sonnet"));
        assert_eq!(
            stored_cfg.model_verification,
            Some(ModelVerificationState::Pending),
            "a stale result for a superseded model must never overwrite the current state"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- Live model discovery: resolution, URL normalization, key-shape
    //     rejection, and response parsing — every piece network-free.
    //     `resolve_base_url_from` is deliberately never exercised with a
    //     bundled fixture that carries `env_vars` here: that path calls
    //     `goose::config::declarative_providers::expand_env_vars`, which
    //     touches the process-lifetime `Config::global()` `OnceCell` —
    //     see `goose_acp`'s own test module doc for why that must stay
    //     off this file's test path. The glm/kimi env-var-expansion path
    //     is exercised live only (see this change's verification report).

    fn fixture_declarative_config(
        name: &str,
        base_url: &str,
    ) -> goose::config::declarative_providers::DeclarativeProviderConfig {
        let json = serde_json::json!({
            "name": name,
            "engine": "openai",
            "display_name": name,
            "api_key_env": format!("{}_API_KEY", name.to_uppercase()),
            "base_url": base_url,
            "models": [],
        })
        .to_string();
        goose::config::declarative_providers::deserialize_provider_config(&json)
            .expect("fixture declarative config must parse")
    }

    #[test]
    fn resolve_base_url_from_prefers_custom_json_over_bundled() {
        let custom = vec![fixture_declarative_config(
            "sarvam",
            "https://api.sarvam.ai/v1",
        )];
        let bundled = vec![fixture_declarative_config(
            "sarvam",
            "https://bundled-must-not-win.example/v1",
        )];
        let (url, source) = resolve_base_url_from("sarvam", &custom, &bundled)
            .expect("sarvam must resolve from the custom tier");
        assert_eq!(url, "https://api.sarvam.ai/v1");
        assert_eq!(source, BaseUrlSource::CustomProviderJson);
    }

    #[test]
    fn resolve_base_url_from_falls_back_to_bundled_when_no_custom_json_matches() {
        let bundled = vec![fixture_declarative_config(
            "mistral",
            "https://api.mistral.ai/v1/chat/completions",
        )];
        let (url, source) = resolve_base_url_from("mistral", &[], &bundled)
            .expect("mistral must resolve from the bundled tier");
        assert_eq!(url, "https://api.mistral.ai/v1/chat/completions");
        assert_eq!(source, BaseUrlSource::BundledDeclarative);
    }

    #[test]
    fn resolve_base_url_from_refuses_when_neither_tier_has_the_name() {
        // xai's real-world shape: allowlisted and Wired for inference,
        // but a hand-written native goose provider with no JSON
        // definition in either tier — this must be a clean `None`, not
        // a panic or a guessed URL.
        assert!(resolve_base_url_from("xai", &[], &[]).is_none());
    }

    #[test]
    fn resolve_provider_base_url_refuses_unmapped_provider_by_name() {
        // anthropic needs no goose repin at all — `goose_declarative_provider_for`
        // returns `None` for it, so this must fail at that first gate,
        // never reach the filesystem.
        let err = resolve_provider_base_url("anthropic")
            .expect_err("anthropic has no goose provider mapping to resolve");
        assert_eq!(err.roshera_provider_id, "anthropic");
        assert!(err.reason.contains("no known goose provider"));
    }

    // --- models_url: table test across every raw base_url shape actually
    //     observed (custom-provider JSON root, bundled chat/completions
    //     endpoints, a bundled root with no completions suffix, and a
    //     trailing slash) ---

    #[test]
    fn models_url_appends_directly_onto_an_already_root_shaped_base_url() {
        assert_eq!(
            models_url("https://api.sarvam.ai/v1"),
            "https://api.sarvam.ai/v1/models"
        );
    }

    #[test]
    fn models_url_trims_a_trailing_slash_before_appending() {
        assert_eq!(
            models_url("https://api.sarvam.ai/v1/"),
            "https://api.sarvam.ai/v1/models"
        );
    }

    #[test]
    fn models_url_strips_a_chat_completions_suffix() {
        assert_eq!(
            models_url("https://api.mistral.ai/v1/chat/completions"),
            "https://api.mistral.ai/v1/models"
        );
        assert_eq!(
            models_url("https://api.moonshot.cn/v1/chat/completions"),
            "https://api.moonshot.cn/v1/models"
        );
    }

    #[test]
    fn models_url_appends_onto_a_bundled_root_with_no_completions_suffix() {
        // zhipu's default base_url (`ZHIPU_BASE_URL`'s default,
        // "https://open.bigmodel.cn/api/paas/v4") has no
        // chat/completions tail to strip.
        assert_eq!(
            models_url("https://open.bigmodel.cn/api/paas/v4"),
            "https://open.bigmodel.cn/api/paas/v4/models"
        );
    }

    // --- reject_implausible_key_shape: the fix for the 649-char Vite
    //     error string that reached state/ai-provider.json as a
    //     "credential" ---

    #[test]
    fn reject_implausible_key_shape_accepts_a_plausible_key() {
        assert!(reject_implausible_key_shape("sk-abc123XYZ-plausible-token").is_ok());
    }

    #[test]
    fn reject_implausible_key_shape_rejects_empty() {
        assert!(reject_implausible_key_shape("").is_err());
        assert!(reject_implausible_key_shape("   ").is_err());
    }

    #[test]
    fn reject_implausible_key_shape_rejects_multiline_input() {
        let err = reject_implausible_key_shape("first line\nsecond line")
            .expect_err("multi-line input must be rejected");
        assert!(err.contains("line break"));
    }

    #[test]
    fn reject_implausible_key_shape_rejects_leading_whitespace() {
        let err = reject_implausible_key_shape(" sk-leading-space")
            .expect_err("leading whitespace must be rejected");
        assert!(err.contains("leading whitespace"));
    }

    #[test]
    fn reject_implausible_key_shape_rejects_bracket_prefixed_input() {
        for bad in ["{\"error\": true}", "[1, 2, 3]", "<html>error</html>"] {
            assert!(
                reject_implausible_key_shape(bad).is_err(),
                "'{bad}' must be rejected as an implausible key"
            );
        }
    }

    #[test]
    fn reject_implausible_key_shape_rejects_the_real_vite_error_that_reached_state_json() {
        // Verbatim from `state/ai-provider.json` (gitignored, gets
        // overwritten by whatever is currently configured — captured
        // 2026-08-01, 577 characters, no literal line breaks): a Vite
        // build error pasted into the API key field and saved as a
        // credential. Caught here by the bracket-prefix rule (starts
        // with `[`) and independently by the length rule (577 > 256) —
        // either alone would have refused it.
        let vite_error = "[plugin:vite:oxc] Transform failed with 1 error:  [PARSE_ERROR] Error: Expected `,` or `)` but found `Identifier`      \u{256d}\u{2500}[ src/components/settings/ProviderSettingsDialog.tsx:527:16 ]      \u{2502}  514 \u{2502}         {data && (      \u{2502}                  \u{252c}        \u{2502}                  \u{2570}\u{2500}\u{2500} Opened here      \u{2502}   527 \u{2502}           <div className=\"flex max-h-[60vh] flex-col gap-3 overflow-y-auto pr-1 pt-2\">      \u{2502}                \u{2500}\u{2500}\u{2500}\u{2500}\u{252c}\u{2500}\u{2500}\u{2500}\u{2500}        \u{2502}                    \u{2570}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500} `,` or `)` expected \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{256f} C:/Users/Varun Sharma/Roshera-CAD/roshera-app/src/components/settings/ProviderSettingsDialog.tsx";
        // 577 Unicode scalar values (JS `.length`, which is what the
        // frontend's own length check counts) — more UTF-8 bytes than
        // that because the box-drawing characters are 3 bytes each, but
        // `reject_implausible_key_shape` only needs `raw.len() > 256` to
        // be true, which byte length already guarantees here.
        assert_eq!(vite_error.chars().count(), 577);
        let err = reject_implausible_key_shape(vite_error)
            .expect_err("the real Vite error string must be rejected");
        assert!(err.contains("bracket character"));
    }

    #[test]
    fn reject_implausible_key_shape_rejects_absurd_length() {
        let too_long = "a".repeat(300);
        let err = reject_implausible_key_shape(&too_long)
            .expect_err("a 300-character token must be rejected");
        assert!(err.contains("300"));
    }

    // --- parse_discovered_models: verbatim vendor output, never sorted,
    //     never merged with a local list ---

    #[test]
    fn parse_discovered_models_reads_the_live_sarvam_shape() {
        // Verified live 2026-08-01: `GET https://api.sarvam.ai/v1/models`
        // (no key required) returned exactly this body — one model, no
        // context-limit field at all, and critically NOT `sarvam-30b`.
        let body: Value = serde_json::from_str(
            r#"{"object":"list","data":[{"id":"sarvam-105b","object":"model","created":0,"owned_by":"sarvam"}]}"#,
        )
        .expect("fixture body must parse");
        let models = parse_discovered_models(&body);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "sarvam-105b");
        assert_eq!(models[0].context_limit, None);
        assert!(
            !models.iter().any(|m| m.id == "sarvam-30b"),
            "the live vendor response never mentions sarvam-30b — discovery must \
             not reintroduce it from anywhere"
        );
    }

    #[test]
    fn parse_discovered_models_accepts_a_bare_array() {
        let body: Value = serde_json::json!([{"id": "model-a"}, {"id": "model-b"}]);
        let models = parse_discovered_models(&body);
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["model-a", "model-b"],
            "order must be preserved exactly as returned, never sorted"
        );
    }

    #[test]
    fn parse_discovered_models_reads_a_context_limit_when_present() {
        let body: Value = serde_json::json!({
            "data": [{"id": "big-model", "context_length": 131072}]
        });
        let models = parse_discovered_models(&body);
        assert_eq!(models[0].context_limit, Some(131072));
    }

    #[test]
    fn parse_discovered_models_never_reorders_vendor_output() {
        let body: Value = serde_json::json!({
            "data": [{"id": "zzz-last"}, {"id": "aaa-first"}]
        });
        let models = parse_discovered_models(&body);
        assert_eq!(
            models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            vec!["zzz-last", "aaa-first"]
        );
    }
}
