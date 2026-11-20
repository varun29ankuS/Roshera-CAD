/**
 * GD&T tools — kernel-certified datum designation and feature control frame
 * evaluation. Verdicts are exact measurements against the B-Rep, never
 * approximations from a mesh or renderer.
 *
 * Persistence: DRF and FCF annotations live in-process for the server's
 * lifetime (session). A server restart clears all GD&T state — the backend
 * surfaces `"persistence":"session"` on every response as the honesty signal.
 */

import type { ToolHost } from "../registry.js";
import { z } from "zod";
import { api, ok, fail, BASE, ApiError, scopeHeaders, readRefusable, withStructured, TIMEOUT_MS, PART_ID_DOC } from "../core.js";
import { typedErrorResult } from "../gates.js";

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
    // Keep the REFUSED marker (the refusal cache reads it) AND the body, so
    // fail() surfaces the catalogue's error_code / details as structured
    // content instead of prose (MCP_AUDIT.md §2).
    return fail(new ApiError(`REFUSED: ${msg}`, e.status, e.body));
  } catch {
    return fail(new ApiError(`REFUSED: ${e.body}`, e.status, e.body));
  }
}

/**
 * A refused GD&T EDIT (update / delete) as a typed refusal with a top-level
 * `reason` — the shape the Blackboard's `roshera:refusal` card requires.
 *
 * The edit routes answer `{ "error": <code>, "message": <sentence> }`
 * (handlers/gdt.rs `refusal`), not a catalogue body, so `refusalOrFail`'s
 * `error`-first order (timeline.ts) would lift the CODE as the reason. Here
 * the sentence is the reason and the code is `error_code`; a catalogue body
 * (`error_code` present) keeps its own `error` sentence, code and hint. The
 * body rides verbatim as `refused`, so a 409 `datum_referenced` keeps the
 * `annotations` it names — exactly the frames to fix first. 404 is included:
 * `annotation_not_found` / `annotation_not_on_part` / `datum_not_found` are
 * the commonest refused deletes. A non-JSON body (a transport failure, an
 * extractor rejection) falls through to `fail()`.
 */
function gdtEditRefusal(e: unknown) {
  if (e instanceof ApiError && [404, 409, 422].includes(e.status)) {
    let body: unknown;
    try {
      body = JSON.parse(e.body);
    } catch {
      return fail(e);
    }
    if (body !== null && typeof body === "object") {
      const b = body as Record<string, unknown>;
      const words = (k: string) =>
        typeof b[k] === "string" && (b[k] as string).length > 0 ? (b[k] as string) : null;
      const catalogue = words("error_code");
      const reason = catalogue
        ? words("error") ?? words("message") ?? e.message
        : words("message") ?? words("reason") ?? words("error") ?? e.message;
      const code = catalogue ?? words("error");
      return typedErrorResult({
        refused: body,
        reason,
        ...(code !== null ? { error_code: code } : {}),
        retryable: b.retryable === true,
        ...(words("hint") !== null ? { hint: words("hint") } : {}),
      });
    }
  }
  return fail(e);
}

/** An edit that names nothing to change: refused before any request, so no
 *  no-op `gdt_*_update` event is recorded. */
function noChange(tool: string, fields: string) {
  return typedErrorResult({
    refused: true,
    error_code: "no_change_specified",
    reason: `${tool} names nothing to change; pass at least one of ${fields}`,
    retryable: false,
  });
}

/**
 * The undo disclosure every GD&T edit result carries, read from the server's
 * own `persistence` field — never assumed. `"timeline"` means the route
 * recorded a `gdt_*` design event, so `timeline_undo` takes the edit back.
 */
function undoDisclosure(persistence: unknown): { persistence: unknown; undoable: boolean; undo: string } {
  const undoable = persistence === "timeline";
  return {
    persistence: persistence ?? null,
    undoable,
    undo: undoable
      ? "Recorded on the timeline as a design event: timeline_undo takes this edit back."
      : `The server reported persistence '${String(persistence)}': this edit is not recorded on the timeline and cannot be undone.`,
  };
}

