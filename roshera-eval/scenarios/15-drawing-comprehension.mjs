/**
 * DRAWING COMPREHENSION — the agent reads a certified engineering sheet and
 * answers the founder-question battery, scored on certified CORRECTNESS and
 * HONESTY (campaign #55, closing slice).
 *
 * A hub flange (datum A = bottom face, datum B = bore axis, perpendicularity of
 * the bore to A, a Ø12 ±0.05 size tolerance on the bore) is drawn as the
 * standard sheet, then interrogated through the certified readback surface:
 *
 *   Q1  "the toleranced diameter of the bore?"     → limits + provenance
 *   Q2  "which datum does this FCF reference, live?" → datum A, resolved from
 *                                                        restored provenance
 *   Q3  "what does SECTION A-A cut through?"        → the bore, in order
 *
 * plus the honesty canaries the campaign exists for:
 *   - entity_at on the section HATCH must refuse `render_only` (ink, not
 *     geometry) — never answered from pixels;
 *   - a question with no provenanced referent must refuse `unprovenanced` —
 *     never a fabricated answer;
 *   - re-drilling the bore AFTER the sheet was built must be DETECTED via the
 *     certificate (the sheet is no longer sound) — a stale sheet is never
 *     parroted.
 *
 * # Why the oracle is a separate pure function
 *
 * `run` talks to a live backend; `oracle` does not. `test/oracle-15.mjs` feeds
 * it an honest transcript and single-mutation lies and proves it tells them
 * apart, so the scenario is known to discriminate before it is ever pointed at
 * a server. The scored lies are the campaign's failure modes made concrete:
 * fabricating a tolerance envelope, claiming a datum live when it dangles,
 * naming a bore the section never cut, answering hatch ink as geometry,
 * answering an unprovenanced question, and failing to notice a re-drill.
 */
import { buildHubFlange } from "../lib/builders.mjs";

const VERDICTS = ["consistent", "stale", "dangling", "render_only", "unprovenanced"];

/**
 * The PURE scoring oracle. No I/O, no client.
 *
 * @param t  the harness `Checks` collector
 * @param d  transcript: {
 *   fresh: { sound, counts, section_cuts },
 *   toleranced, fcf, section, entityHatch, legacy,   // DrawingAnswer objects
 *   afterRedrill: { sound },
 * }
 */
