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
  // verify_claim's real language (tools/inspect.ts:63-99): an expression over
  // bindings, each bound to one of the five closed measure kinds.
  claims: [{
    name: "volume", expr: "v",
    bindings: [{ var: "v", measure: { kind: "volume", part: "solid:0" } }],
    expected: 1, tolerance: 0.01,
  }],
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

check("a script mutated after construction does not change what the policy replays", async () => {
  const script = [{ tool: "create_cylinder", args: { radius: 1 } }];
  const p = scriptedPolicy(script);
  script.push({ tool: "boolean", args: {} });   // caller mutates AFTER construction
  await p.act({ task, observation: null, history: [] });   // first action
  assert.deepEqual(
    await p.act({ task, observation: null, history: [] }),
    { done: true },
    "a script mutated after construction must not change what the policy replays",
  );
});

check("a script's args mutated after construction does not change what the policy replays", async () => {
  const script = [{ tool: "create_cylinder", args: { radius: 1 } }];
  const p = scriptedPolicy(script);
  script[0].args.radius = 999;                    // mutate the NESTED object
  const first = await p.act({ task, observation: null, history: [] });
  assert.equal(first.args.radius, 1,
    "a script's args mutated after construction must not change what the policy replays");
});

check("the replayed args are frozen at every level, not merely copied", async () => {
  // structuredClone alone satisfies the check above, so on its own that test
  // would stay green with `deepFreeze` deleted — the defence would be
  // untested. This pins the property directly; what it BUYS (a consumer
  // editing args in flight while episode.mjs still holds them for the
  // trajectory write) is proven end-to-end in episode.test.mjs, "a frozen
  // action survives a session that tries to edit it mid-call".
  const p = scriptedPolicy([{ tool: "create_cylinder", args: { radius: 1, nested: { depth: 5 } } }]);
  const action = await p.act({ task, observation: null, history: [] });
  assert.ok(Object.isFrozen(action.args), "the args object itself is frozen");
  assert.ok(Object.isFrozen(action.args.nested),
    "and so is every nested plain object — a shallow freeze leaves the real payload editable");
  assert.throws(() => { action.args.nested.depth = 999; }, TypeError);
});

for (const [name, fn] of checks) { await fn(); process.stdout.write(`  ok - ${name}\n`); }
process.stdout.write(`\npolicy: ${checks.length} checks passed\n`);
