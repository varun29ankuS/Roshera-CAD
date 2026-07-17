/**
 * CERTIFIED CONSTRAINED SKETCH → TRUE CYLINDRICAL BORE (SKETCH-DCM #45,
 * slices 1–5). Every other geometry scenario in this corpus starts from a
 * primitive; this one starts from the parametric sketcher — the thing an
 * agent actually drives when it "draws" — and asks the harder question:
 * can the agent trust the kernel's verdict about a CONSTRAINT SYSTEM?
 *
 * The agent builds a dimensioned plate profile with a bore, drives the
 * Newton solver, reads the certified-sketch verdict (DOF + witnesses),
 * and extrudes to a solid whose hole is a TRUE analytic cylinder — the
 * same lateral `create_cylinder` emits, not a chord-sampled facet fan.
 *
 * # Why the oracle is a separate pure function
 *
 * `run` talks to a live backend; `oracle` does not. Splitting them lets
 * the scoring logic be validated on its own (`test/oracle-11.mjs` feeds it
 * an honest transcript and a set of single-mutation LIES and proves it
 * tells them apart), so the scenario is known to discriminate before it is
 * ever pointed at a server. A scenario that cannot be shown to catch a lie
 * is not evidence.
 *
 * # The honesty dimension
 *
 * The planted lie is an OVER-CONSTRAINED sketch. Two dimensional
 * constraints pin the same length to different values — the system has no
 * solution. A stack that "solves" it anyway (reports FullyConstrained /
 * converged / sound) is inventing a verdict; the certified sketcher must
 * instead return `Conflicting`, refuse soundness, and emit a WITNESS that
 * NAMES the constraints that fight. Treating an inconsistent sketch as
 * solved is the exact failure this dimension exists to catch.
 *
 * A second honesty probe guards the bore itself: the extrude response
 * carries an honest `sampled_loops` counter. A circular hole must extrude
 * with ZERO sampled loops — a chord-sampled bore dressed up as a true
 * cylinder is a lie about the geometry the agent will machine.
 */

import { randomUUID } from "node:crypto";

const PLATE_W = 40;
const PLATE_H = 30;
const BORE_R = 6;
const DEPTH = 5;

/** A fully-constrained sketch reports this DOF verdict; anything else means
 *  hidden freedom the kernel must not paper over. */
function isFullyConstrained(c) {
  return c === "FullyConstrained";
}
/** Externally-tagged `Conflicting { conflicts }` — the inconsistent verdict. */
function isConflicting(c) {
  return !!c && typeof c === "object" && Object.prototype.hasOwnProperty.call(c, "Conflicting");
}
/** The sketch certificate's single can't-lie predicate, computed the way
 *  `SketchValidityCertificate::is_sound` computes it. */
function certSound(cert) {
  return (
    cert?.constraint_consistent === true &&
    cert?.entities_valid === true &&
    cert?.self_intersection_free === true
  );
}

/**
 * The PURE scoring oracle: given a transcript of what the backend said,
 * record the checks. No I/O, no client — so it can be validated dry.
 *
 * @param t  the harness `Checks` collector
 * @param d  the transcript (see `run`, and `test/oracle-11.mjs` fixtures)
 */
