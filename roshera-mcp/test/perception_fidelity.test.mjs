/**
 * FIDELITY REACHES THE AGENT — the disconnection-gate proof for the
 * requested-vs-measured block (2026-08-13).
 *
 * The kernel measures fidelity and the api-server attaches it to a mutating
 * op's OWN response at `body.perception.fidelity`
 * (api-server/src/main.rs:1326-1336 `attach_fidelity`, called at main.rs:4237
 * cylinder, 4456 box, 5361 revolve, 6026 loft; block shape at
 * main.rs:1247-1314 `fidelity_json`). The MCP client then rebuilt the
 * perception object with a FIXED key set (core.ts `perceptionFromBody`) that
 * had no `fidelity`, so the block was DROPPED before any agent saw it: built,
 * correct, and wired to nothing — the fourteen-times failure class.
 *
 * This is therefore a PRODUCTION-CALL-SITE test, not a helper test: it drives
 * the REAL dispatch table (surface.js `buildTable` → the real `create_cylinder`
 * / `create_box` handlers → `okp` → `perceive` → `perceptionFromBody`) against
 * a local stub backend whose bodies carry fidelity blocks copied from
 * `fidelity_json`'s own shape, and asserts the block survives to the TOOL
 * RESULT the agent reads.
 *
 *   Build the fixture first (never touches dist/):
 *     npx tsc -p tsconfig.json --outDir test/.build
 *   Run:
 *     node test/perception_fidelity.test.mjs
 *
 * Proves:
 *   1. a fidelity block on a mutating op's response reaches the tool result
 *      BYTE-FOR-BYTE — nothing is rebuilt, renamed or rounded;
 *   2. `worst.signed_relative_deviation` — the field roshera-rl's
 *      `rewardFromResult` reads (roshera-rl/lib/reward.mjs:123) — is present
 *      and is the number the backend sent;
 *   3. a report that measured NOTHING keeps `fidelity_ok` ABSENT (main.rs:1276-
 *      1279 omits the key deliberately: a green boolean over an unmeasured
 *      quantity is the "certified sound at 9.97%" pattern the block exists to
 *      end), while its `gaps` still arrive;
 *   4. an op whose response carries NO fidelity block yields NO `fidelity` key
 *      — never a fabricated default, never a null (main.rs:1330-1332: an empty
 *      report inserts nothing at all);
 *   5. the rest of the perception verdict is unchanged by the passthrough.
 *
 * Ambient mode is pinned to `cert` because that is what roshera-rl's session
 * pins (roshera-rl/lib/mcp_session.mjs:428) and it is the mode in which the
 * perception OBJECT — rather than `compactVerdict`'s one prose line — is what
 * the agent receives (core.ts:930).
 */

import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const PART_ID = 7;
const PART_UUID = "aaaaaaaa-1111-4111-8111-111111111111";

// ─── The two fidelity blocks, shaped by `fidelity_json` (main.rs:1274-1313) ──
//
// Field-for-field: `op`, optional `fidelity_ok`, `tolerance`, `worst` (or
// null), `quantities` (FidelityQuantity — name/requested/measured/
// relative_deviation/signed_relative_deviation/method, fidelity.rs:84-109),
// `gaps` (FidelityGap — name/reason, fidelity.rs:137-140), `note`.
//
// The numbers are a cylinder measured off its tessellation, the calibration
// case main.rs:4234-4236 describes: a small NEGATIVE deviation, because the
// tessellation's largest radial vertex distance sits just inside the analytic
// radius.
const FIDELITY_MEASURED = {
  op: "cylinder",
  fidelity_ok: true,
  tolerance: 0.02,
  worst: {
    name: "radius",
    requested: 25.0,
    measured: 24.99924521508539,
    relative_deviation: 0.0000301913965844,
    signed_relative_deviation: -0.0000301913965844,
    direction: "built SMALLER than requested",
  },
  quantities: [
    {
      name: "radius",
      requested: 25.0,
      measured: 24.99924521508539,
      relative_deviation: 0.0000301913965844,
      signed_relative_deviation: -0.0000301913965844,
      method:
        "largest perpendicular distance of a tessellation vertex from the requested axis",
    },
    {
      name: "height",
      requested: 60.0,
      measured: 60.0,
      relative_deviation: 0.0,
      signed_relative_deviation: 0.0,
      method: "span of the tessellation projected onto the requested axis",
    },
  ],
  gaps: [],
  note:
    "fidelity compares the REQUEST to the RESULT; `sound` compares the result " +
    "to the laws of topology.",
};

