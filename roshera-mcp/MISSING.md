# Roshera MCP — gaps found while dogfooding (live build session 2026-06-15)

Running notes of what's missing/broken in the MCP tool surface, found by
actually trying to build parts through it. Fix top-down.

## 1. create_* tools don't return object_uuid  → BLOCKS boolean/transform  [FIXING]
`create_box`, `create_cylinder`, `create_cone`, `create_sphere` return only
`{ part_id, placement }`. But `boolean` and `transform` require an
`object_uuid` (the viewport object id, a v4 UUID — NOT the kernel part id).

The backend ALREADY returns it: every create endpoint
(`POST /api/sketch/{id}/extrude`, `POST /api/geometry`, `POST /api/geometry/cone`)
responds with `object.id` = the registered UUID (see api-server `state.register_id_mapping`).
The MCP handlers just throw the response away and re-derive `part_id` via
`newestPartId()`.

Effect: a part built with the one-call create tools is un-composable —
there is no way to get its UUID, so it can never be a boolean operand.
`psketch_extrude` and `boolean` already return `object_uuid` correctly;
the primitives are the odd ones out.

Fix: capture the create response and return `object_uuid: r.object?.id`.
No backend change needed. (Done in this session.)

## 2. No part_id → object_uuid lookup endpoint  [OPEN, backend]
After a restart the agent only knows part ids (from `list_parts`). There is
no way to recover an object's UUID, so existing parts can't be composed.
`AppState::get_uuid(local_id)` exists in the backend but is unexposed.
Proposed: `GET /api/agent/parts/{id}/uuid` → `{ uuid }`, or just add `uuid`
to each `PartSummary`/`PartReport` in the agent listing. Requires an
api-server rebuild, so deferred — note it and move on.

## 3. clear_parts does NOT fully reset kernel state  [OPEN, backend]
A *failed* op leaves stale/half-built entities behind. After a revolve fails
validation, the NEXT (valid) revolve also fails with phantom
`ConnectivityError "Boundary edge N detected — potential gap"` (saw 21 errors),
even the exact profile that succeeded moments earlier. `clear_parts` (deletes
parts) does not clear it; only `clear_timeline` (full model wipe, "events_removed:40,
model_reconciled:true") restores a clean slate, after which the same revolve
succeeds. So: failed ops poison subsequent revolves. Either roll back fully on
failure, or have clear_parts also reconcile the kernel entity stores.
Workaround for now: clear_timeline (not clear_parts) between builds.

## 4. section_view returns 404 on revolve-produced solids  [OPEN, backend]
`GET /api/agent/parts/{id}/section` → `render_section` returns `None` (→404) for
the revolved flanged housing, BOTH axial normals ([1,0,0] and [0,1,0]) through
the part center. A plain extruded box sections fine (area exact). So the bug is
specific to revolve-built geometry (curved/analytic lateral + revolved caps) —
the section's plane∩solid arm drops the loop. Part of the known SECTION #85
family. Repro: revolve the housing profile, warm its cache, then section.

## 5. Chained primitive booleans don't yield a watertight flanged housing  [OPEN, kernel]
Two realistic build recipes, two different boolean failures:
 (a) all-primitive: union(flange,body) then difference bore, counterbore, 6 bolts
     → LOOKS perfect but 192 open + 64 non-manifold edges, all at the COAXIAL
     counterbore-over-bore step + bore/flange-bottom — the annular-cap case,
     boolean #27 (diagnosed, fix pending).
 (b) revolve body + boolean the 6 bolt holes into the revolved flange
     → body watertight, but differencing small cylinders into the densely-
     faceted revolved annular flange face SHREDS it (4350 open edges from 6 holes).
The watertight path that works TODAY: full housing as ONE revolve (flange+body+
bore+counterbore), no booleans → 0/0, closed manifold. Bolt holes remain the gap
(can't add them without hitting (a) or (b)). A hole-aware EXTRUDE of the flange
(central bore + 6 bolts, one op) IS clean (2072 tris, no booleans) — so the
missing primitive is a robust union of that extruded flange with the revolved
tube (the tube revolve itself needs finding #3's clean-state to validate).


## N. Cylinder surface closest_point blind spots (found by #15 spatial-query harness) [OPEN, kernel]
The spatial-query core (`queries::nearest_on_solid` / `signed_distance`) is exact
against analytic SDFs for box/sphere/cylinder-wall, EXCEPT two `Cylinder`
`closest_point` degeneracies surfaced by the #15 harness:
- **Axis point (ρ=0):** a point exactly on the cylinder axis has no defined
  radial projection (any angle), so nearest falls back to a cap instead of the
  (nearer) wall. e.g. point on axis at mid-height of an r=10 cyl reports distance
  to cap, not 10 to the wall.
- **Seam (u=0, +X) projection:** a radial probe whose nearest wall point sits on
  the u=0 seam is rejected by `point_inside_face_uv` (the seam is excluded from
  the trim), so nearest again falls to a cap. Off-seam probes (+Y etc.) are exact.
Both are the SAME root family as the raycast seam caveat (#12/#13). A real fix =
treat the u=0 seam as in-trim for closest-point/containment (periodic wrap), and
special-case the axis-point as "nearest = wall at distance r". Deferred: low-impact
(measure-zero probe locations), and the seam fix touches the shared trim predicate
used everywhere, so it needs a careful pass. The #15 harness pins both by probing
off-seam/off-axis and documenting the exclusion.
