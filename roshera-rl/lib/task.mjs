/**
 * The task spec.
 *
 * Two fields are mandatory and both are load-bearing:
 *
 *   `toolAllowlist` — the action space, FROZEN per task. `find_tool` ranks by
 *   IDF over the tool corpus, so any new tool perturbs rankings corpus-wide.
 *   That is correct for a product and wrong for an environment: a drifting
 *   action space makes two trajectories incomparable. The metatools
 *   (find_tool, describe_tool, workbench) appear only when a task names them
 *   — a task ABOUT tool discovery is legitimate, but it is a different task.
 *
 *   `claims` — machine-checkable ground truth, closed by `verify_claim`.
 *   Without them scoring falls back to a model's opinion, which is the exact
 *   thing this environment exists to avoid.
 *
 * Every claim carries a tolerance. Geometry reproduces to ~4e-8, so an exact
 * float equality claim can never pass; a claim that cannot pass is a broken
 * task, not a hard one.
 */

const SPLITS = Object.freeze(["train", "eval"]);

export function defineTask({
  id, prompt, toolAllowlist, claims, stepBudget, tokenBudget, split,
}) {
  if (typeof id !== "string" || id.trim() === "") {
    throw new Error("a task needs a non-empty id");
  }
  if (typeof prompt !== "string" || prompt.trim() === "") {
    throw new Error(`task ${id}: a task needs a prompt`);
  }
  if (!Array.isArray(toolAllowlist) || toolAllowlist.length === 0) {
    throw new Error(
      `task ${id}: an empty tool allowlist is not an action space. The ` +
      `allowlist is frozen per task because find_tool's IDF ranking shifts ` +
      `corpus-wide when tools change, and a moving action space makes two ` +
      `trajectories incomparable`,
    );
  }
  if (!Array.isArray(claims) || claims.length === 0) {
    throw new Error(
      `task ${id}: a task with no claim has no ground truth, and scoring ` +
      `would fall back to a model's opinion`,
    );
  }
  for (const c of claims) {
    if (typeof c?.tolerance !== "number" || !(c.tolerance > 0)) {
      throw new Error(
        `task ${id}: claim ${c?.name} needs a positive tolerance — geometry ` +
        `reproduces to ~4e-8, so an exact equality claim can never pass`,
      );
    }
  }
  if (!SPLITS.includes(split)) {
    throw new Error(`task ${id}: split must be one of ${SPLITS.join(", ")}`);
  }
  if (!Number.isInteger(stepBudget) || stepBudget <= 0) {
    throw new Error(`task ${id}: stepBudget must be a positive integer`);
  }
  if (!Number.isInteger(tokenBudget) || tokenBudget <= 0) {
    throw new Error(`task ${id}: tokenBudget must be a positive integer`);
  }
  return Object.freeze({
    id, prompt,
    toolAllowlist: Object.freeze([...toolAllowlist]),
    claims: Object.freeze(claims.map((c) => Object.freeze({ ...c }))),
    stepBudget, tokenBudget, split,
  });
}

/**
 * The seed set. Deliberately ONE task for slice 1: this slice proves the
 * episode loop, not task coverage. Volume arrives in slice 3, where a
 * parametric family derives its claims from the sampled parameters — for a
 * parametric family the REQUEST is the ground truth.
 */
export const TASKS = Object.freeze([
  defineTask({
    id: "cylinder-r25-h60",
    prompt:
      "Create a single cylinder with radius 25 mm and height 60 mm, then " +
      "verify it. Declare a design intent first.",
    toolAllowlist: ["timeline_checkpoint", "create_cylinder", "verify_part"],
    claims: [
      { name: "radius", quantity: "radius", expected: 25, tolerance: 0.02 },
      { name: "height", quantity: "height", expected: 60, tolerance: 0.02 },
    ],
    stepBudget: 12,
    tokenBudget: 40000,
    split: "train",
  }),
]);

export function taskById(id) {
  return TASKS.find((t) => t.id === id);
}
