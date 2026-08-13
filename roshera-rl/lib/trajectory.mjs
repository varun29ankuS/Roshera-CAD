/**
 * The trajectory record — one JSONL file per episode.
 *
 * This is the post-training artifact, not a log, so the format is a
 * first-class deliverable:
 *
 *   line 1   header   { schema_version, task_id, seed, kernel_sha,
 *                       mcp_version, tool_allowlist, split, started_at }
 *   line 2+  step     { i, action, result_digest, reward, refusal, ms }
 *   last     terminal { outcome, reward_final, claims, recipe_ref,
 *                       tokens, wall_ms }
 *
 * `reward_final` is the terminal reading of each NAMED component — not a sum
 * and not a scalar. Naming it `total` would imply an aggregation the
 * environment deliberately refuses to perform: weighting soundness against
 * fidelity against refusal count is a training choice with no kernel
 * justification, so consumers scalarize and the environment reports.
 *
 * `recipe_ref` points at the timeline lineage entry so the build is
 * RE-ISSUABLE. Geometry reproduces to ~4e-8 and is not bit-stable, so replay
 * is recipe-level, never byte-level. Stating that in the schema stops a
 * downstream consumer assuming a determinism the kernel never promised.
 *
 * `tool_allowlist` is stamped here because a shifting action space makes two
 * trajectories incomparable — `find_tool` ranks by IDF over the tool corpus,
 * so any new tool perturbs rankings corpus-wide.
 */
import { appendFileSync, writeFileSync, readFileSync } from "node:fs";

export const SCHEMA_VERSION = "roshera-rl/1";

/**
 * Every episode lands in exactly one of these. Borrowed from
 * `geometry-engine/src/harness/exploration.rs`, which already got this right:
 * an episode that never ran must never be reported as an episode that ran and
 * scored nothing.
 */
export const OUTCOMES = Object.freeze([
  "COMPLETED",        // the policy declared done
  "BUDGET_EXHAUSTED", // step or token cap hit first
  "CRASHED",          // the MCP process died
  "SETUP_FAILED",     // document creation or spawn failed; no episode happened
  "RATE_LIMITED",     // the shared 6000/min EvalHarness budget refused us
]);

class Trajectory {
  #path;
  #closed = false;
  constructor(path) { this.#path = path; }

  step({ i, action, resultDigest, reward, refusal, ms }) {
    if (this.#closed) throw new Error("trajectory already closed");
    appendFileSync(this.#path, JSON.stringify({
      kind: "step", i, action, result_digest: resultDigest,
      reward, refusal: refusal ?? null, ms,
    }) + "\n");
  }

  close({ outcome, rewardFinal, claims, recipeRef, tokens, wallMs }) {
    if (this.#closed) throw new Error("trajectory already closed");
    if (!OUTCOMES.includes(outcome)) {
      throw new Error(
        `unknown outcome ${JSON.stringify(outcome)} — the taxonomy is closed ` +
        `(${OUTCOMES.join(", ")}); a new category is a design change, not a typo`,
      );
    }
    appendFileSync(this.#path, JSON.stringify({
      kind: "terminal", outcome, reward_final: rewardFinal,
      claims, recipe_ref: recipeRef ?? null, tokens, wall_ms: wallMs,
    }) + "\n");
    this.#closed = true;
  }
}

export function openTrajectory({
  path, taskId, seed, kernelSha, mcpVersion, toolAllowlist, split,
}) {
  writeFileSync(path, JSON.stringify({
    kind: "header", schema_version: SCHEMA_VERSION, task_id: taskId, seed,
    kernel_sha: kernelSha, mcp_version: mcpVersion,
    tool_allowlist: toolAllowlist, split,
    started_at: new Date().toISOString(),
    replay: "recipe-level: re-issue recipe_ref. Geometry reproduces to ~4e-8; " +
            "byte-identical replay is NOT promised.",
  }) + "\n");
  return new Trajectory(path);
}

export function readTrajectory(path) {
  const lines = readFileSync(path, "utf8").trim().split("\n").map((l) => JSON.parse(l));
  return {
    header: lines.find((l) => l.kind === "header"),
    steps: lines.filter((l) => l.kind === "step"),
    terminal: lines.find((l) => l.kind === "terminal"),
  };
}
