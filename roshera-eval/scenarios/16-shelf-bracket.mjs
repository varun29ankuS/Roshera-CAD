/**
 * WALL-MOUNTED SHELF BRACKET — FDM PLA, envelope- and interface-constrained.
 *
 * Task prompt: the founder-authored task spec dated 2026-07-23 (Part 1 §B —
 * "Wall-mounted shelf bracket, FDM PLA, simulation-scored, checkpointed"). The
 * agent receives that brief and must design a single-piece PLA bracket that
 * supports 150 N applied 180 mm from the wall plane, mounts through two M6
 * bolts spaced 60 mm vertically, and fits a 220 × 160 × 60 mm envelope while
 * MINIMIZING printed mass.
 *
 * # What this scenario scores — and what it deliberately does NOT
 *
 * The kernel is a geometry kernel. It can certify a solid's soundness, measure
 * its bounding box, read its analytic feature dimensions, integrate its volume,
 * and count its parts — so those are scored here, exactly and only. The spec's
 * PHYSICS gates (von Mises ≤ 20 MPa, deflection ≤ 2.0 mm), its PRINTABILITY
 * gates (wall thickness ≥ 2× nozzle, overhang > 45° justification, support
 * volume), and its ORIENTATION rule (root bending tension in the layer plane)
 * are NOT things this kernel can compute today. They are NOT silently dropped
 * and NOT fake-scored: every one is declared in `unscored_criteria` below with
 * the reason it is out of scope. The scored subset is exactly the honestly
 * verifiable subset — that discipline is the product's whole point.
 *
 * # Why the oracle is a separate pure function
 *
 * `run` talks to a live backend; `oracle` does not. `test/oracle-16.mjs` feeds
 * the oracle an honest transcript and a set of single-mutation LIES and proves
 * it tells them apart, so the scenario is known to discriminate before it is
 * ever pointed at a server. A scenario that cannot be shown to catch a lie is
 * not evidence.
 *
 * # The honesty dimension
 *
 * A soundness verdict must be INTERNALLY CONSISTENT: a solid the kernel calls
 * `sound` may never simultaneously report open edges or a non-watertight shell.
 * The scored subset must also stay honest about itself — a run that quietly
 * reported one of the physics gates as "passed" would be inventing a verdict
 * the kernel cannot back. This scenario scores neither, and the dry validation
 * asserts the unscored set is declared, not scored.
 *
 * NOTE ON THE BUILD. `run` builds a deterministic REFERENCE bracket (a baseline
 * L — wall plate + top arm + the two frozen M6 bores) that honors the frozen
 * interfaces and envelope. It is checkpoint-1-grade geometry, not an optimized
 * submission; it exists so the scored oracle can be exercised end to end. Per
 * the corpus v2 direction, the scripted build is the seam an agent's own build
 * replaces — the identical oracle then scores the agent's part.
 */

import { steelMassKg } from "../lib/geom.mjs";

// ── Frozen interface + envelope (founder task spec 2026-07-23, Part 1 §B) ──
const ENVELOPE_MM = [220, 160, 60]; // hard bound; part must fit inside
const M6_RADIUS = 3; // Ø6 mounting bolts
const BOLT_SPACING_MM = 60; // vertical spacing, frozen
const BOLT_Z_LOW = 45;
const BOLT_Z_HIGH = 105; // 45 + 60
const PLATE_X = 4; // wall-plate mid-thickness (X): bolt bores centered here

// Printed-PLA density for the mass ranking metric: ~1.24 g/cm³ = 1.24e-3 g/mm³
// (documented material assumption; the spec fixes the ultimate-strength
// reference but not density, so the standard printed-PLA figure is cited).
const PLA_DENSITY_G_PER_MM3 = 1.24e-3;

/** Cluster this solid's cylindrical faces into distinct bores keyed by
 *  (rounded radius, rounded Y, rounded Z), tolerant of a bore being carried by
 *  more than one cylinder face after the boolean split. Returns one entry per
 *  physical hole: { radius, y, z, count }. */
