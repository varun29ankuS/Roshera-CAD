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
import { referencePolicy, scriptedPolicy, assertActionAllowed } from "../lib/policy.mjs";
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

// ─── the reference policy (the one the batch actually runs) ────────────────
//
// `verify_part` requires `part_id` (roshera-mcp/src/tools/perception.ts:182)
// and the create result is where that number comes from (tools/create.ts:256).
// The batch used to send `args: {}`, which the tool rejected on its schema
// while the episode still reported COMPLETED. The end-to-end proof that the
// BATCH now sends the id lives in wiring.test.mjs; these pin the policy's own
// behaviour, including the branch that has no id to send.

/** core.ts:380-385 ok(data) → mcp_session.readToolResult envelope shape. */
const envelope = (data) => ({
  is_error: false, text: JSON.stringify(data), data,
  parse_error: null, structured: null, refusal: null, rate_limited: false,
});

check("the reference policy declares an intent, builds, then verifies THE PART IT BUILT", async () => {
  const p = referencePolicy({ intent: "shaft blank ø50 x 60 long", radius: 25, height: 60 });
  const ctx = { task, observation: null, history: [] };
  assert.deepEqual(await p.act(ctx), {
    tool: "timeline_checkpoint", args: { name: "shaft blank ø50 x 60 long" },
  });
  assert.deepEqual(await p.act(ctx), {
    tool: "create_cylinder", args: { radius: 25, height: 60 },
  });
  // create.ts:253-260 — the create result carries object_uuid, part_id, placement.
  const created = envelope({
    object_uuid: "3f2b8c1e-77aa-4a9f-8b21-9f0f2a6d5e10", part_id: 4, placement: null,
    perception: { sound: true },
  });
  assert.deepEqual(await p.act({ task, observation: created, history: [] }), {
    tool: "verify_part", args: { part_id: 4 },
  }, "the id is READ from the observation — never hardcoded, never omitted");
  assert.deepEqual(await p.act(ctx), { done: true });
});

check("with no part_id to verify, the reference policy throws rather than declare done", async () => {
  // `part_id` is `newestPartId()`'s result and is legitimately null when the
  // backend reports no parts (core.ts:581-585). A `{done: true}` here would
  // report a COMPLETED episode that verified nothing — the exact defect.
  const p = referencePolicy({ intent: "shaft blank ø50 x 60 long", radius: 25, height: 60 });
  await p.act({ task, observation: null, history: [] });
  await p.act({ task, observation: null, history: [] });
  const idless = envelope({ object_uuid: "u", part_id: null, placement: null });
  await assert.rejects(
    () => p.act({ task, observation: idless, history: [] }),
    /no integer part_id/,
    "the failure must be stated, not converted into a green episode",
  );
});

check("the reference policy's args are frozen, like the scripted one's", async () => {
  const p = referencePolicy({ intent: "shaft blank ø50 x 60 long", radius: 25, height: 60 });
  const a = await p.act({ task, observation: null, history: [] });
  assert.ok(Object.isFrozen(a.args),
    "episode.mjs holds args across session.call and then writes them to the " +
    "trajectory; an editable object would let the record differ from what was sent");
  assert.throws(() => { a.args.name = "something else"; }, TypeError);
});

for (const [name, fn] of checks) { await fn(); process.stdout.write(`  ok - ${name}\n`); }
process.stdout.write(`\npolicy: ${checks.length} checks passed\n`);
