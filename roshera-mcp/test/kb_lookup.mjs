// kb_lookup — tiered knowledge-base retrieval (policy KB, vault
// Research/2026-07-31-policy-knowledge-base.md). Node-runnable WITHOUT a live
// backend: kb_lookup is served entirely from compiled data, no fetch.
//
// RED-first: this suite was authored before src/tools/kb.ts existed, so every
// group failed (kb_lookup absent from the table) until the tool landed.
//
// Groups:
//   (s) SURFACE INVARIANT — kb_lookup is in the FULL table only, never in
//       CORE_SURFACE / MINIMAL_SURFACE; the minimal bill is UNCHANGED by its
//       existence (pinned to the pre-kb measurement).
//   (p) PROVENANCE — every response carrying a value carries a non-empty
//       `source` (the DfmValue no-number-without-provenance rule extended to
//       reference data), with a negative-control mutation proof of the oracle.
//   (r) REFERENCE CONTRACT — the doc's §5.3 worked examples reproduce exactly.
//   (h) HONESTY — [V]/conflicting items refuse by name, never silently default.
//   (1) TIER 1 — pack chunks byte-identical to the vault doc (when present),
//       token estimates pinned to the doc's measured numbers, certification
//       boundary fields correct per §3.0.
//   (2) TIER 2 — playbook chunks + tool_sequence names all resolve in the table.
//   (f) FUNNEL — find_tool discovers kb_lookup by intent; invoke dispatches it.
//
// Run: node test/kb_lookup.mjs   (exit 0 = pass, non-zero = fail)

import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  buildTable,
  CORE_SURFACE,
  MINIMAL_SURFACE,
  exposedNamesFor,
  billFor,
} from "../dist/surface.js";
import { rankTools } from "../dist/metatools.js";

let failures = 0;
const fail = (m) => {
  console.error("  ✗ " + m);
  failures += 1;
};
const pass = (m) => console.log("  ✓ " + m);
const eq = (a, b) => JSON.stringify(a) === JSON.stringify(b);

const table = buildTable();
const kb = table.get("kb_lookup");

/** Parse the JSON payload out of a tool result ({content:[{type:"text",text}]}). */
function payloadOf(res) {
  try {
    return JSON.parse(res.content[0].text);
  } catch {
    return null;
  }
}

async function lookup(kind, key, args) {
  if (!kb) return null;
  const res = await kb.handler({ kind, key, ...(args !== undefined ? { args } : {}) });
  return payloadOf(res);
}

// ── (s) SURFACE INVARIANT ────────────────────────────────────────────────────
console.log("(s) SURFACE: kb_lookup in the FULL table only; minimal bill unmoved");
{
  if (kb) pass("kb_lookup is registered in the tool table");
  else fail("kb_lookup is NOT in the tool table");

  if (!CORE_SURFACE.includes("kb_lookup")) pass("kb_lookup absent from CORE_SURFACE");
  else fail("kb_lookup must NEVER be in CORE_SURFACE");
  if (!MINIMAL_SURFACE.includes("kb_lookup")) pass("kb_lookup absent from MINIMAL_SURFACE");
  else fail("kb_lookup must NEVER be in MINIMAL_SURFACE");

  const minimal = exposedNamesFor(table, "minimal");
  if (!minimal.includes("kb_lookup")) pass("minimal exposure omits kb_lookup");
  else fail("minimal exposure must omit kb_lookup");

  const full = exposedNamesFor(table, "full");
  if (full.includes("kb_lookup")) pass("full exposure includes kb_lookup");
  else fail("full exposure must include kb_lookup");

  // The load-bearing number: the resident minimal bill measured 5070 tokens
  // immediately BEFORE kb_lookup existed (scale_s2, 2026-07-31). The design's
  // zero-marginal-cost claim means kb_lookup's OWN existence/growth must not
  // move this number by one token — CORE_SURFACE/META_SURFACE tools are the
  // only inputs to this bill and kb_lookup is deliberately in neither.
  //
  // Pin re-based 2026-08-01: the bill had already drifted to 5139 BEFORE this
  // session's dimensioning-playbook/flange-table changes (confirmed by
  // stashing this session's 3 edited files and re-measuring at HEAD — same
  // 5139, same scale_s2.mjs "exceeds 5000 target" failure) — pre-existing
  // drift from unrelated CORE_SURFACE-tool wording changes on this branch,
  // same class of issue scale_s2.mjs's own comment documents for the
  // blackboard commit. Not this task's regression; re-pinned so this test
  // still catches a REAL future move.
  const minimalBill = billFor(table, MINIMAL_SURFACE);
  if (minimalBill === 5139)
    pass(`minimal bill ${minimalBill} == re-based pin 5139 (kb_lookup's zero marginal cost holds; 2026-08-01 dimensioning/flange work moved it 0 tokens)`);
  else
    fail(`minimal bill ${minimalBill} != 5139 — something moved the resident surface`);
}

