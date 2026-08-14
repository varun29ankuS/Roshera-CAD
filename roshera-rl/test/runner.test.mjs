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
import { OUTCOMES, readTrajectory } from "../lib/trajectory.mjs";
import { KernelIdentityConflict } from "../lib/provenance.mjs";

const dir = mkdtempSync(join(tmpdir(), "roshera-run-"));
let n = 0;
/** documentId → how many DELETEs to refuse before accepting one. */
const refuseDeletes = new Map();
const deleteLog = [];

let parts = 0;

// What `/health` reports for `resolveKernelIdentity` to read. Defaults to no
// `build` object at all — the server-cannot-say branch, which is not a
// conflict — so every check below that does not care about kernel identity
// keeps behaving exactly as it did before `runBatch` started resolving one.
let healthReply = {};

const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  if (req.method === "GET" && url === "/health") {
    res.writeHead(200, { "Content-Type": "application/json" });
    return res.end(JSON.stringify(healthReply));
  }
  // Each episode allocates its own BRepModel (api-server/src/part_mgr.rs:
  // 340-358) and drops it at reap (:416-427).
  if (req.method === "POST" && url === "/api/parts") {
    res.writeHead(200, { "Content-Type": "application/json" });
    return res.end(JSON.stringify({ id: `part-${parts++}` }));
  }
  if (req.method === "DELETE" && url.startsWith("/api/parts/")) {
    res.writeHead(200, { "Content-Type": "application/json" });
    return res.end(JSON.stringify({ success: true, id: url.split("/").pop() }));
  }
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

/** mcp_session.mjs `readModelScope` — one solid, the one this episode built. */
const OWN_MODEL_ONLY = {
  read_by: "list_parts", visible_parts: [1], visible_count: 1,
  built_here: 1, shared_model_detected: false,
};

/** Each fake session records which document AND part it was pinned to. */
const pinnedPerSession = [];
const fakeSpawn = async ({ documentId, partId }) => {
  const seen = [];
  pinnedPerSession.push({ documentId, partId, seen });
  return {
    async call(tool) { seen.push({ tool, documentId }); return CREATED_OK; },
    async claims(cs) { return cs.map((c) => ({ name: c.name, verified: true })); },
    async recipeRef() { return { step_count: 1, steps: [] }; },
    async modelScope() { return OWN_MODEL_ONLY; },
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
  // AND ITS OWN MODEL. A distinct document was never enough: `ActiveModel`
  // routes on `X-Roshera-Part-Id` (api-server/src/part_mgr.rs:264, 286-312),
  // so four episodes sharing one part id build into one shared `BRepModel`
  // however distinct their documents — measured live on 2026-08-13.
  const heldParts = pinnedPerSession.map((s) => s.partId);
  assert.equal(new Set(heldParts).size, 4,
    `four concurrent episodes must hold four distinct parts, got ${JSON.stringify(heldParts)}`);
  assert.ok(heldParts.every((p) => typeof p === "string" && p.length > 0),
    "and each must actually hold one — an absent pin falls back to the global model");
});

