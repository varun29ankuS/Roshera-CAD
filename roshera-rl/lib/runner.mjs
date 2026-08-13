/**
 * Run N episodes under a concurrency cap, tally the outcomes, and REAP.
 *
 * The cap is real: process-per-episode means memory, and the api-server's
 * EvalHarness rate class (6000 req/min) is shared across every concurrent
 * episode. Both are measured here rather than assumed — RATE_LIMITED is its
 * own outcome precisely so the ceiling shows up in the tally instead of
 * being averaged into a lower score.
 */
import { join } from "node:path";
import { mkdirSync } from "node:fs";
import { runEpisode, reapDocument, unscoredFor, MCP_VERSION } from "./episode.mjs";
import { OUTCOMES, openTrajectory } from "./trajectory.mjs";
import { spawnMcpSession } from "./mcp_session.mjs";
import { mergeFinal } from "./reward.mjs";

/**
 * THE REAPER — the batch-level backstop `episode.mjs` names.
 *
 * Each episode already attempts its own DELETE and REPORTS the outcome
 * (`result.reap`). This retries every document that attempt could not drop —
 * a DELETE lost to a network blip, or refused because the document was
 * momentarily the active one (api-server/src/documents.rs:561-564) — and then
 * states plainly which documents are STILL orphaned in PartManager's DashMap.
 *
 * One retry pass, not a loop: a document the backend refuses twice is not
 * going to yield to a third identical request, and a batch runner that spins
 * on cleanup would hold the rate class the next batch needs. What it will not
 * do is claim a cleanup it did not achieve — the survivors are returned so
 * the caller can print them.
 */
export async function reapOrphans({ baseUrl, authHeader, results }) {
  const orphans = [];
  for (const r of results) {
    if (!r?.documentId) continue;
    if (r.reap?.reaped === true) continue;
    const retry = await reapDocument(baseUrl, authHeader, r.documentId);
    if (retry.reaped === true) {
      r.reap = { reaped: true, reason: `reaped by the batch reaper after: ${r.reap?.reason ?? "no first attempt"}` };
      continue;
    }
    r.reap = { reaped: false, reason: retry.reason };
    orphans.push({ documentId: r.documentId, reason: retry.reason });
  }
  return orphans;
}

/**
 * The record for an episode whose POLICY FACTORY threw — the one failure that
 * happens before `runEpisode` is entered, so `runEpisode` cannot record it.
 *
 * `SETUP_FAILED`, not `CRASHED`: the taxonomy is closed (trajectory.mjs
 * OUTCOMES — a new category is a design change, not a typo), and of the six,
 * SETUP_FAILED is the one that means "no episode happened". `CRASHED` says the
 * MCP process died, and here no process was ever spawned — no document was
 * created either, which is why `documentId` is null and nothing is reaped.
 *
 * A TRAJECTORY IS STILL WRITTEN. A batch is read afterwards from its
 * trajectories, so a result carrying a `trajectoryPath` that no file backs is
 * an episode nobody can diagnose without re-running the batch — the same
 * defect the `error` field was added to `close()` to fix. The header/terminal
 * pair here is the same shape every other SETUP_FAILED episode writes, built
 * from the same `unscoredFor` so the two cannot drift.
 */
function policyFactoryFailed({ item, trajectoryPath, kernelSha, error }) {
  const detail = `the policy factory threw before the episode began: ${error}`;
  const rewardFinal = mergeFinal([]);
  const { claims, recipeRef } = unscoredFor(item.task, "SETUP_FAILED", detail);
  try {
    const traj = openTrajectory({
      path: trajectoryPath, taskId: item.task.id, seed: item.seed, kernelSha,
      mcpVersion: MCP_VERSION, toolAllowlist: [...item.task.toolAllowlist],
      split: item.task.split,
    });
    traj.close({
      outcome: "SETUP_FAILED", rewardFinal, claims, recipeRef,
      tokens: 0, wallMs: 0, error: detail,
    });
  } catch (e) {
    // An unwritable outDir must not turn one episode's setup failure into a
    // dead batch — that is the very defect this function exists to remove. The
    // returned result still carries the reason, and says the record is missing
    // rather than leaving a dangling path unexplained.
    return {
      outcome: "SETUP_FAILED", rewardFinal, documentId: null,
      trajectoryPath: null, wallMs: 0,
      error: `${detail} (and its trajectory could not be written: ${String(e?.message ?? e)})`,
      reap: { reaped: null, reason: "no document was created" },
    };
  }
  return {
    outcome: "SETUP_FAILED", rewardFinal, documentId: null, trajectoryPath,
    wallMs: 0, error: detail,
    reap: { reaped: null, reason: "no document was created" },
  };
}

export async function runBatch({
  tasks, policyFor, seeds, concurrency = 4, baseUrl, authHeader = {},
  outDir, kernelSha, mcpEntry, spawn = spawnMcpSession,
}) {
  mkdirSync(outDir, { recursive: true });
  const queue = tasks.map((task, i) => ({ task, seed: seeds[i] ?? i, i }));
  const results = [];

  const worker = async () => {
    for (;;) {
      const item = queue.shift();
      if (item === undefined) return;
      const trajectoryPath = join(outDir, `${item.task.id}-${item.seed}-${item.i}.jsonl`);
      // THE POLICY FACTORY IS THIRD-PARTY CODE AND RUNS BEFORE THE EPISODE.
      // It used to be called while building `runEpisode`'s argument object,
      // i.e. outside the `.catch` below: a factory that threw rejected this
      // worker, then `Promise.all`, then the whole batch — taking down every
      // sibling episode, including ones that had already finished and been
      // recorded. Every other failure mode here is a named per-episode
      // outcome, and this one now is too.
      let policy;
      try {
        policy = policyFor(item.task, item.seed);
      } catch (e) {
        results.push(
          policyFactoryFailed({
            item, trajectoryPath, kernelSha, error: String(e?.message ?? e),
          }),
        );
        continue;
      }
      // An episode never throws: every failure mode is a named outcome. A
      // worker that could die on one episode would silently shrink the batch.
      const r = await runEpisode({
        task: item.task, policy, seed: item.seed,
        baseUrl, authHeader, trajectoryPath, kernelSha, mcpEntry, spawn,
      }).catch((e) => ({
        outcome: "CRASHED", rewardFinal: { components: {}, gaps: [] },
        documentId: null, trajectoryPath, wallMs: 0, error: String(e?.message ?? e),
        reap: { reaped: null, reason: "the episode threw before reporting a document" },
      }));
      results.push(r);
    }
  };

  await Promise.all(
    Array.from({ length: Math.max(1, Math.min(concurrency, queue.length)) }, worker),
  );

  const orphans = await reapOrphans({ baseUrl, authHeader, results });

  // Every outcome name appears, zeros included: an absent key reads as "not
  // measured", which is the one thing this taxonomy exists to prevent.
  const tally = Object.fromEntries(OUTCOMES.map((o) => [o, 0]));
  for (const r of results) tally[r.outcome] = (tally[r.outcome] ?? 0) + 1;
  return { results, tally, orphans };
}
