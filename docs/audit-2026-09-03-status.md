# Audit 2026-09-03 — status and remaining work

Handoff written 2026-09-27. `main` is pushed and clean after the Task 72 commit (`36d7719a`) and this doc.
The campaign runs at 4–6 tasks a day; each task is committed and pushed as soon as its review
approves it.

## ⚠ Owed before anything else

- **Full geometry-engine red-gate on `main`.** Task 101 (`7313cbdc`) changes the kernel (shell now
  removes its source solid; new recorded parameters on nurbs_loft/offset/reanchor; a new
  `extrude_polygon` entry) and landed on its targeted suites only — the full gate run was stopped for
  low system memory. Run `powershell -File roshera-backend/scripts/red-gate.ps1` on a machine with
  memory to spare (the target directory was cleaned after this handoff, so it rebuilds from scratch);
  any new red → diagnose against `7313cbdc`.
- Rebuild `api-server` and the MCP `dist/` (then reconnect the MCP server) before the next live demo —
  both changed repeatedly since the last build.

## How this campaign runs

One implementer at a time writes the fix RED-first with a mutation that turns the test red again; one
reviewer reads the working-tree diff (and probes it); scoped fix rounds follow; the controller verifies
by a different method, then commits and pushes. Every task surfaces one to three new findings; those are
filed as new tasks, not fixed in passing.

Working files (git-ignored, this machine only):

- Plan with every task's full text: `docs/superpowers/plans/2026-09-03-audit-fixes.md`
- Ledger with rulings, rounds and commits: `.superpowers/sdd/2026-09-03-audit-fixes/progress.md`
- Per-task briefs, reports, reviews and commit drafts in the same directory
- Second-pass audit findings (2026-09-24): `docs/superpowers/plans/2026-09-24-audit-findings.md`

## Landed since the last status (ef54bf2d → HEAD)

| Task | Commit | What changed |
|---|---|---|
| 65 | e3cae4e8 | A rigid transform moves every shell (voids and peer bodies), not only the outer one |
| 66 | d7d513d4 | Mirror is one recorded op and replays the right way out; arcs/surfaces map exactly under reflection; new certificate conjunct `shells_outward`; void/peer classifier fixed; self-intersection confirm bounded |
| 71 | b15aac1a | MCP: refusals and halts are errors; `cad_program` stops at them; roshera-rl counts halted work |
| 70 | 0449d017 | App: the gizmo commits through the kernel route and snaps back on refusal; scale removed; server Error frames parse |
| 93 | 2bceb67f | MCP drift tests re-pinned with reasons; a ranking bug fixed; `test:drift` runs in CI |
| 67 | 093e4580 | A `fast` response no longer calls a B-Rep-only check `sound`; MCP batch tools certify every step |
| 68 | afc16a51 | A restart restores each branch's own history and replays only main; raw-id/foreign-solid references quarantine typed |
| 69 | 1c876dca | Ops on a dead branch are refused or reported (`recording_failures`), never silently dropped |
| 100 | 352c8bc0 | One branch switch (does NOT rebuild — says so); undo/redo/truncate/mould rebuild off to the side and refuse what they cannot reproduce; truncate pre-flight shares one plan with truncate |
| 101 | 7313cbdc | Torus, shell, nurbs_loft, polygon extrude, datums and re-anchor survive a restart; a gate fails any recorded kind without a replay arm (**red-gate owed**) |
| 104 | c8a87960 | Undo survives a restart (durable applied head); a new op discards the redo tail for good; two races closed |
| 72 | 36d7719a | Logout and rotation survive a restart; a refresh token is spent exactly once, across instances |

Earlier landings (be50862e .. 46406039, Tasks 1–49 and 34b) are in `git log`.

## Remaining work, in the order to take it

### History layer (highest value — the kernel's history is the product's substrate)

- **79** Kind-keyed replay id maps and document-stable, replay-reproducible solid ids. Needs its own
  design pass. It unblocks 100b and removes the honest over-refusals Tasks 68/100 introduced.
- **100b** Switching branches rebuilds the live model (depends on 79).
- **110** Lanes that bypass the durable applied head (moulds and `/timeline/record` not durable, etc.).
- **74** Truncate and clear are still memory-only.
- **78** Merge duplicates identical ops and is never re-certified.
- **102, 105–109** Recording-failure delivery per client; tx rollback unrecorded; sketch extrude_cut /
  revolve transient faces; imports unrecorded; recorder suppression window process-wide; undo success
  over a skipped event.

### Security / API

- **111** The app's logout never calls the server; a 503 on refresh logs the user out.
- **73** Login throttling keys on a client-set header; lockout never wired.
- **85, 86** API-key row decoding; SQLite vs Postgres divergence (tests run on SQLite, production on Postgres).
- 09-03 originals: **9, 28, 29, 30, 31**.

### Kernel / math

- **80** Surface-surface marching returns truncated curves as Ok.
- **81** Mass properties can label an open shell exact.
- **82** Malformed request numbers become 0.0.
- **87, 90, 91, 92, 96, 97, 98, 103** and the 09-03 kernel queue **50–64, 35**.

### MCP / app / eval / docs

- **75, 76, 77, 83, 84, 88, 94, 95, 99**, and 09-03 **10, 11, 19–25, 27**.
- **89** Dead code that would be dangerous if wired — needs Varun's delete-or-fix ruling.

## Decisions waiting on Varun

- Task 89: delete or fix the dead-but-dangerous code (CommandProcessor, 2FA, the WS AICommand path,
  unused executors, dead timeline-engine modules).
- Concave chord fillet sign convention (currently a named refusal).
- Legacy `Coincident`/`Concentric` mate freedoms (excluded until an acknowledgement path exists).
- RBAC mapping for the ~120 authenticated-but-unauthorized routes.
- The dead `/api/ai/command` surface.

## Environment notes

- `git` is not on the Bash PATH on this machine — use PowerShell. `cargo fmt -p <crate>`, never `--all`.
- Memory is tight: build with `-j 2`; a rustc crash with nonsense errors is usually OOM.
- Runs go one cargo at a time, foreground, chunked under ten minutes; agents that wait on background
  runs never wake up.
- Rotate the OpenRouter key; drop `ROSHERA_DEV_INSECURE=1` from any running server.
- The showreel video lives in `brag-output/` (git-excluded locally): `brag.mp4` (v3), `brag-master.mp4`.
  Its music is Mixkit track 474 (commercial online use; not TV/radio broadcast).
