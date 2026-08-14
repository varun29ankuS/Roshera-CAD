/**
 * The policy adapter seam.
 *
 * A policy is anything with:
 *
 *   act(ctx) -> Promise<{ tool, args } | { done: true }>
 *   tokensUsed() -> number
 *
 * where ctx = { task, observation, history }. `observation` is the parsed
 * result of the previous tool call — the ambient perception block IS the
 * observation, so there is no separate sensing step.
 *
 * `tokensUsed()` is CUMULATIVE: the total tokens this policy has spent so far
 * in this episode, monotonically non-decreasing across calls — never a
 * per-step delta. `episode.mjs` compares it against the task's `tokenBudget`
 * on every step, so a per-step delta would silently never trip the budget.
 * The scripted policy below returns a constant 0 because it is not a model.
 *
 * Slice 1 ships exactly one implementation: a scripted policy, deterministic
 * and free, so the harness can be tested without spending tokens. Real model
 * adapters (anthropic, openai-compatible, acp) arrive in slice 2 behind this
 * same interface.
 */
import { digestOf } from "./provenance.mjs";

/**
 * Recursively freezes plain objects and arrays, at every level, in place.
 *
 * TWO CALLERS, one helper. `scriptedPolicy` below freezes a script's `args`;
 * `episode.mjs` freezes every reward vector as it is recorded, so the frozen
 * history it hands the policy is frozen at the ENTRIES, not merely at the
 * array. Both are the same requirement — a record that cannot be edited after
 * the fact — so they share one implementation rather than two that can drift.
 *
 * WHAT THIS PROTECTS: both shapes are JSON-shaped data — plain objects and
 * arrays nested to arbitrary depth (a polyline's point list, a nested options
 * object; a reward's `components` object and its `gaps` array of objects).
 * `deepFreeze` walks that whole shape and freezes every plain object/array it
 * reaches, so no nested field can be edited after construction.
 *
 * WHAT THIS DOES NOT PROTECT: exotic types — Map, Set, Date, TypedArray,
 * class instances — are frozen only at the top property level if one is
 * ever reached; `Object.freeze` locks an object's own property slots, but a
 * Map's or Set's entries live in internal slots, not own properties, so
 * `Map#set`/`Set#add`/etc. keep working on a frozen instance. Tool args in
 * this codebase are plain JSON-shaped data (matching what the kernel's MCP
 * tools accept), so that gap is not expected to matter here — but it is a
 * real limit on this helper, stated plainly rather than silently claimed
 * away.
 */
export function deepFreeze(value) {
  if (value === null || typeof value !== "object" || Object.isFrozen(value)) {
    return value;
  }
  Object.freeze(value);
  for (const key of Object.keys(value)) {
    deepFreeze(value[key]);
  }
  return value;
}

/**
 * Replays a fixed action script, then declares done.
 *
 * The script is defensively copied and each step is frozen at construction
 * time: a caller that keeps a reference to the original array and later
 * pushes, splices, or edits an element — including editing a field NESTED
 * inside `args` — must not change what the policy replays mid-episode. A
 * policy whose own docstring calls it deterministic has to actually be
 * immune to that, or the trajectory it produced is not reproducible — the
 * same drift `assertActionAllowed` refuses when an action space moves out
 * from under a stamped trajectory.
 *
 * `args` is deep-cloned with `structuredClone` BEFORE it is deep-frozen: the
 * frozen value stored inside the policy is a separate object from whatever
 * the caller retains, so the caller's own copy of `args` stays ordinarily
 * mutable — freezing never leaks out and makes the caller's assignment
 * throw. It simply stops being the thing the policy replays. See
 * `deepFreeze` above for exactly what depth of nesting is covered.
 */
export function scriptedPolicy(script) {
  const frozenScript = Object.freeze(script.map((step) => {
    const copy = { ...step };
    if (copy.args !== undefined) {
      copy.args = deepFreeze(structuredClone(copy.args));
    }
    return Object.freeze(copy);
  }));
  let i = 0;
  return {
    async act() {
      if (i >= frozenScript.length) return { done: true };
      return frozenScript[i++];
    },
    tokensUsed() { return 0; },
    describe() {
      return { kind: "scripted", script_digest: digestOf(frozenScript) };
    },
  };
}

