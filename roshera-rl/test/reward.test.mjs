/**
 * Reward-vector proof.
 *
 * The environment reports named components and NEVER scalarizes. A component
 * it could not measure is absent with a reason, never 0 — the same discipline
 * FidelityReport::gaps keeps in the kernel, and for the same reason: a
 * fabricated zero reads as "measured, and terrible", which is a louder lie
 * than silence.
 */
import assert from "node:assert/strict";
import { rewardFromResult, mergeFinal } from "../lib/reward.mjs";

const checks = [];
const check = (name, fn) => checks.push([name, fn]);

check("a sound result with fidelity yields both components", () => {
  const r = rewardFromResult({
    perception: {
      sound: true,
      fidelity: { fidelity_ok: false, worst: { signed_relative_deviation: -0.0997 } },
    },
  });
  assert.equal(r.components.sound, true);
  assert.equal(r.components.fidelity_signed, -0.0997);
  assert.deepEqual(r.gaps, []);
});

check("no fidelity block is a GAP, never fidelity_signed: 0", () => {
  const r = rewardFromResult({ perception: { sound: true } });
  assert.equal(r.components.sound, true);
  assert.ok(!("fidelity_signed" in r.components),
    "an unmeasured component must be ABSENT, not zero");
  const gap = r.gaps.find((g) => g.name === "fidelity_signed");
  assert.ok(gap && typeof gap.reason === "string" && gap.reason.length > 0,
    "and the absence must carry a stated reason");
});

check("a typed refusal is recorded, and is not scored as failure", () => {
  const r = rewardFromResult({
    refused: true, gate: "verification_scope", reason: "unverified work",
  });
  assert.equal(r.components.refused, "verification_scope");
  assert.ok(!("sound" in r.components),
    "a refused call built nothing — soundness was never measured, so it is absent");
  assert.ok(r.gaps.some((g) => g.name === "sound"));
});

check("mergeFinal keeps the WORST fidelity, not the last or the mean", () => {
  const merged = mergeFinal([
    rewardFromResult({ perception: { sound: true, fidelity: { worst: { signed_relative_deviation: -0.01 } } } }),
    rewardFromResult({ perception: { sound: true, fidelity: { worst: { signed_relative_deviation: -0.0997 } } } }),
    rewardFromResult({ perception: { sound: true, fidelity: { worst: { signed_relative_deviation: 0.002 } } } }),
  ]);
  assert.equal(merged.components.fidelity_signed, -0.0997,
    "the worst deviation is the honest terminal reading; a mean would hide it");
  assert.equal(merged.components.sound, true);
  assert.equal(merged.components.refusals, 0);
});

check("mergeFinal over nothing measured reports gaps, not zeros", () => {
  const merged = mergeFinal([]);
  assert.deepEqual(merged.components, { refusals: 0 });
  assert.ok(merged.gaps.some((g) => g.name === "sound"));
  assert.ok(merged.gaps.some((g) => g.name === "fidelity_signed"));
});

check("mergeFinal counts refusals across the episode", () => {
  const merged = mergeFinal([
    rewardFromResult({ refused: true, gate: "intent", reason: "r" }),
    rewardFromResult({ perception: { sound: true } }),
    rewardFromResult({ refused: true, gate: "single_point", reason: "r" }),
  ]);
  assert.equal(merged.components.refusals, 2);
  assert.equal(merged.components.sound, true);
});

for (const [name, fn] of checks) { fn(); process.stdout.write(`  ok - ${name}\n`); }
process.stdout.write(`\nreward: ${checks.length} checks passed\n`);
