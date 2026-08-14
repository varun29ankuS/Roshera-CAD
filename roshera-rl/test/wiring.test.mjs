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
import { spawnSync } from "node:child_process";
import { once } from "node:events";
import { readFileSync, existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { OUTCOMES } from "../lib/trajectory.mjs";
import { readToolResult } from "../lib/mcp_session.mjs";

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
    // Each episode allocates its own BRepModel beside its document
    // (api-server/src/part_mgr.rs:340-358).
    if (req.method === "POST" && (req.url ?? "") === "/api/parts") {
      return res.end(JSON.stringify({ id: `part-${Math.random()}` }));
    }
    res.end("{}");
  });
  stub.listen(0, "127.0.0.1");
  await once(stub, "listening");

  const spawned = [];
  const pinnedParts = [];
  const calls = [];
  process.env.ROSHERA_URL = `http://127.0.0.1:${stub.address().port}`;
  process.env.ROSHERA_RL_TEST_SPAWN = "1";
  // The injected session speaks the SAME envelope the real one does
  // (readToolResult over a core.ts:380-385 `ok()` body) — a fake with a
  // friendlier shape would prove the entry point drives a runner that only
  // works against fakes.
  const CREATED_OK = readToolResult({
    content: [{ type: "text", text: JSON.stringify({
      object_uuid: "3f2b8c1e-77aa-4a9f-8b21-9f0f2a6d5e10", part_id: 1,
      perception: { sound: true, brep_valid: true, watertight: true },
    }, null, 2) }],
  });
  globalThis.__roshera_rl_test_spawn = async ({ documentId, partId }) => {
    spawned.push(documentId);
    pinnedParts.push(partId);
    return {
      async call(tool, args) { calls.push({ tool, args }); return CREATED_OK; },
      async claims(cs) { return cs.map((c) => ({ name: c.name, verified: true })); },
      async recipeRef() { return { step_count: 1, steps: [] }; },
      // mcp_session.mjs `readModelScope` — one solid, the one it built.
      async modelScope() {
        return {
          read_by: "list_parts", visible_parts: [1], visible_count: 1,
          built_here: 1, shared_model_detected: false,
        };
      },
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
  assert.ok(pinnedParts.every((p) => typeof p === "string" && p.length > 0),
    "and every spawned session was pinned to a part it owns — an absent pin " +
    "puts the episode back on the shared global BRepModel (part_mgr.rs:291-296)");
  for (const k of OUTCOMES) assert.ok(k in mod.lastRun.tally);
  assert.ok(Array.isArray(mod.lastRun.orphans),
    "and the reaper's verdict reaches the entry point, so an un-reaped document is printable");

  // THE SEED POLICY MUST ACTUALLY VERIFY. The live batch emitted
  // `{tool:"verify_part", args:{}}`, and `verify_part`'s schema requires
  // `part_id` (a number — roshera-mcp/src/tools/perception.ts:182), so the real
  // tool rejected every one of those calls: the reference batch reported
  // COMPLETED while never once exercising verification. The id it must carry is
  // the `part_id` the create result itself returns (tools/create.ts:256), read
  // off the previous observation — never a hardcoded number.
  const verify = calls.find((c) => c.tool === "verify_part");
  assert.ok(verify, "the reference batch must actually call verify_part");
  assert.equal(verify.args?.part_id, CREATED_OK.data.part_id,
    "verify_part must carry the part_id the create result reported — an empty " +
    "args object is rejected by the tool's schema and verifies nothing");
  assert.deepEqual(calls.map((c) => c.tool),
    ["timeline_checkpoint", "create_cylinder", "verify_part"],
    "and it declares an intent, builds, then verifies — in that order");

  stub.close();
  rmSync(outDir, { recursive: true, force: true });
  delete globalThis.__roshera_rl_test_spawn;
  delete process.env.ROSHERA_RL_TEST_SPAWN;
});

check("a valueless --concurrency is refused, not silently run as zero episodes", () => {
  // `--concurrency` with no value took the next argv entry (undefined at the
  // end of the line) → NaN → zero workers → "0 episodes", an all-zero tally
  // and EXIT 0: a silent no-op that reads exactly like a clean run. Run as a
  // child so the refusal's own process.exit cannot end this suite.
  const r = spawnSync(process.execPath, [bin, "--concurrency"], { encoding: "utf8" });
  assert.equal(r.status, 2, "a bad flag must fail loudly");
  assert.match(r.stderr, /positive integer/);
  assert.ok(!/episodes →/.test(r.stdout ?? ""), "and nothing may have run");
});

check("package.json exposes it as a script", () => {
  const pkg = JSON.parse(readFileSync(join(HERE, "..", "package.json"), "utf8"));
  assert.ok(pkg.scripts?.batch, "npm run batch must exist");
});

// AWAITED. The behavioural check above is async: without the await, `ok - …`
// and the "N checks passed" banner printed while it was still pending, and a
// failure arrived afterwards as an unhandled rejection — the exit code was
// right but the green banner preceded the verdict, which is the one place a
// test suite must not be misread.
for (const [name, fn] of checks) { await fn(); process.stdout.write(`  ok - ${name}\n`); }
process.stdout.write(`\nwiring: ${checks.length} checks passed\n`);
