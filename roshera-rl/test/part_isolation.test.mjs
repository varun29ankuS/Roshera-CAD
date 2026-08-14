/**
 * Per-episode MODEL isolation — the harness half.
 *
 * The document pin already gives each episode its own TIMELINE. It does not
 * give it its own live `BRepModel`: `ActiveModel` routes on
 * `X-Roshera-Part-Id` (api-server/src/part_mgr.rs:264, 276) and falls back to
 * the ONE global `AppState.model` when that header is absent
 * (part_mgr.rs:291-296). Measured on 8 concurrent live episodes (2026-08-13):
 * part ids 73…93 in supposedly fresh documents, every session reading every
 * other session's solids.
 *
 * Nothing ever asked for isolation — `POST /api/parts` (part_mgr.rs:340-358)
 * had no caller anywhere in the repo. This suite pins the harness side of the
 * fix: the episode CREATES a part beside its document, PINS the child to it,
 * REAPS it, and RECORDS what its own model actually contains, so a shared
 * model is detectable from the trajectory rather than merely absent from it.
 *
 *   node test/part_isolation.test.mjs
 */
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import { runEpisode } from "../lib/episode.mjs";
import { readTrajectory } from "../lib/trajectory.mjs";
import { scriptedPolicy } from "../lib/policy.mjs";
import { defineTask } from "../lib/task.mjs";
import { readToolResult } from "../lib/mcp_session.mjs";

// `childEnv` and `readModelScope` are pulled in inside the checks that use
// them: an export this suite names but the module does not have is then ONE
// red check among the others, rather than a suite that cannot be loaded at all
// and reports nothing about the episode lifecycle.
const session = () => import("../lib/mcp_session.mjs");

const dir = mkdtempSync(join(tmpdir(), "roshera-part-"));

const createdDocuments = [];
const createdParts = [];
const deletedParts = [];
const deletedDocuments = [];
/** null = create the part; a number = answer POST /api/parts with that status. */
let partCreateStatus = null;

const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  const send = (obj, status = 200) => {
    res.writeHead(status, { "Content-Type": "application/json" });
    res.end(JSON.stringify(obj));
  };
  if (req.method === "POST" && url === "/api/documents") {
    const id = `doc-${createdDocuments.length}`;
    createdDocuments.push(id);
    return send({ id, name: "ep", active: false });
  }
  if (req.method === "DELETE" && url.startsWith("/api/documents/")) {
    deletedDocuments.push(url.split("/").pop());
    return send({});
  }
  // part_mgr.rs:340-358 — `CreatePartResponse { id }`, a PartManager UUID.
  if (req.method === "POST" && url === "/api/parts") {
    if (partCreateStatus !== null) return send({ error_code: "refused" }, partCreateStatus);
    const id = randomUUID();
    createdParts.push(id);
    return send({ id });
  }
  // part_mgr.rs:416-427 — `{success, id}`.
  if (req.method === "DELETE" && url.startsWith("/api/parts/")) {
    deletedParts.push(url.split("/").pop());
    return send({ success: true, id: url.split("/").pop() });
  }
  send({}, 404);
});
stub.listen(0, "127.0.0.1");
await once(stub, "listening");
const baseUrl = `http://127.0.0.1:${stub.address().port}`;

/** core.ts:380-385 ok(data) → the envelope a real session returns. */
const ok = (data) => readToolResult({
  content: [{ type: "text", text: JSON.stringify(data, null, 2) }],
});
/** core.ts:471-497 fail(e) → prose, isError. */
const fail = (msg) => readToolResult({
  content: [{ type: "text", text: `ERROR: ${msg}` }], isError: true,
});
/** create.ts:253-260 in `cert` ambient mode. */
const CREATED_OK = ok({
  object_uuid: "3f2b8c1e-77aa-4a9f-8b21-9f0f2a6d5e10", part_id: 1, placement: null,
  perception: { sound: true, brep_valid: true, watertight: true, volume: 117809.7 },
});
/** geometry-engine/src/readable/part.rs:405-416 — what list_parts returns. */
const summary = (id) => ({
  id, name: `Cylinder ${id}`, anchor_datum_id: 0,
  anchor_datum_name: "WorldOrigin", location_oneliner: `cylinder ${id}`,
});

const task = defineTask({
  id: "t", prompt: "p", toolAllowlist: ["create_cylinder"],
  claims: [{
    name: "volume", expr: "v",
    bindings: [{ var: "v", measure: { kind: "volume", part: "solid:0" } }],
    expected: 117809.724509617, tolerance: 117.8,
  }],
  stepBudget: 2, tokenBudget: 1000, split: "train",
});

