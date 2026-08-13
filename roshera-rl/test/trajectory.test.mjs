/**
 * Trajectory record proof.
 *
 * The trajectory IS the post-training artifact, so its shape is a
 * deliverable rather than a log. Two properties matter most: the terminal
 * record is named `reward_final` and is per-component (never a sum — the
 * environment does not scalarize), and every record carries `recipe_ref` so
 * replay is understood as recipe-level. Geometry reproduces to ~4e-8 and is
 * NOT bit-stable; a consumer that assumed byte-replay would be wrong, so the
 * schema says so rather than leaving it to be discovered.
 */
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  SCHEMA_VERSION, OUTCOMES, openTrajectory, readTrajectory,
} from "../lib/trajectory.mjs";

const dir = mkdtempSync(join(tmpdir(), "roshera-rl-"));
const checks = [];
const check = (name, fn) => checks.push([name, fn]);

check("a written trajectory round-trips header, steps and terminal", () => {
  const path = join(dir, "ep1.jsonl");
  const t = openTrajectory({
    path, taskId: "flange-dn50", seed: 7, kernelSha: "e0548c88",
    mcpVersion: "0.1.0", toolAllowlist: ["create_cylinder", "verify_part"],
    split: "train",
  });
  t.step({
    i: 0, action: { tool: "create_cylinder", args: { radius: 25 } },
    resultDigest: "sha256:abc", reward: { components: { sound: true }, gaps: [] },
    refusal: null, ms: 42,
  });
  t.close({
    outcome: "COMPLETED",
    rewardFinal: { components: { sound: true, fidelity_signed: -0.0997 }, gaps: [] },
    claims: [{ name: "bore", verified: true }],
    recipeRef: "recipe/42", tokens: 1234, wallMs: 900,
  });

  const { header, steps, terminal } = readTrajectory(path);
  assert.equal(header.schema_version, SCHEMA_VERSION);
  assert.equal(header.task_id, "flange-dn50");
  assert.equal(header.seed, 7);
  assert.deepEqual(header.tool_allowlist, ["create_cylinder", "verify_part"]);
  assert.equal(header.split, "train");
  assert.equal(steps.length, 1);
  assert.equal(steps[0].action.tool, "create_cylinder");
  assert.equal(terminal.outcome, "COMPLETED");
  assert.equal(terminal.recipe_ref, "recipe/42");
  assert.equal(terminal.tokens, 1234);
});

check("the terminal reward is per-component and is NOT named total", () => {
  const path = join(dir, "ep2.jsonl");
  const t = openTrajectory({
    path, taskId: "t", seed: 1, kernelSha: "x", mcpVersion: "y",
    toolAllowlist: [], split: "eval",
  });
  t.close({
    outcome: "COMPLETED",
    rewardFinal: { components: { sound: false, fidelity_signed: 0.12 }, gaps: [] },
    claims: [], recipeRef: null, tokens: 0, wallMs: 1,
  });
  const { terminal } = readTrajectory(path);
  assert.ok("reward_final" in terminal, "the field is reward_final");
  assert.ok(!("reward_total" in terminal),
    "`total` would imply an aggregation the environment refuses to perform");
  assert.equal(terminal.reward_final.components.fidelity_signed, 0.12);
});

check("an unknown outcome is refused rather than written", () => {
  const path = join(dir, "ep3.jsonl");
  const t = openTrajectory({
    path, taskId: "t", seed: 1, kernelSha: "x", mcpVersion: "y",
    toolAllowlist: [], split: "eval",
  });
  assert.throws(
    () => t.close({
      outcome: "FINISHED_MAYBE", rewardFinal: { components: {}, gaps: [] },
      claims: [], recipeRef: null, tokens: 0, wallMs: 1,
    }),
    /unknown outcome/i,
    "an outcome outside the taxonomy is a bug, not a new category",
  );
});

check("the taxonomy is exactly the six named outcomes", () => {
  assert.deepEqual([...OUTCOMES].sort(), [
    "BUDGET_EXHAUSTED", "COMPLETED", "CRASHED", "INVALID_ACTION",
    "RATE_LIMITED", "SETUP_FAILED",
  ]);
});

check("closing twice is refused — a trajectory has one terminal record", () => {
  const path = join(dir, "ep4.jsonl");
  const t = openTrajectory({
    path, taskId: "t", seed: 1, kernelSha: "x", mcpVersion: "y",
    toolAllowlist: [], split: "eval",
  });
  const term = {
    outcome: "CRASHED", rewardFinal: { components: {}, gaps: [] },
    claims: [], recipeRef: null, tokens: 0, wallMs: 1,
  };
  t.close(term);
  assert.throws(() => t.close(term), /already closed/i);
});

for (const [name, fn] of checks) { fn(); process.stdout.write(`  ok - ${name}\n`); }
rmSync(dir, { recursive: true, force: true });
process.stdout.write(`\ntrajectory: ${checks.length} checks passed\n`);
