---
title: Fields
description: Exact signed-distance fields computed on the true B-Rep surface, refined adaptively where the surface is, and traced back to the face that produced each value.
---

## Distance to the real surface

Roshera computes the **signed distance** from any point to a solid: negative inside the material, positive outside. The distance comes from the analytic surfaces of the B-Rep, not from a mesh, and the sign is decided from the surface normal at the nearest point, which is correct for every surface type, including tori and NURBS.

## Adaptive, not voxels

A field over a region is sampled with an adaptive octree that refines only the cells the surface can pass through. Cost grows with surface area, not volume, and resolution is chosen by the question asked rather than fixed by a grid.

## Every value has a face

Each sample records which face gave its nearest distance. A thin-wall or clearance finding therefore points at real geometry that can be selected, measured and changed.

## What it is for

- **Wall thickness and clearance** checks tied to the exact part.
- **Comparing versions**: the difference between two fields shows where material moved between two points in a part's history.
- **Field-driven design**: the base for lattices, variable-thickness shells and smooth blends that stay attached to the precise solid they came from.
