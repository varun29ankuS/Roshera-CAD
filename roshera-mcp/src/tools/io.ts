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
import { ACK_UNSOUND } from "./modify.js";

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
      "`save_path`, else ~/Desktop/<file_name>. ENFORCED: refused (422) for any " +
      "part mutated (or never certified) since its last full verification — " +
      "call verify_part first. ALSO ENFORCED (409): refused for any part " +
      "whose live kernel verdict is unsound, even if fully verified — an " +
      "exported file carries no certificate, so shipping a known defect as a " +
      "file is the same hole gate 4 closed for drawings (acknowledge_unsound " +
      "for a deliberate export of the defect). An unverified OR unsound " +
      "solid cannot become a file.",
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
      acknowledge_unsound: ACK_UNSOUND,
    },
    async ({ format, objects, file_name, save_path, quality, acknowledge_unsound }) => {
      try {
        const r = await api("POST", "/api/export", {
          format,
          objects,
          quality,
          ...(acknowledge_unsound ? { acknowledge_unsound: true } : {}),
        });
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
      "passed:false like a watertightness failure. ENFORCED: refused while the " +
      "part's live verdict is unsound — a sheet would print the defect as " +
      "dimensioned truth (acknowledge_unsound for a deliberate inspection sheet).",
    {
      part_id: z.number().int().describe("kernel part/solid id from list_parts"),
      name: z.string().optional().describe("title-block name for the sheet"),
      // Unlike the 9 mutating geometry routes ACK_UNSOUND's doc comment (modify.ts)
      // describes, `POST /api/parts/{id}/drawing` (drawing_mgr::
      // create_part_drawing) NOW carries its own server-side solid-soundness
      // refusal, so this flag is forwarded rather than dropped. It used to be
      // a client-only check whose comment here said, in as many words, that
      // "if the backend ever grows a drawing-side gate, this comment and the
      // handler both need to change together" — this is that change. Without
      // the forward, an agent deliberately inspecting a BROKEN part's drawing
      // acknowledges the unsoundness at the gate, gets past gates.ts, and is
      // then refused 409 by the server with no way to say so: a gate the
      // caller cannot legitimately escape is a bug, not a constraint.
      acknowledge_unsound: ACK_UNSOUND,
    },
    async ({ part_id, name, acknowledge_unsound }) => {
      try {
        const params = new URLSearchParams();
        if (name) params.set("name", name);
        if (acknowledge_unsound === true) params.set("acknowledge_unsound", "true");
        const qs = params.toString() ? `?${params.toString()}` : "";
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
      "semantic data (not a file) use drawing_read_semantics instead. " +
      "ENFORCED: refused while the sheet's live certificate carries stale/" +
      "dangling facts (regenerate with make_drawing — no override) or " +
      "Error-severity layout findings (acknowledge_layout_issues for a " +
      "draft-for-review export). An uncertified sheet cannot become a file.",
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
      // Read by the dispatch gate (gates.ts) AND forwarded to the backend as
      // a query parameter (item 5, 2026-08-15: `export_svg`/`export_pdf`/
      // `export_dxf` in `drawing_mgr.rs` gained their own live
      // `refuse_unsound_sheet` gate, matching `sheetExportGate` here fact for
      // fact). Forwarding it is what makes this an escape ON THE RECORD
      // rather than an MCP-process-local skip: the server-side gate is the
      // one a raw HTTP client would hit too, and it needs the same
      // acknowledgement this tool's caller already gave. Only sent as
      // `?acknowledge_layout_issues=true` when the caller actually passed
      // `true` — never defaulted onto a call that omitted it (same
      // discipline `ACK_UNSOUND`'s doc comment states for the 9 mutating
      // routes).
      acknowledge_layout_issues: z
        .boolean()
        .optional()
        .describe(
          "draft-for-review override: export although the sheet's layout-" +
            "quality certificate has Error findings (otherwise refused). " +
            "Never bypasses stale/dangling facts.",
        ),
      // The export routes ALSO refuse a sheet of a solid the kernel has
      // verified UNSOUND, which is a different fact from the sheet's own
      // layout quality: `acknowledge_layout_issues` deliberately does not
      // open it (pinned server-side by
      // `acknowledge_layout_issues_does_not_bypass_an_unsound_solid`). Two
      // distinct refusals need two distinct acknowledgements — collapsing
      // them would let a caller who meant "the layout is rough" also assert
      // "and I know the geometry is broken", which they never said.
      acknowledge_unsound: ACK_UNSOUND,
    },
    async ({
      drawing_id, format, file_name, save_path,
      acknowledge_layout_issues, acknowledge_unsound,
    }) => {
      try {
        const { join } = await import("node:path");
        const dest = save_path ?? join(await defaultSaveDir(), file_name);
        const params = new URLSearchParams();
        if (acknowledge_layout_issues === true) params.set("acknowledge_layout_issues", "true");
        if (acknowledge_unsound === true) params.set("acknowledge_unsound", "true");
        const qs = params.toString() ? `?${params.toString()}` : "";
        const bytes = await saveBinary(
          `/api/drawings/${drawing_id}/${format}${qs}`,
          dest,
        );
        return ok({ saved_to: dest, bytes, format });
      } catch (e) {
        return fail(e);
      }
    },
  );
}
