/**
 * Bulkhead panel: a 100x80x10 plate with four 40x30 pockets milled 6 mm deep
 * (leaving a 4 mm floor), then a graceful fillet-all r1.5.
 *
 * Oracles:
 *  - pre-fillet volume is EXACT: 100*80*10 - 4*(40*30*6) = 51200 mm^3
 *  - each pocket step and the fillet-all stay sound
 *  - fillet-all is graceful (rounds what it can, adds blend faces, stays a
 *    valid closed solid) — volume stays within a sane window of the pre-fillet
 */
import { subtractBoxes, filletAll } from "../lib/builders.mjs";

export default {
  id: "04-bulkhead",
  title: "Bulkhead (100x80x10 - 4 pockets) + graceful fillet-all",
  dims: ["correctness", "soundness", "performance"],
  budgetMs: 90000,
  async run(ctx, t) {
    const { c } = ctx;
    const panel = await ctx.time("panel 100x80x10", () =>
      c.post("/api/geometry/box", { center: [0, 0, 0], u_axis: [1, 0, 0], v_axis: [0, 1, 0], width: 100, depth: 80, height: 10, name: "panel" }),
    );
    // Pocket tool boxes: 40x30, 6 mm deep from the top (base z=4, height 6.2 to
    // overshoot the z=10 face), one in each quadrant.
    const boxes = [[26, 21], [-26, 21], [26, -21], [-26, -21]].map(([px, py]) => ({
      center: [px, py, 4], width: 40, depth: 30, height: 6.2, name: "pocket",
    }));
    const { uuid, id } = await ctx.time("mill 4 pockets", () => subtractBoxes(c, panel.object.id, boxes));

    const perPocket = await c.perceive(id);
    t.sound("panel + 4 pockets certifies sound", perPocket);
    t.approxRel("pre-fillet volume == 51200 mm^3 exactly", perPocket.volume, 51200, 0.001);

    const fr = await ctx.time("fillet-all r1.5", () => filletAll(c, id, 1.5));
    t.eq("fillet-all endpoint returns 200", fr.status, 200, { dim: "soundness" });
    const perFillet = await ctx.time("certify filleted", () => c.perceive(fr.id));
    t.sound("filleted bulkhead certifies sound", perFillet);
    t.ok("fillet added blend faces", perFillet.face_count > perPocket.face_count, {
      detail: `${perPocket.face_count} -> ${perFillet.face_count} faces`,
    });
    t.approxRel("volume stays within 5% of pre-fillet (graceful)", perFillet.volume, 51200, 0.05);
  },
};
