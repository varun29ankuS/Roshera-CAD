/**
 * Tier-3 reference data for kb_lookup — engineering numbers as cited lookups,
 * never resident tables and never bare values (vault policy KB 2026-07-31 §5).
 *
 * The invariant this module enforces STRUCTURALLY (the same shape discipline
 * `DfmValue { value, derivation }` enforces in geometry-engine's dfm/report.rs,
 * extended from measured values to reference values): every answer is
 * `{ value, source }`, and every non-answer is a typed refusal naming its
 * reason. There is no code path that emits a number without a source string —
 * `answer()` is the only constructor and it demands one.
 *
 * Sourcing honesty (doc §9 / dfm/provenance.rs discipline): the ISO tables
 * below are transcriptions of the published standards' tables, not
 * clause-verified against a purchased edition — every ISO-sourced answer
 * carries that caveat in `source_note`. Items the doc marks [V] needs-Varun
 * or vendor-conflicting (house clearance class Q6, house tolerance class Q1,
 * K-factor by material, house stock list) REFUSE rather than default.
 */

export interface RefAnswer {
  value: Record<string, unknown>;
  source: string;
  source_note?: string;
}

export interface RefRefusal {
  refused: true;
  reason: string;
  open_question?: string;
  conflict?: Record<string, unknown>;
  valid_keys?: string[];
}

export type RefResult = RefAnswer | RefRefusal;

/** The ONLY answer constructor: a value cannot leave this module sourceless. */
function answer(
  value: Record<string, unknown>,
  source: string,
  source_note?: string,
): RefResult {
  if (source.trim().length === 0) {
    // A reference value that lost its citation is a bug, and the honest
    // rendering of that bug is a refusal — never a bare number.
    return refuse(
      "internal: this lookup produced a value without a source citation — refusing rather than emitting a bare number (no-number-without-provenance invariant)",
    );
  }
  return source_note ? { value, source, source_note } : { value, source };
}

function refuse(
  reason: string,
  extra?: Omit<RefRefusal, "refused" | "reason">,
): RefRefusal {
  return { refused: true, reason, ...(extra ?? {}) };
}

const TRANSCRIPTION_NOTE =
  "transcribed from the standard's published tables; not clause-verified against a purchased edition — confirm before it enters certified output (dfm/provenance.rs discipline)";

// ─── ISO 273 clearance holes: [fine(close), medium, coarse(free)] mm ────────

const ISO273: Record<string, [number, number, number]> = {
  "M1.6": [1.7, 1.8, 2.0],
  M2: [2.2, 2.4, 2.6],
  "M2.5": [2.7, 2.9, 3.1],
  M3: [3.2, 3.4, 3.6],
  M4: [4.3, 4.5, 4.8],
  M5: [5.3, 5.5, 5.8],
  M6: [6.4, 6.6, 7.0],
  M8: [8.4, 9.0, 10.0],
  M10: [10.5, 11.0, 12.0],
  M12: [13.0, 13.5, 14.5],
  M14: [15.0, 15.5, 16.5],
  M16: [17.0, 17.5, 18.5],
  M20: [21.0, 22.0, 24.0],
  M24: [25.0, 26.0, 28.0],
};

// class name → [column index, ISO series name]
const CLEARANCE_CLASS: Record<string, [number, string]> = {
  close: [0, "fine"],
  fine: [0, "fine"],
  medium: [1, "medium"],
  normal: [1, "medium"],
  free: [2, "coarse"],
  coarse: [2, "coarse"],
};

