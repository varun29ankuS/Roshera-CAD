# Fast, exact certificate: architecture and ranked plan

Date: 2026-11-17. Analysis and design only. Nothing was built, run, or edited; every timing below is

> **Status 2026-11-22:** ranked items 1–4 landed on 2026-11-17 — 98a88172 (quality pass projects
> once, per face in parallel), cc0e6fb2 (one mesh per certificate), 9258c2e3 (manifold analysis by
> sorting), c68aafc4 (import edge rebuild in parallel, applied in order). Items 5–10 (part-relative
> element size, a deflection gate, self-intersection on the certificate mesh, `CertMesh`, per-face
> cache, analytic distances) have no landing commit; item 6 is tracked in
> `docs/workplan-2026-11-22-status.md`, the rest in `PARKING_LOT.md`.
either the measurement supplied with the task or an estimate derived from it, and is labelled.

Labels used throughout:

- **[V]** verified by reading the code at the cited location.
- **[M]** measured (supplied: release build, CADGenBench parts 208 and 211).
- **[I]** inferred / estimated. Needs a measurement before it is believed.

Paths are relative to `roshera-backend/`.

---

## 0. The five findings that drive everything

1. **The mesh density is size-blind, and the chord tolerance is not what sets it.** [V]
   `TessellationParams::default()` has `max_edge_length: 0.1` (absolute mm) and `max_segments: 100`
   (`geometry-engine/src/tessellation/mod.rs:40`). In `arc_steps_for_quality` and
   `linear_steps_for_quality` (`tessellation/surface.rs:41`, `:93`) the result is the MAX of the
   sagitta, edge-length and angle counts, capped at `max_segments`. Edge length 0.1 mm beats the
   sagitta count for every radius above about 0.02 mm at chord 0.1, so any face direction longer than
   10 mm is pinned at the 100-segment cap. That is why chord 0.1 and chord 0.001 give EXACTLY
   3,364,271 triangles [M]: about 6,700 triangles per face regardless of what the face is. A cylinder
   50 mm tall gets 100 rows where 1 is geometrically exact.

2. **The same mesh is built twice inside one certificate, and again by the server.** [V]
   `compute_certificate_from_validation` (`primitives/topology_builder.rs:3178`) calls
   `manifold_and_shell_orientation_report` (chord 0.1, otherwise default; `harness/watertight.rs:383`)
   and then `tessellate_solid(.., TessellationParams::default())` at `:3272`. `tessellate_solid`
   floors the chord at `5e-4 x extent` (`tessellation/mod.rs:326`), so for any part with extent
   >= 200 mm the two parameter sets are bit-for-bit the same struct and the two meshes are the same
   mesh. Below 200 mm they differ only in a chord that does not bind (finding 1). The api-server then
   meshes cold solids at `default()` again (`api-server/src/main.rs:7945`).

3. **The quality pass projects every facet corner twice, serially, and mostly for nothing.** [V]
   `mesh_quality_with_normals` (`harness/watertight.rs:1158`) first calls `measure_face_u_spans`
   (`:1074`, three `project_uv` per facet of every periodic face), then in its own serial loop runs
   three more `project_uv` per facet (`:1290-1294`) BEFORE asking whether the surface is periodic
   (`:1300`). The projected values are used only inside `if let Some(p) = surface.period_u()`. On a
   plane or a non-periodic NURBS face all three projections are discarded. A vertex is shared by
   about six facets, so each vertex is projected about twelve times. Every facet also takes two to
   four `DashMap` lookups (`model.faces.get`, `model.surfaces.get`).

4. **The manifold analysis is four `HashMap`s over ten million directed edges.** [V]
   `manifold_report_mesh` (`harness/watertight.rs:180`) builds `weld_map`, `directed`, `edge_face`
   and `undirected` with the default SipHash hasher, single-threaded. That is the 4.1 s [M].

5. **The certificate has no deflection (distance) gate today.** [V] See section 1.4. "Tessellation
   faithful to the true surfaces" is currently a statement about facet NORMALS, not about how far a
   facet sits from the surface. This matters for the plan: a coarser mesh does not weaken a distance
   promise, because there is none, and the plan adds one.

A sixth, smaller fact: the task statement says the edge rebuild doubles samples up to 1025; the code
says `EDGE_REBUILD_MAX_SAMPLES: usize = 513` (`export-engine/src/formats/step/mod.rs:634`) [V].

---

## 1. What each check actually needs

### 1.1 Watertight, manifold, oriented, Euler

What it is today [V]: a property of the WELDED TRIANGLE MESH. `manifold_report_mesh` welds by a 1e-6
lattice, counts triangles per undirected edge (1 = open, 3+ = fan) and repeats of a directed edge
(flip). `closed` additionally requires zero `weld_refusals`.

