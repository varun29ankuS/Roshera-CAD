<p align="center">
  <img src="assets/logo.png" alt="Roshera" width="200" />
</p>

<h1 align="center">Roshera</h1>

<p align="center">
  An agent runtime for geometry — a native Rust B-Rep kernel with a semantic surface AI can query, reason about, and act on.
  <br />
  <strong>Every operation returns a validity certificate. The kernel cannot lie.</strong>
</p>

<p align="center">
  <a href="#status"><img src="https://img.shields.io/badge/status-work%20in%20progress-yellow" alt="Status" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-FSL--1.1--Apache--2.0-blue" alt="License" /></a>
</p>

---

<p align="center">
  <img src="assets/Roshera_demo.gif" alt="Roshera demo — an agent builds a rocket thrust chamber and a keyed gear pair, each verified watertight and sound" width="100%" />
</p>

<p align="center">
  <em>An agent drives the kernel over the bridge to build real mechanical parts — a revolved rocket thrust chamber and a pair of keyed involute gears — and the kernel certifies each one as a closed, watertight, physically-sound solid.</em>
</p>

---

Roshera is an **agent runtime for geometry**. The product is the kernel and the bridge it exposes: a native Rust B-Rep engine whose primitives, topology, and operations carry enough semantic structure for an LLM to query, reason about, and drive directly. Humans orchestrate; agents execute. The React/Three.js frontend that ships in this repo is one client of that runtime — it talks to the kernel over REST + WebSocket the same way an external agent would.

