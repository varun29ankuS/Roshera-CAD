# Drawing quality critique: STEP -> production drawing

Date: 2026-11-17. Analysis only: nothing was built, run or edited.
Markers: **[V]** read in the code or seen in a file; **[S]** seen on a rendered sheet; **[I]** inferred, needs a run.

## 0. Verdict

The pipeline is honest and tidy, but it is not yet a production drawing generator. Three things stand between it and "world class":

1. **The sheet can be incomplete and still certify sound.** `select_dimensions` silently drops dimensions (all angles when there are more than two, all but five diameters/radii, everything past eight per view), and the certificate's completeness walk only covers bores and GD&T. [V]
2. **The part is drawn in whatever frame the STEP file holds it, with a fixed Front/Top/Right/Iso set.** There is no choice of front view, no reduction to the views the part needs, and the one section is undimensioned and not aligned to its parent. [V]
3. **Nothing a checker or homologation engineer signs for is on an imported part's sheet**: no per-dimension tolerance is ever drawn, no thread callouts, no datums or frames unless authored, and the text is below ISO minimum height. [V]

The earlier agent's first editing slice (authored tolerance shows on the sheet) is the right first *editing* slice if it also covers linear dimensions, but it should not be the first thing built. Automatic sheet quality comes first, because the Volvo audience will judge the untouched sheet of their own part before anyone edits anything.

## 1. What I looked at

| Material | What |
|---|---|
| Reference drawings | CADGenBench 101, 102, 105, 110, 120, 125, 133 (PNG) |
| NIST | CTC-01 PDF (the others not opened) |
| Earlier agent | `PHASE1.md`, `EDITING_DESIGN.md`, `score.py` in `.claude/worktrees/agent-abc1fbf8d1987237f/roshera-eval/drawing-bench/` |
| Code | `geometry-engine/src/drawing/` (`dimensioning.rs`, `section_view.rs`, `sheet_certificate.rs`, `layout.rs`, `svg.rs`, `pdf.rs`, `part_frame.rs`, `hole_table.rs`, `verify.rs` issue kinds); `drawing-overlays.tsx` only by grep |
| Real Roshera sheets | **One**: `brag-output/work/steps/drawing.svg` (flange, A3, dated 2026-09-26), which I rendered with Inkscape. Plus a UI screenshot `Roshera-vault/Business/deck-assets/ui-drawing-sheet.png` (2026-07-21). |

**No current sheet exists on disk.** The 09-26 sheet predates about 22 commits to `drawing/` (11-11 to 11-14: round-part conventions, hole stacks, fillet outlines, visibility fix, ISO 7200 title block). So everything in section 3 marked [S] describes the pipeline seven weeks ago; the current state is judged from code. I did not read `drawing_mgr.rs`, `drawing_edit.rs`, `DrawingsWorkspace.tsx` or `SheetPanel.tsx` in full; claims about them are taken from the earlier agent's documents.

The CADGenBench references print "this drawing deliberately departs from drafting standards". They are a good inventory of *what content* a dense drawing carries (sections with depth dimensions, details, TYP notes, counts). They are a bad style target: 120 is unreadable, 105 dimensions from a centre origin with duplicated values, 110 mixes comma decimals. Recall against them measures coverage, not quality.

## 2. What separates world class from acceptable

### 2.1 The machinist

A machinist wants to set up and cut without phoning anyone. In order of how often a drawing fails them:

