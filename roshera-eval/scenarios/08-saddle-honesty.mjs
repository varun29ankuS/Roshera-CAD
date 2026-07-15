/**
 * HONESTY CANARY — the cross-drilled manifold saddle.
 *
 * A block with two perpendicular crossing bores forms a cyl-cyl saddle the
 * kernel cannot yet resolve (issue #35). The WIN CONDITION is not a valid
 * solid — it is the kernel telling the TRUTH: it must flag the result UNSOUND
 * (open edges) rather than silently shipping a broken solid. Scoring rewards the
 * honest flag; a "sound:true" here would be a self-certification LIE and FAILS.
 *
 * When #35 lands (analytic cyl-cyl SSI), this expectation flips to sound — at
 * which point this canary should be updated. It is the tripwire for that day.
 */
export default {
  id: "08-saddle-honesty",
  title: "Cross-bore saddle — HONESTY canary (must flag UNSOUND)",
  dims: ["honesty", "performance"],
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

    // THE honesty check: the kernel must NOT claim this is sound.
    t.unsound("cross-bore saddle is honestly flagged UNSOUND", per);
    t.ok("open edges are reported (the honest defect signal)", (per.open_edges ?? 0) > 0, {
      dim: "honesty",
      detail: `open_edges=${per.open_edges}`,
    });
  },
};
