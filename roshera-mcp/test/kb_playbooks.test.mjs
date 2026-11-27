// Part-family playbooks (2026-11-18) — the map an agent opens before it makes
// a whole PART, not a single feature. Node-runnable WITHOUT a live backend:
// kb_lookup is compiled data, and every check here is static.
//
// RED-first: authored before the aerofoil_section / sheet_metal_bracket /
// mounting_plate playbooks, the naca_profile and bolt_edge_distance references
// and the worked examples existed — every group below failed until they landed.
//
// Groups:
//   (k) KEYS — each new part-family playbook resolves through kb_lookup and
//       carries the five-part shape (spec first, sizes from tables, build order,
//       datum plan, verify) plus a non-empty tool_sequence.
//   (n) NAMES — every tool a playbook names (tool_sequence, the prose, and every
//       op of its worked example, including ops nested in cad_program) exists in
//       the MCP table. A typo in a playbook is a map that sends the agent to a
//       street that does not exist.
//   (w) WORKED EXAMPLES — every step's args validate against THAT tool's own
//       zod schema (chained '$N.key' placeholders and cross-call `bind` fields
//       get a typed dummy), so a wrong parameter name or type fails here, not in
//       front of an engineer.
//   (a) NACA — the generator reproduces known profile values: NACA 0012
//       half-thickness at x/c = 0.3 is 0.06002c, NACA 2412 max camber 0.02c at
//       0.4c, the closed-TE coefficient closes the trailing edge exactly, and
//       the open-TE coefficient leaves the published 0.021·t·c gap. Mutation
//       control: a profile function with the coefficients swapped fails the
//       closed-TE check.
//   (h) HONESTY — a missing trailing-edge choice REFUSES (the agent must ask),
//       5-digit sections refuse by name, every answer carries a source, and the
//       faceting deviation is reported, never an "exact" claim.
//   (e) EDGE DISTANCE — bolt_edge_distance reproduces EN 1993-1-8 Table 3.3.
//   (c) CONCEPTS — the concept-board skills (2026-11-27): names real, honesty
//       words present, and the concepts worked example generated from its
//       recipe, validated op by op, chained only backwards.
//
// Run: npm run test:gates:build && node test/kb_playbooks.test.mjs  (exit 0 = pass)

import { buildTable } from "./.build/surface.js";

let failures = 0;
const fail = (m) => {
  console.error("  ✗ " + m);
  failures += 1;
};
const pass = (m) => console.log("  ✓ " + m);

const table = buildTable();
const kb = table.get("kb_lookup");

function payloadOf(res) {
  try {
    return JSON.parse(res.content[0].text);
  } catch {
    return null;
  }
}
async function lookup(kind, key, args) {
  if (!kb) return null;
  return payloadOf(await kb.handler({ kind, key, ...(args !== undefined ? { args } : {}) }));
}

const FAMILIES = ["aerofoil_section", "sheet_metal_bracket", "mounting_plate"];

