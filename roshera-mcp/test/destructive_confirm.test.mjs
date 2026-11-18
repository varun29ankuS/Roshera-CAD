/**
 * Destructive tools need an explicit `confirm: true` (MCP_AUDIT.md §4).
 *
 * `clear_parts` deletes every part, `clear_timeline` rewrites the ledger to
 * zero events, `blackboard_clear` wipes the notebook. Each used to run on a
 * bare call — the cheapest call an agent can make was the most destructive.
 * Now each refuses typed (`confirmation_required`) unless the call says
 * `confirm: true`, and sends NOTHING to the server when it refuses.
 *
 *   (c1) clear_parts: refused without confirm, no request; runs with it;
 *   (c2) clear_timeline: the same;
 *   (c3) blackboard_clear: the same;
 *   (c4) the schema advertises `confirm` on all three (an agent can learn the
 *        rule from describe_tool, not only from a refusal);
 *   (c5) cad_program: allow_destructive alone is not confirmation — the op
 *        refuses and the program stops; with confirm in the op it runs.
 *
 *     npm run test:gates:build && node test/destructive_confirm.test.mjs
 */
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL, fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));

let seen = [];
const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  let raw = "";
  req.on("data", (c) => (raw += c));
  req.on("end", () => {
    seen.push({ method: req.method, url });
    const send = (obj) => {
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify(obj));
    };
    if (req.method === "DELETE" && url === "/api/agent/parts") return send({ success: true, deleted: 3 });
    if (req.method === "POST" && url === "/api/timeline/clear") {
      return send({ events_removed: 12, model_reconciled: true, branch_id: "main" });
    }
    if (req.method === "POST" && url === "/api/blackboard/clear") return send({ success: true });
    if (req.method === "POST" && url === "/api/timeline/checkpoint") return send({ id: "cp-1" });
    if (req.method === "POST" && url === "/api/blackboard/entries") return send({ id: "bb-1" });
    if (req.method === "GET" && url === "/api/agent/parts") return send([]);
    res.writeHead(404, { "Content-Type": "application/json" });
    res.end("{}");
  });
});
stub.listen(0, "127.0.0.1");
await once(stub, "listening");
process.env.ROSHERA_URL = `http://127.0.0.1:${stub.address().port}`;

const { buildTable } = await import(pathToFileURL(join(HERE, ".build", "surface.js")).href);
const { toolJsonSchema } = await import(pathToFileURL(join(HERE, ".build", "registry.js")).href);
const table = buildTable();
const call = (name, args = {}) => table.get(name).handler(args, {});

let passed = 0;
const check = async (label, fn) => {
  seen = [];
  await fn();
  passed += 1;
  console.log(`  ok - ${label}`);
};

// clear_parts is intent-gated like every solid-mutating verb.
await call("timeline_checkpoint", { name: "destructive-confirm proof — reset the scene before the fixture", branch: "main" });

const cases = [
  ["clear_parts", {}, "DELETE", "/api/agent/parts"],
  ["clear_timeline", { branch_id: "main" }, "POST", "/api/timeline/clear"],
  ["blackboard_clear", {}, "POST", "/api/blackboard/clear"],
];

for (const [i, [tool, args, method, url]] of cases.entries()) {
  await check(`(c${i + 1}) ${tool}: refused without confirm, runs with it`, async () => {
    const refused = await call(tool, args);
    assert.equal(refused.isError, true, `${tool} without confirm must refuse`);
    assert.equal(refused.structuredContent?.error_code, "confirmation_required");
    assert.match(refused.structuredContent?.reason ?? "", /confirm: true/);
    assert.ok(!seen.some((s) => s.url === url), `${tool} sent ${method} ${url} without confirmation`);

    const done = await call(tool, { ...args, confirm: true });
    assert.notEqual(done.isError, true, done.content?.[0]?.text);
    assert.ok(seen.some((s) => s.method === method && s.url === url), `${tool} with confirm must run`);
  });
}

await check("(c4) the schema advertises confirm", async () => {
  for (const [tool] of cases) {
    const props = toolJsonSchema(table.get(tool)).properties ?? {};
    assert.equal(props.confirm?.type, "boolean", `${tool} must declare confirm`);
  }
});

await check("(c5) cad_program: allow_destructive is not confirmation", async () => {
  // clear_timeline above wiped the ledger the open intent lived in.
  await call("timeline_checkpoint", { name: "destructive-confirm proof — programmatic scene reset", branch: "main" });
  const stopped = await call("cad_program", { allow_destructive: true, ops: [{ tool: "clear_parts" }] });
  assert.equal(stopped.isError, true);
  assert.equal(stopped.structuredContent?.stopped_at, 0);
  assert.ok(!seen.some((s) => s.url === "/api/agent/parts" && s.method === "DELETE"));

  const ran = await call("cad_program", { allow_destructive: true, ops: [{ tool: "clear_parts", args: { confirm: true } }] });
  assert.notEqual(ran.isError, true, ran.content?.[0]?.text);
  assert.ok(seen.some((s) => s.url === "/api/agent/parts" && s.method === "DELETE"));
});

stub.close();
console.log(`\ndestructive_confirm: ${passed} checks passed`);
