#!/usr/bin/env node
/**
 * DRY VALIDATION for scenario 12 (mass-properties honesty).
 *
 * Runs NO backend. Feeds the pure oracle an honest transcript and
 * single-mutation lies, proving it tells them apart. A scenario is only
 * evidence if it can be shown to catch the thing it claims to catch.
 *
 * Usage: node test/oracle-12.mjs   (exit 0 = the oracle discriminates)
 */
import { Checks } from "../lib/harness.mjs";
import scenario, { oracle } from "../scenarios/12-mass-properties-honesty.mjs";

const BOX_VOL = 20 * 10 * 30; // 6000
const CYL_VOL = Math.PI * 5 * 5 * 20; // 500π

/** A transcript of a metrology kernel that stated its accuracy honestly. */
function honest() {
  return {
    box: {
      solid_id: 1,
      volume: BOX_VOL,
      mass: BOX_VOL * 7.85e-6,
      method: "Analytical",
      provenance: {
        volume: { exactness: "exact" },
        center_of_mass: { exactness: "exact" },
        inertia: { exactness: "exact" },
      },
    },
    cyl: {
      solid_id: 2,
      // The mesh estimate sits a fraction under πr²h (chord under-fills
      // the round face), comfortably inside the stated 1% bound.
      volume: CYL_VOL * (1 - 0.004),
      mass: CYL_VOL * (1 - 0.004) * 7.85e-6,
      method: "Tessellated",
      provenance: {
        volume: { exactness: "approximate", method: "mesh_tonon", rel_error_bound: 0.01 },
        center_of_mass: { exactness: "approximate", method: "mesh_tonon", rel_error_bound: 0.01 },
        inertia: { exactness: "approximate", method: "mesh_tonon", rel_error_bound: 0.02 },
      },
    },
  };
}

const clone = (o) => structuredClone(o);

const LIES = [
  {
    name: "dresses the curved solid's inertia as Exact (invents precision)",
    mutate: (d) => {
      d.cyl.provenance.inertia = { exactness: "exact" };
    },
  },
  {
    name: "stamps Exact on a loose box volume (lies about the label)",
    mutate: (d) => {
      d.box.volume = BOX_VOL * 1.05; // 5% off, still claiming exact
    },
  },
  {
    name: "states a volume bound the value sits outside of",
    mutate: (d) => {
      d.cyl.volume = CYL_VOL * 1.2; // 20% off, bound still 1%
    },
  },
  {
    name: "labels Approximate but with a zero (useless) error bound",
    mutate: (d) => {
      d.cyl.provenance.inertia = { exactness: "approximate", method: "mesh_tonon", rel_error_bound: 0.0 };
    },
  },
  {
    name: "labels Approximate but names no method",
    mutate: (d) => {
      d.cyl.provenance.volume = { exactness: "approximate", rel_error_bound: 0.01 };
    },
  },
  {
    name: "downgrades a real exact box quantity to bare/unlabelled",
    mutate: (d) => {
      d.box.provenance.inertia = { exactness: "unavailable" };
    },
  },
  {
    name: "a wrong cylinder volume behind an honestly-wide bound (caught by the independent oracle)",
    mutate: (d) => {
      d.cyl.volume = CYL_VOL * 1.4;
      d.cyl.provenance.volume.rel_error_bound = 1.0; // "honestly" up to 100% wrong
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
    console.log("FAIL  scenario 12 must declare the honesty dimension");
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
