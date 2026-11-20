/**
 * Drawing comprehension tools (campaign #55) — the agent's CERTIFIED readback
 * surface over a Roshera engineering sheet.
 *
 * `drawing_read_semantics` returns the queryable semantic sheet + its live
 * certificate; `drawing_query` answers ONE typed, scoped question (toleranced diameter, FCF
 * datum, what SECTION A-A cuts through, dimension/hole/entity-at) with
 * provenance + a live-check verdict. Every answer is certified against the LIVE
 * model — never pixel inference — and honest-refuses (render_only /
 * unprovenanced) rather than fabricate.
 */

import type { ToolHost } from "../registry.js";
import { z } from "zod";
import {
  api,
  apiBytes,
  ok,
  fail,
  imageFreshness,
  lastGlobalMutationTurnValue,
  recordImageSent,
} from "../core.js";

/** One space-map finding as a line: severity, rule, message, measured vs tolerance. */
function findingLine(f: any): string {
  const t = f?.tolerance ?? {};
  const range =
    t.min != null && t.max != null
      ? `${t.min}..${t.max}`
      : t.min != null
        ? `>= ${t.min}`
        : t.max != null
          ? `<= ${t.max}`
          : "";
  const m = typeof f?.measured === "number" ? +f.measured.toFixed(3) : f?.measured;
  return `${f?.severity === "error" ? "ERROR" : "warn"} ${f?.rule}: ${f?.message} [${m} ${f?.unit ?? ""} vs ${range}]`;
}

/** A space's findings as lines, errors before warnings. */
function findingLines(s: any): string[] {
  const fs: any[] = Array.isArray(s?.findings) ? s.findings : [];
  return [...fs]
    .sort((a, b) => (a?.severity === "error" ? 0 : 1) - (b?.severity === "error" ? 0 : 1))
    .map(findingLine);
}

/** One layout finding as a stable line: severity, view and message. */
function issueLine(i: any): string {
  const view = i?.view ? ` [${i.view}]` : "";
  return `${i?.severity ?? "?"}${view}: ${i?.message ?? i?.kind ?? "issue"}`;
}

/** The sheet's layout findings as lines, or null when the check cannot be read. */
async function layoutLines(drawingId: string): Promise<{ lines: string[]; passed: boolean | null } | null> {
  try {
    const q = await api("GET", `/api/drawings/${drawingId}/quality`);
    const issues: any[] = Array.isArray(q?.issues) ? q.issues : [];
    return { lines: issues.map(issueLine), passed: typeof q?.passed === "boolean" ? q.passed : null };
  } catch {
    return null;
  }
}

/**
 * Run a sheet edit and report what it did to the layout (2026-11-19): the
 * agent set the bracket's bores to H7, the longer label collided with R25,
 * and the edit's answer said only "sound" -- the export gate caught what the
 * agent never saw. Every sheet-editing tool now answers with the issues its
 * edit CREATED and RESOLVED and whether the sheet still passes, read from the
 * same layout check the export gate uses. `null` when that check cannot be
 * read -- never a guessed "clean".
 */
async function withSheetEffect<T>(
  drawingId: string,
  edit: () => Promise<T>,
): Promise<{ result: T; sheet_effect: any }> {
  const before = await layoutLines(drawingId);
  const result = await edit();
  const after = await layoutLines(drawingId);
  if (before === null || after === null) {
    return { result, sheet_effect: null };
  }
  const was = new Set(before.lines);
  const now = new Set(after.lines);
  const created = after.lines.filter((l) => !was.has(l));
  const resolved = before.lines.filter((l) => !now.has(l));
  return {
    result,
    sheet_effect: {
      layout_passes: after.passed,
      new_issues: created,
      resolved_issues: resolved,
      open_issues: after.lines.length,
      note:
        created.length > 0
          ? "this edit created layout issues: fix them (move_mm, or tell the engineer) before going on"
          : "no new layout issues",
    },
  };
}