- **Complete and non-redundant.** Every feature is sized once and located once. No feature needs arithmetic to find. No closed chains; if an overall is given with all its parts, one is a reference in parentheses.
- **Dimensioned the way it is made and measured.** Locations come from the surfaces the part sits on in the vice or fixture (two or three datum faces, or a bore and a face), as baseline or ordinate dimensions. Chain dimensions only where the function is the pitch itself. Hole-to-hole pitch is stated directly when the mating part is located by those holes.
- **Dimensions on visible outlines, in the view that shows the true shape.** Never to hidden lines; if the feature is internal, there is a section. Diameters of turned features are on the longitudinal view or section, not as radial leaders on the end view. Radii are on the view that shows the arc.
- **Hole callouts are complete, on one leader**: count, diameter, THRU or depth, counterbore or countersink, thread designation with class and thread depth, pattern ("6x Ø10 THRU EQ SP ON Ø90 PCD"). A coordinate hole table is right for irregular patterns of many holes, wrong for a bolt circle.
- **Sections and details where they earn their place.** A section through each distinct internal profile, cut through hole axes, carrying the depth and wall dimensions. A detail where a feature would print under about 3 mm.
- **Default notes that remove clutter**: unspecified radii, break sharp edges, general tolerance class, default surface texture, deburr. "TYP" and counts are true.
- **Legible at arm's length and at half size.** A2 sheets are printed on A3 in most shops. Text 3.5 mm on A2 and larger (2.5 mm minimum anywhere), thick 0.5 / thin 0.25 line group, first dimension line about 10 mm off the outline and 7 mm or more between lines, shorter dimensions nearer the part, no dimension line crossing another, no text on geometry.
- **No false precision.** ISO 129-1 metric values have no trailing zeros ("120", not "120.00"); the decimals shown match the tolerance.

### 2.2 The homologation / product engineer at a truck OEM

They do not make the part. They release it, hand it to a supplier, and file it with a test agency. They look for:

