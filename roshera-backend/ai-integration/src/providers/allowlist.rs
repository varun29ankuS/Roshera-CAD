//! Server-owned LLM provider allowlist — the single source of truth for
//! which providers (and which credential modes on each) Roshera will
//! ever construct.
//!
//! # The criterion is WHO chooses, not WHAT is chosen
//!
//! The security defect this module exists to close was never "a
//! subprocess provider exists somewhere" — it was that goose's ACP
//! surface let a *client* pick a provider at runtime
//! (`session/set_config_option` with `config_id: "provider"`,
//! `providers/set`) against a registry populated unconditionally with
//! entries that spawn CLI binaries present on the host (`claude-acp`,
//! `claude-code`, `codex`, `cursor-agent`, `ollama`, …). Those two
//! things are separable:
//!
//! - **Client-selected provider switching is rejected unconditionally**
//!   at the transport layer (`api-server/src/acp_gate.rs` refuses the
//!   RPC methods outright). No entry in this list re-opens that path.
//! - **Server-side configured providers** — chosen deliberately by the
//!   operator/user through Roshera's own settings endpoint and stored
//!   server-side — are governed by this list. An entry that spawns a
//!   local process (`SubscriptionCli`) is *allowed to exist here*
//!   because the user selects it explicitly, with a consent step, in
//!   configuration; it is still never reachable by a client mid-session.
//!
//! Every entry carries a documented reason. Adding an entry is a code
//! change reviewed against these criteria — never a registry that
//! auto-populates.
//!
//! # Honest refusal
//!
//! `resolve` / `resolve_mode` return [`ProviderSelectionRefusal`], a
//! typed, serializable refusal naming what was requested and what is
//! allowed — consistent with the kernel-wide contract that operations
//! outside the verified envelope refuse loudly instead of approximating.

use serde::Serialize;
use thiserror::Error;

/// How a credential for a provider is supplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialMode {
    /// A static API key pasted into Roshera's settings (or inherited
    /// from the environment). Pure HTTPS; no local process.
    ApiKey,
    /// An OAuth profile created by the vendor's own login tooling on
    /// this machine (e.g. `ant auth login`). Short-lived tokens on
    /// `Authorization: Bearer`; nothing static to leak. Pure HTTPS.
    OauthProfile,
    /// Workload Identity Federation — env-var-driven token exchange for
    /// deployed servers. Detected and reported; see the entry's wiring
    /// status for whether this build can serve inference with it.
    WorkloadIdentity,
    /// The vendor's local CLI carrying the user's own subscription
    /// login (Claude Code for Max/Pro, Codex for ChatGPT Plus/Pro).
    /// **Spawns a local process** — requires explicit consent at
    /// configuration time, and is only coherent where the backend and
    /// the user share a machine (desktop / single-user local backend).
    SubscriptionCli,
}

impl CredentialMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            CredentialMode::ApiKey => "api_key",
            CredentialMode::OauthProfile => "oauth_profile",
            CredentialMode::WorkloadIdentity => "workload_identity",
            CredentialMode::SubscriptionCli => "subscription_cli",
        }
    }

    pub fn parse(s: &str) -> Option<CredentialMode> {
        match s {
            "api_key" => Some(CredentialMode::ApiKey),
            "oauth_profile" => Some(CredentialMode::OauthProfile),
            "workload_identity" => Some(CredentialMode::WorkloadIdentity),
            "subscription_cli" => Some(CredentialMode::SubscriptionCli),
            _ => None,
        }
    }
}

