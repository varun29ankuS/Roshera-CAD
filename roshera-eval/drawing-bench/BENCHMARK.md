# Drawing benchmark: STEP -> automatic production drawing

Baseline: 2026-11-18. Worktree `agent-a6997b44675ecaf85` on base `135d55bc` plus the uncommitted drawing slices 1-3 (sheet completeness, tolerances, property panel). Debug build of `export-engine/examples/drawing_bench.rs` (`cargo build -p export-engine --example drawing_bench`, `debug=false`). Sheet timings are from a DEBUG build and are pessimistic against a release build.

## What is compared, and what is not

Three sheets per part, side by side on one contact sheet (`runs/<label>/<id>/contact.png`):

| Column | What it is |
|---|---|
| Human reference | the CADGenBench input drawing (`reference/<id>/input.png`) or the NIST PMI PDF |
| Roshera | the automatic sheet, untouched: the same `standard_drawing_auto` the app's "drawing" route builds, certified against the model as imported |
| FreeCAD 1.0 TechDraw | headless `freecadcmd` (`freecad_sheet.py`): a third-angle Front/Top/Right projection group plus an isometric, hidden lines on, on the smallest ISO sheet that fits |

What the FreeCAD column shows and does not show:

* TechDraw has **no automatic dimensioning**. Every dimension on a TechDraw sheet is placed by hand, so the FreeCAD column compares projection, hidden-line removal and view layout only. It says nothing about dimension quality.
* Headless TechDraw cannot export a page (`TechDrawGui.exportPageAsSvg` needs the GUI). The harness assembles the sheet from each view's own SVG (`TechDraw.viewPartAsSvg`), placed at the view's position and scale, with hidden lines dashed. The frame and title block of a real TechDraw page are not reproduced.
* FreeCAD's importer (OCCT) is the oracle for "can this STEP be read": it read all 11 NIST files and 47 of 49 CADGenBench files (109 and 136 crashed TechDraw with an access violation).

**SolidWorks, NX, Inventor, CATIA and Creo are not available on this machine.** No number here compares Roshera with them. The critique's settling test (ten held-out parts drawn automatically by Roshera and by one incumbent's auto-dimension pass, shown unlabelled to three practising engineers) has not been run. Until it is, "world class" is a claim, not a result.

The CADGenBench references print "this drawing deliberately departs from drafting standards". They are an inventory of WHAT a dense drawing states (sections with depth dimensions, details, counts), not a style target. Several are sheet-metal parts drawn as their flat pattern (107, 114, 130 in part): their ordinate dimensions are unfolded lengths that a sheet of the formed solid cannot state. Reference recall therefore measures coverage of content, not drafting quality.

## Reference sets (inventory, from PHASE1 2026-11-18)

### CADGenBench (HuggingAI4Engineering/cadgenbench-data) [V]
- 84 task folders. **46 + 3 have `input.png` (+ `input2.png` in 3 cases) + `description.yaml`; 32 have `input.step`, `input.mesh.npz`, `renders/`, `edit_description.txt`.**
- `1xx` = **generation-from-drawing** tasks (ids 101-143, 145-150; 144 is not in the benchmark). Task text: "Reproduce the geometry as accurately as possible from the drawing."
- `2xx` = **editing tasks** (starting solid + instruction). **They have NO drawing.** 32 of our 81 local STEP files (201-250) therefore cannot be scored against a reference drawing. **Scoreable set = 49 parts: 101-143, 145-150.**
- Format: single raster **PNG, 3072x2118 px, ~0.75-0.85 MB**, A2 landscape sheet. No PDF/SVG/vector source, so reference dimensions must be extracted by looking (vision/human), not parsed. A few ids have a second image (`input2.png`).
- Downloaded to `roshera-eval/drawing-bench/reference/<id>/{input.png,description.yaml}` (about 40 MB total at 49 ids; full folder listing in `reference/_listing.json`). Data only, nothing executed.
- Our STEP files: `...\Downloads\standard-manufacturing-co_smc-harness-v1-2-fable-5-2-max_20261113-034752\<id>\output.step`. Note these are another team's *reconstructions from the drawing*, so "our drawing vs reference drawing" compares a re-modelled part, not the benchmark's ground-truth part. Dimension values that the reconstruction got wrong will show up as "wrong" on our sheet through no fault of the drawing pipeline. Mitigation: score only against dimensions the reconstruction's own geometry has (see 3, check C0), or use the benchmark's certified-sound subset.

