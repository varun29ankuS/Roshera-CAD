/**
 * Live exercise of the dispatch gates against the RUNNING backend (:8081,
 * or ROSHERA_URL). Uses the invoke funnel for every call so args are parsed
 * by each tool's own schema, exactly as a real agent call is.
 *
 * Footprint: one checkpoint + one notebook line + one 5 mm cube (deleted at
 * the end). Nothing else is touched.
 *
 *   node test/constraint_gates_live.mjs   (after: npx tsc -p tsconfig.json --outDir test/.build)
 */

import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const { buildTable } = await import(
  pathToFileURL(join(HERE, ".build", "surface.js")).href
);

const table = buildTable();
const invoke = (name, args) => table.get("invoke").handler({ name, args }, {});
const firstJson = (r) => JSON.parse(r.content[0].text);

// Reachability probe first — an honest "backend not reachable" beats a wall
// of misleading assertion failures.
const probe = await invoke("list_parts", {});
if (probe.isError) {
  console.error("live backend not reachable/authorized — aborting:");
  console.error(probe.content[0].text.slice(0, 400));
  process.exit(2);
}
console.log("backend reachable; live gate pass starting");

const boxArgs = {
  width: 5,
  depth: 5,
  height: 5,
  center: [200, 200, 0],
  name: "gate-verify cube",
};

// 1. intent gate, live: no checkpoint open in this session → refused before
//    the kernel is touched.
const r1 = await invoke("create_box", boxArgs);
assert.equal(firstJson(r1).refused, true);
assert.equal(firstJson(r1).gate, "intent");
console.log("  ok - live: create_box with no open checkpoint refused (intent gate)");

// 2. refusal cache, live: identical re-issue answered from cache with the
//    same refusal + the cache note.
const r2 = await invoke("create_box", boxArgs);
assert.equal(r2.content[0].text, r1.content[0].text);
assert.match(r2.content[1].text, /refusal cache/);
console.log("  ok - live: identical re-issue served the SAME refusal from cache");

// 3. generic checkpoint name refused, live.
const r3 = await invoke("timeline_checkpoint", { name: "step 3" });
assert.equal(firstJson(r3).gate, "intent");
console.log("  ok - live: generic checkpoint name 'step 3' refused");

// 4. real intent opens the gate + auto-writes the notebook line.
const cp = await invoke("timeline_checkpoint", {
  name: "dispatch-gate live verification: 5 mm cube at [200,200,0], then removed",
  description:
    "proves the intent gate, refusal cache and notebook mirror against the live kernel",
});
const cpj = firstJson(cp);
assert.ok(!cp.isError, `checkpoint failed: ${cp.content[0].text.slice(0, 300)}`);
console.log(
  `  ok - live: checkpoint recorded (${JSON.stringify(cpj.checkpoint?.id ?? cpj.checkpoint)}), notebook line ${JSON.stringify(cpj.notebook_entry)}`,
);

// 5. the same mutating call now proceeds; the kernel certifies it.
const r5 = await invoke("create_box", boxArgs);
const j5 = firstJson(r5);
assert.ok(!r5.isError, `create_box failed: ${r5.content[0].text.slice(0, 300)}`);
assert.equal(j5.refused, undefined);
console.log(
  `  ok - live: create_box proceeded with intent open — part ${j5.part_id}, perception: ${j5.perception}`,
);

// 6. cleanup: remove the verification cube.
const del = await invoke("delete_part", { part_id: j5.part_id });
assert.ok(!del.isError, `cleanup delete failed: ${del.content[0].text.slice(0, 300)}`);
console.log(`  ok - live: verification cube (part ${j5.part_id}) deleted`);

console.log("\nconstraint_gates_live: all live checks passed");
