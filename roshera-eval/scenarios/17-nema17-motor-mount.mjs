/**
 * VIBRATION-AWARE NEMA 17 MOTOR MOUNT — FDM PETG, belt-driven printer axis.
 *
 * Task prompt: the founder-authored task spec dated 2026-07-23 (Part 2 —
 * "Vibration-Aware NEMA 17 Motor Mount for a Belt-Driven 3D-Printer Axis
 * (PETG, FDM)"). The agent receives that brief and must design a single-piece
 * PETG mount that carries a NEMA 17 motor, reacts belt tension + torque + weight,
 * bolts to 2020 aluminium extrusion, fits a 90 × 70 × 60 mm envelope, and
 * MINIMIZES printed mass while keeping the first natural frequency clear of the
 * printer's excitation band.
 *
 * # What this scenario scores — and what it deliberately does NOT
 *
 * The kernel certifies soundness, measures the bounding box, reads analytic
 * feature dimensions, integrates volume, and counts parts — so those are scored
 * here, exactly and only. The spec's headline requirement — first natural
 * frequency f₁ ≥ 120 Hz with the 350 g motor attached — is a MODAL analysis the
 * kernel does not perform; so are the static-stress, deflection, wall-thickness,
 * overhang/support, orientation, and thermal gates. None is silently dropped and
 * none is fake-scored: each is declared in `unscored_criteria` below with the
 * reason it is out of scope. The scored subset is exactly the honestly
 * verifiable subset. Scoring the frozen INTERFACES (the 4×M3 + 22 mm boss motor
 * register and the 2×M5 frame provision) is precisely what a geometry kernel CAN
 * ground truth — and getting the interface wrong is the failure that makes a
 * mount unbuildable regardless of how well it would have simulated.
 *
 * # Why the oracle is a separate pure function
 *
 * `run` talks to a live backend; `oracle` does not. `test/oracle-17.mjs` feeds
 * the oracle an honest transcript and single-mutation LIES and proves it tells
 * them apart, so the scenario is known to discriminate before it is ever pointed
 * at a server. A scenario that cannot be shown to catch a lie is not evidence.
 *
 * # The honesty dimension
 *
 * A soundness verdict must be INTERNALLY CONSISTENT (sound ⟹ watertight ∧ zero
 * open edges). And the scored subset stays honest about itself: a run that
 * reported f₁ or a stress gate as "passed" would be inventing a verdict the
 * kernel cannot back. This scenario scores neither; the dry validation asserts
 * the unscored set is declared, not scored.
 *
 * NOTE ON THE BUILD. `run` builds a deterministic REFERENCE mount (baseline
 * L-plate + pilot boss + frozen bores) honoring the frozen interfaces and
 * envelope — checkpoint-1-grade geometry so the scored oracle can be exercised
 * end to end. Per the corpus v2 direction, the scripted build is the seam an
 * agent's own build replaces; the identical oracle then scores the agent's part.
 */

// ── Frozen interface + envelope (founder task spec 2026-07-23, Part 2 §D) ──
const ENVELOPE_MM = [90, 70, 60]; // hard bound; part must fit inside
const M3_RADIUS = 1.5; // Ø3 motor bolts (NEMA 17)
const M3_SQUARE_MM = 31; // 4×M3 on a 31 mm square
const BOSS_RADIUS = 11; // Ø22 pilot-boss register bore
const M5_RADIUS = 2.5; // Ø5 frame (2020 T-nut) bolts
const FACE_X = 4; // motor-plate mid-thickness (X); motor bores centered here
const SQ = M3_SQUARE_MM / 2; // 15.5 — half the bolt square
const SQ_CZ = 31; // motor pattern center Z (plate center)
const FOOT_Z = 4; // frame-foot mid-thickness (Z); frame bores centered here

// Printed-PETG density for the mass ranking metric: ~1.27 g/cm³ = 1.27e-3 g/mm³
// (documented material assumption; the spec fixes the ultimate-strength
// reference but not density, so the standard printed-PETG figure is cited).
const PETG_DENSITY_G_PER_MM3 = 1.27e-3;

