# Roshera Verification Fort — Blueprint
*2026-06-25. Synthesis of three coverage audits (arsenal / gaps / spatial-loop) + the parametric-sketch and ambient-blackboard pillars, into one wall-by-wall build order.*

## The governing law
**The certificate is only as strong as its invariant SET.** Every defect that escapes becomes a new permanent invariant. We now have an external witness — **OpenCascade** (cadquery/OCP, the kernel FreeCAD uses) — to tell us when the set is incomplete, driveable from the shell. The fort is not "more checks"; it is a *complete, enumerated, audited* set with no unguarded path out, plus the perception to catch errors of *placement* the cert is silent on.

## The five walls
1. **Internal soundness** — the `ValidityCertificate` (have it; harden it).
2. **Cross-kernel portability** — export → OpenCascade re-import → valid (first brick laid 2026-06-25: pcurve fix + OCC gate, commit `fa9f6b3`).
3. **Spatial-relational perception** — the agent's eyes: clearance / interference / placement, so errors self-announce (the impeller hub-float class).
4. **Legibility** — the ambient blackboard: every part carries its proof *and* its reasoning.
5. **The parametric sketch-spine** — see / edit / regenerate the 2D intent behind every 3D part. The workhorse-maker.

---

## Build order

### TIER 0 — Free wins: guards we already wrote but never turned on
*Pure wiring of existing code. Highest ROI in the whole plan.*
- **0.1 — Get `oriented` back into the cert.** `manifold_report` computes `inconsistent_directed_edges`/`oriented` but `compute_certificate` drops it and `is_sound()` never checks orientation → **a flipped-normal solid (the nurbs_loft B2a class we hit) certifies `sound`.** Extract + gate. `provenance.rs`, `topology_builder.rs:2491`.
- **0.2 — Wire `validate_pcurve_references` into `validate_model`.** Written, referenced only by its own tests. `validation.rs:1382`.
- **0.3 — Re-enable `check_face_orientations`** (dead code) — B-Rep face-orientation consistency across shared edges. `validation.rs:1561`.
- **0.4 — Expose REST-only capabilities as MCP tools:** `part_distance`, `part_features`, `part_coverage`, GD&T face/edge verify. Trivial wrappers; immediately widens what the agent can ask.
- **0.5 — Expose the kernel's spatial primitives as agent tools:** `point_query` (signed_distance + inside/outside + nearest face), `ray_query` (raycast hits/depths), `region_query`. They EXIST and are SDF-verified (`queries/field.rs`, `queries/raycast.rs`) but are completely unreachable by the agent today.

### TIER 1 — High ROI: the spatial loop + the interop cert dimension
- **1.1 — `parts_clearance(a,b)`** → surface-to-surface min distance + `{contact | gap=d | interfere=−d}` (sample B's boundary against A's `signed_distance` field). **Fold it into the ambient perception block** so adjacency errors self-announce ("nearest neighbour part 3: gap 0.4 mm"). *Single biggest reduction in "the human had to catch it"* — it directly closes the impeller hub-float class.
- **1.2 — `interop_valid` cert dimension.** Promote the OCC round-trip gate (`tests/step_occt_pcurve_gate.rs`, `tools/step_occt_validate.py`) from an export test to a certificate dimension: export → OCC re-import → BRepCheck-valid → topology fingerprint matches. Then `sound` ⟹ *travels*.
- **1.3 — Promote `brep_integrity` oracles to the runtime cert:** coincident edges, duplicate vertices, loop closure, pinched/non-manifold vertex. Currently test-only (`harness/brep_integrity.rs`).

### TIER 2 — The parametric sketch-spine + legibility
- **2.1 — See the 2D sketch of a 3D part.** Frontend draws the generating profile on its plane when a part is selected; agent gets a 2D-sketch render. (`get_revolve_profile` already recovers the data.)
- **2.2 — Generalize the retained-profile↔part link** so extrude/loft retain their sketch like revolve does.
- **2.3 — Seamless edit→regenerate** — editing the sketch re-evaluates the dependent part through the timeline (the parametric-on-timeline hybrid).
- **2.4 — Sketch-validity certificate as a build precondition** — verification *starts* at the 2D source (closed, non-self-intersecting, fully constrained). `sketch2d/sketch_certificate.rs` exists; wire it as the gate.
- **2.5 — Ambient blackboard, system half** — build ops auto-post parameters + provenance to the part's blackboard. (Agent half — logging math/assumptions/decisions — adopted as a standing default.)

### TIER 3 — Deeper invariants (real math, lower frequency)
- **3.1 — Finer self-intersection in the cert.** Cert runs at coarse 0.5 chord and cannot see coplanar overlap; run Deep-level (or refine) within a face-count budget. (#24)
- **3.2 — Curved edge-off-surface** — promote the coarse 12×12 (u,v)-grid *warning* to a blocking, pcurve-based/dense check on NURBS/ruled/revolution faces (analytic-only today). `validation.rs:574`.
- **3.3 — Knot re-validation post-op** — re-check every reachable `KnotVector` (monotonicity, end-multiplicity = degree+1, count) after operations, not only at construction.
- **3.4 — SameParameter / SameRange** — 3D edge vs 2D pcurve parameter agreement (OCC's most common reject; needs pcurves — now present).
- **3.5 — Built-vs-intended placement assertion** (generalize `verify_claim` to anchor/axis/centroid vs intended) and **built-feature == recognized-feature** invariant (depth-ladder rung 3).
- **3.6 — Remaining classes:** G1/G2 tangency where an op promised it; thin-wall / narrow-region; invalid 2D trim loop; zero-length edge; wire/shell imbrication; per-entity tolerance sanity.

---

## Cross-cutting principles
- **Thread tolerances** from the caller — the cert hardcodes chord 0.1 / 0.5, weld 1e-6, 40° normal dev. Single-chord checks are blind below facet scale.
- **Every closed gap gets a non-vacuous gate test** — and where coverage is bounded (top-N, coarse chord, skip-list), `log()` what was dropped. No silent caps.
- **OpenCascade is the oracle** for Walls 1–2: any new invariant is validated against real OCC via the `.step_verify` harness.

## Status (2026-06-25)
- Wall 2 first brick: **DONE** (pcurve + OCC gate, `fa9f6b3`).
- Agent-half of Wall 4 (legibility): **adopted** as a standing default.
- Everything else: **queued in this order.** Recommended start: **Tier 0** — it's all wiring of code that already exists, and 0.1 closes a live hole (flipped solids passing `sound`).
