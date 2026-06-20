# Agent-Geometry Loop — Comprehensive Campaign

> Mandate (expanded 2026-06-20): drive the kernel to be a true **agent-runtime for
> geometry** — an agent can SEE geometry, BUILD it layer-by-layer, and the kernel
> CAN'T LIE about whether it's sound OR *what it is*. The `/loop` drives every
> track below in dependency order, fills every gap it finds (never pins), and
> commits each verified, compile-safe increment.

This sits above the sketch-conflict work in `SKETCHER_OVERHAUL_CAMPAIGN.md`; that
campaign is Track 1.

---

## Track 1 — Sketch verification  (ACTIVE — the proven groove)
The sketch-validity certificate + the adversarial harness that finds & fills gaps.
- DONE: `SketchValidityCertificate` (`sketch2d/sketch_certificate.rs`),
  `harness::sketch_validity` oracle, `tests/sketch_certificate_gate.rs` (27 cases).
- Gaps filled by the loop so far: closed-polyline self-intersection, derived-line
  phantom DOF, mixed Coincident+Distance, Parallel/Perp+Angle, Collinear+Perp,
  transitive coincidence vs Distance, transitive coincidence vs Coordinate.
- NEXT: witness-configuration cases, drag-stability, a **solver-correctness sibling
  harness** (a well-constrained sketch solves to the right geometry), then the
  backend slices B2 (persist sketches in `BRepModel` + event-source via
  `OperationRecorder` + reverse-link `sketch_source` on Solid), B1 (unify the live
  api-server path onto the real constrained `Sketch` via `csketch.rs`), B4
  (constrained-editor REST + surface `certify()` to the agent).

## Track 2 — The agent EYE (perception)  ★ next foundation
The precondition for everything semantic: the agent must be able to *see*.
- **Sketch-render**: rasterize a 2D sketch → PNG, exposed via REST + MCP. Today
  only *solids* render (`render_part`/`scene_view`); a raw sketch is invisible to
  the agent. Build it so a VLM (or I) can look at a sketch and judge it.
- **Face-ID renders (set-of-marks)**: `render_part mode='ids'` paints each B-Rep
  face a distinct colour + returns a colour→face_id legend. Verify/harden it as
  the reliable substrate for face selection.

## Track 3 — Semantic IDENTITY (the recognition moat)  "is a gear a gear?"
Soundness ≠ identity. A gear and a house pass the certificate identically.
- **Analytic feature-recognition** (certifiable, kernel-native, like watertight):
  N equally-spaced radial teeth about a centre → gear; hole patterns (through/
  blind, count, pitch); symmetry; roof-over-rectangle → house; plate + mounting
  holes + web → bracket. Each detector ships with a harness: *build* the feature,
  assert it's *recognised*.
- **VLM recognition** (render → see → recognise) as the complementary judgement
  layer — fast, broad, but a model verdict, not a proof.

## Track 4 — Claimed vs recognised (the LABELLER)
Wire Track 3 to the labeller: a part claims label `spur-gear-20T`; the kernel
recognises the actual geometry; it flags a mismatch (17 teeth ≠ 20). Catches
*"you asked for X, this is Y"* — which no certificate or CAD kernel checks today.

## Track 5 — Layer-by-layer BUILDING (the agent build loop)
The feature-on-face recipe, made reliable for an agent.
- **The recipe**: `render(ids)` → pick face → `plane_from_face` →
  sketch-on-face → `extrude`/`extrude_cut` → **certify** → repeat. All primitives
  exist as MCP tools today.
- **A skill/workflow** encoding the recipe so the agent *follows* it (perceive →
  sketch-on-face → extrude → verify → repeat) instead of rediscovering it.
- **Certificate as the gate**: certify soundness after *each* feature
  (perception-as-default) so a bad layer can't silently compound.
- **Persistent face IDs (#11)**: a face reference must survive the *next* edit —
  the parametric-robustness piece, and the main thing to harden (without it,
  "the face I sketched on" can renumber after a later boolean).
- **Harness**: a "build-a-bracket-in-5-features, certify each, then edit feature 2
  and re-verify the rest still resolve" gate.

## Track 6 — Blackboard verification
Select a blackboard claim → verify against kernel ground truth: numerical re-eval
of the formula AND geometric measurement on the certified part (throat radius,
area ratio, volume). The "notebook that can't lie." (Pairs with Track 4 — the
agent's *reasoning* checked against the geometry, the agent's *labels* checked
against recognition.)

---

## Sequencing the loop picks from
1. Keep Track 1 green & escalating (the groove that's finding real bugs).
2. **Track 2 sketch-render** (foundation for Tracks 3-4) — so the agent can see.
3. Track 5 persistent-face-IDs + the build-loop skill (unblocks reliable
   layer-by-layer building).
4. Track 3 analytic recognition (one detector at a time, each harness-gated).
5. Track 4 labeller wiring; Track 6 blackboard verify.

Discipline (all tracks): production-grade only (no stubs/todos), compile-safe
slices, verify each compiles+passes before the next, commit each increment with a
clear message (no AI-authorship trailers), `cargo fmt` clean, never run
`cargo build/run` beyond `check`/`test`, slow-is-smooth.
