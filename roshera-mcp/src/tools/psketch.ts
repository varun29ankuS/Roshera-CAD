/** Parametric sketching (csketch — constraint-solver backed). */

import type { ToolHost } from "../registry.js";
import { z } from "zod";
import { api, ok, fail, okp, newestPartId } from "../core.js";

/** Wire shape of a sketch plane: a standard-plane string or a custom
 * orthonormal frame (as returned by psketch_plane_from_face). The
 * backend REFUSES non-orthonormal custom frames typed (a skewed frame
 * would silently shear the profile) — do not hand-build one unless the
 * axes are unit-length and perpendicular. */
const planeSchema = z
  .union([
    z.enum(["xy", "xz", "yz"]),
    z.object({
      origin: z.array(z.number()).length(3).describe("plane-local (0,0) in world mm"),
      u_axis: z.array(z.number()).length(3).describe("unit sketch-X in world coords"),
      v_axis: z.array(z.number()).length(3).describe("unit sketch-Y in world coords"),
    }),
  ])
  .describe(
    "sketch plane: 'xy'|'xz'|'yz' or {origin,u_axis,v_axis} custom frame " +
      "(use psketch_plane_from_face for a face-anchored frame); default 'xy'",
  );

export function registerPsketchTools(server: ToolHost) {
  server.tool(
    "psketch_begin",
    "START a new PARAMETRIC sketch session (constraint-solver backed); " +
      "returns csketch_id. The sketch is pure 2D — its world placement is " +
      "chosen at psketch_extrude/psketch_revolve time via their `plane` " +
      "param (standard or custom/face-anchored frame). Opens the session " +
      "only — add geometry with psketch_add_entity, then psketch_constrain/" +
      "psketch_solve. Prefer over create_sketch for machine-precision solving.",
    {},
    async () => {
      try {
        const s = await api("POST", "/api/csketch", {});
        return ok({ csketch_id: s.id });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "psketch_add_entity",
    "ADD ONE entity to an existing parametric sketch (start a session with " +
      "psketch_begin, not this tool). Returns the entity id. kind→params " +
      "(sketch-plane mm/radians): point {x,y,fixed?} · line {start,end " +
      "point-uuids} · circle {cx,cy,radius} · arc {cx,cy,radius,start_angle," +
      "end_angle} · rectangle {x1,y1,x2,y2} · polyline {points:[[x,y]…],closed} " +
      "· spline {degree, control_point_ids:[point-uuids]} — SHARED CPs make the " +
      "spline a solver citizen (clamped, interpolates first/last CP; optional " +
      "weights[]). Raw {degree, control_points:[[x,y]…], knots[]} also accepted.",
    {
      csketch_id: z.string().uuid().describe("csketch id (psketch_begin)"),
      kind: z
        .enum([
          "point",
          "line",
          "circle",
          "arc",
          "rectangle",
          "polyline",
          "spline",
        ])
        .describe("entity type; see description for each type's params"),
      params: z
        .record(z.unknown())
        .describe("entity params for `kind` (see description); sketch-plane mm/radians"),
    },
    async ({ csketch_id, kind, params }) => {
      try {
        const r = await api("POST", `/api/csketch/${csketch_id}/${kind}`, params);
        return ok({ id: r.id });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "psketch_constrain",
    "Add a constraint to a parametric sketch. GEOMETRIC: Horizontal/Vertical/" +
      "Parallel/Perpendicular/Coincident/Collinear/Midpoint/PointOnCurve/" +
      "Tangent/Concentric/Equal/Symmetric (about a line). CONTINUITY: " +
      "SmoothTangent (G1), CurvatureContinuity (G2) between a curve pair; " +
      "CurvatureExtremum holds a spline point at the apex. DIMENSIONAL: " +
      "{Distance:80.0} mm / {Radius:6.0} / {Diameter:12.0} / {Length:40.0} / " +
      "{XCoordinate:x} / {YCoordinate:y} / {Angle:1.57} rad / {Curvature:k}; " +
      "inequalities {MinDistance:d} / {MaxDistance:d}. Unsupported kind/entity " +
      "pairings REFUSE typed (irreducible residual, 0 DOF removed) rather than " +
      "silently no-op — read psketch_certify after constraining.",
    {
      csketch_id: z.string().uuid().describe("csketch id (psketch_begin)"),
      constraint_type: z
        .record(z.unknown())
        .describe(
          "the constraint, e.g. {Horizontal:{}} or {Distance:80.0} (mm) / " +
            "{Angle:1.57} (radians) — see description",
        ),
      entities: z
        .array(z.record(z.string()))
        .describe("target entity refs, e.g. [{Line:uuid}] or [{Point:uuid},{Point:uuid}]"),
    },
    async ({ csketch_id, constraint_type, entities }) => {
      try {
        const r = await api("POST", `/api/csketch/${csketch_id}/constraint`, {
          id: crypto.randomUUID(),
          constraint_type,
          entities,
          priority: "High",
          status: "Satisfied",
          name: null,
        });
        return ok({ constraint_id: r.id });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "psketch_solve",
    "Run the Newton-Raphson solver. Converged = geometry satisfies every " +
      "constraint exactly. Returns status + solved entity positions.",
    { csketch_id: z.string().uuid().describe("csketch id (psketch_begin)") },
    async ({ csketch_id }) => {
      try {
        const report = await api("POST", `/api/csketch/${csketch_id}/solve`, {});
        const summary = await api("GET", `/api/csketch/${csketch_id}`);
        return ok({ status: report.status, points: summary.points });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "psketch_certify",
    "FULL certified-sketch verdict: solver status, per-constraint " +
      "satisfied/violated + residuals, per-entity constrainment (fully/under/" +
      "over + free DOFs), minimal conflict WITNESSES (QuickXplain names which " +
      "constraints fight), DOF summary, decomposition stats.",
    { csketch_id: z.string().uuid().describe("csketch id (psketch_begin)") },
    async ({ csketch_id }) => {
      try {
        const cert = await api("POST", `/api/csketch/${csketch_id}/certify`, {});
        return ok(cert);
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "psketch_dof",
    "DOF summary + per-entity constrainment: which entities are fully " +
      "constrained, which still move (and by how many DOFs), which are over-" +
      "constrained and via which constraint ids. Read before extruding — " +
      "'solved' is not 'fully defined'.",
    { csketch_id: z.string().uuid().describe("csketch id (psketch_begin)") },
    async ({ csketch_id }) => {
      try {
        const cert = await api("POST", `/api/csketch/${csketch_id}/certify`, {});
        return ok({
          constrainedness: cert.constrainedness,
          dof: cert.dof,
          entity_statuses: cert.entity_statuses,
          decomposition: cert.decomposition,
          witnesses: cert.witnesses,
        });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "psketch_op",
    "Sketch operation MAINTAINED by minted constraints. op→params " +
      "(EntityRef={Line:uuid}):\n" +
      "trim {entity, cutter:EntityRef, pick:[x,y] on span to REMOVE};\n" +
      "extend {entity:EntityRef (line/arc), end:'start'|'end', boundary:EntityRef};\n" +
      "offset {entity on a closed loop, distance (+ enlarges)} (self-intersect refuses typed);\n" +
      "mirror {entities:[EntityRef], axis:uuid of a CONSTRUCTION line};\n" +
      "linear_pattern {entities, count (incl. source), dx, dy};\n" +
      "circular_pattern {entities, center:uuid|center_position:[x,y], count, angle_step (rad)};\n" +
      "curve_pattern {entities, rail:EntityRef (spline/arc), count, spacing? (arc-length; omit=even)};\n" +
      "phyllotaxis_pattern {entities, center…, count, spacing (c in r=c·√n)} (golden-angle Vogel spiral);\n" +
      "construction {entity:EntityRef, is_construction:bool}.\n" +
      "Returns the typed outcome + a fresh certificate digest; refusals typed (details.kind).",
    {
      csketch_id: z.string().uuid().describe("csketch id (psketch_begin)"),
      op: z
        .enum([
          "trim",
          "extend",
          "offset",
          "mirror",
          "linear_pattern",
          "circular_pattern",
          "curve_pattern",
          "phyllotaxis_pattern",
          "construction",
        ])
        .describe("operation; see description for each op's params"),
      params: z
        .record(z.unknown())
        .describe("op params (see description); lengths mm, angles radians"),
    },
    async ({ csketch_id, op, params }) => {
      try {
        const route: Record<string, ["POST" | "PATCH", string]> = {
          trim: ["POST", "trim"],
          extend: ["POST", "extend"],
          offset: ["POST", "offset"],
          mirror: ["POST", "mirror"],
          linear_pattern: ["POST", "pattern/linear"],
          circular_pattern: ["POST", "pattern/circular"],
          curve_pattern: ["POST", "pattern/curve"],
          phyllotaxis_pattern: ["POST", "pattern/phyllotaxis"],
          construction: ["PATCH", "construction"],
        };
        const [method, path] = route[op];
        const r = await api(method, `/api/csketch/${csketch_id}/${path}`, params);
        return ok(r);
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "psketch_plane_from_face",
    "Derive a CUSTOM sketch plane from a planar face of an existing part " +
      "(sketch-on-face). Returns {origin,u_axis,v_axis} — pass it as `plane` " +
      "to psketch_extrude/psketch_revolve to place the profile on that face; " +
      "the frame's normal points OUT of the part, so a positive extrude " +
      "distance grows a boss and a negative one cuts inward only via boolean. " +
      "Refuses typed when the face is not planar or not owned by the part.",
    {
      object_uuid: z.string().uuid().describe("part object uuid (e.g. from list_parts)"),
      face_id: z
        .number()
        .int()
        .nonnegative()
        .describe("kernel face id on that part (inspect/perception tools list them)"),
    },
    async ({ object_uuid, face_id }) => {
      try {
        const plane = await api("POST", "/api/sketch/plane-from-face", {
          object_id: object_uuid,
          face_id,
        });
        return ok({ plane });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "psketch_extrude",
    "Extrude the parametric sketch's closed regions into a solid. Hole-aware " +
      "(circles inside an outline become bores). `plane` places the profile in " +
      "the world (default XY; custom frames must be orthonormal — refused typed " +
      "otherwise); `direction` (default plane normal) may be oblique but an " +
      "in-plane direction refuses typed (zero-thickness sweep). On topology " +
      "errors the response names every gap/dangling endpoint for surgical repair.",
    {
      csketch_id: z.string().uuid().describe("csketch id (psketch_begin)"),
      distance: z.number().describe("extrusion length (mm); sign sets direction"),
      plane: planeSchema.optional(),
      direction: z
        .array(z.number())
        .length(3)
        .optional()
        .describe("world-space extrude direction; default = plane normal; oblique allowed"),
      name: z.string().optional().describe("display name"),
    },
    async ({ csketch_id, distance, plane, direction, name }) => {
      try {
        const r = await api("POST", `/api/csketch/${csketch_id}/extrude`, {
          distance,
          plane: plane ?? undefined,
          direction: direction ?? undefined,
          name: name ?? null,
        });
        const part_id = await newestPartId();
        return await okp(
          {
            object_uuid: r.object?.id ?? null, // pass to `boolean` as an operand
            part_id, // kernel id for render/verify/inspect tools
            solid_id: r.solid_id,
            triangles: r.stats?.triangle_count,
            regions: r.stats?.regions,
          },
          part_id,
        );
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "psketch_revolve",
    "Revolve the parametric sketch's closed regions about an IN-PLANE axis. " +
      "Typed-analytic where honest: lines → exact cylinder/cone bands + planar " +
      "annuli, arcs/splines → exact revolved surfaces; full circles fall back to " +
      "sampling (stats.sampled_loops). Profiles must not cross the axis. " +
      "`plane` places the profile in the world (default XY; custom frames must " +
      "be orthonormal — refused typed otherwise).",
    {
      csketch_id: z.string().uuid().describe("csketch id (psketch_begin)"),
      plane: planeSchema.optional(),
      axis_origin: z
        .array(z.number()).length(2)
        .describe("point on the axis, sketch-plane coords [x,y] mm"),
      axis_direction: z
        .array(z.number()).length(2)
        .describe("axis direction in sketch-plane coords [x,y]"),
      angle: z.number().optional().describe("sweep angle in radians (default 2π, full)"),
      segments: z
        .number()
        .int()
        .min(3)
        .max(512)
        .optional()
        .describe("angular tessellation count for sampled loops"),
      name: z.string().optional().describe("display name"),
    },
    async ({ csketch_id, plane, axis_origin, axis_direction, angle, segments, name }) => {
      try {
        const r = await api("POST", `/api/csketch/${csketch_id}/revolve`, {
          plane: plane ?? undefined,
          axis_origin,
          axis_direction,
          angle: angle ?? undefined,
          segments: segments ?? undefined,
          name: name ?? null,
        });
        const part_id = await newestPartId();
        return await okp(
          {
            object_uuid: r.object?.id ?? null,
            part_id,
            solid_id: r.solid_id,
            triangles: r.stats?.triangle_count,
            regions: r.stats?.regions,
            analytic_loops: r.stats?.analytic_loops,
            sampled_loops: r.stats?.sampled_loops,
          },
          part_id,
        );
      } catch (e) {
        return fail(e);
      }
    },
  );
}