/** Cluster cylindrical faces into distinct bores keyed by (rounded radius,
 *  rounded position across the bore axis), tolerant of a bore carried by more
 *  than one cylinder face after the boolean split. `axisAbs` selects the bore
 *  axis; positions are keyed by the two coordinates orthogonal to it. */
function bores(features, axisAbs) {
  const isAxis = (a) => {
    if (!Array.isArray(a)) return false;
    const [ax, ay, az] = a.map((v) => Math.abs(v));
    if (axisAbs === "x") return ax > 0.9 && ay < 0.1 && az < 0.1;
    if (axisAbs === "z") return az > 0.9 && ax < 0.1 && ay < 0.1;
    return false;
  };
  const groups = new Map();
  for (const f of features) {
    if (f?.surface_kind !== "cylinder") continue;
    if (!isAxis(f.axis)) continue;
    const r = f.radius ?? (f.diameter != null ? f.diameter / 2 : null);
    if (r == null) continue;
    const o = f.origin ?? [null, null, null];
    // Key by the two coordinates orthogonal to the bore axis.
    const p = axisAbs === "x" ? [o[1], o[2]] : [o[0], o[1]];
    const key = `${Math.round(r * 2) / 2}|${Math.round(p[0])}|${Math.round(p[1])}`;
    const g = groups.get(key) ?? { radius: r, u: p[0], v: p[1], count: 0 };
    g.count += 1;
    groups.set(key, g);
  }
  return [...groups.values()];
}

/** The extents of the part, sorted descending, fit inside the envelope sorted
 *  descending — an orientation-independent "does it fit in the box" test. */
function fitsEnvelope(dims, envelope) {
  if (!Array.isArray(dims) || dims.length !== 3) return false;
  const d = [...dims].map(Math.abs).sort((a, b) => b - a);
  const e = [...envelope].sort((a, b) => b - a);
  return d.every((x, i) => x <= e[i] + 1e-6);
}

/**
 * The PURE scoring oracle. No I/O, no client — so it can be validated dry.
 *
 * @param t  the harness `Checks` collector
 * @param d  the transcript: { steps:[{name,per}], final, features, partCount }
 */
