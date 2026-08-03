// ontology coverage — can a new tool silently vanish into the `meta` fallback?
//
// The defect this gate exists for ALREADY HAPPENED: `psketch_plane_from_face`
// was registered with no entry in `BENCH_OF` (src/registry.ts), so `metaFor`'s
// `?? "meta"` fallback classified it `meta` — invisible on the sketch bench and
// on every other bench — until a human noticed and reclassified it (105607ed).
// The fallback cannot distinguish "deliberately meta" from "nobody classified
// this yet"; this file makes that distinction explicit and loud.
//
// The invariant: `"meta"` is NEVER an explicit value in BENCH_OF — it is ONLY
// produced by the fallback. So `metaFor(name).bench === "meta"` is exactly the
// set of tools that took the fallback path, and every member must be on the
// allowlist below (each with a structural justification) or this gate fails.
//
// Node-runnable WITHOUT a live backend; exercises the compiled dist/ directly
// (build first: `npm run build`).
//
//   (o0) census            — tools-per-bench counts + total, printed so drift
//        is visible as a number, not only as pass/fail
//   (o1) total classification — every registered tool has an EXPLICIT bench;
//        a fallback (`meta`) tool fails unless allowlisted; allowlist hygiene
//        (no stale names, no entries that stopped being meta) is asserted too
//   (o2) no dead benches   — every bench the code declares has ≥1 tool, and
//        every observed bench is declared (no orphan vocabulary either way)
//   (o3) ratchet floor     — total registry size at the MEASURED level
//
// Run: node test/ontology_coverage.mjs   (exit 0 = pass, non-zero = fail)

import { buildTable, META_SURFACE } from "../dist/surface.js";
import { metaFor } from "../dist/registry.js";
import { SWITCHABLE_BENCHES } from "../dist/workbench.js";

let failures = 0;
const fail = (m) => {
  console.error("  ✗ " + m);
  failures += 1;
};
const pass = (m) => console.log("  ✓ " + m);

// ─────────────────────────────────────────────────────────────────────────────
// The ONLY tools allowed to reach the `meta` fallback, each with the reason it
// is deliberately meta. The structural test of every justification: a meta tool
// must reside in META_SURFACE (always-on in every mode), because `meta` is the
// one bench no `workbench()` call can ever expose — a fallback tool that is NOT
// always-on is invisible, which is precisely the psketch_plane_from_face bug.
// ─────────────────────────────────────────────────────────────────────────────
const DELIBERATELY_META = {
  find_tool: "the funnel's discovery verb — searches the table, is not IN a bench",
  describe_tool: "the funnel's schema reader — reads the table, is not IN a bench",
  invoke: "the funnel's dispatcher — reaches every bench, belongs to none",
};

const table = buildTable();
const names = table.names();

// ─────────────────────────────────────────────────────────────────────────────
console.log("(o0) census: tools per bench over the ACTUAL registry");
const byBench = new Map();
for (const n of names) {
  const b = metaFor(n).bench;
  if (!byBench.has(b)) byBench.set(b, []);
  byBench.get(b).push(n);
}
{
  const rows = [...byBench.entries()].sort((a, b) => b[1].length - a[1].length);
  for (const [bench, tools] of rows)
    console.log(`      ${bench.padEnd(9)} ${String(tools.length).padStart(3)}`);
  console.log(`      ${"TOTAL".padEnd(9)} ${String(names.length).padStart(3)}`);
  const fallthrough = (byBench.get("meta") ?? []).length;
  pass(
    `census: ${names.length} tools, ${names.length - fallthrough} explicitly ` +
      `classified, ${fallthrough} on the fallback path`,
  );
}

