---
title: For AI labs
description: A geometry environment where every action returns a machine-checkable certificate: verifiable rewards, honest evals and real engineering tasks for models that design physical things.
---

<div class="facts"><div><b>0</b><span>LLM judges in our evals: the kernel grades</span></div><div><b>23</b><span>certificate-scored engineering scenarios</span></div><div><b>100+</b><span>geometry, drawing and verification tools over MCP</span></div></div>

## The problem

Models are starting to design physical parts. Grading that work is the bottleneck. A rendered part that *looks* right can be non-manifold, self-intersecting or a millimetre off where it matters, and a vision model or a human glancing at a screenshot will not notice. Reward signals and evals built on judgement do not scale, and they reward plausible over correct.

## What Roshera gives a lab

**A verifier, not a judge.** Roshera is a native B-Rep kernel that certifies its own results. Every build returns a soundness certificate (closed, manifold, outward, no self-intersections, with the measurements); every drawing a sheet certificate; every tolerance a measured in-spec or out-of-spec verdict. These are facts about the geometry, computed exactly, so they can be used directly as rewards and as eval scores.

**Honesty as a measurable property.** When an operation falls outside what the kernel can verify, it refuses with a typed reason instead of approximating. That lets an eval score not just *did the model build it* but *did the model claim something the geometry does not support*.

**Real engineering tasks.** Gears, nozzles, flanges to their standards, brackets with GD&T, aerofoil ribs from a NACA designation, production drawings from STEP files: tasks with exact answers an engineer would recognise.

**One interface for people and models.** The same commands drive the CAD app and the agent, over REST and the Model Context Protocol, with a recorded history every episode can replay, undo and branch.

## Proof

| | |
|---|---|
| **AGENT-EVAL** | 23 scenarios scored on correctness (exact analytic oracles, e.g. shoelace area cross-checked against the kernel's volume integral), soundness, honesty and performance. Nothing graded by a language model. |
| **Episode runner** | Agent-driven, certificate-scored episodes: a fresh document per episode, a policy acting through MCP tools, claims verified against kernel ground truth at the end, a trajectory file a trainer can consume, failures reported as named outcomes. |
| **Caught in the act** | In a recorded session an agent entered three hole positions with the wrong sign. The kernel measured all three out of spec by 74 to 144 mm, and the agent corrected them before release. |

## Where it stands

<ul class="status"><li data-s="today"><b>Today</b>Certified modelling, drawings, GD&T verification, STEP import and export, MCP and REST, certificate-scored eval scenarios.</li><li data-s="today"><b>Today</b>Episode runner producing certificate-scored trajectories (early: tested end to end at small scale).</li><li data-s="building"><b>Building</b>Larger task corpora (part families with held-out sets), meshing and simulation with results traced to faces, so physics joins the reward.</li></ul>

## Work with us

We work with labs and agent teams on:

- **Evals**: certificate-graded engineering benchmarks, including held-out sets.
- **Environments**: the kernel as an RL environment with verifiable rewards.
- **Tool access**: Roshera as the geometry tool behind your agent.

Write to [info@roshera.tech](mailto:info@roshera.tech?subject=Roshera%20for%20AI%20labs) with what your models need to build or be graded on.