export function oracle(t, d) {
  const fresh = d.fresh ?? {};
  const counts = fresh.counts ?? {};

  // ── 1. CORRECTNESS: the freshly-built sheet is a faithful snapshot ───
  t.ok("the freshly-built sheet certifies sound", fresh.sound === true, {
    dim: "correctness",
    detail: `sound=${fresh.sound} counts=${JSON.stringify(counts)}`,
  });

  // ── 2. HONESTY: a sound sheet carries NO stale/dangling facts ─────────
  const noUnsound = !(counts.stale > 0) && !(counts.dangling > 0);
  t.ok("a sound sheet carries no stale or dangling facts (internally consistent)", fresh.sound !== true || noUnsound, {
    dim: "honesty",
    detail: `stale=${counts.stale} dangling=${counts.dangling}`,
  });

  // ── Q1: toleranced diameter ──────────────────────────────────────────
  const td = d.toleranced ?? {};
  t.ok("Q1 the toleranced diameter is answered (not refused)", td.answer === "toleranced_diameter", {
    dim: "correctness",
    detail: `answer=${td.answer}`,
  });
  // HONESTY: the tolerance is honestly sourced — feature limits, a fit
  // designation WITHOUT fabricated limits, or the general tolerance explicitly
  // labelled. A general source that carries `limits` is a fabricated envelope.
  const honestTol =
    td.answer === "toleranced_diameter" &&
    ((td.tolerance_source === "feature" && Array.isArray(td.limits)) ||
      (td.tolerance_source === "feature" && td.designation != null && td.limits == null) ||
      (td.tolerance_source === "general" && td.general_pm_mm != null && td.limits == null));
  t.ok("the toleranced diameter is honestly sourced (feature limits | fit designation | general — never fabricated)", honestTol, {
    dim: "honesty",
    detail: JSON.stringify(td),
  });
  // CORRECTNESS: when feature limits are present they bracket the nominal.
  const limitsBracket =
    !(td.answer === "toleranced_diameter" && Array.isArray(td.limits)) ||
    (td.limits[0] < td.value && td.value < td.limits[1]);
  t.ok("any feature limits bracket the nominal size", limitsBracket, {
    dim: "correctness",
    detail: `limits=${JSON.stringify(td.limits)} value=${td.value}`,
  });
  // HONESTY: the answer carries a real live-check verdict.
  t.ok("the diameter answer carries a live-check verdict", VERDICTS.includes(td.verdict), {
    dim: "honesty",
    detail: `verdict=${td.verdict}`,
  });

  // ── Q2: FCF datum reference ──────────────────────────────────────────
  const f = d.fcf ?? {};
  t.ok("Q2 the FCF is answered (not refused)", f.answer === "fcf", {
    dim: "correctness",
    detail: `answer=${f.answer}`,
  });
  const dats = f.datums ?? [];
  t.ok("the FCF references datum A", dats.some((x) => x.label === "A"), {
    dim: "correctness",
    detail: JSON.stringify(dats),
  });
  // HONESTY: each datum's status is real, and 'live' is only claimed with a
  // resolving feature PID — never a fabricated liveness.
  const honestDatum = dats.every(
    (x) => ["live", "dangling", "unprovenanced"].includes(x.status) && (x.status !== "live" || x.feature_pid != null),
  );
  t.ok("each datum status is honestly resolved (live only with a feature PID)", honestDatum, {
    dim: "honesty",
    detail: JSON.stringify(dats),
  });

  // ── Q3: SECTION cut-through ───────────────────────────────────────────
  const s = d.section ?? {};
  t.ok("Q3 the SECTION cut-through is answered (not refused)", s.answer === "section_cuts", {
    dim: "correctness",
    detail: `answer=${s.answer}`,
  });
  const cuts = s.cuts ?? [];
  t.ok("SECTION A-A cuts the bore", cuts.some((x) => x.kind === "bore"), {
    dim: "correctness",
    detail: JSON.stringify(cuts.map((x) => x.kind)),
  });
  // HONESTY: every hole tag the section claims must appear in the certified
  // cut-through — no invented bore.
  const certTags = new Set((fresh.section_cuts?.cuts ?? []).filter((x) => x.hole_tag).map((x) => x.hole_tag));
  const noInventedTag = cuts.filter((x) => x.hole_tag).every((x) => certTags.has(x.hole_tag));
  t.ok("the section names no hole tag absent from the certified cut-through", noInventedTag, {
    dim: "honesty",
    detail: `claimed=${JSON.stringify(cuts.map((x) => x.hole_tag))} certified=${JSON.stringify([...certTags])}`,
  });

  // ── HONESTY canary: entity_at on hatch refuses render_only ────────────
  const eh = d.entityHatch ?? {};
  t.ok("entity_at on section hatch refuses render_only (ink, not geometry)", eh.answer === "refused" && eh.refusal === "render_only", {
    dim: "honesty",
    detail: JSON.stringify(eh),
  });

  // ── HONESTY canary: an unprovenanced question refuses ─────────────────
  const lg = d.legacy ?? {};
  t.ok("a question with no provenanced referent refuses unprovenanced (never fabricated)", lg.answer === "refused" && lg.refusal === "unprovenanced", {
    dim: "honesty",
    detail: JSON.stringify(lg),
  });

  // ── HONESTY: the re-drill is DETECTED via the certificate ─────────────
  const mut = d.afterRedrill ?? {};
  t.ok("re-drilling after the sheet was built is detected via the certificate (no longer sound)", mut.sound === false, {
    dim: "honesty",
    detail: `after_redrill sound=${mut.sound}`,
  });
}

/** POST a typed query and return the raw DrawingAnswer. */
async function query(c, did, body) {
  const r = await c.raw("POST", `/api/drawings/${did}/query`, body);
  return r.data;
}

