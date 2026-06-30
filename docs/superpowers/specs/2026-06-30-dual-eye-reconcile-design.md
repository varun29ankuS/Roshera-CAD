# Dual-Eye Reconcile — Design Spec

**Date:** 2026-06-30
**Status:** Approved design, pre-implementation
**Strategic context:** "Move 1" of the agent-eyes plan — close the dual-eye loop so the
kernel's three perception channels cross-check each other. This is the foundation both the
injected-defect benchmark (Move 2) and the wedge artifact (Move 3) build on.

---

## 1. Problem

Roshera already has three independent perception channels:

- **Truth eye** — `ValidityCertificate` (`geometry-engine/src/primitives/provenance.rs:483`),
  11 soundness dimensions + tessellation/mesh-quality reports, produced by
  `BRepModel::certify_solid` (`topology_builder.rs:2560`), attached to mutating responses via
  `certified_response()` (`api-server/src/main.rs:1065`).
- **Scene eye** — server-side render (`geometry-engine/src/render/mod.rs`), with set-of-marks
  already present: `ids` mode returns `face_legend: Vec<(face_id, rgb)>`; `diagnostic` mode marks
  open/non-manifold edges. Endpoints in `api-server/src/handlers/agent.rs`.
- **Semantic eye** — `recognize_features` (`geometry-engine/src/primitives/feature_recognition.rs:101`):
  through/blind holes, fillets, chamfers, bosses, spherical features.

There is **no reconciler**: nothing compares "the face the cert flagged" against "the face that
renders as defective" against "the feature recognition reported." The differentiator — an eye that
cross-checks itself and therefore *cannot lie* — is exactly this missing piece.

Prior art in the repo (to reuse, not duplicate): `tests/agent_build_eval.rs` already does
`assert_eye_agrees` (render-measured dims vs B-Rep soundness, exact seen∪unseen partition) and
`assert_recognizes_bore` (built-feature == recognized-feature) — but only in tests, not production.

## 2. Goals / Non-goals

**Goals**
- Ambient cross-check on every mutating op that **cannot refreeze the backend**.
- A deterministic consistency check that gates `is_sound()` (the hard "cannot lie" verdict).
- A heavier, heuristic, advisory reconcile report covering render-based axes.
- Surface both through existing channels (REST perception block, MCP embedded perception +
  `perceive`/`ground_truth`, perception stream) — no endpoint/tool sprawl.

**Non-goals (Move 1)**
- Intent-vs-reality ("built ≠ what the agent claimed") — needs per-op intent, which ambient cannot
  supply; stays the existing on-demand `verify_claim` MCP tool.
- Submodular next-best-view. Move 1 ships a fixed Fibonacci-sphere viewpoint set; submodular NBV to
  *prove* a face is unseeable is a logged follow-up.
- Per-edge identity matching in Truth↔Scene (Move 1 uses exact count agreement, deterministic;
  per-edge identity is a refinement).

## 3. Key decisions (all approved)

1. **Loop home:** ambient on every op.
2. **Placement:** blend — cheap O(n) deterministic cross-check inline + full render-based reconcile
   async-after-commit, cached, surfaced on next read / stream.
3. **Disagreement axes (all four):** Truth↔Scene, Coverage/blind-spots, Truth↔Semantic,
   Scene↔Semantic.
4. **Verdict coupling — split:** deterministic checks fold into `is_sound` and can refuse; heuristic
   /NBV checks are advisory, severity-tagged, and never silently flip `is_sound`.
5. **Architecture:** hybrid — pure reconcile logic + record types in the kernel; async orchestration
   (snapshot/spawn/cache/surface) in the api-server. Mirrors the existing cert/features/render split.

## 4. Two tiers

### Cheap tier — inline, synchronous, deterministic

Runs inside `certify_solid` (it already holds the mesh and can call `recognize_features`):

- **Truth↔Semantic (deterministic):** every `RecognizedFeature`'s `face_id`s exist and are **not** in
  the cert's flagged/degenerate set. O(features).
- **Truth↔Scene, exact count slice (deterministic):** the certified mesh's
  `open_edges`/`nonmanifold_edges` equal the cert's `boundary_edges`/`nonmanifold_edges`. O(1) given
  the cert. (The *visual* per-face Truth↔Scene confirmation is the heavy/heuristic tier below; this
  count slice is its deterministic, gating part.)

