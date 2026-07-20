/** Perception tools — the agent's eyes: renders, X-rays, sections, certs. */

import type { ToolHost } from "../registry.js";
import { z } from "zod";
import { api, ok, fail, ApiError } from "../core.js";

export function registerPerceptionTools(server: ToolHost) {
  server.tool(
    "render_part",
    "SEE a part: deterministic offscreen render as an image. mode 'ids' returns " +
      "a color→face_id legend (to address topology); 'diagnostic' highlights " +
      "defects; 'depth'/'normals' are exact G-buffer channels.",
    {
      part_id: z.number().int().describe("part id (list_parts)"),
      mode: z
        .enum(["shaded", "ids", "depth", "normals", "diagnostic"])
        .default("shaded")
        .describe("render channel ('ids' returns a face_id legend; 'diagnostic' highlights defects)"),
      view: z.enum(["iso", "front", "top", "right"]).default("iso").describe("camera view"),
      size: z.number().int().min(64).max(2048).default(512).describe("image size in px"),
    },
    async ({ part_id, mode, view, size }) => {
      try {
        const r = await api(
          "GET",
          `/api/agent/parts/${part_id}/render?mode=${mode}&view=${view}&size=${size}`,
        );
        const content: any[] = [
          { type: "image", data: r.png_base64, mimeType: "image/png" },
        ];
        if (mode === "ids") {
          content.push({
            type: "text",
            text: `face legend (rgb → face_id): ${JSON.stringify(r.face_legend)}`,
          });
        }
        if (mode === "diagnostic") {
          content.push({
            type: "text",
            text:
              `defects — open_edges (red hole rims, missing faces): ${r.open_edges}; ` +
              `nonmanifold_edges (magenta, overlapping faces): ${r.nonmanifold_edges}. ` +
              `Both 0 = watertight. Front-face culled, so missing/flipped faces read as see-through holes.`,
          });
        }
        return { content };
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "ground_truth",
    "The kernel's OWN account of a part: PROVENANCE (designed surface vs bare " +
      "primitive), the validity certificate (brep_valid, watertight, manifold, " +
      "euler, sound), and the display-mesh verdict. `designed:false` or " +
      "`sound:false` = stop and fix.",
    { part_id: z.number().int().describe("part id (list_parts)") },
    async ({ part_id }) => {
      try {
        return ok(await api("GET", `/api/agent/parts/${part_id}/truth`));
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "occupancy_view",
    "Non-deceivable SDF X-ray: a slice-stack ('#'=solid, '.'=empty) sampled " +
      "from the EXACT solid — reveals internal cavities, wall thickness and " +
      "through-holes a render hides. Each layer is a z-slice (rows y, cols x).",
    {
      part_id: z.number().int().describe("part id (list_parts)"),
      n: z
        .number()
        .int()
        .optional()
        .default(20)
        .describe("grid resolution per axis (clamped to 4..48)"),
    },
    async ({ part_id, n }) => {
      try {
        const r = await api("GET", `/api/agent/parts/${part_id}/occupancy?n=${n}`);
        const summary = `occupancy n=${r.n} dims=${JSON.stringify(r.dims)} fill_fraction=${r.fill_fraction.toFixed(3)} bbox=${JSON.stringify(r.bbox)}`;
        return {
          content: [
            { type: "text" as const, text: summary },
            { type: "text" as const, text: r.slices },
          ],
        };
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "scene_view",
    "SEE THE WHOLE SCENE: composite every part into one auto-framed image from " +
      "an orbit camera (azimuth/elevation, world-Z up). mode 'diagnostic' " +
      "highlights open (red) / non-manifold (magenta) edges.",
    {
      az: z.number().default(35).describe("azimuth degrees around world Z"),
      el: z.number().default(20).describe("elevation degrees above the horizon"),
      mode: z
        .enum(["shaded", "ids", "depth", "normals", "diagnostic"])
        .default("shaded")
        .describe("render channel ('diagnostic' highlights open/non-manifold edges)"),
      size: z.number().int().min(64).max(2048).default(720).describe("image size in px"),
      quality: z
        .enum(["coarse", "medium", "fine"])
        .default("medium")
        .describe("coarse=fast, fine=resolve curved silhouettes"),
    },
    async ({ az, el, mode, size, quality }) => {
      try {
        const r = await api(
          "GET",
          `/api/agent/scene/orbit?az=${az}&el=${el}&mode=${mode}&size=${size}&quality=${quality}`,
        );
        return {
          content: [
            { type: "image" as const, data: r.png_base64, mimeType: "image/png" },
            {
              type: "text" as const,
              text: `scene az=${az}° el=${el}° dir=${JSON.stringify(r.dir)} open=${r.open_edges} nm=${r.nonmanifold_edges}`,
            },
          ],
        };
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "verify_part",
    "EXPLICIT FULL CERTIFICATE — the expensive checks the ambient verdict skips: " +
      "brep_valid + watertight + manifold + self-intersection-free + " +
      "construction/tessellation/mesh-quality. The authoritative 'is this a real " +
      "closed solid' answer. ALWAYS call after a boolean or multi-feature build. " +
      "Returns a diagnostic image (red=open, magenta=non-manifold).",
    {
      part_id: z.number().int().describe("part id (list_parts)"),
      view: z.enum(["iso", "front", "top", "right"]).default("iso").describe("camera view for the diagnostic image"),
    },
    async ({ part_id, view }) => {
      try {
        // EXPLICIT FULL CERT: ?full=1 runs the complete (O(n²)) certificate. This
        // is the verify path, so the full budget (TIMEOUT_MS) applies — we WANT the
        // expensive checks here. Image + display-mesh counts from the render.
        const p = await api("GET", `/api/agent/parts/${part_id}/perception?full=1`);
        const r = await api(
          "GET",
          `/api/agent/parts/${part_id}/render?mode=diagnostic&view=${view}&size=720`,
        );
        const cert = p?.cert ?? null;
        const valid = (cert?.brep_valid ?? p.valid) === true;
        const sound = p?.sound === true;
        const meshClean = r.open_edges === 0 && r.nonmanifold_edges === 0;
        return {
          content: [
            {
              type: "text",
              text: JSON.stringify(
                {
                  part_id,
                  sound,
                  brep_valid: valid,
                  brep_watertight: (cert?.watertight ?? p.watertight) === true,
                  manifold: cert?.manifold ?? null,
                  self_intersection_free: cert?.self_intersection_free ?? null,
                  tessellation_clean: cert?.tessellation_clean ?? null,
                  mesh_quality_clean: cert?.mesh_quality_clean ?? null,
                  construction_consistent: cert?.construction_consistent ?? null,
                  // Dual-eye gate: "consistent" | "inconsistent" | "not_applicable".
                  // Feeds `sound` — an inconsistent dual-eye means the B-Rep cert and the
                  // mesh cert disagree; the solid is flagged UNSOUND.
                  eyes_consistent: cert?.eyes_consistent ?? "not_applicable",
                  verdict: p?.verdict ??
                    (!valid
                      ? "BROKEN — B-Rep invalid (a real topological defect; see the image)"
                      : meshClean
                        ? "OK — valid closed solid"
                        : "OK — valid solid; display mesh has tessellation T-junctions only (not a defect)"),
                  display_mesh: {
                    open_edges: r.open_edges,
                    nonmanifold_edges: r.nonmanifold_edges,
                    note: "display tessellation quality only — does NOT determine validity",
                  },
                  dims: p.dims ?? null,
                  // Advisory dual-eye reconcile report. {"status":"pending"} when the async
                  // worker has not yet completed for the current solid state. When ready:
                  // {status, discrepancies:[{severity, kind, description}], coverage:{seen,total}}.
                  reconcile: p?.reconcile ?? { status: "pending" },
                  cert: cert ?? undefined,
                },
                null,
                2,
              ),
            },
            { type: "image", data: r.png_base64, mimeType: "image/png" },
          ],
        };
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "get_pointer",
    "What is the HUMAN pointing at in the viewport? Returns their latest click " +
      "(object, face_id, world position) + hover report. Grounds 'this face / here'.",
    {},
    async () => {
      try {
        return ok(await api("GET", "/api/agent/pointer"));
      } catch (e) {
        if (e instanceof ApiError && e.status === 404) {
          return ok({ pointer: null, note: "user has not clicked anything yet" });
        }
        return fail(e);
      }
    },
  );

  server.tool(
    "section_view",
    "CUTAWAY: slice a part with a plane (point `p` + `normal`); returns the " +
      "cross-section image + section area. SEE a hollow interior, wall thickness, bores.",
    {
      part_id: z.number().int().describe("part id (list_parts)"),
      p: z
        .array(z.number()).length(3)
        .default([0, 0, 0])
        .describe("a point on the cutting plane [x,y,z] mm"),
      normal: z
        .array(z.number()).length(3)
        .default([1, 0, 0])
        .describe("the cutting-plane normal [x,y,z]"),
    },
    async ({ part_id, p, normal }) => {
      try {
        const q = `nx=${normal[0]}&ny=${normal[1]}&nz=${normal[2]}&px=${p[0]}&py=${p[1]}&pz=${p[2]}`;
        const r = await api("GET", `/api/agent/parts/${part_id}/section?${q}`);
        return {
          content: [
            { type: "image" as const, data: r.png_base64, mimeType: "image/png" },
            {
              type: "text" as const,
              text: `section area=${r.section_area?.toFixed?.(2)} extent_u=${r.extent_u?.toFixed?.(2)} extent_v=${r.extent_v?.toFixed?.(2)} units=${r.units}`,
            },
          ],
        };
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "dimension_part",
    "DIMENSION a part in one call: a 2×2 multi-view image with leader+label " +
      "callouts AND a structured table (id, kind, value, face ids, 3D anchor), " +
      "including POSITION rows (each bore/boss axis's X/Y offsets from a NAMED " +
      "datum). Values off analytic surfaces, never pixels. Labels follow document_units.",
    { part_id: z.number().int().describe("part id (list_parts)") },
    async ({ part_id }) => {
      try {
        const r = await api("GET", `/api/agent/parts/${part_id}/dimensions`);
        const dims: any[] = r.dimensions ?? [];
        const rows = dims
          .map((d: any) => {
            const base = `${d.id}  ${d.label}  (${d.kind} ${d.value.toFixed(2)}${
              d.unit === "deg" ? "°" : ""
            })  faces=[${d.entities.join(",")}]  @[${d.anchor
              .map((c: number) => c.toFixed(1))
              .join(", ")}]`;
            return d.datum
              ? `${base}  from ${d.datum.name} @[${d.datum.origin
                  .map((c: number) => c.toFixed(1))
                  .join(", ")}]`
              : base;
          })
          .join("\n");
        const overall = `overall L×W×H = ${r.dims.l.toFixed(2)} × ${r.dims.w.toFixed(
          2,
        )} × ${r.dims.h.toFixed(2)} ${r.units}`;
        // One line naming each distinct datum so the reference is explicit.
        const datums = [
          ...new Map(
            dims
              .filter((d: any) => d.datum)
              .map((d: any) => [JSON.stringify(d.datum.origin), d.datum]),
          ).values(),
        ]
          .map(
            (dt: any) =>
              `datum: ${dt.name} (${dt.kind}) @[${dt.origin
                .map((c: number) => c.toFixed(1))
                .join(", ")}]`,
          )
          .join("\n");
        const text = datums ? `${overall}\n${datums}\n${rows}` : `${overall}\n${rows}`;
        return {
          content: [
            { type: "image" as const, data: r.png_base64, mimeType: "image/png" },
            { type: "text" as const, text },
          ],
        };
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "measure_faces",
    "MEASURE the exact relation of two faces (or inspect one): plane‖plane → " +
      "distance; angled planes → dihedral (0–180°); parallel cylinder axes → " +
      "centre distance (bolt circles); axis ⟂ plane → axis-to-plane distance; " +
      "one cylinder → Ø; one plane → area+normal. Kernel-exact; REFUSES on " +
      "skew/consumed faces. Face ids from select_face / render 'ids'.",
    {
      part_a: z.number().int().describe("kernel part id of the first face's solid"),
      face_a: z.number().int().describe("first face id"),
      part_b: z.number().int().optional().describe("second face's solid (omit with face_b for single-face info)"),
      face_b: z.number().int().optional().describe("second face id (omit for single-face info)"),
    },
    async ({ part_a, face_a, part_b, face_b }) => {
      try {
        const body: Record<string, unknown> = {
          a: { part_id: part_a, kind: "face", id: face_a },
          b:
            face_b !== undefined
              ? { part_id: part_b ?? part_a, kind: "face", id: face_b }
              : null,
        };
        const r = await api("POST", "/api/agent/measure", body);
        return ok({
          kind: r.kind,
          relation: r.relation ?? null,
          value: r.value,
          unit: r.unit,
          label: r.label,
          anchor: r.anchor,
          direction: r.direction ?? null,
          entities: r.entities,
        });
      } catch (e) {
        // Surface kernel refusals (422) with the verbatim reason — the honest
        // "cannot measure that" answer, distinct from transport errors.
        if (e instanceof ApiError && (e.status === 422 || e.status === 404)) {
          try {
            const body = JSON.parse(e.body);
            return fail(new Error(`REFUSED: ${body.reason ?? e.body}`));
          } catch {
            return fail(e);
          }
        }
        return fail(e);
      }
    },
  );

  server.tool(
    "part_coverage",
    "COVERAGE honesty: which faces the 4 standard views SHOW vs leave unseen — " +
      "so you know when to request another camera angle.",
    { part_id: z.number().int().describe("part id (list_parts)") },
    async ({ part_id }) => {
      try {
        return ok(await api("GET", `/api/agent/parts/${part_id}/coverage`));
      } catch (e) {
        return fail(e);
      }
    },
  );
}
