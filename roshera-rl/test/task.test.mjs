/**
 * Task-spec proof.
 *
 * A task without an explicit tool allowlist is not a task: `find_tool` ranks
 * by IDF over the tool corpus, so ANY new tool perturbs rankings corpus-wide.
 * A drifting action space makes two trajectories incomparable and trains a
 * policy against a moving target, so the allowlist is mandatory and frozen.
 *
 * A task without machine-checkable claims is also not a task — it would have
 * no ground truth, and scoring would fall back to a model's opinion, which is
 * the exact thing this environment exists to avoid.
 *
 * AND a claim must be written in the language `verify_claim` actually speaks
 * (roshera-mcp/src/tools/inspect.ts:63-99 — `{expr, bindings, expected,
 * tolerance?}` over the five closed measure kinds at inspect.ts:73-80,
 * mirroring the kernel's `Measurement` enum at
 * geometry-engine/src/readable/claim.rs:26-40). A claim phrased in any other
 * vocabulary cannot be checked at all, which is a broken task wearing ground
 * truth.
 */
import assert from "node:assert/strict";
import { defineTask, TASKS, taskById } from "../lib/task.mjs";

const checks = [];
const check = (name, fn) => checks.push([name, fn]);

const volumeClaim = {
  name: "volume", expr: "v",
  bindings: [{ var: "v", measure: { kind: "volume", part: "solid:0" } }],
  expected: 117809.724509617, tolerance: 117.8,
};

const valid = {
  id: "cylinder-r25-h60", prompt: "Build a cylinder of radius 25 and height 60.",
  toolAllowlist: ["create_cylinder", "verify_part"],
  claims: [volumeClaim],
  stepBudget: 12, tokenBudget: 40000, split: "train",
};

check("a valid task freezes and round-trips", () => {
  const t = defineTask(valid);
  assert.equal(t.id, "cylinder-r25-h60");
  assert.equal(t.stepBudget, 12);
  assert.throws(() => { t.id = "other"; }, TypeError,
    "a task must be immutable — a mutated spec mid-run silently invalidates its trajectories");
  assert.throws(() => { t.toolAllowlist.push("boolean"); }, TypeError,
    "the allowlist container must resist mutation too, not just the top-level scalar fields");
  assert.throws(() => { t.claims[0].tolerance = 999; }, TypeError,
    "each claim must resist mutation too, not just the top-level scalar fields");
  assert.throws(() => { t.claims[0].bindings[0].measure.part = "solid:9"; }, TypeError,
    "including the binding a claim is measured through");
});

check("an empty tool allowlist is refused", () => {
  assert.throws(() => defineTask({ ...valid, toolAllowlist: [] }), /allowlist/i);
});

check("a task with no claims is refused", () => {
  assert.throws(() => defineTask({ ...valid, claims: [] }), /claim/i);
});

check("an unknown split is refused", () => {
  assert.throws(() => defineTask({ ...valid, split: "maybe" }), /split/i);
});

check("a claim without a tolerance is refused", () => {
  const { tolerance, ...noTolerance } = volumeClaim;
  assert.throws(
    () => defineTask({ ...valid, claims: [noTolerance] }),
    /tolerance/i,
    "an exact float equality claim can never pass — geometry reproduces to ~4e-8",
  );
});

check("a claim with an infinite tolerance is refused", () => {
  assert.throws(
    () => defineTask({ ...valid, claims: [{ ...volumeClaim, tolerance: Infinity }] }),
    /tolerance/i,
    "an infinite tolerance makes the claim unfalsifiable — it can never fail at scoring time",
  );
});

check("a claim phrased as a bare `quantity` is refused", () => {
  // THE CRITICAL 3 REGRESSION. `{quantity:"radius", expected:25}` is not a
  // misspelling of verify_claim's schema: radius is not one of the five
  // measurable kinds, so this claim could never be checked and the episode
  // would have scored ground truth it never looked at.
  assert.throws(
    () => defineTask({
      ...valid,
      claims: [{ name: "radius", quantity: "radius", expected: 25, tolerance: 0.02 }],
    }),
    /expr/i,
  );
});

check("a claim naming a measure kind verify_claim cannot measure is refused", () => {
  assert.throws(
    () => defineTask({
      ...valid,
      claims: [{
        ...volumeClaim,
        bindings: [{ var: "r", measure: { kind: "radius", part: "solid:0" } }],
      }],
    }),
    /closed set/i,
    "the measure enum is closed on both sides of the wire (inspect.ts:73-80, claim.rs:26-40)",
  );
});

check("a binding missing the field its kind requires is refused", () => {
  // agent.rs:144-165 deserialises the tagged union strictly: a face_area
  // binding carrying a `part` is rejected on the wire, not measured.
  assert.throws(
    () => defineTask({
      ...valid,
      claims: [{ ...volumeClaim, bindings: [{ var: "a", measure: { kind: "face_area", part: "solid:0" } }] }],
    }),
    /integer `face` id/i,
  );
  assert.throws(
    () => defineTask({
      ...valid,
      claims: [{ ...volumeClaim, bindings: [{ var: "v", measure: { kind: "volume" } }] }],
    }),
    /needs `part`/i,
  );
});

check("a claim with no bindings is refused", () => {
  assert.throws(
    () => defineTask({ ...valid, claims: [{ ...volumeClaim, bindings: [] }] }),
    /binding/i,
    "an expression bound to no measurement is never compared against geometry",
  );
});

check("the seed set is non-empty and every claim is machine-checkable", () => {
  const KINDS = ["volume", "surface_area", "face_area", "edge_length", "constant"];
  assert.ok(TASKS.length >= 1);
  for (const t of TASKS) {
    assert.ok(t.toolAllowlist.length > 0);
    assert.ok(t.claims.length > 0);
    assert.ok(["train", "eval"].includes(t.split));
    for (const c of t.claims) {
      assert.equal(typeof c.expr, "string");
      for (const b of c.bindings) assert.ok(KINDS.includes(b.measure.kind));
    }
  }
});

check("the seed task's claims pin BOTH requested dimensions", () => {
  // πr²h and 2πr(r+h) are independent in (r, h), so a cylinder satisfying
  // both at these tolerances has the radius and height that were asked for.
  const t = taskById("cylinder-r25-h60");
  const volume = t.claims.find((c) => c.name === "volume");
  const area = t.claims.find((c) => c.name === "surface_area");
  assert.ok(Math.abs(volume.expected - Math.PI * 25 * 25 * 60) < 1e-9);
  assert.ok(Math.abs(area.expected - 2 * Math.PI * 25 * (25 + 60)) < 1e-9);
  // A claim whose band is wider than the error it exists to catch is theatre:
  // r = 25.1 moves the volume 0.8%, eight times the 1e-3 tolerance.
  const offBy = Math.PI * 25.1 * 25.1 * 60;
  assert.ok(Math.abs(offBy - volume.expected) > volume.tolerance,
    "a 0.1mm radius error must fail this claim, or the claim proves nothing");
});

check("taskById finds a seed task and misses cleanly", () => {
  assert.equal(taskById(TASKS[0].id).id, TASKS[0].id);
  assert.equal(taskById("no-such-task"), undefined);
});

for (const [name, fn] of checks) { fn(); process.stdout.write(`  ok - ${name}\n`); }
process.stdout.write(`\ntask: ${checks.length} checks passed\n`);
