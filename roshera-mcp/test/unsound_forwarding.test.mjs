/**
 * acknowledge_unsound WIRE-FORWARDING proof (2026-08-04).
 *
 * Context: the unsound-base gate used to live only in `gates.ts` (the MCP
 * client). It now ALSO lives in Rust (`api-server/src/main.rs::
 * refuse_unsound_base`, the same 9 mutating routes: boolean, shell, mirror,
 * fillet, chamfer, transform, pattern/linear, pattern/circular,
 * face/extrude), enforced independently of this client. Both sides honour
 * the SAME escape hatch — `acknowledge_unsound: true` — but before this fix
 * the MCP handlers built their outgoing request bodies WITHOUT the flag: the
 * client gate would let an acknowledged repair through, and the server gate
 * would then refuse it right back, because the key never crossed the wire.
 *
 * This exercises the REAL dispatch modules (ToolTable wrapper + gates.ts +
 * the real tool handlers, compiled from src to test/.build) against a local
 * stub backend that RECORDS the JSON body of every mutating POST it
 * receives — the only way to observe what actually crossed the wire, as
 * opposed to merely observing that the tool call returned success (a test
 * that only checked the return value would pass against the bug).
 *
 *   Build the fixture first (never touches dist/):
 *     npx tsc -p tsconfig.json --outDir test/.build
 *   Run:
 *     node test/unsound_forwarding.test.mjs
 *
 * Proves, for `boolean` (two-base op) and `shell` (single-base op):
 *   - `acknowledge_unsound: true` on the tool call → the backend receives a
 *     body containing `acknowledge_unsound: true`.
 *   - the flag omitted on the tool call → the backend receives a body that
 *     does NOT contain the key at all (never defaulted to `false`).
 *   - both assertions are made AFTER confirming the backend actually
 *     received a request for that route (a call swallowed upstream by the
 *     intent gate or the client-side unsound-base gate would trivially
 *     "pass" the omitted-key assertion with no request at all — see the two
 *     preconditions below).
 *
 * Two preconditions this file sets up so the request actually reaches the
 * stub (both gates in `gates.ts` run BEFORE the real handler and would
 * otherwise swallow the call, making the "field absent" half vacuous):
 *   1. INTENT GATE — `boolean`/`shell` are in `MUTATES_SOLIDS`; a real
 *      checkpoint is opened first via `timeline_checkpoint`.
 *   2. CLIENT UNSOUND-BASE GATE — the no-flag case needs the part's live
 *      verdict to read SOUND, or gates.ts refuses pre-flight and the stub
 *      never sees a request. The stub reports every part sound:true.
 */

import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const UUID_A = "aaaaaaaa-2222-4333-8444-555555555555"; // solid 7 — shell/boolean base
const UUID_B = "bbbbbbbb-2222-4333-8444-555555555555"; // solid 8 — boolean tool

// ─── Stub backend ───────────────────────────────────────────────────────────

const counts = { checkpoint: 0, blackboard: 0 };
/** Parsed JSON bodies received per mutating route, in call order. */
const bodies = { boolean: [], shell: [] };

const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  const send = (obj) => {
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify(obj));
  };
  let raw = "";
  req.on("data", (c) => (raw += c));
  req.on("end", () => {
    const body = raw.length ? JSON.parse(raw) : null;

    if (req.method === "GET" && url === "/api/scene/snapshot") {
      return send({
        objects: [
          { id: UUID_A, analytical_geometry: { solid_id: 7 } },
          { id: UUID_B, analytical_geometry: { solid_id: 8 } },
        ],
      });
    }
    if (req.method === "GET" && /^\/api\/agent\/parts\/\d+\/perception$/.test(url)) {
      // Every part reads SOUND — the no-flag case must reach the stub
      // (a false verdict here would let the client gate swallow the call
      // and make the "field absent" assertion vacuous; see module doc).
      return send({ valid: true, watertight: true, open_edges: 0 });
    }
    if (req.method === "GET" && /^\/api\/agent\/parts\/\d+$/.test(url)) {
      return send({
        id: 7,
        topology: { face_count: 6 },
        volume: 1000,
        location: { center_world: [0, 0, 0], dimensions_world: [1, 1, 1] },
      });
    }
    if (req.method === "GET" && url === "/api/agent/parts") {
      return send([{ id: 7 }, { id: 8 }, { id: 9 }]);
    }
    if (req.method === "GET" && url === "/api/document/units") {
      return send({ unit: "mm" });
    }
    if (req.method === "POST" && url === "/api/geometry/boolean") {
      bodies.boolean.push(body);
      return send({
        object: { id: "cccccccc-2222-4333-8444-555555555555" },
        solid_id: 9,
        valid: true,
        watertight: true,
      });
    }
    if (req.method === "POST" && url === "/api/geometry/shell") {
      bodies.shell.push(body);
      return send({
        object: { id: UUID_A },
        solid_id: 7,
        stats: { triangle_count: 12 },
        valid: true,
        watertight: true,
      });
    }
    if (req.method === "POST" && url === "/api/timeline/checkpoint") {
      counts.checkpoint++;
      return send({ id: "cp-1", name: JSON.parse(raw || "{}").name });
    }
    if (req.method === "POST" && url === "/api/blackboard/entries") {
      counts.blackboard++;
      return send({
        id: "bb-1",
        text: JSON.parse(raw || "{}").text,
        author: "agent",
        createdAt: 0,
        updatedAt: 0,
      });
    }
    res.writeHead(404, { "Content-Type": "application/json" });
    res.end("{}");
  });
});