### What the reference sheets look like (looked at 101, 105, 110, 120, 125) [V]
Common template: A2 landscape, ISO-style zone border (1-12 / A-H), an **empty** title block (MATERIAL/ORIGINATED/CHECKED/RELEASED/WEIGHT/TITLE/PROJECT all blank, Hugging Face + Mecado logos), and the printed disclaimer **"THIS DRAWING DELIBERATELY DEPARTS FROM DRAFTING STANDARDS ... NOT INTENDED FOR HUMAN USE"**. So:
- Title block content is **not** a scoring target (it is blank). Only "has a title block" is.
- No tolerances (no +/-), no GD&T FCFs, no surface finish on the samples seen. Datum symbols appear only occasionally (101 has A and B datum boxes). Projection is not stated. Views are **not** aligned in the strict projection layout (101: views scattered, front view in the middle, top bottom-left); do NOT score view placement against it.
- Content that does recur:
  - 3-6 orthographic views + 1 shaded **ISO** (101: 1 ISO; 105: 2 ISO; 125: 2 ISO).
  - **Section views** A-A / C-C / D-D with hatch (101, 105, 110, 120, 125) and **detail views** with circle + scale (105 detail C x4; 110 details A/B/C and section D-D; 120 sections A-A/B-B/C-C).
  - Dimension kinds: linear, diameter (Ø), radius (R, "R10 TYP", "5x R15 TYP", "R2.00 TYP"), **angle** (150.5 deg, 112.5 deg, 85.06 deg, pattern angles), chamfer ("CH 2.50 X 45 deg TYP", "8X 1.5 x 45 deg"), arc length/pitch, hole **callouts** ("4X 12 THRU", "28 THRU", "CBORE Ø35 - 5 DEEP", "CSINK Ø12 X 90 deg", "6 X M12x1.75-6H - 15 DEEP", "3xØ35", "3xØ15.5").
  - Notes ("R1 All Around", "Break Edge", "pocket depth remains same for every pocket", "ALL POCKET DEPTH IS 2.00MM").
  - Dimensions are dense: 120 is a 300 mm enclosure with ~60 dimensions in a single view.
- Precision is mixed: some sheets state two decimals (105, 125), others integers or comma decimals (110: "60,4", "46,2"). The scorer parses both.

### NIST MBE PMI test models (second reference set) [V]
- Downloaded to `reference-nist/`: 11 PMI-annotated PDFs (CTC-01..05, FTC-06..11, each one page of a 3D-annotated view, **not** a 2D drawing sheet) and the model zip (57 MB, **not extracted except** the 11 `*.stp` in `reference-nist/step/`).
- STEP files are labelled "geometry only" (AP203, no PMI). The PMI (position, flatness, perpendicularity, profile, +/- tolerances, datums A/B/C) lives only in the PDFs (e.g. CTC-01: Ø25 +/-0.15, Ø35 0/-0.2, Ø20 +0.05/-0.10, 60 deg +/-0.5, position 0.75 A B C, flatness 0.2, perpendicularity 1.5 A, profile 0.5 A / 1.25 A B C).
- Use: ground truth for **tolerance + GD&T + datum** scoring, which CADGenBench cannot test. Roshera cannot import this PMI (see gap G5), so the test is "import geometry, apply the PMI through our GD&T/datum API (gdt sidecar), generate the sheet, score that the FCFs/tolerances/datums are present, attached to the right feature, and readable".

