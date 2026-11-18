/**
 * Typed errors from the routes agent tools reach (MCP_AUDIT.md §2).
 *
 * These api-server handlers used to answer a bare status with an EMPTY body
 * (timeline history / scrub / rebuild-certificate / undo / redo / clear, the
 * two Kernel Lens routes, GD&T report, face query, revolve profile). Since
 * 2026-11-18 each answers the error catalogue's body (`error_code`, `error`,
 * `hint`, `details`). This proves the MCP side carries that body to the agent
 * as structured content for every tool that reaches one of them, against a
 * stub that answers exactly the catalogue shape (`error_catalog.rs`
 * `ApiError`'s Serialize impl, pinned below).
 *
 *   (r1) each tool's refusal reaches structuredContent typed;
 *   (r2) the timeline tools' typed refusals (refusalOrFail) lift the
 *        catalogue's error_code / hint to the top level instead of burying
 *        them in `refused`;
 *   (r3) the stub's body shape is the catalogue's own.
 *
 *     npm run test:gates:build && node test/typed_route_errors.test.mjs
 */
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { readFileSync } from "node:fs";
import { pathToFileURL, fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));

const body = (status, code, error) => ({
  status,
  payload: { error_code: code, error, retryable: false, hint: `hint for ${code}`, details: { route: "x" }, success: false },
});
const ROUTES = [
  [/^\/api\/timeline\/history\//, body(404, "branch_not_found", "branch 'b9' has no history on this timeline")],
  [/^\/api\/timeline\/scrub\//, body(404, "branch_not_found", "branch 'b9' has no history on this timeline")],
  [/^\/api\/timeline\/rebuild-certificate\//, body(404, "branch_not_found", "branch 'b9' has no history on this timeline")],
  [/^\/api\/lens\//, body(404, "solid_not_found", "part 9 does not name a live solid")],
  [/^\/api\/agent\/parts\/\d+\/gdt$/, body(404, "solid_not_found", "solid 9 not found")],
  [/^\/api\/agent\/parts\/\d+\/profile$/, body(404, "solid_not_found", "part 9 has no revolve profile")],
  [/^\/api\/agent\/faces\//, body(404, "solid_not_found", "face 9 does not exist")],
  [/^\/api\/timeline\/mould$/, body(409, "branch_replay_refused", "the edit breaks feature 4")],
  [/^\/api\/timeline\/checkpoint$/, { status: 200, payload: { id: "cp-1" } }],
  [/^\/api\/blackboard\/entries$/, { status: 200, payload: { id: "bb-1" } }],
];

const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  req.resume();
  req.on("end", () => {
    const hit = ROUTES.find(([re]) => re.test(url));
    res.writeHead(hit ? hit[1].status : 404, { "Content-Type": "application/json" });
    res.end(JSON.stringify(hit ? hit[1].payload : {}));
  });
});
stub.listen(0, "127.0.0.1");
await once(stub, "listening");
process.env.ROSHERA_URL = `http://127.0.0.1:${stub.address().port}`;

const { buildTable } = await import(pathToFileURL(join(HERE, ".build", "surface.js")).href);
const { validateOp } = await import(pathToFileURL(join(HERE, ".build", "metatools.js")).href);
const { resetSessionGates } = await import(pathToFileURL(join(HERE, ".build", "gates.js")).href);
const table = buildTable();

/** Minimal valid args from the tool's own schema (required fields only). */
const argsFor = (name, extra = {}) => {
  const schema = table.get(name).schema;
  const shape = schema.shape ?? {};
  const out = {};
  for (const [k, v] of Object.entries(shape)) {
    if (v.isOptional()) continue;
    const t = v._def.typeName;
    out[k] = t === "ZodNumber" ? 9 : t === "ZodString" ? "b9" : t === "ZodBoolean" ? false : null;
  }
  return { ...out, ...extra };
};

let passed = 0;
const check = async (label, fn) => {
  resetSessionGates();
  await fn();
  passed += 1;
  console.log(`  ok - ${label}`);
};

const TOOLS = [
  ["timeline_history", {}, "branch_not_found"],
  ["timeline_scrub", { sequence: 1 }, "branch_not_found"],
  ["rebuild_certificate", {}, "branch_not_found"],
  ["lens_explain", {}, "solid_not_found"],
  ["lens_why_not_sound", {}, "solid_not_found"],
  ["gdt_report", {}, "solid_not_found"],
  ["get_revolve_profile", {}, "solid_not_found"],
  ["get_face", {}, "solid_not_found"],
];

await check("(r1) each tool's refusal reaches structuredContent typed", async () => {
  const untyped = [];
  for (const [tool, extra, code] of TOOLS) {
    assert.ok(table.has(tool), `${tool} is not registered`);
    const { parsed } = await validateOp(table, tool, argsFor(tool, extra));
    const r = await table.get(tool).handler(parsed, {});
    if (r.isError !== true || r.structuredContent?.error_code !== code) {
      untyped.push(`${tool}: isError=${r.isError} structured=${JSON.stringify(r.structuredContent)} text=${r.content?.[0]?.text?.slice(0, 120)}`);
    }
  }
  assert.deepEqual(untyped, [], untyped.join("\n"));
});

await check("(r2) timeline refusals lift error_code and hint to the top level", async () => {
  // timeline_mould changes the model: it is intent-gated like every builder.
  await table.get("timeline_checkpoint").handler({ name: "typed refusal proof — widen the bore to 5 mm", branch: "main" }, {});
  const { parsed } = await validateOp(table, "timeline_mould", { value: 5, target_event_id: "e1", parameter: "radius" });
  const r = await table.get("timeline_mould").handler(parsed, {});
  assert.equal(r.isError, true);
  assert.equal(r.structuredContent?.error_code, "branch_replay_refused");
  assert.equal(r.structuredContent?.hint, "hint for branch_replay_refused");
  assert.equal(r.structuredContent?.refused?.error, "the edit breaks feature 4", "the server body stays whole");
});

await check("(r3) the stub's body is the catalogue's own shape", async () => {
  const src = readFileSync(join(HERE, "..", "..", "roshera-backend", "api-server", "src", "error_catalog.rs"), "utf8");
  const m = src.match(/pub struct ApiError \{([\s\S]*?)\n\}/);
  assert.ok(m, "ApiError is no longer declared in error_catalog.rs");
  assert.match(m[1], /#\[serde\(rename = "error_code"\)\]\s*pub code: ErrorCode/);
  for (const f of ["error", "retryable", "hint", "details", "success"]) {
    assert.match(m[1], new RegExp(`pub ${f}:`), `ApiError.${f}`);
  }
});

stub.close();
console.log(`\ntyped_route_errors: ${passed} checks passed`);
