/**
 * KINEMATIC ASSEMBLY — can the agent trust a certificate about a thing that
 * MOVES? (kinematic-assembly campaign, Slice 6; spec §3.7.)
 *
 * Every other scenario in this corpus certifies a static PART. This one
 * certifies a MECHANISM: the agent assembles a hinge + a slider from real
 * parts, mates them by label, solves, drives the joints, and reads the
 * motion-stamped interference table — all without rendering anything.
 *
 * # Why the oracle is a separate pure function
 *
 * `run` talks to a live backend; `oracle` does not. Splitting them lets the
 * scoring logic be validated on its own (`test/oracle-10.mjs` feeds it both
 * an honest transcript and a LYING one and proves it tells them apart), so
 * the scenario is known to discriminate before it is ever pointed at a
 * server. A scenario that cannot be shown to catch a lie is not evidence.
 *
 * # The honesty dimension
 *
 * This scenario's honesty checks are the point. Three ways the stack could
 * lie, each planted deliberately and each scored:
 *
 *   1. **A floating part.** An instance with no mate path to ground is not
 *      located by anything. `fully_grounded:false` must say so — a shaded
 *      render never would.
 *   2. **A contradictory mate pair.** Two mates that cannot both hold must
 *      produce a WITNESS naming exactly them, not a shrug.
 *   3. **An uncertifiable sweep.** A slider with no limits has unbounded
 *      travel and there is no finite range to certify. It must REFUSE and
 *      say why. Treating that refusal as a pass — "no interference found,
 *      therefore clear" — is the exact failure this dimension exists to
 *      catch, so the oracle FAILS a transcript whose refused sweep is
 *      reported clear-and-certified.
 */

/** Radians per degree. */
const DEG = Math.PI / 180;

/** The hinge's declared limit band (radians). */
const HINGE_BAND = 30 * DEG;

/**
 * The PURE scoring oracle: given a transcript of what the backend said,
 * record the checks. No I/O, no client — so it can be validated dry.
 *
 * @param t   the harness `Checks` collector
 * @param d   the transcript (see `run`, and `test/oracle-10.mjs` for the
 *            honest/lying fixtures)
 */
