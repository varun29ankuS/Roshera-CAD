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

## Corpus (v1)

| # | Scenario | Key oracles |
|---|----------|-------------|
| 01 | Spur gear m=2 z=16 + keyed bore | sound · χ=0 · 293 faces · vol≈5555.8 (shoelace cross-check) · mass≈0.0436 kg |
| 02 | Rao-bell nozzle (revolve + wall 2) | sound · 4 faces · χ=0 · vol≈16309.7 · meridian section 320 mm² |
| 03 | Injector plate (disk + 36 bores) | sound · χ=−70 (genus 36) · volume == analytic oracle |
| 04 | Bulkhead (100×80×10 − 4 pockets) + fillet-all | pre-fillet vol=51200 exact · fillet-all graceful + sound |
| 05 | Pocketed + bored block + fillet-all | pocket vol=56000 exact · χ=−6 · fillet-all sound |
| 06 | Hub flange + GD&T + drawing | sound · χ=−12 · flatness/perp/position IN SPEC 0.00 · drawing quality passed |
| 07 | STEP round-trip of the blended flange | re-import sound:true (blend surfaces survive AP242) |
| 08 | Cross-bore saddle — **honesty canary** | kernel flags UNSOUND (a `sound:true` here would be a lie → FAIL) |
| 09 | Gear four-view drawing | completes < 90 s · /health stays live under load · quality passed |

The **saddle canary (08)** scores PASS by being honestly unsound — issue #35.
When the analytic cyl∘cyl SSI lands, its expectation flips to `sound`; the
scenario is the tripwire for that day (see the note in `scenarios/08-*.mjs`).

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
    01-gear.mjs ... 09-drawing-perf.mjs
    index.mjs              the ordered corpus
```

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
