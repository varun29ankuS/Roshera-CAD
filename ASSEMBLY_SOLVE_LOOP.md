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
- [x] **AS1** — `solved_poses() -> (SolveReport, Vec<SolvedPose>)`; HARNESS: an
      injector dropped at 3 WRONG poses solves onto the fixed chamber every time.
      ✅ 36 tests (b92b89e)
- [x] **AS2** — api-server: `assembly_verify` returns `solve` + `solved_poses`;
      HARNESS: SolvedPose serde round-trips; cargo check green. ✅ 37 tests
- [ ] **AS3** — rebuild backend + LIVE: drop the injector + turbopump at WRONG
      poses, declare their mates, call `assembly_verify`; the returned solved poses
      snap them onto the fixed chamber. **HARNESS:** the live solved poses match
      the expected seated positions (chamber fixed, injector on top, turbopump on
      its mount) within tolerance.

## STATE
- Current: **AS3** (rebuild backend + live demo of the solver placing parts).
- Last green: **AS2** — endpoint returns solved poses, 37 tests + cargo check green.
- Blockers: none. AS3 needs a backend release rebuild (~15 min); no MCP reconnect (the tool just gets extra fields back).
