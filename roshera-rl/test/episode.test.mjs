/**
 * Episode-lifecycle proof.
 *
 * Every episode lands in exactly one named outcome. The taxonomy is borrowed
 * from exploration.rs because it already got the important part right: an
 * episode that never ran must never be reported as an episode that ran and
 * scored nothing. A zero-scored COMPLETED for a crashed process is the same
 * class of lie as a fabricated fidelity zero.
 */
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { runEpisode } from "../lib/episode.mjs";
import { readTrajectory } from "../lib/trajectory.mjs";
import { scriptedPolicy } from "../lib/policy.mjs";
import { defineTask } from "../lib/task.mjs";

const dir = mkdtempSync(join(tmpdir(), "roshera-ep-"));
const created = [];
const deleted = [];
let failCreate = false;

const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  if (req.method === "POST" && url === "/api/documents") {
    if (failCreate) { res.writeHead(500); return res.end("{}"); }
    const id = `doc-${created.length}`;
    created.push(id);
    res.writeHead(200, { "Content-Type": "application/json" });
    return res.end(JSON.stringify({ id, name: "ep", active: false }));
  }
  if (req.method === "DELETE" && url.startsWith("/api/documents/")) {
    deleted.push(url.split("/").pop());
    res.writeHead(200, { "Content-Type": "application/json" });
    return res.end("{}");
  }
  res.writeHead(404); res.end("{}");
});
stub.listen(0, "127.0.0.1");
await once(stub, "listening");
const baseUrl = `http://127.0.0.1:${stub.address().port}`;

const task = defineTask({
  id: "t", prompt: "p", toolAllowlist: ["create_cylinder", "verify_part"],
  claims: [{ name: "radius", quantity: "radius", expected: 25, tolerance: 0.02 }],
  stepBudget: 3, tokenBudget: 1000, split: "train",
});

/** A fake MCP session. `behaviour` decides what each call returns. */
function fakeSpawn(behaviour) {
  return async () => ({
    async call(tool, args) { return behaviour(tool, args); },
    async claims() { return [{ name: "radius", verified: true }]; },
    async recipeRef() { return "recipe/1"; },
    async close() {},
  });
}

const checks = [];
const check = (name, fn) => checks.push([name, fn]);

check("a completed episode writes COMPLETED and reaps its document", async () => {
  const path = join(dir, "a.jsonl");
  const r = await runEpisode({
    task, policy: scriptedPolicy([{ tool: "create_cylinder", args: { radius: 25 } }]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: path, kernelSha: "abc",
    spawn: fakeSpawn(() => ({ perception: { sound: true, fidelity: { worst: { signed_relative_deviation: -0.001 } } } })),
  });
  assert.equal(r.outcome, "COMPLETED");
  assert.ok(deleted.includes(r.documentId), "the document is dropped, not leaked");
  const { header, steps, terminal } = readTrajectory(path);
  assert.deepEqual(header.tool_allowlist, ["create_cylinder", "verify_part"]);
  assert.equal(steps.length, 1);
  assert.equal(terminal.outcome, "COMPLETED");
  assert.equal(terminal.recipe_ref, "recipe/1");
});

check("a policy that never stops hits the step budget, honestly", async () => {
  const path = join(dir, "b.jsonl");
  const forever = { async act() { return { tool: "create_cylinder", args: {} }; }, tokensUsed: () => 0 };
  const r = await runEpisode({
    task, policy: forever, seed: 1, baseUrl, authHeader: {},
    trajectoryPath: path, kernelSha: "abc",
    spawn: fakeSpawn(() => ({ perception: { sound: true } })),
  });
  assert.equal(r.outcome, "BUDGET_EXHAUSTED");
  assert.equal(readTrajectory(path).steps.length, 3, "exactly stepBudget steps ran");
});

check("a dead session is CRASHED, never a zero-scored COMPLETED", async () => {
  const path = join(dir, "c.jsonl");
  const r = await runEpisode({
    task, policy: scriptedPolicy([{ tool: "create_cylinder", args: {} }]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: path, kernelSha: "abc",
    spawn: fakeSpawn(() => { throw new Error("EPIPE: MCP process died"); }),
  });
  assert.equal(r.outcome, "CRASHED");
  assert.notEqual(readTrajectory(path).terminal.outcome, "COMPLETED");
});

check("a 429 from the backend is RATE_LIMITED, its own outcome", async () => {
  const path = join(dir, "d.jsonl");
  const r = await runEpisode({
    task, policy: scriptedPolicy([{ tool: "create_cylinder", args: {} }]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: path, kernelSha: "abc",
    spawn: fakeSpawn(() => { const e = new Error("429"); e.status = 429; throw e; }),
  });
  assert.equal(r.outcome, "RATE_LIMITED");
});

check("a failed document creation is SETUP_FAILED — no episode happened", async () => {
  const path = join(dir, "e.jsonl");
  failCreate = true;
  const r = await runEpisode({
    task, policy: scriptedPolicy([]), seed: 1, baseUrl, authHeader: {},
    trajectoryPath: path, kernelSha: "abc",
    spawn: fakeSpawn(() => ({ perception: { sound: true } })),
  });
  failCreate = false;
  assert.equal(r.outcome, "SETUP_FAILED");
});

check("an out-of-allowlist action ends the episode as INVALID_ACTION, not BUDGET_EXHAUSTED", async () => {
  const path = join(dir, "f.jsonl");
  let called = 0;
  const r = await runEpisode({
    task, policy: scriptedPolicy([{ tool: "boolean", args: {} }]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: path, kernelSha: "abc",
    spawn: fakeSpawn(() => { called += 1; return { perception: { sound: true } }; }),
  });
  assert.equal(called, 0, "the disallowed tool must never reach the session");
  assert.equal(r.outcome, "INVALID_ACTION",
    "zero real steps run must not read the same as a genuinely exhausted budget");
});

check("a spawn failure after document creation still reaps the document", async () => {
  const path = join(dir, "g.jsonl");
  const r = await runEpisode({
    task, policy: scriptedPolicy([]), seed: 1, baseUrl, authHeader: {},
    trajectoryPath: path, kernelSha: "abc",
    spawn: async () => { throw new Error("ECONNREFUSED: MCP process failed to start"); },
  });
  assert.equal(r.outcome, "SETUP_FAILED");
  assert.ok(deleted.includes(r.documentId),
    "the document must be reaped even when spawn fails after creation, not orphaned in PartManager");
});

check("a policy that throws leaves a record of why, not silence", async () => {
  const path = join(dir, "h.jsonl");
  const throwingPolicy = {
    async act() { throw new Error("policy blew up: bad state"); },
    tokensUsed: () => 0,
  };
  const r = await runEpisode({
    task, policy: throwingPolicy, seed: 1, baseUrl, authHeader: {},
    trajectoryPath: path, kernelSha: "abc",
    spawn: fakeSpawn(() => ({ perception: { sound: true } })),
  });
  assert.equal(r.outcome, "CRASHED");
  assert.ok(r.error && r.error.includes("policy blew up"),
    "the returned object must carry the reason, the way SETUP_FAILED does");
  const { steps, terminal } = readTrajectory(path);
  assert.equal(steps.length, 1, "the crash is recorded as a step, not dropped silently");
  assert.equal(terminal.outcome, "CRASHED");
});

for (const [name, fn] of checks) { await fn(); process.stdout.write(`  ok - ${name}\n`); }
stub.close();
rmSync(dir, { recursive: true, force: true });
process.stdout.write(`\nepisode: ${checks.length} checks passed\n`);
