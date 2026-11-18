/**
 * Failures that look like successes — `timeline_undo`, `timeline_redo` and a
 * conflicted `timeline_merge` (MCP_AUDIT.md 1.4, 1.5).
 *
 * The routes answer HTTP 200 with `success: false` when nothing changed:
 * nothing to undo / redo, an unknown session, a refused replay
 * (`handlers/timeline.rs::undo_operation` / `redo_operation`), and a merge
 * that stopped on conflicts (`branches.rs::MergeView`). The tools used to
 * wrap any 200 as an ordinary result, so the agent's client saw success,
 * `cad_program` kept going, and the refusal cache never learned of it.
 *
 *   (t1) an undo with nothing to undo is an ERROR result whose structured
 *        content carries the typed refusal (error_code + the server's body);
 *   (t2) the same for redo, and for a server-typed refusal code verbatim;
 *   (t3) a conflicted merge is an error carrying its conflict witnesses, and
 *        no rebuild certificate is fetched for a merge that did not land;
 *   (t4) a successful undo and merge stay successes (the fix is not
 *        "everything is an error");
 *   (t5) cad_program STOPS at a failed undo: the op after it never runs.
 *
 *     npm run test:gates:build && node test/timeline_failures.test.mjs
 */
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL, fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));

let seen = [];
/** What the next undo / redo / merge answers. */
let undoAnswer = null;
let redoAnswer = null;
let mergeAnswer = null;

const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  let raw = "";
  req.on("data", (c) => (raw += c));
  req.on("end", () => {
    seen.push({ method: req.method, url });
    const send = (status, payload) => {
      res.writeHead(status, { "Content-Type": "application/json" });
      res.end(JSON.stringify(payload));
    };
    if (url === "/api/timeline/undo") return send(200, undoAnswer);
    if (url === "/api/timeline/redo") return send(200, redoAnswer);
    if (/^\/api\/branches\/[^/]+\/merge$/.test(url)) return send(200, mergeAnswer);
    if (url.startsWith("/api/timeline/rebuild-certificate/")) return send(200, { sound: true });
    if (url === "/api/timeline/checkpoints") return send(200, []);
    send(404, {});
  });
});
stub.listen(0, "127.0.0.1");
await once(stub, "listening");
process.env.ROSHERA_URL = `http://127.0.0.1:${stub.address().port}`;

const { buildTable } = await import(pathToFileURL(join(HERE, ".build", "surface.js")).href);
const { resetSessionGates } = await import(pathToFileURL(join(HERE, ".build", "gates.js")).href);
const table = buildTable();
const call = (name, args = {}) => table.get(name).handler(args, {});

let passed = 0;
const check = async (label, fn) => {
  seen = [];
  resetSessionGates();
  await fn();
  passed += 1;
  console.log(`  ok - ${label}`);
};

await check("(t1) nothing to undo is an error carrying the typed refusal", async () => {
  undoAnswer = { success: false, message: "Nothing to undo - at beginning of timeline", can_undo: false };
  const r = await call("timeline_undo");
  assert.equal(r.isError, true, "a refused undo must not be success-shaped");
  assert.equal(r.structuredContent?.error_code, "nothing_to_undo");
  assert.equal(r.structuredContent?.reason, undoAnswer.message);
  assert.deepEqual(r.structuredContent?.refused, undoAnswer, "the server's body, verbatim");
});

await check("(t2) redo refusals, including a server-typed code, are errors", async () => {
  redoAnswer = { success: false, message: "Nothing to redo - at end of timeline" };
  let r = await call("timeline_redo");
  assert.equal(r.isError, true);
  assert.equal(r.structuredContent?.error_code, "nothing_to_redo");

  // A refused call is cached (an identical re-issue is answered from the
  // refusal cache without a request); start the second case clean.
  resetSessionGates();
  redoAnswer = { success: false, message: "replay refused", error_code: "branch_replay_refused", hint: "h" };
  r = await call("timeline_redo");
  assert.equal(r.isError, true);
  assert.equal(r.structuredContent?.error_code, "branch_replay_refused", "a server code is kept verbatim");
  assert.equal(r.structuredContent?.hint, "h");
});

await check("(t3) a conflicted merge is an error with its witnesses, no certificate fetched", async () => {
  mergeAnswer = {
    success: false,
    merged_into: "main",
    strategy: "three-way",
    events_merged: 0,
    conflicts: [{ subject: "part 7", verdict: "both_modified" }],
    statistics: {},
    recording_branch: "main",
  };
  const r = await call("timeline_merge", { source: "b1", target: "main", strategy: "three-way", certify: true });
  assert.equal(r.isError, true, "nothing merged: the client must see a failure");
  assert.equal(r.structuredContent?.error_code, "branch_merge_conflict");
  assert.deepEqual(r.structuredContent?.refused?.conflicts, mergeAnswer.conflicts);
  assert.ok(
    !seen.some((s) => s.url.startsWith("/api/timeline/rebuild-certificate/")),
    "no certificate is fetched for a merge that did not land",
  );
});

await check("(t4) successes stay successes", async () => {
  undoAnswer = { success: true, message: "Undo operation completed successfully", event_id: "e1" };
  const u = await call("timeline_undo");
  assert.notEqual(u.isError, true, u.content[0].text);
  mergeAnswer = { success: true, merged_into: "main", strategy: "three-way", events_merged: 2, conflicts: [] };
  const m = await call("timeline_merge", { source: "b1", target: "main", strategy: "three-way", certify: true });
  assert.notEqual(m.isError, true, m.content[0].text);
  assert.deepEqual(JSON.parse(m.content[0].text).certificate, { sound: true });
});

await check("(t5) cad_program stops at a failed undo", async () => {
  undoAnswer = { success: false, message: "Nothing to undo - at beginning of timeline", can_undo: false };
  const r = await call("cad_program", {
    ops: [{ tool: "timeline_undo" }, { tool: "timeline_checkpoints" }],
  });
  assert.equal(r.isError, true, "a stopped program is a failure");
  const ledger = r.structuredContent;
  assert.equal(ledger.stopped_at, 0);
  assert.equal(ledger.completed, 0);
  assert.ok(!seen.some((s) => s.url === "/api/timeline/checkpoints"), "the op after the stop never ran");
});

stub.close();
console.log(`\ntimeline_failures: ${passed} checks passed`);
