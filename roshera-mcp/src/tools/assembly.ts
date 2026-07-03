/**
 * ASSEMBLY — TRUE assemblies: positioned part INSTANCES referencing shared
 * geometry (not a boolean merge), plus the kinematic assembly certificate.
 */

import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import { api, ok, fail } from "../core.js";

/** Build a row-major 4×4 transform from a translation (+ optional axis-angle
 *  rotation in degrees about a unit axis). Both optional → identity. */
function buildTransform(
  position?: [number, number, number],
  rotation_deg?: number,
  rotation_axis?: [number, number, number],
): number[][] {
  const I = [
    [1, 0, 0, 0],
    [0, 1, 0, 0],
    [0, 0, 1, 0],
    [0, 0, 0, 1],
  ];
  let m = I.map((r) => r.slice());
  if (rotation_deg && rotation_axis) {
    const [ax, ay, az] = rotation_axis;
    const len = Math.hypot(ax, ay, az) || 1;
    const [x, y, z] = [ax / len, ay / len, az / len];
    const a = (rotation_deg * Math.PI) / 180;
    const c = Math.cos(a);
    const s = Math.sin(a);
    const t = 1 - c;
    // Rodrigues rotation matrix.
    m = [
      [t * x * x + c, t * x * y - s * z, t * x * z + s * y, 0],
      [t * x * y + s * z, t * y * y + c, t * y * z - s * x, 0],
      [t * x * z - s * y, t * y * z + s * x, t * z * z + c, 0],
      [0, 0, 0, 1],
    ];
  }
  if (position) {
    m[0][3] = position[0];
    m[1][3] = position[1];
    m[2][3] = position[2];
  }
  return m;
}

