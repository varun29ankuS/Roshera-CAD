/** Inspection tools — read facts off the live model: parts, faces, features. */

import type { ToolHost } from "../registry.js";
import { z } from "zod";
import { api, ok, fail, BASE, ApiError, setDocumentUnitCache, AUTH_HEADERS } from "../core.js";

export function registerInspectTools(server: ToolHost) {
  server.tool(
    "list_parts",
    "List every part in the live model (id, name, kind).",
    {},
    async () => {
      try {
        return ok(await api("GET", "/api/agent/parts"));
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "get_part",
    "Full report for one part: world placement (center, dimensions, anchor " +
      "datum), topology fingerprint, name.",
    { part_id: z.number().int().describe("part id (list_parts)") },
    async ({ part_id }) => {
      try {
        return ok(await api("GET", `/api/agent/parts/${part_id}`));
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "mass_properties",
    "Exact mass properties: volume (mm³), mass (kg), centre of mass (mm), " +
      "inertia tensor + principal moments (kg·mm²), principal axes " +
      "(accuracy-gated). The response's 'units' object carries per-field labels.",
    { part_id: z.number().int().describe("part id (list_parts)") },
    async ({ part_id }) => {
      try {
        return ok(await api("GET", `/api/agent/parts/${part_id}/mass`));
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "verify_claim",
    "VERIFY a math claim against kernel GROUND TRUTH. Bind each variable in " +
      "`expr` to an exact measurement, assert `expected`; evaluated " +
      "deterministically (NOT an LLM). Verdict: verified | false (with " +
      "abs_error) | refused (a binding didn't resolve — never a silent pass).",
    {
      expr: z
        .string()
        .describe("math expression over the binding variable names, e.g. 'a_exit / a_throat'"),
      bindings: z
        .array(
          z.object({
            var: z.string().describe("variable name used in expr"),
            measure: z
              .object({
              kind: z
                .enum([
                  "volume",
                  "surface_area",
                  "face_area",
                  "edge_length",
                  "constant",
                ])
                .describe("what exact quantity to measure"),
              part: z
                .string()
                .optional()
                .describe("part object UUID — for volume / surface_area"),
              face: z.number().int().optional().describe("face id — for face_area"),
              edge: z.number().int().optional().describe("edge id — for edge_length"),
              value: z.number().optional().describe("the value — for constant"),
            })
              .describe("the exact quantity to bind this variable to"),
          }),
        )
        .describe("variable→measurement bindings"),
      expected: z.number().describe("the value the expression should equal"),
      tolerance: z
        .number()
        .optional()
        .describe("absolute tolerance; omit for auto (1e-6 relative)"),
    },
    async ({ expr, bindings, expected, tolerance }) => {
      try {
        return ok(
          await api("POST", "/api/agent/verify-claim", {
            expr,
            bindings,
            expected,
            ...(tolerance !== undefined ? { tolerance } : {}),
          }),
        );
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "get_revolve_profile",
    "RECOVER the editable [r,z] meridian a revolved part was built from. Read " +
      "it, edit a radius, call revolve again to REGENERATE. 404 if not a revolve.",
    { part_id: z.number().int().describe("part id (list_parts)") },
    async ({ part_id }) => {
      try {
        return ok(await api("GET", `/api/agent/parts/${part_id}/profile`));
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "get_face",
    "Per-face report: surface type, area, principal curvatures, boundary edges, " +
      "neighbours. Face ids from render_part 'ids' or get_pointer.",
    { face_id: z.number().int().describe("kernel face id (render 'ids' legend or get_pointer)") },
    async ({ face_id }) => {
      try {
        return ok(await api("GET", `/api/agent/faces/${face_id}`));
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "part_distance",
    "MEASURE two parts from their world AABBs: gap (clearance), overlap " +
      "(penetration), centre distance, unit direction a→b. For clearance/nudge decisions.",
    {
      part_a: z.number().int().describe("part id (list_parts)"),
      part_b: z.number().int().describe("part id (list_parts)"),
    },
    async ({ part_a, part_b }) => {
      try {
        return ok(await api("GET", `/api/agent/parts/distance/${part_a}/${part_b}`));
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "part_features",
    "READ analytic FEATURE sizes off the B-Rep: per-face feature dimensions " +
      "(cylinder diameters + axes for bores/bosses, plane normals) + a distinct-" +
      "diameter summary. Exact, never from pixels.",
    { part_id: z.number().int().describe("part id (list_parts)") },
    async ({ part_id }) => {
      try {
        return ok(await api("GET", `/api/agent/parts/${part_id}/features`));
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "select_face",
    "Address a face by DESCRIPTION — resolves it or REFUSES (never picks among " +
      "equal matches). Returns face_id + persistent_id, or ambiguous / not_found.",
    {
      part_id: z.number().int().describe("part id (list_parts)"),
      kind: z
        .enum(["any", "planar", "cylindrical", "spherical", "conical", "toroidal", "nurbs"])
        .default("any")
        .describe("surface-type filter"),
      normal_dir: z
        .array(z.number()).length(3)
        .optional()
        .describe("keep faces whose normal aligns with [x,y,z] (within angle_tol_deg)"),
      extremal: z
        .enum(["none", "largest_area", "smallest_area", "most_along"])
        .default("none")
        .describe("tie-breaker (most_along uses `along`)"),
      along: z
        .array(z.number()).length(3)
        .optional()
        .describe("direction for most_along (default: normal_dir, else +Z)"),
      angle_tol_deg: z.number().default(12).describe("normal match tolerance (degrees)"),
    },
    async ({ part_id, kind, normal_dir, extremal, along, angle_tol_deg }) => {
      try {
        // Read the body regardless of status: 404 (not_found) / 409 (ambiguous)
        // are the kernel's meaningful REFUSALS, not transport errors.
        const res = await fetch(`${BASE}/api/agent/parts/${part_id}/select-face`, {
          method: "POST",
          headers: { "Content-Type": "application/json", ...AUTH_HEADERS },
          body: JSON.stringify({
            kind,
            normal_dir: normal_dir ?? null,
            extremal,
            along: along ?? null,
            angle_tol_deg,
          }),
        });
        const j = await res.json();
        return ok(j);
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "select_edge",
    "Address an EDGE by description — resolves or REFUSES. Returns edge_id + " +
      "persistent_id, or ambiguous / not_found.",
    {
      part_id: z.number().int().describe("part id (list_parts)"),
      curve_kind: z
        .enum(["any", "line", "arc", "circle", "nurbs"])
        .default("any")
        .describe("edge curve-type filter"),
      blend: z
        .enum(["any", "filleted", "chamfered", "unblended"])
        .default("any")
        .describe("keep only already filleted/chamfered/unblended edges"),
      convexity: z
        .enum(["any", "convex", "concave"])
        .default("any")
        .describe("concave = re-entrant edges (target to fillet a concave corner)"),
      direction: z
        .array(z.number()).length(3)
        .optional()
        .describe("keep edges whose tangent aligns with [x,y,z] (within angle_tol_deg)"),
      extremal: z
        .enum(["none", "longest", "shortest", "most_along"])
        .default("none")
        .describe("tie-breaker (most_along uses `along`)"),
      along: z
        .array(z.number()).length(3)
        .optional()
        .describe("direction for most_along (default: direction, else +Z)"),
      angle_tol_deg: z.number().default(12).describe("tangent match tolerance (degrees)"),
    },
    async ({ part_id, curve_kind, blend, convexity, direction, extremal, along, angle_tol_deg }) => {
      try {
        const res = await fetch(`${BASE}/api/agent/parts/${part_id}/select-edge`, {
          method: "POST",
          headers: { "Content-Type": "application/json", ...AUTH_HEADERS },
          body: JSON.stringify({
            curve_kind,
            blend,
            convexity,
            direction: direction ?? null,
            extremal,
            along: along ?? null,
            angle_tol_deg,
          }),
        });
        return ok(await res.json());
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "set_part_color",
    "Set a part's display RGB for scene_view renders. Registry-only — does NOT " +
      "modify geometry.",
    {
      part_id: z.number().int().describe("part id (list_parts)"),
      r: z.number().int().min(0).max(255).describe("red 0–255"),
      g: z.number().int().min(0).max(255).describe("green 0–255"),
      b: z.number().int().min(0).max(255).describe("blue 0–255"),
    },
    async ({ part_id, r, g, b }) => {
      try {
        const res = await api("POST", `/api/agent/parts/${part_id}/color`, { r, g, b });
        return ok(res);
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "document_units",
    "Display-only document unit (mm/cm/m/in/ft). Model stays mm-native; " +
      "switching re-renders labels/verdicts at display time — geometry and stored " +
      "values are never converted. No arg → GET; with arg → PATCH and echo. " +
      "dimension_part / measure_faces labels follow this.",
    {
      unit: z
        .enum(["mm", "cm", "m", "in", "ft"])
        .optional()
        .describe("omit to read; provide to set"),
    },
    async ({ unit }) => {
      try {
        if (unit === undefined) {
          // GET current unit
          const r = await api("GET", "/api/document/units");
          setDocumentUnitCache(r.unit);
          return ok({ unit: r.unit });
        }
        // PATCH new unit
        const r = await api("PATCH", "/api/document/units", { unit });
        setDocumentUnitCache(r.unit);
        return ok({ unit: r.unit });
      } catch (e) {
        // Surface 400 refusals verbatim (invalid unit token from backend).
        if (e instanceof ApiError && e.status === 400) {
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
}
