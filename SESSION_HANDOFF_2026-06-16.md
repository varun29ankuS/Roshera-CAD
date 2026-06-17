# Session handoff — 2026-06-16 (overnight "Sound Agent Eye" campaign)

Branch: `harness-1000-sweep`. All work committed and green. No pushes (local only).

## What this session was
An autonomous overnight run of the **Sound Agent Eye** campaign: build the agent's
SOUND perception/spatial substrate — read from analytic surfaces, never the mesh;
every output recoverable to `(entity, world-xyz)`; verification automatic. 16-task
TaskList, self-paced `/loop`, commit green / defer-with-diagnosis.

## Shipped tonight (commits, oldest→newest on this branch)
- `8b889f4` #14 spatial-query slice 1 — **point** primitive (classify inside/outside/on
  via ray-parity + `nearest_on_solid`; through-hole reads Outside). `queries::point`.
- `d0ac573` #14 slice 2 — **field** primitive (`signed_distance` + `sample_field` grid;
  negative inside; each sample recoverable to a world-xyz node + nearest face).
- `d6fcbb7` #14 slice 3 — **region** primitive (`faces_in_box` / `faces_in_sphere`;
  edge-curve-sound face AABBs + sphere envelope). `queries::region`.
- `5e58af9` #14 slice 4 — **relational** primitive (coaxial / parallel / perpendicular
  axes + `coaxial_clusters`; reads cylinder/cone axis + plane normal). `queries::relational`.
- `c9a2617` #15 — **spatial-query core harness** (`tests/spatial_query_core.rs`):
  cross-primitive consistency + closed-form box/sphere/cylinder SDF exactness. Found +
  documented two `Cylinder::closest_point` blind spots (axis-point degeneracy + u=0
  seam projection) — logged in `roshera-mcp/MISSING.md`, probed off-axis/off-seam.
- `4a97b56` #22 — **drawing centerlines** (analytic chain-line axes + centre marks for
  cylindrical features; coaxial-deduped; ISO-128 dash-dot). `drawing::centerlines`.
- `8129585` #22 — HLR design note (`drawing/HIDDEN_LINE_REMOVAL_PLAN.md`).
- `78db84c` #22 — **hidden-line removal** via the raytrace eye (`drawing::visibility`):
  occluded edges dashed, sound per-segment ray↔surface visibility, partial-edge split,
  `standard_drawing_hlr` (additive; #20 wireframe untouched). `e8598a4` import cleanup.

(Earlier in the same continuous session, before this file's window: #12 raytrace eye,
 #13 raytrace soundness harness, #1–8/#10/#11/#20 — see prior handoffs + git log.)

## ★ Follow-on (same day, interactive): #21 re-diagnosed → #19 + #9 SHIPPED
Varun: "tackle #21 tessellator." Investigated and **disproved the #21 hypothesis**:
- `5651141` test(#21): primitive shared-edge watertightness battery
  (`tests/primitive_tess_watertight.rs`) — standalone AND boolean-trimmed
  sphere/cone/torus (incl. a planar face sharing a circle edge with a curved
  face) are watertight at all chord tolerances. The tessellator was innocent;
  the cache + planar + curved-CDT paths already sample shared edges bit-exactly.
- The REAL blocker was revolve's reverted analytic path giving each band its OWN
  circle copy (different EdgeIds → cache samples independently → gaps),
  misattributed to the tessellator.
- `5ce82b0` feat(#19/#9): analytic Cylinder + annular-Plane bands (shared ring-
  circle EdgeIds, mirrors create_cylinder_topology; self-check + grid fallback =
  zero regression). 48-seg tube = 4 faces not 192. **Unblocked #9** — section
  tube x/y now pass (the "404 bug").
- `e1db4fe` feat(#19 v2): analytic Cone bands for sloped edges (frustum tube =
  2 Cone + 2 Plane). Revolve now emits the full Cylinder/Cone/Plane band set.
- `8bb38c9` docs: corrected the REVOLVE_ANALYTIC_BANDS_PLAN.
Gates all green: revolve_analytic_faces 3/3, section_revolve tube 2/2,
revolve_watertight 7/7, revolve_volume 14/14, lib revolve 32/32. Only a profile
vertex ON the axis (r→0 disc/apex) still uses the watertight grid fallback (v3).
**#9, #19, #21 all COMPLETE.**

## Campaign state
DONE: #1–15, #19, #20, #21, #22. (#16, #17 remain — see below.)
The **spatial-query core** (`geometry-engine/src/queries/`) now has all five composable
primitives — ray (`raycast`/`raytrace`), point, field, region, relational — each
analytic-sound and recoverable. This is the inhabitable substrate: the agent can ask
point/field/region/relational questions of the space, all verified against closed-form
truth. The **drawing module** is now mechanical-grade: auto-dimensions (#20) + centerlines
+ hidden-line removal, all rendered through the SVG pipeline.

## Remaining (only two — both genuinely need Varun's steer)
- **#16 set_dimension / mould verb** — the composable form needs persistent IDs (#11,
  the parametric-timeline-hybrid prereq) so re-evaluation can remap face/edge refs.
- **#17 live Three.js viewport eye** — frontend/transport plumbing (shared camera pose →
  backend raytrace). Backend half is DONE (`raytrace_ortho` takes any camera basis);
  Varun flagged the live-camera wiring as a later/explore item.

(#9, #19, #21 — formerly this cluster — are now DONE; see the follow-on section above.)

## Open follow-ups (small, user-gated)
- #22 per-dimension **tolerances**: needs user spec (not autonomously derivable; the
  general ISO-2768-m note already renders). Easy to add once Varun defines the tol model.
- Two `Cylinder::closest_point` blind spots (axis + seam) in `MISSING.md` — low-impact
  (measure-zero probe locations); the seam fix touches the shared trim predicate, so it
  wants a careful pass alongside the #12/#13 seam caveat.

## How to verify
`cargo test -p geometry-engine --lib queries::` and `--lib drawing` (lib unit tests),
plus integration: `--test spatial_query_core --test drawing_centerlines --test drawing_hlr`.
All green at handoff.
