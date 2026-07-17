#!/usr/bin/env node
/**
 * DRY VALIDATION for scenario 11 (certified constrained sketch → bore).
 *
 * Runs NO backend. It feeds the scenario's pure oracle two transcripts —
 * one HONEST, one LYING per mutation — and proves the oracle tells them
 * apart. A scenario is only evidence if it can be shown to catch the thing
 * it claims to catch.
 *
 * Usage: node test/oracle-11.mjs   (exit 0 = the oracle discriminates)
 */
import { Checks } from "../lib/harness.mjs";
import scenario, { oracle } from "../scenarios/11-sketch-certified-bore.mjs";

const PLATE_W = 40;
const PLATE_H = 30;
const BORE_R = 6;
const DEPTH = 5;
const VOL = (PLATE_W * PLATE_H - Math.PI * BORE_R * BORE_R) * DEPTH;

/** A transcript of a backend that told the truth about a real sketch. */
function honest() {
  return {
    good: {
      solve: {
        status: { kind: "converged", iterations: 4, final_error: 1e-12 },
        violations: [],
        solve_time_ms: 2.1,
        entities_solved: 9,
        constraints_solved: 3,
        entities_skipped: [],
        certificate: {
          sound: true,
          constrainedness: "FullyConstrained",
          solver: { kind: "converged", final_error: 1e-12 },
          closed_profile: true,
          free_dofs: 0,
          redundant_constraints: 0,
          violated_constraints: 0,
          fully_constrained_entities: 5,
          under_constrained_entities: 0,
          over_constrained_entities: 0,
          witnesses: [],
        },
      },
      certify: {
        constrainedness: "FullyConstrained",
        constraint_consistent: true,
        redundant_constraints: 0,
        closed_profile: true,
        profile: "NestedRegions",
        self_intersection_free: true,
        entities_valid: true,
        issues: [],
        solver: { kind: "converged", final_error: 1e-12 },
        dof: { total_free_dofs: 0, constraint_dofs_removed: 3, status: "FullyConstrained" },
        decomposition: { components: 2, planned_components: 2, dense_components: 0, clusters: 2 },
        constraint_facts: [],
        entity_statuses: [],
        witnesses: [],
        continuity: [],
      },
      dof: { total_free_dofs: 0, status: "FullyConstrained" },
      extrude: {
        success: true,
        solid_id: 7,
        certificate: { sound: true, constrainedness: "FullyConstrained" },
        stats: { regions: 1, analytic_loops: 2, sampled_loops: 0 },
      },
      solid: {
        solid_id: 7,
        sound: true,
        brep_valid: true,
        watertight: true,
        manifold: true,
        self_intersection_free: true,
        tessellation_clean: true,
        mesh_quality_clean: true,
        euler: 0,
        open_edges: 0,
        volume: VOL,
      },
    },
    over: {
      certify: {
        constrainedness: { Conflicting: { conflicts: 1 } },
        constraint_consistent: false,
        redundant_constraints: 0,
        closed_profile: false,
        profile: "Open",
        self_intersection_free: true,
        entities_valid: true,
        issues: ["constraints inconsistent: 2-member conflict set"],
        solver: { kind: "conflicting", conflicts: 1, final_error: 5.0 },
        dof: { total_free_dofs: 0, constraint_dofs_removed: 3, status: "OverConstrained" },
        decomposition: { components: 1, planned_components: 0, dense_components: 1, clusters: 0 },
        constraint_facts: [],
        entity_statuses: [],
        witnesses: [
          {
            kind: "numeric_conflict",
            constraints: [
              { id: "c-dist", constraint_type: { Dimensional: { Distance: 10 } }, residual: 0.0 },
              { id: "c-len", constraint_type: { Dimensional: { Length: 20 } }, residual: 10.0 },
            ],
            minimal: true,
            oracle_calls: 5,
          },
        ],
        continuity: [],
      },
      planted: ["c-dist", "c-len"],
    },
  };
}

const clone = (o) => structuredClone(o);

/** Each lie is ONE mutation of the honest transcript. */
const LIES = [
  {
    name: "silently 'solves' the over-constrained sketch (invents a verdict)",
    mutate: (d) => {
      d.over.certify.constrainedness = "FullyConstrained";
      d.over.certify.constraint_consistent = true;
      d.over.certify.solver = { kind: "converged", final_error: 1e-12 };
      d.over.certify.witnesses = [];
    },
  },
  {
    name: "keeps the conflict verdict but shrugs (no witness)",
    mutate: (d) => {
      d.over.certify.witnesses = [];
    },
  },
  {
    name: "witnesses the WRONG constraints",
    mutate: (d) => {
      d.over.certify.witnesses[0].constraints = [
        { id: "c-dist", constraint_type: {}, residual: 0 },
        { id: "c-9999", constraint_type: {}, residual: 3 },
      ];
    },
  },
  {
    name: "emits an empty witness set (minimal flag over nothing)",
    mutate: (d) => {
      d.over.certify.witnesses[0].constraints = [];
    },
  },
  {
    name: "reports conflict but with a 'converged' solver verdict",
    mutate: (d) => {
      d.over.certify.solver = { kind: "converged", final_error: 1e-12 };
    },
  },
  {
    name: "claims fully constrained while free DOF remain",
    mutate: (d) => {
      d.good.solve.certificate.free_dofs = 3;
    },
  },
  {
    name: "passes a chord-sampled bore off as a true cylinder",
    mutate: (d) => {
      d.good.extrude.stats.analytic_loops = 1;
      d.good.extrude.stats.sampled_loops = 1;
    },
  },
  {
    name: "the extruded bore is not actually a through-hole (chi wrong)",
    mutate: (d) => {
      d.good.solid.euler = 2;
    },
  },
  {
    name: "reports the extruded solid sound while it has open edges",
    mutate: (d) => {
      d.good.solid.watertight = false;
      d.good.solid.open_edges = 6;
      // sound must not survive a broken watertight cert
      d.good.solid.sound = true;
    },
    // Note: t.sound requires sound===true AND no failed cert dims. Setting
    // watertight:false with sound:true is exactly the lie — a sound verdict
    // over a non-watertight shell.
  },
  {
    name: "inflates the extruded volume past its analytic oracle",
    mutate: (d) => {
      d.good.solid.volume = VOL * 1.5;
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
      console.log(
        `ok    caught: ${lie.name}  (${caught.length} check(s): ${caught
          .map((c) => `[${c.dim}] ${c.name}`)
          .join("; ")})`,
      );
    }
  }

  if (!scenario.dims.includes("honesty")) {
    failures += 1;
    console.log("FAIL  scenario 11 must declare the honesty dimension");
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
