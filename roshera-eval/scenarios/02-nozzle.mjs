/**
 * Rao-bell rocket nozzle: a contoured wall of revolution built in ONE op from
 * an inner flow contour + a constant wall thickness (the revolve band-explosion
 * TODO is proven fixed — it lands as just 4 smooth faces).
 *
 * Oracles:
 *  - soundness certificate
 *  - structural: exactly 4 faces (two smooth SurfaceOfRevolution walls + 2 rims)
 *  - hollow-tube genus: chi = 0
 *  - volume within +/-0.5% of the proven 16309.7 mm^3
 *  - meridian (axial) section area = 320 mm^2 +/-1% (the SECTION A-A cut)
 */
export default {
  id: "02-nozzle",
  title: "Rao-bell nozzle (revolve + wall thickness)",
  dims: ["correctness", "soundness", "performance"],
  budgetMs: 60000,
  async run(ctx, t) {
    const { c } = ctx;
    const profile = [[16, 0], [16, 15], [12, 25], [10, 32], [11.5, 40], [14, 50], [17, 62], [19, 72], [20, 80]];
    await ctx.time("revolve nozzle wall", () =>
      c.post("/api/geometry/revolve", {
        profile,
        axis_origin: [0, 0, 0],
        axis_direction: [0, 0, 1],
        angle_deg: 360,
        segments: 96,
        wall_thickness: 2,
        name: "rao_nozzle",
      }),
    );
    const id = await c.newestPartId();
    const per = await ctx.time("certify", () => c.perceive(id));

    t.sound("nozzle certifies sound", per);
    t.eq("4 faces (two smooth walls + 2 rims)", per.face_count, 4, { dim: "correctness" });
    t.eq("hollow tube genus (chi = 0)", per.euler, 0, { dim: "correctness" });
    t.approxRel("volume ~ 16309.7 mm^3", per.volume, 16309.7, 0.005);

    // Meridian section: a plane containing the axis (normal perpendicular to it).
    const sec = await ctx.time("meridian section", () => c.sectionArea(id, [0, 0, 40], [0, 1, 0]));
    t.eq("meridian section endpoint returns 200", sec.status, 200);
    t.approxRel("meridian section area = 320 mm^2", sec.data?.section_area, 320, 0.01);
  },
};
