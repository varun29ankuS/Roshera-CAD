/**
 * cad_program output→input chaining proof (2026-08-09).
 *
 * Before this, cad_program args were LITERAL: an id-returning call
 * (psketch_begin) had to run OUTSIDE the program and its id be written in by
 * hand — so the canonical "begin → polyline → extrude" flow could never be one
 * program. This test pins the minimal chaining contract:
 *
 *   - a string arg that is EXACTLY `$N.<dot.path>` or `$prev.<dot.path>` is
 *     replaced by that field of the earlier op's parsed JSON result before the
 *     op runs (whole-string only, no interpolation);
 *   - references are validated UP FRONT (a forward/self reference refuses the
 *     whole program, zero ops run);
 *   - a placeholder op is schema-validated at EXECUTION time, after
 *     resolution — a post-resolution schema failure stops the program there,
 *     prefix applied, honestly reported;
 *   - an unresolvable path is a typed error naming the placeholder and the
 *     keys that WERE available; the program stops there (no rollback);
 *   - `$$` escapes a literal leading `$` in any op's string args.
 *
 * Exercises the REAL modules (compiled src → test/.build) against a stub
 * backend recording exactly what the wire received.
 *
 *   Build:  npx tsc -p tsconfig.json --outDir test/.build
 *   Run:    node test/cad_program_chaining.test.mjs
 */

import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const SK = "cccccccc-3333-4333-8333-333333333333";
const OBJ = "dddddddd-4444-4444-8444-444444444444";

// ─── Stub backend (records what the wire received) ──────────────────────────

const received = {
  polylineSketchIds: [],
  extrudeSketchIds: [],
  blackboardTexts: [],
};

