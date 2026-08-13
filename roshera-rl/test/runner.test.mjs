/**
 * THE ISOLATION PROOF — the load-bearing test of slice 1.
 *
 * Two concurrent episodes must not see each other's parts or each other's
 * gate state. This is the whole claim of the slice: if it does not hold,
 * every trajectory produced in parallel is contaminated and worthless as
 * training data, in a way no downstream consumer could detect.
 *
 * The stub records the document of every call, so a shared document across
 * two episodes is directly observable. It also owns the DELETE route, so the
 * reaper can be driven against a backend that refuses one.
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
import { readToolResult } from "../lib/mcp_session.mjs";

const dir = mkdtempSync(join(tmpdir(), "roshera-run-"));
let n = 0;
/** documentId → how many DELETEs to refuse before accepting one. */
const refuseDeletes = new Map();
const deleteLog = [];

const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  if (req.method === "POST" && url === "/api/documents") {
    res.writeHead(200, { "Content-Type": "application/json" });
    return res.end(JSON.stringify({ id: `doc-${n++}`, active: false }));
  }
  if (req.method === "DELETE" && url.startsWith("/api/documents/")) {
    const id = url.split("/").pop();
    deleteLog.push(id);
    const left = refuseDeletes.get(id) ?? 0;
    if (left > 0) {
      refuseDeletes.set(id, left - 1);
      // api-server/src/documents.rs:561-564 — the backend refuses to delete
      // the ACTIVE document with a typed error, not a network failure.
      res.writeHead(409, { "Content-Type": "application/json" });
      return res.end(JSON.stringify({ error_code: "document_delete_refused_active" }));
    }
    res.writeHead(200, { "Content-Type": "application/json" });
    return res.end("{}");
  }
  res.writeHead(200, { "Content-Type": "application/json" });
  res.end("{}");
});
stub.listen(0, "127.0.0.1");
await once(stub, "listening");
const baseUrl = `http://127.0.0.1:${stub.address().port}`;

/** core.ts:380-385 ok() in `cert` mode → the envelope a real session returns. */
const CREATED_OK = readToolResult({
  content: [{ type: "text", text: JSON.stringify({
    object_uuid: "3f2b8c1e-77aa-4a9f-8b21-9f0f2a6d5e10", part_id: 1,
    perception: { sound: true, brep_valid: true, watertight: true },
  }, null, 2) }],
});

const task = defineTask({
  id: "t", prompt: "p", toolAllowlist: ["create_cylinder"],
  claims: [{
    name: "volume", expr: "v",
    bindings: [{ var: "v", measure: { kind: "volume", part: "solid:0" } }],
    expected: 117809.724509617, tolerance: 117.8,
  }],
  stepBudget: 4, tokenBudget: 1000, split: "train",
});

/** Each fake session records which document it was pinned to. */
const pinnedPerSession = [];
const fakeSpawn = async ({ documentId }) => {
  const seen = [];
  pinnedPerSession.push({ documentId, seen });
  return {
    async call(tool) { seen.push({ tool, documentId }); return CREATED_OK; },
    async claims(cs) { return cs.map((c) => ({ name: c.name, verified: true })); },
    async recipeRef() { return { step_count: 1, steps: [] }; },
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
  const slowSpawn = async () => {
    live += 1; peak = Math.max(peak, live);
    await new Promise((r) => setTimeout(r, 20));
    return {
      async call() { return CREATED_OK; },
      async claims(cs) { return cs.map((c) => ({ name: c.name, verified: true })); },
      async recipeRef() { return { step_count: 1, steps: [] }; },
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
  const flakySpawn = async () => {
    const mine = i++;
    return {
      async call() {
        if (mine === 0) throw new Error("EPIPE");
        return CREATED_OK;
      },
      async claims(cs) { return cs.map((c) => ({ name: c.name, verified: true })); },
      async recipeRef() { return { step_count: 1, steps: [] }; },
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

check("THE REAPER retries a DELETE the episode could not land", async () => {
  // episode.mjs names runner.mjs's reaper as the backstop for a failed
  // DELETE at two call sites. Until now runner.mjs contained no reaping at
  // all, so a document lost to a blip stayed in PartManager's DashMap
  // forever, under a comment asserting otherwise.
  deleteLog.length = 0;
  const nextDoc = `doc-${n}`;
  refuseDeletes.set(nextDoc, 1);   // the episode's own attempt is refused once
  const { results, orphans } = await runBatch({
    tasks: [task], policyFor: () => scriptedPolicy([{ tool: "create_cylinder", args: {} }]),
    seeds: [1], concurrency: 1, baseUrl, authHeader: {}, outDir: dir,
    kernelSha: "abc", spawn: fakeSpawn,
  });
  assert.equal(results[0].documentId, nextDoc);
  assert.equal(deleteLog.filter((d) => d === nextDoc).length, 2,
    "the episode tried once and the reaper tried again — not once, not never");
  assert.deepEqual(orphans, [], "the retry landed, so nothing is orphaned");
  assert.equal(results[0].reap.reaped, true);
});

check("a document the reaper still cannot drop is REPORTED, not assumed clean", async () => {
  deleteLog.length = 0;
  const nextDoc = `doc-${n}`;
  refuseDeletes.set(nextDoc, 99);  // refuse everything
  const { results, orphans } = await runBatch({
    tasks: [task], policyFor: () => scriptedPolicy([{ tool: "create_cylinder", args: {} }]),
    seeds: [1], concurrency: 1, baseUrl, authHeader: {}, outDir: dir,
    kernelSha: "abc", spawn: fakeSpawn,
  });
  assert.equal(orphans.length, 1, "an un-reaped document must surface, not vanish");
  assert.equal(orphans[0].documentId, nextDoc);
  assert.ok(orphans[0].reason.includes("409"), "and the refusal reason is carried, not flattened");
  assert.equal(results[0].reap.reaped, false);
});

for (const [name, fn] of checks) { await fn(); process.stdout.write(`  ok - ${name}\n`); }
stub.close();
rmSync(dir, { recursive: true, force: true });
process.stdout.write(`\nrunner: ${checks.length} checks passed\n`);
