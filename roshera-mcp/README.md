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

## Known kernel issues surfaced through this API

- `#24` — extruded circles tessellate as ~64 planar strips, not one
  analytic cylinder face (`get_face` on a bore reports `plane`).
- `#26` — `clear_parts` can wedge the model (subsequent extrudes fail
  until server restart); prefer `delete_part` loops meanwhile.
- Part ids renumber after deletion — always re-`list_parts` before
  follow-up deletes (the persistent-ID work, task #11, fixes this class).
