# SESSION HANDOFF — 2026-06-20

> Written before a laptop restart. Read this top-to-bottom, then start with the
> ★ IN-FLIGHT item. `main` is clean and pushed; nothing is lost.

---

## TL;DR

- **`main` @ `70eed9e`** — clean working tree, fully in sync with `origin/main`. All of today's work is committed + pushed.
- **One fix in flight, preserved on branch `wip-cylinder-bore-fix` (commit `011e1fbb9`) — UNVERIFIED. Verify + land it FIRST** (details below).
- Backend + frontend + MCP all need to be (re)started after the laptop restart — see **Environment**.
- The TODO list (kernel task tracker) is reproduced at the bottom, prioritized.

---

## ★ IN-FLIGHT — do this first: cylinder ID-surface (bore) tessellation fix

**Branch:** `wip-cylinder-bore-fix` (commit `011e1fbb9`, based off `03bf45f`). One file: `roshera-backend/geometry-engine/src/tessellation/surface.rs` (+191).

**The bug (Varun's stated priority — "the ID surface of the cylinder needs work, it's been a problem for a long time"):** inner / concave cylindrical faces (a bore / inner-diameter wall) tessellate **degenerate** — tangled triangles, garbage normals, a black "scribble" in shaded mode — while the **outer** cylinder of the same part tessellates perfectly smooth. **Verified eyes-on:** rendered an imported bored-lug part in `normals` mode — OD cylinder = clean red→green→blue gradient; ID bore wall = jagged magenta streaks. The B-Rep is **valid + watertight** (`validate_solid_scoped` passes, open=0 nm=0) — this is a **display-tessellation defect on the reversed inner cylindrical face** (orientation/sense → wrong grid winding / seam → collapsed slivers + flipped normals), NOT a topology error.

**The fix (in the branch, NOT verified by me):** `surface.rs +191` — inner-face winding/seam handling in the cylindrical-surface tessellation path. The agent was stopped mid-verification (laptop restart); its last words: *"changes restored, the one lib failure is pre-existing and unrelated to my change, confirming the rest of the lib tests pass."* So it had run lib tests and flagged ONE failure as pre-existing — **this needs independent confirmation.**

**To land it:**
1. `git merge wip-cylinder-bore-fix` onto current `main` — **EXPECT a `surface.rs` conflict with #38's audit-param change** (both touch `tessellation/surface.rs`). Resolve **keeping BOTH** (#38's `audit()` spherical-fan budget + the inner-face winding/seam fix — different sites).
2. **VERIFY MYSELF, eyes-on:** build a bored cylinder / tube (`create_cylinder` then boolean-subtract a smaller coaxial `create_cylinder`, OR import a bored STEP) → render the bore in **`normals` AND `shaded`** → **READ the image** → the ID wall must go scribble → smooth (consistent inward normals, no degenerate slivers).
3. Run the tessellation + watertight gates + `lib boolean` (123/0) + golden + `torus_boolean_is_deterministic`. **Confirm the "1 lib failure" the agent saw is genuinely pre-existing** — check it on `main` WITHOUT the change; if it only appears WITH the change, it's a regression → don't land.
4. Green + bore-clean (verified by me, not claimed) → `cargo fmt` → push → delete branch `wip-cylinder-bore-fix`. Still scribbled / regression → `git reset` the merge (revert), report.

---

## WHAT LANDED TODAY (all on `main`, pushed)

| Commit | What |
|---|---|
| `7515c65` | **#37 curved-boolean DETERMINISM** — sorted-iteration weld; same input → same output. THE unblock: poke_matrix is now a trustworthy gate. Verified (torus built 5× → identical volume). |
| `e160a7f` | **#36 no-hangs** — spherical-fan tessellation budget; previously-hanging curved booleans terminate. |
| `4ef85a2` | **#19 ASSEMBLIES + INSTANCING** — `InstancedAssembly` (part_id + transform, geometry REUSED not copied), REST `/api/assembly/*`, 5 MCP tools, transform-aware render. kernel 8/8 + api 7/7. North-star pillar. |
| `6f89d81` | **#42 per-part BLACKBOARD scoping** — `BlackboardScope{Part(uuid),Assembly(uuid),Document}`; each part/assembly its own notebook. Isolation proven 19/0. (Cross-pollination #43/#44 still TODO.) |
| `442d75c` | **Agent-eye reconciled** to 2 scopes (part + assembly); assembly = named instanced assembly by id OR whole-scene composite when blank. Dropped the redundant 3rd tab. |
| `6bc0e01` | **★ #45 STEP EXPORT FIX** — root cause was the sampled-surface fallback writing `B_SPLINE_SURFACE_WITH_KNOTS` with **EMPTY knot vectors** → strict readers (FreeCAD/OCC, Parasolid, ACIS) drop the face → boundary-edge gaps → invalid solid → "nothing opens". Fix: `clamped_uniform_knots` + qualified AP242 `FILE_SCHEMA` id. **Round-trip oracle: 7 gaps → 0, valid.** Permanent gate `step_roundtrip_tests`. |
| `03bf45f` | **#46 frontend drag-drop STEP import** — drop a `.step` on the viewport → loads + honest coverage card (resolved vs unsupported entity types, per-solid validity). Backend `/api/geometry/import_step` was already proven (imports real FreeCAD files clean). |
| `cc4a22e` / `70eed9e` | **MCP server icon + name** — `icons` metadata (GUI clients) + server name `🅡 ROSHERA`. NOTE: Claude Code (terminal CLI) shows "Calling roshera…" as TEXT and caches the name; the glyph does NOT show in the CLI — that's a client limitation, not fixable server-side. Mark lives on the boot banner + GUI clients. |
| `20c7b54` | **#38 perf LANDED** — curved-boolean AUDIT 3.4× faster (no-hang test 349s → 111s) via coarse `audit()` tessellation params; leak-detection preserved. The "weld regression" was a STALE TEST from commit #65 (doubled-facet removal), not a perf bug — assertion corrected. ALSO: import-button moved under Toolbar **Export ▸ Import ▸ STEP** (floating button removed). |
| `29e5081` | **Orange `🅡 ROSHERA` startup banner** in the api-server (prints on listen). |

**Earlier in the multi-day run (already on main):** GD&T kernel-verified conformance, transform+cross-entity-consistency invariant, labeller (core + assertion + measurement + color + delete + visibility), mass-props guard, gap-finder harness, dimensioned color-coded labels.

---

## DISCIPLINE / LESSONS (carry these — they were learned the hard way today)

- **VERIFY THE GOAL YOURSELF.** An agent's "it passes" is NOT evidence. Today the hang-guard agent *claimed* a fix that my own run showed still failed. Always re-run the goal test on `main` after merge.
- **EYES ON.** For any visual/geometry change, RENDER it and READ the image — don't trust the JSON verdict. `brep_valid=true` ≠ "looks right" (the bore scribble was valid + watertight but visually broken).
- **An empty worktree diff ≠ reverted.** It can be **stashed** (an agent stashing to time a baseline) — `git stash list` BEFORE concluding an agent reverted. I mis-stopped the #38 agent mid-baseline-measurement; recovered only because the stash survived.
- **Don't run `cargo` in a LIVE agent's worktree** — it contends on the shared `target` build lock and both crawl. Verify on `main` after merge instead.
- **Capture `cargo`/command exit codes directly, not after a pipe** (`X | grep` makes `$?` = grep's exit). This masked an fmt failure earlier.
- **Never blind-land a boolean-core change.** `poke_matrix` (33-cell curved-boolean gate) must stay green; revert if it regresses. It's now deterministic (#37) so the signal is trustworthy — but it's SLOW (>500s) and the torus cell has historic flakiness.
- **Boolean F1/F2 corefinement (#35) is NOT an autonomous task** — THREE agents have reverted on it (the fix is geometrically reachable but perturbs the shared curved-weld path that poke_matrix protects). It needs a focused hands-on session with Varun.
- House rules: production-grade only (no unwrap/expect/panic/todo/stub outside tests); DashMap not Mutex<HashMap>; `cargo fmt --all` before commit; never force-push main; never `--no-verify` on a real main landing (WIP-preserve worktree commits are the exception); no Co-Authored-By trailers; never loosen the global weld tolerance; API-only AI (no local models); timeline/event-sourced not parametric.

---

## ENVIRONMENT (after the laptop restart, nothing is running)

- **Backend** (was running from the prior session; dead after restart). Start it — needs a rebuild to pick up the banner + STEP fix:
  `cd roshera-backend && cargo run -p api-server`  (listens on `:8081`; prints the orange `🅡 ROSHERA` banner). Run it in **Varun's own terminal** for persistence (an agent-launched background server dies with the session).
- **Frontend**: dev server, hot-reloads (do NOT rebuild it). If not running: the usual dev command in `roshera-app/`.
- **MCP**: configured in `.mcp.json` (key `roshera` → `roshera-mcp/dist/index.js`, `ROSHERA_URL=http://localhost:8081`). Server reports name `🅡 ROSHERA`. If `index.ts` changes, `cd roshera-mcp && npm run build` then `/mcp` reconnect.
- **STEP export fix is LIVE** once the backend restarts — the real test is: re-export the part that previously wouldn't open, and open it in FreeCAD. (The `Downloads/ROSHERA_FIXED_export.step` sample has all-analytic faces so it does NOT exercise the fallback fix — use a part with boolean-split faces.)
- **Stashes:** `git stash list` — `stash@{1}: f1-void-fix` is the **#35 corefinement** WIP (keep for the focused session). `stash@{0}` is the old #38 perf (already landed via branch — obsolete, safe to drop).
- **Disk** was ~24 GB (watch it; prune stale `.claude/worktrees/agent-*` and run `cargo clean` on dead worktree targets if low). One worktree dir (`agent-a6ff2d42…`) may still be on disk — its work is safe on `wip-cylinder-bore-fix`; `git worktree prune` once unlocked.

---

## TODO LIST (kernel task tracker — prioritized)

**★ Do first:** verify + land `wip-cylinder-bore-fix` (cylinder bore tessellation, above).

**Geometry core (the depth campaign — moves the "~8/100 geometry maturity" number; this is where it matters):**
- **#35 — Shared-imprint corefinement (F1 planar + F2/#17 curved).** THE deep boolean fix. FOCUSED HANDS-ON session with Varun, poke_matrix-gated. (WIP in `stash@{1}` f1-void-fix.) 3 agents have reverted — do NOT throw another autonomous agent at it.
- **#34 — Multi-body boolean output.** Oversized bore severs a part into 2 bodies; kernel correctly refuses single-Solid output → need to emit multiple Solids (detect disjoint outer shells).
- **#33 — Boolean-robustness campaign** (the 4 bracket failure classes; F1/F2 = #35, F4 = #34, F3 already robust).
- **#40 — Robust fillet/chamfer at scale** (multi-edge corners, fillet-on-union, variable radius).
- **#39 — Patterns / arrays** (linear, circular, mirror — bolt circles, hole grids; manufacturing staple).
- **#18 — Loft surfacing control** (tangency / guide rails to kill lumpiness).
- **#2 — cdt panic frequency + robust per-face fallback.**

**Assemblies / blackboard (continue what landed):**
- **#43 — Cross-pollination Step 1:** assembly blackboard AGGREGATES its member parts' entries (read-only, via #19 assembly→part_ids). (Varun's design.)
- **#44 — Cross-pollination Step 2:** live cross-scope value references (a part's `throat_area` flows into an assembly calc, auto-recompute, cycle-refusal). The richer pass.

**Showcase / product:**
- **#41 — ★ SHOWCASE: real multi-part assembly end-to-end** (sound + labelled + GD&T-toleranced + drawn + STEP, via MCP eyes-on, + the moat moment: feed a bad spec → kernel REFUSES). Now UNBLOCKED by #19 assemblies + the STEP fix.
- **#27 — Demos** (flagship engine + honesty/moat).
- **#23 — Dimensioned axial-profile drawing** (section + feature dims).

**Smaller / polish:**
- Import coverage report is MISLEADING — a VALID solid reports `ADVANCED_FACE: 23 failed` (the faces reconstruct via the shell; the per-entity resolver miscounts them as "failed"). Fix the report accuracy so it doesn't say "23 failed" on a sound import.
- **Husk-pruning at export** — the original failing STEP carried 10 orphan boolean-husk shells (the #36 invalid-husk-in-SolidStore issue); separate bug, bloats files. Prune husks at export.
- **#25 — Pre-commit gate must build the frontend (tsc),** not just `cargo check` (frontend TS errors have slipped through).
- **#32 — `/api/scene/snapshot` must serialize a material** for agent/MCP-built parts.
- **#21 — Live frontend camera control from the agent.** **#20 — render quality** (ground plane / shadow / AO). **#11/#12 — per-object colour** in the live viewport.
- **#17 — Robust NURBS boolean** (cockpit openings / sidepod inlets — overlaps #35 curved corefinement).
- **#28 — Verification hardening campaign** (ordered ladder: solid-local → cross-entity → semantic/intent).

**FROZEN (don't add): new eye/GD&T/blackboard/labeller-D5 polish — depth-dominant, fix the geometric core.**

---

## NORTH STAR (the filter for every increment)

Varun: *"in 1 yr we go from here to 100-part assemblies, automatic, in 15 min"* and *"it doesn't matter if we go slow, as long as we add things that SCALE and that MATTER."* The product vision: **"3D printing by thinking about products"** — describe the product, the agent builds it as a SOUND, MANUFACTURABLE artifact (watertight, toleranced, STEP-clean, verified). The moat: outputs are **TRUE (the kernel can't lie)**, not just plausible. Honest current state: robust at simple parts; boolean breaks at mid-complexity; STEP interop just fixed; assemblies + per-part notebooks just landed. The gap IS the program — climb the complexity ladder, report capability truthfully.