Result is a **new tri-state certificate dimension `eyes_consistent`**
(`NotApplicable | Consistent | Inconsistent`), beside `construction_consistent`/`labels_consistent`,
ANDed into `is_sound()`:

```
is_sound = … ∧ eyes_consistent != Inconsistent
```

No render on the hot path. `NotApplicable` when there is nothing to cross-check (empty solid).

### Heavy tier — async-after-commit, heuristic, advisory

After the op commits, the server spawns a task that snapshots the solid (read-lock to clone, then
**lock dropped** before rendering), renders N NBV viewpoints, and runs the render-based axes:

- **Truth↔Scene** — per-face visual confirmation that cert-flagged faces render flagged and vice versa.
- **Coverage/blind-spots** — Fibonacci-sphere viewpoints → exact seen/unseen face partition.
- **Scene↔Semantic** — each recognized feature renders as that feature (a through-hole reads as a
  bore, not a flipped-normal wall).

Produces a `ReconcileReport`. **Never flips `is_sound`** — severity-tagged advisory only.

## 5. Data structures

New tri-state on `ValidityCertificate` (`provenance.rs`):

```rust
pub eyes_consistent: TriState   // NotApplicable | Consistent | Inconsistent
```

New kernel module `geometry-engine/src/perception/reconcile.rs`:

```rust
pub struct ReconcileReport {
    pub solid_id: SolidId,
    pub cert_fingerprint: u64,      // ties report to the exact cert it reconciled
    pub status: ReconcileStatus,    // Pending | Clean | DiscrepanciesFound
    pub discrepancies: Vec<Discrepancy>,
    pub coverage: Coverage,
    pub viewpoints: u32,
    pub duration_ms: u64,
}

pub enum ReconcileStatus { Pending, Clean, DiscrepanciesFound }

pub struct Discrepancy {
    pub axis: ReconcileAxis,        // TruthScene | Coverage | TruthSemantic | SceneSemantic
    pub severity: Severity,         // Error | Warning | Info
    pub faces: Vec<FaceId>,
    pub edges: Vec<EdgeId>,
    pub message: String,
    pub truth_says: String,         // the three channels, side by side
    pub scene_says: String,
    pub semantic_says: String,
}

pub struct Coverage { pub seen: Vec<FaceId>, pub unseen: Vec<FaceId>, pub total: usize }
```

The deterministic Truth↔Semantic findings also appear in the report (as `axis: TruthSemantic`) for a
complete picture, but their **gating effect is carried solely by `eyes_consistent`** — the report
never changes `is_sound`.

**Pure, unit-testable functions** (server does all rendering; functions take pre-rendered frames):

```rust
pub fn check_eyes_consistent(cert: &ValidityCertificate, features: &[RecognizedFeature]) -> TriState;
pub fn reconcile_full(
    solid: &Solid,
    cert: &ValidityCertificate,
    features: &[RecognizedFeature],
    frames: &[RenderFrame],          // ids + diagnostic, one set per viewpoint
) -> ReconcileReport;
```

The `truth_says / scene_says / semantic_says` triplet makes every discrepancy read as "the cert says
X, the eye sees Y, recognition says Z" — the legible "caught a lie neither sense found" artifact.

## 6. Async orchestration, caching, surfacing (api-server)

- **Trigger:** in `certified_response()`, after the op commits and the cheap verdict is built,
  fire-and-forget the reconcile task. The op response returns immediately.
- **Snapshot:** task takes a brief read-lock to clone the target solid (or its tessellation inputs),
  then **drops the lock** before rendering. The write path is never contended. (Precise inverse of
  the auto-cert regression that ran heavy cert synchronously under the write lock.)
- **Rendering:** N NBV viewpoints via existing `render_solids_dir()` (single solid, arbitrary dir) in
  `ids` + `diagnostic` modes; then `reconcile_full(...)`.
- **Cache:** `DashMap<(SolidId, u64 /*cert_fingerprint*/), Arc<ReconcileReport>>` on app state.
  `cert_fingerprint` = hash of the memoised cert (changes iff the solid changed). Lookup:
  fresh → return; fingerprint mismatch / not computed → `Pending`; an in-flight set prevents duplicate
  tasks per fingerprint; superseded entries evicted when the new one lands (bounded memory).