stub.listen(0, "127.0.0.1");
await once(stub, "listening");
const port = stub.address().port;

// BASE is read at core.js module load — set it BEFORE importing anything.
process.env.ROSHERA_URL = `http://127.0.0.1:${port}`;

const { buildTable } = await import(
  pathToFileURL(join(HERE, ".build", "surface.js")).href
);

const table = buildTable();
const call = (name, args) => table.get(name).handler(args, {});
const firstJson = (r) => JSON.parse(r.content[0].text);

let passed = 0;
const check = (label, fn) => {
  fn();
  passed++;
  console.log(`  ok - ${label}`);
};

// ─── Open a real intent checkpoint (precondition 1) ─────────────────────────

const cp = await call("timeline_checkpoint", {
  name: "acknowledge_unsound wire-forwarding proof — verification cube",
  branch: "main",
});
check("checkpoint opens the intent (precondition for the mutating calls below)", () => {
  assert.equal(firstJson(cp).checkpoint.id, "cp-1");
  assert.equal(counts.checkpoint, 1);
});

// ─── boolean (two-base op) ───────────────────────────────────────────────────

const b1 = await call("boolean", { op: "union", object_a: UUID_A, object_b: UUID_B });
check("boolean without acknowledge_unsound: request reaches the backend, field absent", () => {
  assert.equal(firstJson(b1).refused, undefined, "not gate-refused");
  assert.equal(bodies.boolean.length, 1, "the stub actually received a request");
  assert.ok(
    !("acknowledge_unsound" in bodies.boolean[0]),
    `body must not carry the key at all when the caller omitted it; got ${JSON.stringify(bodies.boolean[0])}`,
  );
});

const b2 = await call("boolean", {
  op: "union",
  object_a: UUID_A,
  object_b: UUID_B,
  acknowledge_unsound: true,
});
check("boolean with acknowledge_unsound:true: the backend receives the flag", () => {
  assert.equal(firstJson(b2).refused, undefined, "not gate-refused");
  assert.equal(bodies.boolean.length, 2);
  assert.equal(
    bodies.boolean[1].acknowledge_unsound,
    true,
    `body must carry acknowledge_unsound:true; got ${JSON.stringify(bodies.boolean[1])}`,
  );
});

// ─── shell (single-base op) ──────────────────────────────────────────────────

const shellArgs = { object: UUID_A, thickness: 2, faces_to_remove: [] };

const s1 = await call("shell", shellArgs);
check("shell without acknowledge_unsound: request reaches the backend, field absent", () => {
  assert.equal(firstJson(s1).refused, undefined, "not gate-refused");
  assert.equal(bodies.shell.length, 1, "the stub actually received a request");
  assert.ok(
    !("acknowledge_unsound" in bodies.shell[0]),
    `body must not carry the key at all when the caller omitted it; got ${JSON.stringify(bodies.shell[0])}`,
  );
});

const s2 = await call("shell", { ...shellArgs, acknowledge_unsound: true });
check("shell with acknowledge_unsound:true: the backend receives the flag", () => {
  assert.equal(firstJson(s2).refused, undefined, "not gate-refused");
  assert.equal(bodies.shell.length, 2);
  assert.equal(
    bodies.shell[1].acknowledge_unsound,
    true,
    `body must carry acknowledge_unsound:true; got ${JSON.stringify(bodies.shell[1])}`,
  );
});

stub.close();
console.log(`\nunsound_forwarding: ${passed} checks passed`);
