// timeline_dependencies: the feature graph in the agent's terms (sequence
// numbers, operations), and what an edit would rebuild -- over a canned
// backend, so the shaping is pinned without a server.
//
// Run: npm run test:gates:build && node test/timeline_dependencies.test.mjs

import { buildTableWithControls } from "./.build/surface.js";

let failures = 0;
const fail = (m) => {
  console.error("  ✗ " + m);
  failures += 1;
};
const pass = (m) => console.log("  ✓ " + m);
const textOf = (res) => (res?.content ?? []).find((c) => c.type === "text")?.text ?? "";

const BOX = "00000000-0000-4000-8000-000000000001";
const CYL = "00000000-0000-4000-8000-000000000002";
const BOOL = "00000000-0000-4000-8000-000000000003";
const FIL = "00000000-0000-4000-8000-000000000004";
const graph = {
  nodes: [
    { id: BOX, sequence_number: 1, operation_type: "create_box_3d" },
    { id: CYL, sequence_number: 2, operation_type: "create_cylinder_3d" },
    { id: BOOL, sequence_number: 3, operation_type: "boolean" },
    { id: FIL, sequence_number: 4, operation_type: "fillet_edges" },
  ],
  edges: [
    { from: BOX, to: BOOL, critical: true },
    { from: CYL, to: BOOL, critical: true },
    { from: BOOL, to: FIL, critical: true },
  ],
};
const asked = [];
const real = globalThis.fetch;
globalThis.fetch = async (url) => {
  const u = String(url);
  asked.push(u);
  const body = u.includes("rebuild_from=")
    ? { ...graph, rebuild_plan: u.includes(CYL) ? [BOOL, FIL] : [] }
    : graph;
  return { ok: true, status: 200, text: async () => JSON.stringify(body) };
};

const { table } = buildTableWithControls();
const tool = table.get("timeline_dependencies");
try {
  console.log("(d1) the graph in sequence numbers");
  {
    const r = JSON.parse(textOf(await tool.handler({ branch: "main" })));
    if (r.events?.length === 4 && r.events[2].seq === 3 && r.events[2].op === "boolean") pass("4 events, each with seq and op");
    else fail(`events: ${JSON.stringify(r.events)}`);
    if (JSON.stringify(r.uses) === JSON.stringify([[1, 3, true], [2, 3, true], [3, 4, true]])) pass(`uses as [from_seq, to_seq, critical]: ${JSON.stringify(r.uses)}`);
    else fail(`uses: ${JSON.stringify(r.uses)}`);
    if (r.would_rebuild === undefined) pass("no rebuild plan unless asked");
    else fail("rebuild plan without from_seq");
  }
  console.log("(d2) what an edit to the cylinder (seq 2) would rebuild");
  {
    const r = JSON.parse(textOf(await tool.handler({ branch: "main", from_seq: 2 })));
    if (JSON.stringify(r.would_rebuild) === JSON.stringify([{ seq: 3, op: "boolean" }, { seq: 4, op: "fillet_edges" }]))
      pass(`would_rebuild: ${JSON.stringify(r.would_rebuild)}`);
    else fail(`would_rebuild: ${JSON.stringify(r.would_rebuild)}`);
    if (asked.some((u) => u.includes(`rebuild_from=${CYL}`))) pass("asked the kernel with the cylinder's event id");
    else fail(`asked: ${asked.join(" | ")}`);
  }
  console.log("(d3) an unknown sequence is refused, never guessed");
  {
    const res = await tool.handler({ branch: "main", from_seq: 99 });
    if (res.isError && /no event with sequence 99/.test(textOf(res))) pass(`refused: ${textOf(res).slice(0, 90)}`);
    else fail(`from_seq 99: ${textOf(res).slice(0, 200)}`);
  }
  console.log("(d4) it is a read");
  {
    const a = table.get("timeline_dependencies").annotations;
    if (a?.readOnlyHint === true) pass("annotated read-only");
    else fail(`annotations: ${JSON.stringify(a)}`);
  }
} finally {
  globalThis.fetch = real;
}

if (failures > 0) {
  console.error(`\nFAIL -- ${failures} problem(s)`);
  process.exit(1);
}
console.log("\nPASS -- the feature graph, in the agent's terms");
