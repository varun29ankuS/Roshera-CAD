/**
 * Units and ids, read as an LLM reads them (MCP_AUDIT.md §5).
 *
 * Angles: `revolve` took degrees (`angle_deg`) and `psketch_revolve` radians
 * (`angle`) for the same shape. `psketch_revolve` now takes `angle_deg`
 * (degrees, the unit `revolve`, `transform.rotation.angle_deg`,
 * `drill_pattern.start_angle_deg` and `assembly_add_instance.rotation_deg`
 * use) and still accepts the old radians `angle` for an existing caller.
 *
 *   (u1) angle_deg 90 reaches the route as π/2 radians (the route's unit);
 *   (u2) the deprecated radians `angle` passes through unchanged;
 *   (u3) both at once is a typed refusal and nothing is sent;
 *   (u4) every id parameter names its id space (integer part_id vs string
 *        object_uuid) — except the resident tools the minimal token budget
 *        holds, listed here by name so a NEW undocumented id parameter fails;
 *   (u5) the server instructions state both id spaces and the angle rule,
 *        and index.ts sends them.
 *
 *     npm run test:gates:build && node test/units_and_ids.test.mjs
 */
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { readFileSync } from "node:fs";
import { pathToFileURL, fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const SK = "bbbbbbbb-1111-4222-8333-444444444444";

let revolveBodies = [];
const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  let raw = "";
  req.on("data", (c) => (raw += c));
  req.on("end", () => {
    const send = (obj) => {
      res.writeHead(200, { "Content-Type": "application/json" });
      res.end(JSON.stringify(obj));
    };
    if (req.method === "POST" && url === `/api/csketch/${SK}/revolve`) {
      revolveBodies.push(JSON.parse(raw));
      return send({ object: { id: "cccccccc-1111-4222-8333-444444444444" }, solid_id: 3, stats: {} });
    }
    if (req.method === "POST" && url === "/api/timeline/checkpoint") return send({ id: "cp-1" });
    if (req.method === "POST" && url === "/api/blackboard/entries") return send({ id: "bb-1" });
    if (req.method === "GET" && url === "/api/agent/parts") return send([{ id: 3 }]);
    if (req.method === "GET" && url === "/api/document/units") return send({ unit: "mm" });
    res.writeHead(404, { "Content-Type": "application/json" });
    res.end("{}");
  });
});
stub.listen(0, "127.0.0.1");
await once(stub, "listening");
process.env.ROSHERA_URL = `http://127.0.0.1:${stub.address().port}`;
process.env.ROSHERA_MCP_AUTOVERIFY = "0";

const { buildTable, MINIMAL_SURFACE } = await import(pathToFileURL(join(HERE, ".build", "surface.js")).href);
const { toolJsonSchema } = await import(pathToFileURL(join(HERE, ".build", "registry.js")).href);
const core = await import(pathToFileURL(join(HERE, ".build", "core.js")).href);
const table = buildTable();
const call = (name, args) => table.get(name).handler(args, {});
const revolveArgs = (extra) => ({ csketch_id: SK, axis_origin: [0, 0], axis_direction: [0, 1], ...extra });

let passed = 0;
const check = async (label, fn) => {
  revolveBodies = [];
  await fn();
  passed += 1;
  console.log(`  ok - ${label}`);
};

await call("timeline_checkpoint", { name: "units proof — revolve the bearing seat profile 90 degrees", branch: "main" });

await check("(u1) angle_deg reaches the route in radians", async () => {
  const r = await call("psketch_revolve", revolveArgs({ angle_deg: 90 }));
  assert.notEqual(r.isError, true, r.content[0].text);
  assert.equal(revolveBodies.length, 1);
  assert.ok(Math.abs(revolveBodies[0].angle - Math.PI / 2) < 1e-12, `sent ${revolveBodies[0].angle}`);
});

await check("(u2) the deprecated radians angle still works", async () => {
  const r = await call("psketch_revolve", revolveArgs({ angle: 1.25 }));
  assert.notEqual(r.isError, true, r.content[0].text);
  assert.equal(revolveBodies[0].angle, 1.25);
});

await check("(u3) both angles at once is refused, nothing sent", async () => {
  const r = await call("psketch_revolve", revolveArgs({ angle_deg: 90, angle: 1.0 }));
  assert.equal(r.isError, true);
  assert.equal(r.structuredContent?.error_code, "invalid_parameter");
  assert.equal(revolveBodies.length, 0);
});

// Resident tools whose id descriptions the minimal token budget holds (the
// budget is a decision for Varun, test/scale_s2.mjs); their TYPES still tell
// the spaces apart (integer vs uuid string) and the server instructions say it.
const BUDGET_HELD = new Set([
  "render_part.part_id",
  "verify_part.part_id",
  "section_view.part_id",
  "get_part.part_id",
  "mass_properties.part_id",
  "delete_part.part_id",
  "boolean.object_a",
  "boolean.object_b",
  "psketch_plane_from_face.object_uuid",
]);
// Out of scope for this change (drawing tools are owned by another worktree).
const NOT_MINE = new Set(["make_drawing.part_id"]);

await check("(u4) every id parameter names its id space", async () => {
  const missing = [];
  for (const e of table.all()) {
    const props = toolJsonSchema(e).properties ?? {};
    for (const [k, v] of Object.entries(props)) {
      const key = `${e.name}.${k}`;
      const isInt = k === "part_id" && v.type === "integer";
      const isUuid = /^(object|object_a|object_b|object_uuid|object_id|base)$/.test(k) && v.type === "string";
      if (!isInt && !isUuid) continue;
      if (BUDGET_HELD.has(key) || NOT_MINE.has(key)) {
        assert.ok(MINIMAL_SURFACE.includes(e.name) || NOT_MINE.has(key), `${key} is held but not resident`);
        continue;
      }
      const d = String(v.description ?? "");
      const ok = isInt ? /integer part_id/.test(d) && /object_uuid/.test(d) : /object_uuid/.test(d) && /part_id/.test(d);
      if (!ok) missing.push(`${key}: ${d}`);
    }
  }
  assert.deepEqual(missing, [], `id parameters that do not name their id space:\n${missing.join("\n")}`);
});

await check("(u5) the server instructions state both id spaces and the angle rule", async () => {
  const text = core.ID_SPACES_INSTRUCTIONS;
  assert.match(text, /integer part_id/);
  assert.match(text, /object_uuid/);
  assert.match(text, /degrees/);
  const index = readFileSync(join(HERE, "..", "src", "index.ts"), "utf8");
  assert.match(index, /instructions:\s*ID_SPACES_INSTRUCTIONS/, "index.ts must send the instructions");
});

stub.close();
console.log(`\nunits_and_ids: ${passed} checks passed`);
