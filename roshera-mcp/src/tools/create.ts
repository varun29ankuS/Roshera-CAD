/** Creation tools — sketches, composite primitives, revolve, loft. */

import type { ToolHost } from "../registry.js";
import { z } from "zod";
import {
  api,
  ok,
  fail,
  okp,
  placement,
  newestPartId,
  PlaneSchema,
  resolvePlane,
  cross3,
} from "../core.js";

export function registerCreateTools(server: ToolHost) {
  server.tool(
    "create_sketch",
    "Start a click-draft sketch session on a plane; returns sketch_id for " +
      "sketch_points/sketch_add_shape/sketch_extrude. Prefer create_box / " +
      "create_cylinder when they fit (fewer round trips); prefer psketch_* for " +
      "constraint-exact geometry.",
    {
      plane: PlaneSchema,
      tool: z
        .enum(["rectangle", "circle", "polyline"])
        .describe("first shape's kind"),
    },
    async ({ plane, tool }) => {
      try {
        const s = await api("POST", "/api/sketch", { plane, tool });
        return ok({ sketch_id: s.id, shapes: s.shapes?.length ?? 1 });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "sketch_add_shape",
    "Add another shape to an existing sketch (e.g. hole circles inside an outer " +
      "boundary — outer/hole roles are assigned at extrude time). Returns the " +
      "new shape_index.",
    {
      sketch_id: z.string().describe("sketch_id from create_sketch"),
      tool: z
        .enum(["rectangle", "circle", "polyline"])
        .describe("shape kind to add"),
    },
    async ({ sketch_id, tool }) => {
      try {
        const s = await api("POST", `/api/sketch/${sketch_id}/shape`, { tool });
        return ok({ shape_index: (s.shapes?.length ?? 1) - 1 });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "sketch_points",
    "BATCH-add plane-local points to a sketch shape in one call.",
    {
      sketch_id: z.string().describe("sketch_id from create_sketch"),
      points: z
        .array(z.array(z.number()).length(2))
        .min(1)
        .describe(
          "plane-local [u,v] points (mm). rectangle: 2 opposite corners; " +
            "circle: center then a rim point; polyline: every vertex of the closed polygon",
        ),
      shape_index: z
        .number()
        .int()
        .optional()
        .describe("target shape (from sketch_add_shape); omit = first shape"),
    },
    async ({ sketch_id, points, shape_index }) => {
      try {
        const base =
          shape_index === undefined
            ? `/api/sketch/${sketch_id}/point`
            : `/api/sketch/${sketch_id}/shape/${shape_index}/point`;
        for (const p of points) {
          await api("POST", base, { point: p });
        }
        return ok({ added: points.length });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "sketch_extrude",
    "Extrude the sketch into a solid along the plane normal. Multi-shape " +
      "sketches get region detection (outer boundary + holes). Returns the new " +
      "part id + placement.",
    {
      sketch_id: z.string().describe("sketch_id from create_sketch"),
      distance: z
        .number()
        .describe("extrusion length (mm) along the plane normal; sign sets direction"),
      name: z.string().optional().describe("display name"),
    },
    async ({ sketch_id, distance, name }) => {
      try {
        await api("POST", `/api/sketch/${sketch_id}/extrude`, {
          distance,
          name: name ?? null,
        });
        const id = await newestPartId();
        return await okp({ part_id: id, placement: id !== null ? await placement(id) : null }, id);
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "plane_from_face",
    "Derive a sketch plane FROM an existing planar face ('sketch on this face'). " +
      "Returns {origin, u_axis, v_axis} to pass as a `plane` to create_*/create_sketch.",
    {
      object_id: z.string().uuid().describe("the part's public object UUID"),
      face_id: z
        .number()
        .int()
        .describe("planar face id from get_pointer or a render 'ids' legend"),
    },
    async ({ object_id, face_id }) => {
      try {
        return ok(
          await api("POST", "/api/sketch/plane-from-face", { object_id, face_id }),
        );
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.registerTool(
    "create_box",
    {
      description:
        "ONE-CALL analytic box: `width`×`depth` on the plane, extruded `height` " +
        "along +normal. The box BASE sits at the base-centre; it is NOT centred " +
        "on the base point. Returns part id + placement.",
      inputSchema: z
        .object({
          plane: PlaneSchema.default("xy").describe("orientation: width→u, depth→v, height→u×v"),
          cx: z.number().default(0).describe("base-centre u offset (mm)"),
          cy: z.number().default(0).describe("base-centre v offset (mm)"),
          center: z
            .array(z.number()).length(3)
            .optional()
            .describe("explicit world base-centre [x,y,z] mm; overrides plane+cx+cy"),
          width: z.number().positive().describe("size along u (mm)"),
          depth: z.number().positive().describe("size along v (mm)"),
          height: z.number().describe("extrusion along normal (mm)"),
          name: z.string().optional().describe("display name"),
        })
        .strict(),
    },
    async ({ plane, cx, cy, center, width, depth, height, name }) => {
      try {
        // Base centre: explicit `center` wins; otherwise plane origin + cx·u + cy·v.
        // Orientation (u/v axes and the height direction u×v) always follows the plane.
        const p = resolvePlane(plane);
        const base =
          center ?? [0, 1, 2].map((i) => p.o[i] + cx * p.u[i] + cy * p.v[i]);
        const r = await api("POST", "/api/geometry/box", {
          center: base,
          u_axis: p.u,
          v_axis: p.v,
          width,
          depth,
          height,
          name: name ?? null,
        });
        const id = await newestPartId();
        return await okp(
          {
            object_uuid: r.object?.id ?? null, // operand for boolean / transform
            part_id: id,
            placement: id !== null ? await placement(id) : null,
          },
          id,
        );
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.registerTool(
    "create_cylinder",
    {
      description:
        "ONE-CALL analytic cylinder with one smooth lateral face. Its BASE-face " +
        "centre sits at `center` (or cx,cy on the plane); it extrudes `height` " +
        "along +`axis`, NOT centred on the base point. Returns part id + placement.",
      inputSchema: z
        .object({
          plane: PlaneSchema.default("xy").describe("orientation when center/axis omitted"),
          cx: z.number().default(0).describe("base-centre u offset (mm)"),
          cy: z.number().default(0).describe("base-centre v offset (mm)"),
          center: z
            .array(z.number()).length(3)
            .optional()
            .describe("explicit world base-centre [x,y,z] mm; overrides plane+cx+cy"),
          axis: z
            .array(z.number()).length(3)
            .optional()
            .describe("extrusion direction [x,y,z]; default = plane normal"),
          radius: z.number().positive().describe("radius (mm)"),
          height: z.number().describe("extrusion length along +axis (mm)"),
          name: z.string().optional().describe("display name"),
        })
        .strict(),
    },
    async ({ plane, cx, cy, center, axis, radius, height, name }) => {
      try {
        // Base centre: explicit `center` wins; otherwise plane origin + cx·u + cy·v.
        // Axis: explicit `axis` wins; otherwise u × v (the plane normal).
        const p = resolvePlane(plane);
        const base =
          center ?? [0, 1, 2].map((i) => p.o[i] + cx * p.u[i] + cy * p.v[i]);
        const dir = axis ?? cross3(p.u, p.v);
        const r = await api("POST", "/api/geometry/cylinder", {
          center: base,
          axis: dir,
          radius,
          height,
          name: name ?? null,
        });
        const id = await newestPartId();
        return await okp(
          {
            object_uuid: r.object?.id ?? null, // operand for boolean / transform
            part_id: id,
            placement: id !== null ? await placement(id) : null,
          },
          id,
        );
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "create_cone",
    "ONE-CALL analytic cone or frustum with a true smooth cone surface. " +
      "`base_radius` sits at `center`; `top_radius` (0 = sharp apex) at " +
      "center+axis·height. Returns part id + placement.",
    {
      center: z
        .array(z.number()).length(3)
        .default([0, 0, 0])
        .describe("world base-face centre [x,y,z] mm"),
      axis: z
        .array(z.number()).length(3)
        .default([0, 0, 1])
        .describe("apex direction [x,y,z]"),
      base_radius: z.number().nonnegative().describe("base radius (mm)"),
      top_radius: z.number().nonnegative().default(0).describe("top radius (mm); 0 = apex"),
      height: z.number().positive().describe("base-to-top along axis (mm)"),
      name: z.string().optional().describe("display name"),
    },
    async ({ center, axis, base_radius, top_radius, height, name }) => {
      try {
        const r = await api("POST", "/api/geometry/cone", {
          center,
          axis,
          base_radius,
          top_radius,
          height,
          name: name ?? null,
        });
        const id = r.solid_id ?? (await newestPartId());
        return await okp(
          {
            object_uuid: r.object?.id ?? null, // operand for boolean / transform
            part_id: id,
            placement: id !== null ? await placement(id) : null,
          },
          id,
        );
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.registerTool(
    "create_sphere",
    {
      description:
        "ONE-CALL analytic sphere of `radius` at `center`. Returns part id + placement.",
      inputSchema: z
        .object({
          radius: z.number().positive().describe("sphere radius (mm)"),
          center: z
            .array(z.number()).length(3)
            .optional()
            .describe("world centre [x,y,z] mm; defaults to the origin"),
          name: z.string().optional().describe("display name"),
        })
        .strict(),
    },
    async ({ radius, center }) => {
      try {
        const r = await api("POST", "/api/geometry", {
          shape_type: "sphere",
          parameters: { radius },
          // Top-level `position` → the kernel builds the sphere there
          // (world-absolute), matching create_cylinder/create_cone.
          position: center ?? [0, 0, 0],
        });
        const id = await newestPartId();
        return await okp(
          {
            object_uuid: r.object?.id ?? null, // operand for boolean / transform
            part_id: id,
            placement: id !== null ? await placement(id) : null,
          },
          id,
        );
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "revolve",
    "Solid of revolution from a closed [r,z] meridian (mm) about an axis — " +
      "axisymmetric parts (nozzles, vessels) watertight in one op. Give ONE of " +
      "`profile` (sampled polyline) or `profile_segments` (typed line/arc/nurbs, " +
      "each revolved to an EXACT surface — the right mode for nozzles/vessels). " +
      "Loop must be simple, r≥0, not crossing the axis; hollow = trace the wall " +
      "section. smooth/bore_radius/wall_thickness apply to `profile` only.",
    {
      profile: z
        .array(z.array(z.number()).length(2))
        .min(3)
        .optional()
        .describe("closed [r,z] meridian (mm); r=from axis, z=along axis; auto-closes"),
      profile_segments: z
        .array(
          z.discriminatedUnion("type", [
            z.object({
              type: z.literal("line"),
              start: z.array(z.number()).length(2).describe("[r,z] mm"),
              end: z.array(z.number()).length(2).describe("[r,z] mm"),
            }),
            z.object({
              type: z.literal("arc"),
              center: z.array(z.number()).length(2).describe("centre [r,z] mm"),
              radius: z.number().positive().describe("mm"),
              start_angle: z.number().describe("radians (+r→+z)"),
              end_angle: z.number().describe("radians (+r→+z)"),
              ccw: z.boolean().describe("sweep start→end sense"),
            }),
            z.object({
              type: z.literal("nurbs"),
              degree: z.number().int().min(1).max(7).describe("degree"),
              control_points: z
                .array(z.array(z.number()).length(2))
                .min(2)
                .describe("[r,z] CPs mm"),
              weights: z.array(z.number()).optional().describe("rational weights, one per CP"),
              knots: z.array(z.number()).describe("knot vector (CPs+degree+1)"),
            }),
          ]),
        )
        .min(1)
        .optional()
        .describe(
          "typed [r,z] segments in loop order (auto-closes): line→cylinder/cone/" +
            "cap, arc→torus/sphere, nurbs→smooth wall. Full-360° ONLY; exclusive " +
            "with profile/smooth/bore_radius/wall_thickness",
        ),
      axis_origin: z
        .array(z.number()).length(3)
        .default([0, 0, 0])
        .describe("point on the axis [x,y,z] mm"),
      axis_direction: z
        .array(z.number()).length(3)
        .default([0, 0, 1])
        .describe("axis direction [x,y,z]"),
      angle_deg: z.number().default(360).describe("sweep degrees (profile_segments must be 360)"),
      segments: z.number().int().min(3).max(512).default(96).describe("angular tessellation count"),
      smooth: z
        .boolean()
        .optional()
        .describe("`profile` mode: fit a smooth NURBS wall (needs bore_radius)"),
      bore_radius: z.number().optional().describe("hollow bore radius (mm) for smooth=true"),
      wall_thickness: z
        .number()
        .optional()
        .describe("contoured mode: `profile` = inner contour, outer offset by this (mm)"),
      name: z.string().optional().describe("display name"),
    },
    async ({
      profile,
      profile_segments,
      axis_origin,
      axis_direction,
      angle_deg,
      segments,
      smooth,
      bore_radius,
      wall_thickness,
      name,
    }) => {
      try {
        const r = await api("POST", "/api/geometry/revolve", {
          // The server refuses profile+profile_segments together; forward
          // only the mode the caller chose.
          ...(profile !== undefined ? { profile } : {}),
          ...(profile_segments !== undefined ? { profile_segments } : {}),
          axis_origin,
          axis_direction,
          angle_deg,
          segments,
          smooth: smooth ?? false,
          bore_radius: bore_radius ?? 0,
          wall_thickness: wall_thickness ?? 0,
          name: name ?? null,
        });
        const id = r.solid_id ?? (await newestPartId());
        return await okp(
          {
            object_uuid: r.object?.id ?? null, // operand for boolean / transform
            part_id: id,
            triangles: r?.stats?.triangle_count ?? null,
            placement: id !== null ? await placement(id) : null,
          },
          id,
        );
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "nurbs_loft",
    "Watertight FREEFORM SOLID: skin one NURBS surface through a stack of " +
      "cross-section rings — for organic shapes revolve/extrude can't make " +
      "(bulged barrels, ogives, lobed transitions). First and last rings become " +
      "planar caps.",
    {
      sections: z
        .array(z.array(z.array(z.number()).length(3)).min(3))
        .min(2)
        .describe(
          "ordered stack of OPEN cross-section rings of [x,y,z] points (mm), " +
            "SAME count each (auto-closed); first/last must be planar (caps)",
        ),
      degree_u: z
        .number()
        .int()
        .min(1)
        .max(7)
        .default(3)
        .describe("NURBS degree around each section"),
      degree_v: z
        .number()
        .int()
        .min(1)
        .max(7)
        .default(3)
        .describe("NURBS degree along the loft (3 = G2 continuity)"),
      name: z.string().optional().describe("display name"),
    },
    async ({ sections, degree_u, degree_v, name }) => {
      try {
        const r = await api("POST", "/api/geometry/nurbs_loft", {
          sections,
          degree_u,
          degree_v,
          name: name ?? null,
        });
        const id = r.solid_id ?? (await newestPartId());
        return await okp(
          {
            object_uuid: r.object?.id ?? null, // operand for boolean / transform
            part_id: id,
            triangles: r?.stats?.triangle_count ?? null,
            placement: id !== null ? await placement(id) : null,
          },
          id,
        );
      } catch (e) {
        return fail(e);
      }
    },
  );
}