What decides the verdict: the mesh of one face is a triangulated region. Its interior edges are each
used twice inside that face, in opposite directions, if the face tessellator is correct. Its rim
edges are the segments of the shared `EdgeSampleCache` chains (`tessellation/edge_cache.rs:85`),
which every face on a B-Rep edge reads identically. So the global verdict decomposes exactly into:

- (a) per face: every interior mesh edge is used twice with opposite direction, and the rim of the
  face's mesh is exactly its loops' sample chains, each segment once;
- (b) per B-Rep edge: its chain is walked by exactly two face-uses, in opposite directions;
- (c) no weld-lattice key is shared between vertices that (a) and (b) treat as distinct (two faces
  touching away from a shared edge).

Interior density does not enter (a)-(c) at all. A 3.4 M-triangle mesh is not needed; any
boundary-conforming mesh gives the same answer PROVIDED the tessellator behaves the same at both
densities. That proviso is real: the code records density-dependent tessellator defects (the
`REVOLVE-TESS-SEAM` note at `tessellation/mod.rs:319`; "a scribble can be density-dependent" at
`watertight.rs:545`). So the honest rule is:

> The mesh verdict belongs to (B-Rep, tessellator, parameters). Certify the mesh that ships, at the
> parameters it ships at, and key the verdict by those parameters. A mesh exported at other
> parameters is a different mesh and gets its own verdict.

What a coarser mesh could MISS, and what keeps the promise:

| Could be missed | Why | Kept by |
|---|---|---|
| A tessellator fault that only appears at a finer density | per-family code paths in `surface.rs` | certify the shipped parameters; export at other parameters re-certifies (verdict keyed by params) |
| A feature thinner than the weld cap | unchanged: `MESH_WELD_CAP` 4e-6 is absolute and density-independent [V] | nothing changes |
| An edge off its faces | never a mesh property; `validate_solid_scoped` judges it on the B-Rep [V] | nothing changes |

Can it be decided with no interior meshing? Part (b) yes, on the B-Rep plus edge chains. Part (a)
no: it is a claim about what the face tessellator emitted, so the face must be meshed, but once, at
the shipped density, and checked locally.

### 1.2 Shells outward

Today [V]: `misoriented_shells` (`watertight.rs:419`), sign of the per-shell divergence sum with a
1e-6 relative margin. A sign needs only a closed, consistently oriented mesh. For a mesh within
deflection `d` of the surface, `|V_mesh - V_true| <= d x Area`. Guard to add: if
`|V_signed| <= d x Area` the sign is not established by this mesh; refine that shell (halve `d`)
rather than report. On real parts `|V|` exceeds `d x Area` by orders of magnitude, so this never
fires, and when it does the answer is refined instead of guessed.

### 1.3 Self-intersection

Today [V]: a SEPARATE mesh at chord 0.5 with `audit()` (24 segments), spatial-hash broad phase,
segment/triangle narrow phase, pairs sharing a welded vertex skipped, coarse crossings confirmed at
chord/16 on up to 16 faces (`harness/self_intersection.rs:292`, `:398`). The confirmation removes
false positives. Nothing removes false NEGATIVES: two faces that interpenetrate by less than the sag
of the 24-segment facets need not cross as meshes. So today's promise is "no gross self-overlap
visible at chord 0.5", and the error string says so ("coarse-chord").

