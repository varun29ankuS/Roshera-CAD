#!/usr/bin/env node
/**
 * DRY VALIDATION for scenario 17 (NEMA 17 motor mount).
 *
 * Runs NO backend. Feeds the pure oracle an honest transcript and
 * single-mutation lies, proving it tells them apart. Also asserts the
 * HONESTY-BY-OMISSION contract: the spec's unscoreable criteria — headlined by
 * the f₁ ≥ 120 Hz modal requirement, plus stress, deflection, wall thickness,
 * overhang, orientation, thermal — are DECLARED in `unscored_criteria`, never
 * silently scored.
 *
 * Usage: node test/oracle-17.mjs   (exit 0 = the oracle discriminates)
 */
import { Checks } from "../lib/harness.mjs";
import scenario, { oracle } from "../scenarios/17-nema17-motor-mount.mjs";

const sound = (over = {}) => ({
  solid_id: 9, sound: true, watertight: true, manifold: true,
  self_intersection_free: true, tessellation_clean: true, mesh_quality_clean: true,
  open_edges: 0, euler: -8, ...over,
});

const SQ = 15.5; // half the 31 mm bolt square
const CZ = 31; // motor pattern center Z

/** An honestly-built reference mount: 4×M3 square + Ø22 boss + 2×M5 frame. */
function honest() {
  return {
    steps: [
      { name: "motor face plate", per: sound() },
      { name: "frame foot", per: sound() },
      { name: "union → L mount", per: sound() },
      { name: "Ø22 pilot boss", per: sound() },
      { name: "M3 bolt #0", per: sound() },
      { name: "M3 bolt #1", per: sound() },
      { name: "M3 bolt #2", per: sound() },
      { name: "M3 bolt #3", per: sound() },
      { name: "M5 frame bolt #0", per: sound() },
      { name: "M5 frame bolt #1", per: sound() },
    ],
    final: sound({ dims: [60, 50, 56], volume: 120000 }),
    features: [
      { face_id: 1, surface_kind: "plane", normal: [1, 0, 0], origin: [0, 0, 31] },
      // Ø22 pilot boss (axis X), central.
      { face_id: 2, surface_kind: "cylinder", radius: 11, diameter: 22, axis: [1, 0, 0], origin: [4, 0, CZ] },
      // 4×M3 on the 31 mm square (axis X).
      { face_id: 3, surface_kind: "cylinder", radius: 1.5, diameter: 3, axis: [1, 0, 0], origin: [4, +SQ, CZ - SQ] },
      { face_id: 4, surface_kind: "cylinder", radius: 1.5, diameter: 3, axis: [1, 0, 0], origin: [4, -SQ, CZ - SQ] },
      { face_id: 5, surface_kind: "cylinder", radius: 1.5, diameter: 3, axis: [1, 0, 0], origin: [4, +SQ, CZ + SQ] },
      { face_id: 6, surface_kind: "cylinder", radius: 1.5, diameter: 3, axis: [1, 0, 0], origin: [4, -SQ, CZ + SQ] },
      // 2×M5 frame bores (axis Z).
      { face_id: 7, surface_kind: "cylinder", radius: 2.5, diameter: 5, axis: [0, 0, 1], origin: [15, 0, 4] },
      { face_id: 8, surface_kind: "cylinder", radius: 2.5, diameter: 5, axis: [0, 0, 1], origin: [45, 0, 4] },
    ],
    partCount: 1,
  };
}

const clone = (o) => structuredClone(o);
const m3faces = (d) => d.features.filter((f) => f.radius === 1.5);

const LIES = [
  {
    name: "a mutating step slipped through UNSOUND (a bore torn the shell)",
    mutate: (d) => { d.steps[3].per = sound({ sound: false, watertight: false, open_edges: 9 }); },
  },
  {
    name: "final calls itself sound while reporting open edges",
    mutate: (d) => { d.final = sound({ dims: [60, 50, 56], volume: 120000, open_edges: 4, watertight: false }); },
  },
  {
    name: "the mount busts the 90×70×60 envelope (95 mm long — exceeds the 90 mm max bound)",
    mutate: (d) => { d.final.dims = [95, 50, 56]; },
  },
  {
    name: "only THREE M3 motor bolts present (dropped one)",
    mutate: (d) => { d.features = d.features.filter((f) => f.face_id !== 6); },
  },
  {
    name: "the M3 pattern is the wrong size (not a 31 mm square)",
    mutate: (d) => { for (const f of m3faces(d)) f.origin = [4, f.origin[1] * 0.6, f.origin[2]]; },
  },
  {
    name: "the pilot boss is missing (no Ø22 register)",
    mutate: (d) => { d.features = d.features.filter((f) => f.radius !== 11); },
  },
  {
    name: "the pilot boss is the wrong bore (Ø16, not Ø22)",
    mutate: (d) => { const b = d.features.find((f) => f.radius === 11); b.radius = 8; b.diameter = 16; },
  },
  {
    name: "only ONE M5 frame bore (mount cannot bolt to the extrusion)",
    mutate: (d) => { d.features = d.features.filter((f) => f.face_id !== 8); },
  },
  {
    name: "the model is TWO disconnected pieces, not one",
    mutate: (d) => { d.partCount = 2; },
  },
  {
    name: "a broken build reports zero volume (mass metric non-physical)",
    mutate: (d) => { d.final.volume = 0; },
  },
];

/** The spec criteria this kernel cannot score — must be DECLARED, not scored.
 *  f₁ (modal) is the headline requirement and the first thing that must appear. */
const MUST_DECLARE = [/natural frequency|f₁|modal|120 hz/i, /von mises|stress/i, /deflection/i, /wall/i, /overhang|support/i, /orientation/i, /thermal|60 ?°?c/i];

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
    console.log("FAIL  scenario 17 must declare the honesty dimension");
  } else {
    console.log("ok    scenario declares the honesty dimension");
  }

  const declared = Array.isArray(scenario.unscored_criteria) ? scenario.unscored_criteria : [];
  const declaredText = declared.map((u) => `${u.criterion} ${u.reason}`).join(" | ");
  const undeclared = MUST_DECLARE.filter((re) => !re.test(declaredText));
  if (declared.length === 0 || undeclared.length > 0) {
    failures += 1;
    console.log(`FAIL  unscored_criteria must DECLARE every unscoreable spec gate (missing: ${undeclared.map(String).join(", ")})`);
  } else {
    console.log(`ok    unscored_criteria declares all ${declared.length} unscoreable gates with reasons (incl. f₁ modal)`);
  }
  const tProbe = new Checks(scenario.id);
  oracle(tProbe, honest());
  const scoredNames = tProbe.items.map((i) => i.name).join(" | ");
  const leaked = MUST_DECLARE.filter((re) => re.test(scoredNames));
  if (leaked.length > 0) {
    failures += 1;
    console.log(`FAIL  a scored check fake-scores an unscoreable gate: ${leaked.map(String).join(", ")}`);
  } else {
    console.log("ok    no scored check touches an unscoreable modal/physics/printability gate");
  }

  console.log(
    failures === 0
      ? `\nORACLE VALIDATED — honest transcript passes, all ${LIES.length} lies caught, unscored set declared not scored.`
      : `\n${failures} ORACLE DEFECT(S).`,
  );
  process.exit(failures === 0 ? 0 : 1);
}

main();
