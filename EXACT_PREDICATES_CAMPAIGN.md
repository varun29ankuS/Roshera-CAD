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

## ★ Research synthesis (2026-06-21 — OCCT/CGAL/Shewchuk/Attene, sourced)

**Biggest finding:** `operations/boolean.rs` calls **NO exact predicate** — the
split-face/weld path is purely `Tolerance` + `add_or_find(x,y,z,tol)` + `f64`
cross products (OCCT-style epsilon geometry). The 3 `incircle` mentions there are
comments, not calls. So the corefinement weld has *no* robust sign decision — this
is the structural root of the unshared-edge failures (#17/#35). The *only* real
consumer of `orient2d` today is `math/circumcircle.rs` (Delaunay short-circuit).

**Comparative landscape:**
- **OCCT** = tolerance-based, NOT exact: `BRepAlgoAPI_BOP`→`BOPAlgo_BOP`→
  `BOPAlgo_PaveFiller`→`IntTools_FaceFace`; `Precision::Confusion()≈1e-7`; robustness
  via *valid-tolerance inflation* + Fuzzy Boolean (`SetFuzzyValue`). Robust in
  practice, no exactness guarantee.
- **Parasolid/ACIS** = tolerant modeling (per-entity tolerance attributes,
  auto-propagated) + interval arithmetic for *consistency* over exactness. Neither
  computes a true determinant sign.
- **CGAL/Triangle/Shewchuk** = exact. Roshera's "can't lie" cert + exact predicates
  is a *stronger* model than either commercial kernel — a real differentiator.

**Exact Stage-A/B/C constants** (derive from `ε = 2^-53`, the canonical values):
`ccwA=(3+16ε)ε`, `ccwB=(2+12ε)ε`, `ccwC=(9+64ε)ε²`; `o3dA=(7+56ε)ε`,
`o3dB=(3+28ε)ε`, `o3dC=(26+288ε)ε²`; `iccA=(10+96ε)ε`, `iccB=(4+48ε)ε`,
`iccC=(44+576ε)ε²`; `ispA=(16+224ε)ε`, `ispB=(5+72ε)ε`, `ispC=(71+1408ε)ε²`.
The file's `CCWERRBOUNDSA` matches ccwA, but `ICCERRBOUNDSA=1e-15`/`ISPERRBOUNDSA=1.6e-15`
are hand-tuned + TOO LOOSE (true iccA≈1.066e-15, ispA≈1.776e-15) — sign-error risk.

**Missing primitives** (the exact/non-exact dividing line): `fast_two_sum`,
`two_diff`, `two_one_sum`, `two_two_diff`, **`fast_expansion_sum_zeroelim`**,
**`scale_expansion_zeroelim`**, **`estimate`**. `two_sum`/`two_product`/`split` are
already correct. Expansion sizes are bounded (orient2d≤16, orient3d≤192,
incircle≤1152) → stack arrays.

**Rust crates** (no nalgebra dep, all MIT/Apache): `robust` (georust, best dep
option), `geometry-predicates` (no_std, best vendoring reference), `spade` internals.
Per the no-nalgebra/own-math rule: VENDOR a clean Shewchuk in `exact_predicates.rs`,
cross-check against those two.

**Phased path (dependency- & risk-ordered):**
- **Phase 1 (low risk, self-contained):** add the missing expansion primitives
  (+ unit tests) → replace hand-tuned constants with `ε`-derived A/B/C → implement
  true Stage B→C→D (exact) for orient2d, orient3d, incircle, insphere → a
  **degeneracy harness with a bignum/i128 oracle** (the proof of exactness; the
  current code has NO exact-tier-vs-oracle test). Cannot regress callers.
- **Phase 2 (medium):** route tessellation/CDT incircle-flip + `circumcircle.rs`
  fully through exact predicates (kills a `curved_cdt` nondeterminism source);
  then replace `f64` side/orientation decisions in `boolean.rs` split-face
  classification with `orient2d`/`orient3d` (keep tolerance weld for now). Guard
  with poke_matrix (33/33, deterministic) + HARNESS-1000.
- **Phase 3 (high value, high risk — the #17/#35 cure):** **indirect predicates**
  (Attene 2020 / Cherchi 2020-22, clean-room from papers — the C++ is LGPL, do NOT
  copy): `ImplicitPoint{Explicit,LPI,TPI}` with cached λ/d homogeneous coords at
  FP/Interval/Exact tiers; produce boolean intersection vertices as LPI/TPI so two
  operands referencing the same defining planes yield the SAME point → shared
  edges → exact watertight weld, eliminating `edge_remaps=0`.

Sources: Shewchuk 1997 (people.eecs.berkeley.edu/~jrs/papers/robustr.pdf);
predicates.c (cs.cmu.edu/~quake/robust.html); OCCT Boolean docs; Attene arXiv
2105.09772 + Cherchi mesh-arrangements/booleans papers; crates `robust` /
`geometry-predicates`. Full report in the research agent transcript.

## Discipline
Production-grade, compile-safe slices, each predicate harness-gated (the harness
is the proof of exactness), `cargo fmt` clean, commit each verified increment.
The harness IS the verification: a predicate that can't be shown to give the right
sign on a degenerate input is not exact. This is the boolean-robustness moat.
