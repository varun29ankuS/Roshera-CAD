<p align="center">
  <img src="assets/logo.png" alt="Roshera" width="200" />
</p>

<h1 align="center">Roshera</h1>

<p align="center">
  <strong>AI-native CAD engine — B-Rep geometry kernel built from scratch in Rust.</strong>
</p>

<p align="center">
  <a href="#what-works-today"><img src="https://img.shields.io/badge/primitives-production-brightgreen" alt="Primitives" /></a>
  <a href="#what-works-today"><img src="https://img.shields.io/badge/NURBS-production-brightgreen" alt="NURBS" /></a>
  <a href="#what-works-today"><img src="https://img.shields.io/badge/booleans-working-yellow" alt="Booleans" /></a>
  <a href="#what-works-today"><img src="https://img.shields.io/badge/AI--native-working-yellow" alt="AI" /></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-dual-blue" alt="License" /></a>
</p>

---

Roshera is a boundary representation CAD system with an LLM-driven design workflow, production-grade NURBS mathematics, and a vision-aware AI pipeline that sees your viewport. No wrappers around OpenCASCADE or Parasolid — every line of the geometry kernel is original.

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
  export-engine/       STL, OBJ, encrypted .ros (AES-256-GCM), STEP (in progress)
  rag-engine/          Vamana-indexed retrieval for design knowledge
  api-server/          Axum REST + WebSocket API
  shared-types/        Common type definitions

roshera-app/           React + Three.js + TypeScript browser client
```

## Status

| Layer | Component | Status |
|-------|-----------|--------|
| **Math** | Vector3, Matrix4, Quaternion | Tested, SIMD-optimized |
| | B-spline, NURBS evaluation | Tested, hardened against singularities |
| **Primitives** | Box, Sphere, Cylinder, Cone, Torus | B-Rep topology with Euler validation |
| **Topology** | Manifold detection, adjacency | Tested |
| **Tessellation** | Per-surface dispatch, adaptive subdivision | Tested for analytic surfaces |
| **Operations** | Extrude (draft, taper, twist) | Tested |
| | Boolean (union, intersect, difference) | Implemented, edge cases in progress |
| | Fillet (constant-radius) | Implemented |
| | Chamfer, Offset, Sewing | Implemented |
| | Revolve (full/partial) | Implemented |
| | Sweep (single path) | Implemented, multi-guide not started |
| | Loft (ruled surfaces) | Implemented, smooth NURBS loft not started |
| **Sketch 2D** | Newton-Raphson constraint solver | Implemented |
| **Assembly** | Data model + mates | Defined; constraint solver not started |
| **Export** | STL, OBJ, encrypted .ros | Working |
| | STEP | Bridge implemented, output validation needed |
| **AI** | Claude + OpenAI providers | Working |
| | Vision pipeline + smart routing | Implemented |
| | Natural language command parsing | Working |
| **Infrastructure** | Timeline (event-sourced history) | Working |
| | RAG (Vamana vector index) | Working |
| | Session manager (multi-user, RBAC) | Working |
| **Frontend** | React + R3F viewport, toolbar, chat | Working |

## Performance

Measured numbers from the geometry kernel. See individual sections below for detail and methodology.

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

The Roshera mark is constructed from a Boolean union of a rectangle (2x × 3.236x) and a circle (radius x), expressing the core operation of the geometry kernel itself.

## License

Dual licensed. Free for non-commercial use (research, education, personal projects). Commercial use requires a paid license.

See [LICENSE](LICENSE) for details. Contact 29.varuns@gmail.com for commercial licensing.