/**
 * THE REFERENCE POLICY for the seed cylinder task: declare an intent, build,
 * then VERIFY THE PART THAT WAS BUILT.
 *
 * It exists because `scriptedPolicy` structurally cannot do the last step. A
 * script is fixed at construction, and `verify_part`'s schema requires
 * `part_id` — `z.number().int()`, roshera-mcp/src/tools/perception.ts:182 — a
 * number that does not exist until the create call has returned. The batch
 * emitted `{tool: "verify_part", args: {}}`, the real tool rejected it with a
 * schema validation error, and the episode still reported COMPLETED: the
 * reference batch never once exercised verification. Hardcoding an id would be
 * worse, not better — ids are minted by the kernel and a boolean re-mints them.
 *
 * So this policy is MINIMALLY STATEFUL: it reads the id off the observation it
 * was handed. `observation` is the previous call's `readToolResult` ENVELOPE
 * (episode.mjs:240 assigns `observation = result`), and `create_cylinder`
 * returns `part_id` in its own result body (roshera-mcp/src/tools/create.ts:256
 * — `part_id: id`, the id `newestPartId()` resolved). Hence
 * `observation.data.part_id`, and nothing else.
 *
 * WHEN THE ID IS ABSENT IT THROWS, and that is the honest branch. `part_id` is
 * `newestPartId()`'s result, which is legitimately `null` when the backend
 * reports no parts (core.ts:581-585), so the absence is real and reachable.
 * Declaring `{done: true}` there would end the episode COMPLETED having
 * verified nothing — precisely the defect this policy exists to remove — and
 * the outcome taxonomy is closed (trajectory.mjs:60-67), so there is no
 * "policy could not proceed" outcome to reach for. A throw is recorded by
 * `episode.mjs:173-185` as CRASHED with the reason in both the step and the
 * returned object, which states what happened instead of hiding it under a
 * green one.
 *
 * `args` is frozen for the same reason `scriptedPolicy`'s is: `episode.mjs`
 * holds the object across `session.call` and then writes it to the trajectory,
 * so a consumer editing it in flight would make the record differ from what
 * was sent. These args are flat and hold only primitives, so a single
 * `Object.freeze` covers them completely.
 */
export function referencePolicy({ intent, radius, height }) {
  let i = 0;
  return {
    async act({ observation }) {
      const step = i;
      i += 1;
      if (step === 0) {
        return Object.freeze({
          tool: "timeline_checkpoint",
          args: Object.freeze({ name: intent }),
        });
      }
      if (step === 1) {
        return Object.freeze({
          tool: "create_cylinder",
          args: Object.freeze({ radius, height }),
        });
      }
      if (step === 2) {
        const partId = observation?.data?.part_id;
        if (!Number.isInteger(partId)) {
          throw new Error(
            `the reference policy cannot call verify_part: the previous result ` +
            `carried no integer part_id (saw ${JSON.stringify(partId)}). ` +
            `create_cylinder returns it at result.part_id ` +
            `(roshera-mcp/src/tools/create.ts:256) and verify_part requires it ` +
            `(tools/perception.ts:182). Declaring done here would report a ` +
            `COMPLETED episode that verified nothing`,
          );
        }
        return Object.freeze({
          tool: "verify_part",
          args: Object.freeze({ part_id: partId }),
        });
      }
      return { done: true };
    },
    tokensUsed() { return 0; },
    describe() {
      return { kind: "scripted", script_digest: digestOf("reference-policy/v1") };
    },
  };
}

/**
 * The action space stamped in the trajectory header and the action space
 * actually permitted must be the SAME set. Checking here, at the point of
 * action, is what keeps that true for every policy implementation rather than
 * only the well-behaved ones.
 */
export function assertActionAllowed(task, action) {
  if (action?.done === true) return;
  if (!task.toolAllowlist.includes(action?.tool)) {
    throw new Error(
      `task ${task.id}: tool ${JSON.stringify(action?.tool)} is outside the ` +
      `frozen allowlist [${task.toolAllowlist.join(", ")}] — permitting it ` +
      `would make this trajectory incomparable to every other one`,
    );
  }
}
