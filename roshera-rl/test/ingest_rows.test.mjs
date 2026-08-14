/**
 * Rows proof — the pure half of ingestion.
 *
 * Fixtures are derived from a REAL saved trajectory, not invented:
 * `test/fixtures/complete.jsonl` starts from
 * .superpowers/sdd/2026-08-13-rl-episode-loop-slice1/live-durability-verified/
 * cylinder-r25-h60-0-0.jsonl (a live run against a real kernel: two
 * re-issuable recipe steps, one carrying a reissue mapping, one stating why
 * it has none) with a `provenance` block added (the format moved after that
 * file was written — see lib/trajectory.mjs) and one refusal step injected,
 * because grepping every saved trajectory in that slice turned up zero
 * refusals. The refusal's shape (`{gate, reason}`) is episode.mjs's own
 * (episode.mjs:382-387); the gate name and reason template are copied
 * verbatim from roshera-mcp/src/gates.ts:602-612 (unsoundBaseGateRefusal) —
 * see test/fixtures/build_fixtures.mjs for the exact provenance.
 *
 * `test/fixtures/malformed.jsonl` is a real header line followed by a
 * truncated JSON line — the shape a killed process leaves behind.
 *
 * The provenance-absent case does not need its own fixture file: every
 * trajectory saved before `buildProvenance` existed (first-live-trajectory
 * .jsonl, live-isolated-8/*, live-durability-verified/* before this test's
 * own edit) already looks exactly like this — no `provenance` key at all,
 * not even a stated absence. This test reconstructs that real shape by
 * stripping the key back out of the complete fixture's own header.
 */
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join, dirname } from "node:path";
import { rowsFromTrajectory } from "../lib/ingest/rows.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const fixturesDir = join(here, "fixtures");
const completePath = join(fixturesDir, "complete.jsonl");
const malformedPath = join(fixturesDir, "malformed.jsonl");
const completeText = readFileSync(completePath, "utf8");
const malformedText = readFileSync(malformedPath, "utf8");

/** Strip `provenance` back out of the complete fixture's header line. */
function withoutProvenance(text) {
  const lines = text.trim().split("\n");
  const header = JSON.parse(lines[0]);
  delete header.provenance;
  lines[0] = JSON.stringify(header);
  return lines.join("\n") + "\n";
}

const checks = [];
const check = (name, fn) => checks.push([name, fn]);

