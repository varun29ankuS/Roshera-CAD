#!/usr/bin/env node
/**
 * DRY VALIDATION for scenario 13 (ε-coincident face boolean robustness).
 *
 * Runs NO backend. Feeds the pure oracle an honest transcript and
 * single-mutation lies, proving it discriminates.
 *
 * Usage: node test/oracle-13.mjs   (exit 0 = the oracle discriminates)
 */
import { Checks } from "../lib/harness.mjs";
import scenario, { oracle } from "../scenarios/13-coincident-face-robustness.mjs";

const EDGE = 10;
const EPS = 1e-6;
const UNION_VOL = EDGE * EDGE * (2 * EDGE - EPS);

/** A transcript of a kernel that fused the ε-coincident blocks soundly. */
function honest() {
  return {
    union: {
      solid_id: 3,
      sound: true,
      brep_valid: true,
      watertight: true,
      manifold: true,
      self_intersection_free: true,
      tessellation_clean: true,
      mesh_quality_clean: true,
      euler: 2,
      open_edges: 0,
      nonmanifold_edges: 0,
      model_debris_orphan_faces: 0,
      volume: UNION_VOL,
      face_count: 6,
    },
  };
}

const clone = (o) => structuredClone(o);

const LIES = [
  {
    name: "sound:true over a shell with open edges (not watertight)",
    mutate: (d) => {
      d.union.watertight = false;
      d.union.open_edges = 8;
    },
  },
  {
    name: "sound:true over a non-manifold shell",
    mutate: (d) => {
      d.union.manifold = false;
      d.union.nonmanifold_edges = 4;
    },
  },
  {
    name: "sound:true while orphan sliver faces linger",
    mutate: (d) => {
      d.union.model_debris_orphan_faces = 2;
    },
  },
  {
    name: "the two blocks never actually merged (chi wrong)",
    mutate: (d) => {
      d.union.euler = 4;
    },
  },
  {
    name: "the fused volume is wrong (lost material at the seam)",
    mutate: (d) => {
      d.union.volume = UNION_VOL * 0.95;
    },
  },
  {
    name: "the union regressed to honestly-unsound (capability tripwire)",
    mutate: (d) => {
      d.union.sound = false;
      d.union.watertight = false;
      d.union.open_edges = 12;
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
    console.log("FAIL  scenario 13 must declare the honesty dimension");
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