// ── (p) PROVENANCE ──────────────────────────────────────────────────────────
console.log("(p) PROVENANCE: every value carries a source; oracle mutation-proved");
{
  /** The no-bare-number oracle: a payload that answers (has `value` or `text`)
   *  must cite a non-empty string `source`; a payload that refuses must name a
   *  non-empty `reason`. Anything else is a bare number / silent dodge. */
  const hasProvenance = (p) => {
    if (!p || typeof p !== "object") return false;
    if (p.refused === true) return typeof p.reason === "string" && p.reason.length > 0;
    if ("value" in p || "text" in p)
      return typeof p.source === "string" && p.source.length > 0;
    return false;
  };

  // Negative controls FIRST — prove the oracle itself catches the defect class.
  if (!hasProvenance({ value: { diameter_mm: 6.4 } }))
    pass("oracle rejects a bare value with no source (mutation control)");
  else fail("oracle FAILED to reject a sourceless value — the oracle is vacuous");
  if (!hasProvenance({ value: { diameter_mm: 6.4 }, source: "" }))
    pass("oracle rejects an empty-string source (mutation control)");
  else fail("oracle FAILED to reject an empty source");
  if (!hasProvenance({ refused: true }))
    pass("oracle rejects a refusal with no reason (mutation control)");
  else fail("oracle FAILED to reject a reasonless refusal");

  // Live sweep: every kind/key the tool serves, including refusal paths.
  const sweep = [
    ["reference", "clearance_hole", { fastener: "M6", class: "close" }],
    ["reference", "clearance_hole", { fastener: "M6" }], // refusal (Q6)
    ["reference", "tap_drill", { thread: "M6x1.0" }],
    ["reference", "general_tolerance", { nominal_mm: 50, class: "m" }],
    ["reference", "general_tolerance", { nominal_mm: 50 }], // refusal (Q1)
    ["reference", "fit_class", { nominal_mm: 20, fit: "H7/g6" }],
    ["reference", "thread_spec", { thread: "M6" }],
    ["reference", "standard_stock", { kind: "sheet" }], // refusal (needs Varun)
    ["reference", "bend_allowance", { material: "5052-H32", thickness_mm: 2, angle_deg: 90, radius_mm: 2 }], // conflict refusal
    ["reference", "bend_allowance", { thickness_mm: 2, angle_deg: 90, radius_mm: 2, k_factor: 0.41 }],
    ["reference", "drill_size", { diameter_mm: 5.2 }],
    ["reference", "drill_size", { diameter_mm: 5.2, series: "letter" }], // refusal (untranscribed)
    ["pack", "fdm"],
    ["pack", "sla"],
    ["pack", "cnc_3_axis"],
    ["pack", "nope"], // refusal (unknown key)
    ["playbook", "hole"],
    ["playbook", "snap_fit"],
    ["playbook", "nope"], // refusal (unknown key)
  ];
  for (const [kind, key, args] of sweep) {
    const p = await lookup(kind, key, args);
    const tag = `${kind}:${key}${args ? " " + JSON.stringify(args) : ""}`;
    if (p && hasProvenance(p)) pass(`provenance holds for ${tag}${p.refused ? " (refusal names its reason)" : ""}`);
    else fail(`provenance VIOLATED for ${tag}: ${JSON.stringify(p)}`);
  }
}

