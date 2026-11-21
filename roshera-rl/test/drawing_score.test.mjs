/**
 * The drawing agent mode's scoring (agent-eval/drawing_score.mjs).
 *
 * The before/after grade, below-bar count and certificate come from the
 * KERNEL (the harness reads them over REST); only what the kernel cannot see
 * -- which verbs the agent tried and which were refused -- is read from the
 * agent's stream, and the reward says which is which. The reward is a NAMED
 * vector, never a sum (roshera-rl's rule: weighting is the trainer's choice).
 *
 * Every stream line here is the shape Claude Code's stream-json writes
 * (assistant `tool_use` blocks, user `tool_result` blocks), as classify.mjs
 * already reads it.
 */
import assert from "node:assert/strict";
import {
  LAYOUT_VERBS, stepsFromStream, drawingReward, drawingOutcome,
} from "../agent-eval/drawing_score.mjs";

const checks = [];
const check = (name, fn) => checks.push([name, fn]);

const use = (id, name, input) =>
  JSON.stringify({ type: "assistant", message: { content: [{ type: "tool_use", id, name, input }] } });
const result = (id, text, isError = false) =>
  JSON.stringify({
    type: "user",
    message: { content: [{ type: "tool_result", tool_use_id: id, is_error: isError, content: [{ type: "text", text }] }] },
  });

const stream = [
  use("t1", "mcp__roshera__kb_lookup", { kind: "playbook", key: "manufacturing_drawing" }),
  result("t1", '{"value":"..."}'),
  use("t2", "mcp__roshera__invoke", { name: "drawing_spaces", args: { drawing_id: "d" } }),
  result("t2", '{"grade":0.42}'),
  use("t3", "mcp__roshera__invoke", { name: "drawing_align", args: { drawing_id: "d", dimensions: ["d1", "d2"] } }),
  result("t3", '{"verb":"align","grade":"0.42 -> 0.55"}'),
  use("t4", "mcp__roshera__drawing_move_view", { drawing_id: "d", view: "RIGHT" }),
  result("t4", '{"error_code":"layout_verb_refused","refusal":"no_room"}', true),
  "not json at all",
  use("t5", "mcp__roshera__invoke", { name: "drawing_look", args: { drawing_id: "d", space: "view:FRONT" } }),
  result("t5", '{"caption":"FRONT"}'),
].join("\n");

check("steps unwrap invoke, keep direct calls, and pair each call with its result", () => {
  const steps = stepsFromStream(stream);
  assert.deepEqual(
    steps.map((s) => [s.i, s.tool, s.ok]),
    [[0, "kb_lookup", true], [1, "drawing_spaces", true], [2, "drawing_align", true],
     [3, "drawing_move_view", false], [4, "drawing_look", true]],
  );
  assert.deepEqual(steps[2].args, { drawing_id: "d", dimensions: ["d1", "d2"] });
  assert.equal(steps[3].refusal, "no_room", "the refusal's stable code is kept");
  assert.equal(steps[2].refusal, null);
});

check("a call with no result (the run was cut off) is not counted as ok", () => {
  const steps = stepsFromStream(use("t9", "mcp__roshera__drawing_align", { drawing_id: "d" }));
  assert.equal(steps.length, 1);
  assert.equal(steps[0].ok, null, "unknown, never assumed fine");
});

check("the layout verbs are exactly the hands MCP serves", () => {
  assert.deepEqual([...LAYOUT_VERBS].sort(), [
    "drawing_align", "drawing_move_dimension", "drawing_move_view",
    "drawing_reattach_leader", "drawing_remove_dimension", "drawing_remove_view",
  ]);
});

const sheet = (grade, below) => ({ grade, below_bar: below, bar: 0.8, worst: "title-block", by_rule: {} });

check("the reward is the kernel's before/after plus the transcript's verb counts, labelled", () => {
  const r = drawingReward({
    before: sheet(0.42, 7),
    after: sheet(0.55, 4),
    certificateAfter: { sound: true, complete: false },
    steps: stepsFromStream(stream),
  });
  assert.equal(r.grade_before, 0.42);
  assert.equal(r.grade_after, 0.55);
  assert.equal(r.grade_delta, 0.13);
  assert.equal(r.below_bar_before, 7);
  assert.equal(r.below_bar_after, 4);
  assert.equal(r.sound_after, true);
  assert.equal(r.complete_after, false);
  assert.equal(r.verbs_applied, 1);
  assert.equal(r.verbs_refused, 1);
  assert.equal(r.looks, 1);
  assert.deepEqual(r.refusals_by_code, { no_room: 1 });
  assert.match(r.sources.grade_before, /kernel/);
  assert.match(r.sources.verbs_applied, /transcript/);
  assert.ok(!("total" in r) && !("score" in r), "never scalarized");
});

check("a reading the harness could not take is ABSENT with a reason, never 0", () => {
  const r = drawingReward({ before: sheet(0.42, 7), after: null, certificateAfter: null, steps: [] });
  assert.equal(r.grade_after, undefined);
  assert.equal(r.grade_delta, undefined);
  assert.match(r.absent.grade_after, /no space map/);
  assert.match(r.absent.sound_after, /no certificate/);
});

check("float noise does not show as a grade change", () => {
  const r = drawingReward({
    before: sheet(0.1 + 0.2, 1), after: sheet(0.3, 1), certificateAfter: { sound: true, complete: true }, steps: [],
  });
  assert.equal(r.grade_delta, 0);
});

check("the outcome follows the closed roshera-rl taxonomy", () => {
  assert.equal(drawingOutcome({ code: 0, timedOut: false }), "COMPLETED");
  assert.equal(drawingOutcome({ code: null, timedOut: true }), "BUDGET_EXHAUSTED");
  assert.equal(drawingOutcome({ code: 1, timedOut: false }), "CRASHED");
});

let failed = 0;
for (const [name, fn] of checks) {
  try {
    fn();
    console.log(`  ok  ${name}`);
  } catch (e) {
    failed += 1;
    console.log(`  FAIL ${name}\n       ${e.message}`);
  }
}
console.log(`drawing_score: ${checks.length - failed}/${checks.length}`);
process.exit(failed ? 1 : 0);