/** Compact one-line verdict tally for a sheet certificate. */
function certificateLine(cert: any): string {
  const c = cert?.counts ?? {};
  const sound = cert?.sound === true;
  const parts = [
    `${c.consistent ?? 0} consistent`,
    c.stale ? `${c.stale} STALE` : null,
    c.dangling ? `${c.dangling} DANGLING` : null,
    c.render_only ? `${c.render_only} render_only` : null,
    c.unprovenanced ? `${c.unprovenanced} unprovenanced` : null,
    c.omitted ? `${c.omitted} OMITTED` : null,
    c.not_dimensioned ? `${c.not_dimensioned} NOT DIMENSIONED` : null,
    c.waived ? `${c.waived} waived` : null,
  ].filter(Boolean);
  const quality = cert?.quality?.passed === false ? " | LAYOUT ISSUES" : "";
  // Sound = what the sheet shows is true; complete = it shows everything.
  const complete = cert?.complete === false ? " | INCOMPLETE" : "";
  return `${sound ? "SOUND ✓" : "UNSOUND ✗"} ${parts.join(", ")}${complete}${quality}`;
}

/** Render a section cut-through list compactly. */
function sectionLine(ct: any): string {
  const cuts: any[] = ct?.cuts ?? [];
  if (cuts.length === 0) return "SECTION A-A cuts through: (nothing)";
  const items = cuts.map((c) => {
    const tag = c.hole_tag ? ` ${c.hole_tag}` : "";
    const span =
      Array.isArray(c.span) && c.span.length === 2
        ? ` [${c.span[0].toFixed(1)}..${c.span[1].toFixed(1)}]`
        : "";
    return `${c.kind}${tag}${span}`;
  });
  return `SECTION A-A cuts through (in order): ${items.join(" → ")}`;
}

/** Render one typed query answer as a compact, honest line. */
function answerLine(a: any): string {
  switch (a?.answer) {
    case "toleranced_diameter": {
      const v = a.value?.toFixed?.(3) ?? a.value;
      let tol: string;
      if (a.limits) {
        tol = `limits [${a.limits[0].toFixed(3)}, ${a.limits[1].toFixed(3)}]`;
      } else if (a.designation) {
        tol = `fit ${a.designation} (numeric envelope not resolved — no fabricated limits)`;
      } else if (a.tolerance_source === "general") {
        // Under an ISO 2768 class the value is by size; null = the standard
        // assigns none at this size (below 0.5 mm / above 4000 mm).
        const std = a.general_standard ? ` (${a.general_standard})` : "";
        tol = a.general_pm_mm != null
          ? `general ±${a.general_pm_mm}${std}`
          : `general${std}: the standard assigns no value at this size`;
      } else {
        tol = "no tolerance";
      }
      const meas = a.measured != null ? `, measured ${a.measured.toFixed?.(3) ?? a.measured}` : "";
      return `${a.label}: nominal ${v} ${a.unit}, ${tol} [${a.verdict}${meas}] (source: ${a.tolerance_source})`;
    }
    case "fcf": {
      const datums = (a.datums ?? [])
        .map((d: any) => `${d.label}(${d.status})`)
        .join(" ");
      const dstr = datums ? ` → datums: ${datums}` : " (no datum)";
      return `FCF #${a.index} ${a.characteristic_glyph} ${a.tolerance_text}${dstr} [${a.verdict}]`;
    }
    case "section_cuts":
      return sectionLine(a);
    case "dimensions": {
      const facts: any[] = a.facts ?? [];
      return facts
        .map((f) => `${f.label} = ${f.value ?? "?"} ${f.unit} [${f.live?.verdict}]`)
        .join("\n");
    }
    case "hole": {
      const f = a.fact ?? {};
      const t = a.tolerance;
      const tol = t
        ? t.limits
          ? ` limits [${t.limits[0]}, ${t.limits[1]}]`
          : t.designation
            ? ` fit ${t.designation}`
            : ""
        : "";
      return `${f.label} [${f.live?.verdict}]${tol}`;
    }
    case "entity_at":
      return `${a.role}${a.label ? ` "${a.label}"` : ""}${
        a.face_ids?.length ? ` faces=[${a.face_ids.join(",")}]` : ""
      }${a.pid ? ` pid=${a.pid}` : ""}`;
    case "refused":
      return `REFUSED: ${a.reason} (${a.refusal})`;
    default:
      return JSON.stringify(a);
  }
}