// ── (r) REFERENCE CONTRACT — the doc's §5.3 worked examples ─────────────────
console.log("(r) REFERENCE: doc §5.3 worked examples reproduce");
{
  const ch = await lookup("reference", "clearance_hole", { fastener: "M6", class: "close" });
  if (ch && ch.value && ch.value.diameter_mm === 6.4 && /ISO 273/.test(ch.source))
    pass(`clearance_hole(M6, close) = 6.4 mm, source cites ISO 273`);
  else fail(`clearance_hole(M6, close) wrong: ${JSON.stringify(ch)}`);

  const td = await lookup("reference", "tap_drill", { thread: "M6x1.0", material: "aluminum", engagement_pct: 75 });
  if (td && td.value && td.value.diameter_mm === 5.0)
    pass(`tap_drill(M6x1.0, 75%) = 5.0 mm`);
  else fail(`tap_drill(M6x1.0) wrong: ${JSON.stringify(td)}`);

  const gt = await lookup("reference", "general_tolerance", { nominal_mm: 50, class: "m" });
  if (gt && gt.value && gt.value.plus_minus_mm === 0.3 && /ISO 2768/.test(gt.source))
    pass(`general_tolerance(50, m) = ±0.3 mm, source cites ISO 2768`);
  else fail(`general_tolerance(50, m) wrong: ${JSON.stringify(gt)}`);

  const fc = await lookup("reference", "fit_class", { nominal_mm: 20, fit: "H7/g6" });
  if (
    fc && fc.value &&
    eq(fc.value.hole_tol_um, [0, 21]) &&
    eq(fc.value.shaft_tol_um, [-20, -7]) &&
    /ISO 286/.test(fc.source)
  )
    pass(`fit_class(20, H7/g6): hole [0,+21] µm, shaft [-20,-7] µm, ISO 286`);
  else fail(`fit_class(20, H7/g6) wrong: ${JSON.stringify(fc)}`);

  // A second fit as an independent check of the deviation+IT machinery:
  // 25 H7/p6 → hole [0,+21], shaft ei=+22 (p, >18–30), es=+22+13=+35.
  const fp = await lookup("reference", "fit_class", { nominal_mm: 25, fit: "H7/p6" });
  if (fp && fp.value && eq(fp.value.hole_tol_um, [0, 21]) && eq(fp.value.shaft_tol_um, [22, 35]))
    pass(`fit_class(25, H7/p6): shaft [+22,+35] µm (interference machinery checks out)`);
  else fail(`fit_class(25, H7/p6) wrong: ${JSON.stringify(fp)}`);

  const ts = await lookup("reference", "thread_spec", { thread: "M6" });
  if (ts && ts.value && ts.value.pitch_mm === 1.0 && ts.value.tap_drill_mm === 5.0 &&
      ts.value.clearance_mm && ts.value.clearance_mm.close === 6.4)
    pass(`thread_spec(M6): pitch 1.0, tap drill 5.0, close clearance 6.4 in one record`);
  else fail(`thread_spec(M6) wrong: ${JSON.stringify(ts)}`);

  const ds = await lookup("reference", "drill_size", { diameter_mm: 5.2 });
  if (ds && ds.value && typeof ds.value.nearest_under_mm === "number" && typeof ds.value.nearest_over_mm === "number" &&
      ds.value.nearest_under_mm <= 5.2 && ds.value.nearest_over_mm >= 5.2)
    pass(`drill_size(5.2) brackets: [${ds.value.nearest_under_mm}, ${ds.value.nearest_over_mm}]`);
  else fail(`drill_size(5.2) wrong: ${JSON.stringify(ds)}`);

  const ba = await lookup("reference", "bend_allowance", { thickness_mm: 2.0, angle_deg: 90, radius_mm: 2.0, k_factor: 0.41 });
  // BA = θ·(R + K·t) = (π/2)·(2 + 0.41·2) = (π/2)·2.82 ≈ 4.430 mm
  const expected = (Math.PI / 2) * (2 + 0.41 * 2);
  if (ba && ba.value && Math.abs(ba.value.bend_allowance_mm - expected) < 1e-9 && ba.value.k_factor === 0.41)
    pass(`bend_allowance(t2, 90°, R2, K=0.41) = ${ba.value.bend_allowance_mm} (caller-supplied K, cited)`);
  else fail(`bend_allowance with explicit K wrong: ${JSON.stringify(ba)}`);
  if (ba && /caller/i.test(ba.source))
    pass("bend_allowance source names the K-factor as caller-supplied, not a house default");
  else fail(`bend_allowance source must attribute K to the caller: ${JSON.stringify(ba && ba.source)}`);
}

