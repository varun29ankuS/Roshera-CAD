# ☀️ MORNING SUMMARY (read this first) — 2026-06-21 overnight

**Branch `fix/bore-tess-verification`, all gated + pushed. Tree is green (compiles; pre-commit passed each commit).**

## ✅ FIXED + PUSHED (9 audit-found bugs, 3 commits)
- **fc7c185** — 4 reachable correctness bugs: `bisect_root` returned a NaN-poisoned root to NURBS point-inversion (→None); `Cone::offset` recomputed half-angle wrongly (→apex-shift, mirrors offset_exact); `insert_knot` multiplicity guard ignored `times` (→`mult+times`); `Ellipse::check_continuity` acos un-clamped (→NaN G1 misclassified as G0).
- **a5c5021** — panic guards + doc: empty `sections` in `create_frame_driven_sweep` (panic+underflow); empty `entity_ids[0]` in transform translate/rotate/scale/mirror (4 panics); degenerate `create_offset_cone` sin~0 (apex at ∞); corrected the stale CLAUDE.md `timeline_handlers` note.
- **b45280e** — STEP export: `AXIS2_PLACEMENT` ref_direction was hardcoded `[1,0,0]`, degenerate for X-aligned cylinders/cones/planes (OCCT/FreeCAD rejected) → now Gram-Schmidt-orthogonalized vs the axis. **(One of the two STEP-export bugs behind your FreeCAD issue.)**

