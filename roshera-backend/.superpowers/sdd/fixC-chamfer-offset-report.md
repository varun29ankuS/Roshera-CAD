# Fix C — chamfer offset direction from geometry, not raw loop winding

## Problem
Fix B (reverted at 9eab76a) had made boolean Difference reverse tool-face loop
winding. The full red-gate proved Fix B BROKE two things: brep_integrity
half-edge opposition (`boolean/difference: 4 orientation-flipped shared edges`)
and polyline_cut hole volumes (under-subtracted). Fix B was reverted. But one
consumer legitimately depended on the winding being consistent: the concave
CHAMFER offset in `operations/chamfer.rs::compute_chamfer_offsets`. With Fix B
gone, `tests/chamfer_concave_three_edge_corner.rs::concave_three_edge_corner_chamfers_watertight`
regressed (oriented=false, 308 inconsistent directed edges).

Root cause at `chamfer.rs` offset-direction computation: the in-face inward
offset direction is `face_normal.cross(&t_loop)`, where `t_loop` was oriented by
the RAW loop-winding flag `edge_orientation_in_face`. On a boolean-Difference
tool face the loop flag disagrees with the (proven-correct) outward normal, so
`t_loop` — and thus `offset_dir` — points the WRONG way; the chamfer offset
lands OUTSIDE the face polygon and the chamfer surface outside the solid.

## Fix (correct, does NOT touch boolean winding)
Derive the offset direction from GEOMETRY, exactly like the already-landed Fix A
did for the convexity sign. Reuse Fix A's helper
`edge_classification::geometry_signed_edge_tangent(model, edge_id, face1_id, n1, midpoint)`,
which returns the edge tangent oriented CCW around `face1`'s OUTWARD normal `n1`
via an edge-local face-membership test (robust for annular / holed / curved
faces — not a whole-face centroid).

The loop-orientation sign is CONSTANT along the edge, so it is resolved ONCE at
the edge midpoint and applied per sample.

### Files / lines
- `geometry-engine/src/operations/chamfer.rs:13` — import trimmed to
  `use super::fillet::get_face_oriented_normal;` (`edge_orientation_in_face` no
  longer used in this file).
- `geometry-engine/src/operations/chamfer.rs` `compute_chamfer_offsets`, the
  pre-loop sign derivation (was ~1537-1548): replaced the two raw
  `edge_orientation_in_face` loop-flag reads with a geometry-derived sign
  resolved once at the midpoint:
  `mid_point = curve.point_at(0.5)`, `raw_mid_tangent = curve.tangent_at(0.5)`,
  `mid_normal{1,2} = face_normal_at_point(...)` (outward-oriented),
  `t_geo{1,2} = super::edge_classification::geometry_signed_edge_tangent(model, edge_id, face{1,2}_id, &mid_normal{1,2}, &mid_point)?`,
  `face{1,2}_loop_sign = sign(t_geo{1,2} · raw_mid_tangent)`.
- `geometry-engine/src/operations/chamfer.rs` sample loop (was ~1568-1575):
  removed `edge_dir_sign`; `t_loop{1,2} = edge_tangent * face{1,2}_loop_sign`.
Offset MAGNITUDE and everything else unchanged — only the tangent SIGN
derivation changed.

## Sign-derivation + frame reasoning
This function walks the CURVE tangent directly
(`edge_tangent = curve.tangent_at(t)`), NOT `Edge::tangent_at`. The helper's
returned vector `t_geo` is built from `raw_tangent = Edge::tangent_at(0.5)`
(= `curve.tangent_at(0.5) * edge.orientation.sign()`) times the geometry sign
`s_helper`, i.e. `t_geo = curve_mid_tangent * edge.orientation.sign() * s_helper`
— the geometry-correct loop direction as a physical vector.

The sign that maps THIS function's curve-tangent frame onto that loop direction
is `s = sign(t_geo · raw_mid_tangent)` where `raw_mid_tangent = curve.tangent_at(0.5)`:
`s = sign(edge.orientation.sign() * s_helper * |curve_mid_tangent|²) = edge.orientation.sign() * s_helper`.
So `t_loop = curve.tangent_at(t) * s = curve.tangent_at(t) * edge.orientation.sign() * s_helper`.

Equivalence check on a CONSISTENT primitive face (where `s_helper == loop_sign`):
this equals the old `curve.tangent_at(t) * edge.orientation.sign() * loop_sign`
EXACTLY → convex path is byte-identical. On a boolean-Difference tool face
`s_helper` is derived from the outward normal + local geometry (correct) whereas
`edge.orientation.sign() * loop_sign` is wrong → the concave booleaned chamfer is
corrected. Also note at the midpoint `edge_tangent * s == t_geo`, and the helper
guarantees `(n1 × t_geo) · into_face ≥ 0`, so `offset_dir = n1.cross(t_loop)`
points INTO the face material by construction.