// ─────────────────────────────────────────────────────────────────────────────
console.log("(o1) total classification: no tool reaches the fallback unlisted");
{
  const fallthrough = (byBench.get("meta") ?? []).sort();

  // Every fallback tool must be deliberately meta — this is THE drift gate.
  const unclassified = fallthrough.filter((n) => !(n in DELIBERATELY_META));
  if (unclassified.length === 0)
    pass(
      `all ${names.length - fallthrough.length} non-meta tools carry an explicit ` +
        `bench; the ${fallthrough.length} fallback tools are all allowlisted`,
    );
  else
    for (const n of unclassified)
      fail(
        `UNCLASSIFIED tool '${n}' fell through to the 'meta' fallback — it is ` +
          `invisible on every workbench (the psketch_plane_from_face bug). ` +
          `Add it to BENCH_OF in src/registry.ts, or allowlist it HERE with a reason.`,
      );

  // Allowlist hygiene, both directions: no stale names, no entries that have
  // since been classified (an allowlisted-but-classified entry is dead text
  // that would mask a future regression back to the fallback).
  for (const n of Object.keys(DELIBERATELY_META)) {
    if (!table.has(n))
      fail(`allowlist names '${n}' but it is not in the registry — stale entry`);
    else if (metaFor(n).bench !== "meta")
      fail(
        `allowlist names '${n}' but it is classified '${metaFor(n).bench}' — ` +
          `remove the dead allowlist entry`,
      );
    else if (!META_SURFACE.includes(n))
      fail(
        `'${n}' is allowlisted as deliberately meta but is NOT in META_SURFACE — ` +
          `a meta tool outside the always-on surface is unreachable from every ` +
          `bench, which is exactly the invisibility this gate exists to prevent`,
      );
    else pass(`'${n}' deliberately meta: ${DELIBERATELY_META[n]}`);
  }
}

// ─────────────────────────────────────────────────────────────────────────────
console.log("(o2) no dead classifications: every declared bench is populated");
{
  // The full bench vocabulary the code declares: the switchable benches
  // (workbench.ts, what `workbench(mode)` can enter), plus the two fixed ones —
  // `core` (always-on residents) and `meta` (the funnel). This mirrors the
  // `Bench` union in src/registry.ts, which types cannot export to runtime.
  const DECLARED = ["core", ...SWITCHABLE_BENCHES, "meta"];

  for (const bench of DECLARED) {
    const count = (byBench.get(bench) ?? []).length;
    if (count > 0) pass(`bench '${bench}' populated (${count} tools)`);
    else
      fail(
        `bench '${bench}' is declared but has ZERO tools — a typo in BENCH_OF ` +
          `or dead vocabulary; either way, surface it`,
      );
  }

  // And the reverse: a bench observed in classifications that the declared
  // vocabulary does not know is a typo'd value hiding tools from every switch.
  const undeclared = [...byBench.keys()].filter((b) => !DECLARED.includes(b));
  if (undeclared.length === 0)
    pass(`no undeclared bench values in use (vocabulary: ${DECLARED.join(", ")})`);
  else
    for (const b of undeclared)
      fail(
        `tools are classified into undeclared bench '${b}' ` +
          `(${byBench.get(b).join(", ")}) — no workbench mode can ever expose them`,
      );
}

// ─────────────────────────────────────────────────────────────────────────────
console.log("(o3) ratchet floor (encodes observed reality, never aspiration)");
// Measured 2026-08-03 against the registry at feat/sketch-dcm-45: 109 tools,
// 106 explicitly classified + the 3 deliberately-meta funnel verbs. A shrink
// below the floor means tools were removed without this gate hearing about it;
// growth passes and should then RAISE the floor to the new measurement.
const FLOOR_TOTAL = 109;
if (names.length >= FLOOR_TOTAL)
  pass(`registry size ${names.length} ≥ floor ${FLOOR_TOTAL}`);
else
  fail(`registry SHRANK: ${names.length} tools < floor ${FLOOR_TOTAL}`);

console.log(
  failures === 0
    ? "\nPASS — every tool explicitly placed; the fallback holds only the allowlisted funnel"
    : `\nFAIL — ${failures} problem(s)`,
);
process.exit(failures === 0 ? 0 : 1);
