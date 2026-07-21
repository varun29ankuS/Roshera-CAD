#!/usr/bin/env node
/**
 * Roshera MCP server — the agent-facing tool surface over the Roshera
 * geometry kernel's REST API.
 *
 * Design doctrine (from the 2026-06-12 live sessions):
 *  - LATENCY: batch/composite tools collapse N round trips into one
 *    (create_cylinder = sketch+points+extrude; drill_pattern = N bores +
 *    N differences in a single call).
 *  - PERCEPTION: every mutating op carries an ambient one-line verdict;
 *    render/section/occupancy tools return images the agent SEES.
 *  - SHARED ATTENTION: get_pointer reads what the human is pointing at.
 *  - PLACEMENT IS EXPLICIT: create tools take coordinates and echo world
 *    placement.
 *
 * SCALE (Slice 2, spec 2026-07-20-mcp-scale-architecture-design.md):
 *  Every tool registers its {name, schema, handler} into ONE internal table
 *  (registry.ts). The DEFAULT exposed MCP surface is minimal-complete — the
 *  ~15 core modeling/perception tools + the 3 meta-tools — so the worst-case
 *  client (no list_changed, injects the whole surface every turn) pays a small
 *  fixed bill, while the entire long tail stays reachable via find_tool /
 *  describe_tool / invoke. ROSHERA_MCP_SURFACE=full restores the full 90-tool
 *  exposure (transition escape hatch).
 *
 * Server URL via ROSHERA_URL (default http://localhost:8081).
 */

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { BASE, AUTH_HEADERS } from "./core.js";
import { RegisteredTool, canonicalJson, fnv1a64hex } from "./registry.js";
import { setRegistryWarning } from "./metatools.js";
import {
  buildTableWithControls,
  resolveSurfaceMode,
  exposedNamesFor,
  billFor,
  MINIMAL_SURFACE,
  META_SURFACE,
} from "./surface.js";

// The Roshera mark (roshera-app/public/favicon.svg, inlined as a data URI so the
// server stays self-contained). MCP clients that render server icons (per the MCP
// `icons` spec, supported by SDK >= ~1.18) show this next to "Calling roshera";
// clients that don't yet render server icons simply show the name as text.
const ROSHERA_ICON =
  "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSI0OCIgaGVpZ2h0PSI0OCIgdmlld0JveD0iMCAwIDQ4IDQ4Ij4NCiAgPGNpcmNsZSBjeD0iMjQiIGN5PSIyNCIgcj0iMjMiIGZpbGw9IiNGNURGQTAiIHN0cm9rZT0iI0Q0NjQ1QyIgc3Ryb2tlLXdpZHRoPSIyLjUiLz4NCiAgPHBhdGggZD0iTTE2IDM2VjEyaDhjNC40MTggMCA4IDMuMTM0IDggN3MtMy41ODIgNy04IDdoLTgiIGZpbGw9Im5vbmUiIHN0cm9rZT0iI0M0NTI0QSIgc3Ryb2tlLXdpZHRoPSIwIi8+DQogIDxwYXRoIGQ9Ik0xNiAxMiBMMjggMTIgUTMyIDEyIDMyIDE5IFEzMiAyNiAyOCAyNiBMMTYgMzYgWiIgZmlsbD0iI0M0NTI0QSIvPg0KICA8cGF0aCBkPSJNMjggMTIgUTMyIDEyIDMyIDE5IFEzMiAyNiAyOCAyNiBaIiBmaWxsPSIjRDQ2NDVDIi8+DQo8L3N2Zz4NCg==";

const server = new McpServer({
  // Display name Claude Code shows in "Calling …". The 🅡 glyph (negative
  // circled R) renders the Roshera mark inline in the CLI status line.
  name: "🅡 ROSHERA",
  version: "0.1.0",
  icons: [{ src: ROSHERA_ICON, mimeType: "image/svg+xml", sizes: ["48x48"] }],
});