export function oracle(t, d) {
  const steps = Array.isArray(d.steps) ? d.steps : [];
  const final = d.final ?? {};

  // ── 1. SOUNDNESS at EVERY mutating step (fail on any unsound op) ──────
  t.ok("the build performed at least one mutating step", steps.length > 0, {
    dim: "soundness",
    detail: `${steps.length} step(s)`,
  });
  for (const s of steps) t.sound(`step certifies sound: ${s.name}`, s.per);

  // ── 2. The final part is watertight AND manifold ─────────────────────
  t.ok("final mount is watertight", final.watertight === true, {
    dim: "soundness", detail: `watertight=${final.watertight}`,
  });
  t.ok("final mount is manifold", final.manifold === true, {
    dim: "soundness", detail: `manifold=${final.manifold}`,
  });

  // ── 3. HONESTY: a 'sound' verdict is internally consistent ───────────
  t.ok(
    "the final soundness verdict is internally consistent (sound ⟹ watertight, zero open edges)",
    final.sound !== true || (final.open_edges === 0 && final.watertight !== false),
    { dim: "honesty", detail: `sound=${final.sound} watertight=${final.watertight} open_edges=${final.open_edges}` },
  );

  // ── 4. Envelope compliance: fits inside 90 × 70 × 60 mm ──────────────
  t.ok(
    "the mount fits within the 90 × 70 × 60 mm envelope",
    fitsEnvelope(final.dims, ENVELOPE_MM),
    { dim: "correctness", detail: `dims=${JSON.stringify(final.dims)} envelope=${JSON.stringify(ENVELOPE_MM)}` },
  );

  const feats = Array.isArray(d.features) ? d.features : [];

  // ── 5. NEMA 17 motor register: 4×M3 on a 31 mm square (axis +X) ──────
  const m3 = bores(feats, "x").filter((b) => Math.abs(b.radius - M3_RADIUS) < 0.25);
  t.eq("four M3 motor bolts present", m3.length, 4, { dim: "correctness" });
  t.ok(
    "the four M3 bolts measure Ø3.0 off the analytic cylinder",
    m3.length === 4 && m3.every((b) => Math.abs(b.radius * 2 - 3) < 0.4),
    { dim: "correctness", detail: `diameters=${m3.map((b) => (b.radius * 2).toFixed(2)).join(",")}` },
  );
  // The 4 centers form a 31 mm square: |Y|=15.5 and Z∈{15.5,46.5} (spacing 31).
  const ys = [...new Set(m3.map((b) => Math.round(b.u)))].sort((a, b) => a - b);
  const zs = [...new Set(m3.map((b) => Math.round(b.v)))].sort((a, b) => a - b);
  t.ok(
    "the M3 pattern is a 31 mm square (adjacent bolt spacing 31 mm)",
    m3.length === 4 &&
      ys.length === 2 && zs.length === 2 &&
      Math.abs(ys[1] - ys[0] - M3_SQUARE_MM) < 0.6 &&
      Math.abs(zs[1] - zs[0] - M3_SQUARE_MM) < 0.6 &&
      m3.every((b) => Math.abs(Math.abs(b.u) - SQ) < 0.6),
    { dim: "correctness", detail: `ys=${JSON.stringify(ys)} zs=${JSON.stringify(zs)}` },
  );

  // ── 6. Pilot-boss register bore: a single Ø22 (axis +X), central ─────
  const boss = bores(feats, "x").filter((b) => Math.abs(b.radius - BOSS_RADIUS) < 0.6);
  t.eq("one Ø22 pilot-boss register bore present", boss.length, 1, { dim: "correctness" });
  t.ok(
    "the pilot boss measures Ø22 and is centered on the motor pattern",
    boss.length === 1 &&
      Math.abs(boss[0].radius * 2 - 22) < 1.0 &&
      Math.abs(boss[0].u) < 1.5 && Math.abs(boss[0].v - SQ_CZ) < 1.5,
    { dim: "correctness", detail: `Ø=${boss[0] ? (boss[0].radius * 2).toFixed(2) : "?"} at (y=${boss[0]?.u},z=${boss[0]?.v})` },
  );

  // ── 7. Frame provision: 2×M5 (2020 T-nut) bores (axis +Z) ────────────
  const m5 = bores(feats, "z").filter((b) => Math.abs(b.radius - M5_RADIUS) < 0.25);
  t.eq("two M5 frame-mount bores present", m5.length, 2, { dim: "correctness" });
  t.ok(
    "the frame bores measure Ø5.0 off the analytic cylinder",
    m5.length === 2 && m5.every((b) => Math.abs(b.radius * 2 - 5) < 0.5),
    { dim: "correctness", detail: `diameters=${m5.map((b) => (b.radius * 2).toFixed(2)).join(",")}` },
  );

  // ── 8. Single-piece topology: exactly one solid remains ──────────────
  t.eq("the mount is a single piece (one solid part)", d.partCount, 1, { dim: "correctness" });

  // ── 9. Mass — the PRIMARY ranking metric (recorded, not a pass/fail) ──
  const volMm3 = Number(final.volume);
  const massG = volMm3 * PETG_DENSITY_G_PER_MM3;
  t.ok(
    "printed mass is recorded as the ranking metric (finite, positive, from kernel volume × documented PETG density)",
    Number.isFinite(massG) && massG > 0,
    { dim: "correctness", detail: `volume=${Number.isFinite(volMm3) ? volMm3.toFixed(1) : final.volume} mm³ → ${Number.isFinite(massG) ? massG.toFixed(1) : "?"} g PETG` },
  );
}

