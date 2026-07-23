# AGENT-EVAL-α

A scored CAD benchmark where **the kernel's own certificates grade real CAD
jobs**. Each scenario is an ordered sequence of REST calls against a live
Roshera backend plus machine-checkable expectations; the runner scores every
job on four dimensions and prints a scorecard.

This is the instrument behind "the kernel is getting better" (a chart, not a
vibe) and the seed of the fundraise benchmark: **certified CAD vs judge-by-vibes**.

## Why certificates, not screenshots

Roshera's moat is a kernel that *cannot lie*: every operation carries a
self-certifying verdict (`sound`, `watertight`, `manifold`, `euler_characteristic`,
GD&T `conforms`, drawing `quality.passed`, import `validation`). AGENT-EVAL-α
turns that into a repeatable measurement: each job is scored by those
certificates **and** by exact analytic oracles the runner computes independently
(e.g. a shoelace polygon area cross-checked against the kernel's volume
integration). Nothing here is graded by an LLM.

## Score dimensions

| Dimension     | What it measures                                                             |
|---------------|------------------------------------------------------------------------------|
| `correctness` | exact analytic / structural oracles (volume, χ, face count, section area, GD&T verdict) |
| `soundness`   | the kernel's soundness certificate holds at each step                        |
| `honesty`     | geometry the kernel *can't* build is flagged UNSOUND — no lie slips through  |
| `performance` | wall-clock budgets (per scenario, and health-liveness under a heavy drawing) |

## Corpus

