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
import { mkdtempSync, rmSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
import { runEpisode, unverifiedMutatingWork, verifyClaimActuallyMeasured } from "../lib/episode.mjs";
import { readTrajectory } from "../lib/trajectory.mjs";
import { scriptedPolicy } from "../lib/policy.mjs";
import { defineTask } from "../lib/task.mjs";
import { readToolResult } from "../lib/mcp_session.mjs";

const dir = mkdtempSync(join(tmpdir(), "roshera-ep-"));
const created = [];
const deleted = [];
const parts = [];
let failCreate = false;
let createStatus = 500;

const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  // The episode allocates its own BRepModel beside its document
  // (api-server/src/part_mgr.rs:340-358) and drops it at reap (:416-427).
  if (req.method === "POST" && url === "/api/parts") {
    const id = `part-${parts.length}`;
    parts.push(id);
    res.writeHead(200, { "Content-Type": "application/json" });
    return res.end(JSON.stringify({ id }));
  }
  if (req.method === "DELETE" && url.startsWith("/api/parts/")) {
    res.writeHead(200, { "Content-Type": "application/json" });
    return res.end(JSON.stringify({ success: true, id: url.split("/").pop() }));
  }
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
/**
 * A successful op result whose gate-3 pre-flight could NOT complete —
 * `registry.ts`'s `attachGatePreflightGaps` merging `gates.ts`'s
 * `GatePreflightGap[]` into the op's own JSON (item 1, audit S4). Copied
 * verbatim from that shape, the same discipline `CREATED_OK` above follows:
 * a fake that speaks a friendlier shape than the wire proves nothing.
 */
const CREATED_WITH_GATE_PREFLIGHT = ok({
  object_uuid: "3f2b8c1e-77aa-4a9f-8b21-9f0f2a6d5e10", part_id: 1, placement: null,
  perception: { sound: true, brep_valid: true, watertight: true, volume: 117809.7, face_count: 3 },
  gate_preflight: "unavailable",
  gate_preflight_gaps: [
    {
      ref: "3f2b8c1e-77aa-4a9f-8b21-9f0f2a6d5e10",
      stage: "verify",
      reason:
        "GET /api/agent/parts/1/perception → timed out after 4000ms (backend " +
        "may still be computing a heavy op; raise ROSHERA_MCP_TIMEOUT_MS)",
    },
  ],
});

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
    // mcp_session.mjs `readModelScope` — one solid, the one this episode built.
    async modelScope() {
      return {
        read_by: "list_parts", visible_parts: [1], visible_count: 1,
        built_here: 1, shared_model_detected: false,
      };
    },
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

check("a policy cannot edit its own history to erase a recorded refusal", async () => {
  // `Object.freeze(rewards.slice())` froze the ARRAY; the entries were the very
  // objects `mergeFinal(rewards)` reads afterwards. So `history[0].components
  // .refused = null` silently deleted a refusal from the terminal tally — the
  // episode rewriting the record of itself, in the same call that freezes the
  // task and the script precisely to stop that.
  const path = join(dir, "n.jsonl");
  /** gates.ts:121-131 gateRefusal(payload) → envelope. */
  const REFUSED = readToolResult({
    content: [{ type: "text", text: JSON.stringify({
      refused: true, gate: "verification_scope",
      reason: "solid-mutating ops ran under this checkpoint with no verify_part since",
    }, null, 2) }],
    isError: true,
  });
  let meddled = 0;
  const meddling = {
    async act({ history }) {
      if (history.length === 0) return { tool: "create_cylinder", args: {} };
      meddled += 1;
      // Every level a reward vector has: the entry, its components object, its
      // gaps array, and an object inside that array.
      try { history[0].components.refused = null; } catch { /* frozen, as intended */ }
      try { history[0].components.sound = true; } catch { /* frozen */ }
      try { history[0].gaps.length = 0; } catch { /* frozen */ }
      try { if (history[0].gaps[0]) history[0].gaps[0].reason = "nothing to see"; } catch { /* frozen */ }
      return { done: true };
    },
    tokensUsed: () => 0,
  };
  const r = await runEpisode({
    task, policy: meddling, seed: 1, baseUrl, authHeader: {},
    trajectoryPath: path, kernelSha: "abc", spawn: fakeSpawn(() => REFUSED),
  });
  assert.equal(meddled, 1, "the policy really did get its hands on the history");
  assert.equal(r.outcome, "COMPLETED");
  const { terminal } = readTrajectory(path);
  assert.equal(terminal.reward_final.components.refusals, 1,
    "a refusal the kernel issued must survive the policy that earned it");
  assert.ok(!("sound" in terminal.reward_final.components),
    "and a soundness verdict that was never measured must not appear from nowhere");
  assert.ok(terminal.reward_final.gaps.some((g) => g.name === "sound"));
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

check("a SETUP_FAILED names WHICH stage failed and carries the underlying error", async () => {
  // The live run hit this twice — a 401 on document creation, and a spawn that
  // died on a missing dependency — and both wrote the SAME reason string,
  // "document creation or spawn failed", with the real error discarded.
  // Diagnosing it took a hand-run probe. A stated reason that names both
  // possibilities and commits to neither is not a stated reason.
  const path = join(dir, "m1.jsonl");
  failCreate = true; createStatus = 401;
  const r1 = await runEpisode({
    task, policy: scriptedPolicy([]), seed: 1, baseUrl, authHeader: {},
    trajectoryPath: path, kernelSha: "abc", spawn: fakeSpawn(() => CREATED_OK),
  });
  failCreate = false; createStatus = 500;
  assert.equal(r1.outcome, "SETUP_FAILED");
  const t1 = readTrajectory(path).terminal;
  assert.match(t1.error, /document creation/i, "the RECORD, not only the return value, names the stage");
  assert.match(t1.error, /401/, "and carries the underlying error text");
  for (const c of t1.claims) {
    assert.match(c.absent, /document creation/i,
      "the per-claim absence says which stage denied the measurement");
  }

  const path2 = join(dir, "m2.jsonl");
  const r2 = await runEpisode({
    task, policy: scriptedPolicy([]), seed: 1, baseUrl, authHeader: {},
    trajectoryPath: path2, kernelSha: "abc",
    spawn: async () => { throw new Error("ECONNREFUSED: cannot find module @modelcontextprotocol/sdk"); },
  });
  assert.equal(r2.outcome, "SETUP_FAILED");
  const t2 = readTrajectory(path2).terminal;
  assert.match(t2.error, /spawn/i, "a spawn failure is a DIFFERENT stage and says so");
  assert.match(t2.error, /ECONNREFUSED/, "and carries the real reason, not a disjunction");
  assert.ok(!/document creation/i.test(t2.error),
    "the two setup failures must be distinguishable from the record alone");
  for (const c of t2.claims) assert.match(c.absent, /spawn/i);
});

check("a crash's reason reaches the trajectory terminal too, not only the return value", async () => {
  const path = join(dir, "m3.jsonl");
  const r = await runEpisode({
    task, policy: scriptedPolicy([{ tool: "create_cylinder", args: {} }]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: path, kernelSha: "abc",
    spawn: fakeSpawn(() => { throw new Error("EPIPE: MCP process died"); }),
  });
  assert.equal(r.outcome, "CRASHED");
  const { terminal } = readTrajectory(path);
  assert.match(terminal.error, /EPIPE/,
    "whoever reads the trajectory afterwards must not have to re-run the batch to learn why");
});

check("a COMPLETED terminal carries error: null — a determinate absence, not a missing key", async () => {
  const path = join(dir, "m4.jsonl");
  await runEpisode({
    task, policy: scriptedPolicy([{ tool: "create_cylinder", args: { radius: 25 } }]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: path, kernelSha: "abc",
    spawn: fakeSpawn(() => CREATED_OK),
  });
  const { terminal } = readTrajectory(path);
  assert.ok("error" in terminal, "the field is always present, so its absence is never ambiguous");
  assert.equal(terminal.error, null);
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

check("a step whose gate-3 pre-flight was unavailable records that fact in the JSONL a trajectory writes", async () => {
  // THE LOAD-BEARING CASE for item 1 (audit S4). A gates.ts unit test proves
  // the marker CONSTRUCTS correctly; it does not prove anything in production
  // carries it end to end. This repo has the exact incident on record: the
  // kernel started emitting a fidelity block and `core.ts` rebuilt perception
  // from a FIXED KEY SET, so the block reached NO agent behind 42 green
  // tests. `Trajectory.step()` (trajectory.mjs) writes exactly such a fixed
  // key set — `{i, action, result_digest, reward, refusal, ms}` — and
  // `episode.mjs`'s own `traj.step()` call only ever forwards a DIGEST of
  // `result.data`, never the raw value, so this is the one place the same
  // failure mode could recur one layer downstream of gates.ts.
  const path = join(dir, "gate-preflight.jsonl");
  const r = await runEpisode({
    task, policy: scriptedPolicy([{ tool: "create_cylinder", args: { radius: 25 } }]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: path, kernelSha: "abc",
    spawn: fakeSpawn(() => CREATED_WITH_GATE_PREFLIGHT),
  });
  assert.equal(r.outcome, "COMPLETED");
  const { steps } = readTrajectory(path);
  assert.equal(steps.length, 1);
  const step = steps[0];
  assert.equal(step.gate_preflight, "unavailable",
    "the STEP LINE ITSELF must carry the fact — a digest of result.data cannot be " +
    "read back by anything scoring the trajectory afterward, only compared byte-for-byte");
  assert.equal(Array.isArray(step.gate_preflight_gaps), true);
  assert.equal(step.gate_preflight_gaps.length, 1);
  assert.equal(step.gate_preflight_gaps[0].ref, "3f2b8c1e-77aa-4a9f-8b21-9f0f2a6d5e10");
  assert.equal(step.gate_preflight_gaps[0].stage, "verify");
  assert.match(step.gate_preflight_gaps[0].reason, /timed out after 4000ms/);
});

check("a healthy step (no preflight gap) writes no gate_preflight key at all", async () => {
  // The mirror assertion: a normal successful step must NOT grow a new key —
  // an absent marker means "the gate ran", and that must stay true in the
  // JSONL as much as in the raw tool result (item 1's own constraint).
  const path = join(dir, "gate-preflight-clean.jsonl");
  await runEpisode({
    task, policy: scriptedPolicy([{ tool: "create_cylinder", args: { radius: 25 } }]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: path, kernelSha: "abc",
    spawn: fakeSpawn(() => CREATED_OK),
  });
  const { steps } = readTrajectory(path);
  assert.equal(steps.length, 1);
  assert.equal("gate_preflight" in steps[0], false);
  assert.equal("gate_preflight_gaps" in steps[0], false);
});

// ─── item 7 (audit S3.1) — gate 6's by-omission escape, closed at the episode's own end ──
//
// Gate 6 fires only when a NEW checkpoint closes the open one; an episode
// that opens one checkpoint, mutates, and simply STOPS is never asked to
// verify anything. The design ruling: close this in roshera-rl, not MCP,
// because the episode is the only place a session has a defined end — a
// PURE function over the step tool/success list `runEpisode`'s own loop
// already gathers, no MCP query, no new tool.
//
// `unverifiedMutatingWork` unit-tested directly first (every branch of its
// own mirror of gates.ts's `MUTATES_SOLIDS`/`VERIFIES`/`intentUnverified`
// bookkeeping — gates.ts:269-290, :347, :1069-1089), then proved end to end
// through `runEpisode` for the two directions the brief names explicitly.

check("unverifiedMutatingWork: a mutating success with nothing after it is one tool, count one", () => {
  const r = unverifiedMutatingWork([{ tool: "create_cylinder", ok: true }]);
  assert.deepEqual(r, { count: 1, tools: ["create_cylinder"] });
});

check("unverifiedMutatingWork: an empty step list is the clean answer, not an absence", () => {
  assert.deepEqual(unverifiedMutatingWork([]), { count: 0, tools: [] });
});

check("unverifiedMutatingWork: verify_part clears everything that preceded it", () => {
  const r = unverifiedMutatingWork([
    { tool: "create_cylinder", ok: true },
    { tool: "verify_part", ok: true },
  ]);
  assert.deepEqual(r, { count: 0, tools: [] });
});

check("unverifiedMutatingWork: verify_claim clears too, when it actually measured something", () => {
  const r = unverifiedMutatingWork([
    { tool: "boolean", ok: true },
    { tool: "verify_claim", ok: true, claimMeasured: true },
  ]);
  assert.deepEqual(r, { count: 0, tools: [] });
});

// ─── F1 (2026-08-15 review H3) — the load-bearing case: a contentless
//     verify_claim between a mutation and the episode's end must NOT read as
//     verified work. `episode.mjs` copies gates.ts's clearing rule, not only
//     its VERIFIES set (see H3: `VerifyMeasure::Constant` references no part
//     at all, and the handler is structurally always HTTP 200).

check("unverifiedMutatingWork: a verify_claim step with no claimMeasured:true does NOT clear — the conservative default", () => {
  const r = unverifiedMutatingWork([
    { tool: "boolean", ok: true },
    { tool: "verify_claim", ok: true }, // no claimMeasured key at all
  ]);
  assert.deepEqual(r, { count: 1, tools: ["boolean"] }, "measured nothing — not a look");
});

check("unverifiedMutatingWork: a verify_claim step explicitly marked claimMeasured:false does NOT clear", () => {
  const r = unverifiedMutatingWork([
    { tool: "boolean", ok: true },
    { tool: "verify_claim", ok: true, claimMeasured: false },
  ]);
  assert.deepEqual(r, { count: 1, tools: ["boolean"] });
});

check("unverifiedMutatingWork: verify_part still clears unconditionally — no claimMeasured needed", () => {
  const r = unverifiedMutatingWork([
    { tool: "boolean", ok: true },
    { tool: "verify_part", ok: true },
  ]);
  assert.deepEqual(r, { count: 0, tools: [] });
});

check("verifyClaimActuallyMeasured: all-constant bindings measure nothing, however cleanly they verify", () => {
  const args = { bindings: [{ var: "x", measure: { kind: "constant", value: 1 } }] };
  const data = { verified: true, refused: false, computed: 1 };
  assert.equal(verifyClaimActuallyMeasured(args, data), false);
});

check("verifyClaimActuallyMeasured: a refused verdict is not a look, even with a real binding", () => {
  const args = { bindings: [{ var: "v", measure: { kind: "volume", part: "some-uuid" } }] };
  const data = { verified: false, refused: true, computed: null };
  assert.equal(verifyClaimActuallyMeasured(args, data), false);
});

check("verifyClaimActuallyMeasured: a real binding, not refused, IS a look", () => {
  const args = { bindings: [{ var: "v", measure: { kind: "volume", part: "some-uuid" } }] };
  const data = { verified: true, refused: false, computed: 8 };
  assert.equal(verifyClaimActuallyMeasured(args, data), true);
});

check("verifyClaimActuallyMeasured: malformed args/data degrade to false, never throw", () => {
  assert.equal(verifyClaimActuallyMeasured(null, null), false);
  assert.equal(verifyClaimActuallyMeasured({}, {}), false);
  assert.equal(verifyClaimActuallyMeasured(undefined, undefined), false);
});

check("unverifiedMutatingWork: a NEW timeline_checkpoint resets the tally — it already passed gate 6 itself to close", () => {
  const r = unverifiedMutatingWork([
    { tool: "create_cylinder", ok: true },
    { tool: "timeline_checkpoint", ok: true },
  ]);
  assert.deepEqual(r, { count: 0, tools: [] });
});

check("unverifiedMutatingWork: clear_timeline resets the tally too — gate 6 excludes it by the same design as gates.ts", () => {
  const r = unverifiedMutatingWork([
    { tool: "boolean", ok: true },
    { tool: "clear_timeline", ok: true },
  ]);
  assert.deepEqual(r, { count: 0, tools: [] });
});

check("unverifiedMutatingWork: a refused mutating call built nothing, so it counts for nothing", () => {
  assert.deepEqual(unverifiedMutatingWork([{ tool: "create_cylinder", ok: false }]), { count: 0, tools: [] });
});

check("unverifiedMutatingWork: distinct verbs in `tools`, every call still tallied in `count`", () => {
  const r = unverifiedMutatingWork([
    { tool: "boolean", ok: true },
    { tool: "boolean", ok: true },
    { tool: "fillet_edges", ok: true },
  ]);
  assert.equal(r.count, 3, "count is every dispatch, matching gates.ts's own intentUnverified.count");
  assert.deepEqual(r.tools, ["boolean", "fillet_edges"], "tools is the distinct-verb Set, matching gates.ts's own intentUnverified.tools");
});

check("unverifiedMutatingWork: malformed input degrades to the clean answer rather than throwing", () => {
  assert.deepEqual(unverifiedMutatingWork(null), { count: 0, tools: [] });
  assert.deepEqual(unverifiedMutatingWork(undefined), { count: 0, tools: [] });
  assert.deepEqual(
    unverifiedMutatingWork([null, {}, { tool: "create_cylinder", ok: true }]),
    { count: 1, tools: ["create_cylinder"] },
  );
});

/** verify_part's own body: `sound` at the TOP LEVEL (tools/perception.ts:206), no `perception` wrapper. */
const VERIFIED_OK = ok({ sound: true, part_id: 1, brep_valid: true, watertight: true });

check("an episode that ends with mutating work never verified is flagged in its terminal record", async () => {
  const path = join(dir, "unverified-mutation.jsonl");
  const r = await runEpisode({
    task, policy: scriptedPolicy([{ tool: "create_cylinder", args: { radius: 25 } }]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: path, kernelSha: "abc",
    spawn: fakeSpawn((tool) => (tool === "verify_part" ? VERIFIED_OK : CREATED_OK)),
  });
  assert.equal(r.outcome, "COMPLETED");
  const { terminal } = readTrajectory(path);
  assert.ok(terminal.unverified_mutations,
    "the field must be PRESENT, not silently omitted — S3.1's whole exploit is a record that stays quiet");
  assert.equal(terminal.unverified_mutations.count, 1);
  assert.deepEqual(terminal.unverified_mutations.tools, ["create_cylinder"]);
});

check("an episode that verifies its mutating work before ending is NOT flagged", async () => {
  const path = join(dir, "verified-mutation.jsonl");
  const r = await runEpisode({
    task, policy: scriptedPolicy([
      { tool: "create_cylinder", args: { radius: 25 } },
      { tool: "verify_part", args: { part_id: 1 } },
    ]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: path, kernelSha: "abc",
    spawn: fakeSpawn((tool) => (tool === "verify_part" ? VERIFIED_OK : CREATED_OK)),
  });
  assert.equal(r.outcome, "COMPLETED");
  const { terminal } = readTrajectory(path);
  assert.equal(terminal.unverified_mutations.count, 0);
  assert.deepEqual(terminal.unverified_mutations.tools, []);
});

// ─── F1 (2026-08-15 review H3) — end to end through `runEpisode`: the load-
//     bearing case named in the fix brief. A contentless verify_claim between
//     a mutation and the episode's own end must NOT let `unverified_mutations`
//     read as verified — the corpus field item 7 added must not inherit the
//     same hole H3 found in gates.ts's live gate.

const CLAIM_TASK = defineTask({
  id: "t-claim", prompt: "p", toolAllowlist: ["create_cylinder", "verify_claim"],
  claims: [{
    name: "volume", expr: "v",
    bindings: [{ var: "v", measure: { kind: "volume", part: "solid:0" } }],
    expected: 117809.724509617, tolerance: 117.8,
  }],
  stepBudget: 3, tokenBudget: 1000, split: "train",
});

/** claim.rs's ClaimVerdict shape for an all-constant, genuinely-successful
 *  claim: `verified: true`, but nothing about any part was ever measured. */
const CONSTANT_ONLY_CLAIM_OK = ok({
  verified: true, refused: false, computed: 1, expected: 1,
  abs_error: 0, tolerance_used: 1e-6, resolved: [["x", 1]], unresolved: [],
});

/** claim.rs's ClaimVerdict shape for a REFUSED claim (unresolved binding) —
 *  structurally still a 200/`ok()` result, per H3's first finding. */
const REFUSED_CLAIM = ok({
  verified: false, refused: true, computed: null, expected: 8,
  abs_error: null, tolerance_used: 1e-6, resolved: [], unresolved: ["v"],
});

check("F1: an all-constant verify_claim does not clear unverified_mutations end to end", async () => {
  const path = join(dir, "f1-constant-claim.jsonl");
  await runEpisode({
    task: CLAIM_TASK,
    policy: scriptedPolicy([
      { tool: "create_cylinder", args: { radius: 25 } },
      {
        tool: "verify_claim",
        args: {
          expr: "x",
          bindings: [{ var: "x", measure: { kind: "constant", value: 1 } }],
          expected: 1,
        },
      },
    ]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: path, kernelSha: "abc",
    spawn: fakeSpawn((tool) => (tool === "verify_claim" ? CONSTANT_ONLY_CLAIM_OK : CREATED_OK)),
  });
  const { terminal } = readTrajectory(path);
  assert.equal(terminal.unverified_mutations.count, 1,
    "the constant-only claim measured nothing about the cylinder — F1's load-bearing case");
  assert.deepEqual(terminal.unverified_mutations.tools, ["create_cylinder"]);
});

check("F1: a REFUSED verify_claim (structurally a 200) does not clear unverified_mutations end to end", async () => {
  const path = join(dir, "f1-refused-claim.jsonl");
  await runEpisode({
    task: CLAIM_TASK,
    policy: scriptedPolicy([
      { tool: "create_cylinder", args: { radius: 25 } },
      {
        tool: "verify_claim",
        args: {
          expr: "v",
          bindings: [{ var: "v", measure: { kind: "volume", part: "bad-uuid" } }],
          expected: 8,
        },
      },
    ]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: path, kernelSha: "abc",
    spawn: fakeSpawn((tool) => (tool === "verify_claim" ? REFUSED_CLAIM : CREATED_OK)),
  });
  const { terminal } = readTrajectory(path);
  assert.equal(terminal.unverified_mutations.count, 1,
    "a claim the kernel declined to evaluate is not a look");
  assert.deepEqual(terminal.unverified_mutations.tools, ["create_cylinder"]);
});

check("F1: a verify_claim over a REAL binding, not refused, DOES clear unverified_mutations end to end", async () => {
  const path = join(dir, "f1-real-claim.jsonl");
  const REAL_CLAIM_OK = ok({
    verified: true, refused: false, computed: 117809.724509617, expected: 117809.724509617,
    abs_error: 0, tolerance_used: 117.8, resolved: [["v", 117809.724509617]], unresolved: [],
  });
  await runEpisode({
    task: CLAIM_TASK,
    policy: scriptedPolicy([
      { tool: "create_cylinder", args: { radius: 25 } },
      {
        tool: "verify_claim",
        args: {
          expr: "v",
          bindings: [{ var: "v", measure: { kind: "volume", part: "3f2b8c1e-77aa-4a9f-8b21-9f0f2a6d5e10" } }],
          expected: 117809.724509617,
        },
      },
    ]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: path, kernelSha: "abc",
    spawn: fakeSpawn((tool) => (tool === "verify_claim" ? REAL_CLAIM_OK : CREATED_OK)),
  });
  const { terminal } = readTrajectory(path);
  assert.equal(terminal.unverified_mutations.count, 0, "a genuine measurement clears it, exactly as before");
  assert.deepEqual(terminal.unverified_mutations.tools, []);
});

check("a mutating call the kernel refused contributes no unverified work — nothing was built to verify", async () => {
  const path = join(dir, "refused-mutation.jsonl");
  await runEpisode({
    task, policy: scriptedPolicy([{ tool: "create_cylinder", args: { radius: 25 } }]),
    seed: 1, baseUrl, authHeader: {}, trajectoryPath: path, kernelSha: "abc",
    spawn: fakeSpawn(() => fail("kernel refused the call")),
  });
  const { terminal } = readTrajectory(path);
  assert.equal(terminal.unverified_mutations.count, 0);
  assert.deepEqual(terminal.unverified_mutations.tools, []);
});

check("a SETUP_FAILED episode (no steps ever ran) reports the clean answer, not an absence — zero steps really did run", async () => {
  const path = join(dir, "unverified-mutation-setup-failed.jsonl");
  failCreate = true; createStatus = 500;
  await runEpisode({
    task, policy: scriptedPolicy([]), seed: 1, baseUrl, authHeader: {},
    trajectoryPath: path, kernelSha: "abc", spawn: fakeSpawn(() => CREATED_OK),
  });
  failCreate = false;
  const { terminal } = readTrajectory(path);
  assert.deepEqual(terminal.unverified_mutations, { count: 0, tools: [] });
});

// ─── the copy of gates.ts's MUTATES_SOLIDS is PINNED, not merely disclosed ───
//
// `episode.mjs` copies `MUTATES_SOLIDS` rather than importing it, for a good
// reason: gates.ts lives in a sibling package with its own build, and item 7
// asks for a PURE function over data this package already holds, not a new
// cross-package dependency. But a copy is two independently-maintained
// surfaces that only disagree at runtime — the failure class this repo keeps
// an ontology-drift gate for, after `psketch_plane_from_face` was classified
// MCP-side and never added backend-side.
//
// The precedent the source comment cites for accepting the drift
// (`handlers/timeline.rs:5478-5484`) is in fact a case where the repo
// MECHANICALLY PINNED two surfaces across packages rather than living with
// it. This does the same, in the same style: read the other surface from
// disk and assert set equality. If gates.ts gains a mutating verb — as it did
// twice on this branch alone — this fails until the copy follows.
check("the MUTATES_SOLIDS copy equals gates.ts's, verb for verb", () => {
  const setLiteral = (src, name) => {
    const m = src.match(new RegExp(`const ${name}\\s*=\\s*new Set(?:<string>)?\\(\\[([\\s\\S]*?)\\]\\)`));
    assert.ok(m, `could not locate ${name}'s set literal — the parse, not the sets, is broken`);
    return new Set([...m[1].matchAll(/"([a-z0-9_]+)"/g)].map((x) => x[1]));
  };
  const gatesSrc = readFileSync(join(HERE, "..", "..", "roshera-mcp", "src", "gates.ts"), "utf8");
  const episodeSrc = readFileSync(join(HERE, "..", "lib", "episode.mjs"), "utf8");

  const theirs = setLiteral(gatesSrc, "MUTATES_SOLIDS");
  const ours = setLiteral(episodeSrc, "MUTATES_SOLIDS");

  // Vacuity guard: a regex that silently matched nothing would make two empty
  // sets compare equal and this check would pass forever while proving zero.
  assert.ok(theirs.size >= 15, `parsed only ${theirs.size} verbs from gates.ts — the parse is broken`);
  assert.ok(theirs.has("boolean") && ours.has("boolean"), "neither set should be missing 'boolean'");

  const missingHere = [...theirs].filter((t) => !ours.has(t));
  const extraHere = [...ours].filter((t) => !theirs.has(t));
  assert.deepEqual(
    { missingHere, extraHere }, { missingHere: [], extraHere: [] },
    "gates.ts and episode.mjs disagree about which tools mutate solids. An episode's " +
    "unverified-mutation check would then miss work the gate counts, or flag work it does not.",
  );
});

// ─── the copy of gates.ts's VERIFIES is PINNED too (M3 / F1 follow-up) ──────
//
// M3 (2026-08-15 review): `VERIFIES` was copied (gates.ts:388, episode.mjs)
// but never pinned — only `MUTATES_SOLIDS` was. Same shape, same regex, same
// vacuity-guard discipline as the pin above; a third verification verb added
// to gates.ts now fails this test until episode.mjs's copy follows, instead
// of silently making the episode-end check over- or under-report forever.
//
// WHAT THIS DOES NOT PIN, stated plainly rather than implied: SET equality
// proves the two files agree on WHICH tools count as "verifying" — it proves
// nothing about the CLEARING RULE this fix (F1) attached to `verify_claim`
// specifically (`verifyClaimActuallyMeasured` in both files: refused-body and
// all-constant-bindings checks). Two files can carry byte-identical
// `VERIFIES` sets while one clears unconditionally and the other does not —
// exactly the review's own point about M2/M3 ("membership parity proves
// nothing about [behaviour]"). That agreement is NOT mechanically pinned
// here; it is covered only by the parallel hand-written scenarios above (the
// gates.ts suite's "F1" checks in verification_scope_gate.test.mjs and this
// file's "F1" checks), which a future edit to either copy's clearing logic
// could still defeat without this test noticing. A mechanical pin over
// `verifyClaimActuallyMeasured`'s BODY (not just its name) would need a
// source-level behavioural-equivalence check neither file has infrastructure
// for today — left as a named gap, not silently assumed covered.
check("the VERIFIES copy equals gates.ts's, verb for verb", () => {
  const setLiteral = (src, name) => {
    const m = src.match(new RegExp(`const ${name}\\s*=\\s*new Set(?:<string>)?\\(\\[([\\s\\S]*?)\\]\\)`));
    assert.ok(m, `could not locate ${name}'s set literal — the parse, not the sets, is broken`);
    return new Set([...m[1].matchAll(/"([a-z0-9_]+)"/g)].map((x) => x[1]));
  };
  const gatesSrc = readFileSync(join(HERE, "..", "..", "roshera-mcp", "src", "gates.ts"), "utf8");
  const episodeSrc = readFileSync(join(HERE, "..", "lib", "episode.mjs"), "utf8");

  const theirs = setLiteral(gatesSrc, "VERIFIES");
  const ours = setLiteral(episodeSrc, "VERIFIES");

  // Vacuity guard: a regex that silently matched nothing would make two empty
  // sets compare equal and this check would pass forever while proving zero.
  assert.ok(theirs.size >= 2, `parsed only ${theirs.size} verbs from gates.ts — the parse is broken`);
  assert.ok(theirs.has("verify_part") && ours.has("verify_part"), "neither set should be missing 'verify_part'");
  assert.ok(theirs.has("verify_claim") && ours.has("verify_claim"), "neither set should be missing 'verify_claim'");

  const missingHere = [...theirs].filter((t) => !ours.has(t));
  const extraHere = [...ours].filter((t) => !theirs.has(t));
  assert.deepEqual(
    { missingHere, extraHere }, { missingHere: [], extraHere: [] },
    "gates.ts and episode.mjs disagree about which tools count as having verified. An " +
    "episode's unverified-mutation check would then miss a verb the gate counts, or " +
    "flag one it does not.",
  );
});

for (const [name, fn] of checks) { await fn(); process.stdout.write(`  ok - ${name}\n`); }
stub.close();
rmSync(dir, { recursive: true, force: true });
process.stdout.write(`\nepisode: ${checks.length} checks passed\n`);
