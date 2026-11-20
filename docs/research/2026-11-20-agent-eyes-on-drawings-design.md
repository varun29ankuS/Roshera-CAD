# The agent sees and makes drawings: space map, eyes, hands, scores

2026-11-20.

Varun asked for this directly:
- "the biggest thing is to give agent the power to see and execute to the best of its abilities"
- "give it eyes... let it make drawings ... I want to see this implemented with results"

This builds on two sections of `roshera-eval/drawing-bench/BENCHMARK.md`: "drafting craft, measured" and "the layout model: break the sheet into spaces, never average".

## Intent

Automatic sheets are true (no wrong value on any sheet) but not professional. The 11-18 bench found 0 of 50 complete, about 300 label collisions, and 611 of 777 values with trailing zeros. Professionals get sheets right by looking at them and adjusting them space by space.

The agent gets three powers:
1. **Eyes:** a structured map of the sheet's spaces, plus the rendered sheet or a crop of any space as an image.
2. **Hands:** layout verbs on those spaces. Each verb is recorded, undoable and re-certified.
3. **Judgment:** a score per space. Every verb answers with the score change for each space it touched, worst first, plus the certificate verdict.

With these it runs a loop:
1. look;
2. pick the worst space;
3. act;
4. re-score;
5. stop when no space is below its bar, and say what is left.

## Goals

- **The sheet is a hierarchy of spaces.** Each space has a stable id, its contents and its own score. The sheet's grade is its WORST space; scores are never averaged.
- **The agent can see the whole sheet, or one space, as an image.**
- **The agent changes the layout through typed verbs** on space and dimension ids.
- **Results, measured on bench parts:**
  - the automatic sheet's craft score (worst space and mean per space) against the sheet after the agent's loop;
  - the certificate stays SOUND throughout;
  - rendered before and after;
  - held-out parts reported separately.

## Non-goals (this slice)

- New dimension-selection, view-selection or line-work algorithms. These come later and are scored on the same space map.
- The human editor UI. The verbs are built so the editor can reuse them.
- Hanomi material. None is available.

## Anti-goals

- **Pixel inference replacing facts.** Images judge appearance only; every number comes from the kernel and is re-checked.
- **A loop that makes a sheet less true.** Every verb is certificate-gated, and an unsound result is refused.
- **Averaged scores.**

## Design

### 1. The space map (kernel, `geometry-engine/src/drawing/`)

It is computed from the laid-out sheet (`compute_layout`). The spaces:
- **Fixed zones:** the frame, the title block, the notes strip, the table column and each table.
- **View cells:** one per view. Each holds the view's geometry box and four annotation bands (top, bottom, left, right). Each band holds lanes, and each lane holds dimensions.
- **Free rectangles:** the largest empty rectangles of paper.

Each space carries its id, kind, box (paper mm), parent, contents (dimension, table and view ids), and its findings and score.

Per-space checks have stable IDs and tolerances, from ISO 128, 129, 3098, 5457 and 7200, with NIST sheets as the reference:

| ID | Checks |
|---|---|
| `ALIGN-ROW` | Spread of collinear dimension lines |
| `LANE-PITCH` | About 10 mm from the outline, then 7 to 8 mm between lanes |
| `TEXT-CLEAR` | Text-to-line and text-to-text clearance; collisions |
| `CROWDING` | Ink density in the space |
| `EMPTY` | Unused paper in a cell or zone |
| `TABLE-ANCHOR` | Table edges sit on the frame or title-block grid |
| `VIEW-ALIGN` | Projected views on one axis; equal gutters |
| `LEADER-ANGLE` | Leaders at 30 to 60 degrees, no crossings |
| `TEXT-HEIGHT` | At least 2.5 mm, and 3.5 mm on A3 and larger |
| `TRAILING-ZERO` | Trailing zeros (ISO 129-1) |

Each finding gives its rule, space, severity, and the measured value against its tolerance. A space's score comes from its findings; the sheet's grade is the minimum over its spaces.

Exposed as `GET /api/drawings/{id}/spaces` and the MCP tool `drawing_spaces`.

### 2. Eyes

- `GET /api/drawings/{id}/render.png?space=<id>&px=<n>`: the whole sheet, or one space with a margin, rendered from the sheet's SVG. Use `resvg`/`tiny-skia` if they are already locked (`svg2pdf` brings `usvg`), otherwise the lightest raster crate already locked.
- MCP `drawing_look {drawing_id, space?}` returns the image as MCP image content, together with that space's scores.

