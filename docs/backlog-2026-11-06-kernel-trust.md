# Kernel trust backlog — 2026-11-06

The complete list of work found in the 2026-11-05/06 sessions: STEP import, the
booleans that build cones/tori, tessellation, the SDF deep look, and the
sketch2d audit. Cadence (Varun): **two items a day**, in the order below. Every
item lands RED-first (a test that failed before the fix), behind a red-gate run
over the affected binaries, and gets a ✓ with its commit here.

Severity: **C** the kernel can report sound/solved/inside when it is not ·
**H** wrong geometry or DOF, silently · **M** robustness / honest failure ·
**L** cosmetic or latent. Sketch findings were verified against the code
(C1–C4, M2 re-read at the cited lines).

## Done in these sessions

| ✓ | Item | Commit |
|---|---|---|
| ✓ | Missing STEP parts listed in the tree, not silently absent | 5196962b, 9543702f |
| ✓ | STEP cone semi-angle read in the file's angle unit (degrees) | ce46e9eb |
| ✓ | Cone apex-cap + two-circle torus-band meshers | b7c200de |
| ✓ | Boolean: torus cut by a perpendicular plane | 3c55693d |
| ✓ | Boolean: cone cut by axis-parallel planes keeps its apex cap | be104e74 |
| ✓ | Conformal development chart for cylinder/cone meshing; SSI seeding in true lengths; SSI corrector at degenerate parametrisations | d1d27277 |
| ✓ | SDF + `classify_point` sign from the angle-weighted pseudonormal (torus/NURBS/ruled were never counted by ray parity) | 6824778f |
| ✓ | SK-C1 certificate describes the sketch as stored (`constraints_satisfied`, `solvable`); extrude/revolve refuse an unsatisfied sketch | 484d96a7 |
| ✓ | SK-C2 `is_sound` requires every constraint satisfied as stored; `try_add_constraint` refuses impossible dimension values; first `Diverged` test | 9065f627 |

## Architecture — agent first, human alongside (Varun 2026-11-06)

The principle: the agent is the primary user, a human works in the same
document at the same time, and neither may be surprised by the other. Checked
for the sketch subsystem on 2026-11-06 (facts verified in code); the same
checks are owed for every other subsystem (ARCH-7).

- **ARCH-1 Sketch edits are not events.** Only `sketch_extrude`,
  `sketch_revolve` and `csketch_construction` are recorded
  (`api-server/src/csketch.rs` 1778, 2293, 2814), with `Author::System`
  (3089). Adding entities/constraints, editing dimensions and solving never
  reach the timeline: no undo, no replay, no "who constrained what". Every
  sketch mutation becomes a timeline event carrying its author (agent / human
  identity) and intent, and the sketch is rebuilt by replay like the solid.
- **ARCH-2 No live sync.** Sketch mutations are not broadcast (only the solids
  they produce). Publish sketch deltas (entity / constraint / solve / dragged)
  over the WebSocket so the human's SketchPanel and the agent see each other's
  edits as they happen; the agent subscribes to the sketches it works on.
- **ARCH-3 No revision.** A sketch has no revision counter, so a certificate
  or an extrude is not pinned to what was certified. Add a monotonic revision;
  every response carries it; the certificate records it; extrude/revolve take
  `expected_revision` and refuse (typed, with the current revision and what
  changed) when the sketch moved underneath.
- **ARCH-4 One solve model for both users.** Agents batch edits then solve;
  humans expect always-solved dragging. Make "solved" a stamp on a revision
  (`solved_revision`): the UI solves live on every human edit; an agent may
  batch inside a transaction and solve once; extrude refuses a sketch whose
  stored geometry does not satisfy its constraints (SK-C1) or accepts
  `solve_first: true` and returns the solve's delta (what moved, how far).
- **ARCH-5 One response envelope for every sketch mutation:** the result,
  the compact certificate (sound / satisfied / solvable / DOF / violated),
  the revision, the delta, and machine-readable next actions — so the agent
  never has to re-query to know where it stands (`psketch_solve` drops
  `violations` and the certificate today: SK-C4).
