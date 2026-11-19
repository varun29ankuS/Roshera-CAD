---
title: When a seam is not an edge
description: A profile sampled into 160 facets drew 160 lines across the part. Here is the rule that keeps a faceted surface's outline and drops its seams.
date: 2026-11-19
---

The wing rib's front view came out striped. The rib is a 161-point aerofoil extruded 6 mm, so its side is 160 flat facets, a few degrees apart. Each seam between two facets is a real topological edge, and the drawing faithfully drew every one: 160 lines across a 6 mm thickness.

A drafter draws the rib's outline. The seams are not edges of the part; they are an artefact of how the profile was sampled.

## The rule

A seam is left out of the drawing when all of these hold:

- both faces beside it are flat,
- they turn by less than 20° from one another,
- and they belong to a run of at least four such facets.

The last condition matters. A single shallow bend, a plate with a 10° kink halfway along, is a real edge a machinist needs to see, and it stays. Only a surface made of many small facets loses its seams.

## The silhouette stays

There is one exception. Where a facet seam is the **silhouette** in the view, where the surface turns from facing the viewer to facing away, it is the outline, and it is drawn. Seen end-on, the top and bottom of the aerofoil are silhouettes of the faceted surface, so they stay; seen from the side, the leading and trailing edges are the outline.

## The numbers

On a test lens of 80 facets, the front and side views together drew 160 lines across the thickness before the rule and none along the chord after it, while both tips and both silhouettes stayed. Switching the rule off brings the stripes back and fails the test, so the test proves the rule does the work.

It is a small change with a large effect on whether a drawing reads at a glance, which is the only way a drawing is ever read.