## 🛑 NEEDS-VARUN (real, but too big/risky/design for unsupervised overnight — see B-list + A12 below for details/fix-paths)
- **B1 ★ geometric validation is a STUB** (`primitives/validation.rs:474`): Standard/Deep validation stamps `geometry_valid:true` WITHOUT checking — the kernel can certify invalid geometry. Plus a torn shell (boundary edge) is a warning not an error. THE verification moat.
- **A12 ★ STEP export still facets SurfaceOfRevolution** (your nozzle screenshot): `extract_surface_data` has no arm → degree-1 grid. Complete fix-path documented below; deferred because it's high-visibility + multi-piece (a subtle error ships a STEP that *looks* fixed). B3 above fixed the *other* STEP bug.
- **B2-sketch** sketch→3D bridge facets every curve (`csketch.rs:1534`); **B9** boolean single-curve branch emits a dummy Line (`boolean.rs:3881`); **B4** SSI corrector branch-hops (the #35 cyl-cyl saddle); **B3-sketch** dimensional constraints (Diameter/Length) silently no-op yet count as DOF; **B5** RBAC dead/bypassed; **B7** spatial queries (point/field/region) orphaned from API/MCP (the "inhabitable substrate" moat unreachable); **B8** EdgeStore::add skips indexes (find_edge_between empty for all primitive edges); **B12** transform skips surfaces when update_parameterization=false; sweep `validate_swept_solid` stub.

## ⚠️ PRE-EXISTING TEST FAILURES (NOT mine — fail at HEAD) + a GATE GAP
- P1 `nozzle_profile_measured_dims_match` (throat not detected), P2 `tessellation refining_tolerance`, P3 `ros loft_watertight_after_roundtrip`.
- ★ **CORRECTION (2026-06-22):** the CI test gate ALREADY EXISTS + is enforced (`.github/workflows/ci.yml:117` `cargo nextest run --workspace`, no continue-on-error). NO new gate needed. BUT **CI has been RED for a long time and pushed-past** — the latest run (27913210589) Test Suite fails the nextest step with **27 failing tests** (Frontend Build + Security + fmt + clippy all PASS; only tests are red). They cluster on the KNOWN-DEEP B-list, not the small stuff:
  - **NURBS boolean watertightness ~13** — `nurbs_boolean_watertight.rs` w01–w10, `nurbs_minus_box_completes_and_imprints`: the #17 corefinement gap (memory `nurbs-corefinement-17`).
  - **NURBS loft watertightness ~5** — `nurbs_loft.rs`×3, `nurbs_car_body`, `golden_nurbs_loft_barrel`, `shell_nurbs_barrel`.
  - **Boolean robustness ~4** — `intersection/union_commutativity_parity`, `union_commutative_polyline_hexagons`, `curved_boolean_on_rotated_primitive_terminates`, `polyline_cut_harness`.
  - **Tessellation ~3** — `refining_tolerance_converges` (P2), `high_curvature_nurbs_no_skinny_triangles`, `chord_tolerance_actually_enforced`.
  - validation `ground_truth.rs:67` (B1-adjacent), `nozzle_profile_measured_dims` (P1), `loft_watertight_after_roundtrip` (P3).
  ACTION (NEEDS-VARUN): this is the real robustness backlog — #17 NURBS corefinement is the biggest cluster (~13 tests). Getting CI green = fixing the deep NURBS boolean/loft/tessellation work. The small-safe sweep does NOT touch these.

## 🧹 DEAD CODE (remove with your permission — rule 5) & MISSING
- `tessellate_surface` (placeholder+buggy), `render_solids_with_labels`, `drawing/section_view.rs` pipeline, `ai-integration/providers/universal_endpoint.rs` (608-line dup), api-server `full_integration_executor` field + `send_collaborators_update`, math SIMD suite + `trimmed_nurbs.rs`, `timeline_impl.rs merge_branches_alt`/`replay_from` (dangerous if called).
- **rag-engine DOES NOT EXIST** — only aspirational CLAUDE.md + a Dockerfile stub that would fail a clean build.
- `/api/metrics` command/perf trackers never written (always 0) + hardcoded DB metrics.

## 🔌 WIRING (your main question) — mostly GOOD
- **Frontend↔backend + MCP↔backend: fully consistent — zero broken endpoints, zero param mismatches** (~60 MCP tools + all ~22 frontend clients verified against the route table). One frontend bug: **`CADMesh.tsx` leaks Three.js geometry/material (no dispose)** under edit churn — ready-to-apply fix: `useEffect(() => () => { geometry.dispose(); material.dispose(); }, [geometry, material])`. (Left for you — I can't build/verify the frontend per your rules.)
- Backend endpoints with no UI: assembly explode/interferences/simulate; dead export MIME entries glTF/FBX/IGES (501).
- api-server routes 100% wired to real handlers; the CLAUDE.md "~30 orphaned WS handlers" note was stale (fixed in a5c5021).

## Audit coverage: 10/10 modules (math, primitives, operations, tessellation, sketch/queries, api-server, ai/rag, timeline/session, export, frontend/mcp). Full per-module findings + the A/B/C worklist are below.

---

# Overnight Autonomous Fix Campaign — started 2026-06-21 night

Varun: "create loops to start fixing the codebase ... and wiring each and
everything ... i am done for the day .. headed to sleep." → autonomous overnight
work: AUDIT the whole architecture, FIX bugs, WIRE everything, gated + committed.

## Working agreement (autonomous, no oversight)
- Branch `fix/bore-tess-verification`. Production-grade only. cargo fmt, NO AI trailers.
- Every fix is a GATED slice: implement → `cargo test`/`cargo check` green → commit → push.
- Pre-commit runs `--all-targets`; keep every `AppState{}` constructor + test target complete.
- REVERT on any gate break. NO force-push. NO merge to `main` (ask Varun). NO destructive ops.
- One concern per commit, clear message. Append every action to the PROGRESS LOG below.
- If something needs Varun's judgment (design call, risky refactor), note it under NEEDS-VARUN and move on.

## Process (the loop)
1. Collect the 10 audit-agent reports as they complete (TaskOutput / SendMessage per agent id).
2. Synthesize into a prioritized worklist below (critical → high → medium). Dedup.
3. Fix top item: implement, verify (test/check), commit, push, log. Repeat.
4. Prefer correctness + wiring (dead/half-wired features → wire or document). Small, safe slices.
5. Morning: write a SUMMARY (fixed / open / needs-Varun) at the top for Varun.

## Audit agents (in flight)
- math — `a991c0ce7253adaba`
- primitives — `a7f41ce7407fb1795`
- operations — `adae14d44eed90600`
- tessellation+render+drawing — `a6ea025bd17636918`
- sketch2d+queries+readable+datum — `a49c293476ab56629`
- api-server (route↔handler wiring) — `a5916267ff41863dd`
- ai-integration+rag — `a14a358526c3d7513`
- timeline+session+shared-types — `a1d3ef8e8e61250ab`
- export-engine — `ad57afde7ca19d92e`
- frontend+mcp (contract wiring) — `a8f4a644f563a2bb6`

## Known item (diagnosed, pre-audit) — FIX FIRST
**STEP export facets SurfaceOfRevolution.** `export-engine/src/formats/ros_snapshot.rs:769`
`extract_surface_data` has NO `SurfaceOfRevolution` arm → falls to the degree-1 grid
fallback (line ~852, n=10) → a faceted ~10-gon in FreeCAD (Varun's screenshot). The
analytic surface is correct; only the export facets it. FIX: add a `SurfaceOfRevolution`
arm that emits an EXACT smooth representation — either (a) a proper STEP
`SURFACE_OF_REVOLUTION` entity (the importer already handles it, `formats/step/handlers/tier2/swept.rs`)
by writing the profile curve (`write_b_spline_curve`) + `AXIS1_PLACEMENT`, or (b) a rational
NURBS surface of revolution (Piegl–Tiller A8.1) emitted as `SurfaceData::Nurbs`. Prefer (a)
(no rational-circle math, FreeCAD-native, round-trips with the existing import). Gate: export
the smooth nozzle, confirm the STEP carries an analytic surface (not the degree-1 grid), and
ideally re-import → watertight. Same class likely affects Ruled/Offset — check during the fix.

## AUDIT FINDINGS (per module, as agents report)
### tessellation+render+drawing (a6ea025...) — DONE
Production tessellation dispatch is CORRECT — every surface type routes to a proper path
(incl. the just-fixed SurfaceOfRevolution wedge); NO faceting/wrong-path traps on live surfaces.
Bugs (all medium-or-lower, none on hot path):
- [MED] `tessellation/surface.rs:5930` `tessellate_surface` — DEAD placeholder ("uniform for now"),
  ignores params + index-skip desync bug when evaluate_full errs. Orphaned (0 callers) but `pub`. → remove-with-permission (NEEDS-VARUN) or `#[doc(hidden)]`/private.
- [MED] `render/viewpoint.rs:81-99` EYE-6 az/el axis mismatch: fibonacci lattice is Y-up but `az_el` reads elevation from Z-up index → returned az/el don't match `dir`. `dir` itself correct. FIXABLE (safe).
- [MED] `drawing/section_view.rs:117` reachable `.unwrap()` w/o `#[allow]+Reason` (lint policy). Dead path. FIXABLE (pattern-bind).
- [LOW] `surface.rs:5230` fillet boundary-resample can desync shared-edge samples → rare seam crack.
- [LOW] `drawing/projection.rs:13` no silhouette/tangent edges → curved parts lack outline (design gap).
- [LOW] `drawing/visibility.rs:283` ortho curved-edge occlusion grazing (only Iso mitigated).
- [LOW] `render/profile.rs:443,487` half-angle flat-start + symmetry-axis only-world-axes edge cases.
WIRING: tessellation fully wired. DEAD: `tessellate_surface`, `render::render_solids_with_labels`
(mod.rs:430), entire `drawing/section_view.rs` pipeline (only a test calls it; REST/MCP `section_view`
is the render-based `operations::section`). → dead-code list for NEEDS-VARUN (rule 5: no delete w/o permission).

## SYNTHESIZED WORKLIST (from audits — 2026-06-21 night)

### A. SAFE FIXES — do autonomously, each gated (test→commit→push). Each mirrors a known-correct sibling or is a clear guard.
- [ ] A1 `math/bisection.rs:59-61` `bisect_root` returns NaN midpoint as success → return None on non-finite f(mid). PRODUCTION-reachable (NURBS inversion nurbs.rs:1732/1780). TEST: bracket with NaN mid → None.
- [ ] A2 `primitives/surface.rs:3200-3208` `Cone::offset` recomputes half_angle (wrong, NaN-prone) → mirror `offset_exact` (3210): keep half_angle, shift apex along axis. TEST: offset cone half_angle unchanged, apex moved.
- [ ] A3 `primitives/curve.rs:3824,3845` `Ellipse::evaluate_derivatives` never pushes result[0]=position (order 0 → empty vec, all orders shifted) → push position first like every other curve. Also fix order≥3 axis trig (alternate cos/sin). TEST: evaluate_derivatives(t,0)[0]==evaluate position.
- [ ] A4 `math/nurbs.rs:1084-1100` `insert_knot` checks `mult>=p` but inserts `times` → check `mult+times>degree` like `insert_knot_u` (2516). TEST: insert beyond degree rejected.
- [ ] A5 `primitives/curve.rs:4027` `Ellipse::check_continuity` `acos(dot)` unclamped → NaN at dot≈1 → clamp(-1,1). (math/continuity_analysis.rs:316 also: `(1-dot).acos()` should be `dot.clamp().acos()` — orphaned but fix.)
- [ ] A6 `math/nurbs.rs:585-589` `evaluate_derivatives_simd` 2nd-deriv ÷W² should be ÷W (scalar twin :312 correct). Orphaned (bench only) but wrong. TEST vs scalar on rational curve.
- [ ] A7 `math/vector3.rs:553-558` `signed_angle` antiparallel → signum(0)=0 → returns 0 not ±π; clamp + handle. Orphaned, low pri.
- [ ] A8 `primitives/curve.rs:1089` `Arc::evaluate_derivatives` order≥4 odd: y-sign wrong. Low pri.
- [ ] A9 `primitives/vertex.rs:631` `VertexStore::remove` decrements total_created on already-DELETED → u64 underflow. Guard.
- [ ] A10 `shared-types/src/traits.rs:278` `From<String> for GeometryId` fabricates UUIDv5 from garbage → should error (mirror `from_string` :267). And M3 dup `ExportFormat` (commands.rs:145 vs geometry_commands.rs:852 — FBX drift).
- [ ] A11 Update `roshera-backend/CLAUDE.md` "Module reality check": the `protocol/timeline_handlers.rs`/`geometry_handlers.rs` "~30 orphaned handlers" note is STALE (files deleted; all WS inlined in message_handlers.rs). Doc-only.
- [ ] A12 STEP export facets SurfaceOfRevolution (ros_snapshot.rs:769 — see Known item). Medium; do carefully w/ round-trip test.

### B. NEEDS-VARUN — too big / risky / a design call for unsupervised overnight. Do NOT guess.
- B1 ★ `primitives/validation.rs:474-567` geometric validation (Standard/Deep) is a STUB that stamps `geometry_valid:true` without checking — the central "kernel can certify invalid geometry" defect. Also: torn shell (boundary edge) downgraded to warning not error (725-785); `analyze_edge_usage` uses `0..len()` on holey ids (498); orphaned `check_face_orientations`/`validate_pcurve_references`/certificate (1229-1362, the "SHA256" is SipHash+SystemTime, doc false). HUGE — the verification moat. Design + scope with Varun.
- B2 ★ `api-server/src/csketch.rs:1534-1647` sketch→3D bridge facets EVERY curve (arc/circle/ellipse/spline → 64-gon polylines) before B-Rep — STEP/booleans see prisms. Kernel CAN build NURBS edges (spline2d.rs correct). The headline product defect (task #9). Big bridge rewrite.
- B3 `constraint_solver.rs:669,740` dimensional constraints (Diameter/Length/Area/...) silently return 0 residual yet count as DOF-removed → solver lies about constrainedness. Implement real residuals (math + care).
- B4 `math/surface_intersection.rs:667-735` SSI corrector ignores predicted advance + reseeds from domain midpoint → branch-hops on cylinders (the #35 cyl-cyl saddle root). Deep, known (boolean-cyl-cyl-saddle-35).
- B5 RBAC dead/bypassed: `session-manager/permissions.rs` `check_permission`/`can_access_object` zero prod callers; api-server auth off by default. Security posture = design call.
- B6 Two sketch systems parallel (live click-to-place has NO solver; csketch/MCP has it) — unify? (tasks #8/#14). Design.
- B7 Spatial queries (point/field/region/relational in `queries/`) implemented + sound but ORPHANED from API/MCP — the "inhabitable substrate" moat is unreachable by agents. Wire to MCP/REST (medium-big).
- B8 `EdgeStore::add` (edge.rs:855) fast path skips indexes → `find_edge_between`/`edges_on_curve` empty for ALL primitive edges. Pervasive; fixing = perf/correctness tradeoff (index every add?). Scope w/ Varun.

### C. ORPHANED / DEAD (mostly remove-with-permission — rule 5; note, don't delete autonomously)
- `tessellation/surface.rs:5930` `tessellate_surface` (buggy placeholder), `render/mod.rs:430` `render_solids_with_labels`, `drawing/section_view.rs` pipeline, `ai-integration/src/providers/universal_endpoint.rs` (608-line dead dup), `api-server` `full_integration_executor` field + `send_collaborators_update`, math SIMD suite + `trimmed_nurbs.rs` + several exact_predicates, `timeline_impl.rs` `merge_branches_alt`/`replay_from` (dangerous if called), WS `BranchManager` path (H3 split-brain).
- rag-engine: DOES NOT EXIST (only CLAUDE.md aspirational + a Dockerfile stub that would fail a clean build). Note for Varun.
- api-server `/api/metrics`: command/perf trackers never written (record_* uncalled) + hardcoded DB metrics → wire record_* or remove.

## MORE AUDIT FINDINGS (operations, math, primitives, sketch, timeline, ai, api — in)
### operations (adae...) — critical fallback defects
- B9 CRIT `operations/boolean.rs:3881-3929` `merge_connected_curves` SINGLE-curve branch DISCARDS the marched intersection geometry and emits a dummy `Line(ORIGIN→+X)` (multi-curve branch just below is correct). Hit by every non-analytic boolean fallback (NURBS×*, off-axis cyl-sphere, general cyl-cyl). Boolean imprints garbage/drops the cut. → NEEDS-VARUN (deep, the boolean core).
- B10 CRIT `operations/sweep.rs:1230` `validate_swept_solid` is an Ok-but-does-nothing stub (others run validate_solid_scoped) → bad sweeps certify clean. SAFE-ish fix: call validate_solid_scoped like extrude/revolve. (sweep also has the rail/twist degradations below.)
- A13 `operations/sweep.rs:374-377` `sections.len()-1` usize underflow panic on empty frames → guard. SAFE.
- A14 `operations/transform.rs:283/299/321/343` `entity_ids[0]` no-bounds panic on empty Vec → guard. SAFE.
- A15 `operations/revolve.rs:2150` accepts angle up to 4π → self-intersecting solid; clamp/reject >2π. SAFE.
- A16 `operations/offset.rs:357` `create_offset_cone` divides by sin(half_angle) no guard → apex at ∞. SAFE guard.
- B11 HIGH `operations/revolve.rs:1352` `create_helical_sweep` (pitch≠0) non-manifold + falsely Closed (no caps). NEEDS care.
- B12 HIGH `operations/transform.rs:96` surfaces NOT transformed when update_parameterization=false → stale analytic faces. NEEDS care (why is the flag there?).
- B13 `operations/sweep.rs:345/632` Rail/BiRail/MultiGuide ignore guides (plain sweep); twist about world-Z not path tangent. + blend.rs:375 G2/G3 silently → degree-1 ruled. "Silently-degraded features" → NEEDS-VARUN (should error or implement).
- WIRING: `sweep_profile` has NO REST/MCP endpoint (only timeline replay). section/pattern thin recording.
### others (summarized; details in B-list above): primitives validation STUB (B1), sketch facet bridge (B2), dim-constraints no-op (B3), SSI corrector (B4), RBAC dead (B5), queries orphaned (B7), edge-index skip (B8). math: bisect_root/Cone/insert_knot/ellipse = SAFE (fixing now). api-server: solid, /metrics dead trackers, CLAUDE.md stale (A11). timeline: core sound; merge_branches_alt/replay_from orphaned-dangerous, session leak (H4), RBAC dead. ai: LLM fine, rag-engine DOESN'T EXIST, NL Boolean/Transform half-wired.

## A12 (STEP SurfaceOfRevolution export) — DEFERRED to a focused session (NOT attempted overnight)
Why deferred: highest-visibility fix (Varun's FreeCAD screenshot) where a subtle error ships a
STEP that LOOKS fixed but isn't; the clean path has real surface area (new SurfaceData variant +
extract arm + to_model arm + extracting an inline curve-builder to a reusable helper + a new
writer arm + an AXIS1_PLACEMENT emitter + a round-trip test) — not safe to land context-saturated
overnight without Varun able to verify the round-trip. B3 (ref_direction) — the OTHER STEP export
bug — IS fixed + pushed, so X-axis analytic surfaces now export correctly.
COMPLETE FIX PATH (for the focused session):
  Approach A (exact, recommended): emit STEP SURFACE_OF_REVOLUTION (importer ready, swept.rs:84/180).
   1. ros_snapshot.rs:124 add `SurfaceData::SurfaceOfRevolution{axis_origin:[f64;3],axis_direction:[f64;3],profile:CurveData,angle:f64}`.
   2. EXTRACT the inline CurveData→Box<dyn Curve> match (ros_snapshot.rs ~388-437, the to_model Curves loop) into `fn build_curve_from_data(&CurveData)->Option<Box<dyn Curve>>`; reuse it in the loop AND the new surface arm.
   3. extract_surface_data arm (before fallback :842): downcast SurfaceOfRevolution (fields axis_origin/axis_direction/profile_curve/angle, surface.rs:5036), profile via extract_curve_data(&*sor.profile_curve).
   4. to_model surface arm (:447 match): build_curve_from_data(profile) + SurfaceOfRevolution::new(pt(axis_origin),vec(axis_direction),profile_curve,angle).
   5. writer.rs write_surface arm (:507): id_curve=self.write_curve(profile)?; id_axis1=self.write_axis1_placement(origin,axis)?; writeln SURFACE_OF_REVOLUTION('',{id_curve},{id_axis1}); + add write_axis1_placement (AXIS1_PLACEMENT = a CARTESIAN_POINT loc + a DIRECTION axis; analogue of write_axis2_placement_3d:211 minus ref_dir).
   6. ALSO route an Arc profile through to_nurbs() (B4) so a partial-revolve arc isn't emitted as a full CIRCLE.
   7. TEST: build a revolved solid (revolve_profile, curved meridian → SurfaceOfRevolution face), export_step, assert output contains "SURFACE_OF_REVOLUTION"; re-import via import_step_content → watertight.
  Approach B (alt): A8.1 rational NURBS surface of revolution → SurfaceData::Nurbs (fewer plumbing pieces, but silent-math risk — needs an evaluate-vs-SurfaceOfRevolution test).
ALSO (same fallback trap, do alongside): B2 RuledSurface + OffsetSurface faceted; Ellipse CURVE has no extract_curve_data arm (→ to_nurbs).

### frontend+mcp (re-audit ac3e99...) — DONE, wiring is STRONG
- Frontend↔backend + MCP↔backend contracts fully consistent: NO broken endpoints, NO param mismatches (~22 frontend clients + ~60 MCP tools checked against the route table). api-server routes 100% wired to real handlers.
- ONLY real bug: [MED] CADMesh.tsx:87-154 Three.js geometry+material built in useMemo, never disposed → VRAM leak under edit churn. Fix: useEffect(()=>()=>{geometry.dispose();material.dispose();},[geometry,material]). (Frontend — left for Varun to build-verify per the no-npm-build rule.)
- Minor: dead export MIME glTF/FBX/IGES (export-api.ts:23, 501 if used); backend assembly explode/interferences/simulate endpoints have no UI caller.

## B1 — VALIDATION MOAT — scoped + ready to implement (Varun chose "both consistency + self-intersection")
THE STUB: `primitives/validation.rs` — `GeometryValidationResults` (L854) + `DeepValidationResults` (L857) are EMPTY unit structs; `validate_geometry_parallel` (L474) + `validate_deep_parallel` (L483) return them empty; `combine_results` (L535) hardcodes `geometry_valid: true` (L566). Dispatch: geometry runs at `level >= Standard` (L354), deep at `level == Deep` (L365). Error type: `ValidationError::GeometryError { message: String, location: EntityLocation }` (L102); `EntityLocation { solid_id, shell_id, face_id, loop_id, edge_id, vertex_id }` all `Option`. Point-on-surface: `Surface::contains_point(&Point3, Tolerance) -> bool` (surface.rs:637) — EXACT for analytic (Plane/Cyl/Cone/Sphere/Torus via exact closest_point), UNRELIABLE for NURBS/Ruled/Revolution/Offset (iterative closest_point, false-negatives at seams → would WRONGLY fail valid curved geometry). Edge fields: `edge.start_vertex`/`end_vertex`/`curve_id`/`param_range`; `face.surface_id`/`outer_loop`/`inner_loops`; `surface.type_name()`.

IMPLEMENTATION PLAN (gated, incremental, CALIBRATE before flipping geometry_valid):
- SLICE 1a (machinery + ANALYTIC consistency): change `GeometryValidationResults` to `{ errors: Vec<ValidationError> }` (update the `else` literals at L361/L372 to `::default()`). Implement `validate_geometry_parallel`: par over faces; for faces whose `surface.type_name()` is analytic, check each loop edge's endpoint vertices AND a few curve samples (`curve.evaluate(param_range mid/quarters)`) satisfy `surface.contains_point(p, tol)` → else push `GeometryError{face_id,edge_id,...}`. Wire `combine_results`: extend `all_errors` with `geometry.errors`, set `geometry_valid = (geometry-error count == 0)` (NOT hardcoded true). 
- ★ CALIBRATE: build, run the lib + a representative integration set, DIFF the new failures vs the 27-red baseline. Analytic contains_point is exact so false-positives should be ~0; any new failures on KNOWN-GOOD analytic solids → tighten/loosen tol or fix the check. Report the flagged set to Varun before promoting.
- SLICE 1b (CURVED consistency): for NURBS/Revolution/Ruled/Offset, don't trust closest_point — instead sample the edge curve and the SURFACE's own boundary/param grid and check the edge lies within the face's trimmed param region by direct (u,v) sampling + position compare; conservative tol.
- SLICE 1c: planar-face flatness (downcast Plane: |dot(p-origin,normal)| < tol for loop pts) + degenerate/zero-area faces.
- SLICE 2 (SELF-INTERSECTION, #24): pairwise face-face SSI for non-adjacent faces (BVH-broadphase to cut O(n²); reuse math SSI). Any real crossing between non-edge-sharing faces → GeometryError. This is the headline "can't lie" case (the 10mm shell). Heaviest slice; do after 1a/1b land + are calibrated.
RISK: stricter validation will surface genuine defects (incl. some of the 27 red tests) — that is the POINT, but keep analytic-exact first so geometry_valid never lies AND never false-fails. Recommend implementing as a FRESH focused pass (not saturated) given the moat-criticality.

## PROGRESS LOG (append-only)
- 2026-06-22 (Varun awake): A12 STEP SURFACE_OF_REVOLUTION export (5090eaf, backend rebuilt+live on 8081 for FreeCAD verify); small-safe sweep A3/A9/CADMesh (7650d16, c961af5); CI investigated (red, 27 tests, mostly #17).
- ★ B1 SLICE 1a DONE+PUSHED (a2afa7b): geometric-consistency validation — validate_geometry_parallel now checks every analytic face's edges lie on its surface (exact closest_point); geometry_valid is HONEST (no longer hardcoded true). FINDING: on its FIRST run it caught a genuine pre-existing defect — fillet_edges produces trim edges that sit measurably off the planar face they border. Landed as ValidationWarning::GeometryInconsistency (edge+distance) NOT errors, so geometry_valid is honest WITHOUT breaking ops that validate their result. lib 3847 pass (only the 2 pre-existing).
- ★ B1 SUBSTANTIALLY COMPLETE (the verification moat now inspects GEOMETRY, not just topology):
  - 1a analytic edge-on-surface consistency (a2afa7b) — caught a real fillet trim-edge defect on first run.
  - 1b curved-surface consistency (conservative (u,v)-grid upper-bound) + 1c degenerate faces (e0e0560).
  - Slice 2 (a5bf9ac): self_overlapping_planar_faces #70 (Standard) + mesh_self_intersects #24 (Deep) — closes the "shell self-intersection isn't in the cert" gap.
  - All feed an HONEST geometry_valid; surfaced as ValidationWarning::GeometryInconsistency (located + distance) so the cert stops lying WITHOUT breaking ops.
- B1 FOLLOW-UPS (open): (1) ★ FIX the fillet/chamfer trim-edge-off-plane defect B1 found → then PROMOTE the consistency findings from warnings to BLOCKING errors (catch→enforce). (2) finer pcurve-based curved-surface check (replace the coarse grid). (3) manufacturing_valid still hardcoded true (separate moat dimension). (4) consider running #24 self-intersection at Standard for ops that opt in.
- ★ #70 CHAMFER-CROSSES-FILLET (the defect B1 exposed; taken on as a dedicated task) — 2 real defects (2 deep investigations):
  - DEFECT 1 (cap off-plane) FIXED: C0 corner cap was a flat Plane through the 3 endpoints but two rims are fillet arcs bulging ~0.24 off it (cap_vertices_coplanar chamfer.rs:2621 short-circuits ≤3 corners, never tests arc midpoints). FIX: when any cap rim is an arc, fillet.rs:5902 routes to the curved G1 builder (interpolates the arcs exactly) not the planar one. Gated by mixed_kind_1c2f_corner_cap_arc_rims_on_surface_70. TODO: same branch at the 2nd fillet dispatch + chamfer.rs:2923 (chamfer-closes order).
  - DEFECT 2 (faces 3/5 self-overlap) = the deeper #72, NOT fixed. On a cube face bordering two corner-blended edges, the two inset trim tracks run FULL-span and cross in a bowtie at their intersection (e.g. (4,5,4)). self_overlapping_planar_faces is CORRECT (real defect). Canonical pinned test: harness/integration.rs:562 chamfer_crossing_fillet_self_overlap_pinned_70 (#[ignore]).
    ★ PROVEN (2026-06-22, worktree-implemented+empirically tested): the contained "retrim + junction vertex" post-pass is GEOMETRICALLY INSUFFICIENT — (a) the trim rails are NurbsCurve not Line (downcast::<Line> = silent no-op; fixable), and (b) FATAL: the rails are SHARED edges (e17 ∈ planar face3 AND fillet face7; e20 ∈ face3 AND fillet face8), so rewire_edge_vertex (in-place param_range retrim) moves the edge on BOTH faces → tears the fillet face loop → solid goes Euler-invalid (V−E+F=3, non-watertight). Reverted, no regression shipped.
    REAL FIX = corner-construction REDESIGN (not a post-pass): the mixed-corner path DELIBERATELY clears the V-side setbacks (blend_graph::clear_setbacks_at, fillet.rs:572) so fillet arcs run full-length to the cap-rim offset points, letting the simpler CF-γ.6 cap connect — the bowtie is that simplification's residual cost. Either (a) STOP clearing setbacks for these corners + teach the cap synthesizer to bridge the setback junctions, OR (b) true loop-aware rail SPLIT at J across ALL incident faces (planar + both fillet faces + cap) + rebuild the cap rim to the junctions. Multi-file core blend/cap change — poke-matrix + watertight gated, verify by RENDERING the corner. Focused human-guided kernel session (touches the same risk surface as #35/#17/the 27 CI failures). Then un-ignore integration.rs:562 + flip the regression test to require self_overlapping_planar_faces==[].
    ★★ UPDATE (2026-06-22, 2nd worktree): option (a) restore-setbacks is ALSO PROVEN BLOCKED. The chamfer runs FIRST + fixes its cap-rim at the full offset + is RETRACTION-UNAWARE (chamfer.rs:1708-1728). Full apex retraction (setback=r, anchor (4,4,4)) kills the bowtie BUT the retracted fillet arcs (5,4,4)/(4,4,5) no longer meet the chamfer rim → planar cap can't close (hard BlendFailed); zero retraction closes the cap but keeps the bowtie; Hoffmann 0.707r keeps it too. MUTUALLY EXCLUSIVE. REAL FIX = a genuine multi-patch VERTEX-BLEND primitive (excise corner triangles + bridge apex-retracted arcs to the chamfer rim across non-coplanar planes) + a retraction-aware chamfer = MULTI-WEEK kernel project, not a session task. WIN CAPTURED: the moat flags geometry_valid=false (kernel honest, no silent hole = the "can't lie" thesis). Interim product call: 1C2F fail-loud with VertexBlendUnsupported vs leave it flagged.
    ★ BUILD UNDERWAY (2026-06-22, staged — Varun: "unblock it with the new code"):
    - STAGE 0 DONE+COMMITTED (dcf4a99): compute_apex_setbacks_mixed + solve_corner_junctions + R1 de-risk PROVEN (re-anchoring a planar bevel's V-end vertices + chord replacement keeps the loop closed + face planar → chamfer-rim rebuild is topologically SAFE, no full bevel re-synthesis). blend_graph.rs +1192, 5 tests, all additive.
    - STAGE 1 KEY FINDING (not shipped — IMPEDANCE MISMATCH): the live #70 path is chamfer-FIRST, fillet-SECOND, so by fillet-time the chamfer edge is GONE (splice_blend_edge) → the fillet's corner is ConvexCorner{degree:2}, NOT degree:3. Stage 0's solvers filter degree:3-with-3-edges → NO-OP on the live corner (compute_apex_setbacks_mixed skips; solve_corner_junctions → MissingTopology on the absent chamfer edge). BUT the MATH IS RIGHT: Stage 0's J1=(5,5,4)/J2=(4,5,5)/P12=(5,4,5) EQUAL the live rim corners v11/v9/v12 — the solvers just need POST-surgery inputs (chamfer bevel-face plane via Plane::from_three_points + the 2 fillet cylinder descriptors), not the 3 original sharp edges.
    - EDGE-BUDGET WALL (confirms apex-RETRACTION is THE mechanism): the 3 cube-notch chords (triangle v9-v11-v12) are ALREADY manifold-2 (e13=chamfer↔cap, e19/e22=fillet↔cap); closing a notch with them → manifold-3 (invalid), with a new edge → manifold-1 (hole). Only retracting the fillet V-caps inward (apex setback) OPENS a real gap the cap bridges with NEW manifold-2 edges. Baseline mesh: euler=-3, boundary_edges=90 (genuinely non-watertight, real holes).
    - RE-SCOPED PATH (3 deliverables, each gated): #1 a DEGREE-2 POST-SURGERY apex/junction solver (post-chamfer bevel face + 2 fillet cylinders, not 3 edges; ~120 lines reusing predict_corner_blend_axis/least_squares_concurrent_point; unit-test J/P=live rim corners) — ✅ DONE (2bfd8aa): solve_corner_junctions_post_surgery + reconstruct_bevel_plane (RuledSurface→Plane::from_three_points) + fillet_cap_spine (live fillet face is CylindricalFillet not Cylinder → spine+radius). Test reproduces apex (4,4,4) + J/P = live rim corners (5,5,4)/(4,5,5)/(5,4,5) exactly; returns Ok on the real degree-2 input. blend_graph 22/22, no regression. NEXT. #2 apply setbacks in the fillet chain (fillet.rs:563, 1C2F-gated) → retract V-caps + open gaps — RISKS the shared rails, MUST be poke_matrix-gated. #3 excise cube-face triangles + rebuild cap on retracted rims (trim_cap_arc_in_place + reanchor + R1 chord-replace + insert_cap_into_face_loop→pub(crate)).
- 2026-06-21 night: launched 10 audit agents; created this campaign; STEP-export faceting diagnosed.
- All 10 audits in. 9 safe fixes committed+pushed (fc7c185, a5c5021, b45280e). A12 deferred (fix-path documented). Morning summary written at top.
- 2026-06-22 (Varun awake, "sweep the small safe ones"): A3 ellipse evaluate_derivatives + A9 vertex remove underflow (7650d16), CADMesh Three.js dispose leak (c961af5). 11 fixes total now pushed.
- CI INVESTIGATION (the "CI gate" item): gate already exists + ENFORCED, but CI is RED — 27 failing tests, dominated by #17 NURBS corefinement (~13). This IS the remaining real backlog (see the corrected note above). The small-safe sweep is now COMPLETE; everything left is the deep B-list / #17.
- tessellation audit: no hot-path bugs; safe fixes queued; dead-code → NEEDS-VARUN.
- 8/10 audits in (export+frontend rate-limited, re-run pending). Synthesized worklist above.
- ✅ BATCH 1 COMMITTED+PUSHED (fc7c185): A1 bisect_root NaN→None, A2 Cone::offset apex-shift, A4 insert_knot mult+times, A5 ellipse continuity acos clamp. geometry-engine lib 3847 pass.
- ⚠ PRE-EXISTING failures found (fail at HEAD with batch stashed — NOT mine, NEEDS-VARUN):
  P1 `render::profile::nozzle_profile_measured_dims_match` (profile.rs:968 throat NOT detected — the render/profile dominant-half-angle/symmetry-axis bugs the tess audit flagged).
  P2 `harness::tessellation_oracle::refining_tolerance_converges_volume_and_adds_triangles`.
  (pre-commit only compiles, never ran these → they rotted unnoticed. The CI/test gate doesn't run lib tests.)
- ✅ BATCH 2 COMMITTED+PUSHED (a5c5021): A13 sweep empty-sections guard, A14 transform 4× empty-entity_ids guards, A16 offset_cone sin~0 guard, A11 CLAUDE.md stale-doc fix. lib 3847 pass (same 2 pre-existing). Deferred A15 (helical >2π nuance).
- Remaining A-list mostly deferred (uncertain/low): A3 ellipse derivs (deriv-vector convention), A7/A8 (orphaned low-pri), A9 (VertexFlags API), A10 (From<String> trait can't error — semantics → NEEDS-VARUN), A12 STEP export (medium, do carefully). B12 confirmed: transform.rs:96 surfaces only moved if update_parameterization (intentional flag? → NEEDS-VARUN).
- Re-running the 2 rate-limited audits ONE AT A TIME to finish wiring coverage (Varun: "figure out if everything is wired"): export ✅, then frontend+mcp (still pending).
### export-engine (re-audit a2fc7c...) — DONE
- ★ A12 FIX-PATH (clean, low-risk): SurfaceOfRevolution STEP export → emit a real STEP SURFACE_OF_REVOLUTION entity (importer already handles it: step/handlers/tier2/swept.rs:84/180). Steps: (1) add `SurfaceData::SurfaceOfRevolution{axis_origin,axis_direction,profile:CurveData,angle}` to ros_snapshot.rs:124; (2) extract_surface_data arm (before fallback ~842): downcast SurfaceOfRevolution, serialize profile via the EXISTING extract_curve_data(&*sor.profile_curve); (3) writer.rs write_surface arm: self.write_curve(profile) + new write_axis1_placement(origin,axis) (3-line analogue of write_axis2_placement_3d:211, emits AXIS1_PLACEMENT) + emit `#id=SURFACE_OF_REVOLUTION('',#curve,#axis1);`; (4) add the to_model reverse arm (ros_snapshot.rs:447, SurfaceOfRevolution::new) so the enum match stays exhaustive + ROS round-trips. NO rational-circle math, reuses curve plumbing. GATE: export a revolved solid, assert STEP contains "SURFACE_OF_REVOLUTION" (not degree-1 B_SPLINE); ideally re-import→watertight. NOTE: a partial-revolve ARC profile would be emitted as a full basis CIRCLE (B4) — route Arc profiles through to_nurbs() (exact rational, curve.rs:3677) when serializing the profile.
- B2 (same fallback trap): RuledSurface + OffsetSurface also faceted to degree-1 grid (extract_surface_data downcasts only Plane/Cyl/Sphere/Cone/Torus/GeneralNurbs). + Ellipse CURVE: extract_curve_data (ros_snapshot.rs:655) has no Ellipse arm → 20-pt polyline (Ellipse has exact to_nurbs curve.rs:3994). → add arms (use to_nurbs) — medium.
- ✅ B3 FIXED (this batch): writer.rs:211 write_axis2_placement_3d hardcoded ref_direction [1,0,0] → degenerate STEP for X-axis Plane/Cyl/Cone/Torus (OCCT/FreeCAD reject). Now Gram-Schmidt orthogonalizes the ref_dir vs the axis + fallback. Affects ALL analytic surface exports.
- B5 coord precision {:.6} below declared 1e-7 uncertainty + truncates knots/weights (→ {:.9}); B6 ROS edge ParameterRange always reset to unit() (range never serialized); B7 binary STL normal not normalized. LOW.
- WIRING: STL/OBJ/ROS/STEP-export all wired via POST /api/export; STEP-import via /api/geometry/import_step + MCP import_step. No MCP export tool (exports REST-only).