/** Parse an optional selector JSON string; `undefined` when absent/blank. */
function parseSelector(raw: string | undefined, field: string): { value?: unknown; error?: ReturnType<typeof fail> } {
  if (raw === undefined || raw.trim().length === 0) return {};
  try {
    return { value: JSON.parse(raw) };
  } catch {
    return { error: fail(new Error(`${field} is not valid JSON: ${raw}`)) };
  }
}

// ─── Tool registration ─────────────────────────────────────────────────

export function registerGdtTools(server: ToolHost): void {
  // ── gdt_datum ──────────────────────────────────────────────────────────

  server.tool(
    "gdt_datum",
    "DESIGNATE a face as a datum (label + target) OR LIST current datums (omit " +
      "label/target). Datums pin to PersistentIds and dangle honestly when their " +
      "face is consumed. Qualifying: planar → datum plane, cylindrical → datum " +
      "axis; else REFUSED. Session-only. Target by face_id or `selector` " +
      "(select_face shape).",
    {
      part_id: z.number().int().describe(PART_ID_DOC),
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
            headers: { "Content-Type": "application/json", ...scopeHeaders() },
            // Bounded like every api() call: a wedged server is a 504-shaped
            // error, not a hang (MCP_AUDIT.md §2).
            signal: AbortSignal.timeout(TIMEOUT_MS),
            body: JSON.stringify({
              label,
              face_id: face_id ?? null,
              selector: selectorObj,
            }),
          },
        );

        // A non-2xx answer throws ApiError with the body; refusalFrom() below
        // turns a 409/422 into a typed refusal.
        const body = await readRefusable(res, `POST /api/agent/parts/${part_id}/datums`);
        return withStructured(ok(body));
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
      part_id: z.number().int().describe(PART_ID_DOC),
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
            headers: { "Content-Type": "application/json", ...scopeHeaders() },
            // Bounded like every api() call: a wedged server is a 504-shaped
            // error, not a hang (MCP_AUDIT.md §2).
            signal: AbortSignal.timeout(TIMEOUT_MS),
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

        const body: any = await readRefusable(res, `POST /api/agent/parts/${part_id}/fcf`);

        const verdict: any = body?.verdict;
        const line = verdict != null ? compactGdtVerdict(verdict) : "(no verdict)";
        // `annotation_id` is the handle PATCH / DELETE .../fcf/{id} take and
        // `modifier` qualifies the tolerance; both were dropped before
        // 2026-11-18 (MCP_AUDIT.md 1.8). The compact line stays for reading;
        // the verdict object rides beside it.
        return withStructured(
          ok({
            part_id,
            annotation_id: body?.annotation_id ?? null,
            annotation_pid: body?.annotation_pid,
            modifier: body?.modifier ?? null,
            verdict: line,
            verdict_detail: verdict ?? null,
            persistence: body?.persistence ?? "session",
          }),
        );
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
      part_id: z.number().int().describe(PART_ID_DOC),
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

  // ── Correcting authored GD&T ───────────────────────────────────────────
  //
  // One verb per edit, the label_create / label_rename / label_delete shape:
  // an `action` on gdt_fcf would make its required characteristic and
  // tolerance_mm optional for every call (and break its schema parity with
  // the backend row). Each route records a `gdt_*` design event through the
  // kernel mediators (geometry_engine::gdt::edit), so an edit is corrected in
  // place — and is itself undoable — instead of undoing the timeline back
  // past the mistake.

  server.tool(
    "gdt_fcf_update",
    "EDIT / FIX an authored Feature Control Frame by annotation_id (from gdt_fcf " +
      "or gdt_report). Only the fields given change: basic dims (e.g. a wrong " +
      "sign), zone width, datum_refs, characteristic, modifier, or the target " +
      "face. Returns the re-evaluated certified verdict. Recorded on the " +
      "timeline (undoable). Correct a wrong frame with this, not timeline_undo.",
    {
      part_id: z.number().int().describe(PART_ID_DOC),
      annotation_id: z
        .string()
        .min(1)
        .describe("the frame's annotation_id (gdt_fcf result / gdt_report)"),
      characteristic: z
        .enum(["flatness", "perpendicularity", "parallelism", "position"])
        .optional()
        .describe("new GD&T characteristic"),
      tolerance_mm: z.number().positive().optional().describe("new tolerance zone width in millimetres"),
      datum_refs: z
        .array(z.string())
        .optional()
        .describe("new ordered datum labels, replacing the old list ([] for flatness)"),
      modifier: z
        .enum(["rfs", "mmc", "lmc"])
        .optional()
        .describe("material condition; the kernel evaluates RFS only (MMC/LMC → not_evaluable)"),
      basic: z
        .array(z.number()).length(2)
        .optional()
        .describe("new basic dims [x, y] mm relative to the DRF origin (position)"),
      target_face: z
        .number()
        .int()
        .optional()
        .describe("move the frame to this kernel face id"),
      target_selector: z
        .string()
        .optional()
        .describe("move the frame to a face by description: JSON string shaped like select_face"),
    },
    async ({
      part_id,
      annotation_id,
      characteristic,
      tolerance_mm,
      datum_refs,
      modifier,
      basic,
      target_face,
      target_selector,
    }) => {
      const sel = parseSelector(target_selector, "target_selector");
      if (sel.error) return sel.error;
      // Only the fields given: the route keeps every omitted one.
      const body: Record<string, unknown> = {
        ...(characteristic !== undefined ? { characteristic } : {}),
        ...(tolerance_mm !== undefined ? { tolerance_mm } : {}),
        ...(datum_refs !== undefined ? { datum_refs } : {}),
        ...(modifier !== undefined ? { modifier } : {}),
        ...(basic !== undefined ? { basic } : {}),
        ...(target_face !== undefined ? { face_id: target_face } : {}),
        ...(sel.value !== undefined ? { selector: sel.value } : {}),
      };
      if (Object.keys(body).length === 0) {
        return noChange(
          "gdt_fcf_update",
          "characteristic, tolerance_mm, datum_refs, modifier, basic, target_face, target_selector",
        );
      }
      try {
        const r: any = await api(
          "PATCH",
          `/api/agent/parts/${part_id}/fcf/${encodeURIComponent(annotation_id)}`,
          body,
        );
        const verdict: any = r?.verdict;
        return withStructured(
          ok({
            part_id,
            annotation_id: r?.annotation_id ?? annotation_id,
            annotation_pid: r?.annotation_pid,
            modifier: r?.modifier ?? null,
            verdict: verdict != null ? compactGdtVerdict(verdict) : "(no verdict)",
            verdict_detail: verdict ?? null,
            ...undoDisclosure(r?.persistence),
          }),
        );
      } catch (e) {
        return gdtEditRefusal(e);
      }
    },
  );

  server.tool(
    "gdt_fcf_delete",
    "DELETE / REMOVE an authored Feature Control Frame by annotation_id (from " +
      "gdt_fcf or gdt_report). Recorded on the timeline (undoable). Drop a wrong " +
      "frame with this, not timeline_undo.",
    {
      part_id: z.number().int().describe(PART_ID_DOC),
      annotation_id: z
        .string()
        .min(1)
        .describe("the frame's annotation_id (gdt_fcf result / gdt_report)"),
    },
    async ({ part_id, annotation_id }) => {
      try {
        const r: any = await api(
          "DELETE",
          `/api/agent/parts/${part_id}/fcf/${encodeURIComponent(annotation_id)}`,
        );
        return withStructured(
          ok({
            part_id,
            annotation_id: r?.annotation_id ?? annotation_id,
            deleted: r?.success === true,
            ...undoDisclosure(r?.persistence),
          }),
        );
      } catch (e) {
        return gdtEditRefusal(e);
      }
    },
  );

  server.tool(
    "gdt_datum_update",
    "EDIT a datum by its current letter: re-letter it (new_label) and/or move " +
      "it to another surface (by id or selector). Re-lettering a datum a frame " +
      "references is REFUSED (409, naming the frames). Recorded on the timeline " +
      "(undoable).",
    {
      part_id: z.number().int().describe(PART_ID_DOC),
      label: z.string().min(1).describe("the datum's current letter, e.g. 'A'"),
      new_label: z.string().min(1).optional().describe("the new letter, e.g. 'C'"),
      face_id: z
        .number()
        .int()
        .optional()
        .describe("move the datum to this kernel face id; mutually exclusive with selector"),
      selector: z
        .string()
        .optional()
        .describe("move the datum to a face by description: JSON string shaped like select_face"),
    },
    async ({ part_id, label, new_label, face_id, selector }) => {
      const sel = parseSelector(selector, "selector");
      if (sel.error) return sel.error;
      const body: Record<string, unknown> = {
        ...(new_label !== undefined ? { label: new_label } : {}),
        ...(face_id !== undefined ? { face_id } : {}),
        ...(sel.value !== undefined ? { selector: sel.value } : {}),
      };
      if (Object.keys(body).length === 0) {
        return noChange("gdt_datum_update", "new_label, face_id, selector");
      }
      try {
        const r: any = await api(
          "PATCH",
          `/api/agent/parts/${part_id}/datums/${encodeURIComponent(label)}`,
          body,
        );
        return withStructured(
          ok({
            part_id,
            label: r?.label ?? new_label ?? label,
            previous_label: label,
            kind: r?.kind,
            persistent_id: r?.persistent_id,
            face_id: r?.face_id ?? null,
            ...undoDisclosure(r?.persistence),
          }),
        );
      } catch (e) {
        return gdtEditRefusal(e);
      }
    },
  );

  server.tool(
    "gdt_datum_delete",
    "DELETE / REMOVE a datum by its letter. REFUSED (409 datum_referenced, " +
      "naming the frames) while any FCF references it — edit or delete those " +
      "frames first. Recorded on the timeline (undoable).",
    {
      part_id: z.number().int().describe(PART_ID_DOC),
      label: z.string().min(1).describe("the datum letter to delete, e.g. 'B'"),
    },
    async ({ part_id, label }) => {
      try {
        const r: any = await api(
          "DELETE",
          `/api/agent/parts/${part_id}/datums/${encodeURIComponent(label)}`,
        );
        return withStructured(
          ok({
            part_id,
            label: r?.label ?? label,
            deleted: r?.success === true,
            ...undoDisclosure(r?.persistence),
          }),
        );
      } catch (e) {
        return gdtEditRefusal(e);
      }
    },
  );

  server.tool(
    "gdt_dimensional_delete",
    "DELETE / REMOVE a size tolerance (±, limits or fit on a feature) by " +
      "annotation_id (gdt_report, kind 'dimensional'). An FCF id is REFUSED " +
      "(use gdt_fcf_delete). Recorded on the timeline (undoable).",
    {
      part_id: z.number().int().describe(PART_ID_DOC),
      annotation_id: z
        .string()
        .min(1)
        .describe("the size tolerance's annotation_id (gdt_report)"),
    },
    async ({ part_id, annotation_id }) => {
      try {
        const r: any = await api(
          "DELETE",
          `/api/agent/parts/${part_id}/size-tolerance/${encodeURIComponent(annotation_id)}`,
        );
        return withStructured(
          ok({
            part_id,
            annotation_id: r?.annotation_id ?? annotation_id,
            deleted: r?.success === true,
            ...undoDisclosure(r?.persistence),
          }),
        );
      } catch (e) {
        return gdtEditRefusal(e);
      }
    },
  );
}
