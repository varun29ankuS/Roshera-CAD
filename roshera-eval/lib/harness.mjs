/**
 * AGENT-EVAL-alpha harness: the assertion engine + sequential scenario runner +
 * scorecard formatter.
 *
 * A scenario is a module that default-exports:
 *   { id, title, dims:[...], budgetMs, async run(ctx, t) }
 * where `ctx = { c (client), time(label,fn), geom }` and `t` is a Checks
 * collector. The runner clears the model before each scenario, times the whole
 * run, guards it, and rolls the per-check results up into a scorecard.
 *
 * Every check is tagged with a SCORE DIMENSION:
 *   correctness  — exact analytic / structural oracles
 *   soundness    — kernel soundness certificates
 *   honesty      — unsound geometry is flagged unsound (no lie slipped through)
 *   performance  — wall-clock budgets
 */

export const DIMS = ["correctness", "soundness", "honesty", "performance"];

const CERT_DIMS = [
  "brep_valid",
  "watertight",
  "manifold",
  "self_intersection_free",
  "tessellation_clean",
  "mesh_quality_clean",
];

/** Per-scenario assertion collector. */
export class Checks {
  constructor(scenarioId) {
    this.scenarioId = scenarioId;
    this.items = []; // { dim, name, passed, detail }
  }
  record(dim, name, passed, detail = "") {
    this.items.push({ dim, name, passed: !!passed, detail: String(detail) });
    return passed;
  }
  ok(name, cond, { dim = "correctness", detail = "" } = {}) {
    return this.record(dim, name, !!cond, detail);
  }
  eq(name, actual, expected, { dim = "correctness" } = {}) {
    const pass = actual === expected;
    return this.record(dim, name, pass, `got ${fmt(actual)}, expected ${fmt(expected)}`);
  }
  approxRel(name, actual, expected, relTol, { dim = "correctness" } = {}) {
    const err = expected === 0 ? Math.abs(actual) : Math.abs(actual - expected) / Math.abs(expected);
    const pass = Number.isFinite(actual) && err <= relTol;
    return this.record(
      dim,
      name,
      pass,
      `got ${fmt(actual)}, expected ${fmt(expected)} (rel err ${(err * 100).toFixed(3)}% <= ${(relTol * 100).toFixed(3)}%)`,
    );
  }
  approxAbs(name, actual, expected, absTol, { dim = "correctness" } = {}) {
    const err = Math.abs(actual - expected);
    const pass = Number.isFinite(actual) && err <= absTol;
    return this.record(dim, name, pass, `got ${fmt(actual)}, expected ${fmt(expected)} (abs err ${fmt(err)} <= ${fmt(absTol)})`);
  }
  substr(name, haystack, needle, { dim = "correctness" } = {}) {
    const hs = String(haystack ?? "");
    const pass = hs.includes(needle);
    return this.record(dim, name, pass, `"${needle}" ${pass ? "found in" : "NOT in"} "${hs.slice(0, 120)}"`);
  }
  /** Soundness: perception.sound must be true; lists the certified cert dims. */
  sound(name, perception, { dim = "soundness" } = {}) {
    const p = perception ?? {};
    const pass = p.sound === true;
    const trueDims = CERT_DIMS.filter((k) => p[k] === true);
    const failDims = CERT_DIMS.filter((k) => p[k] === false);
    const detail = pass
      ? `SOUND [${trueDims.join(",")}] chi=${p.euler}`
      : `NOT SOUND (failed: ${failDims.join(",") || "cheap verdict"}; open_edges=${p.open_edges})`;
    return this.record(dim, name, pass, detail);
  }
  /** Honesty: perception.sound must be FALSE (the kernel refused to lie). */
  unsound(name, perception, { dim = "honesty" } = {}) {
    const p = perception ?? {};
    const pass = p.sound === false;
    const detail = pass
      ? `honestly flagged UNSOUND (open_edges=${p.open_edges}, failed=${CERT_DIMS.filter((k) => p[k] === false).join(",")})`
      : `expected UNSOUND but kernel reported sound=${p.sound}`;
    return this.record(dim, name, pass, detail);
  }
  get passed() {
    return this.items.every((i) => i.passed);
  }
}

function fmt(v) {
  if (typeof v === "number") return Number.isInteger(v) ? String(v) : v.toFixed(4);
  return JSON.stringify(v);
}

