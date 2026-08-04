/**
 * Durability disclosure proof (2026-08-04) — the MCP half of the #39
 * follow-up: `/api/durability/status` and `manifest.durability` (the
 * evidence pack) already report a QUARANTINED document honestly, but
 * nothing on the agent-facing reads (`/api/agent/parts/{id}/perception`,
 * `/api/timeline/history/{branch}`) carried it, and — even after the
 * backend was fixed to carry it — a naive MCP wrapper would silently drop
 * the new field while mapping the response onto its own shape. This is
 * exactly the defect one layer up.
 *
 * Exercises the REAL dispatch modules (ToolTable wrapper + core.ts api() +
 * the real tool handlers, compiled from src to test/.build) against a local
 * stub backend that serves the SAME two response shapes the real backend
 * now serves on a quarantined document:
 *   - `GET /api/agent/parts/{id}/perception[?full=1]` → adds a `durability`
 *     object (never a bare bool).
 *   - `GET /api/timeline/history/{branch}` → the SHAPE changes from a bare
 *     array to `{events, durability}`.
 *
 * Proves:
 *   1. `verify_part` (the explicit `/perception?full=1` wrapper) surfaces
 *      `durability` in its structured JSON, unrewritten.
 *   2. `timeline_history` (the `/timeline/history` wrapper) still returns
 *      every event AND surfaces `durability` — the old
 *      `(Array.isArray(r) ? r : [])` mapping would have silently dropped
 *      both on a quarantined document (the object shape is not an array).
 *   3. The ambient ombudsman path (`compactVerdict`, what every mutating
 *      tool's default response line uses) loudly prefixes the quarantine
 *      note when the perception fetch it was built from carried one.
 *   4. On an UNQUARANTINED document (no `durability` field served at all),
 *      none of the above surface a `durability` key or note — additive,
 *      not noise.
 *
 *   Build the fixture first (never touches dist/):
 *     npx tsc -p tsconfig.json --outDir test/.build
 *   Run:
 *     node test/durability_disclosure.test.mjs
 */

import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const UUID = "11111111-2222-4333-8444-555555555555";

const QUARANTINE = {
  state: "quarantined",
  first_break_sequence: 4,
  first_break_kind: "quarantine_probe_unknown_op",
  reason: "the current kernel cannot replay this event kind",
  events_served: 4,
  events_total: 5,
};

// ─── Stub backend: toggled between quarantined / clean by a flag ──────────

let quarantined = true;

const HISTORY_EVENTS = [
  {
    id: "e1",
    sequence_number: 0,
    timestamp: "2026-08-04T00:00:00Z",
    operation_type: "create_box_3d",
    operation: { type: "CreatePrimitive" },
    author: "Claude",
    author_kind: "ai",
    affected_parts: ["solid:7"],
  },
];

const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  const send = (obj) => {
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify(obj));
  };
  // Force flowing mode so `end` fires even for a bodyless GET.
  req.on("data", () => {});
  req.on("end", () => {
    if (req.method === "GET" && url === "/api/scene/snapshot") {
      return send({
        objects: [{ id: UUID, analytical_geometry: { solid_id: 7 } }],
      });
    }
    if (req.method === "GET" && url.startsWith("/api/agent/parts/7/perception")) {
      const base = {
        solid_id: 7,
        sound: true,
        status: "sound",
        verified: true,
        verdict: "SOUND — full kernel certificate clean",
        valid: true,
        watertight: true,
        open_edges: 0,
        nonmanifold_edges: 0,
        dims: [5, 5, 5],
        cert: { sound: true, brep_valid: true, watertight: true },
        reconcile: { status: "pending" },
      };
      return send(quarantined ? { ...base, durability: QUARANTINE } : base);
    }
    if (req.method === "GET" && url.startsWith("/api/agent/parts/7/render")) {
      return send({ png_base64: "AAAA", open_edges: 0, nonmanifold_edges: 0 });
    }
    if (req.method === "GET" && url.startsWith("/api/timeline/history/main")) {
      return send(
        quarantined
          ? { events: HISTORY_EVENTS, durability: QUARANTINE }
          : HISTORY_EVENTS,
      );
    }
    if (req.method === "GET" && url === "/api/document/units") {
      return send({ unit: "mm" });
    }
    res.writeHead(404, { "Content-Type": "application/json" });
    res.end("{}");
  });
});