| # | Scenario | Key oracles | Honesty probe |
|---|----------|-------------|---------------|
| 01 | Spur gear m=2 z=16 + keyed bore | sound · χ=0 · 293 faces · vol≈5555.8 (shoelace cross-check) · mass≈0.0436 kg | — |
| 02 | Rao-bell nozzle (revolve + wall 2) | sound · 4 faces · χ=0 · vol≈16309.7 · meridian section 320 mm² | — |
| 03 | Injector plate (disk + 36 bores) | sound · χ=−70 (genus 36) · volume == analytic oracle | — |
| 04 | Bulkhead (100×80×10 − 4 pockets) + fillet-all | pre-fillet vol=51200 exact · fillet-all graceful + sound | — |
| 05 | Pocketed + bored block + fillet-all | pocket vol=56000 exact · χ=−6 · fillet-all sound | — |
| 06 | Hub flange + GD&T + drawing | sound · χ=−12 · flatness/perp/position IN SPEC 0.00 · drawing quality passed | — |
| 07 | STEP round-trip of the blended flange | re-import sound:true (blend surfaces survive AP242) | — |
| 08 | Cross-bore saddle (#35 slice-1 guard) | sound · volume == Steinmetz oracle | was the honesty canary; now a soundness tripwire |
| 09 | Gear four-view drawing | completes < 90 s · /health stays live under load · quality passed | — |
| 10 | Kinematic assembly — hinge + slider | solve · certify · drive · motion-stamped interference | floating part named · conflict WITNESSED · unbounded sweep REFUSES |
| 11 | Certified sketch → TRUE cylindrical bore (#45) | converged · fully-constrained (0 DOF) · sound · χ=0 · analytic bore (0 sampled loops) · vol oracle | over-constrained sketch reported `Conflicting` with a witness, never silently "solved" |
| 12 | Mass-properties provenance | box volume == closed form · cylinder vol == πr²h | a mesh estimate labelled `Approximate` (never `Exact`); the self-certified error bound must HOLD |
| 13 | ε=1e-6 coincident-face union | χ=2 · vol == merged-block oracle · sound | `sound:true` may never mask open/non-manifold edges or orphan sliver faces |
| 14 | Quadric SSI — cyl∘sphere bite + near-tangency | volume physically bounded · sound ⟹ genus-0 | unsound must NAME its defect; the near-tangency case REFUSES rather than faking a clean solid |
| 15 | Certified drawing comprehension (semantic sheet readback) | toleranced-Ø / FCF / datum answered from provenance | HATCH ink refuses `render_only`; an unprovenanced question refuses `unprovenanced` |
| 16 | Wall-mounted shelf bracket (FDM PLA) — founder task spec 2026-07-23 §B | sound at every step · watertight+manifold · fits 220×160×60 · 2×M6 @ 60 mm frozen · single-piece · mass (PLA) = ranking | sound ⟹ watertight; stress/deflection/orientation/wall/overhang DECLARED `unscored`, never fake-scored |
| 17 | Vibration-aware NEMA 17 motor mount (FDM PETG) — founder task spec 2026-07-23 Part 2 | sound at every step · fits 90×70×60 · 4×M3 @ 31 mm sq + Ø22 boss + 2×M5 · single-piece · mass (PETG) = ranking | f₁≥120 Hz modal + stress/deflection/wall/overhang/orientation/thermal DECLARED `unscored`, never fake-scored |

### The honesty through-line

Every honesty-bearing scenario ships a **pure oracle** (`export function
oracle`) separated from its `run`, plus a dry-validation harness under
`test/oracle-<nn>.mjs` that feeds the oracle an honest transcript and a set
of single-mutation LIES and proves the oracle passes the truth and catches
every lie — WITHOUT a live backend. A scenario that cannot be shown to catch
a lie is not evidence. Run them all with `npm run test:oracle` (no server
needed).

The **saddle (08)** was the original honesty canary; #35 Slice 1 landed and
it now guards the fix (sound + Steinmetz volume) — the tripwire fired. The
**quadric-SSI scenario (14)** plays that role for the harder quadric cases:
it does not hard-assert a magic volume the kernel may not yet produce, but it
catches any FABRICATED verdict (impossible volume, `sound` over a broken
shell, an unsound verdict that names no defect) and flips to a strict guard
the day general cyl∘sphere SSI lands.

### A note on scenario numbering

Slot **10** is the kinematic-assembly scenario. The #55 drawing-comprehension
scenario (certified semantic sheet readback) also reserved "scenario 10" in
its spec — a latent collision. It is resolved here by assembly owning 10 (it
shipped) and drawing-comprehension taking the next free slot when it is built;
this wave's additions are appended cleanly at 11–14, with no two files sharing
a number.

## Run it

Prerequisite: a live Roshera api-server (default `http://127.0.0.1:8081`).
Node ≥ 18 (uses global `fetch`; no dependencies, no build step).

```bash
cd roshera-eval
node run.mjs                      # whole corpus
node run.mjs 01-gear 02-nozzle    # named scenarios only
node run.mjs --json report.json   # choose the machine-report path
ROSHERA_URL=http://host:8081 node run.mjs   # point at another backend
```

The runner clears the model before each scenario (part ids renumber; this is
the reset) and again at the end (so the honesty-canary debris does not linger).
It prints a per-scenario / per-check scorecard and writes `report.json`.

**Exit code = number of failed scenarios** (0 = suite green), so it drops
straight into CI. The one prerequisite CI must satisfy is a reachable backend;
the runner preflights `/health` and exits 2 with a clear message if it is down.

## Layout

```
roshera-eval/
  run.mjs                  entry point (preflight, run, scorecard, JSON, exit code)
  lib/
    client.mjs             HTTP client + derived reads (certs, uuid lookup, edges)
    harness.mjs            assertion engine + sequential runner + scorecard
    geom.mjs               deterministic profile generators + shoelace oracle
    builders.mjs           shared build recipes (extrude, drill, fillet-all, flange)
  scenarios/
    01-gear.mjs ... 17-nema17-motor-mount.mjs
    index.mjs              the ordered corpus
  test/
    oracle-10.mjs ... oracle-17.mjs   dry validation of each honesty oracle
```

`npm run test:oracle` runs every dry-validation harness (10–17) with no
backend; `npm run eval` runs the live corpus.

### Honesty by omission (scenarios 16–17)

The founder-authored task specs (2026-07-23) carry criteria this kernel cannot
yet compute — von Mises stress, deflection, f₁ ≥ 120 Hz modal, wall-thickness
minimums, overhang/support, print orientation, thermal. Those are NOT dropped
and NOT graded by a stand-in number: each scenario declares them in an
`unscored_criteria` manifest (criterion · spec_ref · reason), and the scored
subset is exactly the honestly-verifiable subset (soundness at every step,
watertight/manifold, envelope, the frozen bolt interfaces at spec Ø and spacing,
single-piece topology, mass as the ranking metric). `test/oracle-16.mjs` and
`oracle-17.mjs` enforce the contract offline: they prove the honest transcript
passes, every planted lie is caught, AND no scored check ever touches an
unscoreable gate. When the physics/printability scoring bridge lands, those
declared criteria migrate from `unscored_criteria` into scored checks.

## v2 direction

- **LLM in the loop**: replace a scenario's scripted build steps with an
  agent (Claude / another model) given the same brief; score the *result*
  with the identical oracle set. "Agent X does CAD better than agent Y"
  becomes a number.
- **Cross-agent leaderboard**: same corpus, many agents, one scorecard.
- **CAD-Judge benchmark**: publish certified-CAD scoring against vibe-graded
  baselines — the fundraise story that our verdicts are ground truth, not
  inference.
- **Corpus growth**: every new honest refusal found while dogfooding becomes a
  new canary; every fixed kernel bug becomes a permanent regression scenario.