function clearanceHole(args: Record<string, unknown>): RefResult {
  const fastener = String(args.fastener ?? "").toUpperCase().replace(/\s+/g, "");
  if (!fastener) return refuse("clearance_hole needs args.fastener (e.g. 'M6')");
  const row = ISO273[fastener];
  if (!row) {
    return refuse(
      `fastener '${fastener}' is not in the transcribed ISO 273 table (M1.6–M24 metric coarse sizes) — a named gap, not a computed guess`,
      { valid_keys: Object.keys(ISO273) },
    );
  }
  const clsRaw = args.class;
  if (clsRaw === undefined || clsRaw === null || String(clsRaw).length === 0) {
    return refuse(
      "no house-wide clearance class exists — architecture doc §8-Q6 (close vs medium vs free) is OPEN, needs Varun; pass args.class explicitly per task rather than inheriting a silent default",
      { open_question: "architecture doc §8-Q6", valid_keys: Object.keys(CLEARANCE_CLASS) },
    );
  }
  const cls = CLEARANCE_CLASS[String(clsRaw).toLowerCase()];
  if (!cls) {
    return refuse(`unknown clearance class '${String(clsRaw)}'`, {
      valid_keys: Object.keys(CLEARANCE_CLASS),
    });
  }
  const [idx, series] = cls;
  return answer(
    { diameter_mm: row[idx], class: String(clsRaw).toLowerCase(), series },
    `ISO 273 ${series} series (requested as '${String(clsRaw)}'), ${fastener}`,
    TRANSCRIPTION_NOTE,
  );
}

// ─── Metric threads: ISO 261/262 coarse pitch + standard tap-drill chart ────

const COARSE_PITCH: Record<string, number> = {
  "M1.6": 0.35,
  M2: 0.4,
  "M2.5": 0.45,
  M3: 0.5,
  M4: 0.7,
  M5: 0.8,
  M6: 1.0,
  M8: 1.25,
  M10: 1.5,
  M12: 1.75,
  M14: 2.0,
  M16: 2.0,
  M20: 2.5,
  M24: 3.0,
};

/** Standard metric coarse tap-drill chart (≈75–77% thread engagement, d − p). */
const TAP_DRILL: Record<string, number> = {
  "M1.6": 1.25,
  M2: 1.6,
  "M2.5": 2.05,
  M3: 2.5,
  M4: 3.3,
  M5: 4.2,
  M6: 5.0,
  M8: 6.8,
  M10: 8.5,
  M12: 10.2,
  M14: 12.0,
  M16: 14.0,
  M20: 17.5,
  M24: 21.0,
};

interface ParsedThread {
  designation: string; // "M6"
  major_mm: number;
  pitch_mm: number;
  is_coarse: boolean;
}

function parseThread(raw: unknown): ParsedThread | RefRefusal {
  const s = String(raw ?? "").toUpperCase().replace(/\s+/g, "");
  const m = /^(M\d+(?:\.\d+)?)(?:X(\d+(?:\.\d+)?))?$/.exec(s);
  if (!m) {
    return refuse(
      `cannot parse thread '${String(raw)}' — expected metric designation like 'M6' or 'M6x1.0'`,
      { valid_keys: Object.keys(COARSE_PITCH) },
    );
  }
  const designation = m[1];
  const major = Number(designation.slice(1));
  const coarse = COARSE_PITCH[designation];
  if (coarse === undefined) {
    return refuse(
      `thread size '${designation}' is not in the transcribed ISO 261/262 coarse table (M1.6–M24) — a named gap`,
      { valid_keys: Object.keys(COARSE_PITCH) },
    );
  }
  const pitch = m[2] !== undefined ? Number(m[2]) : coarse;
  return {
    designation,
    major_mm: major,
    pitch_mm: pitch,
    is_coarse: Math.abs(pitch - coarse) < 1e-9,
  };
}

/**
 * Percent engagement per the Machinery's Handbook metric relation:
 * %E = 76.98 × (major − drill) / pitch  ⇔  drill = major − (%E/76.98)·pitch.
 */
const ENGAGEMENT_CONST = 76.98;

