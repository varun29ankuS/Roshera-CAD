/**
 * Layer 1 — WORKBENCHES (spec `2026-07-20-mcp-scale-architecture-design.md`,
 * §Layer 1, slice S3). A tier-1 ATTENTION optimization on top of the already-
 * correct minimal surface — NEVER a capability gate. Entering a bench exposes
 * that bench's tools on top of the always-on core+meta surface and retires the
 * previously-active bench; the long tail stays reachable through
 * find_tool/describe_tool/invoke in every mode (that reachability is the S2
 * guarantee — `invoke` dispatches through the ToolTable, bypassing the live
 * server's enabled-gate entirely).
 *
 * This module is PURE: it owns the session bench-state machine and computes the
 * exposed name set + the enable/disable transition, but touches no live server.
 * index.ts injects an `apply(toEnable, toDisable)` that flips the real McpServer
 * tool handles and lets the SDK emit `tools/list_changed`. The test suite drives
 * the same controller with a recording `apply` and asserts the invariants
 * (status shape, exposure set, ≤36 ceiling) without starting a server.
 */

import { z } from "zod";
import {
  ToolTable,
  ToolHost,
  Bench,
  metaFor,
  estimateTokens,
} from "./registry.js";
import type { SurfaceMode } from "./surface.js";
import { ok } from "./core.js";
import { gist } from "./metatools.js";

/** The user-facing `workbench(mode)` argument. */
export type WorkbenchMode =
  | "sketch"
  | "assembly"
  | "drawing"
  | "analysis"
  | "simulate"
  | "labels"
  | "timeline"
  | "core_only"
  | "status"
  | "catalog";

/** The benches a `workbench` call can switch between (core/meta are always on). */
export const SWITCHABLE_BENCHES: Bench[] = [
  "sketch",
  "assembly",
  "drawing",
  "analysis",
  "simulate",
  "labels",
  "timeline",
];

/**
 * The soft ceiling on the live (exposed) surface in any bench (spec §Layer 1).
 * 36, not 35: `timeline` (2026-07-31) is the first bench whose 15 tools push
 * core+meta (21) past the original 35 — MINIMAL_SURFACE(21) + 15 = 36. Raised
 * by exactly the overshoot rather than left as a hard 35, per the "soft
 * ceiling" framing above: an attention budget, not a hard capability gate.
 *
 * 36 -> 45 (2026-08-10, task #12): the psketch_* family (10 tools) joined
 * CORE_SURFACE, so MINIMAL_SURFACE grew 21 -> 31. `exposedNames()` is now
 * deduped (a core∩bench member — blackboard_add_entry, timeline_checkpoint,
 * or a psketch_* tool — is never double-counted), so the formula per bench is
 * MINIMAL_SURFACE(31) + that-bench's-own-count - its-overlap-with-core. Every
 * bench's live count, measured after dedup: sketch 36 (31+15-10 psketch_*
 * overlap), timeline 45 (31+15-1 timeline_checkpoint overlap), labels 41
 * (31+11-1 blackboard_add_entry overlap), analysis 45 (31+14, zero overlap),
 * assembly 43 (31+12, zero overlap), drawing 39 (31+8, zero overlap),
 * core_only 31. Max is 45 (analysis and timeline tie). Raised to the new
 * measured max, same "raised by exactly the overshoot" policy as the
 * original 35->36 move.
 *
 * 45 -> 46 (2026-08-11, task #10): `recipe_get` joined the timeline bench
 * (retrieval of a certified past build as a re-parameterizable plan), so
 * timeline measures 31 + 16 - 1 = 46 and is now the sole max. Raised by
 * exactly the overshoot again, per the same policy — this is an attention
 * budget, and one more read-only retrieval verb on the bench an agent enters
 * precisely to consult history is squarely what that budget is for.
 *
 * 46 -> 50 (2026-11-20): the five GD&T correction verbs (gdt_fcf_update,
 * gdt_fcf_delete, gdt_datum_update, gdt_datum_delete, gdt_dimensional_delete)
 * joined the drawing bench, which already measured 31 + 14 = 45 after the
 * drawing_set_* / drawing_dimensions / iso286_limits additions; it now
 * measures 31 + 19 = 50 and is the sole max. Raised by exactly the overshoot,
 * same policy: an agent enters the drawing bench to author and correct
 * callouts, and fixing a wrong frame in place is what that bench is for.
 *
 * 50 -> 51 (2026-11-20): drawing_spaces joined the drawing bench (31 + 20 =
 * 51, the sole max). Raised by exactly the overshoot, same policy: an agent
 * enters the drawing bench to make a sheet right, and seeing the sheet as its
 * scored spaces is what it needs to do that.
 *
 * 51 -> 52 (2026-11-20): drawing_look joined the drawing bench (31 + 21 =
 * 52, the sole max). Raised by exactly the overshoot, same policy: seeing the
 * sheet, or one space of it, as a picture is the other half of the agent's
 * eyes on a drawing.
 *
 * 52 -> 58 (2026-11-20): the agent's hands joined the drawing bench --
 * drawing_move_dimension, drawing_align, drawing_move_view,
 * drawing_reattach_leader, drawing_remove_view, drawing_remove_dimension
 * (31 + 27 = 58, the sole max). Raised by exactly the overshoot, same policy:
 * the eyes find the worst space and these are the verbs that fix it; a bench
 * that can see a defect and not touch it is half a bench.
 */
