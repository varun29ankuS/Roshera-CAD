// find_tool recall — is the funnel's context-reach real, or theoretical?
//
// ROSHERA_MCP_SURFACE=minimal exposes ~18 direct tools; the other ~70 are
// reachable ONLY through find_tool → describe_tool → invoke. That reach is
// worth nothing if an agent phrasing a need in plain words cannot surface the
// right tool. This harness measures exactly that, offline: find_tool's ranking
// is deterministic and local to the table (no LLM, no embeddings, no backend),
// so the whole scorecard runs without a server.
//
// Node-runnable WITHOUT a live backend. Exercises the compiled dist/ directly
// (build first: `npm run build`).
//
//   (f0) fixture integrity — every expected tool name EXISTS in the registry
//        (expectations are derived from the table, never from memory)
//   (f1) recall scorecard  — ≥25 natural-language needs, biased to the long
//        tail outside MINIMAL_SURFACE; per-case hit@1 (ranked first) and
//        hit@any (anywhere in the top-5 returned set); every MISS printed with
//        query → expected → what actually came back
//   (f2) ratchet floors    — asserted at the MEASURED level, not an aspiration
//
// Run: node test/find_tool_recall.mjs   (exit 0 = pass, non-zero = fail)

import { buildTable, MINIMAL_SURFACE } from "../dist/surface.js";

let failures = 0;
const fail = (m) => {
  console.error("  ✗ " + m);
  failures += 1;
};
const pass = (m) => console.log("  ✓ " + m);
/** Parse the first text content block of a tool result as JSON. */
const jsonOf = (res) => {
  const t = (res?.content ?? []).find((c) => c.type === "text");
  return t ? JSON.parse(t.text) : null;
};

// ─────────────────────────────────────────────────────────────────────────────
// The fixture: realistic needs a competent engineer would phrase, paired with
// the tool they would expect. `core: true` marks the few residents of
// MINIMAL_SURFACE included as controls; everything else is long tail — the
// population the funnel exists for.
// ─────────────────────────────────────────────────────────────────────────────
const CASES = [
  // ── shaping / dressing (long tail) ──
  { query: "round off the sharp edges on this bracket", expected: "fillet_edges" },
  { query: "put a bevel on the top edge", expected: "chamfer_edges" },
  { query: "hollow out this part leaving 2mm walls", expected: "shell" },
  { query: "drill a bolt circle of 6 holes", expected: "drill_pattern" },
  { query: "move the part 10mm to the left", expected: "transform" },
  { query: "union all of these bodies into one solid", expected: "boolean_many" },
  { query: "loft a smooth surface through these profiles", expected: "nurbs_loft" },
  // ── interrogation / measurement (long tail) ──
  { query: "measure the distance between these two faces", expected: "measure_faces" },
  { query: "how far apart are these two parts", expected: "part_distance" },
  { query: "what are the overall dimensions of this part", expected: "dimension_part" },
  { query: "is this point inside the solid or in air", expected: "point_query" },
  { query: "shoot a ray and tell me what it hits first", expected: "ray_query" },
  { query: "can this part actually be machined on a CNC", expected: "dfm_check" },
  { query: "pick the face I am pointing at", expected: "select_face" },
  // ── assembly (long tail) ──
  { query: "check the two parts don't collide", expected: "assembly_interference" },
  { query: "mate the pin into the hole", expected: "assembly_mate" },
  { query: "add another copy of this part to the assembly", expected: "assembly_add_instance" },
  // ── GD&T / annotation (long tail) ──
  { query: "define datum A on this face", expected: "gdt_datum" },
  { query: "attach a flatness tolerance to the base face", expected: "gdt_fcf" },
  { query: "name this face so I can refer to it later", expected: "label_create" },
  // ── io / drawings / knowledge (long tail) ──
  { query: "export this for the printer", expected: "export_part" },
  { query: "bring in the STEP file the customer sent", expected: "import_step" },
  { query: "make a technical drawing of the part", expected: "make_drawing" },
  { query: "what clearance hole size does an M8 bolt need", expected: "kb_lookup" },
  // ── timeline (long tail) ──
  { query: "undo the last operation", expected: "timeline_undo" },
  { query: "show me the design history", expected: "timeline_history" },
  { query: "branch off so I can try an alternative", expected: "timeline_branch" },
  { query: "merge my experiment back into main", expected: "timeline_merge" },
  // ── sketch constraints (long tail) ──
  { query: "make these two sketch lines perpendicular", expected: "psketch_constrain" },
  { query: "how many degrees of freedom are left in the sketch", expected: "psketch_dof" },
  // ── core controls (already resident — the funnel should at least agree) ──
  { query: "how heavy is this part in aluminium", expected: "mass_properties", core: true },
  { query: "cut a slot through the plate", expected: "boolean", core: true },
  { query: "give me a sectioned view", expected: "section_view", core: true },
];

const table = buildTable();
const findTool = table.get("find_tool");