export function oracle(t, d) {
  // ── 1. The mates PLACE the parts (the answer is derived, not authored) ──
  t.ok("the mate system solves", d.solve?.solved === true, {
    detail: `solved=${d.solve?.solved} refused=${d.solve?.refused_reason ?? "-"}`,
  });
  t.ok("the solve converges", d.solve?.converged === true, {
    detail: `residual=${d.solve?.residual_norm}`,
  });

  // ── 2. Mobility is REPORTED, not failed: a hinge is a design ──────────
  const cert = d.certify?.certificate ?? {};
  t.ok(
    "the mechanism's mobility is reported, not treated as a defect",
    d.certify?.constrainedness?.status === "mobile" &&
      d.certify.constrainedness.dof > 0,
    { detail: JSON.stringify(d.certify?.constrainedness) },
  );

  // ── 3. HONESTY: the planted floating part is named ────────────────────
  t.ok(
    "the planted FLOATING part is caught (a render would never say so)",
    cert.fully_grounded === false,
    { dim: "honesty", detail: `fully_grounded=${cert.fully_grounded}` },
  );
  t.ok(
    "a floating part blocks soundness",
    d.certify?.sound === false,
    { dim: "honesty", detail: `sound=${d.certify?.sound}` },
  );

  // ── 4. HONESTY: the planted conflict is WITNESSED, not shrugged at ────
  const witnesses = d.conflict?.witnesses ?? [];
  t.ok(
    "the contradictory mate pair produces a conflict witness",
    witnesses.length > 0,
    { dim: "honesty", detail: `${witnesses.length} witness(es)` },
  );
  const w = witnesses[0];
  t.ok(
    "the witness NAMES exactly the two mates that fight",
    !!w &&
      Array.isArray(w.mates) &&
      w.mates.length === 2 &&
      d.conflict.planted.every((m) => w.mates.includes(m)),
    {
      dim: "honesty",
      detail: `witness=${JSON.stringify(w?.mates)} planted=${JSON.stringify(d.conflict?.planted)}`,
    },
  );
  t.ok(
    "the witness is honestly flagged minimal (or honestly flagged NOT)",
    !!w && typeof w.minimal === "boolean",
    { dim: "honesty", detail: `minimal=${w?.minimal}` },
  );

  // ── 5. The agent's kinematic hand: drive the hinge ────────────────────
  t.ok("the hinge drives to a value inside its band", d.drag_in?.converged === true, {
    detail: `converged=${d.drag_in?.converged} applied=${d.drag_in?.applied}`,
  });
  t.approxAbs(
    "the joint lands on the value asked for",
    d.drag_in?.applied,
    15 * DEG,
    1e-9,
  );
  t.ok(
    "an in-band drive reports NO limit hit",
    d.drag_in?.limit == null,
    { detail: JSON.stringify(d.drag_in?.limit) },
  );
  t.ok(
    "the drag reports the scope it moved (never a blind mutation)",
    Array.isArray(d.drag_in?.scope?.instances) &&
      d.drag_in.scope.instances.length > 0 &&
      d.drag_in?.constrainedness != null,
    { detail: JSON.stringify(d.drag_in?.scope) },
  );

  // ── 6. Limits CLAMP and say so ────────────────────────────────────────
  t.ok("a beyond-limit drive is clamped, not errored", d.drag_out?.dragged === true, {
    detail: `dragged=${d.drag_out?.dragged}`,
  });
  t.approxAbs(
    "the joint bottoms out AT its declared limit",
    d.drag_out?.applied,
    HINGE_BAND,
    1e-9,
  );
  t.ok(
    "and the at-limit fact is reported, quoting what was asked for",
    d.drag_out?.limit != null &&
      Math.abs(d.drag_out.limit.requested - 90 * DEG) < 1e-9,
    { detail: JSON.stringify(d.drag_out?.limit) },
  );

  // ── 7. The joints are DERIVED from the mates — nothing authored ───────
  const sweeps = d.interference?.sweeps ?? [];
  t.ok(
    "every swept motion is a joint DERIVED from a mate — nothing authored",
    sweeps.length > 0 && sweeps.every((s) => s.source?.source === "driven_mate"),
    { detail: JSON.stringify(sweeps.map((s) => s.source)) },
  );
  t.ok(
    "the swept gate is CONTINUOUS (a thin part cannot slip between samples)",
    sweeps.length > 0 &&
      sweeps.every((s) => s.refusal != null || s.method?.method === "nonlinear_toi"),
    { detail: JSON.stringify(sweeps.map((s) => s.method)) },
  );
  t.ok(
    "ε is kernel-derived and recorded, never zero",
    (d.interference?.epsilon?.effective ?? 0) > 0,
    { detail: JSON.stringify(d.interference?.epsilon) },
  );

  // ── 8. HONESTY: an uncertifiable sweep REFUSES, and a refusal is not a
  //      pass. This is the sharpest check in the scenario: the slider has
  //      unbounded travel, so there is no finite range to certify. A stack
  //      that reported it as certified-clear would be inventing a verdict.
  const refused = sweeps.filter((s) => s.refusal != null);
  t.ok(
    "the unbounded slider's sweep REFUSES rather than inventing a range",
    refused.length > 0,
    { dim: "honesty", detail: `${refused.length} refused sweep(s)` },
  );
  t.ok(
    "a refused sweep never claims a certified clearance",
    refused.every(
      (s) => s.min_certified_clearance == null && s.first_contact == null,
    ),
    {
      dim: "honesty",
      detail: JSON.stringify(
        refused.map((s) => ({
          min: s.min_certified_clearance,
          first: s.first_contact,
        })),
      ),
    },
  );

  // ── 9. The interference is MOTION-STAMPED: "they hit AT θ" ────────────
  const hits = sweeps.flatMap((s) => s.interference ?? []);
  const stamped = hits.filter((h) => Number.isFinite(h?.at?.param));
  t.ok(
    "the planted swing collision is found and MOTION-STAMPED",
    stamped.length > 0,
    {
      detail: `hits=${hits.length} stamped=${stamped.length} params=${JSON.stringify(
        stamped.map((h) => h.at.param),
      )}`,
    },
  );
  t.ok(
    "every interference fact carries a depth and the angle it happens at",
    hits.length === 0 || hits.every((h) => h.depth > 0 && Number.isFinite(h.at?.param)),
    { dim: "honesty", detail: JSON.stringify(hits.slice(0, 3)) },
  );
}

