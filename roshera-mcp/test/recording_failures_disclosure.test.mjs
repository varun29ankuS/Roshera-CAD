/**
 * LOST OPERATIONS REACH THE AGENT (Task 69).
 *
 * The api-server reports an operation that an EARLIER call's kernel op
 * recorded and that was then lost (its branch was retired before the drain
 * worker appended it, or the durable store refused it) exactly ONCE: on the
 * next mutating response, at `body.perception.recording_failures`
 * (`certified_response`, api-server/src/main.rs). The MCP rebuilds the
 * perception with a FIXED key set (core.ts `perceptionFromBody` / `perceive`),
 * so a key missing there is dropped before any agent sees it — and because the
 * backend reports once, that drop would make the loss reach no one.
 *
 * Drives the REAL `api()` → embedded-perception stash → `perceive()` path
 * against a stub backend, then the one-line `compactVerdict`.
 *
 *   Build the fixture first (never touches dist/):
 *     npx tsc -p tsconfig.json --outDir test/.build
 *   Run:
 *     node test/recording_failures_disclosure.test.mjs
 */

import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));

// Shaped by `timeline_engine::RecordFailure` (serde, snake_case stage).
const LOST = [
  {
    stage: "append",
    kind: "create_box_3d",
    branch: "5ad229f6-ed4d-4870-8958-d1084604526d",
    sequence: null,
    document: null,
    error: "Invalid operation: Cannot add events to non-active branch",
  },
];

let carryFailures = true;
let nextId = 7;

const stub = http.createServer((req, res) => {
  const url = (req.url ?? "").split("?")[0];
  const send = (obj) => {
    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify(obj));
  };
  req.on("data", () => {});
  req.on("end", () => {
    if (req.method === "POST" && url === "/api/geometry/box") {
      const perception = {
        sound: true,
        valid: true,
        watertight: true,
        cert: { sound: true, brep_valid: true, watertight: true },
        face_count: 6,
        volume: 1000,
        verdict: "SOUND",
      };
      if (carryFailures) perception.recording_failures = LOST;
      return send({ success: true, solid_id: nextId, perception });
    }
    return send({});
  });
});
stub.listen(0, "127.0.0.1");
await once(stub, "listening");
process.env.ROSHERA_URL = `http://127.0.0.1:${stub.address().port}`;

const core = await import(pathToFileURL(join(HERE, ".build", "core.js")).href);

let failures = 0;
async function check(name, fn) {
  try {
    await fn();
    console.log(`ok   ${name}`);
  } catch (e) {
    failures += 1;
    console.log(`FAIL ${name}\n     ${e.message}`);
  }
}

await check("a lost earlier op survives the perception rebuild verbatim", async () => {
  carryFailures = true;
  nextId = 7;
  const body = await core.api("POST", "/api/geometry/box", { width: 10, depth: 10, height: 10 });
  const p = await core.perceive(body.solid_id);
  assert.deepEqual(p.recording_failures, LOST);
});

await check("the one-line verdict says an earlier op was not recorded", async () => {
  const line = core.compactVerdict({
    sound: true,
    brep_valid: true,
    watertight: true,
    recording_failures: LOST,
  });
  assert.match(line, /1 OPERATION\(S\) NOT RECORDED/);
  // The list is recorder-wide: the note must not presume the ops were this
  // agent's own, and must say who recorded them.
  assert.match(line, /possibly by another client/);
  assert.match(line, /check p\.recording_failures\[\]\.author\/channel/);
  assert.doesNotMatch(line, /EARLIER/);
  assert.doesNotMatch(line, / {2}/);
  assert.match(line, /SOUND ✓/, "the part verdict itself is untouched");
});

await check("a report on an earlier call of a multi-call tool is carried, not overwritten", async () => {
  carryFailures = true;
  nextId = 9;
  await core.api("POST", "/api/geometry/box", { width: 10, depth: 10, height: 10 });
  carryFailures = false;
  nextId = 10;
  const body = await core.api("POST", "/api/geometry/box", { width: 10, depth: 10, height: 10 });
  const p = await core.perceive(body.solid_id);
  assert.deepEqual(p.recording_failures, LOST, "the once-only report must survive the second call");
});

await check("nothing lost → no key and no note (additive, not noise)", async () => {
  carryFailures = false;
  nextId = 8;
  const body = await core.api("POST", "/api/geometry/box", { width: 10, depth: 10, height: 10 });
  const p = await core.perceive(body.solid_id);
  assert.equal(p.recording_failures, undefined);
  assert.doesNotMatch(core.compactVerdict(p), /NOT RECORDED/);
});

stub.close();
if (failures > 0) {
  console.log(`\n${failures} check(s) failed`);
  process.exit(1);
}
console.log("\nall recording-failure disclosure checks passed");
