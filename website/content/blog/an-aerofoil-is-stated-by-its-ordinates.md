---
title: An aerofoil is stated by its ordinates
description: A chord and a depth do not let a machinist make an aerofoil. Roshera now measures the profile's ordinates from the solid and puts them on the sheet, certified.
date: 2026-11-19
---

We drew a wing rib: a NACA 2412 section, 300 mm chord, 6 mm thick, with two spar bores and a lightening hole. The sheet had the chord, the thickness and the bores, located and toleranced. It was missing the most important thing: the shape.

A machinist cannot make an aerofoil from a chord and a maximum depth. Aerofoil drawings state the profile as a **table of ordinates**: the height of the upper and lower surface above the chord line at standard stations along the chord, 0, 1.25, 2.5, 5, 7.5, 10, 15 … 90, 95 and 100 % of chord, with the chord, the maximum thickness and where it occurs, and the trailing-edge thickness.

## Measured, not copied

Roshera does not copy those numbers from the NACA formula. It measures them from the part:

1. **Find the outline.** A planar face whose boundary is a long run of short, gently turning straight edges (a profile sampled into a polygon, as a generated aerofoil is) or mostly spline edges (as in an imported model) is a freeform outline.
2. **Find the chord.** The two points furthest apart are the leading and trailing edge. The leading edge is the rounded end, the trailing edge the thin one, so the outline itself says which is which. The chord runs to the middle of an open trailing edge.
3. **Cut at each station.** At each station along the chord, the outline's own edges are intersected with a line across it; the highest and lowest crossings are the upper and lower ordinates.

On a NACA 2412 test rib every ordinate matches the NACA formula within 0.15 mm, the faceting of a 60-point profile. The maximum thickness comes out at 12 % of chord near 30 %, as the designation says.

## Certified like everything else

The table is a set of claims, so the sheet certificate re-measures the chord and every ordinate on the live solid. Change the part and the table goes stale; the sheet cannot be released until it is regenerated. A deliberately wrong ordinate, 0.5 mm too full at 30 % chord, makes the sheet unsound in the test suite.

The same mechanism covers any freeform outline: cams, curved brackets, guide profiles. Wherever a shape cannot be stated by a few dimensions, its ordinates can be.
