---
title: Kernel
description: A native B-Rep geometry kernel in Rust that certifies its results, refuses what it cannot verify, and records every change.
---

## Native, written from scratch

Roshera's kernel is a boundary-representation (B-Rep) solid modeller written in Rust. It is not a wrapper around another kernel. It includes:

- exact geometric predicates for robust booleans,
- extrude, revolve, sweep, loft, shell, fillet and chamfer,
- a 2D sketch solver with degree-of-freedom accounting,
- its own hidden-line removal, tessellation and section cutting,
- STEP import and export, plus STL, OBJ and its own lossless format.

## It checks its own work

Mutating operations return a **certificate**, not just a result. A solid is reported sound only when it is closed, manifold, outward-facing and free of self-intersections, with the measurements that prove it. Drawings carry a sheet certificate that re-measures every printed value. GD&T frames are evaluated on the real faces and report in spec or out of spec, with the measured value.

## It refuses instead of guessing

When an operation falls outside what the kernel can verify, it returns a typed refusal with the reason. No approximation is labelled as exact. An agent or a person is told what was refused and why, and the model is left unchanged.

## History is the model

Every change is an event on a timeline. Parts are rebuilt by replaying their history, so a document reopens exactly as it was, and any step can be undone, branched or compared. A dimension edited on a drawing changes the recorded parameter that drives it, and the part rebuilds from there.

## Persistent identity

Faces, edges and features keep stable identities across rebuilds. A tolerance, a datum or a drawing dimension stays attached to the same feature after the part changes, and is reported dangling if that feature disappears.
