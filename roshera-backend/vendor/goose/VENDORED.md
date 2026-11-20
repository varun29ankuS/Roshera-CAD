# Vendored goose

This directory is the source of the goose agent harness, vendored into Roshera
on 2026-11-04 so the harness is Roshera's own code to change.

- **Upstream:** https://github.com/aaif-goose/goose (formerly block/goose),
  rev `022c17c3946b506b031fc828543cb9e6650dbfd5` (committed 2026-07-30) — the
  same rev Roshera previously consumed as a git dependency, so the build is
  unchanged by the move itself.
- **License:** Apache License 2.0 — see `LICENSE` in this directory. It
  applies to everything under `vendor/goose`; Roshera's own license does not
  change it. Modified files must carry a notice that they were changed.
- **Copied:** the workspace `Cargo.toml`, `LICENSE`, `clippy.toml`,
  `vendor/v8` (a two-file workspace stub), and every crate under `crates/`
  except `goose-cli` and `goose-test` (binaries Roshera does not build).
- **Not compiled:** `goose-local-inference` is present only because Cargo
  must see the source of an optional dependency to resolve it. Its feature
  (`local-inference`) stays off — Roshera is API-only (see
  `api-server/Cargo.toml`'s rules on the `goose` dependency).
- **Modifications by Roshera** (Apache-2.0 §4(b) — changed files are noted):
  - 2026-11-04 `crates/goose/Cargo.toml`: `process-wrap` pinned to `=9.1.0`,
    the version this rev's own `Cargo.lock` resolved. 9.1.1 moved
    `ProcessSession` behind a new `process-session` feature and broke the
    unix-only path in `agents/platform_extensions/developer/shell.rs` (seen in
    CI on Linux; Windows never compiles that path).
  - 2026-11-20 `crates/goose/src/acp/server.rs`, `acp/server_factory.rs`:
    the per-connection session cap is configurable
    (`AcpServerFactoryConfig::max_active_sessions` /
    `GooseAcpAgentOptions::max_active_sessions`, falling back to
    `GOOSE_MAX_ACTIVE_AGENTS`; upstream hard-coded 100 and ignored the
    config), and the connection's session map holds agents weakly so an
    LRU-evicted session really unloads and its stdio extension process is
    killed (upstream kept every session agent alive for the connection's
    life). Constructor call sites in `acp/server/schedule.rs`,
    `tests/acp_transport_auth_test.rs` and `tests/acp_fixtures/mod.rs` gained
    the new field; `tests/acp_extension_process_lifecycle_test.rs` is new.
- **Nested workspace:** `roshera-backend/Cargo.toml` excludes `vendor/`; the
  vendored crates inherit their settings from this directory's `Cargo.toml`.