// ── (k) KEYS ────────────────────────────────────────────────────────────────
console.log("(k) KEYS: each part-family playbook resolves with the five-part shape");
const books = {};
for (const key of FAMILIES) {
  const p = await lookup("playbook", key);
  books[key] = p;
  if (!p || p.refused) {
    fail(`playbook:${key} did not resolve: ${JSON.stringify(p)?.slice(0, 200)}`);
    continue;
  }
  pass(`playbook:${key} resolves`);
  const text = String(p.text ?? "");
  const shape = [
    [/SPEC FIRST/, "SPEC FIRST section"],
    [/ask_choice/, "names ask_choice for the closed questions"],
    [/build nothing/i, "build nothing until answered"],
    [/kb_lookup\(kind:"reference"/, "sizes from a reference table"],
    [/BUILD/, "build order"],
    [/datum A/i, "datum plan"],
    [/VERIFY/, "verify section"],
    [/NOT certified|not kernel-certified|uncertified/i, "names what is not certified"],
  ];
  for (const [re, what] of shape) {
    if (re.test(text)) pass(`playbook:${key} text has ${what}`);
    else fail(`playbook:${key} text lacks ${what}`);
  }
  if (/\bTODO\b|placeholder|TBD/i.test(text)) fail(`playbook:${key} text carries a TODO/placeholder`);
  else pass(`playbook:${key} text carries no TODO/placeholder`);
  if (Array.isArray(p.tool_sequence) && p.tool_sequence.length > 0) pass(`playbook:${key} has a tool_sequence`);
  else fail(`playbook:${key} lacks a tool_sequence`);
  if (p.worked_example && Array.isArray(p.worked_example.steps) && p.worked_example.steps.length > 0)
    pass(`playbook:${key} carries a worked_example (${p.worked_example.steps.length} steps)`);
  else fail(`playbook:${key} must carry a worked_example with steps`);
}

const aero = books.aerofoil_section;
if (aero && /twisted_blade/.test(String(aero.text)) && /nurbs_loft/.test(String(aero.text)))
  pass("aerofoil_section names the twisted_blade next step (stations lofted with nurbs_loft)");
else fail("aerofoil_section must carry the twisted_blade next-step note");
if (aero && /aerodynamic/i.test(String(aero.text)) && /never/i.test(String(aero.text)))
  pass("aerofoil_section states aerodynamic performance is never claimed");
else fail("aerofoil_section must state aerodynamic performance is never claimed");

// Backward compatibility: an old playbook still answers, without a worked example.
{
  const hole = await lookup("playbook", "hole");
  if (hole && !hole.refused && typeof hole.text === "string") pass("feature playbook 'hole' still answers (backward compatible)");
  else fail(`feature playbook 'hole' broke: ${JSON.stringify(hole)}`);
  const bad = await lookup("playbook", "nope");
  if (bad && bad.refused && Array.isArray(bad.valid_keys) && FAMILIES.every((k) => bad.valid_keys.includes(k)))
    pass("unknown playbook key refuses and lists the new families among valid_keys");
  else fail(`unknown playbook refusal must list the new families: ${JSON.stringify(bad)}`);
  if (bad && typeof bad.reason === "string" && !/exactly the 8/.test(bad.reason))
    pass("refusal reason no longer claims a stale count of 8");
  else fail(`refusal reason carries a stale count: ${JSON.stringify(bad && bad.reason)}`);
}

// ── (n) NAMES ───────────────────────────────────────────────────────────────
console.log("(n) NAMES: every tool a playbook names exists in the MCP table");
// Prose scan: identifiers with a tool-family prefix, plus the bare verbs.
const PREFIX = /\b(?:psketch_[a-z_]+|gdt_[a-z_]+|select_[a-z_]+|label_[a-z_]+|timeline_[a-z_]+|drill_[a-z_]+|create_[a-z_]+|measure_[a-z_]+|verify_[a-z_]+|dfm_[a-z_]+|mass_[a-z_]+|section_[a-z_]+|ray_[a-z_]+|point_[a-z_]+|part_[a-z_]+|fillet_[a-z_]+|chamfer_[a-z_]+|boolean_[a-z_]+|nurbs_[a-z_]+|make_[a-z_]+|dimension_[a-z_]+|ask_[a-z_]+|cad_[a-z_]+|kb_[a-z_]+)\b/g;
const NOT_TOOLS = new Set(["kb_reference", "part_id", "part_a", "part_b", "point_count", "dfm_check_pack_arg"]);
function opsOf(example) {
  const out = [];
  for (const s of example?.steps ?? []) {
    out.push({ tool: s.tool, args: s.args ?? {}, bind: s.bind ?? {}, where: `step ${s.step}` });
    if (s.tool === "cad_program") {
      (s.args?.ops ?? []).forEach((op, i) =>
        out.push({ tool: op.tool, args: op.args ?? {}, bind: op.bind ?? {}, where: `step ${s.step} op ${i}`, nested: true }),
      );
    }
  }
  return out;
}
for (const key of FAMILIES) {
  const p = books[key];
  if (!p || p.refused) continue;
  const missingSeq = (p.tool_sequence ?? []).filter((n) => !table.has(n));
  if (missingSeq.length === 0) pass(`playbook:${key} tool_sequence all resolve (${p.tool_sequence.length})`);
  else fail(`playbook:${key} tool_sequence names unknown tools: ${missingSeq.join(", ")}`);

  const prose = String(p.text) + "\n" + JSON.stringify(p.worked_example ?? {});
  const named = [...new Set([...prose.matchAll(PREFIX)].map((m) => m[0]))].filter((n) => !NOT_TOOLS.has(n));
  const unknown = named.filter((n) => !table.has(n));
  if (unknown.length === 0) pass(`playbook:${key} prose names ${named.length} tools, all real`);
  else fail(`playbook:${key} prose names unknown tools: ${unknown.join(", ")}`);

  const ops = opsOf(p.worked_example);
  const badOps = ops.filter((o) => !table.has(o.tool));
  if (badOps.length === 0) pass(`playbook:${key} worked example: all ${ops.length} tool calls exist`);
  else fail(`playbook:${key} worked example names unknown tools: ${badOps.map((o) => `${o.where}:${o.tool}`).join(", ")}`);
}

// Mutation control for (n): a planted typo must be caught by the same scan.
{
  const planted = "then psketch_extrud the profile";
  const named = [...planted.matchAll(PREFIX)].map((m) => m[0]).filter((n) => !table.has(n));
  if (named.includes("psketch_extrud")) pass("name scan catches a planted typo 'psketch_extrud' (mutation control)");
  else fail("name scan is vacuous — it missed a planted typo");
}

// ── (w) WORKED EXAMPLES validate against each tool's own schema ────────────
console.log("(w) WORKED EXAMPLES: every op's args validate against that tool's schema");
const PH = /^\$(?:prev|\d+)\./;
const UUID = "00000000-0000-4000-8000-000000000000";
const UUID_KEYS = new Set(["csketch_id", "object", "object_a", "object_b", "base", "object_uuid", "object_id", "part"]);
/** Replace chained placeholders (and `bind` fields) with a dummy of the type the
 *  key implies: uuid-shaped handles get a uuid, everything else an integer id. */
function dummify(v, key, inUuidArray) {
  if (typeof v === "string" && PH.test(v)) return UUID_KEYS.has(key) || inUuidArray ? UUID : 1;
  if (Array.isArray(v)) return v.map((x) => dummify(x, key, key === "tools"));
  if (v && typeof v === "object") {
    const o = {};
    for (const [k, x] of Object.entries(v)) o[k] = dummify(x, k, false);
    return o;
  }
  return v;
}
function withBind(args, bind) {
  const out = JSON.parse(JSON.stringify(args));
  for (const path of Object.keys(bind ?? {})) {
    const parts = path.split(".");
    let cur = out;
    for (let i = 0; i < parts.length - 1; i++) cur = cur[parts[i]] ??= {};
    const leaf = parts[parts.length - 1];
    cur[leaf] = UUID_KEYS.has(leaf) ? UUID : 1;
  }
  return out;
}
function unknownKeys(schema, args) {
  let s = schema;
  while (s && s._def && (s._def.typeName === "ZodEffects" || s._def.typeName === "ZodOptional")) s = s._def.schema ?? s._def.innerType;
  const shape = s && typeof s.shape === "object" ? s.shape : typeof s?._def?.shape === "function" ? s._def.shape() : null;
  if (!shape) return [];
  return Object.keys(args).filter((k) => !(k in shape));
}
for (const key of FAMILIES) {
  const p = books[key];
  if (!p || p.refused) continue;
  let bad = 0;
  let checked = 0;
  for (const o of opsOf(p.worked_example)) {
    const entry = table.get(o.tool);
    if (!entry) continue;
    const args = o.tool === "cad_program" ? { ...o.args, ops: (o.args.ops ?? []).map(({ bind, ...op }) => op) } : o.args;
    const full = dummify(withBind(args, o.bind), "", false);
    const res = entry.schema.safeParse(full);
    checked += 1;
    if (!res.success) {
      bad += 1;
      fail(`playbook:${key} ${o.where} ${o.tool} args fail its schema: ${JSON.stringify(res.error.issues).slice(0, 300)}`);
    }
    // zod objects strip unknown keys silently, so a misspelled OPTIONAL
    // parameter would pass safeParse and be dropped at dispatch: check names.
    const extra = unknownKeys(entry.schema, full);
    if (extra.length > 0) {
      bad += 1;
      fail(`playbook:${key} ${o.where} ${o.tool} passes parameters its schema does not have: ${extra.join(", ")}`);
    }
  }
  if (bad === 0) pass(`playbook:${key} worked example: ${checked} calls validate against their tool schemas`);
}
// Mutation control for (w): a wrong parameter name must fail the same check.
{
  const entry = table.get("psketch_extrude");
  const res = entry.schema.safeParse({ csketch_id: UUID, lenght: 300 });
  if (!res.success) pass("schema check rejects a misspelled required parameter (mutation control)");
  else fail("schema check is vacuous — a misspelled parameter passed");
  const extra = unknownKeys(entry.schema, { csketch_id: UUID, distance: 300, plnae: "xy" });
  if (extra.includes("plnae")) pass("name check catches a misspelled OPTIONAL parameter zod would silently strip (mutation control)");
  else fail("name check is vacuous — a misspelled optional parameter passed");
}

// The aerofoil example's polyline is the generator's own output, not a copy.
{
  const ex = aero?.worked_example;
  const prog = ex?.steps?.find((s) => s.tool === "cad_program" && (s.args?.ops ?? []).some((o) => o.args?.kind === "polyline"));
  const poly = prog?.args?.ops?.find((o) => o.args?.kind === "polyline");
  const gen = await lookup("reference", "naca_profile", ex?.steps?.[0]?.args?.args ?? {});
  if (poly && gen && gen.value && JSON.stringify(poly.args.params.points) === JSON.stringify(gen.value.points))
    pass(`aerofoil worked example polyline == naca_profile output (${gen.value.points.length} vertices)`);
  else fail("aerofoil worked example polyline must be exactly the generator's points");
  if (ex && ex.steps[0].tool === "kb_lookup") pass("aerofoil worked example opens with the naca_profile lookup");
  else fail("aerofoil worked example must open with the naca_profile lookup");
}

// ── (a) NACA analytic checks ────────────────────────────────────────────────
console.log("(a) NACA: the generator reproduces published profile values");
{
  for (const te of ["sharp", "open"]) {
    const r = await lookup("reference", "naca_profile", { designation: "0012", chord_mm: 1000, trailing_edge: te, stations_x_mm: [300] });
    const up = r?.value?.stations?.[0]?.upper_y_mm;
    if (typeof up === "number" && Math.abs(up / 1000 - 0.06002) <= 1e-4)
      pass(`NACA 0012 (${te} TE) half-thickness at x/c=0.3 = ${(up / 1000).toFixed(5)}c (published 0.06002c ±1e-4)`);
    else fail(`NACA 0012 (${te}) half-thickness at 0.3c wrong: ${JSON.stringify(r?.value?.stations ?? r)}`);
  }
  const c = await lookup("reference", "naca_profile", { designation: "2412", chord_mm: 1000, trailing_edge: "sharp" });
  const mc = c?.value?.max_camber;
  if (mc && Math.abs(mc.value_mm - 20) < 1e-6 && Math.abs(mc.at_x_mm - 400) < 1e-6)
    pass(`NACA 2412 max camber ${mc.value_mm} mm at x=${mc.at_x_mm} mm on c=1000 (0.02c at 0.4c)`);
  else fail(`NACA 2412 max camber wrong: ${JSON.stringify(mc ?? c)}`);

  // Closed TE: the -0.1036 coefficient set sums to zero, so the TE closes.
  const sharp = c?.value;
  const last = sharp?.points?.[sharp.points.length - 1];
  const first = sharp?.points?.[0];
  if (sharp && sharp.te_thickness_mm === 0 && first && first[0] === 1000 && Math.abs(first[1]) < 1e-9)
    pass("sharp TE (a4 = -0.1036) closes exactly: TE thickness 0, one TE vertex at (c, 0)");
  else fail(`sharp TE must close exactly: te=${sharp?.te_thickness_mm} first=${JSON.stringify(first)} last=${JSON.stringify(last)}`);
  if (sharp && sharp.coefficient_a4 === -0.1036) pass("sharp TE reports a4 = -0.1036");
  else fail(`sharp TE must report a4 -0.1036: ${sharp?.coefficient_a4}`);

  // Open TE: published coefficient -0.1015 leaves 2·5t·0.0021 = 0.021·t·c.
  const o = await lookup("reference", "naca_profile", { designation: "0012", chord_mm: 1000, trailing_edge: "open" });
  if (o?.value && Math.abs(o.value.te_thickness_mm - 0.021 * 0.12 * 1000) < 1e-6 && o.value.coefficient_a4 === -0.1015)
    pass(`open TE (a4 = -0.1015) leaves ${o.value.te_thickness_mm} mm = 0.021·t·c on NACA 0012 c=1000`);
  else fail(`open TE gap wrong: ${JSON.stringify(o?.value?.te_thickness_mm ?? o)}`);

  // Mutation control: the closed-TE oracle fed the OPEN coefficient must fail.
  const yt = (x, a4) => 5 * 0.12 * (0.2969 * Math.sqrt(x) - 0.126 * x - 0.3516 * x * x + 0.2843 * x ** 3 + a4 * x ** 4);
  if (Math.abs(yt(1, -0.1036)) < 1e-12 && Math.abs(yt(1, -0.1015)) > 1e-4)
    pass("closed-TE oracle distinguishes the coefficients (mutation control: -0.1015 does not close)");
  else fail("closed-TE oracle cannot tell -0.1036 from -0.1015");

  // Blunt TE at a stated thickness is honoured and named as a departure.
  const b = await lookup("reference", "naca_profile", { designation: "2412", chord_mm: 200, trailing_edge: "blunt", te_thickness_mm: 1.0 });
  if (b?.value && Math.abs(b.value.te_thickness_mm - 1.0) < 1e-3 && /depart/i.test(JSON.stringify(b.value.departures ?? "")))
    pass(`blunt TE 1.0 mm honoured (measured ${b.value.te_thickness_mm.toFixed(4)} mm) and named as a departure`);
  else fail(`blunt TE wrong: ${JSON.stringify(b?.value?.te_thickness_mm ?? b)}`);

  // Faceting is measured and stated, the polygon is not the exact profile.
  const f = b?.value?.faceting;
  if (f && typeof f.max_deviation_mm === "number" && f.max_deviation_mm > 0 && f.max_deviation_mm < 0.05 && Array.isArray(b.value.probes) && b.value.probes.length >= 6)
    pass(`faceting stated: max ${f.max_deviation_mm.toExponential(2)} mm off the analytic profile, ${b.value.probes.length} probes for the kernel`);
  else fail(`faceting deviation must be measured and stated: ${JSON.stringify(f ?? b)}`);
  // Probe distances are real: each probe sits that far from the emitted polygon.
  const pts = b?.value?.points ?? [];
  const segDist = (p, a, q) => {
    const dx = q[0] - a[0], dy = q[1] - a[1];
    const L2 = dx * dx + dy * dy;
    let t = L2 > 0 ? ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / L2 : 0;
    t = Math.max(0, Math.min(1, t));
    return Math.hypot(p[0] - (a[0] + t * dx), p[1] - (a[1] + t * dy));
  };
  const polyDist = (p) => {
    let best = Infinity;
    for (let i = 0; i < pts.length; i++) best = Math.min(best, segDist(p, pts[i], pts[(i + 1) % pts.length]));
    return best;
  };
  const probeOk = (b?.value?.probes ?? []).every((pr) => Math.abs(polyDist(pr.point) - pr.polygon_distance_mm) < 1e-6);
  if (probeOk && pts.length > 0) pass("every probe's stated polygon distance re-measures independently (brute-force point-to-polygon)");
  else fail("probe distances do not re-measure against the emitted polygon");
  // Polygon area re-measures (shoelace) and sits just under the analytic area.
  let A = 0;
  for (let i = 0; i < pts.length; i++) {
    const [x1, y1] = pts[i];
    const [x2, y2] = pts[(i + 1) % pts.length];
    A += x1 * y2 - x2 * y1;
  }
  A = Math.abs(A) / 2;
  if (b?.value && Math.abs(A - b.value.polygon_area_mm2) < 1e-6 && b.value.analytic_area_mm2 > A && (b.value.analytic_area_mm2 - A) / A < 1e-3)
    pass(`polygon area ${A.toFixed(4)} mm² re-measures; analytic ${b.value.analytic_area_mm2.toFixed(4)} mm² is larger by ${((b.value.analytic_area_mm2 / A - 1) * 100).toExponential(2)}%`);
  else fail(`area bookkeeping wrong: shoelace ${A} vs ${JSON.stringify({ poly: b?.value?.polygon_area_mm2, analytic: b?.value?.analytic_area_mm2 })}`);
}

// ── (h) HONESTY ─────────────────────────────────────────────────────────────
console.log("(h) HONESTY: the agent must ask; untranscribed series refuse; sources cited");
{
  const noTe = await lookup("reference", "naca_profile", { designation: "2412", chord_mm: 200 });
  if (noTe && noTe.refused === true && /trailing/i.test(noTe.reason) && Array.isArray(noTe.valid_keys))
    pass("naca_profile without trailing_edge REFUSES (sharp TE is a hazard the engineer must choose)");
  else fail(`naca_profile without trailing_edge must refuse: ${JSON.stringify(noTe)}`);
  const five = await lookup("reference", "naca_profile", { designation: "23012", chord_mm: 200, trailing_edge: "sharp" });
  if (five && five.refused === true && /5-digit/.test(five.reason))
    pass("5-digit designation refuses by name (mean-line table not transcribed)");
  else fail(`5-digit designation must refuse: ${JSON.stringify(five)}`);
  const bluntNoT = await lookup("reference", "naca_profile", { designation: "2412", chord_mm: 200, trailing_edge: "blunt" });
  if (bluntNoT && bluntNoT.refused === true && /te_thickness_mm/.test(bluntNoT.reason))
    pass("blunt TE without a stated thickness refuses (never a default)");
  else fail(`blunt TE without thickness must refuse: ${JSON.stringify(bluntNoT)}`);
  const ok = await lookup("reference", "naca_profile", { designation: "2412", chord_mm: 200, trailing_edge: "sharp" });
  if (ok && /Report 460/.test(ok.source) && /-0\.1036/.test(ok.source) && /not in/i.test(ok.source))
    pass("naca_profile source cites NACA Report 460 and names -0.1036 as a later modification");
  else fail(`naca_profile source must cite Report 460 and the modification: ${JSON.stringify(ok && ok.source)}`);
  if (ok && /faceted|polygon/i.test(JSON.stringify(ok.value?.faceting ?? "")) && !/"exact"\s*:\s*true/.test(JSON.stringify(ok)))
    pass("naca_profile names the profile as faceted, never exact");
  else fail("naca_profile must name the polygon as faceted");
}

// ── (e) EDGE DISTANCE ───────────────────────────────────────────────────────
console.log("(e) EDGE DISTANCE: EN 1993-1-8 Table 3.3 minimums");
{
  const e = await lookup("reference", "bolt_edge_distance", { hole_diameter_mm: 9 });
  if (e?.value && Math.abs(e.value.e1_min_mm - 10.8) < 1e-9 && Math.abs(e.value.e2_min_mm - 10.8) < 1e-9 &&
      Math.abs(e.value.p1_min_mm - 19.8) < 1e-9 && Math.abs(e.value.p2_min_mm - 21.6) < 1e-9 && /1993-1-8/.test(e.source))
    pass("bolt_edge_distance(d0 = 9): e1 = e2 = 10.8, p1 = 19.8, p2 = 21.6 mm, EN 1993-1-8 Table 3.3");
  else fail(`bolt_edge_distance(9) wrong: ${JSON.stringify(e)}`);
  const viaFastener = await lookup("reference", "bolt_edge_distance", { fastener: "M8" });
  if (viaFastener?.value && viaFastener.value.hole_diameter_mm === 9 && /house default/i.test(viaFastener.source))
    pass("bolt_edge_distance(M8) resolves d0 via ISO 273 and says the class is the house default");
  else fail(`bolt_edge_distance(M8) must resolve d0 = 9 and attribute the class: ${JSON.stringify(viaFastener)}`);
  const none = await lookup("reference", "bolt_edge_distance", {});
  if (none && none.refused === true) pass("bolt_edge_distance with no hole refuses");
  else fail(`bolt_edge_distance({}) must refuse: ${JSON.stringify(none)}`);
}

// ── (c) CONCEPT-BOARD SKILLS (2026-11-27) ───────────────────────────────────
// concepts / pov / explain_with_maths: not part families (no datum plan, no
// SPEC FIRST -- an open-ended request is the opposite case), so they carry
// their own checks: every tool they name is real, the honesty words are
// there, and the concepts worked example -- generated from concept A's recipe
// by recipeToProgram -- validates op by op, chains only to earlier ops that
// produce what it reads, and uses the cited clearance sizes.
console.log("(c) CONCEPTS: the concept-board skills and the recipe -> program mapping");
{
  const { recipeToProgram, BRACKET_CONCEPT_SET, bracketAVolume } = await import("./.build/tools/kb_concepts.js");
  const SKILLS = ["concepts", "pov", "explain_with_maths"];
  const skill = {};
  for (const key of SKILLS) {
    const p = await lookup("playbook", key);
    skill[key] = p;
    if (!p || p.refused) {
      fail(`playbook:${key} did not resolve: ${JSON.stringify(p)?.slice(0, 200)}`);
      continue;
    }
    pass(`playbook:${key} resolves`);
    const text = String(p.text ?? "");
    if (/\bTODO\b|placeholder|TBD/i.test(text)) fail(`playbook:${key} carries a TODO/placeholder`);
    const missingSeq = (p.tool_sequence ?? []).filter((n) => !table.has(n));
    if (p.tool_sequence?.length > 0 && missingSeq.length === 0) pass(`playbook:${key} tool_sequence all resolve (${p.tool_sequence.length})`);
    else fail(`playbook:${key} tool_sequence empty or names unknown tools: ${missingSeq.join(", ")}`);
    const prose = text + "\n" + JSON.stringify(p.worked_example ?? {});
    const named = [...new Set([...prose.matchAll(PREFIX)].map((m) => m[0]))].filter((n) => !NOT_TOOLS.has(n));
    const unknown = named.filter((n) => !table.has(n));
    if (unknown.length === 0) pass(`playbook:${key} prose names ${named.length} tools, all real`);
    else fail(`playbook:${key} prose names unknown tools: ${unknown.join(", ")}`);
  }
  const c = String(skill.concepts?.text ?? "");
  const want = [
    [/open-ended/i, "when: an open-ended request"],
    [/straight to building/, "a sized request goes straight to building"],
    [/SPEC FIRST/, "a standard-governed part stays SPEC FIRST"],
    [/THREE concepts/, "three concepts"],
    [/roshera:concept/, "names the card fence"],
    [/create_box[\s\S]*create_cylinder[\s\S]*create_cone[\s\S]*psketch_extrude/, "maps every shape to its verb"],
    [/difference/, "subtract is a boolean difference"],
    [/fillet_edges at r/, "a blend is a fillet on the junction"],
    [/never approximate/, "never approximate"],
    [/REFUSES the build/, "refuses with the reason"],
    [/NOT certified/, "a concept is not certified"],
    [/estimate/, "numbers are estimates"],
  ];
  for (const [re, what] of want) (re.test(c) ? pass : fail)(`concepts text: ${what}`);
  const pv = String(skill.pov?.text ?? "");
  for (const [re, what] of [
    [/LOAD PATH/, "load path"],
    [/MAKER, mill/, "maker: milling reach"],
    [/45 degrees past vertical downward/, "maker: FDM overhang rule"],
    [/estimate/, "says estimate"],
    [/NOT AVAILABLE YET[\s\S]*FIT[\s\S]*HAND AND EYE/, "Fit and Hand-and-eye named as not available"],
  ]) (re.test(pv) ? pass : fail)(`pov text: ${what}`);
  const mx = String(skill.explain_with_maths?.text ?? "");
  for (const [re, what] of [
    [/\\frac\{F L\^3\}\{3 E I\}/, "a worked derivation in LaTeX"],
    [/never an invented diagram/, "graphics only from recipe or kernel data"],
    [/estimate/, "a hand calculation is an estimate"],
  ]) (re.test(mx) ? pass : fail)(`explain_with_maths text: ${what}`);

  // Chaining: every '$N.key' reads an EARLIER op that produces that key.
  const PRODUCES = {
    csketch_id: new Set(["psketch_begin"]),
    edge_id: new Set(["select_edge"]),
    object_uuid: new Set(["create_box", "create_cylinder", "create_cone", "psketch_extrude", "boolean", "boolean_many", "fillet_edges"]),
    part_id: new Set(["create_box", "create_cylinder", "create_cone", "psketch_extrude", "boolean", "boolean_many", "fillet_edges"]),
  };
  const chainFaults = (ops) => {
    const faults = [];
    ops.forEach((op, i) => {
      for (const m of JSON.stringify(op.args).matchAll(/"\$(\d+)\.([a-z_]+)"/g)) {
        const n = Number(m[1]);
        const from = ops[n];
        if (!(n < i) || !from || !PRODUCES[m[2]]?.has(from.tool)) faults.push(`op ${i} ${op.tool} reads $${n}.${m[2]} from ${from?.tool ?? "nothing"}`);
      }
    });
    return faults;
  };
  {
    const planted = [{ tool: "create_box", args: {} }, { tool: "boolean", args: { object_a: "$0.object_uuid", object_b: "$5.object_uuid" } }];
    if (chainFaults(planted).length === 1) pass("chain check catches a planted forward reference (mutation control)");
    else fail("chain check is vacuous -- a forward reference passed");
  }

  // The worked example: every op validates against its schema, names only its own parameters.
  const ex = skill.concepts?.worked_example;
  if (!ex) fail(`concepts must carry a worked_example: ${JSON.stringify(skill.concepts?.worked_example_refused ?? null)}`);
  else {
    let bad = 0;
    let checked = 0;
    for (const o of opsOf(ex)) {
      const entry = table.get(o.tool);
      if (!entry) {
        bad += 1;
        fail(`concepts ${o.where}: unknown tool ${o.tool}`);
        continue;
      }
      const args = o.tool === "cad_program" ? { ...o.args, ops: (o.args.ops ?? []).map(({ bind, ...op }) => op) } : o.args;
      const full = dummify(withBind(args, o.bind), "", false);
      const res = entry.schema.safeParse(full);
      checked += 1;
      if (!res.success) {
        bad += 1;
        fail(`concepts ${o.where} ${o.tool} args fail its schema: ${JSON.stringify(res.error.issues).slice(0, 300)}`);
      }
      const extra = unknownKeys(entry.schema, full);
      if (extra.length > 0) {
        bad += 1;
        fail(`concepts ${o.where} ${o.tool} passes parameters its schema does not have: ${extra.join(", ")}`);
      }
    }
    if (bad === 0) pass(`concepts worked example: ${checked} calls validate against their tool schemas`);

    const prog = ex.steps.find((s) => s.tool === "cad_program");
    const faults = chainFaults(prog?.args?.ops ?? []);
    if (prog && faults.length === 0) pass(`concepts program: all ${prog.args.ops.length} ops chain to earlier ops that produce what they read`);
    else fail(`concepts program chaining: ${faults.join("; ") || "no cad_program step"}`);

    // The program IS the mapping's output for concept A (it cannot drift from the recipe).
    const again = recipeToProgram(BRACKET_CONCEPT_SET.concepts[0].recipe);
    if (!again.refused && JSON.stringify(again.ops) === JSON.stringify(prog?.args?.ops)) pass("concepts program == recipeToProgram(concept A)");
    else fail("the worked example's program must be recipeToProgram's output for concept A");
    // One boolean per later step; subtract -> difference.
    const steps = BRACKET_CONCEPT_SET.concepts[0].recipe.steps;
    const booleans = (prog?.args?.ops ?? []).filter((o) => o.tool === "boolean");
    if (booleans.length === steps.length - 1 && booleans.filter((b) => b.args.op === "difference").length === steps.filter((s) => s.op === "subtract").length)
      pass(`one boolean per later step (${booleans.length}), subtract -> difference`);
    else fail(`boolean count/op mismatch: ${JSON.stringify(booleans.map((b) => b.args.op))}`);
    // The worked example's verify_claim expects the hand volume.
    const claim = ex.steps.find((s) => s.tool === "verify_claim");
    const hand = 60 * 40 * 5 + 60 * 5 * 50 + 2 * 0.5 * 30 * 30 * 5 - Math.PI * 5 * (11.5 ** 2 + 4 * 1.7 ** 2 + 2 * 2.75 ** 2);
    if (claim && Math.abs(claim.args.expected - hand) < 1e-3 && Math.abs(bracketAVolume() - hand) < 1e-3)
      pass(`verify_claim expects the hand volume ${claim.args.expected} mm^3`);
    else fail(`verify_claim volume wrong: ${claim?.args?.expected} vs ${hand}`);
    // The card posted in the example is the concept set itself.
    const post = ex.steps.find((s) => s.tool === "blackboard_add_entry");
    const body = /^```roshera:concept\n([\s\S]*)\n```$/.exec(String(post?.args?.text ?? ""));
    if (body && JSON.stringify(JSON.parse(body[1])) === JSON.stringify(BRACKET_CONCEPT_SET)) pass("the example posts the concept set as one roshera:concept fence");
    else fail("the example must post the concept set verbatim in a roshera:concept fence");
  }

  // Hole sizes are the cited clearances, not typed from memory.
  const m5 = await lookup("reference", "clearance_hole", { fastener: "M5" });
  const m3 = await lookup("reference", "clearance_hole", { fastener: "M3" });
  const holes = BRACKET_CONCEPT_SET.concepts[0].recipe.steps.filter((s) => s.op === "subtract" && s.shape.radius < 5).map((s) => s.shape.radius * 2);
  const okSizes = holes.length === 6 && holes.every((d) => d === m5?.value?.diameter_mm || d === m3?.value?.diameter_mm);
  if (okSizes) pass(`concept A's holes are ISO 273 clearances (M5 D${m5.value.diameter_mm}, M3 D${m3.value.diameter_mm})`);
  else fail(`concept A's hole diameters ${JSON.stringify(holes)} must be the clearance_hole values (M5 ${m5?.value?.diameter_mm}, M3 ${m3?.value?.diameter_mm})`);

  // A blend cannot be planned before the union exists: refused with the procedure, never dropped.
  const blended = recipeToProgram(BRACKET_CONCEPT_SET.concepts[2].recipe);
  if (blended.refused === true && /select_edge/.test(blended.reason) && /fillet_edges/.test(blended.reason) && /refuse the build/.test(blended.reason))
    pass("a blended recipe is refused with the fillet procedure (never silently built without it)");
  else fail(`a blend must refuse static planning: ${JSON.stringify(blended)}`);
  // The plate and the slot map to their verbs, and chain correctly.
  const b = recipeToProgram(BRACKET_CONCEPT_SET.concepts[1].recipe);
  const tools = (b.ops ?? []).map((o) => o.tool);
  if (!b.refused && tools.filter((t) => t === "select_edge").length === 4 && tools.includes("fillet_edges") && tools.includes("boolean_many"))
    pass("plate -> create_box + 4 select_edge + fillet_edges; slot -> box + 2 cylinders + boolean_many union");
  else fail(`plate/slot mapping wrong: ${tools.join(", ")}`);
  const bFaults = b.refused ? ["refused"] : chainFaults(b.ops);
  if (bFaults.length === 0) pass("concept B's program chains to earlier ops that produce what they read");
  else fail(`concept B chaining: ${bFaults.join("; ")}`);
}

console.log(failures === 0 ? "\nPASS — part-family playbooks hold" : `\nFAIL — ${failures} problem(s)`);
process.exit(failures === 0 ? 0 : 1);
