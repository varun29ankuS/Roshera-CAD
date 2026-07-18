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
 * Structure: shared plumbing in core.ts; tools grouped by domain under
 * tools/ (perception, inspect, queries, create, modify, psketch, io,
 * timeline, blackboard, labels, assembly).
 *
 * Server URL via ROSHERA_URL (default http://localhost:8081).
 */

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { BASE } from "./core.js";
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

registerPerceptionTools(server);
registerInspectTools(server);
registerQueryTools(server);
registerCreateTools(server);
registerModifyTools(server);
registerPsketchTools(server);
registerIoTools(server);
registerTimelineTools(server);
registerBlackboardTools(server);
registerLabelTools(server);
registerAssemblyTools(server);
registerGdtTools(server);
registerDrawingTools(server);

const transport = new StdioServerTransport();
await server.connect(transport);
console.error(`roshera-mcp connected (API: ${BASE})`);
