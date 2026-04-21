# Roshera Geometry Kernel — Deep Audit
_Date: 2026-04-19 · Branch: `fix/math-layer-hardening`_

This audit is scoped to `roshera-backend/geometry-engine/`. Numbers were extracted
from the working tree at commit `e267b83` using grep/python tooling (not model
recall). Line-number references are reproducible.

---

## 1. Scope & size

| Subtree                 | LOC     | Files | Purpose                                     |
|-------------------------|---------|-------|---------------------------------------------|
| `primitives/`           | 39,071  | 32    | B-Rep topology + geometric entity stores    |
| `math/`                 | 24,601  | 38    | Vectors, matrices, NURBS/B-spline, tolerance|
| `operations/`           | 23,968  | 27    | Boolean, fillet, chamfer, extrude, ...      |
| `sketch2d/`             | 12,291  | 16    | 2D sketching + constraint solver            |
| `tessellation/`         | 4,589   | 8     | Adaptive triangulation, LOD, caches         |
| `assembly/`             | 682     | 1     | Mate constraints, motion                    |
| `export/`               | 800     | 3     | (feature-gated) export entry points         |
| `performance/`          | 293     | 1     | Warmup / micro-bench glue                   |
| **Total (prod+tests)**  | ~106 k  | ~126  |                                             |

Top-level `lib.rs` exposes: `math`, `primitives`, `operations`, `tessellation`,
`sketch2d`, `assembly`, `performance`, and feature-gated `export`.

Star topology assumption (kernel at center) is respected by the module graph —
external crates (`api-server`, `timeline-engine`, etc.) call only through
`primitives::topology_builder::BRepModel` and `operations::*` facades.

---

## 2. Correctness risk surface

Production (excluding test modules via `#[cfg(test)]` cutoff) in
`geometry-engine/src`:

