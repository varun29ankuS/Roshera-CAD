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
    "One-shot kinematic assembly certificate from a self-contained spec (no " +
      "prior assembly_create). Declare parts + mates + mechanisms inline; " +
      "returns a 5-dim verdict: mates_consistent, fully_grounded (no floating " +
      "part), dof+mobility, no_static_interference, swept_clearance_ok (Parry " +
      "CCD, conservative by `epsilon`). is_sound = AND of all five.\n" +
      "MATE: {kind:'Concentric'|'Coincident'|'Fixed', a:<iid>, feature_a:{Axis:" +
      "{origin,direction}}|{Face:{point,normal}}, b, feature_b} (Concentric=2 " +
      "Axis; Coincident=2 Face, antiparallel).\n" +
      "MECHANISM: {moving:<iid>, joint:{Revolute:{axis_origin,axis_dir}}|" +
      "{Prismatic:{…}}|{Spherical:{center}}|'Fixed', base_translation, " +
      "base_rotation:[x,y,z,w], range:[lo,hi], samples}.\n" +
      "Persistent workflow: assembly_create + assembly_mate + assembly_certify.",
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
              .describe("world position [x,y,z] mm (default origin)"),
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
        .describe("tessellation deviation bound (mm); certified clearance = parry_distance − epsilon"),
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
    "Create a TRUE assembly: a named scene of positioned part INSTANCES (not a " +
      "boolean merge). Instances reference parts by id and reuse geometry — the " +
      "same part can be placed many times. Returns the assembly id.",
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
      "(same object twice = two instances, no copy). Returns the instance id + " +
      "assembly perception.",
    {
      assembly_id: z.string().describe("assembly id"),
      object: z
        .string()
        .describe("the part's object_uuid (from a create_* or boolean result)"),
      position: z
        .array(z.number())
        .length(3)
        .optional()
        .describe("world translation [x,y,z] mm"),
      rotation_deg: z.number().optional().describe("rotation angle about rotation_axis (degrees)"),
      rotation_axis: z
        .array(z.number())
        .length(3)
        .optional()
        .describe("unit rotation axis, e.g. [0,0,1]"),
      transform: z
        .array(z.array(z.number()).length(4))
        .length(4)
        .optional()
        .describe("raw row-major 4×4 pose (overrides position/rotation)"),
      name: z.string().optional().describe("placement name, e.g. 'wheel-FL'"),
      color: z
        .array(z.number().int().min(0).max(255))
        .length(3)
        .optional()
        .describe("per-instance display RGB (0–255)"),
    },
    async ({
      assembly_id,
      object,
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
        // Server keys assembly instances by the part's document UUID
        // (`AddInstanceRequest.part_id: Uuid`), same handle as assembly_verify.
        const r = await api(
          "POST",
          `/api/assembly/${assembly_id}/instance`,
          { part_id: object, transform: t, name, color },
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
      "unique_part_count (the gap is the reuse), each instance's part/transform/" +
      "soundness, combined bbox, all_sound.",
    { assembly_id: z.string().describe("assembly id") },
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
      "Returns the updated assembly perception.",
    {
      assembly_id: z.string().describe("assembly id"),
      instance_id: z.string().describe("instance id from assembly_list_instances"),
      position: z
        .array(z.number())
        .length(3)
        .optional()
        .describe("world translation [x,y,z] mm"),
      rotation_deg: z.number().optional().describe("rotation angle about rotation_axis (degrees)"),
      rotation_axis: z
        .array(z.number())
        .length(3)
        .optional()
        .describe("unit rotation axis, e.g. [0,0,1]"),
      transform: z
        .array(z.array(z.number()).length(4))
        .length(4)
        .optional()
        .describe("raw row-major 4×4 pose (overrides position/rotation)"),
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
    "SEE A WHOLE ASSEMBLY: composite every instance at its transform into one " +
      "image from an orbit camera (world-Z up). mode 'diagnostic' highlights " +
      "open (red) / non-manifold (magenta) edges.",
    {
      assembly_id: z.string().describe("assembly id"),
      az: z.number().default(35).describe("azimuth degrees around world Z"),
      el: z.number().default(20).describe("elevation degrees above horizon"),
      mode: z
        .enum(["shaded", "ids", "depth", "normals", "diagnostic"])
        .default("shaded")
        .describe("render channel"),
      size: z.number().int().min(64).max(2048).default(720).describe("image size in px"),
      quality: z
        .enum(["coarse", "medium", "fine"])
        .default("medium")
        .describe("tessellation quality"),
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

  // ── Kinematic assembly: mates, solve, certify, drag, interference, dof
  //    (kinematic-assembly campaign, Slice 6; spec §3.7). Thin wrappers over
  //    the REST document surface — the psketch precedent. The tool
  //    DESCRIPTIONS are the agent's manual for the mate taxonomy: an agent
  //    that reads only these should be able to assemble, solve, drive and
  //    certify a mechanism without rendering anything.

  /** One end of a mate: an instance + WHERE on it the connector frame sits.
   *  The durability ladder, best first — a label (label → PID → assertion)
   *  survives geometry edits; a raw frame does not and must pass the
   *  anti-fabrication anchor probe at certify time. */
  const connectorSpec = z.object({
    instance_id: z.string().uuid().describe("the instance this frame sits on"),
    label: z
      .string()
      .optional()
      .describe("DURABLE (best): a face label; follows the face through edits"),
    pid: z.string().optional().describe("raw persistent face id (durable, unnamed)"),
    face_id: z
      .number()
      .optional()
      .describe("live kernel face id; durability derived (PID if any, else fingerprint)"),
    frame: z
      .object({
        origin: z.array(z.number()).length(3).describe("origin [x,y,z] mm"),
        z_axis: z.array(z.number()).length(3).describe("z axis (the joint axis)"),
        x_axis: z.array(z.number()).length(3).describe("x axis"),
      })
      .optional()
      .describe("RAW coords (NOT durable); must sit on real geometry or certify refuses it"),
  });

  async function makeConnector(assembly_id: string, spec: any): Promise<string> {
    const body: any = { instance_id: spec.instance_id };
    if (spec.frame) {
      body.frame = spec.frame;
    } else {
      body.face = {
        label: spec.label ?? null,
        pid: spec.pid ?? null,
        face_id: spec.face_id ?? null,
      };
    }
    const conn = await api("POST", `/api/assembly/${assembly_id}/connector`, body);
    return conn.id;
  }

  server.tool(
    "assembly_mate",
    "MATE two instances — connectors + mate in ONE call. A mate relates two " +
      "FRAMES and IS the joint (exact DOF by construction).\n" +
      "JOINTS: Fastened (0 DOF) · Revolute{limits} (1 rot/z) · Slider{limits} " +
      "(1 trans/z) · Cylindrical{rot_limits,trans_limits} · Planar (2 trans+" +
      "spin) · Ball (3 rot) · PinSlot{slot_dir_x,limits}. OVERLAYS: " +
      "Distance{value} mm · Angle{value} rad · Parallel · Tangent. COUPLINGS " +
      "(pass `couples` = related mate ids): GearRatio{ratio} & " +
      "RackPinion{pinion_radius} take 2, Screw{lead} takes 1.\n" +
      "REFUSED (typed): Cam, Path, Symmetric.\n" +
      "LIMITS (rot=rad, trans=mm) give assembly_certify a finite range: a " +
      "rotation without limits sweeps a full turn; a translation without limits " +
      "refuses certification.",
    {
      assembly_id: z.string().uuid().describe("assembly id"),
      action: z
        .enum(["create", "edit", "remove"])
        .default("create")
        .describe("create a mate (+ its connectors), edit its kind, or remove it"),
      kind: z
        .any()
        .optional()
        .describe(
          "the mate kind, e.g. 'Fastened' or {Revolute:{limits:[-0.1,0.1]}} " +
            "(required for create/edit)",
        ),
      a: connectorSpec.optional().describe("side A connector (required for create)"),
      b: connectorSpec.optional().describe("side B connector (required for create)"),
      couples: z
        .array(z.string().uuid())
        .optional()
        .describe("for coupling kinds: the related mate ids"),
      mate_id: z
        .string()
        .uuid()
        .optional()
        .describe("the mate to edit/remove"),
    },
    async ({ assembly_id, action, kind, a, b, couples, mate_id }) => {
      try {
        if (action === "remove") {
          if (!mate_id) return fail(new Error("remove needs mate_id"));
          await api("DELETE", `/api/assembly/${assembly_id}/mate/${mate_id}`);
          return ok({ removed: mate_id });
        }
        if (action === "edit") {
          if (!mate_id || kind === undefined)
            return fail(new Error("edit needs mate_id and kind"));
          return ok(
            await api("PATCH", `/api/assembly/${assembly_id}/mate/${mate_id}`, {
              kind,
            }),
          );
        }
        if (!a || !b || kind === undefined)
          return fail(new Error("create needs kind, a and b"));
        const ca = await makeConnector(assembly_id, a);
        const cb = await makeConnector(assembly_id, b);
        return ok(
          await api("POST", `/api/assembly/${assembly_id}/mate`, {
            kind,
            a: ca,
            b: cb,
            couples: couples ?? null,
          }),
        );
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "assembly_solve",
    "SOLVE the mate system: parts are PLACED by their mates. Returns each " +
      "instance's solved pose, per-mate facts (enforced/violation/anchor) and a " +
      "verdict (constrainedness + conflict witnesses). `converged:false` = the " +
      "mates cannot all hold; the witnesses name which fight.",
    {
      assembly_id: z.string().uuid().describe("assembly id"),
      ground: z
        .string()
        .uuid()
        .optional()
        .describe("grounded instance (default: first)"),
    },
    async ({ assembly_id, ground }) => {
      try {
        return ok(
          await api("POST", `/api/assembly/${assembly_id}/solve`, {
            ground: ground ?? null,
          }),
        );
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "assembly_certify",
    "THE FULL CERTIFICATE. `is_sound` = AND of: mates_consistent · " +
      "fully_grounded (no floating part) · no_static_interference · " +
      "swept_clearance_ok · mates_anchored (no invented coordinate) · " +
      "mates_in_contact · mates_enforced. Mobility is REPORTED, not failed. " +
      "`sweeps` = one fact per motion (continuous TOI, no tunneling) with ε and " +
      "any contact; an uncertifiable sweep REFUSES with a reason. Certified " +
      "clearance = distance − ε.",
    {
      assembly_id: z.string().uuid().describe("assembly id"),
      ground: z.string().uuid().optional().describe("grounded instance"),
      epsilon: z
        .number()
        .optional()
        .describe("clearance margin (mm); honoured only ABOVE the kernel floor"),
    },
    async ({ assembly_id, ground, epsilon }) => {
      try {
        return ok(
          await api("POST", `/api/assembly/${assembly_id}/certify`, {
            ground: ground ?? null,
            epsilon: epsilon ?? null,
          }),
        );
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "assembly_dof",
    "DOF + per-instance constrainment: which instances are fully located, which " +
      "MOVE (and how — 'rotation about axis A through P'), which are over-" +
      "constrained and via which mates. Reports STRUCTURAL vs NUMERIC DOF; when " +
      "they disagree (`special_geometry:true`) the geometry is the truth. Cheap.",
    { assembly_id: z.string().uuid().describe("assembly id") },
    async ({ assembly_id }) => {
      try {
        return ok(await api("GET", `/api/assembly/${assembly_id}/dof`));
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "assembly_drag",
    "DRIVE A JOINT: set a joint parameter, the affected chain re-solves, poses " +
      "written back. Driveable: Revolute (rotation), Slider (translation), " +
      "Cylindrical (both); Planar/Ball/PinSlot REFUSE. Drive a coupling's base " +
      "joint, not the coupling. Limits CLAMP (not error). `converged:false` = " +
      "UNREACHABLE, all poses restored (never a half-stroke). `rank_transitions` " +
      "warns of a singular pose.",
    {
      assembly_id: z.string().uuid().describe("assembly id"),
      mate_id: z.string().uuid().describe("the joint to drive"),
      param: z
        .enum(["rotation", "translation"])
        .describe("rotation about the connector z, or translation along it"),
      value: z
        .number()
        .describe("target value (radians for rotation, mm for translation)"),
      ground: z.string().uuid().optional().describe("grounded instance"),
    },
    async ({ assembly_id, mate_id, param, value, ground }) => {
      try {
        return ok(
          await api("POST", `/api/assembly/${assembly_id}/drag`, {
            mate_id,
            param,
            value,
            ground: ground ?? null,
          }),
        );
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "assembly_interference",
    "WHAT TOUCHES, AND WHEN — fix a mechanism without rendering. `static_pairs` " +
      "= overlaps at the solved pose. `sweeps` walk every mate-derived joint " +
      "through its range with continuous TOI, returning motion-stamped facts. " +
      "`first_contact` = where motion first closes; `min_certified_clearance` = " +
      "distance − ε. A `refusal` = not certified (says why).",
    {
      assembly_id: z.string().uuid().describe("assembly id"),
      ground: z.string().uuid().optional().describe("grounded instance"),
      epsilon: z.number().optional().describe("clearance margin (mm) above the kernel floor"),
    },
    async ({ assembly_id, ground, epsilon }) => {
      try {
        const q = new URLSearchParams();
        if (ground) q.set("ground", ground);
        if (epsilon !== undefined) q.set("epsilon", String(epsilon));
        const qs = q.toString();
        return ok(
          await api(
            "GET",
            `/api/assembly/${assembly_id}/interference${qs ? `?${qs}` : ""}`,
          ),
        );
      } catch (e) {
        return fail(e);
      }
    },
  );
}
