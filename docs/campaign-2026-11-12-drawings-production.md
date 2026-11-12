# Drawings to production — campaign (2026-11-12)

Varun, 11-12: "fix the gaps and build what the research showed us ... finetune
this to a large extent so that this can go in production."

Rule for every item: a RED test first (fails on today's code for the stated
reason), then the fix, then the scoped drawing red-gate. A rendered sheet is
LOOKED AT before an item is called done — six sheet defects in August were
found only by a human looking.

## Items, in order

| # | Item | Done when |
|---|------|-----------|
| P1 | **Title block (ISO 7200)** — owner, title, drawing no. (≤16), date (ISO 8601), creator, approver, document type, status, material, mass, units, sheet, rev. Separate band for general tolerance, projection symbol, scale. Status `FOR REVIEW` + watermark until a person approves. Mass = kernel volume × density of a *named* material (unknown material → dash, never a guess). `VERIFIED` cell shows the sheet certificate digest, computed fresh at render; absent → `UNVERIFIED`. Agent can set the fields (MCP tool). One cell model shared by SVG and DXF. | Projection symbol no longer overlaps SCALE; a sheet without approver reads FOR REVIEW; ISO 7200 field checker passes; DXF carries the same cells. |
| P1b | **General tolerance is self-consistent** — "±0.1 MM … ISO 2768-m" claims two different tolerances (2768-m is ±0.3 at 30–120 mm, ±0.5 at 120–400 — ISO 2768-1:1989 table). State the class; derive per-size values from the class table when a value is needed, including the structured readback (`GeneralTolerance.linear_mm`), which today answers ±0.1 for every size. | No sheet prints a linear value that contradicts its stated class. |
| P2 | **Completeness by rank** (Light & Gossard 1982) — each sheet dimension is an equation on the part's free parameters; complete ⇔ rank = DOF, redundant ⇔ dependent row, missing ⇔ null-space direction. Added to the sheet certificate beside "every feature inked once". | A sheet with a duplicated chain dimension is flagged redundant; a sheet missing a hole location is flagged under-dimensioned, naming the free direction. |
| P3 | **Layout by optimisation** — candidate slots per dimension; verifier rules as hard conflicts; Kakoulis 2023 aesthetics as cost; parallel spans nested into tracks by containment depth; boundary-labelled leaders (Bekos et al. 2007). Replaces the greedy row search. | Every existing drawing fixture still clean; total dimension-line length and crossings measured and no worse on any fixture, better on at least the plate and flange. |
| P4 | **Datum-first dimensioning** — datums from mating faces (assembly mates) or the largest planar / axial faces; locate features from the datum frame (baseline), not chained. | Plate hole locations measured from datum edges; no chained location dimension on any fixture. |
| P5 | **View + section selection by score** — views chosen by set cover over recognised-feature visibility minus hidden-edge count; section plane scored by hidden internal features it exposes. | A part with a feature only visible from below gets that view; a part that needs no section gets none. |
| P6 | **Read-back evaluator** — a vision model (API-only) reads the exported sheet; values read back must equal kernel values. Different method → independent check of legibility. | Runs on the fixture set; disagreements are reported, never auto-fixed. |

## P7 — Standards library people can edit (Varun, 11-12)

"are we referencing the actual standards to build datum, tolerancing ... can
that be made so that humans can add and edit them too"

Today: ISO 2768 / 286 / 273 / 261 / B16.5 / EN 1092-1 tables are hard-coded in
`roshera-mcp/src/tools/kb_reference.ts`, marked "transcribed, not
clause-verified"; the kernel now holds a second ISO 2768 copy
(`drawing/types.rs::GeneralTolerance::linear_for`) — drift risk. Datum rules
(Y14.5 precedence, DRF validity) are code in `geometry-engine/src/gdt/`.

| Step | Done when |
|------|-----------|
| One library as data: per table, standard + edition + table/clause + rows + status (`transcribed` / `verified against <edition> by <person> on <date>`). Kernel loads it; MCP reads it through the API (the TS copy goes). | One ISO 2768 table in the repo; KB and kernel answers come from it. |
| Every kernel answer that uses a table cites table + row + status; a sheet built on unverified data says so. | Readback of a general tolerance names "ISO 2768-1 Table 1, class m, transcribed". |
| People add/edit tables and company overrides (house counterbore table, default class) in the app; each edit is a timeline event (who/when/what). | A person adds an ISO 4762 counterbore table; the agent then sizes counterbores from it. |
| Datum scheme per part is edited by a person (A/B/C faces); the kernel checks it against the Y14.5 rules and refuses an invalid one. | An invalid scheme (secondary parallel to primary) is refused by name. |

## P8 — People edit the sheet to their liking (Varun, 11-12)

"how can user edit the drawing to their liking ? like views and how the
dimensioning is done"

Today: add/remove views (bar under the sheet), 1st/3rd angle, title-block
fields, and dimension VALUES (which edit the part). Missing: drag a view,
per-view scale, move/flip/hide a dimension, add one by picking edges, choose
baseline vs chain and the datum faces, user-drawn section line / detail
circle, hand-placed notes, leaders, GD&T frames.

