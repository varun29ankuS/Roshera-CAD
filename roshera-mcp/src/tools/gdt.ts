/**
 * GD&T tools — kernel-certified datum designation and feature control frame
 * evaluation. Verdicts are exact measurements against the B-Rep, never
 * approximations from a mesh or renderer.
 *
 * Persistence: DRF and FCF annotations live in-process for the server's
 * lifetime (session). A server restart clears all GD&T state — the backend
 * surfaces `"persistence":"session"` on every response as the honesty signal.
 */

import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import { api, ok, fail, BASE, ApiError, AUTH_HEADERS } from "../core.js";

// ─── Compact-verdict helpers ───────────────────────────────────────────

/** GD&T characteristic → display glyph (UTF-8 safe; fallback for plain text). */
function glyphFor(characteristic: string): string {
  switch (characteristic.toLowerCase()) {
    case "flatness":        return "⏥"; // ⏥
    case "perpendicularity": return "⊥"; // ⊥
    case "parallelism":     return "∥"; // ∥
    case "position":        return "⌖"; // ⌖
    default:                return characteristic;
  }
}

/**
 * Disclose the resolved position DRF (origin + derivation) as a compact
 * suffix — self-certifying: the true position is
 * `origin + basic.x·x_axis + basic.y·y_axis`. Empty when the verdict carries
 * no frame (every non-position characteristic).
 */
function frameSuffix(verdict: any): string {
  const f = verdict.frame;
  if (f == null || !Array.isArray(f.origin)) return "";
  const o = f.origin.map((v: number) => Number(v.toFixed(4))).join(", ");
  const deriv = f.derivation != null ? `; ${f.derivation}` : "";
  return ` [DRF origin (${o})${deriv}]`;
}

/**
 * Render one verdict as a single compact line — token-diet format:
 *   ⊥ 0.05 A → IN SPEC (measured 0.031mm, residual 2e-4)
 *   ⌖ 0.10 A B → OUT OF SPEC (measured 0.142mm, residual 1e-5) [DRF origin (…)]
 *   ⏥ 0.05 → NOT EVALUABLE: datum 'A' is dangling
 */
function compactGdtVerdict(verdict: any): string {
  const glyph = glyphFor(verdict.characteristic ?? "");
  const tol = verdict.tolerance_label ?? `${verdict.tolerance_mm}mm`;
  const datums: string[] = (verdict.datum_statuses ?? []).map(
    (d: any) => d.label as string,
  );
  const datumStr = datums.length > 0 ? ` ${datums.join(" ")}` : "";
  const prefix = `${glyph} ${tol}${datumStr}`;

  const conforms: string = (verdict.conforms ?? "").toLowerCase();
  if (conforms === "in_spec") {
    const meas = verdict.measured_label ?? (verdict.measured_mm != null ? `${verdict.measured_mm}mm` : null);
    const res = verdict.fit_residual_mm != null ? `, residual ${verdict.fit_residual_mm.toExponential(0)}` : "";
    const measStr = meas != null ? ` (measured ${meas}${res})` : "";
    return `${prefix} → IN SPEC${measStr}${frameSuffix(verdict)}`;
  }
  if (conforms === "out_of_spec") {
    const meas = verdict.measured_label ?? (verdict.measured_mm != null ? `${verdict.measured_mm}mm` : null);
    const res = verdict.fit_residual_mm != null ? `, residual ${verdict.fit_residual_mm.toExponential(0)}` : "";
    const measStr = meas != null ? ` (measured ${meas}${res})` : "";
    return `${prefix} → OUT OF SPEC${measStr}${frameSuffix(verdict)}`;
  }
  // not_evaluable
  const reason = verdict.reason ?? "unknown reason";
  return `${prefix} → NOT EVALUABLE: ${reason}`;
}

/** Render a datum entry as a short description line. */
function compactDatum(d: any): string {
  const res: any = d.resolution ?? {};
  const status: string = (res.status ?? "live").toLowerCase();
  const kindStr = d.kind === "axis" ? "axis" : "plane";
  if (status === "dangling") {
    return `  datum ${d.label} [${kindStr}] — DANGLING (source face consumed)`;
  }
  return `  datum ${d.label} [${kindStr}] — live`;
}

// ─── Refusal surfacing (verbatim) ─────────────────────────────────────