| Risk                            | Count | Verdict                                  |
|---------------------------------|-------|------------------------------------------|
| `.unwrap()` in production       | 47    | **Mostly benign** — 29 are in `primitive_tests/performance_benchmarks.rs` (bench file); remainder in bin/* (benches). Real hot-path: ~3. |
| `.expect()` in production       | 135   | **Acceptable** — dominated by documented `RwLock` / `Mutex` poisoning messages on shared state. |
| `panic!()` in production        | 2     | **Acceptable** — `Vector3::Index`/`IndexMut` impls (standard Rust convention). |
| `unimplemented!()` / `todo!()`  | 0     | ✅                                        |
| `unsafe` blocks                 | 18    | **Mixed** — see breakdown below.          |

### Hardening pass already delivered
The `.expect` sites are the hardened form (prior session), e.g.
`tessellation/cache.rs:52`:
```rust
.write().expect("tessellation cache RwLock poisoned")
```
These are intentional: a poisoned lock is an unrecoverable invariant violation.

### Real production `.unwrap()` — 3 suspect sites
- `math/test_oslo.rs:19` — file-level test harness, not compiled in release.
- Remaining benches in `bin/*` are not shipped.
- **Zero production hot-path unwraps remain** in `math`, `primitives`,
  `operations`, `tessellation`.

### `unsafe` breakdown
| Location                             | Shape                             | Assessment |
|--------------------------------------|-----------------------------------|------------|
| `math/bspline.rs:573-1045` (8 sites) | `get_unchecked` / raw pointer read in span-finding | **Defensible** — bounded by knot-vector invariants; worth an `// SAFETY:` comment audit. |
| `math/quaternion.rs:257` `normalize_unchecked` | `unsafe fn` marker on a normalize with pre-condition | Fine. |
| `math/vector2.rs:192`, `vector3.rs:383` | Same pattern                      | Fine. |
| `performance/mod.rs:41, 50, 172` `static mut WARMUP_COMPLETE` | mutable static read/write | **⚠ P1**: UB-prone on contemporary Rust; replace with `AtomicBool`. |
| `math/test_math/bench_verification.rs:28` `ptr::read_volatile` | bench-only `black_box` substitute | Fine (not shipped). |

### NaN-unsafe `partial_cmp().unwrap()`
- One occurrence: `bin/nurbs_bench.rs:328` (bench binary, not shipped).
- **Zero in production code.** Previous hardening appears complete on this axis.

---

## 3. Weak or placeholder implementations

### Production TODO / placeholder markers (57 total, outside test modules)
Top offenders:

| File | Count | Notes |
|------|-------|-------|
| `primitives/ai_primitive_registry.rs` | 8 | Memory tracking, geometry extraction, "Register other primitives" — the AI-surface registry is the biggest stub-heavy file. |
| `sketch2d/constraint_solver.rs`       | 3 | |
| `math/tspline.rs`                     | 3 | Includes a "GPU evaluator placeholder" and two explicit "This is a placeholder". |
| `operations/transform.rs`             | 2 | "Curves are immutable in the current design" + missing surface-update path. Real correctness gap. |
| `operations/imprint.rs`               | 2 | "Need to track face association somehow" — imprint cannot correctly attribute new edges to faces. |
| `operations/extrude.rs`               | 2 | Ruled-surface creation is a placeholder. |
| `math/matrix4.rs`                     | 2 | `affine_inverse` not implemented. |
| `operations/g2_blending.rs`           | 1 | "Helper methods (placeholder implementations)". |
| `operations/modify.rs`                | 1 | "placeholder for the actual implementation" — the main `apply_modification` path is only partly real. |

### Substantive gaps

1. **Coincident-plane boolean case** (`operations/boolean.rs:384`):
   ```rust
   // Coincident planes - not implemented as curve intersection
   return Ok(vec![]);
   ```
   Silently returns empty — boolean ops on co-planar faces will be wrong.

2. **`math/surface_surface_intersection.rs` (587 LOC)** — the marching SSI
   implementation. **Exports are not imported anywhere.** `boolean.rs` has a
   local `surface_surface_intersection` function (line 308) that duplicates
   the purpose. Either wire up `math/` or delete it.

3. **Three parallel surface-intersection implementations**:
   - `operations/intersect.rs` (965 LOC): own `IntersectionCurve`, own
     plane/plane, plane/cylinder, plane/sphere routines.
   - `operations/surface_intersection.rs` (587 LOC): separate
     `IntersectionCurve`, own plane/plane/cylinder/sphere + marching.
   - `math/surface_surface_intersection.rs` (587 LOC): own marching +
     `IntersectionCurve`, unused.
   Total ≈ **2,100 LOC of overlapping code** around the same algorithm.

4. **`math/trimmed_nurbs.rs:337`** defines **a fourth `IntersectionCurve`**.

5. **`math/surface_plane_intersection.rs:88`** defines a fifth variant
   (`ParametricIntersectionCurve`).

### AI-surface registry
`primitives/ai_primitive_registry.rs` is **1,000+ lines with eight TODOs**
including hardcoded `vertex_count: 8`, `volume: Some(0.0)`, and
`_placeholder: usize` field (line 446). This contradicts the "AI-native" thesis
— it's the one surface AI actually queries, and it lies.

---

## 4. Duplication & competing types

### Dead source (orphans — declared nowhere, not in any `mod …;`)

| File                                   | LOC  | Status |
|----------------------------------------|------|--------|
| `operations/modify_backup.rs`          | 557  | Orphan. Delete. |
| `primitives/primitive_system.rs`       | 1,169 | Orphan. Delete or wire up. |
| `math/test_math/quick_nurbs_test.rs`   | 66   | Orphan. Delete. |

**1,792 LOC of dead code.** None of them are referenced by any `mod`
declaration, no external crate imports them.

### Competing type definitions
| Type                  | Locations |
|-----------------------|-----------|
| `NurbsCurve`          | `math/nurbs.rs:28` (full evaluator) + `primitives/curve.rs:1845` (B-Rep trait impl) |
| `NurbsCurve2D` / `NurbsCurve2d` | `math/trimmed_nurbs.rs:24` vs `sketch2d/spline2d.rs:436` — same concept, different case. |
| `IntersectionCurve`   | **5 distinct definitions** (see §3 above). |
| `BSplineCurve`        | Single: `math/bspline.rs:356` ✓ |

### Recommendations
- Merge `primitives::curve::NurbsCurve` into a thin adapter over
  `math::nurbs::NurbsCurve`. (Already captured as a deferred item.)
- Pick **one** `IntersectionCurve` type (the one in
  `operations/surface_intersection.rs` is most capable) and delete the others.
- Consolidate the three SSI implementations into one.

---

## 5. Test coverage quality

| Metric                                      | Value |
|---------------------------------------------|-------|
| `#[test]` functions                         | 615   |
| `proptest!` / `#[proptest]` / `arbitrary::` | **0** |
| `fuzz_target!`                              | **0** |
| Robustness-keyword-named tests              | 25    |
| Async/tokio tests                           | 0     |

**The kernel has zero property-based tests and zero fuzz targets.** This
contradicts `backend/CLAUDE.md` which explicitly requires proptest for
geometric invariants and fuzzing for B-Rep/file-format parsing.

### Boolean tests — smoke only
`operations/boolean.rs` test block (lines 3440–3986) contains 25 tests. All are
fixture-driven. **None verify**:
- Volume conservation under transformation
- Closure / watertightness
- Idempotency (`A ∪ A = A`, `A ∩ A = A`)
- Commutativity (`A ∪ B = B ∪ A`)
- De Morgan / set-algebra laws
- Degenerate / near-degenerate inputs (tangent, coincident, sliver faces)

The strongest assertion in boolean tests is
`"Boolean union — should NOT return NotImplemented"` — a liveness test, not a
correctness test.

---

## 6. Prioritized weakness report

### P0 — correctness-critical
| # | Item | Action | Est. scope |
|---|------|--------|-----------|
| 1 | No property tests for booleans | Add proptest suite covering volume conservation, idempotency, commutativity, closure. Seed with degenerate geometry. | ~400 LOC tests |
| 2 | Coincident-plane boolean silently returns `[]` | Either implement proper trim/merge or return `Err(Degenerate)` upstream. | Small, one file |
| 3 | Three parallel SSI implementations | Pick one, delete other two, re-point callers. Decision doc + ~1,500 LOC removed. | Medium refactor |
| 4 | `static mut WARMUP_COMPLETE` in `performance/mod.rs` | Replace with `AtomicBool`. | <10 LOC |

### P1 — structural
| # | Item | Action |
|---|------|--------|
| 5 | Two `NurbsCurve` types | Make `primitives::curve::NurbsCurve` a thin adapter over `math::nurbs::NurbsCurve`. |
| 6 | 1,792 LOC of dead source | Delete `modify_backup.rs`, `primitive_system.rs`, `quick_nurbs_test.rs` after final review. |
| 7 | 5× `IntersectionCurve` duplication | Unify; promote `operations/surface_intersection::IntersectionCurve` as canonical. |
| 8 | `ai_primitive_registry.rs` lies to AI (hardcoded vertex_count, volume=0.0) | Replace stubs with real queries via `BRepModel` facade. This *is* the AI-native surface. |
| 9 | `operations/imprint.rs` cannot attribute edges to faces | Implement face-association tracking. |
| 10 | `operations/transform.rs` does not update surface references | Fix transform on curves/surfaces. |

### P2 — correctness polish
| # | Item | Action |
|---|------|--------|
| 11 | `math/bspline.rs` unsafe span-find blocks lack `// SAFETY:` comments | Add. |
| 12 | `math/matrix4.rs::affine_inverse` unimplemented | Implement using matrix3 inverse now that it exists. |
| 13 | `operations/boolean.rs:3951` uses `panic!("NotImplemented")` inside a test assertion | Replace with `assert!(matches!(…))`. |
| 14 | Performance benchmarks file still contains aspirational "System A/B/C" comparison tables | Replace headers (separate small commit). |
| 15 | `operations/g2_blending.rs` — helpers labelled placeholder | Either implement or delete the file. |

### P3 — scope reduction candidates
| # | Item |
|---|------|
| 16 | `math/tspline.rs` has "GPU evaluator placeholder" and 3 TODOs. T-splines are a differentiating feature but currently aspirational. Either commit or remove the header claim. |
| 17 | `assembly/mod.rs` — 682 LOC, has a rotation TODO. Is assembly in scope for v1, or yanked to a later milestone? |

---

## 7. Summary

**What is genuinely good**
- Production-hot-path `.unwrap()`/`.panic!()` surface is clean (prior
  hardening pass was effective).
- `math/` core (`vector*`, `matrix*`, `bspline`, `nurbs`, `tolerance`) is
  coherent and has thoughtful edge-case tests.
- B-Rep topology (`primitives/topology_builder`, `shell`, `solid`) is correct
  and passes the Euler-character / manifold tests it defines.
- Tessellation + cache layer is well-structured.

**What is actually weak**
1. **Zero property tests on boolean ops.** This is the kernel's most
   correctness-sensitive module and it has only liveness tests.
2. **~2,100 LOC of competing SSI code across three files** — pick one.
3. **1,792 LOC of orphaned source** not in any `mod` tree.
4. **`ai_primitive_registry.rs` contains hardcoded lies** (`volume: 0.0`,
   `vertex_count: 8`) on the one surface AI actually touches. This
   directly undermines the "AI-readable kernel" thesis.
5. **`static mut` without atomics** in `performance/mod.rs`.

**Is it worth pursuing as a product?**
The substrate is real. The math layer is serious work and the hardening pass
proves it can carry production-grade discipline. The weaknesses above are all
addressable in a small number of focused refactors — none of them require
rewriting the kernel. The single most important investment is (1), property
testing of booleans; it converts the kernel from "it compiles and doesn't
panic" to "it is provably correct on a distribution of inputs".
