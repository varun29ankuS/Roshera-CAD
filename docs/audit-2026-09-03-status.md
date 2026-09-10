# Audit 2026-09-03 — status and remaining work

Paused 2026-09-08 at `46406039` on `main`, tree clean, not pushed. Resume at Task 50.

## How this campaign runs

Full-codebase audit (nine read-only auditors, 42 triaged findings) turned into a task list.
Each task is executed by one implementer at a time, reviewed by one reviewer against a
working-tree diff, fixed in scoped rounds, and committed by the controller with one commit per
task. Every fix lands RED-first with a mutation that turns its test red. Each task has surfaced
one to three new findings; those are filed as new tasks rather than fixed in passing.

Working files (git-ignored, on this machine only):

- Plan with every task's full text: `docs/superpowers/plans/2026-09-03-audit-fixes.md`
- Ledger with rulings, rounds and commits: `.superpowers/sdd/2026-09-03-audit-fixes/progress.md`
- Per-task briefs, reports, review diffs and commit drafts in the same directory.
  Briefs for Tasks 50-54 are already extracted.

## Landed on main (be50862e .. 46406039)

| Task | Commit | What changed |
|---|---|---|
| 1 | be50862e | permissions: the granter check reads its own answer |
| 2 | 98c2f9fd | auth: refresh tokens rotate; `aud` is checked |
| 3 | 446cab6a | database: role decoders no longer escalate; `try_get` cells |
| 4 | 5ac1ec6a | idempotency key scoped to principal, method, path and query |
| 5 | dca4586c | sketch extrude/revolve routes carry the permission layer |
| 6 | 34633f4c | checkpoint persistence failure returns typed and rolls back |
| 7 | 8d7cf896 | WebSocket create builds what it says (cone, torus, refusals, real id) |
| 8 | c34ed4f6 | export soundness gate shared by REST and WebSocket |
| 12 | a6cf84d2 | boolean-minted faces measure their UV domain; consumers refuse unset |
| 32 | a9d13a70 | every mint site measures its UV domain |
| 33 | cf874115 | tessellator re-cuts a seam-straddling window |
| 13 | 298864bc | chamfer on curved open edges uses interpolated rails |
| 14 | de202c7d | chord fillets are not fed as radii; function sampling |
| 15 | df4fd7ab | sketch DOF report runs its rank pass; `singular_configuration` |
| 16 | 0496b278 | mismatched constraints are refused, not silently satisfied |
| 17 | c71d9b4f | drawing certificate carries omitted facts; hole table honest on depth |
| 18 | 0f720225 | assembly: a refused sweep is not stored as clear |
| 26 | a0cad5c7 | harness: lint headers, `CaseGate`, ignored-reds script, parked state |
| 36 | f24fb8d1 | chamfer offsets honour the edge parameter range |
| 37 | caddb391 | fillet-face conforming stitch; cdt collinear fallback |
| 42 | ac476969 | loft registers correspondence between dissimilar sections |
| 41 | 780dfcab | assembly: unswept freedoms named; `OverlayWithoutJoint`; `NotRun` |
| 45 | fda8a3d4 | nine parked reds dispositioned |
| 38 | a782c766 | redundant-constraint witness deterministic across processes |
| 39 | aa15f441 | inference snap proposes the endpoint, not the line |
| 40 | cc646851 | explicit-scale drawing uses the automatic placement; typed refusals |
| 43 | e9341aae | watertight weld addresses its grid with checked arithmetic; refused vertices reach the certificate |
| 44 | d3685df1 | convex fan refuses a self-intersecting contour |
| 46 | 439ccbc2 | loft bands oriented from slab signed volume; closed-loft seam registered |
| 47 | b6add19e | tangent boolean that certifies unsound is refused (bisected to 1016a695) |
| 48 | 30e7a483 | sketch parameter columns walk in a stable entity order |
| 49 | f4eaa2e8 | ellipse closest point converges (certified bisection) |
| 34 + 34b | 46406039 | seam window is a clean loop; tessellator meshes it as a hole |

## Remaining tasks, in the order to take them

