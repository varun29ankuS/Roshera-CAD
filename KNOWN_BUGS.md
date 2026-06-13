# Known Bugs — Roshera Kernel

A living ledger of defects found while exercising the kernel through the
API and tests. Append as we find; move to **Fixed** (with the commit)
when closed. The diagnostic render (`GET /api/agent/parts/{id}/render?mode=diagnostic`,
`open_edges`/`nonmanifold_edges`) is the standing oracle for geometry validity.

Status key: 🔴 open · 🟡 in progress · 🟢 fixed

---

## Boolean

### #35 🟡 Difference cut intersecting another bore leaves open faces
Box − vertical bore − crossing horizontal bore: the cylinder∩cylinder
saddle leaves ~15 open + 3 non-manifold edges. Repro:
`boolean::tests::diff_intersecting_bores_35` (ignored). Localized to the
saddle between the two bore cutters' facets.

### #36 🔴 Boolean leaves invalid operand husks in the solid store
After a boolean, the consumed operands remain in `SolidStore` as
degenerate/invalid solids (Euler χ ≠ 2). The API drops the
UUID→solid_id mapping but the kernel `Solid` persists. Live repro:
extrude two boxes, union → result, but `GET /api/agent/parts` still
lists both operands; each is invalid. **Amplifies #29.** Fix:
`boolean_operation` must remove consumed operands from `SolidStore` on
success. Workaround: `DELETE /api/agent/parts/{id}` the husks.

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

### #37 🔴 Chamfer on a boolean-reshaped face → invalid B-Rep
Single-edge chamfer of an edge adjacent to a face that a prior boolean
reshaped fails post-validation (χ=3). The same single-edge chamfer
succeeds on a fresh box. Workaround: chamfer BEFORE the boolean on clean
geometry, then interpenetrate-union.

### #70 🔴 Chamfer crossing a fillet → self-overlapping solid
Chamfering over a previously filleted region produces a
topologically-clean but geometrically self-overlapping solid. Pinned.

### #38 🔴 Fillet rejects a geometrically valid radius
R20 on a 30mm-tall corner between 140/220mm faces rejected
"Invalid radius: 20"; R12 ok. Looks like an over-conservative
radius-feasibility gate. Low severity.

---

## Validation / model lifecycle

### #29 🔴 Op post-validation runs over the WHOLE model
A new operation's validation validates *every* solid, so any unrelated
invalid solid (e.g. a #36 husk) blocks an otherwise-valid op. Confirmed
live: chamfer rejected because of leftover union husks. Validation
should scope to the solid(s) the op touched.

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
