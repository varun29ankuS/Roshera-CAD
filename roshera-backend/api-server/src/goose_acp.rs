//! Goose ACP integration — slice 1: embed the goose agent harness with
//! every built-in tool surface provably disabled.
//!
//! The deliverable of this slice is *negative capability*: an `/acp`
//! endpoint through which no goose built-in tool — above all no shell
//! execution and no file editing — is reachable. Wiring Roshera's own
//! MCP tools into a session is slice 2 and deliberately absent here.
//!
//! Threat model, in dependency order:
//!
//! 1. **`goose-mcp` builtin extensions (Developer / ComputerController /
//!    Memory as MCP servers)** — excluded *structurally*: only `goose-mcp`
//!    (via `goose-cli`) ever calls `register_builtin_extensions`, and this
//!    workspace never depends on it, so goose's `BUILTIN_REGISTRY` stays
//!    empty for the process lifetime. Even a hostile config entry
//!    (`type: builtin, name: computercontroller`) fails closed: the
//!    registry lookup returns `None` and activation cannot spawn anything.
//!
//! 2. **In-process platform extensions** — a second, separate "Developer"
//!    (ShellTool + EditTools) is compiled unconditionally into the `goose`
//!    crate itself (`agents/platform_extensions/developer`), registered
//!    `default_enabled: true`, and auto-written into the config file by a
//!    migration that runs on config reads. Omitting `goose-mcp` does NOT
//!    remove it. [`initialize`] disables **every** entry in goose's
//!    `PLATFORM_EXTENSIONS` registry — not a hardcoded list, and not just
//!    the `default_enabled: true` subset — so a dependency bump that adds
//!    or re-classifies an upstream extension is disabled automatically
//!    instead of silently joining our tool surface. It then *verifies*
//!    the enabled set is empty, refusing to boot otherwise.
//!    `extensionmanager` is disabled first — its whole purpose is giving
//!    the model tools to re-enable extensions at runtime, so leaving it
//!    on would demote every other disable from structural to advisory.
//!    Roshera keeps none of them (no `todo`, no memory extension either:
//!    Roshera's memory is the certified, event-sourced timeline, and the
//!    agent's planning surface is the Blackboard) — the target state is
//!    Roshera's own MCP server as the *only* tool provider (slice 2).
//!
//! 3. **Config location** — goose reads its config through
//!    `Config::global()`, a process-lifetime `OnceCell` whose path is
//!    resolved from the `GOOSE_PATH_ROOT` env var at first touch (the
//!    `AcpServerFactoryConfig.config_dir` field is NOT fully honored —
//!    goose's own source says so). [`initialize`] therefore sets
//!    `GOOSE_PATH_ROOT` to a Roshera-owned directory before any goose code
//!    runs, and `main()` calls it before anything can touch a goose type.
//!    Note goose still layers `C:\ProgramData\goose\config.yaml` (admin-
//!    writable only) underneath ours; our file is the write target and
//!    takes precedence, and the post-lockdown verification reads the
//!    *merged* view, so a system-level re-enable is caught at boot.
//!
//! 4. **Provider** — pinned to `anthropic` (same `ANTHROPIC_API_KEY` env
//!    var Roshera's own Claude provider uses) with the model Roshera
//!    already defaults to. The `GOOSE_PROVIDER` / `GOOSE_MODEL` env vars
//!    are removed because goose consults them *before* the config file —
//!    an inherited shell environment must not out-vote the pin. See
//!    [`acp_router`] for the `session/update_provider` bypass this pin
//!    does NOT close.

use std::path::PathBuf;
use std::sync::OnceLock;

/// Provider pinned as goose's default. Matches Roshera's own provider
/// policy (API-only, Anthropic) and reuses the same `ANTHROPIC_API_KEY`.
const PINNED_PROVIDER: &str = "anthropic";

/// Model pinned for the default provider — the same default model
/// Roshera's own `ClaudeConfig` uses (`ai-integration/src/providers/claude.rs`).
const PINNED_MODEL: &str = "claude-sonnet-5";