/// Whether this build can actually serve inference through a mode, or
/// only stores/validates/detects it. Reported verbatim to the UI so a
/// seam is never presented as a working path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status", content = "reason")]
pub enum WiringStatus {
    /// Inference is served end-to-end through this mode in this build.
    Wired,
    /// The configuration surface (selection, validation, detection,
    /// persistence) exists, but inference is deliberately not wired —
    /// the reason says why. Activating a seam-only mode as the serving
    /// provider is a typed refusal, never a silent no-op.
    SeamOnly(&'static str),
}

/// One credential mode allowed on a provider entry.
#[derive(Debug, Serialize)]
pub struct ModeEntry {
    pub mode: CredentialMode,
    /// True when configuring this mode causes Roshera to spawn a local
    /// binary at inference time. Surfaced in status and consent UI —
    /// never silent.
    pub spawns_local_process: bool,
    pub wiring: WiringStatus,
    /// Why this mode is on the allowlist.
    pub reason: &'static str,
}

/// One provider Roshera may construct, with its documented rationale.
#[derive(Debug, Serialize)]
pub struct AllowlistedProvider {
    /// Stable identifier used by the settings endpoint and stored
    /// config (`"anthropic"`, `"openai"`).
    pub id: &'static str,
    pub display_name: &'static str,
    /// Why this provider is on the allowlist.
    pub reason: &'static str,
    pub modes: &'static [ModeEntry],
}

/// The allowlist. Exactly the providers Roshera can be configured to
/// use, selected server-side through the settings endpoint. Anything
/// absent — including every goose registry entry a client might name
/// over ACP — is refused by construction.
pub const PROVIDER_ALLOWLIST: &[AllowlistedProvider] = &[
    AllowlistedProvider {
        id: "anthropic",
        display_name: "Anthropic (Claude)",
        reason: "First-party hosted Claude API; Roshera's native ClaudeProvider. \
                 Operator/user-configured only.",
        modes: &[
            ModeEntry {
                mode: CredentialMode::ApiKey,
                spawns_local_process: false,
                wiring: WiringStatus::Wired,
                reason: "Static Anthropic API key over HTTPS (x-api-key).",
            },
            ModeEntry {
                mode: CredentialMode::OauthProfile,
                spawns_local_process: false,
                wiring: WiringStatus::Wired,
                reason: "Short-lived OAuth token from an `ant auth login` profile \
                         (or ANTHROPIC_AUTH_TOKEN) over HTTPS Bearer auth — \
                         nothing static to store or leak.",
            },
            ModeEntry {
                mode: CredentialMode::WorkloadIdentity,
                spawns_local_process: false,
                wiring: WiringStatus::SeamOnly(
                    "WIF env vars are detected and reported, but this build does \
                     not perform the /v1/oauth/token exchange yet. The config \
                     schema reserves the mode so adding it is a config change, \
                     not a redesign.",
                ),
                reason: "The correct credential for a deployed backend — \
                         interactive login is for a developer's own machine.",
            },
            ModeEntry {
                mode: CredentialMode::SubscriptionCli,
                spawns_local_process: true,
                wiring: WiringStatus::Wired,
                reason: "User-selected use of their own Claude Max/Pro \
                         subscription via the locally installed Claude Code CLI. \
                         Serves the agent surface (/acp): the goose harness's \
                         `claude-code` provider spawns the CLI and speaks stdio; \
                         the CLI performs its own HTTPS with the user's login. \
                         Roshera's /api/ai REST routes still require an API \
                         credential (their tool_use protocol is not carried \
                         over the CLI transport). Spawns a local process — \
                         explicit consent required; coherent only where backend \
                         and user share a machine (desktop / single-user \
                         backend), stated in the UI rather than hidden.",
            },
        ],
    },
    AllowlistedProvider {
        id: "openai",
        display_name: "OpenAI",
        reason: "Hosted OpenAI API, per Roshera's API-only policy. \
                 Operator/user-configured only.",
        modes: &[
            ModeEntry {
                mode: CredentialMode::ApiKey,
                spawns_local_process: false,
                wiring: WiringStatus::Wired,
                reason: "Static OpenAI API key over HTTPS Bearer auth \
                         (OPENAI_API_KEY). Wires the agent (/acp) surface only, \
                         via goose's own NATIVE OpenAI provider (a hand-written \
                         Rust implementation, not a declarative JSON definition \
                         — `crates/goose/src/providers/openai_def.rs`) — see \
                         xai's entry for the repin mechanism, identical here. \
                         REST routes (/api/ai/command) still have no native \
                         Roshera LLMProvider for OpenAI and stay unavailable \
                         through this key — that gap is unchanged by this repin. \
                         The key is accepted unverified; an invalid key surfaces \
                         at first use. Because goose implements this provider \
                         natively rather than declaratively, model discovery \
                         cannot resolve a base URL for it — same structural gap \
                         as xai, refused typed rather than guessed.",
            },
            ModeEntry {
                mode: CredentialMode::SubscriptionCli,
                spawns_local_process: true,
                wiring: WiringStatus::SeamOnly(
                    "Codex CLI detection, consent, and configuration are landed; \
                     the `codex exec` spawn path is deliberately not wired in \
                     this build rather than shipped half-done.",
                ),
                reason: "User-selected use of their own ChatGPT Plus/Pro \
                         subscription via the locally installed Codex CLI \
                         (`codex exec --json`). Spawns a local process — \
                         explicit consent required; coherent only where backend \
                         and user share a machine.",
            },
        ],
    },
    AllowlistedProvider {
        id: "xai",
        display_name: "xAI Grok",
        reason: "Grok models from xAI, over xAI's OpenAI-compatible Chat \
                 Completions API. Operator/user-configured only.",
        modes: &[ModeEntry {
            mode: CredentialMode::ApiKey,
            spawns_local_process: false,
            wiring: WiringStatus::Wired,
            reason: "Static xAI API key over HTTPS Bearer auth (XAI_API_KEY). \
                     Wires the agent (/acp) surface only: `api-server`'s \
                     `PUT /api/ai/provider` sets XAI_API_KEY in the process \
                     environment and pins goose's active_provider to its own \
                     native `xai` provider (`repin_goose_to_declarative_provider`) \
                     — the same env-var-first credential resolution that \
                     already makes a bare ANTHROPIC_API_KEY work for goose's \
                     anthropic default, generalized. REST routes \
                     (/api/ai/command) have no native Roshera LLMProvider for \
                     xAI and stay unavailable through this key — same \
                     REST/agent split already documented on anthropic's \
                     subscription_cli entry. The key itself is accepted \
                     unverified (no vetted synchronous credential-check \
                     client for xAI exists in this build); an invalid key \
                     surfaces at first use, not at save time.",
        }],
    },
    AllowlistedProvider {
        id: "mistral",
        display_name: "Mistral AI",
        reason: "Hosted Mistral AI API, OpenAI-compatible Chat Completions. \
                 Operator/user-configured only.",
        modes: &[ModeEntry {
            mode: CredentialMode::ApiKey,
            spawns_local_process: false,
            wiring: WiringStatus::Wired,
            reason: "Static Mistral API key over HTTPS Bearer auth \
                     (MISTRAL_API_KEY). Wires the agent (/acp) surface only, \
                     via goose's bundled `mistral` declarative provider — see \
                     xai's entry above for the exact repin mechanism, \
                     identical here. REST routes (/api/ai/command) have no \
                     native Roshera LLMProvider for Mistral and stay \
                     unavailable through this key. The key is accepted \
                     unverified; an invalid key surfaces at first use.",
        }],
    },
    AllowlistedProvider {
        id: "glm",
        display_name: "Zhipu GLM",
        reason: "Zhipu's GLM models, OpenAI-compatible Chat Completions API. \
                 Operator/user-configured only.",
        modes: &[ModeEntry {
            mode: CredentialMode::ApiKey,
            spawns_local_process: false,
            wiring: WiringStatus::Wired,
            reason: "Static Zhipu API key over HTTPS Bearer auth \
                     (ZHIPU_API_KEY). Wires the agent (/acp) surface only, \
                     via goose's bundled `zhipu` declarative provider — \
                     Roshera's own allowlist id for this vendor is `glm` \
                     (the model family users type), which \
                     `repin_goose_to_declarative_provider` maps to goose's \
                     `zhipu` provider name; see xai's entry for the repin \
                     mechanism, identical here. REST routes \
                     (/api/ai/command) have no native Roshera LLMProvider \
                     for GLM and stay unavailable through this key. The key \
                     is accepted unverified; an invalid key surfaces at \
                     first use.",
        }],
    },
    AllowlistedProvider {
        id: "kimi",
        display_name: "Moonshot Kimi",
        reason: "Moonshot's Kimi models, over Moonshot's own OpenAI-compatible \
                 Chat Completions API. Operator/user-configured only.",
        modes: &[ModeEntry {
            mode: CredentialMode::ApiKey,
            spawns_local_process: false,
            wiring: WiringStatus::Wired,
            reason: "Static Moonshot API key over HTTPS Bearer auth \
                     (MOONSHOT_API_KEY) — distinct from the Kimi Code CLI \
                     subscription credential goose separately supports \
                     (`kimi_code`, untouched by this entry). Wires the agent \
                     (/acp) surface only, via goose's bundled `moonshot` \
                     declarative provider — Roshera's own allowlist id for \
                     this vendor is `kimi` (the model family users type), \
                     which `repin_goose_to_declarative_provider` maps to \
                     goose's `moonshot` provider name; see xai's entry for \
                     the repin mechanism, identical here. REST routes \
                     (/api/ai/command) have no native Roshera LLMProvider \
                     for Kimi and stay unavailable through this key. The key \
                     is accepted unverified; an invalid key surfaces at \
                     first use.",
        }],
    },
    AllowlistedProvider {
        id: "deepseek",
        display_name: "DeepSeek",
        reason: "DeepSeek's hosted models, OpenAI-compatible Chat \
                 Completions API. Operator/user-configured only.",
        modes: &[ModeEntry {
            mode: CredentialMode::ApiKey,
            spawns_local_process: false,
            wiring: WiringStatus::Wired,
            reason: "Static DeepSeek API key over HTTPS Bearer auth \
                     (DEEPSEEK_API_KEY). Wires the agent (/acp) surface only, \
                     via goose's bundled `custom_deepseek` declarative \
                     provider — Roshera's own allowlist id for this vendor is \
                     `deepseek` (the vendor name users type), which \
                     `repin_goose_to_declarative_provider` maps to goose's \
                     `custom_deepseek` provider name (a rename like glm's \
                     mapping to `zhipu`, because goose's own bundled name for \
                     this vendor isn't the bare vendor string either); see \
                     xai's entry for the repin mechanism, identical here. \
                     Because it has a declarative JSON definition (unlike \
                     openai/xai's native Rust providers), model discovery CAN \
                     resolve a base URL for it. REST routes (/api/ai/command) \
                     have no native Roshera LLMProvider for DeepSeek and stay \
                     unavailable through this key. The key is accepted \
                     unverified; an invalid key surfaces at first use. Its \
                     default model is `None` here: the bundled JSON lists \
                     `deepseek-v4-flash`/`deepseek-v4-pro`, but querying that \
                     list live requires a key this build does not hold, and a \
                     model copied from documentation rather than verified \
                     live is exactly how `sarvam-30b` — a model that vendor \
                     does not serve — ended up in this repo's config before.",
        }],
    },
    AllowlistedProvider {
        id: "sarvam",
        display_name: "Sarvam AI",
        reason: "Sarvam AI, an Indian LLM vendor exposing an \
                 OpenAI-compatible Chat Completions API. \
                 Operator/user-configured only.",
        modes: &[ModeEntry {
            mode: CredentialMode::ApiKey,
            spawns_local_process: false,
            wiring: WiringStatus::Wired,
            reason: "Static Sarvam AI API key over HTTPS Bearer auth \
                     (SARVAM_API_KEY). Unlike xai/mistral/glm/kimi, Sarvam \
                     has no goose-bundled declarative provider — instead \
                     the custom-provider JSON lives at \
                     `state/goose-root/config/custom_providers/sarvam.json`, \
                     which goose's declarative-provider loader \
                     (`declarative_providers.rs::load_provider` and \
                     `register_declarative_providers`) reads ahead of its \
                     bundled definitions and registers into the same \
                     `ProviderRegistry` as the bundled vendors, keyed by \
                     the same `name` field — so `repin_goose_to_declarative_provider` \
                     maps Roshera's `sarvam` id to goose's provider name \
                     `sarvam` (an identity mapping, unlike glm's mapping to \
                     `zhipu`) and it resolves the same way xai/mistral/glm/ \
                     kimi already do. The base URL \
                     (`https://api.sarvam.ai/v1`) and the single live \
                     model `sarvam-105b` were verified against \
                     `GET https://api.sarvam.ai/v1/models` on 2026-08-01; \
                     `sarvam-30b`, previously declared in that JSON, was \
                     removed because the live API does not serve it — \
                     declaring a model the vendor doesn't serve is the \
                     exact failure this seam existed to prevent. This \
                     wires the agent (/acp) surface only: there is no \
                     native Roshera `LLMProvider` for Sarvam, so REST \
                     routes (/api/ai/command) stay unavailable through \
                     this key. The key itself is accepted unverified; an \
                     invalid key surfaces at first use.",
        }],
    },
];

/// Typed refusal for provider/mode selection outside the allowlist.
#[derive(Debug, Error, Serialize)]
pub enum ProviderSelectionRefusal {
    #[error(
        "provider '{requested}' is not on Roshera's provider allowlist \
         (allowed: {allowed:?}). Providers are operator/user-configured \
         server-side only; anything that resolves by spawning an arbitrary \
         local binary is excluded by construction."
    )]
    NotAllowlisted {
        requested: String,
        allowed: Vec<&'static str>,
    },
    #[error(
        "credential mode '{requested_mode}' is not supported for provider \
         '{provider}' (supported: {supported:?})"
    )]
    ModeUnsupported {
        provider: &'static str,
        requested_mode: String,
        supported: Vec<&'static str>,
    },
}

