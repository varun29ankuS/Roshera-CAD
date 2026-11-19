---
title: For engineering teams
description: Production drawings from your STEP files, parts from their specification, and a kernel that refuses to release a sheet that disagrees with its part.
---

<div class="facts"><div><b>1 step</b><span>STEP file to a dimensioned, toleranced production drawing</span></div><div><b>0.1 mm</b><span>every printed value re-measured against the solid</span></div><div><b>0</b><span>wrong sheets released: a stale sheet cannot be exported</span></div></div>

## The problem

Drawings are still made by hand from 3D models, one dimension at a time, and checked by eye. They take hours, they drift out of step with the model when the part changes, and the mismatch is found on the shop floor. Most "AI for CAD" adds speed without adding any check, so it produces wrong drawings faster.

## What Roshera does

**Production drawings from the model.** Import a STEP file, or build the part in Roshera. The drawing comes with views and sections, dimensions spaced to ISO 129, a hole table, fits from the ISO 286 tables (`Ø40 H7 (+0.025/0)`), datums and position tolerances verified on the real faces, profile ordinates for freeform outlines, and an ISO 7200 title block. The sheet size and scale are found by search, preferring full size.

**Checked before you see it.** A sheet certificate re-measures every printed value on the solid, lists anything the sheet leaves out, and refuses release when the sheet no longer matches the part.

**Edited like a drawing.** Move dimensions and views, set tolerances, add notes. On parts built in Roshera, change a size on the sheet and the part rebuilds from its history. Every edit is an undoable step.

**An agent that asks.** Describe what you need. The agent reads the part, asks for what it must not invent (material, process, which features take a fit), and explains what it measured.

## Proof

- [Watch it](/#watch): import → drawing → close reading → edits on the sheet, real recordings.
- [How the sheet certificate works](/drawings).
- [An aerofoil stated by its ordinates](/blog/an-aerofoil-is-stated-by-its-ordinates): measured from the solid, within 0.15 mm of the NACA formula.
- [A DXF that opens right in other CAD tools](/blog/a-dxf-that-opens-right-in-other-cad-tools).

## Typical parts

Machined and fabricated parts: brackets, plates, flanges (EN 1092-1), housings, shafts, aerofoil ribs. Playbooks for each family look up standard sizes from cited tables rather than from memory.

## Where it stands

<ul class="status"><li data-s="today"><b>Today</b>STEP import, certified solids, production drawings with fits, hole tables, GD&T and profile tables, on-sheet editing, PDF / DXF / SVG export.</li><li data-s="building"><b>Building</b>Detailed manufacturability checks (DFM), drawing layout you arrange view by view, meshing and simulation that feed back into the design.</li></ul>

## Start a pilot

Send us a part you make today (STEP is ideal), the process, and what your drawings must show: standards, tolerancing, title block, projection method. We return the drawing and its certificate. [info@roshera.tech](mailto:info@roshera.tech?subject=Roshera%20pilot)
