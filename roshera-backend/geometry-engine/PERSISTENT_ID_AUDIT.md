# Persistent Topological ID Audit

> **Status:** Design audit (no code yet). Gates implementation slices.
> **Author seat:** Task #40.
> **Verified against working tree:** 2026-05-06.

---

## 1. Why this matters

For an agent runtime, the durable name an LLM reasons about ("the top
face of the boss") MUST survive parameter edits, regenerations, and
downstream operations. Today the kernel's `VertexId / EdgeId / FaceId`
are dense `u32` indices into store arrays — they are stable within one
`BRepModel` instance, and they are blown away on every operation that
synthesizes new topology.

This breaks three things in the agent path:

1. **Re-running a timeline event** with edited parameters produces a
   `BRepModel` whose IDs have no relation to the prior run's IDs. An
   agent that said "fillet edge 17" cannot find edge 17 again.
2. **Queries through the readable surface** (`readable/query.rs`)
   resolve to live `FaceId`s; persisting a query result is meaningless
   the moment the model is regenerated.
3. **`OperationRecorder` payloads** carry `inputs: Vec<u64>` / `outputs:
   Vec<u64>` (see `operations/recorder.rs`) — these are runtime indices.
   Replaying a timeline against a fresh model produces the same shape
   but different IDs; downstream events can't refer back.

This audit defines the gap and proposes the integration seam. No
implementation yet.

---

## 2. Where IDs are minted today

### 2.1 ID types

```
primitives/vertex.rs:22   pub type VertexId = u32;
primitives/edge.rs:25     pub type EdgeId   = u32;
primitives/face.rs:29     pub type FaceId   = u32;
```

Plus `LoopId`, `ShellId`, `SolidId`, `CurveId`, `SurfaceId` — same
pattern, all `u32`.

### 2.2 Allocation

`VertexStore::add_unchecked` (vertex.rs:366):

```rust
pub fn add_unchecked(&mut self, x: f64, y: f64, z: f64) -> VertexId {
    let id = self.next_id.fetch_add(1, Ordering::Relaxed);
    // ... push to SoA columns ...
    id
}
```

Identical pattern across `EdgeStore::add` (edge.rs:631), `FaceStore::add`
(face.rs:1149), `LoopStore::add` (loop.rs:750), `ShellStore::add`
(shell.rs:994), `SolidStore::add` (solid.rs:980). Each store carries
its own `next_id: AtomicU32` and counts upward forever.

### 2.3 Dedup paths that collapse identity

`VertexStore::add_or_find` (vertex.rs:249) and `add_or_find_with_dedup`
(vertex.rs:278) collapse spatially-coincident vertices into a single
ID by tolerance. **This is correct geometry, but it is not identity.**
Two logically distinct vertices that happen to be at the same point
get the same `VertexId`. Conversely, a vertex that moves by more than
tolerance during a parameter edit gets a new `VertexId` even though the
LLM thinks of it as the same vertex.

`EdgeStore::add_or_find` (edge.rs:773) and `add_or_find_with_dedup`
(edge.rs:794) do the same for edges (endpoint-pair match).

---

## 3. How operations destroy ID continuity

### 3.1 Boolean

`operations/boolean.rs::boolean_operation` (line 206) follows the
classical pipeline: face-face intersect → split faces along curves →
classify in/out/on → select → **`reconstruct_topology`** (line 232).

`reconstruct_topology` allocates fresh `FaceId`/`EdgeId`/`VertexId`/
`SolidId`s for the result. None of the input IDs are preserved. The
recorder block at line 234 captures `inputs: [solid_a, solid_b]` and
`outputs: [result_solid]` — only the top-level solid IDs, none of the
sub-topology.

### 3.2 Extrude

`operations/extrude.rs::extrude_face` (line 291) routes to one of:

- `create_unified_extrusion` (line 372) — fuses with parent solid
- `create_complex_unified_extrusion` (line 322) — draft/twist/taper
- `create_fresh_extrusion` (line 335) — no parent

All three synthesize new side faces, new top cap, new edges, new
vertices, and emit a fresh `SolidId`. The base face is consumed
(replaced inside the parent shell). The recorder gets
`inputs: [face_id]`, `outputs: [unified_solid_id]` — base FaceId in,
SolidId out, no continuity for the cap or sides.

### 3.3 Fillet / chamfer / boolean / sweep / revolve / loft

Same pattern by inspection (`operations/fillet.rs`,
`operations/chamfer.rs`, `operations/sweep.rs`, etc.): allocate fresh
output topology, record only the top-level (Solid|Face)Id pair.

### 3.4 Transform

`operations/transform.rs::transform_solid` is the one exception: it
mutates vertex coordinates in place. Vertex/edge/face IDs survive a
pure rigid transform. This works **only** because no topology is
synthesized.

---

## 4. What's actually needed

A persistent ID scheme MUST satisfy four invariants:

1. **Replay determinism.** Replaying the same timeline events against
   the same starting state produces the same persistent IDs, even
   though the underlying `u32` indices differ run-to-run.
2. **Edit stability.** Editing a parameter on an event (e.g., changing
   extrude distance from 10mm to 12mm) preserves the persistent IDs of
   topology whose **role** is unchanged. The "top cap of extrude X"
   stays "top cap of extrude X". Topology that genuinely disappears
   (e.g., a face consumed by a deeper boolean cut) gets a tombstone,
   not a recycled ID.
3. **Composability.** A persistent ID derived from a derived ID stays
   well-defined. "Top edge of fillet of top cap of extrude" must
   resolve to one stable PID for all agents to agree on.
4. **Privacy compatibility.** Persistent IDs are part of the .ros
   format (HIST chunk replay needs them); they MUST honor the AIPR
   privacy layer in `ros_format`.

Standard solution in the literature: **Kripac's persistent naming
scheme** (1995) — every topological entity gets a name derived
deterministically from (parent persistent name(s), operation kind,
role-within-operation). When an operation runs, it computes outputs'
PIDs from inputs' PIDs and a role descriptor. Replay with edited
parameters re-derives the same PIDs as long as the role descriptors
are stable.

---

## 5. Proposed integration seam (design only — not implemented)

### 5.1 New type

```rust
// primitives/persistent_id.rs (new file)
//
// A 128-bit persistent identifier for topological entities, derived
// deterministically from operation lineage. Stable across regeneration
// and parameter edits.
#[derive(Copy, Clone, Eq, Hash, PartialEq, Debug, Serialize, Deserialize)]
pub struct PersistentId(pub u128);

impl PersistentId {
    /// Mint a root PID (e.g. for primitive creation, where there are
    /// no parent topology PIDs to derive from).
    pub fn root(seed: &[u8]) -> Self { /* blake3(seed) → low 128 bits */ }

    /// Derive a PID from parent PIDs + operation tag + role.
    pub fn derive(parents: &[PersistentId], op_tag: &str, role: &Role) -> Self {
        // blake3(canonical_encoding(parents, op_tag, role))
    }
}
```

### 5.2 Sidecar map on BRepModel

```rust
// In BRepModel:
pub struct BRepModel {
    // ... existing fields ...
    pub vertex_pids: HashMap<VertexId, PersistentId>,
    pub edge_pids:   HashMap<EdgeId,   PersistentId>,
    pub face_pids:   HashMap<FaceId,   PersistentId>,
    pub solid_pids:  HashMap<SolidId,  PersistentId>,
    pub vertex_pid_inverse: HashMap<PersistentId, VertexId>,
    // ... mirror for edge/face/solid ...
}
```

Sidecar maps (not embedded in the SoA stores) preserve cache locality
of the existing `VertexStore` / `EdgeStore` / `FaceStore` columnar
layout. The PID lookup is a constant-time hashmap probe; not on the
inner-loop hot path of math ops.

### 5.3 Role enum

```rust
// operations/role.rs (new)
//
// Stable, serializable description of "which output is this" for
// every operation that synthesizes topology. Hashed into PersistentId.
#[derive(Clone, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum Role {
    // Extrude
    ExtrudeSide { base_edge_pid: PersistentId },
    ExtrudeCapStart, ExtrudeCapEnd,
    ExtrudeSideEdgeStart { base_vertex_pid: PersistentId },
    ExtrudeSideEdgeEnd   { base_vertex_pid: PersistentId },

    // Boolean
    BooleanFromA { source_face_pid: PersistentId },
    BooleanFromB { source_face_pid: PersistentId },
    BooleanCutEdge { face_a_pid: PersistentId, face_b_pid: PersistentId },

    // Fillet
    FilletRoll  { source_edge_pid: PersistentId },
    FilletSeamA { source_edge_pid: PersistentId, on_face: PersistentId },
    FilletSeamB { source_edge_pid: PersistentId, on_face: PersistentId },

    // Sweep / loft / revolve / chamfer / pattern / mirror — same shape.

    // Primitive root (no parents)
    Root { kind: PrimitiveKind, key: String },
}
```

The `key` discriminator on `Root` lets us mint multiple distinct PIDs
for primitives created with the same parameters in the same timeline
(e.g. two boxes of size 1.0 at different positions get different roots
because the timeline event ID is in the key).

### 5.4 Integration point: OperationRecorder

`operations/recorder.rs::RecordedOperation` already carries
`inputs: Vec<u64>` and `outputs: Vec<u64>`. Extend with:

```rust
pub struct RecordedOperation {
    pub kind: String,
    pub parameters: serde_json::Value,
    pub inputs:   Vec<u64>,
    pub outputs:  Vec<u64>,
    // New:
    pub input_pids:  Vec<PersistentId>,
    pub output_pids: Vec<PersistentId>,
    pub output_roles: Vec<Role>,  // 1:1 with output_pids
}
```

The kernel side computes output PIDs as `PersistentId::derive(input_pids,
&kind, &role)` for each output during the operation. The timeline
recorder serializes both sets, and replay re-derives.

### 5.5 Replay semantics

When `Timeline::replay` rebuilds a model from events:

- Each event's `output_pids` are the truth: the rebuilt model's
  topology gets these PIDs assigned in the order outputs are produced.
- If the operation is **deterministic** in role assignment (and it must
  be — that's the whole point), the rebuilt PIDs will be identical to
  the original recorded ones. We log a divergence diagnostic (don't
  fail) when they don't match — that's a kernel bug, not a runtime
  error.

---

## 6. Operations that currently break ID continuity

Concrete list of every operation that mints fresh u32 IDs without a
preservation strategy. Each will need PID lineage logic.

| Operation                  | File                              | Output PID derivation needed for                       |
|----------------------------|-----------------------------------|--------------------------------------------------------|
| `boolean_operation`        | `operations/boolean.rs:206`       | All result solid faces, edges, vertices                |
| `extrude_face`             | `operations/extrude.rs:291`       | Side faces, cap face(s), side edges, top vertices      |
| `extrude_profile`          | `operations/extrude.rs:762`       | Same as above + bottom cap face                        |
| `revolve_face`             | `operations/revolve.rs`           | Lateral surface, cap(s), seam edge                     |
| `sweep_profile`            | `operations/sweep.rs`             | Side surfaces, caps, rail-aligned edges                |
| `loft_profiles`            | `operations/loft.rs`              | Side surfaces, cap(s)                                  |
| `fillet_edges`             | `operations/fillet.rs`            | Roll surfaces, seam edges, modified neighbor faces     |
| `fillet_robust`            | `operations/fillet_robust.rs`     | Same as above                                          |
| `chamfer_edges`            | `operations/chamfer.rs`           | Bevel face, seam edges                                 |
| `pattern_*`                | `operations/pattern.rs`           | Each instance derives PIDs from (source PID, index)    |
| `mirror`                   | (in `transform.rs`)               | Mirrored topology derives PIDs from (source PID, mirror_axis) |
| `offset`                   | `operations/offset.rs`            | Offset surfaces                                        |
| `imprint`                  | `operations/imprint.rs`           | New imprint edges, split sub-faces                     |
| `sew`                      | `operations/sew.rs`               | Merged edges (which input wins is the role)            |
| `delete`                   | `operations/delete.rs`            | Tombstones — PID survives in lineage but resolves to None |
| `g2_blending`              | `operations/g2_blending.rs`       | Blend faces, seam edges                                |
| `face_arrangement`         | `operations/face_arrangement.rs`  | Re-partitioned faces                                   |
| `blend`                    | `operations/blend.rs`             | Blend surfaces                                         |
| `draft`                    | `operations/draft.rs`             | Modified faces (PID survives — same logical face)      |
| `project`                  | `operations/project.rs`           | Projected curves on target faces                       |
| `modify`                   | `operations/modify.rs`            | Depends on sub-op                                      |
| `transform_solid/faces/edges` | `operations/transform.rs`      | **No new PIDs** — pure mutation of geometry            |

Primitive constructors that mint root PIDs:

| Constructor                       | File                          |
|-----------------------------------|-------------------------------|
| `TopologyBuilder::create_box_3d`  | `primitives/topology_builder` |
| `create_sphere_3d`                | (same)                        |
| `create_cylinder_3d`              | (same)                        |
| `create_cone_3d`                  | (same)                        |
| `plane_primitive`                 | (same)                        |
| `create_point_2d` / `create_line_2d` / `create_circle_2d` / `create_rectangle` | (same) |

Total: ~22 operation entry points + ~9 primitive constructors that
need lineage wiring.

---

## 7. Implementation sequencing (next slices, not this audit)

This audit produces no code. The sequencing of follow-up slices is:

1. **Slice 40-A — types only.** Add `PersistentId`, `Role`, sidecar
   maps on `BRepModel`. No call-site wiring. Tests: PID hash
   determinism, role serialization round-trip.
2. **Slice 40-B — primitive roots.** Wire root PID minting into the
   ~9 primitive constructors. Each gets a deterministic key from the
   timeline event ID + parameters. Tests: same primitive twice in
   the same timeline → different PIDs; same primitive with same key
   → same PID across replay.
3. **Slice 40-C — extrude lineage.** Wire `extrude_face` and
   `extrude_profile` to derive output PIDs from input PIDs + roles.
   Smallest non-trivial op; sets the pattern. Tests: extrude twice,
   edit distance, verify side-face PIDs are stable; verify cap-face
   PID stable.
4. **Slice 40-D — boolean lineage.** Wire `boolean_operation`. The
   hard one because faces split. Each split sub-face's role carries
   the parent face PID + the cutter face PID. Tests: union, edit
   tool size, verify the surviving original-face PIDs.
5. **Slice 40-E — fillet/chamfer/revolve/sweep/loft.** Mechanical
   port of the Slice 40-C pattern.
6. **Slice 40-F — pattern/mirror/offset/imprint/sew.** Same.
7. **Slice 40-G — recorder + timeline replay.** Extend
   `RecordedOperation` and timeline replay to thread PIDs end-to-end.
   This is what unlocks edit-stability for agents.
8. **Slice 40-H — readable surface.** Extend `readable/query.rs` to
   accept PID-anchored queries. Optional UI: PID inspector in
   ModelTree.

Each slice is independently shippable, gated by tests, and leaves the
kernel in a working state.

---

## 8. Out of scope for this audit

- Implementation. This is the design gate.
- Wire-format (.ros) extension for PIDs. That's slice 40-G.
- Integration with the constraint solver in sketch2d. PIDs are 3D
  topology only; sketch2d entities have their own ID stream and
  don't need this scheme today.
- Performance benchmarking. PID maps add O(1) overhead per topology
  lookup; not on the math hot path. Defer benchmarks until 40-D.
- Migration of existing .ros files. Old files have no PIDs; they
  load with `face_pids = {}` and operations re-mint roots on first
  edit. No lossy migration needed.

---

## 9. Open questions (answer before implementation)

1. **PID size.** 128 bits gives ~2^64 collisions per timeline before
   birthday-bound risk. 64 bits would halve sidecar memory. Pick
   based on profiling 40-A. Tentative: 128 bits.
2. **Hash function.** `blake3` (already in workspace? grep) vs
   `xxhash`. Determinism + endian-stability matter; collision
   resistance is nice-to-have. Tentative: `blake3` for the strong-
   determinism guarantee, fall back to `xxhash` if blake3 isn't
   already in the dep tree.
3. **Tombstones.** When an operation deletes a PID's topology, does
   the PID resolve to `None` or to `Some(Tombstone { reason })`?
   Tentative: `Tombstone` so agents can tell "deleted by op X" from
   "never existed".
4. **Sketch2d → 3D promotion.** When a sketch profile is extruded,
   the sketch's 2D entities get promoted to 3D vertices/edges/faces.
   Should those carry the sketch entity's PID forward? Tentative:
   yes, with a `PromotedFromSketch { sketch_id, entity_id }` role
   wrapper.

These get answered in the slice 40-A PR, not this audit.

---

## 10. References

- Kripac, J. (1995). *A Mechanism for Persistently Naming Topological
  Entities in History-Based Parametric Solid Models*. Proc. Solid
  Modeling.
- Hoffmann, C. (1989). *Geometric and Solid Modeling*, Ch. 7
  (history-based modeling).
