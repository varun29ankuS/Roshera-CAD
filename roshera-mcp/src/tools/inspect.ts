/** Inspection tools — read facts off the live model: parts, faces, features. */

import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import { api, ok, fail, BASE } from "../core.js";

export function registerInspectTools(server: McpServer) {
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
    { part_id: z.number().int() },
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
    "Exact mass properties: volume, mass, center of mass, inertia tensor, " +
      "principal axes (mesh-integrated, accuracy-gated against closed form).",
    { part_id: z.number().int() },
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
      "`expr` to an exact measurement (volume / surface_area / face_area / " +
      "edge_length / constant), assert `expected`; evaluated deterministically " +
      "(NOT an LLM). Three-state verdict: verified | false (with abs_error) | " +
      "refused (a binding could not resolve — never a silent pass).",
    {
      expr: z
        .string()
        .describe("math expression over the binding variable names, e.g. 'a_exit / a_throat'"),
      bindings: z
        .array(
          z.object({
            var: z.string().describe("variable name used in expr"),
            measure: z.object({
              kind: z.enum([
                "volume",
                "surface_area",
                "face_area",
                "edge_length",
                "constant",
              ]),
              part: z
                .string()
                .optional()
                .describe("part object UUID — for volume / surface_area"),
              face: z.number().int().optional().describe("face id — for face_area"),
              edge: z.number().int().optional().describe("edge id — for edge_length"),
              value: z.number().optional().describe("the value — for constant"),
            }),
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
    "RECOVER the editable meridian a revolved part was built from — the [r,z] " +
      "half-plane profile. Read it, edit a radius, call revolve again to " +
      "REGENERATE (the edit→regenerate loop). 404 if not built by a revolve.",
    { part_id: z.number().int() },
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
    "Per-face report: surface type, area, principal curvatures, boundary " +
      "edges, neighbours. Face ids come from render_part mode 'ids' or " +
      "get_pointer.",
    { face_id: z.number().int() },
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
    "MEASURE two parts' spatial relationship from their world AABBs: gap " +
      "(clearance), overlap (penetration), center distance, and the unit " +
      "direction a→b. For clearance checks and nudge decisions.",
    {
      part_a: z.number().int(),
      part_b: z.number().int(),
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
    "READ analytic FEATURE sizes off the B-Rep: every face's feature dimension " +
      "(cylinder diameters + axes for bores/bosses, plane normals) plus a " +
      "distinct-diameter summary. Exact values, never measured from pixels.",
    { part_id: z.number().int().describe("kernel part id from list_parts") },
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
    "Address a face by DESCRIPTION — the kernel resolves it or REFUSES (never " +
      "picks among equal matches). Give a surface `kind`, optional `normal_dir`, " +
      "optional `extremal` tie-breaker. Returns face_id + persistent_id, or " +
      "`ambiguous` (candidates) / `not_found`.",
    {
      part_id: z.number().int(),
      kind: z
        .enum(["any", "planar", "cylindrical", "spherical", "conical", "toroidal", "nurbs"])
        .default("any"),
      normal_dir: z.tuple([z.number(), z.number(), z.number()]).optional(),
      extremal: z
        .enum(["none", "largest_area", "smallest_area", "most_along"])
        .default("none"),
      along: z.tuple([z.number(), z.number(), z.number()]).optional(),
      angle_tol_deg: z.number().default(12),
    },
    async ({ part_id, kind, normal_dir, extremal, along, angle_tol_deg }) => {
      try {
        // Read the body regardless of status: 404 (not_found) / 409 (ambiguous)
        // are the kernel's meaningful REFUSALS, not transport errors.
        const res = await fetch(`${BASE}/api/agent/parts/${part_id}/select-face`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
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
    "Address an EDGE by description — resolves or REFUSES. `curve_kind`, a " +
      "`blend` filter (filleted/chamfered/unblended), optional `direction`, " +
      "optional `extremal`. Returns edge_id + persistent_id, or `ambiguous` / " +
      "`not_found`.",
    {
      part_id: z.number().int(),
      curve_kind: z.enum(["any", "line", "arc", "circle", "nurbs"]).default("any"),
      blend: z.enum(["any", "filleted", "chamfered", "unblended"]).default("any"),
      direction: z.tuple([z.number(), z.number(), z.number()]).optional(),
      extremal: z.enum(["none", "longest", "shortest", "most_along"]).default("none"),
      along: z.tuple([z.number(), z.number(), z.number()]).optional(),
      angle_tol_deg: z.number().default(12),
    },
    async ({ part_id, curve_kind, blend, direction, extremal, along, angle_tol_deg }) => {
      try {
        const res = await fetch(`${BASE}/api/agent/parts/${part_id}/select-edge`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            curve_kind,
            blend,
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
      part_id: z.number().int().describe("kernel part id from list_parts"),
      r: z.number().int().min(0).max(255),
      g: z.number().int().min(0).max(255),
      b: z.number().int().min(0).max(255),
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
}