// ── (h) HONESTY — [V]/conflicting items refuse by name ──────────────────────
console.log("(h) HONESTY: open questions and vendor conflicts refuse, never default");
{
  const noClass = await lookup("reference", "clearance_hole", { fastener: "M6" });
  if (noClass && noClass.refused === true && /Q6/.test(noClass.reason))
    pass("clearance_hole without class REFUSES naming open question Q6 (no house default)");
  else fail(`clearance_hole without class must refuse naming Q6: ${JSON.stringify(noClass)}`);

  const noTolClass = await lookup("reference", "general_tolerance", { nominal_mm: 50 });
  if (noTolClass && noTolClass.refused === true && /Q1/.test(noTolClass.reason))
    pass("general_tolerance without class REFUSES naming open question Q1");
  else fail(`general_tolerance without class must refuse naming Q1: ${JSON.stringify(noTolClass)}`);

  const noK = await lookup("reference", "bend_allowance", { material: "5052-H32", thickness_mm: 2, angle_deg: 90, radius_mm: 2 });
  if (
    noK && noK.refused === true &&
    noK.conflict && eq(noK.conflict.k_factor_range, [0.33, 0.5]) &&
    !("value" in noK)
  )
    pass("bend_allowance without K REFUSES, surfacing the 0.33–0.5 vendor range unpicked");
  else fail(`bend_allowance without K must surface the conflict, not pick: ${JSON.stringify(noK)}`);

  const stock = await lookup("reference", "standard_stock", { kind: "sheet" });
  if (stock && stock.refused === true && /Varun|house/i.test(stock.reason) && !("value" in stock))
    pass("standard_stock REFUSES (vendor/region dependent; no house list on file)");
  else fail(`standard_stock must refuse pending a house stock list: ${JSON.stringify(stock)}`);

  const nonH = await lookup("reference", "fit_class", { nominal_mm: 20, fit: "F7/h6" });
  if (nonH && nonH.refused === true && /basic-hole|H\b/.test(nonH.reason))
    pass("fit_class non-H hole system REFUSES by name (only basic-hole transcribed)");
  else fail(`fit_class(F7/h6) must refuse: ${JSON.stringify(nonH)}`);

  const fine = await lookup("reference", "thread_spec", { thread: "M8x1.0" });
  if (fine && fine.refused === true)
    pass("thread_spec fine-pitch REFUSES (coarse series transcribed only) — named gap");
  else fail(`thread_spec(M8x1.0) must refuse as untranscribed: ${JSON.stringify(fine)}`);

  const letter = await lookup("reference", "drill_size", { diameter_mm: 5.2, series: "letter" });
  if (letter && letter.refused === true)
    pass("drill_size letter series REFUSES (untranscribed) — named gap");
  else fail(`drill_size(series letter) must refuse: ${JSON.stringify(letter)}`);

  const badKey = await lookup("reference", "youngs_modulus", { material: "steel" });
  if (badKey && badKey.refused === true && Array.isArray(badKey.valid_keys))
    pass("unknown reference key REFUSES listing the valid keys");
  else fail(`unknown reference key must refuse with valid_keys: ${JSON.stringify(badKey)}`);
}

