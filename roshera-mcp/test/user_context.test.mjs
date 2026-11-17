/**
 * `user_context`: the agent's read of what the person hovers and has
 * selected in the open app. Proves the production call site: the tool is on
 * the surface, is read-only for the gates, is benched "core", and calling
 * it issues `GET /api/agent/user-context` as an agent (so the server can
 * tell the app the agent looked) and hands the answer back unaltered.
 *
 *     npm run test:gates:build && node test/user_context.test.mjs
 */
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));

const ANSWER = {
  state: "live",
  document_id: "doc-1",
  seq: 4,
  age_ms: 120,
  tool: "select",
  pick_filter: "face",
  hover: {
    kind: "face",
    object_id: "aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa",
    id: 12,
    solid_id: 3,
    part_name: "Flange",
    persistent_id: "0123456789abcdef0123456789abcdef",
    surface_type: "cylinder",
    resolved: true,
  },
  selection: [],
  feature_anchor: 7,
};

/** @type {{ method: string, url: string, agent: string | null }[]} */
const seen = [];
const stub = http.createServer((req, res) => {
  seen.push({ method: req.method ?? "", url: req.url ?? "", agent: req.headers["x-roshera-agent"] ?? null });
  res.writeHead(200, { "Content-Type": "application/json" });
  res.end(JSON.stringify(req.url === "/api/agent/user-context" ? ANSWER : { ok: true }));
});
stub.listen(0, "127.0.0.1");
await once(stub, "listening");
process.env.ROSHERA_URL = `http://127.0.0.1:${stub.address().port}`;

try {
  const { buildTable } = await import(pathToFileURL(join(HERE, ".build", "surface.js")).href);
  const { metaFor } = await import(pathToFileURL(join(HERE, ".build", "registry.js")).href);

  const table = buildTable();
  assert.ok(table.has("user_context"), "user_context is registered on the surface");
  assert.equal(metaFor("user_context").bench, "core", "it is on the core bench, not the meta fallback");

  const entry = table.get("user_context");
  const result = await entry.handler({});
  const calls = seen.filter((s) => s.url === "/api/agent/user-context");
  assert.equal(calls.length, 1, "one call to the user-context route");
  assert.equal(calls[0].method, "GET", "a read");
  assert.ok(calls[0].agent, "sent as an agent, so the app is told the agent looked");

  assert.notEqual(result.isError, true, "a 200 is not an error");
  const text = result.content.find((c) => c.type === "text")?.text ?? "";
  const body = JSON.parse(text);
  assert.equal(body.state, "live");
  assert.equal(body.hover.persistent_id, ANSWER.hover.persistent_id, "the persistent id reaches the agent");
  assert.equal(body.hover.id, 12);
  assert.equal(body.feature_anchor, 7);

  console.log("user_context.test.mjs: all assertions passed");
} finally {
  stub.close();
}