- **Identity and control**: part number, revision letter with a revision table (what changed, who, when), drawing status, drawn/checked/approved names and dates, sheet n of m, document owner. ISO 7200 fields filled, not dashes.
- **Material and condition**: material with its standard designation, heat treatment, surface treatment or coating, mass (computed, with the density stated).
- **Interfaces, not everything.** Mounting-hole patterns, mating faces, envelope and clearance dimensions, with real tolerances. This is where position tolerances and datums matter: datum A the mounting face, B and C the locating holes or edges, position of the hole pattern to A|B|C, flatness of the mounting face, profile on a clearance surface.
- **Tolerancing strategy**: a general tolerance class in the title block (ISO 2768-m or the newer ISO 22081 approach) and explicit tolerances only where function needs them. Fits by ISO 286 designation with limits shown. Tolerance stack-up answerable from the sheet.
- **Special characteristics flagged** (safety, regulatory or fit-critical dimensions carry a symbol and are listed) so inspection plans follow from the drawing.
- **Standards declared**: projection symbol, units, scale, tolerancing principle (ISO 8015 or ASME Y14.5 edition), general tolerance standard. **Check the projection default for this audience**: Indian practice (BIS SP 46) and European ISO practice are first angle; the 09-26 Roshera sheet states third angle. [I: confirm what Volvo's drawings use]
- **Traceability to the 3D model**: the sheet names the model and its revision, and the two agree. This is exactly what the sheet certificate proves, and no incumbent proves it.
- **Change visibility between revisions.** What moved between rev A and rev B.

### 2.3 Reference standards for the numbers

ISO 128 (line types, views, sections), ISO 129-1 (dimensioning), ISO 3098 (lettering heights 2.5 / 3.5 / 5), ISO 5455 (scales), ISO 5457 (sheet, frame, zones), ISO 7200 (title block), ISO 1101 / 5459 (geometrical tolerances, datums), ISO 286 (fits), ISO 2768 or ISO 22081 (general tolerances), ISO 21920 (surface texture indication), ISO 6410 (threads), ISO 13715 (edges of undefined shape). ASME equivalents: Y14.5, Y14.3, Y14.2.

## 3. Roshera against that bar

### 3.1 Strong: keep and show in the video

- **Sheet certificate and export refusal** (`sheet_certificate.rs`): each inked value is re-measured against the live model; stale and dangling facts are typed; a tampered frame fails. Nothing in SolidWorks, NX or CATIA does this. It is the homologation story ("the drawing and the model cannot disagree").
- **True hidden-line removal with provenance** per polyline, centre lines, ISO line-weight hierarchy (0.5 visible, 0.25 hidden, 0.18 dimension), frame with zones, projection symbol. [V][S]
- **Dimension spacing constants are right**: first line 11 mm, stack 8 mm, extension gap and overshoot 1.5 mm, 3:1 filled arrowheads (`layout.rs:188-193`). [V]
- **First/third angle switch that re-lays out the views**, dragged views kept on their projection line. [V from commit log and PHASE1]
- **Round-part and hole-stack conventions** (Ø outside, PCD as one callout, "4x Ø6.60 THRU, counterbore"), chamfer folded into one callout, each dimension once across views (`dedup_dimensions_global`). [V]
- **Dimension edit drives the part** on Roshera-built parts, and refuses honestly on imported ones.
- **GD&T as recorded design history** with datums and frames drawn from the model sidecar, and a semantic readback an agent can query.
- **Vector PDF/DXF from one source** (`pdf.rs` transcodes the SVG).

### 3.2 Weak: verified in the current code

| # | Finding | Where | Why it matters |
|---|---|---|---|
| W1 | **Silent dimension dropping.** More than two angles in a view: all angles removed. Equal (kind, value within 0.5 mm) in a view: collapsed to one with no count. Diameters and radii capped at five (largest three, smallest two). Everything else capped at eight per view. | `dimensioning.rs:220-281` `select_dimensions` | A part with six distinct bores loses one. Two different 12 mm steps become one. Reference 125 has eleven angles: it would get none. |
| W2 | **The certificate does not see W1.** The live-side walk is `omitted_bore_facts` plus `gdt_live_side_facts` only. A dropped radius, angle, step or slot is not an `Omitted` fact, so the sheet certifies sound. | `sheet_certificate.rs:1211-1213` | This contradicts "the kernel cannot lie". A shop reader assumes a sound sheet is complete. |
| W3 | **No view selection.** Views are hard-coded Front, Top, Right, Isometric along world axes. `draw_in_part_frame` only undoes a recorded assembly placement; a single-part STEP is drawn as modelled. | `dimensioning.rs:1539-1544`, `part_frame.rs` | OEM parts are usually modelled in vehicle position. A bracket tilted 12 degrees gets three skewed views and projected widths. A shaft gets three views where one plus a section is correct. |
| W4 | **One section, only when bores exist, never dimensioned, not aligned to its parent.** `section_view` emits no dimensions; the section is packed into whichever slot gives the largest scale. | `dimensioning.rs:3369-3385, 2803-2830, 2942` | The references carry their depth and wall dimensions in sections (101, 105, 125, 133). Ours has to hang them on hidden lines or drop them. Pocket and cavity parts get no section at all. |
| W5 | **Section drafting quality.** Hatch pitch is 4.0 model units before scaling, so it prints at 0.8 mm at 1:5 and 8 mm at 2:1. The cut outline is two-point segments from the tessellated cap (arcs are faceted; DXF receives segments). Edges behind the plane are added with no occlusion among themselves. | `section_view.rs:38, 114-150` | Hatch that changes pitch with scale and faceted arcs read as amateur at a glance. |
| W6 | **No tolerance is ever drawn.** `ToleranceRef` is joined for diameters only and no renderer prints it. Fits do not resolve (no ISO 286 table). | grep of `svg.rs`, `layout.rs`, `dxf.rs`; EDITING_DESIGN section 0 | The homologation engineer's first question. |
| W7 | **Text is under the ISO minimum.** Dimension font-size 3.1 mm, notes 2.4, tables and GD&T 2.6, title-block labels 2.4, all in generic `sans-serif`. Font-size is the em box, so capital height is roughly 70% of it: about 2.2, 1.7 and 1.8 mm. ISO 3098 minimum is 2.5; 3.5 is normal for dimensions on A3 and larger. The font is not pinned, so the PDF depends on what the machine resolves. | `layout.rs:26-44`, `svg.rs:211-251` | Unreadable when an A2 is printed on A3. Inconsistent output between machines. |
| W8 | **No thread, no blind-counterbore, no chamfered-exit callouts; irregular patterns fall to a table.** | `hole_features.rs` (no "thread" anywhere) | 101's "6x M12x1.75-6H 15 DEEP" cannot be stated. A STEP has no thread data, so this must be an authored annotation, suggested from tap-drill size. |
| W9 | **No detail, auxiliary, half, offset or aligned section, broken view; no general notes beyond units and ISO 2768-m; no surface texture, edge condition, revision table.** | PHASE1 section 2, confirmed by grep | See 2.1 and 2.2. |
| W10 | **Trailing zeros.** Hole-table labels are `{:.2}`; the 09-26 sheet prints 120.00, 37.00. | `hole_table.rs:520-550` [V], sheet [S] | False precision; against ISO 129-1. |

### 3.3 Seen on the 09-26 flange sheet (may be fixed since; re-check on a fresh render)

- TOP view: the Ø120 outline and a second ring are **dashed**, as if hidden. FRONT and RIGHT: the boss is dashed. A commit on 11-12 ("edges seen along their own face are visible") likely addresses this. [S]
- Ø60 is drawn as a vertical linear dimension standing beside the circle, with extension lines that do not reach the feature. The Ø120 appears as "120.00" with no Ø, twice (FRONT and RIGHT). The Ø30 bore appears only in the table.
- The six-hole bolt circle is an X/Y coordinate table, and through holes read "depth 12.00". The 11-11 round-part work targets this.
- "12.00" and "10.50" overlap. The 21.00 height is dimensioned to hidden lines.
- SECTION A-A: one half hatched, lines running past the part, the bolt hole not cut, not aligned with TOP, no dimensions.
- A stray circle at the boss fillet in FRONT (the 11-13 fillet-outline fix).
- The isometric draws every hidden edge solid over the shading, so the part looks transparent.
- Title block: drawn by, date and material are dashes; no general tolerance cell, mass, surface or units field. Every view carries "(1:1)" repeating the sheet scale.
- Good: frame and zones, projection symbol, line hierarchy, centre lines, clean title-block design, notes strip.

### 3.4 Is the earlier ranked gap list right?

Mostly right about what is absent; wrong about order, and it misses the most serious item.

| Earlier | My view |
|---|---|
| (missing) | **New top item: W1 + W2**, silent drops and a certificate that cannot see them. PHASE1 says the certificate "catches omitted bores" and stops there. |
| G7 orientation, "medium-high [I]" | **Raise to second.** The code confirms there is no orientation logic for a single-part STEP. Every other measurement is meaningless on a skewed view. |
| G2 sections, high | Agree, but restate: the first defect is that the section carries **no dimensions** and is **not aligned**; "more sections" comes after. |
| G1 detail views, ranked first | **Demote to about eighth.** A detail matters when a feature prints small. Dimensioned sections, orientation and completeness matter on every part. Details also cost the most (new view kind through layout, three renderers and the certificate). |
| G6 tolerance on sheet, high | Agree; raise above G3/G4 for the homologation audience. |
| G3 hole callouts, G4 radii/angles | Agree. G4 is partly W1: the angles and radii exist in the feature table and are then capped away. |
| G5 AP242 PMI import | High for Volvo if they export PMI. **Ask them which STEP flavour and CAD system they use before building.** If their STEP is geometry-only, this is not on the demo path. |
| G8 density/performance | Agree, medium. Do not demo a part like 120. |
| G9 notes/TYP | Raise slightly: a notes block is cheap and is the first thing a machinist reads. |
| G11 labels, formats | Split out **legibility (W7, W10)** as its own item, near the top. It is two days of work and changes the first impression of every sheet. |
| (missing) | **Dimension scheme**: nothing chooses datums and dimensions from them; locations come from a bounding-box corner. Missing: no dimensions to hidden lines when a section exists. Missing: title-block content for release (material standard, treatment, general tolerance, revision table). |

On PHASE1's method: the reconstruction caveat (the STEP files are another team's re-models) is correct and the C0 filter is the right answer. `score.py` has real defects: it is recall-only, so over-dimensioning scores well; it matches on value alone, so any 12 on the sheet satisfies any 12 in the reference; hole matching uses a substring search (`12` matches `112`); view kinds are matched against all text on the sheet; the tolerance check matches the general-tolerance note's "±0.1". Fix these before quoting a number.

