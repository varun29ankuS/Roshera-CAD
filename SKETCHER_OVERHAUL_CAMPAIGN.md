# 2D Sketcher Overhaul — Campaign

> Goal: make Roshera's 2D sketcher **competitive with SolidWorks / Onshape /
> Fusion** — auditable, editable, hideable. Started 2026-06-20 after a live
> session surfaced that the sketcher is structurally a one-shot flow, not a real
> sketch entity.

---

## 1. The diagnosis (the WHY)

The sketcher is built as a **one-shot "draw → extrude/revolve" flow**, not as a
**persistent, first-class, mode-aware sketch *entity***. Every symptom found in
the live session falls out of that single root:

| Symptom (live-found) | Root |
|---|---|
| Drawn curve **vanished on finish** | No persistent sketch entity (it's a transient session) |
| Generating curve **occluded** by the solid | Rendered coincident with the wall; no on-top treatment |
| XZ sketch shows as an **unreadable diagonal** | No camera look-at normal-to the sketch plane |
| **Segment table eats half the screen** (26 length fields) | No "committed, show clean curve" notion; per-segment-length is wrong UI for a curve |
| **Line chases the mouse** forever | No Draw/Edit/View mode — only Draw mode; cursor-chase is a draw affordance with no off-state |
| Sketch **not hideable** like a solid | A sketch isn't a first-class scene object with a visibility flag |

**Leverage (the head start):** the *kernel already has the constraint layer* — 39
constraints + a solver, csketch API ~95% (see memory `sketch-dcm-campaign`). This
is mostly a **frontend + wiring** campaign, not a from-scratch solver build.

**Acceptance bar = the six symptoms above are gone**, plus the "pro feel" of live
constraint solving.

---

## 2. Current state (inventory)

- `roshera-app/src/components/viewport/SketchOverlay.tsx` — **2717-LOC monolith**:
  capture plane, snap pipeline, csketch entities, dimension labels, geometric-
  constraint badges, draggable point handles, live preview (`PolylinePreview`,
  `RectanglePreview`, `CirclePreview`, `SketchPreview`, `CSketchDimensions`,
  `CSketchPoints`, `CommittedShapesGuides`).
- `ServerSketches.tsx` — passive renderer for backend/agent-authored sketches
  (deletes-on-consume by design).
- `SketchPanel.tsx` — the floating panel: tool select (polyline/rect/circle),
  the per-segment **length table**, finish op (extrude / cut / revolve).
- `scene-store.ts` — `sketch` slice (transient session: `active`, `serverId`,
  `shapes`, `points`, `hover`, `editingSourceObjectId`, …), `serverSketches` map,
  `enterSketch`/`exitSketch`/`setSketchPoint`.
- `lib/sketch-api.ts`, `lib/csketch-api.ts` — REST clients (csketch = the
  constrained, solver-backed layer; ~95% per the DCM campaign).
- `ModelTree.tsx` — already nests *consumed* sketches under their producing
  feature via `analyticalGeometry.params.sketch_id`.
- **Kernel**: `sketch2d/` (39 constraints + `constraint_solver`), the parametric
  csketch layer, `revolve`/`extrude` (currently consume + DELETE the profile).

### Already landed this session (P0 quick wins)
- ✅ Committed sketches **persist** on finish (only empty sessions auto-delete) —
  `scene-store.exitSketch` (`hasContent` guard).
- ✅ Committed sketches render **on-top** (`depthTest:false` + renderOrder) —
  `ServerSketches.tsx`.
- ✅ **Plane look-at** on sketch entry (camera snaps normal-to plane) —
  `CameraController.tsx`.
- ✅ Segment-table **height-capped + scrollable** — `SketchPanel.tsx`.

---

## 3. Architecture target

### ★ The decision (Varun, 2026-06-20): backend-first, because verification
The 2D sketch **model + constraint solver belong in the KERNEL**, not the
frontend — per the project's own star-topology rule (*backend-driven, frontend =
thin display layer*). The decisive reason isn't tidiness: **the validity /
verification layer can only certify what the kernel owns.** A frontend sketch
can't be certified "well-constrained" or "constraint-consistent"; only a *kernel*
sketch entity can carry sketch-validity into the `ValidityCertificate`. So
backend-first is the **precondition for the "can't lie" moat extending to
sketches** — the apex of this whole campaign.

> Corner cut to fix: the `hiddenSketchIds` visibility I added this session is
> **frontend-only state**. For a true first-class entity, visibility + lifecycle
> + consumed-linkage must be **kernel/timeline properties**, broadcast to the
> frontend (persist across reload, sync across collaborators, live in history).

