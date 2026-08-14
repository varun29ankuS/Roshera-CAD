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
const noIngest = process.argv.includes("--no-ingest");

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

// Same seam, for ingestion. The wiring test drives this real entry point
// with an injected fake ingester against no database at all, so the
// override is reachable ONLY through this explicit test env flag; a
// production run always falls through to the real `runIngest`
// (bin/ingest.mjs), imported lazily so importing run-batch.mjs never pulls
// in `pg` for a run that passes `--no-ingest`.
const testIngest = process.env.ROSHERA_RL_TEST_INGEST
  ? globalThis.__roshera_rl_test_ingest
  : undefined;

const { tally, results, orphans } = await runBatch({
  tasks, seeds, concurrency, baseUrl,
  authHeader: key ? { Authorization: `ApiKey ${key}` } : {},
  outDir,
  // The OPERATOR'S CLAIM, and nothing else — `runBatch` asks the server what
  // it actually is and refuses the whole batch if the two disagree
  // (provenance.mjs `resolveKernelIdentity`). This used to default to the
  // literal string `"unknown"`, which is exactly the kind of value the claim
  // check exists to catch: every batch run without the env var set would have
  // "claimed" a build named unknown, disagreed with whatever the live server
  // actually reports, and refused before a single episode ran. Leaving it
  // `undefined` when unset means "no claim was made" — the server's own
  // answer is recorded either way.
  kernelSha: process.env.ROSHERA_KERNEL_SHA,
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
// MODEL ISOLATION, per episode, from each session's own `list_parts` read
// (lib/mcp_session.mjs `readModelScope`). Every episode here builds ONE
// cylinder, so an isolated session sees exactly one solid; a session that sees
// more is reading another episode's `BRepModel` — which is what eight
// concurrent episodes did on 2026-08-13, with nothing in the output saying so.
// Episodes that never scored report an absence and are counted as such, never
// as isolated.
const scopes = results.map((r) => r.modelScope ?? { absent: "no reading was returned" });
const shared = scopes.filter((s) => s.shared_model_detected === true);
const unread = scopes.filter((s) => typeof s.absent === "string");
process.stdout.write(
  `\nmodel isolation: ${scopes.length - shared.length - unread.length}/${results.length} ` +
  `episodes saw only what they built` +
  (unread.length ? `, ${unread.length} took no reading` : "") + `\n`,
);
for (const s of shared) {
  process.stdout.write(
    `  ⚠ SHARED MODEL: ${s.visible_count} solid(s) visible [${s.visible_parts.join(", ")}], ` +
    `${s.built_here} built here\n`,
  );
}

// Stated, never assumed clean: a document or part the reaper could not drop is
// still live in PartManager's DashMap, and silence here would be the assertion
// that it isn't.
if (orphans.length) {
  process.stdout.write(`\n⚠ ${orphans.length} resource(s) NOT reaped:\n`);
  for (const o of orphans) {
    const what = o.documentId ? `document ${o.documentId}` : `part ${o.partId}`;
    process.stdout.write(`  ${what}  ${o.reason}\n`);
  }
}

/**
 * Ingestion into Postgres, run AFTER the batch and reported but never
 * allowed to fail it: `results`/`tally`/`orphans` above are already
 * complete by the time this runs (the runner wrote every episode's JSONL
 * itself, lib/episode.mjs), so JSONL on disk is already the full, correct
 * record. A database that is down, unreachable, or rejects a row costs
 * nobody a trajectory — it costs a line in this run's own output, which is
 * exactly what `lastIngest` exists to make assertable rather than
 * swallowed. `--no-ingest` skips the attempt outright (no ROSHERA_RL_PG in
 * this environment, or a deliberate dry run).
 */
export let lastIngest = { ran: false };
if (!noIngest) {
  const ingest = testIngest
    ?? ((dir) => import("./ingest.mjs").then((m) => m.runIngest({ dir })));
  try {
    const result = await ingest(outDir);
    lastIngest = { ran: true, ok: true, result };
    const n = result?.results?.length ?? 0;
    process.stdout.write(`\ningest: ${n} file(s) from ${outDir}\n`);
  } catch (e) {
    // Surfaced, never silent: stderr says the database step failed, but
    // nothing above this block changes, and nothing below throws.
    lastIngest = { ran: true, ok: false, error: e?.message ?? String(e) };
    process.stderr.write(`\n⚠ ingest failed (batch still succeeded): ${lastIngest.error}\n`);
  }
}

/** What this run produced. Exported so the wiring test can assert the entry
 *  point really drove the runner rather than merely mentioning it. */
export const lastRun = { results, tally, orphans };
