/**
 * Drawing PERFORMANCE + liveness under load. Generate the standard four-view
 * sheet for the 293-face spur gear (the heaviest HLR/dimension job in the
 * corpus) and assert:
 *   - it completes within the 90 s budget (regression guard for the drawing
 *     compute path — #33 filed the gear sheet wedging the backend)
 *   - the /health endpoint stays LIVE throughout (polled concurrently) — the
 *     drawing job must not hold the write lock and freeze the server
 *   - the layout-quality invariants pass
 */
import { involuteGearProfile, keyedBoreProfile } from "../lib/geom.mjs";
import { extrudeProfiles } from "../lib/builders.mjs";

export default {
  id: "09-drawing-perf",
  title: "Gear four-view drawing < 90 s, health stays live",
  dims: ["performance", "correctness"],
  budgetMs: 130000,
  async run(ctx, t) {
    const { c } = ctx;
    const gear = involuteGearProfile();
    const bore = keyedBoreProfile();
    const id = await ctx.time("build gear", () => extrudeProfiles(c, [gear.pts, bore.pts], 8, "gear_for_drawing"));
    t.eq("gear is the 293-face part", (await c.partReport(id)).topology.face_count, 293, { dim: "correctness" });

    // Poll /health concurrently while the (heavy) drawing computes.
    let healthLive = true;
    let polls = 0;
    const poller = setInterval(async () => {
      polls++;
      try {
        const h = await c.raw("GET", "/health", undefined, 3000);
        if (h.data?.status !== "healthy") healthLive = false;
      } catch {
        healthLive = false;
      }
    }, 3000);

    const DRAW_BUDGET = 90000;
    const t0 = Date.now();
    let drawMs = null;
    let drawStatus = null;
    let passed = null;
    let wedged = false;
    try {
      const dr = await c.raw("POST", `/api/parts/${id}/drawing?name=spur_gear`, undefined, DRAW_BUDGET);
      drawMs = Date.now() - t0;
      drawStatus = dr.status;
      passed = dr.data?.quality?.passed;
    } catch (e) {
      drawMs = Date.now() - t0;
      wedged = true;
      t.record("performance", "drawing did not time out / wedge the backend", false, `after ${drawMs}ms: ${e.message}`);
    } finally {
      clearInterval(poller);
    }

    if (!wedged) {
      t.record("performance", `gear drawing completes < ${DRAW_BUDGET}ms`, drawMs <= DRAW_BUDGET, `${drawMs}ms`);
      t.eq("drawing endpoint returns 200", drawStatus, 200, { dim: "correctness" });
      t.ok("drawing quality passed:true", passed === true, { dim: "correctness", detail: `passed=${passed}` });
    }
    t.record("performance", "health stayed live throughout drawing", healthLive, `${polls} concurrent /health polls, all healthy=${healthLive}`);

    // Confirm the server is responsive after the job.
    const h = await c.raw("GET", "/health", undefined, 5000);
    t.ok("server responsive after drawing", h.data?.status === "healthy", { dim: "performance" });
  },
};
