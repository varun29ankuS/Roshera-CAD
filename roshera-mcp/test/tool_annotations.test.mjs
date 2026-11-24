// Every tool says what it does to its world, as MCP annotations (effects.ts).
//
//   (a1) every registered tool carries readOnlyHint + destructiveHint
//   (a2) every name the effects table lists is a registered tool (no drift)
//   (a3) a tool annotated read-only is one the refusal cache also treats as
//        read-only (gates.ts) -- two independent lists that must agree
//   (a4) every delete / clear / remove tool is destructive, never read-only
//   (a5) a served tool keeps its own definition's effects
//   (a6) a shape-changing tool (create/modify modules) is never read-only,
//        except plane_from_face, which only derives a plane
//
// Run: npm run test:gates:build && node test/tool_annotations.test.mjs

import { buildTableWithControls } from "./.build/surface.js";
import { READ_TOOLS, ADDITIVE_TOOLS } from "./.build/effects.js";
import { isReadOnly } from "./.build/gates.js";
import { servedMeta } from "./.build/served_meta.js";

let failures = 0;
const fail = (m) => {
  console.error("  ✗ " + m);
  failures += 1;
};
const pass = (m) => console.log("  ✓ " + m);

const { table } = buildTableWithControls();
const all = table.all();

console.log("(a1) every tool is annotated");
{
  const missing = all.filter((e) => typeof e.annotations?.readOnlyHint !== "boolean" || typeof e.annotations?.destructiveHint !== "boolean");
  if (missing.length === 0) pass(`${all.length} tools carry readOnlyHint + destructiveHint`);
  else fail(`unannotated: ${missing.map((e) => e.name).join(", ")}`);
  const read = all.filter((e) => e.annotations.readOnlyHint).length;
  const add = all.filter((e) => !e.annotations.readOnlyHint && !e.annotations.destructiveHint).length;
  console.log(`    read ${read} · additive ${add} · changes/removes ${all.length - read - add}`);
}

console.log("(a2) the effects table names only registered tools");
{
  const stray = [...READ_TOOLS, ...ADDITIVE_TOOLS].filter((n) => !table.has(n));
  if (stray.length === 0) pass(`${READ_TOOLS.size} read + ${ADDITIVE_TOOLS.size} additive names, all registered`);
  else fail(`not registered: ${stray.join(", ")}`);
  const both = [...READ_TOOLS].filter((n) => ADDITIVE_TOOLS.has(n));
  if (both.length === 0) pass("no tool is both read and additive");
  else fail(`in both sets: ${both.join(", ")}`);
}

console.log("(a3) read-only agrees with the refusal cache's read-only");
{
  const disagree = all.filter((e) => e.annotations.readOnlyHint && !isReadOnly(e.name)).map((e) => e.name);
  if (disagree.length === 0) pass("every read-only tool is read-only to the refusal cache too");
  else fail(`annotated read-only but the cache treats them as state-changing: ${disagree.join(", ")}`);
}

console.log("(a4) delete / clear / remove tools are destructive");
{
  const bad = all.filter((e) => /(^|_)(delete|clear|remove)(_|$)/.test(e.name) && (e.annotations.readOnlyHint || !e.annotations.destructiveHint));
  const n = all.filter((e) => /(^|_)(delete|clear|remove)(_|$)/.test(e.name)).length;
  if (bad.length === 0) pass(`all ${n} delete/clear/remove tools are destructive`);
  else fail(`not destructive: ${bad.map((e) => e.name).join(", ")}`);
}

console.log("(a5) served tools keep their own effects");
{
  const served = all.filter((e) => servedMeta(e.name));
  const wrong = served.filter((e) => e.annotations.readOnlyHint !== (servedMeta(e.name).effects === "read_only"));
  if (served.length > 0 && wrong.length === 0) pass(`${served.length} served tools keep their declared effects`);
  else fail(`served tools: ${served.length}, mismatched: ${wrong.map((e) => e.name).join(", ")}`);
}

console.log("(a6) a shape-changing tool is not read-only");
{
  const { setShapeTools } = await import("./.build/registry.js");
  // The shape set is computed by surface.ts from the create/modify modules; re-derive it here the same way.
  const { ToolTable } = await import("./.build/registry.js");
  const { registerCreateTools } = await import("./.build/tools/create.js");
  const { registerModifyTools } = await import("./.build/tools/modify.js");
  const t = new ToolTable();
  registerCreateTools(t);
  registerModifyTools(t);
  void setShapeTools;
  const bad = t.names().filter((n) => n !== "plane_from_face" && table.get(n)?.annotations.readOnlyHint);
  if (bad.length === 0) pass(`none of the ${t.names().length} create/modify tools is read-only (plane_from_face aside)`);
  else fail(`read-only shape tools: ${bad.join(", ")}`);
}

if (failures > 0) {
  console.error(`\nFAIL -- ${failures} problem(s)`);
  process.exit(1);
}
console.log("\nPASS -- every tool says what it does");