- **ARCH-6 Intent and authority on sketch routes.** csketch mutations carry no
  `X-Roshera-Intent`; bring them under the same intent and
  verification-scope gates as the solid routes, and record a human override of
  an agent's constraint as such.
- **ARCH-7 The same five checks (events, live sync, revision, response
  envelope, intent) for every other subsystem:** solids/booleans, assemblies
  and mates, GD&T and dimensions, drawings, STEP import. Produce a per-
  subsystem table and file the gaps here.

Scheduling: ARCH-1..ARCH-5 go BEFORE the sketch parity block (Days 9–12) —
new constraint types land on an event-sourced, revisioned, live-synced sketch
instead of being bolted onto today's island. SK-C1..C4 come first regardless:
they are correctness, and they do not conflict with the redesign.

## UI / UX — the human alongside the agent (Varun 2026-11-06)

Standing rules (memory, Varun ×7): the VIEWPORT IS THE WORK — every panel a
transparent, closable overlay on the canvas, no blur, headers hug, 11 px
floor, numbers in the status bar; UI = time-to-recognition (cards and
tick/cross, not paragraphs); screenshot first, then propose.

- **UX-0 Audit first.** Screenshot every surface (viewport, tree, sketch mode,
  drawing sheet, agent panel, timeline, refusals) on the robot arm and a
  sketch-built part; list what a CAD user cannot find, cannot read at a
  glance, or cannot undo. The items below are what is already known; the
  audit adds and re-ranks.
- **UX-1 See the agent work.** Live: what it is doing now, which entity it is
  touching (highlighted in the viewport), the intent it declared, each
  refusal it hit and how it recovered — a human must be able to follow and
  interrupt without reading logs.
- **UX-2 Human edits are first-class.** Drag a sketch point / edit a
  dimension / move a part and the agent sees it (ARCH-2), the timeline
  records it with the human as author (ARCH-1), and a human override of an
  agent decision is marked as one.
- **UX-3 Sketch mode at Onshape/SolidWorks level:** live solve while
  dragging, DOF colouring (under / fully / over / conflicting, from the
  certificate), constraint glyphs on the geometry, dimensions editable in
  place, the conflict witness highlighted when the sketch over-constrains,
  inference snaps shown before they commit.
- **UX-4 Trust states, everywhere, at a glance:** sound / unsound / not
  imported / unsolved / unverified as one consistent glyph + colour language
  (tree, viewport, drawing, agent panel), each clickable through to the
  certificate's reason in plain words.
- **UX-5 Undo / redo across everything** (Ctrl+Z / Ctrl+Y over solids,
  sketches, imports, GD&T — depends on ARCH-1 for sketches and on imports
  being timeline events).
- **UX-6 Assembly tree completion:** resizable rail, per-part history under
  Roshera-built parts, isolate / hide-others / show-all, search, the "not
  imported" and "unsound" rows explained on hover (merges UI-1).
- **UX-7 Speed a human feels:** scene load from the mesh cache (STEP-1),
  instancing (STEP-2), progressive load of large assemblies, no frame over
  50 ms on the 572-part arm.
- **UX-8 Errors a human can act on:** every refusal shown as a card (what
  was refused, why, the one-click fix where there is one — e.g. "solve the
  sketch first"), never a silent no-op ("Create Drawing" failing today).
- **UX-9 Drawings in the UI:** sheet view with zoom/pan, live update when the
  part changes, export buttons, the drawing's own certificate (scenario 19
  checks) visible.

Scheduling: UX-0 runs alongside Day 2; UX items then interleave one per day
with the kernel items (one kernel + one UX), so neither side waits weeks.

## The queue (two a day)

### Now — large imports (Varun 2026-11-06, "add the two items and solve them")

The robot arm never reached the live viewport: the server ran out of memory
registering 572 imported parts, and a client cut-off left 33 of them in the
scene while the model held all of them.

- **IMP-1 An import lands whole or not at all.** `import_step_geometry`
  merges every solid into the live model, then registers them one by one
  across `.await`s; when the client disconnects axum drops the handler
  future mid-loop, so the model holds solids the scene never shows. Fix: the
  merge → certify → register pipeline runs in a detached task the request
  only awaits (a dropped request no longer cancels it). A server crash
  mid-import is a durability question, not this one.
