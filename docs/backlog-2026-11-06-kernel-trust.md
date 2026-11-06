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

## The queue (two a day)

### Day 1 — sketch certificate honesty

1. **SK-C1 Certificate certifies a re-solved copy, not the stored sketch.**
   `sketch_certificate.rs:819` solves a private copy and measures residuals
   after it; `extrude_csketch` never solves (`api-server/src/csketch.rs:2092`),
   so a 10×7 rectangle carrying `Length(40)` certifies satisfied/sound and
   extrudes 10×7. Fix: facts from an UN-solved copy (the `continuity_facts`
   pattern, line 756); the re-solve reported separately as "solvable"; extrude
   and revolve refuse (typed) when stored residuals exceed tolerance.
2. **SK-C2 `is_sound` ignores satisfaction.** `is_sound()` (line 505) is
   consistent ∧ valid ∧ no self-intersection; a full-rank infeasible system
   (triangle sides 1, 1, 5) reads sound. Fix: add "every constraint satisfied
   on the stored sketch"; validate dimension values at
   `Sketch::try_add_constraint` (`sketch.rs:990`), reusing `set_scalar`'s rules
   (`Distance(-5)` is accepted today). First `Diverged` test in the repo.

### Day 2 — solve verdict + self-intersection

3. **SK-C4 Solve status from the plain count.** `constraint_solver.rs:1098`: the
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
