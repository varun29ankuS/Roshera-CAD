# Session Handoff — 2026-06-15

Branch: **`harness-1000-sweep`** (pushed to origin). `main` backlog (23 commits) also pushed.
Everything below is committed + on GitHub.

## ▶ START HERE
This session pivoted from boolean bug-fixing → **revolve + the rocket-engine
dogfood** → an **architecture decision** (parametric/timeline hybrid) → the
**MCP** as the real product surface. Read this, then `KNOWN_BUGS.md` and the
memory files `parametric-timeline-hybrid.md`, `revolve-tessellation-cone-bands.md`,
`cone-rim-seam-alignment.md`.

## Shipped this session (commits, all on `harness-1000-sweep`)
- `ae1c8ad` BOOL #27/#32 **cone rim-seam**: cone primitive seam vertex now =
  `Circle::x_axis()` → the offset/stacked cone∪cylinder "rocket" welds (was a
  279-open husk with correct volume). Flipped coincident-base pin to a live gate.
- `7af8e4e` BOOL #27/#32 **closed-circle rim weld** in `canonicalise`: a
  frustum∪frustum throat (two coincident full-circle rims) now welds (was
  watertight-looking but odd-Euler invalid).
- `398606d` SECTION #85c: cutaway through a periodic **seam** no longer drops a
  generator (was direction-dependent / 0 caps).
- `ca7a684` API `POST /api/geometry/cone` (frustum + placement).
- `27d053c` **REVOLVE-TESS**: cone/sloped revolve bands were non-watertight
  (open edges scaled with density) → triangulate the wedge in (u,v) param space
  from exact boundary cache samples. Gate `tests/revolve_watertight.rs` (7 cases).
  Also adds `POST /api/geometry/revolve`.
- `fa26c18` REVOLVE smooth per-vertex normals (kill faceted "rectangles" on
  sloped bands).
- `8733df4` **MCP** tools: `revolve`, `create_cone`, `create_sphere`,
  `section_view` added to roshera-mcp.
- KNOWN_BUGS updates (`03a575d`, `078a6ae`, + this session's additions).
- Full geometry-engine lib gate green throughout: **3724 passed / 0 failed**.

## Architecture decision (Varun, locked) — task #64, memory `parametric-timeline-hybrid.md`
**Timeline model + parametric tree for certain things (hybrid).** Parametric =
a dependency-DAG VIEW + re-eval policy layered ON the event timeline, NOT a
replacement. Both re-eval modes wanted: dirty-subtree (interactive edit) +
replay-from-point (scrub/branch). This is THE engineer/agent edit→re-evaluate→
optimize loop. **Prerequisite: persistent entity IDs (task #11) — build FIRST.**
Build sequence in the memory file. `/api/geometry/revolve` currently bypasses
sketches (no editable feature) — engine should move to sketch→revolve.

## MCP — the product surface (task #22)
`roshera-mcp/` is a TS proxy (@modelcontextprotocol/sdk + zod) over REST :8081.
`.mcp.json` wires `node roshera-mcp/dist/index.js` → `ROSHERA_URL=:8081`. `dist`
is built with the new tools. **On Claude restart the MCP auto-connects — BUT it
proxies to the api-server, which must be running.** To make tools work:
```
cd roshera-backend
ROSHERA_DEV_BRIDGE=1 ./target/demo/api-server.exe    # binds 0.0.0.0:8081
```
(rebuild it with `cargo build -p api-server --profile demo` after kernel changes).
Tools now: create_box/cylinder/cone/sphere, revolve, boolean, create_sketch +
sketch_*, transform, timeline_scrub, render_part, verify_part, section_view,
list_parts, get_part (dims), mass_properties, delete_part/clear_parts, get_pointer.
**Next MCP slice (task #22): `export_stl/step`, `fillet`/`chamfer`, and a
`report` tool = the dimensioned engineering PDF.**

## What was built (in-memory model — dies on server restart; rebuild scripts in /tmp)
- **real_rocket_engine** — one revolve: parabolic bell nozzle (ε=16, exit M=3.6),
  chamber, filleted throat, convergent, mounting flange, external cooling ribs,
  vent-bored injector dome. watertight+valid, dims 52×52×84.5, 295k tris.
  Idealized thrust ≈ 180 N @ 20 bar Pc (vacuum), scales with Pc; ε=16 = vacuum/
  upper-stage bell. (`/tmp/real_engine.py`)
- **engine_mount_bracket** — thrust plate 110×110×12, center bore Ø30, 8× bolt
  circle @Ø44 (matches engine flange Ø52), 4× corner mounts. watertight+valid.
  Built via the sketch-region path (`/tmp/bracket2.py`).

## Open findings / tasks
- #61 🔴 BOOL determinism: rbox-diag45 Intersection non-deterministic (10th digit), PRE-EXISTING.
- #62 fillet/chamfer the engine (via API) — not done.
- #64 PARAMETRIC-HYBRID campaign (blocked by #11 persistent IDs).
- #65 🔴 box∪cylinder boss + chained bores → non-watertight (use sketch-regions instead).
- 🔴 REVOLVE axis-touch: domes/spheres/solid-cones with a pole reject or go non-watertight (workaround: pole vent bore).
- 🔴 mass-properties errors on a hollow revolve ("Loop < 3 vertices" — closed circular cavity loop).
- 🔴 oblique vertical-plane section (nz=0, off-seam) still 0 caps (separate marching limitation).
- ⏳ Pending user asks: dimensioned engineering PDF on Desktop (→ MCP `report` tool); fit engine onto bracket.

## Working directives (unchanged)
slow-is-smooth; NEVER rush boolean/tess core; FULL gate (lib 3724/0 + poke +
suites) before claiming a core fix; ATTRIBUTE-via-`git stash` if a gate reddens;
validate_solid_scoped (B-Rep) AND manifold_report (mesh) — both, not one;
prefer sketch-regions over boolean subtraction for plates-with-holes;
push back, don't amplify.