export const LIVE_SURFACE_CEILING = 58;

/** Callback that flips real server tool handles on a bench switch. */
export type ApplyTransition = (toEnable: string[], toDisable: string[]) => void;

export interface WorkbenchStatus {
  active_bench: string; // "core_only" | a bench name | "full"
  exposed_count: number;
  token_bill: number;
  benches: Record<string, number>; // bench name → registered tool count
  surface_mode: SurfaceMode;
  ceiling: number;
}

export interface WorkbenchResult extends WorkbenchStatus {
  switched: boolean;
  exposed_bench_tools: string[];
  notice: string;
}

/** Most characters of a tool's purpose the catalog shows (the map stays ~4k tokens). */
const CATALOG_GIST_MAX = 90;

/** What each bench is FOR, in one line -- the catalog's headings. */
export const BENCH_PURPOSE: Record<Bench, string> = {
  core: "Make, combine, change, inspect and certify parts; always live.",
  sketch: "Constrained 2D sketches (psketch_*), solved and certified, then extruded or revolved.",
  assembly: "Place part instances, mate them, solve and count freedoms, check interference.",
  drawing: "Manufacturing drawings: views, dimensions, GD&T, the sheet's layout and its review.",
  analysis: "Ask the geometry: measure, probe, find features, check manufacturability, look up cited engineering data.",
  simulate: "Strength studies (FEA), walked through with the person: mesh -> supports, loads, material -> run -> verdict.",
  labels: "Names on faces and edges, the shared notebook, and questions to the person.",
  timeline: "History: checkpoints, undo/redo, branches, merge, scrub, certified recipes.",
  meta: "Find, describe and run any tool: find_tool, describe_tool, invoke.",
};

export interface WorkbenchCatalog {
  active_bench: string;
  tool_count: number;
  benches: Array<{
    bench: Bench;
    for: string;
    tool_count: number;
    tools: Array<{ name: string; does: string; live?: boolean }>;
  }>;
  how: string;
}

export class Workbench {
  /** null = core_only (no bench active); a Bench = that bench is active. */
  private active: Bench | null = null;
  private apply: ApplyTransition = () => {};

  constructor(
    private readonly table: ToolTable,
    private readonly minimalSurface: string[],
    private readonly mode: SurfaceMode,
  ) {}

  /** index.ts injects the real server enable/disable applier after mounting. */
  setApply(fn: ApplyTransition): void {
    this.apply = fn;
  }