## 4. Critique of EDITING_DESIGN.md

### 4.1 What is right

- A dimension is a spec bound to persistent ids; the value is measured, never typed. Authored and automatic dimensions are one type on the sheet.
- A lost entity is a typed dangling refusal, never a silent re-target.
- Edits are timeline events, with placement stored separately from GD&T content.
- Driving versus reference is derived from the recorded history, and reference dimensions print in parentheses.
- An ISO 286 table rather than a formula, with a refusal outside its range.
- A taste fault (an overlapping move) is kept and reported; only a lie is refused. This is the correct line.

### 4.2 What is wrong or over-engineered

- **`TextContainsValue` and the closed-word grammar will refuse normal drafting.** The commonest suffix an engineer types is a thread or a standard: "M12x1.75-6H", "Ra 3.2", "SEE NOTE 3", "DIN 74". All contain digits. The kernel stays honest by labelling provenance, not by forbidding typing: keep verified structured fields (count, depth, thread) and allow free text beside them, recorded in the certificate as "authored text, not verified".
- **`CountMismatch` on "4X"** is right as a warning in the certificate, wrong as a refusal: the user may be counting across two patterns the recogniser splits.
- **A preview round trip on every cursor move** will lag. Return the pick geometry once and draw the ghost in the client; measure on commit.
- **Too early**: composite frames, ISO 2768-2 K/H/L, the `/api/iso286` route, inch leading-zero rules, DXF MTEXT stacking, ordinate/baseline/chain as their own event kinds. None is needed before slice 3.

