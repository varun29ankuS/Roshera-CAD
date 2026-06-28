# ASSEMBLY_SOLVE_LOOP — make the mate solver POSITION parts, not just check them

**The gap Varun caught (2026-06-28):** the live demo pre-placed every part at
absolute world coordinates, so designating the chamber "ground" did nothing — the
mates only *checked* a layout I had already hand-built. A real assembly fixes ONE
part and the mates *solve* every other part into place relative to it. The
Gauss-Newton solver (S5) already computes that solved configuration; the endpoint
just throws the solved poses away and returns only the pass/fail certificate.

This loop exposes the solved poses end-to-end, so the mates do the placing.

## The 3 loops — run on EVERY slice, in order
1. **BUILD ↔ TEST** — implement; unit tests green.
2. **VERIFY ↔ HARNESS** — the property the slice must ALWAYS guarantee (stated per
   slice); add it as a test and keep it green.
3. **BENCHMARK ↔ VERIFY** — no regression to the existing 35 tests.

Then `cargo fmt`, commit + push the green slice, update STATE, advance.

## Rules
- Production-grade; obey workspace lints (no `unwrap`/`expect`/`panic`).
- `assembly-engine` is a library — AS1/AS2 need no backend restart; AS3 does.
- Run tests under a timeout. NEVER commit a red slice.

## Slices
- [ ] **AS1** — assembly-engine: `solved_poses() -> (SolveReport, Vec<SolvedPose>)`
      returns where the solve puts each instance (ground never moves).
      **HARNESS:** an injector dropped at several deliberately WRONG poses
      (off-axis / far / tilted), mated concentric + coincident to the fixed
      chamber, SOLVES to the seated pose — on the z-axis, base on the chamber top
      (z=16) — `converged` and translation within 1e-3, for every starting pose.
- [ ] **AS2** — api-server: `assembly_verify` returns `solved` (per-instance
      translation + rotation) and the solve report alongside the certificate.
      **HARNESS:** SolvedPose serde round-trips; cargo check green.
- [ ] **AS3** — rebuild backend + LIVE: drop the injector + turbopump at WRONG
      poses, declare their mates, call `assembly_verify`; the returned solved poses
      snap them onto the fixed chamber. **HARNESS:** the live solved poses match
      the expected seated positions (chamber fixed, injector on top, turbopump on
      its mount) within tolerance.

## STATE
- Current: **AS1**.
- Last green: — (start).
- Blockers: none.
