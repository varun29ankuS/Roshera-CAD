/** Spatial-query tools — exact-analytic point / ray / region probes. */

import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import { api, ok, fail } from "../core.js";

export function registerQueryTools(server: McpServer) {
  server.tool(
    "point_query",
    "PROBE a world point against a part: SIGNED DISTANCE (negative inside, " +
      "positive outside), inside/outside/on classification, nearest boundary " +
      "face + exact closest point. Exact-analytic, never a mesh lookup — a " +
      "point in a through-hole reads OUTSIDE.",
    {
      part_id: z.number().int().describe("kernel part id from list_parts"),
      point: z
        .tuple([z.number(), z.number(), z.number()])
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
    "CAST a ray through a part: ORDERED face crossings (face id, exact hit " +
      "point, oriented normal, distance), near→far. Exact analytic surface " +
      "intersections clipped to real trim loops — never a mesh approximation. " +
      "Two crossings of a wall = its thickness; empty hits = missed.",
    {
      part_id: z.number().int().describe("kernel part id from list_parts"),
      origin: z.tuple([z.number(), z.number(), z.number()]),
      direction: z
        .tuple([z.number(), z.number(), z.number()])
        .describe("need not be unit length"),
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
    "ASK 'what is in here?': a BOX (center + half_extents) or SPHERE (center + " +
      "radius) region returns which parts/faces meet it and whether it is " +
      "EMPTY. Omit part_id to scan the whole scene (clearance-envelope check). " +
      "Face extents are exact trim-curve envelopes; box wins if both given.",
    {
      center: z.tuple([z.number(), z.number(), z.number()]),
      half_extents: z
        .tuple([z.number(), z.number(), z.number()])
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