stub.listen(0, "127.0.0.1");
await once(stub, "listening");
const port = stub.address().port;

process.env.ROSHERA_URL = `http://127.0.0.1:${port}`;
process.env.ROSHERA_MCP_AUTOVERIFY = "1";
process.env.ROSHERA_AMBIENT_PERCEPTION = "compact";

const core = await import(pathToFileURL(join(HERE, ".build", "core.js")).href);
const { buildTable } = await import(
  pathToFileURL(join(HERE, ".build", "surface.js")).href
);

const table = buildTable();
const call = (name, args) => table.get(name).handler(args, {});

let passed = 0;
const check = (label, fn) => {
  fn();
  passed++;
  console.log(`  ok - ${label}`);
};

const textOf = (result) =>
  result.content
    .filter((c) => c.type === "text")
    .map((c) => c.text)
    .join("\n");

// ─── 1. verify_part surfaces durability on a quarantined document ─────────

quarantined = true;
{
  const r = await call("verify_part", { part_id: 7, view: "iso" });
  check("verify_part is not an error", () => {
    assert.notEqual(r.isError, true, textOf(r));
  });
  const parsed = JSON.parse(textOf(r));
  check("verify_part surfaces durability.state = quarantined", () => {
    assert.equal(parsed.durability?.state, "quarantined", JSON.stringify(parsed));
  });
  check("verify_part's durability names the offending event kind", () => {
    assert.equal(
      parsed.durability?.first_break_kind,
      "quarantine_probe_unknown_op",
      JSON.stringify(parsed),
    );
  });
  check("verify_part's part-level verdict is UNCHANGED (still sound)", () => {
    assert.equal(parsed.sound, true, JSON.stringify(parsed));
  });
}

// ─── 2. timeline_history surfaces durability + still returns events ───────

{
  const r = await call("timeline_history", {
    branch: "main",
    start: 0,
    limit: 100,
    include_operations: false,
  });
  check("timeline_history is not an error", () => {
    assert.notEqual(r.isError, true, textOf(r));
  });
  const parsed = JSON.parse(textOf(r));
  check("timeline_history still returns the clean-prefix events (not silently emptied)", () => {
    assert.equal(parsed.count, 1, JSON.stringify(parsed));
    assert.equal(parsed.events.length, 1, JSON.stringify(parsed));
    assert.equal(parsed.events[0].id, "e1", JSON.stringify(parsed));
  });
  check("timeline_history surfaces durability.state = quarantined", () => {
    assert.equal(parsed.durability?.state, "quarantined", JSON.stringify(parsed));
  });
}

// ─── 3. ambient compact verdict loudly notes the quarantine ────────────────

{
  const perception = await core.perceive(7);
  check("ambient perceive() carries durability through from GET /perception", () => {
    assert.equal(perception?.durability?.state, "quarantined", JSON.stringify(perception));
  });
  const line = core.compactVerdict(perception);
  check("compactVerdict prefixes a loud DOCUMENT QUARANTINED note", () => {
    assert.match(line, /^⚠ DOCUMENT QUARANTINED/, line);
  });
  check("compactVerdict does NOT drop the part's own SOUND verdict", () => {
    assert.match(line, /SOUND/, line);
  });
}

// ─── 4. clean document: no durability noise anywhere ───────────────────────

quarantined = false;
{
  const r = await call("verify_part", { part_id: 7, view: "iso" });
  const parsed = JSON.parse(textOf(r));
  check("verify_part on a clean document carries no durability key", () => {
    assert.equal(
      Object.prototype.hasOwnProperty.call(parsed, "durability"),
      false,
      JSON.stringify(parsed),
    );
  });
}
{
  const r = await call("timeline_history", {
    branch: "main",
    start: 0,
    limit: 100,
    include_operations: false,
  });
  const parsed = JSON.parse(textOf(r));
  check("timeline_history on a clean document carries no durability key", () => {
    assert.equal(
      Object.prototype.hasOwnProperty.call(parsed, "durability"),
      false,
      JSON.stringify(parsed),
    );
  });
}
{
  const perception = await core.perceive(7);
  const line = core.compactVerdict(perception);
  check("compactVerdict on a clean document carries no quarantine prefix", () => {
    assert.ok(!line.startsWith("⚠ DOCUMENT QUARANTINED"), line);
  });
}

stub.close();
console.log(`\ndurability_disclosure: ${passed} checks passed`);