  /** The tool names of one bench that are actually present in the table, sorted. */
  benchToolNames(bench: Bench): string[] {
    return this.table
      .names()
      .filter((n) => metaFor(n).bench === bench)
      .sort();
  }

  /** Every switchable-bench tool name (the set index mounts disabled at start). */
  allSwitchableBenchTools(): string[] {
    const out: string[] = [];
    for (const b of SWITCHABLE_BENCHES) out.push(...this.benchToolNames(b));
    return out;
  }

  /** The minimal (core+meta) names that exist in the table. */
  private minimalPresent(): string[] {
    return this.minimalSurface.filter((n) => this.table.has(n));
  }

  /**
   * The names currently exposed as live MCP tools for the active state.
   * Deduped (2026-08-10): a tool can be BOTH core-resident and bench-
   * classified (blackboard_add_entry, timeline_checkpoint, and now the whole
   * psketch_* family — 10 of 'sketch's 15 tools) since index.ts mounts a
   * core∩bench member once and never disables it on a bench switch. Returning
   * it twice here would double-count it in `status().exposed_count` AND in
   * `billFor`'s token sum (billFor sums this array, so a duplicate name pays
   * its token cost twice) — an over-report, not a real second tool.
   */
  exposedNames(): string[] {
    if (this.mode === "full") {
      return this.table.names().filter((n) => metaFor(n).bench !== "meta");
    }
    const core = this.minimalPresent();
    const bench = this.active ? this.benchToolNames(this.active) : [];
    return [...new Set([...core, ...bench])];
  }

  private benchCounts(): Record<string, number> {
    const counts: Record<string, number> = {};
    for (const n of this.table.names()) {
      const b = metaFor(n).bench;
      counts[b] = (counts[b] ?? 0) + 1;
    }
    return counts;
  }

  private billFor(names: string[]): number {
    let sum = 0;
    for (const n of names) {
      const e = this.table.get(n);
      if (e) sum += estimateTokens(e);
    }
    return sum;
  }

  status(): WorkbenchStatus {
    const exposed = this.exposedNames();
    return {
      active_bench:
        this.mode === "full"
          ? "full"
          : this.active
            ? this.active
            : "core_only",
      exposed_count: exposed.length,
      token_bill: this.billFor(exposed),
      benches: this.benchCounts(),
      surface_mode: this.mode,
      ceiling: LIVE_SURFACE_CEILING,
    };
  }

  /**
   * Enter a bench (or report status / drop to core_only). Returns the payload
   * the `workbench` tool surfaces. In `full` surface mode switching is a no-op
   * with an honest notice; in `minimal` mode it flips the live toolset and
   * always carries the client-capability caveat (spec honesty §3.5).
   */
  /**
   * The MAP of the whole surface: every bench, what it is for, and each of
   * its tools with the gist of what it does -- so an agent can see what
   * exists instead of guessing search words. Changes nothing.
   */
  catalog(): WorkbenchCatalog {
    const order: Bench[] = ["core", ...SWITCHABLE_BENCHES, "meta"];
    const live = new Set(this.exposedNames());
    const benches = order.map((b) => {
      const tools = this.benchToolNames(b).map((name) => {
        const e = this.table.get(name);
        return {
          name,
          does: gist(e?.description ?? "", CATALOG_GIST_MAX),
          ...(live.has(name) || b === "meta" ? { live: true } : {}),
        };
      });
      return { bench: b, for: BENCH_PURPOSE[b], tool_count: tools.length, tools };
    });
    return {
      active_bench: this.active ?? "core_only",
      tool_count: this.table.size,
      benches,
      how: "Tools marked live are callable directly. Any other tool: workbench('<bench>') makes its bench live, or invoke({name, args}) runs it now (describe_tool({name}) shows its arguments). find_tool searches by intent.",
    };
  }

