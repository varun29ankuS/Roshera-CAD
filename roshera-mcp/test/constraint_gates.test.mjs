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
// A SECOND object/part, dedicated to the gate-3 fail-open (S4) tests below —
// isolated from UUID/part 7 so toggling its snapshot/perception failure mode
// cannot perturb any assertion that already runs against part 7.
const UUID2 = "66666666-7777-4888-8999-aaaaaaaaaaaa";

// ─── Stub backend ───────────────────────────────────────────────────────────

const counts = {
  snapshot: 0,
  perception: 0,
  shell: 0,
  checkpoint: 0,
  blackboard: 0,
  perception2: 0,
};
let partSound = false;
// Gate-3 fail-open (S4) controls for UUID2/part 42. "ok" leaves both fetches
// answering normally; "fail" answers with a 404 (so the pre-flight fetch
// THROWS, exercising the S4 catch paths — gates.ts:485-487/507-509 before
// this item, now the `unavailable` arms of partIdForUuid/liveVerdict);
// "hang" never responds, so the client's own AbortSignal.timeout fires
// (ROSHERA_MCP_PERCEPTION_TIMEOUT_MS is set below, before the fixture is
// imported, so this resolves in well under a second).
let snapshotMode = "ok"; // "ok" | "fail" | "hang"
let perceptionMode2 = "ok"; // "ok" | "fail" | "hang" — governs part 42 only

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
      if (snapshotMode === "hang") return; // never respond — client times out
      if (snapshotMode === "fail") {
        res.writeHead(404, { "Content-Type": "application/json" });
        return res.end(JSON.stringify({ error: "no such document" }));
      }
      return send({
        objects: [
          { id: UUID, analytical_geometry: { solid_id: 7 } },
          { id: UUID2, analytical_geometry: { solid_id: 42 } },
        ],
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
    if (req.method === "GET" && url === "/api/agent/parts/42/perception") {
      counts.perception2++;
      if (perceptionMode2 === "hang") return; // never respond — client times out
      if (perceptionMode2 === "fail") {
        res.writeHead(404, { "Content-Type": "application/json" });
        return res.end(JSON.stringify({ error: "no such part" }));
      }
      return send({ valid: true, watertight: true, open_edges: 0 });
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
// PERCEPTION_TIMEOUT_MS is also read at core.js module load. Lowered so the
// "hang" fail-open tests below resolve in well under a second instead of
// waiting out the production 4000ms default.
process.env.ROSHERA_MCP_PERCEPTION_TIMEOUT_MS = "200";

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

// The refusal probe used to be `clearance_hole` M8 with no class — that was a
// genuine refusal until §8-Q6 was DECIDED (`105607ed`, 2026-08-02): the house
// now answers ISO 273 medium and says whose answer it is. The cache assertion
// still needs a live refusal, so it moved to a fastener genuinely outside the
// transcribed table — a named gap, which is the refusal that is meant to
// survive. Pinning the decided case is a separate check below, so this file
// cannot go stale the same way twice.
const k1 = await call("kb_lookup", {
  kind: "reference",
  key: "clearance_hole",
  args: { fastener: "M99" },
});
const k2 = await call("kb_lookup", {
  kind: "reference",
  key: "clearance_hole",
  args: { fastener: "M99" },
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
  assert.equal(firstJson(k3).value.class_source, "explicit");
});

// §8-Q6 is decided, and the answer says whose answer it is. An unclassed
// lookup must ANSWER (not refuse) and must mark the class as the house's
// choice, never the engineer's — a bare diameter cannot tell those apart.
const k4 = await call("kb_lookup", {
  kind: "reference",
  key: "clearance_hole",
  args: { fastener: "M8" },
});
check("unclassed clearance_hole answers ISO 273 medium and attributes the choice", () => {
  assert.equal(firstJson(k4).refused, undefined);
  assert.equal(firstJson(k4).value.diameter_mm, 9.0);
  assert.equal(firstJson(k4).value.class, "medium");
  assert.equal(firstJson(k4).value.series, "medium");
  assert.equal(
    firstJson(k4).value.class_source,
    "house_default",
    "an unrequested class must be attributed to the house, never read as the engineer's decision",
  );
});

// ─── 2b. checkpoint name quality + auto notebook mirror ─────────────────────

// The last five are clock/date readings dressed as names — the shape the
// UI button used to mint ("Checkpoint 9:59:36 PM"). They slip the generic
// regex (its tail only accepts a plain ordinal), so without the dedicated
// CLOCK_CHECKPOINT_NAME rule these five would reach the backend and this
// loop would fail on counts.checkpoint staying 0.
for (const bad of [
  "step 3",
  "cp 2",
  "7",
  "checkpoint",
  "Checkpoint 9:59:36 PM",
  "checkpoint 9:59",
  "10:05",
  "2026-08-01",
  "cp 12/31/26",
]) {
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
  // A result with a COMPLETE pre-flight must be unchanged — no key added, no
  // shape change (item 1's own constraint). Pinned here, on a call that ran
  // through the real gate 3 loop and found the base genuinely sound, so a
  // regression that started stamping `gate_preflight` on every proceed (not
  // only a fail-open one) would be caught here, not only on the new tests
  // below that expect the marker.
  assert.deepEqual(
    Object.keys(firstJson(r6)).sort(),
    ["object_uuid", "part_id", "perception", "placement", "triangles"].sort(),
    "no gate_preflight (or any other new key) on a call whose pre-flight completed",
  );
});

// ─── S4 — gate 3's fail-open, made visible (item 1) ─────────────────────────
//
// Two independent skip paths, each proven separately, per the brief:
//  - partIdForUuid's live fetch fails → the ref cannot even be RESOLVED
//    (gates.ts's `resolve` stage) — the op still proceeds (fail-open stays
//    open) and its result now carries `gate_preflight: "unavailable"` naming
//    the ref and the underlying error.
//  - liveVerdict's live fetch fails → the ref resolves but its verdict
//    cannot be READ (`verify` stage) — same proceed, same marker, and the
//    reason text is provably DIFFERENT (a timeout vs a 404), so a reader can
//    tell them apart rather than seeing an undifferentiated "unavailable".
// UUID2/part 42 is used throughout so `counts.shell` / `counts.perception`
// from the part-7 tests above are never touched by these.

const shellArgs2 = { object: UUID2, thickness: 2, faces_to_remove: [] };

snapshotMode = "fail"; // GET /api/scene/snapshot → 404, so the resolve step throws
const shellBeforeResolveFail = counts.shell;
const r7 = await call("shell", shellArgs2);
check("resolve-stage fail-open (404 on snapshot) still lets the op proceed", () => {
  assert.equal(firstJson(r7).refused, undefined, "the fail-open must REMAIN open — never a refusal");
  assert.equal(counts.shell, shellBeforeResolveFail + 1, "the kernel op actually ran");
});
check("resolve-stage fail-open is now MARKED, naming the ref and the 404", () => {
  const j = firstJson(r7);
  assert.equal(j.gate_preflight, "unavailable");
  assert.equal(j.gate_preflight_gaps.length, 1);
  assert.equal(j.gate_preflight_gaps[0].ref, UUID2);
  assert.equal(j.gate_preflight_gaps[0].stage, "resolve");
  assert.match(j.gate_preflight_gaps[0].reason, /404/, "the reason names the 404, not a generic label");
});
snapshotMode = "ok";

perceptionMode2 = "hang"; // resolves fine (part 42); the perception read times out
const perception2Before = counts.perception2;
const shellBeforeVerifyFail = counts.shell;
const r8 = await call("shell", shellArgs2);
check("verify-stage fail-open (perception timeout) still lets the op proceed", () => {
  assert.equal(firstJson(r8).refused, undefined, "the fail-open must REMAIN open — never a refusal");
  assert.equal(counts.shell, shellBeforeVerifyFail + 1, "the kernel op actually ran");
  assert.equal(counts.perception2, perception2Before + 1, "the live fetch really was attempted");
});
check("verify-stage fail-open is MARKED, and the reason names a TIMEOUT — distinct from the 404 above", () => {
  const j = firstJson(r8);
  assert.equal(j.gate_preflight, "unavailable");
  assert.equal(j.gate_preflight_gaps.length, 1);
  assert.equal(j.gate_preflight_gaps[0].ref, UUID2);
  assert.equal(j.gate_preflight_gaps[0].stage, "verify");
  assert.match(j.gate_preflight_gaps[0].reason, /timed out after 200ms/);
  assert.doesNotMatch(j.gate_preflight_gaps[0].reason, /404/,
    "a timeout must not read the same as the 404 case — a reader tells them apart");
});
perceptionMode2 = "ok";

const r9 = await call("shell", shellArgs2);
check("once both fetches answer again, the SAME base ref proceeds with no marker at all", () => {
  const j = firstJson(r9);
  assert.equal(j.refused, undefined);
  assert.equal(j.gate_preflight, undefined, "absent marker means the gate ran — must stay true");
  assert.equal(j.gate_preflight_gaps, undefined);
});

stub.close();
// Two of the checks above deliberately leave a request unanswered (the
// "hang" fail-open modes) so the CLIENT's own timeout fires; the server side
// of that socket is otherwise left dangling. Force it closed so the process
// exits promptly instead of waiting on a connection nothing will ever answer.
stub.closeAllConnections();
console.log(`\nconstraint_gates: ${passed} checks passed`);
