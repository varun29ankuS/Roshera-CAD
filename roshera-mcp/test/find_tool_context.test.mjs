// find_tool in CONTEXT -- the right tool for what the agent is doing, every
// time, at the least token cost (Varun 2026-11-24: "the agent should be able
// to pick the right tool given the context with minimum token usage").
//
// Cases written 2026-11-24 BEFORE the context code existed, and frozen. The
// context is the domain the session is working in: the workbench it entered,
// or the domain of its last working call (a sketch, assembly, drawing or
// simulate tool); a tool that makes or changes a part puts it back on the
// part. Same calls, same query -> same answer.
//
//   (x1) a generic verb means the context's tool ("solve it" in a study)
//   (x2) no context: an ambiguous verb is SAID to be ambiguous, never picked
//   (x3) the context comes from the last working call, not only a bench
//   (x4) a shape-changing call puts the context back on the part
//   (x5) context lifts its own bench without stealing other intents
//   (x6) the answer is small: a resolved verb returns one match
//
// Run: npm run test:gates:build && node test/find_tool_context.test.mjs

import { buildTableWithControls } from "./.build/surface.js";
import { resetWorkingContext, noteWorkingCall } from "./.build/registry.js";

let failures = 0;
const fail = (m) => {
  console.error("  ✗ " + m);
  failures += 1;
};
const pass = (m) => console.log("  ✓ " + m);
const jsonOf = (res) => JSON.parse((res?.content ?? []).find((c) => c.type === "text").text);

const { table } = buildTableWithControls();
const find = async (query) => jsonOf(await table.get("find_tool").handler({ query }));
const bench = async (mode) => table.get("workbench").handler({ mode });
const first = (r) => r.matches?.[0]?.name;

async function inBench(b, query, expected) {
  resetWorkingContext();
  await bench(b);
  const r = await find(query);
  if (first(r) === expected) pass(`[${b}] "${query}" -> ${expected}`);
  else fail(`[${b}] "${query}" -> ${first(r)} (expected ${expected}; got ${(r.matches ?? []).map((m) => m.name).join(", ")})`);
  await bench("core_only");
  return r;
}

console.log("(x0) every meaning names a real tool in the bench it is listed under");
{
  const { VERB_MEANINGS } = await import("./.build/metatools.js");
  const { metaFor } = await import("./.build/registry.js");
  let bad = 0;
  for (const [verb, { means }] of Object.entries(VERB_MEANINGS))
    for (const [b, tool] of Object.entries(means)) {
      if (!table.has(tool)) { bad += 1; fail(`${verb}/${b}: '${tool}' is not a registered tool`); }
      else if (metaFor(tool).bench !== b) { bad += 1; fail(`${verb}/${b}: '${tool}' is in bench '${metaFor(tool).bench}'`); }
    }
  if (bad === 0) pass(`${Object.keys(VERB_MEANINGS).length} verbs, every meaning resolves in its bench`);
}

console.log("(x1) a generic verb means the context's tool");
await inBench("simulate", "solve it", "sim_static");
await inBench("simulate", "run it", "sim_static");
await inBench("sketch", "solve it", "psketch_solve");
await inBench("assembly", "solve it", "assembly_solve");
await inBench("sketch", "verify it", "psketch_certify");
await inBench("assembly", "verify it", "assembly_certify");
await inBench("drawing", "export it", "drawing_export_sheet");

console.log("(x2) no context: an ambiguous verb is said to be ambiguous");
{
  resetWorkingContext();
  await bench("core_only");
  const r = await find("solve it");
  const means = r.ambiguous?.means ?? {};
  if (means.sketch === "psketch_solve" && means.assembly === "assembly_solve" && means.simulate === "sim_static")
    pass(`"solve it" -> ambiguous: ${JSON.stringify(means)}`);
  else fail(`"solve it" without context: ${JSON.stringify(r).slice(0, 300)}`);
  if (typeof r.ambiguous?.resolve === "string" && r.ambiguous.resolve.length > 10) pass(`says how to resolve: ${r.ambiguous.resolve}`);
  else fail("no resolve hint");
  // A verb the part itself owns needs no context: the part is the default domain.
  const v = await find("verify it");
  if (first(v) === "verify_part" && !v.ambiguous) pass(`"verify it" with no domain -> verify_part (the part's own meaning)`);
  else fail(`"verify it" with no domain -> ${first(v)} ${JSON.stringify(v.ambiguous ?? {})}`);
  const e = await find("export it");
  if (first(e) === "export_part") pass(`"export it" with no domain -> export_part`);
  else fail(`"export it" with no domain -> ${first(e)}`);
}

