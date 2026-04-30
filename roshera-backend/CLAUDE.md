# Roshera CAD Backend — Development Guide

> **Last verified:** 2026-04-30. Every claim in this file has been checked
> against the working tree and the live `cargo check --workspace` output.
> If you change something, re-verify and update the relevant section. No
> aspirational claims, no historical victory laps, no module-completion
> percentages.

---

## Hard rules

1. **Understand context, intent, and impact before changing anything.** Read
   the call sites, trace the data flow, name the invariant you are touching.
2. **Production-grade only.** No `todo!()`, no `unimplemented!()`, no stub
   functions that always return `Ok(())` / `false` / a default. If a feature
   is not implemented, it must not appear in the public surface.
3. **All stubs must be replaced with production code.** When you find a stub,
   implement it. Do not silently delete a stub from the public API — that
   breaks callers. Implement, then verify with tests.
4. **Never run `cargo build` / `cargo run` / `cargo test` / `cargo bench` on
   your own.** Only run them when explicitly asked to start, restart,
   rebuild, or verify. `cargo check` is permitted for verification when
   the user has just asked to verify a claim.
5. **Never delete files without explicit permission.**
6. **Commits must NOT contain `Co-Authored-By: Claude` or
   `Generated with Claude Code` trailers.**
7. **No local AI runtimes.** API-only providers (Claude, OpenAI). No
   Ollama, Whisper, LLaMA, Candle, or Coqui. The policy is enforced at
   `ai-integration/src/providers/mod.rs`.
8. **Timeline-based history (event-sourced), not a parametric feature
   tree.** See "Timeline architecture" below.
9. **DashMap for concurrent shared state, not HashMap.** This is the
   foundation of parallel AI branch exploration; HashMap silently breaks it.
10. **Never bypass the workspace lint policy** (`unwrap_used = "deny"`,
    `expect_used = "deny"`, `panic = "deny"`). When you legitimately need
    one of these, document the invariant immediately above the call and
    use `#[allow(clippy::expect_used)]` with a `// Reason: …` comment.

---

## Workspace layout (verified against `roshera-backend/Cargo.toml`)

Seven crates, resolver = "2":

| Crate              | Responsibility                                           |
|--------------------|----------------------------------------------------------|
| `shared-types`     | `ObjectId`, `Position3D`, common DTOs                    |
| `geometry-engine`  | B-Rep kernel, math, primitives, operations, tessellation |
| `session-manager`  | Multi-user RBAC, session lifecycle                       |
| `timeline-engine`  | Event-sourced design history, branches, replay           |
| `api-server`       | Axum HTTP/WebSocket/SSE server                           |
| `ai-integration`   | Claude provider; provider-manager dispatch               |
| `export-engine`    | STL, OBJ, STEP, IGES, ROS                                |

Workspace dependencies that matter: `axum 0.8.4` (with `ws`),
`tower-http 0.6` (cors/fs/trace), `dashmap 6.1`, `parking_lot 0.12`,
`rayon 1.8`, `lru 0.16`, `tokio 1.0` (`full`), `uuid 1.0`
(`v4,v5,serde`), `thiserror 2.0`.

Workspace lint policy (enforced by `[workspace.lints.clippy]`):
`unwrap_used = "deny"`, `expect_used = "deny"`, `panic = "deny"`.

`roshera-app/` (frontend) sits outside this workspace.

---

## Architecture

### Star topology

The geometry kernel is the single source of truth. Every other crate
talks to it. AI never builds geometry directly — it translates and
delegates. Export queries the kernel for tessellation. The api-server
orchestrates but contains no geometric logic.

```
                ai-integration ──┐
                                 ▼
       session-manager ─▶  geometry-engine  ◀─ export-engine
                                 ▲
                          api-server (REST/WS/SSE)
                                 ▲
                          shared-types (DTOs)
```

### Timeline, not parametric tree

- Every operation is an immutable event.
- The model is rebuilt by replaying events.
- Branches diverge from a parent at a fork point and carry their own
  events; the main timeline trunk is never mutated by branches.
- Branches are stored in a `DashMap<BranchId, Branch>`, and per-branch
  state in `DashMap<EntityId, Entity>` — this is what enables parallel
  AI exploration without locking.
- `OperationRecorder` (geometry-engine `operations/recorder.rs`) is the
  trait the kernel uses to emit `RecordedOperation`. timeline-engine
  provides the `TimelineRecorder` implementation that bridges sync
  kernel calls to the async timeline via an MPSC channel — the kernel
  is never blocked by recording.

### Backend-driven frontend

