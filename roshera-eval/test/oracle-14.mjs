#!/usr/bin/env node
/**
 * DRY VALIDATION for scenario 14 (quadric SSI honesty).
 *
 * Runs NO backend. Feeds the pure oracle an honest transcript and
 * single-mutation lies, proving it discriminates. Note: the honest
 * transcript models the near-tangency case as an HONEST REFUSAL — the
 * expected behaviour until robust quadric-tangency lands — and the oracle
 * also accepts a genuinely-sound tangency solid; only a FABRICATED clean
 * solid is scored a lie.
 *
 * Usage: node test/oracle-14.mjs   (exit 0 = the oracle discriminates)
 */
import { Checks } from "../lib/harness.mjs";
import scenario, { oracle } from "../scenarios/14-quadric-ssi-honesty.mjs";

const CYL_VOL = Math.PI * 10 * 10 * 30;
const SPH_VOL = (4 / 3) * Math.PI * 6 * 6 * 6;
const BITE_LO = CYL_VOL - SPH_VOL;

/** A transcript: a sound cyl∘sphere bite + an honestly-refused tangency. */
function honest() {
  return {
    bite: {
      solid_id: 5,
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
      issues: [],
      // A ~hemisphere bite: removes roughly half the sphere.
      volume: CYL_VOL - SPH_VOL * 0.5,
    },
    tangency: {
      // The degenerate grazing bore is honestly refused (409 conflict).
      op: { ok: false, status: 409 },
      perception: null,
    },
  };
}

const clone = (o) => structuredClone(o);

const LIES = [
  {
    name: "the bite claims sound over a shell with open edges",
    mutate: (d) => {
      d.bite.watertight = false;
      d.bite.open_edges = 5;
    },
  },
  {
    name: "the bite removed more than a whole sphere (impossible volume)",
    mutate: (d) => {
      d.bite.volume = BITE_LO - 100;
    },
  },
  {
    name: "the bite added material (volume above the intact cylinder)",
    mutate: (d) => {
      d.bite.volume = CYL_VOL + 100;
    },
  },
  {
    name: "a sound bite that is secretly two pieces / a void (chi wrong)",
    mutate: (d) => {
      d.bite.euler = 4;
    },
  },
  {
    name: "the bite is unsound but names no defect (a shrug, not a refusal)",
    mutate: (d) => {
      d.bite.sound = false;
      d.bite.open_edges = 0;
      d.bite.watertight = true;
      d.bite.issues = [];
    },
  },
  {
    name: "the near-tangency case FABRICATES a clean solid (claims sound, hides open edges)",
    mutate: (d) => {
      d.tangency.op = { ok: true, status: 200 };
      d.tangency.perception = { sound: true, watertight: false, manifold: true, open_edges: 4, volume: 50000 };
    },
  },
  {
    name: "the near-tangency solid claims a physically impossible volume",
    mutate: (d) => {
      d.tangency.op = { ok: true, status: 200 };
      d.tangency.perception = {
        sound: true,
        watertight: true,
        manifold: true,
        self_intersection_free: true,
        open_edges: 0,
        nonmanifold_edges: 0,
        volume: 40 * 40 * 40 + 1, // more than the whole block
      };
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

  // The honest-refusal path must ALSO be accepted when the kernel instead
  // resolves the tangency soundly — prove the oracle does not punish a
  // genuinely-correct sound solid.
  {
    const d = clone(honest());
    d.tangency.op = { ok: true, status: 200 };
    d.tangency.perception = {
      sound: true,
      watertight: true,
      manifold: true,
      self_intersection_free: true,
      open_edges: 0,
      nonmanifold_edges: 0,
      volume: 40 * 40 * 40 - 12000,
    };
    const tc = new Checks(scenario.id);
    oracle(tc, d);
    const caught = tc.items.filter((i) => !i.passed);
    if (caught.length > 0) {
      failures += 1;
      console.log(`FAIL  a genuinely-sound tangency solid was wrongly rejected: ${caught.map((c) => c.name).join("; ")}`);
    } else {
      console.log("ok    a genuinely-sound, physically-bounded tangency solid is accepted (no false alarm)");
    }
  }

  if (!scenario.dims.includes("honesty")) {
    failures += 1;
    console.log("FAIL  scenario 14 must declare the honesty dimension");
  } else {
    console.log("ok    scenario declares the honesty dimension");
  }

  console.log(
    failures === 0
      ? `\nORACLE VALIDATED — honest transcript passes, all ${LIES.length} lies caught, no false alarm.`
      : `\n${failures} ORACLE DEFECT(S).`,
  );
  process.exit(failures === 0 ? 0 : 1);
}

main();
