/**
 * Part-pin proof — per-episode `BRepModel` isolation.
 *
 * The document pin (`ROSHERA_DOCUMENT`, core.ts) scopes the TIMELINE. It does
 * not scope the live model: `ActiveModel` routes on `X-Roshera-Part-Id`
 * (api-server/src/part_mgr.rs:264, 276) and falls back to the ONE global
 * `AppState.model` whenever that header is absent (part_mgr.rs:291-296) — and
 * this client never sent it. Measured on 8 concurrent live episodes
 * (2026-08-13, recorded in core.ts's own `newestPartId` comment): four sessions
 * reported the same `part_id` 97 and three the same 101, because all eight were
 * building into one shared model.
 *
 * `ROSHERA_PART` closes that: the harness creates one part per episode
 * (`POST /api/parts`, part_mgr.rs:340-358) and pins the child to it, so every
 * call resolves to that episode's own `BRepModel`.
 *
 *   Build the fixture first (never touches dist/):
 *     npx tsc -p tsconfig.json --outDir test/.build
 *   Run:
 *     node test/part_pin.test.mjs
 *
 * The stub backend implements `resolve_active_model` (part_mgr.rs:286-312)
 * faithfully — absent header → the legacy model; a non-UUID → 400; an
 * unregistered UUID → 404; a known UUID → that part's model — so a test
 * "episode" cannot invent an id the real backend would refuse.
 */
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { randomUUID } from "node:crypto";

const HERE = dirname(fileURLToPath(import.meta.url));
const PART_HEADER = "x-roshera-part-id"; // part_mgr.rs:264, lower-cased by node
const OBJECT_UUID = "cccccccc-3333-4333-8333-333333333333";

// ─── Stub backend: one legacy model + a PartManager-shaped registry ─────────

/**
 * `AppState.model` — the single global model every un-headered call reaches.
 *
 * `SolidStore::next_id` starts at 0 and is never reused within a model
 * (geometry-engine/src/primitives/solid.rs:1929, 1953-1963), so a fresh model's
 * first solid is id 0 while a long-lived shared one hands out far higher ones.
 */
const legacyModel = { solids: [], nextSolidId: 0 };
/** `PartManager.parts` — uuid → its own `BRepModel` (part_mgr.rs:97). */
const partModels = new Map();
/** Every request the stub saw: method, path, and the part header (or null). */
let seen = [];

const uuidRe = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/**
 * `resolve_active_model` (part_mgr.rs:286-312), including both refusals:
 * a header that is not a UUID is InvalidParameter (400) and a well-formed
 * UUID that is not registered is PartNotFound (404).
 */
function resolveActiveModel(header) {
  if (header === undefined) return { model: legacyModel };
  if (!uuidRe.test(header)) return { status: 400, error_code: "invalid_parameter" };
  const model = partModels.get(header);
  if (model === undefined) return { status: 404, error_code: "part_not_found" };
  return { model };
}

const stub = http.createServer((req, res) => {
  const url = (req.url ?? "").split("?")[0];
  const header = req.headers[PART_HEADER];
  seen.push({ method: req.method, url, part: header ?? null });
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    const send = (obj, status = 200) => {
      res.writeHead(status, { "Content-Type": "application/json" });
      res.end(JSON.stringify(obj));
    };
    // POST /api/parts — the harness's own call (part_mgr.rs:340-358). It is
    // NOT part-routed: it creates the model the header will later name.
    if (req.method === "POST" && url === "/api/parts") {
      const id = randomUUID();
      partModels.set(id, { solids: [], nextSolidId: 0 });
      return send({ id });
    }
    if (req.method === "DELETE" && url.startsWith("/api/parts/")) {
      const id = url.slice("/api/parts/".length);
      if (!partModels.delete(id)) return send({ error_code: "part_not_found" }, 404);
      return send({ success: true, id });
    }
    // Everything below is part-routed exactly as an `ActiveModel` handler is.
    const routed = resolveActiveModel(header);
    if (routed.model === undefined) {
      return send({ error_code: routed.error_code }, routed.status);
    }
    const model = routed.model;
    if (req.method === "POST" && url === "/api/geometry/cylinder") {
      const id = model.nextSolidId;
      model.nextSolidId += 1;
      model.solids.push({
        id,
        name: `Cylinder ${id}`,
        anchor_datum_id: 0,
        anchor_datum_name: "WorldOrigin",
        location_oneliner: `cylinder ${id}`,
      });
      return send({
        solid_id: id,
        object: { id: OBJECT_UUID },
        stats: { triangle_count: 96 },
        perception: { sound: true, brep_valid: true, watertight: true },
      });
    }
    // GET /api/agent/parts — `handlers::agent::list_parts` (agent.rs:70-81),
    // an `ActiveModel` read returning `PartSummary`s
    // (geometry-engine/src/readable/part.rs:405-416).
    if (req.method === "GET" && url === "/api/agent/parts") {
      return send(model.solids);
    }
    if (req.method === "GET" && /^\/api\/agent\/parts\/\d+$/.test(url)) {
      const id = Number(url.split("/").pop());
      const found = model.solids.find((s) => s.id === id);
      if (found === undefined) return send({ error_code: "solid_not_found" }, 404);
      return send({ ...found, location: { center_world: [0, 0, 0], dimensions_world: [1, 1, 1] } });
    }
    if (req.method === "GET" && /^\/api\/agent\/parts\/\d+\/perception$/.test(url)) {
      const id = Number(url.split("/")[4]);
      const found = model.solids.find((s) => s.id === id);
      if (found === undefined) return send({ error_code: "solid_not_found" }, 404);
      return send({ sound: true, valid: true, watertight: true, verdict: "SOUND", cert: null });
    }
    if (req.method === "GET" && /^\/api\/agent\/parts\/\d+\/render$/.test(url)) {
      return send({ open_edges: 0, nonmanifold_edges: 0, triangle_count: 96 });
    }
    if (req.method === "POST" && url === "/api/timeline/checkpoint") {
      return send({ id: "cp-1", name: body ? JSON.parse(body).name : null });
    }
    if (req.method === "GET" && url === "/api/document/units") {
      return send({ unit: "mm" });
    }
    if (req.method === "GET" && url === "/api/scene/snapshot") {
      return send({ objects: model.solids.map((s) => ({ id: OBJECT_UUID, analytical_geometry: { solid_id: s.id } })) });
    }
    send({});
  });
});
stub.listen(0, "127.0.0.1");
await once(stub, "listening");
const port = stub.address().port;