// ── 1. Register EVERY tool + composition + meta-tools into the single table ──
// (Assembly + surface policy live in surface.ts, shared with the test suite.)
// The Workbench controller (S3) rides along, owning the bench-switch state.
const { table, workbench } = buildTableWithControls();

// ── 2. Mount the active surface onto the live server ─────────────────────────
// Default = minimal-complete (core verbs + workbench/cad_program + 3 meta-tools);
// everything else is in the table, reachable via invoke. ROSHERA_MCP_SURFACE=full
// exposes all directly (transition escape hatch).
/**
 * Mount one table entry as a live MCP tool, preserving its exact schema. Returns
 * the SDK's registered-tool handle so a bench switch can enable()/disable() it
 * (each flip emits `tools/list_changed`). `enabled:false` mounts a bench tool
 * present-but-hidden until its bench is entered.
 */
function mount(entry: RegisteredTool, enabled = true) {
  const handle = server.registerTool(
    entry.name,
    { description: entry.description, inputSchema: entry.schema as any },
    entry.handler,
  );
  if (!enabled) handle.disable();
  return handle;
}

const mode = resolveSurfaceMode();

// The names actually mounted as live MCP tools (for the bill + the sanity guard).
const exposedNames: string[] = [];

if (mode === "full") {
  // Everything exposed directly; benches are inactive (workbench → no-op notice).
  for (const name of exposedNamesFor(table, "full")) {
    const entry = table.get(name);
    if (entry) {
      mount(entry, true);
      exposedNames.push(name);
    }
  }
} else {
  // Minimal: core+meta enabled; every switchable-bench tool mounted DISABLED so
  // a workbench switch can reveal/retire it via the SDK's list_changed path.
  for (const name of exposedNamesFor(table, "minimal")) {
    const entry = table.get(name);
    if (entry) {
      mount(entry, true);
      exposedNames.push(name);
    }
  }
  const benchHandles = new Map<string, ReturnType<typeof mount>>();
  for (const name of workbench.allSwitchableBenchTools()) {
    // A bench tool that is ALSO in the always-on core surface (e.g.
    // blackboard_add_entry) is already mounted enabled above; mounting it
    // again would throw. Leave it out of benchHandles so bench switches
    // never disable it.
    if (exposedNames.includes(name)) continue;
    const entry = table.get(name);
    if (entry) benchHandles.set(name, mount(entry, false));
  }
  // Wire the controller's transition to the real server handles. enable()/
  // disable() each emit tools/list_changed; capable clients refresh, others
  // keep the long tail via invoke (workbench's returned notice says so).
  workbench.setApply((toEnable, toDisable) => {
    for (const n of toDisable) benchHandles.get(n)?.disable();
    for (const n of toEnable) benchHandles.get(n)?.enable();
  });
}

// A minimal-surface sanity guard: every intended tool must have been present in
// the table. A typo in CORE_SURFACE would silently shrink the surface — refuse.
if (mode === "minimal") {
  const missing = MINIMAL_SURFACE.filter((n) => !table.has(n));
  if (missing.length) {
    console.error(
      `roshera-mcp FATAL: minimal surface names not found in the table: ${missing.join(", ")}`,
    );
    process.exit(1);
  }
}

// ── 3. Token bill (the MEASURE) ──────────────────────────────────────────────
const minimalBill = billFor(table, MINIMAL_SURFACE);
const fullBill = billFor(table, table.names());
const exposedBill = billFor(table, exposedNames);