  enter(mode: WorkbenchMode): WorkbenchResult | WorkbenchCatalog {
    if (mode === "catalog") return this.catalog();
    if (mode === "status") {
      return {
        ...this.status(),
        switched: false,
        exposed_bench_tools: this.active
          ? this.benchToolNames(this.active)
          : [],
        notice:
          this.mode === "full"
            ? "FULL surface mode: every tool is exposed directly; benches are inactive."
            : this.active
              ? `Active bench: '${this.active}'. find_tool/describe_tool/invoke reach every tool regardless of the active bench.`
              : "No bench active (core+meta only). Enter a bench to expose its tools, or reach any tool via find_tool/invoke.",
      };
    }

    if (this.mode === "full") {
      // Full exposure already shows everything; switching cannot add or remove.
      const st = this.status();
      return {
        ...st,
        switched: false,
        exposed_bench_tools: [],
        notice:
          `Surface is in FULL mode (ROSHERA_MCP_SURFACE=full): all ${st.exposed_count} ` +
          `tools are already exposed directly, so workbench('${mode}') is a no-op. ` +
          `Unset ROSHERA_MCP_SURFACE (default minimal) to use benches as an attention optimization.`,
      };
    }

    // ── minimal mode: perform the switch ────────────────────────────────────
    const target: Bench | null = mode === "core_only" ? null : (mode as Bench);
    const oldBench = this.active;
    const oldTools = oldBench ? this.benchToolNames(oldBench) : [];
    const newTools = target ? this.benchToolNames(target) : [];
    const oldSet = new Set(oldTools);
    const newSet = new Set(newTools);
    const toDisable = oldTools.filter((n) => !newSet.has(n));
    const toEnable = newTools.filter((n) => !oldSet.has(n));

    this.apply(toEnable, toDisable);
    this.active = target;

    const st = this.status();
    const caveat =
      "A tools/list_changed notification was emitted, but toolset switching may not " +
      "refresh on every client — find_tool / describe_tool / invoke reach every tool " +
      "regardless of the active bench (benches optimize attention, never capability).";
    const notice =
      target === null
        ? `Retired ${oldBench ? `the '${oldBench}' bench` : "any active bench"}: ` +
          `core+meta only (${st.exposed_count} tools live). ${caveat}`
        : `Entered '${target}' bench: ${newTools.length} bench tools now exposed on top of ` +
          `core+meta (${st.exposed_count} tools live${
            oldBench ? `, retired the '${oldBench}' bench` : ""
          }). ${caveat}`;

    return {
      ...st,
      switched: toEnable.length > 0 || toDisable.length > 0,
      exposed_bench_tools: newTools,
      notice,
    };
  }
}

// ─── The `workbench` core tool ───────────────────────────────────────────────

/**
 * Register the `workbench` tool into the table, closing over the session
 * controller. Always in the core surface (minimal AND full) so a bench is one
 * predictable call away, and `workbench("status")` always answers.
 */
export function registerWorkbenchTool(host: ToolHost, wb: Workbench): void {
  host.tool(
    "workbench",
    "The MAP and the benches. 'catalog' lists every tool by bench with what it does -- start here. Entering a bench makes its tools live beside core+meta and retires the previous one (attention only: invoke runs any tool in any mode). Benches: sketch, assembly, drawing (dimensions/GD&T), analysis (measure/probe/DFM/knowledge), simulate (strength studies), labels (labels/notebook/questions), timeline (history/branches). 'core_only' retires the bench; 'status' = live count + token bill.",
    {
      mode: z
        .enum([
          "catalog",
          "sketch",
          "assembly",
          "drawing",
          "analysis",
          "simulate",
          "labels",
          "timeline",
          "core_only",
          "status",
        ])
        .describe("'catalog' for the map, a bench to enter, 'core_only' to retire the bench, or 'status'"),
    },
    async ({ mode }) => ok(wb.enter(mode as WorkbenchMode)),
  );
}