// ── (1) TIER 1 — packs ──────────────────────────────────────────────────────
console.log("(1) TIER 1: pack chunks match the vault doc; certification boundary holds");
{
  // Doc-measured token pins (§3, exact ceil(chars/4) of each RETRIEVAL CHUNK).
  const PACK_TOKENS = {
    fdm: 259,
    injection_molding: 224,
    sla: 175,
    sls: 190,
    cnc_3_axis: 194,
    cnc_5_axis: 188,
    sheet_metal: 204,
    casting: 196,
  };
  // §3.0 honesty table.
  const PACK_CERT = {
    fdm: { arg: "fdm", rules: ["fdm.overhang", "fdm.min_wall", "fdm.min_bore", "fdm.trapped_volume"], presence: "certified" },
    injection_molding: { arg: "injection_molding", rules: ["mold.draft"], presence: "certified" },
    cnc_3_axis: { arg: null, rules: [], presence: "schema_slot_no_rules" },
    sheet_metal: { arg: null, rules: [], presence: "schema_slot_no_rules" },
    sla: { arg: null, rules: [], presence: "none" },
    sls: { arg: null, rules: [], presence: "none" },
    cnc_5_axis: { arg: null, rules: [], presence: "none" },
    casting: { arg: null, rules: [], presence: "none" },
  };

  // Independent oracle: re-extract the chunks from the vault doc itself when
  // the vault is present (it is gitignored — fall back to the token pins only).
  const here = path.dirname(fileURLToPath(import.meta.url));
  const vaultDoc = path.resolve(here, "../../Roshera-vault/Research/2026-07-31-policy-knowledge-base.md");
  let docChunks = null;
  if (fs.existsSync(vaultDoc)) {
    docChunks = {};
    const txt = fs.readFileSync(vaultDoc, "utf8");
    const re = /```RETRIEVAL-CHUNK:([A-Z0-9_:]+)\n([\s\S]*?)```/g;
    let m;
    while ((m = re.exec(txt)) !== null) {
      let t = m[2];
      if (t.endsWith("\n")) t = t.slice(0, -1);
      docChunks[m[1]] = t;
    }
  }

  for (const [key, tokens] of Object.entries(PACK_TOKENS)) {
    const p = await lookup("pack", key);
    if (!p || p.refused) {
      fail(`pack:${key} did not answer: ${JSON.stringify(p)}`);
      continue;
    }
    if (p.token_estimate === tokens) pass(`pack:${key} token_estimate ${tokens} == doc-measured`);
    else fail(`pack:${key} token_estimate ${p.token_estimate} != doc-measured ${tokens}`);

    if (docChunks) {
      const want = docChunks["PACK:" + key.toUpperCase()];
      if (want !== undefined && p.text === want) pass(`pack:${key} text is byte-identical to the vault chunk`);
      else fail(`pack:${key} text DRIFTED from the vault chunk`);
    }

    const cert = PACK_CERT[key];
    if (p.dfm_check_pack_arg === cert.arg) pass(`pack:${key} dfm_check_pack_arg = ${JSON.stringify(cert.arg)}`);
    else fail(`pack:${key} dfm_check_pack_arg ${JSON.stringify(p.dfm_check_pack_arg)} != ${JSON.stringify(cert.arg)}`);
    if (eq(p.kernel_certified_rules, cert.rules)) pass(`pack:${key} kernel_certified_rules exact`);
    else fail(`pack:${key} kernel_certified_rules ${JSON.stringify(p.kernel_certified_rules)} != ${JSON.stringify(cert.rules)}`);
    if (p.kernel_presence === cert.presence) pass(`pack:${key} kernel_presence = ${cert.presence}`);
    else fail(`pack:${key} kernel_presence ${p.kernel_presence} != ${cert.presence}`);

    // Certification boundary: an uncertified pack must SAY so in the payload.
    if (cert.arg === null) {
      if (p.certified === false && typeof p.certification === "string" && /not.*kernel|kernel.*not|uncertified/i.test(p.certification))
        pass(`pack:${key} marks itself NOT kernel-certified in the payload`);
      else fail(`pack:${key} must mark uncertified guidance: ${JSON.stringify({ certified: p.certified, certification: p.certification })}`);
    } else if (p.certified === true) {
      pass(`pack:${key} marks its kernel-certified rules`);
    } else {
      fail(`pack:${key} should be certified=true`);
    }
  }

  const unknown = await lookup("pack", "wire_edm");
  if (unknown && unknown.refused === true && Array.isArray(unknown.valid_keys) && unknown.valid_keys.includes("fdm"))
    pass("unknown pack key refuses listing the 8 valid processes");
  else fail(`pack:wire_edm must refuse with valid_keys: ${JSON.stringify(unknown)}`);
}