function tapDrill(args: Record<string, unknown>): RefResult {
  const t = parseThread(args.thread);
  if ("refused" in t) return t;

  const material = args.material !== undefined ? String(args.material) : undefined;
  const materialNote = material
    ? ` Vendor guidance on engagement-by-material DIVERGES (softer materials ~75–80%, harder ~60–70%) — surfaced, not resolved: material '${material}' did not change this number; choose engagement_pct deliberately.`
    : "";

  const pctRaw = args.engagement_pct;
  const pct = pctRaw === undefined ? 75 : Number(pctRaw);
  if (!Number.isFinite(pct) || pct <= 0 || pct > 100) {
    return refuse(`engagement_pct must be in (0, 100], got ${String(pctRaw)}`);
  }

  const chart = t.is_coarse ? TAP_DRILL[t.designation] : undefined;
  if (chart !== undefined && pct >= 70 && pct <= 80) {
    const actualPct =
      Math.round(((ENGAGEMENT_CONST * (t.major_mm - chart)) / t.pitch_mm) * 10) / 10;
    return answer(
      { diameter_mm: chart, percent_thread: actualPct, thread: `${t.designation}x${t.pitch_mm}` },
      `standard metric coarse tap-drill chart (d − p convention), ${t.designation}x${t.pitch_mm}; chart value gives ${actualPct}% engagement (Machinery's Handbook relation %E = 76.98·(D−drill)/P).${materialNote}`,
      TRANSCRIPTION_NOTE,
    );
  }

  // Off-chart request (fine pitch, or a deliberate non-standard engagement):
  // compute from the published relation and bracket with real drills.
  const exact = t.major_mm - (pct / ENGAGEMENT_CONST) * t.pitch_mm;
  if (exact <= 0) return refuse(`engagement ${pct}% is not achievable for ${t.designation}x${t.pitch_mm}`);
  const rounded = Math.round(exact * 100) / 100;
  return answer(
    {
      diameter_mm: rounded,
      percent_thread: pct,
      thread: `${t.designation}x${t.pitch_mm}`,
      note: "computed, not a chart row — pick the nearest standard drill via drill_size and re-check the resulting engagement",
    },
    `computed from the Machinery's Handbook metric relation drill = D − (%E/76.98)·P at caller-requested ${pct}% engagement, ${t.designation}x${t.pitch_mm}.${materialNote}`,
  );
}

// ─── ISO 2768-1 general tolerances (Table 1, linear dimensions) ─────────────

// [over, up_to_incl, f, m, c, v] — null = class not defined for the range.
const ISO2768_LINEAR: Array<[number, number, number | null, number | null, number | null, number | null]> = [
  [0.5, 3, 0.05, 0.1, 0.2, null],
  [3, 6, 0.05, 0.1, 0.3, 0.5],
  [6, 30, 0.1, 0.2, 0.5, 1.0],
  [30, 120, 0.15, 0.3, 0.8, 1.5],
  [120, 400, 0.2, 0.5, 1.2, 2.5],
  [400, 1000, 0.3, 0.8, 2.0, 4.0],
  [1000, 2000, 0.5, 1.2, 3.0, 6.0],
  [2000, 4000, null, 2.0, 4.0, 8.0],
];

const TOL_CLASS_COL: Record<string, number> = { f: 2, m: 3, c: 4, v: 5 };

