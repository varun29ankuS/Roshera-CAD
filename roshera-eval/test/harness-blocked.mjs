#!/usr/bin/env node
/**
 * DRY VALIDATION for the harness's BLOCKED attribution.
 *
 * Runs NO backend. A scenario that cannot even START (its `clear_parts`
 * reset throws — an unreachable backend, a 429 from the rate limiter, a bad
 * identity) measured NOTHING. Until 2026-08-17 the harness recorded that as a
 * failed `soundness` check, so a sweep that never reached the server printed
 * "soundness 0/19" — a fabricated measurement that reads as nineteen unsound
 * kernel results when the true statement is "the kernel was never asked".
 * That is the exact shape of lie this suite exists to catch, in the suite's
 * own instrument.
 *
 * The three properties, each proven against the mutation it would miss:
 *
 *   B1  a blocked scenario reports `blocked` with the REASON, and records no
 *       checks at all      (mutation: attributing the failure to a dimension)
 *   B2  a blocked scenario is NOT passed — `Checks.passed` is vacuously true
 *       over an empty list (mutation: reporting the empty run as green)
 *   B3  a blocked scenario moves NO dimension tally — an unexercised
 *       dimension shrinks its denominator, it never gains failed checks
 *       (mutation: a fabricated zero in the scorecard)
 *
 * Usage: node test/harness-blocked.mjs   (exit 0 = the attribution is honest)
 */

import { runScenario, summarize } from "../lib/harness.mjs";

let failures = 0;
const check = (name, cond, detail = "") => {
  if (cond) {
    process.stdout.write(`  ok   ${name}\n`);
  } else {
    failures++;
    process.stdout.write(`  FAIL ${name}${detail ? ` — ${detail}` : ""}\n`);
  }
};

const REASON = "HTTP 429 rate limited";

/** A client whose reset throws — the scenario can never start. */
const blockedClient = {
  clearParts: async () => {
    throw new Error(REASON);
  },
};

/** A client that resets cleanly, so the scenario body runs. */
const workingClient = { clearParts: async () => {} };

const scenario = {
  id: "test-blocked",
  title: "a scenario that cannot start",
  run: async (_ctx, t) => {
    t.record("soundness", "the body ran", true, "");
  },
};

process.stdout.write("\nBLOCKED attribution\n");

const blocked = await runScenario(scenario, blockedClient, null);

// B1 — the reason is carried, and nothing is attributed to a dimension.
check("B1a blocked carries the reason", String(blocked.blocked ?? "").includes(REASON), `blocked=${JSON.stringify(blocked.blocked)}`);
check("B1b no checks are recorded at all", blocked.checks.length === 0, `recorded ${blocked.checks.length}: ${JSON.stringify(blocked.checks.map((c) => `${c.dim}/${c.name}`))}`);
check(
  "B1c nothing is attributed to the soundness dimension",
  !blocked.checks.some((c) => c.dim === "soundness"),
  "a setup failure is not a soundness measurement",
);

// B2 — an empty check list must not score green.
check("B2 a blocked scenario is not passed", blocked.passed === false, `passed=${blocked.passed}`);

// B3 — the scorecard's dimension tallies stay untouched.
const sum = summarize([blocked]);
check(
  "B3a the soundness tally is untouched (0 of 0, never 0 of N)",
  sum.dimensions.soundness.total === 0 && sum.dimensions.soundness.pass === 0,
  `soundness=${JSON.stringify(sum.dimensions.soundness)}`,
);
check("B3b the blocked count is surfaced", sum.scenarios.blocked === 1, `blocked=${sum.scenarios.blocked}`);
check("B3c no checks are counted", sum.checks.total === 0, `checks=${JSON.stringify(sum.checks)}`);

// CONTROL — the same scenario against a working client must behave normally,
// so the guard above cannot be passing by disabling the harness outright.
process.stdout.write("\nControl: the unblocked path still measures\n");
const ran = await runScenario(scenario, workingClient, null);
check("C1 an unblocked scenario is not marked blocked", ran.blocked === null || ran.blocked === undefined, `blocked=${JSON.stringify(ran.blocked)}`);
check("C2 an unblocked scenario records its checks", ran.checks.length === 1, `recorded ${ran.checks.length}`);
check("C3 an unblocked passing scenario passes", ran.passed === true, `passed=${ran.passed}`);
check("C4 its soundness tally IS moved", summarize([ran]).dimensions.soundness.total === 1, "the control proves B3a is not vacuous");

process.stdout.write(
  failures === 0
    ? "\nharness-blocked: all checks passed — a blocked scenario reports BLOCKED, not a fabricated zero.\n"
    : `\nharness-blocked: ${failures} check(s) FAILED.\n`,
);
process.exit(failures === 0 ? 0 : 1);
