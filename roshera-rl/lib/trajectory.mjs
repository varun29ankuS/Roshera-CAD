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
 *                       model_scope, tokens, wall_ms, error }
 *
 * `reward_final` is the terminal reading of each NAMED component — not a sum
 * and not a scalar. Naming it `total` would imply an aggregation the
 * environment deliberately refuses to perform: weighting soundness against
 * fidelity against refusal count is a training choice with no kernel
 * justification, so consumers scalarize and the environment reports.
 *
 * `recipe_ref` carries the build's timeline lineage so it is RE-ISSUABLE.
 * Geometry reproduces to ~4e-8 and is not bit-stable, so replay is
 * recipe-level, never byte-level. Stating that in the schema stops a
 * downstream consumer assuming a determinism the kernel never promised.
 *
 * It is an OBJECT, and it is never a bare null. `recipe_get` returns
 * `{source, step_count, sequence_range, sequence_contiguous,
 * undecodable_events, checkpoints, certificate_summary, steps}`
 * (roshera-mcp/src/tools/timeline.ts:267-283) — there is no `ref` field on it
 * and there never was, so a `recipe_ref` string was structurally null in
 * every trajectory this schema ever produced. Two forms appear here now:
 *
 *   - a RECIPE: the fields above, including the `steps` themselves. The steps
 *     are embedded because the address does not survive the episode — the
 *     document is deleted at reap and `DELETE /api/documents` purges its
 *     `timeline_events` (session-manager/src/database.rs:1704) — so a
 *     descriptor alone would be a dangling pointer and the replay guarantee
 *     would be vacuous;
 *   - an ABSENCE: `{absent: "<reason>"}`. Every non-COMPLETED outcome takes
 *     this branch, because terminal scoring never ran. A bare null would read
 *     as "there was no recipe", which is a different and false claim.
 *
 * `claims` follows the same rule: it is never `[]` for a task that declares
 * claims (`defineTask` refuses a task with none), so an empty array could
 * only ever mean "we did not check" — which is stated per claim as
 * `{name, verified: null, absent: "<reason>"}` instead.
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
  "INVALID_ACTION",   // the policy named a tool outside its own declared action space
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

  close({ outcome, rewardFinal, claims, recipeRef, modelScope, tokens, wallMs, error }) {
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
      // WHAT THIS EPISODE'S OWN MODEL HELD, read by `list_parts` inside the
      // session (mcp_session.mjs `readModelScope`). Concurrent episodes shared
      // one `BRepModel` for as long as this environment has existed, and
      // nothing in the record said so — the evidence was an unexplained
      // `part_id` in the 70s-90s. An OBJECT, never a bare null: an episode
      // that took no reading says so with a reason.
      model_scope: modelScope ?? { absent: "the caller recorded no model-scope reading" },
      // THE UNDERLYING ERROR, in the record and not only in the return value.
      // A batch is read afterwards from its trajectories, so a failure whose
      // reason lives only in the returned object is a failure nobody can
      // diagnose without re-running it — measured: two SETUP_FAILED episodes
      // (a 401 on document creation, a spawn that died on a missing
      // dependency) were indistinguishable in their records. ALWAYS PRESENT,
      // `null` when the episode had no error, so an absent key is never
      // mistaken for a clean run.
      error: error ?? null,
    }) + "\n");
    this.#closed = true;
  }
}

export function openTrajectory({
  path, taskId, seed, kernelSha, mcpVersion, toolAllowlist, split, provenance,
}) {
  writeFileSync(path, JSON.stringify({
    kind: "header", schema_version: SCHEMA_VERSION, task_id: taskId, seed,
    kernel_sha: kernelSha, mcp_version: mcpVersion,
    tool_allowlist: toolAllowlist, split,
    started_at: new Date().toISOString(),
    replay: "recipe-level: re-issue recipe_ref. Geometry reproduces to ~4e-8; " +
            "byte-identical replay is NOT promised.",
    // THE FULL PROVENANCE BLOCK — kernel/mcp/policy/harness identity and the
    // single `attributable` flag a consumer filters on. `provenance` is not
    // yet REQUIRED here because not every caller of this constructor has
    // assembled one, but an unassembled block is an ABSENCE, not a null: this
    // is the identity field the whole plan exists to attach, so a caller that
    // skipped it says why, the same way `episode.mjs`'s `model_scope` does a
    // few lines below.
    //
    // THE REASON STATES ONLY WHAT THIS FUNCTION CAN KNOW. It used to say the
    // call site "predates buildProvenance" — a guess about the caller, and a
    // false one at both of `runner.mjs`'s setup-failure paths, one of which is
    // literally inside the catch block of a `buildProvenance` call. Since
    // `rows.mjs` persists this block whole into `rl_run.provenance`, that
    // false reason reached the corpus. All this function observes is that no
    // block arrived and no reason came with it; WHY is the caller's to state,
    // and every caller in this package now does.
    provenance: provenance ?? {
      absent: "the caller passed no provenance block and stated no reason for its absence",
    },
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
