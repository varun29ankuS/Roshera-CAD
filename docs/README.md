# Documentation index

Every Markdown doc tracked in this repo (outside `roshera-backend/vendor/`
and `node_modules/`), with one line on what it is for. Last verified:
2026-11-22.

Status tags: **current**: the doc of record for its topic. **cited**: dated,
but the plan of record refers to it by item ID, so it stays. **historical**:
a dated snapshot whose open items now live in `PARKING_LOT.md` or the
workplan status; a deletion candidate. **machine-read**: parsed by a script;
edit only by its own rules.

## Plan of record

| Doc | Status | Purpose |
|---|---|---|
| `docs/roshera-drawing-dfm-plan.pdf` | current | The 90-day workplan (2026-11-22): STEP → production drawing + CNC DFM for the pilots |
| `docs/workplan-2026-11-22-status.md` | current | Status against the plan, corrections to it, in-scope items carried from older docs |
| `PARKING_LOT.md` | current | Everything the plan's triage rule parks, including open items carried from retired docs |
| `docs/dfm-plan-2026-11-22.md` | current | DFM pack design: analyzers, CNC / turning / sheet-metal / casting rules and their defaults |
| `docs/campaign-2026-11-12-drawings-production.md` | cited | Drawings campaign P1–P8 (title block, completeness by rank, datum-first, standards library, pinned sheet edits), pilot app |
| `docs/backlog-2026-11-06-kernel-trust.md` | cited | Kernel-trust backlog; the plan cites its SK-* sketch items |

## Repo and contributor docs

| Doc | Status | Purpose |
|---|---|---|
| `README.md` | current | What Roshera is, the certificate, architecture, status table, performance, getting started |
| `CONTRIBUTING.md` | current | License of contributions (AGPL + dual licensing), DCO sign-off, ground rules |
| `CLAUDE.md` | current | Project rules for agents working in the repo |
| `roshera-backend/CLAUDE.md` | current | Backend rules, merge and disconnection gates, workspace layout, conventions |
| `roshera-agent-policy.md` | current | The rules every Roshera agent works under (delivered as AGENTS.md); a live prompt |
| `docs/DEPLOYMENT.md` | current | Runbook for a single-tenant partner stack (`docker-compose.partner.yml`), env vars, durability |
| `docs/README.md` | current | This index |

## Backend

| Doc | Status | Purpose |
|---|---|---|
| `roshera-backend/geometry-engine/KNOWN_REDS.md` | machine-read | Red-test ratchet read by `scripts/red-gate.ps1` |
| `roshera-backend/KNOWN_DISCONNECTED.md` | machine-read | Disconnected-capability ratchet read by `scripts/disconnection-gate.ps1` |
| `roshera-backend/geometry-engine/docs/burndown-diag-loft-family.md` | current | Diagnoses behind the KNOWN_REDS loft entries (KNOWN_REDS links its headings) |
| `roshera-backend/geometry-engine/src/primitives/PRIMITIVES_MODULE_GUIDE.md` | current | Tour of the primitives module's main files |
| `roshera-backend/geometry-engine/src/primitives/primitive_tests/EVIL_EDGE_CASES.md` | current | The six degenerate-topology tests in `topology_tests.rs` and why each matters |
| `roshera-backend/geometry-engine/PERFORMANCE_METRICS.md` | current (unverified numbers) | 2026-05-13 tessellation timings; math rows disagree with the README |
| `roshera-backend/geometry-engine/examples/README.md` | current | Kernel demo examples, the demo gallery refresh, their CI role |
| `roshera-backend/timeline-engine/README.md` | current | Event-sourced timeline crate: modules, usage, how to add an operation |
| `roshera-backend/api-server/src/protocol/README.md` | current | The ClientMessage / ServerMessage protocol carried over `/ws` |
| `roshera-backend/api-server/src/VIEWPORT_BRIDGE.md` | current | Dev-only bridge that lets a tool drive and snapshot the live viewport |

## Clients and tools

