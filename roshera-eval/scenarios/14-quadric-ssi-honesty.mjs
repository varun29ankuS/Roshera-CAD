/**
 * QUADRIC SURFACE-SURFACE INTERSECTION HONESTY — a curved-curved bite and
 * a near-tangency canary (OCCT-parity roadmap #2: quadric SSI; memory:
 * "general cyl∘sphere SSI still needed"; #35 Slice 2 unequal-radii saddle).
 *
 * A sphere bites the wall of a cylinder — a genuine quadric∘quadric
 * intersection whose section curve is a spatial quartic, not a circle. And
 * a near-TANGENT bore pair, the numerically degenerate case where an
 * inexact intersector invents a sliver or a phantom clean solid.
 *
 * # Why this scenario is honesty-shaped, not a fixed volume oracle
 *
 * The general cyl∘sphere lens has NO elementary closed-form volume, and
 * whether the kernel resolves the hard quadric cases soundly TODAY is
 * exactly what a live run reveals — so this scenario does not hard-assert a
 * single magic number. Instead it scores what CANNOT be faked regardless of
 * capability:
 *
 *   - a produced solid's volume must lie inside PHYSICAL BOUNDS computed
 *     independently here (you cannot remove more than the whole sphere, nor
 *     add material);
 *   - a `sound` verdict must be INTERNALLY CONSISTENT (watertight, manifold,
 *     no open / non-manifold edges) and, if sound, a single genus-0 solid;
 *   - an `unsound` verdict must NAME its defect — the kernel may honestly
 *     refuse a hard intersection, but "unsound for no stated reason" is
 *     itself a dishonest verdict;
 *   - the near-tangency case must not FABRICATE a clean solid: it either
 *     refuses, or returns a solid that is consistent AND physically bounded.
 *
 * # Why the oracle is a separate pure function
 *
 * `run` talks to a live backend; `oracle` does not. `test/oracle-14.mjs`
 * feeds it an honest transcript and single-mutation lies and proves it
 * tells them apart, so the scenario is known to discriminate before it is
 * ever pointed at a server. This is the tripwire for the day quadric SSI
 * lands — the same role the saddle scenario (08) played for #35 Slice 1.
 */

// Cylinder r10 × h30, sphere r6 seated on the wall — a scallop bite.
const CYL = { r: 10, h: 30 };
const SPH = { r: 6 };
const CYL_VOL = Math.PI * CYL.r * CYL.r * CYL.h; // 9424.78
const SPH_VOL = (4 / 3) * Math.PI * SPH.r * SPH.r * SPH.r; // 904.78
// The bite removes at most a whole sphere and never adds material.
const BITE_LO = CYL_VOL - SPH_VOL;
const BITE_HI = CYL_VOL;

// The near-tangency probe lives in a 40³ block; any produced solid must
// hold strictly positive, sub-block volume.
const BLOCK_VOL = 40 * 40 * 40;

/** A sound verdict is internally consistent: no hidden shell defects. */
function soundConsistent(per) {
  return (
    per?.sound !== true ||
    (per.watertight !== false &&
      per.manifold !== false &&
      per.self_intersection_free !== false &&
      !(per.open_edges > 0) &&
      !(per.nonmanifold_edges > 0))
  );
}
/** An unsound verdict must NAME its defect — an honest refusal, not a shrug. */
function unsoundNamesDefect(per) {
  if (per?.sound !== false) return true; // not an unsound verdict
  return (
    per.open_edges > 0 ||
    per.watertight === false ||
    per.manifold === false ||
    per.self_intersection_free === false ||
    (Array.isArray(per.issues) && per.issues.length > 0)
  );
}

/**
 * The PURE scoring oracle. No I/O, no client.
 *
 * @param t  the harness `Checks` collector
 * @param d  transcript: `{ bite: perception, tangency: { op, perception } }`
 */