The dot-product form is what absorbs the `edge.orientation` composition
automatically, which is why we could NOT use the scalar
`geometry_loop_tangent_sign` here (that sign is defined relative to
`Edge::tangent_at`, but this function multiplies the raw CURVE tangent).

## Completeness grep (other raw-loop-flag DIRECTION consumers)
Grepped `operations/` for `edge_orientation_in_face` / `loop_sign` /
`.cross(&t_loop)` DIRECTION consumers that would be wrong on boolean-Difference
tool faces without Fix B:
- `chamfer.rs:1537/1543` — THE offset consumer named by the branch review. FIXED
  here. It is the SOLE remaining sibling.
- `fillet.rs:6250-6266` — already Fix-A'd; uses
  `edge_classification::geometry_loop_tangent_sign`. Correct.
- `edge_classification.rs:274` (`loop_flag_tangent_sign`) — the intentional
  Fix-A degenerate-geometry FALLBACK, reached only when the in-surface
  perpendicular is degenerate or membership is indeterminate at every probe
  scale. Not a primary direction source; not a bug.
- `section.rs:1160-1181` (`loop_signature`) — a loop-matching signature, not a
  direction/offset consumer. Unrelated.
No other siblings.

## RED → GREEN (verbatim)

### RED (before fix, Fix B reverted)
```
thread 'concave_three_edge_corner_chamfers_watertight' (...) panicked at geometry-engine\tests\chamfer_concave_three_edge_corner.rs:185:5:
concave-chamfered notched box must be a sound oriented B-Rep; manifold=false oriented=false brep_valid=true inconsistent_directed_edges=308 cert=ValidityCertificate { ... manifold: false, euler_characteristic: -148, boundary_edges: 0, nonmanifold_edges: 56, oriented: false, inconsistent_directed_edges: 308, ... }
test concave_three_edge_corner_chamfers_watertight ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.31s
```

### GREEN (after fix)
```
running 1 test
test concave_three_edge_corner_chamfers_watertight ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
```

## Mutation proof (verbatim)
Reverted ONLY the sign derivation back to `edge_dir_sign * face_loop_sign` (raw
`edge_orientation_in_face` flag), keeping the rest of the fix:
```
concave-chamfered notched box must be a sound oriented B-Rep; manifold=false oriented=false brep_valid=true inconsistent_directed_edges=308 cert=ValidityCertificate { ... oriented: false, inconsistent_directed_edges: 308, ... }
test concave_three_edge_corner_chamfers_watertight ... FAILED
test result: FAILED. 0 passed; 1 failed; ...
```
Identical failure signature (308 inconsistent directed edges) → the sign
derivation is exactly what carries the fix. Restored → GREEN:
```
test concave_three_edge_corner_chamfers_watertight ... ok
test result: ok. 1 passed; 0 failed; ...
```

## No-regression suite (all green)
- `chamfer_concave_three_edge_corner` — ok. 1 passed (restored).
- `chamfer_three_edge_corner` (convex planar cap) — ok. 3 passed.
- `chamfer_closed_edge` — ok. 5 passed.
- `chamfer_n_edge_corner` — ok. 3 passed.
- `chamfer_volume_invariants` — ok. 6 passed.
- `chamfer_world_class` — ok. 11 passed.
- `fillet_chamfer_dihedral_matrix` — ok. 15 passed.
- `fillet_chamfer_stress` — ok. 6 passed.
- `fillet_chamfer_volume_invariants` — ok. 4 passed.
- `fillet_concave_three_edge_corner` — ok. 1 passed.
- `edge_convexity_boolean_notch` — ok. 1 passed.
- `edge_convexity_boolean_annular` — ok. 2 passed.
- **Fix-B-reverted greens re-confirmed still green:**
  - `--lib every_operation_is_orientation_consistent_and_euler_balanced`
    (brep_integrity half-edge opposition) — ok. 1 passed.
  - `--test polyline_cut_harness` (hole volumes) — ok. 34 passed.

## Constraints
No `unwrap`/`expect`/`panic` in production; all fallible calls `?`-propagated
with typed `OperationError`. No TODO/stub. `with_rollback` untouched. Convex
chamfer path byte-identical (proven by equivalence argument + `chamfer_three_edge_corner`
green). Boolean winding NOT touched.

## Commit
`chamfer: derive offset direction from geometry, not raw loop winding (Fix C — concave chamfer correct without touching boolean winding)`
Hash: <filled in below>

## Concerns
None material. The geometry helper's degenerate fallback
(`loop_flag_tangent_sign`) is shared with Fix A and is only reached for
genuinely degenerate micro-geometry; on the concave-notch topology the
membership probe is decisive, so the fallback cannot silently reintroduce the
bug (same guarantee Fix A already relies on).
