// The agent's MAP of the tool surface (2026-11-24, Varun: "I am only able to
// see 31 tools ... make sure it is structured properly for the agent to
// explore").
//
//   (c1) workbench('catalog') lists EVERY registered tool exactly once, under
//        its bench, each bench with what it is for, each tool with a gist
//   (c2) the catalog's live marks are exactly what is callable now
//   (c3) the map stays affordable (token estimate)
//   (c4) 'simulate' is a real bench: the five sim_* tools, switchable, and
//        gone from 'analysis'
//   (c5) find_tool results carry a gist, describe_tool the whole purpose;
//        gist keeps a whole first sentence and is not cut at "e.g."
//
// Run: npm run test:gates:build && node test/workbench_catalog.test.mjs

import { buildTableWithControls, MINIMAL_SURFACE } from "./.build/surface.js";
import { SWITCHABLE_BENCHES } from "./.build/workbench.js";
import { gist } from "./.build/metatools.js";

let failures = 0;
const fail = (m) => {
  console.error("  ✗ " + m);
  failures += 1;
};
const pass = (m) => console.log("  ✓ " + m);
const jsonOf = (res) => JSON.parse((res?.content ?? []).find((c) => c.type === "text").text);

const { table } = buildTableWithControls();
const wb = table.get("workbench");

console.log("(c1) the catalog lists every tool once, by bench");
const cat = jsonOf(await wb.handler({ mode: "catalog" }));
{
  const listed = cat.benches.flatMap((b) => b.tools.map((t) => t.name));
  const unique = new Set(listed);
  const all = table.names();
  if (listed.length === unique.size) pass(`no tool listed twice (${listed.length})`);
  else fail(`a tool is listed twice: ${listed.length} entries, ${unique.size} names`);
  const missing = all.filter((n) => !unique.has(n));
  if (missing.length === 0 && unique.size === all.length) pass(`every one of the ${all.length} registered tools is on the map`);
  else fail(`off the map: ${missing.join(", ")}`);
  if (cat.tool_count === all.length) pass(`tool_count ${cat.tool_count}`);
  else fail(`tool_count ${cat.tool_count} != ${all.length}`);

  const noFor = cat.benches.filter((b) => typeof b.for !== "string" || b.for.length < 10);
  if (noFor.length === 0) pass(`every bench says what it is for (${cat.benches.map((b) => b.bench).join(", ")})`);
  else fail(`benches without a purpose line: ${noFor.map((b) => b.bench).join(", ")}`);

  const noGist = cat.benches.flatMap((b) => b.tools).filter((t) => typeof t.does !== "string" || t.does.length < 8 || t.does.length > 91);
  if (noGist.length === 0) pass("every tool has a gist of 8..91 characters");
  else fail(`bad gists: ${noGist.map((t) => `${t.name}=${JSON.stringify(t.does)}`).join("; ")}`);

  for (const b of cat.benches)
    for (const t of b.tools) {
      const want = table.get(t.name) && b.bench;
      if (!want) fail(`${t.name} under ${b.bench} is not a registered tool`);
    }
}

console.log("(c2) live marks = what is callable now");
{
  const live = new Set(cat.benches.flatMap((b) => b.tools.filter((t) => t.live).map((t) => t.name)));
  const metaNames = cat.benches.find((b) => b.bench === "meta").tools.map((t) => t.name);
  const expected = new Set([...MINIMAL_SURFACE.filter((n) => table.has(n)), ...metaNames]);
  const extra = [...live].filter((n) => !expected.has(n));
  const absent = [...expected].filter((n) => !live.has(n));
  if (extra.length === 0 && absent.length === 0) pass(`core_only: ${live.size} tools marked live = the minimal surface`);
  else fail(`live marks wrong -- marked but not live: ${extra.join(",")}; live but unmarked: ${absent.join(",")}`);

  await wb.handler({ mode: "simulate" });
  const cat2 = jsonOf(await wb.handler({ mode: "catalog" }));
  const simLive = cat2.benches.find((b) => b.bench === "simulate").tools.every((t) => t.live === true);
  if (simLive && cat2.active_bench === "simulate") pass("after workbench('simulate') every sim tool is marked live");
  else fail(`after entering simulate: active ${cat2.active_bench}, sim tools live: ${simLive}`);
  await wb.handler({ mode: "core_only" });
}