export default {
  id: "17-nema17-motor-mount",
  title: "NEMA 17 motor mount — soundness, envelope, frozen M3+boss / M5 interface, mass (PETG)",
  dims: ["correctness", "soundness", "honesty", "performance"],
  budgetMs: 90000,
  oracle,

  /** The founder-authored task brief handed to the agent (Part 2 of the SME
   *  vetting task spec, 2026-07-23). */
  task_prompt:
    "Design a single-piece, FDM-printed PETG mount that carries a NEMA 17 stepper " +
    "motor (350 g) driving a belt-driven printer axis, attached to 2020 aluminium " +
    "extrusion. It reacts three loads at once: GT2 belt tension (20 N nominal × 2.0 " +
    "dynamic = 40 N lateral at the pulley plane), the motor's torque reaction " +
    "(0.45 N·m through the bolt pattern), and motor weight (3.5 N). Motor interface " +
    "is frozen: NEMA 17 — 4 × M3 on a 31 mm square with a 22 mm pilot-boss bore for " +
    "register. Frame interface: 2020 extrusion, 2 × M5 T-nuts; tool access must " +
    "exist in the assembled orientation. Envelope: 90 × 70 × 60 mm. Material: PETG, " +
    "allowable = 0.4 × documented printed-PETG ultimate (≈ 20 MPa). HARD modal " +
    "requirement: first natural frequency with the 350 g motor attached ≥ 120 Hz. " +
    "Deflection ≤ 0.3 mm at the pulley plane under the 40 N dynamic belt load. " +
    "Objective: MINIMIZE printed mass while walking f₁ above the 120 Hz floor. " +
    "(Founder-authored task spec 2026-07-23, Part 2.)",

  /** Criteria the SPEC contains that THIS kernel cannot verify today. Declared,
   *  never silently dropped and never fake-scored. Doubles as the scoring backlog. */
  unscored_criteria: [
    { criterion: "first natural frequency (with 350 g motor attached) ≥ 120 Hz", spec_ref: "Part 2 §C/§D.8", reason: "pending external physics (modal / frequency-extraction) scoring bridge — the kernel performs no modal analysis; a lumped-mass eigen-solve is out of scope" },
    { criterion: "max von Mises stress ≤ 20 MPa (0.4 × printed-PETG ultimate) under the combined static load", spec_ref: "Part 2 §C/§D.2", reason: "pending external physics (FEA static) scoring bridge" },
    { criterion: "max deflection at the pulley plane ≤ 0.3 mm under the 40 N dynamic belt load", spec_ref: "Part 2 §C/§D.9", reason: "pending external physics (FEA static) scoring bridge" },
    { criterion: "walls ≥ 1.6 mm (4 perimeters) everywhere", spec_ref: "Part 2 §D.3", reason: "pending printability analyzer — minimum-wall / thin-region extraction not implemented" },
    { criterion: "unsupported overhangs > 45° require justification; support volume reported", spec_ref: "Part 2 §D.3", reason: "pending printability/slicer bridge — overhang and support estimation not implemented" },
    { criterion: "print-orientation rule — no primary load path (belt/torque reaction) crossing layer interfaces in peel", spec_ref: "Part 2 §D.4", reason: "pending printability/anisotropy analyzer — layer direction vs load path is not modeled" },
    { criterion: "thermal: motor face at up to 60 °C sustained; contact geometry stated", spec_ref: "Part 2 §D.10", reason: "pending thermal-structural scoring bridge — no thermal model in the kernel" },
    { criterion: "assembly access — T-nut and M3 tool paths reachable in the assembled orientation", spec_ref: "Part 2 §D.6", reason: "pending assembly/reachability analyzer — tool-access swept volume not modeled" },
    { criterion: "static and modal simulation runtime ≤ 10 min on a laptop-class machine", spec_ref: "Part 2 §D.12", reason: "pending external physics scoring bridge — no simulation is run by the kernel" },
  ],

  async run(ctx, t) {
    const { c } = ctx;
    const steps = [];
    const perceiveNewest = async (name) => {
      const per = await c.perceive(await c.newestPartId());
      steps.push({ name, per });
      return per;
    };

    // ── Motor face plate: vertical, X∈[0,8], Y∈[-25,25], Z∈[6,56] ────────
    await ctx.time("motor face plate", () =>
      c.post("/api/geometry/box", {
        center: [FACE_X, 0, SQ_CZ], u_axis: [1, 0, 0], v_axis: [0, 1, 0],
        width: 8, depth: 50, height: 50, name: "motor_plate", fast: true,
      }),
    );
    const platePer = await perceiveNewest("motor face plate");

    // ── Frame foot: horizontal 2020 mounting base, X∈[0,60], Z∈[0,8] ─────
    await ctx.time("frame foot", () =>
      c.post("/api/geometry/box", {
        center: [30, 0, FOOT_Z], u_axis: [1, 0, 0], v_axis: [0, 1, 0],
        width: 60, depth: 50, height: 8, name: "frame_foot", fast: true,
      }),
    );
    const footPer = await perceiveNewest("frame foot");

    // ── Union plate ∪ foot → the single-piece L mount ───────────────────
    const plateUuid = await c.uuidForPart(platePer.solid_id);
    const footUuid = await c.uuidForPart(footPer.solid_id);
    await ctx.time("union plate ∪ foot", () =>
      c.post("/api/geometry/boolean", {
        operation: "union", object_a: plateUuid, object_b: footUuid, fast: true,
      }),
    );
    let uuid = await c.uuidForPart(await c.newestPartId());
    await perceiveNewest("union → L mount");

    // ── Ø22 pilot-boss register bore (axis +X), central on the pattern ──
    const boss = await c.post("/api/geometry/cylinder", {
      center: [FACE_X, 0, SQ_CZ], axis: [1, 0, 0], radius: BOSS_RADIUS, height: 10,
      name: "pilot_boss", fast: true,
    });
    await ctx.time("bore Ø22 pilot boss", () =>
      c.post("/api/geometry/boolean", {
        operation: "difference", object_a: uuid, object_b: boss.object.id, fast: true,
      }),
    );
    uuid = await c.uuidForPart(await c.newestPartId());
    await perceiveNewest("Ø22 pilot boss");

    // ── 4×M3 on the 31 mm square (axis +X) ──────────────────────────────
    const m3 = [
      [FACE_X, +SQ, SQ_CZ - SQ], [FACE_X, -SQ, SQ_CZ - SQ],
      [FACE_X, +SQ, SQ_CZ + SQ], [FACE_X, -SQ, SQ_CZ + SQ],
    ];
    for (let k = 0; k < m3.length; k++) {
      const bore = await c.post("/api/geometry/cylinder", {
        center: m3[k], axis: [1, 0, 0], radius: M3_RADIUS, height: 10,
        name: `m3_${k}`, fast: true,
      });
      await ctx.time(`drill M3 #${k}`, () =>
        c.post("/api/geometry/boolean", {
          operation: "difference", object_a: uuid, object_b: bore.object.id, fast: true,
        }),
      );
      uuid = await c.uuidForPart(await c.newestPartId());
      await perceiveNewest(`M3 bolt #${k}`);
    }

    // ── 2×M5 frame provision (axis +Z) through the foot ─────────────────
    const m5 = [[15, 0, FOOT_Z], [45, 0, FOOT_Z]];
    for (let k = 0; k < m5.length; k++) {
      const bore = await c.post("/api/geometry/cylinder", {
        center: m5[k], axis: [0, 0, 1], radius: M5_RADIUS, height: 12,
        name: `m5_${k}`, fast: true,
      });
      await ctx.time(`drill M5 #${k}`, () =>
        c.post("/api/geometry/boolean", {
          operation: "difference", object_a: uuid, object_b: bore.object.id, fast: true,
        }),
      );
      uuid = await c.uuidForPart(await c.newestPartId());
      await perceiveNewest(`M5 frame bolt #${k}`);
    }

    // ── Read the final part's structural + feature facts for the oracle ──
    const id = await c.newestPartId();
    const final = await ctx.time("certify final mount", () => c.perceive(id));
    const features = (await c.get(`/api/agent/parts/${id}/features`)).features ?? [];
    const partCount = (await c.listParts()).length;

    oracle(t, { steps, final, features, partCount });
  },
};
