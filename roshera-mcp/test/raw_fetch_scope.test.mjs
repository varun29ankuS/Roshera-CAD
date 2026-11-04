/**
 * Raw-fetch scope proof — every backend call carries the session's scope.
 *
 * `api()` (core.ts) sends the intent, document and part headers on every
 * call. Seven tool paths bypass `api()` with a raw `fetch` (so they can read a
 * refusal body) and, before this test, sent only auth: gdt_datum, gdt_fcf,
 * label_create, label_resolve, select_face, select_edge, and saveBinary. In a
 * part-pinned session those calls silently resolved against the backend's
 * legacy global model (`ActiveModel` falls back when `X-Roshera-Part-Id` is
 * absent), and the GD&T / label mutations lost their intent attribution.
 *
 * Second defect: label_create / label_resolve / select_face / select_edge
 * returned `ok(...)` on ANY status, so a 400/404/409 refusal came back
 * success-shaped. A refusal must reach the agent as an error.
 *
 *   npx tsc -p tsconfig.json --outDir test/.build && node test/raw_fetch_scope.test.mjs
 */
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL, fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { tmpdir } from "node:os";

const HERE = dirname(fileURLToPath(import.meta.url));
const PART = "dddddddd-4444-4444-8444-444444444444";
const DOC = "doc-raw-fetch-scope";

/** url → headers of the LAST request to it. */
const seen = new Map();
/** url → [status, body] overrides. */
const answers = new Map();

const stub = http.createServer((req, res) => {
  const url = (req.url ?? "").split("?")[0];
  seen.set(`${req.method} ${url}`, req.headers);
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    const [status, obj] = answers.get(`${req.method} ${url}`) ?? [200, {}];
    if (obj instanceof Buffer) {
      res.writeHead(status, { "Content-Type": "application/octet-stream" });
      return res.end(obj);
    }
    res.writeHead(status, { "Content-Type": "application/json" });
    res.end(JSON.stringify(obj));
  });
});
stub.listen(0, "127.0.0.1");
await once(stub, "listening");
const port = stub.address().port;

process.env.ROSHERA_URL = `http://127.0.0.1:${port}`;
process.env.ROSHERA_PART = PART;
process.env.ROSHERA_DOCUMENT = DOC;

const core = await import(pathToFileURL(join(HERE, ".build", "core.js")).href);
await core.bindSessionDocument();
const { buildTable } = await import(pathToFileURL(join(HERE, ".build", "surface.js")).href);
const table = buildTable();
const call = (name, args) => table.get(name).handler(args, {});

answers.set("POST /api/timeline/checkpoint", [200, { id: "cp-1" }]);
await call("timeline_checkpoint", { name: "datum A on the mounting face of the bracket" });

const checks = [];
const check = (name, fn) => checks.push([name, fn]);

function assertScoped(key) {
  const h = seen.get(key);
  assert.ok(h, `${key} reached the stub`);
  assert.equal(h["x-roshera-part-id"], PART, `${key} carried the part pin`);
  assert.equal(h["x-roshera-document"], DOC, `${key} carried the document pin`);
  assert.ok(h["x-roshera-intent"], `${key} carried the open intent`);
}

check("gdt_datum carries part, document and intent", async () => {
  answers.set("POST /api/agent/parts/7/datums", [200, { label: "A" }]);
  await call("gdt_datum", { part_id: 7, label: "A", face_id: 3 });
  assertScoped("POST /api/agent/parts/7/datums");
});

check("gdt_fcf carries part, document and intent", async () => {
  answers.set("POST /api/agent/parts/7/fcf", [200, { verdict: null }]);
  await call("gdt_fcf", {
    part_id: 7,
    characteristic: "flatness",
    tolerance_mm: 0.05,
    target_face: 3,
  });
  assertScoped("POST /api/agent/parts/7/fcf");
});

check("label_create carries scope, and a 400 refusal is an error", async () => {
  answers.set("POST /api/agent/parts/7/labels", [400, { error: "missing_name" }]);
  const r = await call("label_create", { part_id: 7, name: "x", kind: "face", entity_id: 3 });
  assertScoped("POST /api/agent/parts/7/labels");
  assert.equal(r.isError, true, `a 400 must not come back success-shaped: ${JSON.stringify(r)}`);
});

check("label_resolve carries scope, and a 404 refusal is an error", async () => {
  answers.set("GET /api/agent/parts/7/labels/throat/resolve", [404, { error_code: "not_found" }]);
  const r = await call("label_resolve", { part_id: 7, name: "throat" });
  assertScoped("GET /api/agent/parts/7/labels/throat/resolve");
  assert.equal(r.isError, true, `a 404 must not come back success-shaped: ${JSON.stringify(r)}`);
});

check("select_face carries scope; an ambiguous 409 is an error that still lists candidates", async () => {
  answers.set("POST /api/agent/parts/7/select-face", [409, { resolved: false, candidates: [4, 5] }]);
  const r = await call("select_face", { part_id: 7, kind: "planar", extremal: "none", angle_tol_deg: 12 });
  assertScoped("POST /api/agent/parts/7/select-face");
  assert.equal(r.isError, true, `an ambiguous selection is a refusal: ${JSON.stringify(r)}`);
  assert.match(r.content[0].text, /candidates/, "the candidate list survives in the refusal");
});

check("select_edge carries scope; a resolved 200 is a success", async () => {
  answers.set("POST /api/agent/parts/7/select-edge", [200, { resolved: true, edge_id: 9 }]);
  const r = await call("select_edge", {
    part_id: 7,
    curve_kind: "any",
    blend: "any",
    convexity: "any",
    extremal: "none",
    angle_tol_deg: 12,
  });
  assertScoped("POST /api/agent/parts/7/select-edge");
  assert.notEqual(r.isError, true, "a resolved selection is a success");
});

check("every call carries one stable agent-session id, unique to this process", async () => {
  // The server keeps gate-6 state (built-but-unverified work) per agent
  // session; without this id every MCP process would share one tally.
  const ids = new Set(
    [...seen.values()].map((h) => h["x-roshera-agent-session"]).filter(Boolean),
  );
  assert.equal(ids.size, 1, `one session id across all calls, got ${[...ids]}`);
  const [id] = ids;
  assert.match(id, /^[0-9a-f-]{36}$/, `a UUID, got ${id}`);
  const other = await import(
    `${pathToFileURL(join(HERE, ".build", "core.js")).href}?second=${Date.now()}`
  );
  assert.notEqual(
    other.scopeHeaders()["X-Roshera-Agent-Session"],
    id,
    "a second MCP process must not share this one's session",
  );
});

check("saveBinary carries part and document", async () => {
  answers.set("GET /api/export/probe", [200, Buffer.from("solid x\nendsolid x\n")]);
  await core.saveBinary("/api/export/probe", join(tmpdir(), `roshera_probe_${Date.now()}.stl`));
  const h = seen.get("GET /api/export/probe");
  assert.equal(h["x-roshera-part-id"], PART, "saveBinary carried the part pin");
  assert.equal(h["x-roshera-document"], DOC, "saveBinary carried the document pin");
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
process.stdout.write(`\nraw_fetch_scope: ${checks.length - failed}/${checks.length} checks passed\n`);
if (failed > 0) process.exitCode = 1;