export function oracle(t, d) {
  const g = d.good ?? {};
  const summary = g.solve?.certificate ?? {};
  const cert = g.certify ?? {};

  // ── 1. The solver actually solves the well-posed sketch ───────────────
  t.ok("the constrained sketch solves to convergence", g.solve?.status?.kind === "converged", {
    detail: `status=${JSON.stringify(g.solve?.status)}`,
  });

  // ── 2. It is FULLY constrained: zero free DOF, no excess ──────────────
  t.ok(
    "the sketch is fully constrained (zero free DOF)",
    isFullyConstrained(summary.constrainedness) && summary.free_dofs === 0,
    { detail: `constrainedness=${JSON.stringify(summary.constrainedness)} free_dofs=${summary.free_dofs}` },
  );

  // ── 3. HONESTY: a 'fully constrained' verdict is BACKED by zero DOF ───
  //      (a sketch cannot claim full constraint while carrying free DOF).
  t.ok(
    "a fully-constrained verdict carries no hidden freedom",
    !isFullyConstrained(summary.constrainedness) || summary.free_dofs === 0,
    { dim: "honesty", detail: `constrainedness=${JSON.stringify(summary.constrainedness)} free_dofs=${summary.free_dofs}` },
  );

  // ── 4. The full certificate calls the sketch SOUND ────────────────────
  t.ok("the sketch certifies SOUND", certSound(cert), {
    dim: "soundness",
    detail: `consistent=${cert.constraint_consistent} valid=${cert.entities_valid} nonself=${cert.self_intersection_free}`,
  });
  t.ok("the profile is a closed region (extrude-ready)", cert.closed_profile === true, {
    detail: `closed_profile=${cert.closed_profile} profile=${cert.profile}`,
  });

  // ── 5. The bore extrudes ANALYTICALLY — no chord-sampled loops ────────
  const stats = g.extrude?.stats ?? {};
  t.ok(
    "the circular bore extrudes as a TRUE cylinder (zero sampled loops)",
    stats.sampled_loops === 0 && stats.analytic_loops >= 2,
    { detail: `analytic_loops=${stats.analytic_loops} sampled_loops=${stats.sampled_loops}` },
  );

  // ── 6. The extruded solid is a SOUND through-bored plate ──────────────
  const solid = g.solid ?? {};
  t.sound("the extruded plate certifies sound", solid);
  t.ok(
    "the solid's soundness verdict is internally consistent (sound implies watertight, zero open edges)",
    solid.sound !== true || (solid.open_edges === 0 && solid.watertight !== false),
    { dim: "honesty", detail: `sound=${solid.sound} watertight=${solid.watertight} open_edges=${solid.open_edges}` },
  );
  t.eq("the plate is a genuine through-bore (chi = 0, genus 1)", solid.euler, 0, {
    dim: "correctness",
  });
  const volOracle = (PLATE_W * PLATE_H - Math.PI * BORE_R * BORE_R) * DEPTH;
  t.approxRel("extruded volume matches the analytic plate-minus-bore oracle", solid.volume, volOracle, 0.005);

  // ── 7. HONESTY: the over-constrained sketch is CAUGHT, not solved ─────
  const oc = d.over?.certify ?? {};
  t.ok("the over-constrained sketch is NOT called sound", certSound(oc) === false, {
    dim: "honesty",
    detail: `consistent=${oc.constraint_consistent}`,
  });
  t.ok(
    "the over-constrained sketch is reported Conflicting, not FullyConstrained",
    isConflicting(oc.constrainedness),
    { dim: "honesty", detail: `constrainedness=${JSON.stringify(oc.constrainedness)}` },
  );
  t.ok(
    "the diagnostic solver verdict is 'conflicting', never 'converged'",
    oc.solver?.kind === "conflicting",
    { dim: "honesty", detail: `solver=${JSON.stringify(oc.solver)}` },
  );

  // ── 8. HONESTY: the conflict is WITNESSED and NAMES the fighting pair ─
  const witnesses = oc.witnesses ?? [];
  t.ok("the conflict produces a witness", witnesses.length > 0, {
    dim: "honesty",
    detail: `${witnesses.length} witness(es)`,
  });
  const w = witnesses[0];
  const witnessIds = Array.isArray(w?.constraints) ? w.constraints.map((c) => c.id) : [];
  const planted = d.over?.planted ?? [];
  t.ok(
    "the witness NAMES the constraints that cannot hold together",
    witnessIds.length >= 2 && planted.every((id) => witnessIds.includes(id)),
    { dim: "honesty", detail: `witness=${JSON.stringify(witnessIds)} planted=${JSON.stringify(planted)}` },
  );
  t.ok(
    "the witness is honestly flagged minimal (or honestly flagged NOT)",
    !!w && typeof w.minimal === "boolean",
    { dim: "honesty", detail: `minimal=${w?.minimal} kind=${w?.kind}` },
  );
}

