/** Inspection tools — read facts off the live model: parts, faces, features. */

import type { ToolHost } from "../registry.js";
import { z } from "zod";
import { api, ok, fail, BASE, ApiError, setDocumentUnitCache, scopeHeaders, readRefusable, PART_ID_DOC } from "../core.js";

export function registerInspectTools(server: ToolHost) {
  server.tool(
    "list_parts",
    "List every live part: integer part_id (what part_id args take), name, kind. " +
      "boolean/transform/shell take an object_uuid instead — the one returned by " +
      "the call that made the solid.",
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
    "Full report for one part: world placement in mm (center, dimensions, " +
      "anchor datum), topology fingerprint, name.",
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
    "VERIFY a math claim against kernel GROUND TRUTH. SPEC-FIRST pattern: " +
      "declare the checkable invariants a build must satisfy (an area ratio, " +
      "a volume, a wall relation) BEFORE building, then check each one here " +
      "after — the kernel measures; never self-grade from your own numbers. " +
      "Bind each variable in `expr` to an exact measurement, assert " +
      "`expected`; evaluated deterministically (NOT an LLM). Verdict: " +
      "verified | false (with abs_error) | refused (a binding didn't resolve " +
      "— never a silent pass). Pairs with verify_part: that certifies the " +
      "topology is sound; this certifies the numbers you promised.",
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
    { part_id: z.number().int().describe(PART_ID_DOC) },
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
      part_a: z.number().int().describe(PART_ID_DOC),
      part_b: z.number().int().describe(PART_ID_DOC),
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
    { part_id: z.number().int().describe(PART_ID_DOC) },
    async ({ part_id }) => {
      try {
        return ok(await api("GET", `/api/agent/parts/${part_id}/features`));
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "dfm_check",
    "DFM (design-for-manufacturability): can this part be made by the process, " +
      "and what will hurt? Runs a rule pack against the part's REAL B-Rep, " +
      "exact-or-refuse — never a fabricated pass. ENFORCED: refused (422) if " +
      "the part has been mutated (or never certified) since its last full " +
      "verification, or is certified unsound — call verify_part first. Packs: " +
      "'fdm' (overhang, min_wall, min_bore, trapped_volume); 'injection_molding' " +
      "(draft, side_action, trapped_core, sharp_internal_corner); 'cnc_3_axis' " +
      "(internal_corner_radius vs the smallest tool, corner_reach L/D, " +
      "hole_depth_ratio, min_hole_diameter, standard_drill_size, " +
      "blind_hole_floor, hole_access vs your setup_directions, min_wall); " +
      "'cnc_turning' (axisymmetry about the spindle, bore_depth_ratio, " +
      "min_wall); 'sheet_metal' (uniform_thickness, bend_radius, hole_to_edge, " +
      "hole_to_bend, flange_length, developable); 'casting' (draft, min_wall, " +
      "max_wall, internal_fillet, cored_hole, side_core, trapped_core). Every " +
      "threshold is a parameter with a cited default (shop practice, never " +
      "dressed up as a standard). Returns the kernel's DfmReport verbatim: " +
      "per-rule verdicts (pass | violation | unverifiable, each with " +
      "measured/limit values, the faces involved and the threshold's " +
      "provenance), a summary that is Pass ONLY if every rule passed, and " +
      "'animations' — one explanation per violation (tool approach, drill " +
      "plunge, setup cycle, mould separation, layer build, caliper, spindle " +
      "spin) built from the same measured values.",
    {
      part_id: z.number().int().describe(PART_ID_DOC),
      pack: z
        .enum(["fdm", "injection_molding", "cnc_3_axis", "cnc_turning", "sheet_metal", "casting"])
        .describe("which rule pack to evaluate"),
      nozzle_diameter: z
        .number()
        .optional()
        .describe("fdm pack only; mm, default 0.4"),
      build_direction: z
        .array(z.number()).length(3)
        .optional()
        .describe("fdm pack only; unit build-up direction, default [0,0,1]"),
      pull_direction: z
        .array(z.number()).length(3)
        .optional()
        .describe("injection_molding / casting; unit mold-pull direction, default [0,0,1]"),
      min_draft_deg: z
        .number()
        .optional()
        .describe("injection_molding / casting; minimum draft angle in degrees, default 1.0"),
      setup_directions: z
        .array(z.array(z.number()).length(3))
        .optional()
        .describe(
          "cnc_3_axis; the setup plan — unit vectors from the part toward the spindle, " +
            "default [[0,0,1],[0,0,-1]] (top op, flip). Add [1,0,0] etc. for side ops.",
        ),
      min_tool_diameter: z
        .number()
        .optional()
        .describe("cnc_3_axis; smallest end mill in mm, default 3.0"),
      spindle_axis: z
        .array(z.array(z.number()).length(3))
        .length(2)
        .optional()
        .describe("cnc_turning; [[point],[direction]]; default inferred from the OD (refused if not unique)"),
      thickness: z
        .number()
        .optional()
        .describe("sheet_metal; gauge in mm; default inferred from proven wall pairs (refused when unprovable)"),
      params: z
        .record(z.unknown())
        .optional()
        .describe(
          "any other pack threshold by its kernel field name (e.g. max_drill_depth_ratio, " +
            "min_wall, max_wall, hole_edge_factor, allow_flat_bottom_holes); omitted = cited default",
        ),
    },
    async (args) => {
      try {
        return ok(await api("POST", `/api/agent/parts/${args.part_id}/dfm`, dfmBody(args)));
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
      part_id: z.number().int().describe(PART_ID_DOC),
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
        // 404 (not_found) / 409 (ambiguous) are the kernel's meaningful
        // REFUSALS: returned as an ERROR carrying the full body (the candidate
        // list included), never success-shaped (readRefusable).
        const res = await fetch(`${BASE}/api/agent/parts/${part_id}/select-face`, {
          method: "POST",
          headers: { "Content-Type": "application/json", ...scopeHeaders() },
          body: JSON.stringify({
            kind,
            normal_dir: normal_dir ?? null,
            extremal,
            along: along ?? null,
            angle_tol_deg,
          }),
        });
        return ok(await readRefusable(res, `select_face on part ${part_id}`));
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
      part_id: z.number().int().describe(PART_ID_DOC),
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
          headers: { "Content-Type": "application/json", ...scopeHeaders() },
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
        return ok(await readRefusable(res, `select_edge on part ${part_id}`));
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
      part_id: z.number().int().describe(PART_ID_DOC),
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

  server.tool(
    "document_rename",
    "Rename a document's catalog entry — display name only, never geometry, " +
      "timeline, or id (`api-server/src/documents.rs::rename_document`, " +
      "`PATCH /api/documents/{id}`). Omit `document_id` to rename the currently " +
      "active document; pass it to rename any other registered one (see list from " +
      "the document tab strip or an agent that tracked an id earlier).",
    {
      name: z
        .string()
        .describe("new display name — trimmed server-side; non-empty; ≤200 chars; no control characters"),
      document_id: z
        .string()
        .optional()
        .describe("document id to rename; omit to rename the currently active document"),
    },
    async ({ name, document_id }) => {
      try {
        let id: string;
        if (document_id !== undefined) {
          id = document_id;
        } else {
          const docs = await api("GET", "/api/documents");
          const active = Array.isArray(docs)
            ? docs.find((d: any) => d?.active === true)
            : undefined;
          if (!active || typeof active.id !== "string") {
            return fail(new Error("no active document found — pass document_id explicitly"));
          }
          id = active.id;
        }
        const r = await api("PATCH", `/api/documents/${encodeURIComponent(id)}`, { name });
        return ok(r);
      } catch (e) {
        return fail(e);
      }
    },
  );
}

/** dfm_check's arguments → the kernel's `PackParams` body (tagged by
 *  `pack`). The two original packs keep their explicit defaults; the
 *  process packs send only what the caller set — every omitted threshold
 *  takes the kernel's cited default, echoed back on the report. */
export function dfmBody(args: {
  pack: string;
  nozzle_diameter?: number;
  build_direction?: number[];
  pull_direction?: number[];
  min_draft_deg?: number;
  setup_directions?: number[][];
  min_tool_diameter?: number;
  spindle_axis?: number[][];
  thickness?: number;
  params?: Record<string, unknown>;
}): Record<string, unknown> {
  const { pack } = args;
  if (pack === "fdm") {
    return {
      pack,
      nozzle_diameter: args.nozzle_diameter ?? 0.4,
      build_direction: args.build_direction ?? [0, 0, 1],
    };
  }
  if (pack === "injection_molding") {
    return {
      pack,
      pull_direction: args.pull_direction ?? [0, 0, 1],
      min_draft_deg: args.min_draft_deg ?? 1.0,
    };
  }
  const body: Record<string, unknown> = { ...(args.params ?? {}), pack };
  const set = (k: string, v: unknown) => {
    if (v !== undefined) body[k] = v;
  };
  if (pack === "cnc_3_axis") {
    set("setup_directions", args.setup_directions);
    set("min_tool_diameter", args.min_tool_diameter);
  } else if (pack === "cnc_turning") {
    set("spindle_axis", args.spindle_axis);
  } else if (pack === "sheet_metal") {
    set("thickness", args.thickness);
  } else if (pack === "casting") {
    set("pull_direction", args.pull_direction);
    set("min_draft_deg", args.min_draft_deg);
  }
  return body;
}