Design: the auto sheet is the default; each person's change is a PINNED EDIT
recorded as a timeline event and re-applied when the part changes, so a
regenerated sheet keeps it. The kernel still certifies: an added dimension is
measured from geometry; a hidden dimension that leaves a feature unspecified is
named by the sheet certificate. Datum/baseline choice joins P4 + P7. Built
after the frame cache (pinned edits replay on every rebuild).

## Deep UX pass -- engineer-first (Varun, 11-12)

"I want this to be really close to original CAD softwares.. so people can
adopt ... the ui and ux has to cater to the engineers .. not the other way
around." / "The ai button and the whole ux of selecting a model/ and the ui
has to improve drastically too."

After the functional gaps close. Each workflow is benchmarked against how
SolidWorks / NX / Onshape / Fusion do it, then rebuilt:

| Workflow | Known problems today |
|----------|----------------------|
| AI button + agent panel | Button and panel feel bolted on; model/provider choice is unclear; 70-120 s turns with little feedback; chat focus leaks keys to viewport shortcuts. |
| Choosing a model | Selection flow unclear (which model, what it costs, what it is good at). |
| Drawing window | Reworked 11-12 (overlays); still: no drag of views, no per-view scale, hole-table rows not editable, view placement not projection-correct. |
| Dimension change | Reworked 11-12 (context, progress, Stop); edit still ~45 s. |
| Timeline | Must play like a video (frames landed 11-12); the movie UI (chapters left, scrubber bottom) is designed, not built. |
| Modelling | Feature edit, sketch, select-a-face-and-build: not yet reviewed against CAD conventions. |

## Pilot app (Varun, 11-12): desktop app for Windows AND macOS -- after the work is complete

"we have to build this into an app... I have to run pilots." Decided: a
desktop app for Windows and macOS, not hosted web -- part files stay on the
engineer's machine (manufacturers' IP), internet only for the AI agent.
Varun: "we will do that only after we complete the work.. and apps would have
to be both windows and mac". Tauri covers both from one codebase.

| Step | Done when |
|------|-----------|
| Shell: Tauri (Rust, same toolchain) hosting the built frontend; the release `api-server` runs as a sidecar on a free local port, started/stopped with the app. | Double-click opens Roshera with no terminal, no Node, no cargo. |
| Data lives in the platform's app-data folder (`%APPDATA%\Roshera` on Windows, `~/Library/Application Support/Roshera` on macOS); nothing written next to the app. | Uninstall/reinstall keeps the engineer's work. |
| First run: a setup screen for the AI provider key (stored in the OS keychain (Windows Credential Manager / macOS Keychain)), with the app fully usable without it (modelling, drawings, STEP). | A pilot without a key still models and draws. |
| Shipped config: auth on, no `ROSHERA_DEV_INSECURE`, no dev keys in the bundle. | A scan of the installer finds no secrets. |
| Installers: signed MSI/NSIS for Windows; signed + notarized .dmg for macOS (Apple Silicon and Intel); auto-update from a release feed. | A pilot on either OS gets fixes without reinstalling. |
| Crash + error reporting to a local log the engineer can send us (opt-in upload). | A pilot problem arrives with its log. |

Order: LAST -- after the functional gaps and the deep engineer-first UX pass
are complete (Varun). The kernel already builds on both OSes; macOS needs a
CI runner for the notarized build.

### Collecting edge cases and bugs from pilots (Varun asked, 11-12)

The document IS its event history, so every failure a pilot meets replays
exactly -- 11-12's hub (21 events) reproduced the refused fillet, the stale
sheet and the wrong quartic roots. Build on that:

| Piece | What it does |
|-------|--------------|
| "Report a problem" in the app | Bundles the document's history, the kernel version, the refusal or error, the operation that failed, the engineer's note and an optional screenshot into ONE file. |
| Automatic capture | Every refusal, caught panic (e.g. the triangulation panic), stale-sheet refusal and quarantine is logged locally with its document and sequence -- even when nobody files a report. |
| IP stays theirs | Nothing leaves the machine without consent: the bundle is a file the pilot chooses to send. An "anonymize" option scales and perturbs dimensions while keeping topology and the failure. |
| Bug -> test | Each bundle replays in our harness and becomes a RED test before the fix (exactly the 11-12 flow). The bundles become the regression corpus. |

## Gaps carried into the campaign

- BOOL-COAX-1: an overlapping coaxial cutter through a plate+boss union certifies unsound.
- KB has no ISO 4762 counterbore table — the agent refused to pick counterbore sizes.
- Chat box focus: typed keys leak into viewport shortcuts.
- MCP and app sometimes disagree on the active document.
- Agent turn latency 70–120 s.
- STEP-IMP-1 (GrabCAD assemblies), BOOL-CONE-1 — kernel items, tracked in the backlog.

## Not building

- QR code in the title block: there is no public URL for it to point at yet; the
  certificate digest in the VERIFIED cell does the verification job today.
