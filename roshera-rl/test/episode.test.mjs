/**
 * Episode-lifecycle proof.
 *
 * Every episode lands in exactly one named outcome. The taxonomy is borrowed
 * from exploration.rs because it already got the important part right: an
 * episode that never ran must never be reported as an episode that ran and
 * scored nothing. A zero-scored COMPLETED for a crashed process is the same
 * class of lie as a fabricated fidelity zero.
 *
 * The injected sessions here return `readToolResult` ENVELOPES built from
 * results copied out of the MCP source, because a fake that speaks a
 * friendlier shape than the wire is how CRITICAL 1-4 survived seven reviews.
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
import { readToolResult } from "../lib/mcp_session.mjs";

const dir = mkdtempSync(join(tmpdir(), "roshera-ep-"));
const created = [];
const deleted = [];
let failCreate = false;
let createStatus = 500;

const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  if (req.method === "POST" && url === "/api/documents") {
    if (failCreate) { res.writeHead(createStatus); return res.end("{}"); }
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

/** core.ts:380-385 ok(data) → envelope. */
const ok = (data) => readToolResult({
  content: [{ type: "text", text: JSON.stringify(data, null, 2) }],
});
/** core.ts:485-497 fail(e) → envelope. */
const fail = (msg) => readToolResult({
  content: [{ type: "text", text: `ERROR: ${msg}` }], isError: true,
});
/** okp() in `cert` mode: create.ts:253-260 fields + the perception object. */
const CREATED_OK = ok({
  object_uuid: "3f2b8c1e-77aa-4a9f-8b21-9f0f2a6d5e10", part_id: 1, placement: null,
  perception: { sound: true, brep_valid: true, watertight: true, volume: 117809.7, face_count: 3 },
});
/** auth_middleware.rs:870-874 → core.ts:189-193. */
const RATE_LIMITED = fail(
  'POST /api/geometry/cylinder → 429: {"error":"Rate limit exceeded","code":"RATE_LIMIT_EXCEEDED","status":429}',
);

const task = defineTask({
  id: "t", prompt: "p", toolAllowlist: ["create_cylinder", "verify_part"],
  claims: [{
    name: "volume", expr: "v",
    bindings: [{ var: "v", measure: { kind: "volume", part: "solid:0" } }],
    expected: 117809.724509617, tolerance: 117.8,
  }],
  stepBudget: 3, tokenBudget: 1000, split: "train",
});

/** timeline.ts:267-283 — what recipe_get actually returns. */
const RECIPE = {
  retrieved_by: "recipe_get", reference: "doc-0",
  source: { kind: "durable_document", reference: "doc-0", branch: "main", document: "doc-0" },
  step_count: 1, sequence_range: [0, 0], sequence_contiguous: true, undecodable_events: 0,
  checkpoints: [], certificate_summary: null,
  steps: [{ sequence: 0, op_kind: "create_cylinder", params: { radius: 25, height: 60 } }],
  note: "steps embedded because the document is deleted at reap",
};

/** A fake MCP session. `behaviour` decides what each call returns. */
function fakeSpawn(behaviour) {
  return async () => ({
    async call(tool, args) { return behaviour(tool, args); },
    async claims(taskClaims) {
      return taskClaims.map((c) => ({ name: c.name, verified: true, computed: c.expected }));
    },
    async recipeRef() { return RECIPE; },
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
    spawn: fakeSpawn(() => CREATED_OK),
  });
  assert.equal(r.outcome, "COMPLETED");
  assert.ok(deleted.includes(r.documentId), "the document is dropped, not leaked");
  assert.equal(r.reap.reaped, true, "and the reap outcome is REPORTED, not assumed");
  const { header, steps, terminal } = readTrajectory(path);
  assert.deepEqual(header.tool_allowlist, ["create_cylinder", "verify_part"]);
  assert.equal(steps.length, 1);
  assert.equal(terminal.outcome, "COMPLETED");
  assert.equal(terminal.reward_final.components.sound, true);
  assert.equal(terminal.recipe_ref.step_count, 1,
    "recipe_ref carries what recipe_get actually returns — there is no `ref` field");
  assert.equal(terminal.claims[0].verified, true);
});

