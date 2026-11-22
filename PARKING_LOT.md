# Parking lot (90-day workplan, 2026-11-22 → 2027-02-20)

Triage rule: *does this block a pilot part or make a pilot drawing/DFM
verdict wrong?* Yes → fix it. No → it goes here, unopened for 90 days.
Plan: `docs/roshera-drawing-dfm-plan.pdf`; status against it:
`docs/workplan-2026-11-22-status.md`.

## Parked by the plan
- 3D CAD feature expansion (sweeps, variable fillets, complex lofts)
- FEA / physics simulation and the meshing feedback loop
- Omnigraph / PLM work-graph trials
- AI-lab RL corpus (roshera-rl) and multi-tenant cloud
- Big refactors (boolean.rs / main.rs split)
- Website hero animations and shodh-* repos
- Generalist VC cold outreach (until the Volvo PO)
- NURBS authoring and NURBS×NURBS booleans (keep only STEP-import speed)
- Sketch ARCH-1..5, polyline solver (fix SK-H2 / SK-M2 only for a live demo)

## Parked from the work list (found after the plan)
- DFM sheet-metal pack beyond what shipped (8be09cc1): bend relief,
  K-factor flat pattern, hole-to-hole spacing — unless the drone supplier
  sends bent parts
- DFM casting pack beyond what shipped (8be09cc1)
- Drawing sheet orientation switch after creation (PATCH verb) and an app
  sheet selector (7eaae786 chooses orientation at creation only)
- Drawing lettering A0–A2, duplicate RIGHT view, characteristic numbers
- MCP KB `general_tolerance` still cites ISO 2768-2 (cancelled; ISO
  22081:2021) — fold into the P1 standards-library task, not before
- Vault KB text for CNC / sheet metal / casting packs (says "cannot run")

## Carried from older docs (2026-11-22 doc consolidation)

Open items from older backlogs, handoffs and audits, kept here so none is
lost if those docs are deleted. "Unverified" means no landing commit was
found and the code was not re-read: check before working on one.

**Sketch** (`docs/backlog-2026-11-06-kernel-trust.md`; the plan parks all
but SK-H2 / SK-M2):
- SK-H1 zero-length direction satisfies `angle_residual`: still open
  (`sketch2d/constraint_solver.rs:2822` returns `[0, 0]`)
- SK-H2 failed solve still overwrites the sketch; no NaN guard (unverified)
- SK-H3 infinite/ray lines counted at 4 DOF (unverified)
- SK-M2 vertex hash saturates: still open (`sketch2d/sketch_topology.rs:409`
  divides by `tolerance.distance`, default 1e-10, and casts to `i32`)
- SK-M1, SK-M3, SK-M4..M8, SK-J analytic Jacobians, SK-P1..P9 parity block,
  ARCH-1..7, UX-0..9 and the backlog's "Low" list (unverified)

**Kernel / queries / import** (same backlog; unverified unless noted):
- SDF-2 never-overestimate distance; SDF-3 BVH for nearest/raycast;
  BM-1/BM-2 batch marching
- RAY-1: torus now intersects (`queries/raycast.rs:280`); NURBS / ruled /
  offset surfaces not re-checked
- TESS-1 topology-first fallback mesher; TESS-2 split mesh quality out of
  `is_sound` (decision for Varun)
- BOOL-1 torus cut through its equator seam; BOOL-2 cone cut tangent to its
  rim; BOOL-3 plane∘torus oblique arm; BOOL-CONE-1 box minus an overshooting
  cone (countersinks); BOOL-COAX-1 coaxial cutter through plate+boss
  (`docs/campaign-2026-11-12-drawings-production.md`)
- STEP-2 GPU instancing (STEP-3, STEP-IMP-1 and the CADGenBench import
  fixes are in `docs/workplan-2026-11-22-status.md`)
- IMP-2 tail: the 5e-3 mass-properties mesh bound is empirical; exact
  per-face integration is the real answer
- UI-1 tree polish; AGENT-1 `assembly_tree` MCP tool; DRW-1 assembly drawings

**Certificate speed** (`roshera-eval/speed/FAST_EXACT_CERTIFICATE.md`):
items 5, 7–10 (part-relative element size, self-intersection on the
certificate mesh, `CertMesh`, per-face cache, analytic distances in the
repairs). Item 6 is in the status doc.

**MCP / single registry** (`roshera-eval/mcp/`):
- Single-registry stages 2–8 (gates behind dispatch, read tools, mutating
  core, drawing/GD&T, search/resources/progress, in-process app agent)
- MCP_AUDIT open rows: 1.10 registry drift (ratchet at 23 differences),
  non-atomic composites (`drill_pattern`, `boolean_many`, `sketch_points`,
  `timeline_branch`, `drawing_set_title`), remaining bare-status handlers,
  naming the recorded timeline step

**App / human-agent parity** (`roshera-eval/ui/`,
`docs/handoff-2026-11-14-ui.md`, `docs/2026-08-22-agent-surface-open-work.md`;
unverified):
- Parity gaps 2–5, 7–9: agent assembly edits not broadcast; assembly
  instances and mates barely editable; no history view in the Assembly
  workspace; feature edit is numbers only; no reorder / rename / un-delete /
  reject-branch; agent GD&T and labels read-only in the app; undo is one
  step, not one feature
- Feedback loop §b pick mode both ways, §c accept/reject/edit at step k,
  §d parity rule; conversation UI design §2, §3, §6, §7
- Drawing and Assembly workspaces restyled to the mockups; properties
  panel restyle; Hole tool (kernel route + UI)
- Blackboard run-folding and turn sections; ChoiceCard (needs structured
  `ask_choice` on the wire); lineage hover-to-seek and cluster collapse;
  the ACP fields the agent must send (`startedAt`/`endedAt`, `detail`,
  `parentToolCallId`, permission titles, choice delivery, progress)
- Chat box focus leaking keys to viewport shortcuts; MCP and app disagreeing
  on the active document; 70–120 s agent turns

**History / security** (`docs/audit-2026-09-03-status.md`; task numbers
refer to its git-ignored plan; unverified):
- History: 79 replay-stable solid ids (blocks 100b, a branch switch that
  rebuilds), 110 lanes that bypass the durable applied head, 74 truncate /
  clear memory-only, 78 merge duplicates ops, 102, 105–109
- Security / API: 111 app logout never calls the server, 73 login
  throttling keyed on a client header, 85, 86 SQLite vs Postgres
  divergence, 9, 28–31
- Kernel: 80 marching SSI returns truncated curves as Ok, 81 an open shell
  labelled exact mass, 82 malformed numbers become 0.0, 87, 90–92, 96–98,
  103, 50–64, 35
- MCP / app / eval / docs: 75–77, 83, 84, 88, 94, 95, 99, 10, 11, 19–25, 27
- Decisions for Varun: Task 89 dead-but-dangerous code (delete or fix);
  concave chord fillet sign; legacy Coincident/Concentric mate freedoms;
  RBAC for ~120 authenticated-but-unauthorized routes; the dead
  `/api/ai/command` surface

**Pilot app** (`docs/campaign-2026-11-12-drawings-production.md`): the
Tauri desktop app for Windows and macOS, built last.
