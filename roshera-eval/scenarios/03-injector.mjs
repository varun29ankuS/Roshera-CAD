/**
 * Injector faceplate: an Ø80 disk drilled with three concentric orifice rings
 * (8 / 12 / 16 holes, Ø3 each) = 36 sequential boolean differences.
 *
 * Oracles:
 *  - soundness after all 36 cuts
 *  - genus-36 topology: chi = 2 - 2*36 = -70 (each through-hole is one handle)
 *  - volume vs the exact analytic oracle: pi*40^2*8 - 36*pi*1.5^2*8
 */
import { drillCylinders } from "../lib/builders.mjs";

export default {
  id: "03-injector",
  title: "Injector plate (disk + 3 orifice rings, 36 bores)",
  dims: ["correctness", "soundness", "performance"],
  // 36 sequential certified booleans (bore + difference + re-cert per hole);
  // ~4 s/hole on a warm-but-busy backend, so the budget carries real headroom.
  budgetMs: 240000,
  async run(ctx, t) {
    const { c } = ctx;
    const cyl = await ctx.time("disk r40 h8", () =>
      c.post("/api/geometry/cylinder", { center: [0, 0, 0], axis: [0, 0, 1], radius: 40, height: 8, name: "faceplate" }),
    );
    let uuid = cyl.object.id;

    const rings = [
      { count: 8, ring_r: 15 },
      { count: 12, ring_r: 25 },
      { count: 16, ring_r: 35 },
    ];
    for (const { count, ring_r } of rings) {
      const holes = [];
      for (let k = 0; k < count; k++) {
        const th = (2 * Math.PI * k) / count;
        holes.push({ center: [ring_r * Math.cos(th), ring_r * Math.sin(th), -1], axis: [0, 0, 1], radius: 1.5, height: 10 });
      }
      ({ uuid } = await ctx.time(`drill ${count}-hole ring @r${ring_r}`, () => drillCylinders(c, uuid, holes)));
    }

    const id = await c.newestPartId();
    const per = await ctx.time("certify", () => c.perceive(id));

    // Exact analytic volume oracle.
    const oracleVol = Math.PI * 40 * 40 * 8 - 36 * Math.PI * 1.5 * 1.5 * 8;

    t.sound("injector certifies sound after 36 bores", per);
    t.eq("genus-36 topology (chi = -70)", per.euler, -70, { dim: "correctness" });
    t.approxRel("volume == analytic oracle", per.volume, oracleVol, 0.005);
  },
};