- **Surfacing (existing channels only):**
  - **REST:** `GET /api/agent/parts/{id}/perception` gains a `reconcile` block (report or `pending`),
    riding the existing `?full=1` opt-in shape.
  - **MCP:** the embedded perception block returned on every mutating op gains `eyes_consistent`
    (instant, from cert); `perceive()` / `ground_truth` surface the full report when ready.
  - **Stream:** on reconcile completion, push a perception-stream event (#23 continuous-perception
    stream) so a watching agent/UI updates without polling.

## 7. NBV viewpoint selection (Coverage)

- **Set:** Fibonacci-sphere of N viewpoints (deterministic, near-uniform; no clustering). Start N≈26;
  a named config constant.
- **Computation:** render each in `ids` mode → union `face_legend` IDs = `seen`; `unseen = all − seen`.
  Reuses the partition logic proven in `assert_eye_agrees`.
- **Severity:** an unseen face is usually honest physics (internal/occluded). Coverage discrepancies
  are `Info`/`Warning` ("eye cannot inspect face F — internal/occluded"), never `Error`. This is the
  "admit what I can't see" signal, not a defect.
- **Lineage (cited):** Fibonacci-sphere sampling; viewpoint-entropy + submodular greedy NBV
  (Massios–Fisher) as the refinement path — logged, not silently dropped.

## 8. Testing

- **Unit (pure fns)** — `check_eyes_consistent` and `reconcile_full` over synthetic inputs with
  injected disagreements: feature on a cert-flagged face → `Inconsistent`; open-edge count mismatch →
  `Inconsistent`; occluded face → a `Coverage` unseen entry. No server. (Seed of the Move 2 benchmark.)
- **Integration** — build a part via the API, assert `eyes_consistent` in the cert, poll the reconcile
  report to `Clean`, assert the coverage partition is exact (`seen ∪ unseen = total`).
- **"Caught a lie"** — a solid where one channel is wrong (e.g. flipped-normal bore) → assert a
  `TruthScene` discrepancy is reported. Demo nucleus.
- **Perf guard** — assert `certified_response` returns *before* the reconcile task finishes (proves the
  heavy tier is off the hot path; no auto-cert-style freeze).

## 9. Coherence / no-dead-code gate (explicit)

- Every new `pub fn` has a real caller: `certify_solid` → `check_eyes_consistent`; server
  orchestration → `reconcile_full`. No orphans.
- Reuse existing `FaceId` / `EdgeId` / `RenderFrame` / `RecognizedFeature` / `TriState`. Define
  `Severity` / `ReconcileAxis` / `ReconcileStatus` once. No duplication.
- Update the cert's self-description in lockstep: `certificate_json()` serializer **and** the
  "certificate lists its N dimensions" doc/count (count goes +1), so the kernel's account of itself
  stays honest.
- Final verification: build + clippy clean with **zero new warnings** on the new module (workspace
  denies `unwrap`/`expect`/`panic`; confirm no new `dead_code`).

## 10. Files touched (anticipated)

- `geometry-engine/src/perception/reconcile.rs` — new module (logic + types).
- `geometry-engine/src/perception/mod.rs` — module wiring (or add to existing `lib.rs` module tree).
- `geometry-engine/src/primitives/provenance.rs` — add `eyes_consistent`, fold into `is_sound()`,
  serialize.
- `geometry-engine/src/primitives/topology_builder.rs` — `certify_solid` calls `check_eyes_consistent`.
- `api-server/src/main.rs` — `certified_response()` spawns the reconcile task; cache on app state;
  `certificate_json()` includes `eyes_consistent`.
- `api-server/src/handlers/agent.rs` — `perception` endpoint `reconcile` block; perception-stream event.
- `roshera-mcp/src/index.ts` — embedded perception gains `eyes_consistent`; `perceive`/`ground_truth`
  surface the report.
- Tests as in §8.

## 11. Follow-ups (logged, out of scope for Move 1)

- Submodular NBV to prove a face is unseeable (vs merely unseen by the fixed set).
- Per-edge identity matching in Truth↔Scene (beyond count agreement).
- Intent-vs-reality reconcile layered on `verify_claim`.
- Move 2: injected-defect benchmark (this spec's tests are the seed).