The frontend (`roshera-app/`) is a thin display layer over Three.js.
Geometry is computed server-side and tessellated for display. The
backend ships display-ready meshes (vertices, normals, indices); the
frontend does not own a geometry kernel.

---

## AI integration

**Policy: API-only.** The only production LLM provider is `ClaudeProvider`
(`ai-integration/src/providers/claude.rs`). When `ANTHROPIC_API_KEY` is
absent, every entry point on `ClaudeProvider` returns
`ProviderError::ProviderUnavailable` — there is no offline fallback, no
keyword parser, no synthetic mock. The api-server gates dispatch on
`ai_configured` and surfaces a 503 to clients when no LLM is registered.

ASR and TTS providers are not yet implemented. The `ASRProvider` and
`TTSProvider` traits exist in `providers/mod.rs`; production
implementations have not been wired. `ProviderManager::asr()` and
`tts(_)` return `ProviderUnavailable` so callers fail loudly.

`MockASRProvider`, `MockLLMProvider`, `MockTTSProvider` exist only when
compiled with `#[cfg(test)]` or `--features mock-providers`. Release
builds without the flag never compile the mock module.

Forbidden dependencies: `candle`, `whisper-rs`, `coqui-tts`, `llama-rs`,
or any other local-inference crate. Do not add them.

---

## Module reality check (verified 2026-04-30)

