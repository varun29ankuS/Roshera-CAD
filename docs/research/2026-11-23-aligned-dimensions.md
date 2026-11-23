# Aligned dimensions (ISO 129-1)

Root cause it answers: the layout draws a linear dimension along a view's
horizontal or vertical only, so a length whose measured direction lies in a
view's plane but on neither axis (a slot 60 degrees round in TOP, CADGenBench
103's "12.49" at 45 degrees, NIST CTC 04's "20"s at about 22 degrees) is drawn
true in no principal view. Since the true-length rule (bc7550fe) such a length
is never offered and the certificate owes it
(`completeness::OmissionReason::NotTrueLengthInAnyView`).

ISO 129-1 answers it with an ALIGNED dimension: the dimension line parallel to
the measured direction, extension lines square to it, the value written along
the line (readable from the bottom or the right of the sheet).

## Status (2026-11-23): built, uncommitted

Implemented as designed below: `dimensioning::draws_aligned` /
`shows_true_length_in_plane`, `layout::place_aligned` /
`is_aligned_placement`, the general `placed_text_box`, the parallel-line
`find_dimensions_on_each_others_lines`, `hands::Shape::Aligned`, the aligned
arms of `dimension_band_depths` and `shift_placed_dimension`, and the
every-view `completeness::true_length_in_no_view`. Not done: an
auxiliary view for lengths skew to all three principal planes (still owed,
`NotTrueLengthInAnyView`).

## Which candidates become aligned

`dimensioning::project_records` draws a `length` / `position` / `extent` as
`a, b = anchor -/+ direction * value / 2`, projected into the view. So:

- `shows_true_length(d)`: the dominant-axis span `max(|dx|, |dy|)` equals the
  value -- drawn true along the view's horizontal or vertical (unchanged).
- NEW `draws_aligned(d)`: not `shows_true_length`, and the EUCLIDEAN span
  `|b - a|` equals the value (within the same tolerance, 1 um or 1e-6 of the
  value). Equivalent to: the measured direction lies in the view's plane and
  on neither of the view's axes.
- `shows_true_length_in_plane(d) = shows_true_length(d) || draws_aligned(d)`:
  the candidate filter (`candidates_from_records`) and the certificate's
  `true_length_in_no_view` both use it, so the layout and the certificate
  cannot disagree about which views owe a length.

A length foreshortened in every principal view (its direction out of all three
planes) is still owed with `NotTrueLengthInAnyView`; that needs an auxiliary
view, out of scope here.

`true_length_in_no_view` used to stop at the first oblique reading ("one
oblique reading is enough": a direction on a world axis is true in every view
it reads in). That is no longer so -- a direction in the XY plane is oblique in
FRONT and RIGHT and aligned in TOP -- so a length is now owed only when NO
principal view draws it true or aligned. An aligned candidate the layout finds
no room for is owed through the ordinary per-view path (`NotOnSheet`, or
`NotPlaced` for a carried one), never covered silently.

Same-view dedup (`dedup_dimensions_global` Pass A) classifies a span as H or V
and compares intervals along that axis; an aligned span has no such interval,
so aligned candidates are compared only with each other
(`same_aligned_span`: equal value, the same two ends either way round) and
one is kept -- CADGenBench 103 has two records of one "12.49" and drew it
twice. Completeness's `same_statement` uses the same rule for an aligned
candidate, and never credits an aligned span from a level or upright
dimension (or the reverse) that merely shares its dominant interval.

## How `PlacedDimension` carries it

No new field. An aligned dimension is a straight dimension whose `line` is
neither horizontal nor vertical:

- `line`: the dimension line, parallel to `b - a` (sheet space), offset from
  the feature along the normal by the lane distance; its length equals the
  drawn span (`value * scale`) unless the text is taken outside the span, when
  it runs on under the text as a stacked dimension's does.
- `ext`: from `GAP` off each feature point along the normal to `EXT` past the
  line -- square to it.
- `arrows`: tips at the line's ends, `dir` along the line (inside arrows
  pointing out of the span's middle; outside arrows when the text does not fit).
- `text_anchor`: the middle of the text, `TGAP` off the line on the text's
  "above" side; `text_rot_deg`: the line's angle normalised into [-90, 90) so
  the value reads from the bottom or the right (ISO 129-1).

`layout::is_aligned_placement(pd)` (not ordinate, no arc, line not degenerate,
line on neither axis) is the one predicate every reader uses.

## Readers that assumed 0 / -90 degree text or H/V lines

