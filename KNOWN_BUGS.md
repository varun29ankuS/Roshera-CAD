# Known Bugs — Roshera Kernel

A living ledger of defects found while exercising the kernel through the
API and tests. Append as we find; move to **Fixed** (with the commit)
when closed. The diagnostic render (`GET /api/agent/parts/{id}/render?mode=diagnostic`,
`open_edges`/`nonmanifold_edges`) is the standing oracle for geometry validity.

Status key: 🔴 open · 🟡 in progress · 🟢 fixed

---

## Boolean

### #35 🟡 Difference cut intersecting another bore leaves open faces
Box − vertical bore − crossing horizontal bore: the second cut's wall
fragments where it breaks into the FIRST bore's void. Repro:
`boolean::tests::diff_intersecting_bores_35` (ignored) → 15 open + 3 nm,
euler −10. **Localized (diagnostic render, 2026-06-13):** the open edges
are the saddle intersection curve where the horizontal tunnel wall meets
the vertical bore void — the hbore wall facets that pass through the
already-empty vbore region are not trimmed/welded against the vbore wall
facets, leaving the saddle loop open. Both bores are 24-gon prisms, so
this is faceted plane∩plane at the saddle, not analytic SSI. Fix lives in
the difference pipeline's handling of a cutter face that crosses a
pre-existing void boundary (the second operand's wall must be clipped to
material and welded to the first void's wall along the shared saddle).

### #36 🟢 Boolean leaves invalid operand husks in the solid store
After a boolean the consumed operands lingered in `SolidStore` as
degenerate solids (Euler χ ≠ 2) — phantom parts that amplified #29.
Fixed: `boolean_operation` now removes both operands from `SolidStore`
on success (inside the rollback closure, so a failure restores them).
This aligns the kernel with the API, which already unregisters the
operand UUIDs and broadcasts `object_deleted`. Verified live: a union's
`GET /api/agent/parts` now lists only the result. Commutativity parity
tests updated to `deep_clone` operands for the second ordering.

### #32 🔴 Coincident-face union produces invalid B-Rep (Same-Domain)
Unioning a solid whose face sits *exactly coincident* on another's face
(e.g. a riser resting on a plinth top) yields χ=4 + non-manifold rim
edges. Interpenetrating the solids 10mm makes the identical union clean
(χ=2, open=0 nm=0). Needs Same-Domain face unification. Workaround:
always interpenetrate, never touch face-to-face.

### #27 🟢 Coaxial stacked-step union left buried cap
Fixed via annular face-with-hole interior-point. See boolean campaign.

### #33 🟢 Offset/partial-overlap chained union invalid
Fixed (line-extent classification).

### #34 🟢 Difference ops leave OPEN faces (counterbore floor)
Fixed via `drop_nested_inner_loops` after merge.

---

## Blends (fillet / chamfer)

### #82 🔴 Multi-edge ring chamfer — corner-patch not implemented
Chamfering a closed ring of edges errors at the shared corner vertices
(Cliff / ConvexCorner): "corner-patch synthesis for this vertex kind is
not yet implemented." Workaround: apply each edge in a separate call
(single-edge chamfer works on clean geometry).

### #37 🟢 Chamfer on a boolean-reshaped face → invalid B-Rep
NOT a chamfer bug — the *validator* was wrong. Any solid with a face that
has a hole (a through-bore, counterbore, or a box pierced by another box)
has `V−E+F ≠ 2`, because the naive Euler formula only holds when every
face is a disk. The validator used `V−E+F = 2`, so it rejected every
boolean result with a face-hole, which then blocked chamfer/fillet on it.
Fixed: `validate_euler_characteristic_for_solid` now uses the generalized
**Euler–Poincaré** identity `V − E + F − R = 2(S − G)` (R = inner loops,
S = shells, G = genus), counting R/S across all shells. Pierced/bored
solids validate; chamfer on a union result succeeds. Regression:
`boolean::tests::pierced_face_union_passes_euler_poincare_validation_37`.
The mixed-kind-corner cap's error filter was broadened to the `"Invalid
Euler"` prefix to keep matching the reworded message.

### #70 🔴 Chamfer crossing a fillet → self-overlapping solid
Chamfering over a previously filleted region produces a
topologically-clean but geometrically self-overlapping solid. Pinned.

### #38 🔴 Fillet rejects a geometrically valid radius
R20 on a 30mm-tall corner between 140/220mm faces rejected
"Invalid radius: 20"; R12 ok. Looks like an over-conservative
radius-feasibility gate. Low severity.

---

## Validation / model lifecycle

### #29 🟡 Op post-validation runs over the WHOLE model
A new operation's validation validated *every* solid, so any unrelated
invalid solid (e.g. a #36 husk) blocked an otherwise-valid op.
Added `validation::validate_solid_scoped(model, solid_id, …)` (keeps
errors on the touched solid + model-global, drops other solids') and
wired it into the 7 single-solid ops: chamfer, fillet, revolve,
transform, draft, loft, shell. **Remaining (#39):** `blend` and
`pattern` validate by face-sets (pattern spans multiple new solids) and
need a face-set-scoped variant.

### #24 🔴 Sketch-extrude of a circle → 64 planar faces, not one cylinder
A circular profile extrudes to a 64-sided faceted prism instead of an
analytic cylinder. Makes round bores faceted and inflates downstream
boolean facet counts.

### #28 🔴 Full 2π revolve of an offset rectangle → tessellation_empty

---

## Other notes (not bugs, but gotchas)

- **Edge IDs are global and accumulate across solids.** A "fresh box" is
  not always edges 0..11. Always probe `GET /api/agent/edges/{id}` and
  classify by endpoint coordinates before selecting edges for a blend.