function generalTolerance(args: Record<string, unknown>): RefResult {
  const nominal = Number(args.nominal_mm);
  if (!Number.isFinite(nominal) || nominal <= 0) {
    return refuse("general_tolerance needs a positive args.nominal_mm");
  }
  const clsRaw = args.class;
  if (clsRaw === undefined || clsRaw === null || String(clsRaw).length === 0) {
    return refuse(
      "no house-wide ISO 2768 class exists — architecture doc §8-Q1 (f/m/c/v) is OPEN, needs Varun; pass args.class explicitly per task rather than inheriting a silent default",
      { open_question: "architecture doc §8-Q1", valid_keys: Object.keys(TOL_CLASS_COL) },
    );
  }
  const cls = String(clsRaw).toLowerCase();
  const col = TOL_CLASS_COL[cls];
  if (col === undefined) {
    return refuse(`unknown ISO 2768 class '${String(clsRaw)}'`, {
      valid_keys: Object.keys(TOL_CLASS_COL),
    });
  }
  if (nominal < 0.5) {
    return refuse(
      "ISO 2768-1 assigns NO general tolerance below 0.5 mm — the standard requires an individually indicated tolerance there; this is the standard's own refusal, not a gap in the table",
    );
  }
  const row = ISO2768_LINEAR.find(([over, upto]) => nominal > over && nominal <= upto)
    ?? (nominal <= 3 ? ISO2768_LINEAR[0] : undefined);
  if (!row) {
    return refuse(`nominal ${nominal} mm exceeds ISO 2768-1 Table 1's 4000 mm upper bound`);
  }
  const v = row[col] as number | null;
  if (v === null) {
    return refuse(
      `ISO 2768-1 defines no class-${cls} value for the ${row[0]}–${row[1]} mm range — the standard's own gap, stated rather than interpolated`,
    );
  }
  return answer(
    { plus_minus_mm: v, class: cls, range_mm: [row[0], row[1]] },
    `ISO 2768-1 Table 1 (linear dimensions), class ${cls}, over ${row[0]} up to ${row[1]} mm`,
    TRANSCRIPTION_NOTE,
  );
}

// ─── ISO 286 fits (basic-hole system, common shaft letters, IT5–IT11) ───────

// Main size-range upper bounds (mm); ranges are (prev, bound].
const ISO286_BOUNDS = [3, 6, 10, 18, 30, 50, 80, 120, 180, 250, 315, 400, 500];

// Standard tolerance IT grades (µm), ISO 286-1, rows aligned to ISO286_BOUNDS.
const IT_GRADES: Record<number, number[]> = {
  5: [4, 5, 6, 8, 9, 11, 13, 15, 18, 20, 23, 25, 27],
  6: [6, 8, 9, 11, 13, 16, 19, 22, 25, 29, 32, 36, 40],
  7: [10, 12, 15, 18, 21, 25, 30, 35, 40, 46, 52, 57, 63],
  8: [14, 18, 22, 27, 33, 39, 46, 54, 63, 72, 81, 89, 97],
  9: [25, 30, 36, 43, 52, 62, 74, 87, 100, 115, 130, 140, 155],
  10: [40, 48, 58, 70, 84, 100, 120, 140, 160, 185, 210, 230, 250],
  11: [60, 75, 90, 110, 130, 160, 190, 220, 250, 290, 320, 360, 400],
};

// Fundamental deviations (µm), rows aligned to ISO286_BOUNDS.
// a–h carry es (upper deviation, ≤0); k–s carry ei (lower deviation, ≥0).
const SHAFT_ES_NEG: Record<string, number[]> = {
  d: [20, 30, 40, 50, 65, 80, 100, 120, 145, 170, 190, 210, 230],
  e: [14, 20, 25, 32, 40, 50, 60, 72, 85, 100, 110, 125, 135],
  f: [6, 10, 13, 16, 20, 25, 30, 36, 43, 50, 56, 62, 68],
  g: [2, 4, 5, 6, 7, 9, 10, 12, 14, 15, 17, 18, 20],
  h: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
};
const SHAFT_EI_POS: Record<string, number[]> = {
  k: [0, 1, 1, 1, 2, 2, 2, 3, 3, 4, 4, 4, 5], // IT4–IT7 only; 0 outside
  m: [2, 4, 6, 7, 8, 9, 11, 13, 15, 17, 20, 21, 23],
  n: [4, 8, 10, 12, 15, 17, 20, 23, 27, 31, 34, 37, 40],
  p: [6, 12, 15, 18, 22, 26, 32, 37, 43, 50, 56, 62, 68],
};
// s splits sub-ranges above 50 mm: [up_to_incl, ei_µm].
const SHAFT_S_EI: Array<[number, number]> = [
  [3, 14], [6, 19], [10, 23], [18, 28], [30, 35], [50, 43],
  [65, 53], [80, 59], [100, 71], [120, 79], [140, 92], [160, 100],
  [180, 108], [200, 122], [225, 130], [250, 140], [280, 158], [315, 170],
  [355, 190], [400, 208], [450, 232], [500, 252],
];

