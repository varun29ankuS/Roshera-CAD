/**
 * Reward-vector proof.
 *
 * The environment reports named components and NEVER scalarizes. A component
 * it could not measure is absent with a reason, never 0 — the same discipline
 * FidelityReport::gaps keeps in the kernel, and for the same reason: a
 * fabricated zero reads as "measured, and terrible", which is a louder lie
 * than silence.
 *
 * EVERY input here is a real MCP result run through the real
 * `readToolResult`, not a hand-shaped object. The suite used to feed
 * `rewardFromResult` bodies no tool has ever produced, which is how a refusal
 * scored as a success for three task reviews. Shape provenance is cited on
 * each helper; the wire-level assertions live in `mcp_session.test.mjs`.
 */
import assert from "node:assert/strict";
import { rewardFromResult, mergeFinal } from "../lib/reward.mjs";
import { readToolResult } from "../lib/mcp_session.mjs";

/** core.ts:380-385 — `ok(data)`. */
const ok = (data) => readToolResult({
  content: [{ type: "text", text: JSON.stringify(data, null, 2) }],
});
/** core.ts:485-497 — `fail(e)`: prose + isError. */
const fail = (msg) => readToolResult({
  content: [{ type: "text", text: `ERROR: ${msg}` }], isError: true,
});
/** gates.ts:121-131 — `gateRefusal(payload)`. */
const gateRefusal = (payload) => readToolResult({
  content: [{ type: "text", text: JSON.stringify({ refused: true, ...payload }, null, 2) }],
  isError: true,
});
/** core.ts:929-930 — `okp()` in `cert` mode: the perception OBJECT, whose
 *  `sound` (core.ts:338-339) is the authoritative verdict. */
const created = (perception) => ok({ object_uuid: "3f2b…", part_id: 1, placement: null, perception });

const checks = [];
const check = (name, fn) => checks.push([name, fn]);

check("a sound result yields the soundness component", () => {
  const r = rewardFromResult(created({
    sound: true, brep_valid: true, watertight: true, manifold: null,
    volume: 117809.7, face_count: 3,
    verdict: "OK — valid closed solid (cheap verdict; verify_part to certify)",
  }));
  assert.equal(r.components.sound, true);
  assert.equal(r.components.refused, null);
});

check("no fidelity block is a GAP, never fidelity_signed: 0", () => {
  const r = rewardFromResult(created({ sound: true }));
  assert.ok(!("fidelity_signed" in r.components),
    "an unmeasured component must be ABSENT, not zero");
  const gap = r.gaps.find((g) => g.name === "fidelity_signed");
  assert.ok(gap && gap.reason.length > 0, "and the absence must carry a stated reason");
  assert.ok(gap.reason.includes("never delivered"),
    "the reason must be the TRUE one: the api-server attaches fidelity at " +
    "body.perception.fidelity and roshera-mcp's fixed-key perception drops it");
});

check("fidelity IS read if it ever reaches this process", () => {
  // main.rs:1281-1298 `fidelity_json` — the block's exact shape, attached to
  // the perception by attach_fidelity (main.rs:1326-1334). It does not reach
  // an MCP client today; when the passthrough lands, this must measure it
  // with no further change.
  const r = rewardFromResult(created({
    sound: true,
    fidelity: {
      op: "nurbs_loft", fidelity_ok: false, tolerance: 0.02,
      worst: {
        name: "section_radius", requested: 25, measured: 22.5,
        relative_deviation: 0.0997, signed_relative_deviation: -0.0997,
        direction: "built SMALLER than requested",
      },
      quantities: [], gaps: [], note: "fidelity compares the REQUEST to the RESULT…",
    },
  }));
  assert.equal(r.components.fidelity_signed, -0.0997);
  assert.ok(!r.gaps.some((g) => g.name === "fidelity_signed"));
});

