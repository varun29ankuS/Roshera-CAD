/**
 * A batch step is gated on a CERTIFICATE, never on the cheap seed, and a
 * missing verdict halts (audit 2026-09-03, Task 67).
 *
 * `boolean_many` and `drill_pattern` sent every per-step boolean with
 * `fast: true`, which skips the api-server's full certificate. The response
 * then carried only the lightweight seed — and that seed wrote
 * `sound`/`watertight` from B-Rep validity alone. `api()` stashed it,
 * `perceive()` reused it as "the full certificate the op already ran", and the
 * per-step halt gate `if (p && p.sound !== true)` read B-Rep validity as
 * soundness. The same gate read a MISSING verdict (a perception timeout,
 * which the drill loop's own comments say happens there) as a pass.
 *
 * Exercises the REAL dispatch modules (ToolTable wrapper + gates.ts + real
 * handlers, compiled from src to test/.build) against a local stub backend.
 *
 *   Build the fixture first (never touches dist/):
 *     npm run test:gates:build
 *   Run:
 *     node test/batch_step_certification.test.mjs
 *
 * Proves:
 *   1. a step whose response carries a seed that SAYS sound:true but no
 *      certificate is not counted as certified: the step's real certificate
 *      is fetched, it is unsound, and the batch halts;
 *   2. a response marked `certificate: "not_run"` is not a verdict either —
 *      the real (sound) certificate is fetched and the batch completes;
 *   3. a response that DID carry the full certificate is reused as-is (no
 *      second certification round-trip);
 *   4. a perception timeout HALTS boolean_many and drill_pattern with a typed
 *      reason — it is not a pass;
 *   5. the per-step booleans no longer opt out of the certificate (`fast`);
 *   6. the one-line verdict renders an absent `sound` as NO VERDICT, never as
 *      SOUND and never as UNSOUND.
 */

import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const BASE_UUID = "aaaaaaaa-6767-4333-8444-555555555555"; // solid 7 — sound base
const toolUuid = (n) => `bbbbbbbb-6767-4333-8444-${String(n).padStart(12, "0")}`;

// ─── Stub backend ───────────────────────────────────────────────────────────

/** What the NEXT boolean returns: the result solid id and its body's perception. */
let booleanPlan = { solid_id: 21, perception: undefined };
const booleanBodies = [];
const perceptionGets = {};
let cylinders = 0;

/** The base-shaped `fast: true` seed: `sound` from B-Rep validity, zeros unmeasured. */
const LYING_SEED = {
  sound: true,
  valid: true,
  brep_valid: true,
  watertight: true,
  open_edges: 0,
  nonmanifold_edges: 0,
  verdict: "OK — valid closed solid; display mesh watertight",
};
/** The honest not-run shape the fixed api-server sends on `fast: true`. */
const NOT_RUN_SEED = {
  brep_valid: true,
  certificate: "not_run",
  verdict: "NO VERDICT — full kernel certificate NOT run (fast opt-out)",
  display_mesh_open_edges: 0,
  display_mesh_nonmanifold_edges: 0,
};
/** A fidelity block, which rides only on a mutating op's own response. */
const FIDELITY = {
  op: "cone",
  fidelity_ok: false,
  worst: { name: "height", signed_relative_deviation: -0.0997, requested: 60, measured: 54.018 },
  gaps: [],
};
const soundCert = () => ({
  sound: true,
  brep_valid: true,
  watertight: true,
  manifold: true,
  shells_outward: true,
  self_intersection_free: true,
  tessellation_clean: true,
  mesh_quality_clean: true,
});
/** A full-certificate perception, as the default mutating response embeds it. */
const certifiedPerception = (sound) => ({
  sound,
  valid: true,
  brep_valid: true,
  watertight: true,
  manifold: true,
  self_intersection_free: sound,
  open_edges: 0,
  nonmanifold_edges: 0,
  verdict: sound ? "SOUND — full kernel certificate clean" : "UNSOUND — full kernel certificate flags a defect (see cert)",
  cert: { ...soundCert(), sound, self_intersection_free: sound },
});

