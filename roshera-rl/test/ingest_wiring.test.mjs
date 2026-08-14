/**
 * Ingestion's own production-call-site proof (Task 7, the same
 * disconnection-gate rule `wiring.test.mjs` proves for the runner itself).
 *
 * A package that is built, correct, and reachable from nothing is exactly
 * the shape fourteen prior capabilities in this repo took. This suite
 * proves TWO things behaviourally, never by grepping source text for a
 * function name:
 *
 *   1. `bin/run-batch.mjs` really calls ingestion after a real batch, with
 *      that batch's own `outDir` — and `--no-ingest` really suppresses it.
 *   2. A failing ingest is REPORTED, never allowed to fail the batch: JSONL
 *      on disk is already the full, correct record by the time ingestion
 *      runs, so a database outage must cost nobody their trajectories.
 *
 * `bin/ingest.mjs`'s own `--verify` exit-code contract (non-zero on drift,
 * naming each file) is proven separately against its pure `reportVerify`
 * function and against the real CLI process — neither needs a live
 * Postgres, and this suite deliberately never opens one (that is
 * `ingest_store.test.mjs`'s job, gated on `ROSHERA_RL_PG`).
 */
import assert from "node:assert/strict";
import http from "node:http";
import { spawnSync } from "node:child_process";
import { once } from "node:events";
import { readFileSync, existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { readToolResult } from "../lib/mcp_session.mjs";
import { reportVerify } from "../bin/ingest.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const runBatchBin = join(HERE, "..", "bin", "run-batch.mjs");
const ingestBin = join(HERE, "..", "bin", "ingest.mjs");

const checks = [];
const check = (name, fn) => checks.push([name, fn]);

check("an executable entry point exists", () => {
  assert.ok(existsSync(ingestBin), "roshera-rl/bin/ingest.mjs must exist");
});

/**
 * Drives the REAL `bin/run-batch.mjs` against a stub HTTP backend and a
 * faked MCP session (the same seam `wiring.test.mjs` uses), with a faked
 * ingester recorded via `globalThis.__roshera_rl_test_ingest`. Returns the
 * imported module (so `mod.lastRun` / `mod.lastIngest` are assertable) and
 * the array of `{dir}` calls the fake ingester recorded.
 *
 * Every call gets its own stub server, its own temp outDir, and a fresh
 * cache-busted import — state left behind by one scenario (the env flags,
 * the globals, the listening server) must never leak into the next.
 */
async function runBatchWithFakeIngest({ extraArgv = [], fakeIngest }) {
  const stub = http.createServer((req, res) => {
    res.writeHead(200, { "Content-Type": "application/json" });
    if (req.method === "POST" && (req.url ?? "") === "/api/documents") {
      return res.end(JSON.stringify({ id: `doc-${Math.random()}`, active: false }));
    }
    if (req.method === "POST" && (req.url ?? "") === "/api/parts") {
      return res.end(JSON.stringify({ id: `part-${Math.random()}` }));
    }
    res.end("{}");
  });
  stub.listen(0, "127.0.0.1");
  await once(stub, "listening");

  process.env.ROSHERA_URL = `http://127.0.0.1:${stub.address().port}`;
  process.env.ROSHERA_RL_TEST_SPAWN = "1";
  process.env.ROSHERA_RL_TEST_INGEST = "1";

  const CREATED_OK = readToolResult({
    content: [{ type: "text", text: JSON.stringify({
      object_uuid: "6a1c9d2e-88bb-4a9f-8b21-9f0f2a6d5e11", part_id: 1,
      perception: { sound: true, brep_valid: true, watertight: true },
    }, null, 2) }],
  });
  globalThis.__roshera_rl_test_spawn = async ({ partId }) => ({
    async call(tool) {
      if (tool === "verify_part") return CREATED_OK;
      return CREATED_OK;
    },
    async claims(cs) { return cs.map((c) => ({ name: c.name, verified: true })); },
    async recipeRef() { return { step_count: 1, steps: [] }; },
    async modelScope() {
      return {
        read_by: "list_parts", visible_parts: [1], visible_count: 1,
        built_here: 1, shared_model_detected: false,
      };
    },
    async close() {},
  });
  globalThis.__roshera_rl_test_ingest = fakeIngest;

  const outDir = mkdtempSync(join(tmpdir(), "roshera-ingest-wiring-"));
  process.argv = [process.argv[0], runBatchBin, "--out", outDir, "--concurrency", "1", ...extraArgv];

  let mod;
  try {
    mod = await import(pathToFileURL(runBatchBin).href + `?t=${process.hrtime.bigint()}`);
  } finally {
    stub.close();
    rmSync(outDir, { recursive: true, force: true });
    delete globalThis.__roshera_rl_test_spawn;
    delete globalThis.__roshera_rl_test_ingest;
    delete process.env.ROSHERA_RL_TEST_SPAWN;
    delete process.env.ROSHERA_RL_TEST_INGEST;
  }
  return { mod, outDir };
}

check("the entry point actually calls the ingester after a real batch, with that batch's own outDir", async () => {
  const calls = [];
  const { mod, outDir } = await runBatchWithFakeIngest({
    fakeIngest: async (dir) => { calls.push(dir); return { mode: "ingest", dir, results: [] }; },
  });
  assert.ok(mod.lastRun.results.length >= 1, "the batch actually ran episodes");
  assert.equal(calls.length, 1, "the ingester must be called exactly once");
  assert.equal(calls[0], outDir, "the ingester must be called with THIS batch's own outDir, not a default or a guess");
  assert.deepEqual(mod.lastIngest, { ran: true, ok: true, result: { mode: "ingest", dir: outDir, results: [] } });
});

check("--no-ingest suppresses the call entirely", async () => {
  const calls = [];
  const { mod } = await runBatchWithFakeIngest({
    extraArgv: ["--no-ingest"],
    fakeIngest: async (dir) => { calls.push(dir); return { mode: "ingest", dir, results: [] }; },
  });
  assert.ok(mod.lastRun.results.length >= 1, "the batch still ran episodes");
  assert.equal(calls.length, 0, "--no-ingest must mean the ingester is never invoked");
  assert.deepEqual(mod.lastIngest, { ran: false });
});

check("a throwing ingester does NOT fail the batch — the failure is surfaced, not swallowed", async () => {
  const calls = [];
  const { mod, outDir } = await runBatchWithFakeIngest({
    fakeIngest: async (dir) => {
      calls.push(dir);
      throw new Error("simulated Postgres outage");
    },
  });
  // The property that matters most: JSONL is the source of truth, so a
  // database being unreachable must cost nobody their trajectories. The
  // batch's own result must be exactly as complete as it would have been
  // with no ingestion step at all.
  assert.equal(calls.length, 1, "the ingester was still invoked with the batch's outDir");
  assert.equal(calls[0], outDir);
  assert.ok(mod.lastRun.results.length >= 1, "the batch's own results must be intact — a DB outage is not an episode failure");
  assert.equal(mod.lastIngest.ran, true);
  assert.equal(mod.lastIngest.ok, false, "the ingest failure must be recorded, never silently absorbed");
  assert.match(mod.lastIngest.error, /simulated Postgres outage/,
    "the thrown reason must be surfaced verbatim, not replaced with a generic message");
});

check("reportVerify exits non-zero and names every drifted file", () => {
  const { lines, exitCode } = reportVerify({
    checked: 3,
    drifted: [
      { path: "C:\\runs\\a.jsonl", reason: "the file's bytes changed since it was ingested" },
      { path: "C:\\runs\\b.jsonl", reason: "the file could not be read at its recorded path" },
    ],
  });
  assert.equal(exitCode, 1, "any drift must be a non-zero exit — a ratchet that exits 0 is decoration");
  const joined = lines.join("\n");
  assert.match(joined, /C:\\runs\\a\.jsonl/);
  assert.match(joined, /C:\\runs\\b\.jsonl/);
  assert.match(joined, /bytes changed since it was ingested/);
  assert.match(joined, /could not be read at its recorded path/);
});

check("reportVerify exits zero when nothing drifted", () => {
  const { lines, exitCode } = reportVerify({ checked: 5, drifted: [] });
  assert.equal(exitCode, 0);
  assert.match(lines[0], /5 file\(s\) checked, 0 drifted/);
});

check("the CLI with no arguments refuses loudly rather than doing nothing silently", () => {
  const r = spawnSync(process.execPath, [ingestBin], { encoding: "utf8" });
  assert.equal(r.status, 2);
  assert.match(r.stderr, /usage: node bin\/ingest\.mjs/);
});

check("the CLI without ROSHERA_RL_PG refuses with a named reason, never a silent no-op", () => {
  const env = { ...process.env };
  delete env.ROSHERA_RL_PG;
  const r = spawnSync(process.execPath, [ingestBin, "./runs"], { encoding: "utf8", env });
  assert.notEqual(r.status, 0, "a missing connection string must not exit clean");
  const output = (r.stdout ?? "") + (r.stderr ?? "");
  assert.match(output, /ROSHERA_RL_PG/,
    "the absence must be STATED with a reason, never a bare crash or a silent pass");
  // A deliberate refusal, not an uncaught exception Node happened to print
  // the reason inside: this must never regress into a bare stack trace
  // just because the message text still matches the regex above.
  assert.doesNotMatch(output, /\n\s+at .*\(.*:\d+:\d+\)/,
    "this must be a deliberate refusal (a named reason on stderr, non-zero exit) — " +
    "not a stack trace that happens to contain the reason text");
});

check("package.json exposes the ingest CLI and folds this suite into the test chain", () => {
  const pkg = JSON.parse(readFileSync(join(HERE, "..", "package.json"), "utf8"));
  assert.ok(pkg.scripts?.ingest, "npm run ingest must exist");
  assert.ok(pkg.scripts?.["ingest:verify"], "npm run ingest:verify must exist");
  // The name promises this suite is IN the chain, not merely that the CLI
  // scripts exist beside it — dropping ingest_wiring.test.mjs from
  // pkg.scripts.test must fail this check, or the name is a stated reason
  // the body doesn't actually make true.
  assert.match(pkg.scripts?.test ?? "", /test\/ingest_wiring\.test\.mjs/,
    "pkg.scripts.test must actually run ingest_wiring.test.mjs — removing it from the chain " +
    "must fail this check, not leave it green under a name that says otherwise");
});

// The real production binding, not the fake injected through
// ROSHERA_RL_TEST_INGEST. run-batch.mjs:165 reaches the ingester as
// `m.runIngest` inside `import("./ingest.mjs")` — every other check in this
// suite substitutes a fake there and so never actually consults this
// export. A rename here degrades production to a caught "m.runIngest is
// not a function" at exit 0, with every other check in this file still
// green. Importing the module directly (no fake, no database) is the only
// way to prove the name production depends on is really there. Placed
// LAST so that if this ever regresses, every other check in this file has
// already printed its own "ok" line first — the contrast (everything else
// green, only this dark) is visible in a single run's output, not just
// asserted in prose.
check("the real module exports a callable under the exact name run-batch.mjs calls", async () => {
  const real = await import("../bin/ingest.mjs");
  assert.equal(typeof real.runIngest, "function",
    "run-batch.mjs:165 reaches the ingester as m.runIngest — a rename here degrades production " +
    "to a caught 'not a function' at exit 0, with every other check still green");
});

// AWAITED, same discipline as wiring.test.mjs: the behavioural checks above
// are async, and printing the summary before they settle would let a
// failure land as an unhandled rejection AFTER the green banner.
for (const [name, fn] of checks) { await fn(); process.stdout.write(`  ok - ${name}\n`); }
process.stdout.write(`\ningest_wiring: ${checks.length} checks passed\n`);