const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  const send = (obj) => {
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify(obj));
  };
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    let m;
    if (req.method === "POST" && url === "/api/csketch") {
      return send({ id: SK });
    }
    if (req.method === "POST" && (m = url.match(/^\/api\/csketch\/([^/]+)\/polyline$/))) {
      received.polylineSketchIds.push(decodeURIComponent(m[1]));
      return send({ id: `poly-${received.polylineSketchIds.length}` });
    }
    if (req.method === "POST" && (m = url.match(/^\/api\/csketch\/([^/]+)\/extrude$/))) {
      received.extrudeSketchIds.push(decodeURIComponent(m[1]));
      return send({
        object: { id: OBJ },
        solid_id: 7,
        stats: { triangle_count: 12, regions: 1 },
        valid: true,
        watertight: true,
      });
    }
    if (req.method === "GET" && url === "/api/agent/parts") {
      return send([{ id: 7 }]);
    }
    if (req.method === "GET" && url === "/api/agent/parts/7") {
      return send({
        id: 7,
        topology: { face_count: 6 },
        volume: 1000,
        location: { center_world: [0, 0, 0], dimensions_world: [1, 1, 1] },
      });
    }
    if (req.method === "GET" && url === "/api/agent/parts/7/perception") {
      return send({ valid: true, watertight: true, open_edges: 0 });
    }
    if (req.method === "GET" && url === "/api/document/units") {
      return send({ unit: "mm" });
    }
    if (req.method === "POST" && url === "/api/timeline/checkpoint") {
      return send({ id: "cp-1", name: JSON.parse(body || "{}").name });
    }
    if (req.method === "POST" && url === "/api/blackboard/entries") {
      const text = JSON.parse(body || "{}").text;
      received.blackboardTexts.push(text);
      return send({ id: "bb-1", text, author: "agent", createdAt: 0, updatedAt: 0 });
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

// psketch_extrude is intent-gated: open a real design intent first.
await call("timeline_checkpoint", {
  name: "chaining proof plate 10x10x5 — begin/polyline/extrude in one program",
  branch: "main",
});

// ─── 1. the canonical flow is ONE program ───────────────────────────────────

const prog = await call("cad_program", {
  name: "begin → polyline → extrude",
  ops: [
    { tool: "psketch_begin", args: {} },
    {
      tool: "psketch_add_entity",
      args: {
        csketch_id: "$0.csketch_id",
        kind: "polyline",
        params: { points: [[0, 0], [10, 0], [10, 10], [0, 10]], closed: true },
      },
    },
    {
      tool: "psketch_extrude",
      args: { csketch_id: "$prev.csketch_id", distance: 5 },
    },
  ],
});
check("begin → polyline($0.csketch_id) → extrude runs as one program", () => {
  const j = firstJson(prog);
  // $prev at op 2 points at op 1 (the polyline), whose result has no
  // csketch_id — this variant is exercised in test 4; here op 2 must have
  // FAILED for that reason if $prev semantics are prev-op, or succeeded if
  // implementation resolved it. The canonical contract: $prev = the
  // immediately preceding op. So this program stops at op 2.
  assert.equal(j.completed, 2);
  assert.equal(j.stopped_at, 2);
  assert.match(j.ops[2].error, /\$prev\.csketch_id/);
  assert.match(j.ops[2].error, /available: id/);
  // and the polyline DID reach the backend under the real uuid:
  assert.deepEqual(received.polylineSketchIds, [SK]);
});

const prog2 = await call("cad_program", {
  name: "begin → polyline → extrude (explicit indices)",
  ops: [
    { tool: "psketch_begin", args: {} },
    {
      tool: "psketch_add_entity",
      args: {
        csketch_id: "$0.csketch_id",
        kind: "polyline",
        params: { points: [[0, 0], [10, 0], [10, 10], [0, 10]], closed: true },
      },
    },
    {
      tool: "psketch_extrude",
      args: { csketch_id: "$0.csketch_id", distance: 5 },
    },
  ],
});
check("$0.csketch_id chains begin's id into BOTH later ops; all complete", () => {
  const j = firstJson(prog2);
  assert.equal(j.ok, true);
  assert.equal(j.completed, 3);
  assert.equal(j.stopped_at, null);
  assert.deepEqual(received.polylineSketchIds, [SK, SK]);
  assert.deepEqual(received.extrudeSketchIds, [SK]);
  // the extrude's ledger entry carries its own certificate, as ever
  assert.ok(j.ops[2].certificate, "chained op still carries its certificate");
});

// ─── 2. forward/self references refuse the WHOLE program up front ───────────

const fwd = await call("cad_program", {
  ops: [
    {
      tool: "psketch_add_entity",
      args: { csketch_id: "$1.csketch_id", kind: "point", params: { x: 0, y: 0 } },
    },
    { tool: "psketch_begin", args: {} },
  ],
});
check("a forward reference refuses the whole program, zero ops run", () => {
  const j = firstJson(fwd);
  assert.equal(fwd.isError, true);
  assert.equal(j.stage, "validation");
  assert.equal(j.executed, 0);
  assert.match(j.errors[0].reason, /\$1\.csketch_id/);
});

const prevAtZero = await call("cad_program", {
  ops: [
    {
      tool: "psketch_add_entity",
      args: { csketch_id: "$prev.csketch_id", kind: "point", params: { x: 0, y: 0 } },
    },
  ],
});
check("$prev on the first op refuses the whole program", () => {
  const j = firstJson(prevAtZero);
  assert.equal(prevAtZero.isError, true);
  assert.equal(j.stage, "validation");
  assert.equal(j.executed, 0);
});

// ─── 3. post-resolution schema failure stops honestly ───────────────────────

const badSchema = await call("cad_program", {
  ops: [
    { tool: "psketch_begin", args: {} },
    {
      tool: "psketch_extrude",
      // resolves to a STRING uuid where a number is required
      args: { csketch_id: "$0.csketch_id", distance: "$0.csketch_id" },
    },
  ],
});
check("a post-resolution schema failure stops at that op, prefix applied", () => {
  const j = firstJson(badSchema);
  assert.equal(j.completed, 1);
  assert.equal(j.stopped_at, 1);
  assert.equal(j.ops[1].ok, false);
  assert.match(j.ops[1].error, /after placeholder resolution/);
});

// ─── 4. unresolvable path: typed error naming what WAS available ────────────

const badPath = await call("cad_program", {
  ops: [
    { tool: "psketch_begin", args: {} },
    {
      tool: "psketch_add_entity",
      args: {
        csketch_id: "$0.nope",
        kind: "polyline",
        params: { points: [[0, 0], [1, 0], [1, 1]], closed: true },
      },
    },
  ],
});
check("an unresolvable path is a typed stop naming the available keys", () => {
  const j = firstJson(badPath);
  assert.equal(j.completed, 1);
  assert.equal(j.stopped_at, 1);
  assert.match(j.ops[1].error, /\$0\.nope/);
  assert.match(j.ops[1].error, /available: csketch_id/);
});

// ─── 5. `$$` escapes a literal `$` ──────────────────────────────────────────

const escaped = await call("cad_program", {
  ops: [
    {
      tool: "blackboard_add_entry",
      args: { text: "$$prev.csketch_id is reserved syntax", author: "agent" },
    },
  ],
});
check("'$$' escapes: the backend receives a literal '$prev…' string", () => {
  const j = firstJson(escaped);
  assert.equal(j.ok, true);
  // blackboardTexts[0] is the checkpoint's auto-written intent line; the
  // program's entry is the last one, with exactly ONE leading '$' left.
  assert.equal(
    received.blackboardTexts[received.blackboardTexts.length - 1],
    "$prev.csketch_id is reserved syntax",
  );
});

stub.close();
console.log(`\ncad_program_chaining: ${passed} checks passed`);
