/**
 * Verification-scope gate proof (2026-08-11, task #9 half B) —
 * CONSTRAINT BEATS STEERING.
 *
 * The intent gate already forces a design intent to be DECLARED before any
 * geometry is built. Nothing forced anyone to LOOK at what came out. Measured
 * in this repo: a loft shipped CERTIFIED SOUND carrying a 9.97% shape error,
 * because soundness is a statement about topology and says nothing about
 * whether the result is the geometry that was asked for. "Check what you
 * built" is exactly the kind of policy sentence a model can decline, so it is
 * converted here into a refusal at the same dispatch choke point.
 *
 * Exercises the REAL dispatch modules (ToolTable wrapper + gates.ts + real
 * handlers, compiled from src to test/.build) against a local stub backend.
 *
 *   Build the fixture first (never touches dist/):
 *     npx tsc -p tsconfig.json --outDir test/.build
 *   Run:
 *     node test/verification_scope_gate.test.mjs
 *
 * Proves:
 *   1. the FIRST checkpoint is never gated (nothing was built under nothing);
 *   2. a checkpoint that CLOSES an intent with unverified mutating work is
 *      refused TYPED (gate: verification_scope), naming what was built and
 *      both verification verbs — and the backend never sees the call;
 *   3. verify_part clears it: the IDENTICAL closing call then proceeds, which
 *      also proves the refusal is never served from the refusal cache;
 *   4. verify_claim clears it too;
 *   5. skip_verification:true is the one escape, and it is explicit;
 *   6. a checkpoint with NO mutating work under it closes freely;
 *   7. a verify that ran BEFORE the mutation does NOT count — the geometry
 *      built after it was still never inspected;
 *   8. a read that merely REPORTS (get_part / list_parts / mass_properties)
 *      is not a verification and does not clear the gate;
 *   9. clear_timeline is deliberately out of scope: wiping the ledger the work
 *      lives in must not be nagged;
 *  10. the generic-name gate still fires first and independently.
 */

import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const PART_UUID = "cccccccc-3333-4333-8333-333333333333";

// ─── Stub backend ───────────────────────────────────────────────────────────

const counts = {
  checkpoint: 0, // POST /api/timeline/checkpoint
  box: 0, //        POST /api/geometry/box
  cylinder: 0, //   POST /api/geometry/cylinder
  clear: 0, //      DELETE /api/geometry
};