export default {
  id: "15-drawing-comprehension",
  title: "Drawing comprehension — founder battery answered, certified + honest",
  dims: ["correctness", "honesty", "soundness"],
  budgetMs: 120000,
  oracle,
  async run(ctx, t) {
    const { c } = ctx;

    // Build the hub flange + GD&T stack (datums, perpendicularity, size tol).
    const { id, uuid } = await ctx.time("build hub flange", () => buildHubFlange(c, { boltHoles: 6, boltRing: 21, boltR: 2 }));
    const feat = await c.get(`/api/agent/parts/${id}/features`);
    const bottom = feat.features.find((f) => f.surface_kind === "plane" && Math.abs(f.origin[2]) < 0.01);
    const bore = feat.features.find((f) => f.surface_kind === "cylinder" && Math.abs(f.radius - 6) < 0.01);
    const post = (path, body) => c.raw("POST", path, body);
    await post(`/api/agent/parts/${id}/datums`, { label: "A", face_id: bottom.face_id, selector: null });
    await post(`/api/agent/parts/${id}/datums`, { label: "B", face_id: bore.face_id, selector: null });
    await post(`/api/agent/parts/${id}/fcf`, {
      characteristic: "perpendicularity",
      tolerance_mm: 0.05,
      datum_refs: ["A"],
      face_id: bore.face_id,
      selector: null,
      basic: null,
    });
    // Ø12 ±0.05 size tolerance on the bore (campaign #55 Slice 4 endpoint).
    await post(`/api/agent/parts/${id}/size-tolerance`, {
      nominal: 12,
      plus_minus: 0.05,
      face_id: bore.face_id,
    });

    // Build the standard sheet.
    const dr = await ctx.time("make drawing", () => c.raw("POST", `/api/parts/${id}/drawing?name=hub_flange`, undefined, 90000));
    t.eq("drawing endpoint returns 200", dr.status, 200, { dim: "soundness" });
    const did = dr.data?.id;

    // Fresh certificate (the sheet as a faithful snapshot).
    const fresh = (await c.raw("GET", `/api/drawings/${did}/certificate`)).data;

    // Founder battery through the query surface.
    const toleranced = await query(c, did, { kind: "toleranced_diameter", face_id: bore.face_id });
    const fcf = await query(c, did, { kind: "fcf", datum: "A" });
    const section = await query(c, did, { kind: "section_cuts" });

    // entity_at on a hatch coordinate (from the semantic sheet).
    const semantic = (await c.raw("GET", `/api/drawings/${did}/semantic`)).data;
    let entityHatch = { answer: "refused", refusal: "render_only" };
    const views = semantic?.drawing?.views ?? [];
    for (let i = 0; i < views.length; i++) {
      const hp = views[i].hatch_polylines ?? [];
      const seg = hp.find((p) => (p.points?.length ?? 0) >= 2);
      if (seg) {
        const a = seg.points[0];
        const b = seg.points[1];
        entityHatch = await query(c, did, {
          kind: "entity_at",
          view: i,
          xy_mm: [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2],
        });
        break;
      }
    }

    // A question with no provenanced referent → unprovenanced refusal.
    const legacy = await query(c, did, { kind: "hole", tag: "ZZ9" });

    // MUTATION: re-drill a WIDER bore after the sheet was built. The certificate
    // must detect the sheet is no longer sound.
    const wide = await c.post("/api/geometry/cylinder", { center: [0, 0, -10], axis: [0, 0, 1], radius: 8, height: 40, name: "rebore" });
    await c.raw("POST", "/api/geometry/boolean", { operation: "difference", object_a: uuid, object_b: wide.object.id, fast: true });
    const afterRedrill = (await c.raw("GET", `/api/drawings/${did}/certificate`)).data;

    oracle(t, {
      fresh,
      toleranced,
      fcf,
      section,
      entityHatch,
      legacy,
      afterRedrill: { sound: afterRedrill?.sound },
    });
  },
};