// ─────────────────────────────────────────────────────────────────────────────
console.log("(f0) fixture integrity: every expected tool exists in the ACTUAL registry");
{
  if (!findTool) fail("find_tool itself is missing from the table");
  else pass("find_tool present in the table");

  const missing = CASES.filter((c) => !table.get(c.expected));
  if (missing.length === 0)
    pass(`all ${CASES.length} expected tools exist in the registry (${table.all().length} tools total)`);
  else
    for (const c of missing)
      fail(`fixture names a tool that does NOT exist: '${c.expected}' (query: "${c.query}") — fix the fixture, expectations must come from the registry`);

  const longTail = CASES.filter((c) => !c.core);
  const leaked = longTail.filter((c) => MINIMAL_SURFACE.includes(c.expected));
  if (longTail.length >= 25)
    pass(`${longTail.length} long-tail cases (≥25 required) + ${CASES.length - longTail.length} core controls`);
  else fail(`only ${longTail.length} long-tail cases — need ≥25`);
  if (leaked.length === 0) pass("no long-tail case actually resides in MINIMAL_SURFACE (they measure the funnel, not the residents)");
  else for (const c of leaked) fail(`case marked long-tail but '${c.expected}' is in MINIMAL_SURFACE — mark it core or replace it`);
}

if (failures > 0) {
  console.error(`\nFAIL — fixture is broken (${failures} problem(s)); scorecard not run`);
  process.exit(1);
}

// ─────────────────────────────────────────────────────────────────────────────
console.log("(f1) recall scorecard: hit@1 / hit@any(top-5) per natural-language need");
const LIMIT = 5; // find_tool's default result count — hit@any means "in the top 5"
let hit1 = 0;
let hitAny = 0;
const misses = [];
for (const c of CASES) {
  const res = await findTool.handler({ query: c.query, limit: LIMIT });
  const out = jsonOf(res);
  const names = (out?.matches ?? []).map((m) => m.name);
  const at1 = names[0] === c.expected;
  const anyN = names.includes(c.expected);
  if (at1) hit1 += 1;
  if (anyN) hitAny += 1;
  const tag = at1 ? "PASS@1" : anyN ? `PASS@any(#${names.indexOf(c.expected) + 1})` : "MISS  ";
  console.log(`  ${tag}  "${c.query}" → ${c.expected}${at1 ? "" : `  [got: ${names.join(", ") || "(nothing)"}]`}`);
  if (!anyN) misses.push({ query: c.query, expected: c.expected, got: names });
}

const pct = (n) => ((100 * n) / CASES.length).toFixed(1) + "%";
console.log(`\n  hit@1   ${hit1}/${CASES.length}  (${pct(hit1)})`);
console.log(`  hit@any ${hitAny}/${CASES.length}  (${pct(hitAny)})  [expected anywhere in top ${LIMIT}]`);

if (misses.length > 0) {
  console.log(`\n  ── the ${misses.length} MISS(es): where the funnel fails to reach ──`);
  for (const m of misses) {
    console.log(`  MISS  query:    "${m.query}"`);
    console.log(`        expected: ${m.expected}`);
    console.log(`        returned: ${m.got.length ? m.got.join(", ") : "(no matches at all)"}`);
  }
}

// ─────────────────────────────────────────────────────────────────────────────
console.log("\n(f2) ratchet floors (encode observed reality, never aspiration)");
// RATCHET, not a target. Two measurements against the 109-tool registry:
//
//   2026-08-03 baseline: hit@1 = 19/33 (57.6%), hit@any = 24/33 (72.7%)
//   2026-08-03 after the SYNONYM_GROUPS vocabulary fix in `src/metatools.ts`:
//                        hit@1 = 25/33 (75.8%), hit@any = 32/33 (97.0%)
//
// The baseline's 9 misses were diagnosed as 8 vocabulary gaps + 1 ranking gap.
// Adding the missing intent words (move/translate, collide/interference,
// tolerance/flatness/GD&T, perpendicular/constraint, DOF/freedom, machinable/
// CNC, "heavy", and the plural "dimensions" — `tokenize` does no stemming)
// fixed exactly the 8, confirming the diagnosis. IDF weighting was already
// present and was never the problem: it cannot rank a query in which no
// informative token matched anything.
//
// The 1 survivor is the ranking case, untouched by vocabulary as predicted:
// "name this face so I can refer to it later" → label_rename outranks
// label_create even though "name" is already a synonym. Create-vs-mutate
// disambiguation is the next real piece of work here.
//
// The floors below are exactly the observed post-fix values — a retrieval
// regression fails this gate; an improvement passes and should then RAISE the
// floor to the new measurement. Never set these above what a run has shown.
// 2026-08-03 (later): structural ranker rework (stemming, prefix-anchored
// matching, purpose-word IDF, phrases, create-vs-mutate intent, one-edit
// fuzzy) measured hit@1 = 27/33, hit@any = 33/33 here — and, more importantly,
// lifted the UNFITTED test/find_tool_heldout.mjs from 27/88 to 44/88 hit@1.
// That held-out file is the generalisation number; this one is the ratchet.
const FLOOR_HIT1 = 27;
const FLOOR_HITANY = 33;
if (hit1 >= FLOOR_HIT1) pass(`hit@1 ${hit1}/${CASES.length} ≥ floor ${FLOOR_HIT1}`);
else fail(`hit@1 REGRESSED: ${hit1}/${CASES.length} < floor ${FLOOR_HIT1}`);
if (hitAny >= FLOOR_HITANY) pass(`hit@any ${hitAny}/${CASES.length} ≥ floor ${FLOOR_HITANY}`);
else fail(`hit@any REGRESSED: ${hitAny}/${CASES.length} < floor ${FLOOR_HITANY}`);

console.log(failures === 0 ? "\nPASS — funnel recall at or above the measured floor" : `\nFAIL — ${failures} problem(s)`);
process.exit(failures === 0 ? 0 : 1);
