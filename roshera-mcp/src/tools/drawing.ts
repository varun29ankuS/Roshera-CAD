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
import { api, ok, fail } from "../core.js";

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
  ].filter(Boolean);
  const quality = cert?.quality?.passed === false ? " | LAYOUT ISSUES" : "";
  return `${sound ? "SOUND ✓" : "UNSOUND ✗"} ${parts.join(", ")}${quality}`;
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
        tol = `general ±${a.general_pm_mm}${a.general_standard ? ` (${a.general_standard})` : ""}`;
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
