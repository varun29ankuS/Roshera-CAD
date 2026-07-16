/** Parametric sketching (csketch — constraint-solver backed). */

import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import { api, ok, fail, okp, newestPartId } from "../core.js";

export function registerPsketchTools(server: McpServer) {
  server.tool(
    "psketch_create",
    "Create a PARAMETRIC sketch (constraint-solver backed, XY plane). Add " +
      "entities loosely, constrain, solve — the solver places everything to " +
      "machine precision (unlike create_sketch's click-draft).",
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
    "psketch_add",
    "Add an entity. kind=point {x,y,fixed?}, line {start,end point uuids}, " +
      "circle {cx,cy,radius}, arc {cx,cy,radius,start_angle,end_angle}, " +
      "rectangle {x1,y1,x2,y2}, polyline {points[[x,y]...],closed}, " +
      "spline {degree, control_point_ids:[point uuids]} — SHARED control " +
      "points: the spline is a solver citizen (drag/constrain the points, " +
      "zero phantom DOF; clamped, interpolates first/last CP — weld organic " +
      "joins by reusing profile vertices as end CPs; optional weights[] for " +
      "rational NURBS). Raw form {degree, control_points[[x,y]...], knots[]} " +
      "also accepted. Returns the entity id.",
    {
      csketch_id: z.string().uuid(),
      kind: z.enum([
        "point",
        "line",
        "circle",
        "arc",
        "rectangle",
        "polyline",
        "spline",
      ]),
      params: z.record(z.unknown()),
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
    "Add a constraint. geometric: Horizontal/Vertical/Parallel/Perpendicular/" +
      "Coincident/Tangent/Concentric/Equal on entities; CONTINUITY (organic): " +
      "SmoothTangent = G1 (tangent direction) and CurvatureContinuity = G2 " +
      "(tangent + traversal-signed curvature) between any curve pair at their " +
      "join — [{Line: uuid}, {Spline: uuid}] etc.; CurvatureExtremum on " +
      "[{Spline: uuid}, {Point: uuid}] holds the point at stationary " +
      "curvature (the apex). dimensional: {Distance: 80.0} / {Radius: 6.0} / " +
      "{Angle: 1.57}; {Curvature: k} on [curve] or at a point's foot on " +
      "[curve, point]. entities = [{Line: uuid}] or [{Point: uuid}, " +
      "{Point: uuid}] … The certificate reports MEASURED continuity " +
      "deviations per join (psketch_certify → continuity).",
    {
      csketch_id: z.string().uuid(),
      constraint_type: z.record(z.unknown()),
      entities: z.array(z.record(z.string())),
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
    { csketch_id: z.string().uuid() },
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
    "FULL certified-sketch verdict (the kernel can't lie about constraint " +
      "state): solver verdict, per-constraint satisfied/violated with " +
      "residuals, per-entity constrainment (fully/under/over + free DOFs, " +
      "cluster-localised), minimal conflict WITNESSES (QuickXplain — names " +
      "exactly which constraints fight; `minimal:false` = honestly " +
      "un-minimised), DOF summary, decomposition stats.",
    { csketch_id: z.string().uuid() },
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
    "DOF summary + per-entity constrainment status: which entities are " +
      "fully constrained, which still move (and by how many DOFs), which " +
      "are over-constrained and via which constraint ids. Read this before " +
      "extruding — 'solved' is not 'fully defined'.",
    { csketch_id: z.string().uuid() },
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
    "Sketch operation on a parametric sketch — the result is MAINTAINED by " +
      "minted constraints, not a one-shot copy. op=trim {entity,cutter " +
      "(EntityRef like {Line: uuid}), pick:[x,y] on the span to REMOVE} — " +
      "carrier-invariant constraints on the trimmed entity are RE-APPLIED " +
      "onto the survivors (outcome.constraints_reapplied); extent-bound " +
      "ones are dropped (outcome.constraints_removed); " +
      "extend {entity: EntityRef (line or arc — arcs grow their sweep " +
      "along the carrier), end:'start'|'end', boundary: EntityRef}; offset " +
      "{entity: EntityRef on a closed loop, distance (+ enlarges)} — " +
      "all-arc loops are fully maintained (per-junction G1 + concentric " +
      "webs) and a globally self-intersecting result refuses typed " +
      "(details.kind=self_intersecting, names the colliding segments); " +
      "mirror {entities:[EntityRef], axis: uuid of a CONSTRUCTION line} — " +
      "supports legacy center-angle arcs and shared-CP splines; " +
      "linear_pattern {entities, count (total incl. source), dx, dy}; " +
      "circular_pattern {entities, center: uuid | center_position:[x,y], " +
      "count, angle_step (rad)} — pattern sources: points, circles, " +
      "shared-endpoint lines/arcs, shared-CP splines (entity webs, fully " +
      "maintained); curve_pattern {entities (points/circles), rail: " +
      "EntityRef (spline/arc, may be construction), count, spacing? " +
      "(arc-length; omit = fill evenly)} — instances held ON the rail; " +
      "arc rails maintain TRUE arc-length spacing (ArcLength links), " +
      "spline rails maintain placement chords; " +
      "phyllotaxis_pattern {entities, center: uuid | center_position:[x,y], " +
      "count (florets incl. source), spacing (c in r=c*sqrt(n))} — Vogel " +
      "spiral at the EXACT golden angle (137.5078°), the biomimetic seed " +
      "arrangement; construction {entity: EntityRef, " +
      "is_construction: bool}. Returns the typed outcome (created/deleted " +
      "entities, minted constraints, provenance) + a fresh certificate " +
      "digest. Refusals are typed (details.kind).",
    {
      csketch_id: z.string().uuid(),
      op: z.enum([
        "trim",
        "extend",
        "offset",
        "mirror",
        "linear_pattern",
        "circular_pattern",
        "curve_pattern",
        "phyllotaxis_pattern",
        "construction",
      ]),
      params: z.record(z.unknown()),
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
    "psketch_extrude",
    "Extrude the parametric sketch's closed regions into a solid. Hole-aware " +
      "(circles inside an outline become bores). On topology errors the " +
      "response names every gap/dangling endpoint for surgical repair.",
    {
      csketch_id: z.string().uuid(),
      distance: z.number(),
      name: z.string().optional(),
    },
    async ({ csketch_id, distance, name }) => {
      try {
        const r = await api("POST", `/api/csketch/${csketch_id}/extrude`, {
          distance,
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
    "Revolve the parametric sketch's closed regions about an IN-PLANE axis " +
      "(a point + direction in sketch coordinates). Typed-analytic where " +
      "honest: lines revolve to exact cylinder/cone bands and planar " +
      "annuli, arcs/splines to exact revolved surfaces; full-circle loops " +
      "fall back to explicit sampling (stats.sampled_loops counts them — a " +
      "revolved circle's torus lateral has no analytic path yet). Profiles " +
      "must not cross the axis. angle defaults to 2*pi (full revolution).",
    {
      csketch_id: z.string().uuid(),
      axis_origin: z.tuple([z.number(), z.number()]),
      axis_direction: z.tuple([z.number(), z.number()]),
      angle: z.number().optional(),
      segments: z.number().int().min(3).max(512).optional(),
      name: z.string().optional(),
    },
    async ({ csketch_id, axis_origin, axis_direction, angle, segments, name }) => {
      try {
        const r = await api("POST", `/api/csketch/${csketch_id}/revolve`, {
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
