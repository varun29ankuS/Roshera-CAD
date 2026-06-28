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
- [x] **AS3** — LIVE: all 3 parts dumped at the origin → the solve DERIVED chamber
      `[0,0,0]` (fixed), injector `[0,0,16]` (seated on top), turbopump `[20,0,0]`
      (on its mount); converged in 2 iters, residual ~1e-25; applied to the scene
      → assembled engine. HARNESS met: live solved poses == expected. ✅
- [x] **AS4** — interference = PENETRATION, not contact: `no_static_interference`
      uses Parry EPA penetration depth on each part's CONVEX HULL (flush mating
      contact ~0 is allowed; only overlap beyond CONTACT_TOL=1e-3 flags), with a
      boolean fallback for EPA-degenerate / unsupported pairs. Convex hull = exact
      for convex parts, conservative for concave (until convex decomposition).
      HARNESS: `flush_faces_touch_but_do_not_interfere` + `overlapping_parts_interfere`.
      ✅ 38 tests. (Live re-verify after the backend rebuild → engine should certify SOUND.)

## STATE
- **LOOP COMPLETE — AS1→AS3.** The mate solver now POSITIONS parts, not just checks
  them: `solved_poses()` (AS1) → endpoint returns them (AS2) → live demo derived the
  whole assembled config from all-three-at-the-origin (AS3). One part fixed, the rest
  solved around it.
- Last green: **AS3** live (chamber `[0,0,0]`, injector `[0,0,16]`, turbopump `[20,0,0]`).
- **FOLLOW-UP surfaced by AS3 (CONTACT vs PENETRATION):** the seated injector sits FLUSH
  on the chamber (mating faces touch), and `no_static_interference` reads that contact as
  interference → cert returns `is_sound:false` though the placement is correct. A real
  assembly's mating faces touch BY DESIGN; the interference check must distinguish
  tangential CONTACT (allowed) from PENETRATION (overlap). Next slice candidate.
