/**
 * Policy-adapter proof.
 *
 * The scripted policy exists so the harness itself can be tested
 * deterministically and without spending tokens. It is not a training
 * target. The allowlist check lives here, at the point of action, because an
 * environment that silently permits an out-of-allowlist tool has an action
 * space that differs from the one stamped in its own trajectory header.
 */
import assert from "node:assert/strict";
import { scriptedPolicy, assertActionAllowed } from "../lib/policy.mjs";
import { defineTask } from "../lib/task.mjs";

const task = defineTask({
  id: "t", prompt: "p", toolAllowlist: ["create_cylinder"],
  claims: [{ name: "r", quantity: "radius", expected: 1, tolerance: 0.01 }],
  stepBudget: 5, tokenBudget: 100, split: "train",
});

const checks = [];
const check = (name, fn) => checks.push([name, fn]);

check("a scripted policy replays its script then declares done", async () => {
  const p = scriptedPolicy([{ tool: "create_cylinder", args: { radius: 1 } }]);
  const first = await p.act({ task, observation: null, history: [] });
  assert.deepEqual(first, { tool: "create_cylinder", args: { radius: 1 } });
  const second = await p.act({ task, observation: null, history: [] });
  assert.deepEqual(second, { done: true }, "an exhausted script is done, not stuck");
});

check("a scripted policy reports zero tokens — it is not a model", () => {
  assert.equal(scriptedPolicy([]).tokensUsed(), 0);
});

check("an action outside the allowlist is refused at the boundary", () => {
  assert.throws(
    () => assertActionAllowed(task, { tool: "boolean", args: {} }),
    /allowlist/i,
    "the stamped action space and the permitted one must be the same set",
  );
});

check("an allowed action passes", () => {
  assert.doesNotThrow(() => assertActionAllowed(task, { tool: "create_cylinder", args: {} }));
});

check("a done action is not checked against the allowlist", () => {
  assert.doesNotThrow(() => assertActionAllowed(task, { done: true }));
});

for (const [name, fn] of checks) { await fn(); process.stdout.write(`  ok - ${name}\n`); }
process.stdout.write(`\npolicy: ${checks.length} checks passed\n`);
