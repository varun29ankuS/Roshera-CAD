/**
 * A refusal and a halt are FAILURES the client sees (audit 2026-09-03,
 * Task 71).
 *
 * `cad_program` stops on a result whose `isError` is true and ledgers
 * everything else as `ok: true`. Three tool paths answered a refusal or an
 * unsound halt with an MCP SUCCESS body:
 *   - the timeline tools' `refusalOrFail` turned a typed backend refusal
 *     (409/422/404) into `ok({ refused: <body> })`;
 *   - `boolean_many` and `drill_pattern` returned an UNSOUND halt as
 *     `ok({ halted: ... })`;
 *   - `ask_choice` and `kb_lookup` returned `ok({ refused: true, ... })`.
 * So a refused `timeline_mould` inside a program was ledgered ok:true and the
 * next op built on a model the agent believed had changed. And the program's
 * own stopped result was itself an MCP success.
 *
 * Exercises the REAL dispatch modules (ToolTable wrapper + gates.ts + real
 * handlers, compiled from src to test/.build) against a local stub backend
 * that COUNTS what reached the wire — a stop is proven by the ops after it
 * never arriving, not by the ledger's word alone.
 *
 *   Build the fixture first (never touches dist/):
 *     npm run test:gates:build
 *   Run:
 *     node test/refusal_stops_program.test.mjs
 *
 * Proves:
 *   1. a program whose op 1 is a backend-refused timeline_mould stops AT
 *      index 1 (the mould reached the backend; op 2 never did) and is reported
 *      as a failure (`isError: true`, ledger in structuredContent);
 *   2. a program containing a halted boolean_many stops there (the boolean
 *      reached the backend; the op after it never did);
 *   3. an all-ok program is NOT an error (a mutant that always sets isError
 *      is caught);
 *   4. a stopped program is never served from the refusal cache: re-issuing
 *      it runs its prefix again (the prefix really executed the first time);
 *   5. a halted boolean_many still counts as unverified mutating work — the
 *      closing checkpoint is refused by gate 6 (verification_scope);
 *   6. every halt and direct refusal is an error result carrying its typed
 *      payload: boolean_many and drill_pattern halts, timeline_mould,
 *      ask_choice, kb_lookup (pack, playbook, reference) — and a resolved
 *      lookup is not an error.
 */

import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const BASE_UUID = "aaaaaaaa-2222-4333-8444-555555555555"; // solid 7 — sound base
const TOOL_UUID = "bbbbbbbb-2222-4333-8444-555555555555"; // solid 8 — cutter
const BOX_UUID = "cccccccc-3333-4333-8333-333333333333"; // solid 1
const EVENT_ID = "dddddddd-4444-4444-8444-444444444444";

// ─── Stub backend (counts every mutating route it receives) ────────────────

const counts = { checkpoint: 0, box: 0, boolean: 0, mould: 0, cylinder: 0 };
const MOULD_REFUSAL = {
  verdict: "broken_downstream",
  broken: [{ feature: "fillet 3", reason: "edge no longer exists" }],
};