console.log("(c3) the map stays affordable");
{
  const tokens = Math.ceil(JSON.stringify(cat).length / 4);
  if (tokens <= 6000) pass(`catalog ~${tokens} tokens (<= 6000)`);
  else fail(`catalog ~${tokens} tokens -- over the 6000 budget`);
}

console.log("(c4) 'simulate' is a real bench");
{
  const SIM = ["sim_materials", "sim_mesh", "sim_session", "sim_setup", "sim_static"];
  if (SWITCHABLE_BENCHES.includes("simulate")) pass("SWITCHABLE_BENCHES includes 'simulate'");
  else fail(`SWITCHABLE_BENCHES: ${SWITCHABLE_BENCHES.join(",")}`);
  const sim = cat.benches.find((b) => b.bench === "simulate");
  const names = sim ? sim.tools.map((t) => t.name).sort() : [];
  if (JSON.stringify(names) === JSON.stringify(SIM)) pass(`simulate = ${names.join(", ")}`);
  else fail(`simulate bench holds ${names.join(", ")}`);
  const analysis = cat.benches.find((b) => b.bench === "analysis").tools.map((t) => t.name);
  const stray = analysis.filter((n) => n.startsWith("sim_"));
  if (stray.length === 0) pass("no sim_* tool left in 'analysis'");
  else fail(`still in analysis: ${stray.join(",")}`);
  const st = jsonOf(await wb.handler({ mode: "simulate" }));
  if (st.switched === true && SIM.every((n) => st.exposed_bench_tools.includes(n))) pass(`entering simulate exposes the five (${st.exposed_count} live)`);
  else fail(`enter simulate: ${JSON.stringify(st).slice(0, 200)}`);
  await wb.handler({ mode: "core_only" });
}

console.log("(c5) search results carry a gist; describe_tool the whole purpose");
{
  const res = jsonOf(await table.get("find_tool").handler({ query: "will the bracket hold the load", limit: 5 }));
  const long = res.matches.filter((m) => m.purpose.length > 201);
  if (long.length === 0) pass(`every result purpose <= 201 chars (longest ${Math.max(...res.matches.map((m) => m.purpose.length))})`);
  else fail(`full purposes returned: ${long.map((m) => m.name).join(", ")}`);
  const bytes = JSON.stringify(res).length;
  if (bytes < 3200) pass(`a five-result search is ~${Math.ceil(bytes / 4)} tokens`);
  else fail(`a five-result search is ~${Math.ceil(bytes / 4)} tokens`);

  const d = jsonOf(await table.get("describe_tool").handler({ name: "sim_setup" }));
  if (d.purpose === table.get("sim_setup").description) pass("describe_tool returns the whole purpose");
  else fail("describe_tool's purpose is not the registered one");

  const g1 = gist("Measure two faces, e.g. a gap. Then more.");
  if (g1 === "Measure two faces, e.g. a gap.") pass(`gist skips "e.g.": ${JSON.stringify(g1)}`);
  else fail(`gist cut wrongly: ${JSON.stringify(g1)}`);
  const g3 = gist("THE FULL CERTIFICATE. Every mate solved and every pair checked. More text.");
  if (g3 === "THE FULL CERTIFICATE. Every mate solved and every pair checked.") pass(`a thin first sentence runs on: ${JSON.stringify(g3)}`);
  else fail(`thin first sentence: ${JSON.stringify(g3)}`);
  const g2 = gist("word ".repeat(80).trim(), 50);
  if (g2.length <= 51 && g2.endsWith("…") && !g2.includes("wor…")) pass(`gist cuts at a word: ${JSON.stringify(g2)}`);
  else fail(`gist cut: ${JSON.stringify(g2)}`);
}

if (failures > 0) {
  console.error(`\nFAIL -- ${failures} problem(s)`);
  process.exit(1);
}
console.log("\nPASS -- the map holds");
