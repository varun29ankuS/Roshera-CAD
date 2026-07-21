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
import { Workbench, registerWorkbenchTool } from "./workbench.js";
import { registerCadProgram } from "./cad_program.js";
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

/**
 * The core modeling + perception verbs always in the default surface, plus the
 * two composition/attention tools (workbench, cad_program) added in S3/S4 — all
 * always exposed in minimal mode so a bench switch and a certified program are
 * one predictable call away.
 */
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
  "workbench",
  "cad_program",
  // The agent→human notebook write verb stays in the default surface so every
  // agent knows the channel exists; list/edit/clear live in the labels bench.
  "blackboard_add_entry",
];

/** The 3 meta-tools — the fixed-cost funnel to the long tail. */
export const META_SURFACE = ["find_tool", "describe_tool", "invoke"];

/** Core + meta = the minimal-complete default exposure (~18 tools). */
export const MINIMAL_SURFACE = [...CORE_SURFACE, ...META_SURFACE];

/**
 * Register every kernel tool, the two composition tools (workbench, cad_program),
 * and the three meta-tools into one fresh table, and build the session Workbench
 * controller over it. The meta-tools + cad_program close over that same table
 * (find_tool / describe_tool / invoke / cad_program all read and dispatch from
 * it); the Workbench controller owns the bench-switch state machine.
 *
 * `index.ts` calls this to get both the table and the controller (which it then
 * wires to the live server's enable/disable). The test suite and the thin
 * `buildTable()` alias use only the table.
 */
export function buildTableWithControls(): {
  table: ToolTable;
  workbench: Workbench;
} {
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
  // Composition tools (S3/S4) — registered into the SAME table so find_tool
  // surfaces them and the token bill counts them.
  const workbench = new Workbench(table, MINIMAL_SURFACE, resolveSurfaceMode());
  registerWorkbenchTool(table, workbench);
  registerCadProgram(table, table);
  // Meta-tools last (they close over the fully-populated table).
  registerMetaTools(table, table);
  return { table, workbench };
}

/**
 * Build only the tool table (the meta-tools + cad_program close over it). The
 * S3/S4 Workbench controller is discarded — used by tests and any caller that
 * needs the static surface without the live bench state machine.
 */
export function buildTable(): ToolTable {
  return buildTableWithControls().table;
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
