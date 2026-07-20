/**
 * Side-effect-free assembly of the tool table + the surface-selection policy —
 * shared by index.ts (which mounts + serves) and the test suite (which asserts
 * the flip, ranking, and validation parity without starting a server).
 *
 * Importing this module registers nothing on any live MCP server and opens no
 * transport; it only builds the in-memory table.
 */

import { ToolTable, RegisteredTool, estimateTokens } from "./registry.js";
import { registerMetaTools } from "./metatools.js";
import { registerPerceptionTools } from "./tools/perception.js";
import { registerInspectTools } from "./tools/inspect.js";
import { registerQueryTools } from "./tools/queries.js";
import { registerCreateTools } from "./tools/create.js";
import { registerModifyTools } from "./tools/modify.js";
import { registerPsketchTools } from "./tools/psketch.js";
import { registerIoTools } from "./tools/io.js";
import { registerTimelineTools } from "./tools/timeline.js";
import { registerBlackboardTools } from "./tools/blackboard.js";
import { registerLabelTools } from "./tools/labels.js";
import { registerAssemblyTools } from "./tools/assembly.js";
import { registerGdtTools } from "./tools/gdt.js";
import { registerDrawingTools } from "./tools/drawing.js";

/** The 15 core modeling + perception verbs always in the default surface. */
export const CORE_SURFACE = [
  "create_box",
  "create_cylinder",
  "create_sphere",
  "create_cone",
  "boolean",
  "revolve",
  "get_part",
  "list_parts",
  "render_part",
  "scene_view",
  "section_view",
  "verify_part",
  "mass_properties",
  "delete_part",
  "clear_parts",
];

/** The 3 meta-tools — the fixed-cost funnel to the long tail. */
export const META_SURFACE = ["find_tool", "describe_tool", "invoke"];

/** Core + meta = the minimal-complete default exposure (~18 tools). */
export const MINIMAL_SURFACE = [...CORE_SURFACE, ...META_SURFACE];

/**
 * Register every tool + the three meta-tools into one fresh table. The meta-
 * tools close over that same table (find_tool / describe_tool / invoke all read
 * and dispatch from it).
 */
export function buildTable(): ToolTable {
  const table = new ToolTable();
  registerPerceptionTools(table);
  registerInspectTools(table);
  registerQueryTools(table);
  registerCreateTools(table);
  registerModifyTools(table);
  registerPsketchTools(table);
  registerIoTools(table);
  registerTimelineTools(table);
  registerBlackboardTools(table);
  registerLabelTools(table);
  registerAssemblyTools(table);
  registerGdtTools(table);
  registerDrawingTools(table);
  registerMetaTools(table, table);
  return table;
}

export type SurfaceMode = "minimal" | "full";

/** ROSHERA_MCP_SURFACE=full restores the full exposure; default is minimal. */
export function resolveSurfaceMode(): SurfaceMode {
  const raw = (process.env.ROSHERA_MCP_SURFACE ?? "minimal").toLowerCase();
  return raw === "full" ? "full" : "minimal";
}

/**
 * The tool names exposed as direct MCP tools for a given mode.
 *  - `full`    restores today's full 90-tool exposure (the transition escape
 *              hatch): every kernel tool directly, meta-tools omitted — with the
 *              whole surface present, the funnel is redundant.
 *  - `minimal` (default) the 15 core verbs + 3 meta-tools; the long tail stays
 *              reachable through invoke at fixed cost.
 */
export function exposedNamesFor(table: ToolTable, mode: SurfaceMode): string[] {
  if (mode === "full") {
    return table.names().filter((n) => !META_SURFACE.includes(n));
  }
  return MINIMAL_SURFACE.filter((n) => table.has(n));
}

/** Summed token-cost proxy over a set of tool names. */
export function billFor(table: ToolTable, names: string[]): number {
  let sum = 0;
  for (const n of names) {
    const e = table.get(n);
    if (e) sum += estimateTokens(e);
  }
  return sum;
}
