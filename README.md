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

<p align="center">
  <strong><a href="https://varun29ankus.github.io/Roshera-CAD/">Read the technical sheets →</a></strong>
  <br />
  <sub>The Claim · The Certificate · Casebook · Envelope</sub>
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

The differentiator is the readable surface: geometry is not just triangles, it is a queryable model with named features, intent, and history. And every operation an agent runs returns the kernel's **full validity certificate** — a nine-conjunct AND over brep-validity, watertightness, manifoldness, orientation, self-intersection, construction consistency, feature consistency, and two mesh-quality dimensions — computed synchronously under the model write lock and failing closed. The geometry carries its own proof instead of a render that merely looks right.

A second, render-based reconcile views each solid from fourteen viewpoints and cross-checks what it sees against what the kernel claims. That layer is **advisory**: it runs off the write lock and does not gate a mutation. The distinction matters, so it is stated here rather than blurred — see [The Certificate](https://varun29ankus.github.io/Roshera-CAD/certificate.html) for the anatomy, and [Status](#status) for what's actually usable today.

## Why a certificate matters

AI can generate CAD everywhere now. Nobody can trust what it generates — because the standard ways of checking are blind. We built a benchmark that injects four classes of silent geometric lie (flipped normals, self-intersections, torn seams, non-manifold facets) into sound parts and asks each verifier:

| Verifier | Lies caught |
|---|---|
| B-Rep validity check alone | **0 / 4** |
| "Mesh looks closed" heuristic | **2 / 4** |
| A frontier vision model, shown the render | misses the flagships |
| **Roshera's certificate** | **4 / 4** |

The benchmark is reproducible (`geometry-engine/tests/injected_defect_benchmark.rs`; every table cell is derived from measured verdicts at run time, never hardcoded). Two more artifacts back the same claim:

- **Adversarial predicate census** (`geometry-engine/tests/adversarial_predicate_census.rs`) — 12,000 near-degenerate point-in-polygon cases (edge-graze, sliver, arc-chord families) checked against a `BigRational` oracle. The shipped floating-point ray-cast variants flip sign on hundreds of them; the exact predicate core matches the oracle on every decided case — zero mismatches. This is why the certificate can afford to be exact where it counts.
- **Measured certificate cost** (`geometry-engine/benches/certificate_cost.rs`) — the certificate is cheap enough to run synchronously on the hot path: sub-millisecond cold on a box, a couple hundred milliseconds on an all-edges filleted cube, and a sub-microsecond cache read on any unmutated re-read. Numbers in [Performance](#performance).

The same certificate acts as the referee for autonomous design. A certified exploration harness (`geometry-engine/examples/certified_exploration.rs`) samples rocket-engine variants deterministically from a seed, lets the certificate kill every unsound one, and ranks the sound survivors by wall-material volume at a fixed internal-volume target — so the winner is provably the most material-efficient *sound* design in the sweep, not merely one that renders well. An optimizer without the certificate happily picks corpses; ours can't.

| Dark Mode | Light Mode |
|-----------|------------|
| ![Dark Mode](roshera-app/docs/screenshots/dark-mode.png) | ![Light Mode](roshera-app/docs/screenshots/light-mode.png) |

## Architecture

```
roshera-backend/       Rust workspace (10 crates)
  geometry-engine/     B-Rep kernel: exact predicates, NURBS math, primitives, topology, operations, tessellation, 2D drawings
  ai-integration/      LLM providers (Claude, OpenAI) + vision pipeline
  timeline-engine/     Event-sourced design history with branching
  session-manager/     Multi-user RBAC + Argon2 auth + durable API keys
  assembly-engine/     Instances, mates, SE(3) solve, DOF, certified interference
  export-engine/       STL, OBJ, STEP (AP242 export + import, round-trip tested), encrypted .ros (AES-256-GCM)
  ros-format/          Native .ros serialization
  verdict-harness/     Multi-agent verdict/consensus harness
  api-server/          Axum REST + WebSocket API
  shared-types/        Common type definitions

roshera-app/           React + Three.js + TypeScript browser client
roshera-mcp/           MCP server exposing the kernel to agents
roshera-eval/          Certificate-scored agent evaluation scenarios (17)
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
verified sound and watertight before the next step. The timeline already
answers provenance questions: `GET /api/evidence-pack` bundles a
document's recorded history, its per-operation certificates, and the
design notebook into one reviewable artifact. Next is embeddings and
retrieval over that history so an agent can ask "how did this corner get
to be 4mm" without re-deriving the model from scratch.

## Status

What follows is honest about what's tested versus what's implemented but rough. Measured perf numbers are in [Performance](#performance). The kernel carries a red-test ratchet (`geometry-engine/KNOWN_REDS.md`) whose tolerated-failure allowlist is currently empty — no known-failing test is carried as accepted.

| Layer | Component | Status |
|-------|-----------|--------|
| **Math** | Vector3, Matrix4, Quaternion | Tested |
| | B-spline, NURBS evaluation | Tested |
| | Exact predicates (orient2d, point-in-polygon, shoelace sign) | 12,000-case adversarial census vs a `BigRational` oracle — zero mismatches where the float ray-cast variants flip sign |
| **Primitives** | Box, Sphere, Cylinder, Cone, Torus | B-Rep topology with Euler validation |
| **Topology** | Manifold detection, adjacency | Tested |
| **Tessellation** | Per-surface dispatch, adaptive subdivision | Watertight for analytic surfaces and extruded/polyline curved profiles; residual non-watertight welds remain only on curved∖curved boolean corefinement (e.g. cross-drilled cylinders) |
| **Operations** | Extrude (draft, taper, twist) | Implemented; side-faces sound and watertight, including partial-arc walls as trimmed cylinder faces |
| | Boolean (union, intersect, difference) | Works, incl. shared-imprint corefinement for NURBS × analytic cuts; coincident-face weld has known ordering sensitivity |
| | Fillet (constant-radius) | Works; unsupported vertex-blend corners return a structured typed refusal (the reason, not a wrong blend) |
| | Chamfer | Works; closed-edge rim chamfers are **plane–cylinder only** — cone/torus/revolve-seam rims return NotImplemented |
| | Offset, Sewing | Implemented, lightly tested |
| | Revolve (full/partial) | Works; piecewise-analytic bands (arc → exact Torus/Sphere) on the typed strict path, 360° profiles without inner loops |
| | Sweep (single path) | Implemented; multi-guide not done |
| | Loft (ruled surfaces) | Implemented; smooth NURBS loft not done |
| **Certificates** | Soundness (per-operation) | Nine-conjunct AND, computed synchronously under the write lock on every mutating-op response, memoized per solid, failing closed |
| | Sketch — DOF, conflict witnesses | Minimal conflicting constraint sets via QuickXplain; honest `minimal: false` rather than a fabricated core |
| | Rebuild (timeline mould) | Per-feature verdicts; global soundness re-measured from the resulting B-Rep |
| **Sketch 2D** | Constraint solver (Newton-Raphson) + DR-plan | Works; per-entity constrainment exposed as queryable kernel facts. Parametric sketch (`psketch_*`) drives the same solver over the agent surface |
| **Drawings** | Certified 2D sheets | Multi-view layout, SECTION A-A, hole tables, GD&T feature-control frames resolved against disclosed datum reference frames, and `drawing_export_sheet` to PDF/DXF/SVG — all derived from the live B-Rep |
| **Perception** | Spatial-relationship queries | Ordered ray crossings with exact hit points, SDF X-ray occupancy slices (`occupancy_view`, non-deceivable), view-coverage honesty (`part_coverage` — which faces the standard views leave unseen), section cutaways — an agent asks instead of parsing meshes |
| **Identity** | Persistent IDs + labels | `FacePid` carries durable face identity across re-extrude and timeline replay (fillet/chamfer/pattern lineage not yet minted); labels pin names to faces/edges/planes and resolve-or-refuse (never a wrong entity) |
| **Durability** | Event-log persistence, replay, quarantine | Works; boot is a full replay (boot-time snapshots not used). Verified live by kill + resurrect. `GET /api/evidence-pack` bundles recorded history + certificates (absent → null with reason, never fabricated) + notebook + provenance-labeled mass properties |
| **Agent surface** | MCP minimal surface + meta-tool funnel | Works; 21 default tools (18 core verbs + `find_tool`/`describe_tool`/`invoke`), the ~90-tool long tail reachable via `invoke` at fixed cost with identical schema validation. `blackboard_add_entry` (agent→human notebook) is in the default surface |
| **Assembly** | Instances, mates, SE(3) solve, interference | Gauss-Newton SE(3) solver with an analytic Jacobian, DOF/mobility analysis, interactive drag, and a dozen-plus mate/joint kinds (Cam/Path/Symmetric are typed but not numerically enforced). Certified interference: static overlap (EPA-backed, enclosure-aware) plus continuous nonlinear time-of-impact swept clearance via Parry — `min_clearance = raw_min_clearance − ε`, a conservative lower bound, with a typed refusal when a pair's distance is unsupported |
| **Export** | STL, OBJ, encrypted .ros | Works |
| | STEP (AP242) | Export and import implemented (tiered writer + parser); primitive and blended-flange round-trips assert topology, validity, and soundness are preserved |
| **AI** | Claude + OpenAI providers | Works |
| | Vision pipeline | Implemented |
| | Natural language command parsing | Works for common commands |
| **Infrastructure** | Timeline (event-sourced history) | Works. `EventCertificate` projection is defined and honestly reported (per-op-class-honest), but no producer records it on the timeline yet — the evidence-pack reports it as absent rather than inventing one |
| | Session manager (multi-user, RBAC) | Works |
| | Blackboard (agent↔human notebook) | REST + MCP + live frontend panel |
| | Auth posture | Secure by default (auth enforced on an empty environment; bypass requires the explicit, loudly-logged `ROSHERA_DEV_INSECURE=1`). Argon2 password auth; API keys persist across restart with a full provision/list/revoke lifecycle (revoked stays revoked). Residual: timeline authorship is taken from the request body, so an authenticated caller can mislabel who authored an event |
| **Frontend** | React + R3F viewport, toolbar, chat | Works; rough around the edges |

For a capability map maintained against the source tree rather than intent — including the four
places a check is deliberately narrower than its name — see
[Envelope](https://varun29ankus.github.io/Roshera-CAD/envelope.html).

## Performance

Numbers below are labeled with the date and machine they were taken on. Criterion, release/bench profile (`CARGO_PROFILE_BENCH_LTO=off`, `CARGO_PROFILE_BENCH_CODEGEN_UNITS=16`), median estimate. These are internal regression figures, not comparisons against any third-party kernel. Reproduce with the commands at the end. Full detail in [BENCHMARKS.md](BENCHMARKS.md).

### Certificate cost (`certificate_cost`)

The nine-conjunct soundness certificate, computed synchronously under the write lock. **Cold** is the price paid once per mutating op on an empty cache; **memoized** is the cache read on any unmutated re-read. Measured **2026-07-21 on a quiet machine** (i7-1355U).

| Solid | Cold (per mutation) | Memoized re-read |
|-------|---------------------|------------------|
| Box (6 faces) | 0.29 ms | < 1 µs |
| Bored boolean (f7 straddling bore: union + off-centre through-bore) | 38.5 ms | < 1 µs |
| Filleted cube (all edges, r3) | 224.5 ms | < 1 µs |

The cold cost scales with topological complexity; memoization collapses every unmutated re-read to under a microsecond, which is what makes a synchronous certificate on the hot path affordable.

### Math microbenchmarks (`geometry_bench`)

Measured on an earlier quiet run; **re-measurement pending a quiet machine**.

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

### Primitive creation (`geometry_bench`)

Full B-Rep topology construction with a fresh `BRepModel` per iteration. Measured on an earlier quiet run; **re-measurement pending a quiet machine**.

| Primitive | Time |
|-----------|------|
| Box | 65 µs |
| Sphere | 49 µs |
| Cylinder | 50 µs |

### Tessellation and Boolean (structure of the current suites)

The older synthetic rows — a flat "1M-triangle" tessellation estimate and "1k-face" Boolean timings — were retired as measurement concepts when their harness was replaced. The current tree measures:

- **Tessellation** (`tessellation_bench`) — per-surface (box, sphere, cylinder, cone, torus, curved NURBS) at **coarse / default / fine** chord-tolerance tiers, rather than one extrapolated triangle count.
- **Boolean** (`boolean_classification`) — named real parts (box∪box overlap, coincident boss + cap merge, 3-prism chain, box−cylinder poke, straddling f7 bore, box∩cylinder) so timings track the corefinement paths the kernel actually exercises, not a synthetic face count.

Fresh timings for both are omitted here rather than published from a loaded machine; run the commands below on a quiet host to reproduce.

### Coverage gaps

- **NURBS surface eval and memory-per-vertex** — no timing bench in the current tree; the old estimates were retired with the internal suite.
- **Delete primitives** — correctness tests only (`delete_solid`, `delete_face`, cascade, orphan cleanup); no Criterion target.
- **2D sketch creation (sketch2d)** — a large subsystem with a broad passing correctness suite (185 integration + 338 unit tests after the constraint-solver campaign); the `sketch_solver` bench times the solve step, not sketch creation.

### Reproduce

```bash
cd roshera-backend
CARGO_PROFILE_BENCH_LTO=off CARGO_PROFILE_BENCH_CODEGEN_UNITS=16 \
  cargo bench -p geometry-engine \
  --bench geometry_bench --bench certificate_cost \
  --bench tessellation_bench --bench boolean_classification
```

## Getting Started

```bash
# Backend
cd roshera-backend
cargo run --bin api-server
# API on http://localhost:8081, WebSocket on ws://localhost:8081/ws

# Frontend (separate terminal)
cd roshera-app
npm install
npm run dev
# UI on http://localhost:5173 (proxies /api and /ws to localhost:8081)
```

### Docker

```bash
cd roshera-backend
docker compose up
```

### Prerequisites

- Rust 1.75+
- Node.js 20.19+ (Vite 8 requirement)

## API

```bash
# Create a box (returns the tessellated mesh + its soundness certificate)
curl -X POST http://localhost:8081/api/geometry \
  -H "Content-Type: application/json" \
  -d '{"shape_type": "box", "parameters": {"width": 10, "height": 10, "depth": 10}, "position": [0, 0, 0]}'
```

```javascript
// WebSocket
const ws = new WebSocket("ws://localhost:8081/ws");
ws.send(JSON.stringify({
  type: "GeometryCommand",
  data: {
    command: {
      cmd: "CreatePrimitive",
      primitive_type: "Sphere",
      parameters: { params: { radius: 5.0 } }
    }
  }
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
