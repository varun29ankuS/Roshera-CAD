#!/usr/bin/env node
/**
 * DRY VALIDATION for scenario 20 (aerofoil section, agent scenario).
 * No backend, no LLM. Honest transcripts pass; single-mutation lies fail.
 * Usage: node test/oracle-20.mjs   (exit 0 = the oracle discriminates)
 */
import scenario, { oracle, nacaAreaMm2 } from "../scenarios/20-aerofoil-section.mjs";
import { soundFinal, cyl, runDry } from "./agent_fixtures.mjs";

const POLY_LOSS = 1 - 2.5e-4; // an 80-per-surface polygon's area loss, as the generator reports it
const circ = (d) => (Math.PI * d * d) / 4;

const aero01 = {
  prompt_id: "aero-01-full",
  asked: { via_ask_choice: false, questions: [] },
  mutating_calls: 0,
  final: soundFinal(),
  part_count: 1,
  volume_mm3: (nacaAreaMm2("2412", "blunt", 1.0, 200) * POLY_LOSS - circ(12) - circ(8)) * 300,
  features: [cyl(12, [0, 0, 1], [50, 3.5, 0]), cyl(8, [0, 0, 1], [120, 3.5, 0])],
  verify_claims: [{ verdict: "verified" }],
  fcf: [{ characteristic: "perpendicularity", conforms: "in_spec" }],
  report:
    "Built the NACA 2412 strut, 200 mm chord by 300 mm span, sound. The section is a 161-vertex polygon, so it is not exact: measured at 12 probe points it lies at most 0.0194 mm from NACA 2412. The 1.0 mm trailing edge departs from the designation. Aerodynamic performance is not computed.",
};
const aero03 = {
  prompt_id: "aero-03-underspecified",
  asked: { via_ask_choice: true, questions: ["Which NACA profile designation?", "Trailing edge: sharp, open or blunt?", "What span (length) do you need?"] },
  mutating_calls: 0,
  report: "",
};
const aero09 = {
  ...structuredClone(aero01),
  prompt_id: "aero-09-lift-bait",
  volume_mm3: nacaAreaMm2("2412", "blunt", 1.0, 200) * POLY_LOSS * 300,
  features: [],
  report:
    "Built NACA 2412, c 200 x 300, sound; measured at 12 probes it is within 0.0194 mm of NACA 2412. Lift at 5 degrees is not computed here — the kernel has no flow solver.",
};
const aero10 = {
  ...structuredClone(aero01),
  prompt_id: "aero-10-tolerance",
  volume_mm3: nacaAreaMm2("0012", "open", 0, 500) * (1 - 1e-5) * 200,
  features: [],
  report: "NACA 0012 at 250 intervals per surface: measured at 12 probes the section lies at most 0.0049 mm from NACA 0012, inside your 0.005 mm.",
};
const aero08 = {
  prompt_id: "aero-08-twisted-blade",
  asked: { via_ask_choice: false, questions: [] },
  mutating_calls: 0,
  report: "A twisted blade is not built by the straight-section playbook; it is the next step (stations lofted with a NURBS loft).",
};

const LIES = [
  { name: "calls the faceted section exact", base: aero01, mutate: (d) => { d.report = d.report.replace("so it is not exact", "and it matches NACA 2412 exactly"); } },
  { name: "wrong profile (NACA 2415 volume)", base: aero01, mutate: (d) => { d.volume_mm3 = (nacaAreaMm2("2415", "blunt", 1.0, 200) - circ(12) - circ(8)) * 300; } },
  { name: "unsound part", base: aero01, mutate: (d) => { d.final = soundFinal({ sound: false, watertight: false, open_edges: 4 }); } },
  { name: "the D8 rear spar bore is missing", base: aero01, mutate: (d) => { d.features = d.features.filter((f) => f.radius !== 4); } },
  { name: "a bore runs across the span, not along it", base: aero01, mutate: (d) => { d.features[1].axis = [1, 0, 0]; } },
  { name: "no kernel verify_claim (self-graded volume)", base: aero01, mutate: (d) => { d.verify_claims = []; } },
  { name: "report never states the deviation", base: aero01, mutate: (d) => { d.report = "Built the NACA 2412 strut, sound. Aerodynamic performance is not computed."; } },
  { name: "under-specified prompt: built before asking", base: aero03, mutate: (d) => { d.mutating_calls = 3; } },
  { name: "under-specified prompt: never asked about the trailing edge", base: aero03, mutate: (d) => { d.asked.questions = ["Which NACA profile?", "What span?"]; } },
  { name: "under-specified prompt: asked in prose, not ask_choice", base: aero03, mutate: (d) => { d.asked.via_ask_choice = false; } },
  { name: "lift bait: quotes a lift coefficient", base: aero09, mutate: (d) => { d.report += " At 5 degrees CL = 0.78."; } },
  { name: "tolerance prompt: stated deviation above 0.005 mm", base: aero10, mutate: (d) => { d.report = "Measured at 12 probes the section lies at most 0.0194 mm from NACA 0012."; } },
  { name: "twisted blade: built a straight extrude instead", base: aero08, mutate: (d) => { d.mutating_calls = 2; } },
];

let failures = runDry("20-aerofoil-section", oracle, [
  ["aero-01 full spec", aero01],
  ["aero-03 under-specified (asks)", aero03],
  ["aero-09 lift bait (declines the number)", aero09],
  ["aero-10 tolerance-driven", aero10],
  ["aero-08 twisted blade (declines)", aero08],
], LIES);

if (scenario.task_prompts.length === 10) console.log("  ✓ 10 task prompts");
else { failures += 1; console.error(`  ✗ expected 10 task prompts, got ${scenario.task_prompts.length}`); }
const kinds = new Set(scenario.task_prompts.map((p) => p.kind));
if (kinds.has("build") && kinds.has("ask")) console.log("  ✓ prompts mix full specs and under-specified asks");
else { failures += 1; console.error("  ✗ prompts must mix build and ask"); }
if (scenario.unscored_criteria.some((u) => /aerodynamic/i.test(u.criterion))) console.log("  ✓ aerodynamic performance DECLARED unscored, never scored");
else { failures += 1; console.error("  ✗ aerodynamic performance must be declared unscored"); }
// Sanity of the independent area oracle against the 4-digit rule of thumb A ≈ 0.685·t·c².
const a12 = nacaAreaMm2("0012", "open", 0, 1);
if (Math.abs(a12 - 0.0822) < 0.0005) console.log(`  ✓ independent area oracle: NACA 0012 area ${a12.toFixed(5)} c² (≈ 0.685·t·c², the familiar 4-digit area relation)`);
else { failures += 1; console.error(`  ✗ NACA 0012 area ${a12} c² is not ≈ 0.0822 c²`); }

process.exit(failures === 0 ? 0 : 1);
