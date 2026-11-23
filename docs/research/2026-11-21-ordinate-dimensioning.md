# Ordinate dimensioning for long location ladders

Varun 2026-11-21: "Ordinate (ISO 129)". Root cause it answers: a view with a
long ladder of locations from one datum (CADGenBench 118's FRONT: about 20
positions) needs one stacked lane per location, more than `MAX_LANES` and
more paper than any sheet's gap, so the sheet search climbs sheets that cannot
help (118 ended on A1 at 1:2 with 1.2 % of the paper used) and the locations
are still not drawn.

## Status (2026-11-23): built

Wired in `layout::place_dimensions_routed` (`place_ordinate_band`), with
`drawing/ordinate.rs` (`find_ladder`, `spread`, `extension_line`). As built,
differences from the design below:

- Trigger is **10** locations (Varun 11-22), not 6: `ORDINATE_MIN_LADDER`.
- Members share the datum END and lie on ONE side of it (a location measured
  from elsewhere TO the datum's coordinate is not a member: 124's TOP "1.5").
- Both directions' bands are laid FIRST, nearest the part; both stacks then
  take lanes beyond and round them. No stacked number is put on a band line
  in any pass; a stacked extension line may still cross a band number (they
  share the ladder's features) and is broken round it.
- `verify::find_dimensions_on_each_others_lines` has an ordinate arm: two
  values whose lines leave the part < 0.5 mm apart are an error (a ladder
  drawn too small does not certify clean).
- Placed ids are matched to view dimensions through `layout::base_dim_id`
  (`-note`, `-origin`).
- Part 109 (11-23): A1 1:5 with 15 errors -> A0 1:1, every dimension placed,
  1 error (the older "5.5"/"6" pair); certificate sound + complete, 110
  consistent.

## What is drawn (ISO 129-1 ordinate dimensioning)

For one view, one direction (along x or along y), when at least
`ORDINATE_MIN_LADDER` (10; was 6 in this design) locations share one datum end:

- one band, on one side of the view (the stack's usual side, else the other);
- the datum's extension line carries `0`;
- every location's extension line runs from the part to the band, and its
  value is written at the END of the extension line, along it (rotated for a
  horizontal ladder, ISO 129-1 reading direction), no dimension line, no
  arrowheads;
- values closer than one text height plus a gap are spread apart in order;
  the extension line of a moved value is JOGGED: straight from the part to
  the band's knee line, then slanted to the value's place (ISO 129-1 allows
  a jogged extension line for ordinate dimensions).

Locations that are not in a ladder stay stacked dimensions.

## How it is carried

`PlacedDimension` gains `ordinate: bool` (`#[serde(default)]`). An ordinate
value is placed as:

- `line`: degenerate, at the foot of its text (no dimension line);
- `ext[0]`: part edge -> knee; `ext[1]`: knee -> foot (degenerate without a
  jog);
- `arrows`: present but not inked (`ordinate` says so);
- `text_anchor` / `text_rot_deg`: the value along the extension line.

The datum's `0` is one more `PlacedDimension` with `dim_id`
`"{first member}-origin"`, recognised (like `-note`) wherever placed ids are
matched to the view's dimensions (completeness, the sheet search's shortfall,
the profile).

## Who reads it

- SVG and DXF renderers: draw `ext` of an ordinate dimension (today a
  degenerate `line` skips them), skip its arrows.
- Quality checks: text collisions and lines-through-text are generic over
  items and line segments and apply unchanged; the extension-lines-apart
  check sees the ordinate's lines like any others.
- Space map: `shape_of` classes a degenerate line as `Other`: no lane rules
  apply (ALIGN-ROW / LANE-PITCH are for stacked lanes). Correct for ordinate.
- Band depth (`dimension_band_depths`): an ordinate band's depth is its
  standoff plus its widest value, not a stack.
- Layout verbs: an ordinate value moves as a point callout (its text);
  re-anchoring the band is a later verb.

## Tests (RED first)

1. A view with 8 locations from one datum draws them as ordinate: one band,
   a `0`, no arrows, every value placed, nothing unplaced.
2. Close locations are spread and their extension lines jogged; no two
   values overlap; no line runs through a value.
3. Fewer than 6 locations stay stacked (no behaviour change).
4. SVG inks the extension lines and no arrowheads for an ordinate value; DXF
   the same.
5. Bench: 118 (and 103) drawn complete on a smaller sheet, judged against
   the floors.
