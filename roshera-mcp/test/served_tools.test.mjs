/**
 * Backend-defined tools, registered generically (src/served.ts) — the
 * single-registry stage-1 adapter, proven against the backend's own
 * definition files (`roshera-backend/api-server/src/agent_tools/*.json`).
 *
 *   (s1) every definition is registered from the definition file alone: the
 *        table holds it, classified by its definition (bench, stability,
 *        effects → MCP annotations), with NO second classification in
 *        registry.ts's BENCH_OF and no hand-written registration module;
 *   (s2) PRODUCTION CALL SITE: every definition's every example validates
 *        through the SDK path (validateOp) and dispatches through the real
 *        table to the route the definition names, against a stub server;
 *   (s3) the listing a client receives matches the definition: properties,
 *        required, types;
 *   (s4) the schema converter refuses what it does not model (a oneOf, an
 *        unknown format, a non-string enum) and a listing in another
 *        registry major version — never a partial bench;
 *   (s5) an unknown argument is refused, not dropped;
 *   (s6) result shapes: assembly_create renames `id` to `assembly_id` and
 *        echoes the name; assembly_view returns the image plus its caption.
 *
 *     npm run test:gates:build && node test/served_tools.test.mjs
 */
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { readFileSync, readdirSync } from "node:fs";
import { pathToFileURL, fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const build = (p) => import(pathToFileURL(join(HERE, ".build", p)).href);

let seen = [];
const stub = http.createServer((req, res) => {
  let raw = "";
  req.on("data", (c) => (raw += c));
  req.on("end", () => {
    seen.push({ method: req.method, url: req.url ?? "", body: raw.length ? JSON.parse(raw) : undefined });
    res.writeHead(200, { "Content-Type": "application/json" });
    // One answer that satisfies every result shape the definitions use.
    res.end(
      JSON.stringify({
        id: "99999999-9999-4999-8999-999999999999",
        png_base64: "iVBORw0KGgo=",
        instance_count: 2,
        unique_part_count: 1,
        open_edges: 0,
        nonmanifold_edges: 0,
      }),
    );
  });
});
stub.listen(0, "127.0.0.1");
await once(stub, "listening");
process.env.ROSHERA_URL = `http://127.0.0.1:${stub.address().port}`;

const { buildTable } = await build("surface.js");
const { validateOp } = await build("metatools.js");
const { metaFor, toolJsonSchema, explicitBenchTable } = await build("registry.js");
const served = await build("served.js");
const table = buildTable();
const reg = served.loadCheckedInDefs();

let passed = 0;
const check = async (label, fn) => {
  seen = [];
  await fn();
  passed += 1;
  console.log(`  ok - ${label}`);
};

await check("(s1) every definition is registered and classified from its definition", async () => {
  assert.ok(reg.tools.length >= 12, `expected the assembly bench, got ${reg.tools.length}`);
  const bench = explicitBenchTable();
  for (const def of reg.tools) {
    const entry = table.get(def.name);
    assert.ok(entry, `${def.name} is defined by the backend but not registered`);
    assert.deepEqual(entry.served, def, `${def.name} was registered from something other than its definition`);
    assert.equal(entry.description, def.description);
    assert.deepEqual(metaFor(def.name), { bench: def.bench, stability: def.stability });
    assert.deepEqual(entry.annotations, {
      readOnlyHint: def.effects === "read_only",
      destructiveHint: def.effects === "destructive",
      idempotentHint: def.idempotent,
    });
    assert.ok(!(def.name in bench), `${def.name} has a second classification in BENCH_OF`);
  }
  const surface = readFileSync(join(HERE, "..", "src", "surface.ts"), "utf8");
  assert.ok(!/tools\/assembly\.js/.test(surface), "surface.ts still registers the hand-written assembly module");
  for (const f of readdirSync(join(HERE, "..", "src", "tools")).filter((f) => f.endsWith(".ts") && f !== "assembly.ts")) {
    const src = readFileSync(join(HERE, "..", "src", "tools", f), "utf8");
    for (const def of reg.tools) {
      assert.ok(!src.includes(`"${def.name}"`), `src/tools/${f} hand-registers ${def.name}`);
    }
  }
});

await check("(s2) every example validates and calls the route its definition names", async () => {
  let calls = 0;
  for (const def of reg.tools) {
    for (const ex of def.examples) {
      seen = [];
      const { parsed } = await validateOp(table, def.name, ex.args);
      const result = await table.get(def.name).handler(parsed, {});
      assert.notEqual(result.isError, true, `${def.name} '${ex.title}': ${result.content?.[0]?.text}`);
      const route = def.call.switch ? def.call.cases[parsed[def.call.switch]] : def.call;
      assert.equal(seen.length, 1, `${def.name} '${ex.title}' made ${seen.length} requests`);
      const pattern = new RegExp(
        "^" + route.path.replace(/[.*+?^$()|[\]\\]/g, "\\$&").replace(/\\?\{[A-Za-z0-9_]+\\?\}/g, "[^/?]+") + "(\\?.*)?$",
      );
      assert.equal(seen[0].method, route.method, `${def.name} '${ex.title}'`);
      assert.match(seen[0].url, pattern, `${def.name} '${ex.title}' called ${seen[0].url}`);
      assert.equal(seen[0].body === undefined, route.body === undefined, `${def.name}: body iff the route takes one`);
      calls += 1;
    }
  }
  assert.ok(calls >= 12, `only ${calls} examples ran`);
});

await check("(s3) the listing a client receives matches the definition", async () => {
  const types = (s, root) => {
    while (s && s.$ref) s = s.$ref.slice(2).split("/").reduce((t, k) => t?.[k], root);
    return s?.type ?? (s?.enum ? "string" : null);
  };
  for (const def of reg.tools) {
    const listed = toolJsonSchema(table.get(def.name));
    assert.deepEqual(Object.keys(listed.properties).sort(), Object.keys(def.input_schema.properties).sort(), def.name);
    assert.deepEqual([...(listed.required ?? [])].sort(), [...(def.input_schema.required ?? [])].sort(), def.name);
    for (const [k, v] of Object.entries(def.input_schema.properties)) {
      assert.equal(types(listed.properties[k], listed), types(v, def.input_schema), `${def.name}.${k}`);
    }
    assert.equal(listed.additionalProperties, false, `${def.name} must refuse unknown arguments`);
  }
});

await check("(s4) the converter and the loader refuse what they do not model", async () => {
  assert.throws(() => served.jsonSchemaToZod({ oneOf: [{ type: "string" }] }), /oneOf/);
  assert.throws(() => served.jsonSchemaToZod({ type: "string", format: "email" }), /format 'email'/);
  assert.throws(() => served.jsonSchemaToZod({ enum: [1, 2] }), /string enums/);
  assert.throws(() => served.jsonSchemaToZod({ type: "tuple" }), /not modelled/);
  assert.throws(() => served.checkRegistryVersion("3.0.0", "test"), /refusing the whole listing/);
  assert.doesNotThrow(() => served.checkRegistryVersion("2.4.1", "test"));
  // A listing with one unconvertible definition registers NOTHING.
  const { ToolTable } = await build("registry.js");
  const t = new ToolTable();
  const good = reg.tools[0];
  const broken = { ...good, name: "broken_tool", input_schema: { type: "object", properties: { x: { anyOf: [] } } } };
  assert.throws(() => served.registerServedTools(t, { registry_version: "2.0.0", tools: [good, broken], source: "test" }));
  assert.equal(t.size, 0, "no partial bench");
});

await check("(s5) an unknown argument is refused, not dropped", async () => {
  await assert.rejects(
    validateOp(table, "assembly_dof", { assembly_id: "11111111-1111-4111-8111-111111111111", verbose: true }),
    /verbose|Unrecognized/,
  );
});

await check("(s6) result shapes: rename + echo, and image + caption", async () => {
  const created = await table.get("assembly_create").handler({ name: "gearbox" }, {});
  const out = JSON.parse(created.content[0].text);
  assert.equal(out.assembly_id, "99999999-9999-4999-8999-999999999999");
  assert.equal(out.name, "gearbox");
  assert.ok(!("id" in out), "the raw id field is renamed, not duplicated");

  const { parsed } = await validateOp(table, "assembly_view", { assembly_id: "11111111-1111-4111-8111-111111111111" });
  const view = await table.get("assembly_view").handler(parsed, {});
  assert.equal(view.content[0].type, "image");
  assert.equal(view.content[0].data, "iVBORw0KGgo=");
  assert.equal(view.content[1].text, "assembly az=35° el=20° instances=2 distinct_parts=1 open=0 nm=0");
  assert.match(seen.at(-1).url, /az=35&el=20&mode=shaded&size=720&quality=medium$/);
});

stub.close();
console.log(`\nserved_tools: ${passed} checks passed`);
