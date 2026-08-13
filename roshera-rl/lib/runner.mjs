/**
 * Run N episodes under a concurrency cap and tally the outcomes.
 *
 * The cap is real: process-per-episode means memory, and the api-server's
 * EvalHarness rate class (6000 req/min) is shared across every concurrent
 * episode. Both are measured here rather than assumed — RATE_LIMITED is its
 * own outcome precisely so the ceiling shows up in the tally instead of
 * being averaged into a lower score.
 */
import { join } from "node:path";
import { mkdirSync } from "node:fs";
import { runEpisode } from "./episode.mjs";
import { OUTCOMES } from "./trajectory.mjs";
import { spawnMcpSession } from "./mcp_session.mjs";

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
      // An episode never throws: every failure mode is a named outcome. A
      // worker that could die on one episode would silently shrink the batch.
      const r = await runEpisode({
        task: item.task, policy: policyFor(item.task, item.seed), seed: item.seed,
        baseUrl, authHeader, trajectoryPath, kernelSha, mcpEntry, spawn,
      }).catch((e) => ({
        outcome: "CRASHED", rewardFinal: { components: {}, gaps: [] },
        documentId: null, trajectoryPath, wallMs: 0, error: String(e?.message ?? e),
      }));
      results.push(r);
    }
  };

  await Promise.all(
    Array.from({ length: Math.max(1, Math.min(concurrency, queue.length)) }, worker),
  );

  // Every outcome name appears, zeros included: an absent key reads as "not
  // measured", which is the one thing this taxonomy exists to prevent.
  const tally = Object.fromEntries(OUTCOMES.map((o) => [o, 0]));
  for (const r of results) tally[r.outcome] = (tally[r.outcome] ?? 0) + 1;
  return { results, tally };
}