check("a policy that never stops hits the step budget, honestly", async () => {
  const path = join(dir, "b.jsonl");
  const forever = { async act() { return { tool: "create_cylinder", args: {} }; }, tokensUsed: () => 0 };
  const r = await runEpisode({
    task, policy: forever, seed: 1, baseUrl, authHeader: {},
    trajectoryPath: path, kernelSha: "abc", spawn: fakeSpawn(() => CREATED_OK),
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

check("a 429 RESULT is RATE_LIMITED — the shape production actually produces", async () => {
  // CRITICAL 2: `client.callTool` returns isError RESULTS; it does not throw,
  // and ApiError.status never crosses stdio. The old test injected a THROWN
  // error carrying `.status = 429`, certifying a path the wire cannot reach.
  const path = join(dir, "d.jsonl");
  const r = await runEpisode({
    task, policy: scriptedPolicy([
      { tool: "create_cylinder", args: {} }, { tool: "verify_part", args: {} },
    ]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: path, kernelSha: "abc",
    spawn: fakeSpawn(() => RATE_LIMITED),
  });
  assert.equal(r.outcome, "RATE_LIMITED",
    "8 concurrent episodes saturating the shared class must show up in the tally, " +
    "not be averaged into a lower score");
  const { steps, terminal } = readTrajectory(path);
  assert.equal(steps.length, 1, "the episode stops at the ceiling instead of burning its budget against it");
  assert.equal(terminal.reward_final.components.call_failures, 1);
});

check("a THROWN 429 is still RATE_LIMITED (secondary path)", async () => {
  // Kept because a future transport could throw, and it costs one line. It is
  // NOT the load-bearing case — the one above is.
  const path = join(dir, "d2.jsonl");
  const r = await runEpisode({
    task, policy: scriptedPolicy([{ tool: "create_cylinder", args: {} }]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: path, kernelSha: "abc",
    spawn: fakeSpawn(() => { const e = new Error("429"); e.status = 429; throw e; }),
  });
  assert.equal(r.outcome, "RATE_LIMITED");
});

check("a failed document creation is SETUP_FAILED — no episode happened", async () => {
  const path = join(dir, "e.jsonl");
  failCreate = true; createStatus = 500;
  const r = await runEpisode({
    task, policy: scriptedPolicy([]), seed: 1, baseUrl, authHeader: {},
    trajectoryPath: path, kernelSha: "abc", spawn: fakeSpawn(() => CREATED_OK),
  });
  failCreate = false;
  assert.equal(r.outcome, "SETUP_FAILED");
});

check("a 429 on document creation is RATE_LIMITED, not SETUP_FAILED", async () => {
  // Document creation is the FIRST request every episode makes, so under
  // concurrency the shared class refuses here first. Calling that a setup
  // defect would hide the ceiling this outcome exists to measure.
  const path = join(dir, "e2.jsonl");
  failCreate = true; createStatus = 429;
  const r = await runEpisode({
    task, policy: scriptedPolicy([]), seed: 1, baseUrl, authHeader: {},
    trajectoryPath: path, kernelSha: "abc", spawn: fakeSpawn(() => CREATED_OK),
  });
  failCreate = false; createStatus = 500;
  assert.equal(r.outcome, "RATE_LIMITED");
});

check("an out-of-allowlist action ends the episode as INVALID_ACTION, not BUDGET_EXHAUSTED", async () => {
  const path = join(dir, "f.jsonl");
  let called = 0;
  const r = await runEpisode({
    task, policy: scriptedPolicy([{ tool: "boolean", args: {} }]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: path, kernelSha: "abc",
    spawn: fakeSpawn(() => { called += 1; return CREATED_OK; }),
  });
  assert.equal(called, 0, "the disallowed tool must never reach the session");
  assert.equal(r.outcome, "INVALID_ACTION",
    "zero real steps run must not read the same as a genuinely exhausted budget");
  const step = readTrajectory(path).steps[0];
  assert.ok(step.reward.gaps.some((g) => g.name === "sound" && g.reason.length > 0),
    "a harness refusal measured no soundness either — an empty gaps list would " +
    "breach the absence-is-stated doctrine a kernel refusal already honours");
  assert.ok(step.reward.gaps.some((g) => g.name === "fidelity_signed"));
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
    trajectoryPath: path, kernelSha: "abc", spawn: fakeSpawn(() => CREATED_OK),
  });
  assert.equal(r.outcome, "CRASHED");
  assert.ok(r.error && r.error.includes("policy blew up"),
    "the returned object must carry the reason, the way SETUP_FAILED does");
  const { steps, terminal } = readTrajectory(path);
  assert.equal(steps.length, 1, "the crash is recorded as a step, not dropped silently");
  assert.equal(terminal.outcome, "CRASHED");
});

check("every non-COMPLETED terminal states WHY its claims were not checked", async () => {
  // `defineTask` refuses a task with zero claims, so `claims: []` is
  // impossible by construction — a consumer computing verified/total got 0/0
  // and could not tell "no claims" from "never checked".
  const cases = [
    ["BUDGET_EXHAUSTED", { async act() { return { tool: "create_cylinder", args: {} }; }, tokensUsed: () => 0 }],
    ["INVALID_ACTION", scriptedPolicy([{ tool: "boolean", args: {} }])],
    ["CRASHED", { async act() { throw new Error("boom"); }, tokensUsed: () => 0 }],
  ];
  for (const [expected, policy] of cases) {
    const path = join(dir, `i-${expected}.jsonl`);
    const r = await runEpisode({
      task, policy, seed: 1, baseUrl, authHeader: {}, trajectoryPath: path,
      kernelSha: "abc", spawn: fakeSpawn(() => CREATED_OK),
    });
    assert.equal(r.outcome, expected);
    const { terminal } = readTrajectory(path);
    assert.equal(terminal.claims.length, task.claims.length,
      `${expected}: every declared claim must be accounted for, not dropped`);
    for (const c of terminal.claims) {
      assert.equal(c.verified, null);
      assert.ok(typeof c.absent === "string" && c.absent.length > 0,
        `${expected}: an unchecked claim states WHY`);
    }
    assert.ok(typeof terminal.recipe_ref?.absent === "string",
      `${expected}: a bare null recipe_ref would read as "there was no recipe"`);
  }
});

check("a frozen action survives a session that tries to edit it mid-call", async () => {
  // WHAT deepFreeze PROTECTS, tested where it matters: `episode.mjs` holds
  // `action.args` across `session.call` and then WRITES IT to the trajectory.
  // A consumer that edits args in flight would make the trajectory record
  // something other than what was sent — a silently unreproducible episode.
  // structuredClone alone does not catch this: the clone is the very object
  // handed to `call`. Delete `deepFreeze` from policy.mjs and this goes red.
  const path = join(dir, "j.jsonl");
  const meddling = fakeSpawn((tool, args) => {
    try { args.radius = 999; } catch { /* frozen, as intended */ }
    try { args.nested.depth = 999; } catch { /* frozen at every level */ }
    return CREATED_OK;
  });
  await runEpisode({
    task,
    policy: scriptedPolicy([{ tool: "create_cylinder", args: { radius: 25, nested: { depth: 5 } } }]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: path, kernelSha: "abc",
    spawn: meddling,
  });
  const step = readTrajectory(path).steps[0];
  assert.equal(step.action.args.radius, 25,
    "the trajectory must record the args that were actually sent");
  assert.equal(step.action.args.nested.depth, 5,
    "including NESTED args — a shallow freeze would let this one through");
});

check("the result digest labelled fnv1a64 IS FNV-1a", async () => {
  // The label and the algorithm have to agree: the old digest computed
  // `h*prime ^ c` from seed 0, which is neither FNV-1a nor FNV-1, in the one
  // field a consumer would use to tell two runs apart.
  const path = join(dir, "l.jsonl");
  await runEpisode({
    task, policy: scriptedPolicy([{ tool: "create_cylinder", args: { radius: 25 } }]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: path, kernelSha: "abc",
    spawn: fakeSpawn(() => CREATED_OK),
  });
  // Reference FNV-1a/64 (offset basis 14695981039346656037, prime
  // 1099511628211, XOR-then-multiply, over the UTF-8 bytes).
  let h = 14695981039346656037n;
  for (const b of new TextEncoder().encode(JSON.stringify(CREATED_OK.data))) {
    h = ((h ^ BigInt(b)) * 1099511628211n) & 0xffffffffffffffffn;
  }
  assert.equal(readTrajectory(path).steps[0].result_digest, `fnv1a64:${h.toString(16)}`);
});

check("an unwritable trajectory path is SETUP_FAILED, not a throw", async () => {
  // The trajectory header is written synchronously before any other I/O, so
  // this was the one path out of runEpisode that could throw.
  const r = await runEpisode({
    task, policy: scriptedPolicy([]), seed: 1, baseUrl, authHeader: {},
    trajectoryPath: join(dir, "no-such-dir", "k.jsonl"), kernelSha: "abc",
    spawn: fakeSpawn(() => CREATED_OK),
  });
  assert.equal(r.outcome, "SETUP_FAILED");
  assert.ok(r.error.includes("trajectory could not be opened"));
});

for (const [name, fn] of checks) { await fn(); process.stdout.write(`  ok - ${name}\n`); }
stub.close();
rmSync(dir, { recursive: true, force: true });
process.stdout.write(`\nepisode: ${checks.length} checks passed\n`);