console.log("(x3) the context comes from the last working call");
{
  resetWorkingContext();
  await bench("core_only");
  noteWorkingCall("sim_setup");
  const r = await find("solve it");
  if (first(r) === "sim_static" && /sim_setup|simulate/.test(r.resolved_by ?? "")) pass(`after sim_setup, "solve it" -> sim_static (${r.resolved_by})`);
  else fail(`after sim_setup: ${first(r)} / ${r.resolved_by}`);
  // A read in between (a render, a list) does not change what the agent is doing.
  noteWorkingCall("render_part");
  noteWorkingCall("kb_lookup");
  const r2 = await find("solve it");
  if (first(r2) === "sim_static") pass(`a render and a lookup in between keep the study's context`);
  else fail(`after render_part + kb_lookup: ${first(r2)}`);
  noteWorkingCall("psketch_constrain");
  const r3 = await find("solve it");
  if (first(r3) === "psketch_solve") pass(`then a sketch call: "solve it" -> psketch_solve`);
  else fail(`after psketch_constrain: ${first(r3)}`);
}

console.log("(x3b) a real dispatched call sets the context (the choke point, not a test hook)");
{
  resetWorkingContext();
  await bench("core_only");
  const real = globalThis.fetch;
  globalThis.fetch = async () => ({ ok: true, status: 200, text: async () => "{}" });
  try {
    await table.get("sim_session").handler({});
  } finally {
    globalThis.fetch = real;
  }
  const r = await find("solve it");
  if (first(r) === "sim_static") pass(`after a dispatched sim_session call: "solve it" -> sim_static`);
  else fail(`after a dispatched sim_session call: ${first(r)} ${JSON.stringify(r.ambiguous ?? {})}`);
  resetWorkingContext();
}

console.log("(x4) a shape-changing call puts the context back on the part");
{
  resetWorkingContext();
  noteWorkingCall("psketch_constrain");
  noteWorkingCall("fillet_edges");
  const r = await find("verify it");
  if (first(r) === "verify_part") pass(`sketch, then fillet_edges: "verify it" -> verify_part`);
  else fail(`after fillet_edges: ${first(r)} (${r.resolved_by})`);
}

console.log("(x5) context lifts its own bench without stealing other intents");
await inBench("simulate", "apply a force on the top face", "sim_setup");
await inBench("simulate", "fix the bottom face as the support", "sim_setup");
await inBench("simulate", "export the part as stl", "export_part");
await inBench("simulate", "select the top face", "select_face");
await inBench("sketch", "undo that", "timeline_undo");
await inBench("assembly", "make a box 10 by 20 by 30", "create_box");

// (x7) frozen 2026-11-24 before the per-domain word meanings existed: one
// word, a different meaning per domain.
console.log("(x7) a word means what it means in the domain");
await inBench("simulate", "pin the hole", "sim_setup"); // a pin SUPPORT
await inBench("assembly", "pin the lever to the base", "assembly_mate"); // a revolute MATE
await inBench("simulate", "the load is 300 N downward on the tab", "sim_setup");
await inBench("simulate", "add a constraint on the base face", "sim_setup"); // a support
await inBench("sketch", "add a constraint between the line and the arc", "psketch_constrain");
await inBench("sketch", "fix this point where it is", "psketch_constrain"); // X/Y coordinate constraints
await inBench("simulate", "what material is it made of", "sim_materials");
{
  // No domain: "aluminium" is not a study word, the question is the part's mass.
  resetWorkingContext();
  await bench("core_only");
  const r = await find("how heavy is this part in aluminium");
  if (first(r) === "mass_properties") pass(`no domain: "how heavy is this part in aluminium" -> mass_properties`);
  else fail(`no domain: "how heavy is this part in aluminium" -> ${first(r)} (${(r.matches ?? []).map((m) => m.name).join(", ")})`);
}

console.log("(x6) a resolved verb costs one match");
{
  resetWorkingContext();
  await bench("simulate");
  const r = await find("solve it");
  const tokens = Math.ceil(JSON.stringify(r).length / 4);
  if (r.matches?.length === 1 && tokens <= 150) pass(`"solve it" in simulate: 1 match, ~${tokens} tokens`);
  else fail(`"solve it" in simulate: ${r.matches?.length} matches, ~${tokens} tokens`);
  await bench("core_only");
}

resetWorkingContext();
if (failures > 0) {
  console.error(`\nFAIL -- ${failures} problem(s)`);
  process.exit(1);
}
console.log("\nPASS -- the right tool in context");