export default {
  id: "10-assembly-kinematics",
  title: "Kinematic assembly — hinge + slider: solve, drive, certify a MOTION",
  dims: ["correctness", "soundness", "honesty", "performance"],
  budgetMs: 60000,
  oracle,
  async run(ctx, t) {
    const { c } = ctx;

    // ── Parts: a base, a swinging arm, a slider rod, and a stop block ──
    const base = await c.post("/api/geometry/box", {
      center: [0, 0, 0], u_axis: [1, 0, 0], v_axis: [0, 1, 0],
      width: 20, depth: 20, height: 4, name: "base",
    });
    const arm = await c.post("/api/geometry/box", {
      center: [0, 0, 0], u_axis: [1, 0, 0], v_axis: [0, 1, 0],
      width: 24, depth: 3, height: 3, name: "arm",
    });
    const rod = await c.post("/api/geometry/cylinder", {
      center: [0, 0, 0], axis: [0, 0, 1], radius: 2, height: 16, name: "rod",
    });
    const stop = await c.post("/api/geometry/box", {
      center: [0, 0, 0], u_axis: [1, 0, 0], v_axis: [0, 1, 0],
      width: 4, depth: 4, height: 6, name: "stop",
    });

    // ── The assembly document ─────────────────────────────────────────
    const asm = await c.post("/api/assembly", { name: "hinge_and_slider" });
    const aid = asm.id;
    const place = async (part, transform, name) =>
      (await c.post(`/api/assembly/${aid}/instance`, {
        part_id: await c.uuidForPart(part.object.id),
        transform,
        name,
      })).id;
    const I = (x, y, z) => [
      [1, 0, 0, x], [0, 1, 0, y], [0, 0, 1, z], [0, 0, 0, 1],
    ];
    const iBase = await place(base, I(0, 0, 0), "base");
    const iArm = await place(arm, I(0, 0, 4), "arm");
    const iRod = await place(rod, I(0, 0, 4), "rod");
    // The stop sits ON the arm's swing circle — the planted collision.
    const iStop = await place(stop, I(9, 9, 6), "stop");

    // ── Mates ─────────────────────────────────────────────────────────
    const connector = async (instance_id, frame) =>
      (await c.post(`/api/assembly/${aid}/connector`, { instance_id, frame })).id;
    const F = (origin, z_axis = [0, 0, 1], x_axis = [1, 0, 0]) => ({
      origin, z_axis, x_axis,
    });
    const mate = async (kind, a, b, couples) =>
      c.post(`/api/assembly/${aid}/mate`, { kind, a, b, couples: couples ?? null });

    // Hinge: the arm swings on the base about z, LIMITED to ±30°.
    const hinge = await mate(
      { Revolute: { limits: [-HINGE_BAND, HINGE_BAND] } },
      await connector(iBase, F([0, 0, 2])),
      await connector(iArm, F([0, 0, -1.5])),
    );
    // Slider: the rod runs up the base's axis. NO limits — unbounded travel,
    // which the swept gate must honestly refuse to certify.
    await mate(
      { Slider: { limits: null } },
      await connector(iBase, F([0, 0, 2])),
      await connector(iRod, F([0, 0, -8])),
    );
    // The stop block is left FLOATING on purpose (no mate) — plant #1.

    const solve = await ctx.time("solve the mechanism", () =>
      c.post(`/api/assembly/${aid}/solve`, { ground: iBase }),
    );
    const certify = await ctx.time("certify the mechanism", () =>
      c.post(`/api/assembly/${aid}/certify`, { ground: iBase }),
    );

    // ── Drive the hinge ───────────────────────────────────────────────
    const drag_in = await ctx.time("drive the hinge to 15°", () =>
      c.post(`/api/assembly/${aid}/drag`, {
        mate_id: hinge.id, param: "rotation", value: 15 * DEG, ground: iBase,
      }),
    );
    const drag_out = await c.post(`/api/assembly/${aid}/drag`, {
      mate_id: hinge.id, param: "rotation", value: 90 * DEG, ground: iBase,
    });

    const interference = await ctx.time("motion-stamped interference table", () =>
      c.raw("GET", `/api/assembly/${aid}/interference?ground=${iBase}`),
    );

    // ── Plant #2: a contradictory mate pair, on a SEPARATE assembly so it
    //    cannot perturb the mechanism above.
    const asm2 = await c.post("/api/assembly", { name: "conflict_probe" });
    const bid = asm2.id;
    const p1 = (await c.post(`/api/assembly/${bid}/instance`, {
      part_id: await c.uuidForPart(base.object.id), transform: I(0, 0, 0), name: "g",
    })).id;
    const p2 = (await c.post(`/api/assembly/${bid}/instance`, {
      part_id: await c.uuidForPart(arm.object.id), transform: I(0, 0, 4), name: "m",
    })).id;
    const conn = async (instance_id, frame) =>
      (await c.post(`/api/assembly/${bid}/connector`, { instance_id, frame })).id;
    // The same part fastened to TWO different places: impossible.
    const m1 = await c.post(`/api/assembly/${bid}/mate`, {
      kind: "Fastened",
      a: await conn(p1, F([0, 0, 2])),
      b: await conn(p2, F([0, 0, -1.5])),
      couples: null,
    });
    const m2 = await c.post(`/api/assembly/${bid}/mate`, {
      kind: "Fastened",
      a: await conn(p1, F([0, 0, 7])),
      b: await conn(p2, F([0, 0, -1.5])),
      couples: null,
    });
    const conflictCert = await c.post(`/api/assembly/${bid}/certify`, { ground: p1 });

    oracle(t, {
      solve,
      certify,
      drag_in,
      drag_out,
      interference,
      conflict: {
        witnesses: conflictCert.witnesses ?? [],
        planted: [m1.id, m2.id],
      },
    });
  },
};