// ─── purity: no database, no filesystem, no clock inside the module ───────
check("rows.mjs imports no filesystem module and constructs no Date", () => {
  const src = readFileSync(join(here, "..", "lib", "ingest", "rows.mjs"), "utf8");
  assert.ok(!/from\s+["']node:fs/.test(src), "rows.mjs must not import node:fs");
  assert.ok(!/\bnew Date\(/.test(src), "rows.mjs must not read the clock");
  assert.ok(!/\bfetch\(/.test(src), "rows.mjs must not perform network I/O");
});

// ─── complete trajectory: episode row, attributable carried through ───────
check("a complete trajectory maps to one episode row with attributable carried through", () => {
  const r = rowsFromTrajectory(completeText, { path: completePath });
  assert.equal(r.quarantine, undefined, "a well-formed file carries no quarantine key");
  assert.ok(r.episode, "an episode row is produced");
  assert.equal(r.episode.task_id, "cylinder-r25-h60");
  assert.equal(r.episode.seed, 0);
  assert.equal(r.episode.outcome, "COMPLETED");
  assert.equal(r.episode.attributable, true,
    "the fixture's provenance.attributable is true and must be carried through unchanged");
  assert.equal(r.run.attributable, true);
  // `kernel.reported_by` is always "server" — rows.mjs must pass it through
  // unmodified, never rewrite or launder it.
  assert.equal(r.run.provenance.kernel.reported_by, "server");
  assert.equal(r.run.provenance.kernel.sha, "3a9375f9");
});

// ─── steps: index, tool, duration ──────────────────────────────────────────
check("steps map with their index, tool and duration", () => {
  const r = rowsFromTrajectory(completeText, { path: completePath });
  assert.equal(r.steps.length, 4);
  assert.deepEqual(
    r.steps.map((s) => [s.index, s.tool, s.duration_ms]),
    [
      [0, "timeline_checkpoint", 69],
      [1, "create_cylinder", 470],
      [2, "verify_part", 152],
      [3, "boolean_subtract", 88],
    ],
  );
  assert.equal(r.steps[0].episode_id, r.episode.episode_id,
    "every step row is tied to the episode it belongs to");
});

// ─── refusals: their own rows, carrying gate and reason ───────────────────
check("refusals become their own rows carrying gate and reason", () => {
  const r = rowsFromTrajectory(completeText, { path: completePath });
  assert.equal(r.refusals.length, 1, "exactly one of the four steps was a refusal");
  const refusal = r.refusals[0];
  assert.equal(refusal.step_index, 3);
  assert.equal(refusal.gate, "unsound_base");
  assert.match(refusal.reason, /UNSOUND by the kernel's live verdict/);
  assert.equal(refusal.episode_id, r.episode.episode_id);
});

// ─── claims: expected, computed, verified ──────────────────────────────────
check("claim results carry expected, computed and verified", () => {
  const r = rowsFromTrajectory(completeText, { path: completePath });
  assert.equal(r.claims.length, 2);
  const volume = r.claims.find((c) => c.name === "volume");
  assert.ok(volume, "the volume claim survives");
  assert.equal(volume.verified, true);
  assert.equal(volume.expected, 117809.72450961724);
  assert.equal(volume.computed, 117790.346542991);
  const area = r.claims.find((c) => c.name === "surface_area");
  assert.equal(area.verified, true);
  assert.equal(typeof area.tolerance_used, "number");
});

// ─── recipe steps: reissue mapping ─────────────────────────────────────────
check("recipe steps carry their reissue mapping", () => {
  const r = rowsFromTrajectory(completeText, { path: completePath });
  assert.equal(r.recipeSteps.length, 3);
  const create = r.recipeSteps.find((s) => s.op_kind === "create_cylinder_3d");
  assert.ok(create.reissue, "the create step has a real reissue mapping");
  assert.equal(create.reissue.method, "POST");
  assert.equal(create.reissue.path, "/api/geometry/cylinder");
  assert.equal(create.reissue.body.radius, 25);
  assert.equal(create.reissue_absent_reason, null);

  const setName = r.recipeSteps.find((s) => s.op_kind === "set_name");
  assert.equal(setName.reissue, null,
    "set_name genuinely has no re-issue route in the real recipe");
  assert.match(setName.reissue_absent_reason, /no re-issue mapping is defined for `set_name`/);

  const boolean = r.recipeSteps.find((s) => s.op_kind === "boolean_union");
  assert.ok(boolean.reissue, "the boolean step has a real reissue mapping");
  assert.equal(boolean.reissue.method, "POST");
  assert.equal(boolean.reissue.path, "/api/geometry/boolean");
  assert.deepEqual(boolean.reissue.symbolic_operands, ["object_a", "object_b"]);
  assert.equal(boolean.reissue.body.object_a, "solid:0");
  assert.equal(boolean.reissue.body.object_b, "solid:1");
});

// ─── recipe row + solids + lineage edges (v1 scope) ────────────────────────
check("the recipe row and derived solids rows reflect the real recipe", () => {
  const r = rowsFromTrajectory(completeText, { path: completePath });
  assert.ok(r.recipe, "a recipe row is produced");
  assert.equal(r.recipe.reference, "61a53497-dbec-48d0-b789-45f4cbe803ee");
  assert.equal(r.recipe.step_count, 3);

  assert.equal(r.solids.length, 2,
    "create_cylinder_3d produced solid:0, boolean_union produced solid:2");
  assert.deepEqual(r.solids.map((s) => s.token).sort(), ["solid:0", "solid:2"]);
  const created = r.solids.find((s) => s.token === "solid:0");
  assert.equal(created.produced_by_sequence, 85);
  const merged = r.solids.find((s) => s.token === "solid:2");
  assert.equal(merged.produced_by_sequence, 93);
  assert.equal(merged.op_kind, "boolean_union");
});

// ─── lineage edges (v1 scope): the actual cross product, not just its size ─
check("lineage edges are the real recipe step's inputs x outputs cross product", () => {
  const r = rowsFromTrajectory(completeText, { path: completePath });
  // create_cylinder_3d has no inputs and set_name has no outputs — neither
  // contributes an edge under the v1 rule (see the module docstring). Only
  // boolean_union carries both, so it is the sole source of edges here:
  // two consumed operands x one produced output = 2 edges.
  assert.equal(r.lineageEdges.length, 2);
  const byFrom = Object.fromEntries(r.lineageEdges.map((e) => [e.from, e]));
  assert.equal(byFrom["solid:0"].to, "solid:2");
  assert.equal(byFrom["solid:1"].to, "solid:2");
  for (const e of r.lineageEdges) {
    assert.equal(e.via_sequence, 93);
    assert.equal(e.op_kind, "boolean_union");
    assert.equal(e.episode_id, r.episode.episode_id);
  }
});

// ─── provenance absent → rows still produced, flagged unattributable ──────
check("a trajectory whose provenance is absent still produces rows but with attributable: false", () => {
  const text = withoutProvenance(completeText);
  const r = rowsFromTrajectory(text, { path: "legacy.jsonl" });
  assert.equal(r.quarantine, undefined, "an absent provenance block is not a malformed file");
  assert.equal(r.episode.attributable, false);
  assert.equal(r.run.attributable, false);
  assert.equal(typeof r.run.provenance.absent, "string");
  assert.ok(r.run.provenance.absent.length > 0);
  // rows are NOT dropped — the corpus must not look cleaner than it is.
  assert.equal(r.steps.length, 4);
  assert.equal(r.refusals.length, 1);
  assert.equal(r.claims.length, 2);
  assert.equal(r.recipeSteps.length, 3);
  assert.equal(r.lineageEdges.length, 2, "lineage derivation runs the same regardless of attributable");
});

// ─── recipe_ref stated absence (every non-COMPLETED outcome) ──────────────
// This is the COMMON case for any episode that never reached terminal
// scoring, not an edge case: `unscoredFor` (lib/episode.mjs:94-109) is the
// REAL production code that builds it, for every one of BUDGET_EXHAUSTED /
// INVALID_ACTION / CRASHED / RATE_LIMITED / SETUP_FAILED, and returns
// `recipeRef: { absent: reason }` — a bare object with no `steps`, no
// `reference`, nothing else. The reason string below is copied verbatim
// from `NO_TERMINAL_SCORING.BUDGET_EXHAUSTED` (episode.mjs:50).
check("a stated-absence recipe_ref (every non-COMPLETED outcome) degrades to an absent recipe row and no derived rows", () => {
  const lines = completeText.trim().split("\n");
  const terminal = JSON.parse(lines[lines.length - 1]);
  const reason = "the step or token budget ran out before the policy declared done, so terminal verification never ran";
  terminal.outcome = "BUDGET_EXHAUSTED";
  terminal.recipe_ref = { absent: reason };
  lines[lines.length - 1] = JSON.stringify(terminal);
  const text = lines.join("\n") + "\n";

  const r = rowsFromTrajectory(text, { path: "unscored.jsonl" });
  assert.equal(r.quarantine, undefined, "a stated absence is not a malformed file");
  assert.equal(r.episode.outcome, "BUDGET_EXHAUSTED");
  assert.ok(r.recipe, "a recipe row is still produced, carrying the absence");
  assert.equal(r.recipe.absent, reason);
  assert.equal("reference" in r.recipe, false,
    "a bare {absent} recipe_ref carries no reference — the row must not invent one");
  assert.deepEqual(r.recipeSteps, []);
  assert.deepEqual(r.solids, []);
  assert.deepEqual(r.lineageEdges, []);
});

// ─── malformed file: quarantine entry naming the reason, never a throw ────
check("a malformed file produces a quarantine entry naming the reason rather than throwing", () => {
  const r = rowsFromTrajectory(malformedText, { path: malformedPath });
  assert.equal(r.run, null);
  assert.equal(r.episode, null);
  assert.deepEqual(r.steps, []);
  assert.deepEqual(r.refusals, []);
  assert.deepEqual(r.claims, []);
  assert.equal(r.recipe, null);
  assert.deepEqual(r.recipeSteps, []);
  assert.deepEqual(r.solids, []);
  assert.deepEqual(r.lineageEdges, []);
  assert.equal(r.quarantine.length, 1);
  assert.equal(r.quarantine[0].path, malformedPath);
  assert.match(r.quarantine[0].reason, /not valid JSON/i);
});

// ─── further malformed shapes: absent header / absent terminal ────────────
check("a file with no header record is quarantined, not thrown", () => {
  const text = '{"kind":"step","i":0,"action":{"tool":"create_cylinder","args":{}},"result_digest":null,"reward":{"components":{}},"refusal":null,"ms":1}\n';
  const r = rowsFromTrajectory(text, { path: "no-header.jsonl" });
  assert.equal(r.quarantine.length, 1);
  assert.match(r.quarantine[0].reason, /no header record/i);
});

check("a file with no terminal record is quarantined, not thrown", () => {
  const lines = completeText.trim().split("\n");
  const withoutTerminal = lines.slice(0, -1).join("\n") + "\n";
  const r = rowsFromTrajectory(withoutTerminal, { path: "no-terminal.jsonl" });
  assert.equal(r.quarantine.length, 1);
  assert.match(r.quarantine[0].reason, /no terminal record/i);
});

// ─── run identity: shared producer collapses to one run, never throws ─────
check("two episodes sharing kernel/mcp/policy/harness/split collapse onto the same run_id", () => {
  const header = JSON.parse(completeText.trim().split("\n")[0]);
  const sibling = { ...header, task_id: "cylinder-r10-h20", seed: 5 };
  const rest = completeText.trim().split("\n").slice(1);
  const siblingText = [JSON.stringify(sibling), ...rest].join("\n") + "\n";

  const a = rowsFromTrajectory(completeText, { path: completePath });
  const b = rowsFromTrajectory(siblingText, { path: "sibling.jsonl" });
  assert.equal(a.run.run_id, b.run.run_id,
    "same producer identity (kernel/mcp/policy/harness/split) is one run");
  assert.notEqual(a.episode.episode_id, b.episode.episode_id,
    "different task_id/seed under the same run is a different episode");
});

for (const [name, fn] of checks) { fn(); process.stdout.write(`  ok - ${name}\n`); }
process.stdout.write(`\ningest_rows: ${checks.length} checks passed\n`);