/** Run one scenario end-to-end. Returns a result record. */
export async function runScenario(scenario, client, geom) {
  const t = new Checks(scenario.id);
  const timings = [];
  const time = async (label, fn) => {
    const s = Date.now();
    try {
      return await fn();
    } finally {
      timings.push({ label, ms: Date.now() - s });
    }
  };
  const ctx = { c: client, time, geom };

  // Clean slate before every scenario (part ids renumber; this is the reset).
  let setupError = null;
  try {
    await client.clearParts();
  } catch (e) {
    setupError = `clear_parts failed: ${e.message}`;
  }

  const start = Date.now();
  let crash = null;
  if (setupError) {
    t.record("soundness", "scenario setup (clear_parts)", false, setupError);
  } else {
    try {
      await scenario.run(ctx, t);
    } catch (e) {
      crash = e;
      t.record("soundness", "scenario completed without crashing", false, `threw: ${e.message}`);
    }
  }
  const wallMs = Date.now() - start;

  // Performance dimension: total wall clock vs budget.
  if (scenario.budgetMs) {
    t.record(
      "performance",
      `completed within budget (${scenario.budgetMs}ms)`,
      wallMs <= scenario.budgetMs,
      `wall ${wallMs}ms vs budget ${scenario.budgetMs}ms`,
    );
  }

  return {
    id: scenario.id,
    title: scenario.title,
    passed: t.passed,
    wallMs,
    checks: t.items,
    timings,
    crash: crash ? crash.message : null,
  };
}

/** Run the whole suite in order. */
export async function runSuite(scenarios, client, geom) {
  const results = [];
  for (const s of scenarios) {
    process.stdout.write(`\n▶ ${s.id} — ${s.title}\n`);
    const r = await runScenario(s, client, geom);
    results.push(r);
    const mark = r.passed ? "PASS" : "FAIL";
    process.stdout.write(`  ${mark}  (${r.wallMs}ms, ${r.checks.filter((c) => c.passed).length}/${r.checks.length} checks)\n`);
    for (const c of r.checks.filter((c) => !c.passed)) {
      process.stdout.write(`     ✗ [${c.dim}] ${c.name} — ${c.detail}\n`);
    }
  }
  return results;
}

/** Roll results into per-dimension + overall tallies. */
export function summarize(results) {
  const dimTally = Object.fromEntries(DIMS.map((d) => [d, { pass: 0, total: 0 }]));
  let checksPass = 0, checksTotal = 0;
  for (const r of results) {
    for (const c of r.checks) {
      checksTotal++;
      if (c.passed) checksPass++;
      if (dimTally[c.dim]) {
        dimTally[c.dim].total++;
        if (c.passed) dimTally[c.dim].pass++;
      }
    }
  }
  const scenariosPass = results.filter((r) => r.passed).length;
  return {
    scenarios: { pass: scenariosPass, total: results.length },
    checks: { pass: checksPass, total: checksTotal },
    dimensions: dimTally,
  };
}

/** Pretty ASCII scorecard. */
export function scorecard(results, summary) {
  const L = [];
  L.push("");
  L.push("═".repeat(74));
  L.push("  AGENT-EVAL-α  SCORECARD".padEnd(60) + new Date().toISOString());
  L.push("═".repeat(74));
  L.push("");
  L.push("  " + "SCENARIO".padEnd(34) + "RESULT".padEnd(8) + "CHECKS".padEnd(9) + "TIME");
  L.push("  " + "-".repeat(68));
  for (const r of results) {
    const cp = r.checks.filter((c) => c.passed).length;
    const ct = r.checks.length;
    const mark = r.passed ? "PASS ✓" : "FAIL ✗";
    L.push(
      "  " +
        r.id.padEnd(34) +
        mark.padEnd(8) +
        `${cp}/${ct}`.padEnd(9) +
        `${(r.wallMs / 1000).toFixed(1)}s`,
    );
  }
  L.push("  " + "-".repeat(68));
  L.push("");
  L.push("  SCORE DIMENSIONS");
  for (const d of DIMS) {
    const t = summary.dimensions[d];
    if (t.total === 0) continue;
    const pct = ((t.pass / t.total) * 100).toFixed(0);
    const bar = "█".repeat(Math.round((t.pass / t.total) * 20)).padEnd(20, "░");
    L.push(`    ${d.padEnd(14)} ${bar} ${t.pass}/${t.total} (${pct}%)`);
  }
  L.push("");
  L.push(
    `  SCENARIOS: ${summary.scenarios.pass}/${summary.scenarios.total} passed` +
      `    CHECKS: ${summary.checks.pass}/${summary.checks.total} passed`,
  );
  L.push("═".repeat(74));
  L.push("");
  return L.join("\n");
}
