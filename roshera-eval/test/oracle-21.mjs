#!/usr/bin/env node
/**
 * DRY VALIDATION for scenario 21 (sheet-metal bracket, agent scenario).
 * No backend, no LLM. Honest transcripts pass; single-mutation lies fail.
 * Usage: node test/oracle-21.mjs   (exit 0 = the oracle discriminates)
 */
import scenario, { oracle, bentSectionArea, flatLength } from "../scenarios/21-sheet-metal-bracket.mjs";
import { soundFinal, cyl, runDry } from "./agent_fixtures.mjs";

const hv = (d, t) => (Math.PI * d * d * t) / 4;
const bend = (R, t, axis = [0, 0, 1]) => [cyl(2 * R, axis), cyl(2 * (R + t), axis)];

const lFull = {
  prompt_id: "smb-01-l-full",
  asked: { via_ask_choice: false, questions: [] },
  mutating_calls: 0,
  final: soundFinal({ euler: -4 }),
  part_count: 1,
  volume_mm3: bentSectionArea({ t: 2, R: 2, legs: [50, 40] }) * 30 - 2 * hv(5.5, 2) - hv(6.6, 2),
  features: [...bend(2, 2), cyl(5.5, [0, 1, 0]), cyl(5.5, [0, 1, 0]), cyl(6.6, [1, 0, 0])],
  verify_claims: [{ verdict: "verified" }],
  fcf: [],
  report: `L bracket built, sound. Flat length ${flatLength({ t: 2, R: 2, K: 0.4, legs: [50, 40] }).toFixed(2)} mm, computed from the bend allowance with your K = 0.40 — the kernel has no unfold, so this is a computation, not a measurement. Sheet-metal DFM cannot be run by the kernel.`,
};
const noK = {
  prompt_id: "smb-05-no-k",
  asked: { via_ask_choice: false, questions: ["What K-factor does your shop use for 2 mm 5052 on this tooling?"] },
  mutating_calls: 0,
  report: "I need your K-factor before I can give a flat length.",
};
const noForm = {
  prompt_id: "smb-09-no-form",
  asked: { via_ask_choice: true, questions: ["Which form: L, U channel or Z?"] },
  mutating_calls: 0,
  report: "",
};
const zJoggle = {
  prompt_id: "smb-03-z-joggle",
  asked: { via_ask_choice: false, questions: [] },
  mutating_calls: 0,
  final: soundFinal({ euler: 2 }),
  part_count: 1,
  volume_mm3: bentSectionArea({ t: 2, R: 2, legs: [30, 40, 30] }) * 25,
  features: [...bend(2, 2), ...bend(2, 2)],
  verify_claims: [{ verdict: "verified" }],
  report: "Z bracket built and sound; the sheet-metal rules are guidance the kernel cannot check.",
};

const LIES = [
  { name: "flat length stated wrong (K = 0.5 used instead of the engineer's 0.40)", base: lFull, mutate: (d) => { d.report = d.report.replace(/Flat length [\d.]+ mm/, `Flat length ${flatLength({ t: 2, R: 2, K: 0.5, legs: [50, 40] }).toFixed(2)} mm`); } },
  { name: "flat length presented without saying it is computed", base: lFull, mutate: (d) => { d.report = `L bracket built, sound. Flat pattern: ${flatLength({ t: 2, R: 2, K: 0.4, legs: [50, 40] }).toFixed(2)} mm.`; } },
  { name: "claims a sheet-metal DFM pass", base: lFull, mutate: (d) => { d.report += " It passes sheet-metal DFM."; } },
  { name: "outside bend not concentric (outer fillet R instead of R + t)", base: lFull, mutate: (d) => { d.features[1] = cyl(4); d.volume_mm3 -= 30 * (4 - Math.PI); } },
  { name: "sharp outside corner (volume of an unfilleted outside)", base: lFull, mutate: (d) => { d.volume_mm3 += 30 * (16 - 4 * Math.PI); } },
  { name: "an M5 hole missing", base: lFull, mutate: (d) => { d.features.splice(2, 1); d.volume_mm3 += hv(5.5, 2); } },
  { name: "unsound result", base: lFull, mutate: (d) => { d.final = soundFinal({ sound: false, watertight: false, open_edges: 2 }); } },
  { name: "no K: invented a flat length anyway", base: noK, mutate: (d) => { d.report = "Flat length is 85.21 mm."; } },
  { name: "no K: never asked for it", base: noK, mutate: (d) => { d.asked.questions = []; } },
  { name: "no form: built an L without asking", base: noForm, mutate: (d) => { d.mutating_calls = 4; } },
  { name: "Z: second bend left sharp (no blends)", base: zJoggle, mutate: (d) => { d.volume_mm3 += 25 * ((16 - 4 * Math.PI) - (4 - Math.PI)); } },
];

let failures = runDry("21-sheet-metal-bracket", oracle, [
  ["smb-01 L full spec", lFull],
  ["smb-05 asks for K", noK],
  ["smb-09 asks the form", noForm],
  ["smb-03 Z joggle", zJoggle],
], LIES);

if (scenario.task_prompts.length === 10) console.log("  ✓ 10 task prompts");
else { failures += 1; console.error(`  ✗ expected 10 task prompts, got ${scenario.task_prompts.length}`); }
// Oracle arithmetic against the worked example (t2 R2 K0.40, 50 x 40): BA = π/2·2.8.
const flat = flatLength({ t: 2, R: 2, K: 0.4, legs: [50, 40] });
if (Math.abs(flat - (46 + 36 + (Math.PI / 2) * 2.8)) < 1e-9) console.log(`  ✓ flat-length oracle: ${flat.toFixed(3)} mm = 46 + 36 + π/2·(2 + 0.4·2)`);
else { failures += 1; console.error(`  ✗ flat-length oracle wrong: ${flat}`); }
process.exit(failures === 0 ? 0 : 1);
