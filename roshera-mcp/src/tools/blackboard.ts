/**
 * BLACKBOARD — agent/human shared notebook of editable, event-logged lines.
 * Backend-persisted (GET/POST/PATCH/DELETE /api/blackboard*); a line added
 * here shows up live in the frontend Blackboard panel.
 */

import type { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { z } from "zod";
import { api, ok, fail } from "../core.js";

/** Wire shape of one Blackboard line (mirrors the frontend store). */
interface BlackboardLine {
  id: string;
  text: string;
  author: "user" | "agent";
  createdAt: number;
  updatedAt: number;
}

/**
 * Build the `?scope=…` / `?part_id=…` query suffix for the scoped Blackboard
 * routes. A `part_id` (a part UUID from list_parts, or its integer kernel id)
 * targets THAT part's own notebook; `scope` lets the agent address an
 * assembly (`assembly:<uuid>`) or the document (`document`). Omitting both
 * targets the document-wide notebook.
 */
function blackboardScopeQuery(part_id?: string, scope?: string): string {
  const p = new URLSearchParams();
  if (scope) p.set("scope", scope);
  else if (part_id) p.set("part_id", part_id);
  const s = p.toString();
  return s ? `?${s}` : "";
}

const SCOPE_ARGS = {
  part_id: z
    .string()
    .optional()
    .describe(
      "target THIS part's notebook — a part UUID or integer kernel id. " +
        "Omit for the document-wide notebook.",
    ),
  scope: z
    .string()
    .optional()
    .describe("'document', 'part:<uuid>', or 'assembly:<uuid>'. Wins over part_id."),
};

export function registerBlackboardTools(server: McpServer) {
  server.tool(
    "blackboard_add_entry",
    "WRITE a line to a Blackboard notebook the human SEES live in the app " +
      "(markdown + $math$). Each part has its own notebook (`part_id`); omit " +
      "for document-wide, `scope` for assembly-level. Returns the line id.",
    {
      text: z.string().describe("markdown + $math$ source for the line"),
      author: z.enum(["agent", "user"]).default("agent"),
      ...SCOPE_ARGS,
    },
    async ({ text, author, part_id, scope }) => {
      try {
        const line = (await api("POST", "/api/blackboard/entries", {
          text,
          author,
          ...(part_id ? { part_id } : {}),
          ...(scope ? { scope } : {}),
        })) as BlackboardLine;
        return ok({ id: line.id, author: line.author, text: line.text });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "blackboard_edit_entry",
    "EDIT a Blackboard line in place by id (from blackboard_list); the change " +
      "appears live. Pass the same `part_id`/`scope` it was listed under.",
    {
      id: z.string().describe("line id from blackboard_list"),
      text: z.string().describe("new markdown + $math$ source"),
      ...SCOPE_ARGS,
    },
    async ({ id, text, part_id, scope }) => {
      try {
        const line = (await api(
          "PATCH",
          `/api/blackboard/entries/${encodeURIComponent(id)}${blackboardScopeQuery(part_id, scope)}`,
          { text },
        )) as BlackboardLine;
        return ok({ id: line.id, author: line.author, text: line.text });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "blackboard_list",
    "READ a Blackboard notebook: current lines (id, author, text) in document " +
      "order. `part_id` for that part's notebook; omit for document-wide; " +
      "`scope` for an assembly.",
    { ...SCOPE_ARGS },
    async ({ part_id, scope }) => {
      try {
        const snap = (await api(
          "GET",
          `/api/blackboard${blackboardScopeQuery(part_id, scope)}`,
        )) as {
          lines?: BlackboardLine[];
        };
        const lines = (snap.lines ?? []).map((l) => ({
          id: l.id,
          author: l.author,
          text: l.text,
        }));
        return ok({ count: lines.length, lines });
      } catch (e) {
        return fail(e);
      }
    },
  );

  server.tool(
    "blackboard_clear",
    "CLEAR one Blackboard notebook — every line + its event log. Scoped: " +
      "`part_id` clears only that part's notebook. Destructive; does not " +
      "touch geometry.",
    { ...SCOPE_ARGS },
    async ({ part_id, scope }) => {
      try {
        return ok(
          await api(
            "POST",
            `/api/blackboard/clear${blackboardScopeQuery(part_id, scope)}`,
            {},
          ),
        );
      } catch (e) {
        return fail(e);
      }
    },
  );
}
