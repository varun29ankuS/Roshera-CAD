# roshera-mcp

MCP server exposing the Roshera geometry kernel to agents — the bridge that
makes any MCP client (Claude Code, Claude Desktop, Cursor, …) a Roshera
client with zero integration work.

## What it provides

**Perception**
- `render_part` — deterministic offscreen render returned as **image
  content** directly in the tool result. `mode: "ids"` paints each B-Rep
  face a distinct flat color and returns the color→face_id legend
  (set-of-marks for topology). `depth` / `normals` are exact G-buffer
  channels.
- `get_pointer` — what the human is pointing at in the viewport (click →
  face id + hover report). Grounds "this face / here" in conversation.
- `get_part`, `get_face`, `mass_properties`, `list_parts` — structured
  inspection (world placement, curvatures, exact inertia).

**Modeling**
- Composite one-call creators: `create_box`, `create_cylinder`,
  `create_plate_with_holes` (explicit coordinates, placement echoed back).
- Sketch primitives for everything else: `create_sketch`,
  `sketch_add_shape`, `sketch_points` (batch — a 96-point gear outline is
  one call), `sketch_extrude`, `plane_from_face` (stack features on faces).
- `delete_part`, `clear_parts`.

## Setup

```sh
cd roshera-mcp && npm install && npm run build
```

Registered for this repo in `.mcp.json` (project scope). Requires the
api-server running on `ROSHERA_URL` (default `http://localhost:8081`).

## Surface modes & the scale funnel

A real CAD surface carries hundreds of operations. Paying context for every
tool on every conversation does not scale — and the worst-case MCP client
(no `list_changed`, no deferred schemas) injects the *entire* exposed surface
each turn. So the **default exposed surface is minimal-complete**, and the long
tail stays reachable at fixed cost through a three-tool funnel.

- **`ROSHERA_MCP_SURFACE=minimal`** *(default)* — the 15 core modeling/perception
  verbs plus the 3 meta-tools = **18 tools, ~4.1k tokens**. Everything else lives
  in the internal table, reachable via the funnel.
- **`ROSHERA_MCP_SURFACE=full`** — restores the full **90-tool** direct exposure
  (**~19.4k tokens**); the meta-tools are omitted since the whole surface is
  present. This is the transition escape hatch, not the recommended default.

**The funnel (fixed ~500-token cost, reaches the whole long tail):**
- `find_tool(intent)` — deterministic ranked search over the registry.
- `describe_tool(name)` — full schema + usage notes on demand.
- `invoke(name, args)` — executes any registry tool, validating args against the
  tool's *own* schema first, so a meta-path call is never less checked than a
  direct call.

Benches optimize attention; they never gate capability — `invoke` reaches
everything, always. (Spec: `2026-07-20-mcp-scale-architecture-design.md`.)

## CI gate — the jig ratchet

`roshera-mcp/**` changes are graded in CI (`mcp-quality` job in
`.github/workflows/ci.yml`). The report card is a permanent invariant, like the
fmt gate. Two graders run:

- **`tools/mcp_budget_gate.py`** *(authoritative)* — speaks minimal MCP stdio to
  the built server on both surfaces and asserts the spec §5 Q5 budgets:
  - minimal live-surface bill **≤ 8000 tokens**,
  - **schema hygiene = 100** — every param in every exposed tool has a
    description (a `const`/single-`enum` discriminant is self-describing),
  - per-tool tokens **≤ 3.5× the full-set median**, except a named allowlist of
    wire-contract-fixed schema-heavies (`revolve`, `assembly_mate`,
    `drill_pattern`). (The spec's starting 2× line false-positives ~10
    legitimately-sized multi-param tools on the current right-skewed surface;
    3.5× isolates exactly the three genuine outliers while staying a live
    ratchet — see the script header for the calibration.)

  Tokens use tiktoken (`o200k_base`) when installable, else the registry's
  `chars/4` estimate. Run it locally with:

  ```sh
  cd roshera-mcp && npm run build && python tools/mcp_budget_gate.py
  ```

- **jig** (github.com/Shodh-Labs/jig) — the upstream MCP report-card grader, run
  best-effort and **non-blocking**: it is not published to PyPI under a
  verifiable name (`pip install jig` resolves to an unrelated git-hook tool), so
  CI attempts the real source, verifies the installed CLI is actually the grader,
  and skips loudly otherwise. The fallback gate above holds the line regardless.

## Known kernel issues surfaced through this API

- `#24` — extruded circles tessellate as ~64 planar strips, not one
  analytic cylinder face (`get_face` on a bore reports `plane`).
- `#26` — `clear_parts` can wedge the model (subsequent extrudes fail
  until server restart); prefer `delete_part` loops meanwhile.
- Part ids renumber after deletion — always re-`list_parts` before
  follow-up deletes (the persistent-ID work, task #11, fixes this class).
