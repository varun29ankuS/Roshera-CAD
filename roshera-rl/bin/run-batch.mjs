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
import { referencePolicy, scriptedPolicy } from "../lib/policy.mjs";

/**
 * The dimensions the seed task's claims are written against — r=25, h=60
 * (lib/task.mjs:165-166, whose `expected` values are πr²h and 2πr(r+h) over
 * exactly these). Named here rather than inlined so the two numbers the batch
 * requests and the numbers the claims check stay legible as the same pair.
 */
const CYLINDER = Object.freeze({ radius: 25, height: 60 });

const arg = (name, fallback) => {
  const i = process.argv.indexOf(`--${name}`);
  return i === -1 ? fallback : process.argv[i + 1];
};

/**
 * A numeric flag must be a positive integer. `--concurrency` with no value
 * took the NEXT argv entry (or undefined at the end of the line), so
 * `Number(undefined)` → NaN → `Math.min(NaN, n)` → NaN workers →
 * `Array.from({length: NaN})` → zero workers → "0 episodes", an all-zero
 * tally, and EXIT 0: a silent no-op reported as a successful run.
 */
const positiveInt = (name, fallback) => {
  const raw = arg(name, fallback);
  const n = Number(raw);
  if (!Number.isInteger(n) || n <= 0) {
    process.stderr.write(
      `--${name} needs a positive integer, got ${JSON.stringify(raw)}. ` +
      `Refusing to run: a NaN here silently produces zero episodes and an ` +
      `all-zero tally that reads exactly like a clean run.\n`,
    );
    process.exit(2);
  }
  return n;
};

const baseUrl = process.env.ROSHERA_URL ?? "http://127.0.0.1:8081";
const key = process.env.ROSHERA_API_KEY;
const concurrency = positiveInt("concurrency", "4");
const outDir = arg("out", "./runs");
const repeats = positiveInt("repeats", "1");

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

const { tally, results, orphans } = await runBatch({
  tasks, seeds, concurrency, baseUrl,
  authHeader: key ? { Authorization: `ApiKey ${key}` } : {},
  outDir, kernelSha: process.env.ROSHERA_KERNEL_SHA ?? "unknown",
  mcpEntry: process.env.ROSHERA_MCP_ENTRY,
  ...(testSpawn ? { spawn: testSpawn } : {}),
  // The seed task's reference policy. It is NOT `scriptedPolicy` because the
  // last step cannot be scripted: `verify_part` requires the `part_id` the
  // create call returns (roshera-mcp/src/tools/perception.ts:182 /
  // tools/create.ts:256), and a fixed script has no way to read it — the batch
  // sent `args: {}`, the tool rejected it on its schema, and the episode
  // reported COMPLETED having verified nothing. `referencePolicy` reads the id
  // off the observation. A task that cannot build a cylinder gets an empty
  // script and declares done immediately, as before.
  policyFor: (task) => (
    task.toolAllowlist.includes("create_cylinder")
      ? referencePolicy({
          intent: task.prompt.slice(0, 60),
          radius: CYLINDER.radius,
          height: CYLINDER.height,
        })
      : scriptedPolicy([])
  ),
});

process.stdout.write(`\n${results.length} episodes → ${outDir}\n`);
for (const [outcome, n] of Object.entries(tally)) {
  process.stdout.write(`  ${outcome.padEnd(17)} ${n}\n`);
}
// Stated, never assumed clean: a document the reaper could not drop is still
// live in PartManager's DashMap, and silence here would be the assertion that
// it isn't.
if (orphans.length) {
  process.stdout.write(`\n⚠ ${orphans.length} document(s) NOT reaped:\n`);
  for (const o of orphans) process.stdout.write(`  ${o.documentId}  ${o.reason}\n`);
}

/** What this run produced. Exported so the wiring test can assert the entry
 *  point really drove the runner rather than merely mentioning it. */
export const lastRun = { results, tally, orphans };