// BASE is read at core.js module load — set it BEFORE importing anything.
process.env.ROSHERA_URL = `http://127.0.0.1:${port}`;
// No document pin: this suite measures the PART header, and a stray
// ROSHERA_DOCUMENT inherited from the environment would put a second header on
// every request the assertions below read.
delete process.env.ROSHERA_DOCUMENT;

const coreUrl = pathToFileURL(join(HERE, ".build", "core.js")).href;

/** A fresh core instance: `boundPart` is module state, one binding per process. */
let instances = 0;
async function freshCore() {
  instances += 1;
  return await import(`${coreUrl}?case=${instances}-${Date.now()}`);
}

/** `POST /api/parts` as the HARNESS makes it — never from inside the MCP. */
async function createPart() {
  const res = await fetch(`http://127.0.0.1:${port}/api/parts`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ name: "rl-episode" }),
  });
  return (await res.json()).id;
}

// Every check runs; failures are collected rather than aborting the suite, so
// one run reports the whole picture instead of only its first red.
const checks = [];
const check = (name, fn) => checks.push([name, fn]);

// ─── THE PROOF: N episodes, N models, one solid each ────────────────────────

check("8 concurrent episodes each see EXACTLY their own solid, never each other's", async () => {
  seen = [];
  const N = 8;
  const episodes = [];
  for (let i = 0; i < N; i += 1) {
    // One part per episode, created by the harness, then pinned.
    const partId = await createPart();
    process.env.ROSHERA_PART = partId;
    const core = await freshCore();
    // The pin is read from the environment on this instance's first call.
    // A real episode is a CHILD PROCESS with its own environment; these
    // instances share one, so each is bound here, under its own pin, before
    // the concurrent phase starts.
    await core.api("GET", "/api/agent/parts");
    episodes.push({ partId, core });
  }
  delete process.env.ROSHERA_PART;

  // All eight build at once — the same interleaving the live batch ran.
  const built = await Promise.all(
    episodes.map((e) =>
      e.core.api("POST", "/api/geometry/cylinder", { radius: 25, height: 60 }),
    ),
  );
  // Then each reads the model it is scoped to (what `list_parts` returns —
  // roshera-mcp/src/tools/inspect.ts:16, GET /api/agent/parts).
  const views = await Promise.all(
    episodes.map((e) => e.core.api("GET", "/api/agent/parts")),
  );

  for (let i = 0; i < N; i += 1) {
    assert.equal(
      views[i].length, 1,
      `episode ${i} sees ${views[i].length} solid(s) in its model, expected ` +
      `EXACTLY 1 (its own). More than one means the sessions share a model: ` +
      `ids seen = [${views[i].map((s) => s.id).join(", ")}]`,
    );
    assert.equal(
      views[i][0].id, built[i].solid_id,
      `episode ${i} sees solid ${views[i][0].id} but built ${built[i].solid_id}`,
    );
    assert.equal(
      built[i].solid_id, 0,
      `episode ${i}'s own model minted solid id ${built[i].solid_id}; a fresh ` +
      `model mints 0 (solid.rs:1929), and a shared monotone counter is what ` +
      `produced the live ids in the 70s-90s`,
    );
  }
  const unheaded = seen.filter(
    (s) => s.part === null && s.url !== "/api/parts",
  );
  assert.deepEqual(
    unheaded, [],
    `every call must carry the pin; these did not: ` +
    `${JSON.stringify(unheaded)} — an un-headered call resolves to the legacy ` +
    `global model (part_mgr.rs:291-296) whatever the others do`,
  );
});