// ── (2) TIER 2 — playbooks ──────────────────────────────────────────────────
console.log("(2) TIER 2: playbook chunks + tool_sequence resolve against the table");
{
  const PLAYBOOK_TOKENS = {
    hole: 262,
    boss: 182,
    rib: 173,
    gusset: 143,
    bearing_seat: 189,
    flange: 174,
    bolt_pattern: 190,
    snap_fit: 206,
  };
  for (const [key, tokens] of Object.entries(PLAYBOOK_TOKENS)) {
    const p = await lookup("playbook", key);
    if (!p || p.refused) {
      fail(`playbook:${key} did not answer: ${JSON.stringify(p)}`);
      continue;
    }
    if (p.token_estimate === tokens) pass(`playbook:${key} token_estimate ${tokens} == doc-measured`);
    else fail(`playbook:${key} token_estimate ${p.token_estimate} != doc-measured ${tokens}`);

    if (Array.isArray(p.tool_sequence) && p.tool_sequence.length > 0) {
      const missing = p.tool_sequence.filter((n) => !table.has(n));
      if (missing.length === 0) pass(`playbook:${key} tool_sequence all resolve in the table (${p.tool_sequence.length} tools)`);
      else fail(`playbook:${key} tool_sequence names unknown tools: ${missing.join(", ")}`);
    } else {
      fail(`playbook:${key} must carry a non-empty tool_sequence`);
    }
  }

  const snap = await lookup("playbook", "snap_fit");
  if (snap && typeof snap.certification === "string" && /no kernel|not.*kernel/i.test(snap.certification))
    pass("playbook:snap_fit certification names the total absence of kernel backing");
  else fail(`playbook:snap_fit must flag no-kernel-backing: ${JSON.stringify(snap && snap.certification)}`);
}