/**
 * Surface a backend refusal as "REFUSED: <verbatim reason>".
 * 409 = duplicate label; 422 = non-qualifying surface / missing target / etc.
 * The backend message is never paraphrased — what the kernel says is what the
 * agent reads.
 */
function refusalFrom(e: unknown): ReturnType<typeof fail> | null {
  if (!(e instanceof ApiError)) return null;
  if (e.status !== 409 && e.status !== 422) return null;
  try {
    const body = JSON.parse(e.body);
    const msg: string = body.message ?? body.error ?? e.body;
    return fail(new Error(`REFUSED: ${msg}`));
  } catch {
    return fail(new Error(`REFUSED: ${e.body}`));
  }
}

// ─── Tool registration ─────────────────────────────────────────────────

export function registerGdtTools(server: McpServer): void {
  // ── gdt_datum ──────────────────────────────────────────────────────────

  server.tool(
    "gdt_datum",
    "DESIGNATE a face as a datum (label + target) OR LIST current datums (omit " +
      "label/target). Datums pin to PersistentIds and dangle honestly when their " +
      "face is consumed. Qualifying: planar → datum plane, cylindrical → datum " +
      "axis; else REFUSED. Session-only. Target by face_id or `selector` " +
      "(select_face shape).",
    {
      part_id: z.number().int().describe("part id (list_parts)"),
      label: z
        .string()
        .optional()
        .describe("datum letter e.g. 'A', 'B', 'C' — omit to list"),
      face_id: z
        .number()
        .int()
        .optional()
        .describe("kernel face id; mutually exclusive with selector"),
      selector: z
        .string()
        .optional()
        .describe(
          "face by description: JSON string shaped like select_face body, " +
            "e.g. '{\"kind\":\"planar\",\"extremal\":\"most_along\",\"along\":[0,0,1]}'",
        ),
    },
    async ({ part_id, label, face_id, selector }) => {
      try {
        // No label → GET list
        if (label === undefined && face_id === undefined && selector === undefined) {
          const r = await api("GET", `/api/agent/parts/${part_id}/datums`);
          const datums: any[] = r.datums ?? [];
          if (datums.length === 0) {
            return ok({
              part_id,
              datums: [],
              note: "No datums designated. Use gdt_datum with label + face_id/selector to designate.",
              persistence: r.persistence ?? "session",
            });
          }
          const lines = datums.map(compactDatum).join("\n");
          return ok({
            part_id,
            datums,
            summary: lines,
            persistence: r.persistence ?? "session",
          });
        }

        // With label → POST designate
        if (label === undefined) {
          return fail(
            new Error("provide label (e.g. 'A') when designating a datum"),
          );
        }

        let selectorObj: unknown = null;
        if (selector !== undefined && selector.trim().length > 0) {
          try {
            selectorObj = JSON.parse(selector);
          } catch {
            return fail(new Error(`selector is not valid JSON: ${selector}`));
          }
        }

        // Use raw fetch so 409/422 refusals are read as structured JSON.
        const res = await fetch(
          `${BASE}/api/agent/parts/${part_id}/datums`,
          {
            method: "POST",
            headers: {
              "Content-Type": "application/json",
              "X-Roshera-Agent": "Claude",
              ...AUTH_HEADERS,
            },
            body: JSON.stringify({
              label,
              face_id: face_id ?? null,
              selector: selectorObj,
            }),
          },
        );

        const body = await res.json().catch(() => null);
        if (!res.ok) {
          const msg: string =
            (body as any)?.message ?? (body as any)?.error ?? res.statusText;
          return fail(new Error(`REFUSED: ${msg}`));
        }
        return ok(body);
      } catch (e) {
        const r = refusalFrom(e);
        if (r !== null) return r;
        return fail(e);
      }
    },
  );

  // ── gdt_fcf ────────────────────────────────────────────────────────────

  server.tool(
    "gdt_fcf",
    "AUTHOR a Feature Control Frame; returns an IMMEDIATE certified verdict " +
      "(exact B-Rep, never pixels). Stored + re-evaluated on every gdt_report. " +
      "Characteristics: flatness (⏥, no datum); perpendicularity (⊥) / " +
      "parallelism (∥) vs a datum PLANE (target = planar face OR cylindrical " +
      "AXIS); position (⌖, needs `basic`). Position DRF origin: 1 plane = part " +
      "corner; 2 planes A|B = their intersection; plane + axis |A|B| " +
      "(bolt-circle) = axis B ∩ plane A. Verdicts DISCLOSE the DRF. datum_refs " +
      "must already exist (gdt_datum) or refused (422). Session-only.",
    {
      part_id: z.number().int().describe("part id (list_parts)"),
      characteristic: z
        .enum(["flatness", "perpendicularity", "parallelism", "position"])
        .describe("GD&T characteristic to evaluate"),
      tolerance_mm: z
        .number()
        .positive()
        .describe("tolerance zone width in millimetres"),
      datum_refs: z
        .array(z.string())
        .optional()
        .describe(
          "ordered datum labels e.g. ['A'] or ['A','B']; empty/omit for flatness",
        ),
      target_face: z
        .number()
        .int()
        .optional()
        .describe("kernel face id of the toleranced feature"),
      target_selector: z
        .string()
        .optional()
        .describe(
          "feature by description: JSON string shaped like select_face, " +
            "e.g. '{\"kind\":\"planar\",\"normal_dir\":[0,0,1]}'",
        ),
      basic: z
        .array(z.number()).length(2)
        .optional()
        .describe(
          "basic dims [x, y] mm relative to DRF origin — required for position",
        ),
    },
    async ({
      part_id,
      characteristic,
      tolerance_mm,
      datum_refs,
      target_face,
      target_selector,
      basic,
    }) => {
      try {
        let selectorObj: unknown = null;
        if (target_selector !== undefined && target_selector.trim().length > 0) {
          try {
            selectorObj = JSON.parse(target_selector);
          } catch {
            return fail(
              new Error(`target_selector is not valid JSON: ${target_selector}`),
            );
          }
        }

        const res = await fetch(
          `${BASE}/api/agent/parts/${part_id}/fcf`,
          {
            method: "POST",
            headers: {
              "Content-Type": "application/json",
              "X-Roshera-Agent": "Claude",
              ...AUTH_HEADERS,
            },
            body: JSON.stringify({
              characteristic,
              tolerance_mm,
              datum_refs: datum_refs ?? [],
              face_id: target_face ?? null,
              selector: selectorObj,
              basic: basic ?? null,
            }),
          },
        );

        const body = await res.json().catch(() => null);
        if (!res.ok) {
          const msg: string =
            (body as any)?.message ?? (body as any)?.error ?? res.statusText;
          return fail(new Error(`REFUSED: ${msg}`));
        }

        const verdict: any = (body as any)?.verdict;
        const line = verdict != null ? compactGdtVerdict(verdict) : "(no verdict)";
        return ok({
          part_id,
          annotation_pid: (body as any)?.annotation_pid,
          verdict: line,
          persistence: (body as any)?.persistence ?? "session",
        });
      } catch (e) {
        const r = refusalFrom(e);
        if (r !== null) return r;
        return fail(e);
      }
    },
  );

  // ── gdt_report ─────────────────────────────────────────────────────────

  server.tool(
    "gdt_report",
    "ALL GD&T state for a part: datum list (label, kind, live/dangling) + one " +
      "compact verdict line per FCF annotation, re-evaluated live against the " +
      "B-Rep. Dangling reported honestly. Session-only (cleared on restart).",
    {
      part_id: z.number().int().describe("part id (list_parts)"),
    },
    async ({ part_id }) => {
      try {
        const r = await api("GET", `/api/agent/parts/${part_id}/gdt`);
        const datums: any[] = r.datums ?? [];
        const annotations: any[] = r.annotations ?? [];

        const lines: string[] = [];

        if (datums.length === 0) {
          lines.push("Datums: none");
        } else {
          lines.push("Datums:");
          for (const d of datums) {
            lines.push(compactDatum(d));
          }
        }

        if (annotations.length === 0) {
          lines.push("Annotations: none");
        } else {
          lines.push("Annotations:");
          for (const ann of annotations) {
            const line = compactGdtVerdict(ann.verdict);
            lines.push(`  ${line}`);
          }
        }

        lines.push(
          `⚠️  Persistence: session — GD&T state is cleared on server restart.`,
        );

        return ok({
          part_id,
          datums,
          annotations,
          report: lines.join("\n"),
        });
      } catch (e) {
        return fail(e);
      }
    },
  );
}
