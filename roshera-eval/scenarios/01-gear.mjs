/**
 * Spur gear m=2 z=16 with a DIN-6885 keyed bore.
 *
 * Oracles:
 *  - soundness certificate (closed, manifold, watertight, ...)
 *  - genus-1 topology: euler characteristic chi = 0 (the through-bore)
 *  - structural: face_count = 256 outer segments + 35 bore segments + 2 caps = 293
 *  - INDEPENDENT volume cross-check: the kernel's tessellated volume vs the
 *    shoelace area of the exact profiles this scenario builds (x extrude height)
 *  - recipe fidelity: volume within +/-0.5% of the proven 5555.8 mm^3, steel
 *    mass within +/-1% of 0.0436 kg
 */
import { involuteGearProfile, keyedBoreProfile, shoelaceArea, prismVolume, steelMassKg } from "../lib/geom.mjs";
import { extrudeProfiles } from "../lib/builders.mjs";

export default {
  id: "01-gear",
  title: "Spur gear m=2 z=16 + keyed bore",
  dims: ["correctness", "soundness", "performance"],
  budgetMs: 60000,
  async run(ctx, t) {
    const { c } = ctx;
    const gear = involuteGearProfile({ m: 2, z: 16, alphaDeg: 20, outerPoints: 256 });
    const bore = keyedBoreProfile({ rBore: 5, keyHalfW: 1.5, notchTop: 6.4, points: 35 });

    t.eq("outer profile is 256 points", gear.pts.length, 256);
    t.eq("keyed bore is 35 points", bore.pts.length, 35);

    // Independent analytic oracle from the exact profiles.
    const outerA = Math.abs(shoelaceArea(gear.pts));
    const boreA = Math.abs(shoelaceArea(bore.pts));
    const oracleVol = prismVolume(outerA - boreA, 8);
    const oracleMass = steelMassKg(oracleVol);

    const id = await ctx.time("build gear (sketch + extrude)", () =>
      extrudeProfiles(c, [gear.pts, bore.pts], 8, "spur_gear"),
    );
    const per = await ctx.time("certify", () => c.perceive(id));
    const mass = await c.mass(id);

    t.sound("gear certifies sound", per);
    t.eq("genus-1 topology (chi = 0)", per.euler, 0, { dim: "correctness" });
    t.eq("face count = 256 + 35 + 2 = 293", per.face_count, 293, { dim: "correctness" });
    // Kernel volume must agree with the shoelace oracle (cross-validation).
    t.approxRel("kernel volume == shoelace oracle", per.volume, oracleVol, 0.005);
    // Recipe fidelity vs the proven dogfood targets.
    t.approxRel("volume ~ 5555.8 mm^3 (proven recipe)", per.volume, 5555.8, 0.005);
    t.approxRel("steel mass ~ 0.0436 kg", mass.mass, 0.0436, 0.01);
    t.approxRel("kernel mass == oracle mass", mass.mass, oracleMass, 0.01);
  },
};
