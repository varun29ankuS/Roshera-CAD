/**
 * sim_static's answer as the agent reads it: the report whole, except the
 * result field's per-vertex arrays (5 MB on a 60 x 40 x 50 bracket -- one
 * study swamped the agent's context), which become a summary.
 */
import assert from "node:assert/strict";
import { pathToFileURL, fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
const { simForAgent } = await import(pathToFileURL(join(HERE, ".build", "tools", "inspect.js")).href);

const report = {
  verdict: { verdict: "undecided", reason: "the peak stress did not converge between the two meshes" },
  checks: [{ name: "forces balance", passed: true, detail: "200.0 N against 200.0 N" }],
  field: {
    positions: [[0, 0, 0], [1, 0, 0], [0, 1, 0]],
    displacement: [[0, 0, 0], [0, 0, 0], [0, 0, 0]],
    von_mises: [1, 2, 44.2],
    triangles: [[0, 1, 2]],
    faces: [7],
    min_von_mises: 1,
    max_von_mises: 44.2,
  },
};

const out = simForAgent(report);
assert.deepEqual(out.verdict, report.verdict, "the verdict reaches the agent unchanged");
assert.deepEqual(out.checks, report.checks, "so do the checks");
assert.equal(out.field.vertices, 3);
assert.equal(out.field.triangles, 1);
assert.equal(out.field.max_von_mises, 44.2);
for (const k of ["positions", "displacement", "von_mises", "faces"])
  assert.equal(k in out.field, false, `the field's ${k} array stays with the viewport`);
assert.match(out.field.shown, /Simulate tab/);
// A refusal body (no field) passes through untouched.
const refusal = { error: "sim_part_can_move", free_motions: ["can slide along x"] };
assert.deepEqual(simForAgent(refusal), refusal);
console.log("ok - sim_static gives the agent the report with the field summarised");