/** GET /perception bodies by solid id; "hang" never answers inside the budget. */
const perceptionById = {
  7: certifiedPerception(true),
  21: certifiedPerception(false), // B-Rep valid, certificate UNSOUND
  22: certifiedPerception(true),
  23: "hang",
  24: certifiedPerception(true),
  // A GET body that states NO `sound` — only the B-Rep check (the old
  // `?fast=1` shape). B-Rep validity is not soundness.
  25: { valid: true, brep_valid: true, watertight: true, open_edges: 0 },
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
      return send({ id: `cp-${Date.now()}`, name: body.name, branch: "main" });
    }
    if (req.method === "POST" && url === "/api/geometry/boolean") {
      booleanBodies.push(body);
      const out = { object: { id: BASE_UUID }, solid_id: booleanPlan.solid_id };
      if (booleanPlan.perception !== undefined) out.perception = booleanPlan.perception;
      return send(out);
    }
    if (req.method === "POST" && url === "/api/geometry/cone") {
      // A `fast: true`-shaped response that DID carry a fidelity disclosure.
      return send({ solid_id: 22, perception: { ...NOT_RUN_SEED, fidelity: FIDELITY } });
    }
    if (req.method === "POST" && url === "/api/geometry/cylinder") {
      cylinders++;
      return send({ object: { id: toolUuid(900 + cylinders) }, solid_id: 900 + cylinders });
    }
    if (req.method === "GET" && url === "/api/scene/snapshot") {
      return send({ objects: [{ id: BASE_UUID, analytical_geometry: { solid_id: 7 } }] });
    }
    let m;
    if (req.method === "GET" && (m = url.match(/^\/api\/agent\/parts\/(\d+)\/perception$/))) {
      const id = Number(m[1]);
      perceptionGets[id] = (perceptionGets[id] ?? 0) + 1;
      const p = perceptionById[id];
      if (p === "hang") {
        setTimeout(() => send(certifiedPerception(true)), 1500);
        return;
      }
      return send(p ?? certifiedPerception(true));
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
      return send([{ id: 7 }]);
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

// Read at core.js module load — set BEFORE importing anything. A short
// perception budget makes the "hang" solid a real client-side timeout.
process.env.ROSHERA_URL = `http://127.0.0.1:${port}`;
process.env.ROSHERA_MCP_PERCEPTION_TIMEOUT_MS = "300";

const { buildTable } = await import(pathToFileURL(join(HERE, ".build", "surface.js")).href);
const { resetSessionGates } = await import(pathToFileURL(join(HERE, ".build", "gates.js")).href);
const core = await import(pathToFileURL(join(HERE, ".build", "core.js")).href);

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

async function openIntent(name) {
  resetSessionGates();
  const cp = await call("timeline_checkpoint", { name, branch: "main" });
  assert.notEqual(cp.isError, true, `checkpoint must open: ${cp.content?.[0]?.text}`);
}

let toolSeq = 0;
const nextTool = () => toolUuid(++toolSeq);

// ─── 1. the lying seed is not a certificate ─────────────────────────────────

console.log("1. a step answered by the cheap seed alone");
await openIntent("seed proof — cross bore D6 through the 40 x 40 block");
{
  booleanPlan = { solid_id: 21, perception: LYING_SEED };
  const before = perceptionGets[21] ?? 0;
  const r = await call("boolean_many", { op: "difference", base: BASE_UUID, tools: [nextTool()] });
  check("the step's REAL certificate was fetched (the seed was not taken as one)", () => {
    assert.ok((perceptionGets[21] ?? 0) > before, "GET /perception for the step's solid");
  });
  check("the certificate is unsound, so the batch HALTS", () => {
    assert.equal(r.isError, true, r.content?.[0]?.text);
    const j = firstJson(r);
    assert.match(j.halted, /step 1 .* UNSOUND/);
    assert.doesNotMatch(j.halted, / {2}/, "no absorbed indentation in the halt text");
    assert.equal(j.completed, 1);
  });
}

// ─── 2. a not_run response is not a verdict either ──────────────────────────

console.log("2. a step answered by a certificate:not_run response");
await openIntent("not-run proof — second bore D6 in the 40 x 40 block");
{
  booleanPlan = { solid_id: 22, perception: NOT_RUN_SEED };
  const before = perceptionGets[22] ?? 0;
  const r = await call("boolean_many", { op: "difference", base: BASE_UUID, tools: [nextTool()] });
  check("the real certificate was fetched and, being sound, the batch completes", () => {
    assert.ok((perceptionGets[22] ?? 0) > before, "GET /perception for the step's solid");
    assert.notEqual(r.isError, true, r.content?.[0]?.text);
    assert.equal(firstJson(r).completed, 1);
  });
}

{
  const before = perceptionGets[22] ?? 0;
  await core.api("POST", "/api/geometry/cone", {});
  const p = await core.perceive(22);
  check("an uncertified block's verdict comes from the real certificate, its fidelity is kept", () => {
    assert.ok((perceptionGets[22] ?? 0) > before, "GET /perception ran the certificate");
    assert.equal(p?.sound, true, JSON.stringify(p));
    assert.deepEqual(p?.fidelity, FIDELITY, "the op's own fidelity block survives verbatim");
  });
}

// ─── 3. a response that carried the certificate is reused ───────────────────

console.log("3. a step answered by the full certificate");
await openIntent("reuse proof — third bore D6 in the 40 x 40 block");
{
  booleanPlan = { solid_id: 24, perception: certifiedPerception(true) };
  const before = perceptionGets[24] ?? 0;
  const r = await call("boolean_many", { op: "difference", base: BASE_UUID, tools: [nextTool()] });
  check("the embedded certificate is reused: no second certification round-trip for the step", () => {
    assert.notEqual(r.isError, true, r.content?.[0]?.text);
    // The closing perceive() of the batch is served by GET (the stash was
    // consumed by the step), so at most ONE GET — never one per step plus one.
    assert.ok((perceptionGets[24] ?? 0) - before <= 1, `GETs: ${(perceptionGets[24] ?? 0) - before}`);
  });
}

// ─── 4. a perception timeout halts ──────────────────────────────────────────

console.log("4. a step whose verdict times out");
await openIntent("timeout proof — fourth bore D6 in the 40 x 40 block");
{
  booleanPlan = { solid_id: 23, perception: undefined };
  const n = booleanBodies.length;
  const r = await call("boolean_many", {
    op: "difference",
    base: BASE_UUID,
    tools: [nextTool(), nextTool()],
  });
  check("boolean_many HALTS at the step it could not certify (a missing verdict is not a pass)", () => {
    assert.equal(r.isError, true, r.content?.[0]?.text);
    const j = firstJson(r);
    assert.match(j.halted, /step 1 .*could not be certified/);
    assert.match(j.halted, /timed out/);
    assert.doesNotMatch(j.halted, / {2}/, "no absorbed indentation in the halt text");
    assert.equal(j.completed, 1);
    assert.equal(booleanBodies.length, n + 1, "step 2 never reached the backend");
  });
}
await openIntent("drill timeout proof — 2 x D6 on D40 B.C.");
{
  booleanPlan = { solid_id: 23, perception: undefined };
  const n = booleanBodies.length;
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
  check("drill_pattern HALTS at the hole it could not certify", () => {
    assert.equal(r.isError, true, r.content?.[0]?.text);
    const j = firstJson(r);
    assert.match(j.halted, /hole 1 .*could not be certified/);
    assert.doesNotMatch(j.halted, / {2}/, "no absorbed indentation in the halt text");
    assert.equal(j.holes_completed, 1);
    assert.equal(booleanBodies.length, n + 1, "hole 2's boolean never reached the backend");
  });
}

// ─── 4b. a fetched body that states no `sound` is no verdict ────────────────

console.log("4b. a step whose fetched perception states no sound");
await openIntent("brep-only proof — fifth bore D6 in the 40 x 40 block");
{
  booleanPlan = { solid_id: 25, perception: undefined };
  const n = booleanBodies.length;
  const r = await call("boolean_many", {
    op: "difference",
    base: BASE_UUID,
    tools: [nextTool(), nextTool()],
  });
  check("a B-Rep-valid body with no stated sound HALTS the batch (valid is never promoted to sound)", () => {
    assert.equal(r.isError, true, r.content?.[0]?.text);
    const j = firstJson(r);
    assert.match(j.halted, /step 1 .*could not be certified/);
    assert.match(j.halted, /NO VERDICT/);
    assert.doesNotMatch(j.halted, / {2}/, "no absorbed indentation in the halt text");
    assert.equal(j.completed, 1);
    assert.equal(booleanBodies.length, n + 1, "step 2 never reached the backend");
  });
}

check("statedSoundness: a valid B-Rep alone states no soundness; a stated or failed one does", () => {
  assert.equal(core.statedSoundness({ valid: true, brep_valid: true }), null);
  assert.equal(core.statedSoundness({ valid: true }), null);
  assert.equal(core.statedSoundness({ brep_valid: true, watertight: true, open_edges: 0 }), null);
  assert.equal(core.statedSoundness({ brep_valid: false }), false);
  assert.equal(core.statedSoundness({ valid: false }), false);
  assert.equal(core.statedSoundness({ sound: false, valid: true }), false);
  assert.equal(core.statedSoundness({ sound: true }), true);
});

// ─── 5. the per-step booleans run the certificate ───────────────────────────

check("no per-step boolean opted out of the certificate", () => {
  assert.ok(booleanBodies.length > 0);
  for (const b of booleanBodies) {
    assert.equal("fast" in b, false, `boolean body carried fast: ${JSON.stringify(b)}`);
  }
});

// ─── 6. an absent verdict renders as NO VERDICT ─────────────────────────────

check("compactVerdict renders sound:null as NO VERDICT, never SOUND or UNSOUND", () => {
  const line = core.compactVerdict({ sound: null, brep_valid: true, watertight: null });
  assert.match(line, /NO VERDICT/);
  assert.doesNotMatch(line, / {2}/, "no absorbed indentation in the verdict line");
  assert.doesNotMatch(line, /SOUND ✓/);
  assert.doesNotMatch(line, /UNSOUND/);
});

stub.close();
if (failed > 0) {
  console.log(`\nbatch_step_certification: ${failed} FAILED, ${passed} passed`);
  process.exit(1);
}
console.log(`\nbatch_step_certification: ${passed} checks passed`);
process.exit(0);