- **IMP-2 Registering imported parts is fast and light.** Per part the
  route builds the full mesh as a JSON value into the response (neither
  client reads it), certifies under the model write lock, and each
  certificate validates the WHOLE model then keeps that part's errors
  (`validate_solid_scoped`) and rescans every face for orphans — quadratic
  in the part count. Fix: drop the mesh from the response; certify a batch
  with one model-level pass and per-part checks in bounded parallel; heavy
  work off the async workers.

  Status 2026-11-06: IMP-1 done (bf4d7554). IMP-2 partly done (4c98497c,
  bf4d7554): batch certify + mesh on a 4-thread pool, mesh dropped from the
  response, snapshot off the async workers. Robot arm, debug: importer
  1028 s -> 244 s; batch certify of 572 parts 270 s; server peak 616 MB
  (it never was the memory hog). The live import still had not finished
  after 28 min; the unmeasured step is warming mass properties for 572
  parts.

  2026-11-07: measured — warming the volumes took 2801 s (debug), 2520 s
  of it on three copies of one 20 mm part: `fine()`'s ABSOLUTE 0.01 mm
  edge length pinned its 38 NURBS faces at the segment cap, 1.7 M
  triangles per copy (1006 s). Fix: the mass-properties mesh floors its
  edge length at 0.5 % of the part (`TessellationParams::mass_properties`):
  69 k triangles, 20 s, volume moved 1e-4 relative (bound reported: 5e-3).
  Plus `calculate_solid_volumes`, the batch on a 4-thread pool, wired into
  import registration and the evidence pack (which looped every part the
  same way). Owed: the full geometry-engine red-gate (reaped twice), then
  a live arm import.


### Day 1 — sketch certificate honesty

1. **SK-C1 Certificate certifies a re-solved copy, not the stored sketch.**
   `sketch_certificate.rs:819` solves a private copy and measures residuals
   after it; `extrude_csketch` never solves (`api-server/src/csketch.rs:2092`),
   so a 10×7 rectangle carrying `Length(40)` certifies satisfied/sound and
   extrudes 10×7. Fix: facts from an UN-solved copy (the `continuity_facts`
   pattern, line 756); the re-solve reported separately as "solvable"; extrude
   and revolve refuse (typed) when stored residuals exceed tolerance.
2. ✓ **SK-C2 `is_sound` ignores satisfaction.** `is_sound()` (line 505) is
   consistent ∧ valid ∧ no self-intersection; a full-rank infeasible system
   (triangle sides 1, 1, 5) reads sound. Fix: add "every constraint satisfied
   on the stored sketch"; validate dimension values at
   `Sketch::try_add_constraint` (`sketch.rs:990`), reusing `set_scalar`'s rules
   (`Distance(-5)` is accepted today). First `Diverged` test in the repo.

### Day 2 — solve verdict + self-intersection

3. ✓ **SK-C4 Solve status from the plain count.** Done 2026-11-07: a residual
   above tolerance reads NotConverged (count Over excepted, a real conflict);
   a converged solve takes the rank verdict from `analyze_dofs`, carried as
   `SketchSolveReport.dof`; `psketch_solve` returns violations, dof and the
   certificate (MCP dist rebuild pending Varun). Original finding: `constraint_solver.rs:1098`: the
   DOF-count verdict outranks Converged/NotConverged, so a free point with
   `X=0` twice reads fully constrained, and a count imbalance hides divergence.
   Fix: rank veto in the solve verdict (fold `analyze_dofs` into
   `SketchSolveReport`); a count verdict never masks NotConverged.
   `psketch_solve` (`roshera-mcp/src/tools/psketch.ts:142`) returns
   `violations` + certificate; fix its "exactly" wording.
4. **SK-C3 Self-intersection checked for polylines only.**
   `sketch_validation.rs:443`, `sketch_topology.rs:873` are polyline/stub; a
   bow-tie of four Lines certifies sound and passes the extrude gate. Fix:
   exact pairwise intersection for line/arc/circle/ellipse/spline entities
   (analytic where closed-form, certified subdivision for splines); until a
   pair kind is covered it reports "unchecked" → `self_intersection_free=false`.

### Day 3 — degenerate rows + failed-solve write-back

