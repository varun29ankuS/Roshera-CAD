# Session Handoff — 2026-06-10

Carrier note: only **memory dir + git + this file** survive a model/session switch.
The harness task list (the `#NN` todos) may NOT transfer — they are reproduced
below so nothing is lost. Read `~/.claude/.../memory/kernel-quality-northstar.md`
first; this file is the human-readable summary.

## Where things are

Active work = the **boolean-hardening campaign** on branch
**`bool-edge-poke-mesh`** — 6 commits, all pushed to origin, **UNMERGED from
`main`**. Decision for Varun: open a PR / merge to main, or keep iterating on the
branch.

All green at HEAD `27c9247`: **lib 3678/0**, the four integration gate suites
**26/0** (boolean_fuzz_survey, curved_boolean_poke_envelope,
boolean_determinism_sweep, tess_curved_cdt).

### The 6 commits (oldest → newest)
1. `18890bd` **poke-edge #92** — `tessellate_spherical_large_region` (grid +
   spherical-winding membership + lens-hole stitch). box∪sphere edge-poke
   7.91 → 11.5859 (truth 11.5867), watertight.
2. `1654273` **#90 diag** — `diag_sphere_interior_containment`, splits the
   interior HARD cluster into 3 sub-roots.
3. `d9ee2a4` **EmptyResult** — typed `OperationError::EmptyResult` for
   geometrically-empty booleans (engulf/disjoint). Survey maps it to vol-0.
4. `ad00b1c` **tangent classifier (WIP)** — committed knowingly red (Varun:
   "commit it, don't worry about regression, fix in following steps").
5. `50f0480` **tangent fix-forward** — gate the OnBoundary reclassification to
   ISOLATED contacts → green. box∩sphere r=1: ∩ 4.1878, ∖ 3.8122, watertight.
6. `27c9247` **containment ratchet gate** — `sphere_containment_gate` locks the
   #90 fix.

### Scoreboard
box∘sphere fuzz survey HARD: 23 → 14 this session. Inclusion-exclusion now holds
for contained spheres. Note: the 535/99 historical HARD numbers are NOT
comparable to 14 (different config sets / oracle versions / full-matrix vs
box∘sphere-only) — do not quote a cross-version delta.

## The live frontier — #88 poke-through (start here next)

Sphere spans >1 box face → ≥4 cut circles on the sphere that mutually intersect
on box edges (the multi-circle generalization of the conquered 3-circle corner
#60). `interior-offset [0.5,0.3,0] r=1.05`: ∩ `build_shells components=3` "only 1
planar face" ERR; ∪ 6 non-manifold edges, vol 18.64 vs 12.29.

**Refined 2026-06-10:** the box-arc↔sphere-arc WELD is RULED OUT (zero unwelded
near-coincident vertices). The failure is STRUCTURAL — the 4-circle
`sphere_arrangement_faces` under-produces / mis-connects fragments, or
`select_faces_for_operation` keeps the wrong inside-set. Next concrete step: dump
the 5 selected ∩ fragments + edge connectivity to see which land in which of the
3 components (sphere-bit missing vs box-bits not sharing the arc edge). This is a
focused multi-session campaign, like the corner case was — not a one-pass fix.
Reproduce: `diag_sphere_interior_containment` r=1.05/1.2 + `ROSHERA_BOOL_TRACE=1`.

## Methodology (how this campaign runs)

- `tests/boolean_fuzz_survey.rs` is the loop engine: sweeps box∘{sphere,cyl,cone,
  rot-box,torus}+sphere∘sphere against a grid oracle + invariant battery, prints a
  ranked HARD catalog = the work queue. Run the catalog:
  `cargo test -p geometry-engine --test boolean_fuzz_survey boolean_box_sphere_fuzz_survey -- --ignored --nocapture`.
- Loop: survey → worst HARD cell → trace root (membership oracle FIRST; a 1-line
  trace beats a guessed fix) → smallest scoped fix → FULL FLOOR gate → promote the
  conquered cell into an asserting gate (the ratchet) → commit+push.
- **Iron rule:** NEVER commit a regression — `git checkout` the instant the floor
  flags a conquered cell (`poke_matrix_green_cells_hold` #60 is the tripwire).
  Exception: Varun may explicitly authorize a fix-forward WIP (as for `ad00b1c`).
- Always revert temp `ROSHERA_BOOL_TRACE` diagnostics before committing.
- Constraints: `cargo fmt --all` before push (NOT `$null` redirect); commit msgs
  via `git commit -F - <<'EOF'` heredoc; NO Co-Authored-By / Generated trailers.
- `/loop` ScheduleWakeups are per-session — they do NOT survive a session switch.
  Re-issue `/loop` to resume autonomous hardening.

## Open todos (harness task list — reproduced so they survive the switch)

Boolean campaign / curved robustness:
- **#88 BOOL-BEYOND-TANGENT** (pending) — poke-through multi-face transversal. THE
  live frontier above. Fully characterized + weld ruled out.
- **#89 BOOL-UNION-BROKEN** (pending) — box∪sphere wrong off the tested band.
- **#90 BOOL-∩-CONTAINMENT** (in_progress) — 2 of 3 sub-roots DONE+gated
  (engulf/disjoint via EmptyResult, tangent-contained via isolated-contact gate);
  the remaining sub-root IS the #88 poke-through.
- **#91 FUZZ-SURVEY-Φ2** (pending) — subprocess-isolate the HANG over-report
  (currently ~410 soft, mostly rayon leaked-thread artifact), promote more gates.
- **#72 CHAMFER-TRIM** (pending) — fillet↔chamfer corner-junction synthesis.

Product / other tracks (not boolean):
- **#4 HOVER-α** (in_progress) — mouse-over → AI context hover; backend done,
  α.3 frontend pending.
- **#5 DRAW-α** (pending) — 2D orthographic drawing module (front/top/side).
- **#41 CD-φ.7** (pending) — roshera-parry bridge (custom Shape + QueryDispatcher
  → LMD contact manifolds). All prerequisites done.
- **#42 SIM-α** (pending) — rapier rigid-body world bridge.

Everything tagged `[completed]` (#1–#87 mostly, #92) is captured in git history +
the memory topic files; not reproduced here.

## What is durable vs. lost on switch

- DURABLE: git (pushed branch), the `memory/` dir (auto-loaded via `MEMORY.md`),
  this file.
- LOST unless re-created: the harness task list (reproduced above), any running
  `/loop` wakeup (re-issue `/loop`), conversational context not written down.
