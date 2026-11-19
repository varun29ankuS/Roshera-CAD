---
title: Use cases
description: Production drawings from supplier STEP files, aerospace parts from their specification, and AI agents that need a geometry engine they can trust.
---

## Production drawings from supplier models

A supplier sends a STEP file; manufacturing needs a drawing. Roshera imports the file, checks that the solid is sound, and makes the drawing: views, sections, dimensions, fits from the ISO 286 tables, a hole table, datums and position tolerances. The engineer reviews it on the sheet, adjusts what needs adjusting and approves it. The sheet certificate guarantees the printed numbers match the model, and the export is refused if they ever stop matching.

## Aerospace parts from their specification

Describe the part: an aerofoil rib with its NACA section, chord, thickness, spar bores and lightening holes, material and process. Roshera generates the section from its designation, builds the part, certifies it, and draws it with datums, position tolerances and the profile's ordinate table. Values the specification does not give, such as the trailing-edge form or the fits, are asked for, not assumed.

## Suppliers of machined and fabricated parts

Brackets, plates, flanges, housings and shafts: Roshera's playbooks cover the common families, with standards-based sizes (flange tables, clearance holes, threads, ISO 2768 general tolerances) looked up from cited tables rather than from memory.

## Teams building AI agents for engineering

An agent that writes CAD needs a kernel that tells it when it is wrong. Roshera gives agents certified results, typed refusals and a recorded history they can undo and branch, over REST and the Model Context Protocol.

## Start a pilot

Send us a part you make today, the process and what your drawings must show: [info@roshera.tech](mailto:info@roshera.tech).
