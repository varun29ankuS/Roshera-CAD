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
this is faceted plane∩plane at the saddle, not analytic SSI.

**Root cause refined (2026-06-13, deeper trace + experiment):** NOT a
weld-tolerance problem. Both walls ARE split at the saddle (2nd diff:
51 kept solid-A frags + 48 kept solid-B frags), but each operand imprints
the saddle intersection INDEPENDENTLY, producing vertices that are not
coincident — `canonicalise_face_edges_by_position` reports
`canonical_collapses=0` even at a 1e-3 probe tolerance (1000× the model
1e-6). So the kept A-wall fragments and B-wall fragments meet along two
slightly different polylines → genuine gap → 15 open edges. Position-weld
can't fix it because the points genuinely differ.

**Fix direction — COREFINEMENT (cutting-edge consensus):** compute each
face-pair intersection ONCE and insert the SAME shared vertices/edge into
BOTH faces' splits, rather than letting each operand re-imprint. Two
robustness substrates from current literature:
- **Indirect predicates** (Cherchi/Livesu/Attene, "Interactive and Robust
  Mesh Booleans" SIGGRAPH 2022; "Fast and Robust Mesh Arrangements" SIGGRAPH
  Asia 2020, header-only OSS): represent an intersection vertex IMPLICITLY by
  its defining construction (the 3 planes that meet there), so both faces
  reference the identical point by construction — coincidence is exact, not
  tolerance-based. Ties to #30 (exact predicates).
- **CGAL-style corefinement**: exact constructions under the hood, both
  surfaces refined along one shared polyline.
Roshera is a B-Rep (not a triangle soup), so the targeted version: in
`compute_face_intersections` / the split, intern the intersection curve's
vertices in a shared table keyed by the implicit construction and reuse the
same VertexId/EdgeId when imprinting both operands' faces. Ties to #6
(persist boolean pcurves) and #30. Refs: arXiv:2205.14151, arXiv:2405.12949,
ACM TOG EMBER (10.1145/3528223.3530181).

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

### #38 🟢 Fillet rejected a geometrically valid radius
`validate_fillet_parameters` bounded the radius by `edge_length * 0.5` —
the WRONG dimension. A fillet's rolling ball runs *along* the edge; its
radius is limited by the *perpendicular room on the adjacent faces*, not
the filleted edge's own length. So R20 on a 30mm edge between 200/120mm
faces was falsely rejected. Fixed: bound by the shortest NEIGHBOURING
edge (the edges meeting this one at its endpoints — the perpendicular-room
proxy the tangent line runs along); isolated edges (no neighbours) defer to
downstream construction. Verified empirically: the construction was never
the limit — R20 on a 30mm slab edge constructs a watertight solid.
Regression: `fillet::tests::fillet_large_radius_on_short_edge_between_large_faces_38`;
the deliberate half-edge contract in `tests/fillet_radius_validation.rs`
was rewritten to the neighbour bound. 682 operations tests green.

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
