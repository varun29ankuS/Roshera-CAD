/**
 * Pocketed + bored block: a 60x40x30 block with a 40x20 pocket milled 20 mm
 * deep (10 mm floor), four Ø5 through-bores on a r10 bolt circle, then a
 * graceful fillet-all r1.5.
 *
 * Oracles:
 *  - post-pocket volume is EXACT: 60*40*30 - 40*20*20 = 56000 mm^3
 *  - four through-bores -> genus 4 -> chi = -6
 *  - the drilled block and the fillet-all both stay sound
 */
import { subtractBoxes, drillCylinders, filletAll } from "../lib/builders.mjs";

export default {
  id: "05-pocket-block",
  title: "Pocketed + bored block + fillet-all r1.5",
  dims: ["correctness", "soundness", "performance"],
  budgetMs: 90000,
  async run(ctx, t) {
    const { c } = ctx;
    const block = await ctx.time("block 60x40x30", () =>
      c.post("/api/geometry/box", { center: [0, 0, 0], u_axis: [1, 0, 0], v_axis: [0, 1, 0], width: 60, depth: 40, height: 30, name: "block" }),
    );
    // Pocket 40x20, cut from the top 20 mm deep: base z=10, height 20.2 (overshoot z=30).
    const { uuid: uPocket, id: idPocket } = await ctx.time("mill pocket", () =>
      subtractBoxes(c, block.object.id, [{ center: [0, 0, 10], width: 40, depth: 20, height: 20.2, name: "pocket" }]),
    );
    const perPocket = await c.perceive(idPocket);
    t.sound("pocketed block certifies sound", perPocket);
    t.approxRel("post-pocket volume == 56000 mm^3 exactly", perPocket.volume, 56000, 0.001);

    // Four Ø5 bores on a r10 circle, phased 45 deg so they clear the pocket walls.
    const holes = [];
    for (let k = 0; k < 4; k++) {
      const th = Math.PI / 4 + (2 * Math.PI * k) / 4;
      holes.push({ center: [10 * Math.cos(th), 10 * Math.sin(th), -1], axis: [0, 0, 1], radius: 2.5, height: 32 });
    }
    const { id: idDrill } = await ctx.time("drill 4 bores", () => drillCylinders(c, uPocket, holes));
    const perDrill = await c.perceive(idDrill);
    t.sound("drilled block certifies sound", perDrill);
    t.eq("four through-bores -> chi = -6", perDrill.euler, -6, { dim: "correctness" });

    const fr = await ctx.time("fillet-all r1.5", () => filletAll(c, idDrill, 1.5));
    t.eq("fillet-all endpoint returns 200", fr.status, 200, { dim: "soundness" });
    const perFillet = await c.perceive(fr.id);
    t.sound("filleted block certifies sound", perFillet);
  },
};
