/**
 * Constraint-gate proof (2026-08-01) — exercises the REAL dispatch modules
 * (ToolTable wrapper + gates.ts + the real tool handlers, compiled from src
 * to test/.build) against a local stub backend, so every assertion runs the
 * exact code path a live call runs, minus only the Rust kernel.
 *
 *   Build the fixture first (never touches dist/):
 *     npx tsc -p tsconfig.json --outDir test/.build
 *   Run:
 *     node test/constraint_gates.test.mjs
 *
 * Proves:
 *   1. refusal cache — identical re-issue of a refused call returns the SAME
 *      refusal from cache (kernel untouched); different args do NOT collide;
 *      any state-changing success drops the cache.
 *   2. intent gate — a solid-mutating call with no open checkpoint is refused
 *      (kernel untouched); a generic checkpoint name is refused; a real
 *      intent phrase opens the gate and auto-writes the notebook line.
 *   3. unsound-base gate — a mutating op on a sound==false base is refused
 *      without acknowledge_unsound:true, proceeds with it (through the
 *      invoke funnel, proving the flag survives schema validation), is NEVER
 *      served from cache (re-issues re-read the live verdict), and proceeds
 *      the moment the live verdict flips sound.
 */

import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const UUID = "11111111-2222-4333-8444-555555555555";

// ─── Stub backend ───────────────────────────────────────────────────────────

const counts = {
  snapshot: 0,
  perception: 0,
  shell: 0,
  checkpoint: 0,
  blackboard: 0,
};
let partSound = false;

