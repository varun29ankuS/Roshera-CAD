# Roshera CAD - Project Instructions

## Rules
- Do not run `cargo build`, `cargo run`, `npm run dev`, or `npm run build` on your own — only run them when Varun explicitly asks you to start, restart, or rebuild
- MUST NOT add "Co-Authored-By" or "Generated with Claude Code" to commits
- Production-grade code only. No TODOs, no placeholders, no mocks, no stubs
- Understand context, intent, and impact before changing anything
- No local AI models — API-only (Claude, OpenAI). No Ollama, no Whisper, no LLaMA
- Timeline-based history (event-sourced), not parametric feature trees

## Architecture
- Star topology centered on the geometry kernel
- Backend-driven: frontend is a thin display layer
- WebSocket for real-time collaboration, REST for CRUD

## Codebase
- `roshera-backend/geometry-engine/` — B-Rep kernel, math, primitives, operations, tessellation
- `roshera-backend/api-server/` — Axum HTTP/WebSocket server
- `roshera-backend/ai-integration/` — Claude + OpenAI providers
- `roshera-backend/timeline-engine/` — Event-sourced design history
- `roshera-backend/session-manager/` — Multi-user RBAC
- `roshera-backend/export-engine/` — STL, OBJ, STEP, ROS formats
- `roshera-backend/shared-types/` — Common types (ObjectId, Position3D, etc.)
- `roshera-backend/rag-engine/` — Vamana-indexed RAG
- `roshera-app/` — React/Vite/TypeScript frontend (Three.js viewport)
