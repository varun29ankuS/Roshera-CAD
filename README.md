# Roshera

**AI-native CAD engine — B-Rep geometry kernel built from scratch in Rust.**

Roshera is a boundary representation CAD system with an LLM-driven design workflow, production-grade NURBS mathematics, and a proprietary encrypted file format (.ros) with AI provenance tracking. No wrappers around OpenCASCADE or Parasolid — every line of the geometry kernel is original.

The geometry kernel has been through three rounds of topological audit with all critical and high-severity issues resolved. Math primitives, topology, and operations are hardened against division-by-zero, NaN propagation, and degenerate geometry.

| Dark Mode | Light Mode |
|-----------|------------|
| ![Dark Mode](roshera-app/docs/screenshots/dark-mode.png) | ![Light Mode](roshera-app/docs/screenshots/light-mode.png) |

## Architecture

```
roshera-backend/
  geometry-engine/     B-Rep kernel: math, primitives, topology, operations, tessellation
  ai-integration/      API-based AI providers (Claude, OpenAI)
  timeline-engine/     Event-sourced design history with branching
  session-manager/     Multi-user collaboration with RBAC
  export-engine/       STL, OBJ, encrypted .ros (AES-256-GCM, AI provenance)
  rag-engine/          Vamana-indexed retrieval for design knowledge
  api-server/          Axum REST + WebSocket API
  shared-types/        Common type definitions

roshera-app/           React + Three.js + TypeScript browser client
```

## What Works Today

| Component | Status | Notes |
|-----------|--------|-------|
| **Math** | Production | Vector3, Matrix4, Quaternion, B-spline, NURBS — hardened against singularities |
| **Primitives** | Production | Box, Sphere, Cylinder, Cone, Torus — real B-Rep topology with dihedral angles |
| **Topology** | Production | Euler validation (V-E+F=2), manifold detection, parallel adjacency, real dihedral angles |
| **Tessellation** | Production | Per-surface-type dispatch, adaptive subdivision, proper UV mapping |
| **Extrude** | Production | Face and profile extrusion with draft angle, taper, twist, and scaling |
| **Boolean** | Working | SSI via marching, 3-ray face classification, topology reconstruction |
| **Fillet** | Working | Constant-radius (cylindrical/toroidal/spherical) + variable-radius (NURBS) |
| **Chamfer** | Working | Distance and angle-based chamfers with robust edge lookup |
| **Offset** | Working | Plane, cylinder, sphere, cone, torus, NURBS surface offsets |
| **Sewing** | Working | Topology repair: edge/vertex matching, shell stitching, manifold validation |
| **Revolve / Sweep / Loft** | In development | Pipeline structured, surface generation incomplete |
| **Assembly** | Data model | Components, mates, motion limits defined; constraint solver not started |
| **2D Sketch** | Partial | Newton-Raphson solver loop working; constraint coverage expanding |
| **Export** | STL + OBJ + ROS | Encrypted .ros with AI provenance; STEP writer not yet valid |
| **RAG Engine** | Working | Vamana/DiskANN vector index with cosine/euclidean/dot-product |
| **Timeline** | Working | Event-sourced history with branching |
| **AI Integration** | Working | Claude + OpenAI API providers, natural language command parsing |
| **Frontend** | Working | React + R3F viewport, AI chat, toolbar, model tree, properties panel |

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

## License

Dual licensed. Free for non-commercial use (research, education, personal projects). Commercial use requires a paid license.

See [LICENSE](LICENSE) for details. Contact 29.varuns@gmail.com for commercial licensing.
