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
    // BLOCKED, not failed. The scenario never ran, so NOTHING was measured —
    // recording this as a `soundness` check (as this did until 2026-08-17)
    // publishes a fabricated measurement: a sweep that could not reach the
    // backend at all scored "soundness 0/19", which reads as "the kernel
    // produced nineteen unsound results" when the true statement is "the
    // kernel was never asked". An absence is stated with its reason, never
    // defaulted to a zero. `blocked` below carries that reason, and `passed`
    // is forced false so an empty check list cannot score vacuously green.
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
    // A blocked scenario is never "passed": `Checks.passed` is vacuously true
    // over an empty item list, so without this guard a setup failure would
    // score as a clean green sweep.
    passed: setupError ? false : t.passed,
    /** Non-null when the scenario could not START. Nothing was measured. */
    blocked: setupError,
    // A scenario may set `knownRed: true` to declare "this is EXPECTED to
    // fail today — it documents a live kernel defect, not a broken test."
    // The harness still scores and prints it honestly (nothing here
    // suppresses a failing check); only run.mjs's exit code and the
    // scorecard's PASS/FAIL mark treat it specially. See
    // scenarios/18-multibody-honesty.mjs for the convention writeup — no
    // such mechanism existed before that scenario needed one.
    knownRed: !!scenario.knownRed,
    wallMs,
    checks: t.items,
    timings,
    crash: crash ? crash.message : null,
  };
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// ─── ANSI kit ─────────────────────────────────────────────────────────────
// Hue is a verdict: only the four state marks take colour. Hierarchy takes
// intensity (dim/bold), never hue. Every escape byte is gated on TTY; piped
// output is plain, greppable text. summarize() is untouched.
const TTY = !!process.stdout.isTTY;

// PASS/FAIL/KNOWN-RED are grades and share the traffic-light axis. BLOCKED is
// NOT a grade — it means nothing was measured — so it takes a hue off that
// axis entirely. A column of cyan reads "infrastructure", never "quality",
// which is the whole distinction this harness keeps getting wrong.
const HUE = { pass: 32, fail: 31, knownRed: 33, blocked: 36 };
const ink = (code, s) => (TTY ? `\x1b[${code}m${s}\x1b[0m` : String(s));
const dim = (s) => ink("2", s);

// Every state is triple-encoded — word, glyph, hue — so no channel is
// load-bearing alone: colour-blind eyes, `grep`, and monochrome screenshots
// all still sort the four states.
const STATES = {
  pass: { word: "PASS", glyph: "✓", hue: HUE.pass },
  fail: { word: "FAIL", glyph: "✗", hue: HUE.fail },
  knownRed: { word: "KNOWN-RED", glyph: "⚑", hue: HUE.knownRed },
  blocked: { word: "BLOCKED", glyph: "⊘", hue: HUE.blocked },
};

// Single source of truth for the four states — live line, stream, scorecard.
// They used to be re-derived by a ternary in each place, which is how the
// scorecard's copy quietly lost its blocked branch.
const stateOf = (r) =>
  r.blocked ? "blocked" : r.passed ? "pass" : r.knownRed ? "knownRed" : "fail";
const mark = (key) => {
  const st = STATES[key];
  return ink(`1;${st.hue}`, `${st.glyph} ${st.word}`);
};

const write = (s) => process.stdout.write(s);
const fmtSecs = (ms) => `${(ms / 1000).toFixed(1)}s`;
const fmtClock = (ms) => {
  const t = Math.floor(ms / 1000);
  return `${String(Math.floor(t / 60)).padStart(2, "0")}:${String(t % 60).padStart(2, "0")}`;
};
const termCols = () => (process.stdout.columns || 80) - 1;

// One rewritten status line while a scenario runs. Elapsed wall time is the
// only fact we truly have mid-run (checks arrive when runScenario resolves),
// so it is the only thing shown — and it is the fact that matters, because a
// spinner animates identically whether the kernel is about to return or has
// hung. A 2s scenario showing 00:45 tells you it died before any check fires.
// Returns an eraser.
function liveStatus(label) {
  if (!TTY) return () => {};
  const t0 = Date.now();
  const draw = () => {
    const txt = `  · ${label} ${fmtClock(Date.now() - t0)}`;
    write(`\r\x1b[2K${dim(txt.length > termCols() ? txt.slice(0, termCols()) : txt)}`);
  };
  draw();
  const iv = setInterval(draw, 500);
  return () => {
    clearInterval(iv);
    write(`\r\x1b[2K`);
  };
}

// The inter-scenario rate-limit pause, made visible instead of felt as lag.
async function holdVisible(ms) {
  if (!TTY) return sleep(ms);
  const t0 = Date.now();
  const iv = setInterval(() => {
    write(`\r\x1b[2K${dim(`  ·· rate-limit hold ${((Date.now() - t0) / 1000).toFixed(1)}s`)}`);
  }, 200);
  try {
    await sleep(ms);
  } finally {
    clearInterval(iv);
    write(`\r\x1b[2K`);
  }
}

function emitVerdict(r) {
  const key = stateOf(r);
  if (key === "blocked") {
    // No counts, no time: BLOCKED measured nothing, so it prints nothing
    // numeric. Zero is a measurement; this is the absence of one.
    write(`  ${mark(key)}${dim(" — nothing measured")}\n`);
    write(`     ${ink(HUE.blocked, `⊘ ${r.blocked}`)}\n`);
    return;
  }
  const cp = r.checks.filter((c) => c.passed).length;
  write(`  ${mark(key)}  ${dim(`${cp}/${r.checks.length} checks · ${fmtSecs(r.wallMs)}`)}\n`);
  for (const c of r.checks.filter((c) => !c.passed)) {
    write(`     ${ink(HUE.fail, "✗")} ${dim(`[${c.dim}]`)} ${c.name} — ${c.detail}\n`);
  }
}