function iso286Index(nominal: number): number | null {
  for (let i = 0; i < ISO286_BOUNDS.length; i++) {
    const lower = i === 0 ? 0 : ISO286_BOUNDS[i - 1];
    if (nominal > lower && nominal <= ISO286_BOUNDS[i]) return i;
  }
  return null;
}

function fitClass(args: Record<string, unknown>): RefResult {
  const nominal = Number(args.nominal_mm);
  if (!Number.isFinite(nominal) || nominal <= 0) {
    return refuse("fit_class needs a positive args.nominal_mm");
  }
  const fitRaw = String(args.fit ?? "").replace(/\s+/g, "");
  const m = /^([A-Za-z]{1,2})(\d{1,2})\/([A-Za-z]{1,2})(\d{1,2})$/.exec(fitRaw);
  if (!m) {
    return refuse(
      `cannot parse fit '${String(args.fit)}' — expected hole/shaft form like 'H7/g6'`,
    );
  }
  const [, holeLetter, holeGradeS, shaftLetterRaw, shaftGradeS] = m;
  if (holeLetter !== "H") {
    return refuse(
      `only the basic-hole (H) system is transcribed — hole deviations other than H are a named gap, not computed (asked for '${holeLetter}')`,
    );
  }
  const holeGrade = Number(holeGradeS);
  const shaftGrade = Number(shaftGradeS);
  const shaftLetter = shaftLetterRaw.toLowerCase();
  if (shaftLetterRaw !== shaftLetter) {
    return refuse(
      `'${shaftLetterRaw}' is an uppercase (hole) letter in shaft position — shaft-basis fits are not transcribed; use basic-hole form like 'H7/${shaftLetter}${shaftGradeS}'`,
    );
  }
  const idx = iso286Index(nominal);
  if (idx === null) {
    return refuse(`nominal ${nominal} mm is outside the transcribed ISO 286 range (0, 500]`);
  }
  const itHole = IT_GRADES[holeGrade]?.[idx];
  const itShaft = IT_GRADES[shaftGrade]?.[idx];
  if (itHole === undefined || itShaft === undefined) {
    return refuse(
      `IT grade ${itHole === undefined ? holeGrade : shaftGrade} is outside the transcribed IT5–IT11 table — a named gap`,
      { valid_keys: Object.keys(IT_GRADES).map((g) => "IT" + g) },
    );
  }

  let ei: number;
  let es: number;
  if (shaftLetter in SHAFT_ES_NEG) {
    es = -SHAFT_ES_NEG[shaftLetter][idx];
    ei = es - itShaft;
  } else if (shaftLetter === "js") {
    es = itShaft / 2;
    ei = -itShaft / 2;
  } else if (shaftLetter === "k") {
    ei = shaftGrade >= 4 && shaftGrade <= 7 ? SHAFT_EI_POS.k[idx] : 0;
    es = ei + itShaft;
  } else if (shaftLetter in SHAFT_EI_POS) {
    ei = SHAFT_EI_POS[shaftLetter][idx];
    es = ei + itShaft;
  } else if (shaftLetter === "s") {
    const row = SHAFT_S_EI.find(([upto], i2) => {
      const lower = i2 === 0 ? 0 : SHAFT_S_EI[i2 - 1][0];
      return nominal > lower && nominal <= upto;
    });
    if (!row) return refuse(`nominal ${nominal} mm outside the transcribed s-deviation table`);
    ei = row[1];
    es = ei + itShaft;
  } else {
    return refuse(
      `shaft deviation '${shaftLetter}' is not in the transcribed set — a named gap, not computed`,
      { valid_keys: [...Object.keys(SHAFT_ES_NEG), "js", ...Object.keys(SHAFT_EI_POS), "s"] },
    );
  }

  const clearanceMin = 0 - es; // hole lower bound is 0 in the H system
  const clearanceMax = itHole - ei;
  const character =
    clearanceMin >= 0 ? "clearance" : clearanceMax <= 0 ? "interference" : "transition";
  return answer(
    {
      hole_tol_um: [0, itHole],
      shaft_tol_um: [ei, es],
      clearance_um: [clearanceMin, clearanceMax],
      character,
      fit: `H${holeGrade}/${shaftLetter}${shaftGrade}`,
      nominal_mm: nominal,
    },
    `ISO 286-1/-2 (IT grades + fundamental deviations), H${holeGrade}/${shaftLetter}${shaftGrade} at ${nominal} mm`,
    TRANSCRIPTION_NOTE,
  );
}

