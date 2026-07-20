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
 * (status shape, exposure set, ≤35 ceiling) without starting a server.
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

/** The user-facing `workbench(mode)` argument. */
export type WorkbenchMode =
  | "sketch"
  | "assembly"
  | "drawing"
  | "analysis"
  | "labels"
  | "core_only"
  | "status";

/** The benches a `workbench` call can switch between (core/meta are always on). */
export const SWITCHABLE_BENCHES: Bench[] = [
  "sketch",
  "assembly",
  "drawing",
  "analysis",
  "labels",
];

/** The soft ceiling on the live (exposed) surface in any bench (spec §Layer 1). */
export const LIVE_SURFACE_CEILING = 35;

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

  /** The names currently exposed as live MCP tools for the active state. */
  exposedNames(): string[] {
    if (this.mode === "full") {
      return this.table.names().filter((n) => metaFor(n).bench !== "meta");
    }
    const core = this.minimalPresent();
    const bench = this.active ? this.benchToolNames(this.active) : [];
    return [...core, ...bench];
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
  enter(mode: WorkbenchMode): WorkbenchResult {
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
    "Switch the LIVE toolset to a CAD workbench (attention optimization, not a " +
      "capability gate — every tool stays reachable via find_tool/describe_tool/" +
      "invoke in any mode). Entering a bench exposes its tools on top of the " +
      "core+meta surface and retires the previous bench; the live surface stays " +
      "small (≤35). modes: 'sketch' (create_sketch/psketch_*), 'assembly' " +
      "(assembly_*), 'drawing' (make_drawing/dimension/gdt_*), 'analysis' " +
      "(ray/region/occupancy/coverage/distance/ground_truth), 'labels' (label_*/" +
      "blackboard_*), 'core_only' (retire any bench), 'status' (report the active " +
      "bench, exposed count, token bill, and per-bench tool counts).",
    {
      mode: z
        .enum([
          "sketch",
          "assembly",
          "drawing",
          "analysis",
          "labels",
          "core_only",
          "status",
        ])
        .describe("bench to enter, 'core_only' to retire the bench, or 'status'"),
    },
    async ({ mode }) => ok(wb.enter(mode as WorkbenchMode)),
  );
}
