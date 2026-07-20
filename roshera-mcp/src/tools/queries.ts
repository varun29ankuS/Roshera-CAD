/** Spatial-query tools — exact-analytic point / ray / region probes. */

import type { ToolHost } from "../registry.js";
import { z } from "zod";
import { api, ok, fail } from "../core.js";

export function registerQueryTools(server: ToolHost) {
  server.tool(
    "point_query",
    "PROBE a world point vs a part: SIGNED DISTANCE (− inside, + outside), " +
      "inside/outside/on, nearest face + exact closest point. Exact-analytic — a " +
      "point in a through-hole reads OUTSIDE.",
    {
      part_id: z.number().int().describe("part id (list_parts)"),
      point: z
        .array(z.number()).length(3)
        .describe("world-space query point [x, y, z]"),
    },
    async ({ part_id, point }) => {
      try {
        return ok(
          await api("POST", `/api/agent/parts/${part_id}/point-query`, { point }),
        );
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "ray_query",
    "CAST a ray through a part: ORDERED face crossings (id, exact hit point, " +
      "oriented normal, distance), near→far. Exact analytic, clipped to real " +
      "trim loops. Two crossings of a wall = its thickness; empty = missed.",
    {
      part_id: z.number().int().describe("part id (list_parts)"),
      origin: z
        .array(z.number()).length(3)
        .describe("ray start point [x,y,z] mm (world)"),
      direction: z
        .array(z.number()).length(3)
        .describe("ray direction [x,y,z]; need not be unit length"),
    },
    async ({ part_id, origin, direction }) => {
      try {
        return ok(
          await api("POST", `/api/agent/parts/${part_id}/ray-query`, {
            origin,
            direction,
          }),
        );
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "region_query",
    "ASK 'what is in here?': a BOX (center+half_extents) or SPHERE " +
      "(center+radius) returns which parts/faces meet it and whether EMPTY. Omit " +
      "part_id to scan the whole scene; box wins if both given.",
    {
      center: z
        .array(z.number()).length(3)
        .describe("region centre [x,y,z] mm (world)"),
      half_extents: z
        .array(z.number()).length(3)
        .optional()
        .describe("box half-extents — supply for a BOX region"),
      radius: z
        .number()
        .positive()
        .optional()
        .describe("sphere radius — supply for a SPHERE region"),
      part_id: z
        .number()
        .int()
        .optional()
        .describe("restrict to one part; omit to scan every part"),
    },
    async ({ center, half_extents, radius, part_id }) => {
      try {
        if (!half_extents && radius === undefined) {
          return fail(new Error("provide half_extents (box) or radius (sphere)"));
        }
        return ok(
          await api("POST", "/api/agent/region-query", {
            center,
            ...(half_extents ? { half_extents } : {}),
            ...(radius !== undefined ? { radius } : {}),
            ...(part_id !== undefined ? { part_id } : {}),
          }),
        );
      } catch (e) {
        return fail(e);
      }
    },
  );
}
