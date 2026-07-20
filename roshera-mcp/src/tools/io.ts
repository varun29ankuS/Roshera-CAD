/** I/O tools — STEP import, CAD-file export, drawing generation + fetch. */

import type { ToolHost } from "../registry.js";
import { z } from "zod";
import {
  api,
  ok,
  fail,
  okp,
  newestPartId,
  saveBinary,
  defaultSaveDir,
} from "../core.js";

export function registerIoTools(server: ToolHost) {
  server.tool(
    "import_step",
    "IMPORT a STEP file (AP203/214/242) as real B-Rep solids. Give `path` OR " +
      "inline `content`. Each solid gets the FULL certificate; ok:false = a " +
      "solid imported but is NOT sound (see coverage.validation). Unsupported " +
      "entities listed honestly.",
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
        if (!content && !path) {
          return fail(new Error("provide either `path` or `content`"));
        }
        // #34: a `path` is sent through as-is and read by the SERVER, not
        // this process — a 16-tooth gear STEP export is already 3.3MB, and
        // real CAD STEP files run 10-500MB. Reading it here and re-POSTing
        // it as inline JSON `content` would (a) double the bytes crossing
        // the wire for no reason and (b) still hit the same body-size wall
        // remotely. `content` stays for the genuinely-remote case (the
        // caller doesn't have server-local filesystem access).
        const r = await api("POST", "/api/geometry/import_step", {
          path: path ?? null,
          content: content ?? null,
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
              // FULL certificate verdict (the import path forces it): the honest
              // headline plus the components so a caller sees WHY a solid is
              // unsound (valid B-Rep but open mesh vs. malformed topology).
              sound: o.perception?.sound ?? null,
              brep_valid: o.perception?.brep_valid ?? null,
              watertight: o.perception?.watertight ?? null,
              manifold: o.perception?.manifold ?? null,
              oriented: o.perception?.oriented ?? null,
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
                ? "ok:false — a solid imported but is NOT sound (open/non-manifold/mis-oriented mesh or invalid B-Rep); see coverage.validation for the failing dimension"
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
    "EXPORT parts to a CAD file on disk — STEP (AP242, mm), STL, or OBJ — and " +
      "return the absolute path. `objects` empty = every solid. Saves to " +
      "`save_path`, else ~/Desktop/<file_name>.",
    {
      format: z.enum(["STEP", "STL", "OBJ"]).default("STEP").describe("output file format"),
      objects: z
        .array(z.string().uuid())
        .default([])
        .describe("object_uuids to export; empty = every solid"),
      file_name: z
        .string()
        .regex(/^[\w.-]+$/)
        .describe("file name without directory, e.g. flange_2in.step"),
      save_path: z
        .string()
        .optional()
        .describe("absolute destination path; overrides file_name/Desktop"),
      quality: z
        .enum(["Low", "Medium", "High"])
        .default("High")
        .describe("tessellation quality for STL/OBJ meshes"),
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
    "Generate a 2D engineering DRAWING: four-view sheet (Front/Top/Right + iso), " +
      "hidden-line removal, centerlines, ISO-129 deduped dimensions. Returns the " +
      "drawing id + a QUALITY report (label collisions, redundant dims); treat " +
      "passed:false like a watertightness failure.",
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
          note: "Open in the Drawing workspace, or drawing_export_sheet to save PDF/DXF/SVG to disk.",
        });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "drawing_export_sheet",
    "SAVE the RENDERED sheet from make_drawing to disk as a PDF/DXF/SVG FILE — " +
      "the shop-ready sheet — and return the absolute path. For the queryable " +
      "semantic data (not a file) use drawing_read_semantics instead.",
    {
      drawing_id: z.string().uuid().describe("drawing_id from make_drawing"),
      format: z.enum(["pdf", "dxf", "svg"]).default("pdf").describe("output file format"),
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
