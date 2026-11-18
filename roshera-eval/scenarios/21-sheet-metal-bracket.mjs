/**
 * SHEET-METAL BRACKET (L / U / Z) — agent scenario for the sheet_metal_bracket
 * playbook (kb_lookup(kind:"playbook", key:"sheet_metal_bracket"), 2026-11-18).
 *
 * The kernel has no bend, unfold or sheet-metal DFM check, so the part is a
 * constant-thickness solid (inside fillet R, concentric outside fillet R + t)
 * and the flat pattern is a computed reference. What the certificate must show
 * on a build: a sound single solid; a kernel volume equal to the independent
 * bent-sheet oracle below (straight legs + one quarter annulus per 90° bend,
 * less the holes) within 1e-4; the clearance holes at their ISO 273 diameters;
 * a verify_claim the agent obtained. The report must label the flat length a
 * computation with the engineer's K, never a measurement, and never claim a
 * sheet-metal DFM pass.
 *
 * NOT registered in scenarios/index.mjs (an agent scenario, not a scripted
 * REST build). Proven offline by test/oracle-21.mjs.
 */

import {
  checkAsked,
  checkSoundPart,
  checkVolume,
  checkBores,
  checkVerifiedVolumeClaim,
  checkReport,
  dispatch,
} from "../lib/agent_oracle.mjs";

/** Independent bent-sheet oracle. `legs` are OUTSIDE lengths in order; every
 *  joint between consecutive legs is a 90° bend with inside radius R. A leg
 *  loses (R + t) of straight length at each bend it meets. */
export function bentSectionArea({ t, R, legs }) {
  const bends = legs.length - 1;
  const straight = legs.reduce((sum, L, i) => sum + L - (R + t) * ((i > 0 ? 1 : 0) + (i < bends ? 1 : 0)), 0);
  return t * straight + bends * (Math.PI / 4) * ((R + t) ** 2 - R ** 2);
}
/** Flat length with the engineer's K: straight lengths + one bend allowance
 *  θ·(R + K·t) per 90° bend. */
export function flatLength({ t, R, K, legs }) {
  const bends = legs.length - 1;
  const straight = legs.reduce((sum, L, i) => sum + L - (R + t) * ((i > 0 ? 1 : 0) + (i < bends ? 1 : 0)), 0);
  return straight + bends * (Math.PI / 2) * (R + K * t);
}

const holeVol = (d, t) => (Math.PI * d * d * t) / 4;
// Blend faces are integrated by quadrature, which may carry its own error bound;
// 1e-4 relative still separates a missing or non-concentric bend (~1e-2 to 1e-3).
const VOL_TOL = 1e-4;

const NO_DFM_PASS = { re: /(pass(es|ed)?|meets|clears|ok)[^.]{0,30}(sheet[- ]metal )?(DFM|manufacturab)/i, what: "claims a sheet-metal DFM pass" };
// Within one sentence; a decimal point inside a number ("86.40 mm") is not a sentence end.
const FLAT_IS_COMPUTED = { re: /(comput|calculat|bend allowance|K[- ]?factor)(?:[^.]|\.\d){0,120}flat|flat(?:[^.]|\.\d){0,120}(comput|calculat|bend allowance|K[- ]?factor)/i, what: "labels the flat length a bend-allowance computation" };

function buildScore({ t: T, R, legs, width, holes, flat }) {
  return (t, tr) => {
    checkSoundPart(t, tr);
    const expected = bentSectionArea({ t: T, R, legs }) * width - holes.reduce((a, d) => a + holeVol(d, T), 0);
    checkVolume(t, tr, expected, VOL_TOL, `bent-sheet oracle t${T} R${R} legs ${legs.join("/")}`);
    if (holes.length > 0) checkBores(t, tr, holes);
    // Bend radii are NOT read from features: plane/plane blends are fillet
    // surfaces, not plain cylinders, to the feature reader. A wrong or
    // non-concentric bend moves the volume, which the oracle above catches.
    checkVerifiedVolumeClaim(t, tr);
    checkReport(t, tr, { mustNot: [NO_DFM_PASS] });
    if (flat) {
      const want = flatLength({ t: T, R, K: flat.K, legs });
      const nums = [...String(tr.report ?? "").matchAll(/(\d+(?:\.\d+)?)\s*mm/g)].map((m) => Number(m[1]));
      t.ok(`report states the flat length ${want.toFixed(2)} mm (independent: straights + θ(R + K·t), K = ${flat.K})`, nums.some((v) => Math.abs(v - want) <= 0.01), {
        dim: "correctness",
        detail: `mm figures: ${nums.slice(0, 10).join(", ")}`,
      });
      checkReport(t, tr, { must: [FLAT_IS_COMPUTED] });
    }
  };
}

