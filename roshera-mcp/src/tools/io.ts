/** I/O tools — STEP import, CAD-file export, drawing generation + fetch. */

import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import { readFile } from "node:fs/promises";
import {
  api,
  ok,
  fail,
  okp,
  newestPartId,
  saveBinary,
  defaultSaveDir,
} from "../core.js";

export function registerIoTools(server: McpServer) {
  server.tool(
    "import_step",
    "IMPORT a STEP file (AP203/AP214/AP242) as real B-Rep solids — planar/" +
      "quadric/toroidal faces, NURBS curves+surfaces, revolution/extrusion " +
      "surfaces, voids, assembly instances. Give a `path` OR inline `content`. " +
      "Every solid is kernel-VALIDATED; ok:false = imported but failed " +
      "validation (see report). Unsupported entities are listed honestly.",
    {
      path: z
        .string()
        .optional()
        .describe("filesystem path to a .step/.stp file (read locally)"),
      content: z.string().optional().describe("inline STEP file text"),
      name: z.string().optional().describe("display-name prefix for imported parts"),
    },
    async ({ path, content, name }) => {
      try {
        let text = content;
        if (!text && path) {
          text = await readFile(path, "utf8");
        }
        if (!text) {
          return fail(new Error("provide either `path` or `content`"));
        }
        const r = await api("POST", "/api/geometry/import_step", {
          content: text,
          name: name ?? null,
        });
        const objects = Array.isArray(r.objects) ? r.objects : [];
        const id = await newestPartId();
        return await okp(
          {
            ok: r.success,
            imported: objects.map((o: any) => ({
              object_uuid: o.id,
              part_id: o.solid_id,
              name: o.name,
              brep_valid: o.perception?.brep_valid ?? null,
            })),
            coverage: {
              schema: r.report?.schema ?? null,
              roots_resolved: r.report?.roots_resolved ?? null,
              resolved: r.report?.counts?.resolved ?? null,
              unsupported: r.report?.counts?.unsupported ?? null,
              validation: r.report?.validation ?? null,
            },
            note:
              r.success === false
                ? "ok:false — a solid imported but failed kernel validation; see coverage.validation"
                : "imported; render_part / scene_view to SEE the result",
          },
          id,
        );
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "export_part",
    "EXPORT parts to a real CAD file on disk — STEP (AP242, mm), STL, or OBJ " +
      "— and return the absolute path. The production hand-off: the STEP " +
      "opens in FreeCAD/SolidWorks/slicers. `objects` empty = every solid. " +
      "Saves to `save_path` if given, else ~/Desktop/<file_name>.",
    {
      format: z.enum(["STEP", "STL", "OBJ"]).default("STEP"),
      objects: z.array(z.string().uuid()).default([]),
      file_name: z
        .string()
        .regex(/^[\w.-]+$/)
        .describe("file name without directory, e.g. flange_2in.step"),
      save_path: z
        .string()
        .optional()
        .describe("absolute destination path; overrides file_name/Desktop"),
      quality: z.enum(["Low", "Medium", "High"]).default("High"),
    },
    async ({ format, objects, file_name, save_path, quality }) => {
      try {
        const r = await api("POST", "/api/export", { format, objects, quality });
        if (!r?.download_url) {
          throw new Error(`export returned no download_url: ${JSON.stringify(r)}`);
        }
        const { join } = await import("node:path");
        const dest = save_path ?? join(await defaultSaveDir(), file_name);
        const bytes = await saveBinary(r.download_url, dest);
        return ok({
          saved_to: dest,
          bytes,
          format,
          parts_exported: objects.length === 0 ? "all" : objects.length,
          export_time_ms: r.export_time_ms ?? null,
        });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "make_drawing",
    "Generate a 2D engineering DRAWING: the standard four-view sheet (Front/" +
      "Top/Right + iso) with hidden-line removal, centerlines, automatic " +
      "dimensions (each feature dimensioned once, in its best view — ISO " +
      "129-1 dedup). Returns the drawing id AND a QUALITY report whose " +
      "invariants CANNOT be fooled (label collisions, redundant/on-geometry " +
      "dimensions — the verifier reads the exact boxes the renderer inks). " +
      "Treat passed:false like a watertightness failure.",
    {
      part_id: z.number().int().describe("kernel part/solid id from list_parts"),
      name: z.string().optional().describe("title-block name for the sheet"),
    },
    async ({ part_id, name }) => {
      try {
        const qs = name ? `?name=${encodeURIComponent(name)}` : "";
        const r = await api("POST", `/api/parts/${part_id}/drawing${qs}`);
        const q = r?.quality ?? null;
        return ok({
          drawing_id: r?.id ?? null,
          quality: q,
          verdict: q
            ? q.passed
              ? `OK — clean sheet (${Math.round((q.sheet_utilization ?? 0) * 100)}% utilization, ${
                  q.issues?.length ?? 0
                } advisory issue(s))`
              : `LAYOUT ISSUES — ${q.issues?.length ?? 0} finding(s); see quality.issues`
            : "drawing created (no quality report)",
          note: "Open in the Drawing workspace, or fetch_drawing to save PDF/DXF/SVG to disk.",
        });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "fetch_drawing",
    "SAVE a drawing produced by make_drawing to disk as PDF, DXF, or SVG and " +
      "return the absolute file path — the shop-ready sheet.",
    {
      drawing_id: z.string().uuid().describe("drawing_id from make_drawing"),
      format: z.enum(["pdf", "dxf", "svg"]).default("pdf"),
      file_name: z
        .string()
        .regex(/^[\w.-]+$/)
        .describe("file name without directory, e.g. flange_drawing.pdf"),
      save_path: z
        .string()
        .optional()
        .describe("absolute destination path; overrides file_name/Desktop"),
    },
    async ({ drawing_id, format, file_name, save_path }) => {
      try {
        const { join } = await import("node:path");
        const dest = save_path ?? join(await defaultSaveDir(), file_name);
        const bytes = await saveBinary(`/api/drawings/${drawing_id}/${format}`, dest);
        return ok({ saved_to: dest, bytes, format });
      } catch (e) {
        return fail(e);
      }
    },
  );
}