What is needed for an exact statement: a mesh with a KNOWN two-sided deflection bound `d` (every
surface point within `d` of its face's mesh, every mesh point within `d` of the surface). Then for
two non-adjacent faces:

- mesh distance > 2d  =>  the true surfaces are disjoint. This is a proof, not a heuristic.
- meshes cross        =>  candidate; confirm on those faces at finer `d` (as today).
- 0 < distance <= 2d  =>  undecided at this density; refine ONLY those face pairs (halve `d`,
  re-test), or decide analytically when both are quadrics.

This is strictly stronger than today (it closes the shallow-penetration blind spot) and needs no
second mesh: it runs on the one certificate mesh, whose `d` section 1.4 establishes. Refinement is
confined to the few face pairs that are genuinely close.

Unchanged limitation, stated rather than hidden: triangle pairs sharing a welded vertex are skipped,
so a fold between ADJACENT faces right at their shared edge is not this check's to find.

### 1.4 Tessellation fidelity and mesh quality

What gates soundness today [V] (`primitives/provenance.rs:395`, `:520`):

- `tessellation.clean`: zero triangles with `|cross| <= 1e-12`; stored-normal agreement >= 0.98;
  zero facets whose winding normal is in the wrong hemisphere of the true surface normal at the
  centroid footpoint; analytic agreement >= 0.999.
- `mesh_quality.clean`: zero facets bridging a periodic face (`is_bridging_facet`); max deviation
  between facet normal and true normal <= 40 degrees, where a facet over 40 degrees counts only if
  its centroid is ALSO farther than the chord floor from the surface (`leaves_surface`,
  `watertight.rs:1199`).
- Aspect ratio and minimum angle are readouts, not gates.

So the only place a DISTANCE is measured is to excuse an over-40-degree sliver. There is no
"every facet is within X of the surface" gate. The tessellator aims at chord
`max(0.001, 5e-4 x extent)` and angle 0.1 rad, but the certificate does not verify that it got there.

What each of these needs:

- Hemisphere / 40-degree / bridging checks are per-facet and density-independent in MEANING: a
  correct facet passes at any density. A coarser mesh has larger facets, so a facet's normal
  deviates more from the true normal at its centroid: at angle step `a` the worst deviation of a
  correct facet is about `a/2` per direction. With `max_angle_deviation = 0.1` rad that is about
  3 to 4 degrees, far below 40. The gate keeps its margin as long as the angle bound stays; this
  plan does not touch the angle bound.
- A deflection bound is the thing a coarser mesh genuinely affects, and the thing to add:

  **Promise to add:** every facet of the certified mesh lies within `d_cert` of its face's surface,
  where `d_cert = max(chord_tolerance, 5e-4 x extent)`, which is exactly the chord the tessellator
  is already asked for [V]. Relative to part size by construction (0.05 % of the diagonal), absolute
  in the report (the number in mm is printed). For reference, the CADGenBench gate meshes at
  0.1 % of the diagonal clamped to [0.005, 0.5] mm and 0.5 rad: this promise is twice as tight in
  deflection and five times as tight in angle.

  How to make it exact, not sampled, at no meaningful cost:
  - Plane: deflection 0 by construction.
  - Sphere radius R, vertices on the sphere: the farthest point of a flat triangle from the sphere
    is at its circumcentre (or the midpoint of its longest edge if obtuse); deflection =
    `R - sqrt(R^2 - rho^2)`, `rho` = circumradius or half longest edge. Closed form, three vertices.
  - Cylinder / cone: the sagitta over the facet's u-extent at its largest radius,
    `r_max (1 - cos(du/2))`; the ruling direction contributes 0.
  - Torus: bound by the two principal sagittas over the facet's (du, dv) extent.
  - NURBS / revolution / ruled: measure at the centroid (the footpoint is ALREADY found there for
    the normal check, so the distance is free) and at the three edge midpoints, and add the
    second-order bound `L^2 x kappa_max / 8` from the surface's own curvature over the facet. A
    facet whose measured + bounded deflection exceeds `d_cert` is refined, not reported clean.

  This is "coarse mesh plus an exact per-face check, refining only where a check is near its
  threshold". It converts an unverified tessellator target into a certified number.

### 1.5 Display and mass properties

Display must be the certified mesh, otherwise the certificate describes a mesh nobody looks at
(the code's own reason for meshing at `default()` at `:3262`). Mass properties use
`TessellationParams::mass_properties` (`fine()`: angle 0.02, up to 200 segments, edge length floored
at 0.5 % of the part; `tessellation/mod.rs:129`) because integration needs a finer ANGLE bound than
display. That mesh is not the certificate mesh and should not become it; it is not on the measured
import+certify path [I: no mass-properties call was found in `compute_certificate_from_validation`;
`construction_consistency` and `recognize_features` were not opened]. Leave it separate and lazy.

---

## 2. One mesh, computed once: the certificate mesh

One structure built once per solid, read by every stage, and handed to the display path.

```
CertMesh {
    params: TessellationParams,        // AFTER the extent floor - the key of the verdict
    d_cert: f64,                       // the deflection every facet is certified within
    faces: Vec<FaceBlock>,             // shell order, as tessellate_shell_with emits them
    weld: Vec<u32>,                    // local vertex -> welded id (the existing weld's remap)
}
FaceBlock {
    face_id: u32,
    key: u128,                         // content hash, section 5
    vertices: Vec<MeshVertex>,         // PRE-weld, so uv is always THIS face's chart
    triangles: Vec<[u32; 3]>,          // local indices
    rim: Vec<(EdgeId, bool, Range)>,   // which local vertices are which edge chain, and direction
    true_normal: Vec<Option<Vector3>>, // per facet, at the centroid footpoint
    deflection: Vec<f32> or max only,  // per facet (or just the per-face maximum)
    summary: FaceCertSummary,          // counts for tessellation/mesh quality, u-span, rim check,
                                       // per-face signed-volume and area sums, AABB
}
```

Why each field removes work that exists today:

- **uv in the face's own chart, pre-weld.** The whole reason `project_uv` exists is that the shell
  weld leaves a shared rim vertex carrying ONE face's uv (`watertight.rs:1278` comment [V]). Inside
  `tessellate_shell_with` each face is meshed into its own `part` mesh before the weld
  (`tessellation/mod.rs:417` [V]); at that moment every vertex has its own face's parameters. Keep
  the blocks instead of discarding them and the uv never has to be recovered by projection.
  Where a face tessellator emits `uv: None`, project once per VERTEX in the block, not per facet
  corner.
- **true normal and deflection per facet**, computed in the same per-face rayon task that meshed
  the face (the surface is already in hand; `facet_true_normals` already runs per face-run in
  parallel [V]). Both quality checks and the new deflection gate read them.
- **rim** makes check (a)/(b) of section 1.1 a comparison of small arrays and is what lets a face be
  re-certified alone (section 5).
- **summary** is what the solid-level verdict folds: the solid-level `TessellationQuality` and
  `MeshQuality` are sums / maxima / worst-face selections over faces, which is how the existing code
  already accumulates them (`per_face` maps [V]).

`TriangleMesh` (the display/export type) is produced from the blocks by the existing concatenate +
weld step, unchanged, so display output is byte-identical to today's for the same parameters. The
`CertMesh` is parked on the solid next to `cached_certificate` (as an `Arc`) so the server's
`tessellate_solids(.., default())` at `api-server/src/main.rs:7945` becomes a lookup for a solid
that was just certified.

Stage order with one mesh:

```
B-Rep validation (unchanged)
build CertMesh           per face, parallel: mesh, uv, true normals, deflection, local topology,
                         quality counts
fold                     O(faces): quality verdicts, deflection verdict, shell signed volumes
rim matching + weld check  O(boundary samples): watertight / manifold / oriented / Euler
self-intersection        BVH over face AABBs then triangles, on the same mesh, with the 2d proof
construction / eyes / labels (unchanged)
```

---

## 3. Parallelism and memory

Safe to run in parallel (read-only on `BRepModel`, which the tessellator already reads from rayon
workers [V: `tessellate_shell_with`, `facet_true_normals`]):

| Work | Unit | Determinism rule |
|---|---|---|
| Face meshing | per face | already parallel, results appended in shell order [V] |
| True normals, deflection, quality counts | per face (not per facet: seeds chain facet to facet within a face [V]) | walk each face's facets in order inside the task; fold faces in face order |
| Manifold analysis | parallel sort of packed edge keys | sort is total, output order-independent |
| Self-intersection narrow phase | per broad-phase bucket / BVH node pair | collect, sort pairs, then truncate to the cap (today the cap cuts a `HashMap` iteration, which is already unordered [V]) |
| Import repairs | per edge, compute only | compute in parallel, APPLY serially in edge-id order (section 4) |
| Solids | per solid | already bounded at `CERTIFY_BATCH_THREADS = 4` [V] |

Not safe / keep serial: the weld (`weld_mesh_watertight_range`: "lowest index wins" depends on scan
order [V]); the application of repairs to the model (curve ids are allocated in order).

Memory, part 208 today [I, from struct sizes]: `MeshVertex` is 72 bytes (Point3 + Vector3 +
`Option<(f64, f64)>`). 1.83 M vertices = 132 MB, 3.36 M triangles = 40 MB, face map 13 MB: about
185 MB per mesh, briefly doubled while per-face parts are concatenated. Two such meshes are alive at
different times. The manifold `HashMap`s add roughly 600 MB transient (10 M directed entries x 2
maps, 5 M undirected, 1.8 M weld keys). `true_normals` is 32 bytes x 3.36 M = 108 MB. Peak is about
1 GB for this one solid; four such solids in flight (the batch limit) is about 4 GB, which is the
whole free memory of this machine on a normal day.

Budget to adopt:

- Admission by size, not by count: estimate a solid's triangles before meshing (segment counts per
  face are cheap to compute from the edge chains) and admit solids into the batch while the sum of
  `estimated_triangles x 320 bytes` stays under a fixed budget (1.5 GB is safe here). One large
  solid alone always runs.
- Sort-based manifold analysis needs 10 M x 12 bytes = 120 MB instead of 600 MB.
- After item 5 below the mesh is 8 to 20 times smaller and none of this binds; the budget stays as a
  guard for the pathological part.

---

## 4. The import repairs (37.7 s on part 208)

First, a gap in the evidence: `ROSHERA_IMPORT_PROFILE` times parse, index, build geometry and
assembly placement, but there is no timer around any of the six repair passes
(`export-engine/src/formats/step/mod.rs:181-186` [V]). Which pass owns the 37.7 s is not known. The
analysis below is by reading the cost structure; item 4a is to measure it.

Cost structure of `rebuild_edges_onto_face_intersections` (`:785`) [V]:

- Every two-face edge is probed: 33 samples x 2 global `closest_point`.
- Every edge with `before > max(declared, 1e-7)` and within reach is attempted. One attempt at `n`
  samples costs, per interior sample: `foot(sa)`, `foot(sb)` (in `gap_of`-style selection), then
  `foot(sa)` and `foot(sb)` AGAIN to pick the home face (`:892`), then `home.closest_point` a third
  time (`:897`) - five global projections where two distinct results exist - plus a golden-section
  search whose interval shrinks from 0.2 to 1.1e-13, i.e. about 59 iterations, each a `point_at` and
  a global `closest_point` on the other surface. Then `4(n-1)+1` verification points x 2 projections.
- Doubling 33 -> 65 -> 129 -> 257 -> 513 recomputes every sample each time: 997 samples x about 64
  projections = about 64,000 projections for one stubborn edge.
- All of it is one serial loop over edges.

The rebuilt point of a sample depends on its neighbours at the CURRENT resolution (`along =
next - prev`, `:901`), so results from the previous resolution cannot be reused bit-for-bit, and the
decision "which edges get repaired" hangs on comparing `after` against thresholds. That fixes what is
allowed:

Changes that cannot alter any result (bitwise identical healings and curves):

1. **Compute in parallel, apply in order.** Split the loop: a `par_iter` over the candidate edges
   computes `Option<(curve, k0, k1, before, after)>` reading the model only; a serial loop in
   edge-id order (the existing `BTreeMap` order) then does `model.curves.add`, the edge mutation and
   the report/Lens entries. An edge's computation reads its own edge, its two SURFACES and its
   vertex positions; no rebuild changes a surface or a vertex, so the computations are independent
   of each other's writes [V from the mutation list at `:1024-1035`]. Curve ids come out identical
   because the application order is unchanged.
2. **Call each projection once.** `foot(sa, p)` and `foot(sb, p)` are pure; keep their results
   instead of recomputing, and take `(u0, v0)` from the home foot instead of a third
   `closest_point`. Same values, 40 % fewer projections outside the golden section.
3. **Same split for `raise_edge_tolerances_to_measured_gaps`** (`:1069`): per face in parallel,
   merged in the store's face order (the gap is a max, the per-edge face list keeps its order), and
   for `fit_linear_extrusions_to_faces` (`:686`): per surface in parallel, `surfaces.replace` in
   order.

Changes that are exact in verdict but not bit-identical (need the equivalence test in section 6):

4. **Closed-form distance for analytic surfaces.** `foot` goes point -> (u, v) -> `point_at` ->
   distance. For plane, sphere, cylinder, cone and torus the distance is a direct expression
   (`| |p - c| - R |` for a sphere) with no trigonometry. Values agree to rounding, not to the bit.
   This is the largest single-thread saving on a part of trimmed spheres.
5. **Seeded projection along the edge for NURBS surfaces.** Consecutive samples are neighbours, so
   `closest_point_near` from the previous foot, accepted only if it passes the existing
   `is_footpoint`-style test and otherwise falling back to the global search, replaces most global
   searches. This is the pattern `facet_footpoint` already uses (`watertight.rs:831` [V]). It can
   differ from the global answer only where another sheet of the same surface is closer than the
   local foot, i.e. a surface that nearly touches itself within the 0.1 mm walk.
6. **Do not start what the keep-rule will reject.** The result is kept only if
   `after <= 0.1 x before` or `before > 0.01`, and `after <= 0.01` (`:1020`). Any pre-check must be
   a proven bound, not a guess; the existing `rebuild_cannot_reach_cap` is the model. One more such
   bound is available: when both surfaces are analytic and the edge curve is analytic, `before` can
   be evaluated in closed form at far more than 33 stations at lower cost, which tightens nothing
   and changes nothing but removes the probe cost.

Not proposed: stopping the doubling early when the error stops improving, or replacing golden
section with a faster minimiser. Both change `best`, therefore can change which edges are kept.

---

## 5. Incremental certification by content hash per face

Yes, with one audit to do first.

Key of a face block:

```
H( surface definition bits, face orientation,
   for each loop, for each edge-use: H(the edge's sample chain as f64 bits), direction,
   effective TessellationParams bits (after the extent floor) )
```

Hashing the edge SAMPLE CHAIN rather than the edge definition is deliberate: an edge's sampling
depends on the faces on both sides of it (`EdgeSampleCache::adjacency_index`,
`edge_cache.rs:114` [V]), so the definition alone does not determine what the face tessellator
receives; the chain does. Chains are cheap next to face meshing, so they are always recomputed, and
the face block is looked up by what it would be given.

Why a hit is exact: the face tessellator is documented as reading "only the model and the
edge-sample cache" (`tessellation/mod.rs:396` [V, the comment]). If a face's output is a pure
function of (its surface, its orientation, its loops' chains, the parameters), an equal key means an
equal block. **To audit before relying on it [I]:** the conforming blend/fillet/torus-band paths in
`surface.rs` (`tessellate_blend_cone_conforming`, `tessellate_fillet_face_conforming`,
`tessellate_blend_torus_conforming`, `tessellate_cylinder_edge_band_conforming`) may read a
NEIGHBOUR face's surface. Any that do must add what they read to the key. The test in section 6
(cache hit vs fresh mesh, bitwise) finds any that were missed.

What is cached per face: the `FaceBlock` (mesh, uv, true normals, deflection, summary, AABB).
What is redone on every re-certification, all cheap:

- rim matching and the cross-face weld check: O(boundary samples);
- shell signed-volume: O(faces), from per-face sums taken about a fixed reference;
- self-intersection: only pairs that include a touched face (the BVH over face AABBs gives them);
- B-Rep validation, construction, eyes, labels: unchanged, not mesh work.

Two things that would silently defeat the cache, and the fix:

- The chord floor is `5e-4 x extent` and the extent changes with almost any edit, so every face's
  key would change. Quantise the floor to a geometric ladder (powers of 2^(1/2) of the extent, taken
  downward so the mesh is never coarser than promised). A small edit then keeps the key; `d_cert`
  is the quantised value and is what the certificate states.
- Replay on another machine may not reproduce floats bit-for-bit (the project's own note: "floats
  not bit-stable"). That yields a miss and a recompute, never a wrong hit. The cache is an
  in-process / per-document acceleration, not part of the stored certificate.

Where it lives: beside `Solid::cached_certificate`, in a model-level `DashMap<u128, Arc<FaceBlock>>`
with an LRU bound in bytes. The existing `tessellation/cache.rs` keys by ENTITY ID plus params hash
(`CacheKey`, `:15` [V]) and so cannot detect a changed face; it is not the right key and is not
reused.

---

## 6. Ranked plan

Savings: part 208 from the supplied measurements; part 211 scaled from its stage times (manifold
46 s, self-intersection 4.6 s, display mesh 27 s, quality 67 s, certify 148 s). 208 today:
certificate stages sum to about 34 s, repairs about 37 s.

### 1. Quality pass: project once per vertex, only where it is used, per face in parallel

- **Change:** in `mesh_quality_with_normals` and `measure_face_u_spans`
  (`geometry-engine/src/harness/watertight.rs:1158`, `:1074`): (i) look up face and surface once per
  face, not per facet; (ii) skip the corner projections entirely when `surface.period_u()` is
  `None`; (iii) for periodic faces project each distinct vertex of the face once and share the
  result between the span measurement and the bridging test; (iv) run faces in parallel, facets in
  order within a face, fold in face order.
- **Why exact:** (ii) the projected values are read only inside the `period_u()` branch [V], and
  `crossing_seed` feeds nothing else. (iii) `project_uv` is a pure function of (surface, point,
  seed); for a vertex with a stored uv the seed is that uv, so every one of its twelve calls returns
  the same value. Vertices with `uv: None` take a seed from the previous projection on the face, so
  those keep their original order within the face. (iv) per-face accumulators are independent; the
  solid-level maxima and the worst-face sort are order-independent given the same per-face values.
- **Saving [I]:** 208: 9.2 s -> about 0.5 s (-8.7 s). 211: quality 67 s -> about 6 to 10 s
  (-55 to -60 s).
- **Proof:** `MeshQuality` (all fields including `defects` and `worst_face`) compared `==` between
  the old and new function on every solid of the corpus, kept as a test with the old function
  retained under `#[cfg(test)]`. Mutation check: make the dedupe key ignore the face and the test
  must go red on a welded rim vertex (the torus-rim case named at `:1278`).

### 2. One mesh per certificate

- **Change:** `compute_certificate_from_validation` (`primitives/topology_builder.rs:3178`) builds
  ONE mesh at the display parameters and gives it to `manifold_report_mesh`, `misoriented_shells`,
  `facet_true_normals` and both quality checks. `manifold_and_shell_orientation_report` gains a
  `_of_mesh` sibling (the report half already takes a mesh). The mesh is then parked on the solid
  for `api-server/src/main.rs:7945`.
- **Why exact:** when extent >= 200 mm the floored parameter structs are identical, so it is the
  same mesh by construction [V]. Below that the chord differs (0.1 vs the floor) but does not bind
  [M: identical triangle count; V: edge length dominates]. Where the two meshes are NOT bit-equal on
  some part, this is a deliberate, stated change: the topology verdict moves to the mesh that ships,
  which is the stronger claim. `operations/loft.rs:2305` calls `manifold_report` with the shared
  constants and keeps doing so; its doc comment's promise ("a loft can never mint a solid whose
  certificate then reads oriented = false") must be re-pointed at the display parameters in the same
  change, or it becomes false.
- **Saving [M-derived]:** 208: -7.7 s. 211: -27 s. Plus one more mesh avoided in the server.
- **Proof:** (a) a test that hashes vertices + triangles + face map of the two meshes for every
  corpus solid and reports which are bit-identical; (b) identical `ManifoldReport` and
  `misoriented_shells` on all of them; (c) identical certificate verdicts corpus-wide.

### 3. Manifold analysis by sorting

- **Change:** `manifold_report_mesh` (`harness/watertight.rs:180`): weld by sorting
  `(lattice key, vertex)`; pack each directed edge as `u64`, parallel-sort with its face, run-length
  scan for directed repeats, then the same for undirected keys; union-find unchanged.
- **Why exact:** same definitions, different container. One trap: welded ids are assigned in
  first-seen order today (`weld_map.len()`), and no reported field depends on the numbering
  (counts, per-face counts, Euler, components), so any consistent numbering gives the same report.
- **Saving [I]:** 208: 4.1 s -> about 0.5 s (-3.6 s). 211: about -12 to -15 s. Memory -480 MB.
- **Proof:** `ManifoldReport` field-for-field equal on the corpus AND on the existing injected-defect
  meshes (open edge, fan, flipped facet), which is where a container bug would hide.

### 4. Import repairs: measure, then compute in parallel and apply in order

- **Change:** (a) a timer around each of the six passes under `ROSHERA_IMPORT_PROFILE`
  (`export-engine/src/formats/step/mod.rs:181-186`); (b) `rebuild_edges_onto_face_intersections`
  split into parallel compute / serial apply, and each projection called once (section 4, items
  1-2); (c) the same split for `raise_edge_tolerances_to_measured_gaps` and
  `fit_linear_extrusions_to_faces`.
- **Why exact:** section 4, items 1-3.
- **Saving [I]:** if the rebuild owns the time, 208: 37 s -> 5 to 7 s on this 12-thread machine
  (-30 s); 211: import-only time (33 to 200 s) cut by the same factor. Unknown until (a).
- **Proof:** the full `ImportReport.healings` list (kind, instance, deviation and tolerance as f64
  bits) and every edge's curve control points identical before/after on the corpus; a RED test first
  that asserts the list on one small file with two rebuilt edges.

### 5. Part-relative element size (the intentional mesh change)

- **Change:** `TessellationParams::default()`'s absolute `max_edge_length: 0.1` becomes relative in
  `tessellate_solid` the way `mass_properties` already does it (`tessellation/mod.rs:129`,
  `MASS_PROPERTIES_EDGE_FLOOR = 5e-3` [V]): `max_edge_length = max(0.1, k x extent)`. Chord floor
  (5e-4 x extent) and angle (0.1 rad) are NOT changed; they are what bound the error. Start with
  `k = 5e-3` because a mesh at that element size is already in production for mass properties, and
  settle `k` by measurement on the corpus.
- **Why accuracy holds:** edge length is an element-size knob, not an error bound: on a plane, a
  cylinder ruling or a cone ruling it adds triangles and removes no error; on curved directions the
  sagitta and angle counts still apply (`arc_steps_for_quality` takes the max [V]); NURBS faces
  refine on measured chord error (`curved_cdt.rs:1512`, `:2428`, `:2694` [V]). And the promise stops
  being an assumption: item 6 measures the deflection of every facet against `d_cert`.
- **Saving [I]:** triangle count down 8 to 20 times on 208 (to 170-420 k). Every mesh-proportional
  stage shrinks with it: after items 1-3, 208's remaining 11 s (mesh 7.6, normals 2.1, manifold 0.5,
  quality 0.5) -> about 1 s. 211: remaining about 40 s -> 3 to 5 s. Peak memory about 1 GB -> under
  100 MB.
- **Proof:** this changes output on purpose, so: (a) identical certificate VERDICTS on the corpus
  (46 sound stay sound, the rest fail for the same named reason on the same face); (b) item 6's
  measured maximum deflection <= `d_cert` on every face of every corpus solid; (c) volume from the
  new display mesh within `d_cert x Area` of the mass-properties volume; (d) the existing
  `tess_size_relative_density.rs` test extended to assert the triangle count no longer saturates.

### 6. Certify the deflection, exactly

- **Change:** per-facet deflection computed in the per-face task (closed form for plane, sphere,
  cylinder, cone, torus; centroid + edge midpoints + curvature bound otherwise), per-face maximum on
  `TessellationQuality`, gate `max <= d_cert`, refine the offending face when it fails
  (`harness/watertight.rs` beside `facet_true_normal`; fields in `primitives/provenance.rs`).
- **Why:** it is the missing half of "faithful to the true surfaces" (finding 5) and it is what
  makes item 5 and the self-intersection proof (item 7) sound rather than plausible.
- **Cost [I]:** the centroid footpoint already exists; closed forms are a few flops. Net about zero.
- **Proof:** RED test: a mesh with one sphere vertex pulled 2 x `d_cert` off the surface passes
  today's certificate and must fail the new one, naming the face. Closed forms checked against
  dense sampling on each analytic type.

### 7. Self-intersection on the certificate mesh, with the 2d proof

- **Change:** `harness/self_intersection.rs`: run on the `CertMesh`; broad phase over face AABBs
  then triangles; classify face pairs as proven disjoint (> 2 `d_cert`), crossing (confirm as
  today), or near (refine those faces only). Narrow phase in parallel.
- **Why exact:** section 1.3. Strictly stronger than the current coarse-chord contract.
- **Saving [I]:** removes the third tessellation; 208: 3 s -> under 1 s; 211: 4.6 s -> about 1 s.
- **Proof:** every solid the current check flags is still flagged (existing fixtures: the chamfer
  across a fillet, the folded loft); the drum-with-void chord artifact stays unflagged; a new RED
  fixture with a 0.2 mm interpenetration that the chord-0.5 mesh misses must be caught.

### 8. `CertMesh` / `FaceBlock` as the single carrier

- **Change:** `tessellate_shell_with` (`tessellation/mod.rs:402`) keeps the per-face blocks; the
  checks of items 1, 6, 7 read them; `TriangleMesh` is derived from them by the existing weld.
- **Why exact:** display output goes through the unchanged concatenate + weld.
- **Saving [I]:** removes the remaining footpoint searches for facets whose three vertices carry uv
  (normal evaluated at the seed with one local solve); 208 before item 5: 2.1 s -> about 0.4 s.
  Mostly this item is the enabler for 9.
- **Proof:** `TriangleMesh` checksum-identical to today's for the same parameters on the corpus.

### 9. Per-face certificate cache

- **Change:** section 5.
- **Saving:** none on a first import; on re-certification after an edit touching `t` of `F` faces,
  mesh work drops by about `t / F`.
- **Proof:** for a sequence of edits on corpus parts, the certificate from the cache equals the
  certificate recomputed cold, field for field; face blocks returned by a hit equal a fresh mesh
  bit-for-bit (this is the test that finds a tessellator reading a neighbour).

### 10. Analytic distances and seeded projection in the repairs

- **Change:** section 4, items 4-5.
- **Saving [I]:** another 2 to 5 times on the repair passes after item 4; the step that takes 208's
  repairs from about 6 s to 1-2 s.
- **Proof:** not bit-identical, so: the same set of healings (kind + instance) on the corpus, every
  `after` within 1e-9 mm of the previous value, identical certificate verdicts.

### Where this lands

| | 208 today | after 1-4 (no output change) | after 5-8 | after 10 |
|---|---|---|---|---|
| parse + build | about 1 s | 1 s | 1 s | 1 s |
| repairs | about 37 s | 5-7 s [I] | 5-7 s | 1-2 s [I] |
| certificate | about 34 s | about 13 s | 1.5-2.5 s [I] | same |
| total | about 72 s | about 20 s | about 8-10 s | about 4-5 s |

Items 1-4 change no output and take 208 from about 72 s to about 20 s. That is not yet faster than
FreeCAD's 5 s. Getting under it needs item 5 (a deliberate, measured mesh change guarded by item 6)
and item 10 (verdict-identical but not bit-identical repairs). "Much faster than FreeCAD" on 208 is
not reachable by exact-output changes alone; it is reachable with the stated-bound changes, and the
bound is one the certificate then checks instead of assumes.

Unknowns that would move these numbers: which repair pass dominates (no timer exists); what
B-Rep validation and feature recognition cost on these parts (not in the supplied measurements);
whether the weld inside `tessellate_shell_with` is a large share of the 7.6 s
(`ROSHERA_TESS_TRACE` prints it [V]); the real triangle reduction of item 5 on trimmed spheres.
