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
      "(same object twice = two instances, no copy). Pose: `position` + " +
      "optional `rotation_deg` about `rotation_axis`, OR a raw row-major 4×4 " +
      "`transform`. Returns the instance id + assembly perception.",
    {
      assembly_id: z.string().describe("assembly id from assembly_create"),
      object: z
        .string()
        .describe(
          "the part's object_uuid (from a create_* response or boolean result) — same object twice = two instances",
        ),
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
      .describe(
        "DURABLE: a label naming a face (label → PID → assertion). Prefer this — " +
          "it follows the face through geometry edits.",
      ),
    pid: z
      .string()
      .optional()
      .describe("a raw persistent face id (durable, but unnamed)"),
    face_id: z
      .number()
      .optional()
      .describe(
        "a live kernel face id. Durability is DERIVED: the face's PID when it has " +
          "one, else a geometric fingerprint (degraded — reported per mate).",
      ),
    frame: z
      .object({
        origin: z.array(z.number()).length(3),
        z_axis: z.array(z.number()).length(3),
        x_axis: z.array(z.number()).length(3),
      })
      .optional()
      .describe(
        "RAW coordinates (datum-style). NOT durable, and must sit on the part's " +
          "real geometry or the certificate refuses it as a fabricated joint.",
      ),
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
    "MATE two instances — connectors + mate in ONE call. A mate is ONE " +
      "relationship between two coordinate FRAMES, and the mate IS the joint: " +
      "its DOF signature is exact by construction, so you never stack " +
      "constraints and hope.\n" +
      "JOINTS (primary): Fastened (0 DOF, the rigid lock) · Revolute{limits} " +
      "(1 rot about z) · Slider{limits} (1 trans along z) · " +
      "Cylindrical{rot_limits,trans_limits} (rot+trans) · Planar (2 trans + " +
      "spin) · Ball (3 rot) · PinSlot{slot_dir_x,limits}.\n" +
      "OVERLAYS: Distance{value} · Angle{value} · Parallel · Tangent.\n" +
      "COUPLINGS (pass `couples` = the mate ids they relate): GearRatio{ratio} " +
      "and RackPinion{pinion_radius} take 2; Screw{lead} takes 1 (a " +
      "Cylindrical). The reference configuration is captured from the CURRENT " +
      "poses.\n" +
      "REFUSED (typed, never a silent zero-DOF lie): Cam, Path, Symmetric.\n" +
      "LIMITS are first-class: `{\"Revolute\":{\"limits\":[-0.1,0.1]}}` bounds " +
      "the joint AND gives assembly_certify a finite range to sweep. A " +
      "rotation without limits is swept over a full turn; a TRANSLATION " +
      "without limits has unbounded travel and honestly REFUSES to be " +
      "certified — declare limits if you want a verdict.",
    {
      assembly_id: z.string().uuid(),
      action: z
        .enum(["create", "edit", "remove"])
        .default("create")
        .describe("create a mate (+ its connectors), edit its kind, or remove it"),
      kind: z
        .any()
        .optional()
        .describe(
          'the mate kind, e.g. "Fastened" or {"Revolute":{"limits":[-0.1,0.1]}} ' +
            "(required for create/edit)",
        ),
      a: connectorSpec.optional().describe("side A (required for create)"),
      b: connectorSpec.optional().describe("side B (required for create)"),
      couples: z
        .array(z.string().uuid())
        .optional()
        .describe("for coupling kinds: the mate ids whose parameters are related"),
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
    "SOLVE the mate system: the parts are PLACED by their mates, not by you. " +
      "Returns the solved pose of every instance plus per-mate facts " +
      "(enforced/violation/anchor provenance) and a compact verdict " +
      "(constrainedness + any conflict witnesses) — you can never mutate " +
      "blind. `converged:false` means the mates cannot all hold; read the " +
      "witnesses to see which ones fight.",
    {
      assembly_id: z.string().uuid(),
      ground: z
        .string()
        .uuid()
        .optional()
        .describe("the instance that never moves (defaults to the first)"),
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
    "THE FULL CERTIFICATE — the non-fakeable 'does this go together AND move " +
      "without collision?' verdict. `is_sound` is the AND of: mates_consistent " +
      "· fully_grounded (a part with no mate path to ground is FLOATING) · " +
      "no_static_interference · swept_clearance_ok · mates_anchored (no joint " +
      "declared against an invented coordinate) · mates_in_contact (no paper " +
      "joint) · mates_enforced.\n" +
      "MOBILITY IS REPORTED, NOT FAILED: a mechanism is a design, not a defect.\n" +
      "The `sweeps` carry one fact per motion, over joints DERIVED FROM YOUR " +
      "MATES — nothing is authored, so nothing can be authored wrong. Each " +
      "carries its method (continuous TOI — no tunneling), the ε it ran at, " +
      "and any motion-stamped contact. A sweep it cannot certify REFUSES with " +
      "a reason instead of pretending.\n" +
      "ε is KERNEL-DERIVED from the real tessellation bound; you may only " +
      "RAISE it. Certified clearance = distance − ε, always conservative.",
    {
      assembly_id: z.string().uuid(),
      ground: z.string().uuid().optional(),
      epsilon: z
        .number()
        .optional()
        .describe("only honoured ABOVE the kernel floor — you cannot ask for less"),
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
    "DOF + per-instance constrainment: which instances are fully located, " +
      "which still MOVE (and how — 'rotation about axis A through point P', " +
      "twist-decoded, not just a number), which are over-constrained and via " +
      "which mates. Also reports STRUCTURAL vs NUMERIC DOF side by side: when " +
      "they disagree (`special_geometry:true`) the counting rule lied and the " +
      "geometry is the truth — a four-bar's mobility is a property of its " +
      "CONFIGURATION. Meshless and cheap; read it before you trust a layout.",
    { assembly_id: z.string().uuid() },
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
    "DRIVE A JOINT — your kinematic hand. Set a joint parameter and the " +
      "affected chain re-solves around it; the poses are written back.\n" +
      "param: 'rotation' (θ about the connector z) or 'translation' (s along " +
      "it). Only joints whose freedom those two span are driveable: Revolute " +
      "(θ), Slider (s), Cylindrical (both). Planar, Ball and PinSlot REFUSE " +
      "with a reason — driving θ on a Planar would leave its in-plane " +
      "translations free, so the answer would be an artefact, not kinematics. " +
      "Drive the base joint of a coupling, never the coupling.\n" +
      "LIMITS CLAMP, they do not error: ask for more than the joint has and " +
      "you get its limit plus a `limit` fact saying you bottomed out.\n" +
      "`converged:false` = the drive was UNREACHABLE (something locks it); " +
      "every pose is restored and nothing is written — never a half-stroke.\n" +
      "`scope` names exactly what moved. `rank_transitions` warns that the " +
      "stroke passed through a singular pose where the mechanism gains a DOF.",
    {
      assembly_id: z.string().uuid(),
      mate_id: z.string().uuid().describe("the joint to drive"),
      param: z.enum(["rotation", "translation"]),
      value: z
        .number()
        .describe("target value (radians for rotation, length for translation)"),
      ground: z.string().uuid().optional(),
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
    "WHAT TOUCHES, AND AT WHAT ANGLE — the perception you need to fix a " +
      "mechanism without rendering it. `static_pairs` are overlaps at the " +
      "solved pose. `sweeps` walk every joint DERIVED from your mates through " +
      "its range with continuous time-of-impact (a thin blade CANNOT slip " +
      "between samples) and return motion-stamped facts: 'instances A and B " +
      "interpenetrate by 0.42 at θ=0.15'. `first_contact` is where the motion " +
      "first closes; `min_certified_clearance` is the conservative bound " +
      "(distance − ε) over the whole motion.\n" +
      "A sweep with a `refusal` was NOT certified and says why (unbounded " +
      "slider travel has no finite range) — an honest refusal, not a pass. A " +
      "`manifold_violation` means the motion leaves what the mates allow.",
    {
      assembly_id: z.string().uuid(),
      ground: z.string().uuid().optional(),
      epsilon: z.number().optional(),
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
