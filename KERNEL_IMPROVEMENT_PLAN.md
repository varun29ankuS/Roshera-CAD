# Kernel Improvement Plan

_Date: 2026-04-30 · Source: `KERNEL_AUDIT_2026-04-19.md` · Owner: Varun_

This plan sequences the audit's P0/P1/P2 items into actionable tasks,
maps each finding to a tracked task ID, and records explicit deferrals.

## Scope direction (2026-04-30)

> "ASR/TTS is something we can implement later... let's focus on
> getting this to work first with LLMs."

Consequence: provider-tier ASR/TTS plumbing
(`ai-integration::providers::{ASRProvider, TTSProvider}`,
`ProviderManager::asr/tts`, `MockASRProvider`, `MockTTSProvider`,
`AudioFormat` enums, `send_collaborators_update` audio-channel paths)
remains as scaffolding only. No production wiring, no new ASR/TTS code,
no new local-runtime crates. The traits stay so the surface compiles
and so future work has a docked seam.

The audit is scoped to `geometry-engine`, so this directive does not
remove any audit items — it only governs what *else* we will not pull
into the same window.

---

## Audit ↔ Task map

### Already addressed (no new work)

| Audit ref | Action taken | Task |
|-----------|--------------|------|
| §2 NaN-unsafe partial_cmp | Hardened workspace-wide | (prior) |
| §2 unwrap/expect/panic policy | `[workspace.lints.clippy]` deny | (prior) |
| §3 T-splines (P3 #16) | Deleted: `math/tspline.rs` + benches | #20 |
| §3 NurbsCurve duplication (P1 #5, math layer) | math::nurbs Phase 1 numerical promotion | #13 |
| §4 NurbsCurve adapter — primitives layer | (Phase 2 — pending, see #58 below) | (new) |
| §4 IntersectionCurve duplication (P1 #7) | Curve-curve dispatch consolidated | #49/50 |
| §4 IntersectionCurve duplication (P1 #7) | Curve-surface dispatch consolidated | #51/52/53 |
| §3 boolean classification (P1 partial) | Split-face from_solid stamping | #48 |
| §3 boolean classification (P1 partial) | extract_face_loops DCEL angular sort | #23 |
| §3 boolean classification (P1 partial) | classify_face arity / origin | #54 |
| §6 OperationRecorder gaps | fillet_vertices + chamfer parameter fidelity | #38, #39 |
| §6 sketch2d referential integrity | On-delete cascade | #40 |
| §6 export O(n²) vertex weld | spatial-hash | #41 |
| §6 HashMap → DashMap | Loop/Face/Solid stores | #34 |
| §6 Tessellation watertight + winding | Edge coherence + G1 normals | #36, #37 |

### Open — new tasks below

| Audit ref | Severity | Task |
|-----------|----------|------|
| §6.P0 #1 — boolean property tests | P0 | #57 |
| §6.P0 #2 — coincident-plane boolean | P0 | #56 |
| §6.P0 #3 — 3× SSI consolidation | P0 | #55 |
| §6.P0 #4 — `static mut WARMUP_COMPLETE` | P0 | #59 |
| §4 NurbsCurve adapter (primitives layer) | P1 | #58 |
| §6.P1 #6 — 1,792 LOC dead source | P1 | #60 |
| §6.P1 #8 — ai_primitive_registry truth-fix | P1 | #61 |
| §6.P1 #9 — imprint face attribution | P1 | #62 |
| §6.P1 #10 — transform surface refs | P1 | #63 |
| §6.P2 #11 — bspline SAFETY comments | P2 | #64 |
| §6.P2 #12 — `Matrix4::affine_inverse` | P2 | #65 |
| §6.P2 #13 — panic-in-test → assert! | P2 | #66 |
| §6.P2 #14 — aspirational bench headers | P2 | #67 |
| §6.P2 #15 — g2_blending placeholders | P2 | #68 |
| §6.P3 #17 — assembly rotation gap | P3 | (deferred) |

### Explicit deferrals

| Item | Reason |
|------|--------|
| ASR provider implementation | Scope: 2026-04-30 LLM-only directive |
| TTS provider implementation | Scope: 2026-04-30 LLM-only directive |
| `AudioFormat` plumbing in protocol | Scaffolding — keep, do not extend |
| §6.P3 #16 (T-splines) | Closed — deleted in #20 |
| §6.P3 #17 (assembly rotation) | v2 milestone — assembly currently scoped to mate/interfere/bbox/motion only |

---

## Execution order

The audit identifies P0 #1 (property tests) as the single most
important investment ("it converts the kernel from 'it compiles and
doesn't panic' to 'it is provably correct on a distribution of
inputs'"). However, property tests are most useful **after** the known
algebraic bugs are closed, otherwise they will spend the proptest
budget rediscovering known issues.

Order:

1. **#56** Coincident-plane boolean → `Err(Degenerate)` (small, unblocks
   property-test seed generation: invariants must not flag a known-bad
   path as a counter-example).
2. **#55** SSI consolidation (medium refactor; reduces the surface that
   property tests must cover by ~1,500 LoC).
3. **#59** AtomicBool WARMUP (trivial, parallel-safe).
4. **#60** Dead-source deletion (mechanical, parallel-safe).
5. **#57** Boolean proptest suite (the headline P0).
6. **#58** NurbsCurve adapter (primitives → math).
7. **#61** ai_primitive_registry truth-fix (AI-native surface).
8. **#62, #63** Imprint face attribution + transform surface refs.
9. **#64–#68** P2 polish, in any order.

Items 1-3 are independently small and can be done same day. Items 4
and 6 are mechanical. Items 5, 7, 8, 9 are the substantive work.

---

## Out-of-scope for this plan

- Frontend changes (`roshera-app/`)
- New AI features (LLM tool surface, branch arbitration UX)
- ASR/TTS, audio collaboration paths
- New benchmarks vs. third-party kernels
- Public release / marketing copy

These are tracked elsewhere or postponed.
