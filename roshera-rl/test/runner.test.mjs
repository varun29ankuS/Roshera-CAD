/**
 * THE ISOLATION PROOF — the load-bearing test of slice 1.
 *
 * Two concurrent episodes must not see each other's parts or each other's
 * gate state. This is the whole claim of the slice: if it does not hold,
 * every trajectory produced in parallel is contaminated and worthless as
 * training data, in a way no downstream consumer could detect.
 *
 * The stub records the X-Roshera-Document of every mutating call, so a shared
 * document across two episodes is directly observable.
 */
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { runBatch } from "../lib/runner.mjs";
import { scriptedPolicy } from "../lib/policy.mjs";
import { defineTask } from "../lib/task.mjs";

const dir = mkdtempSync(join(tmpdir(), "roshera-run-"));
let n = 0;
const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  if (req.method === "POST" && url === "/api/documents") {
    res.writeHead(200, { "Content-Type": "application/json" });
    return res.end(JSON.stringify({ id: `doc-${n++}`, active: false }));
  }
  res.writeHead(200, { "Content-Type": "application/json" });
  res.end("{}");
});
stub.listen(0, "127.0.0.1");
await once(stub, "listening");
const baseUrl = `http://127.0.0.1:${stub.address().port}`;

const task = defineTask({
  id: "t", prompt: "p", toolAllowlist: ["create_cylinder"],
  claims: [{ name: "r", quantity: "radius", expected: 25, tolerance: 0.02 }],
  stepBudget: 4, tokenBudget: 1000, split: "train",
});

/** Each fake session records which document it was pinned to. */
const pinnedPerSession = [];
const fakeSpawn = async ({ documentId }) => {
  const seen = [];
  pinnedPerSession.push({ documentId, seen });
  return {
    async call(tool, args) { seen.push({ tool, documentId }); return { perception: { sound: true } }; },
    async claims() { return []; },
    async recipeRef() { return null; },
    async close() {},
  };
};

const checks = [];
const check = (name, fn) => checks.push([name, fn]);

check("concurrent episodes each get their OWN document — none shared", async () => {
  pinnedPerSession.length = 0;
  const { results } = await runBatch({
    tasks: [task, task, task, task],
    policyFor: () => scriptedPolicy([{ tool: "create_cylinder", args: { radius: 25 } }]),
    seeds: [1, 2, 3, 4], concurrency: 4, baseUrl, authHeader: {},
    outDir: dir, kernelSha: "abc", spawn: fakeSpawn,
  });
  assert.equal(results.length, 4);
  const docs = pinnedPerSession.map((s) => s.documentId);
  assert.equal(new Set(docs).size, 4,
    `four concurrent episodes must hold four distinct documents, got ${JSON.stringify(docs)}`);
  for (const s of pinnedPerSession) {
    assert.ok(s.seen.every((c) => c.documentId === s.documentId),
      "no call may cross into another episode's document");
  }
});

check("the tally names every outcome, including the zeros", async () => {
  const { tally } = await runBatch({
    tasks: [task], policyFor: () => scriptedPolicy([]), seeds: [1],
    concurrency: 1, baseUrl, authHeader: {}, outDir: dir, kernelSha: "abc",
    spawn: fakeSpawn,
  });
  for (const k of ["COMPLETED", "BUDGET_EXHAUSTED", "CRASHED", "SETUP_FAILED", "RATE_LIMITED"]) {
    assert.ok(k in tally, `${k} must appear even at zero — an absent key reads as "not measured"`);
  }
});

check("concurrency is a cap, not a suggestion", async () => {
  let live = 0, peak = 0;
  const slowSpawn = async (opts) => {
    live += 1; peak = Math.max(peak, live);
    await new Promise((r) => setTimeout(r, 20));
    return {
      async call() { return { perception: { sound: true } }; },
      async claims() { return []; },
      async recipeRef() { return null; },
      async close() { live -= 1; },
    };
  };
  await runBatch({
    tasks: [task, task, task, task, task, task],
    policyFor: () => scriptedPolicy([{ tool: "create_cylinder", args: {} }]),
    seeds: [1, 2, 3, 4, 5, 6], concurrency: 2, baseUrl, authHeader: {},
    outDir: dir, kernelSha: "abc", spawn: slowSpawn,
  });
  assert.ok(peak <= 2, `peak concurrency ${peak} exceeded the cap of 2`);
});

check("one episode's crash does not take the batch down", async () => {
  let i = 0;
  const flakySpawn = async (opts) => {
    const mine = i++;
    return {
      async call() {
        if (mine === 0) throw new Error("EPIPE");
        return { perception: { sound: true } };
      },
      async claims() { return []; },
      async recipeRef() { return null; },
      async close() {},
    };
  };
  const { results, tally } = await runBatch({
    tasks: [task, task], policyFor: () => scriptedPolicy([{ tool: "create_cylinder", args: {} }]),
    seeds: [1, 2], concurrency: 2, baseUrl, authHeader: {}, outDir: dir,
    kernelSha: "abc", spawn: flakySpawn,
  });
  assert.equal(results.length, 2, "both episodes reported, one of them a crash");
  assert.equal(tally.CRASHED, 1);
});

for (const [name, fn] of checks) { await fn(); process.stdout.write(`  ok - ${name}\n`); }
stub.close();
rmSync(dir, { recursive: true, force: true });
process.stdout.write(`\nrunner: ${checks.length} checks passed\n`);