| Reader | Assumption | Change |
|---|---|---|
| `layout::placed_text_box` | rot 0 -> horizontal box, anything else -> the -90 box | general rotation: the axis-aligned box of the rotated text rectangle (0 and -90 keep their exact formulas). The layout searches with this same box, so placement and checks agree. |
| `verify::find_dimensions_on_each_others_lines` | `close()` only compares two vertical or two horizontal extension lines | any two PARALLEL lines: gap = perpendicular distance, overlap = along the first line. H/V cases reduce to the old arithmetic. The pair test is `verify::on_each_others_lines`, also called by the aligned placement. |
| `verify::find_span_overflows`, `find_lines_through_text` | none (euclidean span, segment tests) | none |
| `spaces.rs` `shape_of` / `lane_coordinate` | `Linear` lane is a row or column | aligned is `Linear` with no lane coordinate (already `None` for a slanted line); bands are read from its box |
| `hands.rs` `shape` | `Linear(horizontal)` | new `Shape::Aligned`: band/lane verbs refuse it with a typed reason (`aligned`), `drawing_align` refuses it as not one row |
| `svg.rs` | `rotate(text_rot_deg)` already general | none |
| `dxf.rs` `dimension_ink`, `linear_definition` | rotation already general; definition reads the line's own direction | none; tested on an aligned dimension |
| `layout::dimension_band_depths` | a stacked dimension with rotation != 0 is in the LEFT band | aligned dimensions are skipped (they sit at the feature, not in a band) |
| `layout::shift_placed_dimension` | linear = horizontal or vertical | aligned: the drag is split along the line (slides the text) and across it (moves the line, extension ends and arrows) |

## Placement: where an aligned dimension finds room

After the stacks, the ordinate bands and the leaders of the view (they keep
their current places and priority), each aligned candidate is placed by
`place_aligned`:

1. Sheet points `A`, `B` of the feature, unit direction `u`, normal `n`.
2. Sides: `+n` and `-n`, the one pointing away from the view's geometry centre
   first (out of the part).
3. Lanes along the normal: offset `STANDOFF + i * STACK`, `i = 0..` out to the
   view's reach (the diagonal of its geometry box plus `STANDOFF`), so a
   feature inside the outline gets its number outside it (its extension lines
   then run across the part, which ISO 129-1 allows but a drafter avoids --
   see "Open").
4. A lane is taken when the text box is clear of other text, of every line
   already placed (this view's and earlier views'), of other views and titles,
   OUTSIDE this view's geometry rectangle (the quality check's
   `DimensionOnGeometry` -- a first build that only kept the text off the
   view's lines drew 8 such errors on the bench), inside the frame; its lines
   run through no text; and it is not printed on another dimension's lines
   (`verify::on_each_others_lines`, factored out of
   `find_dimensions_on_each_others_lines` so placement and check share it --
   CTC 04's two near-equal "20"s were). Two passes: first a lane whose lines
   cross no line already drawn (112's near-level "37.54" ran its extension
   lines through five leaders), then, if none, any clear lane.
5. Text centred on the span when it fits between the arrows (`2 * AR_L` and
   air); otherwise beyond the end, the line run on under it, arrows outside.
6. No lane clear: the dimension is not placed, and the certificate owes it
   by name (`NotOnSheet`), as any dimension without room.

## Certificate

The certificate re-measures a dimension from its RECORD (`dimension_live_check`
by pid / entities), not from the ink, so an aligned dimension is re-measured
exactly like any other. Completeness credits an aligned candidate when it is
inked in the view that offers it, as for any candidate.

## Open

- Long extension lines across the part: an interior feature's number must
  clear the whole geometry rectangle, so its extension lines can run tens of
  millimetres across the view. A drafter would rather break the outline rule
  locally, use a leader, or a detail/auxiliary view; not decided here.
- Lengths skew to all three principal planes: auxiliary view, not built.

## Tests (RED first, mutation-proven)

- an in-plane oblique length is a candidate of the plane-parallel view and of
  no other (mutation: drop `draws_aligned` from the filter);
- placed: line parallel to the direction, drawn length = value x scale,
  extension lines square to it, text along it (mutation: place as H/V);
- `placed_text_box` at 30 degrees contains every corner of the rotated text
  (mutation: the old -90 box);
- SVG inks the slanted line and `rotate(angle)` text; DXF `linear_definition`
  reads feature points `value` apart;
- `find_dimensions_on_each_others_lines` reports two parallel aligned
  dimensions 0.3 mm apart (mutation: the H/V-only `close`);
- completeness: an in-plane length is no longer `NotTrueLengthInAnyView`
  (mutation: owe at the first oblique reading);
- `place_aligned` with no clear lane returns nothing, and takes the next lane
  out when the first is blocked (mutation: place regardless);
- the number stands outside the view's geometry; near-equal aligned spans are
  not printed on each other's lines; a drawn line is not crossed while the
  other side is free (each mutation-proven);
- one aligned span stated once (dedup) and credited by its twin
  (completeness); `hands` reads an aligned dimension as `Shape::Aligned`.
