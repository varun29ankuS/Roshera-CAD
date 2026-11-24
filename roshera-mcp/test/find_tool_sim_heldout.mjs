// find_tool HELD-OUT recall for STRENGTH STUDIES -- the simulate bench.
//
// Written 2026-11-24 BEFORE the vocabulary for "load / force / support" was
// changed and before any score was looked at, after a live search for "check
// if my bracket is strong enough under load" ranked import_step FIRST ("load"
// sat in the import/open synonym group). The cases are frozen: none may be
// edited to make the scorer look better. Where a phrasing is genuinely
// ambiguous, every legitimate tool is allowed and the comment says why.
//
// Node-runnable WITHOUT a live backend (gate build in test/.build; build first
// with `npm run test:gates:build`).
//
//   (s0) fixture integrity -- every allowed tool exists in the registry
//   (s1) scorecard          -- hit@1 / hit@any(top 5); every miss printed
//   (s2) ratchet floors     -- the MEASURED level, never an aspiration
//   (s3) no regression      -- "load" still finds import_step when it means a file
//
// Run: node test/find_tool_sim_heldout.mjs   (exit 0 = pass)

import { buildTable } from "./.build/surface.js";

let failures = 0;
const fail = (m) => {
  console.error("  ✗ " + m);
  failures += 1;
};
const pass = (m) => console.log("  ✓ " + m);
const jsonOf = (res) => {
  const t = (res?.content ?? []).find((c) => c.type === "text");
  return t ? JSON.parse(t.text) : null;
};

// A whole-study question is answered by starting the study (sim_mesh), by its
// run (sim_static), or by the walk-through playbook (kb_lookup) -- all three
// are a competent first grab.
const STUDY = ["sim_mesh", "sim_static", "kb_lookup"];

const CASES = [
  { query: "check if my bracket is strong enough under load", expected: STUDY },
  { query: "will this hold 500 N hanging off the end", expected: STUDY },
  { query: "run an FEA on the clamp", expected: STUDY },
  { query: "how much does the arm bend under the load", expected: STUDY }, // deflection is a result of the run
  { query: "find the peak von mises stress", expected: ["sim_static", "sim_session"] },
  // The safety factor is the run's answer, or the recorded study's.
  { query: "what is the safety factor on this part", expected: ["sim_static", "sim_session"] },
  { query: "mesh the part for analysis", expected: "sim_mesh" },
  { query: "make the elements smaller around the fillet", expected: "sim_mesh" },
  { query: "coarser mesh, it is taking too long", expected: "sim_mesh" },
  { query: "bolt the base face down and push on the tab with 200 N", expected: "sim_setup" },
  { query: "apply a force on the top face", expected: "sim_setup" },
  { query: "fix the bottom face as the support", expected: "sim_setup" },
  { query: "which aluminium alloys can I pick for the study", expected: "sim_materials" },
  { query: "yield strength of 6061-T6 for the analysis", expected: ["sim_materials", "kb_lookup"] },
  { query: "pick up the stress study where we left off", expected: "sim_session" },
  { query: "solve it", expected: "sim_static" },
];

const allowedOf = (c) => (Array.isArray(c.expected) ? c.expected : [c.expected]);
const table = buildTable();
const findTool = table.get("find_tool");

console.log("(s0) fixture integrity");
{
  let broken = 0;
  for (const c of CASES)
    for (const n of allowedOf(c))
      if (!table.get(n)) {
        broken += 1;
        fail(`fixture names a tool that does NOT exist: '${n}' ("${c.query}")`);
      }
  if (broken === 0) pass(`all expected tools across ${CASES.length} cases exist`);
}
if (failures > 0) process.exit(1);

console.log("(s1) scorecard");
let hit1 = 0;
let hitAny = 0;
for (const c of CASES) {
  const allowed = allowedOf(c);
  const names = (jsonOf(await findTool.handler({ query: c.query, limit: 5 }))?.matches ?? []).map((m) => m.name);
  const at1 = allowed.includes(names[0]);
  const anyN = allowed.some((n) => names.includes(n));
  if (at1) hit1 += 1;
  if (anyN) hitAny += 1;
  console.log(`  ${at1 ? "PASS@1 " : anyN ? "PASS@5 " : "MISS   "} "${c.query}" -> ${allowed.join("|")}${at1 ? "" : `  [got: ${names.join(", ")}]`}`);
}
console.log(`\n  hit@1 ${hit1}/${CASES.length}   hit@any ${hitAny}/${CASES.length}`);

console.log("(s2) ratchet floors");
// Measurement history:
//   2026-11-24 frozen cases, first run (load = import/open; the workflow
//              repeated in every sim_* purpose): hit@1 6/16, hit@any 11/16.
//   2026-11-24 load/force, support/hold and material groups; the walk-through
//              moved to the strength_study playbook; sim_setup/sim_materials
//              purposes name supports, forces, steels and aluminium alloys:
//              hit@1 9/16, hit@any 13/16. Still missed, honestly ambiguous or
//              face-dominated: "solve it" (sketch/assembly solvers share the
//              verb), "apply a force on the top face" and "bolt the base face
//              down and push..." (face tools win on "face").
//   2026-11-24 with no context, "solve it" is answered AMBIGUOUS, listing
//              sim_static among the meanings: hit@any 13 -> 14.
const FLOOR_AT1 = 9;
const FLOOR_ANY = 14;
if (hit1 >= FLOOR_AT1) pass(`hit@1 ${hit1} >= floor ${FLOOR_AT1}`);
else fail(`hit@1 ${hit1} < floor ${FLOOR_AT1}`);
if (hitAny >= FLOOR_ANY) pass(`hit@any ${hitAny} >= floor ${FLOOR_ANY}`);
else fail(`hit@any ${hitAny} < floor ${FLOOR_ANY}`);

console.log("(s3) a load is a force: 'under load' never offers a file import");
for (const q of ["check if my bracket is strong enough under load", "how much does the arm bend under the load"]) {
  const names = (jsonOf(await findTool.handler({ query: q, limit: 5 }))?.matches ?? []).map((m) => m.name);
  if (!names.includes("import_step")) pass(`"${q}" -> no import_step in the top 5`);
  else fail(`"${q}" -> import_step offered (${names.join(", ")}): "load" is being read as a file load`);
}

console.log("(s4) a file is still loaded by import_step");
for (const q of ["load the step file from the vendor", "open the STEP model"]) {
  const names = (jsonOf(await findTool.handler({ query: q, limit: 5 }))?.matches ?? []).map((m) => m.name);
  if (names[0] === "import_step") pass(`"${q}" -> import_step first`);
  else fail(`"${q}" -> ${names.join(", ")} (import_step not first)`);
}

if (failures > 0) {
  console.error(`\nFAIL (${failures})`);
  process.exit(1);
}
console.log("\nPASS");