export function registerAssemblyTools(server: McpServer) {
  server.tool(
    "assembly_verify",
    "CERTIFY A KINEMATIC ASSEMBLY — the non-fakeable 'does this physically go " +
      "together AND move without collision?' verdict. Declare mates and " +
      "mechanisms; the kernel solves constraints and returns a 5-dimension " +
      "certificate: mates_consistent, fully_grounded (a part with no mate " +
      "path to ground is FLOATING, named in floating_instances), dof+mobility, " +
      "no_static_interference, swept_clearance_ok (Parry CCD over each " +
      "mechanism's full range, conservative by `epsilon`). is_sound = AND of " +
      "all five — catches a floating massing a shaded render cannot.\n" +
      "MATE: {\"kind\":\"Concentric\"|\"Coincident\"|\"Fixed\", \"a\":<iid>, " +
      "\"feature_a\":{\"Axis\":{\"origin\":[..],\"direction\":[..]}} or " +
      "{\"Face\":{\"point\":[..],\"normal\":[..]}}, \"b\":<iid>, \"feature_b\":{...}}. " +
      "Concentric = two Axis; Coincident = two Face (ANTIPARALLEL normals).\n" +
      "MECHANISM: {\"moving\":<iid>, \"joint\":{\"Revolute\":{\"axis_origin\":[..]," +
      "\"axis_dir\":[..]}} | {\"Prismatic\":{...}} | {\"Spherical\":{\"center\":[..]}} | " +
      "\"Fixed\", \"base_translation\":[..], \"base_rotation\":[x,y,z,w], " +
      "\"range\":[lo,hi], \"samples\":<n>}.",
    {
      ground: z
        .number()
        .int()
        .describe("instance_id of the grounded (fixed reference) part"),
      parts: z
        .array(
          z.object({
            object: z.string().describe("the part's object_uuid"),
            instance_id: z
              .number()
              .int()
              .describe("this occurrence's id, referenced by mates/mechanisms"),
            translation: z
              .array(z.number())
              .length(3)
              .optional()
              .describe("world position [x,y,z] (default origin)"),
            rotation: z
              .array(z.number())
              .length(4)
              .optional()
              .describe("unit quaternion [x,y,z,w] (default identity)"),
          }),
        )
        .describe("every part in the assembly, as an instance"),
      mates: z
        .array(z.any())
        .optional()
        .describe("mate constraints — see MATE format in the description"),
      mechanisms: z
        .array(z.any())
        .optional()
        .describe("mechanisms for swept-clearance — see MECHANISM format"),
      epsilon: z
        .number()
        .optional()
        .default(0.0)
        .describe("tessellation deviation bound; certified clearance = parry_distance − epsilon"),
    },
    async ({ ground, parts, mates, mechanisms, epsilon }) => {
      try {
        const body = {
          ground,
          parts,
          mates: mates ?? [],
          mechanisms: mechanisms ?? [],
          epsilon: epsilon ?? 0.0,
        };
        return ok(await api("POST", "/api/assembly/verify", body));
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "assembly_create",
    "Create a TRUE assembly: a named scene of positioned part INSTANCES (not " +
      "a boolean merge). Instances REFERENCE parts by id and reuse geometry — " +
      "the same part can be placed many times. Returns the assembly id.",
    { name: z.string().min(1).describe("display name, e.g. 'gearbox'") },
    async ({ name }) => {
      try {
        const r = await api("POST", "/api/assembly", { name });
        return ok({ assembly_id: r.id, name });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "assembly_add_instance",
    "Place an INSTANCE of an existing part into an assembly at a world pose " +
      "(same part_id twice = two instances, no copy). Pose: `position` + " +
      "optional `rotation_deg` about `rotation_axis`, OR a raw row-major 4×4 " +
      "`transform`. Returns the instance id + assembly perception.",
    {
      assembly_id: z.string().describe("assembly id from assembly_create"),
      part_id: z.number().int().describe("kernel part id from list_parts"),
      position: z
        .array(z.number())
        .length(3)
        .optional()
        .describe("world translation [x,y,z] mm"),
      rotation_deg: z.number().optional(),
      rotation_axis: z
        .array(z.number())
        .length(3)
        .optional()
        .describe("unit axis for rotation_deg, e.g. [0,0,1]"),
      transform: z
        .array(z.array(z.number()).length(4))
        .length(4)
        .optional()
        .describe("raw row-major 4×4 (overrides position/rotation)"),
      name: z.string().optional().describe("placement name, e.g. 'wheel-FL'"),
      color: z
        .array(z.number().int().min(0).max(255))
        .length(3)
        .optional()
        .describe("per-instance RGB"),
    },
    async ({
      assembly_id,
      part_id,
      position,
      rotation_deg,
      rotation_axis,
      transform,
      name,
      color,
    }) => {
      try {
        const t =
          transform ??
          buildTransform(
            position as [number, number, number] | undefined,
            rotation_deg,
            rotation_axis as [number, number, number] | undefined,
          );
        const r = await api(
          "POST",
          `/api/assembly/${assembly_id}/instance`,
          { part_id, transform: t, name, color },
        );
        return ok(r);
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "assembly_list_instances",
    "List an assembly's instances with PERCEPTION: instance_count vs " +
      "unique_part_count (the gap is the reuse), each instance's part/" +
      "transform/soundness, combined bbox, all_sound.",
    { assembly_id: z.string() },
    async ({ assembly_id }) => {
      try {
        return ok(await api("GET", `/api/assembly/${assembly_id}`));
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "assembly_transform_instance",
    "Re-pose ONE instance without touching the others or the referenced part. " +
      "`position`/`rotation_deg`/`rotation_axis` or a raw `transform`. Returns " +
      "the updated assembly perception.",
    {
      assembly_id: z.string(),
      instance_id: z.string().describe("instance id from assembly_list_instances"),
      position: z.array(z.number()).length(3).optional(),
      rotation_deg: z.number().optional(),
      rotation_axis: z.array(z.number()).length(3).optional(),
      transform: z.array(z.array(z.number()).length(4)).length(4).optional(),
    },
    async ({
      assembly_id,
      instance_id,
      position,
      rotation_deg,
      rotation_axis,
      transform,
    }) => {
      try {
        const t =
          transform ??
          buildTransform(
            position as [number, number, number] | undefined,
            rotation_deg,
            rotation_axis as [number, number, number] | undefined,
          );
        return ok(
          await api(
            "PATCH",
            `/api/assembly/${assembly_id}/instance/${instance_id}`,
            { transform: t },
          ),
        );
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "assembly_view",
    "SEE A WHOLE ASSEMBLY: composite every instance into one image at its " +
      "instance transform, from an orbit camera (az/el, world-Z up). mode " +
      "'diagnostic' highlights open (red) / non-manifold (magenta) edges.",
    {
      assembly_id: z.string(),
      az: z.number().default(35).describe("azimuth degrees around world Z"),
      el: z.number().default(20).describe("elevation degrees above horizon"),
      mode: z
        .enum(["shaded", "ids", "depth", "normals", "diagnostic"])
        .default("shaded"),
      size: z.number().int().min(64).max(2048).default(720),
      quality: z.enum(["coarse", "medium", "fine"]).default("medium"),
    },
    async ({ assembly_id, az, el, mode, size, quality }) => {
      try {
        const r = await api(
          "GET",
          `/api/assembly/${assembly_id}/view?az=${az}&el=${el}&mode=${mode}&size=${size}&quality=${quality}`,
        );
        return {
          content: [
            { type: "image" as const, data: r.png_base64, mimeType: "image/png" },
            {
              type: "text" as const,
              text:
                `assembly az=${az}° el=${el}° instances=${r.instance_count} ` +
                `distinct_parts=${r.unique_part_count} open=${r.open_edges} nm=${r.nonmanifold_edges}`,
            },
          ],
        };
      } catch (e) {
        return fail(e);
      }
    },
  );
}