export function registerDrawingTools(server: ToolHost): void {
  // ── drawing_set_dimension ──────────────────────────────────────────────
  server.tool(
    "drawing_set_dimension",
    "EDIT THE PART FROM ITS DRAWING: set a sheet dimension (id from " +
      "drawing_read_semantics) to a new size. The one recorded parameter that " +
      "drives it is found by measurement, changed, and the part and sheet " +
      "rebuild. Refused typed when none drives it (imported parts), several do, " +
      "or the size is unreachable.",
    {
      drawing_id: z.string().uuid().describe("drawing_id from make_drawing"),
      dimension_id: z.string().describe("dimension id from drawing_read_semantics"),
      value: z.number().describe("new size (mm; degrees for an angle)"),
    },
    async ({ drawing_id, dimension_id, value }) => {
      try {
        const { result: r, sheet_effect } = await withSheetEffect(drawing_id, () =>
          api(
            "POST",
            `/api/drawings/${drawing_id}/dimensions/${encodeURIComponent(dimension_id)}`,
            { value },
            180_000,
          ),
        );
        return ok({
          drawing_id,
          dimension: r?.dimension ?? null,
          driver: r?.driver ?? null,
          sound: r?.mould?.is_sound ?? null,
          sheet_effect,
        });
      } catch (e) {
        return fail(e);
      }
    },
  );

  // ── drawing_dimensions ─────────────────────────────────────────────────
  server.tool(
    "drawing_dimensions",
    "List a sheet's dimensions AS INKED (value, tolerance, authored text) with " +
      "the id each is addressed by, and the dimensions the sheet LEAVES OUT " +
      "(thinned by the automatic selection, not drawn, or suppressed by a named " +
      "person). complete=false means a model dimension is on no view and nobody " +
      "signed for it.",
    {
      drawing_id: z.string().uuid().describe("drawing_id from make_drawing"),
    },
    async ({ drawing_id }) => {
      try {
        const r = await api("GET", `/api/drawings/${drawing_id}/dimensions`);
        const dims: any[] = r?.dimensions ?? [];
        return ok({
          drawing_id,
          dimensions: dims.map((d) => ({
            id: d.id,
            view: d.view,
            kind: d.kind,
            ink: d.ink,
            value: d.value,
            editable: d.editable,
            placed: d.placed,
            suppressed: d.suppressed,
            edit: d.edit ?? null,
          })),
          complete: r?.certificate?.complete ?? null,
          sound: r?.certificate?.sound ?? null,
          omission_summary: r?.certificate?.omission_summary ?? [],
          omissions: (r?.certificate?.omissions ?? []).map((o: any) => ({
            key: o.key,
            view: o.view,
            kind: o.kind,
            label: o.label,
            reason: o.reason,
            waived_by: o.waiver?.by ?? null,
          })),
        });
      } catch (e) {
        return fail(e);
      }
    },
  );

  // ── drawing_dimension_properties ───────────────────────────────────────
  server.tool(
    "drawing_dimension_properties",
    "SET THE PROPERTIES OF ONE SHEET DIMENSION (id from drawing_dimensions): its " +
      "TOLERANCE (symmetric, bilateral, limits, ISO 286 class such as H7 or g6, " +
      "basic, reference), text before/after the value ('4X', 'TYP', " +
      "'M12x1.75-6H'), move it, or SUPPRESS it (signed with `by`; the " +
      "certificate lists it as a waived omission). The value itself is measured " +
      "and cannot be typed (drawing_set_dimension changes the part). Recorded " +
      "in the timeline; undoable. Refused typed for a class outside the ISO 286 " +
      "tables or a dimension without a persistent identity.",
    {
      drawing_id: z.string().uuid().describe("drawing_id from make_drawing"),
      dimension_id: z
        .string()
        .describe("dimension id from drawing_dimensions (or the key of an omitted dimension, to waive it)"),
      tolerance: z
        .enum(["symmetric", "bilateral", "limits", "fit", "basic", "reference", "none"])
        .optional()
        .describe("tolerance form; 'none' removes the tolerance"),
      pm: z.number().optional().describe("symmetric: the +/- value"),
      upper: z
        .number()
        .optional()
        .describe("bilateral: signed upper deviation; limits: upper limit of size"),
      lower: z
        .number()
        .optional()
        .describe("bilateral: signed lower deviation; limits: lower limit of size"),
      fit: z.string().optional().describe("fit: one ISO 286 class, e.g. 'H7' (hole) or 'g6' (shaft)"),
      prefix: z.string().max(40).optional().describe("text before the value, e.g. '4X'; '' clears"),
      suffix: z
        .string()
        .max(40)
        .optional()
        .describe("text after the value, e.g. 'TYP' or 'M12x1.75-6H'; '' clears"),
      move_mm: z
        .array(z.number())
        .length(2)
        .optional()
        .describe("move the text / dimension line [right, down] in sheet mm from its automatic place; [0,0] puts it back"),
      suppress: z.boolean().optional().describe("true takes it off the sheet (needs by); false puts it back"),
      by: z.string().optional().describe("who suppresses it (required with suppress=true)"),
      reason: z.string().optional().describe("why it is suppressed"),
    },
    async (a) => {
      try {
        const body: Record<string, unknown> = {};
        if (a.tolerance !== undefined) {
          switch (a.tolerance) {
            case "none":
              body.tolerance = null;
              break;
            case "symmetric":
              body.tolerance = { type: "symmetric", pm: a.pm };
              break;
            case "bilateral":
              body.tolerance = { type: "bilateral", upper: a.upper, lower: a.lower };
              break;
            case "limits":
              body.tolerance = { type: "limits", upper: a.upper, lower: a.lower };
              break;
            case "fit":
              body.tolerance = { type: "fit", designation: a.fit };
              break;
            default:
              body.tolerance = { type: a.tolerance };
          }
        }
        if (a.prefix !== undefined) body.prefix = a.prefix;
        if (a.suffix !== undefined) body.suffix = a.suffix;
        if (a.move_mm !== undefined) body.text_shift_mm = a.move_mm;
        if (a.suppress !== undefined) {
          body.suppressed = a.suppress ? { by: a.by ?? "", reason: a.reason ?? "" } : null;
        }
        const { result: r, sheet_effect } = await withSheetEffect(a.drawing_id, () =>
          api(
            "PATCH",
            `/api/drawings/${a.drawing_id}/dimensions/${encodeURIComponent(a.dimension_id)}`,
            body,
          ),
        );
        return ok({
          drawing_id: a.drawing_id,
          dimension: r?.dimension ?? null,
          sound: r?.certificate?.sound ?? null,
          complete: r?.certificate?.complete ?? null,
          omission_summary: r?.certificate?.omission_summary ?? [],
          sheet_effect,
        });
      } catch (e) {
        return fail(e);
      }
    },
  );

  // ── drawing_spaces ─────────────────────────────────────────────────────
  server.tool(
    "drawing_spaces",
    "SEE A SHEET REGION BY REGION: title block, notes, tables, each view " +
      "cell with its four annotation bands and their dimension rows (evenly " +
      "pitched? aligned?). Each region has a stable id, its outline in mm, " +
      "what it holds, its drafting-craft findings (row alignment and pitch, " +
      "text clearance and height, crowding, unused paper, table anchoring, " +
      "view alignment, leader angles, trailing zeros: measured vs tolerance, " +
      "severity) and a score. grade = the WORST region, never a mean; " +
      "below_bar counts regions needing work; worst first. `space` narrows " +
      "to one region and those under it.",
    {
      drawing_id: z.string().uuid().describe("drawing_id from make_drawing"),
      space: z
        .string()
        .optional()
        .describe("one space id (e.g. 'view:FRONT/band:top') to narrow to it and the spaces under it"),
    },
    async ({ drawing_id, space }) => {
      try {
        const q = space ? `?space=${encodeURIComponent(space)}` : "";
        const r = await api("GET", `/api/drawings/${drawing_id}/spaces${q}`);
        const spaces: any[] = Array.isArray(r?.spaces) ? r.spaces : [];
        const worst = spaces
          .filter((s) => typeof s?.score === "number" && s.score < 1)
          .sort((a, b) => a.score - b.score || String(a.id).localeCompare(String(b.id)));
        const finding = findingLine;
        // The text the agent reads stays compact: the worst spaces, each with
        // its first findings; the whole map rides in structuredContent only.
        const SHOWN_SPACES = 12;
        const SHOWN_FINDINGS = 5;
        const summary = worst.slice(0, SHOWN_SPACES).map((s) => {
          const lines: string[] = (s.findings ?? []).map(finding);
          return {
            space: s.id,
            kind: s.kind,
            score: +Number(s.score).toFixed(3),
            below_bar: s.score < (r?.bar ?? 0.8),
            findings:
              lines.length > SHOWN_FINDINGS
                ? [...lines.slice(0, SHOWN_FINDINGS), `+${lines.length - SHOWN_FINDINGS} more`]
                : lines,
          };
        });
        const grade = typeof r?.grade === "number" ? +r.grade.toFixed(3) : null;
        const compact = {
          drawing_id,
          focus: r?.focus ?? null,
          grade,
          bar: r?.bar ?? null,
          below_bar: r?.below_bar ?? null,
          worst: r?.worst ?? null,
          by_rule: r?.by_rule ?? {},
          headline:
            `grade ${grade ?? "?"} (worst space ${r?.worst ?? "none"}), ` +
            `${r?.below_bar ?? "?"} of ${spaces.length} spaces below the bar ${r?.bar ?? "?"}`,
          worst_spaces: summary,
          more_spaces_with_findings: Math.max(0, worst.length - SHOWN_SPACES),
          clean_spaces: spaces.length - worst.length,
          note: "the whole map (every space's box, contents and findings) is in structuredContent.map; narrow with `space`",
        };
        return {
          content: [{ type: "text" as const, text: JSON.stringify(compact, null, 2) }],
          structuredContent: { ...compact, map: r },
        };
      } catch (e) {
        return fail(e);
      }
    },
  );

  // ── drawing_look ───────────────────────────────────────────────────────
  // The agent's EYES on a sheet (agent eyes slice 2): GET
  // /api/drawings/{id}/render.png as MCP image content, with the region's
  // findings read from /spaces beside it. The route itself shows the look in
  // the person's open app (an AgentShow sheet_space frame, sent because this
  // call carries X-Roshera-Agent), so the agent never has to remember to.
  server.tool(
    "drawing_look",
    "LOOK AT A SHEET as a picture: render the drawing as a PNG image and see it -- the " +
      "whole sheet, or one region (an id from drawing_spaces) cropped with a " +
      "margin, outlined in magenta, the texts its findings name tinted orange " +
      "-- with that region's findings beside the image. The person's open app " +
      "zooms to the same region with a caption. Judge appearance from the " +
      "image; take every number from the findings.",
    {
      drawing_id: z.string().uuid().describe("drawing_id from make_drawing"),
      space: z
        .string()
        .optional()
        .describe("a space id from drawing_spaces (e.g. 'view:FRONT/band:top'); omit for the whole sheet"),
      px: z
        .number()
        .int()
        .min(64)
        .max(4096)
        .optional()
        .describe("the image's long edge in pixels (default 1600)"),
    },
    async ({ drawing_id, space, px }) => {
      try {
        // The numbers first: an unknown space is refused here, typed, before
        // any picture is drawn or shown.
        const sq = space ? `?space=${encodeURIComponent(space)}` : "";
        const map = await api("GET", `/api/drawings/${drawing_id}/spaces${sq}`);
        const q = new URLSearchParams();
        if (space) q.set("space", space);
        if (px !== undefined) q.set("px", String(px));
        const qs = q.toString() ? `?${q.toString()}` : "";
        const { bytes, headers } = await apiBytes(`/api/drawings/${drawing_id}/render.png${qs}`);

        const spaces: any[] = Array.isArray(map?.spaces) ? map.spaces : [];
        const SHOWN_UNDER = 8;
        const SHOWN_FINDINGS = 3;
        const worstFirst = (xs: any[]) =>
          xs
            .filter((s) => typeof s?.score === "number" && s.score < 1)
            .sort((a, b) => a.score - b.score || String(a.id).localeCompare(String(b.id)));
        const brief = (s: any) => {
          const lines = findingLines(s);
          return {
            space: s.id,
            score: +Number(s.score).toFixed(3),
            findings:
              lines.length > SHOWN_FINDINGS
                ? [...lines.slice(0, SHOWN_FINDINGS), `+${lines.length - SHOWN_FINDINGS} more`]
                : lines,
          };
        };
        const crop = (headers.get("x-roshera-crop") ?? "")
          .split(",")
          .map(Number)
          .filter((n) => Number.isFinite(n));
        const shownTo = headers.get("x-roshera-shown-to");
        const bar = typeof map?.bar === "number" ? map.bar : 0.8;
        let looked: Record<string, unknown>;
        if (space) {
          const here = spaces.find((s) => s?.id === space);
          const under = worstFirst(spaces.filter((s) => s?.id !== space));
          looked = {
            space,
            kind: here?.kind ?? null,
            score: typeof here?.score === "number" ? +here.score.toFixed(3) : null,
            below_bar: typeof here?.score === "number" ? here.score < bar : null,
            findings: here ? findingLines(here) : [],
            spaces_under_with_findings: under.slice(0, SHOWN_UNDER).map(brief),
            more_under: Math.max(0, under.length - SHOWN_UNDER),
          };
        } else {
          const worst = worstFirst(spaces);
          looked = {
            space: null,
            worst_spaces: worst.slice(0, SHOWN_UNDER).map(brief),
            more_spaces_with_findings: Math.max(0, worst.length - SHOWN_UNDER),
          };
        }
        const grade = typeof map?.grade === "number" ? +map.grade.toFixed(3) : null;
        const text = {
          drawing_id,
          ...looked,
          sheet: { grade, bar, below_bar: map?.below_bar ?? null, worst: map?.worst ?? null },
          crop_mm: crop.length === 4 ? crop : null,
          shown_in_app: shownTo === null ? null : Number(shownTo),
          note:
            "the image shows appearance only; the findings are the kernel's measurements. " +
            "Magenta = the region's box, orange = what its findings name.",
        };
        const content: any[] = [];
        const imgKey = `drawing_look:${drawing_id}:${space ?? ""}:${px ?? 1600}`;
        const watermark = lastGlobalMutationTurnValue();
        const fresh = imageFreshness(imgKey, watermark);
        if (fresh.send) {
          content.push({ type: "image", data: bytes.toString("base64"), mimeType: "image/png" });
          recordImageSent(imgKey, watermark, `drawing_look ${drawing_id} ${space ?? "whole sheet"}`);
        } else {
          content.push({ type: "text", text: `IMAGE UNCHANGED — ${fresh.note}` });
        }
        content.push({ type: "text", text: JSON.stringify(text, null, 2) });
        return { content, structuredContent: text };
      } catch (e) {
        return fail(e);
      }
    },
  );

  // ── iso286_limits ──────────────────────────────────────────────────────
  server.tool(
    "iso286_limits",
    "ISO 286 limits and fits from the kernel's tables: the limit deviations " +
      "and limits of size of one tolerance class ('H7', 'g6') at a nominal " +
      "size, or for a pair ('H7/g6') both classes, min/max clearance and " +
      "whether the pair is a clearance, transition or interference one. " +
      "Refused typed outside the tables; never interpolated.",
    {
      nominal_mm: z.number().positive().describe("nominal size in mm (above 0, up to 500)"),
      fit_class: z.string().describe("'H7', 'g6', 'JS9', or a pair 'H7/g6'"),
    },
    async ({ nominal_mm, fit_class }) => {
      try {
        const r = await api(
          "GET",
          `/api/iso286?nominal=${encodeURIComponent(String(nominal_mm))}&class=${encodeURIComponent(fit_class)}`,
        );
        return ok(r);
      } catch (e) {
        return fail(e);
      }
    },
  );

  // ── drawing_set_projection ─────────────────────────────────────────────
  server.tool(
    "drawing_set_projection",
    "Lay a sheet out FIRST-angle (ISO; India, Europe) or THIRD-angle (ASME). " +
      "Views, symbol and note move; dimensions and title block stay. Kept when " +
      "the part is edited.",
    {
      drawing_id: z.string().uuid().describe("drawing_id from make_drawing"),
      projection: z.enum(["first", "third"]).describe("projection method"),
    },
    async ({ drawing_id, projection }) => {
      try {
        const { result: r, sheet_effect } = await withSheetEffect(drawing_id, () =>
          api("PATCH", `/api/drawings/${drawing_id}/projection`, { projection }),
        );
        return ok({
          drawing_id,
          projection: r?.projection ?? projection,
          quality: r?.quality ?? null,
          sheet_effect,
        });
      } catch (e) {
        return fail(e);
      }
    },
  );

  // ── drawing_set_title ──────────────────────────────────────────────────
  server.tool(
    "drawing_set_title",
    "Fill the sheet's ISO 7200 title fields: a SHORT title (<= 30 chars), the " +
      "detail as supplementary_title, the material (named, e.g. 'S275' -- the " +
      "kernel then measures MASS), creator, document type, revision. Approval " +
      "is a person's act: the sheet stays FOR REVIEW until a person approves it " +
      "in the app. Returns the ISO 7200 gaps that remain.",
    {
      drawing_id: z.string().uuid().describe("drawing_id from make_drawing"),
      title: z.string().max(30).optional().describe("short title, <= 30 characters"),
      supplementary_title: z.string().optional().describe("detail that does not fit the title"),
      material: z.string().optional().describe("named material, e.g. 'S275', 'Aluminium 6061-T6'"),
      drawn_by: z.string().optional().describe("creator"),
      document_type: z.string().optional().describe("e.g. 'PART DRAWING'"),
      revision: z.string().max(2).optional().describe("revision index, <= 2 chars; never I or O"),
    },
    async ({ drawing_id, title, ...fields }) => {
      try {
        if (title !== undefined) {
          await api("PATCH", `/api/drawings/${drawing_id}/rename`, { name: title });
        }
        const tb = await api("PATCH", `/api/drawings/${drawing_id}/title-block`, fields);
        return ok({ drawing_id, title_block: tb ?? null });
      } catch (e) {
        return fail(e);
      }
    },
  );

  // ── drawing_read_semantics ─────────────────────────────────────────────
  server.tool(
    "drawing_read_semantics",
    "READ the SEMANTIC model of a sheet + a live certificate (queryable data, " +
      "NOT a file): views, dimensions with PIDs+tolerances, hole table, GD&T " +
      "blocks, SECTION cut-through, and a re-measured-NOW verdict per fact " +
      "(consistent | stale | dangling | render_only | unprovenanced). Never " +
      "pixel inference. Use drawing_query for ONE scoped question; " +
      "drawing_export_sheet to save a PDF/DXF/SVG file.",
    {
      drawing_id: z.string().uuid().describe("drawing_id from make_drawing"),
    },
    async ({ drawing_id }) => {
      try {
        const r = await api("GET", `/api/drawings/${drawing_id}/semantic`);
        const cert = r?.certificate ?? {};
        const drawing = r?.drawing ?? {};
        const facts: any[] = cert.facts ?? [];
        const unsound = facts.filter(
          (f) => f.live?.verdict === "stale" || f.live?.verdict === "dangling",
        );
        return ok({
          drawing_id,
          name: drawing.name ?? null,
          views: (drawing.views ?? []).map((v: any) => v.name),
          projection: drawing.projection ?? "third",
          // Each dimension once (a dimension may appear in several views):
          // the ids drawing_set_dimension edits.
          dimensions: Array.from(
            new Map(
              (drawing.views ?? [])
                .flatMap((v: any) => v.dimensions ?? [])
                .map((d: any) => [d.id, { id: d.id, label: d.label, kind: d.kind, value: d.value }]),
            ).values(),
          ),
          hole_count: (drawing.hole_sites ?? []).length,
          fcf_count: (drawing.fcf_blocks ?? []).length,
          has_section: drawing.section != null,
          certificate: certificateLine(cert),
          section: cert.section_cuts ? sectionLine(cert.section_cuts) : null,
          unsound_facts: unsound.map(
            (f) => `${f.label} [${f.live.verdict}]`,
          ),
          note: "Ask a scoped question with drawing_query (toleranced_diameter, fcf, section_cuts, hole, dimension_of, entity_at).",
        });
      } catch (e) {
        return fail(e);
      }
    },
  );

  // ── drawing_query ────────────────────────────────────────────────────────
  server.tool(
    "drawing_query",
    "ASK one typed, scoped question of a sheet, answered CERTIFIED against the " +
      "live model with provenance + a verdict. Kinds: toleranced_diameter, fcf " +
      "(which datums it references), section_cuts (what SECTION A-A cuts, in " +
      "order), hole, dimension_of, entity_at. Refusals surfaced verbatim.",
    {
      drawing_id: z.string().uuid().describe("drawing_id from make_drawing"),
      kind: z
        .enum([
          "toleranced_diameter",
          "fcf",
          "section_cuts",
          "dimension_of",
          "hole",
          "entity_at",
        ])
        .describe("the question kind"),
      tag: z.string().optional().describe("hole tag, e.g. 'A1' (toleranced_diameter | hole)"),
      face_id: z.number().int().optional().describe("kernel face id (toleranced_diameter | dimension_of)"),
      pid: z.string().optional().describe("dimension PID (toleranced_diameter | dimension_of)"),
      index: z.number().int().optional().describe("FCF block index (fcf)"),
      feature_pid: z.string().optional().describe("toleranced feature PID hex (fcf)"),
      datum: z.string().optional().describe("datum letter the FCF references, e.g. 'A' (fcf)"),
      label: z.string().optional().describe("dimension label substring (dimension_of)"),
      view: z.number().int().optional().describe("view index (entity_at)"),
      xy_mm: z
        .array(z.number()).length(2)
        .optional()
        .describe("view-space coordinate [x, y] in mm (entity_at)"),
    },
    async (args) => {
      try {
        const { drawing_id, kind, ...rest } = args;
        // Build the typed query body, dropping undefined selectors.
        const body: Record<string, unknown> = { kind };
        for (const [k, v] of Object.entries(rest)) {
          if (v !== undefined) body[k] = v;
        }
        const a = await api("POST", `/api/drawings/${drawing_id}/query`, body);
        return ok({
          drawing_id,
          kind,
          answer: answerLine(a),
          raw: a,
        });
      } catch (e) {
        return fail(e);
      }
    },
  );
}