// ─── thread_spec: one record over pitch + tap drill + clearance ─────────────

function threadSpec(args: Record<string, unknown>): RefResult {
  const t = parseThread(args.thread);
  if ("refused" in t) return t;
  if (!t.is_coarse) {
    return refuse(
      `only the ISO 262 coarse-pitch series is transcribed — '${t.designation}x${t.pitch_mm}' is a fine pitch, a named gap (use tap_drill with the explicit pitch for the drill size alone)`,
      { valid_keys: Object.keys(COARSE_PITCH) },
    );
  }
  const clearance = ISO273[t.designation];
  const tap = TAP_DRILL[t.designation];
  if (!clearance || tap === undefined) {
    return refuse(`'${t.designation}' missing from a transcribed table — a named gap`, {
      valid_keys: Object.keys(COARSE_PITCH),
    });
  }
  return answer(
    {
      thread: `${t.designation}x${t.pitch_mm}`,
      major_mm: t.major_mm,
      pitch_mm: t.pitch_mm,
      tap_drill_mm: tap,
      clearance_mm: { close: clearance[0], medium: clearance[1], free: clearance[2] },
    },
    `ISO 261/262 coarse-pitch series (${t.designation}x${t.pitch_mm}); tap drill from the standard d − p chart; clearance from ISO 273 fine/medium/coarse series`,
    TRANSCRIPTION_NOTE,
  );
}

// ─── standard_stock: [V] — vendor/region dependent, refuses pending Varun ───

function standardStock(): RefResult {
  return refuse(
    "standard stock (sheet gauges, bar diameters) is vendor/region dependent and no house supplier list is on file — needs Varun; shipping a generic list would be exactly the invented house default the policy KB forbids (doc §5.2)",
    { open_question: "house stock list [V]" },
  );
}

// ─── bend_allowance: K-factor conflict surfaced, computed only on explicit K ─