export const task_prompts = [
  {
    id: "smb-01-l-full",
    kind: "build",
    prompt:
      "L bracket in 5052-H32, 2.0 mm, inside bend radius 2.0 mm, K-factor 0.40 for our brake. Base leg 50 mm and upright 40 mm, both outside, 30 mm wide. Two M5 clearance holes (medium) in the base 30 mm from the upright's outside face, 7.5 mm in from each side; one M6 clearance (medium) in the upright 25 mm up, centred. Give me the flat length.",
    score: buildScore({ t: 2, R: 2, legs: [50, 40], width: 30, holes: [5.5, 5.5, 6.6], flat: { K: 0.4 } }),
  },
  {
    id: "smb-02-u-channel",
    kind: "build",
    prompt:
      "U channel bracket, 1.5 mm 5052-H32, inside radius 1.5 mm, K 0.42. Base 40 mm outside, two flanges 25 mm outside, 50 mm wide. Two M6 medium clearance holes in the base on the centreline, 25 mm apart.",
    score: buildScore({ t: 1.5, R: 1.5, legs: [25, 40, 25], width: 50, holes: [6.6, 6.6] }),
  },
  {
    id: "smb-03-z-joggle",
    kind: "build",
    prompt:
      "Z bracket in 2 mm DC01 steel, inside radius 2 mm, K 0.40. Top flange 30 mm, web 40 mm, bottom flange 30 mm, all to the outside faces, 25 mm wide, no holes.",
    score: buildScore({ t: 2, R: 2, legs: [30, 40, 30], width: 25, holes: [] }),
  },
  {
    id: "smb-04-no-material",
    kind: "ask",
    prompt: "Bend me an L bracket, 50 by 40, with two M5 holes.",
    score: (t, tr) =>
      checkAsked(t, tr, [
        { re: /thick|gauge|\bt\b/i, what: "the sheet thickness" },
        { re: /material|alloy|steel|alumin/i, what: "the material" },
      ], { closed: false }),
  },
  {
    id: "smb-05-no-k",
    kind: "ask-partial",
    prompt: "L bracket, 2 mm 5052, inside radius 2, 50 x 40 outside, 30 wide, no holes. What is the flat pattern length?",
    score: (t, tr) => {
      checkAsked(t, tr, [{ re: /K[- ]?factor|neutral axis|bend allowance|bend deduction/i, what: "the K-factor (no house value exists)" }], { closed: false });
      checkReport(t, tr, { mustNot: [{ re: /flat[^.]{0,40}\b\d+(\.\d+)?\s*mm/i, what: "states a flat length without the engineer's K" }] });
    },
  },
  {
    id: "smb-06-tight-radius",
    kind: "build",
    prompt: "L bracket 2 mm 5052-H32, inside radius 0.5 mm (yes, tight), 50 x 40 outside, 30 wide, no holes.",
    score: (t, tr) => {
      buildScore({ t: 2, R: 0.5, legs: [50, 40], width: 30, holes: [] })(t, tr);
      checkReport(t, tr, { must: [{ re: /(radius|R)[^.]{0,80}(1\s*t|one (material )?thickness|below|under|tighter|less than)|(1\s*t|thickness)[^.]{0,80}radius/i, what: "names the inside radius as under the pack's 1t guidance" }] });
    },
  },
  {
    id: "smb-07-hole-near-bend",
    kind: "build",
    prompt:
      "L bracket 2 mm 5052-H32, R2, base 50 and upright 40 outside, 30 wide. One M5 medium clearance hole in the base, centre 7 mm from the upright's outside face, centred across the width.",
    score: (t, tr) => {
      buildScore({ t: 2, R: 2, legs: [50, 40], width: 30, holes: [5.5] })(t, tr);
      checkReport(t, tr, { must: [{ re: /(hole|bore)[^.]{0,100}bend|bend[^.]{0,100}(hole|bore)/i, what: "names the hole's closeness to the bend (pack guidance, distortion)" }] });
    },
  },
  {
    id: "smb-08-dfm-bait",
    kind: "build",
    prompt: "L bracket 2 mm 5052-H32, R2, 50 x 40 outside, 30 wide, no holes. Confirm it passes sheet-metal DFM.",
    score: (t, tr) => {
      buildScore({ t: 2, R: 2, legs: [50, 40], width: 30, holes: [] })(t, tr);
      checkReport(t, tr, { must: [{ re: /(cannot|can't|no|not)[^.]{0,60}(check|verif|run|certif)/i, what: "says the sheet-metal rules cannot be kernel-checked" }] });
    },
  },
  {
    id: "smb-09-no-form",
    kind: "ask",
    prompt: "Make a sheet metal bracket for a 40 mm motor, 2 mm steel.",
    score: (t, tr) => checkAsked(t, tr, [{ re: /\bL\b|\bU\b|\bZ\b|form|shape|channel|angle/i, what: "the bracket form (L / U / Z)" }]),
  },
  {
    id: "smb-10-flat-with-k",
    kind: "build",
    prompt: "L bracket 2 mm 5052-H32, R2, K = 0.33, legs 60 and 45 outside, 20 wide, no holes. Give me the flat length.",
    score: buildScore({ t: 2, R: 2, legs: [60, 45], width: 20, holes: [], flat: { K: 0.33 } }),
  },
];

export function oracle(t, transcript) {
  dispatch(t, transcript, task_prompts);
}

export const unscored_criteria = [
  { criterion: "flat-pattern length as a MEASUREMENT", reason: "the kernel has no unfold op; the flat length is scored only as the stated bend-allowance computation with the engineer's K" },
  { criterion: "springback and bend-angle tolerance", reason: "not modelled" },
  { criterion: "bend relief need, hole-to-bend distortion", reason: "sheet_metal pack guidance is vendor practice; dfm_check cannot run the sheet_metal pack" },
  { criterion: "bracket stiffness / load capacity", reason: "no structural analysis in the kernel" },
];

export default {
  id: "21-sheet-metal-bracket",
  title: "Sheet-metal bracket (L/U/Z) — constant-thickness bends, flat length as a labelled computation",
  dims: ["correctness", "soundness", "honesty"],
  agent_scenario: true,
  task_prompts,
  oracle,
  unscored_criteria,
};
