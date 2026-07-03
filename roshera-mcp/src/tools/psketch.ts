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
      "rectangle {x1,y1,x2,y2}, polyline {points[[x,y]...],closed}. Returns " +
      "the entity id.",
    {
      csketch_id: z.string().uuid(),
      kind: z.enum(["point", "line", "circle", "arc", "rectangle", "polyline"]),
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
      "Coincident/Tangent/Concentric/Equal on entities. dimensional: " +
      "{Distance: 80.0} / {Radius: 6.0} / {Angle: 1.57}. entities = " +
      "[{Line: uuid}] or [{Point: uuid}, {Point: uuid}] …",
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
}