check("the tally names every outcome, including the zeros", async () => {
  const { tally } = await runBatch({
    tasks: [task], policyFor: () => scriptedPolicy([]), seeds: [1],
    concurrency: 1, baseUrl, authHeader: {}, outDir: dir, kernelSha: "abc",
    spawn: fakeSpawn,
  });
  // ITERATE THE TAXONOMY, never a hand-copied list. The copy here named five
  // of the six: `INVALID_ACTION` was missing, so deleting it from
  // `runner.mjs`'s tally would have kept this check green — and a batch that
  // silently stopped counting the outcome for "the policy left its own action
  // space" would look like a batch in which that never happened.
  assert.ok(OUTCOMES.length > 0, "the taxonomy is not empty");
  for (const k of OUTCOMES) {
    assert.ok(k in tally, `${k} must appear even at zero — an absent key reads as "not measured"`);
  }
  assert.deepEqual(
    Object.keys(tally).sort(),
    [...OUTCOMES].sort(),
    "and the tally names EXACTLY the taxonomy — an extra key is an outcome the " +
      "closed taxonomy does not define",
  );
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
      async modelScope() { return OWN_MODEL_ONLY; },
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
      async modelScope() { return OWN_MODEL_ONLY; },
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

check("a throwing policy FACTORY fails ONE episode, not the batch", async () => {
  // `policyFor(...)` used to be evaluated while building `runEpisode`'s
  // argument object — outside the `.catch` attached to its promise — so a
  // factory that threw rejected the worker, then `Promise.all`, then the whole
  // batch. Every other failure mode in this system is a named per-episode
  // outcome; this one took every sibling episode down with it, including the
  // ones that had already finished.
  let made = 0;
  const throwingFactory = () => {
    made += 1;
    if (made === 2) throw new Error("no policy registered for this task");
    return scriptedPolicy([{ tool: "create_cylinder", args: {} }]);
  };
  const { results, tally } = await runBatch({
    tasks: [task, task, task], policyFor: throwingFactory, seeds: [1, 2, 3],
    // Serial, so the throw lands on the SECOND episode and the first is
    // already recorded — the case where a batch-wide rejection destroys work
    // that had completed.
    concurrency: 1, baseUrl, authHeader: {}, outDir: dir, kernelSha: "abc",
    spawn: fakeSpawn,
  });
  assert.equal(results.length, 3, "all three episodes reported — the batch did not die");
  assert.equal(tally.SETUP_FAILED, 1, "the failing one is SETUP_FAILED: no session ever existed");
  assert.equal(tally.COMPLETED, 2, "and its siblings kept their outcomes");
  const failed = results.find((r) => r.outcome === "SETUP_FAILED");
  assert.match(
    failed.error,
    /policy/i,
    `the reason must name the policy factory, not a generic setup failure — got ${JSON.stringify(failed.error)}`,
  );
  assert.ok(
    failed.error.includes("no policy registered for this task"),
    "and carry the factory's own message",
  );
  assert.equal(failed.documentId, null, "no document was created, so none is claimed");
  // Every episode has a record: the trajectory is what a batch is read from
  // afterwards, and a result pointing at a file that was never written is a
  // failure nobody can diagnose without re-running the batch.
  const { terminal } = readTrajectory(failed.trajectoryPath);
  assert.equal(terminal.outcome, "SETUP_FAILED");
  assert.ok(terminal.error.includes("no policy registered for this task"));
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

check("a conflicting kernel claim refuses the WHOLE BATCH before any episode runs", async () => {
  // The server states one build; the operator claims another. `runBatch`
  // must ask ONCE, up front, and a genuine disagreement is a batch-level
  // refusal — never a per-episode outcome, and never after work has begun.
  healthReply = { build: { sha: "aaa", dirty: false } };
  const spawned = [];
  const spySpawn = async ({ documentId }) => {
    spawned.push(documentId);
    return {
      async call() { return CREATED_OK; },
      async claims(cs) { return cs.map((c) => ({ name: c.name, verified: true })); },
      async recipeRef() { return { step_count: 1, steps: [] }; },
      async modelScope() { return OWN_MODEL_ONLY; },
      async close() {},
    };
  };
  try {
    // Three tasks at concurrency 3: if the resolve happened anywhere other
    // than before the worker pool starts, all three workers would be free to
    // spawn immediately and `spawned` would be non-empty by the time the
    // rejection is observed.
    await assert.rejects(
      () => runBatch({
        tasks: [task, task, task],
        policyFor: () => scriptedPolicy([{ tool: "create_cylinder", args: {} }]),
        seeds: [1, 2, 3], concurrency: 3, baseUrl, authHeader: {}, outDir: dir,
        kernelSha: "bbb", spawn: spySpawn,
      }),
      (e) => e instanceof KernelIdentityConflict && /aaa/.test(e.message) && /bbb/.test(e.message),
      "the rejection must be the named conflict, carrying both builds",
    );
  } finally {
    healthReply = {};
  }
  // THE PROPERTY THAT MATTERS MOST: not that the call rejected, but that it
  // rejected BEFORE any episode ran. A refusal that fires after eight rows
  // were already written is a complaint, not a refusal.
  assert.equal(spawned.length, 0,
    "no session may have been spawned — the refusal happens before any worker starts, not after");
});

check("a real batch's provenance block is ATTRIBUTABLE — runBatch's OWN DEFAULTS resolve cleanly", async () => {
  // The one check that would have caught a `new URL(...).pathname` bug: this
  // machine's account directory is `C:\Users\Varun Sharma\`, and a path built
  // from a percent-encoded, non-decoded URL does not exist on disk. Neither
  // `mcpEntry` nor `harnessRoot` is passed here — deliberately, so what is
  // under test is `runBatch`'s OWN default resolution (`defaultMcpEntry()`
  // and `DEFAULT_HARNESS_ROOT`), the exact code path every real batch on
  // this machine actually runs. A test that injected its own already-correct
  // paths here would stay green even if the defaults regressed to
  // `.pathname` — which is precisely how that class of bug hides. Every
  // identity in the block should resolve and `attributable` should read
  // true; a false negative here means every real batch on this machine
  // silently produced unattributable rows.
  healthReply = { build: { sha: "cafefeed", dirty: false } };
  try {
    const { results } = await runBatch({
      tasks: [task], policyFor: () => scriptedPolicy([{ tool: "create_cylinder", args: { radius: 25 } }]),
      seeds: [1], concurrency: 1, baseUrl, authHeader: {}, outDir: dir,
      spawn: fakeSpawn,
    });
    const { header } = readTrajectory(results[0].trajectoryPath);
    assert.equal(header.provenance.kernel.sha, "cafefeed");
    assert.equal(header.provenance.kernel.reported_by, "server");
    assert.ok("dist_digest" in header.provenance.mcp,
      "a readable dist/index.js must produce a real digest, not an absence");
    assert.equal(header.provenance.attributable, true,
      `expected an attributable block with real paths, got ${JSON.stringify(header.provenance)}`);
  } finally {
    healthReply = {};
  }
});

check("a policy whose describe() throws fails its OWN episode, not the whole batch", async () => {
  // `buildProvenance` calls `policy.describe()` synchronously, and `policy`
  // is THIRD-PARTY CODE exactly like the policy factory above — a `describe`
  // that throws must not take the batch down, and must not discard a sibling
  // episode that already finished. Serial (concurrency 1) so episode 1
  // completes and is already in `results` before episode 2's factory hands
  // back the throwing policy, and episode 3 proves the worker kept going
  // afterward rather than stopping at the failure.
  let made = 0;
  const factory = () => {
    made += 1;
    if (made === 2) {
      return {
        async act() { return { done: true }; },
        tokensUsed() { return 0; },
        describe() { throw new Error("describe() exploded — a policy that cannot even describe itself"); },
      };
    }
    return scriptedPolicy([{ tool: "create_cylinder", args: {} }]);
  };
  const { results, tally } = await runBatch({
    tasks: [task, task, task], policyFor: factory, seeds: [1, 2, 3],
    concurrency: 1, baseUrl, authHeader: {}, outDir: dir, kernelSha: "abc",
    spawn: fakeSpawn,
  });
  assert.equal(results.length, 3,
    "all three episodes reported — a throwing describe() must not kill the batch");
  assert.equal(tally.SETUP_FAILED, 1, "the failing one is SETUP_FAILED: no episode ever began");
  assert.equal(tally.COMPLETED, 2,
    "and BOTH siblings kept their outcomes — including the one that had already finished");
  const failed = results.find((r) => r.outcome === "SETUP_FAILED");
  assert.ok(
    failed.error.includes("describe() exploded"),
    `the reason must carry describe()'s own message, got ${JSON.stringify(failed.error)}`,
  );
  const { terminal } = readTrajectory(failed.trajectoryPath);
  assert.equal(terminal.outcome, "SETUP_FAILED");
});

check("when the server cannot state its build, the batch still RUNS and every header says so honestly", async () => {
  // An ABSENCE is not a CONFLICT: with no `claimed` sha and a server that
  // states nothing (the default stub reply — no `build` key), the batch must
  // still run to completion, and the written provenance must carry the
  // absence rather than a fabricated identity.
  healthReply = {};
  const { results, tally } = await runBatch({
    tasks: [task], policyFor: () => scriptedPolicy([{ tool: "create_cylinder", args: {} }]),
    seeds: [1], concurrency: 1, baseUrl, authHeader: {}, outDir: dir,
    kernelSha: undefined, spawn: fakeSpawn,
  });
  assert.equal(tally.COMPLETED, 1,
    "an absent kernel identity must not stop the episode from running");
  const { header } = readTrajectory(results[0].trajectoryPath);
  assert.ok(!("sha" in header.provenance.kernel),
    "no sha may be invented when the server stated nothing");
  assert.match(header.provenance.kernel.absent, /build/i);
  assert.equal(header.provenance.attributable, false,
    "an absent kernel identity must make the whole block unattributable, not merely omit the sha");
});

check("batch-invariant provenance (mcp digest, harness identity) is IDENTICAL across episodes in the same batch", async () => {
  // Not proof of a single computation by itself, but a real regression catch:
  // if the per-episode call ever again resolved `mcp`/`harness` independently
  // (the exact defect just fixed), a machine where the tree changes between
  // two episodes' reads would show it here as a mismatch.
  healthReply = { build: { sha: "cafefeed", dirty: false } };
  try {
    const { results } = await runBatch({
      tasks: [task, task], policyFor: () => scriptedPolicy([{ tool: "create_cylinder", args: {} }]),
      seeds: [1, 2], concurrency: 2, baseUrl, authHeader: {}, outDir: dir,
      spawn: fakeSpawn,
    });
    assert.equal(results.length, 2);
    const provs = results.map((r) => readTrajectory(r.trajectoryPath).header.provenance);
    assert.deepEqual(provs[0].mcp, provs[1].mcp,
      "two episodes in the same batch must read the identical mcp digest");
    assert.deepEqual(provs[0].harness, provs[1].harness,
      "and the identical harness identity");
  } finally {
    healthReply = {};
  }
});

for (const [name, fn] of checks) { await fn(); process.stdout.write(`  ok - ${name}\n`); }
stub.close();
rmSync(dir, { recursive: true, force: true });
process.stdout.write(`\nrunner: ${checks.length} checks passed\n`);