/// The meta-permission extension, disabled before everything else: its
/// tools (`manage_extensions`, `search_available_extensions`) exist to
/// let the *model* enable and disable extensions at runtime, so while it
/// is live every other disable is advisory rather than structural. Config
/// key (`name_to_key` form) — its display name is "Extension Manager".
const EXTENSION_MANAGER_KEY: &str = "extensionmanager";

/// The goose root chosen by [`initialize`]. Doubles as the mount gate:
/// [`acp_router`] returns `None` until this is set, so the `/acp` surface
/// structurally cannot exist without the lockdown having run and passed.
static GOOSE_ROOT: OnceLock<PathBuf> = OnceLock::new();

#[derive(Debug, thiserror::Error)]
pub(crate) enum GooseAcpError {
    #[error("goose root '{path}' could not be prepared: {source}")]
    RootDir {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("goose config write failed while {what}: {source}")]
    Config {
        what: &'static str,
        #[source]
        source: goose::config::ConfigError,
    },
    #[error(
        "goose platform extension '{0}' had no config entry to disable — the \
         platform-extension migration did not run where expected, so lockdown \
         cannot be guaranteed; refusing to continue"
    )]
    ExtensionMissing(String),
    #[error(
        "goose still reports enabled extensions after lockdown: {0:?} — an \
         underlying config layer (system config.yaml?) or a goose behavior \
         change is re-enabling tool surface; refusing to boot with it live"
    )]
    LockdownIncomplete(Vec<String>),
}

/// Disable one platform extension by config key, failing closed:
/// `set_extension_enabled` only flips entries that already exist, and
/// returns `false` — without writing anything — when the key is absent.
/// Absence here means the migration that materializes registry entries
/// did not run where this module proved it does (see [`initialize`]'s
/// ordering note), so the disable cannot be trusted and boot must stop.
fn disable_platform_extension(key: &str) -> Result<(), GooseAcpError> {
    if goose::config::set_extension_enabled(key, false) {
        Ok(())
    } else {
        Err(GooseAcpError::ExtensionMissing(key.to_string()))
    }
}

