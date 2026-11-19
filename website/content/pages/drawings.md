---
title: Drawings
description: Production drawings generated from the part, laid out on the right ISO sheet, and certified against the geometry before release.
---

## From the part to the sheet

Give Roshera a part, built in Roshera or imported from STEP, and it makes the manufacturing drawing:

- **Views** in first- or third-angle projection, with hidden-line removal, true circles and ellipses, and a shaded isometric.
- **Sections** through bores and walls, hatched, with the cutting plane drawn on the source view.
- **Dimensions** measured from the exact geometry, each stated once in the view where it reads true, spaced to ISO 129.
- **Hole tables** with tags on the view, and counted callouts such as `3× Ø13 H7`.
- **Fits** from the ISO 286 tables: type H7 and the sheet prints `Ø40 H7 (+0.025/0)`.
- **GD&T**: datum symbols and feature control frames, each verified on the real faces.
- **Profile ordinates** for freeform outlines such as aerofoils: upper and lower surface at standard stations along the chord, with the maximum thickness and the trailing-edge thickness.
- **An ISO 7200 title block** with material, general tolerance, projection, approval and a certificate stamp.

The sheet size and scale are chosen by search: Roshera tries the standard ISO sheets and scales and keeps the one where everything fits and reads, preferring full size.

## The sheet checks itself

Every drawing carries a **sheet certificate**. It re-measures each printed value against the live solid and classifies it:

| Verdict | Meaning |
|---|---|
| consistent | the value on the sheet is what the part measures |
| stale | the part changed; the sheet is out of date |
| dangling | the feature the value refers to no longer exists |
| omitted | the part has a feature the sheet does not show |

A sheet is **sound** when nothing is stale, dangling or omitted, and **complete** when every model dimension is stated or signed off by a named person. Export as a release drawing is refused until the sheet is sound.

## Edit on the sheet

Click any dimension:

- **Set a tolerance**: ± value, limits, an ISO 286 fit, basic or reference.
- **Add text**: `4X`, `TYP`, thread callouts.
- **Move it**: drag it along its line; the sheet re-checks the layout.
- **Change the part**: on a part built in Roshera, type a new size and the part rebuilds from its history, then the drawing re-certifies.

Every edit is one recorded step you can undo.

## Export

- **PDF** for review and the shop floor.
- **DXF** for other CAD tools: millimetres, true arcs, real dimension entities, views as blocks, layer colours.
- **SVG** for the web.
