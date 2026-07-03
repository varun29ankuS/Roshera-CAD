/** Timeline tools — event-sourced history: scrub, clear. */

import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import { randomUUID } from "node:crypto";
import { api, ok, fail } from "../core.js";

export function registerTimelineTools(server: McpServer) {
  server.tool(
    "timeline_scrub",
    "Look at the scene AS OF a past event — non-destructive (live scene " +
      "untouched). Returns object count + mesh stats at that moment.",
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
    "Reset a timeline branch to ZERO events and wipe the live model to match " +
      "— DESTRUCTIVE and irreversible (the event ledger is rewritten, not " +
      "undoable). Use clear_parts instead for an empty scene with preserved " +
      "history.",
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
}
