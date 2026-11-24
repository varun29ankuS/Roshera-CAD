# Vendored goose

This directory is the source of the goose agent harness, vendored into Roshera
so the harness is Roshera's own code to change.

- **Upstream:** https://github.com/aaif-goose/goose (formerly block/goose).
- **Current:** `v1.60.2`, rev `fb55b5a6d7f5b5df96c8f817f1cde6a81880d9f1`
  (committed 2026-11-20), vendored 2026-11-24.
- **History:**
  - 2026-11-04: first vendored, rev `022c17c3946b506b031fc828543cb9e6650dbfd5`
    (1.45.0, committed 2026-07-30).
  - 2026-11-24: upgraded to 1.60.2.
- **License:** Apache License 2.0 (see `LICENSE` in this directory). It
  applies to everything under `vendor/goose`; Roshera's own license does not
  change it. Every modified file must carry a notice that it was changed.
- **Copied:**
  - The workspace `Cargo.toml`, `LICENSE` and `clippy.toml`.
  - Upstream's `Cargo.lock`, as the reference for the dependency versions
    upstream released against. It is not used to build Roshera; see
    "Dependency versions" below.
  - Every crate under `crates/` except `goose-cli` and `goose-test`, which
    are binaries Roshera does not build.
  - Not copied: upstream's `rust-toolchain.toml` (its development compiler
    pin), its `.cargo/` build settings, and everything outside `crates/`.
- **Not compiled:**
  - `goose-local-inference`: present only because Cargo must see the source
    of an optional dependency to resolve it. Its feature (`local-inference`)
    stays off. Roshera is API-only (see `api-server/Cargo.toml`'s rules on
    the `goose` dependency).
  - `goose-mcp`: a dev-dependency of `goose` only, never part of Roshera's
    build.
  - `goose-roaming`.
- **Features Roshera enables:** only `acp-http`, the HTTP transport for
  `/acp`, which 1.45 provided without a feature gate.

## Modifications by Roshera

**None** as of 1.60.2. Apache-2.0 §4(b): changed files would be listed here.

- The 1.45 `process-wrap` pin (`=9.1.0` in `crates/goose/Cargo.toml`) is
  gone. Versions are now pinned in Roshera's own `Cargo.lock` (see below).
- The 1.45 session-cap change is gone. It made the per-connection session
  cap configurable and let an evicted session unload and stop its extension
  process. Upstream 1.60 does this itself: `SessionRuntime` keeps at most
  `GOOSE_MAX_ACTIVE_AGENTS` sessions loaded and releases the least recently
  used idle one. Roshera sets that value in `api-server/src/goose_acp.rs`
  (`initialize`).

## Dependency versions

goose's dependencies resolve through `roshera-backend/Cargo.lock`, not
upstream's lockfile, so a version upstream never released against can slip
in. That is how `process-wrap` 9.1.1 broke the Linux build under 1.45.

After an upgrade, compare the key crates against `Cargo.lock` in this
directory and pin any that differ, with `cargo update -p <crate> --precise
<version>`. On 2026-11-24, `rmcp`, `rmcp-macros` (3.11.0 → 3.10.0) and
`process-wrap` (10.0.2 → 10.0.0) were pinned to upstream's versions.

Never run a blanket `cargo update` in `roshera-backend`.

## What an upgrade must re-prove

These tests live in `api-server/src/goose_acp.rs`.

- **`goose_lockdown_leaves_exactly_roshera_reachable`:** a session's tools
  are exactly Roshera's.
- **`acp_server_selects_no_builtins_and_pins_sessions_to_the_workspace`:**
  - From 1.60, `AcpBuiltinSelection::from_requested(vec![])` means the
    *developer* extension. Roshera builds an empty selection field by field.
  - Sessions run in the agent workspace, whatever `cwd` the client sends.
- **`a_plugin_in_the_agent_workspace_refuses_boot`:** from 1.60, goose starts
  the MCP servers of every enabled plugin with a session, and a newly found
  plugin is enabled by default. Boot refuses if any plugin is found.
- **`a_workbench_switch_reaches_the_goose_agent`:** this is why we moved to
  1.60. The agent's next step sees the tools a `tools/list_changed` adds.
- **The ACP method gate** (`api-server/src/acp_gate.rs`):
  - It is default-deny, so new upstream methods are refused until they are
    added on purpose.
  - Check the `_meta` keys `session/new` reads. On 1.60, the hazardous ones
    are still `provider`, `enabledExtensions`, `recipeDeeplink` and
    `recipeId`.
