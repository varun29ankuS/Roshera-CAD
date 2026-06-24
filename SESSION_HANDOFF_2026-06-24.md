# SESSION HANDOFF — 2026-06-24 (complete-perception + aircraft bug campaign)

## ★ START HERE
- Work branch: **`fix/bore-tess-verification` @ `e4768c6`** — **PUSHED to origin (safe).**
- Session ended because the laptop left mid-run. **4 fixes were in-flight on locked worktrees; their in-progress (uncommitted) work is lost on close — RE-LAUNCH them (exact approaches in §IN-FLIGHT).**
- Everything in §DONE is committed + pushed and safe regardless.
- Detailed running log: `AIRCRAFT_LOOP.md` (committed alongside this).

## DONE + COMMITTED + PUSHED (safe)
**★ COMPLETE PERCEPTION (the headline — Varun's "make certification automatic, not just a call, completely baked in"):**
- **Auto-cert `e10da22`** — every mutating REST endpoint + MCP tool auto-emits the FULL certificate (`certify_solid`, coarse/bounded), no `ground_truth` call. Single chokepoint `certified_response()` in `api-server/src/main.rs`; `?fast=1`/`{fast:true}` opts OUT to lightweight. Closed 4 silent endpoints (transform, face/extrude, cylinder, mirror). Router-integration-verified + MCP-live-confirmed.
- **Intrinsic structural cert `e4768c6`** — the kernel CANNOT return an uncertified solid. Lazy-cached `cached_certificate` on `Solid` (mirrors `cached_mass_props`), invalidated at THREE seams: `record_operation` funnel (fires even when the recorder is detached during replay) + `SolidStore::get_mut` backstop + `set_solid_construction` sidecar. `certify_solid`/`ground_truth` are thin wrappers over `BRepModel::certificate()`. Phase-4 honest call: accessor kept `&mut` (a `&self` path would downgrade the D4 label check and CHANGE cert values → loosen the cert). New non-vacuous gate `certificate_cache.rs`. poke_matrix determinism 3/3 (cert values unchanged), timeline replay green, lib 3859/0. LIVE-CONFIRMED (ground_truth on a fresh box = sound on the structural-cert backend).

**Aircraft kernel bugs (each design-surfaced, gated, MCP-live-confirmed):**
- **B3 `10519f1`** — `revolve`+`wall_thickness` inverted-wall winding. Root: `tessellate_revolution_wedge` keyed winding on the orientation flag; fix = geometric winding from the true surface normal.
- **B4 `3d38f8d`** — `create_box`/extrude construction anchor stranded (`lift(0,0)`). Fix = profile-centroid anchor. **The cert was RIGHT** (resolved harness-Q2).
- **B1 `d11b86f`** — shell of an EXTRUDED box → open B-Rep. Root: ruled side walls + backward-oriented bottom cap. **Took 2 passes** — the first passed a *primitive-box* gate but the live extrude-box shell still failed (live-confirm caught the false positive). Lesson: every fix MCP-live-confirmed, not just gate-passed.

**Sound aircraft parts (live):** fuselage, landing-gear strut, canopy, nacelle, plate wing.

## IN-FLIGHT when the laptop left — RE-LAUNCH (4 parallel worktree fixes, all gated, disjoint files)
1. **Bug 1 — RESOLVED-AS-DIAGNOSIS (afc17daa, honest report): it's the known `#70` CHAMFER-CROSSES-FILLET defect, NOT a simple guard — NEEDS VARUN'S DESIGN CALL.** The proposed early-exit guard is a NO-OP (fillet-first never reaches that code: `is_partial_mixed=false` since no chamfer yet → the degree-2 corner is skipped, no cap synthesized). The REAL defect: in fillet-first, two full-length fillet rim NURBS edges share the cube's top face and CROSS each other near the un-capped corner → face self-overlaps → `validation.rs:743` (the cert CORRECTLY rejects the broken solid). Ordering is NOT topology-invariant: chamfer-first cuts the corner edge so the rims terminate at the chamfer face; fillet-first has no relief (after fillet `[3]`, after follow-up chamfer `[3,5]` — worse). This is the pinned `#70` (fillet.rs:4725 comment; `tessellation-harness.md` "#70 CHAMFER-CROSSES-FILLET … needs Varun's call"). OPTIONS: (a) mark the 2 fillet-first replay tests known-RED until #70 is addressed, or (b) a scoped fillet-rim/shared-face corefinement session with Varun (re-stitch so the rims don't cross, OR reject fillet-first opt-in until corefinement handles it). DO NOT force / blind the cert. [The other 3 fixes — Bug 2, B2a, B2b — were still running at close; re-launch per below.]
2. **Bug 2 — `boolean_determinism_sweep` HANGS** (>120s, SIGTERM). Determinism is load-bearing for "can't lie". TRIAGE: binary-search the ~11 box/sphere/cylinder configs (lines ~120-184) under `timeout` to isolate the hang; likely a degenerate curved boolean (sphere-corner / poke) → non-terminating marcher (add a step cap — cf the prior "missing march cap = 37min hang" lesson) OR O(n²) spatial-grid saturation (the `f64::MAX → infinite [-inf,+inf] grid` class). Fix at root so it terminates + is deterministic (`operations/boolean.rs` or `intersect.rs`). Gates: sweep completes+deterministic + poke_matrix (torus_boolean_is_deterministic) + --lib.
3. **B2a — `nurbs_loft` NON-ORIENTED mesh.** A NACA-0012 loft tessellates to `oriented: FALSE, inconsistent_directed_edges: 297` (constant across TE shapes + a 2.5× fat variant) = REAL flipped facets. The #21-tessellator class (periodic-U loft lateral). Fix the facet WINDING in `tessellate_nurbs_skin_lateral` / `curved_cdt.rs` (the periodic-U seam strip / cap↔lateral stitch / CDT triangle orientation). Gate: a NACA loft → oriented:true + 0 inconsistent_directed_edges + watertight + the FULL watertight matrix + poke_matrix.
4. **B2b — cert facet-deviation fix (VALIDATED, re-land).** Removes 374 airfoil false positives; coarse-still-fails holds. In `harness/watertight.rs::analytic_facet_deviation_deg` (~510) + its `mesh_quality` call site (~657): measure facet vs its 3 vertices' STORED surface normals (`MeshVertex.normal`, max-of-three), fallback to STABLE `closest_point(vertex)` — NOT `closest_point(centroid)`. ★ Gate on a KNOWN-ORIENTED rounded mesh (NOT the airfoil) passing + a COARSE curved mesh STILL failing (guardrail b — non-negotiable, no blinding).
- **When B2a AND B2b both land → the airfoil certifies on merit → curved wings/tail UNBLOCK → resume the airframe (ASK Varun first).**

## THE B2 SAGA (the key learning — 4 honest reverts, each bought a truer diagnosis)
under-resolution (cache resample / crease grids → broke revolve) → cert MEASUREMENT bug (closest_point mislocates at rounded noses → measurement fix validated) → DEEPER orientation bug (the airfoil mesh is GENUINELY flipped). **B2 = two bugs: B2a (orientation) + B2b (measurement).** The cert was RIGHT to flag the airfoil. Forbidding the agents from blinding the cert is exactly what surfaced the real bug each time.

## The 2 pre-existing bugs (both CRITICAL, both diagnosed)
- **Bug 1 (fillet)** — pre-existing on main; small guard fix (above).
- **Bug 2 (boolean hang)** — determinism has been UNVERIFIED while the sweep silently times out; the moat depends on boolean determinism. Needs triage + a per-op watchdog.

## Operational notes (important for the next session)
- **Bash CANNOT curl the backend** (sandboxed → HTTP 000). Live-confirm via **MCP tools only**.
- The **MCP server's `dist` was rebuilt (`tsc`)** but the running client needs a **session restart** to display the full cert breakdown in tool output. The REST layer + elevated top-level `sound` flow through without it.
- The **backend (8081) has been flaky** (dies with no crash log; relaunch: `cd roshera-backend && cargo run -p api-server`). Frontend built clean (vite).
- Branch hygiene: cleaned ~76 orphaned `worktree-agent-*` + merged fix branches (107→31 local). The 31 remaining are prior-session branches (left untouched). 4 in-flight fix worktrees + a stray `bds-fix` worktree may need pruning.
- Standing rules: every fix MCP-live-confirmed; never loosen cert/weld; gated (poke_matrix + watertight matrix + --lib); production-grade; **no AI commit trailers**; cargo build/run only when Varun asks (he authorized these).

## Aircraft manifest
fuselage ✅ · plate wing ✅ (curved airfoil wing pending B2a+B2b) · nacelle ✅ · strut ✅ · canopy ✅ · curved wings/tail/fin/control-surfaces/fillets/ASSEMBLE pending (lifting surfaces need B2).

## Competitive note (Varun asked this session)
CadXStudio (Ernakulam; founders John Santhosh + Praphul S Warrier; inc. Oct 2025; ~₹3 lakh KSUM grant; pre-beta) is **not rewriting a kernel** — "Kernel Platform" is Kerala Startup Mission's registry label, not their tech. Almost certainly LLM→CadQuery→OpenCascade (commodity text-to-CAD). Roshera's native Rust B-Rep + the can't-lie certificate is the real differentiator; their competition is on product/GTM, not the kernel.
