---
title: A DXF that opens right in other CAD tools
description: Millimetres that read as inches, circles as polygons, a white sheet, 3,600 objects. What it took to make a drawing's DXF open properly elsewhere.
date: 2026-11-18
---

A drawing is not finished when it looks right in the tool that made it. It is finished when it opens right in the tool the shop uses. We opened Roshera's DXF drawings in FreeCAD and found a list of problems, each small, together fatal.

## What was wrong, and the fix for each

| In the other tool | Cause | Fix |
|---|---|---|
| The drawing read as inches | no units in the header | the header states metric and millimetres |
| Circles and arcs were polygons | arcs written as many short lines | runs of points on an arc are written as true arcs (bulged polylines) |
| Dimensions were loose text | dimensions written as lines and text | linear dimensions are real dimension entities |
| Ø and × arrived as escape codes | an old DXF version | a version that carries UTF-8 text |
| The part was invisible on a white sheet | colour index 7 (white on white) | near-black ink for the part, a single accent colour for annotation |
| 3,600 objects, slow to open | every segment its own entity | views as blocks, joined polylines: 7 to 11 times fewer vertices |
| Rotated text tilted | an alignment point some importers misread | text is left-justified with no alignment point |
| Ellipses missing | the importer skips ellipses inside blocks | ellipses are written on the sheet, outside the view blocks |

## Two tones

The drawing uses two tones: the part in near-black, everything said about the part (dimensions, tables, frame, title block) in one accent colour. The part reads first, then what is said about it.

## Checked by tests

Each of these is a test on the exported file: the units, the arc count, that no layer uses white, that every view is one block, that no text carries an alignment point, that no ellipse sits inside a block. A drawing exchange format is a contract with someone else's software, and the tests hold us to it.
