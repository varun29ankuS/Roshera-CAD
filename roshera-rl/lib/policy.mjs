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

/**
 * Replays a fixed action script, then declares done.
 *
 * The script is defensively copied and each step is frozen at construction
 * time: a caller that keeps a reference to the original array and later
 * pushes, splices, or edits an element must not change what the policy
 * replays mid-episode. A policy whose own docstring calls it deterministic
 * has to actually be immune to that, or the trajectory it produced is not
 * reproducible — the same drift `assertActionAllowed` refuses when an action
 * space moves out from under a stamped trajectory.
 */
export function scriptedPolicy(script) {
  const frozenScript = Object.freeze(script.map((step) => Object.freeze({ ...step })));
  let i = 0;
  return {
    async act() {
      if (i >= frozenScript.length) return { done: true };
      return frozenScript[i++];
    },
    tokensUsed() { return 0; },
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
