---
title: Kernel
description: A native B-Rep geometry kernel in Rust that certifies its results, refuses what it cannot verify, and records every change as history.
---

<div class="facts"><div><b>Native</b><span>B-Rep written from scratch in Rust, not a wrapper</span></div><div><b>Certified</b><span>every mutating operation returns its proof</span></div><div><b>Event-sourced</b><span>the model is its history: replay, undo, branch</span></div></div>

## The problem

Geometry kernels were built for a person who checks results by eye. When a kernel cannot do an operation exactly, it often returns an approximation anyway, and nothing in the result says so. An agent, or a busy engineer, builds on top of it.

## What Roshera does

**Models solids.** Extrude, revolve, sweep, loft, shell, fillet, chamfer and robust booleans on exact geometric predicates; a 2D sketch solver with degree-of-freedom accounting; its own hidden-line removal, tessellation and section cutting; STEP import and export, STL, OBJ and a lossless native format.

**Proves its results.** A solid is reported sound only when it is closed, manifold, outward-facing and free of self-intersections, with the Euler characteristic, genus and the checks that prove it. GD&T frames are evaluated on the real faces and report in or out of spec with the measured value.

**Refuses instead of guessing.** An operation outside what the kernel can verify returns a typed refusal with the reason, and the model is left unchanged.

**Keeps history as the model.** Every change is an event. Documents reopen by replaying their events; any step can be undone, branched or compared. Faces and edges keep persistent identities across rebuilds, so a tolerance or a drawing dimension stays attached to its feature, or is reported dangling when the feature is gone.

## Proof

- [The part that vanished on reopen](/blog/the-part-that-vanished-on-reopen): replay refused a misordered history instead of rebuilding a wrong part.
- Every fix lands with a test that failed before it, and a deliberate mutation that the test catches.

## Where it stands

<ul class="status"><li data-s="today"><b>Today</b>Modelling, booleans, blends, sketches, history, persistent identity, STEP exchange, certificates.</li><li data-s="building"><b>Building</b>Speed at assembly scale, general curved-surface intersections, meshing and simulation on the certified solid.</li></ul>

## Talk to us

Kernel questions, benchmarks, or a part that breaks other tools: [info@roshera.tech](mailto:info@roshera.tech).
