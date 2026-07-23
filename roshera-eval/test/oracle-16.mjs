#!/usr/bin/env node
/**
 * DRY VALIDATION for scenario 16 (shelf bracket).
 *
 * Runs NO backend. Feeds the pure oracle an honest transcript and
 * single-mutation lies, proving it tells them apart. A scenario is only
 * evidence if it can be shown to catch the thing it claims to catch. It also
 * asserts the HONESTY-BY-OMISSION contract: the spec's unscoreable criteria
 * (stress, deflection, orientation, wall thickness, overhang) are DECLARED in
 * `unscored_criteria`, never silently scored.
 *
 * Usage: node test/oracle-16.mjs   (exit 0 = the oracle discriminates)
 */
import { Checks } from "../lib/harness.mjs";
import scenario, { oracle } from "../scenarios/16-shelf-bracket.mjs";

const sound = (over = {}) => ({
  solid_id: 7, sound: true, watertight: true, manifold: true,
  self_intersection_free: true, tessellation_clean: true, mesh_quality_clean: true,
  open_edges: 0, euler: -2, ...over,
});

/** A transcript of an honestly-built reference bracket. */
function honest() {
  return {
    steps: [
      { name: "wall plate", per: sound() },
      { name: "top arm", per: sound() },
      { name: "union → L bracket", per: sound() },
      { name: "M6 bore @ z=45", per: sound() },
      { name: "M6 bore @ z=105", per: sound() },
    ],
    final: sound({ dims: [190, 40, 150], volume: 900000 }),
    features: [
      { face_id: 1, surface_kind: "plane", normal: [1, 0, 0], origin: [0, 0, 75] },
      { face_id: 2, surface_kind: "plane", normal: [0, 0, 1], origin: [95, 0, 150] },
      { face_id: 3, surface_kind: "cylinder", radius: 3, diameter: 6, axis: [1, 0, 0], origin: [4, 0, 45] },
      { face_id: 4, surface_kind: "cylinder", radius: 3, diameter: 6, axis: [1, 0, 0], origin: [4, 0, 105] },
    ],
    partCount: 1,
  };
}

const clone = (o) => structuredClone(o);

const LIES = [
  {
    name: "a mutating step slipped through UNSOUND (union left an open shell)",
    mutate: (d) => { d.steps[2].per = sound({ sound: false, watertight: false, open_edges: 12 }); },
  },
  {
    name: "final calls itself sound while reporting open edges (verdict not internally consistent)",
    mutate: (d) => { d.final = sound({ dims: [190, 40, 150], volume: 900000, open_edges: 6, watertight: false }); },
  },
  {
    name: "the bracket busts the 220×160×60 envelope (231 mm long)",
    mutate: (d) => { d.final.dims = [231, 40, 150]; },
  },
  {
    name: "only ONE M6 mounting bore is present",
    mutate: (d) => { d.features = d.features.filter((f) => f.origin?.[2] !== 105); },
  },
  {
    name: "a bore is the wrong size (Ø8, not M6 Ø6)",
    mutate: (d) => { const h = d.features.find((f) => f.origin?.[2] === 105); h.radius = 4; h.diameter = 8; },
  },
  {
    name: "the bores are at the wrong spacing (45 mm, not the frozen 60 mm)",
    mutate: (d) => { const h = d.features.find((f) => f.origin?.[2] === 105); h.origin = [4, 0, 90]; },
  },
  {
    name: "the model is TWO disconnected pieces, not one",
    mutate: (d) => { d.partCount = 2; },
  },
  {
    name: "a broken build reports zero volume (mass metric would be non-physical)",
    mutate: (d) => { d.final.volume = 0; },
  },
];

/** The spec criteria this kernel cannot score — must be DECLARED, not scored. */
const MUST_DECLARE = [/von mises|stress/i, /deflection/i, /orientation/i, /wall thickness/i, /overhang|support/i];

function main() {
  let failures = 0;

  const t = new Checks(scenario.id);
  oracle(t, honest());
  const failed = t.items.filter((i) => !i.passed);
  if (failed.length > 0) {
    failures += 1;
    console.log("FAIL  honest transcript did not pass cleanly:");
    for (const f of failed) console.log(`        [${f.dim}] ${f.name} — ${f.detail}`);
  } else {
    console.log(`ok    honest transcript passes all ${t.items.length} checks`);
  }

  for (const lie of LIES) {
    const d = clone(honest());
    lie.mutate(d);
    const tc = new Checks(scenario.id);
    oracle(tc, d);
    const caught = tc.items.filter((i) => !i.passed);
    if (caught.length === 0) {
      failures += 1;
      console.log(`FAIL  lie SURVIVED the oracle: ${lie.name}`);
    } else {
      console.log(`ok    caught: ${lie.name}  (${caught.length} check(s): ${caught.map((c) => `[${c.dim}] ${c.name}`).join("; ")})`);
    }
  }

  if (!scenario.dims.includes("honesty")) {
    failures += 1;
    console.log("FAIL  scenario 16 must declare the honesty dimension");
  } else {
    console.log("ok    scenario declares the honesty dimension");
  }

  // HONESTY BY OMISSION: the unscoreable spec criteria are declared, and no
  // scored check name mentions them (they are never fake-scored).
  const declared = Array.isArray(scenario.unscored_criteria) ? scenario.unscored_criteria : [];
  const declaredText = declared.map((u) => `${u.criterion} ${u.reason}`).join(" | ");
  const undeclared = MUST_DECLARE.filter((re) => !re.test(declaredText));
  if (declared.length === 0 || undeclared.length > 0) {
    failures += 1;
    console.log(`FAIL  unscored_criteria must DECLARE every unscoreable spec gate (missing: ${undeclared.map(String).join(", ")})`);
  } else {
    console.log(`ok    unscored_criteria declares all ${declared.length} unscoreable gates with reasons`);
  }
  const tProbe = new Checks(scenario.id);
  oracle(tProbe, honest());
  const scoredNames = tProbe.items.map((i) => i.name).join(" | ");
  const leaked = MUST_DECLARE.filter((re) => re.test(scoredNames));
  if (leaked.length > 0) {
    failures += 1;
    console.log(`FAIL  a scored check fake-scores an unscoreable gate: ${leaked.map(String).join(", ")}`);
  } else {
    console.log("ok    no scored check touches an unscoreable physics/printability gate");
  }

  console.log(
    failures === 0
      ? `\nORACLE VALIDATED — honest transcript passes, all ${LIES.length} lies caught, unscored set declared not scored.`
      : `\n${failures} ORACLE DEFECT(S).`,
  );
  process.exit(failures === 0 ? 0 : 1);
}

main();
