# Roshera

**The world's first AI-readable geometry kernel.**

Traditional CAD kernels store geometry as pure mathematics: vertices, edges, NURBS control points. To an AI agent, this is opaque data. It can see a cylindrical hole but cannot know it's for an M8 socket head cap screw, that it must have 0.1mm positional tolerance, or that it was designed for CNC machining.

Roshera is a B-Rep CAD system where every geometric feature carries semantic meaning. AI agents can query design intent, check manufacturing constraints, find similar parts, and reason about engineering relationships directly from the geometry itself.

Built from scratch in Rust. No wrappers around OpenCASCADE or Parasolid.

![Roshera CAD UI](Front_ui.jpg)

## How It Works

The kernel has three layers:

**1. B-Rep Engine** — Standard boundary representation: vertices, edges, faces, shells, solids. NURBS curves and surfaces. Boolean operations, extrude, revolve, sweep, loft, fillet, chamfer.

**2. AI-Readable Layer** — Sits on top of B-Rep. A 4-pass classifier detects 20+ feature types (mounting holes, ribs, pockets, walls, bosses) and attaches semantic metadata:
  - Design intent: why the feature exists, what standard it follows
  - Manufacturing constraints: CNC machinable? 3D printable? Minimum tool diameter?
  - Engineering relationships: feature dependencies, mates, alignment
  - 64-dimensional geometric descriptors for similarity search

**3. Query Interface** — Structured queries for AI agents:
  - `find_features_by_type("MountingHole")` — all mounting holes in the model
  - `find_manufacturing_issues("injection_molding")` — features incompatible with that process
  - `find_thin_walls(2.0)` — walls thinner than 2mm
  - `describe_entity(face_id)` — "Cylindrical face, 8mm diameter, depth 12mm, likely M8 mounting hole"

## Architecture

```
roshera-backend/
  geometry-engine/     B-Rep engine + AI-readable semantic layer
  ai-integration/      API-based AI providers (Claude, OpenAI)
  timeline-engine/     Event-sourced design history with branching
  session-manager/     Multi-user collaboration with RBAC
  export-engine/       STL, OBJ, STEP, ROS export
  rag-engine/          Vamana-indexed retrieval for design knowledge
  api-server/          Axum REST + WebSocket API
  shared-types/        Common type definitions

roshera-front/         Leptos/WASM + Three.js browser client
```

## Getting Started

```bash
# Backend
cd roshera-backend
cargo run --bin api-server
# API on http://localhost:3000, WebSocket on ws://localhost:3000/ws

# Frontend (separate terminal)
cd roshera-front
trunk serve
# UI on http://localhost:8080
```

### Docker

```bash
cd roshera-backend
docker compose up
```

### Prerequisites

- Rust 1.75+
- trunk (for frontend): `cargo install trunk`
- wasm32-unknown-unknown target: `rustup target add wasm32-unknown-unknown`

## Capabilities

| Component | Description |
|-----------|-------------|
| **Primitives** | Box, Sphere, Cylinder, Cone, Torus |
| **Operations** | Boolean, Extrude, Revolve, Sweep, Loft, Fillet, Chamfer, Pattern, Blend |
| **Topology** | Euler operators, validation, non-manifold detection |
| **Sketching** | 2D constraint solver (Newton-Raphson), lines, arcs, splines |
| **Assembly** | Mate constraints, motion simulation |
| **Export** | STL (ASCII/binary), OBJ, STEP (ISO 10303), ROS (encrypted) |
| **History** | Event-sourced timeline with branching (not parametric trees) |
| **AI** | API-based providers (Claude, OpenAI), natural language commands |
| **Collaboration** | Multi-user sessions, RBAC, real-time sync via WebSocket |

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

## License

Dual licensed. Free for non-commercial use (research, education, personal projects). Commercial use requires a paid license.

See [LICENSE](LICENSE) for details. Contact 29.varuns@gmail.com for commercial licensing.