function bores(features, { axisAbs = "x" } = {}) {
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
    const key = `${Math.round(r * 2) / 2}|${Math.round(o[1])}|${Math.round(o[2])}`;
    const g = groups.get(key) ?? { radius: r, y: o[1], z: o[2], count: 0 };
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
 * @param d  the transcript (see `run`, and `test/oracle-16.mjs` fixtures):
 *           { steps:[{name,per}], final, features, partCount }
 */
export function oracle(t, d) {
  const steps = Array.isArray(d.steps) ? d.steps : [];
  const final = d.final ?? {};

  // ── 1. SOUNDNESS at EVERY mutating step (fail on any unsound op) ──────
  t.ok("the build performed at least one mutating step", steps.length > 0, {
    dim: "soundness",
    detail: `${steps.length} step(s)`,
  });
  for (const s of steps) {
    t.sound(`step certifies sound: ${s.name}`, s.per);
  }

  // ── 2. The final part is watertight AND manifold ─────────────────────
  t.ok("final bracket is watertight", final.watertight === true, {
    dim: "soundness",
    detail: `watertight=${final.watertight}`,
  });
  t.ok("final bracket is manifold", final.manifold === true, {
    dim: "soundness",
    detail: `manifold=${final.manifold}`,
  });

  // ── 3. HONESTY: a 'sound' verdict is internally consistent ───────────
  //      (sound ⟹ watertight ∧ zero open edges — no lie behind the badge)
  t.ok(
    "the final soundness verdict is internally consistent (sound ⟹ watertight, zero open edges)",
    final.sound !== true || (final.open_edges === 0 && final.watertight !== false),
    { dim: "honesty", detail: `sound=${final.sound} watertight=${final.watertight} open_edges=${final.open_edges}` },
  );

  // ── 4. Envelope compliance: fits inside 220 × 160 × 60 mm ────────────
  t.ok(
    "the bracket fits within the 220 × 160 × 60 mm envelope",
    fitsEnvelope(final.dims, ENVELOPE_MM),
    { dim: "correctness", detail: `dims=${JSON.stringify(final.dims)} envelope=${JSON.stringify(ENVELOPE_MM)}` },
  );

  // ── 5. The two M6 mounting bores are present, at spec Ø and spacing ──
  const feats = Array.isArray(d.features) ? d.features : [];
  const m6 = bores(feats, { axisAbs: "x" }).filter((b) => Math.abs(b.radius - M6_RADIUS) < 0.25);
  t.eq("exactly two M6 mounting bores present", m6.length, 2, { dim: "correctness" });
  t.ok(
    "both mounting bores measure Ø6.0 (M6 clearance) off the analytic cylinder",
    m6.length === 2 && m6.every((b) => Math.abs(b.radius * 2 - 6) < 0.5),
    { dim: "correctness", detail: `diameters=${m6.map((b) => (b.radius * 2).toFixed(2)).join(",")}` },
  );
  const zs = m6.map((b) => b.z).sort((a, b) => a - b);
  t.ok(
    "the mounting bores sit at the frozen positions (60 mm vertical spacing, on the wall plate)",
    m6.length === 2 &&
      m6.every((b) => Math.abs(b.y) < 1.0) &&
      Math.abs(zs[0] - BOLT_Z_LOW) < 1.0 &&
      Math.abs(zs[1] - BOLT_Z_HIGH) < 1.0 &&
      Math.abs(zs[1] - zs[0] - BOLT_SPACING_MM) < 0.5,
    { dim: "correctness", detail: `z=${JSON.stringify(zs)} spacing=${(zs[1] - zs[0]).toFixed(2)}` },
  );

  // ── 6. Single-piece topology: exactly one solid remains ──────────────
  t.eq("the bracket is a single piece (one solid part)", d.partCount, 1, { dim: "correctness" });

  // ── 7. Mass — the PRIMARY ranking metric (recorded, not a pass/fail) ──
  //      Printed PLA mass from the kernel's ground-truth volume × documented
  //      density. Reported so the leaderboard can rank submissions; the value
  //      must be finite and positive (a zero/NaN volume is a broken build).
  const volMm3 = Number(final.volume);
  const massG = volMm3 * PLA_DENSITY_G_PER_MM3;
  t.ok(
    "printed mass is recorded as the ranking metric (finite, positive, from kernel volume × documented PLA density)",
    Number.isFinite(massG) && massG > 0,
    { dim: "correctness", detail: `volume=${Number.isFinite(volMm3) ? volMm3.toFixed(1) : final.volume} mm³ → ${Number.isFinite(massG) ? massG.toFixed(1) : "?"} g PLA (≈ ${steelMassKg(volMm3).toFixed(4)} kg if steel, for cross-reference)` },
  );
}

export default {
  id: "16-shelf-bracket",
  title: "Wall-mounted shelf bracket — soundness, envelope, frozen M6 interface, mass (PLA)",
  dims: ["correctness", "soundness", "honesty", "performance"],
  budgetMs: 90000,
  oracle,

  /** The founder-authored task brief handed to the agent (Part 1 §B of the SME
   *  vetting task spec, 2026-07-23). Verbatim so an agent build can be swapped
   *  in for the scripted reference and scored by the identical oracle. */
  task_prompt:
    "Design a single-piece, FDM-printed PLA wall bracket that supports a shelf " +
    "carrying 150 N vertical downward load applied 180 mm from the wall mounting " +
    "plane. The bracket mounts to a rigid wall through two M6 bolts spaced 60 mm " +
    "vertically on the wall plate; bolt positions are fixed. Service environment " +
    "is indoor, dry, 15–35 °C, out of direct sunlight. The bracket must fit within " +
    "a 220 mm × 160 mm × 60 mm envelope. The objective is to MINIMIZE printed mass " +
    "subject to every hard constraint. Material: PLA, linear-elastic; design " +
    "allowable = 0.4 × documented printed-PLA ultimate (≈ 20 MPa at the ~50 MPa " +
    "reference), source stated. Orientation rule: root bending tension must lie in " +
    "the layer plane — no primary load path may cross layer interfaces in peel. " +
    "(Founder-authored task spec 2026-07-23, Part 1 §B.)",

  /** Criteria the SPEC contains that THIS kernel cannot verify today. Declared,
   *  never silently dropped and never fake-scored. Each carries the reason it is
   *  out of the scored subset — this list doubles as the scoring-bridge backlog. */
  unscored_criteria: [
    { criterion: "max von Mises stress ≤ 20 MPa (0.4 × printed-PLA ultimate)", spec_ref: "Part 1 §C", reason: "pending external physics (FEA static) scoring bridge — the kernel does not run structural analysis" },
    { criterion: "max deflection at the load point ≤ 2.0 mm", spec_ref: "Part 1 §C", reason: "pending external physics (FEA static) scoring bridge" },
    { criterion: "print-orientation rule — root bending tension in the layer plane, no primary load path in peel", spec_ref: "Part 1 §B", reason: "pending printability/anisotropy analyzer — layer direction vs load path is not modeled by the kernel" },
    { criterion: "wall thickness ≥ 2× nozzle (≥ 0.8 mm at 0.4 mm nozzle) everywhere", spec_ref: "Part 1 §C", reason: "pending printability analyzer — minimum-wall / thin-region extraction not implemented" },
    { criterion: "overhangs > 45° only with stated justification; support volume reported", spec_ref: "Part 1 §C", reason: "pending printability/slicer bridge — overhang and support estimation not implemented" },
    { criterion: "static simulation runtime ≤ 10 min on a laptop-class machine", spec_ref: "Part 1 §C", reason: "pending external physics scoring bridge — no simulation is run by the kernel" },
    { criterion: "fillets/chamfers present as load-dissipation features (no single point of failure)", spec_ref: "Part 1 §B/F", reason: "pending stress-concentration analyzer — geometric presence is detectable but its load-dissipation role requires FEA to score" },
  ],

  async run(ctx, t) {
    const { c } = ctx;
    const steps = [];
    const perceiveNewest = async (name) => {
      const per = await c.perceive(await c.newestPartId());
      steps.push({ name, per });
      return per;
    };

    // ── Wall plate: X∈[0,8] (thickness), Y∈[-20,20] (width 40), Z∈[0,150] ──
    await ctx.time("wall plate", () =>
      c.post("/api/geometry/box", {
        center: [PLATE_X, 0, 75], u_axis: [1, 0, 0], v_axis: [0, 1, 0],
        width: 8, depth: 40, height: 150, name: "wall_plate", fast: true,
      }),
    );
    await perceiveNewest("wall plate");

    // ── Top arm: reaches out over the 180 mm load point (X∈[0,190]) ──────
    await ctx.time("top arm", () =>
      c.post("/api/geometry/box", {
        center: [95, 0, 142.5], u_axis: [1, 0, 0], v_axis: [0, 1, 0],
        width: 190, depth: 40, height: 15, name: "top_arm", fast: true,
      }),
    );
    const armPer = await perceiveNewest("top arm");

    // ── Union plate ∪ arm → the single-piece L bracket ──────────────────
    const plateUuid = await c.uuidForPart(steps[0].per.solid_id);
    const armUuid = await c.uuidForPart(armPer.solid_id);
    await ctx.time("union plate ∪ arm", () =>
      c.post("/api/geometry/boolean", {
        operation: "union", object_a: plateUuid, object_b: armUuid, fast: true,
      }),
    );
    let uuid = await c.uuidForPart(await c.newestPartId());
    await perceiveNewest("union → L bracket");

    // ── Drill the two frozen M6 bores through the wall plate (axis = +X) ──
    const holes = [
      { center: [PLATE_X, 0, BOLT_Z_LOW], z: BOLT_Z_LOW },
      { center: [PLATE_X, 0, BOLT_Z_HIGH], z: BOLT_Z_HIGH },
    ];
    for (const h of holes) {
      const bore = await c.post("/api/geometry/cylinder", {
        center: h.center, axis: [1, 0, 0], radius: M6_RADIUS, height: 10,
        name: `m6_bore_z${h.z}`, fast: true,
      });
      await ctx.time(`drill M6 bore @ z=${h.z}`, () =>
        c.post("/api/geometry/boolean", {
          operation: "difference", object_a: uuid, object_b: bore.object.id, fast: true,
        }),
      );
      uuid = await c.uuidForPart(await c.newestPartId());
      await perceiveNewest(`M6 bore @ z=${h.z}`);
    }

    // ── Read the final part's structural + feature facts for the oracle ──
    const id = await c.newestPartId();
    const final = await ctx.time("certify final bracket", () => c.perceive(id));
    const features = (await c.get(`/api/agent/parts/${id}/features`)).features ?? [];
    const partCount = (await c.listParts()).length;

    oracle(t, { steps, final, features, partCount });
  },
};
