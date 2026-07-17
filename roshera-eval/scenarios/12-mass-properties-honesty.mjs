/**
 * MASS-PROPERTIES HONESTY — does the metrology kernel state its own
 * accuracy, or hand back a bare number dressed as truth? (exact-mass-
 * properties campaign; `MassPropertiesProvenance` honesty contract.)
 *
 * A self-certifying metrology kernel states achieved accuracy per
 * quantity, the way `PK_TOPOL_eval_mass_props` returns an achieved
 * accuracy. A box (untrimmed planar faces) is algebraically exactly
 * integrable — every quantity is `Exact`. A cylinder (curved faces) is a
 * mesh/quadrature estimate — its inertia is `Approximate`, and it must SAY
 * SO, carrying the method that produced it and a relative-error ceiling
 * the agent can trust the value to within.
 *
 * # Why the oracle is a separate pure function
 *
 * `run` talks to a live backend; `oracle` does not. `test/oracle-12.mjs`
 * feeds the oracle an honest transcript and single-mutation lies and proves
 * it tells them apart, so the scenario is known to discriminate before it
 * is ever pointed at a server.
 *
 * # The honesty dimension
 *
 * The scenario FAILS if the kernel labels a mesh estimate `Exact`. Three
 * planted lies, each scored:
 *
 *   1. **A curved solid's inertia dressed as Exact.** The cylinder's second
 *      moments come off the tessellation; calling them exact is inventing a
 *      precision the kernel does not have.
 *   2. **An `Exact` label over a loose number.** If a quantity claims Exact,
 *      its value must equal the closed-form to floating-point noise — an
 *      exact label on a 5%-off number is a lie about the label.
 *   3. **A self-certified bound that does not hold.** An `Approximate`
 *      quantity states a relative-error ceiling; if the value sits OUTSIDE
 *      that ceiling versus the analytic oracle, the certificate is false.
 *
 * The independent analytic oracles (closed-form box and cylinder volumes)
 * are computed here and never come from the kernel — so a wrong number is
 * caught no matter how wide the kernel widens its own bound.
 */

// Box 20 × 10 × 30 — an untrimmed polyhedron, exactly integrable.
const BOX = { w: 20, d: 10, h: 30 };
const BOX_VOL = BOX.w * BOX.d * BOX.h; // 6000, closed-form exact
// Cylinder r5 × h20 — curved, a mesh/quadrature estimate.
const CYL = { r: 5, h: 20 };
const CYL_VOL = Math.PI * CYL.r * CYL.r * CYL.h; // 500π

/** A provenance field is Exact. */
const isExact = (p) => p?.exactness === "exact";
/** A provenance field is Approximate with a usable, self-certified bound. */
function isApproxWithBound(p) {
  return (
    p?.exactness === "approximate" &&
    typeof p?.method === "string" &&
    p.method.length > 0 &&
    Number.isFinite(p?.rel_error_bound) &&
    p.rel_error_bound > 0
  );
}
/** Relative error of `actual` against the independent analytic `oracle`. */
const relErr = (actual, oracle) => Math.abs(actual - oracle) / Math.abs(oracle);

/**
 * The PURE scoring oracle. No I/O, no client.
 *
 * @param t  the harness `Checks` collector
 * @param d  the transcript: `{ box: MassReport, cyl: MassReport }`
 */
export function oracle(t, d) {
  const box = d.box ?? {};
  const cyl = d.cyl ?? {};
  const bp = box.provenance ?? {};
  const cp = cyl.provenance ?? {};

  // ── 1. The box volume matches its closed-form value ──────────────────
  t.approxRel("the box volume matches its closed-form value", box.volume, BOX_VOL, 1e-9);

  // ── 2. HONESTY: the box's EXACT label is honest — an exact-labelled
  //      value must equal the closed form to floating-point noise ───────
  t.ok(
    "the box's mass-props are labelled Exact (an exactly-integrable solid)",
    isExact(bp.volume) && isExact(bp.center_of_mass) && isExact(bp.inertia),
    { dim: "honesty", detail: `volume=${JSON.stringify(bp.volume)} inertia=${JSON.stringify(bp.inertia)}` },
  );
  t.ok(
    "an Exact volume label is backed by a value equal to the closed form",
    !isExact(bp.volume) || relErr(box.volume, BOX_VOL) <= 1e-9,
    { dim: "honesty", detail: `exactness=${bp.volume?.exactness} rel_err=${relErr(box.volume, BOX_VOL)}` },
  );

  // ── 3. HONESTY (core): a CURVED solid's inertia is NOT dressed as Exact
  t.ok(
    "the cylinder's inertia is reported Approximate, never Exact (a mesh estimate cannot be exact)",
    cp.inertia?.exactness === "approximate",
    { dim: "honesty", detail: `inertia=${JSON.stringify(cp.inertia)}` },
  );

  // ── 4. HONESTY: an Approximate quantity carries method + a usable bound
  t.ok(
    "every Approximate cylinder quantity states its method and a positive error bound",
    isApproxWithBound(cp.volume) && isApproxWithBound(cp.inertia) && isApproxWithBound(cp.center_of_mass),
    {
      dim: "honesty",
      detail: `volume=${JSON.stringify(cp.volume)} com=${JSON.stringify(cp.center_of_mass)} inertia=${JSON.stringify(cp.inertia)}`,
    },
  );

  // ── 5. HONESTY (killer): the self-certified volume bound actually HOLDS
  t.ok(
    "the cylinder's self-certified volume error bound actually holds vs the analytic oracle",
    cp.volume?.exactness !== "approximate" ||
      relErr(cyl.volume, CYL_VOL) <= cp.volume.rel_error_bound,
    {
      dim: "honesty",
      detail: `rel_err=${relErr(cyl.volume, CYL_VOL)} bound=${cp.volume?.rel_error_bound}`,
    },
  );

  // ── 6. The cylinder volume is correct within a generous independent
  //      tolerance — caught no matter how wide the kernel widens its bound
  t.approxRel("the cylinder volume matches the analytic πr²h oracle", cyl.volume, CYL_VOL, 0.02);
}

export default {
  id: "12-mass-properties-honesty",
  title: "Mass-properties provenance — Exact box vs honestly-Approximate cylinder",
  dims: ["correctness", "honesty", "performance"],
  budgetMs: 20000,
  oracle,
  async run(ctx, t) {
    const { c } = ctx;

    const box = await ctx.time("box 20×10×30", () =>
      c.post("/api/geometry/box", {
        center: [0, 0, 0],
        u_axis: [1, 0, 0],
        v_axis: [0, 1, 0],
        width: BOX.w,
        depth: BOX.d,
        height: BOX.h,
        name: "block",
      }),
    );
    const boxId = await c.newestPartId();
    const boxMass = await ctx.time("box mass properties", () => c.mass(boxId));

    const cyl = await ctx.time("cylinder r5 h20", () =>
      c.post("/api/geometry/cylinder", {
        center: [0, 0, 0],
        axis: [0, 0, 1],
        radius: CYL.r,
        height: CYL.h,
        name: "rod",
      }),
    );
    const cylId = await c.newestPartId();
    const cylMass = await ctx.time("cylinder mass properties", () => c.mass(cylId));

    // Silence unused-binding lint intent: the create responses are the
    // parts the mass reads describe.
    void box;
    void cyl;

    oracle(t, { box: boxMass, cyl: cylMass });
  },
};
