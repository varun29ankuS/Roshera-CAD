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
