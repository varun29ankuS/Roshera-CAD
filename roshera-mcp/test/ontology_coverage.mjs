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
//   (o4) no stale BENCH_OF — every explicit classification names a tool that
//        is STILL registered; a BENCH_OF entry for a removed tool is dead
//        vocabulary invisible through metaFor (its `?? "meta"` fallback only
//        ever looks a name UP, so a stale key that nothing looks up is never
//        observed that way — this reads the raw table instead)
//   (o5) MCP↔backend bench parity — `roshera-backend/api-server/src/
//        agent_registry.rs` carries its OWN `Bench` enum and serves it over
//        `GET /api/agent/tool-registry`; BENCH_OF is a second statement of
//        the same fact. Parsed directly from the checked-in Rust source (not
//        a live fetch, not a hand-maintained snapshot to forget refreshing —
//        the source itself IS the backend's truth, already in this tree), so
//        this runs with no backend process required. A tool benched one way
//        in TypeScript and another in Rust fails here.
//
// Run: node test/ontology_coverage.mjs   (exit 0 = pass, non-zero = fail)

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { buildTable, META_SURFACE } from "../dist/surface.js";
import { metaFor, explicitBenchTable } from "../dist/registry.js";
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

// ─────────────────────────────────────────────────────────────────────────────
console.log("(o4) no stale BENCH_OF: every explicit classification names a live tool");
{
  // Read the RAW table, not through metaFor — metaFor only ever answers a
  // lookup for a name someone already has in hand (a registered tool being
  // classified). A BENCH_OF key for a tool that no longer exists is never
  // looked up by anything, so metaFor can never observe it; it just sits in
  // the compiled table as dead vocabulary that misleads the next reader into
  // thinking that tool is still classified/still exists. explicitBenchTable()
  // exposes the keys directly so this gate can check them.
  const explicit = explicitBenchTable();
  const staleKeys = Object.keys(explicit).filter((n) => !table.has(n));
  if (staleKeys.length === 0)
    pass(
      `all ${Object.keys(explicit).length} BENCH_OF entries name a currently-registered tool`,
    );
  else
    for (const n of staleKeys)
      fail(
        `BENCH_OF names '${n}' (bench '${explicit[n]}') but no such tool is registered — ` +
          `stale classification; remove the dead entry from BENCH_OF in src/registry.ts`,
      );
}

