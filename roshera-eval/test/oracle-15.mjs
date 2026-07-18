#!/usr/bin/env node
/**
 * DRY VALIDATION for scenario 15 (drawing comprehension).
 *
 * Runs NO backend. Feeds the pure oracle an honest transcript and
 * single-mutation lies, proving it discriminates the campaign's failure modes:
 * a fabricated tolerance envelope, a datum falsely claimed live, a bore the
 * section never cut, hatch ink answered as geometry, an unprovenanced question
 * answered, and a re-drill that went undetected.
 *
 * Usage: node test/oracle-15.mjs   (exit 0 = the oracle discriminates)
 */
import { Checks } from "../lib/harness.mjs";
import scenario, { oracle } from "../scenarios/15-drawing-comprehension.mjs";

/** A fully honest transcript: every founder answer certified + honest. */
function honest() {
  return {
    fresh: {
      sound: true,
      counts: { consistent: 9, stale: 0, dangling: 0, render_only: 2, unprovenanced: 0 },
      section_cuts: {
        cuts: [
          { kind: "wall", face_ids: [3], hole_tag: null, span: [-30, -30] },
          { kind: "bore", face_ids: [12], hole_tag: "A1", span: [-6, 6] },
          { kind: "wall", face_ids: [5], hole_tag: null, span: [30, 30] },
        ],
        view_dir: [-1, 0, 0],
      },
    },
    toleranced: {
      answer: "toleranced_diameter",
      label: "A1 Ø12.00",
      value: 12,
      unit: "mm",
      tolerance_source: "feature",
      limits: [11.95, 12.05],
      designation: null,
      general_pm_mm: null,
      general_standard: null,
      feature_pid: "0000000000000000000000000000abcd",
      face_ids: [12],
      verdict: "consistent",
      measured: 12,
    },
    fcf: {
      answer: "fcf",
      index: 0,
      characteristic_glyph: "⊥",
      tolerance_text: "0.05",
      feature_pid: "0000000000000000000000000000abcd",
      datums: [{ label: "A", feature_pid: "00000000000000000000000000001234", status: "live" }],
      verdict: "consistent",
    },
    section: {
      answer: "section_cuts",
      cuts: [
        { kind: "wall", hole_tag: null, span: [-30, -30] },
        { kind: "bore", hole_tag: "A1", span: [-6, 6] },
        { kind: "wall", hole_tag: null, span: [30, 30] },
      ],
      view_dir: [-1, 0, 0],
    },
    entityHatch: { answer: "refused", reason: "section hatch is material texture (ink), not geometry", refusal: "render_only" },
    legacy: { answer: "refused", reason: "no hole row tagged ZZ9", refusal: "unprovenanced" },
    afterRedrill: { sound: false },
  };
}

const clone = (o) => structuredClone(o);

const LIES = [
  {
    name: "the toleranced diameter FABRICATES limits under a 'general' source",
    mutate: (d) => {
      d.toleranced.tolerance_source = "general";
      d.toleranced.general_pm_mm = 0.1;
      d.toleranced.limits = [11.9, 12.1]; // fabricated envelope
    },
  },
  {
    name: "a 'feature' tolerance carries neither limits nor a fit designation (empty claim)",
    mutate: (d) => {
      d.toleranced.limits = null;
      d.toleranced.designation = null;
    },
  },
  {
    name: "the feature limits do not bracket the nominal (physically wrong)",
    mutate: (d) => {
      d.toleranced.limits = [13.0, 13.1];
    },
  },
  {
    name: "the diameter answer carries no live-check verdict",
    mutate: (d) => {
      d.toleranced.verdict = "totally_fine";
    },
  },
  {
    name: "the FCF claims datum A is LIVE but names no feature PID",
    mutate: (d) => {
      d.fcf.datums = [{ label: "A", feature_pid: null, status: "live" }];
    },
  },
  {
    name: "the FCF references no datum A at all",
    mutate: (d) => {
      d.fcf.datums = [{ label: "Q", feature_pid: "x", status: "live" }];
    },
  },
  {
    name: "the section names a bore tag absent from the certified cut-through",
    mutate: (d) => {
      d.section.cuts.push({ kind: "bore", hole_tag: "Z9", span: [1, 2] });
    },
  },
  {
    name: "the section claims to cut no bore at all",
    mutate: (d) => {
      d.section.cuts = d.section.cuts.filter((c) => c.kind !== "bore");
    },
  },
  {
    name: "entity_at answers the hatch as geometry instead of refusing render_only",
    mutate: (d) => {
      d.entityHatch = { answer: "entity_at", role: "dimension", label: "40.00", face_ids: [3], pid: null };
    },
  },
  {
    name: "the unprovenanced question is ANSWERED instead of refused",
    mutate: (d) => {
      d.legacy = { answer: "hole", fact: { label: "ZZ9 Ø0" }, tolerance: null };
    },
  },
  {
    name: "the re-drill goes UNDETECTED (certificate still claims sound)",
    mutate: (d) => {
      d.afterRedrill.sound = true;
    },
  },
  {
    name: "the fresh sheet claims sound while carrying a stale fact (internal lie)",
    mutate: (d) => {
      d.fresh.counts.stale = 1;
    },
  },
];

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
      console.log(`ok    caught: ${lie.name}  (${caught.map((c) => `[${c.dim}] ${c.name}`).join("; ")})`);
    }
  }

  if (!scenario.dims.includes("honesty")) {
    failures += 1;
    console.log("FAIL  scenario 15 must declare the honesty dimension");
  } else {
    console.log("ok    scenario declares the honesty dimension");
  }

  console.log(
    failures === 0
      ? `\nORACLE VALIDATED — honest transcript passes, all ${LIES.length} lies caught.`
      : `\n${failures} ORACLE DEFECT(S).`,
  );
  process.exit(failures === 0 ? 0 : 1);
}

main();
