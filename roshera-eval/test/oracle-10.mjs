#!/usr/bin/env node
/**
 * DRY VALIDATION for scenario 10 (kinematic assembly).
 *
 * Runs NO backend. It feeds the scenario's pure oracle two transcripts —
 * one HONEST, one LYING — and proves the oracle tells them apart.
 *
 * Why this exists: a scenario is only evidence if it can be shown to catch
 * the thing it claims to catch. "All checks passed" against a live server
 * proves nothing on its own — an oracle of `t.ok("fine", true)` would pass
 * too. So each lie below is a SINGLE mutation of the honest transcript, and
 * the oracle must fail on each one and pass on the honest one.
 *
 * Usage: node test/oracle-10.mjs   (exit 0 = the oracle discriminates)
 */
import { Checks } from "../lib/harness.mjs";
import scenario, { oracle } from "../scenarios/10-assembly-kinematics.mjs";

const DEG = Math.PI / 180;

/** A transcript of a backend that told the truth about a real mechanism. */
function honest() {
  return {
    solve: { solved: true, converged: true, residual_norm: 1e-12 },
    certify: {
      sound: false, // the planted floating stop blocks soundness
      constrainedness: { status: "mobile", dof: 2 },
      certificate: {
        fully_grounded: false, // the stop floats — honestly reported
        mates_consistent: true,
        no_static_interference: true,
        swept_clearance_ok: false,
      },
    },
    conflict: {
      planted: ["m-1111", "m-2222"],
      witnesses: [
        { mates: ["m-1111", "m-2222"], kind: "numeric_conflict", minimal: true, oracle_calls: 6 },
      ],
    },
    drag_in: {
      dragged: true,
      converged: true,
      applied: 15 * DEG,
      limit: null,
      scope: { instances: ["i-arm"], mates: ["m-hinge"] },
      constrainedness: { status: "mobile", dof: 2 },
    },
    drag_out: {
      dragged: true,
      converged: true,
      applied: 30 * DEG,
      limit: { requested: 90 * DEG, min: -30 * DEG, max: 30 * DEG },
      scope: { instances: ["i-arm"], mates: ["m-hinge"] },
      constrainedness: { status: "mobile", dof: 2 },
    },
    interference: {
      computed: true,
      epsilon: { effective: 0.04, kernel_floor: 0.04, requested: null, raised_by_caller: false },
      static_pairs: [],
      sweeps: [
        {
          source: { source: "driven_mate", mate_index: 0, param: "rotation" },
          range: [-30 * DEG, 30 * DEG],
          method: { method: "nonlinear_toi", samples: 25 },
          clear: false,
          min_certified_clearance: -0.4,
          epsilon: 0.04,
          first_contact: { param: 0.21 },
          manifold_violation: null,
          interference: [
            { a: "i-arm", b: "i-stop", depth: 0.42, at: { param: 0.26 } },
          ],
          refusal: null,
        },
        {
          // The unbounded slider: REFUSED, and claiming nothing.
          source: { source: "driven_mate", mate_index: 1, param: "translation" },
          range: [0, 0],
          method: { method: "nonlinear_toi", samples: 0 },
          clear: true,
          min_certified_clearance: null,
          epsilon: 0.04,
          first_contact: null,
          manifold_violation: null,
          interference: [],
          refusal: { refusal: "unbounded_travel", mate_index: 1, param: "translation" },
        },
      ],
    },
  };
}

/** Deep clone so each mutation starts from a pristine honest transcript. */
const clone = (o) => structuredClone(o);

/**
 * Each lie is ONE mutation of the honest transcript — the shape of a real
 * regression or a real dishonesty, not a scrambled object.
 */
const LIES = [
  {
    name: "hides the floating part (claims fully grounded)",
    mutate: (d) => {
      d.certify.certificate.fully_grounded = true;
      d.certify.sound = true;
    },
  },
  {
    name: "shrugs at the conflict (no witness)",
    mutate: (d) => {
      d.conflict.witnesses = [];
    },
  },
  {
    name: "witnesses the WRONG mates",
    mutate: (d) => {
      d.conflict.witnesses[0].mates = ["m-1111", "m-9999"];
    },
  },
  {
    name: "treats the refused sweep as a certified pass",
    mutate: (d) => {
      const s = d.interference.sweeps[1];
      s.refusal = null;
      s.min_certified_clearance = 12.0; // an invented range's invented margin
    },
  },
  {
    name: "keeps the refusal but invents a clearance behind it",
    mutate: (d) => {
      d.interference.sweeps[1].min_certified_clearance = 12.0;
    },
  },
  {
    name: "reports interference with no angle (un-actionable)",
    mutate: (d) => {
      d.interference.sweeps[0].interference = [
        { a: "i-arm", b: "i-stop", depth: 0.42, at: {} },
      ];
    },
  },
  {
    name: "loses the interference entirely (silent-clear)",
    mutate: (d) => {
      d.interference.sweeps[0].interference = [];
    },
  },
  {
    name: "ignores the joint limit (drives past the stop)",
    mutate: (d) => {
      d.drag_out.applied = 90 * DEG;
      d.drag_out.limit = null;
    },
  },
  {
    name: "downgrades the swept gate to sampling (tunneling possible)",
    mutate: (d) => {
      d.interference.sweeps[0].method = { method: "sampled_dense", samples: 73 };
    },
  },
  {
    name: "authors the joints instead of deriving them from the mates",
    mutate: (d) => {
      d.interference.sweeps[0].source = { source: "mechanism", moving: 1 };
    },
  },
  {
    name: "runs the collision dimensions at ε = 0 (the old default lie)",
    mutate: (d) => {
      d.interference.epsilon.effective = 0.0;
    },
  },
  {
    name: "drags blind (no scope, no verdict)",
    mutate: (d) => {
      d.drag_in.scope = { instances: [], mates: [] };
      d.drag_in.constrainedness = null;
    },
  },
];

function main() {
  let failures = 0;

  // 1. The honest transcript must pass EVERY check.
  const t = new Checks(scenario.id);
  oracle(t, honest());
  const failed = t.items.filter((i) => !i.passed);
  if (failed.length > 0) {
    failures += 1;
    console.log(`FAIL  honest transcript did not pass cleanly:`);
    for (const f of failed) console.log(`        [${f.dim}] ${f.name} — ${f.detail}`);
  } else {
    console.log(`ok    honest transcript passes all ${t.items.length} checks`);
  }

  // 2. Every lie must be CAUGHT.
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
      console.log(
        `ok    caught: ${lie.name}  (${caught.length} check(s): ${caught
          .map((c) => `[${c.dim}] ${c.name}`)
          .join("; ")})`,
      );
    }
  }

  // 3. The scenario is wired into the corpus with the honesty dimension.
  if (!scenario.dims.includes("honesty")) {
    failures += 1;
    console.log("FAIL  scenario 10 must declare the honesty dimension");
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
