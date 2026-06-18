#!/usr/bin/env node
/**
 * Roshera MCP server — the agent-facing tool surface over the Roshera
 * geometry kernel's REST API.
 *
 * Design doctrine (from the 2026-06-12 live sessions):
 *  - LATENCY: batch tools (`sketch_polygon` takes all points in one call)
 *    and composite tools (`create_cylinder` = sketch+points+extrude in one
 *    call) collapse what previously took N round trips into one.
 *  - PERCEPTION: `render_part` returns the image as MCP image content —
 *    the agent SEES the geometry directly in the tool result. `mode:"ids"`
 *    is set-of-marks for topology (flat color per face + legend).
 *  - SHARED ATTENTION: `get_pointer` reads what the human is pointing at
 *    in the viewport (the HOVER-α bridge), so "this face" grounds against
 *    real topology.
 *  - PLACEMENT IS EXPLICIT: every create tool takes coordinates; every
 *    result echoes the part's world placement.
 *
 * Server URL via ROSHERA_URL (default http://localhost:8081).
 */

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { randomUUID } from "node:crypto";

const BASE = process.env.ROSHERA_URL ?? "http://localhost:8081";

// ─── HTTP helpers ──────────────────────────────────────────────────────

class ApiError extends Error {
  constructor(message: string, public status: number, public body: string) {
    super(message);
  }
}

