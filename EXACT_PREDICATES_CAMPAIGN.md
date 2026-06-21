# Exact Predicates — Parasolid/OCCT/CGAL-class robustness

> Varun (2026-06-21): "improve the exact predicates … get in class of Parasolid /
> OCCT … read their code for reference." This is the geometric-robustness
> substrate — one of the three agreed strategic moves ([[zoo-kcl-and-3-strategic-moves]]),
> and the "fix the physics first" foundation under the boolean weld (#35/#17).

## The honest current state (assessed 2026-06-21)
`geometry-engine/src/math/exact_predicates.rs` is **semi-robust, NOT exact**,
despite a doc line claiming *"exact for all inputs"* (an overclaim to fix):
- It has the **first-stage A-filter** only (`CCWERRBOUNDSA`, `O3DERRBOUNDSA`, …)
  — the file's own comment admits "multi-stage refinement (B and C bounds) is not
  implemented."
- The `orient2d_adapt` fallback computes ONE product-expansion via
  `two_product`/`two_sum` but then **drops the coordinate-difference tails**
  (`acx = pa.x - pc.x` is not a `two_diff`) and **sums the remaining tails in
  ordinary float** (`detleft_tail - detright_tail + det_tail`). That captures more
  precision than the naive cross product but is **not** a true exact determinant.
- So near-degenerate inputs (collinear-but-perturbed, cospherical) can still flip
  sign — the topological-inconsistency risk that breaks boolean welds.

Predicates are used in: `operations/boolean.rs`, `operations/fillet.rs`,
`tessellation/curved_cdt.rs`, `tessellation/surface.rs`, `math/circumcircle.rs`.

## The target (CGAL / Triangle / Shewchuk class)
True **adaptive-precision exact** orient2d / orient3d / incircle / insphere
(Shewchuk 1997): A-filter → B-refine → C-refine → **exact expansion arithmetic**
(`fast_expansion_sum`, `scale_expansion`, `estimate`) with `two_diff` on the
coordinate differences. Guaranteed-correct sign for ALL inputs, ~no slowdown on
the >99% non-degenerate fast path. (Research agent is fetching the exact
predicates.c staging + the OCCT tolerance-model contrast + the Attene/Cherchi
**indirect-predicates** path for constructed intersection points.)

## Upgrade path (dependency-ordered, harness-gated — for the loop)
1. **Exact-arithmetic primitives**: verify/complete `two_sum`, `two_diff`,
   `two_product`, `fast_expansion_sum_zeroelim`, `scale_expansion_zeroelim`,
   `estimate` (the expansion toolkit). Unit-test each against known expansions.
2. **A degeneracy HARNESS first** (gate before changing the predicates): construct
   inputs where the naive float predicate flips sign but the true sign is known —
   nearly-collinear triples, points on a line at irrational-ish offsets,
   cospherical quintuples — and assert the predicate returns the correct sign.
   This will FAIL on the current semi-robust code → proving the gap, then drive
   the fix (never pin).
3. **Port full Shewchuk `orient2d`** (the real multi-stage `orient2dadapt`) →
   harness green. Then **`orient3d`**, **`incircle`**, **`insphere`**.
4. **Fix the overclaiming doc** + keep the fast-path filter (perf).
5. **Wire into the boolean split-face/weld path** (where constructed intersection
   points need robust orientation) — the #35/#17 corefinement robustness fix.
6. **Indirect predicates** (Attene/Cherchi) for constructed points (LPI/TPI) so
   predicates on intersection points stay exact without coordinate blow-up — the
   modern boolean-weld cure.

## Discipline
Production-grade, compile-safe slices, each predicate harness-gated (the harness
is the proof of exactness), `cargo fmt` clean, commit each verified increment.
The harness IS the verification: a predicate that can't be shown to give the right
sign on a degenerate input is not exact. This is the boolean-robustness moat.