// ── 4. Consume the kernel-served registry (Layer 0), honest fallback ─────────
//
// Try GET /api/agent/tool-registry for purpose/bench/token metadata + the drift
// hash. On FETCH FAILURE (the current live situation — the running :8081 backend
// predates the endpoint and 404s) use the compiled-in local table silently but
// logged. On SUCCESS verify the hash and the tool-name inventory; on MISMATCH
// serve local and warn once per session in meta-tool output (spec §3.4).
async function consumeRegistry(): Promise<void> {
  const REGISTRY_TIMEOUT_MS = (() => {
    const n = Number(process.env.ROSHERA_MCP_REGISTRY_TIMEOUT_MS);
    return Number.isFinite(n) && n > 0 ? n : 3000;
  })();
  let payload: any;
  try {
    const res = await fetch(`${BASE}/api/agent/tool-registry`, {
      headers: { "X-Roshera-Agent": "Claude", ...AUTH_HEADERS },
      signal: AbortSignal.timeout(REGISTRY_TIMEOUT_MS),
    });
    if (!res.ok) {
      console.error(
        `roshera-mcp: tool-registry endpoint returned ${res.status} — serving compiled local table (${table.size} tools). ` +
          `(Expected until the backend is rebuilt past registry Slice 1.)`,
      );
      return;
    }
    payload = await res.json();
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    console.error(
      `roshera-mcp: tool-registry endpoint unavailable (${msg}) — serving compiled local table (${table.size} tools).`,
    );
    return;
  }

  const served: any[] = Array.isArray(payload?.tools) ? payload.tools : [];
  const servedHash: string | undefined = payload?.registry_hash;

  // (a) Algorithm/canonicalization parity: reproduce the backend's own hash over
  //     its own served tools. Equality proves TS↔Rust FNV-1a-64 + canonical-JSON
  //     agreement live; inequality means an encoder skew worth surfacing.
  if (servedHash) {
    const reHash = fnv1a64hex(canonicalJson(served));
    if (reHash === servedHash) {
      console.error(
        `roshera-mcp: tool-registry hash verified (${servedHash}) — TS↔Rust FNV-1a-64 parity confirmed over ${served.length} tools.`,
      );
    } else {
      console.error(
        `roshera-mcp: tool-registry hash SKEW — backend=${servedHash} ts-recompute=${reHash}. ` +
          `Serving local table; canonicalization differs (investigate, do not trust cross-language hashes yet).`,
      );
      setRegistryWarning(
        `registry hash could not be reproduced (backend=${servedHash}, local recompute=${reHash}); serving the compiled local table.`,
      );
    }
  }

  // (b) Inventory drift: does the live kernel expose a different tool set than
  //     the compiled table? A real mismatch is loud (serve local + warn once).
  const servedNames = new Set(
    served.map((t) => t?.name).filter((n): n is string => typeof n === "string"),
  );
  const localNames = new Set(table.names());
  // Meta-tools are MCP-only; they never appear in the kernel registry.
  for (const m of META_SURFACE) localNames.delete(m);
  const onlyBackend = [...servedNames].filter((n) => !localNames.has(n));
  const onlyLocal = [...localNames].filter((n) => !servedNames.has(n));
  if (onlyBackend.length || onlyLocal.length) {
    const parts: string[] = [];
    if (onlyBackend.length)
      parts.push(`in kernel but not compiled: ${onlyBackend.join(", ")}`);
    if (onlyLocal.length)
      parts.push(`compiled but not in kernel: ${onlyLocal.join(", ")}`);
    const warn = `tool inventory drift — ${parts.join("; ")}. Serving the compiled local table; run a rebuild to reconcile.`;
    console.error(`roshera-mcp: ${warn}`);
    setRegistryWarning(warn);
  } else if (servedNames.size) {
    console.error(
      `roshera-mcp: tool inventory matches the kernel registry (${servedNames.size} tools).`,
    );
  }
}

// ── 5. Report the active mode + surface + token bill to stderr ───────────────
const tail =
  mode === "minimal"
    ? "Long tail reachable via find_tool/describe_tool/invoke."
    : "Full exposure (transition escape hatch); the funnel meta-tools are omitted.";
console.error(
  `roshera-mcp surface: ${mode.toUpperCase()} — exposing ${exposedNames.length}/${table.size} tools ` +
    `(~${exposedBill} tokens). Bills: minimal=${minimalBill}, full=${fullBill}. ${tail}`,
);

const transport = new StdioServerTransport();
await server.connect(transport);
// Registry consumption is best-effort and must not gate startup; run it after
// connect so the surface is live immediately even if the backend is slow/down.
void consumeRegistry();
console.error(`roshera-mcp connected (API: ${BASE})`);