/** Run the whole suite in order. */
export async function runSuite(scenarios, client, geom) {
  const results = [];
  for (let i = 0; i < scenarios.length; i++) {
    const s = scenarios[i];
    // A full 17-18 scenario sweep run back-to-back trips the live backend's
    // per-window rate limiter (measured 2026-08-08: every scenario after the
    // first got a 429 on its very first request). This is NOT concurrency —
    // runSuite is a strict sequential loop, one scenario at a time — it is
    // simply request VOLUME. A short pause between scenarios (not between
    // requests within one) keeps the suite under the limiter without
    // materially lengthening a sweep that already runs one scenario at a time.
    if (i > 0) await holdVisible(1500);

    write(`\n▶ [${i + 1}/${scenarios.length}] ${s.id} — ${s.title}\n`);

    const erase = liveStatus(`running ${s.id}`);
    let r;
    try {
      r = await runScenario(s, client, geom);
    } finally {
      erase(); // the verdict replaces the status line, in place
    }
    results.push(r);
    emitVerdict(r);
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
  // Blocked scenarios contribute NO checks, so they move no dimension tally.
  // That is the point: a dimension a blocked scenario would have exercised
  // reads as a smaller denominator, never as failed checks.
  const blocked = results.filter((r) => r.blocked).length;
  return {
    scenarios: { pass: scenariosPass, total: results.length, blocked },
    checks: { pass: checksPass, total: checksTotal },
    dimensions: dimTally,
  };
}

// ─── scorecard ────────────────────────────────────────────────────────────
// Eighth-block partials so a bar's length is the fraction, not a rounding of
// it: at 20 cells a whole-block bar quantises to 5% steps, which is wide
// enough to render 19/20 and 20/20 identically.
const EIGHTHS = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
function bar(frac, width) {
  const units = Math.round(Math.max(0, Math.min(1, frac)) * width * 8);
  const full = Math.floor(units / 8);
  const part = EIGHTHS[units % 8];
  return "█".repeat(full) + part + "░".repeat(width - full - (part ? 1 : 0));
}

/** Pretty ASCII scorecard. */
export function scorecard(results, summary) {
  const W = 74, ID = 34, RES = 13, CHK = 8, TIME = 7;
  const TBL = 2 + ID + RES + CHK + TIME;
  const L = [];

  L.push("");
  L.push("═".repeat(W));
  L.push("  AGENT-EVAL-α  SCORECARD".padEnd(W - 24) + dim(new Date().toISOString()));
  L.push("═".repeat(W));
  L.push("");
  L.push("  " + "SCENARIO".padEnd(ID) + "RESULT".padEnd(RES) + "CHECKS".padEnd(CHK) + "TIME".padEnd(TIME));
  L.push(dim("─".repeat(TBL)));

  for (const r of results) {
    const key = stateOf(r);
    const st = STATES[key];
    // Pad from the UNCOLOURED text: `mark()` carries escape bytes that
    // padEnd would count as visible width and misalign every column.
    const resTxt = `${st.glyph} ${st.word}`;
    const pad = " ".repeat(Math.max(0, RES - resTxt.length));
    // BLOCKED prints "—" for both counts and time. 0/0 and 0.0s are tallies,
    // and nothing was tallied — rendering zeros is exactly how "the kernel was
    // never asked" cosplays as "the kernel answered badly".
    const chkTxt =
      key === "blocked"
        ? "—".padEnd(CHK)
        : `${r.checks.filter((c) => c.passed).length}/${r.checks.length}`.padEnd(CHK);
    const timeTxt =
      key === "blocked" ? "—".padStart(TIME - 1) : fmtSecs(r.wallMs).padStart(TIME - 1);
    L.push(
      "  " +
        r.id.padEnd(ID).slice(0, ID) +
        mark(key) + pad + // hue lands on the verdict word only
        (key === "blocked" ? dim(chkTxt) + dim(timeTxt) : chkTxt + timeTxt),
    );
  }

  L.push(dim("─".repeat(TBL)));
  L.push("");
  L.push(dim("  SCORE DIMENSIONS"));
  for (const d of DIMS) {
    const t = summary.dimensions[d];
    if (!t || t.total === 0) continue; // never exercised ≠ 0% — silence is the honest render
    const frac = t.pass / t.total;
    L.push(
      `    ${d.padEnd(14)}${bar(frac, 20)} ` +
        dim(`${String(t.pass).padStart(3)}/${String(t.total).padEnd(3)}`) +
        dim(`${Math.round(frac * 100)}%`.padStart(5)),
    );
  }

  L.push("");
  // The blocked count was computed by `summarize` and then never printed, so
  // the one number that distinguishes "the kernel failed" from "the kernel was
  // never asked" existed in the struct and reached nobody.
  const blockedN = summary.scenarios.blocked || 0;
  L.push(
    `  SCENARIOS ${summary.scenarios.pass}/${summary.scenarios.total} passed` +
      (blockedN ? ` · ${ink(HUE.blocked, `${blockedN} blocked`)}` : "") +
      `    CHECKS ${summary.checks.pass}/${summary.checks.total} passed`,
  );
  L.push(dim(`  Σ scenario time ${fmtClock(results.reduce((a, r) => a + (r.wallMs || 0), 0))}`));
  L.push("═".repeat(W));
  L.push("");
  return L.join("\n");
}