### 4.3 What is missing

- **Picking as SolidWorks/NX users do it.** They pick what they see in the view: silhouette edges of a cylinder (no B-Rep edge exists there), virtual sharps (the theoretical corner under a fillet or chamfer), midpoints, circle centres, and arc conditions (centre, minimum, maximum). Width over two filleted sides and slot length are among the commonest dimensions and need these. `EntityRef` has faces, edges, edge ends and axes only.
- **Re-import is the OEM workflow, and the design defers it.** Persistent ids on imported parts are keyed by file content, so revision B of the STEP makes every authored dimension, tolerance and frame dangle. That is honest and unusable. Before investing in authored annotations on imports, design a re-attach by geometric signature that produces a review list ("38 re-attached, 2 need you"), certified as re-attached, never silent.
- **Choosing the datums of the automatic scheme.** The most valuable single edit is "dimension this part from these two faces and this hole", after which the automatic dimensions regenerate as baselines from them. It replaces dozens of manual moves.
- **Views are part of editing a sheet.** Section (draw the cut line on a parent), detail (circle and scale), projected view from a chosen parent. The design covers dimensions only.
- **Notes and leaders are left as disabled stubs.** A leader note and an editable general-notes list are daily actions and are cheap.
- **Also absent**: surface texture symbol, centre mark and centre line add, auto-arrange and align of selected dimensions, multi-select with "apply to all", moving a dimension to another view by drag, special-characteristic flag and numbered inspection balloons, revision table with change markers (the timeline can generate this automatically, which no incumbent does well), sheet templates, multi-sheet.
- **Suppress with a waiver is good**, but it depends on W2: today most omissions are not facts at all.

### 4.4 Is "an authored tolerance shows on the sheet" the right first slice?

Right content, wrong position, and too narrow.