const stub = http.createServer((req, res) => {
  const url = req.url ?? "";
  const send = (obj) => {
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify(obj));
  };
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    if (req.method === "GET" && url === "/api/scene/snapshot") {
      counts.snapshot++;
      return send({
        objects: [{ id: UUID, analytical_geometry: { solid_id: 7 } }],
      });
    }
    if (req.method === "GET" && url === "/api/agent/parts/7/perception") {
      counts.perception++;
      return partSound
        ? send({ valid: true, watertight: true, open_edges: 0 })
        : send({
            valid: false,
            watertight: false,
            open_edges: 3,
            verdict: "UNSOUND — see verify_part",
          });
    }
    if (req.method === "GET" && url === "/api/agent/parts/7") {
      return send({
        id: 7,
        topology: { face_count: 6 },
        volume: 1000,
        location: { center_world: [0, 0, 0], dimensions_world: [1, 1, 1] },
      });
    }
    if (req.method === "GET" && url === "/api/agent/parts") {
      return send([{ id: 7 }]);
    }
    if (req.method === "GET" && url === "/api/document/units") {
      return send({ unit: "mm" });
    }
    if (req.method === "POST" && url === "/api/geometry/shell") {
      counts.shell++;
      return send({
        object: { id: UUID },
        solid_id: 7,
        stats: { triangle_count: 12 },
        valid: true,
        watertight: true,
      });
    }
    if (req.method === "POST" && url === "/api/timeline/checkpoint") {
      counts.checkpoint++;
      return send({ id: "cp-1", name: JSON.parse(body || "{}").name });
    }
    if (req.method === "POST" && url === "/api/blackboard/entries") {
      counts.blackboard++;
      return send({
        id: "bb-1",
        text: JSON.parse(body || "{}").text,
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
const isRefusal = (r, gate) => {
  const j = firstJson(r);
  return j.refused === true && (gate === undefined || j.gate === gate);
};

let passed = 0;
const check = (label, fn) => {
  fn();
  passed++;
  console.log(`  ok - ${label}`);
};

// ─── 2. intent gate (also seeds the refusal cache for test 1) ───────────────

const shellArgs = { object: UUID, thickness: 2, faces_to_remove: [] };

const r1 = await call("shell", shellArgs);
check("mutating call with no open checkpoint is refused (gate: intent)", () => {
  assert.ok(isRefusal(r1, "intent"));
  assert.equal(r1.isError, true);
  assert.equal(counts.shell, 0, "kernel was never hit");
});

// ─── 1. refusal cache ───────────────────────────────────────────────────────

const r2 = await call("shell", shellArgs);
check("identical re-issue returns the SAME refusal, from cache", () => {
  assert.equal(r2.content[0].text, r1.content[0].text, "byte-identical refusal");
  assert.equal(r2.content.length, 2, "cache note appended");
  assert.match(r2.content[1].text, /refusal cache/);
  assert.equal(counts.shell + counts.snapshot + counts.perception, 0);
});

const r3 = await call("shell", { ...shellArgs, thickness: 3 });
check("different args do NOT collide with the cached refusal", () => {
  assert.ok(isRefusal(r3, "intent"));
  assert.equal(r3.content.length, 1, "fresh refusal, no cache note");
});

const k1 = await call("kb_lookup", {
  kind: "reference",
  key: "clearance_hole",
  args: { fastener: "M8" },
});
const k2 = await call("kb_lookup", {
  kind: "reference",
  key: "clearance_hole",
  args: { fastener: "M8" },
});
const k3 = await call("kb_lookup", {
  kind: "reference",
  key: "clearance_hole",
  args: { fastener: "M8", class: "close" },
});
check("kb_lookup refusal cached on identical re-issue; answered args pass", () => {
  assert.equal(firstJson(k1).refused, true);
  assert.equal(k2.content[0].text, k1.content[0].text);
  assert.match(k2.content[1].text, /refusal cache/);
  assert.equal(firstJson(k3).refused, undefined);
  assert.equal(firstJson(k3).value.diameter_mm, 8.4);
});

// ─── 2b. checkpoint name quality + auto notebook mirror ─────────────────────

for (const bad of ["step 3", "cp 2", "7", "checkpoint"]) {
  const r = await call("timeline_checkpoint", { name: bad, branch: "main" });
  check(`generic checkpoint name '${bad}' is refused`, () => {
    assert.ok(isRefusal(r, "intent"));
    assert.equal(counts.checkpoint, 0);
  });
}

const cp = await call("timeline_checkpoint", {
  name: "verification cube 5 mm — dispatch-gate proof",
  branch: "main",
});
check("real intent phrase records the checkpoint + notebook line", () => {
  const j = firstJson(cp);
  assert.equal(j.checkpoint.id, "cp-1");
  assert.equal(j.notebook_entry.id, "bb-1");
  assert.equal(counts.checkpoint, 1);
  assert.equal(counts.blackboard, 1);
});

// ─── 3. unsound-base gate ───────────────────────────────────────────────────

const r4 = await call("shell", shellArgs);
check("checkpoint success cleared the cache; unsound base now refused live", () => {
  assert.ok(isRefusal(r4, "unsound_base"));
  assert.equal(r4.content.length, 1, "not a cache replay");
  assert.equal(counts.perception, 1, "live verdict was read");
  assert.equal(counts.shell, 0, "kernel op never ran");
  assert.match(firstJson(r4).reason, /part 7 is UNSOUND/);
});

const r5 = await call("shell", shellArgs);
check("unsound_base refusals are never cached — re-issue re-reads live state", () => {
  assert.ok(isRefusal(r5, "unsound_base"));
  assert.equal(r5.content.length, 1);
  assert.equal(counts.perception, 2, "verdict fetched again");
});

const inv = await call("invoke", {
  name: "shell",
  args: { ...shellArgs, acknowledge_unsound: true },
});
check("acknowledge_unsound:true proceeds (via invoke — flag survives schema)", () => {
  assert.equal(firstJson(inv).refused, undefined);
  assert.equal(counts.shell, 1, "kernel op ran exactly once");
});

partSound = true;
const r6 = await call("shell", shellArgs);
check("repaired base (live verdict sound) proceeds without the flag", () => {
  assert.equal(firstJson(r6).refused, undefined);
  assert.equal(counts.shell, 2);
});

stub.close();
console.log(`\nconstraint_gates: ${passed} checks passed`);