function bendAllowance(args: Record<string, unknown>): RefResult {
  const t = Number(args.thickness_mm);
  const angle = Number(args.angle_deg);
  const r = Number(args.radius_mm);
  if (!Number.isFinite(t) || t <= 0) return refuse("bend_allowance needs a positive args.thickness_mm");
  if (!Number.isFinite(angle) || angle <= 0 || angle > 180) {
    return refuse("bend_allowance needs args.angle_deg in (0, 180]");
  }
  if (!Number.isFinite(r) || r < 0) return refuse("bend_allowance needs args.radius_mm >= 0");

  const kRaw = args.k_factor;
  if (kRaw === undefined || kRaw === null) {
    return refuse(
      "no K-factor: published values DIVERGE by material and process (0.33–0.5, doc §3.7 — 'never a blanket constant') and no house per-material table is on file. Re-call with an explicit args.k_factor chosen against a material spec (record the choice in the Blackboard); this lookup will not pick one for you",
      {
        conflict: {
          k_factor_range: [0.33, 0.5],
          basis: "vendor/published sheet-metal guidance, material- and process-dependent [P]",
        },
        open_question: "per-material K-factor table [V] — needs a material spec or Varun",
      },
    );
  }
  const k = Number(kRaw);
  if (!Number.isFinite(k) || k <= 0 || k >= 1) {
    return refuse(`k_factor must be in (0, 1), got ${String(kRaw)}`);
  }
  const theta = (angle * Math.PI) / 180;
  const ba = theta * (r + k * t);
  return answer(
    {
      bend_allowance_mm: ba,
      k_factor: k,
      formula: "BA = theta_rad * (R + K*t)",
      inputs: { thickness_mm: t, angle_deg: angle, radius_mm: r },
      ...(args.material !== undefined ? { material: String(args.material) } : {}),
    },
    `computed via the standard bend-allowance relation BA = θ·(R + K·t) with CALLER-SUPPLIED K = ${k} — the K-factor choice is the caller's, made against a material spec, not a house default`,
  );
}

// ─── drill_size: metric twist-drill index ───────────────────────────────────

const METRIC_DRILLS: number[] = (() => {
  const out: number[] = [];
  for (let d = 10; d <= 130; d++) out.push(d / 10); // 1.0 → 13.0 by 0.1
  for (let d = 135; d <= 250; d += 5) out.push(d / 10); // 13.5 → 25.0 by 0.5
  return out;
})();

function drillSize(args: Record<string, unknown>): RefResult {
  const series = String(args.series ?? "metric").toLowerCase();
  if (series !== "metric") {
    return refuse(
      `only the metric drill series is transcribed — '${series}' (number/letter/fractional-inch) is a named gap, not a converted guess`,
      { valid_keys: ["metric"] },
    );
  }
  const d = Number(args.diameter_mm);
  if (!Number.isFinite(d) || d <= 0) return refuse("drill_size needs a positive args.diameter_mm");
  if (d < 1.0) {
    return refuse(
      "below 1.0 mm the stocked granularity is finer (0.05 mm and micro sizes) and is not transcribed — a named gap",
    );
  }
  if (d > 25.0) {
    return refuse("above 25.0 mm is outside the transcribed metric drill index — a named gap");
  }
  let under = METRIC_DRILLS[0];
  let over = METRIC_DRILLS[METRIC_DRILLS.length - 1];
  for (const s of METRIC_DRILLS) {
    if (s <= d + 1e-9) under = s;
    if (s >= d - 1e-9) {
      over = s;
      break;
    }
  }
  return answer(
    {
      nearest_under_mm: under,
      nearest_over_mm: over,
      exact_match: Math.abs(under - over) < 1e-9,
      requested_mm: d,
    },
    "metric twist-drill series (0.1 mm steps 1.0–13.0 mm, 0.5 mm steps 13.5–25.0 mm — common metric stock granularity)",
    "series granularity is shop convention, not a standard's clause; number/letter/fractional series untranscribed",
  );
}

// ─── Dispatch ───────────────────────────────────────────────────────────────

const REFERENCE_FNS: Record<string, (args: Record<string, unknown>) => RefResult> = {
  clearance_hole: clearanceHole,
  tap_drill: tapDrill,
  general_tolerance: generalTolerance,
  fit_class: fitClass,
  thread_spec: threadSpec,
  standard_stock: standardStock,
  bend_allowance: bendAllowance,
  drill_size: drillSize,
};

export const REFERENCE_KEYS = Object.keys(REFERENCE_FNS);

export function referenceLookup(key: string, args: Record<string, unknown>): RefResult {
  const fn = REFERENCE_FNS[key];
  if (!fn) {
    return refuse(
      `unknown reference key '${key}' — no such data function; the valid keys are listed (asking for data outside them is a question for Varun, not a guess)`,
      { valid_keys: REFERENCE_KEYS },
    );
  }
  return fn(args);
}
