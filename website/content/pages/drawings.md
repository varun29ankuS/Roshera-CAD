---
title: Drawings
description: Production drawings generated from the part, laid out on the right ISO sheet, and certified against the geometry before release.
---

<div class="facts"><div><b>4 verdicts</b><span>consistent · stale · dangling · omitted, for every printed value</span></div><div><b>ISO 286</b><span>fits looked up from the tables, never typed</span></div><div><b>PDF · DXF · SVG</b><span>with true arcs and real dimension entities in DXF</span></div></div>

## The problem

A drawing is a list of promises about a part. In most tools nothing checks those promises after the sheet is made: change the part, forget the sheet, and the shop machines the old numbers.

## What Roshera does

**Makes the sheet from the part.** First- or third-angle views with hidden lines; sections through bores and walls; dimensions measured from the exact geometry and stated once, where they read true; a hole table with tags and counted callouts (`3× Ø13 H7`); datums and feature control frames; profile ordinates for aerofoils and other freeform outlines; an ISO 7200 title block. Sheet size and scale are chosen by search over the ISO sizes.

**Certifies it.** Every printed value is re-measured on the live solid:

| Verdict | Meaning |
|---|---|
| consistent | the sheet says what the part measures (within 0.1 mm) |
| stale | the part changed; the sheet is out of date |
| dangling | the feature it describes no longer exists |
| omitted | the part has a feature the sheet does not show |

A sheet is **sound** when nothing is stale, dangling or omitted, and **complete** when every model dimension is stated or signed off by a named person. Release export is refused until the sheet is sound.

**Lets you edit it.** Drag dimensions and views, set tolerances (± value, limits, ISO 286 fit, basic, reference), add text such as `4X` or thread callouts. On parts built in Roshera, a new size typed on the sheet rebuilds the part from its history. Every edit is one undoable step.

## Proof

- [A drawing that checks its own numbers](/blog/a-drawing-that-checks-its-own-numbers): the certificate, both directions.
- [An aerofoil is stated by its ordinates](/blog/an-aerofoil-is-stated-by-its-ordinates): a mutated ordinate makes the sheet unsound in the test suite.
- [When a seam is not an edge](/blog/when-a-seam-is-not-an-edge): 160 stray lines removed by one rule, silhouettes kept.

## Where it stands

<ul class="status"><li data-s="today"><b>Today</b>Everything above, on imported STEP parts and parts built in Roshera.</li><li data-s="building"><b>Building</b>Arranging the sheet view by view (each view in its own box, sized by you), running dimensions, more view-selection rules.</li></ul>

## Try it on your part

Send a STEP file and what the drawing must show: [info@roshera.tech](mailto:info@roshera.tech?subject=Roshera%20drawing%20pilot).