// ── (3) NEW 2026-08-01 — dimensioning playbook + flange_dims reference ──────
console.log("(3) NEW: dimensioning playbook + flange_dims reference (the DN50 case)");
{
  const dim = await lookup("playbook", "dimensioning");
  if (dim && !dim.refused && Array.isArray(dim.tool_sequence) && dim.tool_sequence.length > 0) {
    const missing = dim.tool_sequence.filter((n) => !table.has(n));
    if (missing.length === 0) pass(`playbook:dimensioning tool_sequence all resolve in the table (${dim.tool_sequence.length} tools)`);
    else fail(`playbook:dimensioning tool_sequence names unknown tools: ${missing.join(", ")}`);
  } else {
    fail(`playbook:dimensioning must answer with a non-empty tool_sequence: ${JSON.stringify(dim)}`);
  }
  if (dim && typeof dim.text === "string" && /datum/i.test(dim.text) && /degree of freedom|DOF/i.test(dim.text))
    pass("playbook:dimensioning text covers datums and one-dimension-per-DOF");
  else fail(`playbook:dimensioning text missing expected content: ${JSON.stringify(dim && dim.text)}`);

  // The DN50 case that started this task must work end to end.
  const dn50 = await lookup("reference", "flange_dims", { standard: "EN 1092-1", class: "PN16", size: "DN50" });
  if (
    dn50 && dn50.value && typeof dn50.source === "string" && dn50.source.length > 0 &&
    dn50.value.flange_od_mm === 165 && dn50.value.bolt_circle_diameter_mm === 125 &&
    dn50.value.bolt_count === 4 && dn50.value.bolt_hole_diameter_mm === 18
  )
    pass(`flange_dims(EN 1092-1 PN16, DN50) = {value, source} — OD 165mm, BCD 125mm, 4x18mm holes, source: "${dn50.source}"`);
  else fail(`flange_dims(EN 1092-1 PN16, DN50) wrong: ${JSON.stringify(dn50)}`);

  const asmeHalf = await lookup("reference", "flange_dims", { standard: "ASME B16.5", class: "150", size: "1/2" });
  if (asmeHalf && asmeHalf.value && asmeHalf.value.flange_od_mm === 89.0 && /ASME B16.5/.test(asmeHalf.source))
    pass(`flange_dims(ASME B16.5 Class 150, NPS 1/2) = {value, source}, OD 89.0mm`);
  else fail(`flange_dims(ASME B16.5, NPS 1/2) wrong: ${JSON.stringify(asmeHalf)}`);

  // Out-of-range REFUSES by name rather than extrapolating.
  const dn250 = await lookup("reference", "flange_dims", { standard: "EN 1092-1", class: "PN16", size: "DN250" });
  if (dn250 && dn250.refused === true && !("value" in dn250) && Array.isArray(dn250.valid_keys))
    pass("flange_dims(EN 1092-1 PN16, DN250) REFUSES (outside the transcribed DN15-200 range) naming valid_keys");
  else fail(`flange_dims(DN250) must refuse, not extrapolate: ${JSON.stringify(dn250)}`);

  const nps12 = await lookup("reference", "flange_dims", { standard: "ASME B16.5", class: "150", size: "12" });
  if (nps12 && nps12.refused === true && !("value" in nps12) && Array.isArray(nps12.valid_keys))
    pass("flange_dims(ASME B16.5 Class 150, NPS 12) REFUSES (outside the transcribed 1/2-8 range) naming valid_keys");
  else fail(`flange_dims(NPS 12) must refuse, not extrapolate: ${JSON.stringify(nps12)}`);

  // Unsupported class/rating also refuses by name (only one class per standard transcribed).
  const class300 = await lookup("reference", "flange_dims", { standard: "ASME B16.5", class: "300", size: "2" });
  if (class300 && class300.refused === true && /150/.test(class300.reason))
    pass("flange_dims(ASME B16.5 Class 300) REFUSES — only Class 150 transcribed");
  else fail(`flange_dims(Class 300) must refuse naming Class 150 as what's covered: ${JSON.stringify(class300)}`);
}

// ── (f) FUNNEL — discovery + dispatch ───────────────────────────────────────
console.log("(f) FUNNEL: find_tool surfaces kb_lookup; invoke dispatches it");
{
  const top5 = (q) => rankTools(table, q, undefined, 5).map((r) => r.name);

  const q1 = top5("clearance hole size for an M6 bolt");
  if (q1.includes("kb_lookup")) pass(`'clearance hole size for an M6 bolt' top-5 = [${q1}] includes kb_lookup`);
  else fail(`'clearance hole …' top-5 = [${q1}] missing kb_lookup`);

  const q2 = top5("tap drill size for a thread");
  if (q2.includes("kb_lookup")) pass(`'tap drill size for a thread' top-5 = [${q2}] includes kb_lookup`);
  else fail(`'tap drill …' top-5 = [${q2}] missing kb_lookup`);

  const q3 = top5("ISO fit tolerance for a bearing seat");
  if (q3.includes("kb_lookup")) pass(`'ISO fit tolerance …' top-5 = [${q3}] includes kb_lookup`);
  else fail(`'ISO fit tolerance …' top-5 = [${q3}] missing kb_lookup`);

  const invoke = table.get("invoke");
  const res = await invoke.handler({
    name: "kb_lookup",
    args: { kind: "reference", key: "clearance_hole", args: { fastener: "M6", class: "close" } },
  });
  const p = payloadOf(res);
  if (p && p.value && p.value.diameter_mm === 6.4 && /ISO 273/.test(p.source))
    pass("invoke('kb_lookup', …) dispatches through the funnel with the same payload");
  else fail(`invoke('kb_lookup') wrong: ${JSON.stringify(p)}`);
}

console.log(failures === 0 ? "\nPASS — kb_lookup invariants hold" : `\nFAIL — ${failures} problem(s)`);
process.exit(failures === 0 ? 0 : 1);