// NOTHING measured: `fidelity_ok` is OMITTED, not `false` and not `true`
// (main.rs:1276-1279), `worst` is null (main.rs:1296), and the gap states why —
// the degenerate-axis branch of `cylinder_fidelity`
// (geometry-engine/src/queries/fidelity.rs:483-488).
const FIDELITY_UNMEASURED = {
  op: "cylinder",
  tolerance: 0.02,
  worst: null,
  quantities: [],
  gaps: [
    {
      name: "radius,height",
      reason:
        "the built solid tessellated to no vertices, or the requested axis is " +
        "degenerate — extents about the axis are undefined, so nothing is compared",
    },
  ],
  note:
    "fidelity compares the REQUEST to the RESULT; `sound` compares the result " +
    "to the laws of topology.",
};

// ─── Stub backend ───────────────────────────────────────────────────────────

const counts = { checkpoint: 0, cylinder: 0, box: 0, perception: 0 };

/** The cheap verdict `certified_response` embeds (main.rs:1058-1145). */
const basePerception = () => ({
  sound: true,
  valid: true,
  watertight: true,
  open_edges: 0,
  nonmanifold_edges: 0,
  face_count: 3,
  volume: 117790.346542991,
  dims: { x: 50, y: 50, z: 60 },
  verdict: "SOUND — valid closed solid",
});

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
      return send({ id: `cp-${counts.checkpoint}`, name: (body ? JSON.parse(body) : {}).name });
    }
    if (req.method === "POST" && url === "/api/geometry/cylinder") {
      counts.cylinder++;
      // Call 1 measured both quantities; call 2 could measure neither.
      return send({
        success: true,
        solid_id: PART_ID,
        object: { id: PART_UUID },
        stats: { triangle_count: 96 },
        perception: {
          ...basePerception(),
          fidelity: counts.cylinder === 1 ? FIDELITY_MEASURED : FIDELITY_UNMEASURED,
        },
      });
    }
    if (req.method === "POST" && url === "/api/geometry/box") {
      counts.box++;
      // NO fidelity block at all — `attach_fidelity` inserts nothing for an
      // empty report (main.rs:1330-1332), so this body is what the wire really
      // looks like when there was nothing to measure.
      return send({
        success: true,
        solid_id: PART_ID,
        object: { id: PART_UUID },
        stats: { triangle_count: 12 },
        perception: basePerception(),
      });
    }
    if (req.method === "GET" && url === "/api/agent/parts") {
      // `newestPartId` reduces over `p.id` (core.ts:599-603) — this id MUST match
      // the POST's `solid_id` or the embedded-perception stash misses.
      return send([{ id: PART_ID, part_id: PART_ID, name: "Cylinder 7" }]);
    }
    if (req.method === "GET" && /^\/api\/agent\/parts\/\d+$/.test(url)) {
      return send({
        part_id: PART_ID,
        volume: 117790.346542991,
        topology: { face_count: 3 },
        location: { center_world: [0, 0, 30], dimensions_world: [50, 50, 60] },
      });
    }
    if (req.method === "GET" && /^\/api\/agent\/parts\/\d+\/perception$/.test(url)) {
      // The READ side has no fidelity producer (verified: `fidelity` does not
      // occur anywhere in api-server/src/handlers/). This response is what it
      // really returns, so a fallback that invented one would be untestable
      // here — and the fast path below is the one production takes.
      counts.perception++;
      return send({ sound: true, valid: true, watertight: true, verdict: "SOUND" });
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

// Read at core.js module load — set BEFORE importing anything.
process.env.ROSHERA_URL = `http://127.0.0.1:${port}`;
// The mode roshera-rl pins (mcp_session.mjs:428): the perception OBJECT, no image.
process.env.ROSHERA_AMBIENT_PERCEPTION = "cert";

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
const check = (label, fn) => {
  fn();
  passed++;
  console.log(`  ok - ${label}`);
};

// ─── drive the real dispatch path ───────────────────────────────────────────

resetSessionGates();
await call("timeline_checkpoint", { name: "shaft blank ø50 x 60 long, turned stock" });

const cyl = await call("create_cylinder", { plane: "xy", cx: 0, cy: 0, radius: 25, height: 60 });
const cylJson = firstJson(cyl);

check("the op reached the backend and its perception object came back", () => {
  assert.equal(counts.cylinder, 1, "the stub really served the create");
  assert.equal(cylJson.part_id, PART_ID, "and the tool resolved the part it just built");
  assert.equal(
    typeof cylJson.perception,
    "object",
    "ROSHERA_AMBIENT_PERCEPTION=cert must yield the perception OBJECT, not a verdict line",
  );
});

check("the fidelity block survives to the tool result, byte-for-byte", () => {
  assert.deepEqual(
    cylJson.perception.fidelity,
    FIDELITY_MEASURED,
    "the block the api-server attached at body.perception.fidelity must arrive " +
      "unaltered — a rebuilt one would be a second, unverifiable statement",
  );
});

check("worst.signed_relative_deviation — the field the RL reward reads — is the backend's number", () => {
  // roshera-rl/lib/reward.mjs:123 reads exactly this path.
  const signed = cylJson.perception.fidelity?.worst?.signed_relative_deviation;
  assert.equal(typeof signed, "number");
  assert.ok(Number.isFinite(signed));
  assert.equal(signed, FIDELITY_MEASURED.worst.signed_relative_deviation);
  assert.ok(signed < 0, "the SIGN is the diagnosis and must not be lost on the way");
});

check("the rest of the verdict is untouched by the passthrough", () => {
  assert.equal(cylJson.perception.sound, true);
  assert.equal(cylJson.perception.watertight, true);
  assert.equal(cylJson.perception.volume, 117790.346542991);
  assert.equal(cylJson.perception.face_count, 3);
  assert.equal(cylJson.perception.verdict, "SOUND — valid closed solid");
});

const cyl2 = await call("create_cylinder", { plane: "xy", cx: 0, cy: 0, radius: 25, height: 60 });
const cyl2Json = firstJson(cyl2);

check("a report that measured NOTHING keeps fidelity_ok ABSENT, and still states its gaps", () => {
  const f = cyl2Json.perception.fidelity;
  assert.ok(f, "the block is delivered even when nothing could be measured");
  assert.equal(
    "fidelity_ok" in f,
    false,
    "main.rs:1276-1279 omits the key deliberately; defaulting it to true here " +
      "would hand a thin client a pass over an unmeasured quantity",
  );
  assert.equal(f.worst, null);
  assert.deepEqual(f.gaps, FIDELITY_UNMEASURED.gaps, "a gap is never a zero");
});

const box = await call("create_box", { plane: "xy", cx: 0, cy: 0, width: 20, depth: 20, height: 10 });
const boxJson = firstJson(box);

check("an op with no fidelity block yields no fidelity key — never a fabricated default", () => {
  assert.equal(counts.box, 1);
  assert.equal(typeof boxJson.perception, "object");
  assert.equal(
    "fidelity" in boxJson.perception,
    false,
    "absence must survive the trip: a null or an empty block would read as " +
      "'measured, and nothing was wrong'",
  );
  assert.equal(boxJson.perception.sound, true, "while the verdict itself still arrives");
});

stub.close();
console.log(`\nperception_fidelity: ${passed} checks passed`);