| Doc | Status | Purpose |
|---|---|---|
| `roshera-app/README.md` | current | Frontend: run, scripts, dev proxy, layout |
| `roshera-mcp/README.md` | current | MCP server: tools, surface modes and the funnel, CI budget gate |
| `roshera-rl/README.md` | current (parked by the plan) | Certificate-scored episode loop, ingestion, agent-eval harness, and what it does not prove |
| `scripts/ground-plan/README.md` | current | Generator for the isometric ground plan of the repo |
| `website/README.md` | current | roshera.tech: build, deploy, writing rules |
| `website/content/**` (14 files) | current | Published site pages and blog posts (left as-is) |

## Evaluation (`roshera-eval/`)

| Doc | Status | Purpose |
|---|---|---|
| `roshera-eval/README.md` | current | AGENT-EVAL-α: scenarios, score dimensions, oracles, how to run |
| `roshera-eval/drawing-bench/BENCHMARK.md` | current | Drawing benchmark method, baselines, the craft score and the layout model |
| `roshera-eval/drawing-bench/EDITING_DESIGN.md` | current | Drawing-editing design (dimensions, tolerances, GD&T on the sheet); basis for the plan's P2 |
| `roshera-eval/mcp/SINGLE_REGISTRY_DESIGN.md` | current | One tool registry owned by the backend; stages 0–1 built, 2–8 parked |
| `roshera-eval/mcp/PARITY_TEST.md` | current | The registry schema-parity ratchet test (stage 0) |
| `docs/archive/MCP_AUDIT.md` | historical | 11-17 MCP surface audit; most rows fixed in d7f8dba2, the rest parked |
| `roshera-eval/speed/FAST_EXACT_CERTIFICATE.md` | current | Certificate speed analysis; items 1–4 landed, 5–10 open |
| `docs/archive/DIAGNOSIS_217_211_249.md` | historical | CADGenBench import diagnosis; open fixes carried to the status doc |
| `roshera-eval/ui/CONVERSATION_UI_DESIGN.md` | current (parked) | Blackboard conversation design; first slice landed |
| `roshera-eval/ui/FEEDBACK_LOOP_DESIGN.md` | current (parked) | Human ↔ agent feedback loop; live user context built |
| `docs/archive/PARITY_AND_FEEDBACK_AUDIT.md` | historical | 11-17 parity audit; open gaps carried to PARKING_LOT |

## Research and direction (`docs/research/`)

| Doc | Status | Purpose |
|---|---|---|
| `2026-11-20-agent-eyes-on-drawings-design.md` | current | The agent sees and edits drawings: space map, eyes, hands, scores (slices 1–4 landed) |
| `2026-11-21-ordinate-dimensioning.md` | current | ISO 129-1 ordinate dimensioning for long location ladders (`ordinate.rs` built, not wired) |
| `2026-11-20-workflows-identities-modes-direction.md` | current | Agreed direction: workflows, identities, modes, ontology, PLM last |
| `2026-11-20-cad-workflow-variations.md` | current | How people design, manufacture and explore with a part |
| `2026-11-20-cad-roles-in-industry.md` | current | Roles in industry and what each needs from a part |
| `2026-11-20-workflow-catalog.md` | current | Workflow catalog with stages and who signs each gate |

## Historical handoffs (`docs/`)

| Doc | Status | Purpose |
|---|---|---|
| `archive/2026-08-22-agent-surface-open-work.md` | historical | Blackboard/lineage designs from 08-22 and measured facts (§5) |

## Not Markdown, also in `docs/`

`index.html`, `certificate.html`, `casebook.html`, `envelope.html`,
`architecture.html`, `site.css`, `viewer.js`, `logo.png`: the technical
sheets (The Claim, The Certificate, Casebook, Envelope) whose published copies
the README links.

`docs/archive/` holds superseded audits and diagnoses kept as evidence (cited by code comments and designs). Deleted 2026-11-22 after their live content moved: the duplicate geometry-engine `EVIL_EDGE_CASES.md`, drawing-bench `PHASE1.md` (its reference inventory is now in `BENCHMARK.md`) and `DRAWING_QUALITY_CRITIQUE.md`, `CONVERSATION_UI_AUDIT.md`, `docs/audit-2026-09-03-status.md`, `docs/handoff-2026-11-14-ui.md` (all in git history).
