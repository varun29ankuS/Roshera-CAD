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
    headers: body !== undefined ? { "Content-Type": "application/json" } : undefined,
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
    mode: z.enum(["shaded", "ids", "depth", "normals"]).default("shaded"),
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
      return { content };
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
  "Delete EVERY part (each deletion timeline-recorded). KNOWN BUG #26: " +
    "after clear, new extrudes may fail until the server restarts — " +
    "prefer delete_part loops until fixed.",
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
      return ok({ part_id: id, placement: id !== null ? await placement(id) : null });
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
      await api("POST", `/api/sketch/${s.id}/extrude`, {
        distance: height,
        name: name ?? null,
      });
      const id = await newestPartId();
      return ok({ part_id: id, placement: id !== null ? await placement(id) : null });
    } catch (e) {
      return fail(e);
    }
  },
);

server.tool(
  "create_cylinder",
  "ONE-CALL cylinder: radius at (cx, cy) on the chosen plane, extruded by " +
    "height. KNOWN BUG #24: the bore/lateral currently tessellates as ~64 " +
    "planar strips, not one analytic cylinder face.",
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
      const s = await api("POST", "/api/sketch", { plane, tool: "circle" });
      await api("POST", `/api/sketch/${s.id}/point`, { point: [cx, cy] });
      await api("POST", `/api/sketch/${s.id}/point`, { point: [cx + radius, cy] });
      await api("POST", `/api/sketch/${s.id}/extrude`, {
        distance: height,
        name: name ?? null,
      });
      const id = await newestPartId();
      return ok({ part_id: id, placement: id !== null ? await placement(id) : null });
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
      await api("POST", `/api/sketch/${s.id}/extrude`, {
        distance: height,
        name: name ?? null,
      });
      const id = await newestPartId();
      return ok({ part_id: id, placement: id !== null ? await placement(id) : null });
    } catch (e) {
      return fail(e);
    }
  },
);

// ─── main ──────────────────────────────────────────────────────────────

const transport = new StdioServerTransport();
await server.connect(transport);
console.error(`roshera-mcp connected (API: ${BASE})`);
