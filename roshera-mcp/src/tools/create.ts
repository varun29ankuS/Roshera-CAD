/** Creation tools — sketches, composite primitives, revolve, loft. */

import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
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

export function registerCreateTools(server: McpServer) {
  server.tool(
    "create_sketch",
    "Start a sketch session on a plane. Returns sketch_id for the shape/" +
      "point/extrude tools. Prefer the composite tools (create_box / " +
      "create_cylinder) when they fit — fewer round trips.",
    {
      plane: PlaneSchema,
      tool: z.enum(["rectangle", "circle", "polyline"]),
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
    "Add another shape to an existing sketch (e.g. hole circles inside an " +
      "outer boundary — region detection assigns outer/hole roles at extrude " +
      "time). Returns the new shape_index.",
    {
      sketch_id: z.string(),
      tool: z.enum(["rectangle", "circle", "polyline"]),
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
    "BATCH-add points to a sketch shape in one call (rectangle: 2 corners; " +
      "circle: center then a radius point; polyline: every vertex of the " +
      "closed polygon). shape_index omitted = the first shape.",
    {
      sketch_id: z.string(),
      points: z.array(z.tuple([z.number(), z.number()])).min(1),
      shape_index: z.number().int().optional(),
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
    "Extrude the sketch into a solid. Multi-shape sketches get region " +
      "detection (outer boundary + holes). Returns the new part id + placement.",
    {
      sketch_id: z.string(),
      distance: z.number(),
      name: z.string().optional(),
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
    "Derive a sketch plane FROM an existing planar face ('sketch on this " +
      "face'). object_id = the part's public UUID; face_id from get_pointer or " +
      "render legend. Returns {origin, u_axis, v_axis}.",
    { object_id: z.string().uuid(), face_id: z.number().int() },
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
        "ONE-CALL analytic box: width × depth extruded by height. Base centre " +
        "is `center` [x,y,z] when given (any world point), else (cx, cy) on the " +
        "named `plane`. Orientation follows the plane; height is along the " +
        "plane normal. Returns part id + placement.",
      inputSchema: z
        .object({
          plane: PlaneSchema.default("xy"),
          cx: z.number().default(0),
          cy: z.number().default(0),
          center: z
            .tuple([z.number(), z.number(), z.number()])
            .optional()
            .describe(
              "explicit world base-centre [x,y,z]; overrides plane+cx+cy",
            ),
          width: z.number().positive(),
          depth: z.number().positive(),
          height: z.number(),
          name: z.string().optional(),
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
        "ONE-CALL analytic cylinder with one smooth lateral face. Base centre " +
        "is `center` [x,y,z] when given (any world point), else (cx, cy) on the " +
        "named `plane`. `axis` sets the extrusion direction (default = plane " +
        "normal). Returns part id + placement.",
      inputSchema: z
        .object({
          plane: PlaneSchema.default("xy"),
          cx: z.number().default(0),
          cy: z.number().default(0),
          center: z
            .tuple([z.number(), z.number(), z.number()])
            .optional()
            .describe(
              "explicit world base-centre [x,y,z]; overrides plane+cx+cy",
            ),
          axis: z
            .tuple([z.number(), z.number(), z.number()])
            .optional()
            .describe("extrusion axis [x,y,z]; defaults to the plane normal"),
          radius: z.number().positive(),
          height: z.number(),
          name: z.string().optional(),
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
    "ONE-CALL analytic cone or frustum. base_radius at `center`; top_radius " +
      "(default 0 = apex) at center+axis*height. True smooth cone surface. " +
      "Returns part id + placement.",
    {
      center: z.tuple([z.number(), z.number(), z.number()]).default([0, 0, 0]),
      axis: z.tuple([z.number(), z.number(), z.number()]).default([0, 0, 1]),
      base_radius: z.number().nonnegative(),
      top_radius: z.number().nonnegative().default(0),
      height: z.number().positive(),
      name: z.string().optional(),
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
        "ONE-CALL analytic sphere of `radius`, centred at `center` [x,y,z] " +
        "(default origin). Returns part id + placement.",
      inputSchema: z
        .object({
          radius: z.number().positive(),
          center: z
            .tuple([z.number(), z.number(), z.number()])
            .optional()
            .describe("world centre [x,y,z]; defaults to the origin"),
          name: z.string().optional(),
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
    "SOLID OF REVOLUTION from a closed [r,z] meridian profile — the primitive " +
      "for any axisymmetric part (nozzles, vessels, a rocket engine in one " +
      "op). Revolved about the axis (default +Z, 360°); one op, watertight. " +
      "Profile must be a simple loop, all r ≥ 0, not crossing the axis. Hollow " +
      "part = trace the wall cross-section.",
    {
      profile: z
        .array(z.tuple([z.number(), z.number()]))
        .min(3)
        .describe("closed [r,z] meridian profile (auto-closes last→first)"),
      axis_origin: z.tuple([z.number(), z.number(), z.number()]).default([0, 0, 0]),
      axis_direction: z.tuple([z.number(), z.number(), z.number()]).default([0, 0, 1]),
      angle_deg: z.number().default(360),
      segments: z.number().int().min(3).max(512).default(96),
      smooth: z
        .boolean()
        .optional()
        .describe(
          "fit a SMOOTH NURBS curve through `profile` (the outer wall) so the " +
            "revolved wall is ONE surface — needs bore_radius",
        ),
      bore_radius: z
        .number()
        .optional()
        .describe("hollow bore radius for a smooth-walled tube (with smooth=true)"),
      wall_thickness: z
        .number()
        .optional()
        .describe(
          "CONTOURED nozzle/vessel (e.g. a Rao bell): `profile` is the INNER flow " +
            "contour, outer wall offset by this thickness — both walls ONE smooth " +
            "SurfaceOfRevolution",
        ),
      name: z.string().optional(),
    },
    async ({
      profile,
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
          profile,
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
      "(bulged barrels, ogives, lobed transitions). `sections` = ordered open " +
      "rings of [x,y,z] points, SAME count each (auto-closed); first and last " +
      "must be planar (they become caps). degree_v=3 gives G2 along the loft.",
    {
      sections: z
        .array(z.array(z.tuple([z.number(), z.number(), z.number()])).min(3))
        .min(2)
        .describe("stack of cross-section rings (each open, equal point count)"),
      degree_u: z.number().int().min(1).max(7).default(3).describe("degree around the section"),
      degree_v: z.number().int().min(1).max(7).default(3).describe("degree along the loft (3 = G2)"),
      name: z.string().optional(),
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
