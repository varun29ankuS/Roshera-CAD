/**
 * Task-spec proof.
 *
 * A task without an explicit tool allowlist is not a task: `find_tool` ranks
 * by IDF over the tool corpus, so ANY new tool perturbs rankings corpus-wide.
 * A drifting action space makes two trajectories incomparable and trains a
 * policy against a moving target, so the allowlist is mandatory and frozen.
 *
 * A task without machine-checkable claims is also not a task — it would have
 * no ground truth, and scoring would fall back to a model's opinion, which is
 * the exact thing this environment exists to avoid.
 */
import assert from "node:assert/strict";
import { defineTask, TASKS, taskById } from "../lib/task.mjs";

const checks = [];
const check = (name, fn) => checks.push([name, fn]);

const valid = {
  id: "cylinder-r25-h60", prompt: "Build a cylinder of radius 25 and height 60.",
  toolAllowlist: ["create_cylinder", "verify_part"],
  claims: [{ name: "radius", quantity: "radius", expected: 25, tolerance: 0.02 }],
  stepBudget: 12, tokenBudget: 40000, split: "train",
};

check("a valid task freezes and round-trips", () => {
  const t = defineTask(valid);
  assert.equal(t.id, "cylinder-r25-h60");
  assert.equal(t.stepBudget, 12);
  assert.throws(() => { t.id = "other"; }, TypeError,
    "a task must be immutable — a mutated spec mid-run silently invalidates its trajectories");
});

check("an empty tool allowlist is refused", () => {
  assert.throws(() => defineTask({ ...valid, toolAllowlist: [] }),
    /allowlist/i);
});

check("a task with no claims is refused", () => {
  assert.throws(() => defineTask({ ...valid, claims: [] }), /claim/i);
});

check("an unknown split is refused", () => {
  assert.throws(() => defineTask({ ...valid, split: "maybe" }), /split/i);
});

check("a claim without a tolerance is refused", () => {
  assert.throws(
    () => defineTask({ ...valid, claims: [{ name: "r", quantity: "radius", expected: 25 }] }),
    /tolerance/i,
    "an exact float equality claim can never pass — geometry reproduces to ~4e-8",
  );
});

check("the seed set is non-empty and every entry is well-formed", () => {
  assert.ok(TASKS.length >= 1);
  for (const t of TASKS) {
    assert.ok(t.toolAllowlist.length > 0);
    assert.ok(t.claims.length > 0);
    assert.ok(["train", "eval"].includes(t.split));
  }
});

check("taskById finds a seed task and misses cleanly", () => {
  assert.equal(taskById(TASKS[0].id).id, TASKS[0].id);
  assert.equal(taskById("no-such-task"), undefined);
});

for (const [name, fn] of checks) { fn(); process.stdout.write(`  ok - ${name}\n`); }
process.stdout.write(`\ntask: ${checks.length} checks passed\n`);