5. **SK-H1 Degenerate geometry satisfies rows.** `angle_residual` returns
   `[0,0]` on a zero direction (`constraint_solver.rs:2812`); tangent uses
   `UNIT_X` (4004); Parallel/Perpendicular/H/V/Collinear/PointOnLine use
   unnormalised products (2162, 2178, 2190, 2202, 4354, 4614). Fix: refuse
   (`UNSUPPORTED_CONSTRAINT_RESIDUAL`) or normalise when a segment is below
   tolerance.
6. **SK-H2 A failed solve still overwrites the sketch; no NaN guard.**
   `sketch_solver.rs:1373` writes back on Unstable/NotConverged; circle/arc
   write-back unvalidated (1935-1954). Fix: snapshot/restore (the
   `csketch.rs:929` pattern), finite + positive checks, NaN rows count as
   violated.

### Day 4 — DOF accounting + topology robustness

7. **SK-H3 Infinite/ray lines counted at 4 DOF** (true 2 / 3;
   `sketch_solver.rs:1896`) — such a sketch can never read fully constrained.
8. **SK-M2 Topology vertex hash saturates.** `(p.x / 1e-10).round() as i32`
   (`sketch_topology.rs:409`) saturates for |x| > 0.21 — open-endpoint
   reporting is wrong. Fix: scale-aware spatial hash (or linear scan) on the
   sketch's own tolerance; plus **SK-M3** loop-closing tolerance mismatch
   (closes at 1e-6, reports gaps only in (1e-10, 1e-9)) and first-edge walk at
   degree-4 vertices.

### Day 5 — scale and enforceability

9. **SK-M1 Absolute tolerances in mixed units.** One 1e-10 residual norm over
   mm, mm² and dimensionless rows; FD step h=1e-8 absolute. Fix: per-row
   normalisation by sketch extent; relative FD step; a scale-invariance test
   (same verdict at 1e-3, 1, 1e4 scale).
10. **SK-M4..M8** Refuse ContactConstraint / MomentOfInertia at the door
    (never enforceable); tangent/point-on-curve against the DRAWN span, not
    the carrier circle/infinite line (or report contact-off-span); Distance at
    coincident points (non-differentiable |p−q|); drag constraints counted
    as over-constraint; wire `priority` validated.

### Day 6 — analytic Jacobians + SDF robustness

11. **SK-J Analytic Jacobians** for every residual (FD only today), with an
    FD cross-check test per constraint. Prerequisite for DCM-grade
    convergence and for honest rank decisions.
12. **SDF-2 Edge double-count / never-overestimate distance.** Faces whose
    closest-point solve fails are skipped (distance can overestimate; the
    adaptive field then under-refines). Fix: per-face fallback (trim-aware
    projection, then edges) and a test that the distance never exceeds a
    brute-force bound.

### Day 7 — ray solver completeness + spatial index

13. **RAY-1 `raycast_all` / `raycast_solid` intersect Plane/Sphere/Cylinder/
    Cone only.** Torus (quartic), NURBS / ruled / offset / fillet surfaces
    return no hit — the agent's raycast and `raytrace_ortho` render miss them.
    Fix: exact torus quartic; certified Newton-on-subdivision for NURBS/ruled;
    an honest "unsupported surface" flag where neither applies.
14. **SDF-3 BVH for nearest/raycast** (`queries/bvh.rs` exists, unused here):
    O(log F) queries; required before batch marching on 500-part assemblies.

### Day 8 — batch marching (Varun's link: maximecb/batchmarching)

15. **BM-1 Tile-batched `raytrace_ortho`.** Ortho rays → one SDF evaluation
    certifies a whole tile (`d(centre) > tile half-width`), subdivide where it
    fails, exact raycast only near silhouettes.
16. **BM-2 Lower-bound SDF + octree sign propagation.** AABB lower bounds
    from the BVH drive marching; a cell with |d| > half-diagonal inherits its
    centre's sign (no per-leaf sign work off the surface band).

### Day 9–12 — sketch parity with Onshape / SolidWorks

17. **SK-P1 Tangency, complete:** arc–arc, circle–circle, line–spline,
    arc–spline, line–ellipse, with INTERNAL/EXTERNAL branch chosen from the
    drawn configuration and kept through solves.