const stub = http.createServer((req, res) => {
  const url = (req.url ?? "").split("?")[0];
  const send = (obj, status = 200) => {
    res.writeHead(status, { "Content-Type": "application/json" });
    res.end(JSON.stringify(obj));
  };
  let raw = "";
  req.on("data", (c) => (raw += c));
  req.on("end", () => {
    const body = raw.length ? JSON.parse(raw) : {};
    if (req.method === "POST" && url === "/api/timeline/checkpoint") {
      counts.checkpoint++;
      return send({ id: `cp-${counts.checkpoint}`, name: body.name, branch: "main" });
    }
    if (req.method === "POST" && url === "/api/blackboard/entries") {
      return send({ id: "bb-1", text: body.text, author: "agent", createdAt: 0, updatedAt: 0 });
    }
    if (req.method === "POST" && url === "/api/geometry/box") {
      counts.box++;
      return send({
        solid_id: 1,
        object: { id: BOX_UUID },
        stats: { triangle_count: 12 },
        perception: { sound: true },
      });
    }
    if (req.method === "POST" && url === "/api/geometry/boolean") {
      counts.boolean++;
      // The boolean's result is solid 9, which reads UNSOUND below — the
      // base (solid 7) stays sound so the client unsound-base pre-gate lets
      // every boolean_many in this file reach the backend (a halt, not a
      // gate refusal, is what is under test).
      return send({ object: { id: BASE_UUID }, solid_id: 9 });
    }
    if (req.method === "POST" && url === "/api/geometry/cylinder") {
      counts.cylinder++;
      // No embedded perception: the drill_pattern boolean that follows is
      // certified by the GET /perception fallback, which reads solid 9 as
      // UNSOUND — the halt under test.
      const n = counts.cylinder;
      return send({
        object: { id: `eeeeeeee-5555-4555-8555-${String(n).padStart(12, "0")}` },
        solid_id: 100 + n,
      });
    }
    if (req.method === "POST" && url === "/api/timeline/mould") {
      counts.mould++;
      return send(MOULD_REFUSAL, 409);
    }
    if (req.method === "GET" && url === "/api/scene/snapshot") {
      return send({
        objects: [
          { id: BASE_UUID, analytical_geometry: { solid_id: 7 } },
          { id: TOOL_UUID, analytical_geometry: { solid_id: 8 } },
          { id: BOX_UUID, analytical_geometry: { solid_id: 1 } },
        ],
      });
    }
    let m;
    if (req.method === "GET" && (m = url.match(/^\/api\/agent\/parts\/(\d+)\/perception$/))) {
      const unsound = m[1] === "9";
      return send({ valid: !unsound, watertight: !unsound, open_edges: unsound ? 4 : 0 });
    }
    if (req.method === "GET" && /^\/api\/agent\/parts\/\d+$/.test(url)) {
      return send({
        id: 7,
        topology: { face_count: 6 },
        volume: 1000,
        location: { center_world: [0, 0, 0], dimensions_world: [10, 10, 10] },
      });
    }
    if (req.method === "GET" && url === "/api/agent/parts") {
      return send([{ id: 1 }, { id: 7 }, { id: 8 }, { id: 9 }]);
    }
    if (req.method === "GET" && url === "/api/document/units") {
      return send({ unit: "mm" });
    }
    send({});
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
const { resetSessionGates } = await import(
  pathToFileURL(join(HERE, ".build", "gates.js")).href
);

const table = buildTable();
const call = (name, args) => table.get(name).handler(args, {});
const firstJson = (r) => JSON.parse(r.content[0].text);

let passed = 0;
let failed = 0;
const check = (label, fn) => {
  try {
    fn();
    passed++;
    console.log(`  ok - ${label}`);
  } catch (e) {
    failed++;
    console.log(`  not ok - ${label}\n      ${String(e?.message ?? e).split("\n").join("\n      ")}`);
  }
};

const boxOp = (w) => ({ tool: "create_box", args: { width: w, depth: 10, height: 5 } });

/** Fresh session gates + an open intent, opened DIRECTLY (not inside the
 *  program) so the op under test sits at the index the test names and the
 *  intent gate never supplies the refusal. */
async function openIntent(name) {
  resetSessionGates();
  const before = counts.checkpoint;
  const cp = await call("timeline_checkpoint", { name, branch: "main" });
  assert.notEqual(cp.isError, true, `checkpoint must open: ${cp.content?.[0]?.text}`);
  assert.equal(counts.checkpoint, before + 1, "checkpoint reached the backend");
}

// ─── 1. a refused timeline_mould stops the program AT its index ─────────────

console.log("1. refused timeline_mould inside a program");
await openIntent("mould proof — base plate 100 x 60 x 8");
{
  const before = { ...counts };
  const prog = await call("cad_program", {
    name: "mould-refusal",
    ops: [
      boxOp(10),
      { tool: "timeline_mould", args: { value: 12, target_event_id: EVENT_ID, parameter: "radius" } },
      boxOp(20),
    ],
  });
  const j = firstJson(prog);
  check("the mould reached the backend (the refusal is the backend's 409, not a gate)", () => {
    assert.equal(counts.mould, before.mould + 1);
  });
  check("the program stopped at index 1: op 2's box never reached the backend", () => {
    assert.equal(counts.box, before.box + 1, "exactly op 0's box ran");
    assert.equal(j.stopped_at, 1);
    assert.equal(j.completed, 1);
    assert.equal(j.ops.length, 2);
    assert.equal(j.ops[0].ok, true);
    assert.equal(j.ops[1].ok, false);
    assert.equal(j.ops[1].tool, "timeline_mould");
    assert.match(j.ops[1].error, /broken_downstream/);
  });
  check("the stopped program is reported as a FAILURE (isError, ledger in structuredContent)", () => {
    assert.equal(prog.isError, true);
    assert.equal(j.ok, false);
    assert.ok(prog.structuredContent, "structuredContent carries the ledger");
    assert.equal(prog.structuredContent.stopped_at, 1);
    assert.deepEqual(prog.structuredContent.ops, j.ops);
  });
}

// ─── 2. a halted boolean_many stops the program there ───────────────────────

console.log("2. halted boolean_many inside a program");
await openIntent("halt proof — cross bores in the 100 x 60 block");
{
  const before = { ...counts };
  const prog = await call("cad_program", {
    name: "halt",
    ops: [
      boxOp(10),
      { tool: "boolean_many", args: { op: "difference", base: BASE_UUID, tools: [TOOL_UUID] } },
      boxOp(30),
    ],
  });
  const j = firstJson(prog);
  check("the boolean reached the backend (a halt, not an unsound-base gate refusal)", () => {
    assert.equal(counts.boolean, before.boolean + 1);
  });
  check("the program stopped at the halt: op 2's box never reached the backend", () => {
    assert.equal(counts.box, before.box + 1, "exactly op 0's box ran");
    assert.equal(j.stopped_at, 1);
    assert.equal(j.completed, 1);
    assert.equal(j.ops[1].ok, false);
    assert.equal(j.ops[1].tool, "boolean_many");
    assert.match(j.ops[1].error, /UNSOUND/);
  });
  check("the stopped program is an error result", () => {
    assert.equal(prog.isError, true);
    assert.equal(prog.structuredContent?.stopped_at, 1);
  });
}

// ─── 3. an all-ok program is not an error ───────────────────────────────────

console.log("3. all-ok program");
await openIntent("clean proof — two stacked blocks 10 x 10 x 5");
{
  const before = { ...counts };
  const prog = await call("cad_program", { name: "clean", ops: [boxOp(10), boxOp(11)] });
  const j = firstJson(prog);
  check("an all-ok program completes and is NOT an error", () => {
    assert.equal(counts.box, before.box + 2);
    assert.equal(j.ok, true);
    assert.equal(j.stopped_at, null);
    assert.notEqual(prog.isError, true);
  });
}

// ─── 4. a stopped program is never served from the refusal cache ────────────

console.log("4. re-issuing a stopped program re-runs its prefix");
await openIntent("cache proof — bolt circle 6 x D12 on D40 B.C.");
{
  // drill_pattern's up-front spacing guard answers with a `REFUSED:` text
  // error — the exact marker the refusal cache recognises in an error result.
  const program = {
    name: "cache",
    ops: [
      boxOp(40),
      {
        tool: "drill_pattern",
        args: { object: BASE_UUID, count: 12, ring_r: 5, hole_r: 3, depth: 20 },
      },
    ],
  };
  const b0 = counts.box;
  const first = await call("cad_program", program);
  const second = await call("cad_program", program);
  check("both issues stopped at the refused drill_pattern", () => {
    assert.equal(firstJson(first).stopped_at, 1);
    assert.equal(firstJson(second).stopped_at, 1);
    assert.match(firstJson(first).ops[1].error, /REFUSED/);
  });
  check("the second issue EXECUTED its prefix again (not a cached refusal)", () => {
    assert.equal(counts.box, b0 + 2, "op 0's box ran once per issue");
    assert.doesNotMatch(second.content[0].text, /\[refusal cache\]/);
  });
}

// ─── 5. a halt still arms gate 6 (the applied prefix is unverified work) ────

console.log("5. halted boolean_many is unverified work");
await openIntent("gate-6 proof — cross bore D8 through the web");
{
  const halted = await call("boolean_many", {
    op: "difference",
    base: BASE_UUID,
    tools: [TOOL_UUID],
  });
  check("boolean_many's halt is an error result carrying the typed halt", () => {
    assert.equal(halted.isError, true);
    const j = firstJson(halted);
    assert.match(j.halted, /step 1 .* UNSOUND/);
    assert.equal(j.completed, 1);
    assert.equal(halted.structuredContent?.halted, j.halted);
  });
  const before = counts.checkpoint;
  const close = await call("timeline_checkpoint", {
    name: "next feature — chamfer 1 x 45 on the top edges",
    branch: "main",
  });
  check("closing the intent is refused by verification_scope: the halted prefix is live", () => {
    assert.equal(close.isError, true);
    assert.equal(firstJson(close).gate, "verification_scope");
    assert.equal(counts.checkpoint, before, "the close never reached the backend");
  });
}

// ─── 5b. drill_pattern's halt is an error result too ────────────────────────

console.log("5b. halted drill_pattern");
await openIntent("drill halt proof — 2 x D6 on D40 B.C.");
{
  const before = { ...counts };
  // Spacing 2·20·sin(π/2) = 40 > 2·3 — clears the up-front overlap guard,
  // so the stop is the per-hole UNSOUND halt, not the spacing refusal.
  // A direct table call skips zod, so the schema defaults are passed here.
  const r = await call("drill_pattern", {
    object: BASE_UUID,
    plane: "xy",
    cx: 0,
    cy: 0,
    z_offset: -1,
    start_angle_deg: 0,
    count: 2,
    ring_r: 20,
    hole_r: 3,
    depth: 20,
  });
  check("drill_pattern reached the per-hole boolean (not the spacing guard)", () => {
    assert.equal(counts.cylinder, before.cylinder + 2, "both bores created");
    assert.equal(counts.boolean, before.boolean + 1, "halted after the first hole");
  });
  check("drill_pattern's halt is an error result carrying the typed halt", () => {
    assert.equal(r.isError, true);
    const j = firstJson(r);
    assert.match(j.halted, /hole 1 .* UNSOUND/);
    assert.equal(j.holes_completed, 1);
    assert.equal(r.structuredContent?.halted, j.halted);
  });
}

// ─── 6. direct refusals are error results carrying the typed refusal ───────

console.log("6. direct refusals");
await openIntent("direct refusal proof — boss D20 x 12");
{
  const r = await call("timeline_mould", {
    value: 12,
    target_event_id: EVENT_ID,
    parameter: "radius",
    branch: "main",
  });
  check("timeline_mould: isError, refused body in text AND structuredContent", () => {
    assert.equal(r.isError, true);
    assert.deepEqual(firstJson(r).refused, MOULD_REFUSAL);
    assert.deepEqual(r.structuredContent?.refused, MOULD_REFUSAL);
  });
}
{
  const r = await call("ask_choice", {
    question: "Which fit?",
    options: [{ value: "H7/g6" }, { value: "H7/g6" }],
  });
  check("ask_choice: a malformed option set is an error result", () => {
    assert.equal(r.isError, true);
    assert.equal(firstJson(r).refused, true);
    assert.equal(r.structuredContent?.refused, true);
  });
}
{
  const r = await call("kb_lookup", { kind: "pack", key: "laser_sintering_magic" });
  check("kb_lookup: an unknown pack is an error result", () => {
    assert.equal(r.isError, true);
    assert.equal(firstJson(r).refused, true);
    assert.equal(r.structuredContent?.refused, true);
  });
}
{
  const r = await call("kb_lookup", { kind: "pack", key: "fdm" });
  check("kb_lookup: a known pack is NOT an error", () => {
    assert.notEqual(r.isError, true);
    assert.equal(firstJson(r).refused, undefined);
  });
}
{
  const r = await call("kb_lookup", { kind: "playbook", key: "no_such_feature" });
  check("kb_lookup: an unknown playbook is an error result", () => {
    assert.equal(r.isError, true);
    assert.equal(firstJson(r).refused, true);
    assert.equal(r.structuredContent?.refused, true);
  });
}
{
  const r = await call("kb_lookup", { kind: "reference", key: "no_such_function" });
  check("kb_lookup: an unknown reference function is an error result", () => {
    assert.equal(r.isError, true);
    assert.equal(firstJson(r).refused, true);
    assert.ok(Array.isArray(firstJson(r).valid_keys));
    assert.equal(r.structuredContent?.refused, true);
  });
}
{
  const r = await call("kb_lookup", {
    kind: "reference",
    key: "clearance_hole",
    args: { fastener: "M6", class: "close" },
  });
  check("kb_lookup: a resolved reference lookup is NOT an error", () => {
    assert.notEqual(r.isError, true);
    assert.equal(firstJson(r).refused, undefined);
    assert.ok("value" in firstJson(r));
  });
}

stub.close();
if (failed > 0) {
  console.log(`\nrefusal_stops_program: ${failed} FAILED, ${passed} passed`);
  process.exit(1);
}
console.log(`\nrefusal_stops_program: ${passed} checks passed`);
