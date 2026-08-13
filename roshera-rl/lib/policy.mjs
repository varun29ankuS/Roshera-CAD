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
 * Slice 1 ships exactly one implementation: a scripted policy, deterministic
 * and free, so the harness can be tested without spending tokens. Real model
 * adapters (anthropic, openai-compatible, acp) arrive in slice 2 behind this
 * same interface.
 */

/** Replays a fixed action script, then declares done. */
export function scriptedPolicy(script) {
  let i = 0;
  return {
    async act() {
      if (i >= script.length) return { done: true };
      return script[i++];
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
