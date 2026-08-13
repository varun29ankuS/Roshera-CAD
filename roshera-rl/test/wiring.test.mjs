/**
 * Production-call-site proof (the disconnection-gate rule).
 *
 * Fourteen times in this repo a capability has been BUILT, been CORRECT, and
 * been WIRED TO NOTHING. A runner nothing invokes is exactly that shape, so
 * this asserts a real entry point exists and reaches runBatch with the seed
 * tasks — not merely that the modules construct.
 */
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { readFileSync, existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { OUTCOMES } from "../lib/trajectory.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const bin = join(HERE, "..", "bin", "run-batch.mjs");

const checks = [];
const check = (name, fn) => checks.push([name, fn]);

check("an executable entry point exists", () => {
  assert.ok(existsSync(bin), "roshera-rl/bin/run-batch.mjs must exist");
});

// RULING (Varun, 2026-08-13): this check is BEHAVIOURAL, not a regex over the
// source. A substring assertion passes on a comment mentioning `runBatch` and
// breaks on a rename, which proves neither that a call site exists nor that it
// works. The entry point is imported with an injected fake spawn against a stub
// backend, and we assert runBatch actually ran the seed tasks.
check("the entry point actually drives runBatch over the seed tasks", async () => {
  const stub = http.createServer((req, res) => {
    res.writeHead(200, { "Content-Type": "application/json" });
    if (req.method === "POST" && (req.url ?? "") === "/api/documents") {
      return res.end(JSON.stringify({ id: `doc-${Math.random()}`, active: false }));
    }
    res.end("{}");
  });
  stub.listen(0, "127.0.0.1");
  await once(stub, "listening");

  const spawned = [];
  process.env.ROSHERA_URL = `http://127.0.0.1:${stub.address().port}`;
  process.env.ROSHERA_RL_TEST_SPAWN = "1";
  globalThis.__roshera_rl_test_spawn = async ({ documentId }) => {
    spawned.push(documentId);
    return {
      async call() { return { perception: { sound: true } }; },
      async claims() { return []; },
      async recipeRef() { return null; },
      async close() {},
    };
  };
  const outDir = mkdtempSync(join(tmpdir(), "roshera-wiring-"));
  process.argv = [process.argv[0], bin, "--out", outDir, "--concurrency", "1"];

  const mod = await import(pathToFileURL(bin).href + `?t=${Date.now()}`);
  assert.ok(Array.isArray(mod.lastRun?.results), "the entry point exported its run result");
  assert.ok(mod.lastRun.results.length >= 1, "at least one seed task ran");
  assert.equal(spawned.length, mod.lastRun.results.length,
    "every episode spawned a session — the runner was really driven, not imported");
  for (const k of OUTCOMES) assert.ok(k in mod.lastRun.tally);

  stub.close();
  rmSync(outDir, { recursive: true, force: true });
  delete globalThis.__roshera_rl_test_spawn;
  delete process.env.ROSHERA_RL_TEST_SPAWN;
});

check("package.json exposes it as a script", () => {
  const pkg = JSON.parse(readFileSync(join(HERE, "..", "package.json"), "utf8"));
  assert.ok(pkg.scripts?.batch, "npm run batch must exist");
});

for (const [name, fn] of checks) { fn(); process.stdout.write(`  ok - ${name}\n`); }
process.stdout.write(`\nwiring: ${checks.length} checks passed\n`);
