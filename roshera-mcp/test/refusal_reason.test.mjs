/**
 * Every typed refusal the timeline tools mint carries a top-level `reason`.
 *
 * The Blackboard draws a forwarded refusal as a `roshera:refusal` card whose
 * schema requires `reason` (roshera-app/src/lib/blackboard-cards.ts). The
 * mould / bind / delete routes answer 409/422 with a body that is NOT an
 * error-catalog body (`{ status: "MouldRejected", reason, kind, ... }` —
 * handlers/timeline.rs), and `refusalOrFail` used to wrap it as
 * `{ refused: <body> }` with nothing at the top level, so the agent's
 * verbatim forward rendered as "payload failed validation — reason:
 * Required". The reason is the server's own sentence, lifted verbatim:
 * body.reason, else body.error, else body.message, else the HTTP error text.
 *
 *     npm run test:gates:build && node test/refusal_reason.test.mjs
 */
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL, fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));

/** What the next mould answers: [status, body]. */
let mouldAnswer = null;

const stub = http.createServer((req, res) => {
  req.resume();
  req.on("end", () => {
    const send = (status, payload) => {
      res.writeHead(status, { "Content-Type": "application/json" });
      res.end(JSON.stringify(payload));
    };
    if (req.url === "/api/timeline/mould") return send(mouldAnswer[0], mouldAnswer[1]);
    // The intent gate needs an open feature before a mutating call.
    if (req.method === "POST" && req.url === "/api/timeline/checkpoint") {
      return send(200, { id: `cp-${Date.now()}`, name: "fillet R12 on the hub edge", branch: "main" });
    }
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
  resetSessionGates();
  await fn();
  passed += 1;
  console.log(`  ok - ${label}`);
};

const mould = async () => {
  const cp = await call("timeline_checkpoint", { name: "fillet R12 on the hub edge", branch: "main" });
  assert.notEqual(cp.isError, true, `checkpoint must open: ${cp.content?.[0]?.text}`);
  const r = await call("timeline_mould", { value: 12, target_event_id: "e1", parameter: "radius", branch: "main" });
  assert.notEqual(r.structuredContent?.gate ?? JSON.parse(r.content[0].text).gate, "intent", "reached the route");
  return r;
};

await check("(r1) a MouldRejected body's reason is lifted to the top level, verbatim", async () => {
  const body = {
    status: "MouldRejected",
    reason: "the edit produces an unsound model",
    kind: "BrokenDownstream",
    target_sequence: 4,
    parameter: "radius",
  };
  mouldAnswer = [409, body];
  const r = await mould();
  assert.equal(r.isError, true);
  assert.equal(r.structuredContent?.reason, body.reason);
  assert.deepEqual(r.structuredContent?.refused, body, "the server's body, verbatim");
  assert.equal(JSON.parse(r.content[0].text).reason, body.reason, "the text the agent reads carries it too");
});

await check("(r2) a body with only `message` lends that as the reason", async () => {
  mouldAnswer = [422, { status: "MouldRejected", message: "no such event" }];
  const r = await mould();
  assert.equal(r.structuredContent?.reason, "no such event");
});

await check("(r3) a body with no words of its own falls back to the HTTP error text", async () => {
  mouldAnswer = [409, { status: "MouldRejected", kind: "BrokenDownstream" }];
  const r = await mould();
  assert.equal(typeof r.structuredContent?.reason, "string");
  assert.ok(r.structuredContent.reason.length > 0, "never an empty reason");
});

await check("(r4) a catalog body keeps its error as the reason and its typed fields", async () => {
  mouldAnswer = [
    422,
    { success: false, error_code: "invalid_parameter", error: "radius must be positive", retryable: false, hint: "h" },
  ];
  const r = await mould();
  assert.equal(r.structuredContent?.reason, "radius must be positive");
  assert.equal(r.structuredContent?.error_code, "invalid_parameter");
  assert.equal(r.structuredContent?.hint, "h");
});

stub.close();
console.log(`refusal_reason: ${passed}/4 passed`);
