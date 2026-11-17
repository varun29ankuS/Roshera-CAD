/**
 * `create_sphere` honours its `name`.
 *
 * The defect (found 2026-11-17 by the MCP wire audit, same class as the
 * mate tool's `connector_id`): `create_sphere` declares a `name` parameter,
 * but its handler never read it, and the route it calls (`POST
 * /api/geometry`, `create_geometry`) takes no name at all — it names the
 * part "Sphere <id>". An agent that asked for "ball seat" got a part called
 * "Sphere 7" and no word that its argument was dropped. Every other
 * `create_*` tool sends `name` to a route that reads it.
 *
 * Driven through the REAL dispatch stack (ToolTable wrapper + gates + the
 * real handler + core.ts), against a stub answering as the server does:
 *
 *   (n1) a named sphere is renamed through the part-name route, by the
 *        uuid the create answered, and the result says the name it carries;
 *   (n2) an unnamed sphere makes no rename call;
 *   (n3) a rename the server refuses is REPORTED on a sphere that still
 *        exists: the result keeps the part and says the name was not
 *        applied, and why — never a silent "Sphere 7";
 *   (n4) the stub's premise is pinned to the Rust source: `create_geometry`
 *        still reads no `name`, and the part-name route still exists.
 *
 *     npm run test:gates:build && node test/create_sphere_name.test.mjs
 */
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { readFileSync } from "node:fs";
import { pathToFileURL, fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const UUID = "5a5a5a5a-0000-4000-8000-000000000007";

let seen = [];
let refuseRename = false;

const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  let raw = "";
  req.on("data", (c) => (raw += c));
  req.on("end", () => {
    const body = raw.length ? JSON.parse(raw) : undefined;
    seen.push({ method: req.method, url, body });
    const send = (status, payload) => {
      res.writeHead(status, { "Content-Type": "application/json" });
      res.end(JSON.stringify(payload));
    };
    if (req.method === "POST" && url === "/api/timeline/checkpoint") {
      return send(200, { id: "cp-1", name: body?.name });
    }
    if (req.method === "POST" && url === "/api/blackboard/entries") {
      return send(200, { id: "bb-1", text: body?.text, author: "agent" });
    }
    if (req.method === "GET" && url === "/api/document/units") return send(200, { unit: "mm" });
    if (req.method === "POST" && url === "/api/geometry") {
      // main.rs::create_geometry answer (the fields this tool reads).
      return send(200, {
        success: true,
        solid_id: 7,
        object: { id: UUID, name: "Sphere 7", objectType: "sphere" },
        perception: {
          sound: true, brep_valid: true, watertight: true, manifold: true, oriented: true,
          faces: 1, edges: 0, vertices: 0, volume: 4188.79, open_edges: 0, nonmanifold_edges: 0,
        },
      });
    }
    if (req.method === "POST" && url === `/api/parts/uuid/${UUID}/name`) {
      if (refuseRename) {
        return send(400, {
          success: false,
          error_code: "invalid_parameter",
          error: "name must not be empty or whitespace",
          retryable: false,
        });
      }
      // main.rs::rename_part_by_uuid answer.
      return send(200, { success: true, solid_id: 7, name: body.name });
    }
    send(404, {});
  });
});
stub.listen(0, "127.0.0.1");
await once(stub, "listening");
process.env.ROSHERA_URL = `http://127.0.0.1:${stub.address().port}`;
// Keep the best-effort ambient-perception fetches short: the stub answers
// 404 for them at once, this only bounds a hang.
process.env.ROSHERA_MCP_PERCEPTION_TIMEOUT_MS = "500";

const { buildTable } = await import(pathToFileURL(join(HERE, ".build", "surface.js")).href);
const table = buildTable();
const call = (name, args) => table.get(name).handler(args, {});
const jsonOf = (result) => JSON.parse(result.content[0].text);
const renames = () => seen.filter((s) => s.url.endsWith("/name"));

let passed = 0;
const check = async (label, fn) => {
  await fn();
  passed += 1;
  console.log(`  ok - ${label}`);
};

const cp = await call("timeline_checkpoint", {
  name: "ball seat for the check valve: one sphere, named",
  branch: "main",
});
assert.notEqual(cp.isError, true, JSON.stringify(cp));

await check("(n1) a named sphere is renamed by the uuid its create answered", async () => {
  seen = [];
  const result = await call("create_sphere", { radius: 10, name: "ball seat" });
  assert.notEqual(result.isError, true, result.content[0].text);
  assert.equal(renames().length, 1, "the name was never sent to the server");
  assert.equal(renames()[0].url, `/api/parts/uuid/${UUID}/name`);
  assert.deepEqual(renames()[0].body, { name: "ball seat" });
  const out = jsonOf(result);
  assert.equal(out.object_uuid, UUID);
  assert.equal(out.name, "ball seat", "the result states the name the part carries");
});

await check("(n2) an unnamed sphere makes no rename call", async () => {
  seen = [];
  const result = await call("create_sphere", { radius: 11 });
  assert.notEqual(result.isError, true, result.content[0].text);
  assert.equal(renames().length, 0);
  assert.equal("name_not_applied" in jsonOf(result), false);
});

await check("(n3) a refused rename is reported on a sphere that still exists", async () => {
  seen = [];
  refuseRename = true;
  const result = await call("create_sphere", { radius: 12, name: "seat" });
  refuseRename = false;
  assert.notEqual(result.isError, true, "the sphere WAS created; the call is not a failure");
  const out = jsonOf(result);
  assert.equal(out.object_uuid, UUID, "the part is still reported");
  assert.equal(out.part_id, 7);
  assert.match(out.name_not_applied, /must not be empty/, "the reason is the server's own");
  assert.equal("name" in out, false, "no name is claimed that the part does not carry");
});

await check("(n4) the premise holds in the server source", async () => {
  const main = readFileSync(
    join(HERE, "..", "..", "roshera-backend", "api-server", "src", "main.rs"),
    "utf8",
  );
  const start = main.indexOf("async fn create_geometry(");
  assert.ok(start > 0, "create_geometry is no longer in main.rs");
  const end = main.indexOf("\nasync fn ", start + 10);
  const fn = main.slice(start, end);
  assert.equal(
    /\.get\("name"\)/.test(fn), false,
    "create_geometry now reads `name`: send it there and drop the rename call",
  );
  assert.match(main, /"\/api\/parts\/uuid\/\{uuid\}\/name"/);
});

stub.close();
console.log(`\ncreate_sphere_name: ${passed} checks passed`);