Kernel and harness first (Varun's standing order), then the API, MCP, eval, frontend and docs.

### Filed during execution (kernel / harness)

- **50** harness: `cargo fmt --all` overflows the Windows argument limit at 909 Rust files, so the
  pre-commit gate failed every Rust commit. The local hook now falls back to per-package checks;
  `scripts/setup.sh` and `scripts/test.sh` still use `--all`; the hook is untracked and no script
  installs it. Also: no rustdoc link gate (two broken intra-doc links passed every gate).
- **51** tessellation: a chord tolerance of zero or less bypasses the relative floor.
- **52** sketch2d: six saturating float-to-int grid loops can hang (~1.8e19 iterations).
- **53** harness: the certificate's own weld (`weld_key`) collapses non-finite vertices onto the origin cell.
- **54** tessellation: `max_edge_length` is inert under the `max_segments` clamp; `weld_mesh_watertight`
  is test-only; `fillet.rs`/`revolve.rs`/an example still mirror the certificate with bare literals.
- **55** tessellation: the weak-fan fallback trusts a cdt error class that is insertion-order dependent.
- **56** api-server + tessellation: a refused face is unattributed on the mesh and the fast verify
  verdict calls it "not a defect".
- **57** loft: `best_relabeling` admits a mirror reversal for a self-mirror section (symmetric L cannot loft).
- **58** tessellation: a non-convex planar cap tessellates torn depending on authored winding.
- **59** loft: cubic loft ignores `closed` yet labels its shell closed; `validate_result:false` ships unvalidated.
- **60** boolean: `acos(d/r)` goes NaN inside the tolerance band the chord criterion admits.
- **61** boolean: the Phase B dedup's own pins pass without it; the two sound tangent configurations
  were never probed at its parent commit.
- **62** sketch2d: decomposition and constrainment still order by entity uuid (serialized certificate
  fields differ across processes).
- **63** sketch2d + primitives: `InvalidParameter` renders "must be must be …" at 30+ sites (one batch).
- **64** sketch2d snap: a refused candidate is invisible to the API; the cursor is never validated;
  `Circle2d`/`Arc2d::closest_point` cannot refuse.
- **35** boolean: a second window at a shared height is never cut (saddle probe declines
  `foreign=true` before `split_cylinder_lateral_by_window`) and the first window's patch is
  re-emitted as its own face. RED is the ignored test in `geometry-engine/tests/tessellated_window.rs`.
  Needs a `near_outer_edge` Steiner keepout as part of the fix.

### Original audit tasks not yet started

- **9** api-server: a lagged WebSocket client is never told; metrics report a constant.
- **10** export-engine: `.ros` GEOM import drops topology while the signature reads Verified.
- **11** export normals and the unset source unit.
- **28** api-server: the permission grant handler discards the grant's answer.
- **29** api-server: every JWT is given the full default scope set.
- **30** api-server: the recorder flush is discarded at seventeen more sites.
- **31** api-server: the WebSocket ROS export writes a client-supplied path with no gate.
- **19** roshera-mcp: six raw fetches return 4xx as success and drop the part pin; KaTeX exemplar;
  psketch descriptions for `singular_configuration` and the coincident centre.
- **20** roshera-eval: knownRed blankets a regression guard; a crash is graded as unsound; oracle-10
  fixture for `NotRun`; scenario 10 accepted sweep sources.
- **21-25** roshera-app: Blackboard false displays; lineage view accuses the kernel; ObjectUpdated
  resets client state; stale caps and a fake ping; layout law (blur, docked aside, a gate that
  cannot fail); `SketchOverlay.tsx` commits before inferring.
- **27** docs: false safety claims and stale operator docs.

## Decisions waiting on Varun

- Concave chord fillet sign convention (currently a named refusal).
- Legacy `Coincident`/`Concentric` mate freedoms: naming them unverified makes every legacy assembly
  unsound with no acknowledgement path; deliberately excluded until one exists.
- RBAC mapping for the ~120 authenticated-but-unauthorized routes.
- The dead `/api/ai/command` surface.
- Push `main` (be50862e .. 46406039 are local only).

## Environment notes

- Rotate the OpenRouter key; drop `ROSHERA_DEV_INSECURE=1`; rebuild `api-server`; reconnect the MCP
  server (dist was rebuilt during Task 17).
- Opus subagents returned a 403 (`oauth_org_not_allowed`) once on 2026-09-08; dispatching on the
  session model worked.
- `cargo fmt -p <crate>` on this machine, never `--all`.
- Runs go one cargo at a time, foreground, chunked under ten minutes; agents that wait on background
  runs never wake up.
