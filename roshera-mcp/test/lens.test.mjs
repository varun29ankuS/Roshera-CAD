/**
 * Kernel Lens tools: `lens_why_not_sound` groups a part's blocking events
 * by failing check, keeps solid-level failures (never drops them), says
 * what to look at next for each, and counts blocking faces it did not
 * fetch. Both tools are registered on the surface.
 *
 *     npm run test:gates:build && node test/lens.test.mjs
 */
import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const { groupBlocking, NEXT_STEP } = await import(
  pathToFileURL(join(HERE, ".build", "tools", "lens.js")).href
);
const { buildTable } = await import(pathToFileURL(join(HERE, ".build", "surface.js")).href);

const blocking = (conjunct, why, measured) => ({
  stage: "certify",
  kind: { blocking: { conjunct } },
  step_instance: null,
  why,
  measured,
});

const part = {
  solid_id: 7,
  sound: false,
  overview: [{ kind: "blocking", count: 3 }],
  faces: [
    { face_id: 1, pid: "a".repeat(32), status: "blocking" },
    { face_id: 2, pid: "b".repeat(32), status: "repaired" },
    { face_id: 3, pid: "c".repeat(32), status: "blocking" },
    { face_id: 4, pid: "d".repeat(32), status: "blocking" },
  ],
  solid_events: [blocking("shells_outward", "inside out", [["signed_volume", -8]])],
};
const stories = [
  {
    face_id: 1,
    pid: "a".repeat(32),
    status: "blocking",
    events: [
      { stage: "import", kind: "loop_reversed", step_instance: 11, why: "bound .F.", measured: [] },
      blocking("watertight", "open", [["open_edges", 4]]),
    ],
  },
  {
    face_id: 3,
    pid: "c".repeat(32),
    status: "blocking",
    events: [blocking("watertight", "open", [["open_edges", 2]])],
  },
];

const out = groupBlocking(part, stories);
assert.equal(out.sound, false);
const byName = Object.fromEntries(out.groups.map((g) => [g.conjunct, g]));
assert.deepEqual(Object.keys(byName).sort(), ["shells_outward", "watertight"]);
assert.deepEqual(
  byName.watertight.faces.map((f) => [f.face_id, f.measured[0][1]]),
  [[1, 4], [3, 2]],
  "each face once per check, with what was measured",
);
assert.equal(byName.watertight.on_solid.length, 0);
assert.equal(byName.shells_outward.faces.length, 0);
assert.equal(byName.shells_outward.on_solid.length, 1, "a solid-level failure is kept");
assert.equal(byName.watertight.next_step, NEXT_STEP.watertight);
assert.equal(out.faces_not_fetched, 1, "face 4 blocks but was not fetched");
assert.ok(
  out.groups.every((g) => g.faces.every((f) => f.why.length > 0)),
  "no import event leaks into the blocking groups",
);

const sound = groupBlocking({ ...part, sound: true, faces: [], solid_events: [] }, []);
assert.deepEqual(sound, { sound: true, groups: [], faces_not_fetched: 0 });

const table = buildTable();
for (const n of ["lens_explain", "lens_why_not_sound"]) {
  assert.ok(table.has(n), `${n} is registered`);
}

console.log("lens.test.mjs: all assertions passed");
