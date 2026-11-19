---
title: Fields, meshing and simulation
description: Exact signed-distance fields on the true B-Rep surface today; meshes and simulation results traced to the faces that produced them, feeding back into the design, next.
---

<div class="facts"><div><b>Exact</b><span>distance from the analytic surface, not a mesh</span></div><div><b>Adaptive</b><span>cost grows with surface area, not volume</span></div><div><b>Traced</b><span>every value knows the face it came from</span></div></div>

## The problem

Simulation today starts by throwing the design away: the model is exported, meshed in another tool, solved in a third, and the result comes back as a picture. Nothing links a hot spot to the face that caused it, so turning a result into a better design is manual, slow and easy to get wrong.

## What Roshera does today: exact fields

Roshera computes the **signed distance** from any point to a solid, negative inside the material and positive outside, from the B-Rep's analytic surfaces. The sign comes from the surface normal at the nearest point, correct for every surface type, tori and NURBS included. Fields over a region are sampled with an adaptive octree that refines only where the surface can pass, and every sample records the face that gave its distance.

Used for wall-thickness and clearance checks tied to the exact part, for comparing two versions of a part (where material moved), and as the base for lattices and variable-thickness shells.

## What we are building: the closed loop

1. **Mesh the certified part**, with every element traced to its face and the mesh quality certified, like everything else in the kernel.
2. **Simulate it**: stress, heat, flow, on the real geometry.
3. **Feed back**: turn the result into a change the kernel can make and check, such as moving material, reshaping a hole or adding a fillet, and record it in the part's history like any other edit.

The animation on the [home page](/#loop) shows the loop on a textbook case with real physics: a plate in tension with a hole has a peak stress three times nominal at the hole's edge (Kirsch); a slot aligned with the load lowers it to 1 + 2b/a (Inglis).

## Where it stands

<ul class="status"><li data-s="today"><b>Today</b>Exact signed-distance fields, adaptive sampling, face provenance, mass properties.</li><li data-s="building"><b>Building</b>Certified meshing, simulation, results feeding design changes back into the timeline.</li></ul>

## Talk to us

If simulation-driven design is your problem, or you want to train models on it: [info@roshera.tech](mailto:info@roshera.tech).
