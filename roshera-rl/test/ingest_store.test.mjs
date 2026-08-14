/**
 * Store proof — the impure half of ingestion, against a REAL Postgres.
 *
 * This suite needs a live database and is the ONE test in this package
 * allowed to talk to one. It is gated on `ROSHERA_RL_PG` (a connection
 * string, e.g. `postgresql://postgres:postgres@localhost/roshera`) and,
 * true to this project's own rule ("absence is stated with a reason, never
 * defaulted"), a missing `ROSHERA_RL_PG` is not silently treated as "pass":
 * it SKIPS, loudly, naming exactly why — a skipped test that says why is
 * honest; one that silently passes is a defect, not a convenience.
 *
 * Every assertion below is scoped by `episode_id` / `path`, never a bare
 * `SELECT count(*) FROM rl_x` — `ROSHERA_RL_PG` is expected to point at a
 * real shared dev database (session-manager's own default,
 * `postgresql://postgres:postgres@localhost/roshera`), so other rows may
 * already live in these tables from other work. Cleanup runs at the START
 * of this file, not the end, so a crashed prior run never poisons the next.
 */
import assert from "node:assert/strict";
import { readFileSync, writeFileSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const completePath = join(here, "fixtures", "complete.jsonl");
const malformedPath = join(here, "fixtures", "malformed.jsonl");

const connectionString = process.env.ROSHERA_RL_PG;

if (!connectionString) {
  process.stdout.write(
    "skip - ingest_store: ROSHERA_RL_PG is not set, so there is no live Postgres to " +
      "ingest into. This suite proves the schema and the idempotent store against a " +
      "real database and cannot do that without one. Set ROSHERA_RL_PG to a Postgres " +
      "connection string (e.g. postgresql://postgres:postgres@localhost/roshera) to run it.\n",
  );
  process.stdout.write("\ningest_store: 0 checks passed, 1 suite skipped (no ROSHERA_RL_PG)\n");
  process.exit(0);
}

const pg = await import("pg");
const { ensureSchema } = await import("../lib/ingest/schema.mjs");
const { ingestFile, ingestDir, verify } = await import("../lib/ingest/store.mjs");
const { rowsFromTrajectory } = await import("../lib/ingest/rows.mjs");

const client = new pg.default.Client({ connectionString });
await client.connect();

const completeText = readFileSync(completePath, "utf8");
const malformedText = readFileSync(malformedPath, "utf8");
const fixtureRows = rowsFromTrajectory(completeText, { path: completePath });
const fixtureEpisodeId = fixtureRows.episode.episode_id;
const fixtureRunId = fixtureRows.run.run_id;

// A deterministic sibling: same producer identity (same run), a DIFFERENT
// task_id/seed so it gets its own episode_id — this is the file the drift
// check mutates, kept separate from the file the count/idempotency checks
// use so the two check groups never interfere with each other.
const siblingHeader = JSON.parse(completeText.trim().split("\n")[0]);
const siblingSeed = 999901;
const siblingRest = completeText.trim().split("\n").slice(1);
const siblingText = [
  JSON.stringify({ ...siblingHeader, task_id: "cylinder-r25-h60-drift-probe", seed: siblingSeed }),
  ...siblingRest,
].join("\n") + "\n";
const siblingEpisodeId = rowsFromTrajectory(siblingText, { path: "sibling" }).episode.episode_id;

// A second sibling: the file used for the "fixed after quarantine" arc
// (Finding 1) — starts malformed at this path, then gets overwritten with
// THIS valid content and re-ingested. Its own distinct episode_id keeps it
// from interfering with the counts/idempotency checks above.
const fixArcSeed = 999902;
const fixArcText = [
  JSON.stringify({ ...siblingHeader, task_id: "cylinder-r25-h60-quarantine-fix-probe", seed: fixArcSeed }),
  ...siblingRest,
].join("\n") + "\n";
const fixArcEpisodeId = rowsFromTrajectory(fixArcText, { path: "fix-arc" }).episode.episode_id;

const scratchDir = mkdtempSync(join(tmpdir(), "roshera-rl-ingest-store-"));
const scratchPath = join(scratchDir, "drift-probe.jsonl");
const fixArcPath = join(scratchDir, "quarantine-fix-probe.jsonl");

async function countWhere(table, whereSql, params) {
  const r = await client.query(`SELECT count(*)::int AS n FROM ${table} WHERE ${whereSql}`, params);
  return r.rows[0].n;
}

async function cleanup() {
  // rl_episode cascades to rl_step/rl_refusal/rl_claim_result/rl_recipe_step/
  // rl_solid/rl_lineage_edge/rl_recipe/rl_certificate (ON DELETE CASCADE) —
  // deleting the episode rows is enough to clear their whole subtree.
  await client.query(`DELETE FROM rl_episode WHERE episode_id = $1 OR episode_id = $2 OR episode_id = $3`, [
    fixtureEpisodeId, siblingEpisodeId, fixArcEpisodeId,
  ]);
  await client.query(`DELETE FROM rl_quarantine WHERE path = $1 OR path = $2`, [malformedPath, fixArcPath]);
}

const checks = [];
const check = (name, fn) => checks.push([name, fn]);

check("ensureSchema creates every rl_ table without error, and is itself idempotent", async () => {
  await ensureSchema(client);
  await ensureSchema(client); // CREATE TABLE IF NOT EXISTS — must tolerate being called twice
});

/**
 * Expected counts are derived from `rowsFromTrajectory`'s OWN pure output
 * for this exact fixture text (`fixtureRows`, computed above), never
 * hardcoded literals — `test/fixtures/complete.jsonl` is a real, evolving
 * fixture shared with Task 5's own suite, not this suite's private data, so
 * asserting a fixed step/recipe-step/solid count here would make this test
 * brittle to a fixture edit that has nothing to do with the store. What
 * this suite must prove is narrower and more durable: whatever rows.mjs
 * computed from the file is exactly what landed in Postgres, no more, no
 * less, no matter how many times the file is ingested.
 */
async function assertCountsMatchFixture(label) {
  assert.equal(await countWhere("rl_run", "run_id = $1", [fixtureRunId]), 1, `rl_run ${label}`);
  assert.equal(await countWhere("rl_episode", "episode_id = $1", [fixtureEpisodeId]), 1, `rl_episode ${label}`);
  assert.equal(await countWhere("rl_step", "episode_id = $1", [fixtureEpisodeId]), fixtureRows.steps.length, `rl_step ${label}`);
  assert.equal(await countWhere("rl_refusal", "episode_id = $1", [fixtureEpisodeId]), fixtureRows.refusals.length, `rl_refusal ${label}`);
  assert.equal(await countWhere("rl_claim_result", "episode_id = $1", [fixtureEpisodeId]), fixtureRows.claims.length, `rl_claim_result ${label}`);
  assert.equal(await countWhere("rl_recipe", "episode_id = $1", [fixtureEpisodeId]), fixtureRows.recipe ? 1 : 0, `rl_recipe ${label}`);
  assert.equal(await countWhere("rl_recipe_step", "episode_id = $1", [fixtureEpisodeId]), fixtureRows.recipeSteps.length, `rl_recipe_step ${label}`);
  assert.equal(await countWhere("rl_solid", "episode_id = $1", [fixtureEpisodeId]), fixtureRows.solids.length, `rl_solid ${label}`);
  assert.equal(await countWhere("rl_lineage_edge", "episode_id = $1", [fixtureEpisodeId]), fixtureRows.lineageEdges.length, `rl_lineage_edge ${label}`);
  assert.equal(
    await countWhere("rl_certificate", "episode_id = $1", [fixtureEpisodeId]),
    fixtureRows.recipe?.certificate_summary ? 1 : 0,
    `rl_certificate ${label}`,
  );
}

check("ingesting the complete fixture produces row counts matching rows.mjs's own computation, scoped to this episode", async () => {
  const res = await ingestFile(client, completePath);
  assert.equal(res.status, "ingested");
  assert.equal(res.episode_id, fixtureEpisodeId);

  await assertCountsMatchFixture("after first ingest");

  // dimension rollups: the fixture's kernel/task identity are real (attributable: true)
  assert.equal(await countWhere("rl_kernel_build", "sha = $1", [fixtureRows.run.provenance.kernel.sha]), 1);
  assert.equal(await countWhere("rl_task_family", "name = $1", [fixtureRows.run.provenance.task.family]), 1);
});

check("ingesting the SAME file again leaves every row count unchanged — idempotency", async () => {
  const res = await ingestFile(client, completePath);
  assert.equal(res.status, "ingested");

  await assertCountsMatchFixture("after second (repeat) ingest");
});

check("a malformed file is quarantined, not dropped, and re-ingesting it is idempotent too", async () => {
  const first = await ingestFile(client, malformedPath);
  assert.equal(first.status, "quarantined");
  assert.match(first.reason, /not valid json/i);
  assert.equal(await countWhere("rl_quarantine", "path = $1", [malformedPath]), 1);

  const second = await ingestFile(client, malformedPath);
  assert.equal(second.status, "quarantined");
  assert.equal(await countWhere("rl_quarantine", "path = $1", [malformedPath]), 1, "still exactly one quarantine row for this path");
});

check("a path that was quarantined and is then fixed clears its stale quarantine row, and verify() reports no drift", async () => {
  // Arrives malformed first.
  writeFileSync(fixArcPath, malformedText, "utf8");
  const first = await ingestFile(client, fixArcPath);
  assert.equal(first.status, "quarantined");
  assert.equal(await countWhere("rl_quarantine", "path = $1", [fixArcPath]), 1,
    "the malformed arrival is quarantined");

  // Someone fixes it on disk: same path, now valid content.
  writeFileSync(fixArcPath, fixArcText, "utf8");
  const second = await ingestFile(client, fixArcPath);
  assert.equal(second.status, "ingested");
  assert.equal(second.episode_id, fixArcEpisodeId);

  assert.equal(await countWhere("rl_quarantine", "path = $1", [fixArcPath]), 0,
    "the stale quarantine row for this path must be gone — it no longer describes this path's state");
  assert.equal(await countWhere("rl_episode", "episode_id = $1", [fixArcEpisodeId]), 1);

  const result = await verify(client);
  assert.ok(
    !result.drifted.some((d) => d.path === fixArcPath),
    "verify() must not report drift for a path whose quarantine record was correctly cleared — " +
      "a drift ratchet that cries over its own stale bookkeeping trains its operator to ignore it",
  );
});

check("re-ingesting an episode whose certificate_summary has vanished deletes the stale rl_certificate row, leaving rl_recipe intact", async () => {
  // fixArcPath currently holds fixArcText (ingested by the previous check).
  // Every other fixture in this suite carries a certificate_summary, so
  // without this check upsertRecipe's "cert vanished, DELETE it" branch
  // (store.mjs, Finding-2 rewrite) would be implemented but never actually
  // run by this suite.
  const lines = fixArcText.trim().split("\n");
  const terminal = JSON.parse(lines[lines.length - 1]);
  assert.ok(terminal.recipe_ref?.certificate_summary, "sanity: the fixture terminal does carry one to remove");
  delete terminal.recipe_ref.certificate_summary;
  lines[lines.length - 1] = JSON.stringify(terminal);
  const noCertText = lines.join("\n") + "\n";

  writeFileSync(fixArcPath, noCertText, "utf8");
  const res = await ingestFile(client, fixArcPath);
  assert.equal(res.status, "ingested");
  assert.equal(res.episode_id, fixArcEpisodeId, "same episode identity — task_id/seed/started_at unchanged");

  assert.equal(await countWhere("rl_certificate", "episode_id = $1", [fixArcEpisodeId]), 0,
    "the certificate row must not survive a re-ingest whose recipe no longer carries a certificate_summary");
  assert.equal(await countWhere("rl_recipe", "episode_id = $1", [fixArcEpisodeId]), 1,
    "the recipe row itself is still present — only its certificate rollup disappeared");
});

check("ingestDir walks every .jsonl fixture in a directory, ingesting the good one and quarantining the bad one", async () => {
  const results = await ingestDir(client, join(here, "fixtures"));
  const complete = results.find((r) => r.path === completePath);
  const malformed = results.find((r) => r.path === malformedPath);
  assert.ok(complete, "the complete fixture was visited");
  assert.equal(complete.status, "ingested");
  assert.equal(complete.episode_id, fixtureEpisodeId);
  assert.ok(malformed, "the malformed fixture was visited");
  assert.equal(malformed.status, "quarantined");
  // fixtures/ has exactly two .jsonl files today (build_fixtures.mjs does not
  // count — it is not .jsonl); a stray fixture added later would show up
  // here as a new element, not silently vanish.
  assert.equal(results.length, 2);
});

check("verify() reports a mutated file's bytes as drifted", async () => {
  writeFileSync(scratchPath, siblingText, "utf8");
  const res = await ingestFile(client, scratchPath);
  assert.equal(res.status, "ingested");
  assert.equal(res.episode_id, siblingEpisodeId);

  const before = await verify(client);
  assert.ok(before.checked > 0, "verify examined at least the file just ingested");
  assert.ok(
    !before.drifted.some((d) => d.path === scratchPath),
    "immediately after ingest, the file matches what was recorded — no drift yet",
  );

  // Mutate the file ON DISK without re-ingesting it — this is the scenario
  // verify() exists to catch: the database now describes bytes that no
  // longer exist.
  writeFileSync(scratchPath, siblingText + "\n// mutated after ingest, never re-ingested\n", "utf8");

  const after = await verify(client);
  assert.ok(after.checked >= before.checked);
  const drifted = after.drifted.find((d) => d.path === scratchPath);
  assert.ok(drifted, "verify must report the mutated scratch file as drifted");
  assert.match(drifted.reason, /changed since it was ingested/);
});

async function main() {
  // Cleanup runs against tables that must already exist — ensure the schema
  // once up front (the "ensureSchema is idempotent" check below re-runs it
  // and is what actually proves the property; this call just makes cleanup
  // safe to run before that check does).
  await ensureSchema(client);
  await cleanup();
  try {
    for (const [name, fn] of checks) {
      await fn();
      process.stdout.write(`  ok - ${name}\n`);
    }
    process.stdout.write(`\ningest_store: ${checks.length} checks passed\n`);
  } finally {
    await cleanup();
    rmSync(scratchDir, { recursive: true, force: true });
    await client.end();
  }
}

await main();
