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
- **Nested workspace:** `roshera-backend/Cargo.toml` excludes `vendor/`; the
  vendored crates inherit their settings from this directory's `Cargo.toml`.