// ─── The pin's shape: present, absent, whitespace ───────────────────────────

check("an explicit ROSHERA_PART pin rides on every call", async () => {
  seen = [];
  const partId = await createPart();
  process.env.ROSHERA_PART = partId;
  const core = await freshCore();
  await core.api("GET", "/api/agent/parts");
  await core.api("POST", "/api/geometry/cylinder", { radius: 3, height: 4 });
  delete process.env.ROSHERA_PART;
  assert.ok(seen.length >= 2, "both calls reached the stub");
  for (const s of seen.filter((x) => x.url !== "/api/parts")) {
    assert.equal(s.part, partId, `${s.method} ${s.url} carried the pin`);
  }
});

check("with no pin, NO header is sent at all — byte-for-byte legacy", async () => {
  seen = [];
  delete process.env.ROSHERA_PART;
  const core = await freshCore();
  await core.api("GET", "/api/agent/parts");
  const call = seen.find((s) => s.url === "/api/agent/parts");
  assert.ok(call, "the probe call reached the stub");
  assert.equal(
    call.part, null,
    "an unpinned client must send no X-Roshera-Part-Id — the backend's " +
    "absent-header fallback (part_mgr.rs:291-296) is what every existing " +
    "client and all 13 legacy-only routes still rely on",
  );
});

check("an empty pin is treated as absent, never as an empty part id", async () => {
  seen = [];
  process.env.ROSHERA_PART = "   ";
  const core = await freshCore();
  await core.api("GET", "/api/agent/parts");
  delete process.env.ROSHERA_PART;
  const call = seen.find((s) => s.url === "/api/agent/parts");
  assert.equal(
    call.part, null,
    "whitespace is not a part id: an empty header is a malformed reference " +
    "the backend answers 400 to (part_mgr.rs:298-308), not the absence meant",
  );
});

check("bindSessionPart() is the explicit binder, and reads the env when called", async () => {
  seen = [];
  const partId = await createPart();
  const core = await freshCore();
  process.env.ROSHERA_PART = partId;
  core.bindSessionPart();
  delete process.env.ROSHERA_PART;
  await core.api("GET", "/api/agent/parts");
  const call = seen.find((s) => s.url === "/api/agent/parts");
  assert.equal(
    call.part, partId,
    "the value is read when bindSessionPart runs, not at module load",
  );
});

// ─── The real dispatch surface, end to end ─────────────────────────────────

check("the real tool table carries the pin on every backend call it makes", async () => {
  seen = [];
  const partId = await createPart();
  process.env.ROSHERA_PART = partId;
  // surface.js statically imports core.js, so this exercises the ONE unqueried
  // core instance — bound on its first call, below.
  const { buildTable } = await import(
    pathToFileURL(join(HERE, ".build", "surface.js")).href
  );
  const table = buildTable();
  const call = (name, args) => table.get(name).handler(args, {});
  // The reference RL policy's exact sequence: declare an intent (the intent
  // gate refuses a mutating call without one — gates.ts:514-528), build, then
  // verify. `list_parts` is the read that exposes a shared model.
  await call("timeline_checkpoint", { name: "boss ø40 x 12 tall on the base plate" });
  const made = await call("create_cylinder", { plane: "xy", cx: 0, cy: 0, radius: 25, height: 60 });
  const madeJson = JSON.parse(made.content[0].text);
  await call("verify_part", { part_id: madeJson.part_id });
  const listed = await call("list_parts", {});
  delete process.env.ROSHERA_PART;

  const unheaded = seen.filter((s) => s.part === null && s.url !== "/api/parts");
  assert.deepEqual(
    unheaded, [],
    `these tool-driven calls carried no pin: ${JSON.stringify(unheaded)}`,
  );
  const parts = JSON.parse(listed.content[0].text);
  assert.equal(parts.length, 1, "list_parts sees exactly the one solid this session built");
  assert.equal(parts[0].id, madeJson.part_id, "and it is the id create_cylinder reported");
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
process.stdout.write(
  `\npart_pin: ${checks.length - failed}/${checks.length} checks passed\n`,
);
if (failed > 0) process.exitCode = 1;
