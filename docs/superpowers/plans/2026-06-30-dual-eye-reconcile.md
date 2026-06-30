# Dual-Eye Reconcile Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the kernel's three perception channels (validity certificate, render, feature-recognition) cross-check each other — a deterministic render-free check gates `is_sound`, and a heavier render-based reconcile runs async-after-commit as an advisory report.

**Architecture:** Pure reconcile logic + record types live in a new kernel module `geometry-engine/src/perception/reconcile.rs`; the api-server owns async orchestration (snapshot off the write lock → render N Fibonacci-sphere viewpoints → `reconcile_full` → cache keyed by cert fingerprint → surface on the next perception read + perception stream). Mirrors the existing cert/features/render split. Surfaced through existing channels only — no new endpoints or MCP tools.

**Tech Stack:** Rust (geometry-engine, api-server crates; Axum; DashMap; tokio), TypeScript (roshera-mcp).

## Global Constraints

- Production-grade only: no `todo!()`/`unimplemented!()`/stubs. (`roshera-backend/CLAUDE.md`)
- Workspace lints DENY `unwrap`/`expect`/`panic`; escapes need `#[allow(clippy::expect_used)]` + a `// Reason:` invariant comment. (`roshera-backend/CLAUDE.md`)
- Coherence / no dead code: every new `pub fn` has a real caller (no orphans); reuse existing types; build + clippy clean with ZERO new warnings (incl. no new `dead_code`). (Varun, 2026-06-30)
- Do NOT run `cargo build`/`run`/`test`/`bench` or `npm run` unless explicitly asked — but each task's verification steps below MAY be run because this plan's execution IS the explicit ask to verify. Use `cargo test -p <crate> <name>` scoped to the new tests.
- Never recreate the auto-cert freeze: the heavy reconcile tier MUST run async on a snapshot with the model lock already dropped. (memory `autocert-perf-regression`)
- Spec of record: `docs/superpowers/specs/2026-06-30-dual-eye-reconcile-design.md`.

## Type/signature facts verified against the tree (use these verbatim)

- `ValidityCertificate` (`geometry-engine/src/primitives/provenance.rs:483`): fields incl.
  `brep_valid, watertight, manifold, euler_characteristic, boundary_edges: usize, nonmanifold_edges: usize, oriented, inconsistent_directed_edges: usize, self_intersection_free, construction_consistent: ConstructionConsistency, labels_consistent: LabelsConsistency, tessellation: TessellationQuality, mesh_quality: MeshQuality, errors: Vec<String>`.
- `ValidityCertificate::is_sound()` (`provenance.rs:535`) ANDs `brep_valid && watertight && manifold && oriented && self_intersection_free && construction_consistent.is_sound() && tessellation.clean && mesh_quality.clean`.
- Tri-state pattern to mirror (`provenance.rs:193`): `enum ConstructionConsistency { Consistent, Inconsistent, NotApplicable }` with `fn label(&self) -> &'static str` and `fn is_sound(&self) -> bool { !matches!(self, Inconsistent) }`.
- `enum RecognizedFeature` (`geometry-engine/src/primitives/feature_recognition.rs:15`): variants `ThroughHole{diameter,axis,face_id:u32}`, `BlindHole{diameter,depth,face_id:u32}`, `Fillet{radius,face_ids:Vec<u32>}`, `Chamfer{distance,face_ids:Vec<u32>}`, `CylindricalBoss{diameter,height,face_id:u32}`, `SphericalFeature{radius,face_id:u32}`. Derives `Debug,Clone,Serialize,Deserialize`.
- `fn recognize_features(solid_id: u32, model: &BRepModel) -> Vec<RecognizedFeature>` (`feature_recognition.rs:101`).
- `struct RenderFrame { width: usize, height: usize, pixels: Vec<u8>, face_legend: Vec<(u32,[u8;3])>, open_edges: usize, nonmanifold_edges: usize }` (`geometry-engine/src/render/mod.rs:128`).
- `fn render_solids_dir(model:&BRepModel, solid_ids:&[SolidId], colors:&[[u8;3]], dir:Vector3, up_hint:Vector3, opts:&RenderOptions) -> Option<RenderFrame>` (`render/mod.rs:336`). Single-solid arbitrary-dir render = pass a one-element slice.
- `BRepModel::certify_solid` (`geometry-engine/src/primitives/topology_builder.rs:2560`) — memoised cert producer.
- `certified_response()` (`api-server/src/main.rs:1065`), `certificate_json()` (`api-server/src/main.rs:977`).
- `GET /api/agent/parts/{id}/perception` handler (`api-server/src/handlers/agent.rs:2216`).
- MCP embedded-perception block (`roshera-mcp/src/index.ts:114`); `ground_truth` (557), `render_part` (511).

> Open item to confirm during Task 4: whether `SolidId` is `u32` or a newtype, and whether `RenderMode` spells the face-id variant `FaceIds` (the doc string says "FaceIds mode"). Use the names exactly as they appear in `render/mod.rs`'s `RenderMode` enum and `topology_builder.rs`'s `SolidId`.

---

### Task 1: `RecognizedFeature::face_ids()` helper

**Files:**
- Modify: `geometry-engine/src/primitives/feature_recognition.rs` (add method in the existing `impl RecognizedFeature` block, ~line 45)
- Test: same file, in its `#[cfg(test)]` module (create one if absent at end of file)