// ─────────────────────────────────────────────────────────────────────────────
console.log("(o5) MCP↔backend bench parity (parsed from agent_registry.rs, no live server)");
{
  // The four MCP-only tools known to have no backend row at all — composition
  // tools the backend never curates, plus one genuine gap (see the note
  // below). Each entry needs BOTH directions asserted: it must actually be an
  // MCP tool with a classification, and it must actually be absent from the
  // backend parse — an allowlist entry that stops being true (the backend
  // grows the row) would silently mask that tool's bench from ever being
  // compared, which is the exact hole this section exists to close.
  const BACKEND_ABSENT = {
    workbench: "composition tool (bench-switch controller) — MCP-side only, not a kernel op",
    cad_program: "composition tool (batch dispatcher) — MCP-side only, not a kernel op",
    ask_choice: "notebook/choice-card builder — MCP-side only, not a kernel op",
    // Genuine gap, not a deliberate design choice: agent_registry.rs's curated
    // table was never updated when psketch_plane_from_face was classified
    // 'sketch' (105607ed). A live consumeRegistry() run would already report
    // this as inventory drift ("compiled but not in kernel") because it is
    // outside META_SURFACE — this just names the same fact statically.
    psketch_plane_from_face: "sketch-on-face helper — missing from agent_registry.rs's curated table (real gap, not by design)",
  };

  const backendPath = path.join(
    path.dirname(fileURLToPath(import.meta.url)),
    "..",
    "..",
    "roshera-backend",
    "api-server",
    "src",
    "agent_registry.rs",
  );
  let rustSource;
  try {
    rustSource = readFileSync(backendPath, "utf8");
  } catch (e) {
    fail(
      `cannot read backend registry source at ${backendPath} (${e.message}) — ` +
        `HOLE 2 cannot be gated without it; this must FAIL loudly, not skip`,
    );
    rustSource = null;
  }

  if (rustSource !== null) {
    const BENCH_WORDS = ["Core", "Sketch", "Assembly", "Drawing", "Analysis", "Labels", "Timeline"];
    const T_CALL = new RegExp(
      `^[ \\t]*t\\(\\s*[\\r\\n]+[ \\t]*"([a-zA-Z0-9_]+)",[ \\t]*[\\r\\n]+[ \\t]*(${BENCH_WORDS.join("|")}),`,
      "gm",
    );
    const backendPairs = new Map();
    for (const m of rustSource.matchAll(T_CALL)) backendPairs.set(m[1], m[2].toLowerCase());

    // Guard against a vacuous pass: if the parser matched nothing (or the
    // wrong count), an empty/short intersection would silently print "no
    // disagreements" while comparing almost nothing. Cross-check against the
    // file's OWN count assertion (`assert_eq!(tools.len(), N`) so a broken
    // regex fails loudly instead of passing quietly.
    const countMatch = rustSource.match(/assert_eq!\(\s*tools\.len\(\),\s*(\d+)/);
    if (!countMatch)
      fail(
        `could not find the 'assert_eq!(tools.len(), N' sanity literal in agent_registry.rs — ` +
          `cannot confirm the parser matched everything; treat any pass below as unproven`,
      );
    else {
      const expected = Number(countMatch[1]);
      if (backendPairs.size === expected)
        pass(`parser matched ${backendPairs.size} backend tool rows (== the file's own tools.len() assertion of ${expected})`);
      else
        fail(
          `parser matched ${backendPairs.size} backend tool rows but agent_registry.rs asserts tools.len() == ${expected} — ` +
            `the T_CALL regex is out of sync with the Rust source's formatting; fix the parser before trusting this gate`,
        );
    }

    // Per-bench census of the parsed backend table (o0-style visibility).
    const backendByBench = new Map();
    for (const [n, b] of backendPairs) {
      if (!backendByBench.has(b)) backendByBench.set(b, []);
      backendByBench.get(b).push(n);
    }
    for (const [bench, tools] of [...backendByBench.entries()].sort((a, b2) => b2[1].length - a[1].length))
      console.log(`      backend ${bench.padEnd(9)} ${String(tools.length).padStart(3)}`);

    const explicit = explicitBenchTable();
    const mcpNames = new Set(Object.keys(explicit));
    const backendNames = new Set(backendPairs.keys());

    // Hygiene of the BACKEND_ABSENT allowlist, both directions.
    for (const [n, reason] of Object.entries(BACKEND_ABSENT)) {
      if (!mcpNames.has(n))
        fail(`BACKEND_ABSENT lists '${n}' but it has no BENCH_OF entry — stale allowlist entry`);
      else if (backendNames.has(n))
        fail(
          `BACKEND_ABSENT lists '${n}' as missing from the backend, but agent_registry.rs now has a row for it ` +
            `(bench '${backendPairs.get(n)}') — remove the allowlist entry and let it be bench-compared below`,
        );
      else pass(`'${n}' has no backend row: ${reason}`);
    }

    // Every MCP-classified name not in the backend parse must be allowlisted —
    // an unallowlisted gap is a new asymmetry nobody decided was fine.
    const uncoveredMcpOnly = [...mcpNames].filter(
      (n) => !backendNames.has(n) && !(n in BACKEND_ABSENT),
    );
    for (const n of uncoveredMcpOnly)
      fail(
        `'${n}' is classified '${explicit[n]}' in BENCH_OF but has no row in agent_registry.rs, and is not ` +
          `on the BACKEND_ABSENT allowlist — either add it to the backend table or allowlist it here with a reason`,
      );

    // Every backend-parsed name must have SOME MCP classification (the
    // reverse asymmetry: the kernel curates a tool TypeScript never bench'd).
    const backendOnly = [...backendNames].filter((n) => !mcpNames.has(n));
    for (const n of backendOnly)
      fail(
        `agent_registry.rs benches '${n}' as '${backendPairs.get(n)}' but BENCH_OF has no entry for it at all`,
      );
    if (backendOnly.length === 0)
      pass(`every one of the ${backendNames.size} backend-benched tools has an MCP classification`);

    // The actual gate: for every name classified on BOTH sides, the bench
    // string must agree. This is the check HOLE 2 exists for — a tool benched
    // one way in TypeScript and another in Rust must fail here, not pass
    // silently the way it does today.
    const both = [...mcpNames].filter((n) => backendNames.has(n));
    const mismatches = both.filter((n) => explicit[n] !== backendPairs.get(n));
    if (mismatches.length === 0)
      pass(`bench agrees on all ${both.length} tools classified on both sides`);
    else
      for (const n of mismatches)
        fail(
          `bench MISMATCH for '${n}': BENCH_OF (TypeScript) says '${explicit[n]}', ` +
            `agent_registry.rs (Rust) says '${backendPairs.get(n)}'`,
        );
  }
}

console.log(
  failures === 0
    ? "\nPASS — every tool explicitly placed; the fallback holds only the allowlisted funnel"
    : `\nFAIL — ${failures} problem(s)`,
);
process.exit(failures === 0 ? 0 : 1);
