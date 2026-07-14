/** Modification tools — shell, blends, booleans (single + batch), transform, delete. */

import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import {
  api,
  ok,
  fail,
  okp,
  perceive,
  compactVerdict,
  perceptionField,
  placement,
  newestPartId,
  uuidForPart,
  allEdgeIds,
  PlaneSchema,
  resolvePlane,
  cross3,
  unit3,
} from "../core.js";

export function registerModifyTools(server: McpServer) {
  server.tool(
    "delete_part",
    "Delete one part (timeline-recorded, undo-safe). WARNING: kernel part ids " +
      "RENUMBER after deletion — re-run list_parts before further deletes.",
    { part_id: z.number().int() },
    async ({ part_id }) => {
      try {
        return ok(await api("DELETE", `/api/agent/parts/${part_id}`));
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "clear_parts",
    "Delete EVERY part (each deletion timeline-recorded, undo-safe). Safe to " +
      "build immediately after.",
    {},
    async () => {
      try {
        return ok(await api("DELETE", "/api/agent/parts"));
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "shell",
    "HOLLOW a solid to a constant wall thickness (wall grows inward), opening " +
      "the listed cap faces. `object` is the OBJECT UUID (not a kernel part " +
      "id). `faces_to_remove` from select_face or a render 'ids' legend; [] = " +
      "fully closed void (rarely the intent). Identity-preserving. Shelling " +
      "can leave a self-intersecting or open wall — ALWAYS verify_part.",
    {
      object: z.string().uuid().describe("object_uuid of the solid to hollow"),
      thickness: z.number().describe("wall thickness (inward); must be non-zero"),
      faces_to_remove: z
        .array(z.number().int().nonnegative())
        .default([])
        .describe("face ids of the caps to open; [] = closed void"),
    },
    async ({ object, thickness, faces_to_remove }) => {
      try {
        const r = await api("POST", "/api/geometry/shell", {
          object,
          thickness,
          faces_to_remove,
        });
        const part_id = r.solid_id ?? (await newestPartId());
        return await okp(
          {
            object_uuid: r.object?.id ?? object, // identity-preserving: same uuid
            part_id,
            triangles: r?.stats?.triangle_count ?? null,
            placement: part_id !== null ? await placement(part_id) : null,
          },
          part_id,
        );
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "fillet_edges",
    "ROUND (fillet) edges with a constant radius. `edge_ids` from select_edge " +
      "or a render 'ids' legend; OMIT to blend ALL edges (over-split hole rims " +
      "auto-heal; seams / over-radius edges are skipped — it rounds everything " +
      "it can rather than refusing the whole op). Identity-preserving. Check " +
      "the returned verdict before trusting it.",
    {
      part_id: z.number().int().describe("kernel part id from list_parts"),
      radius: z.number().positive().describe("fillet radius in model units"),
      edge_ids: z
        .array(z.number().int().nonnegative())
        .optional()
        .describe("edges to round; omit for ALL edges"),
    },
    async ({ part_id, radius, edge_ids }) => {
      try {
        const object = await uuidForPart(part_id);
        const allEdges = !(edge_ids && edge_ids.length > 0);
        const edges =
          edge_ids && edge_ids.length > 0 ? edge_ids : await allEdgeIds(part_id);
        // `all_edges` opts the kernel into "round what it can": edges incident to
        // a corner whose patch synthesis is unimplemented are SKIPPED (not a
        // whole-op refusal). Explicit `edge_ids` keep the honest-refuse contract.
        const r = await api("POST", "/api/geometry/fillet", {
          object,
          edges,
          radius,
          all_edges: allEdges,
        });
        const id = r.solid_id ?? part_id;
        return await okp(
          {
            object_uuid: r.object?.id ?? object, // identity-preserving: same uuid
            part_id: id,
            filleted_edges: edges,
            all_edges: allEdges,
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
    "chamfer_edges",
    "BEVEL (chamfer) edges with an equal-distance flat set back `distance` on " +
      "each adjacent face. `edge_ids` from select_edge or a render 'ids' " +
      "legend; OMIT to chamfer ALL edges. Identity-preserving. Chamfers can " +
      "self-intersect at tight corners — check the returned verdict.",
    {
      part_id: z.number().int().describe("kernel part id from list_parts"),
      distance: z.number().positive().describe("chamfer setback in model units"),
      edge_ids: z
        .array(z.number().int().nonnegative())
        .optional()
        .describe("edges to bevel; omit for ALL edges"),
    },
    async ({ part_id, distance, edge_ids }) => {
      try {
        const object = await uuidForPart(part_id);
        const edges =
          edge_ids && edge_ids.length > 0 ? edge_ids : await allEdgeIds(part_id);
        const r = await api("POST", "/api/geometry/chamfer", {
          object,
          edges,
          distance,
        });
        const id = r.solid_id ?? part_id;
        return await okp(
          {
            object_uuid: r.object?.id ?? object, // identity-preserving: same uuid
            part_id: id,
            chamfered_edges: edges,
            all_edges: !(edge_ids && edge_ids.length > 0),
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
    "boolean",
    "Combine two solids: union / difference (cut b out of a) / intersection. " +
      "Operands are OBJECT UUIDs, not kernel part ids. Both operands are " +
      "consumed; a new solid is born. ALWAYS verify_part after differences " +
      "(bores/slots can leave open faces).",
    {
      op: z.enum(["union", "difference", "intersection"]),
      object_a: z.string().uuid().describe("object_uuid of the base solid"),
      object_b: z.string().uuid().describe("object_uuid of the tool solid"),
    },
    async ({ op, object_a, object_b }) => {
      try {
        const r = await api("POST", "/api/geometry/boolean", {
          operation: op,
          object_a,
          object_b,
        });
        const part_id = await newestPartId();
        return await okp(
          {
            object_uuid: r.object?.id ?? null,
            part_id,
            consumed: r.consumed ?? [object_a, object_b],
            placement: part_id !== null ? await placement(part_id) : null,
          },
          part_id,
        );
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "boolean_many",
    "BATCH boolean: ONE op (difference or union) of MANY tool solids against " +
      "a base, sequentially, in a single call. Every step is certified; the " +
      "batch HALTS at the first unsound step and names the tool that did it. " +
      "The base keeps its uuid; consumed tools are gone.",
    {
      op: z.enum(["union", "difference"]),
      base: z.string().uuid().describe("object_uuid of the base solid"),
      tools: z
        .array(z.string().uuid())
        .min(1)
        .max(64)
        .describe("object_uuids applied in order; all consumed"),
    },
    async ({ op, base, tools }) => {
      try {
        let lastId: number | null = null;
        for (let i = 0; i < tools.length; i++) {
          // fast:true skips the endpoint's own full cert — the perceive() below
          // is the single certification gate per step (was 2× cert work/step).
          await api("POST", "/api/geometry/boolean", {
            operation: op,
            object_a: base,
            object_b: tools[i],
            fast: true,
          });
          lastId = await newestPartId();
          const p = await perceive(lastId);
          if (p && p.sound !== true) {
            return ok({
              object_uuid: base,
              part_id: lastId,
              completed: i + 1,
              of: tools.length,
              halted: `step ${i + 1} (${tools[i]}) left the base UNSOUND — ${compactVerdict(p)}`,
            });
          }
        }
        const p = await perceive(lastId);
        return ok({
          object_uuid: base,
          part_id: lastId,
          completed: tools.length,
          of: tools.length,
          placement: lastId !== null ? await placement(lastId) : null,
          // #37: never a bare null — perceptionField() names WHY when `p` is
          // falsy (timeout / network error / no part id) instead of silently
          // omitting the reason.
          perception: perceptionField(p),
        });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "drill_pattern",
    "ONE-CALL hole pattern: drill `count` bores of radius `hole_r` on a ring " +
      "of radius `ring_r` (centered at cx,cy on `plane`) through a target — " +
      "creates the bore cylinders AND subtracts them in one call. Bores run " +
      "along the plane normal from `z_offset` for `depth`: size them to " +
      "OVERSHOOT both faces. REFUSES overlapping adjacent holes up front " +
      "(2·ring_r·sin(π/count) must exceed 2·hole_r — that regime drives a " +
      "known open boolean bug). Certified per hole; halts on the first " +
      "unsound step.",
    {
      object: z.string().uuid().describe("object_uuid of the solid to drill"),
      plane: PlaneSchema.default("xy"),
      cx: z.number().default(0).describe("pattern center (plane coords)"),
      cy: z.number().default(0),
      count: z.number().int().min(1).max(64),
      ring_r: z.number().positive().describe("ring radius the hole centers sit on"),
      hole_r: z.number().positive(),
      depth: z.number().positive().describe("bore length (overshoot the part!)"),
      z_offset: z
        .number()
        .default(-1)
        .describe("bore start along the normal (default −1 = 1mm under the plane)"),
      start_angle_deg: z.number().default(0),
    },
    async ({
      object,
      plane,
      cx,
      cy,
      count,
      ring_r,
      hole_r,
      depth,
      z_offset,
      start_angle_deg,
    }) => {
      try {
        // The same guard the kernel's exploration recipe uses: adjacent holes
        // whose chord spacing is below 2·hole_r intersect each other, and the
        // resulting cyl∩cyl saddle boolean is a known open kernel bug — refuse
        // loudly instead of hanging.
        if (count >= 2) {
          const spacing = 2 * ring_r * Math.sin(Math.PI / count);
          if (spacing <= 2 * hole_r) {
            return fail(
              new Error(
                `REFUSED: ${count} holes of r=${hole_r} on a ring of r=${ring_r} ` +
                  `are spaced ${spacing.toFixed(3)} < 2·r=${(2 * hole_r).toFixed(3)} ` +
                  `(adjacent holes intersect). Reduce count/hole_r or grow ring_r.`,
              ),
            );
          }
        }
        const p = resolvePlane(plane);
        const n = unit3(cross3(p.u, p.v));
        const bores: string[] = [];
        for (let k = 0; k < count; k++) {
          const th = ((start_angle_deg + (360 * k) / count) * Math.PI) / 180;
          const hx = cx + ring_r * Math.cos(th);
          const hy = cy + ring_r * Math.sin(th);
          const center = [0, 1, 2].map(
            (i) => p.o[i] + hx * p.u[i] + hy * p.v[i] + z_offset * n[i],
          );
          // fast:true — bore blanks are analytic primitives; the difference
          // step's perceive() certifies the merged result that actually matters.
          const r = await api("POST", "/api/geometry/cylinder", {
            center,
            axis: n,
            radius: hole_r,
            height: depth,
            name: `bore ${k + 1}/${count}`,
            fast: true,
          });
          const uuid = r.object?.id;
          if (!uuid) throw new Error(`bore ${k + 1}/${count}: no uuid returned`);
          bores.push(uuid);
        }
        let lastId: number | null = null;
        for (let k = 0; k < bores.length; k++) {
          // fast:true — perceive() below is the single per-hole cert gate.
          await api("POST", "/api/geometry/boolean", {
            operation: "difference",
            object_a: object,
            object_b: bores[k],
            fast: true,
          });
          lastId = await newestPartId();
          const pv = await perceive(lastId);
          if (pv && pv.sound !== true) {
            return ok({
              object_uuid: object,
              part_id: lastId,
              holes_completed: k + 1,
              of: count,
              halted: `hole ${k + 1} left the part UNSOUND — ${compactVerdict(pv)}`,
            });
          }
        }
        const pv = await perceive(lastId);
        return ok({
          object_uuid: object,
          part_id: lastId,
          holes: count,
          placement: lastId !== null ? await placement(lastId) : null,
          // #37: never a bare null — perceptionField() names WHY when `pv` is
          // falsy (this is exactly the bug that was hit here: a sequential
          // per-hole perceive() occasionally missed the short ambient timeout
          // window and returned undefined, which used to render as a silent
          // `"perception": null`).
          perception: perceptionField(pv),
        });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "transform",
    "Move and/or rotate a solid IN PLACE by its object_uuid (identity " +
      "preserved — same uuid). Rotation (about optional center, default " +
      "origin) applies first, then translation. Angle in DEGREES.",
    {
      object: z.string().uuid().describe("object_uuid of the solid to move"),
      translation: z
        .tuple([z.number(), z.number(), z.number()])
        .optional()
        .describe("[dx, dy, dz] world-space offset"),
      rotation: z
        .object({
          axis: z.tuple([z.number(), z.number(), z.number()]),
          angle_deg: z.number(),
          center: z
            .tuple([z.number(), z.number(), z.number()])
            .optional()
            .describe("pivot point, default origin"),
        })
        .optional(),
    },
    async ({ object, translation, rotation }) => {
      try {
        if (!translation && !rotation) {
          return fail(new Error("provide translation and/or rotation"));
        }
        const body: any = { object };
        if (translation) body.translation = translation;
        if (rotation) {
          body.rotation = {
            axis: rotation.axis,
            angle: (rotation.angle_deg * Math.PI) / 180,
            ...(rotation.center ? { center: rotation.center } : {}),
          };
        }
        const r = await api("POST", "/api/geometry/transform", body);
        return ok({
          object_uuid: r.object ?? object,
          moved: true,
          note: "render_part to confirm the new position",
        });
      } catch (e) {
        return fail(e);
      }
    },
  );
}
