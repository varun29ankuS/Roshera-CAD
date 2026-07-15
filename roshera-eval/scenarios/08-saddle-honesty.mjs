/**
 * THE SADDLE — formerly the honesty canary, now the #35 regression guard.
 *
 * A block with two perpendicular crossing bores forms the cyl-cyl saddle
 * (issue #35, THE core CAD bug). Until 2026-07-15 the kernel could not
 * resolve it and this scenario scored the HONEST refusal (UNSOUND flagged,
 * never a silent lie). #35 Slice 1 landed (analytic saddle ellipses with
 * shared crossing vertices + the saddle lateral splitter + the conforming
 * ring-stitch mesher): the saddle now BUILDS SOUND with the volume matching
 * the Steinmetz oracle. The tripwire fired; this scenario now guards the fix.
 */
export default {
  id: "08-saddle-honesty",
  title: "Cross-bore saddle — builds SOUND (#35 slice 1 guard)",
  dims: ["correctness", "soundness", "performance"],
  budgetMs: 30000,
  async run(ctx, t) {
    const { c } = ctx;
    const box = await ctx.time("block 40^3", () =>
      c.post("/api/geometry/box", { center: [0, 0, 0], u_axis: [1, 0, 0], v_axis: [0, 1, 0], width: 40, depth: 40, height: 40, name: "block" }),
    );
    let uuid = box.object.id;

    // First bore along X.
    const boreX = await c.post("/api/geometry/cylinder", { center: [-30, 0, 20], axis: [1, 0, 0], radius: 8, height: 60, name: "boreX" });
    await c.post("/api/geometry/boolean", { operation: "difference", object_a: uuid, object_b: boreX.object.id, fast: true });
    uuid = await c.uuidForPart(await c.newestPartId());
    const perX = await c.perceive(await c.newestPartId());
    t.sound("single bore (no saddle yet) is sound", perX, { dim: "honesty" });

    // Second, perpendicular bore along Y — this creates the saddle.
    const boreY = await c.post("/api/geometry/cylinder", { center: [0, -30, 20], axis: [0, 1, 0], radius: 8, height: 60, name: "boreY" });
    await ctx.time("crossing bore (forms saddle)", () =>
      c.raw("POST", "/api/geometry/boolean", { operation: "difference", object_a: uuid, object_b: boreY.object.id, fast: true }),
    );
    const id = await c.newestPartId();
    const per = await ctx.time("certify saddle", () => c.perceive(id));

    // #35 Slice 1 (2026-07-15): the saddle builds SOUND. Volume oracle for THIS
    // recipe (40³ block, two r=8 through-bores of in-block length 40, crossing):
    // V = 64000 − 2·(π·8²·40) + 16·8³/3 (Steinmetz) ≈ 50645.71.
    const oracle = 64000 - 2 * (Math.PI * 64 * 40) + (16 * 512) / 3;
    t.sound("cross-bore saddle builds SOUND (#35 slice 1)", per);
    t.approxRel("volume matches the Steinmetz oracle", per.volume, oracle, 0.005, {
      dim: "correctness",
    });
  },
};