### The split
| Concern | Home |
|---|---|
| Sketch **entity** (persistent, **event-sourced** in the timeline), geometry, **constraints + solver**, DOF analysis, validation, visibility/lifecycle state, consumed-linkage to its feature, **sketch-validity certificate** | **Backend / kernel** — source of truth |
| **Rendering** the sketch, the **modal rail**, camera look-at, drag handles, picking; sending user **intents** (add-point, drag, set-constraint) and rendering broadcasts | **Frontend** — thin display layer |

### The apex: a sketch-validity certificate (the moat for sketches)
Extend `ValidityCertificate` with a **sketch dimension**: constraint-consistency
(no conflicting constraints), **DOF state** (under / fully / over-constrained),
closed-profile, self-intersection-free. The kernel **refuses to lie** about a
sketch the way it refuses a non-manifold solid. Only possible because the sketch
+ solver live server-side.

### Frontend target (display layer)
A sketch renders as a first-class, hideable object with an explicit **mode**
(Draw / Edit / View) and a **modal workspace** (pastel-red rail) — but every bit
of *state* it shows is owned by the backend. Decompose the 2717-LOC overlay
monolith into a `sketch/` submodule tree as we go.

---

## 4. Phased slices

### P0 — Foundation: sketch as a first-class, mode-aware entity
- [x] Persist on finish · [x] on-top render · [x] plane look-at · [x] cap table.
- [x] **P0.5 — Sketch = a first-class scene entity.** A committed sketch carries
  visibility in the store (`hiddenSketchIds` + `toggleSketchVisibility`),
  `ServerSketches` skips hidden ids, and the `ModelTree` ●/○ toggle hides/shows a
  sketch row exactly like a solid. (Selectable + deletable to follow.)
- [ ] **P0.6 — Draw / Edit / View mode state machine.** Add an explicit mode to
  the `sketch` slice. Cursor-chase preview (`PolylinePreview` hover-tail) and
  per-segment hover labels render ONLY in Draw mode. Reopening a committed sketch
  enters Edit (no rubber-band). A finished, unfocused sketch is View.
