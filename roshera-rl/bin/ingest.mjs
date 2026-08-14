#!/usr/bin/env node
/**
 * Ingestion CLI, and the one place `lib/ingest/store.mjs` is ever handed a
 * REAL `pg` client. Two jobs, chosen by argv:
 *
 *   node bin/ingest.mjs <dir>       ingest every .jsonl file directly inside <dir>
 *   node bin/ingest.mjs --verify    re-check every already-ingested file's bytes
 *                                    against the digest recorded at ingest time
 *
 * `runIngest` is exported separately from `main` so `bin/run-batch.mjs` can
 * call it as a library function after a batch completes — that is this
 * module's production call site, proven behaviourally in
 * `test/ingest_wiring.test.mjs`. `main` only exists to turn argv into a
 * `runIngest` call and an exit code; it is never itself imported.
 *
 * JSONL is the source of truth: this command only ever READS trajectory
 * files already on disk (via `ingestDir`/`ingestFile`, Task 6) and never
 * mutates them. Nothing in the episode-running path depends on this
 * command ever having been run.
 */
import pg from "pg";
import { pathToFileURL } from "node:url";
import { ensureSchema } from "../lib/ingest/schema.mjs";
import { ingestDir, verify } from "../lib/ingest/store.mjs";

/**
 * Connect, ensure the schema, do the one thing asked, close the
 * connection. Both call shapes share this lifecycle so argv-parsing in
 * `main` is the only place that branches on the caller's intent.
 *
 * Throws on any failure (a missing connection string, a connection
 * refused, a query error) rather than swallowing it — the caller (this
 * file's own `main`, or `run-batch.mjs`) decides what a failed ingest
 * means for it; this function never decides that on their behalf.
 */
export async function runIngest({ dir, verifyOnly } = {}) {
  const connectionString = process.env.ROSHERA_RL_PG;
  if (!connectionString) {
    throw new Error(
      "ROSHERA_RL_PG is not set - no Postgres connection string to ingest into " +
        "(e.g. postgresql://postgres:postgres@localhost/roshera).",
    );
  }
  const client = new pg.Client({ connectionString });
  try {
    await client.connect();
    await ensureSchema(client);
    if (verifyOnly) {
      const { checked, drifted } = await verify(client);
      return { mode: "verify", checked, drifted };
    }
    if (!dir) {
      throw new Error("runIngest({ dir }) needs a directory to ingest.");
    }
    const results = await ingestDir(client, dir);
    return { mode: "ingest", dir, results };
  } finally {
    await client.end();
  }
}

function usageAndExit(code) {
  process.stderr.write(
    "usage: node bin/ingest.mjs <dir>\n" +
      "       node bin/ingest.mjs --verify\n" +
      "Requires ROSHERA_RL_PG (a Postgres connection string) in the environment.\n",
  );
  process.exit(code);
}

/**
 * Pure formatting/exit-code decision, split out of `main` so it is testable
 * without a Postgres connection: `test/ingest_wiring.test.mjs` calls this
 * directly with a synthetic `{checked, drifted}` to prove the exit code
 * without ever needing a live database. A ratchet that exits zero on drift
 * is decoration: `--verify` is the command an operator (or a CI gate) runs
 * to find out whether the database still agrees with the JSONL it was
 * built from, so drift MUST fail the process, by name, not just narrate it
 * to stdout.
 */
export function reportVerify({ checked, drifted }) {
  const lines = [`verify: ${checked} file(s) checked, ${drifted.length} drifted`];
  for (const d of drifted) {
    lines.push(`  DRIFTED  ${d.path}`, `    ${d.reason}`);
  }
  return { lines, exitCode: drifted.length > 0 ? 1 : 0 };
}

async function main() {
  const args = process.argv.slice(2);
  if (args.length === 0) usageAndExit(2);

  if (args[0] === "--verify") {
    const verifyResult = await runIngest({ verifyOnly: true });
    const { lines, exitCode } = reportVerify(verifyResult);
    for (const line of lines) process.stdout.write(line + "\n");
    process.exit(exitCode);
  }

  const dir = args[0];
  const { results } = await runIngest({ dir });
  const ingested = results.filter((r) => r.status === "ingested").length;
  const quarantined = results.filter((r) => r.status === "quarantined").length;
  process.stdout.write(
    `ingest: ${results.length} file(s) -> ${ingested} ingested, ${quarantined} quarantined\n`,
  );
  for (const r of results) {
    if (r.status === "quarantined") {
      process.stdout.write(`  QUARANTINED  ${r.path}\n    ${r.reason}\n`);
    }
  }
  process.exit(0);
}

// Only drive the CLI when this file is executed directly. `run-batch.mjs`
// and this file's own test import `runIngest` without ever hitting this
// branch, so importing this module never has a side effect of its own.
if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  // A deliberate refusal, not a stack trace: `runIngest` throws with a
  // named reason (a missing ROSHERA_RL_PG, a connection refused, a query
  // error) and this is the one place that decides what an uncaught reason
  // means for the CLI process — print it on stderr and exit non-zero, the
  // same shape `usageAndExit` already uses, instead of letting Node's
  // default uncaught-exception handler print the reason buried inside a
  // stack trace.
  try {
    await main();
  } catch (e) {
    process.stderr.write(`ingest failed: ${e?.message ?? String(e)}\n`);
    process.exit(1);
  }
}