/// One-time goose lockdown. Must run before ANY goose code path is
/// touched: `Config::global()` is a `OnceCell` that captures
/// `GOOSE_PATH_ROOT` at first use, so a late call silently configures
/// goose against the wrong directory with no error.
///
/// Ordering inside this function is load-bearing:
///
/// 1. Env pins (`GOOSE_PATH_ROOT` set; `GOOSE_PROVIDER`/`GOOSE_MODEL`/
///    `GOOSE_ADDITIONAL_CONFIG_FILES` removed) before the first
///    `Config::global()` touch.
/// 2. Provider pin FIRST among config writes: goose's
///    `load_write_config` returns an empty mapping — without running the
///    platform-extension migration — when the config file does not exist
///    yet, and `set_extension_enabled` only flips entries that already
///    exist. The provider write materializes `config.yaml`; the next
///    write-path read then runs the migration that populates every
///    platform-extension entry, which the disable loop flips off.
pub(crate) fn initialize() -> Result<PathBuf, GooseAcpError> {
    // Roshera-owned root. `state/` is gitignored; override for tests and
    // deployments via ROSHERA_GOOSE_ROOT. An externally inherited
    // GOOSE_PATH_ROOT is deliberately ignored and overwritten: it would
    // point goose at a foreign (e.g. developer-desktop) config where the
    // shell tools are enabled.
    let root = match std::env::var_os("ROSHERA_GOOSE_ROOT") {
        Some(dir) => PathBuf::from(dir),
        None => std::env::current_dir()
            .map_err(|source| GooseAcpError::RootDir {
                path: "<cwd>/state/goose-root".to_string(),
                source,
            })?
            .join("state")
            .join("goose-root"),
    };
    std::fs::create_dir_all(&root).map_err(|source| GooseAcpError::RootDir {
        path: root.display().to_string(),
        source,
    })?;
    // goose validates GOOSE_PATH_ROOT with `Path::is_absolute` and falls
    // back to the OS default (the foreign-config hazard above) when the
    // check fails — absolutize so a relative ROSHERA_GOOSE_ROOT cannot
    // silently disable the override.
    let root = std::path::absolute(&root).map_err(|source| GooseAcpError::RootDir {
        path: root.display().to_string(),
        source,
    })?;

    std::env::set_var("GOOSE_PATH_ROOT", &root);
    // Consulted BEFORE the config file by goose's provider resolution —
    // remove so the config pin below is authoritative.
    std::env::remove_var("GOOSE_PROVIDER");
    std::env::remove_var("GOOSE_MODEL");
    // Extra config layers would merge underneath ours and could carry
    // extension entries we never audited.
    std::env::remove_var("GOOSE_ADDITIONAL_CONFIG_FILES");

    let config = goose::config::Config::global();
    goose::config::set_active_provider(config, PINNED_PROVIDER, PINNED_MODEL).map_err(
        |source| GooseAcpError::Config {
            what: "pinning active_provider",
            source,
        },
    )?;

    // Disable EVERY entry in goose's platform-extension registry — the
    // registry itself is the source of truth, not a hand-maintained list,
    // so an upstream rev bump that adds an extension (or flips a
    // default_enabled) is disabled automatically instead of silently
    // joining the tool surface. Default-off entries (`chatrecall`,
    // `summarize`, `orchestrator`, …) are disabled explicitly too:
    // default-off is a default, not a guarantee, and a persisted config
    // entry would survive it. `extensionmanager` goes first — while its
    // enable/disable tools are live, every other line here is advisory.
    disable_platform_extension(EXTENSION_MANAGER_KEY)?;
    for key in goose::agents::extension::PLATFORM_EXTENSIONS.keys() {
        if *key != EXTENSION_MANAGER_KEY {
            disable_platform_extension(key)?;
        }
    }

    // Fail closed: the *merged* config view (which includes the
    // admin-writable system layer goose stacks underneath ours) must
    // report zero enabled extensions, or the server refuses to boot.
    // This is also the tripwire for a future goose adding an enable
    // path this module does not know about.
    let still_enabled: Vec<String> = goose::config::get_enabled_extensions()
        .iter()
        .map(|extension| extension.name())
        .collect();
    if !still_enabled.is_empty() {
        return Err(GooseAcpError::LockdownIncomplete(still_enabled));
    }

    let _ = GOOSE_ROOT.set(root.clone());
    Ok(root)
}