- [ ] **P0.7 — Modal sketch *workspace* (Varun, 2026-06-20).** Sketch is a
  first-class WORKSPACE, not a floating panel:
  - Its own **button on the left navigation rail** (alongside Transform / Create /
    Operations / Modify / Manufacturing / Export). The agent can enter it too.
  - On entering sketch, the **left rail TRANSFORMS** into the sketch toolset (line
    / arc / circle / rect / spline / trim / dimension / constraints / mirror /
    offset …) — context-swapped, like SolidWorks' sketch ribbon.
  - The rail (and a viewport frame) turn **pastel red** — an unmistakable MODAL
    signal that the app is in sketch mode and global operations are suspended
    until Finish/Exit. Non-sketch actions are gated while modal.
  - Files: the left rail component (`Toolbar`/left-nav), a `sketchMode` flag in
    the store (likely folds into P0.6's mode), theme tokens for the pastel-red
    modal skin.

### P1 — The "pro feel" (leverage the EXISTING kernel solver)
- [ ] **Live constraint solving on drag** — drag a point → POST to the csketch
  solver → geometry re-solves and updates live. THE single biggest jump from
  "polyline tool" to "CAD sketcher." Solver already exists; this is wiring.
- [ ] **Driving dimensions** — type a value into a dimension → solver drives the
  geometry (not the current "move endpoint along segment" hack).
- [ ] **Auto-constraint inference** while drawing (snap → infer & create
  horizontal / vertical / coincident / tangent / equal).
- [ ] **Constraint-status coloring** — under / fully / over-constrained (the
  classic blue/black/red), so the user can audit the sketch's DOF.

### P2 — Entities
- [ ] **Spline** (fit-point + control-point) with control-point editing — replaces
  the 26-segment-length table for curves (a nozzle meridian should be a spline,
  not 26 lengths).
- [ ] **Arc** (3-point / tangent / center-start-end).
- [ ] **Slot**, **ellipse**, **regular polygon**.
- [ ] **Construction geometry** (reference lines/circles, distinct style).

### P3 — Editing operations
- [ ] **Trim** / **extend** to nearest intersection.
- [ ] **Offset** (parallel curve).
- [ ] **Mirror** about a line.
- [ ] **Fillet / chamfer** a sketch corner.
- [ ] **Convert entities** (project an edge of a solid onto the sketch plane).
- [ ] **Pattern** (linear / circular) in-sketch.

### P4 — Dimension & constraint UX
- [ ] Dimensions shown **on hover/select**, not all-at-once (kill the clutter).
- [ ] **Constraint glyphs** (the small badges) placed cleanly, toggleable.
- [ ] Smart dimension placement (offset leaders, no overlap).
- [ ] Sketch-plane **grid** + snap-to-grid toggle.

### P5 — Structural cleanup
- [ ] **Decompose `SketchOverlay.tsx`** into `sketch/` submodules (capture-plane,
  snapping, entities, dimensions, badges, point-handles, preview). Move pure
  helpers (`buildCSketchDimensionLabels`, `collectSnapTargets`) to `lib/`.

---

## 5. Revolve/extrude → sketch sub-part ("consumed but should not vanish")

Varun's explicit ask: a feature's generating sketch should be **retained and
nested** as a sub-part, not deleted.
- [ ] **Kernel** — `revolve`/`extrude` (and the raw `revolve` REST that takes
  `[r,z]` points) **retain their source profile** as a linked sketch entity
  rather than deleting it on consume. The op references the sketch id.
- [ ] **Frontend** — render the consumed sketch persistently (dimmed / on toggle),
  nested under the feature in `ModelTree` (the nesting link already exists), and
  "Edit sketch" reopens it → regenerates the feature (the parametric loop).

---

## 6. Acceptance criteria (the bar)

1. A finished sketch **persists** and is **visible**.
2. A sketch is **hideable** like a solid (tree visibility toggle).
3. **No cursor-chase** line outside Draw mode.
4. The sketch reads **face-on and clean** (no all-segments dimension dump).
5. A **curve is a spline** (control points), not 26 segment lengths.
6. A consumed sketch is a **visible sub-part** of its feature.
7. **Live constraint solving** on drag (the pro-feel proof).

---

## 8. Backend plan — grounded in the 2026-06-20 kernel inventory

**The headline finding: the hard part is already built, but it's not on the live
path.** There are TWO disconnected sketch layers:
- `geometry-engine/src/sketch2d/` — a **production-grade constrained Sketch
  entity + solver**: **46 constraint kinds** (22 geometric + 18 dimensional +
  specials — not "39"), Newton-Raphson + **Tikhonov** regularisation,
  **DOF analysis** (`analyze_dofs`), **conflict/redundancy diagnosis** (Gram-
  Schmidt MUS), the **shared-variable model** (D-Cubed-style endpoint coupling),
  drag solving, and a validation engine (`sketch_validation.rs`: entity / self-
  intersection / constraint-satisfaction / topology). This is a real DCM-grade
  solver.
- `api-server/src/sketch.rs` — a **transient, click-to-place SESSION manager**
  (`DashMap<Uuid, SketchSession>`, in-memory, **never event-sourced**, dies with
  the server). **This is what the frontend talks to today.**

So **sketches fall into the gap**: the powerful constrained `Sketch` is never used
by the live path; the live path is a dumb polyline-bag that gets extruded. The
nozzle meridian I built used the dumb layer. The work is **unify + persist +
certify**, NOT build-a-solver.

### Backend phases (dependency order)
- [ ] **B1 — Unify the live path onto the real `Sketch`.** Route the frontend /
  agent sketch surface to the constrained kernel `Sketch` (via `csketch.rs`, which
  exists but isn't integrated), not the click-to-place session. The dumb session
  becomes a thin adapter or is retired.
- [ ] **B2 — Persist + event-source.** Add `sketches: DashMap<SketchId,
  Arc<Sketch>>` to `BRepModel` (today only a `sketch_planes` stub at
  topology_builder.rs:526); emit `RecordedOperation` from sketch create/edit/
  constrain/solve (today only the *extrude result* is recorded — `create_sketch.rs`
  op is a stub); add reverse-link `sketch_source: Option<SketchId>` on the Solid;
  snapshot/restore in timeline replay. → survives reload, replays, collaborative.
- [ ] **B3 — ★ Sketch-validity certificate (THE MOAT).** Extend
  `ValidityCertificate` (provenance.rs) with a sketch dimension:
  `well_constrained` (Under/Fully/Over/Conflicted — from `analyze_dofs` +
  `diagnose`), `closed_profile`, `self_intersection_free`, `constraint_consistent`.
  **All the data is already computed by the solver/validator — this is wiring**,
  and it makes the kernel refuse to lie about a sketch. Not blocked by B1/B2; most
  useful once B1 lands.
- [ ] **B4 — Constrained-editor REST surface.** Expose `csketch` endpoints
  (add/edit/delete constraint, solve, validate, DOF readout) on the live API,
  RBAC-gated, timeline-recorded. This is what P1 (live constraint solving on drag)
  calls.
- [ ] **B5 (deferred)** — datum-frame invalidation when a datum moves (blocked by
  the propagation-graph design, out of sketch scope).

Then the **frontend** (P0.6/P0.7 modal workspace, rendering) becomes a thin
renderer of B1–B4, and `hiddenSketchIds` moves server-side (a Sketch lifecycle
property, broadcast).

## 7. Sequencing recommendation

P0.5 + P0.6 first (they kill 4 of the 6 live bugs and are the foundation), then
**P1 live-solve** (the headline feature; solver already exists), then P2 spline
(kills the segment-table problem for curves), then the revolve-subpart, then
P3/P4/P5 as polish. Decompose the monolith *incrementally* as each area is
touched, not as a big-bang refactor.