### 3. Hands (MCP verbs; reuse the edit routes in `api-server/src/drawing_edit.rs` and `drawing_mgr.rs` where they exist)

- `drawing_move_dimension {dimension, band?, lane?, offset?}`
- `drawing_align {dimensions[]}`
- `drawing_move_view {view, to | align_with}`
- `drawing_set_sheet {sheet?, scale?}`
- `drawing_place_table {table, zone}`
- `drawing_reattach_leader {dimension, side}`
- `drawing_add_dimension {between | feature}`
- `drawing_remove_dimension {dimension}` (a suppression)

Every verb:
- is recorded on the timeline and undoable;
- is re-certified, and refused if the result would be unsound;
- answers with the score changes per space (worst first), the layout issues it created or resolved, and the certificate verdict.

### 4. The loop and the policy

The `manufacturing_drawing` playbook gains an "improve the sheet" loop:
1. `drawing_spaces`;
2. `drawing_look` on the worst space;
3. a verb;
4. re-score;
5. repeat until every space clears its bar, or two attempts on the same space fail;
6. report what is left.

In Check mode, the checker is never the agent that drew the sheet.

### 4b. Show: the agent's work is animated, and the animation is the user's guide (Varun 2026-11-20)

Varun: "make sure the animations are there when the agents work too... that should be part of the understanding.. animation will sort of become the guide for users to follow".

**Base.** The agent show channel (`POST /api/agent/show`, `stores/agent-show-store.ts`, `AgentShowBar`, `AgentFacesHighlight`) already lets the agent drive the app's 3D view, with a caption and Pause / Take over. The drawing sheet gets the same:

- **Look.** When the agent calls `drawing_look` or reads a space, the app switches to the Drawing workspace, zooms the sheet to that space and outlines it. The caption names the space and its worst finding, by rule ID with the measured value against the tolerance, e.g. "FRONT top band: lanes 4.1 mm apart, ISO 129 practice 7-8 mm".
- **Act.** Every layout verb animates its change on the sheet: a dimension glides from its old lane to its new one, a chain snaps into line, a view slides into alignment, a table moves into its zone. The old position stays as a faint ghost for a moment. The caption says what was done and why, with the rule.
- **Judge.** After each verb, the touched spaces flash their score change: green where it improved, red where it got worse, worst space first, with the certificate verdict.
- **The animation is emitted by the verbs and look tools themselves, server-side, not left to the agent to remember.** The same events are recorded on the timeline, so a finished session can be replayed step by step as a tutorial: "watch how this sheet was made right".
- **The user can learn and intervene:**
  - Pause;
  - Take over, which stops the agent and leaves the sheet as it is;
  - step back and forward through the agent's actions;
  - click a caption's rule ID for a one-line explanation of the rule.
- Speed follows the user's setting. Reduced-motion users get instant changes with the same captions.

This is part of the slices, not an afterthought:
- slice 2 (eyes) emits the look frames;
- slice 3 (hands) emits the act and judge frames;
- the app work ships with slice 3;
- slice 4's results run is recorded so the replay can be shown.

### 5. Results (`roshera-eval/drawing-bench`)

- `bench.py` records the space-map grade for every sheet.
- A new agent mode runs the loop on a fixed set of bench parts (dev and held-out). For each part it records:
  - the grade before and after;
  - findings by rule;
  - the number of verbs used;
  - the certificate;
  - before and after PNGs.
- The headline is the worst-space grade and the count of spaces below their bar, before and after, with dev and held-out reported separately.

## Slices

Each slice lands with RED-first tests, a look at the rendered sheets, and a bench re-run.

1. The space map and its rules, with route and MCP `drawing_spaces`, bench craft scores and the baseline.
2. Eyes: the PNG route and MCP `drawing_look`.
3. Hands: the verbs, with score deltas and certificate gating.
4. The loop: playbook and policy, the bench agent mode, and a results run with the live server.

## Constraints

- **Tests:** RED first and mutation-proven.
- **Production call sites:** every new route or tool has a production call-site test.
- **Registration:** MCP tools are registered on both sides, per the ontology drift gate.
- **Builds:** `-j 2`, debuginfo off, one cargo process at a time.
- **Dependencies:** no new heavy dependency unless it is already locked.
- **Names:** no customer names anywhere.