const stub = http.createServer((req, res) => {
  const url = (req.url ?? "").split("?")[0];
  const send = (obj) => {
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify(obj));
  };
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    if (req.method === "POST" && url === "/api/timeline/checkpoint") {
      counts.checkpoint++;
      const parsed = body ? JSON.parse(body) : {};
      // The gate's escape hatch is MCP-side session policy; the backend has no
      // notion of it and must never be sent one.
      assert.equal(
        parsed.skip_verification,
        undefined,
        "skip_verification must never reach the backend",
      );
      return send({ id: `cp-${counts.checkpoint}`, name: parsed.name });
    }
    if (req.method === "POST" && url === "/api/blackboard/entries") {
      return send({ id: "nb-1" });
    }
    if (req.method === "POST" && url === "/api/geometry/box") {
      counts.box++;
      return send({
        solid_id: 1,
        object: { id: PART_UUID },
        stats: { triangle_count: 12 },
        perception: { sound: true },
      });
    }
    if (req.method === "POST" && url === "/api/geometry/cylinder") {
      counts.cylinder++;
      return send({
        solid_id: 2,
        object: { id: PART_UUID },
        stats: { triangle_count: 96 },
        perception: { sound: true },
      });
    }
    if (req.method === "DELETE" && url === "/api/geometry") {
      counts.clear++;
      return send({ ok: true, deleted: 2 });
    }
    if (req.method === "GET" && url === "/api/agent/parts") {
      return send([{ part_id: 1, name: "Box 1" }]);
    }
    if (req.method === "GET" && /^\/api\/agent\/parts\/\d+$/.test(url)) {
      return send({ part_id: 1, name: "Box 1", volume: 8.0 });
    }
    if (req.method === "GET" && /^\/api\/agent\/parts\/\d+\/perception$/.test(url)) {
      return send({ sound: true, verdict: "SOUND", valid: true });
    }
    if (req.method === "GET" && /^\/api\/agent\/parts\/\d+\/mass/.test(url)) {
      return send({ volume: 8.0, surface_area: 24.0 });
    }
    if (req.method === "GET" && url === "/api/scene/snapshot") {
      return send({
        objects: [{ id: PART_UUID, analytical_geometry: { solid_id: 1 } }],
      });
    }
    if (req.method === "GET" && url === "/api/document/units") {
      return send({ unit: "mm" });
    }
    // Everything else (render/verify extras) answers benignly.
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
const firstJson = (r) => {
  try {
    return JSON.parse(r.content[0].text);
  } catch {
    return {};
  }
};
const isRefusal = (r, gate) => {
  const j = firstJson(r);
  return j.refused === true && (gate === undefined || j.gate === gate);
};
const checkpoint = (name, extra) => call("timeline_checkpoint", { name, ...extra });

let passed = 0;
const check = (label, fn) => {
  fn();
  passed++;
  console.log(`  ok - ${label}`);
};

// ─── 1. the first checkpoint is never gated ─────────────────────────────────

resetSessionGates();
const cp1 = await checkpoint("boss ø40 x 12 tall on the base plate");
check("the first checkpoint opens freely — nothing was built under nothing", () => {
  assert.equal(firstJson(cp1).refused, undefined);
  assert.equal(counts.checkpoint, 1, "it reached the backend");
});

// ─── 2. closing over unverified work is refused typed ───────────────────────

await call("create_box", { plane: "xy", cx: 0, cy: 0, width: 2, depth: 2, height: 2 });
const beforeCheckpoints = counts.checkpoint;
const cp2 = await checkpoint("M8 clearance holes, close fit, 4x base corners");
check("closing an intent over unverified geometry is refused (verification_scope)", () => {
  assert.ok(isRefusal(cp2, "verification_scope"));
  assert.equal(cp2.isError, true);
  assert.equal(
    counts.checkpoint,
    beforeCheckpoints,
    "the backend never saw the closing call",
  );
  const j = firstJson(cp2);
  // It NAMES what was built and BOTH verification verbs.
  assert.deepEqual(j.unverified_operations, ["create_box"]);
  assert.equal(j.unverified_count, 1);
  assert.match(j.closing_intent, /boss/);
  assert.match(j.how_to_proceed, /verify_part/);
  assert.match(j.how_to_proceed, /verify_claim/);
  assert.match(j.how_to_proceed, /skip_verification/);
  // And it says WHY a sound certificate is not enough.
  assert.match(j.reason, /9\.97%/);
});

// ─── 3. verify_part clears it; the IDENTICAL call then proceeds ─────────────

await call("verify_part", { part_id: 1 });
const cp2b = await checkpoint("M8 clearance holes, close fit, 4x base corners");
check("verify_part unblocks the identical closing call (never cached)", () => {
  assert.equal(
    firstJson(cp2b).refused,
    undefined,
    "a refusal-cache replay would deadlock the caller who COMPLIED",
  );
  assert.equal(counts.checkpoint, beforeCheckpoints + 1);
  assert.equal(cp2b.content.length >= 1, true);
});

// ─── 4. verify_claim clears it too ──────────────────────────────────────────

await call("create_cylinder", { plane: "xy", cx: 0, cy: 0, radius: 4, height: 12 });
const cp3refused = await checkpoint("counterbore ø14 x 6 deep, top face");
await call("verify_claim", { part_id: 1, claim: "volume is 8 mm^3" });
const cp3 = await checkpoint("counterbore ø14 x 6 deep, top face");
check("verify_claim clears the gate as well as verify_part", () => {
  assert.ok(isRefusal(cp3refused, "verification_scope"), "armed by the cylinder");
  assert.equal(firstJson(cp3).refused, undefined, "and cleared by verify_claim");
});

// ─── 5. skip_verification:true is the one escape ────────────────────────────

await call("create_box", { plane: "xy", cx: 0, cy: 0, width: 5, depth: 5, height: 5 });
const cpSkip = await checkpoint("relief slot 6 wide x 3 deep, left flank", {
  skip_verification: true,
});
check("skip_verification:true escapes the gate explicitly, not silently", () => {
  assert.equal(firstJson(cpSkip).refused, undefined);
  // The stub asserts the flag never reaches the backend; reaching here proves it.
});

// ─── 6. no mutating work under the intent → closes freely ───────────────────

const cpQuiet = await checkpoint("chamfer 1x45 on the two outer top edges");
check("an intent with no mutating work under it closes without a nudge", () => {
  assert.equal(firstJson(cpQuiet).refused, undefined);
});

// ─── 7. a verify BEFORE the mutation does not count ─────────────────────────

resetSessionGates();
await checkpoint("pilot bore ø6 through, centred");
await call("verify_part", { part_id: 1 }); // looked at the PREVIOUS state
await call("create_box", { plane: "xy", cx: 0, cy: 0, width: 1, depth: 1, height: 1 }); // then built
const cpStale = await checkpoint("spotface ø20 x 1 deep around the bore");
check("a verify that ran BEFORE the mutation does not clear the gate", () => {
  assert.ok(
    isRefusal(cpStale, "verification_scope"),
    "the box built AFTER the verify was still never inspected",
  );
  assert.deepEqual(firstJson(cpStale).unverified_operations, ["create_box"]);
});

// ─── 8. a read that only REPORTS is not a verification ──────────────────────

resetSessionGates();
await checkpoint("hub ø60 x 20, keyed bore");
await call("create_box", { plane: "xy", cx: 0, cy: 0, width: 3, depth: 3, height: 3 });
await call("list_parts", {});
await call("get_part", { part_id: 1 });
await call("mass_properties", { part_id: 1 });
const cpReports = await checkpoint("keyway 6 wide x 3.5 deep, full length");
check("get_part / list_parts / mass_properties do not clear the gate", () => {
  assert.ok(
    isRefusal(cpReports, "verification_scope"),
    "reporting facts is not checking them against anything",
  );
});

// ─── 9. clear_timeline is deliberately out of scope ─────────────────────────

resetSessionGates();
await checkpoint("scrap block for a boolean rehearsal");
await call("create_box", { plane: "xy", cx: 0, cy: 0, width: 9, depth: 9, height: 9 });
const cleared = await call("clear_timeline", {});
check("clear_timeline is never gated — the ledger the work lives in is going", () => {
  assert.equal(firstJson(cleared).refused, undefined);
});
const cpAfterClear = await checkpoint("flange ø120 x 14, 8 x ø14 on ø95 B.C.");
check("and it closes the intent, so the next checkpoint is clean", () => {
  assert.equal(firstJson(cpAfterClear).refused, undefined);
});

// ─── 10. the generic-name gate still fires, independently ───────────────────

resetSessionGates();
await checkpoint("web rib 4 thick between the two bosses");
await call("create_box", { plane: "xy", cx: 0, cy: 0, width: 2, depth: 2, height: 2 });
const cpGeneric = await checkpoint("step 3");
check("a sequence-position name is still refused by the intent gate first", () => {
  assert.ok(isRefusal(cpGeneric, "intent"), "name quality is judged on its own");
});

stub.close();
console.log(`\nverification_scope_gate: ${passed} checks passed`);
