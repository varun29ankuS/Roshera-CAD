# Task #82 Slice 1 — synthesize a concave degree-3 corner-blend patch

**Status:** DONE_WITH_CONCERNS (feature complete + green; one scope
correction to the brief's GREEN test, documented below).

**Code+test commit:** `68cf671`
(`fillet: synthesize concave 3-edge corner sphere cap (Task #82 Slice 1)`).

---

## STEP 0 — confirmed precondition (probe removed before commit)

Built the blend graph over all edges of the `notched_box` and inspected the
origin corner:

```
PROBE origin_vid=8 kind=ConcaveCorner { degree: 3 }
PROBE incident_blend_edges=[12, 15, 20]
```

So the classifier now aggregates the three re-entrant concave edges into
`ConcaveCorner { degree: 3 }`, exactly as expected. No classifier-aggregation
gap — the boolean-winding fixes (A + B) at HEAD already deliver it.

The probe also printed the three notch-wall **oriented** outward normals at the
origin (accounting for `face.orientation`):

```
face 6: (0, 0, -1)   face 8: (0, -1, 0)   face 10: (-1, 0, 0)
```

i.e. `-z, -y, -x`. This is the load-bearing fact for the sign decision below.

---

## vertex_outward sign decision + evidence

With oriented normals `-x, -y, -z`, the per-edge cylinder-axis origins are
`P_i = V − (r/(1+c))·(n_a+n_b)` and the least-squares apex solves to
**`apex = (+r, +r, +r) = (3, 3, 3)` — inside the removed pocket (the void)**,
NOT inside the material. (This is the geometric signature of a re-entrant
corner: the rolling ball sits in the void and the fillet fills material up to
its surface.)

Therefore `vertex_pos − corner_apex = (0,0,0) − (3,3,3) = (−,−,−)` points from
the void back **into the material** — the wrong way for the cap's outward
normal. So the concave case **must negate** `vertex_outward` to `(+,+,+)`,
pointing into the pocket. The brief's expected answer (negate) is correct; my
initial hand-analysis assuming `+x+y+z` notch normals was wrong, and the probe
corrected it — hence STEP 0 was worth doing.

**Adjudication (not assumption):** the GREEN test samples the cap face's
oriented outward normal at the patch point nearest the origin and asserts
`normal · (1,1,1) > 0` (points into the +,+,+ pocket), and it separately
asserts `cert.oriented` (mesh-level coherent winding — the check that would
catch a flipped cap at the tessellation level). Both pass. The resolved face
orientation is `Backward`: radial-outward at the cap point is `(−,−,−)`, and
`Backward` negates it to `(+,+,+)` = into the pocket. ✔

---

## Hidden convex assumption in `apply_apex_sphere_corner` — YES, found + fixed

The body samples the sphere normal for orientation at
`octant_point = sphere_center + vertex_outward * r`. That assumes the cap patch
sits on the `+vertex_outward` side of the sphere — true only for a **convex**
corner. For a concave corner `vertex_outward` was flipped into the void, so
that sample point lands on the **far** side of the sphere, where a sphere's
radial normal has the **opposite** sign to the actual cap patch; the
orientation pick would then choose the wrong face orientation (into the
material).

Fix: sample on the cap-patch side while keeping the orientation **target** as
`vertex_outward`:

```rust
let cap_side = if matches!(corner_kind, BlendVertexKind::ConcaveCorner { .. }) {
    -vertex_outward
} else {
    vertex_outward   // convex: byte-identical to the pre-#82 path
};
let octant_point = sphere_center + cap_side * sphere_radius;
let orientation = orient_face_for_outward_at(&sphere, vertex_outward, u_oct, v_oct)?;
```

No other convex assumption exists in the body: cap-arc location
(centre-coincidence with the apex), the closed-triangle verification, and the
traversal-order loop build are all convexity-agnostic and worked unchanged for
concave (proven by `cert.brep_valid && cert.oriented && cert.watertight`). The
loop winding did **not** need a separate flip — `verify_cap_arcs_form_closed_triangle`
walks the actual cap-arc edge endpoints, which already follow the concave
fillet faces' winding, so the single orientation-flag flip suffices.

---

## `compute_apex_setbacks` — needed widening

Yes. The degree-3 work-list filter was gated on `ConvexCorner { degree: 3 }`;
widened to also match `ConcaveCorner { degree: 3 }`. The interior math needed
**no** sign change: `P_i` is built from the actual oriented face normals (which
place the apex in the void for a re-entrant corner) and the retraction is
`|(apex − P_i)·u_i|` (a magnitude). The `_mixed` variant was left untouched
(equal-radius notch does not use it).

A second, initially-missed preservation gate also needed widening:
`is_three_edge_convex_corner` (renamed → `is_three_edge_apex_corner`) sets the
`original_v?_corner_shared` flag that stops the FIRST edge's splice from
dropping the shared corner vertex. Gated on convex-3 only, the concave corner
vertex was dropped mid-surgery and the remaining edges failed validation
(`BlendEdgeSurgery original_v? 9 missing from model`). Widened to cover
`ConcaveCorner { degree: 3 }`.

---

## Scope correction to the brief's GREEN test (the concern)

The brief's test fillets **all** edges of the notched box and `.expect(...)`s
success. A probe of every corner shows this is **unachievable in Slice 1**: the
notched box has **three `Mixed` corners** at `(20,0,0)`, `(0,20,0)`, `(0,0,20)`
where a concave notch edge meets convex box edges. `Mixed` corners require a
Gregory/S-patch (F5-δ) and are explicitly out of Slice-1 scope.

The GREEN test therefore fillets **only the three re-entrant edges** incident
to the origin (found dynamically via `concave_reentrant_edges`). Those three
share the concave degree-3 apex; their far ends become degree-1 (plain
cylinder-fillet terminations at the Mixed vertices, no patch). This exercises
exactly the concave apex-sphere corner patch and nothing else — the true
subject of the slice.

Defensive follow-on: I added a guard so a concave-3 corner with **unequal**
radii (which would fall into the convex-only `apply_triangular_nurbs_corner`
path now that lifecycle admits the kind) returns a typed
`VertexBlendUnsupported { MixedRadii }` instead of risking a mis-oriented
patch. Equal-radius (the notch) is fully supported; mixed-radius concave is a
typed refusal, preserving the transactional contract.

---

## VERBATIM RED

Pre-existing RED anchor (`concave_three_edge_corner_currently_refuses`) passed
first, confirming the concave corner was refused at the lifecycle gate:

```
test concave_three_edge_corner_currently_refuses ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
```

First run of the new GREEN-target test (before the fixes) failed in per-edge
surgery — the corner-shared preservation gap:

```
thread 'concave_three_edge_corner_fillets_watertight' panicked at
geometry-engine\tests\fillet_concave_three_edge_corner.rs:119:10:
concave three-edge corner fillet must succeed: InvalidGeometry("BlendEdgeSurgery original_v0 9 missing from model")
test concave_three_edge_corner_fillets_watertight ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
```

(After widening the preservation gate, the same class re-surfaced as
`original_v1 9 missing` — which the all-corner probe traced to the three `Mixed`
corners, motivating the scope correction above.)

## VERBATIM GREEN

```
test concave_three_edge_corner_fillets_watertight ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.55s
```

---

## Convex + adjacent no-regression summary (all green)

- `fillet_three_edge_corner` (convex apex path): 3 passed, 1 ignored.
- `edge_convexity_boolean_notch`: 1 passed.
- `fillet_boolean_concave_step_ball_side`: 1 passed.
- `boolean_loop_winding_consistency`: 4 passed.
- `fillet_three_edge_corner_mixed_radii`: 11 passed, 1 ignored.
- `chamfer_three_edge_corner` (3), `chamfer_n_edge_corner` (3),
  `cf_beta_mixed_kind_corner` (10), `cf_gamma_g1_mixed_kind_corner` (5),
  `fillet_corner_cap_mass_props` (3), `blend_scar_preflight` (6): all passed.
- `cargo test -p geometry-engine --lib` filtered to
  `operations::blend_graph`/`operations::lifecycle`/`operations::fillet`:
  199 passed, 0 failed.

The convex apex-sphere path is byte-identical (`cap_side == vertex_outward`,
`corner_kind == ConvexCorner { degree: 3 }` in every previously-hardcoded error
variant, filter arm unchanged).

---

## Every file:line touched (at commit `68cf671`)

- `geometry-engine/src/operations/fillet.rs`
  - `is_three_edge_apex_corner` (renamed from `is_three_edge_convex_corner`,
    now covers `ConcaveCorner { degree: 3 }`) — def ~L1216; callers L1453/L1456.
  - `apply_apex_sphere_corner` — added `corner_kind: BlendVertexKind` param
    (+`#[allow(clippy::too_many_arguments)]`) ~L3768; error variant now
    `corner_kind` ~L3792; cap-side octant sampling gate ~L3820.
  - `create_fillet_transitions` — filter admits `ConcaveCorner { degree: 3 }`
    and binds `corner_kind` ~L7510; error variants → `corner_kind`
    ~L7918/L7981/L8065; `vertex_outward` `let mut` + concave negation ~L8001;
    concave mixed-radii typed refusal guard ~L8131; `apply_apex_sphere_corner`
    call passes `corner_kind` ~L8123.
- `geometry-engine/src/operations/blend_graph.rs`
  - `compute_apex_setbacks` work-list filter widened to `ConcaveCorner { degree: 3 }`
    ~L831.
- `geometry-engine/src/operations/lifecycle.rs`
  - `validate_corner_compatibility` pre-flight allow-list arm
    `(_, Some(ConcaveCorner { degree: 3 })) => return Ok(())` ~L836; doc-table
    row updated ~L699.
- `geometry-engine/tests/fillet_concave_three_edge_corner.rs`
  - Deleted `concave_three_edge_corner_currently_refuses`; added
    `concave_reentrant_edges` helper + GREEN
    `concave_three_edge_corner_fillets_watertight`.

---

## Concerns

1. **Brief scope vs. reality (resolved, but flag for the plan):** "fillet all
   edges of the notched box" is not achievable in Slice 1 because of the three
   `Mixed` corners. The GREEN test is scoped to the re-entrant edges. A future
   slice covering `Mixed` degree-3 corners would let the full all-edges fillet
   succeed and is the natural next step (F5-δ).
2. **Concave mixed-radii is a typed refusal, not a synthesis.** Lifecycle now
   admits `ConcaveCorner { degree: 3 }`, but only the equal-radius apex-sphere
   arm is implemented; unequal radii return `MixedRadii`. Intentional and
   transactionally safe, but worth noting the door is open at the gate while
   the mixed-radii concave patch is not yet built.
3. The helper name `is_three_edge_apex_corner` and the `F5-α` comment lineage
   now span both convexities; docs were updated for honesty, but the broader
   `F5-α/β/δ` naming in this file still reads convex-centric in places.
