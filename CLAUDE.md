# Roshera CAD - Project Instructions

## Rules
- Do not run `cargo build`, `cargo run`, `npm run dev`, or `npm run build` on your own — only run them when Varun explicitly asks you to start, restart, or rebuild
- MUST NOT add "Co-Authored-By" or "Generated with Claude Code" to commits
- Production-grade code only. No TODOs, no placeholders, no mocks, no stubs
- Understand context, intent, and impact before changing anything
- No local AI models — API-only (Claude, OpenAI). No Ollama, no Whisper, no LLaMA
- Timeline-based history (event-sourced), not parametric feature trees
- Honest refusal over silent wrong answers: operations outside a verified envelope return typed errors, never approximations labeled as exact

## Architecture
- Star topology centered on the geometry kernel
- Backend-driven: frontend is a thin display layer; the primary client is an agent (MCP/REST)
- WebSocket for real-time collaboration, REST for CRUD
- Self-certifying kernel: mutating operations emit verifiable certificates (soundness, DOF accounting, conflict witnesses) — "the kernel cannot lie"

## Codebase
- `roshera-backend/` — Rust workspace root (run cargo commands from here)
  - `geometry-engine/` — B-Rep kernel: math (incl. exact predicates), primitives, operations (booleans, blends, extrude/revolve), sketch2d constraint solver, tessellation
  - `api-server/` — Axum HTTP/WebSocket server (REST + realtime)
  - `assembly-engine/` — instance + mate model, SE(3) solve, interference checks
  - `ai-integration/` — Claude + OpenAI providers
  - `timeline-engine/` — event-sourced design history and replay
  - `session-manager/` — multi-user RBAC
  - `export-engine/` — STL, OBJ, STEP, ROS formats
  - `ros-format/` — native serialization format
  - `shared-types/` — common types (ObjectId, Position3D, etc.)
  - `verdict-harness/` — multi-agent verdict/consensus harness
- `roshera-app/` — React/Vite/TypeScript frontend (Three.js viewport)
- `roshera-mcp/` — MCP server exposing the kernel to agents (rebuild dist + reconnect after tool changes)
- `roshera-eval/` — certificate-scored agent evaluation scenarios

## Build & test discipline
- Workspace lints DENY `unwrap`/`expect`/`panic` in production code
- Disable debuginfo for test/dev builds — debug artifacts have filled the disk before:
  `CARGO_PROFILE_DEV_DEBUG=false CARGO_PROFILE_TEST_DEBUG=false cargo test ...`
- Large test sweeps: run chunked in the foreground (geometry-engine has 180+ test binaries); sweep `target/debug/deps` executables after big gates
- The pre-commit hook runs `cargo check --workspace` (~4 min) — use generous commit timeouts
- Tests are RED-first and mutation-proven: a fix lands with a test that failed before it and provably catches the defect afterward