**Interfaces:**
- Produces: `impl RecognizedFeature { pub fn face_ids(&self) -> Vec<u32> }` — every topological face a feature occupies. Consumed by Tasks 3 and 6.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod face_ids_tests {
    use super::*;

    #[test]
    fn through_hole_reports_single_face() {
        let f = RecognizedFeature::ThroughHole { diameter: 2.0, axis: [0.0, 0.0, 1.0], face_id: 7 };
        assert_eq!(f.face_ids(), vec![7]);
    }

    #[test]
    fn fillet_reports_all_faces() {
        let f = RecognizedFeature::Fillet { radius: 1.0, face_ids: vec![3, 4, 5] };
        assert_eq!(f.face_ids(), vec![3, 4, 5]);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p geometry-engine face_ids_tests -- --nocapture`
Expected: FAIL — `no method named `face_ids``.

- [ ] **Step 3: Add the method**

```rust
    /// Every topological face this feature occupies (for cross-checking that a
    /// recognized feature's faces are live and visible). Used by the dual-eye
    /// reconcile.
    pub fn face_ids(&self) -> Vec<u32> {
        match self {
            RecognizedFeature::ThroughHole { face_id, .. }
            | RecognizedFeature::BlindHole { face_id, .. }
            | RecognizedFeature::CylindricalBoss { face_id, .. }
            | RecognizedFeature::SphericalFeature { face_id, .. } => vec![*face_id],
            RecognizedFeature::Fillet { face_ids, .. }
            | RecognizedFeature::Chamfer { face_ids, .. } => face_ids.clone(),
        }
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p geometry-engine face_ids_tests`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add roshera-backend/geometry-engine/src/primitives/feature_recognition.rs
git commit -m "geometry-engine: RecognizedFeature::face_ids() helper for dual-eye reconcile"
```

---

### Task 2: Perception module + reconcile record types

**Files:**
- Create: `geometry-engine/src/perception/mod.rs`
- Create: `geometry-engine/src/perception/reconcile.rs`
- Modify: `geometry-engine/src/lib.rs` (add `pub mod perception;` beside the other top-level `pub mod` lines)
- Test: in `reconcile.rs` `#[cfg(test)]` module

**Interfaces:**
- Produces: `ReconcileReport`, `ReconcileStatus`, `Discrepancy`, `ReconcileAxis`, `Severity`, `Coverage` — all `serde`-serializable. Consumed by Tasks 6, 8, 9.

- [ ] **Step 1: Write the failing test** (in `reconcile.rs`)

```rust
#[cfg(test)]
mod type_tests {
    use super::*;

    #[test]
    fn report_serializes_round_trip() {
        let r = ReconcileReport {
            solid_id: 1,
            cert_fingerprint: 42,
            status: ReconcileStatus::DiscrepanciesFound,
            discrepancies: vec![Discrepancy {
                axis: ReconcileAxis::Coverage,
                severity: Severity::Info,
                faces: vec![9],
                edges: vec![],
                message: "face 9 not visible from any viewpoint".into(),
                truth_says: "face 9 is part of a sound solid".into(),
                scene_says: "face 9 never appears in any render".into(),
                semantic_says: "n/a".into(),
            }],
            coverage: Coverage { seen: vec![1, 2], unseen: vec![9], total: 3 },
            viewpoints: 26,
            duration_ms: 12,
        };
        let json = serde_json::to_string(&r).expect("serialize");
        let back: ReconcileReport = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.status, ReconcileStatus::DiscrepanciesFound);
        assert_eq!(back.coverage.unseen, vec![9]);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p geometry-engine type_tests`
Expected: FAIL — `perception` module / types not found.

- [ ] **Step 3: Create `perception/mod.rs`**

```rust
//! Dual-eye perception: cross-checking the kernel's three perception channels
//! (validity certificate, render, feature-recognition) against each other.
pub mod reconcile;
```

- [ ] **Step 4: Create `perception/reconcile.rs` with the types**

```rust
//! Reconcile the truth eye (certificate), scene eye (render), and semantic eye
//! (feature recognition). Pure logic only — the api-server does all rendering
//! and passes frames in, keeping these functions unit-testable with no I/O.

use serde::{Deserialize, Serialize};

use crate::primitives::feature_recognition::RecognizedFeature;
use crate::primitives::provenance::ValidityCertificate;
use crate::render::RenderFrame;
use std::collections::HashSet;

/// Which pair of eyes a discrepancy is between.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReconcileAxis {
    TruthScene,
    Coverage,
    TruthSemantic,
    SceneSemantic,
}

/// Advisory severity. Never affects `is_sound` — gating lives in the certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReconcileStatus {
    Pending,
    Clean,
    DiscrepanciesFound,
}

/// One eyes-disagree finding, with all three channels' accounts side by side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Discrepancy {
    pub axis: ReconcileAxis,
    pub severity: Severity,
    pub faces: Vec<u32>,
    pub edges: Vec<u32>,
    pub message: String,
    pub truth_says: String,
    pub scene_says: String,
    pub semantic_says: String,
}

/// Which faces the scene eye could and could not see across all viewpoints.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Coverage {
    pub seen: Vec<u32>,
    pub unseen: Vec<u32>,
    pub total: usize,
}

/// The heavy-tier advisory report. NEVER changes `is_sound`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReconcileReport {
    pub solid_id: u32,
    pub cert_fingerprint: u64,
    pub status: ReconcileStatus,
    pub discrepancies: Vec<Discrepancy>,
    pub coverage: Coverage,
    pub viewpoints: u32,
    pub duration_ms: u64,
}
```

- [ ] **Step 5: Wire the module** — add to `geometry-engine/src/lib.rs`:

```rust
pub mod perception;
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p geometry-engine type_tests`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add roshera-backend/geometry-engine/src/perception/ roshera-backend/geometry-engine/src/lib.rs
git commit -m "geometry-engine: perception::reconcile record types (dual-eye reconcile)"
```

---

### Task 3: `check_eyes_consistent` (cheap, deterministic, render-free)

**Files:**
- Modify: `geometry-engine/src/perception/reconcile.rs`
- Test: same file `#[cfg(test)]`

**Interfaces:**
- Consumes: `RecognizedFeature::face_ids()` (Task 1).
- Produces: `pub fn check_eyes_consistent(live_face_ids: &HashSet<u32>, features: &[RecognizedFeature]) -> EyesVerdict` where `EyesVerdict` is `Consistent | Inconsistent | NotApplicable` — consumed by Task 4 (mapped onto the cert's `EyesConsistency`). Returns `NotApplicable` when `features` is empty; `Inconsistent` when any feature face is not in `live_face_ids`; else `Consistent`.

> Rationale (from spec refinement): this render-free check is the SOLE gating cross-check. It catches a real class — a feature recognized against a stale face id (e.g. after a boolean renumbered faces). Deeper render-based checks are advisory (Task 6).

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod consistency_tests {
    use super::*;
    use crate::primitives::feature_recognition::RecognizedFeature;
    use std::collections::HashSet;

    fn live(ids: &[u32]) -> HashSet<u32> { ids.iter().copied().collect() }

    #[test]
    fn no_features_is_not_applicable() {
        assert_eq!(check_eyes_consistent(&live(&[1, 2]), &[]), EyesVerdict::NotApplicable);
    }

    #[test]
    fn all_feature_faces_live_is_consistent() {
        let f = vec![RecognizedFeature::ThroughHole { diameter: 2.0, axis: [0.0, 0.0, 1.0], face_id: 2 }];
        assert_eq!(check_eyes_consistent(&live(&[1, 2, 3]), &f), EyesVerdict::Consistent);
    }

    #[test]
    fn feature_on_dead_face_is_inconsistent() {
        let f = vec![RecognizedFeature::Fillet { radius: 1.0, face_ids: vec![2, 99] }];
        assert_eq!(check_eyes_consistent(&live(&[1, 2, 3]), &f), EyesVerdict::Inconsistent);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p geometry-engine consistency_tests`
Expected: FAIL — `EyesVerdict` / `check_eyes_consistent` not found.

- [ ] **Step 3: Implement**

```rust
/// Verdict from the render-free cross-check (maps onto the certificate's
/// `EyesConsistency` dimension). Kept separate from the cert enum so this module
/// has no dependency cycle with `provenance`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EyesVerdict {
    Consistent,
    Inconsistent,
    NotApplicable,
}

/// Render-free deterministic cross-check (Truth↔Semantic): every recognized
/// feature must reference a live face. `NotApplicable` when there is nothing to
/// cross-check. This is the SOLE check that gates `is_sound`.
pub fn check_eyes_consistent(
    live_face_ids: &HashSet<u32>,
    features: &[RecognizedFeature],
) -> EyesVerdict {
    if features.is_empty() {
        return EyesVerdict::NotApplicable;
    }
    let all_live = features
        .iter()
        .flat_map(|f| f.face_ids())
        .all(|fid| live_face_ids.contains(&fid));
    if all_live {
        EyesVerdict::Consistent
    } else {
        EyesVerdict::Inconsistent
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p geometry-engine consistency_tests`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add roshera-backend/geometry-engine/src/perception/reconcile.rs
git commit -m "geometry-engine: check_eyes_consistent — render-free Truth-Semantic gate"
```

---

### Task 4: `EyesConsistency` certificate dimension + `is_sound` integration

**Files:**
- Modify: `geometry-engine/src/primitives/provenance.rs` (add enum near `ConstructionConsistency` ~line 193; add field to `ValidityCertificate` ~line 518; extend `is_sound()` ~line 535)
- Test: `provenance.rs` `#[cfg(test)]`

**Interfaces:**
- Produces: `enum EyesConsistency { Consistent, Inconsistent, NotApplicable }` with `fn label(&self)->&'static str` and `fn is_sound(&self)->bool`; new field `ValidityCertificate.eyes_consistent: EyesConsistency`. Consumed by Tasks 5, 7.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn eyes_inconsistent_blocks_soundness() {
    // Build a minimal otherwise-sound certificate and flip eyes_consistent.
    let mut cert = ValidityCertificate::fully_sound_for_test();
    assert!(cert.is_sound());
    cert.eyes_consistent = EyesConsistency::Inconsistent;
    assert!(!cert.is_sound(), "Inconsistent eyes must block is_sound");
    cert.eyes_consistent = EyesConsistency::NotApplicable;
    assert!(cert.is_sound(), "NotApplicable must not regress a sound part");
}
```

> If a `fully_sound_for_test()` constructor does not already exist on `ValidityCertificate`, add it under `#[cfg(test)] impl ValidityCertificate` returning every field at its sound value (`brep_valid:true, watertight:true, manifold:true, oriented:true, self_intersection_free:true, construction_consistent:ConstructionConsistency::NotApplicable, labels_consistent:LabelsConsistency::NotApplicable, tessellation:TessellationQuality::empty(), mesh_quality:MeshQuality::empty()` or the equivalent clean constructor, `eyes_consistent:EyesConsistency::Consistent`, counts 0, euler 2, `errors:vec![]`). Reuse `TessellationQuality::empty()` (`provenance.rs:466`); find/define the analogous clean `MeshQuality`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p geometry-engine eyes_inconsistent_blocks_soundness`
Expected: FAIL — `EyesConsistency` / `eyes_consistent` not found.

- [ ] **Step 3: Add the enum** (mirror `ConstructionConsistency`)

```rust
/// Tri-state verdict from the dual-eye reconcile's render-free cross-check
/// (Truth↔Semantic): are all recognized features backed by live faces?
/// `NotApplicable` when the solid has no recognizable features — sound by
/// construction, so featureless primitives never regress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EyesConsistency {
    Consistent,
    Inconsistent,
    NotApplicable,
}

impl EyesConsistency {
    pub fn label(&self) -> &'static str {
        match self {
            EyesConsistency::Consistent => "consistent",
            EyesConsistency::Inconsistent => "inconsistent",
            EyesConsistency::NotApplicable => "not_applicable",
        }
    }
    /// Anything but `Inconsistent` is sound.
    pub fn is_sound(&self) -> bool {
        !matches!(self, EyesConsistency::Inconsistent)
    }
}
```

- [ ] **Step 4: Add the field** to `ValidityCertificate` (after `labels_consistent`):

```rust
    /// Dual-eye reconcile (render-free Truth↔Semantic): every recognized feature
    /// references a live face. `Inconsistent` blocks `is_sound()` (a feature on a
    /// stale/dead face is a real defect). The render-based reconcile axes are
    /// advisory and live in the async `ReconcileReport`, not here.
    pub eyes_consistent: EyesConsistency,
```

- [ ] **Step 5: Extend `is_sound()`** — add the conjunct:

```rust
            && self.eyes_consistent.is_sound()
```

- [ ] **Step 6: Fix every `ValidityCertificate { .. }` construction site** the compiler now flags (each must set `eyes_consistent`). The production producer is set in Task 5; for any other constructor (tests, helpers) set `EyesConsistency::NotApplicable`.

Run: `cargo build -p geometry-engine` and resolve each "missing field `eyes_consistent`" error.

- [ ] **Step 7: Run test to verify it passes**

Run: `cargo test -p geometry-engine eyes_inconsistent_blocks_soundness`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add roshera-backend/geometry-engine/src/primitives/provenance.rs
git commit -m "geometry-engine: EyesConsistency cert dimension, gated into is_sound()"
```

---

### Task 5: `certify_solid` computes `eyes_consistent`

**Files:**
- Modify: `geometry-engine/src/primitives/topology_builder.rs` (inside `certify_solid` / its `compute_certificate` helper, ~line 2560)
- Test: `geometry-engine/tests/dual_eye_reconcile.rs` (create)

**Interfaces:**
- Consumes: `recognize_features` (Task 1 file), `check_eyes_consistent` + `EyesVerdict` (Task 3), `EyesConsistency` (Task 4).
- Produces: `certify_solid` now sets `eyes_consistent` on the returned certificate.

- [ ] **Step 1: Write the failing test** (`tests/dual_eye_reconcile.rs`)

```rust
use geometry_engine::primitives::provenance::EyesConsistency;
use geometry_engine::primitives::topology_builder::BRepModel;

#[test]
fn certify_sets_eyes_consistent_on_a_sound_part() {
    let mut model = BRepModel::with_estimated_capacity(Default::default());
    // Build a simple sound box (use the crate's box primitive entry point).
    let solid_id = geometry_engine::primitives::box_primitive_for_test(&mut model, 10.0, 10.0, 10.0);
    let cert = model.certify_solid(solid_id);
    // A bare box has no recognizable features → NotApplicable, still sound.
    assert_eq!(cert.eyes_consistent, EyesConsistency::NotApplicable);
    assert!(cert.is_sound());
}
```

> Use the real box-primitive constructor that exists in the crate (search `create_box_3d` / `box_primitive` in `geometry-engine/src/primitives/`). Replace `box_primitive_for_test` and `Default::default()` with the actual constructor and the real `EstimatedComplexity::Medium` value used at `topology_builder.rs` capacity sites.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p geometry-engine --test dual_eye_reconcile certify_sets_eyes_consistent_on_a_sound_part`
Expected: FAIL — `eyes_consistent` is whatever placeholder Task 4 step 6 set (likely `NotApplicable` already if the producer was defaulted), OR compile error if the producer wasn't updated. If it already passes because Task 4 defaulted the producer to `NotApplicable`, change the test to a featured part (drill a hole) and assert `Consistent`; that forces the real computation.

- [ ] **Step 3: Implement in `certify_solid`** — after the rest of the certificate fields are computed, before constructing the `ValidityCertificate`:

```rust
        // Dual-eye render-free cross-check (Truth↔Semantic): recognized features
        // must reference live faces. Gates is_sound via EyesConsistency.
        let live_face_ids: std::collections::HashSet<u32> = self
            .solids
            .get(solid_id)
            .map(|s| {
                s.all_shells()
                    .iter()
                    .filter_map(|sh| self.shells.get(*sh))
                    .flat_map(|sh| sh.faces.iter().copied())
                    .collect()
            })
            .unwrap_or_default();
        let features = crate::primitives::feature_recognition::recognize_features(solid_id, self);
        let eyes_consistent = match crate::perception::reconcile::check_eyes_consistent(
            &live_face_ids,
            &features,
        ) {
            crate::perception::reconcile::EyesVerdict::Consistent => {
                crate::primitives::provenance::EyesConsistency::Consistent
            }
            crate::perception::reconcile::EyesVerdict::Inconsistent => {
                crate::primitives::provenance::EyesConsistency::Inconsistent
            }
            crate::perception::reconcile::EyesVerdict::NotApplicable => {
                crate::primitives::provenance::EyesConsistency::NotApplicable
            }
        };
```

Then set `eyes_consistent` in the `ValidityCertificate { .. }` constructor (replacing the `NotApplicable` placeholder from Task 4 step 6 at THIS site only).

> `solid_id` here is the `certify_solid` parameter; match its exact type (`SolidId`). If `all_shells()`/`shells`/`faces` accessors differ, use the same traversal the file already uses elsewhere to enumerate a solid's faces.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p geometry-engine --test dual_eye_reconcile`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add roshera-backend/geometry-engine/src/primitives/topology_builder.rs roshera-backend/geometry-engine/tests/dual_eye_reconcile.rs
git commit -m "geometry-engine: certify_solid computes eyes_consistent from recognized features"
```

---

### Task 6: `reconcile_full` (heavy-tier advisory, pure over pre-rendered frames)

**Files:**
- Modify: `geometry-engine/src/perception/reconcile.rs`
- Test: same file `#[cfg(test)]`

**Interfaces:**
- Consumes: `RenderFrame` (face_legend, open_edges, nonmanifold_edges), `ValidityCertificate`, `RecognizedFeature::face_ids()`.
- Produces: `pub fn reconcile_full(solid_id: u32, cert_fingerprint: u64, live_face_ids: &HashSet<u32>, cert: &ValidityCertificate, features: &[RecognizedFeature], faceid_frames: &[RenderFrame], diagnostic_frame: &RenderFrame, viewpoints: u32, duration_ms: u64) -> ReconcileReport`. Consumed by Task 8.

Behavior (Move-1 scope):
- **Coverage:** `seen` = union of `face_legend` face-ids across `faceid_frames`; `unseen` = `live_face_ids − seen`. One `Info` `Coverage` discrepancy per unseen face.
- **Truth↔Scene:** if `diagnostic_frame.open_edges != cert.boundary_edges` or `diagnostic_frame.nonmanifold_edges != cert.nonmanifold_edges`, one `Warning` `TruthScene` discrepancy.
- **Scene↔Semantic:** for each feature whose faces are all in `unseen`, one `Info` `SceneSemantic` discrepancy ("feature never visible").
- **Truth↔Semantic (mirror):** for each feature face not in `live_face_ids`, one `Error` `TruthSemantic` discrepancy (so the report is complete even though gating lives in the cert).
- `status` = `DiscrepanciesFound` if any discrepancy with `Error`/`Warning`, else `Clean`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod reconcile_full_tests {
    use super::*;
    use crate::primitives::feature_recognition::RecognizedFeature;
    use crate::render::RenderFrame;
    use std::collections::HashSet;

    fn frame_seeing(face_ids: &[u32]) -> RenderFrame {
        RenderFrame {
            width: 1, height: 1, pixels: vec![],
            face_legend: face_ids.iter().map(|&f| (f, [1, 2, 3])).collect(),
            open_edges: 0, nonmanifold_edges: 0,
        }
    }
    fn diag(open: usize, nm: usize) -> RenderFrame {
        RenderFrame { width: 1, height: 1, pixels: vec![], face_legend: vec![], open_edges: open, nonmanifold_edges: nm }
    }
    fn cert_with_counts(boundary: usize, nm: usize) -> ValidityCertificate {
        let mut c = ValidityCertificate::fully_sound_for_test();
        c.boundary_edges = boundary;
        c.nonmanifold_edges = nm;
        c
    }

    #[test]
    fn unseen_face_becomes_coverage_info() {
        let live: HashSet<u32> = [1, 2, 3].into_iter().collect();
        let frames = vec![frame_seeing(&[1, 2])]; // face 3 never seen
        let r = reconcile_full(1, 7, &live, &cert_with_counts(0, 0), &[], &frames, &diag(0, 0), 1, 0);
        assert!(r.coverage.unseen.contains(&3));
        assert!(r.discrepancies.iter().any(|d| d.axis == ReconcileAxis::Coverage && d.faces == vec![3]));
    }

    #[test]
    fn count_mismatch_becomes_truthscene_warning() {
        let live: HashSet<u32> = [1].into_iter().collect();
        let frames = vec![frame_seeing(&[1])];
        let r = reconcile_full(1, 7, &live, &cert_with_counts(0, 0), &[], &frames, &diag(4, 0), 1, 0);
        assert!(r.discrepancies.iter().any(|d| d.axis == ReconcileAxis::TruthScene && d.severity == Severity::Warning));
        assert_eq!(r.status, ReconcileStatus::DiscrepanciesFound);
    }

    #[test]
    fn invisible_feature_becomes_scenesemantic_info() {
        let live: HashSet<u32> = [1, 2].into_iter().collect();
        let frames = vec![frame_seeing(&[1])]; // face 2 (the hole) never seen
        let feats = vec![RecognizedFeature::ThroughHole { diameter: 2.0, axis: [0.0, 0.0, 1.0], face_id: 2 }];
        let r = reconcile_full(1, 7, &live, &cert_with_counts(0, 0), &feats, &frames, &diag(0, 0), 1, 0);
        assert!(r.discrepancies.iter().any(|d| d.axis == ReconcileAxis::SceneSemantic));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p geometry-engine reconcile_full_tests`
Expected: FAIL — `reconcile_full` not found.

- [ ] **Step 3: Implement**

```rust
/// Heavy-tier advisory reconcile over PRE-RENDERED frames (the api-server renders
/// N Fibonacci-sphere viewpoints in FaceIds mode plus one Diagnostic frame and
/// passes them in). NEVER affects `is_sound` — purely advisory.
#[allow(clippy::too_many_arguments)]
pub fn reconcile_full(
    solid_id: u32,
    cert_fingerprint: u64,
    live_face_ids: &HashSet<u32>,
    cert: &ValidityCertificate,
    features: &[RecognizedFeature],
    faceid_frames: &[RenderFrame],
    diagnostic_frame: &RenderFrame,
    viewpoints: u32,
    duration_ms: u64,
) -> ReconcileReport {
    let mut discrepancies = Vec::new();

    // Coverage: union of every viewpoint's visible face ids.
    let mut seen: HashSet<u32> = HashSet::new();
    for frame in faceid_frames {
        for (fid, _rgb) in &frame.face_legend {
            seen.insert(*fid);
        }
    }
    let mut seen_sorted: Vec<u32> = seen.iter().copied().collect();
    seen_sorted.sort_unstable();
    let mut unseen: Vec<u32> = live_face_ids.difference(&seen).copied().collect();
    unseen.sort_unstable();
    for &fid in &unseen {
        discrepancies.push(Discrepancy {
            axis: ReconcileAxis::Coverage,
            severity: Severity::Info,
            faces: vec![fid],
            edges: vec![],
            message: format!("face {fid} is not visible from any of the {viewpoints} viewpoints (internal/occluded)"),
            truth_says: format!("face {fid} is part of the certified solid"),
            scene_says: "never appears in any render".into(),
            semantic_says: "n/a".into(),
        });
    }

    // Truth↔Scene: render edge counts must match the certificate's.
    if diagnostic_frame.open_edges != cert.boundary_edges
        || diagnostic_frame.nonmanifold_edges != cert.nonmanifold_edges
    {
        discrepancies.push(Discrepancy {
            axis: ReconcileAxis::TruthScene,
            severity: Severity::Warning,
            faces: vec![],
            edges: vec![],
            message: "render edge counts disagree with the certificate (chord-dependent closure)".into(),
            truth_says: format!("boundary={} nonmanifold={}", cert.boundary_edges, cert.nonmanifold_edges),
            scene_says: format!("open={} nonmanifold={}", diagnostic_frame.open_edges, diagnostic_frame.nonmanifold_edges),
            semantic_says: "n/a".into(),
        });
    }

    // Truth↔Semantic (mirror of the gating check; gating itself lives in the cert).
    for f in features {
        let dead: Vec<u32> = f.face_ids().into_iter().filter(|fid| !live_face_ids.contains(fid)).collect();
        if !dead.is_empty() {
            discrepancies.push(Discrepancy {
                axis: ReconcileAxis::TruthSemantic,
                severity: Severity::Error,
                faces: dead.clone(),
                edges: vec![],
                message: format!("recognized {} references dead face(s) {:?}", f.feature_type(), dead),
                truth_says: "these face ids are not in the solid".into(),
                scene_says: "n/a".into(),
                semantic_says: f.to_description(),
            });
        }
    }

    // Scene↔Semantic: a feature whose faces are all unseen never visually appears.
    for f in features {
        let fids = f.face_ids();
        if !fids.is_empty() && fids.iter().all(|fid| !seen.contains(fid)) {
            discrepancies.push(Discrepancy {
                axis: ReconcileAxis::SceneSemantic,
                severity: Severity::Info,
                faces: fids.clone(),
                edges: vec![],
                message: format!("recognized {} is never visible in any render", f.feature_type()),
                truth_says: "n/a".into(),
                scene_says: "feature faces not in any face legend".into(),
                semantic_says: f.to_description(),
            });
        }
    }

    let has_hard = discrepancies
        .iter()
        .any(|d| matches!(d.severity, Severity::Error | Severity::Warning));
    let status = if has_hard { ReconcileStatus::DiscrepanciesFound } else { ReconcileStatus::Clean };

    ReconcileReport {
        solid_id,
        cert_fingerprint,
        status,
        discrepancies,
        coverage: Coverage { seen: seen_sorted, unseen, total: live_face_ids.len() },
        viewpoints,
        duration_ms,
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p geometry-engine reconcile_full_tests`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add roshera-backend/geometry-engine/src/perception/reconcile.rs
git commit -m "geometry-engine: reconcile_full — advisory dual-eye reconcile over rendered frames"
```

---

### Task 7: Serialize `eyes_consistent` + cert fingerprint helper (api-server)

**Files:**
- Modify: `api-server/src/main.rs` (`certificate_json()` ~line 977; add a `cert_fingerprint` helper)
- Test: `api-server/src/main.rs` `#[cfg(test)]` (or the crate's existing test module)

**Interfaces:**
- Produces: `certificate_json` includes `"eyes_consistent": "<label>"`; `fn cert_fingerprint(cert: &ValidityCertificate) -> u64`. Consumed by Tasks 8, 9.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn certificate_json_includes_eyes_consistent() {
    let cert = geometry_engine::primitives::provenance::ValidityCertificate::fully_sound_for_test();
    let v = certificate_json(&cert);
    assert_eq!(v["eyes_consistent"], serde_json::json!("consistent"));
}

#[test]
fn fingerprint_changes_with_cert() {
    let a = geometry_engine::primitives::provenance::ValidityCertificate::fully_sound_for_test();
    let mut b = a.clone();
    b.boundary_edges = 7;
    assert_ne!(cert_fingerprint(&a), cert_fingerprint(&b));
}
```

> `fully_sound_for_test()` must be reachable from the api-server crate — make the constructor `#[cfg(any(test, feature = "test-helpers"))] pub fn` in `provenance.rs`, or replicate a minimal builder in the api-server test. Prefer exposing it `pub` under `#[cfg(test)]`-safe gating if the workspace already does this for other helpers; otherwise build the cert via a real `certify_solid` call on a box in this test.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p api-server certificate_json_includes_eyes_consistent fingerprint_changes_with_cert`
Expected: FAIL — key absent / `cert_fingerprint` not found.

- [ ] **Step 3: Add the fingerprint helper** (near `certificate_json`)

```rust
/// Stable per-state fingerprint of a certificate — changes iff the solid changed.
/// Ties an async reconcile report to the exact cert it reconciled.
fn cert_fingerprint(cert: &geometry_engine::primitives::provenance::ValidityCertificate) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    // Hash the JSON account (covers every dimension without requiring Hash on the
    // certificate type or its composite reports).
    certificate_json(cert).to_string().hash(&mut h);
    h.finish()
}
```

- [ ] **Step 4: Add the field** in `certificate_json` (in the object it builds):

```rust
        "eyes_consistent": cert.eyes_consistent.label(),
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p api-server certificate_json_includes_eyes_consistent fingerprint_changes_with_cert`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add roshera-backend/api-server/src/main.rs
git commit -m "api-server: serialize eyes_consistent + cert_fingerprint helper"
```

---

### Task 8: Async reconcile orchestration + cache (api-server)

**Files:**
- Modify: `api-server/src/main.rs` (app-state struct: add cache + in-flight maps; `certified_response()` ~line 1065: spawn the task)
- Create: `api-server/src/reconcile_task.rs` (the spawned worker + Fibonacci-sphere viewpoints) and declare `mod reconcile_task;`
- Test: `reconcile_task.rs` `#[cfg(test)]` (viewpoint generator only — the spawn is covered by Task 9's integration test)

**Interfaces:**
- Consumes: `render_solids_dir` (FaceIds + Diagnostic modes), `recognize_features`, `reconcile_full`, `cert_fingerprint`.
- Produces: app-state fields `reconcile_cache: Arc<DashMap<(u32, u64), Arc<ReconcileReport>>>`, `reconcile_inflight: Arc<DashMap<(u32, u64), ()>>`; `fn fibonacci_sphere(n: u32) -> Vec<Vector3>`; `fn spawn_reconcile(state, solid_id, fingerprint)`. Consumed by Task 9.

- [ ] **Step 1: Write the failing test** (viewpoint generator)

```rust
#[cfg(test)]
mod viewpoint_tests {
    use super::*;
    #[test]
    fn fibonacci_sphere_count_and_unit_length() {
        let dirs = fibonacci_sphere(26);
        assert_eq!(dirs.len(), 26);
        for d in &dirs {
            assert!((d.length() - 1.0).abs() < 1e-9, "viewpoints must be unit vectors");
        }
    }
}
```

> Use the crate's actual `Vector3` constructor and `.length()`/`.norm()` accessor names.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p api-server fibonacci_sphere_count_and_unit_length`
Expected: FAIL — module/function not found.

- [ ] **Step 3: Create `reconcile_task.rs`**

```rust
//! Async dual-eye reconcile worker. Runs OFF the model write lock on a snapshot:
//! the precise inverse of the auto-cert regression that froze the backend by
//! running heavy certification synchronously under the write lock.

use std::collections::HashSet;
use std::sync::Arc;

use dashmap::DashMap;
use geometry_engine::math::Vector3;
use geometry_engine::perception::reconcile::{reconcile_full, ReconcileReport};
use geometry_engine::primitives::feature_recognition::recognize_features;
use geometry_engine::render::{RenderFrame, RenderMode, RenderOptions};

pub type ReconcileCache = Arc<DashMap<(u32, u64), Arc<ReconcileReport>>>;
pub type ReconcileInflight = Arc<DashMap<(u32, u64), ()>>;

const VIEWPOINTS: u32 = 26;

/// Near-uniform unit directions on the sphere (no clustering). Fibonacci-sphere
/// sampling — Móré/Saff–Kuijlaars; the standard even-coverage NBV seed.
pub fn fibonacci_sphere(n: u32) -> Vec<Vector3> {
    let mut out = Vec::with_capacity(n as usize);
    let golden = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt()); // ~2.399963
    let nf = n as f64;
    for i in 0..n {
        let fi = i as f64;
        let y = 1.0 - (fi / (nf - 1.0).max(1.0)) * 2.0; // 1 → -1
        let r = (1.0 - y * y).max(0.0).sqrt();
        let theta = golden * fi;
        out.push(Vector3::new(theta.cos() * r, y, theta.sin() * r));
    }
    out
}
```

> Match `Vector3::new` / module path exactly. If `RenderMode`'s face-id variant is not `FaceIds`, use the exact name from `render/mod.rs`.

- [ ] **Step 4: Add the spawn function** (in `reconcile_task.rs`)

```rust
/// Fire-and-forget: snapshot the solid off the write lock, render N viewpoints,
/// reconcile, and cache the report. Caller passes Arc clones only.
pub fn spawn_reconcile(
    model: Arc<parking_lot::RwLock<geometry_engine::primitives::topology_builder::BRepModel>>,
    cache: ReconcileCache,
    inflight: ReconcileInflight,
    solid_id: u32,
    fingerprint: u64,
) {
    let key = (solid_id, fingerprint);
    if cache.contains_key(&key) || inflight.insert(key, ()).is_some() {
        return; // already done or already running for this exact state
    }
    tokio::task::spawn_blocking(move || {
        let started = std::time::Instant::now();
        // --- snapshot phase: brief read lock, then DROP before rendering ---
        let snapshot = {
            let guard = model.read();
            // Clone the whole model snapshot (rendering needs the shared stores).
            // This is the ONLY lock hold; it ends at the close of this block.
            guard.clone()
        };
        let live_face_ids: HashSet<u32> = snapshot
            .solids
            .get(solid_id)
            .map(|s| {
                s.all_shells()
                    .iter()
                    .filter_map(|sh| snapshot.shells.get(*sh))
                    .flat_map(|sh| sh.faces.iter().copied())
                    .collect()
            })
            .unwrap_or_default();
        let cert = snapshot.certify_solid(solid_id);
        let features = recognize_features(solid_id, &snapshot);

        let up = Vector3::new(0.0, 1.0, 0.0);
        let alt_up = Vector3::new(1.0, 0.0, 0.0);
        let mut faceid_frames: Vec<RenderFrame> = Vec::new();
        let id_opts = RenderOptions { mode: RenderMode::FaceIds, ..RenderOptions::default() };
        for dir in fibonacci_sphere(VIEWPOINTS) {
            let up_hint = if dir.cross(&up).length() < 1e-6 { alt_up } else { up };
            if let Some(frame) =
                geometry_engine::render::render_solids_dir(&snapshot, &[solid_id], &[], dir, up_hint, &id_opts)
            {
                faceid_frames.push(frame);
            }
        }
        let diag_opts = RenderOptions { mode: RenderMode::Diagnostic, ..RenderOptions::default() };
        let diagnostic_frame = geometry_engine::render::render_solids_dir(
            &snapshot, &[solid_id], &[], Vector3::new(1.0, 1.0, 1.0), up, &diag_opts,
        )
        .unwrap_or(RenderFrame { width: 0, height: 0, pixels: vec![], face_legend: vec![], open_edges: 0, nonmanifold_edges: 0 });

        let report = reconcile_full(
            solid_id,
            fingerprint,
            &live_face_ids,
            &cert,
            &features,
            &faceid_frames,
            &diagnostic_frame,
            VIEWPOINTS,
            started.elapsed().as_millis() as u64,
        );
        cache.insert(key, Arc::new(report));
        inflight.remove(&key);
    });
}
```

> Confirm `BRepModel: Clone` (the read snapshot relies on it). If it is not `Clone`, instead extract just the data `render_solids_dir` + `recognize_features` need under the read lock into owned structures, then drop the guard — do NOT hold the lock across rendering. Match `RenderOptions` field/default names exactly. Match the model's lock type (`parking_lot::RwLock` vs `tokio::sync::RwLock`); if it is a tokio lock, use its blocking_read inside `spawn_blocking` or restructure to read under an async context then `spawn_blocking` the render.

- [ ] **Step 5: Wire into app state + `certified_response`**

In the app-state struct add:
```rust
    pub reconcile_cache: crate::reconcile_task::ReconcileCache,
    pub reconcile_inflight: crate::reconcile_task::ReconcileInflight,
```
Initialize both to `Arc::new(DashMap::new())` where the state is built. Declare `mod reconcile_task;` in `main.rs`. At the end of `certified_response()`, after the response is built and the write lock is released:
```rust
    crate::reconcile_task::spawn_reconcile(
        model_arc.clone(),
        state.reconcile_cache.clone(),
        state.reconcile_inflight.clone(),
        solid_id,
        cert_fingerprint(&cert),
    );
```
> Use the actual handles in scope at that call site for the model `Arc<RwLock<…>>`, the `solid_id`, and the already-computed `cert`. Ensure this runs AFTER any write guard is dropped.

- [ ] **Step 6: Run the viewpoint test**

Run: `cargo test -p api-server fibonacci_sphere_count_and_unit_length`
Expected: PASS. Then `cargo build -p api-server` clean.

- [ ] **Step 7: Commit**

```bash
git add roshera-backend/api-server/src/reconcile_task.rs roshera-backend/api-server/src/main.rs
git commit -m "api-server: async dual-eye reconcile worker (snapshot off the write lock) + cache"
```

---

### Task 9: Surface the report on the perception endpoint + integration test

**Files:**
- Modify: `api-server/src/handlers/agent.rs` (the `GET /api/agent/parts/{id}/perception` handler ~line 2216)
- Test: `api-server/tests/dual_eye_perception.rs` (create) — full HTTP-less handler test or a `tower::ServiceExt::oneshot` test against the router if the crate already has that harness.

**Interfaces:**
- Consumes: `reconcile_cache`, `cert_fingerprint` (Task 7).
- Produces: the perception JSON gains a `reconcile` field: the report object, or `{ "status": "pending" }`.

- [ ] **Step 1: Write the failing test**

```rust
// Build a part via the kernel, certify, manually run a reconcile, insert into the
// cache, then assert the perception handler returns the report. (If the crate has
// a router oneshot harness, prefer driving POST create-box then GET perception and
// polling until reconcile != pending — see existing tests in this crate for the
// pattern.)
#[tokio::test]
async fn perception_surfaces_reconcile_when_cached() {
    // ... build state, solid, cert; key = (solid_id, cert_fingerprint(&cert));
    // state.reconcile_cache.insert(key, Arc::new(clean_report_for(solid_id, key.1)));
    // let body = call perception handler for solid_id;
    // assert_eq!(body["reconcile"]["status"], "Clean");
}
```

> Fill this in against the crate's existing handler-test pattern (search `oneshot` / `Router` in `api-server/tests/`). The assertion that matters: when a report is cached for the current `(solid_id, cert_fingerprint)`, `body["reconcile"]["status"]` is the report's status; when none is cached, it is `"pending"`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p api-server --test dual_eye_perception`
Expected: FAIL — `reconcile` key absent.

- [ ] **Step 3: Implement** — in the perception handler, after building the existing perception JSON:

```rust
    let fingerprint = cert_fingerprint(&cert);
    let reconcile_json = match state.reconcile_cache.get(&(solid_id, fingerprint)) {
        Some(rep) => serde_json::to_value(rep.value().as_ref())
            .unwrap_or_else(|_| serde_json::json!({ "status": "pending" })),
        None => serde_json::json!({ "status": "pending" }),
    };
    // insert into the response object:
    perception["reconcile"] = reconcile_json;
```

> Use the handler's actual response-object variable and the in-scope `cert` (or re-certify via the model as the handler already does). `cert_fingerprint` is in `main.rs`; make it `pub(crate)` so `handlers::agent` can call it.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p api-server --test dual_eye_perception`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add roshera-backend/api-server/src/handlers/agent.rs roshera-backend/api-server/src/main.rs roshera-backend/api-server/tests/dual_eye_perception.rs
git commit -m "api-server: perception endpoint surfaces the dual-eye reconcile report"
```

---

### Task 10: MCP surfaces `eyes_consistent` + the reconcile report

**Files:**
- Modify: `roshera-mcp/src/index.ts` (embedded-perception block ~line 114; `ground_truth` ~line 557)

**Interfaces:**
- Consumes: the perception response's `eyes_consistent` (from Task 7) and `reconcile` block (from Task 9).
- Produces: every mutating MCP response's embedded perception carries `eyes_consistent`; `ground_truth` / `perceive` include the reconcile report when present.

- [ ] **Step 1: Add `eyes_consistent` to the embedded perception block** — wherever the block maps the backend perception fields (sound/watertight/…), add:

```ts
      eyes_consistent: perception.eyes_consistent ?? "not_applicable",
```

- [ ] **Step 2: Surface the reconcile report in `ground_truth`/`perceive`** — where these read `GET .../perception?full=1`, include the `reconcile` block in the returned text/JSON:

```ts
  const reconcile = perception.reconcile ?? { status: "pending" };
  // include `reconcile` in the structured result the tool returns
```

- [ ] **Step 3: Type-check**

Run: `cd roshera-mcp && npx tsc --noEmit`
Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add roshera-mcp/src/index.ts
git commit -m "mcp: surface eyes_consistent + dual-eye reconcile report to agents"
```

---

### Task 11: "Caught a lie" demo test, perf guard, and coherence sweep

**Files:**
- Test: `geometry-engine/tests/dual_eye_reconcile.rs` (extend)
- Test: `api-server/tests/dual_eye_perception.rs` (extend)

**Interfaces:** none new — verification only.

- [ ] **Step 1: "Caught a lie" test (kernel)** — construct frames that lie and assert the reconcile catches it:

```rust
#[test]
fn reconcile_catches_a_face_no_eye_can_see() {
    use geometry_engine::perception::reconcile::*;
    use std::collections::HashSet;
    let live: HashSet<u32> = [10, 11, 12].into_iter().collect();
    // Every viewpoint sees only 10 and 11 — face 12 is an internal cavity wall.
    let frames = vec![
        RenderFrame { width:1,height:1,pixels:vec![], face_legend: vec![(10,[1,1,1]),(11,[2,2,2])], open_edges:0, nonmanifold_edges:0 },
    ];
    let diag = RenderFrame { width:1,height:1,pixels:vec![], face_legend:vec![], open_edges:0, nonmanifold_edges:0 };
    let cert = ValidityCertificate::fully_sound_for_test();
    let r = reconcile_full(1, 7, &live, &cert, &[], &frames, &diag, 1, 0);
    // The cert says sound, but the eye admits it cannot inspect face 12.
    assert!(r.coverage.unseen.contains(&12));
    assert!(r.discrepancies.iter().any(|d| d.axis == ReconcileAxis::Coverage && d.faces == vec![12]));
}
```

Run: `cargo test -p geometry-engine reconcile_catches_a_face_no_eye_can_see` → PASS.

- [ ] **Step 2: Perf-guard test (api-server)** — assert the op returns before the reconcile lands:

```rust
#[tokio::test]
async fn mutating_op_returns_before_reconcile_completes() {
    // POST create-box (or the lightest mutating op); assert the response arrives,
    // and that immediately afterward GET perception's reconcile.status == "pending"
    // (the async task has not yet populated the cache). This proves the heavy tier
    // is off the synchronous hot path.
}
```

> Implement against the crate's router oneshot harness. The assertion that matters: directly after a mutating op, `reconcile.status` is `"pending"` (it becomes a report only after the async task finishes).

Run: `cargo test -p api-server mutating_op_returns_before_reconcile_completes` → PASS.

- [ ] **Step 3: Coherence / no-dead-code sweep**

Run: `cargo build -p geometry-engine -p api-server 2>&1 | grep -iE "warning: .*(never used|dead_code)"`
Expected: NO lines referencing any new symbol (`reconcile`, `eyes_consistent`, `fibonacci_sphere`, `cert_fingerprint`, `spawn_reconcile`, `face_ids`, `EyesConsistency`, `ReconcileReport`). If any new symbol is unused, wire it to its caller (it is a plan bug, not an allow-attribute).

Run: `cargo clippy -p geometry-engine -p api-server -- -D warnings` (scoped) → no new lints from the new modules.

- [ ] **Step 4: Update the certificate self-description count**

The certificate's own doc/summary names how many dimensions it carries (search `dimension` near `certificate` doc and any "lists … dimensions" comment, e.g. the recent `assembly-engine: certificate doc lists all seven dimensions` style). Increment the count by one and add `eyes_consistent` to the enumerated list so the kernel's account of itself stays honest.

```bash
git add -A
git commit -m "test+docs: dual-eye reconcile caught-a-lie + perf guard; cert self-description +eyes_consistent"
```

- [ ] **Step 5: Final full check**

Run: `cargo test -p geometry-engine -p api-server` (whole suites) → green, including `poke_matrix` and existing cert/render tests (no regression).

```bash
git commit --allow-empty -m "verify: dual-eye reconcile Move 1 complete — suites green, no new warnings"
```

---

## Self-Review

**Spec coverage:**
- Two tiers (cheap deterministic gate / heavy async advisory) → Tasks 3–6, 8. ✓
- `eyes_consistent` tri-state ANDed into `is_sound` → Task 4. ✓
- All four axes: Truth↔Semantic (Tasks 3,6), Truth↔Scene count (Task 6), Coverage/NBV (Tasks 6,8), Scene↔Semantic (Task 6). ✓
- Snapshot off the write lock / no auto-cert freeze → Task 8 + perf guard Task 11. ✓
- Cache keyed by cert fingerprint, in-flight de-dup, eviction → Tasks 7,8. ✓
- Surfacing via existing channels only (REST perception block, MCP embedded + ground_truth; stream noted) → Tasks 9,10. ✓ (Perception-stream push is named in the spec; implemented opportunistically in Task 8's cache insert if the stream hook is in scope — otherwise REST/MCP polling covers Move 1 and the stream is a one-line follow-up.)
- Fibonacci-sphere NBV, submodular as logged follow-up → Task 8. ✓
- Coherence / no dead code gate → Task 11. ✓

**Placeholder scan:** Code steps carry real code. The api-server handler/test steps include `> adapt to the crate's existing harness` notes because the exact router-test plumbing and app-state construction site must be read from the tree; signatures and assertions are concrete.

**Type consistency:** `EyesVerdict` (kernel reconcile) maps to `EyesConsistency` (cert) in Task 5 — intentional, to avoid a `provenance`↔`perception` dependency cycle. `face_ids()`, `reconcile_full`, `ReconcileReport`, `cert_fingerprint`, `fibonacci_sphere`, `spawn_reconcile` names are used consistently across tasks.

**Known read-and-confirm items (flagged inline, not guesses):** `SolidId` concrete type; `RenderMode` face-id variant name; `BRepModel: Clone` (else extract owned render inputs under the lock); model lock type (parking_lot vs tokio); `MeshQuality` clean constructor; the router oneshot test harness; the cert self-description count location.