- **Position**: it should come after the automatic sheet is trustworthy on the demo parts (plan items 1 to 5). If the untouched sheet of a Volvo part is skewed or missing dimensions, a tolerance editor does not rescue the demo.
- **Width**: as written it only reaches diameters, through the existing size-tolerance route. Release tolerances on a truck part are mostly on hole positions and interface distances. Include linear dimensions in the same slice (the design's T4).
- **For the video**, the convincing sequence on an imported part is: select a dimension, set a tolerance in a property panel, drag it clear, delete one, and the certificate chip updates. So the first editing slice should be "property panel with tolerance for any dimension, plus move and suppress", using the rendering and ISO 286 work the design already describes.
- **GD&T for the demo does not need sheet-side authoring**: datums A, B, C and a position frame on the mounting pattern can be authored by the agent through the existing routes and drawn by the existing sidecar path. Test that on an imported part first (the design's T13).

## 5. A measurable quality bar

Three layers. Thresholds are my proposal; adjust after the first baseline. Split the 49 CADGenBench parts into 30 development and 19 held out, and never tune on the held-out set or on the NIST parts.

### 5.1 Automated, per sheet, no reference

| Id | Check | Source | World-class threshold |
|---|---|---|---|
| A1 | Sheet builds; wall time | bench harness | 100% build; 95th percentile under 30 s |
| A2 | Truth: no stale or wrong value, no non-true-length dimension | certificate | 0 on every sheet |
| A3 | **Completeness**: every feature in the analytic table is sized and located (inked, covered by a counted callout or a note, or an explicit waived omission) | certificate, after plan item 2 | 100% on at least 90% of parts; no part under 90% |
| A4 | No redundancy: no feature dimensioned twice, no closed chain without a reference dimension | `verify.rs` `RedundantDimension` plus a chain check | 0 |
| A5 | Legibility: text-text and text-geometry collisions; dimension lines crossing each other; dimensions to hidden edges; shorter-outside-longer ordering | `verify.rs` plus new checks | 0 collisions on 90% of sheets; at most 2 crossings per sheet; 0 to hidden edges where a section shows the feature |
| A6 | Text and lines: capital height at least 2.5 mm (3.5 for dimensions on A3 and larger); thick to thin ratio 2:1; hatch pitch 2 to 4 mm on paper | SVG parse | 100% |
| A7 | Layout: views projection-aligned; ISO 5455 scale; views cover 35 to 65% of the drawing area; no view without unique information | `verify.rs` plus new | 0 misaligned; 90% of sheets in the fill range |
| A8 | Orientation: the largest planar faces and principal axes are parallel to the sheet axes in the front view | new | 100% within 0.1 degree, or a typed "no natural frame" |
| A9 | Title block and declarations: ISO 7200 mandatory fields, projection symbol, units, scale, general tolerance, mass, material | `check_iso_7200` | all present on 100% |
| A10 | Exports: SVG, PDF and DXF produced; PDF fonts embedded; circles and arcs are true entities in DXF | harness | 100% |

### 5.2 Reference-assisted (CADGenBench, after the C0 reconstruction filter)

| Id | Check | Threshold |
|---|---|---|
| B1 | Dimension recall, matched on kind, value **and feature** (not value alone) | median at least 0.90; worst decile at least 0.75 |
| B2 | Dimension precision: ours that correspond to a real feature and are not duplicates | at least 0.95 |
| B3 | Hole callouts complete (count, size, depth or THRU, counterbore/countersink, thread where authored) | at least 0.95 |
| B4 | Section coverage: every internal profile the reference sections is sectioned or otherwise fully dimensioned on visible edges | at least 0.90 |

Do not score view placement, title block content or style against these references.

### 5.3 Looked at (two minutes per sheet, rendered PNG beside the reference)

Ten yes/no questions, one point each:

1. Is the front view the one a drafter would choose?
2. Are there only the views the part needs?
3. Could you make the part without asking a question?
4. Could you inspect it (are datums and tolerances usable)?
5. Is every value true and on the right feature? (A "no" fails the sheet outright.)
6. Are dimensions on visible edges, in the view showing true shape?
7. Do sections cut where they should and carry the internal dimensions?
8. Is it readable printed at half size?
9. Is the sheet balanced, with nothing crowded or stranded?
10. Would a checker sign it with at most three red marks?

World class: at least 8 of 10 on 85% of sheets, and question 5 is "yes" on all.

### 5.4 NIST (tolerances, datums, frames)

Apply each PDF's annotations through the GD&T routes, then score items present **and attached to the right feature** (by certificate fact, not by text search) over items in the PDF, plus frame syntax per ASME Y14.5 by eye. World class: at least 0.95 on all eleven; 100% of datums.

### 5.5 The comparison that settles "world class"

Take ten held-out parts. Produce the automatic drawing in Roshera and in one incumbent (its automatic model-items or auto-dimension pass, untouched). Show both, unlabelled, to three practising engineers. World class means Roshera is preferred or tied on at least 7 of 10. Until this is run, "world class" is a claim, not a result.

## 6. Ranked build plan

| # | What | Where | Why it matters | Measure | Size |
|---|---|---|---|---|---|
| 1 | **Bench harness and contact sheets.** Loop the 49 parts and 11 NIST parts: SVG, PDF, DXF, `Drawing` JSON, verify and certify summaries, stage timings; render every SVG to PNG (Inkscape is installed) into one page per part beside the reference. | new `export-engine/examples/drawing_bench.rs` (copy of `step_probe.rs`); fix `score.py` as in 3.4 | Nothing can be judged today: there is no current sheet on disk. | A1, A2 baseline; every sheet looked at once | 1 to 2 days |
| 2 | **Completeness accounting; remove the silent caps.** Certificate walks every feature record, not just bores. Replace `select_dimensions` drops with counted callouts ("3x R5"), a table, or a typed `Omitted`. | `dimensioning.rs:220`, `sheet_certificate.rs:1211` | Restores "cannot lie" for completeness; the machinist's first requirement. | A3; a RED test with six distinct bores and three angles | 3 to 5 days |
| 3 | **Orientation and view choice for imported parts.** Derive a natural frame (largest planar face down, main axis of revolution horizontal, longest extent along the sheet). Choose the front as the most descriptive view; drop views with no unique information; one view plus section for turned parts. Default first angle for this audience if confirmed. | `part_frame.rs`, new view-choice module, `standard_drawing_auto_with_projection` specs | Volvo parts arrive in vehicle position. A skewed view is the fastest way to lose the room. | A8, A7, looked-at questions 1 and 2 | 3 to 5 days |
| 4 | **Sections that carry the internal dimensions.** Trigger on any internal feature (pockets, cavities, blind holes, steps), cut through feature axes, place in projection from the parent, dimension depths and walls there, stop dimensioning those to hidden lines. Paper-constant hatch pitch, true arcs, occlusion behind the plane. | `attach_section_view`, `choose_section_plane`, `section_view.rs`, `layout_five_view` | Where the references put a third of their dimensions; the mark of a real drawing. | B4, A5 (hidden-edge count), question 7 | 1 to 2 weeks |
| 5 | **Legibility and declarations pass.** Text to 2.5 / 3.5 mm capital height with a pinned, embedded font; no trailing zeros; drop per-view scale labels equal to the sheet scale; title-block cells for general tolerance, mass, material, treatment, units; hide hidden edges in the pictorial. | `layout.rs:26-44`, `svg.rs`, `title_block.rs`, `hole_table.rs:520` | Changes the first impression of every sheet for two days' work. | A6, A9, question 8 | 2 to 3 days |
| 6 | **Datum-based dimension scheme.** Pick two or three datum features automatically (user can re-pick); locate holes and steps as baselines from them; keep functional pitches direct; diameters on the longitudinal view. | `readable::extract_dimensions` position records, `hole_features.rs`, `round_part.rs` | Dimensions that match how the part is fixtured and inspected; sets up datums A, B, C for GD&T. | A4, B1, questions 3 and 4 | about 1 week |
| 7 | **Tolerances on the sheet, with the first editing slice.** Structured dimension text, ISO 286 table, tolerances on linear as well as diameter dimensions, certificate ink check; property panel on a selected dimension with tolerance, move and suppress. | EDITING_DESIGN slice 1 plus T4, T8, T9; `svg.rs`, `dxf.rs`, `gdt/`, `DrawingsWorkspace.tsx` | The homologation engineer's question, and the editing moment for the video. | NIST score for size tolerances; T1 to T4 | 1 to 1.5 weeks |
| 8 | **Hole callouts completed.** Blind depth, counterbore or countersink on blind holes, counted irregular patterns on non-round parts, thread as an authored annotation suggested from tap-drill size and never asserted. | `hole_features.rs`, `hole_table.rs` | The drill-and-tap sheet. | B3 | about 1 week |
| 9 | **General notes and TYP.** A notes block: unspecified radii (from the dominant fillet radius, counted), break edges, general tolerance, default surface texture; "TYP" only with a verified count; free leader notes recorded as authored. | `Drawing::notes`, `dimensioning.rs` dedup, `types.rs` | Removes clutter the way drafters do; first thing a machinist reads. | dimension count per sheet falls with A3 still 100% | 3 to 4 days |
| 10 | **Detail views**, generated when a dimensioned feature would print under about 3 mm, and available as a tool. | new `drawing/detail_view.rs`, `layout.rs`, three renderers, certificate facts | Small features on large parts (105, 110). | B1 on small features; A5 | 1 to 2 weeks |

After these: AP242 PMI import scored on NIST (once Volvo confirms what they export), re-attach of annotations across STEP revisions, the smart-dimension tool with silhouette and virtual-sharp picks, revision table generated from the timeline.

### For the demo and the video

- Choose three parts where the pipeline is strongest and the story is clear: a round hub or flange, a prismatic bracket with hole patterns, a machined housing with one clean section. Not a ribbed casting.
- Sequence: import STEP, sheet appears, switch first/third angle, the agent adds datums A, B, C and a position tolerance on the mounting pattern, set a tolerance on one dimension, tamper with a value and show the certificate refuse the export.
- Ask Volvo now: which CAD system and STEP application protocol, whether their STEP carries PMI, first or third angle, and whether they can send two or three non-confidential parts ahead of the demo.