The differentiator is the readable surface: geometry is not just triangles, it is a queryable model with named features, intent, and history. And every operation an agent runs returns the kernel's **full validity certificate** — watertight, manifold, oriented, self-intersection-free, mesh-quality-clean, plus a dual-eye consistency check between what was built, what renders, and what the feature recognizer sees — so the geometry carries its own proof of correctness instead of a render that merely looks right. Many things work, many things don't — see [Status](#status) for what's actually usable today.

## Why a certificate matters

AI can generate CAD everywhere now. Nobody can trust what it generates — because the standard ways of checking are blind. We built a benchmark that injects four classes of silent geometric lie (flipped normals, self-intersections, torn seams, non-manifold facets) into sound parts and asks each verifier:

| Verifier | Lies caught |
|---|---|
| B-Rep validity check alone | **0 / 4** |
| "Mesh looks closed" heuristic | **2 / 4** |
| A frontier vision model, shown the render | misses the flagships |
| **Roshera's certificate** | **4 / 4** |

The benchmark is reproducible (`geometry-engine/tests/injected_defect_benchmark.rs`; every table cell is derived from measured verdicts at run time, never hardcoded). The same certificate acts as the referee for autonomous design: in a certified exploration run, an agent swept **300 rocket-engine variants in ~12 minutes** — the certificate killed 108 unsound ones, and the winner is provably the most material-efficient survivor at fixed chamber capacity. An optimizer without the certificate happily picks corpses; ours can't.

| Dark Mode | Light Mode |
|-----------|------------|
| ![Dark Mode](roshera-app/docs/screenshots/dark-mode.png) | ![Light Mode](roshera-app/docs/screenshots/light-mode.png) |

## Architecture

```
roshera-backend/
  geometry-engine/     B-Rep kernel: NURBS math, primitives, topology, operations, tessellation
  ai-integration/      LLM providers (Claude, OpenAI) + vision pipeline + smart routing
  timeline-engine/     Event-sourced design history with branching
  session-manager/     Multi-user collaboration with RBAC
  export-engine/       STL, OBJ, STEP (AP242, round-trip tested), encrypted .ros (AES-256-GCM)
  rag-engine/          Vamana-indexed retrieval for design knowledge
  api-server/          Axum REST + WebSocket API
  shared-types/        Common type definitions

roshera-app/           React + Three.js + TypeScript browser client
```

## Direction

The kernel is the product. The viewport you see in the screenshots is one
client of it, written to prove the bridge works at human latency. The
same REST + WebSocket surface drives agents.

Where this is going: agents that pick faces, fillet edges, run sections,
and walk a part the way a designer would — by reasoning over the
readable surface (named features, persistent IDs, the timeline of
events that produced the current state), not by parsing triangle soup.
An MCP server already exposes the kernel to agents directly — the demo
above was built through it, one tool call per feature, with each result
verified sound and watertight before the next step. Next are embeddings
and retrieval over the timeline so an agent can answer questions like
"how did this corner get to be 4mm" without re-deriving the model from
scratch.

## Status

What follows is honest about what's tested versus what's implemented but rough. Measured perf numbers are in [Performance](#performance).

| Layer | Component | Status |
|-------|-----------|--------|
| **Math** | Vector3, Matrix4, Quaternion | Tested |
| | B-spline, NURBS evaluation | Tested; perf above budget (see Performance) |
| **Primitives** | Box, Sphere, Cylinder, Cone, Torus | B-Rep topology with Euler validation |
| **Topology** | Manifold detection, adjacency | Tested |
| **Tessellation** | Per-surface dispatch, adaptive subdivision | Works for analytic surfaces; extruded curved profiles still have watertightness issues |
| **Operations** | Extrude (draft, taper, twist) | Implemented; side-face seam bugs being chased |
| | Boolean (union, intersect, difference) | Implemented; edge cases (e.g. drill-through-cube) still fail in places |
| | Fillet (constant-radius) | Implemented, lightly tested |
| | Chamfer, Offset, Sewing | Implemented, lightly tested |
| | Revolve (full/partial) | Implemented, lightly tested |
| | Sweep (single path) | Implemented; multi-guide not done |
| | Loft (ruled surfaces) | Implemented; smooth NURBS loft not done |
| **Sketch 2D** | Newton-Raphson constraint solver | Implemented |
| **Assembly** | Data model + mates | Defined; constraint solver not done |
| **Export** | STL, OBJ, encrypted .ros | Works |
| | STEP | Skeleton in place; output not validated |
| **AI** | Claude + OpenAI providers | Works |
| | Vision pipeline + smart routing | Implemented |
| | Natural language command parsing | Works for common commands |
| **Infrastructure** | Timeline (event-sourced history) | Works |
| | RAG (Vamana vector index) | Works |
| | Session manager (multi-user, RBAC) | Works |
| **Frontend** | React + R3F viewport, toolbar, chat | Works; rough around the edges |

## Performance

Measured numbers from the geometry kernel. See individual sections below for detail and methodology. Full table and reproduction commands in [BENCHMARKS.md](BENCHMARKS.md).

### Math microbenchmarks

Criterion, release build, median of 100 samples.

| Operation | Time |
|-----------|------|
| Vector3 dot | 500 ps |
| Vector3 cross | 884 ps |
| Vector3 normalize | 1.68 ns |
| Vector3 add | 984 ps |
| Matrix4 multiply | 5.14 ns |
| Matrix4 inverse | 29.4 ns |
| Matrix4 transpose | 5.50 ns |
| Point3 distance | 505 ps |
| Point3 translate | 868 ps |

### Primitive creation

Full B-Rep topology construction with a fresh `BRepModel` per iteration. Criterion, release build.

| Primitive | Time |
|-----------|------|
| Box | 65 µs |
| Sphere | 49 µs |
| Cylinder | 50 µs |

### Boolean and intersection

Internal benchmark suite on 1k-face inputs, release build.

| Operation | Measured | Internal target |
|-----------|----------|-----------------|
| Boolean union | 50.5 ms | <100 ms |
| Boolean intersection | 75.4 ms | <150 ms |
| Face–face intersection | 25.3 ms | <50 ms |

All three are within their internal regression budgets on this host.

### NURBS and B-spline evaluation

1M-point sweep, release build.

| Operation | Measured | Internal target |
|-----------|----------|-----------------|
| NURBS surface eval | 158 ms | <25 ms |
| B-spline curve eval | 36.8 ms | <10 ms |

Both are above their internal regression budgets — NURBS eval by ~6× and B-spline eval by ~3.7×. Flagged for profiling and optimization.

### Tessellation and memory

| Metric | Measured | Internal target |
|--------|----------|-----------------|
| Tessellation (1M triangles, estimated) | 5,350 ms | <250 ms |
| Memory per 1M vertices (SoA layout) | 34.3 MB | <192 MB |

Memory is under budget on this host. Tessellation is the largest performance gap (~21× over target) and the next thing to profile.

### Coverage gaps

- **Delete primitives** — only correctness tests (`delete_solid`, `delete_face`, cascade, orphan cleanup). No Criterion target yet.
- **2D sketch creation (sketch2d)** — ~5k LoC subsystem with 69 passing correctness tests, but no timing benchmark target.
- **Cone / Torus** — primitive creation benchmarks only cover Box / Sphere / Cylinder.

These are tracked as blind spots to add to `benches/geometry_bench.rs`.

### Methodology

- Host: Windows 11, x86_64, release build.
- Profile overrides: `CARGO_PROFILE_BENCH_LTO=off`, `CARGO_PROFILE_BENCH_CODEGEN_UNITS=16`. Full-LTO would shave an additional 15–30% off these numbers but currently hits a rustc-LLVM OOM on this host — fix tracked separately.
- Criterion numbers reported as the median of 100 samples after a 3-second warmup.
- Internal-suite numbers (Boolean / NURBS / tessellation) are single-sample wall-clock with per-operation iteration counts in the 10–100 range. Treat Criterion numbers as the statistical baseline.
- "Internal target" = internal regression budget. Not a comparison against any third-party kernel.

Reproduce:

```bash
cd roshera-backend
CARGO_PROFILE_BENCH_LTO=off CARGO_PROFILE_BENCH_CODEGEN_UNITS=16 \
  cargo bench -p geometry-engine --bench geometry_bench

CARGO_PROFILE_RELEASE_LTO=off CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 \
  cargo test --release -p geometry-engine --lib test_performance_benchmark_suite -- --nocapture
```

## Getting Started

```bash
# Backend
cd roshera-backend
cargo run --bin api-server
# API on http://localhost:3000, WebSocket on ws://localhost:3000/ws

# Frontend (separate terminal)
cd roshera-app
npm install
npm run dev
# UI on http://localhost:5173
```

### Docker

```bash
cd roshera-backend
docker compose up
```

### Prerequisites

- Rust 1.75+
- Node.js 20+

## API

```bash
# Create a box
curl -X POST http://localhost:3000/api/geometry \
  -H "Content-Type: application/json" \
  -d '{"operation": "create_primitive", "parameters": {"type": "box", "width": 10, "height": 10, "depth": 10}}'
```

```javascript
// WebSocket
const ws = new WebSocket("ws://localhost:3000/ws");
ws.send(JSON.stringify({
  type: "GeometryCommand",
  data: { command: "CreatePrimitive", parameters: { type: "sphere", radius: 5.0 } }
}));
```

## Logo

<p align="center">
  <img src="assets/logo-dimensions.png" alt="Roshera Logo Dimensions" width="400" />
</p>

The Roshera mark is a Boolean union of a rectangle (2x × 3.236x) and a circle (radius x).

## License

**[Functional Source License 1.1 (FSL-1.1-Apache-2.0)](LICENSE)** — Fair Source.

You may use, copy, modify, and redistribute Roshera for any purpose — including
commercial use — **except** offering a product or service that competes with it
(e.g. a competing geometry kernel, CAD engine, or hosted service built on this
code). Internal use, education, research, and building non-competing products
on top of Roshera are all permitted.

**Every release automatically converts to [Apache 2.0](https://www.apache.org/licenses/LICENSE-2.0)
two years after publication** — today's code is tomorrow's open source.

For licensing beyond the grant (OEM/embedding, competing-use exceptions):
29.varuns@gmail.com.