`geometry-engine/src/math/` (24 files): includes `vector3.rs`,
`vector2.rs`, `matrix4.rs`, `quaternion.rs`, `bspline.rs`, `nurbs.rs`,
`bspline_surface.rs`, `trimmed_nurbs.rs`, `surface_intersection.rs`,
`surface_plane_intersection.rs`, `linear_solver.rs`, `tolerance.rs`,
plus tests and benches under `test_math/`. NurbsCurve has been
consolidated (math layer = pure numerical, primitives layer =
trait-object dispatch); see memory entry "NurbsCurve Consolidation
(#13)".

`geometry-engine/src/primitives/`: full B-Rep with `VertexStore`,
`EdgeStore`, `LoopStore`, `FaceStore`, `ShellStore`, `SolidStore`. T-Splines
were removed (1363 LoC + 2 benches deleted); rebuild path documented in
memory entry "T-Splines Deleted (#20)".

`geometry-engine/src/sketch2d/` (15 files): `constraints.rs`,
`constraint_solver.rs`, `sketch.rs`, `sketch_plane.rs`,
`sketch_topology.rs`, `sketch_validation.rs`, plus 2D primitives.

`geometry-engine/src/operations/`: `boolean.rs`, `intersect.rs`,
`extrude.rs`, `revolve.rs`, `sweep.rs`, `loft.rs`, `fillet.rs`,
`chamfer.rs`, `transform.rs`, `recorder.rs`. Every entry point records
to `OperationRecorder` on success.

`api-server/src/protocol/`: REST, WebSocket, and SSE handlers. There is
known dead code in `protocol/timeline_handlers.rs` and parts of
`protocol/geometry_handlers.rs` — orphaned WS handlers from a previous
protocol design (~30 functions / structs). Cleanup is queued; do not
re-introduce calls into them.

`ai-integration/src/providers/`: `claude.rs`, `mock.rs`,
`native_factory.rs`, `universal_endpoint.rs`. No `coqui_tts.rs`, no
`whisper.rs`, no `llama.rs`. There is `ai-integration/config/vision.toml`;
there is no `config/ai_providers.yaml`.

---

## Build status (verified 2026-04-30)

`cargo check --workspace` succeeds in ~45 s on a warm cache. There is
**one** future-incompat note from a transitive dep (`sqlx-postgres v0.8.0`)
and ~234 dead-code warnings in `api-server`, both queued for cleanup.
There are zero compilation errors.

Real workspace manifest is `roshera-backend/Cargo.toml`. There is no
top-level `Roshera-CAD/Cargo.toml`; if you see one re-appear pointing at
`test-minimal-server.rs`, delete it — that file does not exist.

---

## Patterns and conventions

### Error handling

Production code returns `Result<T, E>` with a typed enum error from
`thiserror`. `unwrap()`/`expect()`/`panic!()` are denied workspace-wide.
Allowed escapes:

- **Invariant-guarded** `Option`/`Result`: use `.expect("<one-line invariant>")`
  with the proof immediately above. Mark the call with
  `#[allow(clippy::expect_used)]`.
- **External / user input**: use `.ok_or_else(|| InvalidInput { … })?`.
- **Lock poisoning** (`RwLock`/`Mutex`): `.expect("<component> RwLock poisoned")`.
- **Partial-cmp NaN**: `.unwrap_or(std::cmp::Ordering::Equal)`.
- **`SystemTime::now().duration_since(UNIX_EPOCH)`**: `.unwrap_or_default()`.

### Concurrency

- Shared mutable state is `DashMap`/`Arc<RwLock<…>>`/atomics, never
  `Mutex<HashMap<…>>`.
- Never hold a `RwLock`/`Mutex` guard across `.await`.
- Spatial grid bounds derived from a user-supplied radius **must not**
  use `f64::MAX` as a "search everything" sentinel; that maps to
  `i32::MIN..i32::MAX` and hangs (~1.8e19 iterations). Branch to a
  linear scan over the underlying DashMap instead. See memory entry
  "Bug Class: f64::MAX → i32 grid saturation".

### Tolerances

Use `Tolerance` / `Tolerance2d` consistently. Do not hardcode
`1e-10` / `1e-12` literals at call sites; thread the tolerance from the
caller. `Tolerance::new(distance, angle)`; `.distance()` is the accessor.

### Recording

Every kernel entry point that mutates topology emits a
`RecordedOperation` via `model.record_operation(…)` on success.
Helpers in `topology_builder.rs` factor the boilerplate. Recording is
silent when no recorder is attached. See memory entry
"OperationRecorder Pattern (#40)".

### Tests vs. production

A file's first `#[cfg(test)]` line is the boundary. Everything below it
is test code. Files under `*_tests.rs`, `primitive_tests/`,
`test_math/` directories, and `[dev-dependencies]` consumers are test
code by definition.

### Type pitfalls (memory)

- `Tolerance::new(d, a)` (2 args) vs `Tolerance::from_distance(d)` (1).
- `ParameterRange { start, end }` are fields, not methods.
- `VertexStore::add_or_find(x, y, z, tolerance)` — 4 separate `f64` args.
- Two `NurbsCurve`s: `primitives::curve::NurbsCurve` (impls `Curve`)
  vs `math::nurbs::NurbsCurve` (does not).
- `SurfacePoint.k1` / `.k2` are principal curvatures; there is no
  `max_curvature` field.

---

## Performance targets (internal regression budget — not vendor comparisons)

| Operation                       | Budget       |
|---------------------------------|--------------|
| Boolean Union (1k faces)        | < 100 ms     |
| Boolean Intersect (1k faces)    | < 150 ms     |
| NURBS Surface Eval (1M pts)     | < 25 ms      |
| B-Spline Curve Eval (1M pts)    | < 10 ms      |
| Tessellation (1M triangles)     | < 250 ms     |
| Memory per 1M vertices          | < 192 MB     |

These are **internal targets** to catch regressions, not benchmarks
against any third-party kernel. Do not publish comparative claims.
Benchmark sources live in `geometry-engine/benches/` and
`ai-integration/benches/`. Mocks in benches are gated behind
`required-features = ["mock-providers"]`.

---

## Documentation

Real, up-to-date references in this repo:

- `roshera-backend/api-server/src/protocol/README.md` — protocol surface
- `roshera-backend/timeline-engine/README.md`
- `roshera-backend/api-server/src/VIEWPORT_BRIDGE.md` — viewport bridge
- `roshera-backend/geometry-engine/PERFORMANCE_METRICS.md`
- `roshera-backend/geometry-engine/src/primitives/PRIMITIVES_MODULE_GUIDE.md`
- `roshera-backend/geometry-engine/src/primitives/primitive_tests/EVIL_EDGE_CASES.md`

Repo root: `README.md`, `BENCHMARKS.md`, `KERNEL_AUDIT_2026-04-19.md`,
`ARCHITECTURE-ANALYSIS.md`, `CONTRIBUTING.md`, `DEPENDENCY_ANALYSIS.md`,
`DEPENDENCY_GRAPH.md`. Anything else at the repo root that pre-dates
April 2026 should be assumed deleted; if it re-appears, treat it as
stale and verify before citing.

---

## References (academic)

1. Piegl & Tiller. *The NURBS Book* (2nd ed.), Springer, 1997.
2. Farin. *Curves and Surfaces for CAGD* (5th ed.), Morgan Kaufmann, 2002.
3. Patrikalakis & Maekawa. *Shape Interrogation for Computer-Aided Design and Manufacturing*, Springer, 2002.
4. Hoffmann. *Geometric and Solid Modeling*, Morgan Kaufmann, 1989.
5. Mäntylä. *An Introduction to Solid Modeling*, Computer Science Press, 1988.
6. de Boor. *A Practical Guide to Splines*, Springer, 1978.
7. ISO 10303-42:2022 — STEP geometric and topological representation.
8. IEEE 754-2019 — Floating-Point Arithmetic.
