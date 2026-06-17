# Hidden-line removal (HLR) — design note (#22, next slice)

Status: PLANNED, not started. Centerlines slice shipped (commit 4a97b56).

## Why this is the right approach
The analytic raytrace eye (`queries::raycast_solid`) already does exact
ray↔analytic-surface intersection with face-id + distance. HLR is then a
*visibility classification* on top of it — no new geometry math, fully sound,
and reusing the same machinery the perception layer is built on.

## Algorithm (per view)
1. Re-walk the solid's edges keeping the 3D sample points (like
   `project_solid_edges` but retain `Point3`, not just the projected `[f64;2]`).
   Dedup by `EdgeId`. Linear edges = 2 samples; curved = `DEFAULT_CURVE_SAMPLES`.
2. View direction `w` = third row of `view_matrix_for_projection` (the
   into-scene direction; verified: Front→(0,-1,0), Top→(0,0,-1)).
3. For each consecutive sample pair, take the 3D midpoint `M` and classify:
   - `back` = 2·(bbox diagonal) + 10 (origin clears the part).
   - `origin = M − w·back`, cast `raycast_solid(origin, w)`.
   - hidden iff `hit.distance < back − eps`, with `eps = diag·1e-5 + 1e-3`
     (the edge's own adjacent face returns `t ≈ back`; an occluder in front
     returns `t < back`).
4. Group consecutive same-visibility samples into runs → emit a `Polyline2d`
   per run into `visible` or `hidden`. This splits PARTIALLY-hidden edges at the
   crossover sample (mechanical convention).

## Wiring (additive, non-regressive)
- New `visibility.rs`: `ViewEdges { visible, hidden }` +
  `project_solid_edges_visibility(model, solid, projection, samples)`.
- Add `#[serde(default)] pub hidden_polylines: Vec<Polyline2d>` to
  `ProjectedView` (mirror the centerlines field; update the dxf.rs test
  literal + `project_solid_view` ctor with `Vec::new()`).
- Do NOT mutate `standard_drawing`'s wireframe behaviour in place. Add
  `standard_drawing_hlr(...)` (or a `hlr: bool` arg if callers are few — check
  api-server/mcp first) that sets `polylines = visible`, `hidden_polylines =
  hidden`. Keeps #20 wireframe path intact.
- SVG: add `.hidden { stroke:#111; stroke-width:0.18; stroke-dasharray:2 1.2; }`
  and render `hidden_polylines` dashed.

## Soundness harness (`tests/drawing_hlr.rs`)
- Opaque box, Front view: the 4 back edges classify HIDDEN, the front face
  edges VISIBLE. (A wireframe would show all solid — HLR must not.)
- Bored plate, Front view: the bore's far wall / hidden circle projects DASHED;
  the near silhouette stays solid.
- Determinism: same view twice → identical visible/hidden split.
- Recoverability is inherited (every classification is a real raycast hit).

## Known risks / caveats (carry into implementation)
- Cylinder u=0 seam edge: raycast seam caveat (#12/#13) can mis-occlude a seam
  edge. Acceptable for v1; note it. Probe the segment midpoint, not the seam
  vertex, which mostly dodges it.
- Silhouette edges grazing tangentially: `eps` must absorb the tangent fp
  wobble. Tune `eps` against the box+cylinder harness.
- Perf: O(edges · samples · faces) raycasts per view. Fine for drawing
  generation (not realtime). If a large part is slow, coarsen curved samples for
  HLR only.
- This is the slice most likely to be flaky — if classification proves
  unstable after tuning `eps`, DEFER-WITH-DIAGNOSIS rather than ship a drawing
  that lies (a wireframe that says "visible" for a hidden edge is a sound-eye
  violation).