/// Every allowlisted provider id.
pub fn allowed_provider_ids() -> Vec<&'static str> {
    PROVIDER_ALLOWLIST.iter().map(|p| p.id).collect()
}

/// Resolve a provider id against the allowlist.
pub fn resolve(id: &str) -> Result<&'static AllowlistedProvider, ProviderSelectionRefusal> {
    // Strict, exact-match lookup. There is deliberately no fallback,
    // no fuzzy match, and no dynamic registration path: an id that is
    // not literally in PROVIDER_ALLOWLIST is a typed refusal.
    // (RED-proven: with a goose-style any-name-resolves fallback here,
    // `subprocess_and_local_providers_are_refused_by_name` fails.)
    PROVIDER_ALLOWLIST
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| ProviderSelectionRefusal::NotAllowlisted {
            requested: id.to_string(),
            allowed: allowed_provider_ids(),
        })
}

/// Resolve a (provider, credential mode) pair against the allowlist.
pub fn resolve_mode(
    id: &str,
    mode: CredentialMode,
) -> Result<(&'static AllowlistedProvider, &'static ModeEntry), ProviderSelectionRefusal> {
    let provider = resolve(id)?;
    let entry = provider
        .modes
        .iter()
        .find(|m| m.mode == mode)
        .ok_or_else(|| ProviderSelectionRefusal::ModeUnsupported {
            provider: provider.id,
            requested_mode: mode.as_str().to_string(),
            supported: provider.modes.iter().map(|m| m.mode.as_str()).collect(),
        })?;
    Ok((provider, entry))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// THE load-bearing refusal test: every provider name goose's ACP
    /// registry would happily resolve — including the subprocess-bridge
    /// providers that shell out to CLI binaries present on a developer
    /// machine — must be a typed refusal here, not a resolution.
    #[test]
    fn subprocess_and_local_providers_are_refused_by_name() {
        for forbidden in [
            "claude-acp",
            "claude-code",
            "codex",
            "codex-acp",
            "copilot-acp",
            "amp-acp",
            "gemini-cli",
            "cursor-agent",
            "ollama",
            "local-inference",
        ] {
            let result = resolve(forbidden);
            match result {
                Err(ProviderSelectionRefusal::NotAllowlisted { requested, allowed }) => {
                    assert_eq!(requested, forbidden);
                    assert!(allowed.contains(&"anthropic"));
                }
                other => panic!(
                    "provider '{forbidden}' must be a typed NotAllowlisted refusal, \
                     got: {other:?}"
                ),
            }
        }
    }

    #[test]
    fn unknown_and_empty_ids_are_refused() {
        assert!(resolve("").is_err());
        assert!(
            resolve("Anthropic").is_err(),
            "ids are exact-match, not case-folded"
        );
        assert!(
            resolve("anthropic ").is_err(),
            "ids are exact-match, not trimmed"
        );
        assert!(resolve("does-not-exist").is_err());
    }

    #[test]
    fn allowlisted_providers_resolve() {
        assert_eq!(
            resolve("anthropic").expect("anthropic allowlisted").id,
            "anthropic"
        );
        assert_eq!(resolve("openai").expect("openai allowlisted").id, "openai");
    }

    #[test]
    fn anthropic_api_key_and_oauth_are_wired() {
        let (_, api_key) = resolve_mode("anthropic", CredentialMode::ApiKey).unwrap();
        assert_eq!(api_key.wiring, WiringStatus::Wired);
        assert!(!api_key.spawns_local_process);

        let (_, oauth) = resolve_mode("anthropic", CredentialMode::OauthProfile).unwrap();
        assert_eq!(oauth.wiring, WiringStatus::Wired);
        assert!(!oauth.spawns_local_process);
    }

    /// Subscription-CLI modes exist (user-selected, consent-gated) and
    /// must be marked as spawning a local process — a UI reading this
    /// list can never present the spawn silently. The Claude path is
    /// wired (primary demo path, serving /acp via goose's claude-code
    /// provider); the Codex path is a seam in this build.
    #[test]
    fn subscription_cli_modes_are_flagged_as_spawning() {
        for provider in ["anthropic", "openai"] {
            let (_, entry) = resolve_mode(provider, CredentialMode::SubscriptionCli)
                .unwrap_or_else(|e| panic!("{provider} subscription_cli must be listed: {e}"));
            assert!(
                entry.spawns_local_process,
                "{provider} subscription_cli must declare it spawns a local process"
            );
        }
        let (_, claude) = resolve_mode("anthropic", CredentialMode::SubscriptionCli).unwrap();
        assert_eq!(claude.wiring, WiringStatus::Wired);
        let (_, codex) = resolve_mode("openai", CredentialMode::SubscriptionCli).unwrap();
        assert!(matches!(codex.wiring, WiringStatus::SeamOnly(_)));
    }

    #[test]
    fn openai_oauth_profile_is_mode_unsupported() {
        match resolve_mode("openai", CredentialMode::OauthProfile) {
            Err(ProviderSelectionRefusal::ModeUnsupported {
                provider,
                requested_mode,
                supported,
            }) => {
                assert_eq!(provider, "openai");
                assert_eq!(requested_mode, "oauth_profile");
                assert!(supported.contains(&"api_key"));
            }
            other => panic!("expected ModeUnsupported, got {other:?}"),
        }
    }

    #[test]
    fn refusals_serialize_with_named_fields() {
        let refusal = resolve("ollama").unwrap_err();
        let json = serde_json::to_value(&refusal).expect("refusal serializes");
        assert_eq!(json["NotAllowlisted"]["requested"], "ollama");
    }

    /// Every entry's reason text is shown to the user (settings dialog,
    /// `GET /api/ai/provider`'s `allowlist` field, and — for `SeamOnly` —
    /// the typed PUT refusal). An empty reason would mean the UI silently
    /// swallows the explanation of why a mode is not a working path.
    #[test]
    fn all_entries_have_non_empty_reasons() {
        for provider in PROVIDER_ALLOWLIST {
            assert!(
                !provider.reason.trim().is_empty(),
                "provider '{}' has an empty top-level reason",
                provider.id
            );
            assert!(
                !provider.modes.is_empty(),
                "provider '{}' has no credential modes",
                provider.id
            );
            for mode in provider.modes {
                assert!(
                    !mode.reason.trim().is_empty(),
                    "'{}'/{:?} has an empty mode reason",
                    provider.id,
                    mode.mode
                );
                if let WiringStatus::SeamOnly(why) = mode.wiring {
                    assert!(
                        !why.trim().is_empty(),
                        "'{}'/{:?} is SeamOnly with an empty reason — the UI \
                         would present an unexplained refusal",
                        provider.id,
                        mode.mode
                    );
                }
            }
        }
    }

    /// The exhaustive set of (provider, mode) pairs with a real,
    /// resolvable end-to-end inference path in this build. Marking a mode
    /// `Wired` is a claim the settings UI takes at face value and offers
    /// to the user as a working path — growing this list is a deliberate
    /// code change (wiring a repin path, registering a live
    /// `LLMProvider`), never a side effect of adding an allowlist entry
    /// for a vendor whose only support lives on goose's side or in
    /// unwired `universal_endpoint.rs` code.
    const KNOWN_WIRED_PATHS: &[(&str, CredentialMode)] = &[
        ("anthropic", CredentialMode::ApiKey),
        ("anthropic", CredentialMode::OauthProfile),
        ("anthropic", CredentialMode::SubscriptionCli),
        // 2026-08-01: `api-server`'s `PUT /api/ai/provider` gained
        // `repin_goose_to_declarative_provider`, generalizing the
        // ANTHROPIC_API_KEY-env-var-first mechanism that already served
        // anthropic to any goose provider that resolves its credential
        // the same way. These five wire the /acp agent surface only —
        // REST (/api/ai/command) has no native Roshera LLMProvider for
        // any of them, same split already true of anthropic's
        // subscription_cli entry above.
        ("xai", CredentialMode::ApiKey),
        ("mistral", CredentialMode::ApiKey),
        ("glm", CredentialMode::ApiKey),
        ("kimi", CredentialMode::ApiKey),
        // sarvam has no goose-bundled provider — it resolves through the
        // custom-provider JSON at
        // `state/goose-root/config/custom_providers/sarvam.json`, which
        // `register_declarative_providers` registers into the same
        // `ProviderRegistry` as the bundled vendors above, so it is
        // equally resolvable, not a different tier of "wired".
        ("sarvam", CredentialMode::ApiKey),
        // openai resolves through goose's own NATIVE OpenAI provider (Rust,
        // not a JSON definition) — same repin mechanism, no discovery base
        // URL (see openai's entry above and BaseUrlUnresolved's doc).
        ("openai", CredentialMode::ApiKey),
        // deepseek resolves through goose's bundled `custom_deepseek`
        // declarative JSON, reached via a rename mapping (deepseek ->
        // custom_deepseek), same tier as xai/mistral/glm/kimi.
        ("deepseek", CredentialMode::ApiKey),
    ];

    #[test]
    fn wired_entries_match_the_known_resolvable_paths() {
        for provider in PROVIDER_ALLOWLIST {
            for mode in provider.modes {
                let is_wired = matches!(mode.wiring, WiringStatus::Wired);
                let is_known_resolvable = KNOWN_WIRED_PATHS
                    .iter()
                    .any(|(id, m)| *id == provider.id && *m == mode.mode);
                assert_eq!(
                    is_wired, is_known_resolvable,
                    "'{}'/{:?}: wiring={:?} but KNOWN_WIRED_PATHS says \
                     known_resolvable={} — Wired must correspond exactly to \
                     a documented resolvable inference path, never an entry \
                     marked wired because it 'should' work",
                    provider.id, mode.mode, mode.wiring, is_known_resolvable
                );
            }
        }
    }

    /// The four vendors `api-server`'s `repin_goose_to_declarative_provider`
    /// serves via one of goose's own bundled providers (2026-08-01): each
    /// pins goose's own bundled provider for the vendor and supplies its
    /// API key via the exact env var that provider's credential resolution
    /// reads. None spawns a local process — these are pure API-key
    /// vendors, unlike subscription_cli.
    #[test]
    fn xai_mistral_glm_kimi_api_key_are_wired() {
        for id in ["xai", "mistral", "glm", "kimi"] {
            let (_, entry) = resolve_mode(id, CredentialMode::ApiKey)
                .unwrap_or_else(|e| panic!("'{id}'/api_key must be allowlisted: {e}"));
            assert_eq!(
                entry.wiring,
                WiringStatus::Wired,
                "'{id}'/api_key must be Wired now that the goose repin is generalized"
            );
            assert!(
                !entry.spawns_local_process,
                "'{id}'/api_key is a plain API-key vendor and must not \
                 declare it spawns a local process"
            );
        }
    }

    /// `sarvam` is `Wired` via a different path than the four above: it has
    /// no goose-bundled provider, so `repin_goose_to_declarative_provider`
    /// targets the custom-provider JSON at
    /// `state/goose-root/config/custom_providers/sarvam.json` under an
    /// identity mapping (`sarvam` -> `sarvam`), verified live against
    /// `GET https://api.sarvam.ai/v1/models` on 2026-08-01.
    #[test]
    fn sarvam_api_key_is_wired() {
        let (_, entry) = resolve_mode("sarvam", CredentialMode::ApiKey)
            .unwrap_or_else(|e| panic!("'sarvam'/api_key must be allowlisted: {e}"));
        assert_eq!(
            entry.wiring,
            WiringStatus::Wired,
            "'sarvam'/api_key must be Wired now that its custom-provider JSON is written \
             and verified"
        );
        assert!(
            !entry.spawns_local_process,
            "'sarvam'/api_key is a plain API-key vendor and must not declare it spawns a \
             local process"
        );
    }
}