## Method

* Parts: the 49 CADGenBench reconstructions (ids 101-150 except 144; `output.step` from the SMC harness run of 2026-11-13) and the 11 NIST CTC/FTC AP203 models.
* Split, fixed before any tuning (`split.json`): the 19 CADGenBench parts with the lowest `sha256("roshera-drawing-bench-split-v1:" + id)` are HELD OUT: 101, 104, 106, 110, 111, 113, 114, 116, 120, 121, 122, 128, 129, 130, 131, 133, 135, 146, 147. The other 30 are DEV. Every later change is reported on both halves; nothing is tuned on the held-out parts or on NIST.
* Reference dimension lists (`refs/<id>.json`): 7 parts, fixed for every rerun. Dev: 102, 107, 125. Held out: 101, 110, 114, 130. Each was read off the reference PNG once by an agent and not double-checked by a person; each file says so and names what was left out.
* Scoring (`score.py`, tests in `test_score.py`): a reference dimension is found only by an inked dimension of the same kind class within max(0.05 mm, 0.5 %), one-to-one, with counts honoured; a value present under another kind is reported as `wrong_kind`; holes match by numeric diameter and each claim (count, THRU, counterbore, countersink, thread) is checked on its own row; views come from the sheet's view list. An inked kind the scorer does not know is reported and matches nothing (it used to be scored as a length).
* Reference-free checks (critique section 5.1), from `summary.json` plus a parse of the rendered SVG: A1 build and time, A2 truth (certificate), A3 completeness (`consistent / (consistent + not_dimensioned)`), A4 redundancy, A5 label collisions and layout errors, A6 text capital height (0.70 of the em size) and trailing zeros, A7 views, A8 orientation (rotation from the part's own planar normals and axes to the nearest square pose), A10 exports.
* Limits: our sheet 300 s per part (the A1 bar is 30 s), FreeCAD 240 s. The example writes its stage to disk on entering it, so a part killed on the limit reports where it was. (The first 49 CADGenBench runs used a 600 s limit and an example that did not yet record its stage: their five time-outs read "start".)

Run it: `python -I bench.py --out runs/<label> [--set cadgen|nist|all] [--only ids] [--skip-freecad]`, chunked so each call stays under 10 minutes; `--aggregate-only` re-checks and re-scores every part folder on disk and rewrites `index.json` and `results.md`.

## Baseline (`runs/baseline`)

### Build, truth, time

| | CADGenBench dev (30) | CADGenBench held out (19) | NIST (11) |
|---|---|---|---|
| sheets built | 25 | 18 | 7 |
| failed at import | 0 | 1 (133: no solid) | 4 (ctc_01, ftc_06: no solid in 0.3 s; ctc_02, ftc_07: import still running at 300 s) |
| timed out (600 s, stage not recorded) | 5 (109, 126, 127, 141, 148) | 0 | 0 |
| sheet time median / p95 (debug) | 13.3 s / 39.6 s | 8.4 s / 59.5 s | 6.5 s / 28.6 s |
| sheets under 30 s | 23 / 25 | 14 / 18 | 7 / 7 |
| certificate SOUND | 23 / 25 | 17 / 18 | 7 / 7 |
| stale or dangling facts | 0 | 0 | 0 |
| sheets that omit a bore (NOT SOUND) | 2 (145, 149) | 1 (128) | 0 |
| all three exports (SVG, PDF, DXF) | 25 | 18 | 7 |
| FreeCAD sheet built (median time) | 47 of 49 (4.1 s) | | 11 of 11 (2.3 s) |

Note on NOT SOUND: 128, 145 and 149 each omit a bore the model carries (an `omitted` fact: the certificate refusing to call the sheet sound, not a wrong value); 149's solid was also not sound on import (`brep_valid: false`). No sheet states a wrong value: A2's "no stale or wrong value" holds on all 50 sheets built.

### Completeness, layout, legibility

| | dev | held out | NIST |
|---|---|---|---|
| A3 sheets COMPLETE | 0 | 0 | 1 (ftc_11, 6 faces) |
| A3 coverage median (min) | 0.45 (0.26) | 0.43 (0.25) | 0.58 (0.49) |
| A4 sheets with a redundant dimension | 6 | 0 | 1 |
| A5 sheets with no layout error | 3 | 5 | 4 |
| A5 dimension-label collisions (total) | 294 | 338 | 4 |
| A6 dimension text capital height | 2.17 mm on every sheet (ISO 3098 minimum 2.5; 3.5 normal on A3+) | | |
| A6 dimension texts with trailing zeros | 647 of 816 | 387 of 480 | 93 of 169 |
| A7 views | 3 orthographic + iso on all; a section on 8 / 9 / 3 sheets | | |
| A7 sheets with a projection-misaligned view (warning) | 7 | 7 | 1 |
| A8 measured on NIST | all 7 square (tilt 0.000 deg) | | |
| section dimensions | 0 on every section | | |

Why sheets are incomplete (certificate omissions over all 50 sheets): 4212 `merged_with_equal_value` (equal values in one view collapsed to one dimension with no count), 1717 `view_limit` (the eight-per-view cap), 258 `angle_clutter`, 170 `diameter_limit`, 6 `not_placed`. By kind: 3221 positions, 1853 diameters, 1019 lengths, 270 angles. The dominant gap is the selection caps (critique W1), now visible because the certificate reports them; a counted-callout ("4x R2.5") pass would clear most of the merged ones.

### Against the human references (7 parts)

| id | split | dim recall | dim precision | hole recall | view coverage | headline |
|---|---|---|---|---|---|---|
| 102 | dev | 0.14 | 0.20 | n/a | 0.60 | 0.39 |
| 107 | dev | 0.05 | 0.06 | 1.00 | 1.00 | 0.37 |
| 125 | dev | 0.13 | 0.50 | 1.00 | 0.80 | 0.38 |
| 101 | held out | 0.37 | 0.62 | 0.50 | 0.75 | 0.40 |
| 110 | held out | 0.20 | 0.36 | 0.33 | 0.67 | 0.42 |
| 114 | held out | 0.11 | 0.27 | 1.00 | 1.00 | 0.40 |
| 130 | held out | 0.06 | 0.16 | 0.00 | 1.00 | 0.33 |
| mean dev / held out | | 0.10 / 0.18 | 0.25 / 0.35 | 1.00 / 0.46 | 0.80 / 0.85 | 0.38 / 0.39 |

Read with the caveats above: 107, 114 and most of 130 are flat-pattern ordinate sheets.

### Contact sheets

`roshera-eval/drawing-bench/runs/baseline/<id>/contact.png` for every part (reference | Roshera | FreeCAD), e.g. `runs/baseline/102/contact.png`. `runs/` is gitignored and regenerated by `bench.py`.

What looking at them shows (102, 114, 130, 107, 125, 110 looked at against their references):

* Roshera's sheets carry dimensions, a hole table, a title block, a certificate stamp and three exports; FreeCAD's carry views only. On views alone FreeCAD's hidden-line output is cleaner: our shaded isometric shows every hidden edge through the shading and reads as transparent.
* Our sheets are crowded where the references are sparse: the eight-per-view selection piles dimensions into stacks and labels collide (A5), text is below ISO height (A6), and values print with trailing zeros ("45.00").
* The references section internal features and dimension the section; ours have a section on 20 of 50 sheets with no dimension on it.

## After items 2, 3 and 5 (`runs/after`)

One rerun covers all three changes (a full rerun is about two hours of import-bound debug runs); each change is read off the metric it moves: A8 and the frame for item 2, the hatch for item 3, A6 for item 5. The same `bench.py` aggregates both runs. Our-sheet limit 600 s for the after-run except where noted, FreeCAD and reference images copied from the baseline (`--freecad-from`).

What changed in the code:

* **Item 2, orientation.** `drawing/natural_frame.rs` reads the part's natural frame from its own analytic faces (planar normals and cylinder/cone/torus axes, area-weighted, unsigned, clustered) and gives the smallest rotation that lays the primary direction (and, when the primary itself sat tilted, the heaviest square direction) on world axes. `draw_in_part_frame` / `frame_for_drawing` apply it after undoing any assembly placement. A part whose primary is already square keeps its in-plane angle (its author placed it), and so does a part that is round about its primary. The sheet records the move (`Drawing::drawn_frame`) and everything that re-measures or re-projects the part for that sheet uses it: the certificate (`certify_drawing`), a view added through the route (`drawing_mgr::add_view`), the replayed sheet (`replay::rederive_part_drawing`) and a replayed added view (`drawing_history`). Before this, an assembly part drawn "as designed" was certified against the live, placed model, so a placed sheet's overall sizes would read stale; that is fixed by the same field.
* **Item 3, sections.** Hatch pitch is now constant on paper (3.0 mm square to the lines at every scale; it was 2.83 model units, so 0.57 mm at 1:5 and 5.7 mm at 2:1).
* **Item 5, lettering.** Dimension and note text are 3.6 mm fonts (2.5 mm capitals, the ISO 3098 minimum; they were 2.17 and 1.7 mm), the notes lines 5 mm apart, and the DXF writes the same capital height as the SVG (it wrote the em size as the capital height, 43 % taller than the SVG).

### Results (baseline -> after)

| | CADGenBench dev (30) | CADGenBench held out (19) | NIST (11) |
|---|---|---|---|
| sheets built | 25 -> 24 (136 hit the 600 s limit while rendering; it built in 504 s at baseline: time-limit artifact under a heavier machine load, not a regression) | 18 -> 18 | 7 -> 7 |
| not built, where it stopped | 109 sheet (import 91 s, then over 200 s in the sheet stage: a real drawing-side slowness), 126 import, 127 and 141 import (274-298 s of import, killed seconds into the sheet stage), 148 certify, 136 render | 133 import (no solid) | ctc_01 and ftc_06 import (no solid), ftc_07 import, ctc_02 certify (import 220 s) |
| sheet time median / p95 (debug) | 13.3 / 39.6 s -> 15.5 / 41.0 s | 8.4 / 59.5 s -> 10.0 / 72.7 s | 6.5 / 28.6 s -> 4.8 / 19.9 s |
| certificate SOUND / stale or dangling | 23 -> 22 (136 not built) / 0 -> 0 | 17 -> 17 / 0 -> 0 | 7 -> 7 / 0 -> 0 |
| A3 coverage median (min) | 0.45 (0.26) -> 0.45 (0.26) | 0.43 (0.25) -> 0.43 (0.25) | 0.58 (0.49) -> 0.58 (0.49) |
| A4 sheets with a redundant dimension | 6 -> 5 | 0 -> 0 | 1 -> 1 |
| A5 sheets with no layout error / label collisions | 3 -> 4 / 294 -> 297 | 5 -> 5 / 338 -> 332 | 4 -> 4 / 4 -> 3 |
| **A6 dimension capital height** | **2.17 -> 2.52 mm** | **2.17 -> 2.52 mm** | **2.17 -> 2.52 mm** |
| A6 text classes still under 2.5 mm | hole-table-text, hole-tag, datum-marker-label, title-block field labels/values, logo wordmark, zone letters (dim-text and notes-strip no longer) | same | same |
| A6 trailing zeros | 611 of 777 dimension texts | 387 of 480 | 93 of 169 |
| **A8 parts off square as imported** | **0 of 24** | **0 of 18** | **0 of 7** |
| frames applied | all AsModelled | all AsModelled | all AsModelled |
| sections / section dimensions | 8 / 0 | 9 / 0 | 3 / 0 |
| exports | 24 / 24 | 18 / 18 | 7 / 7 |
| reference scores (B1 recall / B2 precision / B3 holes) | 0.10 / 0.25 / 1.00 unchanged | 0.18 / 0.35 / 0.46 unchanged | n/a |

Sheet timings moved with machine load (two benches and other agents' builds ran alongside); A1 deltas are noise. 103, 112, 119 and 134 were rerun after the area estimate below; their times are comparable to baseline.

What the deltas say:

* **Item 2 has no effect on this benchmark.** Not one of the 49 parts measured (42 CADGenBench + 7 NIST) is off square as imported: every primary direction already lies on a world axis. The first version of the rule also turned parts about a square primary (102 by 20 degrees, 124 by 10 degrees, both against their reference drawings, which keep the modelled angle); it now keeps an author's in-plane angle and only squares parts tilted out of plane, the vehicle-position case. Its value is proven only by the synthetic tests (`drawing_natural_frame.rs`), not by these parts. Volvo parts in vehicle position are the case it exists for; none is in this set.
* **Item 3 (hatch) is not measured by the bench**; `drawing_section_hatch.rs` proves the paper pitch at 1:5, 1:2, 1:1 and 2:1. Section dimensions stay at 0: see "Not done".
* **Item 5 moved A6 for dimensions and notes on every sheet** (2.17 -> 2.52 mm capitals; notes 1.7 -> 2.5 mm). Layout collisions moved within noise (dev +3, held out -6, NIST -1).
* A regression this run caught and fixed: the taller notes strip ran into the hole table where the table sat in the bottom-left corner (21 sheets; the table slot reserved a fixed 12 mm). The slot now reserves the computed notes band and every slot is kept clear of the notes rectangle. Remaining: 112 (also at baseline) and 119 (A4: no slot fits, and the documented fallback slot lands on the top notes line by 1.5 mm). The verifier does NOT report a hole table overlapping the notes (it pairs notes only with view labels), so this was found by measuring the SVGs (`.scratch/opus_drawing_notes_table.py`), not by `verify_drawing`.

### Looked at

105, 110, 124 and 102 against their references on the after-run contact sheets:

* 105 (A3): the hole table is placed over the TOP view and leaders fan across the sheet; a machinist could not use it. Baseline has the same placement.
* 110 and 124: fillet and pocket-corner arcs are drawn as full circles in the plan and isometric views (large phantom circles around the part); the isometric draws every hidden edge over the shading. Baseline has the same defects.
* None of the four would be signed by a checker; question 5 ("every value true") is yes on all four (certificate sound, no stale facts).

### Not done (open for the next agent)

* Item 2: dropping views that carry no unique information (e.g. the second side view of a round part). The sheet code indexes views positionally (`views[0..3]` = FRONT/TOP/RIGHT/ISO in `attach_section_view`), so a drop needs that made index-free first.
* Item 3: section dimensions. A pass that put length and diameter records on SECTION A-A (and took them off views that reached them only through hidden lines) was built and removed: `readable::extract_dimensions` has no planar step, wall or pocket-depth lengths, so on a pocketed test plate it found nothing to place and could not be proven. The dimension table needs those records first. Section alignment to its parent view and detail views were not started.
* Item 4 (smart-dimension add) was not started.
* Item 5: trailing zeros (ISO 129-1; `{:.2}` in `hole_table.rs`, round-part and hole-feature labels, about 70 test assertions), tolerance text (`TOL_FONT_MM` 2.17 mm font, 1.5 mm capitals, below ISO 3098), hole-table/tag/title-block text under 2.5 mm, notes and leaders, GD&T placement.
* Verifier gaps found: a hole table over the notes strip and a hole table over a view's geometry (105) are not reported by `verify_drawing`.
* Drawing defects seen on contact sheets: fillet and pocket-corner arcs drawn as full circles (110, 124), hidden edges drawn over the shaded isometric.

## Next: drafting craft, measured (Varun 2026-11-20)

Varun, comparing against professionally drafted sheets: "it has a lot of professional feel to it ... the lines are organized, properly spaced, each line is aligned ... choice of places for tables, choice of font, how the annotations happen". Also: "we are missing a few algorithms and whole lot of ui/ux improvement".

The plan is to add a CRAFT score beside completeness and truth. Each check is computed from the rendered SVG, has a rule ID and a tolerance, and is held by the regress ratchet:

- alignment of dimension rows and chains, and equal stacking lanes;
- spacing: the first dimension line is about 10 mm from the outline, then 7 to 8 mm between lines (ISO 129 practice); text clear of lines; crowding;
- line weights: two widths in a 2:1 ratio, assigned by line type (ISO 128);
- one drafting font to ISO 3098 type B, 3.5 mm on A3 and larger, used the same way in dimensions, tables and the title block;
- annotation style: arrows 3 mm, filled, about 15 degrees and identical; leaders at 30 to 60 degrees, never crossing anything; text reading from the bottom or the right;
- table zones: hole and revision tables anchored to the frame and title-block grid, never floating;
- view layout: equal gutters, exact projection alignment, balanced margins;
- the title block grid (ISO 7200).

Missing algorithms, scored on this bench:
- the dimension scheme: a minimal complete set as a spanning tree over the features, using the kernel's DOF accounting, with baseline, chain and ordinate styles, symmetric dimensions and counted callouts;
- view selection: the best front view, dropping redundant views, sections, detail, auxiliary and broken views;
- annotation placement: candidate positions, a collision graph, then optimisation;
- line work: fillet silhouettes, tangent edges, thread conventions, selective hidden lines.

Then the editor UI/UX:
- direct manipulation;
- a smart-dimension tool, checked against the solid;
- templates;
- the checker redline workflow;
- the revision diff;
- agent commands;
- the certificate re-checked live while editing.

Reference material: incumbent sheets, plus Hanomi drawings ONLY with Varun's permission, kept internal and never committed.

### The layout model: break the sheet into spaces, never average (Varun 2026-11-20)

Varun: "the key is not to average anything ... but break down the drawing in multiple spaces .. then focus on how to utilize those spaces ... using some algorithm". NIST, the standards and the standard textbooks are the starting references. The sheet is laid out as a hierarchy of spaces, each with its own job and its own score:

1. **Fixed zones, placed first by rule and never floating:** the frame, the title block (ISO 7200), the notes strip, and the table column stacked above the title block.
2. **View cells, placed by constraint layout in the remaining space.** Projected views must align exactly, gutters are equal, margins are balanced. A Cassowary-style solver states this directly instead of correcting it afterwards. The paper/scale search runs on top and scores by how well the zones fit.
3. **The space inside each view cell:** the geometry stays fixed, surrounded by four annotation bands (top, bottom, left, right).
   - Dimensions are assigned to a band by their direction and the datum they reference.
   - A band's depth comes from how many lanes it needs.
   - Lanes sit at the professional pitch: about 10 mm from the outline, then 7 to 8 mm apart.
4. **Free-space map.** The largest empty rectangles are kept in each band and in the leftover paper. Leaders, callouts and FCFs are placed into them in priority order: holes and fits first, general sizes last. Anything that does not fit is reported, never jammed in.
5. **Scoring per space.** Each zone, cell and band is scored on alignment, spacing, crowding and empty paper, and the sheet's grade is its WORST space. One cramped cell cannot hide behind an empty one.

This replaces the greedy lanes and "any free slot" placement. The 3f084e9a table rule (tables take their spot from the free paper by the same rule the search uses) was the first step.