export function oracle(t, d) {
  const bite = d.bite ?? {};

  // ── 1. The bite's volume is PHYSICALLY bounded (independent oracle) ───
  t.ok(
    "the cyl∘sphere bite removes at most a whole sphere and adds nothing",
    Number.isFinite(bite.volume) && bite.volume > BITE_LO - 1e-6 && bite.volume < BITE_HI + 1e-6,
    { dim: "correctness", detail: `volume=${bite.volume} bounds=(${BITE_LO.toFixed(2)}, ${BITE_HI.toFixed(2)})` },
  );

  // ── 2. HONESTY: the bite's verdict is internally consistent ──────────
  t.ok("the bite's sound verdict is internally consistent (no hidden defect)", soundConsistent(bite), {
    dim: "honesty",
    detail: `sound=${bite.sound} watertight=${bite.watertight} manifold=${bite.manifold} open_edges=${bite.open_edges}`,
  });

  // ── 3. HONESTY: an unsound bite NAMES its defect (honest refusal) ─────
  t.ok("an unsound bite names its defect rather than shrugging", unsoundNamesDefect(bite), {
    dim: "honesty",
    detail: `sound=${bite.sound} open_edges=${bite.open_edges} issues=${JSON.stringify(bite.issues)}`,
  });

  // ── 4. A sound bite is a single genus-0 solid ────────────────────────
  t.ok("a sound bite is a single genus-0 solid (chi = 2)", bite.sound !== true || bite.euler === 2, {
    dim: "correctness",
    detail: `sound=${bite.sound} euler=${bite.euler}`,
  });

  // ── 5. HONESTY: the near-tangency case never FABRICATES a clean solid ─
  //      It must refuse, OR return a consistent, physically-bounded solid.
  const tan = d.tangency ?? {};
  const refused = tan.op?.ok === false || tan.perception?.sound === false || tan.perception == null;
  const per = tan.perception ?? {};
  const producedButHonest =
    per.sound === true &&
    soundConsistent(per) &&
    Number.isFinite(per.volume) &&
    per.volume > 0 &&
    per.volume < BLOCK_VOL;
  t.ok(
    "the near-tangency intersection refuses rather than faking a clean solid",
    refused || producedButHonest,
    {
      dim: "honesty",
      detail: `op_ok=${tan.op?.ok} sound=${per.sound} volume=${per.volume} consistent=${soundConsistent(per)}`,
    },
  );
  // ── 6. HONESTY: if the near-tangency op claims a solid, its verdict is
  //      internally consistent (a sound:true masking open edges is a lie) ─
  t.ok("any near-tangency solid's sound verdict is internally consistent", soundConsistent(per), {
    dim: "honesty",
    detail: `sound=${per.sound} watertight=${per.watertight} open_edges=${per.open_edges}`,
  });
}

export default {
  id: "14-quadric-ssi-honesty",
  title: "Quadric SSI — cyl∘sphere bite + near-tangency canary (honest or refuse)",
  dims: ["correctness", "honesty", "performance"],
  budgetMs: 30000,
  oracle,
  async run(ctx, t) {
    const { c } = ctx;

    // ── The bite: a sphere seated on the cylinder wall (r=CYL.r) so half
    //    of it bites into the solid — a true quadric∘quadric SSI. ───────
    const cyl = await ctx.time("cylinder r10 h30", () =>
      c.post("/api/geometry/cylinder", {
        center: [0, 0, 0],
        axis: [0, 0, 1],
        radius: CYL.r,
        height: CYL.h,
        name: "barrel",
      }),
    );
    const sphere = await c.post("/api/geometry/sphere", {
      center: [CYL.r, 0, 0],
      radius: SPH.r,
      name: "biter",
    });
    await ctx.time("subtract the sphere (cyl∘sphere SSI)", () =>
      c.raw("POST", "/api/geometry/boolean", {
        operation: "difference",
        object_a: cyl.object.id,
        object_b: sphere.object.id,
        fast: true,
      }),
    );
    const bite = await ctx.time("certify the bite", async () => c.perceive(await c.newestPartId()));

    // ── The near-tangency canary: two perpendicular bores whose walls
    //    graze within ε. Block centred at [20,20,20], spanning [0,40]³.
    //    Bore A along X at z=30; bore B along Y at z = 30 − (2r − ε), so
    //    the two r=8 walls meet with a 1e-6 overlap — the degenerate
    //    grazing intersection an inexact SSI mis-resolves. ──────────────
    const R = 8;
    const box = await c.post("/api/geometry/box", {
      center: [20, 20, 20],
      u_axis: [1, 0, 0],
      v_axis: [0, 1, 0],
      width: 40,
      depth: 40,
      height: 40,
      name: "stock",
    });
    const boreA = await c.post("/api/geometry/cylinder", {
      center: [-10, 20, 30],
      axis: [1, 0, 0],
      radius: R,
      height: 60,
      name: "boreA",
    });
    const opA = await c.raw("POST", "/api/geometry/boolean", {
      operation: "difference",
      object_a: box.object.id,
      object_b: boreA.object.id,
      fast: true,
    });
    let uuid = await c.uuidForPart(await c.newestPartId());
    const boreB = await c.post("/api/geometry/cylinder", {
      center: [20, -10, 30 - (2 * R - 1e-6)],
      axis: [0, 1, 0],
      radius: R,
      height: 60,
      name: "boreB",
    });
    const opB = await ctx.time("near-tangent crossing bore", () =>
      c.raw("POST", "/api/geometry/boolean", {
        operation: "difference",
        object_a: uuid,
        object_b: boreB.object.id,
        fast: true,
      }),
    );
    void opA;
    let tanPer = null;
    if (opB.ok) {
      tanPer = await c.perceive(await c.newestPartId());
    }

    oracle(t, {
      bite,
      tangency: { op: { ok: opB.ok, status: opB.status }, perception: tanPer },
    });
  },
};
