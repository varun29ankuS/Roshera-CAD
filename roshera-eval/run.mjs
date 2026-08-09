#!/usr/bin/env node
/**
 * AGENT-EVAL-α runner — executes the scored CAD benchmark against the live
 * Roshera backend, scores every job by the kernel's own certificates + exact
 * analytic oracles, prints a scorecard, and writes a JSON report.
 *
 * Usage:
 *   node run.mjs                     # run the whole corpus
 *   node run.mjs 01-gear 02-nozzle   # run named scenarios only
 *   ROSHERA_URL=http://host:8081 node run.mjs
 *   node run.mjs --json out.json     # write the machine report here
 *
 * Prerequisite: a live backend (default http://127.0.0.1:8081). Exit code =
 * number of FAILED scenarios (0 = suite green). The saddle honesty canary
 * PASSES when the kernel honestly flags it unsound.
 */
import { writeFile } from "node:fs/promises";
import { makeClient, BASE, setAuthToken, request } from "./lib/client.mjs";
import * as geom from "./lib/geom.mjs";
import { scenarios as ALL } from "./scenarios/index.mjs";
import { runSuite, summarize, scorecard } from "./lib/harness.mjs";

async function main() {
  const args = process.argv.slice(2);
  let jsonOut = "report.json";
  const names = [];
  for (let i = 0; i < args.length; i++) {
    if (args[i] === "--json") jsonOut = args[++i];
    else names.push(args[i]);
  }
  const scenarios = names.length ? ALL.filter((s) => names.includes(s.id)) : ALL;
  if (scenarios.length === 0) {
    console.error(`No scenarios matched ${JSON.stringify(names)}. Known: ${ALL.map((s) => s.id).join(", ")}`);
    process.exit(2);
  }

  const client = makeClient();

  // Preflight: the backend must be live.
  try {
    const h = await client.raw("GET", "/health", undefined, 5000);
    if (h.data?.status !== "healthy") throw new Error(`health = ${h.data?.status}`);
    process.stdout.write(`Backend live at ${BASE} (uptime ${h.data?.uptime ?? "?"})\n`);
  } catch (e) {
    console.error(`\nFATAL: backend not reachable at ${BASE} — ${e.message}\n` +
      `Start the api-server on :8081 (or set ROSHERA_URL) and retry.\n`);
    process.exit(2);
  }

  // Auth: the live backend now gates mutating endpoints (clear_parts,
  // geometry create/boolean, ...) behind a bearer token. Log in once and
  // let every subsequent request (through this same `client` and any raw
  // `request()` a scenario makes) carry it. Override with
  // ROSHERA_USERNAME/ROSHERA_PASSWORD or skip entirely with ROSHERA_TOKEN.
  if (!process.env.ROSHERA_TOKEN) {
    const username = process.env.ROSHERA_USERNAME ?? "varun";
    const password = process.env.ROSHERA_PASSWORD ?? "Roshera321!";
    try {
      const auth = await request("POST", "/api/auth/login", { username, password }, 10000);
      if (!auth.ok || !auth.data?.token) {
        throw new Error(`login -> ${auth.status}: ${JSON.stringify(auth.data)?.slice(0, 200)}`);
      }
      setAuthToken(auth.data.token);
      process.stdout.write(`Authenticated as ${username}\n`);
    } catch (e) {
      console.error(`\nFATAL: auth login failed — ${e.message}\n`);
      process.exit(2);
    }
  }

  const results = await runSuite(scenarios, client, geom);
  const summary = summarize(results);
  process.stdout.write(scorecard(results, summary));

  // Clear the model so the honesty-canary debris does not linger for the human.
  try {
    await client.clearParts();
  } catch {
    /* best effort */
  }

  const report = {
    tool: "AGENT-EVAL-alpha",
    version: 1,
    backend: BASE,
    timestamp: new Date().toISOString(),
    summary,
    scenarios: results,
  };
  await writeFile(jsonOut, JSON.stringify(report, null, 2));
  process.stdout.write(`JSON report written to ${jsonOut}\n`);

  // `knownRed: true` scenarios document a live, already-reported kernel
  // defect (see scenarios/18-multibody-honesty.mjs) — they are SUPPOSED to
  // fail today, so they do not count against the suite's exit code. If one
  // ever passes, that is the signal its defect got fixed; say so loudly and
  // tell the human to delete the flag rather than silently going green.
  for (const r of results) {
    if (r.knownRed && r.passed) {
      process.stdout.write(
        `\n⚠ ${r.id} is flagged knownRed but PASSED — its defect looks fixed. Remove \`knownRed: true\` from the scenario.\n`,
      );
    }
  }
  const failed = results.filter((r) => !r.passed && !r.knownRed).length;
  process.exit(failed);
}

main().catch((e) => {
  console.error("runner crashed:", e);
  process.exit(2);
});
