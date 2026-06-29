# ASSEMBLY_LOOP — autonomous build of the kinematic assembly module

Design: `Roshera-vault/Development-Journal/assembly-module-design.md`
Decision: memory `assembly-module-direction.md`

## The 3 intertwined loops — run on EVERY slice, in order

1. **BUILD ↔ TEST** — implement the slice; write unit tests; iterate until
   `cargo test -p assembly-engine` is green.
2. **VERIFY ↔ HARNESS** — add an invariant harness (properties that must ALWAYS
   hold for the slice — e.g. "no instance is both grounded and floating",
   "interference is symmetric"); verify correctness/cert; iterate until green.
3. **BENCHMARK ↔ VERIFY** — benchmark the slice against a budget; re-verify no
   regression; iterate.

Then: `cargo fmt`, **commit + push** (one green slice = one commit), update STATE
below, advance to the next slice.

## Rules

- Production-grade: no stubs/todos; obey workspace lints (no `unwrap`/`expect`/
  `panic` outside a proven invariant documented immediately above the call).
- `assembly-engine` is a LIBRARY crate — **no backend restart needed**.
- Run `cargo test`/`bench` under a timeout.
- Commit + push each GREEN slice. **NEVER commit a red slice** — log the blocker
  in STATE and stop.
- Parry (`parry3d-f64`) is the collision/CCD engine; the mate-solve + certificate
  are OURS (geometric, not dynamic — the SE(3) generalization of the 2D sketch DCM).
- Clearance certified as `parry_dist − ε` (ε = the mesh-quality deviation bound we
  already compute). Exact analytic distance only as a tight-tolerance refinement.

## Roadmap (slices)

- [x] **S1** — crate + data model (Instance/Mate/FeatureRef) + grounding/no-float + tests  ✅ 3 tests green
- [x] **S2** — parry3d-f64 + static interference + clearance  ✅ 7 tests green (61831a1)
- [x] **S3** — Phase-1 report (assemblable_phase1) + bench (50 parts = 326 µs)  ✅ 10 tests green (27bfe60)
- [x] **S4** — geometric mate residuals (concentric/coincident/fixed) over SE(3)  ✅ 15 tests green (85044aa)
- [x] **S5a** — DOF analysis (numerical Jacobian + SVD rank + Mobility)  ✅ 18 tests green (4a2293e)
- [x] **S5b** — Gauss-Newton solve + over-constrained detect  ✅ 21 tests green (2e27ae9)
- [x] **S6** — joints (revolute/prismatic/spherical/fixed) + free-DOF parameters  ✅ 24 tests green (955fdd2)
- [x] **S7** — CCD swept clearance (sampled) through the DOF + ε-conservative bound  ✅ 27 tests green (b319ac1)
- [x] **S8** — full assembly certificate (5 dims) + invariant harness  ✅ 33 tests green (58486af)  ← CORE COMPLETE
- [x] **S9** — api bridge: POST /api/assembly/verify over live kernel parts (cargo-checked)  ✅ (d20cd24)
- [x] **S10** — dogfood: the floating turbopump is caught + named, then mounted → sound  ✅ 35 tests (49e38e1)  ← LOOP COMPLETE

## STATE

- **LOOP COMPLETE — S1–S10 all green and pushed.** The full kinematic assembly module: engine (grounding · interference · solver · joints · swept clearance · certificate) + the live api bridge (POST /api/assembly/verify) + the rocket-engine dogfood. 35 tests, 11 commits, every slice green on the first pass.
- Last green: **S10** — rocket-engine dogfood (floating turbopump caught + named, then mounted → sound), 35 tests (49e38e1).
- Remaining (optional): LIVE end-to-end run — the `/api/assembly/verify` endpoint is compiled but the RUNNING backend is the old binary. To watch it live: rebuild the backend (~15 min) + add an MCP `assembly_verify` tool + reconnect. The dogfood test already proves the closure.
- Blockers: none.