/** A fake session that records the part it was pinned to. */
function fakeSpawn(sink, scope) {
  return async ({ documentId, partId }) => {
    sink.push({ documentId, partId });
    return {
      async call() { return CREATED_OK; },
      async claims(cs) { return cs.map((c) => ({ name: c.name, verified: true })); },
      async recipeRef() { return { step_count: 1, steps: [] }; },
      async modelScope() { return scope; },
      async close() {},
    };
  };
}

const checks = [];
const check = (name, fn) => checks.push([name, fn]);

check("an episode creates a part beside its document and pins the child to it", async () => {
  const sink = [];
  const before = createdParts.length;
  const r = await runEpisode({
    task, policy: scriptedPolicy([{ tool: "create_cylinder", args: { radius: 25 } }]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: join(dir, "a.jsonl"),
    kernelSha: "abc", spawn: fakeSpawn(sink, { visible_count: 1 }),
  });
  assert.equal(r.outcome, "COMPLETED");
  assert.equal(createdParts.length, before + 1,
    "the episode must POST /api/parts — nothing else in the repo ever does, " +
    "so without it every session falls back to the ONE global model");
  assert.equal(sink.length, 1, "one session was spawned");
  assert.equal(sink[0].partId, createdParts[createdParts.length - 1],
    "and the child is pinned to THAT part, not to some other episode's");
  assert.equal(r.partId, sink[0].partId, "the result names the part it held");
});

check("two episodes hold two DIFFERENT parts", async () => {
  const sink = [];
  const spawn = fakeSpawn(sink, { visible_count: 1 });
  const one = { tool: "create_cylinder", args: { radius: 25 } };
  await runEpisode({
    task, policy: scriptedPolicy([one]), seed: 1, baseUrl, authHeader: {},
    trajectoryPath: join(dir, "b1.jsonl"), kernelSha: "abc", spawn,
  });
  await runEpisode({
    task, policy: scriptedPolicy([one]), seed: 2, baseUrl, authHeader: {},
    trajectoryPath: join(dir, "b2.jsonl"), kernelSha: "abc", spawn,
  });
  assert.equal(sink.length, 2);
  assert.notEqual(sink[0].partId, sink[1].partId,
    "two episodes sharing one part id would share one BRepModel — the defect");
});

check("the part is reaped at teardown, beside the document", async () => {
  deletedParts.length = 0;
  const sink = [];
  const r = await runEpisode({
    task, policy: scriptedPolicy([{ tool: "create_cylinder", args: {} }]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: join(dir, "c.jsonl"),
    kernelSha: "abc", spawn: fakeSpawn(sink, { visible_count: 1 }),
  });
  assert.ok(deletedParts.includes(r.partId),
    "an un-reaped part leaks a whole BRepModel in PartManager's DashMap");
  assert.equal(r.partReap.reaped, true, "and the outcome is REPORTED, not assumed");
  assert.ok(deletedDocuments.includes(r.documentId), "the document is still reaped too");
});

check("a failed part creation is SETUP_FAILED naming that stage, and the document is reaped", async () => {
  deletedDocuments.length = 0;
  partCreateStatus = 500;
  const path = join(dir, "d.jsonl");
  const sink = [];
  const r = await runEpisode({
    task, policy: scriptedPolicy([{ tool: "create_cylinder", args: {} }]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: path, kernelSha: "abc",
    spawn: fakeSpawn(sink, { visible_count: 1 }),
  });
  partCreateStatus = null;
  assert.equal(r.outcome, "SETUP_FAILED");
  assert.match(r.error, /part creation/,
    `the stage must be named — a 500 here and a dead spawn must not read alike; got ${JSON.stringify(r.error)}`);
  assert.equal(sink.length, 0, "no session was spawned");
  assert.ok(deletedDocuments.includes(r.documentId),
    "the document already existed when the part failed, and must not be orphaned");
  const { terminal } = readTrajectory(path);
  assert.ok(terminal.error.includes("part creation"), "the record names it too");
  assert.ok(terminal.claims[0].absent.includes("part creation"),
    "and every claim's absence carries the concrete stage, not just the category");
});

check("a 429 on part creation is RATE_LIMITED, never SETUP_FAILED", async () => {
  partCreateStatus = 429;
  const r = await runEpisode({
    task, policy: scriptedPolicy([{ tool: "create_cylinder", args: {} }]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: join(dir, "e.jsonl"),
    kernelSha: "abc", spawn: fakeSpawn([], { visible_count: 1 }),
  });
  partCreateStatus = null;
  assert.equal(r.outcome, "RATE_LIMITED",
    "the shared EvalHarness class refusing the second setup call is the rate " +
    "ceiling this outcome exists to surface (auth_middleware.rs:870-874), not " +
    "a setup defect");
});

check("childEnv pins the child to its part, and never leaks an inherited one", async () => {
  const { childEnv } = await session();
  const withPart = childEnv({ documentId: "doc-1", partId: "p-uuid", baseUrl, credential: {} });
  assert.equal(withPart.ROSHERA_PART, "p-uuid");
  assert.equal(withPart.ROSHERA_DOCUMENT, "doc-1", "the document pin is untouched");
  // The child inherits `process.env`, so a stale ROSHERA_PART in the PARENT's
  // environment would silently pin an episode that asked for no part at all.
  process.env.ROSHERA_PART = "inherited-from-the-parent";
  const without = childEnv({ documentId: "doc-1", partId: null, baseUrl, credential: {} });
  delete process.env.ROSHERA_PART;
  assert.ok(!("ROSHERA_PART" in without),
    "an absent part must leave NO key — inheriting one is how a session ends " +
    "up scoped to a model it never created");
});

check("readModelScope reports what THIS session's model holds, from list_parts", async () => {
  const { readModelScope } = await session();
  const isolated = await readModelScope(async () => ok([summary(1)]), ["uuid-a"]);
  assert.equal(isolated.read_by, "list_parts");
  assert.deepEqual(isolated.visible_parts, [1]);
  assert.equal(isolated.visible_count, 1);
  assert.equal(isolated.built_here, 1);
  assert.equal(isolated.shared_model_detected, false);

  // What eight episodes sharing one model actually looks like on the wire.
  const shared = await readModelScope(
    async () => ok([1, 2, 3, 4, 5, 6, 7, 8].map(summary)), ["uuid-a"],
  );
  assert.equal(shared.visible_count, 8);
  assert.equal(shared.shared_model_detected, true,
    "a session that can see seven solids it never built is reading another " +
    "episode's model, and the record must say so");
});

check("a refused or failed list_parts is a STATED absence, never a fabricated zero", async () => {
  const { readModelScope } = await session();
  const refused = await readModelScope(
    async () => fail("GET /api/agent/parts → 404: part not found"), [],
  );
  assert.equal(refused.visible_count, undefined, "no count is invented");
  assert.match(refused.absent, /list_parts/,
    `the absence must name what did not happen; got ${JSON.stringify(refused)}`);
});

check("the terminal record carries model_scope, so a shared model shows in the trajectory", async () => {
  const path = join(dir, "f.jsonl");
  const shared = {
    read_by: "list_parts", visible_parts: [1, 2, 3], visible_count: 3,
    built_here: 1, shared_model_detected: true,
  };
  const r = await runEpisode({
    task, policy: scriptedPolicy([{ tool: "create_cylinder", args: {} }]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: path, kernelSha: "abc",
    spawn: fakeSpawn([], shared),
  });
  assert.deepEqual(r.modelScope, shared,
    "and on the returned result too, so the batch can say it in its own summary");
  const { terminal } = readTrajectory(path);
  assert.deepEqual(terminal.model_scope, shared,
    "the reading is IN the record: a batch is read from its trajectories " +
    "afterwards, and a shared model that shows up nowhere in them is the " +
    "defect that survived eight live episodes");
});

check("an episode that never scored carries a STATED model_scope absence", async () => {
  const path = join(dir, "g.jsonl");
  const forever = { async act() { return { tool: "create_cylinder", args: {} }; }, tokensUsed: () => 0 };
  await runEpisode({
    task, policy: forever, seed: 1, baseUrl, authHeader: {},
    trajectoryPath: path, kernelSha: "abc", spawn: fakeSpawn([], { visible_count: 1 }),
  });
  const { terminal } = readTrajectory(path);
  assert.equal(terminal.outcome, "BUDGET_EXHAUSTED");
  assert.equal(terminal.model_scope.visible_count, undefined);
  assert.ok(typeof terminal.model_scope.absent === "string" && terminal.model_scope.absent.length > 0,
    "a bare null would read as 'the model held nothing', a different and false claim");
});

let failed = 0;
for (const [name, fn] of checks) {
  try {
    await fn();
    process.stdout.write(`  ok - ${name}\n`);
  } catch (e) {
    failed += 1;
    process.stdout.write(`  NOT OK - ${name}\n      ${String(e?.message ?? e).split("\n").join("\n      ")}\n`);
  }
}
stub.close();
rmSync(dir, { recursive: true, force: true });
process.stdout.write(`\npart_isolation: ${checks.length - failed}/${checks.length} checks passed\n`);
if (failed > 0) process.exitCode = 1;
