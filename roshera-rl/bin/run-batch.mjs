#!/usr/bin/env node
/**
 * Run a batch of episodes against a live Roshera backend.
 *
 *   ROSHERA_URL=http://127.0.0.1:8081 ROSHERA_API_KEY=... \
 *     node bin/run-batch.mjs --concurrency 4 --out ./runs
 *
 * Slice 1 drives the scripted reference policy: this proves the loop, not the
 * agent. Model-backed policies arrive in slice 2 behind the same seam.
 */
import { runBatch } from "../lib/runner.mjs";
import { TASKS } from "../lib/task.mjs";
import { scriptedPolicy } from "../lib/policy.mjs";

const arg = (name, fallback) => {
  const i = process.argv.indexOf(`--${name}`);
  return i === -1 ? fallback : process.argv[i + 1];
};

const baseUrl = process.env.ROSHERA_URL ?? "http://127.0.0.1:8081";
const key = process.env.ROSHERA_API_KEY;
const concurrency = Number(arg("concurrency", "4"));
const outDir = arg("out", "./runs");
const repeats = Number(arg("repeats", "1"));

const tasks = [];
const seeds = [];
for (let r = 0; r < repeats; r += 1) {
  for (const t of TASKS) { tasks.push(t); seeds.push(r); }
}

// The wiring test drives this real entry point against a stub backend, so the
// session factory is overridable ONLY through an explicit test env flag. A
// production run never reads it, and the default remains the real stdio client.
const testSpawn = process.env.ROSHERA_RL_TEST_SPAWN
  ? globalThis.__roshera_rl_test_spawn
  : undefined;

const { tally, results } = await runBatch({
  tasks, seeds, concurrency, baseUrl,
  authHeader: key ? { Authorization: `ApiKey ${key}` } : {},
  outDir, kernelSha: process.env.ROSHERA_KERNEL_SHA ?? "unknown",
  mcpEntry: process.env.ROSHERA_MCP_ENTRY,
  ...(testSpawn ? { spawn: testSpawn } : {}),
  policyFor: (task) => scriptedPolicy(
    task.toolAllowlist.includes("create_cylinder")
      ? [{ tool: "timeline_checkpoint", args: { name: task.prompt.slice(0, 60) } },
         { tool: "create_cylinder", args: { radius: 25, height: 60 } },
         { tool: "verify_part", args: {} }]
      : [],
  ),
});

process.stdout.write(`\n${results.length} episodes → ${outDir}\n`);
for (const [outcome, n] of Object.entries(tally)) {
  process.stdout.write(`  ${outcome.padEnd(17)} ${n}\n`);
}

/** What this run produced. Exported so the wiring test can assert the entry
 *  point really drove the runner rather than merely mentioning it. */
export const lastRun = { results, tally };