18. **SK-P2 Distance, complete:** point–line, line–line (parallel),
    point–circle, circle–circle (min / max / centre), horizontal/vertical
    distance dims.
19. **SK-P3 Driven (reference) dimensions:** a flag on `Constraint`; measured,
    never enforced, never counted against DOF.
20. **SK-P4 Fix/ground for lines, circles, arcs** (points only today);
    two-line Collinear; Coradial; Symmetric for line/circle pairs; Equal
    curvature; Normal (Onshape).
21. **SK-P5 Angle with explicit quadrant / supplement choice** preserved from
    the drawn configuration.
22. **SK-P6 Solution-branch preservation:** every solve lands on the solution
    closest to the current geometry (no flips) — the DCM behaviour users
    expect when dragging.
23. **SK-P7 Sketch tools:** trim/extend/split (check what `sketch_ops.rs`
    already has), sketch fillet/chamfer, offset chain, linear/circular sketch
    patterns, mirror, construction toggle, slot/polygon, project/convert
    model edges onto the sketch plane.
24. **SK-P8 Variables and equations:** named parameters and expressions
    driving dimensions (SolidWorks global variables / Onshape variables),
    recorded on the timeline.
25. **SK-P9 MCP surface:** `psketch_add_entity` gains `ellipse`; every
    constraint the kernel supports advertised in `psketch_constrain`'s
    description; driven dimensions and DOF colouring exposed to the agent.

### Day 13+ — tessellation, booleans, STEP, viewport

26. **TESS-1 Topology-first fallback mesher** (arXiv 2604.02141) for faces the
    chart path cannot mesh, replacing per-shape special meshers over time.
27. **TESS-2 Decision (Varun):** split mesh quality out of `is_sound` as its
    own verdict (a valid solid with a poor display mesh reads ⚠ today).
28. **BOOL-1 Torus cut exactly through its equator seam** — refused today
    (seam and cut copies split at different points, do not weld).
29. **BOOL-2 Cone cut tangent to its rim** — keeps the fitted (inexact)
    hyperbola; the exact curve breaks curved-CDT (`PointOnFixedEdge`).
30. **BOOL-3 Plane∘torus oblique / oval arm unreachable** — a torus classifies
    `Other`, so `plane_torus_intersection`'s own arms never run for true
    planes; route deliberately and re-measure the rim-poke tests.
31. **STEP-1 Display-mesh cache** on `Solid` beside `cached_certificate` (scene
    snapshot re-tessellates every part: 83 s / 81 MB on the robot arm, debug).
32. **STEP-2 GPU instancing** of ×N identical parts.
33. **STEP-3 Remaining robot-arm unsound parts** (70 of 572) and failed
    entities — re-probe and classify.
34. **UI-1 Tree polish:** resizable rail, per-part history under Roshera-built
    parts, visible error when "Create Drawing" fails, Ctrl+Z (imports are not
    on the timeline).
35. **AGENT-1 `assembly_tree` MCP tool:** hierarchy + per-part summary
    (standard/custom, key dims, bores/features, soundness).
36. **DRW-1 Assembly drawings:** BOM from the tree, multi-part views with HLR
    across parts, balloons, "Create Assembly Drawing" on file/sub-assembly rows.
37. **MERGE-1 Merge `wip/step-import` to main** — the full geometry-engine
    red-gate has passed on d1d27277; re-run it on the final head first.

## Low (batched opportunistically, not scheduled)

- `get_circle_radius` fabricates `1.0` on a short parameter vector
  (`constraint_solver.rs:2848`).
- Symmetric arc pair emits zero rows on missing state (4259).
- Single-pass Gram-Schmidt without re-orthogonalisation (521).
- `final_error` is the pre-step value at the iteration cap (1129-1164).
- `Rectangle2d` has `pub` fields + `Deserialize`, so `edges()`'s `expect`
  invariant is not enforced by the type (`rectangle2d.rs:47`).
- Constraint residual tests that would pass on degenerate geometry
  (`horizontal_error_is_dir_y`, `parallel_lines_error_zero…`, …) — convert to
  solve-to-target tests as the corresponding items land.