async function api(
  method: "GET" | "POST" | "DELETE",
  path: string,
  body?: unknown,
): Promise<any> {
  const res = await fetch(`${BASE}${path}`, {
    method,
    headers: {
      // Timeline attribution: the backend's agent_author_layer records
      // every kernel op from this request as Author::AIAgent("Claude"),
      // so agent-built features show amber Ⓒ in the Timeline strip.
      "X-Roshera-Agent": "Claude",
      ...(body !== undefined ? { "Content-Type": "application/json" } : {}),
    },
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  if (!res.ok) {
    throw new ApiError(
      `${method} ${path} → ${res.status}: ${text}`,
      res.status,
      text,
    );
  }
  return text.length ? JSON.parse(text) : null;
}

function ok(data: unknown) {
  return {
    content: [{ type: "text" as const, text: JSON.stringify(data, null, 2) }],
  };
}

function fail(e: unknown) {
  const msg = e instanceof Error ? e.message : String(e);
  return {
    content: [{ type: "text" as const, text: `ERROR: ${msg}` }],
    isError: true as const,
  };
}

/** Fetch a part's placement so create-tools can echo where things landed. */
async function placement(partId: number) {
  try {
    const r = await api("GET", `/api/agent/parts/${partId}`);
    return {
      center_world: r?.location?.center_world ?? null,
      dimensions_world: r?.location?.dimensions_world ?? null,
    };
  } catch {
    return null;
  }
}

async function newestPartId(): Promise<number | null> {
  const parts = await api("GET", "/api/agent/parts");
  if (!Array.isArray(parts) || parts.length === 0) return null;
  return parts.reduce((m: number, p: any) => Math.max(m, p.id), 0);
}

/**
 * Automatic perception — the ambient default. After any mutating op, fetch the
 * result part's validity verdict + structural facts so the agent never operates
 * blind. Watertightness comes from the diagnostic render (open / non-manifold
 * edge counts, the same source `verify_part` uses); face-count / volume / bbox
 * from the part query. Default-ON; disable per process with
 * `ROSHERA_MCP_AUTOVERIFY=0`. Best-effort: returns `undefined` (no perception
 * block, never an error) if anything fails, so it can't break a real result.
 */
async function perceive(partId: number | null): Promise<any> {
  if (partId === null || process.env.ROSHERA_MCP_AUTOVERIFY === "0") {
    return undefined;
  }
  try {
    // SOUND channel: /perception reports the B-Rep validity verdict
    // (validate_solid_scoped, mesh-independent) plus a manifold check.
    // The B-Rep verdict is authoritative — a valid solid whose DISPLAY
    // tessellation has T-junctions is NOT broken (see KNOWN_BUGS #65 /
    // EYE-SOUND). We never judge soundness off the display render.
    const p = await api("GET", `/api/agent/parts/${partId}/perception`);
    const part = await api("GET", `/api/agent/parts/${partId}`).catch(() => null);
    const valid = p?.valid === true;
    const meshClean = p?.watertight === true;
    return {
      brep_valid: valid,
      watertight: meshClean,
      open_edges: p?.open_edges ?? null,
      nonmanifold_edges: p?.nonmanifold_edges ?? null,
      dims: p?.dims ?? null,
      face_count: part?.topology?.face_count ?? null,
      volume: part?.volume ?? null,
      verdict: !valid
        ? "BROKEN — B-Rep invalid (real topological defect)"
        : meshClean
          ? "OK — valid closed solid"
          : "OK — valid B-Rep; display mesh has tessellation T-junctions only (not a defect)",
    };
  } catch {
    return undefined;
  }
}

/** `ok()` plus an automatic perception verdict for the resulting part. */
async function okp(data: Record<string, unknown>, partId: number | null) {
  const perception = await perceive(partId);
  return ok(perception === undefined ? data : { ...data, perception });
}

// ─── Server + tools ────────────────────────────────────────────────────

const server = new McpServer({ name: "roshera", version: "0.1.0" });

// ---- perception -------------------------------------------------------

server.tool(
  "render_part",
  "SEE a part: deterministic offscreen render returned as an image. " +
    "mode 'shaded' shows form; 'ids' paints every B-Rep face a distinct " +
    "flat color and returns a color→face_id legend (use it to ADDRESS " +
    "topology: 'the red face is face 12'); 'depth' and 'normals' are exact " +
    "G-buffer channels.",
  {
    part_id: z.number().int().describe("kernel part id from list_parts"),
    mode: z
      .enum(["shaded", "ids", "depth", "normals", "diagnostic"])
      .default("shaded"),
    view: z.enum(["iso", "front", "top", "right"]).default("iso"),
    size: z.number().int().min(64).max(2048).default(512),
  },
  async ({ part_id, mode, view, size }) => {
    try {
      const r = await api(
        "GET",
        `/api/agent/parts/${part_id}/render?mode=${mode}&view=${view}&size=${size}`,
      );
      const content: any[] = [
        { type: "image", data: r.png_base64, mimeType: "image/png" },
      ];
      if (mode === "ids") {
        content.push({
          type: "text",
          text: `face legend (rgb → face_id): ${JSON.stringify(r.face_legend)}`,
        });
      }
      if (mode === "diagnostic") {
        content.push({
          type: "text",
          text:
            `defects — open_edges (red hole rims, missing faces): ${r.open_edges}; ` +
            `nonmanifold_edges (magenta, overlapping faces): ${r.nonmanifold_edges}. ` +
            `Both 0 = watertight. Front-face culled, so missing/flipped faces read as see-through holes.`,
        });
      }
      return { content };
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "verify_part",
  "SELF-CHECK a part's geometry. The verdict is the SOUND, mesh-independent " +
    "B-Rep check (validate_solid_scoped via /perception): brep_valid is the " +
    "authoritative 'is this a real closed solid' answer. The display mesh's " +
    "open/non-manifold edge counts are reported separately as DISPLAY quality " +
    "— a valid B-Rep can still tessellate with T-junctions (KNOWN_BUGS #65), " +
    "which is a rendering artifact, NOT a broken solid. ALWAYS call this after " +
    "a boolean or multi-feature build. Returns the iso diagnostic image so you " +
    "can SEE where any real (red=open / magenta=non-manifold) defect is.",
  {
    part_id: z.number().int().describe("kernel part id from list_parts"),
    view: z.enum(["iso", "front", "top", "right"]).default("iso"),
  },
  async ({ part_id, view }) => {
    try {
      // Sound verdict from the B-Rep; image + display-mesh counts from the render.
      const p = await api("GET", `/api/agent/parts/${part_id}/perception`);
      const r = await api(
        "GET",
        `/api/agent/parts/${part_id}/render?mode=diagnostic&view=${view}&size=720`,
      );
      const valid = p.valid === true;
      const meshClean = r.open_edges === 0 && r.nonmanifold_edges === 0;
      return {
        content: [
          {
            type: "text",
            text: JSON.stringify(
              {
                part_id,
                brep_valid: valid,
                brep_watertight: p.watertight === true,
                verdict: !valid
                  ? "BROKEN — B-Rep invalid (a real topological defect; see the image)"
                  : meshClean
                    ? "OK — valid closed solid"
                    : "OK — valid solid; display mesh has tessellation T-junctions only (not a defect)",
                display_mesh: {
                  open_edges: r.open_edges,
                  nonmanifold_edges: r.nonmanifold_edges,
                  note: "display tessellation quality only — does NOT determine validity",
                },
                dims: p.dims ?? null,
              },
              null,
              2,
            ),
          },
          { type: "image", data: r.png_base64, mimeType: "image/png" },
        ],
      };
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "make_drawing",
  "Generate a 2D engineering DRAWING from a part: the standard four-view " +
    "sheet — Front / Top / Right plus an isometric pictorial — with " +
    "hidden-line removal, centerlines, and automatic dimensions. The sheet " +
    "size and scale are chosen to fit the part (small parts on A4, growing " +
    "to A0), and the views are centered with proper offset dimension lines. " +
    "Returns the new drawing id (open it in the Drawing workspace, or fetch " +
    "GET /api/drawings/<id>/svg|pdf|dxf) AND a QUALITY report — the 2D " +
    "perception layer: whether the layout passed, sheet utilization, and any " +
    "issues (views overlapping, off-sheet, dimensions on the outline). Treat " +
    "the quality report the way you treat watertightness for 3D geometry.",
  {
    part_id: z.number().int().describe("kernel part/solid id from list_parts"),
    name: z.string().optional().describe("title-block name for the sheet"),
  },
  async ({ part_id, name }) => {
    try {
      const qs = name ? `?name=${encodeURIComponent(name)}` : "";
      const r = await api("POST", `/api/parts/${part_id}/drawing${qs}`);
      const q = r?.quality ?? null;
      return ok({
        drawing_id: r?.id ?? null,
        quality: q,
        verdict: q
          ? q.passed
            ? `OK — clean sheet (${Math.round((q.sheet_utilization ?? 0) * 100)}% utilization, ${
                q.issues?.length ?? 0
              } advisory issue(s))`
            : `LAYOUT ISSUES — ${q.issues?.length ?? 0} finding(s); see quality.issues`
          : "drawing created (no quality report)",
        note: "Open in the Drawing workspace, or GET /api/drawings/<id>/svg|pdf|dxf.",
      });
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "get_pointer",
  "What is the HUMAN pointing at in the viewport right now? Returns their " +
    "latest click (object, face_id, world position) joined with the " +
    "kernel's hover report (surface type, area, host part). Use to ground " +
    "'this face / this hole / here'.",
  {},
  async () => {
    try {
      return ok(await api("GET", "/api/agent/pointer"));
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) {
        return ok({ pointer: null, note: "user has not clicked anything yet" });
      }
      return fail(e);
    }
  },
);

// ---- inspection -------------------------------------------------------

server.tool(
  "list_parts",
  "List every part in the live model (id, name, kind).",
  {},
  async () => {
    try {
      return ok(await api("GET", "/api/agent/parts"));
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "get_part",
  "Full report for one part: world placement (center, dimensions, anchor " +
    "datum), topology fingerprint, name.",
  { part_id: z.number().int() },
  async ({ part_id }) => {
    try {
      return ok(await api("GET", `/api/agent/parts/${part_id}`));
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "mass_properties",
  "Exact mass properties: volume, mass, center of mass, inertia tensor, " +
    "principal axes (mesh-integrated, accuracy-gated against closed form).",
  { part_id: z.number().int() },
  async ({ part_id }) => {
    try {
      return ok(await api("GET", `/api/agent/parts/${part_id}/mass`));
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "get_face",
  "Per-face report: surface type, area, principal curvatures " +
    "([0,0]=flat, [±1/r,0]=cylindrical, [±1/r,±1/r]=spherical), boundary " +
    "edges, neighbours. Face ids come from render_part mode 'ids' or " +
    "get_pointer.",
  { face_id: z.number().int() },
  async ({ face_id }) => {
    try {
      return ok(await api("GET", `/api/agent/faces/${face_id}`));
    } catch (e) {
      return fail(e);
    }
  },
);

// ---- mutation: deletion ----------------------------------------------

server.tool(
  "delete_part",
  "Delete one part (timeline-recorded, undo-safe). WARNING: kernel part " +
    "ids RENUMBER after deletion — re-run list_parts before further " +
    "deletes; never reuse stale ids.",
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
    "build immediately after (the post-clear extrude wedge, bug #26, is fixed).",
  {},
  async () => {
    try {
      return ok(await api("DELETE", "/api/agent/parts"));
    } catch (e) {
      return fail(e);
    }
  },
);

// ---- mutation: sketch & build ----------------------------------------

const PlaneSchema = z
  .union([
    z.enum(["xy", "xz", "yz"]),
    z.object({
      origin: z.tuple([z.number(), z.number(), z.number()]),
      u_axis: z.tuple([z.number(), z.number(), z.number()]),
      v_axis: z.tuple([z.number(), z.number(), z.number()]),
    }),
  ])
  .describe(
    "'xy' | 'xz' | 'yz' or a custom plane {origin, u_axis, v_axis} (e.g. " +
      "from plane_from_face)",
  );

server.tool(
  "create_sketch",
  "Start a sketch session on a plane. Returns sketch_id for the shape/" +
    "point/extrude tools. Prefer the composite tools (create_box / " +
    "create_cylinder / sketch_polygon) when they fit — fewer round trips.",
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
    "outer boundary — region detection assigns outer/hole roles at " +
    "extrude time). Returns the new shape_index.",
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
    "circle: center then a point on the radius; polyline: every vertex of " +
    "the closed polygon — a 96-point gear outline is ONE call). " +
    "shape_index omitted = the first shape.",
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
    "detection (outer boundary + holes). Returns the new part id and its " +
    "world placement.",
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
  "Derive a sketch plane FROM an existing planar face (stack features on " +
    "decks: 'sketch on this face'). object_id is the part's public UUID; " +
    "face_id from get_pointer or render legend. Returns {origin, u_axis, " +
    "v_axis} to pass as create_sketch's plane.",
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

// ---- composite creators (one call = one part) -------------------------

server.tool(
  "create_box",
  "ONE-CALL box: width × depth centered at (cx, cy) on the chosen plane, " +
    "extruded by height. Returns part id + world placement.",
  {
    plane: PlaneSchema.default("xy"),
    cx: z.number().default(0),
    cy: z.number().default(0),
    width: z.number().positive(),
    depth: z.number().positive(),
    height: z.number(),
    name: z.string().optional(),
  },
  async ({ plane, cx, cy, width, depth, height, name }) => {
    try {
      const s = await api("POST", "/api/sketch", { plane, tool: "rectangle" });
      await api("POST", `/api/sketch/${s.id}/point`, {
        point: [cx - width / 2, cy - depth / 2],
      });
      await api("POST", `/api/sketch/${s.id}/point`, {
        point: [cx + width / 2, cy + depth / 2],
      });
      const r = await api("POST", `/api/sketch/${s.id}/extrude`, {
        distance: height,
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
  "create_cylinder",
  "ONE-CALL analytic cylinder: radius at (cx, cy) on the chosen plane, extruded " +
    "by height along the plane normal. Uses the analytic cylinder primitive " +
    "(one smooth lateral face) — NOT sketch+extrude, which produced an " +
    "inside-out lateral mesh (⅓ volume, negative inertia, dropped boss walls in " +
    "coaxial bores).",
  {
    plane: PlaneSchema.default("xy"),
    cx: z.number().default(0),
    cy: z.number().default(0),
    radius: z.number().positive(),
    height: z.number(),
    name: z.string().optional(),
  },
  async ({ plane, cx, cy, radius, height, name }) => {
    try {
      // Resolve the plane to a world origin + in-plane (u, v) basis.
      const std: Record<string, { o: number[]; u: number[]; v: number[] }> = {
        xy: { o: [0, 0, 0], u: [1, 0, 0], v: [0, 1, 0] },
        xz: { o: [0, 0, 0], u: [1, 0, 0], v: [0, 0, 1] },
        yz: { o: [0, 0, 0], u: [0, 1, 0], v: [0, 0, 1] },
      };
      const p =
        typeof plane === "string"
          ? std[plane]
          : { o: plane.origin, u: plane.u_axis, v: plane.v_axis };
      const cross = (a: number[], b: number[]) => [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
      ];
      // Cylinder base centre = plane origin + cx·u + cy·v; axis = u × v (the
      // plane normal), matching the old sketch-extrude placement.
      const center = [0, 1, 2].map(
        (i) => p.o[i] + cx * p.u[i] + cy * p.v[i],
      );
      const axis = cross(p.u, p.v);
      const r = await api("POST", "/api/geometry/cylinder", {
        center,
        axis,
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
  "ONE-CALL analytic cone or frustum. base_radius is the radius at `center`; " +
    "top_radius (default 0 = sharp apex) is the radius at center+axis*height. " +
    "A true smooth cone surface (not faceted). Returns part id + placement.",
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

server.tool(
  "create_sphere",
  "ONE-CALL analytic sphere of `radius` at the origin. Returns part id + placement.",
  { radius: z.number().positive(), name: z.string().optional() },
  async ({ radius }) => {
    try {
      const r = await api("POST", "/api/geometry", {
        shape_type: "sphere",
        parameters: { radius },
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
  "Build a SOLID OF REVOLUTION from a closed meridian profile — the correct " +
    "primitive for any axisymmetric part (nozzles, pulleys, bottles, pressure " +
    "vessels, a whole rocket engine in one op). `profile` is a closed polygon of " +
    "[r, z] points (radius-from-axis, height-along-axis), revolved about the axis " +
    "(default +Z through origin, full 360°). One op, no booleans, watertight, " +
    "structured smooth mesh. Profile must be a simple loop with all r ≥ 0 and not " +
    "cross the axis (no r=0 pole). Hollow part = trace the wall cross-section.",
  {
    profile: z
      .array(z.tuple([z.number(), z.number()]))
      .min(3)
      .describe("closed [r,z] meridian profile (auto-closes last→first)"),
    axis_origin: z.tuple([z.number(), z.number(), z.number()]).default([0, 0, 0]),
    axis_direction: z.tuple([z.number(), z.number(), z.number()]).default([0, 0, 1]),
    angle_deg: z.number().default(360),
    segments: z.number().int().min(3).max(512).default(96),
    name: z.string().optional(),
  },
  async ({ profile, axis_origin, axis_direction, angle_deg, segments, name }) => {
    try {
      const r = await api("POST", "/api/geometry/revolve", {
        profile,
        axis_origin,
        axis_direction,
        angle_deg,
        segments,
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
  "Build a watertight FREEFORM SOLID by skinning a single NURBS surface through " +
    "a stack of cross-section rings — the primitive for organic / sculpted shapes " +
    "that revolve and extrude can't make (bulged barrels, ogives, twisted/lobed " +
    "transitions, blended ducts). `sections` is an ordered list of cross-sections, " +
    "each an OPEN ring of [x,y,z] points of the SAME count (the op closes each " +
    "ring); sections are stacked along the loft. The lateral wall is one genuine " +
    "NURBS surface interpolated through the rings; at the default degree_v=3 it is " +
    "G2 (curvature-continuous) along the loft. The first and last sections must be " +
    "planar (they become the end caps). One op, no booleans, watertight.",
  {
    sections: z
      .array(z.array(z.tuple([z.number(), z.number(), z.number()])).min(3))
      .min(2)
      .describe("stack of cross-section rings (each an open ring, equal point count)"),
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

server.tool(
  "section_view",
  "CUTAWAY: slice a part with a plane and return the cross-section as an image " +
    "(steel-filled profile + section area). The way to SEE a hollow interior. " +
    "Plane = point `p` + `normal`; an axial cut (normal ⟂ the part's axis through " +
    "its center) reveals wall thickness, bores and internal cavities.",
  {
    part_id: z.number().int(),
    p: z.tuple([z.number(), z.number(), z.number()]).default([0, 0, 0]),
    normal: z.tuple([z.number(), z.number(), z.number()]).default([1, 0, 0]),
  },
  async ({ part_id, p, normal }) => {
    try {
      const q = `nx=${normal[0]}&ny=${normal[1]}&nz=${normal[2]}&px=${p[0]}&py=${p[1]}&pz=${p[2]}`;
      const r = await api("GET", `/api/agent/parts/${part_id}/section?${q}`);
      return {
        content: [
          { type: "image" as const, data: r.png_base64, mimeType: "image/png" },
          {
            type: "text" as const,
            text: `section area=${r.section_area?.toFixed?.(2)} extent_u=${r.extent_u?.toFixed?.(2)} extent_v=${r.extent_v?.toFixed?.(2)} units=${r.units}`,
          },
        ],
      };
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "dimension_part",
  "DIMENSION a part in ONE call. Returns a 2×2 multi-view image with every " +
    "analytic dimension drawn as a leader+label callout, AND the structured " +
    "table: each row has an id (the handle a future mould edits), kind " +
    "(extent / diameter / length / angle), value, the face ids it spans, and a " +
    "3D anchor. Values are read off analytic surfaces / exact curves, NEVER " +
    "measured from pixels — the id you SEE is the id you edit.",
  { part_id: z.number().int().describe("kernel part id from list_parts") },
  async ({ part_id }) => {
    try {
      const r = await api("GET", `/api/agent/parts/${part_id}/dimensions`);
      const rows = (r.dimensions ?? [])
        .map(
          (d: any) =>
            `${d.id}  ${d.label}  (${d.kind} ${d.value.toFixed(2)}${
              d.unit === "deg" ? "°" : ""
            })  faces=[${d.entities.join(",")}]  @[${d.anchor
              .map((c: number) => c.toFixed(1))
              .join(", ")}]`,
        )
        .join("\n");
      const overall = `overall L×W×H = ${r.dims.l.toFixed(2)} × ${r.dims.w.toFixed(
        2,
      )} × ${r.dims.h.toFixed(2)} ${r.units}`;
      return {
        content: [
          { type: "image" as const, data: r.png_base64, mimeType: "image/png" },
          { type: "text" as const, text: `${overall}\n${rows}` },
        ],
      };
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "create_plate_with_holes",
  "ONE-CALL plate: rectangle (width × depth at cx,cy) with circular holes, " +
    "extruded. Holes are [hx, hy, radius] triples in plane coordinates.",
  {
    plane: PlaneSchema.default("xy"),
    cx: z.number().default(0),
    cy: z.number().default(0),
    width: z.number().positive(),
    depth: z.number().positive(),
    height: z.number(),
    holes: z.array(z.tuple([z.number(), z.number(), z.number()])).default([]),
    name: z.string().optional(),
  },
  async ({ plane, cx, cy, width, depth, height, holes, name }) => {
    try {
      const s = await api("POST", "/api/sketch", { plane, tool: "rectangle" });
      await api("POST", `/api/sketch/${s.id}/point`, {
        point: [cx - width / 2, cy - depth / 2],
      });
      await api("POST", `/api/sketch/${s.id}/point`, {
        point: [cx + width / 2, cy + depth / 2],
      });
      for (const [hx, hy, r] of holes) {
        const sh = await api("POST", `/api/sketch/${s.id}/shape`, {
          tool: "circle",
        });
        const idx = (sh.shapes?.length ?? 1) - 1;
        await api("POST", `/api/sketch/${s.id}/shape/${idx}/point`, {
          point: [hx, hy],
        });
        await api("POST", `/api/sketch/${s.id}/shape/${idx}/point`, {
          point: [hx + r, hy],
        });
      }
      const r = await api("POST", `/api/sketch/${s.id}/extrude`, {
        distance: height,
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


// ---- mutation: boolean (feature stacking) -----------------------------

server.tool(
  "boolean",
  "Combine two solids: union (weld together), difference (cut object_b out " +
    "of object_a), or intersection (keep only the overlap). Operands are " +
    "OBJECT UUIDs (the object_uuid returned by the create/extrude tools), " +
    "NOT kernel part ids. Both operands are consumed; a new solid is born. " +
    "This is feature-stacking — chained unions and bores. ALWAYS verify_part " +
    "the result: differences (bores/slots/counterbores) can leave open faces.",
  {
    op: z.enum(["union", "difference", "intersection"]),
    object_a: z.string().uuid().describe("object_uuid of the base solid"),
    object_b: z
      .string()
      .uuid()
      .describe("object_uuid of the tool solid (subtracted in difference)"),
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
  "transform",
  "Move and/or rotate a solid IN PLACE by its object_uuid. Identity is " +
    "preserved — same uuid, the viewport updates the existing object " +
    "rather than spawning a new one. Rotation (about an optional center, " +
    "default origin) applies first, then translation. Supply translation " +
    "and/or rotation (at least one). Angle is in DEGREES. After moving, " +
    "render_part to confirm the new position.",
  {
    object: z.string().uuid().describe("object_uuid of the solid to move"),
    translation: z
      .tuple([z.number(), z.number(), z.number()])
      .optional()
      .describe("[dx, dy, dz] world-space offset"),
    rotation: z
      .object({
        axis: z
          .tuple([z.number(), z.number(), z.number()])
          .describe("rotation axis (need not be unit length)"),
        angle_deg: z.number().describe("rotation angle in DEGREES"),
        center: z
          .tuple([z.number(), z.number(), z.number()])
          .optional()
          .describe("pivot point, default origin [0,0,0]"),
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

// ─── Parametric sketching (csketch — constraint-solver backed) ────────

server.tool(
  "psketch_create",
  "Create a PARAMETRIC sketch (constraint-solver backed, XY plane). " +
    "Use psketch_* tools to add entities/constraints, solve, and extrude. " +
    "Unlike create_sketch (click-draft), geometry here can be DIMENSIONED " +
    "exactly: add entities loosely, constrain, solve — the solver places " +
    "everything to machine precision.",
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
  "Add an entity to a parametric sketch. kind=point {x,y,fixed?}, " +
    "line {start,end point uuids}, circle {cx,cy,radius}, arc {cx,cy," +
    "radius,start_angle,end_angle}, rectangle {x1,y1,x2,y2}, polyline " +
    "{points[[x,y]...],closed}. Returns the entity id.",
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
  "Add a constraint. geometric: Horizontal/Vertical/Parallel/" +
    "Perpendicular/Coincident/Tangent/Concentric/Equal etc on entities. " +
    "dimensional: {Distance: 80.0} / {Radius: 6.0} / {Angle: 1.57} etc. " +
    "entities = [{Line: uuid}] or [{Point: uuid}, {Point: uuid}] ...",
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
  "Run the Newton-Raphson solver. Converged = geometry now satisfies " +
    "every constraint exactly. Returns status + solved entity positions.",
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
  "Extrude the parametric sketch's closed regions into a solid. " +
    "Hole-aware (circles inside an outline become bores). On topology " +
    "errors the response names every gap/dangling endpoint so you can " +
    "repair the sketch surgically. Records a replayable timeline event.",
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
  "timeline_scrub",
  "Look at the scene AS OF a past event — non-destructive (live scene " +
    "untouched). Returns object count + mesh stats at that moment. " +
    "branch 'main' unless exploring a fork.",
  { branch: z.string().default("main"), sequence: z.number().int() },
  async ({ branch, sequence }) => {
    try {
      const r = await api("GET", `/api/timeline/scrub/${branch}/${sequence}`);
      return ok({
        at_sequence: r.at_sequence,
        events_applied: r.events_applied,
        events_total: r.events_total,
        objects: (r.objects ?? []).map((o: any) => ({
          id: o.id,
          name: o.name,
          triangles: (o.mesh?.indices?.length ?? 0) / 3,
        })),
      });
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "clear_timeline",
  "Reset a timeline branch to ZERO events and wipe the live model to " +
    "match — the one-shot 'start over'. Default branch 'main'. DESTRUCTIVE " +
    "and irreversible: the event ledger is rewritten, not undoable. Unlike " +
    "undo/truncate (which refuse the protected main trunk and need a " +
    "specific event), this clears the whole branch. Use clear_parts instead " +
    "if you only want an empty scene but a preserved history.",
  {
    branch_id: z
      .string()
      .default("main")
      .describe("branch to clear; 'main' is the trunk"),
  },
  async ({ branch_id }) => {
    try {
      // The endpoint seeds its own replay position, so a fresh per-call
      // session id is sufficient; the truncate is branch-scoped, not
      // session-scoped.
      const r = await api("POST", "/api/timeline/clear", {
        session_id: randomUUID(),
        branch_id,
      });
      return ok({
        events_removed: r.events_removed,
        model_reconciled: r.model_reconciled,
        branch_id: r.branch_id ?? branch_id,
      });
    } catch (e) {
      return fail(e);
    }
  },
);

// ─── main ──────────────────────────────────────────────────────────────

const transport = new StdioServerTransport();
await server.connect(transport);
console.error(`roshera-mcp connected (API: ${BASE})`);