check("a gate refusal is recorded, and is not scored as failure", () => {
  const r = rewardFromResult(gateRefusal({
    gate: "verification_scope",
    reason: "solid-mutating ops ran under this checkpoint with no verify_part / verify_claim since the last of them",
  }));
  assert.equal(r.components.refused, "verification_scope");
  assert.ok(!("sound" in r.components),
    "a refused call built nothing — soundness was never measured, so it is absent");
  assert.ok(r.gaps.some((g) => g.name === "sound"));
});

check("a PROSE failure is a failed step, not a successful one", () => {
  const r = rewardFromResult(fail("POST /api/geometry/cylinder → 422: radius must be positive"));
  assert.equal(typeof r.components.call_failed, "string");
  assert.ok(!("sound" in r.components));
  assert.equal(r.components.refused, null, "no gate refused it — that stays determinate");
});

check("mergeFinal keeps the WORST fidelity, not the last or the mean", () => {
  const dev = (d) => created({ sound: true, fidelity: { worst: { signed_relative_deviation: d } } });
  const merged = mergeFinal([
    rewardFromResult(dev(-0.01)),
    rewardFromResult(dev(-0.0997)),
    rewardFromResult(dev(0.002)),
  ]);
  assert.equal(merged.components.fidelity_signed, -0.0997,
    "the worst deviation is the honest terminal reading; a mean would hide it");
  assert.equal(merged.components.sound, true);
  assert.equal(merged.components.refusals, 0);
});

check("mergeFinal over nothing measured reports gaps, not zeros", () => {
  const merged = mergeFinal([]);
  assert.deepEqual(merged.components, { refusals: 0, call_failures: 0 });
  assert.ok(merged.gaps.some((g) => g.name === "sound"));
  assert.ok(merged.gaps.some((g) => g.name === "fidelity_signed"));
});

check("mergeFinal counts refusals across the episode", () => {
  const merged = mergeFinal([
    rewardFromResult(gateRefusal({ gate: "intent", reason: "no checkpoint is open" })),
    rewardFromResult(created({ sound: true })),
    rewardFromResult(gateRefusal({ gate: "single_point_run", reason: "8 consecutive single-point additions" })),
  ]);
  assert.equal(merged.components.refusals, 2);
  assert.equal(merged.components.sound, true);
});

check("EVERY real refusal shape is counted — the CRITICAL 1 regression", () => {
  const refusals = [
    // 1. gates.ts:121-131 — `refused: true`.
    gateRefusal({ gate: "unsound_base", reason: "the base solid is not certified sound" }),
    // 2. timeline.ts:26-35 — `ok({refused: <OBJECT>})`, no isError at all.
    ok({ refused: { success: false, error_code: "document_not_found", error: "no document 'doc-7'", retryable: false } }),
    // 3. an isError result carrying the kernel's REFUSED marker (gates.ts:161).
    fail("POST /api/geometry/drill → 422: REFUSED: hole spacing below the wall-thickness minimum"),
  ];
  const merged = mergeFinal(refusals.map(rewardFromResult));
  assert.equal(merged.components.refusals, 3,
    "an episode the kernel refused three times must never report refusals: 0 — " +
    "that fabricated zero sits under the whole refusals-are-recorded doctrine");
});

check("a rate-limited call is a failure, and says so", () => {
  // auth_middleware.rs:870-874 through core.ts:189-193.
  const r = rewardFromResult(fail(
    'POST /api/geometry/cylinder → 429: {"error":"Rate limit exceeded","code":"RATE_LIMIT_EXCEEDED","status":429}',
  ));
  assert.equal(r.components.rate_limited, true);
  assert.equal(mergeFinal([r]).components.call_failures, 1);
});

check("a refusal with an empty-string gate is still counted, not silently dropped", () => {
  const merged = mergeFinal([rewardFromResult(gateRefusal({ gate: "" }))]);
  assert.equal(merged.components.refusals, 1,
    "an empty gate name is a falsy string, but the refusal itself is real and must be counted");
});

for (const [name, fn] of checks) { fn(); process.stdout.write(`  ok - ${name}\n`); }
process.stdout.write(`\nreward: ${checks.length} checks passed\n`);