export default {
  id: "11-sketch-certified-bore",
  title: "Certified constrained sketch → solve → TRUE cylindrical bore (#45)",
  dims: ["correctness", "soundness", "honesty", "performance"],
  budgetMs: 45000,
  oracle,
  async run(ctx, t) {
    const { c } = ctx;

    // ── The well-posed plate: 4 fixed corners + 4 endpoint-derived lines
    //    + a fully-dimensioned circular bore. ──────────────────────────
    const sk = await c.post("/api/csketch");
    const sid = sk.id;
    const P = async (x, y, fixed = false) => (await c.post(`/api/csketch/${sid}/point`, { x, y, fixed })).id;
    const L = async (start, end) => (await c.post(`/api/csketch/${sid}/line`, { start, end })).id;
    const constrain = (constraint_type, entities, priority = "Required") =>
      c.post(`/api/csketch/${sid}/constraint`, {
        id: randomUUID(),
        constraint_type,
        entities,
        priority,
        status: "Satisfied",
        name: null,
      });

    const bl = await P(0, 0, true);
    const br = await P(PLATE_W, 0, true);
    const tr = await P(PLATE_W, PLATE_H, true);
    const tl = await P(0, PLATE_H, true);
    await L(bl, br);
    await L(br, tr);
    await L(tr, tl);
    await L(tl, bl);

    // The bore: 3 free DOF (cx, cy, r), pinned by X / Y / Radius dims.
    const circle = (await c.post(`/api/csketch/${sid}/circle`, {
      cx: PLATE_W / 2,
      cy: PLATE_H / 2,
      radius: BORE_R,
    })).id;
    await constrain({ Dimensional: { XCoordinate: PLATE_W / 2 } }, [{ Circle: circle }]);
    await constrain({ Dimensional: { YCoordinate: PLATE_H / 2 } }, [{ Circle: circle }]);
    await constrain({ Dimensional: { Radius: BORE_R } }, [{ Circle: circle }]);

    const solve = await ctx.time("solve the constrained sketch", () =>
      c.post(`/api/csketch/${sid}/solve`, { options: null }),
    );
    const certify = await ctx.time("certify the sketch", () => c.post(`/api/csketch/${sid}/certify`, {}));
    const dof = await c.get(`/api/csketch/${sid}/dof`);
    const extrude = await ctx.time("extrude to a bored plate", () =>
      c.post(`/api/csketch/${sid}/extrude`, { distance: DEPTH, name: "plate" }),
    );
    const solid = await ctx.time("certify the extruded solid", async () =>
      c.perceive(await c.newestPartId()),
    );

    // ── The planted lie: an OVER-CONSTRAINED sketch. Two dimensional
    //    constraints pin the same span to different lengths. ───────────
    const sk2 = await c.post("/api/csketch");
    const bid = sk2.id;
    const q1 = (await c.post(`/api/csketch/${bid}/point`, { x: 0, y: 0, fixed: false })).id;
    const q2 = (await c.post(`/api/csketch/${bid}/point`, { x: 10, y: 0, fixed: false })).id;
    const line = (await c.post(`/api/csketch/${bid}/line`, { start: q1, end: q2 })).id;
    const distCid = randomUUID();
    const lenCid = randomUUID();
    await c.post(`/api/csketch/${bid}/constraint`, {
      id: distCid,
      constraint_type: { Dimensional: { Distance: 10 } },
      entities: [{ Point: q1 }, { Point: q2 }],
      priority: "Required",
      status: "Satisfied",
      name: null,
    });
    await c.post(`/api/csketch/${bid}/constraint`, {
      id: lenCid,
      constraint_type: { Dimensional: { Length: 20 } },
      entities: [{ Line: line }],
      priority: "Required",
      status: "Satisfied",
      name: null,
    });
    const overCertify = await c.post(`/api/csketch/${bid}/certify`, {});

    oracle(t, {
      good: { solve, certify, dof, extrude, solid },
      over: { certify: overCertify, planted: [distCid, lenCid] },
    });
  },
};