/// The `/acp` router, or `None` if [`initialize`] has not run — callers
/// merge it conditionally, so the ACP surface cannot be mounted without
/// the lockdown having succeeded first.
///
/// Uses goose's bare `create_acp_router` (binds `/acp` internally,
/// loopback-only WebSocket origin policy, no `/health`/`/status`/MCP-app-
/// proxy — those belong to `goose serve`, and Roshera has its own).
/// Roshera's global `auth_middleware` gates it like every other route:
/// `/acp` is not in `path_is_exempt`, so every request — including the
/// WebSocket upgrade — needs a valid credential.
///
/// ## Known open risk — `session/update_provider` (NOT closed here)
///
/// The provider pin in [`initialize`] sets the *default* only. goose's
/// ACP surface exposes a live `session/update_provider` RPC
/// (`acp/server.rs`) that takes the provider name from the *client* and
/// resolves it against a registry populated unconditionally at startup
/// (`providers/init.rs`) — including subprocess-bridge providers
/// (`claude-acp`, `codex-acp`, `copilot-acp`, `amp-acp`, `claude-code`,
/// `codex`, `gemini-cli`, `cursor-agent`) that shell out to those CLI
/// binaries when present on the host, plus `ollama` and other non-API
/// backends. Nothing in `AcpServerFactoryConfig` lets the embedder
/// restrict that registry. Mitigation in place: the endpoint sits behind
/// Roshera's auth middleware and the only intended client is Roshera's
/// own Blackboard, so exploiting it requires an authenticated caller —
/// hardening debt, not an open door. Closing it outright needs a goose
/// patch (provider allowlist) or an accepted-risk sign-off; do not report
/// this as closed.
pub(crate) fn acp_router() -> Option<axum::Router> {
    let root = GOOSE_ROOT.get()?;
    let server = std::sync::Arc::new(goose::acp::server_factory::AcpServer::new(
        goose::acp::server_factory::AcpServerFactoryConfig {
            // No forced builtins — and BUILTIN_REGISTRY is empty anyway
            // (no `goose-mcp` in the build).
            builtins: Vec::new(),
            data_dir: root.join("data"),
            config_dir: root.join("config"),
            // Closed upstream enum (GooseCli | GooseDesktop); affects a
            // display string, not behavior. Cli is the closer fit for a
            // headless embed.
            goose_platform: goose::agents::GoosePlatform::GooseCli,
            additional_source_roots: Vec::new(),
            // The scheduler is its own background-cron surface with its
            // own review needed; off for slice 1.
            enable_scheduler: false,
        },
    ));
    Some(goose::acp::transport::create_acp_router(server))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use goose::agents::{Agent, AgentConfig, GoosePlatform};
    use goose::config::{GooseMode, PermissionManager};
    use goose::session::{SessionManager, SessionType};
    use std::sync::Arc;

    /// THE proving test of slice 1: after the production boot sequence
    /// (`initialize()`), a session built exactly the way goose's ACP
    /// `session/new` builds one (activate `get_enabled_extensions()`,
    /// which is what `initial_session_extensions` uses when the client
    /// passes no MCP servers) must expose ZERO tools. If any built-in
    /// extension survives boot — `developer`'s shell, `extensionmanager`'s
    /// manage-extensions, anything — `get_prefixed_tools` names it and
    /// this fails.
    ///
    /// Process-global by nature (GOOSE_PATH_ROOT + goose's Config
    /// OnceCell): this must remain the only test in this binary that
    /// touches goose config, and it must set the env var before the first
    /// goose call.
    #[tokio::test]
    async fn goose_lockdown_leaves_no_builtin_tool_reachable() {
        let root =
            std::env::temp_dir().join(format!("roshera-goose-lockdown-{}", uuid::Uuid::new_v4()));
        std::env::set_var("ROSHERA_GOOSE_ROOT", &root);

        let goose_root = initialize().expect("goose lockdown boot must succeed");
        assert!(
            goose_root.join("config").join("config.yaml").exists(),
            "initialize() must materialize the goose config file it locked down"
        );

        // Mirror goose's ACP session start (`initial_session_extensions`):
        // with no client-supplied MCP servers, every activated extension
        // comes from the enabled set in config.
        let enabled = goose::config::get_enabled_extensions();

        let sessions_dir = goose_root.join("proof-sessions");
        let session_manager = Arc::new(SessionManager::new(sessions_dir.clone()));
        let permission_manager = Arc::new(PermissionManager::new(sessions_dir));
        let agent = Agent::with_config(AgentConfig::new(
            session_manager.clone(),
            permission_manager,
            None,
            GooseMode::default(),
            true,
            GoosePlatform::GooseCli,
        ));
        let session = session_manager
            .create_session(
                goose_root.clone(),
                "lockdown-proof".to_string(),
                SessionType::User,
                GooseMode::default(),
            )
            .await
            .expect("session creation");
        let session_id = session.id;

        let mut activation_errors = Vec::new();
        for extension in enabled {
            let name = extension.name();
            if let Err(e) = agent.add_extension(extension, &session_id).await {
                activation_errors.push(format!("{name}: {e}"));
            }
        }
        assert!(
            activation_errors.is_empty(),
            "config-enabled goose extensions failed to activate — reachability \
             cannot be assessed: {activation_errors:?}"
        );

        let tools = agent
            .extension_manager
            .get_prefixed_tools(&session_id, None)
            .await
            .expect("tool enumeration");
        let tool_names: Vec<String> = tools.iter().map(|t| t.name.to_string()).collect();
        assert!(
            tool_names.is_empty(),
            "goose built-in tools are reachable through a fresh ACP-equivalent \
             session: {tool_names:?}"
        );
    }
}
